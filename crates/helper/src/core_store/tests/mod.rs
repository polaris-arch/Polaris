use super::*;
use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};

pub(super) fn fixture(version: &str) -> (tempfile::TempDir, InstalledCoreManifest) {
    let source = tempfile::tempdir().unwrap();
    private_mode(source.path(), 0o700).unwrap();
    let files: Vec<_> = [
        ("sing-box", format!("core-{version}").into_bytes()),
        ("libcronet.so", b"sidecar".to_vec()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        let path = source.path().join(name);
        fs::write(&path, &raw).unwrap();
        private_mode(&path, 0o600).unwrap();
        CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(&raw),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "linux".into(),
        version: version.into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: Some("b".repeat(64)),
        cleaner_sha256: None,
    };
    let id = core
        .id(crate::core_hash::sha256_hex)
        .unwrap()
        .as_str()
        .to_owned();
    (
        source,
        InstalledCoreManifest {
            schema: "polaris-core-manifest-v1".into(),
            id,
            app_version: "1.0.0".into(),
            core,
        },
    )
}

pub(super) fn store() -> (tempfile::TempDir, CoreStore, u32) {
    let uid = nix::unistd::Uid::effective().as_raw();
    let root = if uid == 0 {
        #[cfg(target_os = "linux")]
        let base = Path::new("/root");
        #[cfg(target_os = "macos")]
        let base = Path::new("/private/var/root");
        tempfile::Builder::new()
            .prefix("core-store-test-")
            .tempdir_in(base)
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    private_mode(root.path(), 0o700).unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let store = CoreStore::initialize(&canonical, OwnerPolicy { uid: Some(uid) }).unwrap();
    (root, store, uid)
}

#[test]
fn runtime_open_is_read_only_and_does_not_invent_an_installation() {
    let root = tempfile::tempdir().unwrap();
    private_mode(root.path(), 0o700).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn mismatched_source_never_becomes_an_eligible_version() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::write(source.path().join("libcronet.so"), b"changed").unwrap();
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    assert!(!root
        .path()
        .join("manifests")
        .join(format!("{}.json", manifest.id))
        .exists());
    assert!(!root.path().join("versions").join(&manifest.id).exists());
}

#[test]
fn same_id_is_immutable_and_every_start_rechecks_contents() {
    // Local File drop cannot exclude a descriptor inherited by another test's
    // concurrent spawn. Create this fixture only after the exact child execs.
    match exact_fixture_child_context(SAME_ID_EXACT_TEST)
        .expect("valid exact same-id child protocol")
    {
        Some(context) => {
            emit_exact_fixture_record(&context, "entry");
            same_id_fixture_body();
            emit_exact_fixture_record(&context, "complete");
        }
        None => {
            run_exact_fixture_child(SAME_ID_EXACT_TEST, None, std::time::Duration::from_secs(30))
                .expect("isolated same-id fixture completes once and is reaped")
        }
    }
}

fn same_id_fixture_body() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::AlreadyPresent
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    let verified = store.verify_objects(&id, true).unwrap();
    assert_eq!(verified.manifest.id, manifest.id);
    fs::write(
        root.path()
            .join("versions")
            .join(&manifest.id)
            .join("libcronet.so"),
        b"changed",
    )
    .unwrap();
    assert!(store.verify_objects(&id, true).is_err());
    // A damaged active version cannot be replaced beneath a held lazy-sidecar lease.
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    drop(verified);
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert!(store.verify_objects(&id, true).is_ok());
}

#[test]
fn inherited_version_lease_blocks_repair_until_last_child_reference_closes() {
    match exact_fixture_child_context(INHERITED_VERSION_LEASE_EXACT_TEST)
        .expect("valid exact inherited-version-lease child protocol")
    {
        Some(context) => {
            emit_exact_fixture_record(&context, "entry");
            inherited_version_lease_fixture_body();
            emit_exact_fixture_record(&context, "complete");
        }
        None => run_exact_fixture_child(
            INHERITED_VERSION_LEASE_EXACT_TEST,
            None,
            std::time::Duration::from_secs(30),
        )
        .expect("isolated inherited-version-lease fixture completes once and is reaped"),
    }
}

#[allow(
    unsafe_code,
    reason = "isolated inherited version lease control; child only uses async-signal-safe syscalls"
)]
fn inherited_version_lease_fixture_body() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    let verified = store.verify_objects(&id, true).unwrap();
    let observation = LockObservation::from_file(verified._lease.as_ref().unwrap());
    let lease_fd = verified._lease.as_ref().unwrap().as_raw_fd();
    let device = observation.dev.unwrap();
    let inode = observation.inode.unwrap();
    lock_diagnostics::record("version-shared-acquired", &observation);
    fs::write(
        root.path()
            .join("versions")
            .join(&manifest.id)
            .join("libcronet.so"),
        b"changed",
    )
    .unwrap();

    let (ready_read, ready_write) = nix::unistd::pipe().unwrap();
    let (release_read, release_write) = nix::unistd::pipe().unwrap();
    let ready_read_fd = ready_read.as_raw_fd();
    let ready_write_fd = ready_write.as_raw_fd();
    let release_read_fd = release_read.as_raw_fd();
    let release_write_fd = release_write.as_raw_fd();
    // SAFETY: libc::stat contains only numeric fields; initialize its child-side
    // output buffer before fork, without acquiring any descriptor or lock.
    let mut inherited_stat: nix::libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: this exact exec fixture has no other unit-test fixture. The child
    // uses only preallocated scalars/buffers and the syscalls below, then _exit;
    // it never allocates, formats, locks or invokes Rust Drop.
    let pid = unsafe { nix::libc::fork() };
    assert!(pid >= 0, "controlled version-lease fork failed");
    if pid == 0 {
        // SAFETY: every fd was opened before fork and remains live here. The
        // inherited lease is never unlocked; _exit closes its last reference.
        unsafe {
            nix::libc::alarm(10);
            if nix::libc::close(ready_read_fd) != 0 || nix::libc::close(release_write_fd) != 0 {
                nix::libc::_exit(90);
            }
            let flags = nix::libc::fcntl(lease_fd, nix::libc::F_GETFD);
            if flags < 0
                || flags & nix::libc::FD_CLOEXEC == 0
                || nix::libc::fstat(lease_fd, &mut inherited_stat) != 0
                || inherited_stat.st_dev as u64 != device
                || inherited_stat.st_ino as u64 != inode
            {
                nix::libc::_exit(91);
            }
            let ready = [1u8];
            if nix::libc::write(ready_write_fd, ready.as_ptr().cast(), 1) != 1 {
                nix::libc::_exit(92);
            }
            let mut release = [0u8];
            if nix::libc::read(release_read_fd, release.as_mut_ptr().cast(), 1) != 1
                || release != [1]
            {
                nix::libc::_exit(93);
            }
            nix::libc::_exit(0);
        }
    }
    let mut cleanup = lock_diagnostics::ChildCleanup(Some(pid));
    drop(ready_write);
    drop(release_read);
    let mut ready = File::from(ready_read);
    let mut release = File::from(release_write);
    let mut byte = [0u8];
    ready.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [1], "child validated its exact inherited lease fd");
    drop(verified);
    lock_diagnostics::record("version-parent-file-closed", &observation);

    // One real repair attempt while the known child remains behind the pipe.
    // Accept only native RepairExclusive contention, never an unrelated error.
    let error = store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    let failure = error
        .get_ref()
        .unwrap()
        .downcast_ref::<LockFailure>()
        .unwrap();
    assert!(matches!(failure.role, LockRole::RepairExclusive));
    assert!(matches!(&failure.cause, TryLockError::WouldBlock));
    assert_eq!(failure.observation.dev, Some(device));
    assert_eq!(failure.observation.inode, Some(inode));
    let snapshot = failure.holder_snapshot.as_ref().unwrap();
    let birth = snapshot
        .child_descriptor_birth(pid as u32, lease_fd)
        .expect("same-inode inherited child descriptor and stable native birth");
    assert!(birth[0] > 0);
    writeln!(
        io::stderr().lock(),
        "VERSION_LEASE_CONTROL phase=parent-dropped child_pid={pid} child_birth={birth:?} inherited_fd={lease_fd} dev={device} inode={inode} {error}"
    )
    .unwrap();

    release.write_all(&[1]).unwrap();
    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).unwrap();
    cleanup.0 = None; // Reaped before checking status; never signal a reused PID.
    assert!(matches!(status, nix::sys::wait::WaitStatus::Exited(_, 0)));
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert_eq!(
        store.verify_objects(&id, true).unwrap().manifest.id,
        manifest.id
    );
    writeln!(
        io::stderr().lock(),
        "VERSION_LEASE_CONTROL phase=child-reaped-repair-installed child_pid={pid} child_birth={birth:?} dev={device} inode={inode}"
    )
    .unwrap();
}

const SAME_ID_EXACT_TEST: &str =
    "core_store::tests::same_id_is_immutable_and_every_start_rechecks_contents";
const INHERITED_VERSION_LEASE_EXACT_TEST: &str =
    "core_store::tests::inherited_version_lease_blocks_repair_until_last_child_reference_closes";
const BUSY_LEASE_EXACT_TEST: &str =
    "core_store::tests::busy_leases_allow_new_install_but_prevent_retirement";
const EXACT_FIXTURE_MARKER: &str = "POLARIS_BUSY_LEASE_EXACT_CHILD";
const EXACT_FIXTURE_NONCE: &str = "POLARIS_BUSY_LEASE_EXACT_NONCE";
const EXACT_FIXTURE_PROBE: &str = "POLARIS_BUSY_LEASE_PROTOCOL_CASE";
const EXACT_FIXTURE_PROTOCOL: &str = "core-store-exact-v1";
const EXACT_FIXTURE_RECORD: &str = "EXACT_LEASE_CHILD ";
const EXACT_FIXTURE_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
struct ExactFixtureContext {
    test: String,
    parent_pid: u32,
    nonce: String,
}

fn exact_fixture_args(test: &str) -> [String; 4] {
    [
        "--exact".into(),
        test.into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ]
}

fn exact_fixture_protocol_error(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn parse_exact_fixture_context(
    marker: Option<&str>,
    nonce: Option<&str>,
    test: &str,
    args: &[String],
    parent_pid: u32,
) -> io::Result<Option<ExactFixtureContext>> {
    let (marker, nonce) = match (marker, nonce) {
        (None, None) => return Ok(None),
        (Some(marker), Some(nonce)) => (marker, nonce),
        _ => {
            return Err(exact_fixture_protocol_error(
                "incomplete exact-child marker",
            ))
        }
    };
    if marker.len() > 512
        || nonce.len() != 64
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || args != exact_fixture_args(test).as_slice()
    {
        return Err(exact_fixture_protocol_error(
            "invalid exact-child nonce or argv",
        ));
    }
    let fields: Vec<_> = marker.split('|').collect();
    if fields.len() != 4
        || fields[0] != EXACT_FIXTURE_PROTOCOL
        || fields[1] != test
        || fields[2].parse::<u32>().ok() != Some(parent_pid)
        || parent_pid == 0
        || fields[3] != nonce
    {
        return Err(exact_fixture_protocol_error(
            "inconsistent exact-child identity",
        ));
    }
    Ok(Some(ExactFixtureContext {
        test: test.into(),
        parent_pid,
        nonce: nonce.into(),
    }))
}

fn exact_fixture_child_context(test: &str) -> io::Result<Option<ExactFixtureContext>> {
    fn value(key: &str) -> io::Result<Option<String>> {
        match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err(exact_fixture_protocol_error("non-text exact-child marker"))
            }
        }
    }
    let marker = value(EXACT_FIXTURE_MARKER)?;
    let nonce = value(EXACT_FIXTURE_NONCE)?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let parent_pid = u32::try_from(nix::unistd::getppid().as_raw())
        .map_err(|_| exact_fixture_protocol_error("invalid exact-child parent PID"))?;
    parse_exact_fixture_context(marker.as_deref(), nonce.as_deref(), test, &args, parent_pid)
}

fn exact_fixture_record(context: &ExactFixtureContext, pid: u32, phase: &str) -> String {
    format!(
        "{EXACT_FIXTURE_RECORD}{EXACT_FIXTURE_PROTOCOL} phase={phase} test={} parent_pid={} child_pid={pid} nonce={}",
        context.test, context.parent_pid, context.nonce
    )
}

fn emit_exact_fixture_record(context: &ExactFixtureContext, phase: &str) {
    writeln!(
        io::stderr().lock(),
        "{}",
        exact_fixture_record(context, std::process::id(), phase)
    )
    .expect("write exact fixture lifecycle record");
}

fn verify_exact_fixture_completion(
    success: bool,
    stderr: &str,
    context: &ExactFixtureContext,
    child_pid: u32,
) -> io::Result<()> {
    if !success {
        return Err(io::Error::other(
            "exact fixture child exited unsuccessfully",
        ));
    }
    let entry = exact_fixture_record(context, child_pid, "entry");
    let complete = exact_fixture_record(context, child_pid, "complete");
    let actual: Vec<_> = stderr
        .lines()
        .filter(|line| line.starts_with(EXACT_FIXTURE_RECORD))
        .collect();
    if actual != vec![entry.as_str(), complete.as_str()] {
        return Err(exact_fixture_protocol_error(
            "exact fixture requires one entry and one matching completion",
        ));
    }
    Ok(())
}

struct OwnedExactFixtureChild {
    child: std::process::Child,
    reaped: bool,
}

impl OwnedExactFixtureChild {
    fn kill_and_reap(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.kill()?;
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}

#[derive(Debug)]
struct ExactFixtureReadyTimeout {
    context: ExactFixtureContext,
    child_pid: u32,
    ready_record: String,
    post_ready_elapsed: std::time::Duration,
    termination: std::process::ExitStatus,
}

impl std::fmt::Display for ExactFixtureReadyTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exact fixture post-ready deadline child_pid={} nonce={} elapsed={:?} termination={} ready={}",
            self.child_pid,
            self.context.nonce,
            self.post_ready_elapsed,
            self.termination,
            self.ready_record
        )
    }
}

impl std::error::Error for ExactFixtureReadyTimeout {}

impl Drop for OwnedExactFixtureChild {
    fn drop(&mut self) {
        if !self.reaped {
            // Own this Child, never a discovered PID. Also runs on timeout,
            // I/O/wait failure and unwinding; wait reaps after kill or exit.
            let _ = self.child.kill();
            if let Err(error) = self.child.wait() {
                let _ = writeln!(
                    io::stderr().lock(),
                    "exact fixture child cleanup wait failed: {error}"
                );
            }
        }
    }
}

fn exact_fixture_capture(file: &mut File) -> io::Result<(String, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut raw = Vec::new();
    file.take(EXACT_FIXTURE_OUTPUT_LIMIT as u64 + 1)
        .read_to_end(&mut raw)?;
    let truncated = raw.len() > EXACT_FIXTURE_OUTPUT_LIMIT;
    raw.truncate(EXACT_FIXTURE_OUTPUT_LIMIT);
    Ok((String::from_utf8_lossy(&raw).into_owned(), truncated))
}

fn exact_fixture_timeout_ready(
    stderr: &str,
    context: &ExactFixtureContext,
    child_pid: u32,
) -> io::Result<bool> {
    let entry = exact_fixture_record(context, child_pid, "entry");
    let ready = exact_fixture_record(context, child_pid, "timeout-ready");
    // A concurrent write may not yet have its final newline. Only complete
    // records can establish readiness; a wrong/duplicate identity fails closed.
    let actual: Vec<_> = stderr
        .split_inclusive('\n')
        .filter_map(|line| line.strip_suffix('\n'))
        .filter(|line| line.starts_with(EXACT_FIXTURE_RECORD))
        .collect();
    if actual.is_empty() || actual == vec![entry.as_str()] {
        return Ok(false);
    }
    if actual == vec![entry.as_str(), ready.as_str()] {
        return Ok(true);
    }
    Err(exact_fixture_protocol_error(
        "invalid timeout-ready lifecycle records",
    ))
}

fn observe_exact_fixture_ready(
    stderr: &File,
    context: &ExactFixtureContext,
    child_pid: u32,
) -> io::Result<bool> {
    use std::os::unix::fs::FileExt;
    let length = stderr.metadata()?.len();
    if length > EXACT_FIXTURE_OUTPUT_LIMIT as u64 {
        return Err(exact_fixture_protocol_error(
            "exact fixture output budget exceeded",
        ));
    }
    let mut raw = vec![0; length as usize];
    // read_at does not seek the open description shared with the child's
    // stderr. Inspecting readiness cannot alter where the child writes.
    stderr.read_exact_at(&mut raw, 0)?;
    exact_fixture_timeout_ready(&String::from_utf8_lossy(&raw), context, child_pid)
}

fn wait_exact_fixture_child(
    owned: &mut OwnedExactFixtureChild,
    stdout: &File,
    stderr: &File,
    started: std::time::Instant,
    timeout: std::time::Duration,
    context: &ExactFixtureContext,
    post_ready_timeout: Option<std::time::Duration>,
) -> io::Result<std::process::ExitStatus> {
    let child_pid = owned.child.id();
    let mut ready_started: Option<std::time::Instant> = None;
    loop {
        if stdout.metadata()?.len() > EXACT_FIXTURE_OUTPUT_LIMIT as u64
            || stderr.metadata()?.len() > EXACT_FIXTURE_OUTPUT_LIMIT as u64
        {
            return Err(exact_fixture_protocol_error(
                "exact fixture output budget exceeded",
            ));
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "exact fixture absolute total deadline exceeded",
            ));
        }
        if let Some(status) = owned.child.try_wait()? {
            owned.reaped = true;
            if post_ready_timeout.is_some() {
                return Err(exact_fixture_protocol_error(
                    "timeout control exited before the intended ready deadline",
                ));
            }
            return Ok(status);
        }
        if let Some(short_timeout) = post_ready_timeout {
            if observe_exact_fixture_ready(stderr, context, child_pid)? {
                if ready_started.is_none() && started.elapsed() >= std::time::Duration::from_secs(5)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timeout control matching ready arrived after readiness deadline",
                    ));
                }
                let ready = ready_started.get_or_insert_with(std::time::Instant::now);
                let post_ready_elapsed = ready.elapsed();
                if post_ready_elapsed >= short_timeout {
                    // The independent absolute deadline remains authoritative.
                    // Generic startup/readiness/total timeouts never produce
                    // ExactFixtureReadyTimeout and cannot satisfy the control.
                    if started.elapsed() >= timeout {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "exact fixture absolute total deadline exceeded",
                        ));
                    }
                    if owned.child.try_wait()?.is_some() {
                        owned.reaped = true;
                        return Err(exact_fixture_protocol_error(
                            "timeout control was not running at the ready deadline",
                        ));
                    }
                    use std::os::unix::process::ExitStatusExt;
                    let termination = owned.kill_and_reap()?;
                    if termination.signal() != Some(nix::libc::SIGKILL) {
                        return Err(exact_fixture_protocol_error(
                            "timeout control did not terminate by the owned kill",
                        ));
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        ExactFixtureReadyTimeout {
                            context: context.clone(),
                            child_pid,
                            ready_record: exact_fixture_record(context, child_pid, "timeout-ready"),
                            post_ready_elapsed,
                            termination,
                        },
                    ));
                }
            } else if ready_started.is_some() {
                return Err(exact_fixture_protocol_error(
                    "timeout-ready record disappeared",
                ));
            } else if started.elapsed() >= std::time::Duration::from_secs(5) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timeout control readiness deadline exceeded without matching ready",
                ));
            }
        }
        // Child lifecycle polling only, never a retirement/lock retry.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn run_exact_fixture_child(
    test: &str,
    probe_case: Option<&str>,
    timeout: std::time::Duration,
) -> io::Result<()> {
    static INVOCATION: AtomicU64 = AtomicU64::new(0);
    let parent_pid = std::process::id();
    let serial = INVOCATION.fetch_add(1, Ordering::Relaxed);
    let unix_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| exact_fixture_protocol_error("exact fixture clock unavailable"))?
        .as_nanos();
    let context = ExactFixtureContext {
        test: test.into(),
        parent_pid,
        nonce: crate::core_hash::sha256_hex(format!("{parent_pid}:{serial}:{unix_ns}").as_bytes()),
    };
    // Only anonymous output files exist in the parent. No store, fixture
    // directory, installed core or runtime/version lease exists before exec.
    // Regular files avoid pipe backpressure and inherited-pipe EOF deadlocks.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(exact_fixture_args(test))
        .env(
            EXACT_FIXTURE_MARKER,
            format!(
                "{EXACT_FIXTURE_PROTOCOL}|{test}|{parent_pid}|{}",
                context.nonce
            ),
        )
        .env(EXACT_FIXTURE_NONCE, &context.nonce)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout.try_clone()?))
        .stderr(std::process::Stdio::from(stderr.try_clone()?));
    if let Some(case) = probe_case {
        command.env(EXACT_FIXTURE_PROBE, case);
    }
    let started = std::time::Instant::now();
    let mut owned = OwnedExactFixtureChild {
        child: command.spawn()?,
        reaped: false,
    };
    let child_pid = owned.child.id();
    let post_ready_timeout =
        (probe_case == Some("timeout")).then_some(std::time::Duration::from_millis(100));
    let result = wait_exact_fixture_child(
        &mut owned,
        &stdout,
        &stderr,
        started,
        timeout,
        &context,
        post_ready_timeout,
    );
    drop(owned); // Terminate/reap on every error before inspecting captured output.
    let (stdout, stdout_truncated) = exact_fixture_capture(&mut stdout)?;
    let (stderr, stderr_truncated) = exact_fixture_capture(&mut stderr)?;
    // Forward bounded useful output, including successful stage records, through
    // OS stderr rather than libtest's success-only capture suppression.
    {
        let mut output = io::stderr().lock();
        writeln!(output, "EXACT_LEASE_CAPTURE child_pid={child_pid} stdout_truncated={stdout_truncated} stderr_truncated={stderr_truncated}")?;
        output.write_all(stdout.as_bytes())?;
        output.write_all(stderr.as_bytes())?;
    }
    if stdout_truncated || stderr_truncated {
        return Err(exact_fixture_protocol_error(
            "exact fixture output was truncated",
        ));
    }
    if let Err(error) = &result {
        if let Some(timed) = error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<ExactFixtureReadyTimeout>())
        {
            if !exact_fixture_timeout_ready(&stderr, &timed.context, timed.child_pid)? {
                return Err(exact_fixture_protocol_error(
                    "timeout lacked captured ready evidence",
                ));
            }
        }
    }
    let status = result?;
    verify_exact_fixture_completion(status.success(), &stderr, &context, child_pid)
}

#[test]
fn busy_leases_allow_new_install_but_prevent_retirement() {
    match exact_fixture_child_context(BUSY_LEASE_EXACT_TEST)
        .expect("valid exact busy-lease child protocol")
    {
        Some(context) => {
            emit_exact_fixture_record(&context, "entry");
            busy_lease_fixture_body(&context.nonce);
            emit_exact_fixture_record(&context, "complete");
        }
        None => run_exact_fixture_child(
            BUSY_LEASE_EXACT_TEST,
            None,
            std::time::Duration::from_secs(30),
        )
        .expect("isolated busy-lease fixture completes once and is reaped"),
    }
}

fn busy_lease_fixture_body(nonce: &str) {
    let (root, store, uid) = store();
    let mut leases = Vec::new();
    let started = std::time::Instant::now();
    for (iteration, version) in ["1.0.0", "1.0.1", "1.0.2", "1.0.3", "1.0.4"]
        .into_iter()
        .enumerate()
    {
        let (source, manifest) = fixture(version);
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap_or_else(|error| panic!("busy-lease stage=install iteration={iteration} version={version} held_leases={} elapsed_ns={} error={error:?}",
                leases.len(), started.elapsed().as_nanos()));
        leases.push(
            store
                .verify_objects(&CoreId::parse(&manifest.id).unwrap(),true)
                .unwrap_or_else(|error| panic!("busy-lease stage=outer-start-verify iteration={iteration} version={version} held_leases={} elapsed_ns={} error={error:?}",
                    leases.len(), started.elapsed().as_nanos())),
        );
    }
    eprintln!("busy-lease stage=retire-held held_leases={}", leases.len());
    lock_diagnostics::lease_stage_origin(lock_diagnostics::LeaseStage::OriginalsHeld, nonce);
    assert!(store.retain_latest_three(&[]).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("manifests")).unwrap().count(),
        5
    );
    assert_eq!(
        fs::read_dir(root.path().join("versions")).unwrap().count(),
        5
    );
    // A shared open-description reference survives the original VerifiedCore.
    // Mutation-scope unlock must never release these distinct runtime leases.
    let inherited_leases: Vec<File> = leases
        .iter()
        .map(|core| core._lease.as_ref().unwrap().try_clone().unwrap())
        .collect();
    drop(leases);
    lock_diagnostics::lease_stage_origin(lock_diagnostics::LeaseStage::ClonesHeld, nonce);
    assert!(store.retain_latest_three(&[]).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("manifests")).unwrap().count(),
        5
    );
    assert_eq!(
        fs::read_dir(root.path().join("versions")).unwrap().count(),
        5
    );
    drop(inherited_leases);
    eprintln!("busy-lease stage=retire-after-drop held_leases=0");
    lock_diagnostics::lease_stage_origin(
        lock_diagnostics::LeaseStage::AllLocalLeasesDropped,
        nonce,
    );
    let retired = store.retain_latest_three(&[]).unwrap_or_else(|error| {
        panic!(
            "busy-lease stage=retire-after-drop elapsed_ns={} error={error:?}",
            started.elapsed().as_nanos()
        )
    });
    assert_eq!(
        retired.len(),
        2,
        "busy-lease stage=retire-after-drop elapsed_ns={}",
        started.elapsed().as_nanos()
    );
    assert_eq!(
        fs::read_dir(root.path().join("versions")).unwrap().count(),
        3
    );
}

#[test]
fn exact_fixture_marker_and_completion_fail_closed() {
    let nonce = "a".repeat(64);
    let marker = format!("{EXACT_FIXTURE_PROTOCOL}|{BUSY_LEASE_EXACT_TEST}|42|{nonce}");
    let args = exact_fixture_args(BUSY_LEASE_EXACT_TEST);
    let valid = parse_exact_fixture_context(
        Some(&marker),
        Some(&nonce),
        BUSY_LEASE_EXACT_TEST,
        &args,
        42,
    )
    .unwrap()
    .unwrap();
    assert!(
        parse_exact_fixture_context(None, None, BUSY_LEASE_EXACT_TEST, &args, 42)
            .unwrap()
            .is_none()
    );
    for (marker, nonce) in [
        (Some(marker.as_str()), None),
        (None, Some(nonce.as_str())),
        (Some("wrong-protocol"), Some(nonce.as_str())),
        (Some(marker.as_str()), Some("not-a-nonce")),
    ] {
        assert!(
            parse_exact_fixture_context(marker, nonce, BUSY_LEASE_EXACT_TEST, &args, 42,).is_err()
        );
    }
    let different_nonce = "b".repeat(64);
    for wrong_marker in [
        marker.replace(EXACT_FIXTURE_PROTOCOL, "core-store-exact-v0"),
        marker.replace(BUSY_LEASE_EXACT_TEST, "core_store::tests::different_test"),
    ] {
        assert!(parse_exact_fixture_context(
            Some(&wrong_marker),
            Some(&nonce),
            BUSY_LEASE_EXACT_TEST,
            &args,
            42,
        )
        .is_err());
    }
    assert!(parse_exact_fixture_context(
        Some(&marker),
        Some(&different_nonce),
        BUSY_LEASE_EXACT_TEST,
        &args,
        42,
    )
    .is_err());
    assert!(parse_exact_fixture_context(
        Some(&marker),
        Some(&nonce),
        BUSY_LEASE_EXACT_TEST,
        &args,
        43,
    )
    .is_err());
    let mut wrong_args = args.clone();
    wrong_args[3] = "--test-threads=2".into();
    assert!(parse_exact_fixture_context(
        Some(&marker),
        Some(&nonce),
        BUSY_LEASE_EXACT_TEST,
        &wrong_args,
        42,
    )
    .is_err());
    let entry = exact_fixture_record(&valid, 77, "entry");
    let complete = exact_fixture_record(&valid, 77, "complete");
    let records = format!("{entry}\n{complete}\n");
    assert!(verify_exact_fixture_completion(true, &records, &valid, 77).is_ok());
    assert!(verify_exact_fixture_completion(false, &records, &valid, 77).is_err());
    for wrong in [
        String::new(),
        format!("{entry}\n"),
        format!("{entry}\n{complete}\n{complete}\n"),
        records.replace(&nonce, &different_nonce),
    ] {
        assert!(verify_exact_fixture_completion(true, &wrong, &valid, 77).is_err());
    }
    // Each isolated fixture must bind its own test identity, not merely reuse
    // another fixture's valid nonce and parent identity.
    for test in [SAME_ID_EXACT_TEST, INHERITED_VERSION_LEASE_EXACT_TEST] {
        let target_marker = format!("{EXACT_FIXTURE_PROTOCOL}|{test}|42|{nonce}");
        let target_args = exact_fixture_args(test);
        let target =
            parse_exact_fixture_context(Some(&target_marker), Some(&nonce), test, &target_args, 42)
                .unwrap()
                .unwrap();
        assert_eq!(target.test, test);
        assert!(
            parse_exact_fixture_context(Some(&marker), Some(&nonce), test, &target_args, 42,)
                .is_err()
        );
        assert!(parse_exact_fixture_context(
            Some(&target_marker),
            Some(&nonce),
            BUSY_LEASE_EXACT_TEST,
            &args,
            42,
        )
        .is_err());
    }
}

#[test]
fn exact_timeout_readiness_requires_complete_matching_records() {
    let context = ExactFixtureContext {
        test: BUSY_LEASE_EXACT_TEST.into(),
        parent_pid: 42,
        nonce: "a".repeat(64),
    };
    let entry = exact_fixture_record(&context, 77, "entry");
    let ready = exact_fixture_record(&context, 77, "timeout-ready");
    assert!(!exact_fixture_timeout_ready("", &context, 77).unwrap());
    assert!(!exact_fixture_timeout_ready(&format!("{entry}\n"), &context, 77).unwrap());
    assert!(!exact_fixture_timeout_ready(&format!("{entry}\n{ready}"), &context, 77).unwrap());
    assert!(exact_fixture_timeout_ready(&format!("{entry}\n{ready}\n"), &context, 77).unwrap());
    for wrong in [
        format!(
            "{entry}\n{}\n",
            ready.replace("child_pid=77", "child_pid=78")
        ),
        format!(
            "{entry}\n{}\n",
            ready.replace("parent_pid=42", "parent_pid=43")
        ),
        format!(
            "{entry}\n{}\n",
            ready.replace(&context.nonce, &"b".repeat(64))
        ),
        format!("{entry}\n{ready}\n{ready}\n"),
        format!(
            "{entry}\n{}\n",
            exact_fixture_record(&context, 77, "complete")
        ),
    ] {
        assert!(exact_fixture_timeout_ready(&wrong, &context, 77).is_err());
    }
    // Startup/readiness/total failures must never satisfy the control merely
    // because they share the generic I/O TimedOut kind.
    let startup_timeout = io::Error::new(io::ErrorKind::TimedOut, "no ready before deadline");
    assert!(startup_timeout
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<ExactFixtureReadyTimeout>())
        .is_none());
}

#[test]
fn exact_fixture_child_protocol_controls() {
    const TEST: &str = "core_store::tests::exact_fixture_child_protocol_controls";
    if let Some(context) = exact_fixture_child_context(TEST).expect("valid exact protocol control")
    {
        // Marked child enters this small control once, never the parent runner
        // or the lease fixture. The parent checks exactly one entry/completion.
        emit_exact_fixture_record(&context, "entry");
        match std::env::var(EXACT_FIXTURE_PROBE).as_deref() {
            Ok("once") => {}
            Ok("missing-completion") => return,
            Ok("wrong-nonce") => {
                let mut wrong = context.clone();
                let replacement = if wrong.nonce.starts_with('0') {
                    "1"
                } else {
                    "0"
                };
                wrong.nonce.replace_range(..1, replacement);
                emit_exact_fixture_record(&wrong, "complete");
                return;
            }
            Ok("nonzero") => {
                emit_exact_fixture_record(&context, "complete");
                std::process::exit(41);
            }
            Ok("timeout") => {
                // Only a dedicated protocol child waits. This is not a lease
                // retry/sleep; the parent's deadline must kill and reap it.
                emit_exact_fixture_record(&context, "timeout-ready");
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
            _ => panic!("invalid exact protocol control case"),
        }
        emit_exact_fixture_record(&context, "complete");
        return;
    }
    for case in [
        "once",
        "missing-completion",
        "wrong-nonce",
        "nonzero",
        "timeout",
    ] {
        // Total deadline includes exec/libtest startup. Only matching readiness
        // starts the separate 100 ms interval in the timeout case.
        let result = run_exact_fixture_child(TEST, Some(case), std::time::Duration::from_secs(10));
        if case == "once" {
            result.expect("marked exact child enters and completes once");
        } else if case == "timeout" {
            use std::os::unix::process::ExitStatusExt;
            let error = result.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            let timed = error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<ExactFixtureReadyTimeout>())
                .expect("control requires a live post-ready timeout, not slow startup");
            assert_eq!(timed.context.test, TEST);
            assert_eq!(timed.context.parent_pid, std::process::id());
            assert_eq!(
                timed.ready_record,
                exact_fixture_record(&timed.context, timed.child_pid, "timeout-ready"),
            );
            assert!(timed.post_ready_elapsed >= std::time::Duration::from_millis(100));
            assert_eq!(timed.termination.signal(), Some(nix::libc::SIGKILL));
        } else {
            assert!(result.is_err(), "protocol control must reject {case}");
        }
    }
}

#[test]
fn lock_failures_keep_closed_roles_and_native_error_classification_without_paths() {
    for role in [
        LockRole::StoreMutationExclusive,
        LockRole::RepairExclusive,
        LockRole::OrphanExclusive,
        LockRole::StartShared,
        LockRole::RetainExclusive,
        LockRole::GcExclusive,
    ] {
        let error = lock_error(
            role,
            LockObservation {
                fd: Some(41),
                dev: Some(7),
                inode: Some(11),
                cloexec: Some(true),
            },
            TryLockError::WouldBlock,
        );
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let text = format!("{error:?}");
        assert!(text.contains(&format!("role={}", role.name())));
        for fact in [
            "class=would-block",
            "fd=Some(41)",
            "dev=Some(7)",
            "inode=Some(11)",
            "cloexec=Some(true)",
        ] {
            assert!(text.contains(fact), "missing closed observation: {fact}");
        }
    }
    let error = lock_error(
        LockRole::StoreMutationExclusive,
        LockObservation::default(),
        TryLockError::Error(io::Error::from_raw_os_error(nix::libc::EACCES)),
    );
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("class=io-error"));
    let failure = error
        .get_ref()
        .unwrap()
        .downcast_ref::<LockFailure>()
        .unwrap();
    let source = std::error::Error::source(failure)
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap();
    assert_eq!(source.raw_os_error(), Some(nix::libc::EACCES));
    // Display and Debug stay safe even when an injected original OS error has
    // a path/message. The original cause remains available through source().
    let secret = "/private/fixture/source payload=never-export";
    let error = lock_error(
        LockRole::StartShared,
        LockObservation::default(),
        TryLockError::Error(io::Error::other(secret)),
    );
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
    assert!(error.to_string().contains("fd=None"));
}

#[test]
fn optional_retirement_skips_only_native_contention_and_preserves_other_errors() {
    let root = tempfile::tempdir().unwrap();
    let file = File::create_new(root.path().join("controlled-lease")).unwrap();
    for role in [LockRole::RetainExclusive, LockRole::GcExclusive] {
        assert!(optional_lock_result(&file, role, Ok(())).unwrap());
        assert!(!optional_lock_result(&file, role, Err(TryLockError::WouldBlock)).unwrap());
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::WouldBlock] {
            let error = optional_lock_result(
                &file,
                role,
                Err(TryLockError::Error(io::Error::new(
                    kind,
                    "/private/controlled-error must-not-appear",
                ))),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(error.to_string().contains("class=io-error"));
            assert!(error.to_string().contains(role.name()));
            assert!(!error.to_string().contains("/private/controlled-error"));
        }
    }
}

#[test]
fn role_diagnostics_preserve_shared_and_exclusive_modes_and_descriptor_flags() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("controlled-lease");
    let owner = File::create_new(&path).unwrap();
    let peer_shared = File::open(&path).unwrap();
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    acquire_lock(&owner, LockRole::StartShared).unwrap();
    acquire_lock(&peer_shared, LockRole::StartShared).unwrap();
    let flags = nix::fcntl::fcntl(&contender, nix::fcntl::FcntlArg::F_GETFD).unwrap();
    for role in [
        LockRole::StoreMutationExclusive,
        LockRole::RepairExclusive,
        LockRole::OrphanExclusive,
    ] {
        let error = acquire_lock(&contender, role).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<LockFailure>()
            .unwrap();
        assert_eq!(failure.observation.fd, Some(contender.as_raw_fd()));
        assert_eq!(
            failure.observation.dev,
            Some(contender.metadata().unwrap().dev())
        );
        assert_eq!(
            failure.observation.inode,
            Some(contender.metadata().unwrap().ino())
        );
        assert_eq!(
            failure.observation.cloexec,
            Some(flags & nix::libc::FD_CLOEXEC != 0)
        );
        assert_eq!(
            nix::fcntl::fcntl(&contender, nix::fcntl::FcntlArg::F_GETFD).unwrap(),
            flags
        );
    }
    // The contender is a separately opened, initially unlocked description;
    // never attempt unspecified shared-to-exclusive conversion on a holder.
    let exclusive_path = root.path().join("exclusive-lease");
    let exclusive = File::create_new(&exclusive_path).unwrap();
    let shared_contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&exclusive_path)
        .unwrap();
    acquire_lock(&exclusive, LockRole::StoreMutationExclusive).unwrap();
    let error = acquire_lock(&shared_contender, LockRole::StartShared).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("role=start-shared"));
}

#[test]
fn linked_payload_and_weakened_store_permissions_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::remove_file(source.path().join("sing-box")).unwrap();
    symlink(
        source.path().join("libcronet.so"),
        source.path().join("sing-box"),
    )
    .unwrap();
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
}

#[test]
fn flat_retirement_preserves_unknown_or_busy_cohorts_and_all_versioned_inputs() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    for name in ["sing-box", "libcronet.so"] {
        let path = root.path().join(name);
        fs::write(&path, b"old-flat").unwrap();
        private_mode(&path, 0o600).unwrap();
    }
    for state in [
        crate::legacy_use::LegacyUse::Unknown,
        crate::legacy_use::LegacyUse::InUse,
    ] {
        assert!(store.retire_flat_layout_with(|_| state).unwrap().is_empty());
        for name in ["sing-box", "libcronet.so"] {
            assert_eq!(fs::read(root.path().join(name)).unwrap(), b"old-flat");
        }
    }
    assert_eq!(
        store
            .retire_flat_layout_with(|paths| {
                let mut observed = paths
                    .iter()
                    .map(|p| p.file_name().unwrap().to_str().unwrap())
                    .collect::<Vec<_>>();
                observed.sort_unstable();
                assert_eq!(observed, ["libcronet.so", "sing-box"]);
                assert!(paths.iter().all(|p| p.parent() == Some(root.path())));
                crate::legacy_use::LegacyUse::Idle
            })
            .unwrap()
            .len(),
        2
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    assert!(store.verify_objects(&id, true).is_ok());
}

#[test]
fn mutation_scope_releases_on_error_and_unwind() {
    let (_root, store, _uid) = store();
    let error = (|| -> io::Result<()> {
        let _scope = store.mutation_lock()?;
        Err(io::Error::other("controlled mutation error"))
    })();
    assert!(error.is_err());
    drop(store.mutation_lock().unwrap());
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = store.mutation_lock().unwrap();
        panic!("controlled mutation unwind");
    }));
    assert!(unwound.is_err());
    let _next_scope = store.mutation_lock().unwrap();
}

#[test]
fn migration_preserves_existing_payload_even_when_unverified() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("migration");
    assert_eq!(
        store
            .install_preserving(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    let payload = root
        .path()
        .join("versions")
        .join(&manifest.id)
        .join("sing-box");
    let receipt = root
        .path()
        .join("manifests")
        .join(format!("{}.json", manifest.id));
    let before = fs::read(&receipt).unwrap();
    assert_eq!(
        store
            .install_preserving(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::AlreadyPresent
    );
    fs::write(&payload, b"damaged old payload").unwrap();
    assert!(store
        .install_preserving(&manifest, source.path(), Some(uid), None)
        .is_err());
    assert_eq!(fs::read(&payload).unwrap(), b"damaged old payload");
    assert_eq!(fs::read(&receipt).unwrap(), before);
    assert_eq!(fs::read_dir(root.path().join(".trash")).unwrap().count(), 0);
}

#[test]
fn migration_preserves_orphan_payload_without_manifest() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("migration-orphan");
    let version = root.path().join("versions").join(&manifest.id);
    fs::create_dir(&version).unwrap();
    fs::write(version.join("sing-box"), b"original orphan").unwrap();
    assert!(store
        .install_preserving(&manifest, source.path(), Some(uid), None)
        .is_err());
    assert_eq!(
        fs::read(version.join("sing-box")).unwrap(),
        b"original orphan"
    );
}

#[test]
fn idle_diagnostic_cache_in_another_process_does_not_block_first_repair() {
    const TEST: &str =
        "core_store::tests::idle_diagnostic_cache_in_another_process_does_not_block_first_repair";
    let context = match exact_fixture_child_context(TEST).unwrap() {
        None => {
            run_exact_fixture_child(TEST, None, std::time::Duration::from_secs(30)).unwrap();
            return;
        }
        Some(context) => {
            emit_exact_fixture_record(&context, "entry");
            context
        }
    };
    let (root, store, uid) = store();
    let (source, manifest) = fixture("idle-cache");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut daemon = Daemon(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core_store::tests::idle_storage_cache_daemon_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("POLARIS_TEST_IDLE_CACHE_ROOT", root.path())
            .env("POLARIS_TEST_IDLE_CACHE_ID", &manifest.id)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !root.path().join("cache-ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "diagnostic daemon did not publish readiness"
        );
        assert!(daemon.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let id = CoreId::parse(&manifest.id).unwrap();
    fs::write(
        root.path()
            .join("versions")
            .join(id.as_str())
            .join("libcronet.so"),
        b"changed",
    )
    .unwrap();
    // No second query/stop/invalidate is sent to the daemon before this first
    // actual repair. Its successful storage cache remains strongly retained.
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert!(daemon.0.try_wait().unwrap().is_none());
    fs::write(root.path().join("cache-release"), b"release").unwrap();
    assert!(daemon.0.wait().unwrap().success());
    assert!(store.verify_objects(&id, true).is_ok());
    emit_exact_fixture_record(&context, "complete");
}

#[test]
fn idle_storage_cache_daemon_fixture() {
    let Some(root) = std::env::var_os("POLARIS_TEST_IDLE_CACHE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let id = CoreId::parse(&std::env::var("POLARIS_TEST_IDLE_CACHE_ID").unwrap()).unwrap();
    let owner = OwnerPolicy {
        uid: Some(nix::unistd::Uid::effective().as_raw()),
    };
    let store = CoreStore::open(&root, owner).unwrap();
    let core = store.verify_objects(&id, true).unwrap();
    // This private storage fixture exercises the same diagnostic_entry step.
    // It deliberately has no AdmittedPayload and cannot grant an execution.
    crate::pinned_core::cache_storage_fixture(&root, &id, owner, core);
    fs::write(root.join("cache-ready"), b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("cache-release").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "cache controller disappeared"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn removing_idle_cache_lease_cannot_unlock_a_live_or_unknown_owner() {
    const TEST: &str =
        "core_store::tests::removing_idle_cache_lease_cannot_unlock_a_live_or_unknown_owner";
    let context = match exact_fixture_child_context(TEST).unwrap() {
        None => {
            run_exact_fixture_child(TEST, None, std::time::Duration::from_secs(30)).unwrap();
            return;
        }
        Some(context) => {
            emit_exact_fixture_record(&context, "entry");
            context
        }
    };
    let (_root, store, uid) = store();
    let (source, manifest) = fixture("live-cache-owner");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    let id = CoreId::parse(&manifest.id).unwrap();
    let live = store.verify_objects(&id, true).unwrap();
    let diagnostic = live.diagnostic_cache().unwrap();
    assert!(diagnostic._lease.is_none());
    assert!(diagnostic
        .prepare(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain)
        .is_err());
    drop(diagnostic);
    crate::pinned_core::invalidate_unused();
    let candidate = store.lease_file(&id).unwrap();
    assert!(matches!(
        candidate.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    // Closing an idle cache never explicitly unlocks a shared description that
    // can still belong to a Child/tail/debt owner.
    let inherited = live._lease.as_ref().unwrap().try_clone().unwrap();
    drop(live);
    assert!(matches!(
        candidate.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    drop(inherited);
    candidate.try_lock().unwrap();
    emit_exact_fixture_record(&context, "complete");
}

#[test]
fn helper_store_damage_and_environment_io_keep_distinct_recovery_routes() {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionError, AdmissionErrorKind as K, RecoveryAction as R,
    };
    for (source, kind, recovery) in [
        (
            io::Error::from_raw_os_error(nix::libc::ELOOP),
            K::Mismatch,
            R::RepairHelperInstallation,
        ),
        (
            io::Error::from_raw_os_error(nix::libc::ENOTDIR),
            K::Mismatch,
            R::RepairHelperInstallation,
        ),
        (
            invalid("non-regular object"),
            K::Mismatch,
            R::RepairHelperInstallation,
        ),
        (
            io::Error::from(io::ErrorKind::NotFound),
            K::Missing,
            R::RepairHelperInstallation,
        ),
        (
            io::Error::from(io::ErrorKind::PermissionDenied),
            K::Io,
            R::None,
        ),
        (
            io::Error::from(io::ErrorKind::WouldBlock),
            K::Busy,
            R::RetryAfterStop,
        ),
    ] {
        let mapped = store_admission_failure(source);
        let typed = mapped
            .get_ref()
            .unwrap()
            .downcast_ref::<AdmissionError>()
            .unwrap();
        assert_eq!((typed.kind, typed.recovery), (kind, recovery));
    }
}

use super::*;
use crate::core_store::{CoreStore, LockFailure, OwnerPolicy};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn store() -> (tempfile::TempDir, CoreStore) {
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let store = CoreStore::initialize(root.path(), OwnerPolicy { uid: Some(uid) }).unwrap();
    (root, store)
}
fn failure(store: &CoreStore) -> io::Error {
    match store.mutation_lock() {
        Ok(_) => panic!("expected one nonblocking mutation-lock contention"),
        Err(error) => error,
    }
}
fn observed(error: &io::Error) -> &Snapshot {
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<LockFailure>()
        .unwrap()
        .holder_snapshot
        .as_ref()
        .unwrap()
}

#[test]
fn same_process_holder_and_parent_file_close_have_distinct_original_observations() {
    let (_root, store) = store();
    let owner = store.mutation_lock().unwrap();
    let fd = owner.file.as_ref().unwrap().as_raw_fd();
    let flags =
        nix::fcntl::fcntl(owner.file.as_ref().unwrap(), nix::fcntl::FcntlArg::F_GETFD).unwrap();
    let error = failure(&store);
    let snapshot = observed(&error);
    writeln!(
        io::stderr().lock(),
        "LOCK_CONTROL_SAME_PROCESS phase=owner-held {error}"
    )
    .unwrap();
    assert!(snapshot
        .events
        .iter()
        .any(|e| e.phase == "acquired" && e.fd == Some(fd)));
    assert!(snapshot
        .native
        .peers
        .iter()
        .any(|p| p.identity.pid == std::process::id() && p.fd == fd));
    assert!(snapshot
        .native
        .peers
        .iter()
        .filter(|p| p.fd == fd)
        .all(|p| p.identity.start[0] > 0));
    #[cfg(target_os = "linux")]
    assert!(snapshot
        .native
        .peers
        .iter()
        .any(|p| p.fd == fd && p.flock_write == Some(true)));
    assert_eq!(
        nix::fcntl::fcntl(owner.file.as_ref().unwrap(), nix::fcntl::FcntlArg::F_GETFD).unwrap(),
        flags
    );
    assert!(!error.to_string().contains(_root.path().to_str().unwrap()));
    drop(owner);
    // This is a separately opened description and a new scope, not retrying
    // a failed install or converting the holder's shared/exclusive mode.
    let next = store.mutation_lock().unwrap();
    let snapshot = super::snapshot(&next.observation);
    assert!(snapshot
        .events
        .iter()
        .any(|e| e.phase == "parent-file-closed" && e.fd == Some(fd)));
}

struct ChildCleanup(Option<i32>);

// Direct OS stderr is retained for successful controls too. libtest captures
// eprintln! on success, which otherwise hides the PID needed to distinguish
// this exact diagnostic subprocess from an unrelated production spawn.
fn control_origin(phase: &str, child_pid: Option<u32>) {
    let unix_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_nanos());
    let monotonic_ns = monotonic_ns();
    writeln!(
        io::stderr().lock(),
        "LOCK_CONTROL_ORIGIN origin=isolated-inherited-cloexec phase={phase} parent_pid={} child_pid={child_pid:?} unix_ns={unix_ns:?} monotonic_ns={monotonic_ns} scope=test-only-not-production",
        std::process::id()
    )
    .unwrap();
}

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        // This guard owns the unreaped child. Even assertion failure releases
        // the controlled barrier; no detached thread or external PID is killed.
        let Some(raw) = self.0 else {
            return;
        };
        let pid = nix::unistd::Pid::from_raw(raw);
        if matches!(
            nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
            Ok(nix::sys::wait::WaitStatus::StillAlive)
        ) {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = nix::sys::wait::waitpid(pid, None);
        }
    }
}

#[test]
#[allow(
    unsafe_code,
    reason = "controlled fork-only diagnostic; child uses async-signal-safe syscalls"
)]
fn inherited_cloexec_description_remains_observable_after_parent_file_closes() {
    const PROBE: &str = "POLARIS_LOCK_DIAGNOSTIC_PROBE";
    if std::env::var(PROBE).as_deref() != Ok("isolated-v1") {
        // The deliberate fork barrier must not inherit and prolong unrelated
        // parallel unit tests' descriptors. Exec the exact control first; its
        // later fork sees only this isolated fixture, not the full test cohort.
        control_origin("before-exact-exec", None);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "core_store::lock_diagnostics::tests::inherited_cloexec_description_remains_observable_after_parent_file_closes", "--nocapture"])
            .env(PROBE, "isolated-v1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let child_pid = child.id();
        control_origin("exact-exec-returned", Some(child_pid));
        let output = child.wait_with_output().unwrap(); // Drains both streams concurrently.
        control_origin("exact-child-reaped", Some(child_pid));
        // Only this control's closed diagnostic records are forwarded, not
        // arbitrary child arguments, environment, stdout or panic contents.
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            if line.len() <= 64 * 1024
                && (line.starts_with("LOCK_CONTROL_ORIGIN ")
                    || line.starts_with("controlled-fork-only-child-pid="))
            {
                writeln!(io::stderr().lock(), "{line}").unwrap();
            }
        }
        eprintln!(
            "isolated-lock-control stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "isolated native inheritance control failed"
        );
        return;
    }
    // SAFETY: only the isolated control process receives this deadline. The
    // real parent test process and the rest of the unit cohort are untouched.
    unsafe {
        nix::libc::alarm(20);
    }
    let (root, store) = store();
    // Negative control for raw File close semantics, independent of the
    // production scope guard. Do not change its expected inherited contention.
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.path().join(".core-store.lock"))
        .unwrap();
    owner.try_lock().unwrap();
    let observation = LockObservation::from_file(&owner);
    record("acquired", &observation);
    let fd = owner.as_raw_fd();
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    parent
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let child_fd = child.as_raw_fd();
    // SAFETY: in the child only libc fcntl/write/read/_exit run; no allocator,
    // formatting, mutex, Rust Drop, privilege/device/network operation or exec.
    // The one-byte barrier holds the descriptor between fork and exec/exit.
    let pid = unsafe { nix::libc::fork() };
    assert!(pid >= 0, "controlled fork failed");
    if pid == 0 {
        unsafe {
            nix::libc::alarm(10); // Bound the controlled child even if parent cleanup cannot observe it.
            let flags = nix::libc::fcntl(fd, nix::libc::F_GETFD);
            let ready = [u8::from(flags >= 0 && flags & nix::libc::FD_CLOEXEC != 0)];
            if nix::libc::write(child_fd, ready.as_ptr().cast(), 1) != 1 {
                nix::libc::_exit(91);
            }
            let mut release = [0u8];
            if nix::libc::read(child_fd, release.as_mut_ptr().cast(), 1) != 1 {
                nix::libc::_exit(92);
            }
            nix::libc::_exit(0);
        }
    }
    let mut cleanup = ChildCleanup(Some(pid));
    drop(child);
    let mut ready = [0u8];
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(
        ready,
        [1],
        "actual inherited descriptor must retain CLOEXEC before exec"
    );
    drop(owner);
    record("parent-file-closed", &observation);
    let error = failure(&store);
    let snapshot = observed(&error);
    control_origin("forced-fork-after-parent-file-drop", Some(pid as u32));
    writeln!(
        io::stderr().lock(),
        "controlled-fork-only-child-pid={pid} {error}"
    )
    .unwrap();
    assert!(snapshot
        .events
        .iter()
        .any(|e| e.phase == "parent-file-closed" && e.fd == Some(fd)));
    assert!(snapshot
        .native
        .peers
        .iter()
        .any(|p| p.identity.pid == pid as u32
            && p.fd == fd
            && p.cloexec == Some(true)
            && p.same_executable == Some(true)));
    #[cfg(target_os = "linux")]
    {
        assert!(snapshot
            .native
            .peers
            .iter()
            .any(|p| p.identity.pid == pid as u32 && p.flock_write == Some(true)));
        // The contender may reuse the closed parent's descriptor number. Its
        // open description must not be mislabelled as the original lock holder.
        assert!(!snapshot
            .native
            .peers
            .iter()
            .any(|p| p.identity.pid == std::process::id() && p.flock_write == Some(true)));
    }
    parent.write_all(&[1]).unwrap();
    // Wait/reap belongs to normal control cleanup, not an acquisition retry.
    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).unwrap();
    cleanup.0 = None; // Reaped regardless of status; never signal a potentially reused PID.
    assert!(matches!(status, nix::sys::wait::WaitStatus::Exited(_, 0)));
    let _new_scope = store.mutation_lock().unwrap();
}

#[test]
fn diagnostic_missing_identity_remains_explicitly_unknown() {
    let observation = LockObservation::default();
    let snapshot = snapshot(&observation);
    assert_eq!(snapshot.native.unavailable, 1);
    assert!(snapshot.native.peers.is_empty());
    let formatted = snapshot.to_string();
    assert!(formatted.contains("claim=descriptor-observation-only"));
    assert!(formatted.contains("native-unavailable=1"));
}

#[cfg(target_os = "linux")]
#[test]
fn flock_fields_require_exact_native_kind_device_inode_and_closed_mode() {
    let dev = nix::sys::stat::makedev(8, 1);
    let raw =
        "pos:\t0\nflags:\t02100002\nino:\t99\nlock:\t1: FLOCK ADVISORY WRITE 123 08:01:99 0 EOF\n";
    assert_eq!(platform::flock(raw, dev, 99), Some((true, 123)));
    for wrong in [
        raw.replace("FLOCK", "POSIX"),
        raw.replace("08:01", "08:02"),
        raw.replace(":99", ":100"),
        raw.replace("WRITE", "arbitrary-secret-name"),
        raw.replace("123", "not-a-pid"),
    ] {
        assert_eq!(platform::flock(&wrong, dev, 99), None);
    }
    assert_eq!(
        platform::flock(&raw.replace("WRITE", "READ"), dev, 99),
        Some((false, 123))
    );
}

// Pure source-level regression coverage; these tests do not spawn processes or
// inspect host descriptors. The cross-thread sample uses the same static epoch.
#[test]
fn diagnostic_clock_uses_one_epoch_across_threads() {
    let before = monotonic_ns();
    let child = std::thread::spawn(monotonic_ns).join().unwrap();
    let after = monotonic_ns();
    assert!(before <= child && child <= after);
}

#[test]
fn diagnostic_races_keep_bounded_closed_context_and_total_count() {
    let mut native = Native::default();
    for _ in 0..MAX_RACE_OBSERVATIONS + 3 {
        native.record_race(RaceStage::ParentChanged, 42, Some(7));
    }
    assert_eq!(native.raced, MAX_RACE_OBSERVATIONS + 3);
    assert_eq!(native.race_observations.len(), MAX_RACE_OBSERVATIONS);
    assert_eq!(native.race_observations_lost, 3);
    assert!(native
        .race_observations
        .iter()
        .all(|entry| *entry == (RaceStage::ParentChanged, 42, Some(7))));
}

#[test]
#[allow(
    unsafe_code,
    reason = "isolated fork control; child Drop only gets PID and closes inherited File"
)]
fn mutation_scope_release_and_child_drop_preserve_authority_boundaries() {
    const PROBE: &str = "POLARIS_MUTATION_SCOPE_PROBE";
    if std::env::var(PROBE).as_deref() != Ok("isolated-v1") {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core_store::lock_diagnostics::tests::mutation_scope_release_and_child_drop_preserve_authority_boundaries",
                "--nocapture",
            ])
            .env(PROBE, "isolated-v1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated mutation scope control failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // SAFETY: the deadline applies only to this exact isolated control.
    unsafe { nix::libc::alarm(20) };
    let (_keep_root, keep_store) = store();
    let (_drop_root, drop_store) = store();
    let keep_owner = keep_store.mutation_lock().unwrap();
    let drop_owner = drop_store.mutation_lock().unwrap();
    let inherited_fd = keep_owner.file.as_ref().unwrap().as_raw_fd();
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    parent
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let child_fd = child.as_raw_fd();
    // SAFETY: fork occurs only in the isolated fixture. In the child the actual
    // production guard's non-owner Drop performs getpid/close only, skipping
    // unlock, logging and thread-local observations. Everything else below is
    // libc alarm/fcntl/read/write/_exit; no allocator or inherited mutex is used.
    let pid = unsafe { nix::libc::fork() };
    assert!(pid >= 0, "controlled fork failed");
    if pid == 0 {
        unsafe {
            nix::libc::alarm(10);
            drop(drop_owner);
            let flags = nix::libc::fcntl(inherited_fd, nix::libc::F_GETFD);
            let ready = [u8::from(flags >= 0 && flags & nix::libc::FD_CLOEXEC != 0)];
            if nix::libc::write(child_fd, ready.as_ptr().cast(), 1) != 1 {
                nix::libc::_exit(91);
            }
            let mut release = [0u8];
            if nix::libc::read(child_fd, release.as_mut_ptr().cast(), 1) != 1 {
                nix::libc::_exit(92);
            }
            // Parent has now dropped its original scope, but this descriptor
            // must still exist: success cannot be attributed to exec/exit.
            let still_open = [u8::from(
                nix::libc::fcntl(inherited_fd, nix::libc::F_GETFD) >= 0,
            )];
            if nix::libc::write(child_fd, still_open.as_ptr().cast(), 1) != 1 {
                nix::libc::_exit(93);
            }
            if nix::libc::read(child_fd, release.as_mut_ptr().cast(), 1) != 1 {
                nix::libc::_exit(94);
            }
            nix::libc::_exit(0);
        }
    }
    let mut cleanup = ChildCleanup(Some(pid));
    drop(child);
    let mut ready = [0u8];
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [1]);
    // A real child-side Drop must not unlock the still-live parent scope.
    assert_eq!(failure(&drop_store).kind(), io::ErrorKind::WouldBlock);
    // Genuine scope contention remains immediate and nonblocking.
    assert_eq!(failure(&keep_store).kind(), io::ErrorKind::WouldBlock);
    drop(drop_owner);
    let _next_drop_scope = drop_store.mutation_lock().unwrap();
    drop(keep_owner);
    // One fresh acquisition while the pre-exec child remains behind its
    // barrier and retains the old description; no retries or sleeps.
    let _next_keep_scope = keep_store.mutation_lock().unwrap();
    parent.write_all(&[1]).unwrap();
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(
        ready,
        [1],
        "child must still retain the inherited descriptor"
    );
    assert_eq!(failure(&keep_store).kind(), io::ErrorKind::WouldBlock);
    parent.write_all(&[1]).unwrap();
    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).unwrap();
    cleanup.0 = None;
    assert!(matches!(status, nix::sys::wait::WaitStatus::Exited(_, 0)));
}

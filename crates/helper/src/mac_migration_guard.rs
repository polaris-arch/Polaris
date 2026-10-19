//! Root-only stop proof for one explicitly requested macOS migration.
//!
//! This is administrative CLI code, never an IPC capability. A successful
//! return authorizes only the calling shell's next transaction step. It is not
//! a historical-descendant proof, does not signal processes, and never grants
//! rollback, restart, unlink, payload removal or trust to an old journal.
use crate::core_objects::PayloadDir;
use crate::mac_migration_launchd::{self as launchd, Expected, PlistFacts};
use crate::mac_migration_observer::{
    self as observer, Closure, HeldExecutable, Preflight, ProcessIdentity, Target,
};
use crate::protected_install_path::ProtectedPath;
use polaris_helper_client::InstallPaths;
use polaris_helper_proto::Platform;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STARTED: &str = "migration-stop.started";
const COMPLETE: &str = "migration-stop.complete";
const CANDIDATE: &str = "migration-candidate.closed";
const MAX_RECEIPT: u64 = 4 * 1024 * 1024;
const MAX_OBJECTS: usize = 4096;
const MAX_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const COMMAND_BUDGET: Duration = Duration::from_secs(15);

fn rejected(reason: impl Into<String>) -> io::Error {
    io::Error::other(reason.into())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Birth {
    pid: u32,
    ppid: u32,
    pgid: u32,
    effective_uid: u32,
    start_seconds: u64,
    start_microseconds: u64,
}
impl From<ProcessIdentity> for Birth {
    fn from(p: ProcessIdentity) -> Self {
        Self {
            pid: p.pid,
            ppid: p.ppid,
            pgid: p.pgid,
            effective_uid: p.effective_uid,
            start_seconds: p.start_seconds,
            start_microseconds: p.start_microseconds,
        }
    }
}
impl Birth {
    fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            ppid: self.ppid,
            pgid: self.pgid,
            effective_uid: self.effective_uid,
            start_seconds: self.start_seconds,
            start_microseconds: self.start_microseconds,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Object {
    path: PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    directory: bool,
    sha256: Option<String>,
}

fn snapshot(held: &ProtectedPath) -> io::Result<Object> {
    let before = held.file.metadata()?;
    let sha256 = if before.is_dir() {
        None
    } else {
        if before.len() > MAX_OBJECT_BYTES {
            return Err(rejected("protected object exceeds byte budget"));
        }
        let mut file = held.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut hash = sha2::Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut bytes = 0u64;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes = bytes
                .checked_add(n as u64)
                .ok_or_else(|| rejected("object size overflow"))?;
            if bytes > before.len() {
                return Err(rejected("protected object grew while hashing"));
            }
            hash.update(&buffer[..n]);
        }
        if bytes != before.len() {
            return Err(rejected("protected object changed size"));
        }
        Some(hex::encode(hash.finalize()))
    };
    let after = held.file.metadata()?;
    if (
        before.dev(),
        before.ino(),
        before.len(),
        before.mode(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.len(),
        after.mode(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err(rejected("protected object changed while hashing"));
    }
    held.unchanged()?;
    Ok(Object {
        path: held.path.clone(),
        device: before.dev(),
        inode: before.ino(),
        // Directory entry storage can change without affecting any bound object.
        size: if before.is_dir() { 0 } else { before.len() },
        mode: before.mode(),
        directory: before.is_dir(),
        sha256,
    })
}

struct Installation {
    paths: InstallPaths,
    support: ProtectedPath,
    helper: ProtectedPath,
    plist: ProtectedPath,
    record: Option<ProtectedPath>,
    core: Vec<ProtectedPath>,
}
impl Installation {
    fn open() -> io::Result<Self> {
        Self::open_with_core(true)
    }
    // Committed activation only validates the already committed fixed candidate
    // binding. Old payload/trash changes cannot authorize rollback at this point.
    fn open_with_core(include_core: bool) -> io::Result<Self> {
        let paths = InstallPaths::for_platform(Platform::Mac);
        let support_path = paths
            .core_dir
            .parent()
            .ok_or_else(|| rejected("missing fixed support path"))?;
        let support = ProtectedPath::open(support_path, true)?;
        let helper = ProtectedPath::open(&paths.binary, false)?;
        let plist = ProtectedPath::open(
            paths
                .descriptor
                .as_deref()
                .ok_or_else(|| rejected("missing fixed plist path"))?,
            false,
        )?;
        let record = optional_protected(&support.path.join("helper-installed.json"))?;
        let mut core = Vec::new();
        let mut bytes = 0;
        if include_core {
            collect_core(&paths.core_dir, 0, &mut bytes, &mut core)?;
            if !core
                .iter()
                .any(|p| p.file.metadata().is_ok_and(|m| m.is_file()))
            {
                return Err(rejected("empty managed runtime cohort"));
            }
        }
        Ok(Self {
            paths,
            support,
            helper,
            plist,
            record,
            core,
        })
    }
    fn expected(&self) -> Expected<'_> {
        Expected {
            label: self.paths.service_label,
            plist_path: &self.plist.path,
            helper_path: &self.helper.path,
            support_path: &self.support.path,
            core_dir: &self.paths.core_dir,
        }
    }
    fn facts(&self) -> io::Result<PlistFacts> {
        launchd::read_protected_plist(&self.plist, &self.expected())
    }
    fn objects(&self) -> io::Result<Vec<Object>> {
        std::iter::once(&self.support)
            .chain([&self.helper, &self.plist])
            .chain(self.record.iter())
            .chain(&self.core)
            .map(snapshot)
            .collect()
    }
    fn executables(&self) -> io::Result<(HeldExecutable, Vec<HeldExecutable>)> {
        let helper = HeldExecutable {
            path: self.helper.path.clone(),
            file: self.helper.file.try_clone()?,
        };
        let mut core = Vec::new();
        for held in &self.core {
            if held.file.metadata()?.is_file() {
                core.push(HeldExecutable {
                    path: held.path.clone(),
                    file: held.file.try_clone()?,
                });
            }
        }
        Ok((helper, core))
    }
    fn unchanged(&self, original: &[Object]) -> io::Result<()> {
        if self.objects()? != original {
            return Err(rejected("original installation object changed"));
        }
        // Re-enumerate all names, including .trash/.staging and nested versions.
        // Hashing held files alone cannot prove that no new image was installed.
        let fresh = Self::open()?;
        if fresh.objects()? != original {
            return Err(rejected("managed installation tree changed"));
        }
        Ok(())
    }
}

fn collect_core(
    path: &Path,
    depth: usize,
    bytes: &mut u64,
    held: &mut Vec<ProtectedPath>,
) -> io::Result<()> {
    if depth > 12 || held.len() >= MAX_OBJECTS {
        return Err(rejected("managed tree exceeds enumeration budget"));
    }
    let directory = ProtectedPath::open(path, true)?;
    let mut source = PayloadDir::open(path, Some(0))?;
    let mut names = source.entry_names(MAX_OBJECTS - held.len())?;
    names.sort();
    directory.unchanged()?;
    held.push(directory);
    for name in names {
        if held.len() >= MAX_OBJECTS {
            return Err(rejected("managed tree exceeds object budget"));
        }
        let name = name
            .to_str()
            .ok_or_else(|| rejected("unrepresentable managed filename"))?;
        let child = path.join(name);
        let metadata = fs::symlink_metadata(&child)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_core(&child, depth + 1, bytes, held)?;
        } else {
            // Every regular object is a potential image, irrespective of name,
            // execute-bit changes or whether its version is currently selected.
            let (relative, _) = source.open_regular(name)?;
            let protected = ProtectedPath::open(&child, false)?;
            let actual = relative.metadata()?;
            let named = protected.file.metadata()?;
            if (actual.dev(), actual.ino()) != (named.dev(), named.ino()) {
                return Err(rejected("managed file changed during enumeration"));
            }
            *bytes = bytes
                .checked_add(actual.len())
                .ok_or_else(|| rejected("managed byte budget overflow"))?;
            if actual.len() > MAX_OBJECT_BYTES || *bytes > MAX_TREE_BYTES {
                return Err(rejected("managed files exceed byte budget"));
            }
            held.push(protected);
        }
    }
    Ok(())
}

struct Journal {
    held: ProtectedPath,
    lease: ProtectedPath,
    id: String,
}
impl Journal {
    fn open(path: &Path) -> io::Result<Self> {
        let paths = InstallPaths::for_platform(Platform::Mac);
        let support = paths
            .core_dir
            .parent()
            .ok_or_else(|| rejected("missing support path"))?;
        let id = path
            .file_name()
            .and_then(|p| p.to_str())
            .ok_or_else(|| rejected("invalid journal ID"))?;
        let suffix = id
            .strip_prefix("helper-upgrade.")
            .ok_or_else(|| rejected("not an existing upgrade journal"))?;
        if path.parent() != Some(support)
            || suffix.is_empty()
            || suffix.len() > 64
            || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(rejected("journal is outside fixed recovery namespace"));
        }
        let held = ProtectedPath::open(path, true)?;
        private(&held)?;
        let lease = ProtectedPath::open(&support.join("helper-install.lease"), false)?;
        private(&lease)?;
        // This reuses fd 9's inherited open-file description. Opening a new
        // descriptor and acquiring another lock is deliberately not a fallback.
        crate::process_lease::apply(&lease.path, 9, false)?;
        let value = Self {
            held,
            lease,
            id: id.to_owned(),
        };
        if value.read("transaction-kind", 128)? != b"polaris-helper-upgrade-v1\n"
            || value.read("transaction-destination", 1024)?
                != paths.binary.as_os_str().as_encoded_bytes()
        {
            return Err(rejected("journal transaction binding differs"));
        }
        Ok(value)
    }
    fn read(&self, name: &str, limit: u64) -> io::Result<Vec<u8>> {
        self.held.unchanged()?;
        let guard = ProtectedPath::open(&self.held.path.join(name), false)?;
        private(&guard)?;
        let objects = PayloadDir::open(&self.held.path, Some(0))?;
        let (file, size) = objects.open_regular(name)?;
        if size > limit {
            return Err(rejected("journal object exceeds budget"));
        }
        let actual = file.metadata()?;
        let expected = guard.file.metadata()?;
        if (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
            return Err(rejected("journal read identity differs"));
        }
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != size {
            return Err(rejected("journal object changed while reading"));
        }
        guard.unchanged()?;
        self.held.unchanged()?;
        Ok(bytes)
    }
    fn write_new<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        self.held.unchanged()?;
        let output = serde_json::to_vec(value).map_err(rejected_json)?;
        self.write_bytes_new(name, &output)
    }
    fn write_bytes_new(&self, name: &str, output: &[u8]) -> io::Result<()> {
        self.held.unchanged()?;
        if output.len() as u64 > MAX_RECEIPT {
            return Err(rejected("receipt exceeds budget"));
        }
        let temporary = self.held.path.join(format!(".{name}.next"));
        let destination = self.held.path.join(name);
        // Do not overwrite a previous stop attempt or infer that it completed.
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => return Err(rejected("receipt already exists; manual recovery required")),
            Err(error) => return Err(error),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(&output)?;
        file.sync_all()?;
        let temporary_guard = ProtectedPath::open(&temporary, false)?;
        private(&temporary_guard)?;
        self.held.unchanged()?;
        fs::rename(&temporary, &destination)?;
        self.held.file.sync_all()?;
        self.held.unchanged()?;
        Ok(())
    }
    fn check_lease(&self) -> io::Result<(u64, u64)> {
        self.lease.unchanged()?;
        crate::process_lease::apply(&self.lease.path, 9, false)?;
        let value = self.lease.file.metadata()?;
        Ok((value.dev(), value.ino()))
    }
}
fn rejected_json(error: serde_json::Error) -> io::Error {
    rejected(error.to_string())
}
fn private(held: &ProtectedPath) -> io::Result<()> {
    let m = held.file.metadata()?;
    if m.uid() != 0 || m.mode() & 0o777 != if m.is_dir() { 0o700 } else { 0o600 } {
        return Err(rejected("journal object is not private root-owned storage"));
    }
    #[cfg(target_os = "macos")]
    crate::protected_install_path::no_extended_acl(&held.file)?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    device: u64,
    inode: u64,
    owner: u32,
    mode: u32,
}
fn endpoint_identity(path: &Path) -> io::Result<Option<Endpoint>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() && m.uid() == 0 => Ok(Some(Endpoint {
            device: m.dev(),
            inode: m.ino(),
            owner: m.uid(),
            mode: m.mode(),
        })),
        Ok(_) => Err(rejected("fixed helper endpoint is not a root-owned socket")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

enum EndpointClosure {
    Closed(Option<Endpoint>),
    Busy,
}

#[cfg(target_os = "macos")]
fn endpoint_probe(path: &Path) -> io::Result<EndpointClosure> {
    use nix::sys::socket::{connect, socket, AddressFamily, SockFlag, SockType, UnixAddr};
    use std::os::fd::AsRawFd;
    let before = endpoint_identity(path)?;
    let fd = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
        None,
    )?;
    let address = UnixAddr::new(path)?;
    match connect(fd.as_raw_fd(), &address) {
        Err(nix::errno::Errno::ENOENT | nix::errno::Errno::ECONNREFUSED) => {}
        Ok(()) => return Ok(EndpointClosure::Busy),
        Err(e) => return Err(rejected(format!("endpoint closure is unknown: {e}"))),
    }
    let after = endpoint_identity(path)?;
    if before != after {
        return Err(rejected(
            "helper endpoint identity changed during closure probe",
        ));
    }
    Ok(EndpointClosure::Closed(after))
}
#[cfg(not(target_os = "macos"))]
fn endpoint_probe(_path: &Path) -> io::Result<EndpointClosure> {
    Err(rejected("macOS endpoint probe required"))
}
fn endpoint_closed(path: &Path) -> io::Result<Option<Endpoint>> {
    match endpoint_probe(path)? {
        EndpointClosure::Closed(value) => Ok(value),
        EndpointClosure::Busy => Err(rejected("helper endpoint still accepts connections")),
    }
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "read-only bounded kernel boot-session UUID query"
)]
fn boot_session() -> io::Result<String> {
    let mut bytes = [0u8; 37];
    let mut length = bytes.len();
    // SAFETY: constant NUL-terminated key, exact writable capacity, no writes to kernel state.
    let rc = unsafe {
        nix::libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length != bytes.len()
        || bytes[36] != 0
        || bytes[..36].iter().enumerate().any(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b != b'-'
            } else {
                !b.is_ascii_hexdigit()
            }
        })
    {
        return Err(rejected("invalid kernel boot-session UUID"));
    }
    String::from_utf8(bytes[..36].to_vec()).map_err(|_| rejected("invalid boot-session encoding"))
}
#[cfg(not(target_os = "macos"))]
fn boot_session() -> io::Result<String> {
    Err(rejected("macOS boot session required"))
}

fn shell_birth() -> io::Result<Birth> {
    let pid = u32::try_from(nix::unistd::getppid().as_raw())
        .map_err(|_| rejected("invalid parent PID"))?;
    let value = observer::live_identity(pid)
        .map_err(|e| rejected(format!("parent identity unknown: {e:?}")))?;
    if value.effective_uid != 0 {
        return Err(rejected("root administrative shell required"));
    }
    Ok(value.into())
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str], timeout: Duration) -> io::Result<String> {
    use polaris_system_integration::exec::{Command, CommandCleanup, StdCommandRunner};
    let mut observed = StdCommandRunner.run_observed(
        &Command::new("/bin/launchctl", args.iter().copied()),
        timeout,
    );
    if let Some(pending) = observed.pending_mut() {
        pending.poll_cleanup(Duration::from_secs(2));
    }
    if observed.cleanup() != CommandCleanup::NativeAndReadersReturned {
        return Err(rejected(
            "launchctl cleanup unknown; recovery journal must be preserved",
        ));
    }
    let output = observed
        .operation()
        .ok_or_else(|| rejected("launchctl result unknown"))?
        .map_err(rejected)?;
    if !output.stderr.is_empty() || output.stdout.len() > 2 * 1024 * 1024 {
        return Err(rejected("launchctl output incomplete or unexpected stderr"));
    }
    Ok(output.stdout)
}
#[cfg(not(target_os = "macos"))]
fn launchctl(_args: &[&str], _timeout: Duration) -> io::Result<String> {
    Err(rejected("macOS launchctl required"))
}

fn disabled(label: &str) -> io::Result<bool> {
    launchd::parse_disabled_print(
        &launchctl(&["print-disabled", "system"], COMMAND_BUDGET)?,
        label,
    )
}
fn service_absent(label: &str) -> io::Result<()> {
    // A failed job-specific print is NEVER evidence of absence. Only successful,
    // complete system-domain output with the supported services table qualifies.
    if !launchd::parse_system_services_absent(
        &launchctl(&["print", "system"], COMMAND_BUDGET)?,
        label,
    )? {
        return Err(rejected("launchd service still present"));
    }
    Ok(())
}
fn fenced(label: &str) -> io::Result<()> {
    if !disabled(label)? {
        return Err(rejected("launchd relaunch fence is not disabled"));
    }
    service_absent(label)
}
fn wait_closed(
    label: &str,
    socket: &Path,
    deadline: Instant,
    mut observe: impl FnMut() -> Closure,
) -> io::Result<Option<Endpoint>> {
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| rejected("normal stop deadline expired; closure unproven"))
    };
    loop {
        let disabled = launchd::parse_disabled_print(
            &launchctl(
                &["print-disabled", "system"],
                remaining()?.min(COMMAND_BUDGET),
            )?,
            label,
        )?;
        if !disabled {
            return Err(rejected(
                "normal stop lost its explicit launchd relaunch fence",
            ));
        }
        let absent = launchd::parse_system_services_absent(
            &launchctl(&["print", "system"], remaining()?.min(COMMAND_BUDGET))?,
            label,
        )?;
        if absent {
            match observe() {
                Closure::Clear => {
                    remaining()?;
                    if let EndpointClosure::Closed(endpoint) = endpoint_probe(socket)? {
                        remaining()?;
                        return Ok(endpoint);
                    }
                }
                Closure::Busy(_) => {}
                Closure::Unknown(reason) => {
                    return Err(rejected(format!("normal stop census unknown: {reason:?}")));
                }
            }
        }
        std::thread::sleep(remaining()?.min(Duration::from_millis(100)));
    }
}

fn clear(value: Closure) -> io::Result<()> {
    match value {
        Closure::Clear => Ok(()),
        other => Err(rejected(format!(
            "configured runtime closure unproven: {other:?}"
        ))),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Configuration {
    label: String,
    program: PathBuf,
    arguments: Vec<String>,
    support_path: PathBuf,
    core_dir: PathBuf,
    singbox: Option<PathBuf>,
    confdir: Option<PathBuf>,
    upgrade_closed: bool,
    declared_exit_timeout_seconds: Option<u32>,
    actual_exit_timeout_seconds: u32,
    keep_alive: bool,
    run_at_load: bool,
    inferred_program: bool,
}
impl Configuration {
    fn new(p: &PlistFacts, actual_exit_timeout_seconds: u32) -> Self {
        Self {
            label: p.label.clone(),
            program: p.program.clone(),
            arguments: p.arguments.clone(),
            support_path: p.support_path.clone(),
            core_dir: p.core_dir.clone(),
            singbox: p.singbox.clone(),
            confdir: p.confdir.clone(),
            upgrade_closed: p.upgrade_closed,
            declared_exit_timeout_seconds: p.exit_timeout_seconds,
            actual_exit_timeout_seconds,
            keep_alive: p.keep_alive,
            run_at_load: p.run_at_load,
            inferred_program: p.inferred_program,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StopReceipt {
    schema: String,
    journal_path: PathBuf,
    transaction_id: String,
    journal_device: u64,
    journal_inode: u64,
    boot_session: String,
    shell: Birth,
    lease_device: u64,
    lease_inode: u64,
    original_enabled: bool,
    configuration: Configuration,
    objects: Vec<Object>,
    helper: Birth,
    descendants: Vec<Birth>,
    managed_runtimes: Vec<Birth>,
    group_members: Vec<Birth>,
    completed: bool,
    disabled: bool,
    job_absent: bool,
    census_closed: bool,
    endpoint_closed: bool,
    endpoint: Option<Endpoint>,
}

impl StopReceipt {
    fn identities(&self) -> observer::ReceiptIdentities {
        observer::ReceiptIdentities {
            helper: self.helper.identity(),
            descendants: self.descendants.iter().map(Birth::identity).collect(),
            managed_runtimes: self.managed_runtimes.iter().map(Birth::identity).collect(),
            group_members: self.group_members.iter().map(Birth::identity).collect(),
        }
    }
}

fn validate_rename_devices(journal_device: u64, source_devices: &[u64]) -> io::Result<()> {
    if source_devices
        .iter()
        .any(|device| *device != journal_device)
    {
        return Err(rejected(
            "original helper/plist/record and journal are on different devices; inode-preserving rename required",
        ));
    }
    Ok(())
}

fn stop(journal: &Journal) -> io::Result<()> {
    // Establish all read-only facts and hold every guard before the first mutation.
    let installation = Installation::open()?;
    let mut rename_devices = vec![
        installation.helper.file.metadata()?.dev(),
        installation.plist.file.metadata()?.dev(),
    ];
    if let Some(record) = &installation.record {
        rename_devices.push(record.file.metadata()?.dev());
    }
    validate_rename_devices(journal.held.file.metadata()?.dev(), &rename_devices)?;
    let facts = installation.facts()?;
    let domain = format!("system/{}", installation.paths.service_label);
    let loaded_print = launchctl(&["print", &domain], COMMAND_BUDGET)?;
    let loaded = launchd::validate_loaded_print(&loaded_print, &facts, &installation.expected())?;
    let originally_disabled = launchd::parse_disabled_state(
        &launchctl(&["print-disabled", "system"], COMMAND_BUDGET)?,
        installation.paths.service_label,
    )?
    .unwrap_or(false);
    let objects = installation.objects()?;
    let (helper, managed) = installation.executables()?;
    let target = Target {
        helper_pid: loaded.pid,
        helper: &helper,
        managed_executables: &managed,
    };
    let captured = match observer::capture(target) {
        Preflight::Ready(value) => value,
        other => return Err(rejected(format!("migration preflight unproven: {other:?}"))),
    };
    installation.unchanged(&objects)?;
    let current_print = launchctl(&["print", &domain], COMMAND_BUDGET)?;
    if launchd::validate_loaded_print(&current_print, &facts, &installation.expected())? != loaded
        || launchd::parse_disabled_state(
            &launchctl(&["print-disabled", "system"], COMMAND_BUDGET)?,
            installation.paths.service_label,
        )?
        .unwrap_or(false)
            != originally_disabled
    {
        return Err(rejected("loaded job changed during preflight"));
    }
    if observer::live_identity(loaded.pid)
        .map_err(|e| rejected(format!("helper identity unknown: {e:?}")))?
        != captured.helper_identity()
    {
        return Err(rejected("loaded helper birth changed before stop"));
    }
    let (lease_device, lease_inode) = journal.check_lease()?;
    let metadata = journal.held.file.metadata()?;
    let mut receipt = StopReceipt {
        schema: "polaris-mac-migration-stop-v1".into(),
        journal_path: journal.held.path.clone(),
        transaction_id: journal.id.clone(),
        journal_device: metadata.dev(),
        journal_inode: metadata.ino(),
        boot_session: boot_session()?,
        shell: shell_birth()?,
        lease_device,
        lease_inode,
        original_enabled: !originally_disabled,
        configuration: Configuration::new(&facts, loaded.exit_timeout_seconds),
        objects,
        helper: captured.helper_identity().into(),
        descendants: captured
            .descendants()
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        managed_runtimes: captured
            .managed_runtimes()
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        group_members: captured
            .group_members()
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        completed: false,
        disabled: false,
        job_absent: false,
        census_closed: false,
        endpoint_closed: false,
        endpoint: None,
    };
    journal.write_new(STARTED, &receipt)?;
    journal.write_bytes_new(
        "migration-original-disabled",
        if originally_disabled {
            b"true\n"
        } else {
            b"false\n"
        },
    )?;
    // From this durable boundary, failure grants no rollback or restart authority.
    launchctl(&["disable", &domain], COMMAND_BUDGET)?;
    if !disabled(installation.paths.service_label)? {
        return Err(rejected("disable was not observed"));
    }
    // launchd owns the original job's normal termination. Never signal a census PID.
    let timeout = Duration::from_secs(
        u64::from(loaded.exit_timeout_seconds)
            .saturating_add(15)
            .min(315),
    );
    let deadline = Instant::now() + timeout;
    launchctl(&["bootout", &domain], timeout)?;
    let endpoint = wait_closed(
        installation.paths.service_label,
        &installation.paths.socket,
        deadline,
        || observer::observe_closed(&captured),
    )?;
    installation.unchanged(&receipt.objects)?;
    fenced(installation.paths.service_label)?;
    clear(observer::observe_closed(&captured))?;
    if endpoint_closed(&installation.paths.socket)? != endpoint {
        return Err(rejected("endpoint changed between closure observations"));
    }
    verify_binding(journal, &receipt)?;
    receipt.completed = true;
    receipt.disabled = true;
    receipt.job_absent = true;
    receipt.census_closed = true;
    receipt.endpoint_closed = true;
    receipt.endpoint = endpoint;
    journal.write_new(COMPLETE, &receipt)
}

fn verify_binding(journal: &Journal, receipt: &StopReceipt) -> io::Result<()> {
    let j = journal.held.file.metadata()?;
    let lease = journal.check_lease()?;
    if receipt.schema != "polaris-mac-migration-stop-v1"
        || receipt.journal_path != journal.held.path
        || receipt.transaction_id != journal.id
        || (receipt.journal_device, receipt.journal_inode) != (j.dev(), j.ino())
        || (receipt.lease_device, receipt.lease_inode) != lease
        || receipt.boot_session != boot_session()?
        || receipt.shell != shell_birth()?
    {
        return Err(rejected(
            "stop receipt belongs to another transaction, shell, boot or lease",
        ));
    }
    Ok(())
}
fn validate_stop_pair(started: &StopReceipt, receipt: &StopReceipt) -> io::Result<()> {
    if !receipt.completed
        || !receipt.disabled
        || !receipt.job_absent
        || !receipt.census_closed
        || !receipt.endpoint_closed
    {
        return Err(rejected("stop receipt has no completed closure facts"));
    }
    let mut normalized = receipt.clone();
    normalized.completed = false;
    normalized.disabled = false;
    normalized.job_absent = false;
    normalized.census_closed = false;
    normalized.endpoint_closed = false;
    normalized.endpoint = None;
    if &normalized != started {
        return Err(rejected(
            "stop receipts do not describe the same original capture",
        ));
    }
    Ok(())
}
fn completed_stop(journal: &Journal) -> io::Result<StopReceipt> {
    let started: StopReceipt =
        serde_json::from_slice(&journal.read(STARTED, MAX_RECEIPT)?).map_err(rejected_json)?;
    let receipt: StopReceipt =
        serde_json::from_slice(&journal.read(COMPLETE, MAX_RECEIPT)?).map_err(rejected_json)?;
    validate_stop_pair(&started, &receipt)?;
    let disabled_bytes: &[u8] = if receipt.original_enabled {
        b"false\n"
    } else {
        b"true\n"
    };
    if journal.read("migration-original-disabled", 16)? != disabled_bytes {
        return Err(rejected(
            "original disabled-state file differs from stop receipt",
        ));
    }
    verify_binding(journal, &receipt)?;
    Ok(receipt)
}
fn recheck(journal: &Journal) -> io::Result<()> {
    let receipt = completed_stop(journal)?;
    let installation = Installation::open()?;
    installation.unchanged(&receipt.objects)?;
    let facts = installation.facts()?;
    if Configuration::new(&facts, receipt.configuration.actual_exit_timeout_seconds)
        != receipt.configuration
    {
        return Err(rejected("receipt configured argv or service facts differ"));
    }
    let (helper, managed) = installation.executables()?;
    let target = Target {
        helper_pid: receipt.helper.pid,
        helper: &helper,
        managed_executables: &managed,
    };
    fenced(installation.paths.service_label)?;
    clear(observer::recheck_verified_receipt(
        target,
        &receipt.identities(),
    ))?;
    if endpoint_closed(&installation.paths.socket)? != receipt.endpoint {
        return Err(rejected("endpoint differs from the completed stop receipt"));
    }
    installation.unchanged(&receipt.objects)?;
    fenced(installation.paths.service_label)?;
    verify_binding(journal, &receipt)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CandidateReceipt {
    schema: String,
    journal_path: PathBuf,
    transaction_id: String,
    journal_device: u64,
    journal_inode: u64,
    boot_session: String,
    shell: Birth,
    lease_device: u64,
    lease_inode: u64,
    core_id: String,
    sealed_utility: Object,
    helper: Object,
    installed_plist: Object,
    closed_descriptor: Object,
    plain_descriptor: Object,
    configuration: Configuration,
    birth: Birth,
    descendants: Vec<Birth>,
    managed_runtimes: Vec<Birth>,
    group_members: Vec<Birth>,
    admission_closed: bool,
    slot_empty: bool,
}

fn read_journal_plist(held: &ProtectedPath, fixed: &Expected<'_>) -> io::Result<PlistFacts> {
    // The bytes live in the journal, but argv still points exclusively to the
    // fixed installed objects. Loaded-job validation uses `fixed` unchanged.
    launchd::read_protected_plist(held, &journal_expected(&held.path, fixed))
}

fn journal_expected<'a>(path: &'a Path, fixed: &Expected<'a>) -> Expected<'a> {
    Expected {
        label: fixed.label,
        plist_path: path,
        helper_path: fixed.helper_path,
        support_path: fixed.support_path,
        core_dir: fixed.core_dir,
    }
}

fn require_empty_candidate_cohort(
    descendants: &[ProcessIdentity],
    managed: &[ProcessIdentity],
    group: &[ProcessIdentity],
) -> io::Result<()> {
    if !descendants.is_empty() || !managed.is_empty() || !group.is_empty() {
        return Err(rejected(
            "closed candidate has observed child, managed-runtime or process-group owners",
        ));
    }
    Ok(())
}

fn candidate(
    journal: &Journal,
    id: &polaris_helper_proto::core_manifest::CoreId,
    phase: &str,
) -> io::Result<()> {
    let active = phase == "active";
    let committed = phase == "committed";
    if !active && !committed {
        completed_stop(journal)?;
    }
    let transaction_phase = journal.read("transaction-phase", 128)?;
    if (active || committed)
        && !matches!(
            transaction_phase.as_slice(),
            b"committed-closed\n" | b"activation-pending\n"
        )
    {
        return Err(rejected(
            "candidate activation requires a durable committed phase",
        ));
    }
    if journal.read("migration-core-id", 256)? != format!("{}\n", id.as_str()).as_bytes() {
        return Err(rejected("candidate core ID differs from journal"));
    }
    let installation = Installation::open_with_core(!committed)?;
    let utility = ProtectedPath::open(&journal.held.path.join("helper"), false)?;
    let sealed = snapshot(&utility)?;
    let helper_object = snapshot(&installation.helper)?;
    if sealed.sha256 != helper_object.sha256 || sealed.size != helper_object.size {
        return Err(rejected(
            "installed candidate differs from sealed journal helper",
        ));
    }
    let closed_guard = ProtectedPath::open(&journal.held.path.join("descriptor.closed"), false)?;
    let plain_guard = ProtectedPath::open(&journal.held.path.join("descriptor"), false)?;
    let closed = read_journal_plist(&closed_guard, &installation.expected())?;
    let plain = read_journal_plist(&plain_guard, &installation.expected())?;
    if !closed.upgrade_closed || plain.upgrade_closed {
        return Err(rejected(
            "candidate closed/plain descriptors do not establish intended admission modes",
        ));
    }
    let mut without_closed = closed.clone();
    without_closed
        .arguments
        .retain(|argument| argument != "--upgrade-closed");
    without_closed.upgrade_closed = false;
    if without_closed != plain {
        return Err(rejected(
            "candidate descriptors differ beyond the admission flag",
        ));
    }
    let closed_object = snapshot(&closed_guard)?;
    let plain_object = snapshot(&plain_guard)?;
    let installed_plist = snapshot(&installation.plist)?;
    let required = if active {
        &plain_object
    } else {
        &closed_object
    };
    let matches_required =
        installed_plist.sha256 == required.sha256 && installed_plist.size == required.size;
    let matches_plain =
        installed_plist.sha256 == plain_object.sha256 && installed_plist.size == plain_object.size;
    if !matches_required && !(committed && matches_plain) {
        return Err(rejected(
            "installed candidate descriptor differs from journal phase",
        ));
    }
    if active || committed {
        let receipt: CandidateReceipt =
            serde_json::from_slice(&journal.read(CANDIDATE, MAX_RECEIPT)?)
                .map_err(rejected_json)?;
        let metadata = journal.held.file.metadata()?;
        if receipt.schema != "polaris-mac-migration-candidate-v1"
            || receipt.journal_path != journal.held.path
            || receipt.transaction_id != journal.id
            || (receipt.journal_device, receipt.journal_inode) != (metadata.dev(), metadata.ino())
            || receipt.core_id != id.as_str()
            || receipt.sealed_utility != sealed
            || receipt.helper != helper_object
            || receipt.closed_descriptor != closed_object
            || receipt.plain_descriptor != plain_object
            || receipt.configuration
                != Configuration::new(&closed, receipt.configuration.actual_exit_timeout_seconds)
            || !receipt.admission_closed
            || !receipt.slot_empty
        {
            return Err(rejected(
                "candidate activation lacks matching durable candidate identity and closed receipt",
            ));
        }
    }
    if committed {
        // This offline gate precedes every forward activation, including a new
        // shell or boot. It grants no old stop proof or rollback authority.
        installation.helper.unchanged()?;
        installation.plist.unchanged()?;
        utility.unchanged()?;
        closed_guard.unchanged()?;
        plain_guard.unchanged()?;
        journal.check_lease()?;
        return Ok(());
    }
    let domain = format!("system/{}", installation.paths.service_label);
    let print = launchctl(&["print", &domain], COMMAND_BUDGET)?;
    let (loaded, facts) =
        match launchd::validate_loaded_print(&print, &closed, &installation.expected()) {
            Ok(value) => (value, closed),
            Err(_) if active => (
                launchd::validate_loaded_print(&print, &plain, &installation.expected())?,
                plain,
            ),
            Err(error) => return Err(error),
        };
    let objects = installation.objects()?;
    let (helper, managed) = installation.executables()?;
    let target = Target {
        helper_pid: loaded.pid,
        helper: &helper,
        managed_executables: &managed,
    };
    let captured = match observer::capture(target) {
        Preflight::Ready(value) => value,
        other => return Err(rejected(format!("candidate identity unproven: {other:?}"))),
    };
    if !active {
        require_empty_candidate_cohort(
            captured.descendants(),
            captured.managed_runtimes(),
            captured.group_members(),
        )?;
    }
    crate::admin_probe::retained_ready(&installation.support.path, id)?;
    if !active {
        crate::admin_probe::upgrade_admission(&installation.support.path, true)?;
        if crate::admin_probe::probe_slot(&installation.support.path)?
            != crate::admin_probe::Slot::Empty
        {
            return Err(rejected(
                "candidate admission has no exact native empty-slot observation",
            ));
        }
    }
    installation.unchanged(&objects)?;
    utility.unchanged()?;
    closed_guard.unchanged()?;
    plain_guard.unchanged()?;
    if snapshot(&utility)? != sealed
        || snapshot(&closed_guard)? != closed_object
        || snapshot(&plain_guard)? != plain_object
    {
        return Err(rejected(
            "candidate journal artifacts changed during validation",
        ));
    }
    let after_print = launchctl(&["print", &domain], COMMAND_BUDGET)?;
    if launchd::validate_loaded_print(&after_print, &facts, &installation.expected())? != loaded {
        return Err(rejected("candidate loaded job changed during validation"));
    }
    match observer::capture(target) {
        Preflight::Ready(after) if after.helper_identity() == captured.helper_identity() => {
            if !active {
                require_empty_candidate_cohort(
                    after.descendants(),
                    after.managed_runtimes(),
                    after.group_members(),
                )?;
            }
        }
        _ => {
            return Err(rejected(
                "candidate process birth changed during validation",
            ));
        }
    }
    if !active {
        let (lease_device, lease_inode) = journal.check_lease()?;
        let metadata = journal.held.file.metadata()?;
        let receipt = CandidateReceipt {
            schema: "polaris-mac-migration-candidate-v1".into(),
            journal_path: journal.held.path.clone(),
            transaction_id: journal.id.clone(),
            journal_device: metadata.dev(),
            journal_inode: metadata.ino(),
            boot_session: boot_session()?,
            shell: shell_birth()?,
            lease_device,
            lease_inode,
            core_id: id.as_str().into(),
            sealed_utility: sealed,
            helper: helper_object,
            installed_plist,
            closed_descriptor: closed_object,
            plain_descriptor: plain_object,
            configuration: Configuration::new(&facts, loaded.exit_timeout_seconds),
            birth: captured.helper_identity().into(),
            descendants: captured
                .descendants()
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            managed_runtimes: captured
                .managed_runtimes()
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            group_members: captured
                .group_members()
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            admission_closed: true,
            slot_empty: true,
        };
        journal.write_new(CANDIDATE, &receipt)?;
    }
    journal.check_lease()?;
    Ok(())
}

// Original inode restoration is permitted only after a second complete stop
// proof. A failed candidate-stop receipt is deliberately not reusable.
fn optional_protected(path: &Path) -> io::Result<Option<ProtectedPath>> {
    match fs::symlink_metadata(path) {
        Ok(_) => ProtectedPath::open(path, false).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn same_original(held: &ProtectedPath, original: &Object) -> io::Result<bool> {
    let mut actual = snapshot(held)?;
    actual.path = original.path.clone();
    Ok(actual == *original)
}

fn rollback(journal: &Journal) -> io::Result<()> {
    let phase = String::from_utf8(journal.read("transaction-phase", 128)?)
        .map_err(|_| rejected("invalid transaction phase"))?;
    if !matches!(
        phase.as_str(),
        "legacy-stop-proved\n" | "service-mutation\n" | "registration\n" | "candidate-starting\n"
    ) {
        return Err(rejected(
            "only a recognized precommit phase can request rollback",
        ));
    }
    let original = completed_stop(journal)?;
    let paths = InstallPaths::for_platform(Platform::Mac);
    let descriptor = paths
        .descriptor
        .as_deref()
        .ok_or_else(|| rejected("missing fixed descriptor"))?;
    let support = paths
        .core_dir
        .parent()
        .ok_or_else(|| rejected("missing fixed support"))?;
    let expected = Expected {
        label: paths.service_label,
        plist_path: descriptor,
        helper_path: &paths.binary,
        support_path: support,
        core_dir: &paths.core_dir,
    };
    let mut originals = Vec::new();
    let mut original_helper = None;
    for object in &original.objects {
        let backup = if object.path == paths.binary {
            Some("helper.original")
        } else if object.path == descriptor {
            Some("descriptor.original")
        } else if object.path == support.join("helper-installed.json") {
            Some("record.original")
        } else {
            None
        };
        let held = if let Some(backup) = backup {
            let primary = optional_protected(&object.path)?;
            let saved = optional_protected(&journal.held.path.join(backup))?;
            let primary_matches = primary
                .as_ref()
                .map(|p| same_original(p, object))
                .transpose()?
                .unwrap_or(false);
            let saved_matches = saved
                .as_ref()
                .map(|p| same_original(p, object))
                .transpose()?
                .unwrap_or(false);
            match (primary_matches, saved_matches) {
                (true, false) => primary.ok_or_else(|| rejected("original primary disappeared"))?,
                (false, true) => saved.ok_or_else(|| rejected("original backup disappeared"))?,
                _ => {
                    return Err(rejected(
                        "original inode is not uniquely bound to its approved original/backup path",
                    ));
                }
            }
        } else {
            let held = ProtectedPath::open(&object.path, object.directory)?;
            if !same_original(&held, object)? {
                return Err(rejected(
                    "original core/support binding changed before rollback",
                ));
            }
            held
        };
        if object.path == paths.binary {
            original_helper = Some(originals.len());
        }
        originals.push(held);
    }
    let original_helper_index =
        original_helper.ok_or_else(|| rejected("missing original helper object"))?;
    let mut tree = Vec::new();
    let mut bytes = 0;
    collect_core(&paths.core_dir, 0, &mut bytes, &mut tree)?;
    let tree_objects: Vec<_> = tree.iter().map(snapshot).collect::<io::Result<_>>()?;
    let expected_tree: Vec<_> = original
        .objects
        .iter()
        .filter(|p| p.path.starts_with(&paths.core_dir))
        .cloned()
        .collect();
    if tree_objects != expected_tree {
        return Err(rejected("managed tree membership changed before rollback"));
    }
    let utility = ProtectedPath::open(&journal.held.path.join("helper"), false)?;
    let sealed = snapshot(&utility)?;
    let current_helper = optional_protected(&paths.binary)?;
    let current_is_original = current_helper
        .as_ref()
        .map(|p| same_original(p, &original.objects[original_helper_index]))
        .transpose()?
        .unwrap_or(false);
    if let Some(current) = &current_helper {
        let actual = snapshot(current)?;
        if !current_is_original && (actual.sha256 != sealed.sha256 || actual.size != sealed.size) {
            return Err(rejected(
                "rollback target is neither original inode nor sealed candidate",
            ));
        }
    }
    let current_descriptor = optional_protected(descriptor)?;
    let closed_guard = ProtectedPath::open(&journal.held.path.join("descriptor.closed"), false)?;
    let closed = read_journal_plist(&closed_guard, &expected)?;
    if !closed.upgrade_closed {
        return Err(rejected("rollback candidate has no closed admission argv"));
    }
    let closed_object = snapshot(&closed_guard)?;
    if let Some(current) = &current_descriptor {
        let actual = snapshot(current)?;
        let old = original
            .objects
            .iter()
            .find(|p| p.path == descriptor)
            .ok_or_else(|| rejected("missing original plist object"))?;
        if !same_original(current, old)?
            && (actual.sha256 != closed_object.sha256 || actual.size != closed_object.size)
        {
            return Err(rejected(
                "rollback descriptor is neither original inode nor closed candidate",
            ));
        }
    }
    let helper_original = HeldExecutable {
        path: originals[original_helper_index].path.clone(),
        file: originals[original_helper_index].file.try_clone()?,
    };
    let mut managed = Vec::new();
    for held in &tree {
        if held.file.metadata()?.is_file() {
            managed.push(HeldExecutable {
                path: held.path.clone(),
                file: held.file.try_clone()?,
            });
        }
    }
    if let Some(current) = &current_helper {
        if !current_is_original {
            managed.push(HeldExecutable {
                path: current.path.clone(),
                file: current.file.try_clone()?,
            });
        }
    }
    let old_target = Target {
        helper_pid: original.helper.pid,
        helper: &helper_original,
        managed_executables: &managed,
    };
    let absent = launchd::parse_system_services_absent(
        &launchctl(&["print", "system"], COMMAND_BUDGET)?,
        paths.service_label,
    )?;
    // The shell durably records candidate-starting before bootstrap. If a
    // candidate disappeared before we captured its births, don't invent an
    // empty cohort from the missing job. Preserve the transaction instead.
    let saved_candidate: Option<CandidateReceipt> =
        if absent && phase.trim_end() == "candidate-starting" {
            let receipt: CandidateReceipt =
                serde_json::from_slice(&journal.read(CANDIDATE, MAX_RECEIPT)?)
                    .map_err(rejected_json)?;
            let j = journal.held.file.metadata()?;
            let lease = journal.check_lease()?;
            let installed = current_helper
                .as_ref()
                .ok_or_else(|| rejected("saved candidate has no installed image"))?;
            if receipt.schema != "polaris-mac-migration-candidate-v1"
                || receipt.journal_path != journal.held.path
                || receipt.transaction_id != journal.id
                || (receipt.journal_device, receipt.journal_inode) != (j.dev(), j.ino())
                || receipt.boot_session != boot_session()?
                || receipt.shell != shell_birth()?
                || (receipt.lease_device, receipt.lease_inode) != lease
                || journal.read("migration-core-id", 256)?
                    != format!("{}\n", receipt.core_id).as_bytes()
                || receipt.sealed_utility != sealed
                || receipt.helper != snapshot(installed)?
                || receipt.closed_descriptor != closed_object
                || !receipt.admission_closed
                || !receipt.slot_empty
            {
                return Err(rejected(
                    "absent candidate has no same-transaction captured closure cohort",
                ));
            }
            Some(receipt)
        } else {
            None
        };
    let domain = format!("system/{}", paths.service_label);
    let candidate_helper = current_helper
        .as_ref()
        .filter(|_| !current_is_original)
        .map(|p| -> io::Result<HeldExecutable> {
            Ok(HeldExecutable {
                path: p.path.clone(),
                file: p.file.try_clone()?,
            })
        })
        .transpose()?;
    let mut candidate_managed = Vec::new();
    for p in &tree {
        if p.file.metadata()?.is_file() {
            candidate_managed.push(HeldExecutable {
                path: p.path.clone(),
                file: p.file.try_clone()?,
            });
        }
    }
    let captured;
    let mut stop_receipt = original.clone();
    stop_receipt.schema = "polaris-mac-migration-candidate-stop-v1".into();
    stop_receipt.completed = false;
    stop_receipt.disabled = false;
    stop_receipt.job_absent = false;
    stop_receipt.census_closed = false;
    stop_receipt.endpoint_closed = false;
    stop_receipt.endpoint = None;
    let mut timeout = COMMAND_BUDGET;
    if !absent {
        if current_is_original {
            return Err(rejected(
                "original helper unexpectedly relaunched; rollback denied",
            ));
        }
        let loaded = launchd::validate_loaded_print(
            &launchctl(&["print", &domain], COMMAND_BUDGET)?,
            &closed,
            &expected,
        )?;
        timeout = Duration::from_secs(
            u64::from(loaded.exit_timeout_seconds)
                .saturating_add(15)
                .min(315),
        );
        let target = Target {
            helper_pid: loaded.pid,
            helper: candidate_helper
                .as_ref()
                .ok_or_else(|| rejected("candidate helper image unavailable"))?,
            managed_executables: &candidate_managed,
        };
        let value = match observer::capture(target) {
            Preflight::Ready(value) => value,
            other => {
                return Err(rejected(format!(
                    "candidate rollback capture unproven: {other:?}"
                )));
            }
        };
        stop_receipt.helper = value.helper_identity().into();
        stop_receipt.descendants = value
            .descendants()
            .iter()
            .copied()
            .map(Into::into)
            .collect();
        stop_receipt.managed_runtimes = value
            .managed_runtimes()
            .iter()
            .copied()
            .map(Into::into)
            .collect();
        stop_receipt.group_members = value
            .group_members()
            .iter()
            .copied()
            .map(Into::into)
            .collect();
        stop_receipt.configuration = Configuration::new(&closed, loaded.exit_timeout_seconds);
        captured = Some(value);
    } else {
        captured = None;
        if let Some(saved) = &saved_candidate {
            stop_receipt.helper = saved.birth.clone();
            stop_receipt.descendants = saved.descendants.clone();
            stop_receipt.managed_runtimes = saved.managed_runtimes.clone();
            stop_receipt.group_members = saved.group_members.clone();
            stop_receipt.configuration = saved.configuration.clone();
        }
    }
    let original_locations: Vec<_> = originals.iter().map(snapshot).collect::<io::Result<_>>()?;
    let current_helper_object = current_helper.as_ref().map(snapshot).transpose()?;
    let current_descriptor_object = current_descriptor.as_ref().map(snapshot).transpose()?;
    stop_receipt.objects = original_locations.clone();
    stop_receipt.objects.extend(current_helper_object.clone());
    stop_receipt
        .objects
        .extend(current_descriptor_object.clone());
    verify_binding(journal, &original)?;
    journal.write_new("candidate-stop.started", &stop_receipt)?;
    launchctl(&["disable", &domain], COMMAND_BUDGET)?;
    if !disabled(paths.service_label)? {
        return Err(rejected("candidate rollback disable unobserved"));
    }
    let deadline = Instant::now() + timeout;
    if !absent {
        launchctl(&["bootout", &domain], timeout)?;
    }
    let endpoint = wait_closed(paths.service_label, &paths.socket, deadline, || {
        let saved_closure = saved_candidate.as_ref().map(|saved| {
            let Some(helper) = candidate_helper.as_ref() else {
                return Closure::Unknown(observer::UnknownReason {
                    stage: "rollback",
                    pid: Some(saved.birth.pid),
                    detail: "saved candidate image unavailable".into(),
                });
            };
            observer::recheck_verified_receipt(
                Target {
                    helper_pid: saved.birth.pid,
                    helper,
                    managed_executables: &candidate_managed,
                },
                &observer::ReceiptIdentities {
                    helper: saved.birth.identity(),
                    descendants: saved.descendants.iter().map(Birth::identity).collect(),
                    managed_runtimes: saved.managed_runtimes.iter().map(Birth::identity).collect(),
                    group_members: saved.group_members.iter().map(Birth::identity).collect(),
                },
            )
        });
        let mut busy = None;
        for result in [
            captured.as_ref().map(observer::observe_closed),
            saved_closure,
            Some(observer::recheck_verified_receipt(
                old_target,
                &original.identities(),
            )),
        ]
        .into_iter()
        .flatten()
        {
            match result {
                Closure::Clear => {}
                Closure::Busy(value) => busy = Some(Closure::Busy(value)),
                unknown @ Closure::Unknown(_) => return unknown,
            }
        }
        busy.unwrap_or(Closure::Clear)
    })?;
    if originals
        .iter()
        .map(snapshot)
        .collect::<io::Result<Vec<_>>>()?
        != original_locations
        || optional_protected(&paths.binary)?
            .as_ref()
            .map(snapshot)
            .transpose()?
            != current_helper_object
        || optional_protected(descriptor)?
            .as_ref()
            .map(snapshot)
            .transpose()?
            != current_descriptor_object
        || snapshot(&utility)? != sealed
        || snapshot(&closed_guard)? != closed_object
    {
        return Err(rejected(
            "rollback original/candidate objects changed during stop",
        ));
    }
    let mut final_tree = Vec::new();
    collect_core(&paths.core_dir, 0, &mut 0, &mut final_tree)?;
    if final_tree
        .iter()
        .map(snapshot)
        .collect::<io::Result<Vec<_>>>()?
        != expected_tree
    {
        return Err(rejected("rollback core tree changed during stop"));
    }
    fenced(paths.service_label)?;
    if let Some(captured) = &captured {
        clear(observer::observe_closed(captured))?;
    }
    if let Some(saved) = &saved_candidate {
        let target = Target {
            helper_pid: saved.birth.pid,
            helper: candidate_helper
                .as_ref()
                .ok_or_else(|| rejected("saved candidate image unavailable"))?,
            managed_executables: &candidate_managed,
        };
        clear(observer::recheck_verified_receipt(
            target,
            &observer::ReceiptIdentities {
                helper: saved.birth.identity(),
                descendants: saved.descendants.iter().map(Birth::identity).collect(),
                managed_runtimes: saved.managed_runtimes.iter().map(Birth::identity).collect(),
                group_members: saved.group_members.iter().map(Birth::identity).collect(),
            },
        ))?;
    }
    clear(observer::recheck_verified_receipt(
        old_target,
        &original.identities(),
    ))?;
    if endpoint_closed(&paths.socket)? != endpoint {
        return Err(rejected("rollback endpoint identity changed"));
    }
    verify_binding(journal, &original)?;
    stop_receipt.completed = true;
    stop_receipt.disabled = true;
    stop_receipt.job_absent = true;
    stop_receipt.census_closed = true;
    stop_receipt.endpoint_closed = true;
    stop_receipt.endpoint = endpoint;
    journal.write_new("candidate-stop.complete", &stop_receipt)
}

fn require_metadata_absent(result: io::Result<fs::Metadata>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(rejected("uninstall stop or retirement may have started")),
        Err(error) => Err(error),
    }
}

// A failed uninstall preflight may discard only its new journal, never a stop
// receipt. Exact NotFound under the validated held parent is positive absence.
fn uninstall_preflight_unchanged(journal: &Journal) -> io::Result<()> {
    if journal.read("maintenance-operation", 64)? != b"uninstall\n"
        || !matches!(
            journal.read("transaction-phase", 128)?.as_slice(),
            b"uninstall-staging\n" | b"uninstall-stop-started\n"
        )
    {
        return Err(rejected("not an unchanged uninstall preflight"));
    }
    for name in [
        STARTED,
        COMPLETE,
        "helper.original",
        "descriptor.original",
        "record.original",
    ] {
        journal.held.unchanged()?;
        require_metadata_absent(fs::symlink_metadata(journal.held.path.join(name)))?;
    }
    journal.held.unchanged()
}

/// Administrative arguments are consumed on every platform; never fall through
/// into daemon serving mode when a migration verb is unsupported or malformed.
pub fn run(args: &[String]) -> Option<std::process::ExitCode> {
    let verb = args.first()?.as_str();
    if !matches!(
        verb,
        "migration-stop"
            | "migration-recheck"
            | "migration-candidate"
            | "migration-rollback"
            | "migration-uninstall-preflight-unchanged"
    ) {
        return None;
    }
    let result = (|| {
        if !cfg!(target_os = "macos") || !nix::unistd::Uid::effective().is_root() {
            return Err(rejected("root macOS administrative invocation required"));
        }
        if args.len() < 3 || args[1] != "--journal" || args[2].contains(['\n', '\r', '\0']) {
            return Err(rejected("invalid migration administrative arguments"));
        }
        if verb == "migration-candidate" {
            if args.len() != 7
                || args[3] != "--id"
                || args[5] != "--phase"
                || !matches!(args[6].as_str(), "closed" | "committed" | "active")
            {
                return Err(rejected("invalid migration candidate arguments"));
            }
            let id =
                polaris_helper_proto::core_manifest::CoreId::parse(&args[4]).map_err(rejected)?;
            candidate(&Journal::open(Path::new(&args[2]))?, &id, &args[6])
        } else {
            if args.len() != 3 {
                return Err(rejected("extra migration arguments"));
            }
            let journal = Journal::open(Path::new(&args[2]))?;
            match verb {
                "migration-stop" => stop(&journal),
                "migration-recheck" => recheck(&journal),
                "migration-rollback" => rollback(&journal),
                "migration-uninstall-preflight-unchanged" => {
                    uninstall_preflight_unchanged(&journal)
                }
                _ => unreachable!(),
            }
        }
    })();
    Some(match result {
        Ok(()) => {
            if verb == "migration-candidate" {
                println!("migration-candidate-{}", args[6]);
            } else if verb == "migration-uninstall-preflight-unchanged" {
                println!("uninstall-preflight-unchanged");
            } else {
                println!("{verb}-closed");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "{verb}: {error}; preserve any started transaction; no rollback or restart authorized"
            );
            std::process::ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
mod tests;

//! Read-only, configured-runtime closure for the explicit macOS migration.
//!
//! This is NOT a historical descendant tracker, VM/file-descriptor inventory,
//! or permission to remove old payloads. The transaction must retain immutable
//! original inodes, hold its installer lease, validate launchd/configuration and
//! every protected ancestor, fence relaunch, and independently prove endpoint
//! closure. Original executable paths MUST stay bound to the held files until
//! this observation finishes. No service operation or signal occurs here.
//!
//! `proc_pidpath` reports the native executable vnode's path, not its inode.
//! The inode below belongs to that path's current object. Together with held,
//! unchanged protected paths this implements the deliberately narrower
//! configured-executable contract; it must not be described as a general
//! loaded-image inode attestation. Unresolvable images are never ignored.
//! A full BSD SZOMB record is terminal evidence only after the exact identity
//! is observed again inside stable PID-list boundaries; it needs no image path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_PROCESSES: usize = 131_072;
const MAX_ATTEMPTS: usize = 3;
const OBSERVATION_BUDGET: Duration = Duration::from_secs(5);

/// Descriptor and exact original path already validated by the transaction.
#[derive(Debug)]
pub struct HeldExecutable {
    pub path: PathBuf,
    pub file: File,
}

#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// PID from the transaction's verified, currently loaded launchd job.
    pub helper_pid: u32,
    pub helper: &'a HeldExecutable,
    pub managed_executables: &'a [HeldExecutable],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub effective_uid: u32,
    pub start_seconds: u64,
    pub start_microseconds: u64,
}

impl ProcessIdentity {
    fn same_birth(self, other: Self) -> bool {
        self.pid == other.pid
            && self.start_seconds == other.start_seconds
            && self.start_microseconds == other.start_microseconds
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownReason {
    pub stage: &'static str,
    pub pid: Option<u32>,
    pub detail: String,
}

impl UnknownReason {
    fn new(stage: &'static str, pid: Option<u32>, detail: impl Into<String>) -> Self {
        Self {
            stage,
            pid,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusyKind {
    ManagedExecutable,
    AdditionalHelper,
    OriginalHelper,
    CapturedDescendant,
    CapturedManagedRuntime,
    CapturedProcessGroupMember,
    OriginalProcessGroup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BusyReason {
    pub kind: BusyKind,
    pub process: ProcessIdentity,
    pub executable: PathBuf,
}

#[derive(Debug)]
pub enum Preflight<'a> {
    Ready(Captured<'a>),
    Busy(BusyReason),
    Unknown(UnknownReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Closure {
    Clear,
    Busy(BusyReason),
    Unknown(UnknownReason),
}

/// Sealed in-memory capture. No caller-set completeness flag. The separate
/// crate-private receipt recheck is gated by the transaction guard.
#[derive(Debug)]
pub struct Captured<'a> {
    target: Target<'a>,
    pinned: Vec<FileIdentity>,
    helper: ProcessIdentity,
    descendants: Vec<ProcessIdentity>,
    managed_runtimes: Vec<ProcessIdentity>,
    group_members: Vec<ProcessIdentity>,
}

impl Captured<'_> {
    pub fn helper_identity(&self) -> ProcessIdentity {
        self.helper
    }
    pub fn descendants(&self) -> &[ProcessIdentity] {
        &self.descendants
    }
    pub fn managed_runtimes(&self) -> &[ProcessIdentity] {
        &self.managed_runtimes
    }
    pub fn group_members(&self) -> &[ProcessIdentity] {
        &self.group_members
    }
}

/// Identity facts copied only from a completed capture/closure transaction.
/// This is NOT a completeness proof or a general snapshot deserializer. Before
/// using it the guard must verify a private root-owned persisted receipt, same
/// boot, original shell birth, still-held installer lease, transaction identity,
/// and every original protected object identity. No CLI-supplied boolean or
/// untrusted JSON can authorize this entry point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReceiptIdentities {
    pub helper: ProcessIdentity,
    pub descendants: Vec<ProcessIdentity>,
    pub managed_runtimes: Vec<ProcessIdentity>,
    pub group_members: Vec<ProcessIdentity>,
}

const MAX_RECEIPT_IDENTITIES: usize = 4096;

fn valid_identity(identity: ProcessIdentity) -> bool {
    identity.pid > 1
        && identity.pid <= i32::MAX as u32
        && identity.ppid <= i32::MAX as u32
        && identity.ppid != identity.pid
        && identity.pgid > 0
        && identity.pgid <= i32::MAX as u32
        && identity.start_seconds > 0
        && identity.start_seconds <= i64::MAX as u64
        && identity.start_microseconds < 1_000_000
}

fn validate_receipt_identities(
    helper_pid: u32,
    receipt: &ReceiptIdentities,
) -> Result<(), UnknownReason> {
    let bad = || {
        UnknownReason::new(
            "receipt-identities",
            None,
            "invalid or inconsistent receipt identities",
        )
    };
    let helper = receipt.helper;
    if !valid_identity(helper)
        || helper.pid != helper_pid
        || helper.ppid != 1
        || helper.effective_uid != 0
        || helper.pgid <= 1
    {
        return Err(bad());
    }
    let mut all = BTreeMap::from([(helper.pid, helper)]);
    for category in [
        &receipt.descendants,
        &receipt.managed_runtimes,
        &receipt.group_members,
    ] {
        if category.len() > MAX_RECEIPT_IDENTITIES {
            return Err(bad());
        }
        let mut seen = BTreeSet::new();
        for &identity in category {
            if !valid_identity(identity) || identity.pid == helper.pid || !seen.insert(identity.pid)
            {
                return Err(bad());
            }
            if all
                .insert(identity.pid, identity)
                .is_some_and(|prior| prior != identity)
                || all.len() > MAX_RECEIPT_IDENTITIES
            {
                return Err(bad());
            }
        }
    }
    if receipt
        .group_members
        .iter()
        .any(|identity| identity.pgid != helper.pgid)
    {
        return Err(bad());
    }
    // Captured descendants must retain the capture's complete parent chain.
    // Build edges once to keep adversarial receipt validation bounded.
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for identity in &receipt.descendants {
        children
            .entry(identity.ppid)
            .or_default()
            .push(identity.pid);
    }
    let mut pending = vec![helper.pid];
    let mut reached = BTreeSet::new();
    while let Some(parent) = pending.pop() {
        if let Some(pids) = children.get(&parent) {
            for &pid in pids {
                if !reached.insert(pid) {
                    return Err(bad());
                }
                pending.push(pid);
            }
        }
    }
    if reached.len() != receipt.descendants.len() {
        return Err(bad());
    }
    Ok(())
}

fn receipt_capture<'a>(
    target: Target<'a>,
    pinned: Vec<FileIdentity>,
    receipt: &ReceiptIdentities,
) -> Result<Captured<'a>, UnknownReason> {
    validate_receipt_identities(target.helper_pid, receipt)?;
    if pinned.len() != target.managed_executables.len() + 1 {
        return Err(UnknownReason::new(
            "receipt-identities",
            None,
            "protected cohort size mismatch",
        ));
    }
    Ok(Captured {
        target,
        pinned,
        helper: receipt.helper,
        descendants: receipt.descendants.clone(),
        managed_runtimes: receipt.managed_runtimes.clone(),
        group_members: receipt.group_members.clone(),
    })
}

/// Same-transaction recheck only, after the guard verifies the persisted receipt
/// and original object bindings. Does not recapture a now-absent helper. Every
/// call freshly pins the held ORIGINAL executable paths, then runs the existing
/// complete census and closure policy with before/after pin verification.
// Match the sole caller module, mac_migration_guard; pure policy tests stay cross-platform.
#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) fn recheck_verified_receipt(target: Target<'_>, receipt: &ReceiptIdentities) -> Closure {
    if let Err(error) = validate_receipt_identities(target.helper_pid, receipt) {
        return Closure::Unknown(error);
    }
    #[cfg(target_os = "macos")]
    {
        let pinned = match native::pin(target) {
            Ok(pinned) => pinned,
            Err(error) => return Closure::Unknown(error),
        };
        let captured = match receipt_capture(target, pinned, receipt) {
            Ok(captured) => captured,
            Err(error) => return Closure::Unknown(error),
        };
        observe_closed(&captured)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Closure::Unknown(UnknownReason::new(
            "platform",
            None,
            "macOS libproc required",
        ))
    }
}

fn reconcile_live_identity(
    pid: u32,
    first: (ProcessIdentity, bool),
    second: (ProcessIdentity, bool),
) -> Result<ProcessIdentity, UnknownReason> {
    if !valid_identity(first.0) || first.0.pid != pid || first.1 || second.1 || first.0 != second.0
    {
        return Err(UnknownReason::new(
            "live-identity",
            Some(pid),
            "live BSD identity did not reconcile",
        ));
    }
    Ok(first.0)
}

/// Narrow live parent/shell birth binding: two full BSD records, no image read
/// or process-census claim. Exits, zombies, INEXIT and changed identities fail.
// Match the sole caller module, mac_migration_guard; pure policy tests stay cross-platform.
#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) fn live_identity(pid: u32) -> Result<ProcessIdentity, UnknownReason> {
    if pid <= 1 || pid > i32::MAX as u32 {
        return Err(UnknownReason::new(
            "live-identity",
            Some(pid),
            "invalid live target PID",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        native::live_identity(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(UnknownReason::new(
            "platform",
            Some(pid),
            "macOS libproc required",
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Process {
    identity: ProcessIdentity,
    path: PathBuf,
    path_file: FileIdentity,
}

enum ReadError {
    /// A missing process or changed birth/image requires a new whole census.
    Race,
    /// Exact full BSD identity with SZOMB, never inferred from an image error.
    /// The census must independently observe this same terminal identity again.
    Terminated(ProcessIdentity),
    Unknown(UnknownReason),
}

trait Source {
    fn pids(&mut self) -> Result<Vec<u32>, UnknownReason>;
    fn process(&mut self, pid: u32) -> Result<Process, ReadError>;
}

fn normalize_pids(pids: Vec<u32>) -> Result<Vec<u32>, UnknownReason> {
    if pids.is_empty() || pids.len() >= MAX_PROCESSES {
        return Err(UnknownReason::new(
            "census",
            None,
            "empty or saturated process list",
        ));
    }
    let mut unique = BTreeSet::new();
    for pid in pids {
        if pid > i32::MAX as u32 || !unique.insert(pid) {
            return Err(UnknownReason::new(
                "census",
                Some(pid),
                "invalid or duplicate PID",
            ));
        }
        // PID 0 is the kernel, not a user-space configured runtime.
        if pid == 0 {
            continue;
        }
    }
    unique.remove(&0);
    if !unique.contains(&1) {
        return Err(UnknownReason::new(
            "census",
            None,
            "launchd missing from process list",
        ));
    }
    Ok(unique.into_iter().collect())
}

fn census(source: &mut impl Source, deadline: Instant) -> Result<Vec<Process>, UnknownReason> {
    'attempt: for _ in 0..MAX_ATTEMPTS {
        let pids = normalize_pids(source.pids()?)?;
        let mut processes = Vec::with_capacity(pids.len());
        let mut terminated = Vec::new();
        for &pid in &pids {
            check_deadline(deadline)?;
            match source.process(pid) {
                Ok(process) => processes.push(process),
                Err(ReadError::Terminated(identity)) if identity.pid == pid => {
                    terminated.push(identity);
                }
                Err(ReadError::Terminated(_)) => {
                    return Err(UnknownReason::new(
                        "process-identity",
                        Some(pid),
                        "terminal identity PID mismatch",
                    ));
                }
                Err(ReadError::Race) => continue 'attempt,
                Err(ReadError::Unknown(reason)) => return Err(reason),
            }
        }
        if normalize_pids(source.pids()?)? != pids {
            continue;
        }
        // Recheck births, parents/groups, native image paths and resolved path
        // objects. Never turn ESRCH into a successful omitted process.
        for process in &processes {
            check_deadline(deadline)?;
            match source.process(process.identity.pid) {
                Ok(after) if after == *process => {}
                Ok(_) | Err(ReadError::Race | ReadError::Terminated(_)) => continue 'attempt,
                Err(ReadError::Unknown(reason)) => return Err(reason),
            }
        }
        // A zombie has no executing image, but must not disappear from the
        // evidence. Reconcile the exact full BSD identity and SZOMB a second
        // time, inside the same stable PID-list boundaries. Reuse, reaping,
        // live/terminal transitions and failed reads require a whole rescan.
        for identity in &terminated {
            check_deadline(deadline)?;
            match source.process(identity.pid) {
                Err(ReadError::Terminated(after)) if after == *identity => {}
                Ok(_) | Err(ReadError::Race | ReadError::Terminated(_)) => continue 'attempt,
                Err(ReadError::Unknown(reason)) => return Err(reason),
            }
        }
        if normalize_pids(source.pids()?)? != pids {
            continue;
        }
        check_deadline(deadline)?;
        return Ok(processes);
    }
    Err(UnknownReason::new(
        "census",
        None,
        "process epochs did not reconcile",
    ))
}

fn check_deadline(deadline: Instant) -> Result<(), UnknownReason> {
    if Instant::now() >= deadline {
        Err(UnknownReason::new(
            "deadline",
            None,
            "observation budget exhausted",
        ))
    } else {
        Ok(())
    }
}

fn matching(process: &Process, file: &HeldExecutable, identity: FileIdentity) -> bool {
    process.path == file.path || process.path_file == identity
}

fn busy(kind: BusyKind, process: &Process) -> BusyReason {
    BusyReason {
        kind,
        process: process.identity,
        executable: process.path.clone(),
    }
}

fn descendants(processes: &[Process], helper: ProcessIdentity) -> Vec<ProcessIdentity> {
    let mut members = BTreeSet::from([helper.pid]);
    loop {
        let before = members.len();
        for process in processes {
            if members.contains(&process.identity.ppid) {
                members.insert(process.identity.pid);
            }
        }
        if members.len() == before {
            break;
        }
    }
    processes
        .iter()
        .filter(|p| p.identity.pid != helper.pid && members.contains(&p.identity.pid))
        .map(|p| p.identity)
        .collect()
}

fn preflight<'a>(
    target: Target<'a>,
    pinned: Vec<FileIdentity>,
    processes: &[Process],
) -> Preflight<'a> {
    let Some(helper) = processes
        .iter()
        .find(|p| p.identity.pid == target.helper_pid)
    else {
        return Preflight::Unknown(UnknownReason::new(
            "helper",
            Some(target.helper_pid),
            "verified launchd PID is absent",
        ));
    };
    if helper.identity.effective_uid != 0
        || helper.identity.ppid != 1
        || helper.identity.pgid <= 1
        || helper.path != target.helper.path
        || helper.path_file != pinned[0]
    {
        return Preflight::Unknown(UnknownReason::new(
            "helper",
            Some(target.helper_pid),
            "helper birth, ownership, group or exact image does not match",
        ));
    }
    let mut managed_runtimes = Vec::new();
    for process in processes {
        for (managed, identity) in target.managed_executables.iter().zip(&pinned[1..]) {
            if matching(process, managed, *identity) {
                managed_runtimes.push(process.identity);
                break;
            }
        }
        if process.identity.pid != target.helper_pid && matching(process, target.helper, pinned[0])
        {
            return Preflight::Busy(busy(BusyKind::AdditionalHelper, process));
        }
    }
    Preflight::Ready(Captured {
        target,
        pinned,
        helper: helper.identity,
        descendants: descendants(processes, helper.identity),
        managed_runtimes,
        group_members: processes
            .iter()
            .filter(|p| {
                p.identity.pgid == helper.identity.pgid && p.identity.pid != helper.identity.pid
            })
            .map(|p| p.identity)
            .collect(),
    })
}

fn closure(captured: &Captured<'_>, processes: &[Process]) -> Closure {
    for process in processes {
        let kind = if process.identity.same_birth(captured.helper) {
            Some(BusyKind::OriginalHelper)
        } else if captured
            .descendants
            .iter()
            .any(|p| process.identity.same_birth(*p))
        {
            Some(BusyKind::CapturedDescendant)
        } else if captured
            .managed_runtimes
            .iter()
            .any(|p| process.identity.same_birth(*p))
        {
            Some(BusyKind::CapturedManagedRuntime)
        } else if captured
            .group_members
            .iter()
            .any(|p| process.identity.same_birth(*p))
        {
            Some(BusyKind::CapturedProcessGroupMember)
        } else if process.identity.pgid == captured.helper.pgid {
            Some(BusyKind::OriginalProcessGroup)
        } else if matching(process, captured.target.helper, captured.pinned[0]) {
            Some(BusyKind::AdditionalHelper)
        } else if captured
            .target
            .managed_executables
            .iter()
            .zip(&captured.pinned[1..])
            .any(|(file, identity)| matching(process, file, *identity))
        {
            Some(BusyKind::ManagedExecutable)
        } else {
            None
        };
        if let Some(kind) = kind {
            return Closure::Busy(busy(kind, process));
        }
    }
    Closure::Clear
}

/// Complete read-only preflight. Only `Ready` may precede service mutation.
pub fn capture(target: Target<'_>) -> Preflight<'_> {
    #[cfg(target_os = "macos")]
    {
        let deadline = Instant::now() + OBSERVATION_BUDGET;
        let pinned = match native::pin(target) {
            Ok(v) => v,
            Err(e) => return Preflight::Unknown(e),
        };
        let processes = match census(&mut native::Native, deadline) {
            Ok(v) => v,
            Err(e) => return Preflight::Unknown(e),
        };
        if let Err(e) = native::verify_pins(target, &pinned) {
            return Preflight::Unknown(e);
        }
        if let Err(e) = check_deadline(deadline) {
            return Preflight::Unknown(e);
        }
        preflight(target, pinned, &processes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Preflight::Unknown(UnknownReason::new(
            "platform",
            None,
            "macOS libproc required",
        ))
    }
}

/// Proves only configured executable/captured process closure. The caller must
/// separately verify job absent+disabled and endpoint closure before installing.
pub fn observe_closed(captured: &Captured<'_>) -> Closure {
    #[cfg(target_os = "macos")]
    {
        let deadline = Instant::now() + OBSERVATION_BUDGET;
        if let Err(e) = native::verify_pins(captured.target, &captured.pinned) {
            return Closure::Unknown(e);
        }
        let processes = match census(&mut native::Native, deadline) {
            Ok(v) => v,
            Err(e) => return Closure::Unknown(e),
        };
        if let Err(e) = native::verify_pins(captured.target, &captured.pinned) {
            return Closure::Unknown(e);
        }
        if let Err(e) = check_deadline(deadline) {
            return Closure::Unknown(e);
        }
        closure(captured, &processes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = captured;
        Closure::Unknown(UnknownReason::new(
            "platform",
            None,
            "macOS libproc required",
        ))
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use nix::libc;
    use std::ffi::OsString;
    use std::mem::{size_of, zeroed};
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::MetadataExt;

    // ABI source: Apple XNU f6217f891ac0bb64f3d375211650a4c1ff8ca1ea,
    // bsd/sys/proc_info.h and libsyscall/wrappers/libproc/libproc.{h,c}.
    // Reuse libc's SDK proc_bsdinfo; no handwritten structure or guessed size.
    #[allow(unsafe_code, reason = "read-only macOS libproc linkage")]
    #[link(name = "proc")]
    unsafe extern "C" {}

    pub(super) struct Native;

    fn io(stage: &'static str, pid: Option<u32>, error: std::io::Error) -> UnknownReason {
        UnknownReason::new(stage, pid, error.to_string())
    }

    fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn pinned(file: &HeldExecutable) -> Result<FileIdentity, UnknownReason> {
        if !file.path.is_absolute()
            || file.path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(UnknownReason::new(
                "target",
                None,
                "protected executable requires an exact absolute path",
            ));
        }
        let held = file.file.metadata().map_err(|e| io("held-file", None, e))?;
        let path =
            std::fs::symlink_metadata(&file.path).map_err(|e| io("protected-path", None, e))?;
        if !held.is_file()
            || !path.is_file()
            || held.uid() != 0
            || path.uid() != 0
            || held.mode() & 0o022 != 0
            || path.mode() & 0o022 != 0
            || held.nlink() != 1
            || path.nlink() != 1
            || held.ino() == 0
            || file_identity(&held) != file_identity(&path)
        {
            return Err(UnknownReason::new(
                "protected-path",
                None,
                "held root-owned immutable executable and original path differ",
            ));
        }
        Ok(file_identity(&held))
    }

    pub(super) fn pin(target: Target<'_>) -> Result<Vec<FileIdentity>, UnknownReason> {
        if target.helper_pid <= 1
            || target.helper_pid > i32::MAX as u32
            || target.managed_executables.is_empty()
        {
            return Err(UnknownReason::new(
                "target",
                None,
                "missing helper PID or protected runtime cohort",
            ));
        }
        let mut seen_paths = BTreeSet::new();
        let mut seen_files = BTreeSet::new();
        let mut result = Vec::new();
        for file in std::iter::once(target.helper).chain(target.managed_executables) {
            let identity = pinned(file)?;
            if !seen_paths.insert(&file.path) || !seen_files.insert(identity) {
                return Err(UnknownReason::new(
                    "target",
                    None,
                    "duplicate executable in protected cohort",
                ));
            }
            result.push(identity);
        }
        Ok(result)
    }

    pub(super) fn verify_pins(
        target: Target<'_>,
        expected: &[FileIdentity],
    ) -> Result<(), UnknownReason> {
        if pin(target)? != expected {
            return Err(UnknownReason::new(
                "protected-path",
                None,
                "protected original executable identity changed",
            ));
        }
        Ok(())
    }

    fn native_error(stage: &'static str, pid: u32) -> ReadError {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            ReadError::Race
        } else {
            ReadError::Unknown(io(stage, Some(pid), error))
        }
    }

    #[allow(
        unsafe_code,
        reason = "read-only BSD process identity using libc SDK layout"
    )]
    fn identity(pid: u32) -> Result<(ProcessIdentity, u32), ReadError> {
        // SAFETY: libc's SDK structure is aligned, all-zero valid, and its exact
        // capacity is passed; no returned field is read before a full-size result.
        let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
        let returned = unsafe {
            *libc::__error() = 0;
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                // Apple proc_pidinfo: nonzero arg also searches zombie procs
                // for PROC_PIDTBSDINFO; SHORTBSD is not birth evidence.
                1,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        if returned <= 0 {
            return Err(native_error("process-identity", pid));
        }
        if returned as usize != size_of::<libc::proc_bsdinfo>()
            || info.pbi_pid != pid
            || info.pbi_start_tvsec == 0
            || info.pbi_start_tvusec >= 1_000_000
        {
            return Err(ReadError::Unknown(UnknownReason::new(
                "process-identity",
                Some(pid),
                "invalid or partial BSD identity",
            )));
        }
        // PROC_FLAG_INEXIT = 4 in the pinned Apple proc_info.h. INEXIT alone
        // does not prove termination; only the exact SZOMB status may do so.
        if info.pbi_status != libc::SZOMB && info.pbi_flags & 4 != 0 {
            return Err(ReadError::Race);
        }
        Ok((
            ProcessIdentity {
                pid,
                ppid: info.pbi_ppid,
                pgid: info.pbi_pgid,
                effective_uid: info.pbi_uid,
                start_seconds: info.pbi_start_tvsec,
                start_microseconds: info.pbi_start_tvusec,
            },
            info.pbi_status,
        ))
    }

    pub(super) fn live_identity(pid: u32) -> Result<ProcessIdentity, UnknownReason> {
        let convert = |error| match error {
            ReadError::Unknown(reason) => reason,
            ReadError::Race | ReadError::Terminated(_) => UnknownReason::new(
                "live-identity",
                Some(pid),
                "live BSD identity exited or raced",
            ),
        };
        let (first, first_status) = identity(pid).map_err(convert)?;
        let (second, second_status) = identity(pid).map_err(convert)?;
        reconcile_live_identity(
            pid,
            (first, first_status == libc::SZOMB),
            (second, second_status == libc::SZOMB),
        )
    }

    #[allow(unsafe_code, reason = "read-only exact native executable path")]
    fn image(pid: u32) -> Result<PathBuf, ReadError> {
        // PROC_PIDPATHINFO_MAXSIZE, Apple SDK.
        let mut bytes = [0u8; 4096];
        // SAFETY: writable buffer with its exact capacity; validate returned
        // length and terminating NUL, preserving non-UTF-8 native path bytes.
        let returned = unsafe {
            *libc::__error() = 0;
            libc::proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32)
        };
        if returned <= 0 {
            return Err(native_error("executable-path", pid));
        }
        let length = returned as usize;
        if length >= bytes.len() - 1 || bytes[length] != 0 || bytes[..length].contains(&0) {
            return Err(ReadError::Unknown(UnknownReason::new(
                "executable-path",
                Some(pid),
                "truncated or malformed native path",
            )));
        }
        let path = PathBuf::from(OsString::from_vec(bytes[..length].to_vec()));
        if !path.is_absolute() {
            return Err(ReadError::Unknown(UnknownReason::new(
                "executable-path",
                Some(pid),
                "unresolved native executable path",
            )));
        }
        Ok(path)
    }

    impl Source for Native {
        #[allow(unsafe_code, reason = "read-only complete native PID enumeration")]
        fn pids(&mut self) -> Result<Vec<u32>, UnknownReason> {
            // SAFETY: null/zero is libproc's sizing query; the second call uses
            // an aligned initialized PID array and passes its capacity in bytes.
            let estimate = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
            if estimate <= 0 || estimate as usize >= MAX_PROCESSES - 64 {
                return Err(UnknownReason::new(
                    "census",
                    None,
                    "invalid or excessive native PID count",
                ));
            }
            let capacity = estimate as usize + 64;
            let mut pids = Vec::<i32>::new();
            pids.try_reserve_exact(capacity)
                .map_err(|e| UnknownReason::new("census", None, e.to_string()))?;
            pids.resize(capacity, -1);
            let count = unsafe {
                libc::proc_listallpids(
                    pids.as_mut_ptr().cast(),
                    (capacity * size_of::<i32>()) as i32,
                )
            };
            if count <= 0 || count as usize >= capacity {
                return Err(UnknownReason::new(
                    "census",
                    None,
                    "failed or saturated native PID enumeration",
                ));
            }
            pids.truncate(count as usize);
            if pids.iter().any(|p| *p < 0) {
                return Err(UnknownReason::new(
                    "census",
                    None,
                    "invalid native PID element",
                ));
            }
            Ok(pids.into_iter().map(|p| p as u32).collect())
        }

        fn process(&mut self, pid: u32) -> Result<Process, ReadError> {
            let (before, status) = identity(pid)?;
            if status == libc::SZOMB {
                // No image getter: the full census will recheck this exact
                // identity as SZOMB before excluding it from executing images.
                return Err(ReadError::Terminated(before));
            }
            let path = image(pid)?;
            let metadata = match std::fs::metadata(&path) {
                Ok(value) => value,
                Err(error) => {
                    // Reconcile a genuine exit by rescanning; persistent missing
                    // or denied executable paths remain Unknown, never absent.
                    let (after, status) = identity(pid)?;
                    if after != before || status == libc::SZOMB {
                        return Err(ReadError::Race);
                    }
                    return Err(ReadError::Unknown(io(
                        "executable-object",
                        Some(pid),
                        error,
                    )));
                }
            };
            if !metadata.is_file() || metadata.ino() == 0 || metadata.nlink() == 0 {
                return Err(ReadError::Unknown(UnknownReason::new(
                    "executable-object",
                    Some(pid),
                    "unresolved or deleted native executable",
                )));
            }
            let (after, status) = identity(pid)?;
            if after != before || status == libc::SZOMB || image(pid)? != path {
                return Err(ReadError::Race);
            }
            Ok(Process {
                identity: before,
                path,
                path_file: file_identity(&metadata),
            })
        }
    }
}

#[cfg(test)]
mod tests;

//! B3 immutable content store. Runtime callers select an installed id, never source bytes.
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core_objects::PayloadDir;
use polaris_helper_proto::core_manifest::{CoreId, InstalledCoreManifest};

static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
mod recovery_transaction;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod lock_diagnostics;
// Store mutation authority ends with this scope, not with the last incidental
// fork-inherited descriptor. This guard is never used for a version/runtime
// lease: those intentionally remain locked while any reader retains them.
struct MutationLock {
    file: Option<File>,
    owner_pid: u32,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    observation: LockObservation,
}
impl MutationLock {
    fn new(file: File) -> Self {
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let observation = LockObservation::from_file(&file);
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        lock_diagnostics::record("acquired", &observation);
        Self {
            file: Some(file),
            owner_pid: std::process::id(),
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            observation,
        }
    }
}
impl Drop for MutationLock {
    fn drop(&mut self) {
        // Only the creating process owns the mutation scope. A fork child's
        // Drop must only close its reference, never unlock the parent's scope.
        let owner = self.owner_pid == std::process::id();
        if owner {
            if let Some(file) = &self.file {
                if let Err(error) = File::unlock(file) {
                    // No retry or panic in cleanup. Close is still attempted;
                    // inherited references can then prolong the lock, so a
                    // failed unlock is not reported as successful release.
                    log::warn!(
                        "core-store mutation unlock failed kind={:?} os_code={:?}",
                        error.kind(),
                        error.raw_os_error()
                    );
                }
            }
        }
        drop(self.file.take());
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        if owner {
            lock_diagnostics::record("parent-file-closed", &self.observation);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn observe_test_spawn(
    origin: &'static str,
    phase: &'static str,
    child_pid: Option<u32>,
) {
    lock_diagnostics::spawn_origin(origin, phase, child_pid);
}

#[derive(Clone, Copy)]
enum LockRole {
    StoreMutationExclusive,
    RepairExclusive,
    OrphanExclusive,
    StartShared,
    RetainExclusive,
    GcExclusive,
}
impl LockRole {
    fn name(self) -> &'static str {
        match self {
            Self::StoreMutationExclusive => "store-mutation-exclusive",
            Self::RepairExclusive => "repair-exclusive",
            Self::OrphanExclusive => "orphan-exclusive",
            Self::StartShared => "start-shared",
            Self::RetainExclusive => "retain-exclusive",
            Self::GcExclusive => "gc-exclusive",
        }
    }
}

#[derive(Default)]
struct LockObservation {
    fd: Option<i32>,
    dev: Option<u64>,
    inode: Option<u64>,
    cloexec: Option<bool>,
}
impl LockObservation {
    fn from_file(file: &File) -> Self {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata().ok();
            let flags = nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_GETFD).ok();
            Self {
                fd: Some(file.as_raw_fd()),
                dev: metadata.as_ref().map(MetadataExt::dev),
                inode: metadata.as_ref().map(MetadataExt::ino),
                cloexec: flags.map(|value| value & nix::libc::FD_CLOEXEC != 0),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = file;
            Self::default()
        }
    }
}

// Failure-only observations borrow the original descriptor. They neither open
// another lease nor change its lock, ownership, lifetime or inheritance flags.
struct LockFailure {
    role: LockRole,
    observation: LockObservation,
    cause: TryLockError,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    holder_snapshot: Option<lock_diagnostics::Snapshot>,
}
impl std::fmt::Display for LockFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (class, kind, os_code) = match &self.cause {
            TryLockError::WouldBlock => ("would-block", io::ErrorKind::WouldBlock, None),
            TryLockError::Error(error) => ("io-error", error.kind(), error.raw_os_error()),
        };
        write!(
            f,
            "core-store-lock role={} class={} kind={kind:?} os_code={os_code:?} fd={:?} dev={:?} inode={:?} cloexec={:?}",
            self.role.name(),
            class,
            self.observation.fd,
            self.observation.dev,
            self.observation.inode,
            self.observation.cloexec
        )?;
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        if let Some(snapshot) = &self.holder_snapshot {
            write!(f, " holder-snapshot={snapshot}")?;
        }
        Ok(())
    }
}
impl std::fmt::Debug for LockFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for LockFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            TryLockError::WouldBlock => None,
            TryLockError::Error(error) => Some(error),
        }
    }
}
fn lock_error(role: LockRole, observation: LockObservation, cause: TryLockError) -> io::Error {
    let kind = match &cause {
        TryLockError::WouldBlock => io::ErrorKind::WouldBlock,
        TryLockError::Error(error) => error.kind(),
    };
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let holder_snapshot = if matches!(
        role,
        LockRole::StoreMutationExclusive | LockRole::RepairExclusive | LockRole::RetainExclusive
    ) && matches!(&cause, TryLockError::WouldBlock)
    {
        Some(lock_diagnostics::snapshot(&observation))
    } else {
        None
    };
    io::Error::new(
        kind,
        LockFailure {
            role,
            observation,
            cause,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            holder_snapshot,
        },
    )
}
fn acquire_lock(file: &File, role: LockRole) -> io::Result<()> {
    let result = match role {
        LockRole::StartShared => file.try_lock_shared(),
        _ => file.try_lock(),
    };
    result.map_err(|cause| lock_error(role, LockObservation::from_file(file), cause))
}

fn optional_lock_result(
    file: &File,
    role: LockRole,
    result: Result<(), TryLockError>,
) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => {
            // Expected busy versions remain protected. Test-only observations
            // expose the original descriptor/class without logging any paths
            // or changing lock lifetime, flags or acquisition policy.
            #[cfg(test)]
            eprintln!(
                "{}",
                lock_error(
                    role,
                    LockObservation::from_file(file),
                    TryLockError::WouldBlock
                )
            );
            Ok(false)
        }
        Err(cause) => Err(lock_error(role, LockObservation::from_file(file), cause)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerPolicy {
    pub uid: Option<u32>,
}
impl OwnerPolicy {
    pub const fn privileged() -> Self {
        Self { uid: Some(0) }
    }
    pub(crate) fn check(self, path: &Path) -> io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(invalid("linked store object"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != self.uid.ok_or_else(|| invalid("store owner unavailable"))?
                || meta.mode() & 0o022 != 0
            {
                return Err(invalid("untrusted store owner or mode"));
            }
        }
        #[cfg(windows)]
        {
            use crate::platform::windows::{coreacl, ops::ProcOps, winproc::WinProcOps};
            let name = path.to_str().ok_or_else(|| invalid("invalid store path"))?;
            let sec = WinProcOps::default()
                .read_object_security(name)
                .map_err(|_| invalid("store ACL unavailable"))?;
            if !coreacl::judge_object(name, &sec).is_empty() {
                return Err(invalid("untrusted store ACL"));
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            return Err(invalid("unsupported store platform"));
        }
        Ok(())
    }
}

// Public administrative OwnerPolicy remains unchanged. Only these explicit
// Windows data constructors can mint a native current-process data scope.
#[cfg(not(windows))]
type StoreOwner = OwnerPolicy;
#[cfg(windows)]
#[derive(Clone)]
struct StoreOwner {
    uid: Option<u32>,
    data: Option<std::sync::Arc<crate::platform::windows::user_data::CurrentUserData>>,
}
#[cfg(windows)]
impl PartialEq for StoreOwner {
    fn eq(&self, other: &Self) -> bool {
        self.uid == other.uid
            && match (&self.data, &other.data) {
                (None, None) => true,
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}
#[cfg(windows)]
impl Eq for StoreOwner {}
#[cfg(windows)]
impl From<OwnerPolicy> for StoreOwner {
    fn from(owner: OwnerPolicy) -> Self {
        Self {
            uid: owner.uid,
            data: None,
        }
    }
}
#[cfg(windows)]
impl StoreOwner {
    fn check(&self, path: &Path) -> io::Result<()> {
        if let Some(data) = &self.data {
            return data.check(path);
        }
        OwnerPolicy { uid: self.uid }.check(path)
    }
    fn stamp(&self, path: &Path) -> io::Result<String> {
        if let Some(data) = &self.data {
            return data.stamp(path);
        }
        object_stamp(path)
    }
    fn mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        if let Some(data) = &self.data {
            return data.check(path);
        }
        private_mode(path, mode)
    }
    fn private_dir(&self, path: &Path) -> io::Result<()> {
        if self.data.is_some() {
            fs::create_dir(path)?;
            self.check(path)
        } else {
            private_dir(path)
        }
    }
    fn seal_dir(&self, path: &Path) -> io::Result<()> {
        if self.data.is_some() {
            self.check(path)
        } else {
            seal_dir(path)
        }
    }
}
#[cfg(not(windows))]
impl OwnerPolicy {
    fn stamp(self, path: &Path) -> io::Result<String> {
        object_stamp(path)
    }
    fn mode(self, path: &Path, mode: u32) -> io::Result<()> {
        private_mode(path, mode)
    }
    fn private_dir(self, path: &Path) -> io::Result<()> {
        private_dir(path)
    }
    fn seal_dir(self, path: &Path) -> io::Result<()> {
        seal_dir(path)
    }
}

pub struct CoreStore {
    root: PathBuf,
    _root_object: PayloadDir,
    owner: StoreOwner,
}
pub struct VerifiedCore {
    admitted: Option<std::sync::Arc<polaris_core_supervisor::admitted_payload::AdmittedPayload>>,
    observation: Vec<(PathBuf, String)>,
    owner: StoreOwner,
    pub manifest: InstalledCoreManifest,
    pub binary: File,
    pub sidecars: Vec<File>,
    pub directory: PathBuf,
    _directory_object: std::sync::Arc<PayloadDir>,
    // A diagnostic cache never holds a store lock. Each actual Start receives
    // a fresh shared open-file-description, retained by its native child too.
    _lease: Option<File>,
}
fn object_stamp(path: &Path) -> io::Result<String> {
    let m = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.len(),
            m.ctime(),
            m.ctime_nsec(),
            m.mtime(),
            m.mtime_nsec(),
            m.mode(),
            m.uid(),
            m.gid()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!("{:?}", m))
    }
}
impl VerifiedCore {
    pub fn prepare(
        &self,
        purpose: polaris_core_supervisor::admitted_payload::ExecPurpose,
    ) -> io::Result<polaris_core_supervisor::admitted_payload::PreparedExecution> {
        if self._lease.is_none() {
            return Err(invalid("diagnostic cache has no execution lease"));
        }
        self.admitted
            .as_ref()
            .ok_or_else(|| invalid("no compiled execution admission"))?
            .prepare(purpose)
            .map_err(Into::into)
    }
    fn copy_with_lease(&self, lease: Option<File>) -> io::Result<Self> {
        Ok(Self {
            admitted: self.admitted.clone(),
            observation: self.observation.clone(),
            owner: self.owner.clone(),
            manifest: self.manifest.clone(),
            binary: self.binary.try_clone()?,
            sidecars: self
                .sidecars
                .iter()
                .map(File::try_clone)
                .collect::<io::Result<_>>()?,
            directory: self.directory.clone(),
            _directory_object: std::sync::Arc::clone(&self._directory_object),
            _lease: lease,
        })
    }
    pub(crate) fn diagnostic_cache(&self) -> io::Result<Self> {
        self.copy_with_lease(None)
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn execution_lease(&self) -> io::Result<&File> {
        self._lease
            .as_ref()
            .ok_or_else(|| invalid("execution lease absent"))
    }
    /// Lightweight data observation, never execution admission. The shared
    /// version lease remains owned by this value until its final native user.
    pub fn data_is_current(&self) -> bool {
        self.observation.iter().all(|(p, s)| {
            self.owner.check(p).is_ok() && self.owner.stamp(p).is_ok_and(|now| now == *s)
        })
    }
    pub(crate) fn is_current(&self) -> bool {
        self.admitted.as_ref().is_some_and(|a| a.is_current()) && self.data_is_current()
    }
}
impl std::fmt::Debug for VerifiedCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedCore")
            .field("id", &self.manifest.id)
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl VerifiedCore {
    pub(crate) fn recheck_fixture(&self) -> io::Result<()> {
        use std::io::Seek;
        let mut sidecars = self.sidecars.iter();
        for expected in &self.manifest.core.files {
            let file = if expected.name.starts_with("sing-box") {
                &self.binary
            } else {
                sidecars
                    .next()
                    .ok_or_else(|| invalid("fixture sidecar absent"))?
            };
            let mut file = file.try_clone()?;
            file.rewind()?;
            if file.metadata()?.len() != expected.bytes
                || hash_reader(&mut file, expected.bytes)? != expected.sha256
            {
                return Err(invalid("fixture contents changed"));
            }
        }
        Ok(())
    }
    /// Virtual native-ops tests still bind their exact bytes/id and retain real
    /// file objects. This does not install a root store or attest a device.
    pub(crate) fn fixture(
        directory: &Path,
        lease_path: &Path,
        manifest: InstalledCoreManifest,
    ) -> io::Result<Self> {
        manifest
            .validate(crate::core_hash::sha256_hex)
            .map_err(invalid)?;
        #[cfg(unix)]
        let uid = Some(nix::unistd::Uid::current().as_raw());
        #[cfg(not(unix))]
        let uid = None;
        let objects = PayloadDir::open(directory, uid)?;
        let mut binary = None;
        let mut sidecars = Vec::new();
        for expected in &manifest.core.files {
            let (mut file, bytes) = objects.open_regular(&expected.name)?;
            if bytes != expected.bytes || hash_reader(&mut file, bytes)? != expected.sha256 {
                return Err(invalid("fixture bytes differ"));
            }
            if expected.name.starts_with("sing-box") {
                binary = Some(file);
            } else {
                sidecars.push(file);
            }
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(lease_path)?;
        lease
            .try_lock_shared()
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self {
            admitted: None,
            observation: Vec::new(),
            owner: OwnerPolicy { uid }.into(),
            manifest,
            binary: binary.ok_or_else(|| invalid("fixture core absent"))?,
            sidecars,
            directory: directory.into(),
            _directory_object: std::sync::Arc::new(objects),
            _lease: Some(lease),
        })
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyPresent,
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
fn store_admission_failure(error: io::Error) -> io::Error {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionError, AdmissionErrorKind as K, ExecPurpose, RecoveryAction,
    };
    let structural = matches!(
        error.kind(),
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
    );
    #[cfg(unix)]
    let structural = structural
        || matches!(
            error.raw_os_error(),
            Some(nix::libc::ELOOP) | Some(nix::libc::ENOTDIR)
        );
    let kind = if error.kind() == io::ErrorKind::WouldBlock {
        K::Busy
    } else if error.kind() == io::ErrorKind::NotFound {
        K::Missing
    } else if structural {
        K::Mismatch
    } else {
        K::Io
    };
    AdmissionError {
        kind,
        purpose: Some(ExecPurpose::RunMain),
        member: None,
        recovery: if kind.is_payload_damage() {
            RecoveryAction::RepairHelperInstallation
        } else if kind == K::Busy {
            RecoveryAction::RetryAfterStop
        } else {
            RecoveryAction::None
        },
        detail: error.to_string(),
    }
    .into()
}
fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::windows::winproc::seal_store_object(path, false)?;
    Ok(())
}
fn seal_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    crate::platform::windows::winproc::seal_store_object(path, true)?;
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

impl CoreStore {
    #[cfg(windows)]
    fn execution_maintenance_guard(
        &self,
        id: &CoreId,
    ) -> io::Result<Option<polaris_core_supervisor::admitted_payload::WindowsMaintenanceGuard>>
    {
        // Data-only stores never become execution slots. Their normal lease
        // file and AlreadyPresent RW flush remain unchanged.
        if self.owner.data.is_some() {
            return Ok(None);
        }
        self.owner.check(&self.root.join("leases"))?;
        polaris_core_supervisor::admitted_payload::WindowsMaintenanceGuard::for_store(
            &self.root, id,
        )
    }
    fn pending_repair_ids(&self) -> io::Result<Vec<CoreId>> {
        let mut directory = PayloadDir::open(&self.root.join(".staging"), self.owner.uid)?;
        let mut ids = Vec::new();
        for entry in directory.entry_names(recovery_transaction::MAX_REPAIR_ENTRIES)? {
            let name = entry
                .to_str()
                .ok_or_else(|| invalid("unrepresentable staging entry"))?;
            if let Some(id) = name.strip_prefix("repair-") {
                ids.push(CoreId::parse(id).map_err(invalid)?);
            }
        }
        Ok(ids)
    }
    fn require_no_pending_repair(&self) -> io::Result<()> {
        if !self.pending_repair_ids()?.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "pending core repair must be resumed before retirement or migration",
            ));
        }
        Ok(())
    }
    fn resume_pending_repair(
        &self,
        id: &CoreId,
        mutation: &MutationLock,
    ) -> io::Result<Option<InstallOutcome>> {
        if !recovery_transaction::has_records(self, id)? {
            return Ok(None);
        }
        // Never manufacture a replacement lock for an old or journaled object.
        let lease = self.lease_file(id)?;
        acquire_lock(&lease, LockRole::RepairExclusive)?;
        #[cfg(windows)]
        let _execution = self.execution_maintenance_guard(id)?;
        self.resume_under_lease(id, mutation, &lease)
    }
    fn resume_under_lease(
        &self,
        id: &CoreId,
        mutation: &MutationLock,
        lease: &File,
    ) -> io::Result<Option<InstallOutcome>> {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        match recovery_transaction::resume(self, id, mutation, lease, &cancel)
            .map_err(io::Error::from)?
        {
            recovery_transaction::RepairOutcome::NoPendingJournal => Ok(None),
            outcome => outcome.into_install_result().map(Some),
        }
    }
    fn finish_retirement_barriers(
        &self,
        id: &CoreId,
        mutation: &MutationLock,
        lease: &File,
    ) -> io::Result<()> {
        if recovery_transaction::has_records(self, id)? {
            if self.resume_under_lease(id, mutation, lease)?.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "core repair resumed; retry retirement selection",
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn lease_cached(
        &self,
        id: &CoreId,
        cached: &VerifiedCore,
    ) -> io::Result<VerifiedCore> {
        use polaris_core_supervisor::admitted_payload::{
            AdmissionError, AdmissionErrorKind, RecoveryAction,
        };
        let lease = self.lease_file(id).map_err(store_admission_failure)?;
        acquire_lock(&lease, LockRole::StartShared).map_err(store_admission_failure)?;
        // Recheck after taking the lease: a different process may have repaired
        // the namespace between the diagnostic read and this Start request.
        if cached.manifest.id != id.as_str()
            || cached.owner != self.owner
            || cached.directory != self.root.join("versions").join(id.as_str())
            || !cached.is_current()
        {
            return Err(AdmissionError {
                kind: AdmissionErrorKind::Changed,
                purpose: Some(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain),
                member: None,
                recovery: RecoveryAction::RepairHelperInstallation,
                detail: "cached store namespace changed before Start".into(),
            }
            .into());
        }
        cached.copy_with_lease(Some(lease))
    }
    /// Daemon startup only, before accepting any pinned or retired IPC. Preserve
    /// the entire flat cohort while any native mapping/reader is observed or
    /// cannot be ruled out. This never reclaims immutable version directories.
    pub fn retire_flat_layout(&self) -> io::Result<Vec<PathBuf>> {
        self.retire_flat_layout_with(crate::legacy_use::payload_use)
    }
    fn retire_flat_layout_with(
        &self,
        observe: impl FnOnce(&[PathBuf]) -> crate::legacy_use::LegacyUse,
    ) -> io::Result<Vec<PathBuf>> {
        let _lock = self.mutation_lock()?;
        let names = ["sing-box", "sing-box.exe", "libcronet.so", "libcronet.dll"];
        let mut flat = Vec::new();
        for name in names {
            let path = self.root.join(name);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                    self.owner.check(&path)?;
                    let _ = self._root_object.open_regular(name)?;
                    flat.push(path);
                }
                Ok(_) => return Err(invalid("unexpected legacy flat object")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if flat.is_empty() {
            return Ok(flat);
        }
        if observe(&flat) != crate::legacy_use::LegacyUse::Idle {
            log::info!("legacy flat core retirement deferred: native use active or unknown");
            return Ok(Vec::new());
        }
        for path in &flat {
            self.owner.check(path)?;
            fs::remove_file(path)?;
        }
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(flat)
    }
    fn lease_file(&self, id: &CoreId) -> io::Result<File> {
        let dir = self.root.join("leases");
        self.owner.check(&dir)?;
        let objects = PayloadDir::open(&dir, self.owner.uid)?;
        let path = dir.join(format!("{}.lock", id.as_str()));
        self.owner.check(&path)?;
        let (file, _) = objects.open_regular(&format!("{}.lock", id.as_str()))?;
        #[cfg(windows)]
        if let Some(data) = &self.owner.data {
            data.stamp_named_held(&file, &path, false)?;
        }
        Ok(file)
    }
    fn mutation_lock(&self) -> io::Result<MutationLock> {
        crate::pinned_core::invalidate_unused();
        self.lock_store_mutation()
    }
    // Cold private preparation serializes with publication, but is not a
    // mutation of the selected data. Its caller may already hold the diagnostic
    // cache mutex: never recursively invalidate that cache or retire a warm owner.
    fn lock_store_mutation(&self) -> io::Result<MutationLock> {
        let path = self.root.join(".core-store.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("invalid store lock"));
        }
        #[cfg(windows)]
        self.owner.mode(&path, 0o600)?;
        self.owner.check(&path)?;
        acquire_lock(&file, LockRole::StoreMutationExclusive)?;
        Ok(MutationLock::new(file))
    }
    pub fn open(root: &Path, owner: OwnerPolicy) -> io::Result<Self> {
        Self::open_scoped(root, owner.into())
    }
    #[cfg(windows)]
    pub fn open_current_user_data(root: &Path) -> io::Result<Self> {
        let data = std::sync::Arc::new(crate::platform::windows::user_data::CurrentUserData::open(
            root, false,
        )?);
        Self::open_scoped(
            data.root(),
            StoreOwner {
                uid: None,
                data: Some(data.clone()),
            },
        )
    }
    #[cfg(windows)]
    pub fn initialize_current_user_data(root: &Path) -> io::Result<Self> {
        let data = std::sync::Arc::new(crate::platform::windows::user_data::CurrentUserData::open(
            root, true,
        )?);
        // Probe the actual barrier before any child publication or transaction.
        data.barrier()?;
        Self::initialize_scoped(
            data.root(),
            StoreOwner {
                uid: None,
                data: Some(data.clone()),
            },
        )
    }
    fn open_scoped(root: &Path, owner: StoreOwner) -> io::Result<Self> {
        owner.check(root)?;
        let object = PayloadDir::open(root, owner.uid)?;
        for name in ["versions", "manifests", ".staging", ".trash", "leases"] {
            let child = root.join(name);
            owner.check(&child)?;
            let _ = PayloadDir::open(&child, owner.uid)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            _root_object: object,
            owner,
        })
    }

    /// Administrative installer only. Runtime status/open never creates directories.
    pub fn initialize(root: &Path, owner: OwnerPolicy) -> io::Result<Self> {
        Self::initialize_scoped(root, owner.into())
    }
    fn initialize_scoped(root: &Path, owner: StoreOwner) -> io::Result<Self> {
        owner.check(root)?;
        let _root = PayloadDir::open(root, owner.uid)?;
        for name in ["versions", "manifests", ".staging", ".trash", "leases"] {
            let child = root.join(name);
            match fs::create_dir(&child) {
                Ok(()) => {
                    owner.mode(&child, 0o755)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            owner.check(&child)?;
        }
        Self::open_scoped(root, owner)
    }

    /// Non-creating selected slot probe. Any partial slot or journal remains
    /// selected damage/debt, never permission to fall back to package bytes.
    pub fn has_selected_data(&self, id: &CoreId) -> io::Result<bool> {
        self.require_no_pending_repair()?;
        Ok(
            object_present(&self.root.join("versions").join(id.as_str()))?
                || object_present(
                    &self
                        .root
                        .join("manifests")
                        .join(format!("{}.json", id.as_str())),
                )?
                || object_present(
                    &self
                        .root
                        .join("leases")
                        .join(format!("{}.lock", id.as_str())),
                )?,
        )
    }
    /// Data only: exact receipt, held members and a new shared occupancy lease.
    /// The caller must independently compare compiled selection and admit its
    /// real purpose using the sole execution factory.
    pub fn selected_data(&self, id: &CoreId) -> io::Result<VerifiedCore> {
        self.require_no_pending_repair()?;
        // This remains a data observation, not content/execution admission.
        // Installation hashes under its lease; the execution factory owns
        // purpose-specific content admission. Do not rescan payloads on each
        // current-user metadata observation.
        self.verify_objects(id, false)
    }

    /// Explicit cold data-recovery diagnosis, never execution admission.
    /// None means no selected slot; Some(true) means only known member
    /// absence/size/hash damage. Namespace, receipt, ACL and occupancy failures
    /// remain errors. No path or executable owner escapes this observation.
    pub fn selected_recovery_damage(
        &self,
        compiled: &polaris_helper_proto::core_manifest::CoreManifest,
    ) -> io::Result<Option<bool>> {
        recovery_transaction::selected_recovery_damage(self, compiled)
    }

    pub fn verify_for_start(&self, id: &CoreId) -> io::Result<VerifiedCore> {
        #[cfg(windows)]
        if self.owner.data.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "current-user data store does not grant execution",
            ));
        }
        use polaris_core_supervisor::admitted_payload::UntrustedPayload;
        #[cfg(not(target_os = "macos"))]
        use polaris_core_supervisor::admitted_payload::{admit_selected, ExecPurpose};
        #[cfg(target_os = "macos")]
        let _mutation = self
            .lock_store_mutation()
            .map_err(store_admission_failure)?;
        let mut core = self
            .verify_objects(id, false)
            .map_err(store_admission_failure)?;
        let binary = core.directory.join(if cfg!(windows) {
            "sing-box.exe"
        } else {
            "sing-box"
        });
        #[cfg(target_os = "macos")]
        let admission = polaris_core_supervisor::admitted_payload::admit_macos_root_helper_main(
            UntrustedPayload::bundled(binary),
        );
        #[cfg(not(target_os = "macos"))]
        let admission = admit_selected(UntrustedPayload::bundled(binary), ExecPurpose::RunMain);
        let admitted = admission.map_err(|mut error| {
            use polaris_core_supervisor::admitted_payload::{AdmissionErrorKind, RecoveryAction};
            if matches!(
                error.kind,
                AdmissionErrorKind::Missing
                    | AdmissionErrorKind::Mismatch
                    | AdmissionErrorKind::Changed
            ) {
                error.recovery = RecoveryAction::RepairHelperInstallation;
            }
            io::Error::from(error)
        })?;
        if admitted.id() != id {
            return Err(invalid("compiled core selection differs"));
        }
        #[cfg(target_os = "macos")]
        {
            // A cold generation adds an owned child to the store root. Refresh
            // only after rejecting changes to every selected data observation;
            // otherwise our own mkdir would immediately stale the data cache.
            core = self
                .snapshot_after_private_preparation(id, core, &_mutation)
                .map_err(store_admission_failure)?;
            if !admitted.is_current() {
                return Err(store_admission_failure(invalid(
                    "private execution owner changed during data snapshot",
                )));
            }
        }
        core.admitted = Some(admitted);
        Ok(core)
    }

    #[cfg(target_os = "macos")]
    fn snapshot_after_private_preparation(
        &self,
        id: &CoreId,
        mut data: VerifiedCore,
        mutation: &MutationLock,
    ) -> io::Result<VerifiedCore> {
        if mutation.owner_pid != std::process::id()
            || data.manifest.id != id.as_str()
            || data.observation.iter().any(|(path, before)| {
                path != &self.root
                    && (self.owner.check(path).is_err()
                        || !object_stamp(path).is_ok_and(|now| now == *before))
            })
        {
            return Err(invalid("selected data changed during private preparation"));
        }
        let lease = data
            ._lease
            .take()
            .ok_or_else(|| invalid("preparation lease absent"))?;
        // Transfer the same real shared lease without any unlocked interval.
        // This is storage observation only and never returns an admission owner.
        self.verify_objects_under_lease(id, false, lease)
    }

    // Installation/repair verifies bytes for storage, without granting execution
    // authority or requiring a new loader policy before it can repair the store.
    fn verify_objects(&self, id: &CoreId, hash: bool) -> io::Result<VerifiedCore> {
        let lease = self.lease_file(id)?;
        acquire_lock(&lease, LockRole::StartShared)?;
        self.verify_objects_under_lease(id, hash, lease)
    }
    // Only internal callers already holding this exact occupancy lease use it;
    // never reacquire a shared lock through a separately opened exclusive lease.
    fn verify_objects_under_lease(
        &self,
        id: &CoreId,
        hash: bool,
        lease: File,
    ) -> io::Result<VerifiedCore> {
        let manifests = self.root.join("manifests");
        self.owner.check(&manifests)?;
        let manifest_objects = PayloadDir::open(&manifests, self.owner.uid)?;
        let name = format!("{}.json", id.as_str());
        self.owner.check(&manifests.join(&name))?;
        let (file, length) = manifest_objects.open_regular(&name)?;
        #[cfg(windows)]
        let mut native_observations = Vec::new();
        #[cfg(windows)]
        if let Some(data) = &self.owner.data {
            let path = manifests.join(&name);
            native_observations.push((path.clone(), data.stamp_named_held(&file, &path, false)?));
        }
        if length > 64 * 1024 {
            return Err(invalid("manifest too large"));
        }
        let mut raw = Vec::new();
        file.take(64 * 1024 + 1).read_to_end(&mut raw)?;
        let manifest =
            InstalledCoreManifest::parse(&raw, crate::core_hash::sha256_hex).map_err(invalid)?;
        if manifest.id != id.as_str() {
            return Err(invalid("core id mismatch"));
        }
        let directory = self.root.join("versions").join(id.as_str());
        self.owner.check(&directory)?;
        let mut objects = PayloadDir::open(&directory, self.owner.uid)?;
        let mut actual: Vec<_> = objects
            .entry_names(3)?
            .into_iter()
            .map(|name| {
                name.into_string()
                    .map_err(|_| invalid("invalid payload name"))
            })
            .collect::<io::Result<_>>()?;
        let mut expected: Vec<_> = manifest
            .core
            .files
            .iter()
            .map(|file| file.name.clone())
            .collect();
        actual.sort();
        expected.sort();
        if actual != expected {
            return Err(invalid("payload file set differs"));
        }
        let mut binary = None;
        let mut sidecars = Vec::new();
        for expected in &manifest.core.files {
            self.owner.check(&directory.join(&expected.name))?;
            let (mut file, length) = objects.open_regular(&expected.name)?;
            #[cfg(windows)]
            if let Some(data) = &self.owner.data {
                let path = directory.join(&expected.name);
                native_observations
                    .push((path.clone(), data.stamp_named_held(&file, &path, false)?));
            }
            if length != expected.bytes {
                return Err(invalid("payload size differs"));
            }
            let actual_hash = if hash {
                Some(hash_reader(&mut file, expected.bytes)?)
            } else {
                None
            };
            if actual_hash.is_some_and(|hash| hash != expected.sha256) {
                return Err(invalid("payload hash differs"));
            }
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            if expected.name == "sing-box" || expected.name == "sing-box.exe" {
                binary = Some(file);
            } else {
                sidecars.push(file);
            }
        }
        let mut paths = vec![
            self.root.clone(),
            self.root.join("versions"),
            self.root.join("manifests"),
            self.root.join("leases"),
            directory.clone(),
            manifests.join(&name),
            self.root
                .join("leases")
                .join(format!("{}.lock", id.as_str())),
        ];
        paths.extend(manifest.core.files.iter().map(|f| directory.join(&f.name)));
        let observation = paths
            .into_iter()
            .map(|p| Ok((p.clone(), self.owner.stamp(&p)?)))
            .collect::<io::Result<Vec<_>>>()?;
        #[cfg(windows)]
        for held in &native_observations {
            if !observation.contains(held) {
                return Err(invalid(
                    "opened recovery data differs from named observation",
                ));
            }
        }
        Ok(VerifiedCore {
            admitted: None,
            observation,
            owner: self.owner.clone(),
            manifest,
            binary: binary.ok_or_else(|| invalid("missing binary"))?,
            sidecars,
            directory,
            _directory_object: std::sync::Arc::new(objects),
            _lease: Some(lease),
        })
    }

    fn finish_install_publication(
        &self,
        id: &CoreId,
        mutation: &MutationLock,
        verified: &VerifiedCore,
    ) -> io::Result<()> {
        let lease = verified
            ._lease
            .as_ref()
            .ok_or_else(|| invalid("publication occupancy owner absent"))?;
        let mut publication = recovery_transaction::Publication::open(self, id, mutation, lease)?;
        publication.finish(lease)?;
        if !verified.data_is_current() {
            return Err(invalid("published data changed during directory barriers"));
        }
        Ok(())
    }

    pub fn install(
        &self,
        manifest: &InstalledCoreManifest,
        source: &Path,
        source_uid: Option<u32>,
        source_sid: Option<&str>,
    ) -> io::Result<InstallOutcome> {
        self.install_with_preservation(manifest, source, source_uid, source_sid, false)
    }

    /// Explicit migration may add a new immutable version, but never retires or
    /// repairs an existing payload. Conflicting/corrupt content needs a separate
    /// recovery decision after the previous installation is safely preserved.
    pub fn install_preserving(
        &self,
        manifest: &InstalledCoreManifest,
        source: &Path,
        source_uid: Option<u32>,
        source_sid: Option<&str>,
    ) -> io::Result<InstallOutcome> {
        self.install_with_preservation(manifest, source, source_uid, source_sid, true)
    }

    fn install_with_preservation(
        &self,
        manifest: &InstalledCoreManifest,
        source: &Path,
        source_uid: Option<u32>,
        source_sid: Option<&str>,
        preserve_existing: bool,
    ) -> io::Result<InstallOutcome> {
        #[cfg(not(windows))]
        let _ = source_sid;
        #[cfg(windows)]
        let source_sid = if let Some(data) = &self.owner.data {
            data.guard()?;
            data.barrier()?;
            Some(data.sid())
        } else {
            source_sid
        };
        let mutation = self.mutation_lock()?;
        manifest
            .validate(crate::core_hash::sha256_hex)
            .map_err(invalid)?;
        let id = CoreId::parse(&manifest.id).map_err(invalid)?;
        let destination = self.root.join("versions").join(id.as_str());
        let receipt = self
            .root
            .join("manifests")
            .join(format!("{}.json", id.as_str()));
        if preserve_existing {
            self.require_no_pending_repair()?;
        }
        // Resumption precedes AlreadyPresent and any read/copy of new input.
        // Its errors/outcomes return directly, never through fresh-install cleanup.
        if let Some(outcome) = self.resume_pending_repair(&id, &mutation)? {
            let verified = self.verify_objects(&id, true)?;
            self.finish_install_publication(&id, &mutation, &verified)?;
            return Ok(outcome);
        }
        let version_present = object_present(&destination)?;
        let receipt_present = object_present(&receipt)?;
        if version_present && receipt_present {
            if let Ok(verified) = self.verify_objects(&id, true) {
                if !preserve_existing {
                    let mut options = OpenOptions::new();
                    options.write(true);
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.custom_flags(nix::libc::O_NOFOLLOW);
                    }
                    let file = options.open(&receipt)?;
                    self.owner.check(&receipt)?;
                    file.set_modified(std::time::SystemTime::now())?;
                }
                // Refresh observations after the normal last-use timestamp
                // mutation, retaining the same already held occupancy lease.
                let verified = self.verify_objects_under_lease(
                    &id,
                    true,
                    verified
                        ._lease
                        .as_ref()
                        .ok_or_else(|| invalid("publication lease absent"))?
                        .try_clone()?,
                )?;
                // Also closes an earlier complete publication whose parent
                // barrier failed. A read-only reopen alone is not durability.
                self.finish_install_publication(&id, &mutation, &verified)?;
                return Ok(InstallOutcome::AlreadyPresent);
            }
        }
        if preserve_existing && (version_present || receipt_present) {
            return Err(invalid(
                "migration cannot replace an existing unverified payload or receipt",
            ));
        }
        let lease_path = self
            .root
            .join("leases")
            .join(format!("{}.lock", id.as_str()));
        let repair_lease = if version_present || receipt_present || object_present(&lease_path)? {
            let lease = self.lease_file(&id)?;
            acquire_lock(
                &lease,
                if version_present && receipt_present {
                    LockRole::RepairExclusive
                } else {
                    LockRole::OrphanExclusive
                },
            )?;
            Some(lease)
        } else {
            None
        };
        #[cfg(windows)]
        let _execution = if repair_lease.is_some() {
            self.execution_maintenance_guard(&id)?
        } else {
            None
        };
        let sources = PayloadDir::open(source, source_uid)?;
        let mut stage = None;
        for _ in 0..128 {
            let path = self.root.join(".staging").join(format!(
                "{}-{}",
                std::process::id(),
                STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match self.owner.private_dir(&path) {
                Ok(()) => {
                    stage = Some(path);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let stage = stage.ok_or_else(|| invalid("staging namespace exhausted"))?;
        let copied = (|| -> io::Result<()> {
            for expected in &manifest.core.files {
                let (mut input, length) = sources.open_regular(&expected.name)?;
                #[cfg(windows)]
                if let Some(expected_sid) = source_sid {
                    if crate::platform::windows::winproc::file_owner(&input)? != expected_sid {
                        return Err(invalid("source file owner differs"));
                    }
                }
                if length != expected.bytes {
                    return Err(invalid("source size differs"));
                }
                let path = stage.join(&expected.name);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut output = options.open(&path)?;
                let mut hash = sha2::Sha256::new();
                use sha2::Digest;
                let mut total = 0u64;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    total = total
                        .checked_add(count as u64)
                        .ok_or_else(|| invalid("source size overflow"))?;
                    if total > expected.bytes {
                        return Err(invalid("source grew"));
                    }
                    output.write_all(&buffer[..count])?;
                    hash.update(&buffer[..count]);
                }
                if total != expected.bytes || hex::encode(hash.finalize()) != expected.sha256 {
                    return Err(invalid("source hash differs"));
                }
                #[cfg(windows)]
                self.owner.mode(&path, 0o644)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        &path,
                        fs::Permissions::from_mode(if expected.name.starts_with("sing-box") {
                            0o755
                        } else {
                            0o644
                        }),
                    )?;
                }
                output.sync_all()?;
            }
            self.owner.seal_dir(&stage)?;
            Ok(())
        })();
        if let Err(error) = copied {
            // Repair candidates and originals are retained for diagnosis. Only
            // a wholly fresh legacy install can clean its own private draft.
            if repair_lease.is_none() {
                let _ = fs::remove_dir_all(&stage);
            }
            return Err(error);
        }
        if let Some(lease) = repair_lease.as_ref() {
            let candidate = recovery_transaction::PreparedCandidate::open(self, manifest, &stage)?;
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let outcome =
                recovery_transaction::replace(self, &id, candidate, &mutation, lease, &cancel)
                    .map_err(io::Error::from)?
                    .into_install_result()?;
            let verified = self.verify_objects_under_lease(&id, true, lease.try_clone()?)?;
            self.finish_install_publication(&id, &mutation, &verified)?;
            return Ok(outcome);
        }
        // No prior selected objects exist. Create and hold the exact new lease
        // before publishing any version, then retain partial objects on error.
        // A later explicit retry uses the existing orphan/repair path; unknown
        // replacements are never deleted by a legacy best-effort cleanup.
        let result = (|| {
            use recovery_transaction::{checkpoint, Point};
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let lease = options.open(&lease_path)?;
            self.owner.mode(&lease_path, 0o600)?;
            acquire_lock(&lease, LockRole::OrphanExclusive)?;
            #[cfg(windows)]
            let _execution = self.execution_maintenance_guard(&id)?;
            lease.sync_all()?;
            let mut publication =
                recovery_transaction::Publication::open(self, &id, &mutation, &lease)?;
            checkpoint(Point::FreshLeasePublished)?;
            if object_present(&destination)? {
                return Err(invalid("version concurrently installed"));
            }
            fs::rename(&stage, &destination)?;
            checkpoint(Point::FreshVersionPublished)?;
            let target = self
                .root
                .join("manifests")
                .join(format!("{}.json", id.as_str()));
            let temporary = self
                .root
                .join("manifests")
                .join(format!("{}.new", id.as_str()));
            let raw = manifest.to_json().map_err(invalid)?;
            if temporary.try_exists()? {
                self.owner.check(&temporary)?;
                let objects = PayloadDir::open(&self.root.join("manifests"), self.owner.uid)?;
                let _ = objects.open_regular(&format!("{}.new", id.as_str()))?;
                fs::remove_file(&temporary)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o644);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&raw)?;
            file.sync_all()?;
            self.owner.mode(&temporary, 0o644)?;
            fs::rename(&temporary, &target)?;
            checkpoint(Point::FreshReceiptPublished)?;
            let verified = self.verify_objects_under_lease(&id, true, lease.try_clone()?)?;
            publication.finish(&lease)?;
            if !verified.data_is_current() {
                return Err(invalid("fresh publication namespace changed"));
            }
            Ok(InstallOutcome::Installed)
        })();
        // Failure is not a rollback receipt. Retain the candidate/version,
        // lease and receipt exactly as they stand for replay or diagnosis.
        result
    }

    /// Only orphan versions are eligible; callers supply every live slot id while
    /// holding their existing state lock. Retained manifests protect other App versions.
    pub fn gc(&self, live: &[CoreId]) -> io::Result<Vec<CoreId>> {
        let mutation = self.mutation_lock()?;
        self.require_no_pending_repair()?;
        let mut removed = Vec::new();
        for entry in fs::read_dir(self.root.join("versions"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(id) = CoreId::parse(&name) else {
                continue;
            };
            if live.contains(&id)
                || self
                    .root
                    .join("manifests")
                    .join(format!("{name}.json"))
                    .exists()
            {
                continue;
            }
            self.owner.check(&entry.path())?;
            let lease = self.lease_file(&id)?;
            if !optional_lock_result(&lease, LockRole::GcExclusive, lease.try_lock())? {
                continue;
            }
            #[cfg(windows)]
            let _execution = match self.execution_maintenance_guard(&id) {
                Ok(guard) => guard,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            };
            self.finish_retirement_barriers(&id, &mutation, &lease)?;
            self.trash_version(&id, &entry.path())?;
            removed.push(id);
        }
        Ok(removed)
    }

    /// Keep the latest three installed manifests, plus every busy/live version.
    /// A shared lease covers verification, spawn and lazy sidecar use; no PID
    /// observation or caller-provided empty live list can override that lease.
    pub fn retain_latest_three(&self, live: &[CoreId]) -> io::Result<Vec<CoreId>> {
        let mutation = self.mutation_lock()?;
        self.require_no_pending_repair()?;
        let mut manifests = Vec::new();
        for entry in fs::read_dir(self.root.join("manifests"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(id) = name
                .strip_suffix(".json")
                .and_then(|name| CoreId::parse(name).ok())
            else {
                continue;
            };
            self.owner.check(&entry.path())?;
            manifests.push((entry.metadata()?.modified()?, id, entry.path()));
        }
        manifests.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        let mut removed = Vec::new();
        for (_, id, manifest) in manifests.into_iter().skip(3) {
            if live.contains(&id) {
                continue;
            }
            let lease = self.lease_file(&id)?;
            if !optional_lock_result(&lease, LockRole::RetainExclusive, lease.try_lock())? {
                continue;
            }
            #[cfg(windows)]
            let _execution = match self.execution_maintenance_guard(&id) {
                Ok(guard) => guard,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            };
            self.finish_retirement_barriers(&id, &mutation, &lease)?;
            // Removing the manifest first makes an interrupted cleanup ineligible
            // to start. GC can finish that orphan on a later administrative run.
            fs::remove_file(manifest)?;
            let version = self.root.join("versions").join(id.as_str());
            self.owner.check(&version)?;
            self.trash_version(&id, &version)?;
            removed.push(id);
        }
        Ok(removed)
    }

    fn trash_version(&self, id: &CoreId, version: &Path) -> io::Result<()> {
        self.require_no_pending_repair()?;
        let destination = self.root.join(".trash").join(format!(
            "{}-{}-{}",
            id.as_str(),
            std::process::id(),
            STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::rename(version, &destination)?;
        fs::remove_dir_all(destination)
    }
}

fn object_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn private_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        {
            crate::platform::windows::winproc::seal_store_object(path, mode & 0o044 != 0)
        }
        #[cfg(not(windows))]
        {
            let _ = (path, mode);
            Err(invalid("unsupported store platform"))
        }
    }
}

fn hash_reader(input: &mut File, expected_bytes: u64) -> io::Result<String> {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut input = input.take(expected_bytes + 1);
    let mut bytes = 0u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    if bytes != expected_bytes {
        return Err(invalid("payload changed size"));
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "core_store/macos_private_preparation_tests.rs"]
mod macos_private_preparation_tests;

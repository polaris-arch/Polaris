//! H2b POSIX journal persistence. The root daemon must pass a root-owned, non-writable
//! directory outside the app's tree. Search-only permission may let the child reach the separate
//! UID-writable `cache-owner` directory; journal and lock files remain owner-only. This module
//! never constructs lifecycle, strict-check, or
//! process proofs and is deliberately not wired to StartExact.
//!
//! The stable lock inode is never removed. Every participating helper process opens that inode
//! with O_NOFOLLOW and holds an exclusive flock for the whole store lifetime. Atomic rename is
//! a compare-and-swap only under this cross-process admission rule and the protected directory.

use super::{CacheJournal, CacheOwnerObservation, CachePhase};
use nix::errno::Errno;
use nix::fcntl::{open, openat, renameat, AtFlags, Flock, FlockArg, OFlag};
use nix::sys::stat::{fstat, fstatat, Mode, SFlag};
use nix::unistd::fsync;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Component, Path};

const JOURNAL: &str = "cache-journal-v1.json";
const NEXT: &str = ".cache-journal-v1.next";
const LOCK: &str = "cache-journal-v1.lock";
const OWNER: &str = "cache-owner";
const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const READ_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const WRITE_FLAGS: OFlag = OFlag::O_RDWR
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    UnsafePath,
    UnsafeOwner,
    LockBusy,
    InterruptedWrite,
    MissingJournalWithOwner,
    CorruptJournal,
    StaleJournal,
    UnsafeRecovery,
    Io,
    DurabilityUnknown,
}

/// Inspection only. This value contains no journal, owner fd, or lease authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalDiagnostic {
    pub owner_ref: String,
    pub owner_uid: u32,
    pub phase: &'static str,
}

/// Future auth adapter must mint this from the authenticated peer and mutable-path claim while
/// holding the lifecycle gate. There is intentionally no production constructor in H2c.
#[allow(dead_code)]
pub(crate) struct AuthenticatedOwnerUid {
    local_id: String,
    owner_ref: String,
    uid: u32,
}

/// A held cross-process writer admission. `O_CLOEXEC` prevents a spawned core from retaining
/// the lock. Dropping the value releases the OS lock; no helper code may unlink its file.
///
/// ```compile_fail
/// use polaris_helper::exact_cache::{store::CacheJournalStore, CacheJournal};
/// fn publish(store: &mut CacheJournalStore, journal: &CacheJournal) {
///     store.compare_and_replace(todo!(), None, journal).unwrap();
/// }
/// ```
pub struct CacheJournalStore {
    directory: OwnedFd,
    _lock: Flock<File>,
    owner_uid: u32,
}

/// A real opened cache-owner directory, pinned while the store's flock is held. Its
/// observation is deliberately inaccessible to external crates and is not a stop fence.
#[allow(dead_code)] // H2b remains offline; no lifecycle adapter consumes this proof yet.
pub(crate) struct OwnerDirectory<'store> {
    directory: OwnedFd,
    observation: CacheOwnerObservation,
    parent_dev: u64,
    parent_ino: u64,
    expected_uid: u32,
    _store: PhantomData<&'store CacheJournalStore>,
}

#[allow(dead_code)] // Future lifecycle adapter must keep this value alive through lease claim.
impl OwnerDirectory<'_> {
    pub(crate) fn observation(&self) -> &CacheOwnerObservation {
        &self.observation
    }
}

impl CacheJournalStore {
    /// Open only a root-owned chain ending in a no-list/no-write-for-others directory. This does
    /// not create the directory: provisioning its location and any UID search ACL is separate.
    pub fn open_root_owned(path: &Path) -> Result<Self, StoreError> {
        if nix::unistd::geteuid().as_raw() != 0 {
            return Err(StoreError::UnsafePath);
        }
        let directory = open_root_chain(path)?;
        Self::from_directory(directory, 0)
    }

    fn from_directory(directory: OwnedFd, owner_uid: u32) -> Result<Self, StoreError> {
        checked_directory(&directory, owner_uid, true)?;
        let lock_fd = match openat(&directory, LOCK, WRITE_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => {
                match openat(
                    &directory,
                    LOCK,
                    WRITE_FLAGS | OFlag::O_CREAT | OFlag::O_EXCL,
                    Mode::from_bits_truncate(0o600),
                ) {
                    Ok(fd) => {
                        fsync(&directory).map_err(|_| StoreError::Io)?;
                        fd
                    }
                    Err(Errno::EEXIST) => openat(&directory, LOCK, WRITE_FLAGS, Mode::empty())
                        .map_err(|_| StoreError::UnsafePath)?,
                    Err(_) => return Err(StoreError::Io),
                }
            }
            Err(_) => return Err(StoreError::UnsafePath),
        };
        checked_file(&lock_fd, owner_uid)?;
        let lock = Flock::lock(File::from(lock_fd), FlockArg::LockExclusiveNonblock).map_err(
            |(_, error)| match error {
                Errno::EWOULDBLOCK => StoreError::LockBusy,
                _ => StoreError::Io,
            },
        )?;
        // A cooperating daemon must hold the stable name, not an unlinked/replaced inode.
        let named = entry_stat(&directory, LOCK)?.ok_or(StoreError::UnsafePath)?;
        let held = fstat(&*lock).map_err(|_| StoreError::Io)?;
        if named.st_dev != held.st_dev || named.st_ino != held.st_ino {
            return Err(StoreError::UnsafePath);
        }
        let store = Self {
            directory,
            _lock: lock,
            owner_uid,
        };
        store.load_raw()?;
        Ok(store)
    }

    /// Diagnostic inspection deliberately cannot return a CacheJournal or an owner proof.
    pub fn load_diagnostic(&self) -> Result<Option<JournalDiagnostic>, StoreError> {
        Ok(self.load_raw()?.map(|journal| JournalDiagnostic {
            owner_ref: journal.owner_ref,
            owner_uid: journal.owner_uid,
            phase: phase_name(&journal.phase),
        }))
    }

    /// Parse protected bytes without admitting a phase. Only `load_for_phase` may return the
    /// journal for a transition after checking an authenticated owner UID and the real object.
    fn load_raw(&self) -> Result<Option<CacheJournal>, StoreError> {
        if entry_stat(&self.directory, NEXT)?.is_some() {
            return Err(StoreError::InterruptedWrite);
        }
        let fd = match openat(&self.directory, JOURNAL, READ_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => {
                return if entry_stat(&self.directory, OWNER)?.is_some() {
                    Err(StoreError::MissingJournalWithOwner)
                } else {
                    Ok(None)
                };
            }
            Err(_) => return Err(StoreError::UnsafePath),
        };
        checked_file(&fd, self.owner_uid)?;
        let metadata = fstat(&fd).map_err(|_| StoreError::Io)?;
        if metadata.st_size < 0 || metadata.st_size as u64 > MAX_JOURNAL_BYTES {
            return Err(StoreError::CorruptJournal);
        }
        let mut bytes = Vec::new();
        File::from(fd)
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StoreError::Io)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(StoreError::CorruptJournal);
        }
        let journal =
            CacheJournal::parse_canonical(&bytes).map_err(|_| StoreError::CorruptJournal)?;
        Ok(Some(journal))
    }

    #[allow(dead_code)] // No authenticated-claim constructor exists in this offline slice.
    pub(crate) fn load_for_phase(
        &self,
        auth: &AuthenticatedOwnerUid,
    ) -> Result<Option<CacheJournal>, StoreError> {
        if !polaris_helper_proto::exact::token(&auth.local_id)
            || !polaris_helper_proto::exact::token(&auth.owner_ref)
            || auth.uid == u32::MAX
        {
            return Err(StoreError::UnsafeOwner);
        }
        let journal = self.load_raw()?;
        if let Some(value) = &journal {
            if value.local_id != auth.local_id
                || value.owner_ref != auth.owner_ref
                || value.owner_uid != auth.uid
            {
                return Err(StoreError::UnsafeOwner);
            }
            self.verify_phase_owner(&value.phase, auth.uid)?;
        }
        Ok(journal)
    }

    /// Recovery refuses every ambiguous phase. The UID must eventually come from the
    /// authenticated lifecycle claim, not an app argument. This still does not prove old-core
    /// exit; `CacheJournal::recover_idle` requires a separate StoppedCacheFence.
    #[allow(dead_code)] // Offline until an authenticated UID and lifecycle gate are wired.
    pub(crate) fn load_idle_for_recovery(
        &self,
        auth: &AuthenticatedOwnerUid,
    ) -> Result<Option<(CacheJournal, OwnerDirectory<'_>)>, StoreError> {
        let journal = self.load_for_phase(auth)?;
        if journal
            .as_ref()
            .is_some_and(|value| !matches!(value.phase, CachePhase::Idle { .. }))
        {
            return Err(StoreError::UnsafeRecovery);
        }
        let Some(journal) = journal else {
            return Ok(None);
        };
        let owner = self.observe_owner(auth.uid)?;
        let CachePhase::Idle { owner_key, .. } = &journal.phase else {
            return Err(StoreError::UnsafeRecovery);
        };
        if owner.observation.owner_key != *owner_key {
            return Err(StoreError::UnsafeOwner);
        }
        owner.recheck(&self.directory)?;
        Ok(Some((journal, owner)))
    }

    /// Persist one H2a transition. `expected` is the exact preceding canonical value; an
    /// absent value may only start `migration_pending` while no owner entry exists. The store
    /// enforces disk CAS, not the external evidence needed to create a state-machine successor.
    #[allow(dead_code)] // H2b is offline; only the future helper-owned adapter may publish.
    pub(crate) fn compare_and_replace(
        &mut self,
        auth: &AuthenticatedOwnerUid,
        expected: Option<&CacheJournal>,
        next: &CacheJournal,
    ) -> Result<(), StoreError> {
        self.compare_and_replace_inner(auth, expected, next, WriteFault::None)
    }

    fn compare_and_replace_inner(
        &mut self,
        auth: &AuthenticatedOwnerUid,
        expected: Option<&CacheJournal>,
        next: &CacheJournal,
        fault: WriteFault,
    ) -> Result<(), StoreError> {
        let current = self.load_for_phase(auth)?;
        if next.local_id != auth.local_id
            || next.owner_ref != auth.owner_ref
            || next.owner_uid != auth.uid
        {
            return Err(StoreError::UnsafeOwner);
        }
        let current_bytes = current
            .as_ref()
            .map(CacheJournal::to_canonical_bytes)
            .transpose()
            .map_err(|_| StoreError::CorruptJournal)?;
        let expected_bytes = expected
            .map(CacheJournal::to_canonical_bytes)
            .transpose()
            .map_err(|_| StoreError::CorruptJournal)?;
        if current_bytes != expected_bytes {
            return Err(StoreError::StaleJournal);
        }
        match expected {
            None => {
                if !matches!(next.phase, CachePhase::MigrationPending { .. }) {
                    return Err(StoreError::UnsafeRecovery);
                }
                if entry_stat(&self.directory, OWNER)?.is_some() {
                    return Err(StoreError::MissingJournalWithOwner);
                }
            }
            Some(previous) if !transition_shape(previous, next) => {
                return Err(StoreError::UnsafeRecovery);
            }
            Some(_) => {}
        }
        self.verify_phase_owner(&next.phase, auth.uid)?;
        let bytes = next
            .to_canonical_bytes()
            .map_err(|_| StoreError::CorruptJournal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(StoreError::CorruptJournal);
        }
        // A stale .next is not auto-deleted: its presence makes recovery explicitly blocked.
        let temp_fd = openat(
            &self.directory,
            NEXT,
            WRITE_FLAGS | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|_| StoreError::InterruptedWrite)?;
        checked_file(&temp_fd, self.owner_uid)?;
        let mut temp = File::from(temp_fd);
        temp.write_all(&bytes).map_err(|_| StoreError::Io)?;
        temp.sync_all().map_err(|_| StoreError::Io)?;
        if fault == WriteFault::BeforeRename {
            return Err(StoreError::InterruptedWrite);
        }
        renameat(&self.directory, NEXT, &self.directory, JOURNAL).map_err(|_| StoreError::Io)?;
        if fault == WriteFault::AfterRename {
            return Err(StoreError::DurabilityUnknown);
        }
        fsync(&self.directory).map_err(|_| StoreError::DurabilityUnknown)?;
        Ok(())
    }

    fn observe_owner(&self, expected_uid: u32) -> Result<OwnerDirectory<'_>, StoreError> {
        let before = entry_stat(&self.directory, OWNER)?.ok_or(StoreError::UnsafeOwner)?;
        checked_owner(&before, expected_uid)?;
        let directory = openat(&self.directory, OWNER, DIR_FLAGS, Mode::empty())
            .map_err(|_| StoreError::UnsafeOwner)?;
        let opened = fstat(&directory).map_err(|_| StoreError::Io)?;
        checked_owner(&opened, expected_uid)?;
        if before.st_dev != opened.st_dev || before.st_ino != opened.st_ino {
            return Err(StoreError::UnsafeOwner);
        }
        let parent = fstat(&self.directory).map_err(|_| StoreError::Io)?;
        let owner = OwnerDirectory {
            observation: CacheOwnerObservation {
                owner_key: owner_object_key(expected_uid, &opened),
            },
            directory,
            parent_dev: parent.st_dev as u64,
            parent_ino: parent.st_ino as u64,
            expected_uid,
            _store: PhantomData,
        };
        owner.recheck(&self.directory)?;
        Ok(owner)
    }

    fn verify_phase_owner(&self, phase: &CachePhase, expected_uid: u32) -> Result<(), StoreError> {
        let Some(owner_key) = phase_owner_key(phase) else {
            return Ok(());
        };
        let observed = self.observe_owner(expected_uid)?;
        if observed.observation.owner_key != owner_key {
            return Err(StoreError::UnsafeOwner);
        }
        Ok(())
    }
}

fn owner_object_key(uid: u32, stat: &nix::sys::stat::FileStat) -> String {
    format!("unix-uid-{}-dev-{}-ino-{}", uid, stat.st_dev, stat.st_ino)
}

fn phase_owner_key(phase: &CachePhase) -> Option<&str> {
    match phase {
        CachePhase::MigrationPending { .. } => None,
        CachePhase::Idle { owner_key, .. }
        | CachePhase::Reserved { owner_key, .. }
        | CachePhase::Checked { owner_key, .. }
        | CachePhase::Active { owner_key, .. } => Some(owner_key),
    }
}

fn phase_name(phase: &CachePhase) -> &'static str {
    match phase {
        CachePhase::MigrationPending { .. } => "migration_pending",
        CachePhase::Idle { .. } => "idle",
        CachePhase::Reserved { .. } => "reserved",
        CachePhase::Checked { .. } => "checked",
        CachePhase::Active { .. } => "active",
    }
}

impl OwnerDirectory<'_> {
    fn recheck(&self, parent_fd: &impl AsFd) -> Result<(), StoreError> {
        let parent = fstat(parent_fd).map_err(|_| StoreError::Io)?;
        if parent.st_dev as u64 != self.parent_dev || parent.st_ino as u64 != self.parent_ino {
            return Err(StoreError::UnsafeOwner);
        }
        let opened = fstat(&self.directory).map_err(|_| StoreError::Io)?;
        let named = entry_stat(parent_fd, OWNER)?.ok_or(StoreError::UnsafeOwner)?;
        checked_owner(&opened, self.expected_uid)?;
        checked_owner(&named, self.expected_uid)?;
        if opened.st_dev != named.st_dev || opened.st_ino != named.st_ino {
            return Err(StoreError::UnsafeOwner);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // The two injection points are exercised only by offline storage tests.
enum WriteFault {
    None,
    BeforeRename,
    AfterRename,
}

fn transition_shape(previous: &CacheJournal, next: &CacheJournal) -> bool {
    if previous.schema_version != next.schema_version
        || previous.local_id != next.local_id
        || previous.owner_ref != next.owner_ref
        || previous.owner_uid != next.owner_uid
        || previous.source_path_sha256 != next.source_path_sha256
    {
        return false;
    }
    let same_seen = previous.seen_run_refs == next.seen_run_refs;
    match (&previous.phase, &next.phase) {
        (
            CachePhase::MigrationPending { source, .. },
            CachePhase::Idle {
                origin,
                last_released: None,
                ..
            },
        ) => same_seen && source == origin,
        (
            CachePhase::Idle {
                origin: old_origin,
                owner_key: old_owner,
                ..
            },
            CachePhase::Reserved {
                origin,
                owner_key,
                claim,
                ..
            },
        ) => {
            next.seen_run_refs.len() == previous.seen_run_refs.len() + 1
                && next.seen_run_refs.contains(&claim.run_ref)
                && previous.seen_run_refs.is_subset(&next.seen_run_refs)
                && old_origin == origin
                && old_owner == owner_key
        }
        (
            CachePhase::Reserved {
                origin: old_origin,
                owner_key: old_owner,
                claim: old_claim,
                launch: old_launch,
            },
            CachePhase::Checked {
                origin,
                owner_key,
                claim,
                launch,
            },
        )
        | (
            CachePhase::Checked {
                origin: old_origin,
                owner_key: old_owner,
                claim: old_claim,
                launch: old_launch,
            },
            CachePhase::Active {
                origin,
                owner_key,
                claim,
                launch,
                ..
            },
        ) => {
            same_seen
                && old_origin == origin
                && old_owner == owner_key
                && old_claim == claim
                && old_launch == launch
        }
        (
            CachePhase::Active {
                origin: old_origin,
                owner_key: old_owner,
                claim,
                ..
            },
            CachePhase::Idle {
                origin,
                owner_key,
                last_released,
            },
        ) => {
            same_seen
                && old_origin == origin
                && old_owner == owner_key
                && last_released.as_ref() == Some(&claim.marker())
        }
        _ => false,
    }
}

fn open_root_chain(path: &Path) -> Result<OwnedFd, StoreError> {
    let mut directory = open("/", DIR_FLAGS, Mode::empty()).map_err(|_| StoreError::UnsafePath)?;
    checked_directory(&directory, 0, false)?;
    let mut saw_child = false;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, Path::new(name), DIR_FLAGS, Mode::empty())
                    .map_err(|_| StoreError::UnsafePath)?;
                checked_directory(&directory, 0, false)?;
                saw_child = true;
            }
            _ => return Err(StoreError::UnsafePath),
        }
    }
    if !path.is_absolute() || !saw_child {
        return Err(StoreError::UnsafePath);
    }
    checked_directory(&directory, 0, true)?;
    Ok(directory)
}

fn checked_directory(fd: &impl AsFd, owner_uid: u32, private: bool) -> Result<(), StoreError> {
    let stat = fstat(fd).map_err(|_| StoreError::Io)?;
    let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
    if kind != SFlag::S_IFDIR
        || stat.st_uid != owner_uid
        || stat.st_mode & 0o022 != 0
        || (private && (stat.st_mode & 0o066 != 0 || stat.st_mode & 0o700 != 0o700))
    {
        return Err(StoreError::UnsafePath);
    }
    Ok(())
}

fn checked_file(fd: &impl AsFd, owner_uid: u32) -> Result<(), StoreError> {
    let stat = fstat(fd).map_err(|_| StoreError::Io)?;
    let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
    if kind != SFlag::S_IFREG
        || stat.st_nlink != 1
        || stat.st_uid != owner_uid
        || stat.st_mode & 0o177 != 0
    {
        return Err(StoreError::UnsafePath);
    }
    Ok(())
}

fn checked_owner(stat: &nix::sys::stat::FileStat, expected_uid: u32) -> Result<(), StoreError> {
    let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
    if kind != SFlag::S_IFDIR || stat.st_uid != expected_uid || stat.st_mode & 0o7777 != 0o700 {
        return Err(StoreError::UnsafeOwner);
    }
    Ok(())
}

fn entry_stat(
    directory: &impl AsFd,
    name: &str,
) -> Result<Option<nix::sys::stat::FileStat>, StoreError> {
    match fstatat(directory, OsStr::new(name), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(Errno::ENOENT) => Ok(None),
        Err(_) => Err(StoreError::Io),
    }
}

#[cfg(test)]
mod tests;

//! Candidate-first repair of one selected CoreId. The parent owns policy and
//! real store/exclusive-lease acquisition; mutation workers never reacquire or
//! release their caller's locks. The separate cold data diagnostic acquires and
//! releases its own existing locks, never starts a process or grants authority.
//! Failure retains the immutable plan and both object identities. A resumed
//! uncommitted plan retries the same verified candidate; a committed plan only
//! finishes retirement. Read-only handles are not treated as write exclusion.

mod journal;
mod objects;
#[cfg(test)]
mod tests;

use super::{invalid, private_mode, CoreStore, MutationLock};
use journal::{Plan, MAX_PLAN};
use objects::{Directory, Identity, Namespace, Regular};
use polaris_helper_proto::core_manifest::{CoreId, InstalledCoreManifest};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Non-creating cold observation using the transaction's existing namespace
/// judge and exact locks. A damaged member is not a selectable/admitted object.
pub(super) fn selected_recovery_damage(
    store: &CoreStore,
    compiled: &polaris_helper_proto::core_manifest::CoreManifest,
) -> io::Result<Option<bool>> {
    compiled.validate().map_err(super::invalid)?;
    let id = compiled
        .id(crate::core_hash::sha256_hex)
        .map_err(super::invalid)?;
    let mut namespace = Namespace::open(store)?;
    let mutation_path = store.root.join(".core-store.lock");
    store.owner.check(&mutation_path)?;
    // Unlike the installer, diagnosis never creates or seals a missing lock.
    let (mutation, _) = store._root_object.open_regular(".core-store.lock")?;
    if namespace.identity_at(&mutation_path, false)? != objects::identity(&mutation)? {
        return Err(invalid("recovery diagnostic mutation object changed"));
    }
    super::acquire_lock(&mutation, super::LockRole::StoreMutationExclusive)?;
    let result = (|| {
        store.require_no_pending_repair()?;
        if !store.has_selected_data(&id)? {
            return Ok(None);
        }
        let lease = store.lease_file(&id)?;
        super::acquire_lock(&lease, super::LockRole::RepairExclusive)?;
        let observation = (|| {
            let lease_path = store
                .root
                .join("leases")
                .join(format!("{}.lock", id.as_str()));
            let receipt_path = store
                .root
                .join("manifests")
                .join(format!("{}.json", id.as_str()));
            let receipt = namespace.regular(&receipt_path, 64 * 1024)?;
            let raw = namespace.read(&receipt_path, 64 * 1024)?;
            if raw.len() as u64 != receipt.bytes
                || crate::core_hash::sha256_hex(&raw) != receipt.sha256
            {
                return Err(invalid("recovery diagnostic receipt changed"));
            }
            let manifest = InstalledCoreManifest::parse(&raw, crate::core_hash::sha256_hex)
                .map_err(invalid)?;
            if manifest.id != id.as_str() || manifest.core.files != compiled.files {
                return Err(invalid("recovery diagnostic compiled identity differs"));
            }
            let directory = store.root.join("versions").join(id.as_str());
            namespace.anchor(&directory)?;
            let payload = namespace.directory(&directory)?;
            if payload
                .files
                .keys()
                .any(|name| !compiled.files.iter().any(|f| &f.name == name))
            {
                return Err(invalid("recovery diagnostic unknown member"));
            }
            let damaged = compiled.files.iter().any(|expected| {
                payload.files.get(&expected.name).is_none_or(|actual| {
                    actual.bytes != expected.bytes || actual.sha256 != expected.sha256
                })
            });
            // Same held namespace and exact named objects must survive hashing.
            if namespace.directory(&directory)? != payload
                || namespace.regular(&receipt_path, 64 * 1024)? != receipt
                || namespace.identity_at(&lease_path, false)? != objects::identity(&lease)?
                || namespace.identity_at(&mutation_path, false)? != objects::identity(&mutation)?
            {
                return Err(invalid("recovery diagnostic objects changed"));
            }
            store.require_no_pending_repair()?;
            namespace.check()?;
            Ok(Some(damaged))
        })();
        let release = File::unlock(&lease);
        release?;
        observation
    })();
    let release = File::unlock(&mutation);
    release?;
    result
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum RecoveryState {
    Untouched,
    OriginalsRestored,
    Unknown,
}

#[derive(Debug)]
pub(super) struct RepairError {
    pub cause: io::Error,
    pub recovery: RecoveryState,
    pub journal: Option<PathBuf>,
}

impl std::fmt::Display for RepairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "core repair {:?}: {}", self.recovery, self.cause)
    }
}
impl std::error::Error for RepairError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

impl From<RepairError> for io::Error {
    fn from(error: RepairError) -> Self {
        // Do not relabel Unknown as an ordinary successfully rolled-back error.
        let kind = if error.recovery == RecoveryState::Unknown {
            io::ErrorKind::Other
        } else {
            error.cause.kind()
        };
        io::Error::new(kind, error)
    }
}

#[derive(Debug)]
pub(super) enum RepairOutcome {
    NoPendingJournal,
    Installed {
        receipt: PathBuf,
    },
    /// Active candidate and commit are verified. Old objects or journal cleanup
    /// remain; the parent must surface this debt, not return an unqualified clean.
    InstalledRetirementPending {
        journal: PathBuf,
        cause: io::Error,
    },
}

impl RepairOutcome {
    /// Parent adapter: NoPendingJournal means continue preparation, not success.
    /// A committed repair with retirement debt stays an explicit error at the
    /// old io::Result<InstallOutcome> boundary; no clean result is fabricated.
    pub(super) fn into_install_result(self) -> io::Result<super::InstallOutcome> {
        match self {
            Self::Installed { .. } => Ok(super::InstallOutcome::Installed),
            Self::NoPendingJournal => Err(invalid("no repair installation result")),
            Self::InstalledRetirementPending { journal, cause } => {
                Err(io::Error::other(RetirementPending { journal, cause }))
            }
        }
    }
}

#[derive(Debug)]
struct RetirementPending {
    journal: PathBuf,
    cause: io::Error,
}

impl std::fmt::Display for RetirementPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "core repair committed; retirement pending at {}: {}",
            self.journal.display(),
            self.cause
        )
    }
}

impl std::error::Error for RetirementPending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// Publication durability uses the existing held namespace and the caller's
/// actual mutation/occupancy leases. It is not a second install transaction.
pub(super) struct Publication<'a> {
    namespace: Namespace<'a>,
    mutation: &'a MutationLock,
    selected: CoreId,
    lease: Identity,
}
impl<'a> Publication<'a> {
    pub(super) fn open(
        store: &'a CoreStore,
        selected: &CoreId,
        mutation: &'a MutationLock,
        lease: &File,
    ) -> io::Result<Self> {
        let namespace = Namespace::open(store)?;
        let value = Self {
            namespace,
            mutation,
            selected: selected.clone(),
            lease: objects::identity(lease)?,
        };
        value.check(lease)?;
        Ok(value)
    }
    fn check(&self, lease: &File) -> io::Result<()> {
        self.namespace.check()?;
        let mutation = self
            .mutation
            .file
            .as_ref()
            .ok_or_else(|| invalid("publication mutation owner absent"))?;
        if self.mutation.owner_pid != std::process::id()
            || self
                .namespace
                .identity_at(&self.namespace.store.root.join(".core-store.lock"), false)?
                != objects::identity(mutation)?
            || self.namespace.identity_at(
                &self
                    .namespace
                    .store
                    .root
                    .join("leases")
                    .join(format!("{}.lock", self.selected.as_str())),
                false,
            )? != self.lease
            || objects::identity(lease)? != self.lease
        {
            return Err(invalid("publication lease or namespace changed"));
        }
        Ok(())
    }
    pub(super) fn finish(&mut self, lease: &File) -> io::Result<()> {
        self.check(lease)?;
        let store = self.namespace.store;
        let version = store.root.join("versions").join(self.selected.as_str());
        self.namespace.anchor(&version)?;
        checkpoint(Point::PublicationPayloadSync)?;
        self.namespace.sync(&version)?;
        checkpoint(Point::PublicationStagingSync)?;
        self.namespace.sync(&store.root.join(".staging"))?;
        checkpoint(Point::PublicationVersionsSync)?;
        self.namespace.sync(&store.root.join("versions"))?;
        checkpoint(Point::PublicationLeaseSync)?;
        #[cfg(windows)]
        self.namespace.sync_held_lease(
            &store
                .root
                .join("leases")
                .join(format!("{}.lock", self.selected.as_str())),
            lease,
        )?;
        #[cfg(not(windows))]
        lease.sync_all()?;
        checkpoint(Point::PublicationLeasesSync)?;
        self.namespace.sync(&store.root.join("leases"))?;
        let receipt = store
            .root
            .join("manifests")
            .join(format!("{}.json", self.selected.as_str()));
        let record = self.namespace.regular(&receipt, 64 * 1024)?;
        checkpoint(Point::PublicationReceiptSync)?;
        self.namespace.sync_regular(&receipt, &record)?;
        checkpoint(Point::PublicationManifestsSync)?;
        self.namespace.sync(&store.root.join("manifests"))?;
        checkpoint(Point::PublicationRootSync)?;
        self.namespace.sync(&store.root)?;
        // The caller may have just created the store and its data parents.
        // Sync actual directory handles up to the existing root, never a volume
        // device, and do not turn an unsupported barrier into success.
        let parents = store.root.ancestors().skip(1);
        // The native user-data constructor can create only the last component;
        // all higher ancestors already exist. Only its immediately enclosing
        // entry changed. Do not request write access/flush on untouched Windows
        // system ancestors (or a volume) to publish a private data directory.
        #[cfg(windows)]
        let parents = parents.take(if store.owner.data.is_some() {
            1
        } else {
            usize::MAX
        });
        for parent in parents {
            checkpoint(Point::PublicationParentSync)?;
            self.namespace.sync(parent)?;
        }
        self.check(lease)
    }
}

pub(super) struct PreparedCandidate {
    selected: CoreId,
    stage: String,
    directory: Directory,
    manifest: Vec<u8>,
    root: File,
    stage_object: File,
}

impl PreparedCandidate {
    /// Only a completed helper-owned, direct staging child is eligible. The
    /// parent must create its new inodes privately and close all copy writers.
    pub(super) fn open(
        store: &CoreStore,
        manifest: &InstalledCoreManifest,
        stage: &Path,
    ) -> io::Result<Self> {
        manifest
            .validate(crate::core_hash::sha256_hex)
            .map_err(invalid)?;
        if stage.parent() != Some(store.root.join(".staging").as_path()) {
            return Err(invalid("candidate is not a direct store staging child"));
        }
        let name = stage
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("invalid stage name"))?;
        objects::leaf(name)?;
        if name.starts_with("repair-") {
            return Err(invalid("reserved repair stage name"));
        }
        let namespace = Namespace::open(store)?;
        let stage_object = namespace.open_object(stage, true)?;
        let directory = namespace.directory(stage)?;
        verify_selected(&directory, manifest)?;
        checkpoint(Point::StageSync)?;
        for (name, record) in &directory.files {
            namespace.sync_regular(&stage.join(name), record)?;
        }
        namespace.sync(stage)?;
        namespace.sync(&store.root.join(".staging"))?;
        if namespace.directory(stage)? != directory {
            return Err(invalid("candidate changed during prepare"));
        }
        Ok(Self {
            selected: CoreId::parse(&manifest.id).map_err(invalid)?,
            stage: name.into(),
            directory,
            manifest: manifest.to_json().map_err(invalid)?,
            root: namespace.open_object(&store.root, true)?,
            stage_object,
        })
    }
}

/// Guards are borrowed until publication, rollback and retirement have ended.
/// `exclusive_lease` MUST be the parent's already-acquired exclusive guard;
/// object identity checks below do not establish the lock's exclusivity.
pub(super) fn replace(
    store: &CoreStore,
    selected: &CoreId,
    candidate: PreparedCandidate,
    mutation: &MutationLock,
    exclusive_lease: &File,
    cancel: &AtomicBool,
) -> Result<RepairOutcome, RepairError> {
    let path = journal_path(store, selected);
    let mut ns = Namespace::open(store).map_err(|cause| RepairError {
        cause,
        recovery: RecoveryState::Untouched,
        journal: None,
    })?;
    let mut draft_directory = None;
    let mut draft_manifest = None;
    let mut draft_plan = None;
    let mut prepare = || -> io::Result<(Plan, Vec<u8>)> {
        canceled(cancel)?;
        check_scope(&ns, selected, mutation, exclusive_lease)?;
        if candidate.selected != *selected
            || objects::identity(&candidate.root)? != ns.identity_at(&store.root, true)?
            || objects::identity(&candidate.stage_object)? != candidate.directory.identity
            || ns.directory(&store.root.join(".staging").join(&candidate.stage))?
                != candidate.directory
        {
            return Err(invalid("prepared candidate binding differs"));
        }
        objects::absent(&path)?;
        // Probe all required directory barriers before changing active state.
        for parent in ["versions", "manifests", ".staging", ".trash"] {
            ns.sync(&store.root.join(parent))?;
        }
        let original = optional_directory(&ns, &version_path(store, selected))?;
        if original.as_ref().is_some_and(|old| {
            old.files
                .keys()
                .any(|name| !candidate.directory.files.contains_key(name))
        }) {
            return Err(invalid("unexpected original version members retained"));
        }
        let original_manifest = optional_regular(&ns, &manifest_path(store, selected))?;
        checkpoint(Point::JournalCreate)?;
        ns.create_directory(&path, &mut draft_directory)?;
        ns.anchor(&path)?;
        ns.sync(&store.root.join(".staging"))?;
        ns.create(
            &path.join("candidate-manifest"),
            &candidate.manifest,
            &mut draft_manifest,
        )?;
        #[cfg(windows)]
        store.owner.mode(&path.join("candidate-manifest"), 0o644)?;
        #[cfg(not(windows))]
        private_mode(&path.join("candidate-manifest"), 0o644)?;
        let plan = Plan {
            selected: selected.clone(),
            root: ns.identity_at(&store.root, true)?,
            transaction: ns.identity_at(&path, true)?,
            lease: objects::identity(exclusive_lease)?,
            stage: candidate.stage.clone(),
            candidate: candidate.directory.clone(),
            candidate_manifest: ns.regular(&path.join("candidate-manifest"), 64 * 1024)?,
            original,
            original_manifest,
        };
        let raw = plan.encode()?;
        checkpoint(Point::PlanWrite)?;
        ns.create(&path.join("plan"), &raw, &mut draft_plan)?;
        ns.sync(&path)?;
        ns.sync(&store.root.join(".staging"))?;
        checkpoint(Point::PlanDurable)?;
        canceled(cancel)?;
        Ok((plan, raw))
    };
    let (plan, raw) = match prepare() {
        Ok(value) => value,
        Err(cause) => {
            let cleanup = if let Some(directory) = draft_directory.as_ref() {
                ns.discard_draft(
                    &path,
                    directory,
                    &[
                        ("candidate-manifest", draft_manifest.as_ref()),
                        ("plan", draft_plan.as_ref()),
                    ],
                )
            } else {
                Ok(())
            };
            let recovery = if cleanup.is_ok() {
                RecoveryState::Untouched
            } else {
                RecoveryState::Unknown
            };
            let cause = match cleanup {
                Ok(()) => cause,
                Err(error) => {
                    io::Error::new(cause.kind(), format!("{cause}; draft retained: {error}"))
                }
            };
            return Err(RepairError {
                cause,
                recovery,
                journal: objects::exists(&path).unwrap_or(true).then(|| path.clone()),
            });
        }
    };
    execute(ns, plan, raw, mutation, exclusive_lease, cancel)
}

/// Call before starting a fresh repair while holding the same real guards.
/// Malformed/foreign/torn records are Unknown, never automatically deleted.
pub(super) fn resume(
    store: &CoreStore,
    selected: &CoreId,
    mutation: &MutationLock,
    exclusive_lease: &File,
    cancel: &AtomicBool,
) -> Result<RepairOutcome, RepairError> {
    let path = journal_path(store, selected);
    let load = || -> io::Result<Option<(Namespace<'_>, Plan, Vec<u8>)>> {
        canceled(cancel)?;
        let mut ns = Namespace::open(store)?;
        check_scope(&ns, selected, mutation, exclusive_lease)?;
        if !objects::exists(&path)? {
            return Ok(None);
        }
        ns.anchor(&path)?;
        let raw = ns.read(&path.join("plan"), MAX_PLAN)?;
        let plan = Plan::parse(&raw)?;
        if plan.selected != *selected
            || plan.root != ns.identity_at(&store.root, true)?
            || plan.transaction != ns.identity_at(&path, true)?
        {
            return Err(invalid("repair journal belongs to different objects"));
        }
        Ok(Some((ns, plan, raw)))
    };
    match load().map_err(|cause| RepairError {
        cause,
        recovery: RecoveryState::Unknown,
        journal: Some(path),
    })? {
        None => replay_archived(store, selected, mutation, exclusive_lease, cancel),
        Some((ns, plan, raw)) => execute(ns, plan, raw, mutation, exclusive_lease, cancel),
    }
}

pub(super) const MAX_REPAIR_ENTRIES: usize = 1024;

/// Inventory is administrative data only. Every matching record, including a
/// malformed one, must reach strict verification before it can be forgotten.
fn archived_names(store: &CoreStore) -> io::Result<Vec<(CoreId, String)>> {
    let mut directory =
        crate::core_objects::PayloadDir::open(&store.root.join(".trash"), store.owner.uid)?;
    let mut records = Vec::new();
    for entry in directory.entry_names(MAX_REPAIR_ENTRIES)? {
        let Some(name) = entry.to_str() else {
            return Err(invalid("unrepresentable trash entry"));
        };
        let Some(rest) = name.strip_prefix("repair-") else {
            continue;
        };
        let (id, digest) = rest
            .split_once('-')
            .ok_or_else(|| invalid("invalid archived repair name"))?;
        let id = CoreId::parse(id).map_err(invalid)?;
        CoreId::parse(digest).map_err(invalid)?;
        records.push((id, name.to_owned()));
    }
    records.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(records)
}
pub(super) fn has_records(store: &CoreStore, selected: &CoreId) -> io::Result<bool> {
    Ok(objects::exists(&journal_path(store, selected))?
        || archived_names(store)?.iter().any(|(id, _)| id == selected))
}

/// An archived journal can be a completed audit or a rename whose directory
/// barrier failed. Both have exactly plan+commit and no retained data objects.
/// Replay the bounded barriers every administrative retry; this needs no new
/// permanent marker and does not bind historical candidate inodes forever.
fn replay_archived(
    store: &CoreStore,
    selected: &CoreId,
    mutation: &MutationLock,
    lease: &File,
    cancel: &AtomicBool,
) -> Result<RepairOutcome, RepairError> {
    let records = archived_names(store).map_err(|cause| RepairError {
        cause,
        recovery: RecoveryState::Unknown,
        journal: None,
    })?;
    for (_, name) in records.into_iter().filter(|(id, _)| id == selected) {
        let path = store.root.join(".trash").join(&name);
        let verify = || -> io::Result<Namespace<'_>> {
            let mut ns = Namespace::open(store)?;
            check_scope(&ns, selected, mutation, lease)?;
            ns.anchor(&path)?;
            let mut entries =
                crate::core_objects::PayloadDir::open(&path, store.owner.uid)?.entry_names(2)?;
            entries.sort();
            if entries
                != [
                    std::ffi::OsString::from("commit"),
                    std::ffi::OsString::from("plan"),
                ]
            {
                return Err(invalid(
                    "archived repair still contains data or foreign entries",
                ));
            }
            let raw = ns.read(&path.join("plan"), MAX_PLAN)?;
            let plan = Plan::parse(&raw)?;
            if plan.selected != *selected
                || plan.root != ns.identity_at(&store.root, true)?
                || plan.transaction != ns.identity_at(&path, true)?
                || plan.lease != objects::identity(lease)?
                || name
                    != format!(
                        "repair-{}-{}",
                        selected.as_str(),
                        crate::core_hash::sha256_hex(&raw)
                    )
                || ns.read(&path.join("commit"), 256)? != Plan::commit_bytes(&raw)
            {
                return Err(invalid("archived repair binding differs"));
            }
            Ok(ns)
        };
        let ns = verify().map_err(|cause| RepairError {
            cause,
            recovery: RecoveryState::Unknown,
            journal: Some(path.clone()),
        })?;
        let barriers = (|| -> io::Result<()> {
            canceled(cancel)?;
            checkpoint(Point::ArchiveDirectorySync)?;
            ns.sync(&path)?;
            checkpoint(Point::ArchiveStagingSync)?;
            ns.sync(&store.root.join(".staging"))?;
            checkpoint(Point::ArchiveTrashSync)?;
            ns.sync(&store.root.join(".trash"))?;
            Ok(())
        })();
        if let Err(cause) = barriers {
            if ns.check().is_err() {
                return Err(RepairError {
                    cause,
                    recovery: RecoveryState::Unknown,
                    journal: Some(path),
                });
            }
            return Ok(RepairOutcome::InstalledRetirementPending {
                journal: path,
                cause,
            });
        }
    }
    Ok(RepairOutcome::NoPendingJournal)
}

fn execute(
    mut ns: Namespace<'_>,
    plan: Plan,
    raw: Vec<u8>,
    mutation: &MutationLock,
    lease: &File,
    cancel: &AtomicBool,
) -> Result<RepairOutcome, RepairError> {
    let path = journal_path(ns.store, &plan.selected);
    let mut transaction = Transaction {
        namespace: &mut ns,
        plan: &plan,
        path: &path,
    };
    let committed = (|| {
        check_scope(transaction.namespace, &plan.selected, mutation, lease)?;
        if plan.lease != objects::identity(lease)? {
            return Err(invalid("repair journal lease object changed"));
        }
        transaction.check_entries()?;
        let commit = path.join("commit");
        if !objects::exists(&commit)? {
            return Ok(false);
        }
        if transaction.namespace.read(&commit, 256)? != Plan::commit_bytes(&raw) {
            return Err(invalid("repair commit record differs"));
        }
        transaction.verify_active()?;
        transaction
            .namespace
            .sync_regular(&commit, &transaction.namespace.regular(&commit, 256)?)?;
        transaction.namespace.sync(&path)?;
        Ok(true)
    })()
    .map_err(|cause| RepairError {
        cause,
        recovery: RecoveryState::Unknown,
        journal: Some(path.clone()),
    })?;
    if !committed {
        if let Err(cause) = transaction.publish(cancel) {
            let rollback = transaction.rollback();
            return Err(RepairError {
                cause: match rollback.as_ref() {
                    Ok(_) => cause,
                    Err(error) => {
                        io::Error::new(cause.kind(), format!("{cause}; rollback failed: {error}"))
                    }
                },
                recovery: if rollback.is_ok() {
                    RecoveryState::OriginalsRestored
                } else {
                    RecoveryState::Unknown
                },
                journal: Some(path.clone()),
            });
        }
        // From the first commit-record write onward failure is ambiguous. Never
        // erase that record or claim a rollback after a possibly durable commit.
        let commit_result = (|| {
            checkpoint(Point::CommitWrite)?;
            transaction.namespace.create(
                &path.join("commit"),
                &Plan::commit_bytes(&raw),
                &mut None,
            )?;
            checkpoint(Point::CommitSync)?;
            transaction.namespace.sync(&path)?;
            transaction
                .namespace
                .sync(&transaction.namespace.store.root)?;
            checkpoint(Point::Committed)
        })();
        if let Err(cause) = commit_result {
            return Err(RepairError {
                cause,
                recovery: RecoveryState::Unknown,
                journal: Some(path.clone()),
            });
        }
    }
    // Cancellation after durable commit postpones retirement; it cannot undo it.
    let receipt = transaction
        .namespace
        .store
        .root
        .join(".trash")
        .join(format!(
            "repair-{}-{}",
            plan.selected.as_str(),
            crate::core_hash::sha256_hex(&raw)
        ));
    let retirement = (|| {
        canceled(cancel)?;
        transaction.retire(cancel)?;
        checkpoint(Point::Receipt)?;
        transaction.namespace.release_anchor(&path)?;
        transaction
            .namespace
            .rename(&path, &receipt, &plan.transaction, true)?;
        Ok(receipt.clone())
    })();
    match retirement {
        Ok(receipt) => Ok(RepairOutcome::Installed { receipt }),
        Err(cause) => {
            let retained = if transaction
                .namespace
                .identity_at(&receipt, true)
                .is_ok_and(|id| id == plan.transaction)
            {
                receipt
            } else {
                path
            };
            Ok(RepairOutcome::InstalledRetirementPending {
                journal: retained,
                cause,
            })
        }
    }
}

struct Transaction<'a, 's> {
    namespace: &'a mut Namespace<'s>,
    plan: &'a Plan,
    path: &'a Path,
}

impl Transaction<'_, '_> {
    fn active_version(&self) -> PathBuf {
        version_path(self.namespace.store, &self.plan.selected)
    }
    fn active_manifest(&self) -> PathBuf {
        manifest_path(self.namespace.store, &self.plan.selected)
    }
    fn stage(&self) -> PathBuf {
        self.namespace
            .store
            .root
            .join(".staging")
            .join(&self.plan.stage)
    }

    fn check_entries(&self) -> io::Result<()> {
        self.namespace.check()?;
        let mut directory =
            crate::core_objects::PayloadDir::open(self.path, self.namespace.store.owner.uid)?;
        for entry in directory.entry_names(6)? {
            if !matches!(
                entry.to_str(),
                Some(
                    "plan"
                        | "commit"
                        | "candidate"
                        | "candidate-manifest"
                        | "original-version"
                        | "original-manifest"
                )
            ) {
                return Err(invalid("unrecognized repair journal member"));
            }
        }
        Ok(())
    }

    fn locate(
        &self,
        paths: &[PathBuf],
        expected: &Identity,
        directory: bool,
    ) -> io::Result<PathBuf> {
        let mut found = None;
        for path in paths {
            if objects::exists(path)? && self.namespace.identity_at(path, directory)? == *expected {
                if found.replace(path.clone()).is_some() {
                    return Err(invalid("repair object appears in multiple slots"));
                }
            }
        }
        found.ok_or_else(|| invalid("repair object missing from owned slots"))
    }

    fn allowed(&self, path: &Path, expected: &[&Identity], directory: bool) -> io::Result<()> {
        if objects::exists(path)?
            && !expected.contains(&&self.namespace.identity_at(path, directory)?)
        {
            return Err(invalid("foreign object occupies repair slot"));
        }
        Ok(())
    }

    fn layout(&self) -> io::Result<()> {
        self.check_entries()?;
        let new = &self.plan.candidate.identity;
        let mut active_versions = vec![new];
        if let Some(old) = &self.plan.original {
            active_versions.push(&old.identity);
        }
        self.allowed(&self.active_version(), &active_versions, true)?;
        self.allowed(&self.stage(), &[new], true)?;
        self.allowed(&self.path.join("candidate"), &[new], true)?;
        self.allowed(
            &self.path.join("original-version"),
            &self
                .plan
                .original
                .iter()
                .map(|d| &d.identity)
                .collect::<Vec<_>>(),
            true,
        )?;
        let new = &self.plan.candidate_manifest.identity;
        let mut active_manifests = vec![new];
        if let Some(old) = &self.plan.original_manifest {
            active_manifests.push(&old.identity);
        }
        self.allowed(&self.active_manifest(), &active_manifests, false)?;
        self.allowed(&self.path.join("candidate-manifest"), &[new], false)?;
        self.allowed(
            &self.path.join("original-manifest"),
            &self
                .plan
                .original_manifest
                .iter()
                .map(|f| &f.identity)
                .collect::<Vec<_>>(),
            false,
        )
    }

    fn candidate_location(&self) -> io::Result<PathBuf> {
        self.locate(
            &[
                self.stage(),
                self.path.join("candidate"),
                self.active_version(),
            ],
            &self.plan.candidate.identity,
            true,
        )
    }
    fn manifest_location(&self) -> io::Result<PathBuf> {
        self.locate(
            &[self.path.join("candidate-manifest"), self.active_manifest()],
            &self.plan.candidate_manifest.identity,
            false,
        )
    }

    fn verify_candidate(&self) -> io::Result<()> {
        let directory = self.candidate_location()?;
        let manifest = self.manifest_location()?;
        if self.namespace.directory(&directory)? != self.plan.candidate
            || self.namespace.regular(&manifest, 64 * 1024)? != self.plan.candidate_manifest
        {
            return Err(invalid("candidate changed before commit"));
        }
        let selected = InstalledCoreManifest::parse(
            &self.namespace.read(&manifest, 64 * 1024)?,
            crate::core_hash::sha256_hex,
        )
        .map_err(invalid)?;
        if selected.id != self.plan.selected.as_str() {
            return Err(invalid("resumed candidate selection differs"));
        }
        verify_selected(&self.plan.candidate, &selected)
    }

    fn verify_originals(&self) -> io::Result<()> {
        if let Some(old) = &self.plan.original {
            let at = self.locate(
                &[self.active_version(), self.path.join("original-version")],
                &old.identity,
                true,
            )?;
            if self.namespace.directory(&at)? != *old {
                return Err(invalid("original version changed"));
            }
        }
        if let Some(old) = &self.plan.original_manifest {
            let at = self.locate(
                &[self.active_manifest(), self.path.join("original-manifest")],
                &old.identity,
                false,
            )?;
            if self.namespace.regular(&at, 64 * 1024)? != *old {
                return Err(invalid("original manifest changed"));
            }
        }
        Ok(())
    }

    fn move_to(
        &self,
        from: &Path,
        to: &Path,
        identity: &Identity,
        directory: bool,
        point: Point,
    ) -> io::Result<()> {
        if from == to {
            return Ok(());
        }
        checkpoint(point)?;
        self.layout()?;
        self.namespace.rename(from, to, identity, directory)
    }

    fn publish(&self, cancel: &AtomicBool) -> io::Result<()> {
        self.layout()?;
        self.verify_candidate()?;
        self.verify_originals()?;
        canceled(cancel)?;
        let candidate = self.candidate_location()?;
        if candidate == self.stage() {
            self.move_to(
                &candidate,
                &self.path.join("candidate"),
                &self.plan.candidate.identity,
                true,
                Point::CandidateMove,
            )?;
        }
        if let Some(old) = &self.plan.original {
            canceled(cancel)?;
            let from = self.locate(
                &[self.active_version(), self.path.join("original-version")],
                &old.identity,
                true,
            )?;
            self.move_to(
                &from,
                &self.path.join("original-version"),
                &old.identity,
                true,
                Point::PreserveVersion,
            )?;
        }
        if let Some(old) = &self.plan.original_manifest {
            canceled(cancel)?;
            let from = self.locate(
                &[self.active_manifest(), self.path.join("original-manifest")],
                &old.identity,
                false,
            )?;
            self.move_to(
                &from,
                &self.path.join("original-manifest"),
                &old.identity,
                false,
                Point::PreserveManifest,
            )?;
        }
        canceled(cancel)?;
        self.move_to(
            &self.candidate_location()?,
            &self.active_version(),
            &self.plan.candidate.identity,
            true,
            Point::PublishVersion,
        )?;
        canceled(cancel)?;
        self.move_to(
            &self.manifest_location()?,
            &self.active_manifest(),
            &self.plan.candidate_manifest.identity,
            false,
            Point::PublishManifest,
        )?;
        checkpoint(Point::PublishedSync)?;
        self.namespace.sync(&self.active_version())?;
        self.namespace
            .sync(&self.namespace.store.root.join("versions"))?;
        self.namespace
            .sync(&self.namespace.store.root.join("manifests"))?;
        self.namespace.sync(self.path)?;
        self.verify_active()?;
        self.verify_originals()?;
        canceled(cancel)
    }

    fn verify_active(&self) -> io::Result<()> {
        self.layout()?;
        if self.candidate_location()? != self.active_version()
            || self.manifest_location()? != self.active_manifest()
        {
            return Err(invalid("committed pair is not active"));
        }
        self.verify_candidate()
    }

    fn rollback(&self) -> io::Result<()> {
        checkpoint(Point::Rollback)?;
        self.layout()?;
        self.verify_originals()?;
        // A damaged candidate may be retained for diagnosis. Restoring old
        // objects requires its exact identity, not its successful content hash.
        let manifest = self.manifest_location()?;
        self.move_to(
            &manifest,
            &self.path.join("candidate-manifest"),
            &self.plan.candidate_manifest.identity,
            false,
            Point::RollbackManifest,
        )?;
        let candidate = self.candidate_location()?;
        self.move_to(
            &candidate,
            &self.path.join("candidate"),
            &self.plan.candidate.identity,
            true,
            Point::RollbackVersion,
        )?;
        if let Some(old) = &self.plan.original {
            let from = self.locate(
                &[self.active_version(), self.path.join("original-version")],
                &old.identity,
                true,
            )?;
            self.move_to(
                &from,
                &self.active_version(),
                &old.identity,
                true,
                Point::RestoreVersion,
            )?;
        }
        if let Some(old) = &self.plan.original_manifest {
            let from = self.locate(
                &[self.active_manifest(), self.path.join("original-manifest")],
                &old.identity,
                false,
            )?;
            self.move_to(
                &from,
                &self.active_manifest(),
                &old.identity,
                false,
                Point::RestoreManifest,
            )?;
        }
        self.namespace
            .sync(&self.namespace.store.root.join("versions"))?;
        self.namespace
            .sync(&self.namespace.store.root.join("manifests"))?;
        self.namespace.sync(self.path)?;
        self.verify_originals()?;
        if self.plan.original.is_none() {
            objects::absent(&self.active_version())?;
        }
        if self.plan.original_manifest.is_none() {
            objects::absent(&self.active_manifest())?;
        }
        // locate() permits a backup. Explicitly require restored active names.
        if let Some(old) = &self.plan.original {
            if self.namespace.identity_at(&self.active_version(), true)? != old.identity {
                return Err(invalid("version rollback incomplete"));
            }
        }
        if let Some(old) = &self.plan.original_manifest {
            if self.namespace.identity_at(&self.active_manifest(), false)? != old.identity {
                return Err(invalid("manifest rollback incomplete"));
            }
        }
        checkpoint(Point::RollbackSync)
    }

    fn retire(&self, cancel: &AtomicBool) -> io::Result<()> {
        self.verify_active()?;
        checkpoint(Point::RetireVersion)?;
        if let Some(old) = &self.plan.original {
            let path = self.path.join("original-version");
            if objects::exists(&path)? {
                if self.namespace.identity_at(&path, true)? != old.identity {
                    return Err(invalid("foreign retirement directory"));
                }
                // A previous committed retirement may already have removed a
                // subset. Unknown members or changed surviving objects stop it.
                let remaining = self.namespace.directory(&path)?;
                for (name, actual) in &remaining.files {
                    if old.files.get(name) != Some(actual) {
                        return Err(invalid("retirement member differs"));
                    }
                }
                for (name, actual) in &remaining.files {
                    canceled(cancel)?;
                    checkpoint(Point::RetireMember)?;
                    canceled(cancel)?;
                    self.namespace.check()?;
                    if self
                        .namespace
                        .regular(&path.join(name), objects::MAX_BYTES)?
                        != *actual
                    {
                        return Err(invalid("retirement member replaced"));
                    }
                    fs::remove_file(path.join(name))?;
                    self.namespace.sync(&path)?;
                }
                self.namespace.check()?;
                if self.namespace.identity_at(&path, true)? != old.identity {
                    return Err(invalid("retirement directory replaced"));
                }
                fs::remove_dir(&path)?;
                self.namespace.sync(self.path)?;
            }
        }
        checkpoint(Point::RetireManifest)?;
        canceled(cancel)?;
        if let Some(old) = &self.plan.original_manifest {
            let path = self.path.join("original-manifest");
            if objects::exists(&path)? {
                if self.namespace.regular(&path, 64 * 1024)? != *old {
                    return Err(invalid("foreign retirement manifest"));
                }
                fs::remove_file(path)?;
                self.namespace.sync(self.path)?;
            }
        }
        self.check_entries()?;
        for name in [
            "candidate",
            "candidate-manifest",
            "original-version",
            "original-manifest",
        ] {
            objects::absent(&self.path.join(name))?;
        }
        Ok(())
    }
}

fn check_scope(
    ns: &Namespace<'_>,
    selected: &CoreId,
    mutation: &MutationLock,
    lease: &File,
) -> io::Result<()> {
    let lock = mutation
        .file
        .as_ref()
        .ok_or_else(|| invalid("mutation guard released"))?;
    if mutation.owner_pid != std::process::id()
        || objects::identity(lock)?
            != ns.identity_at(&ns.store.root.join(".core-store.lock"), false)?
        || objects::identity(lease)?
            != ns.identity_at(
                &ns.store
                    .root
                    .join("leases")
                    .join(format!("{}.lock", selected.as_str())),
                false,
            )?
    {
        return Err(invalid("repair scope object or owner differs"));
    }
    ns.check()
}

fn verify_selected(directory: &Directory, manifest: &InstalledCoreManifest) -> io::Result<()> {
    if directory.files.len() != manifest.core.files.len() {
        return Err(invalid("candidate member set differs"));
    }
    for expected in &manifest.core.files {
        let actual = directory
            .files
            .get(&expected.name)
            .ok_or_else(|| invalid("missing candidate member"))?;
        if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
            return Err(invalid("candidate content differs"));
        }
    }
    Ok(())
}

fn optional_directory(ns: &Namespace<'_>, path: &Path) -> io::Result<Option<Directory>> {
    if objects::exists(path)? {
        ns.directory(path).map(Some)
    } else {
        Ok(None)
    }
}
fn optional_regular(ns: &Namespace<'_>, path: &Path) -> io::Result<Option<Regular>> {
    if objects::exists(path)? {
        ns.regular(path, 64 * 1024).map(Some)
    } else {
        Ok(None)
    }
}
pub(super) fn journal_path(store: &CoreStore, selected: &CoreId) -> PathBuf {
    store
        .root
        .join(".staging")
        .join(format!("repair-{}", selected.as_str()))
}
fn version_path(store: &CoreStore, selected: &CoreId) -> PathBuf {
    store.root.join("versions").join(selected.as_str())
}
fn manifest_path(store: &CoreStore, selected: &CoreId) -> PathBuf {
    store
        .root
        .join("manifests")
        .join(format!("{}.json", selected.as_str()))
}
fn canceled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "core repair canceled",
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Point {
    StageSync,
    JournalCreate,
    PlanWrite,
    PlanDurable,
    CandidateMove,
    PreserveVersion,
    PreserveManifest,
    PublishVersion,
    PublishManifest,
    PublishedSync,
    CommitWrite,
    CommitSync,
    Committed,
    Rollback,
    RollbackManifest,
    RollbackVersion,
    RestoreVersion,
    RestoreManifest,
    RollbackSync,
    RetireVersion,
    RetireMember,
    RetireManifest,
    Receipt,
    ArchiveDirectorySync,
    ArchiveStagingSync,
    ArchiveTrashSync,
    FreshLeasePublished,
    FreshVersionPublished,
    FreshReceiptPublished,
    PublicationPayloadSync,
    PublicationStagingSync,
    PublicationVersionsSync,
    PublicationLeaseSync,
    PublicationLeasesSync,
    PublicationReceiptSync,
    PublicationManifestsSync,
    PublicationRootSync,
    PublicationParentSync,
}

pub(super) fn checkpoint(point: Point) -> io::Result<()> {
    #[cfg(test)]
    return tests::checkpoint(point);
    #[cfg(not(test))]
    {
        let _ = point;
        Ok(())
    }
}

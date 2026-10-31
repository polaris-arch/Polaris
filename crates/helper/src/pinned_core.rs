//! Shared read-only core status used by the three authenticated dispatchers.
use crate::core_store::{CoreStore, OwnerPolicy, VerifiedCore};
use polaris_helper_proto::{core_manifest::CoreId, core_protocol::CoreStatus};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
type CacheEntry = (PathBuf, CoreId, OwnerPolicy, Arc<VerifiedCore>);
static SLOT: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn diagnostic_entry(
    root: &Path,
    id: &CoreId,
    owner: OwnerPolicy,
    verified: VerifiedCore,
) -> std::io::Result<CacheEntry> {
    let core = Arc::new(verified.diagnostic_cache()?);
    // Drop the verification lease before publishing the diagnostic cache.
    drop(verified);
    Ok((root.into(), id.clone(), owner, core))
}

#[cfg(test)]
pub(crate) fn cache_storage_fixture(
    root: &Path,
    id: &CoreId,
    owner: OwnerPolicy,
    verified: VerifiedCore,
) {
    // Storage-only fixture has no execution admission. Exercise the exact
    // cache/lease release step without adding a dependency test-admit API.
    assert!(verified
        .prepare(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain)
        .is_err());
    let entry = diagnostic_entry(root, id, owner, verified).unwrap();
    *SLOT.get_or_init(Default::default).lock().unwrap() = Some(entry);
}
pub(crate) fn invalidate_unused() {
    let mutex = SLOT.get_or_init(Default::default);
    {
        let mut slot = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = None;
        mutex.clear_poison();
    }
    polaris_core_supervisor::admitted_payload::invalidate_selected();
}

fn cached_verify(
    root: &Path,
    id: &CoreId,
    owner: OwnerPolicy,
) -> std::io::Result<Arc<VerifiedCore>> {
    let mut slot = SLOT
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| std::io::Error::other("core admission cache poisoned"))?;
    if let Some((r, i, old_owner, core)) = slot.as_ref() {
        if r == root && i == id && *old_owner == owner && core.is_current() {
            return Ok(Arc::clone(core));
        }
    }
    *slot = None;
    let verified = CoreStore::open(root, owner)?.verify_for_start(id)?;
    let entry = diagnostic_entry(root, id, owner, verified)?;
    let core = Arc::clone(&entry.3);
    *slot = Some(entry);
    Ok(core)
}

pub fn status(root: Option<&Path>, id: &CoreId) -> CoreStatus {
    status_with_owner(root, id, OwnerPolicy::privileged())
}

fn status_with_owner(root: Option<&Path>, id: &CoreId, owner: OwnerPolicy) -> CoreStatus {
    let invalid = || {
        invalidate_unused();
        CoreStatus::Invalid {
            id: id.clone(),
            reason: "content-or-owner-invalid".into(),
        }
    };
    let Some(root) = root else {
        return invalid();
    };
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            invalidate_unused();
            return CoreStatus::Absent { id: id.clone() };
        }
        Err(_) => return invalid(),
        Ok(_) => {}
    }
    let Ok(_store) = CoreStore::open(root, owner) else {
        return invalid();
    };
    let manifest = root.join("manifests").join(format!("{}.json", id.as_str()));
    match std::fs::symlink_metadata(&manifest) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match std::fs::symlink_metadata(root.join("versions").join(id.as_str())) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    invalidate_unused();
                    CoreStatus::Absent { id: id.clone() }
                }
                _ => invalid(),
            };
        }
        Err(_) => return invalid(),
        Ok(_) => {}
    }
    // Once an id has a manifest, missing payload/lease files are damage,
    // rather than an invitation to treat the installation as wholly absent.
    match cached_verify(root, id, owner) {
        Ok(core) => CoreStatus::Present {
            id: id.clone(),
            version: core.manifest.core.version.clone(),
        },
        Err(error) => CoreStatus::Invalid {
            id: id.clone(),
            reason: status_reason(&error),
        },
    }
}

pub(crate) fn status_reason(error: &std::io::Error) -> String {
    use polaris_core_supervisor::admitted_payload::AdmissionError;
    error
        .get_ref()
        .and_then(|e| e.downcast_ref::<AdmissionError>())
        .map(|e| e.kind.status_token())
        .unwrap_or("content-or-owner-invalid")
        .into()
}

pub fn verify(root: &Path, id: &CoreId) -> std::io::Result<Arc<VerifiedCore>> {
    leased_verify(root, id, OwnerPolicy::privileged())
}

fn leased_verify(
    root: &Path,
    id: &CoreId,
    owner: OwnerPolicy,
) -> std::io::Result<Arc<VerifiedCore>> {
    let cached = cached_verify(root, id, owner)?;
    match CoreStore::open(root, owner)?.lease_cached(id, &cached) {
        Ok(core) => Ok(Arc::new(core)),
        Err(error) => {
            invalidate_unused();
            Err(error)
        }
    }
}

#[cfg(all(test, unix))]
#[path = "pinned_core/tests/mod.rs"]
mod tests;

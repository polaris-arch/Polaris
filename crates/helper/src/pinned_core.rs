//! Shared read-only core status used by the three authenticated dispatchers.
use crate::core_store::{CoreStore, OwnerPolicy, VerifiedCore};
use polaris_helper_proto::{core_manifest::CoreId, core_protocol::CoreStatus};
use std::path::Path;

pub fn status(root: Option<&Path>, id: &CoreId) -> CoreStatus {
    status_with_owner(root, id, OwnerPolicy::privileged())
}

fn status_with_owner(root: Option<&Path>, id: &CoreId, owner: OwnerPolicy) -> CoreStatus {
    let invalid = || CoreStatus::Invalid {
        id: id.clone(),
        reason: "content-or-owner-invalid".into(),
    };
    let Some(root) = root else {
        return invalid();
    };
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CoreStatus::Absent { id: id.clone() }
        }
        Err(_) => return invalid(),
        Ok(_) => {}
    }
    let Ok(store) = CoreStore::open(root, owner) else {
        return invalid();
    };
    let manifest = root.join("manifests").join(format!("{}.json", id.as_str()));
    match std::fs::symlink_metadata(&manifest) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match std::fs::symlink_metadata(root.join("versions").join(id.as_str())) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
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
    match store.verify_for_start(id) {
        Ok(core) => CoreStatus::Present {
            id: id.clone(),
            version: core.manifest.core.version.clone(),
        },
        Err(_) => invalid(),
    }
}

pub fn verify(root: &Path, id: &CoreId) -> std::io::Result<VerifiedCore> {
    CoreStore::open(root, OwnerPolicy::privileged())?.verify_for_start(id)
}

#[cfg(all(test, unix))]
#[path = "pinned_core/tests/mod.rs"]
mod tests;

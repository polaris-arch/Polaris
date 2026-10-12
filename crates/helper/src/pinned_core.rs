//! Shared read-only core status used by the three authenticated dispatchers.
use crate::core_store::{CoreStore, OwnerPolicy, VerifiedCore};
use polaris_helper_proto::{core_manifest::CoreId, core_protocol::CoreStatus};
use std::path::Path;

pub fn status(root: Option<&Path>, id: &CoreId) -> CoreStatus {
    let Some(root) = root else {
        return CoreStatus::Invalid {
            id: id.clone(),
            reason: "coredir-unset".into(),
        };
    };
    match verify(root, id) {
        Ok(core) => CoreStatus::Present {
            id: id.clone(),
            version: core.manifest.core.version.clone(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            CoreStatus::Absent { id: id.clone() }
        }
        Err(_) => CoreStatus::Invalid {
            id: id.clone(),
            reason: "content-or-owner-invalid".into(),
        },
    }
}
pub fn verify(root: &Path, id: &CoreId) -> std::io::Result<VerifiedCore> {
    CoreStore::open(root, OwnerPolicy::privileged())?.verify_for_start(id)
}

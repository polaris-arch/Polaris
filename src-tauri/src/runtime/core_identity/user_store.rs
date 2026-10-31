//! One exact compiled-ID recovery slot. No active pointer, startup extraction,
//! other-version search or executable authority is stored on disk.
use super::{CoreManifest, InstallStage};
use polaris_helper::core_store::{CoreStore, InstallOutcome, OwnerPolicy, VerifiedCore};
use std::path::{Path, PathBuf};

pub(crate) const USER_STORE: &str = "selected-core-recovery";
fn owner() -> OwnerPolicy {
    #[cfg(unix)]
    {
        OwnerPolicy {
            uid: Some(nix::unistd::getuid().as_raw()),
        }
    }
    #[cfg(not(unix))]
    {
        OwnerPolicy { uid: None }
    }
}
fn error(e: impl std::fmt::Display) -> String {
    format!(
        "CORE_RECOVERY_REQUIRED: selected recovery slot damaged, busy or pending; retry explicit repair: {e}"
    )
}

fn namespace_error(e: std::io::Error) -> String {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionError, AdmissionErrorKind as K, RecoveryAction,
    };
    let structural = matches!(
        e.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
    );
    #[cfg(unix)]
    let structural = structural
        || matches!(
            e.raw_os_error(),
            Some(nix::libc::ELOOP) | Some(nix::libc::ENOTDIR)
        );
    let kind = match e.kind() {
        _ if structural => K::Changed,
        std::io::ErrorKind::NotFound => K::Changed,
        std::io::ErrorKind::WouldBlock => K::Busy,
        std::io::ErrorKind::PermissionDenied => K::Io,
        _ => K::Unknown,
    };
    AdmissionError {
        kind,
        purpose: None,
        member: None,
        recovery: if kind.is_payload_damage() {
            RecoveryAction::ReinstallCurrentApp
        } else if kind == K::Busy || kind == K::Unknown {
            RecoveryAction::RetryAfterStop
        } else {
            RecoveryAction::None
        },
        detail: "selected recovery data unavailable, changed or pending".into(),
    }
    .to_string()
}

// Detect known structural aliases even when resolving a later missing child.
// This is a current-user data observation, not the privileged owner/ACL judge.
fn direct_data_directory(meta: &std::fs::Metadata) -> bool {
    let directory = meta.is_dir() && !meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        directory && meta.file_attributes() & 0x400 == 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        directory
    }
}
fn store_present(root: &Path) -> Result<bool, String> {
    let mut present = false;
    for (index, path) in root.ancestors().enumerate() {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if direct_data_directory(&meta) => {
                if index == 0 {
                    present = true;
                }
            }
            Ok(_) => {
                return Err(namespace_error(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "recovery store ancestor is not a direct directory",
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(namespace_error(e)),
        }
    }
    Ok(present)
}

pub(crate) fn selected_at(
    config: &Path,
    selected: &CoreManifest,
) -> Result<Option<VerifiedCore>, String> {
    let root = config.join(USER_STORE);
    if !store_present(&root)? {
        return Ok(None);
    }
    {
        #[cfg(windows)]
        let store = CoreStore::open_current_user_data(&root).map_err(namespace_error)?;
        #[cfg(not(windows))]
        let store = CoreStore::open(&root, owner()).map_err(namespace_error)?;
        let id = selected.id(polaris_updater::sha256_hex).map_err(error)?;
        if !store.has_selected_data(&id).map_err(namespace_error)? {
            return Ok(None);
        }
        let data = store.selected_data(&id).map_err(namespace_error)?;
        // CoreId binds the complete platform/version/provider/member bytes.
        // Helper/cleaner pins are separately supplied from this App for install;
        // they never make these stored core bytes executable.
        if data.manifest.id != id.as_str()
            || data.manifest.core.files != selected.files
            || !data.data_is_current()
        {
            return Err(namespace_error(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "selected receipt or namespace differs",
            )));
        }
        Ok(Some(data))
    }
}

pub(crate) fn current_data() -> Result<Option<VerifiedCore>, String> {
    let Some(config) = crate::runtime::core_paths::base_dir() else {
        return Ok(None);
    };
    if !store_present(&config.join(USER_STORE))? {
        return Ok(None);
    }
    selected_at(
        config,
        &super::selected_manifest(super::current_platform()?)?,
    )
}
pub(crate) fn current_candidate() -> Result<Option<PathBuf>, String> {
    Ok(current_data()?.map(|data| {
        data.directory.join(if cfg!(windows) {
            "sing-box.exe"
        } else {
            "sing-box"
        })
    }))
}

/// Explicit recovery only. Do not use selected_data here: that strict runtime
/// selector must reject damaged members before returning a real candidate.
pub(crate) fn recovery_damage_at(
    config: &Path,
    selected: &CoreManifest,
) -> Result<Option<bool>, String> {
    let root = config.join(USER_STORE);
    if !store_present(&root)? {
        return Ok(None);
    }
    #[cfg(windows)]
    let store = CoreStore::open_current_user_data(&root).map_err(namespace_error)?;
    #[cfg(not(windows))]
    let store = CoreStore::open(&root, owner()).map_err(namespace_error)?;
    store
        .selected_recovery_damage(selected)
        .map_err(namespace_error)
}

#[derive(Clone, Copy, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UserRecoveryState {
    Installed,
    AlreadyPresent,
    Unsupported,
    Pending,
}

pub(crate) fn install(config: &Path, stage: &InstallStage) -> Result<UserRecoveryState, String> {
    // Include root creation and initialize: either may partially publish the
    // namespace before returning an error. Keep this outside platform branches
    // so every future platform implementation has the same exit discipline.
    polaris_core_supervisor::admitted_payload::with_selected_retirement_on_exit(|| {
        #[cfg(windows)]
        {
            // Retire old observations before mutation; the shared outer guard
            // also covers initialization errors and unwind on every exit.
            polaris_core_supervisor::admitted_payload::invalidate_selected();
            let root = config.join(USER_STORE);
            let initialized = CoreStore::initialize_current_user_data(&root);
            let store = match initialized {
                Ok(store) => store,
                Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    return Ok(UserRecoveryState::Unsupported);
                }
                Err(error) => return Err(namespace_error(error)),
            };
            let attempted = store.install(&stage.manifest, &stage.directory, None, None);
            let outcome = attempted.map_err(namespace_error)?;
            let data = selected_at(config, &stage.manifest.core)?
                .ok_or_else(|| error("committed receipt absent"))?;
            if !data.data_is_current() {
                return Err(error("committed data changed"));
            }
            polaris_core_supervisor::admitted_payload::invalidate_selected();
            return Ok(match outcome {
                InstallOutcome::Installed => UserRecoveryState::Installed,
                InstallOutcome::AlreadyPresent => UserRecoveryState::AlreadyPresent,
            });
        }
        #[cfg(not(windows))]
        {
            let root = config.join(USER_STORE);
            match std::fs::symlink_metadata(&root) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    super::recovery::create_private_directory(&root)?
                }
                Err(e) => return Err(error(e)),
                Ok(_) => {}
            }
            let store = CoreStore::initialize(&root, owner()).map_err(error)?;
            let outcome = store
                .install(&stage.manifest, &stage.directory, owner().uid, None)
                .map_err(error)?;
            let data = selected_at(config, &stage.manifest.core)?
                .ok_or_else(|| error("committed receipt absent"))?;
            if !data.data_is_current() {
                return Err(error("committed data changed"));
            }
            Ok(match outcome {
                InstallOutcome::Installed => UserRecoveryState::Installed,
                InstallOutcome::AlreadyPresent => UserRecoveryState::AlreadyPresent,
            })
        }
    })
}

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
mod windows_native_tests;

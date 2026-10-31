//! Explicit current-user recovery. This never installs or upgrades a Helper.

use super::{HelperRuntime, ManagedCoreStatus, RecoveryCheckPause};
use crate::runtime::core_identity::{self, user_store::UserRecoveryState};
use polaris_helper_proto::core_manifest::CoreManifest;
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CoreRecoveryState {
    Ready,
    Restored,
    NotNeeded,
    StopRequired,
    HelperRequired,
    ReinstallRequired,
    Unavailable,
    Pending,
}

/// Data availability is independent of execution admission and Helper readiness.
/// This never executes the candidate or asks it for a version.
pub(crate) fn user_recovery_requirement() -> Result<(), CoreRecoveryState> {
    let selected = core_identity::selected_manifest(
        core_identity::current_platform().map_err(recovery_error)?,
    )
    .map_err(recovery_error)?;
    if let Some(config) = crate::runtime::core_paths::base_dir() {
        if selected_store_requirement(config, &selected)?.is_some() {
            return Ok(());
        }
    }
    let directory = match crate::runtime::proxy::resolve_core_candidate() {
        Ok(path) => path
            .parent()
            .map(Path::to_path_buf)
            .ok_or(CoreRecoveryState::Unavailable)?,
        Err(error) if error.starts_with("CORE_ADMISSION_Missing:") => {
            // A missing-candidate diagnostic may hide an access error. Observe
            // the exact compiled members of the current package explicitly.
            let helper = super::resolve_recovery_helper_binary().map_err(recovery_error)?;
            helper
                .parent()
                .map(Path::to_path_buf)
                .ok_or(CoreRecoveryState::Unavailable)?
        }
        Err(error) => return Err(recovery_error(error)),
    };
    data_requirement_in(&directory, &selected)
}

/// Observe the actual compiled-ID slot before the strict runtime resolver.
/// Errors are not damage, and a damaged slot never becomes an executable path.
pub(crate) fn selected_store_requirement(
    config: &Path,
    selected: &CoreManifest,
) -> Result<Option<()>, CoreRecoveryState> {
    match core_identity::user_store::recovery_damage_at(config, selected).map_err(recovery_error)? {
        None => Ok(None),
        Some(true) => Ok(Some(())),
        Some(false) => Err(CoreRecoveryState::NotNeeded),
    }
}

fn data_requirement_in(directory: &Path, selected: &CoreManifest) -> Result<(), CoreRecoveryState> {
    selected
        .validate()
        .map_err(|_| CoreRecoveryState::ReinstallRequired)?;
    for member in &selected.files {
        let metadata = match std::fs::symlink_metadata(directory.join(&member.name)) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(CoreRecoveryState::Unavailable),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Ok(());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Ok(());
            }
        }
    }
    // Same-open-object bounded hashing, without constructing an admitted payload.
    // Only explicit size/digest mismatches are damage; read/open errors stay unknown.
    match core_identity::verify_directory(directory, selected) {
        Ok(()) => Err(CoreRecoveryState::NotNeeded),
        Err(error)
            if matches!(
                error.as_str(),
                "CORE_BUNDLE_MISMATCH: payload size differs"
                    | "CORE_BUNDLE_MISMATCH: payload bytes differ"
            ) =>
        {
            Ok(())
        }
        Err(_) => Err(CoreRecoveryState::Unavailable),
    }
}

fn recovery_error(error: String) -> CoreRecoveryState {
    log::warn!("current-user core recovery refused: {error}");
    if error.starts_with("CORE_RECOVERY_BUSY:") {
        return CoreRecoveryState::StopRequired;
    }
    if error.starts_with("CORE_BUNDLE_MISMATCH:") || error.starts_with("CORE_RECOVERY_UNSUPPORTED:")
    {
        return CoreRecoveryState::ReinstallRequired;
    }
    // Generic recovery/IO strings are not evidence of an owned live Core or
    // payload corruption. Do not turn missing recovery inputs into a Stop loop.
    if !error.starts_with("CORE_ADMISSION_") {
        return CoreRecoveryState::Unavailable;
    }
    if error.starts_with("CORE_ADMISSION_Busy:") || error.starts_with("CORE_ADMISSION_Unknown:") {
        CoreRecoveryState::StopRequired
    } else if ["Missing", "Mismatch", "Changed"]
        .iter()
        .any(|kind| error.starts_with(&format!("CORE_ADMISSION_{kind}:")))
    {
        CoreRecoveryState::ReinstallRequired
    } else {
        CoreRecoveryState::Unavailable
    }
}

impl HelperRuntime {
    /// The existing validation pause and Child reservations remain owned by the
    /// blocking operation even if its IPC waiter disappears. Reopen explicitly;
    /// a failed release must never be reported as a successful restoration.
    fn with_user_recovery<T>(
        &self,
        body: impl FnOnce() -> Result<T, CoreRecoveryState>,
    ) -> Result<T, CoreRecoveryState> {
        let mut checks = RecoveryCheckPause::acquire().map_err(recovery_error)?;
        let mut reservation = self.reserve_recovery().map_err(recovery_error)?;
        let result = body();
        let release = reservation.release().map_err(recovery_error);
        let checks_release = checks.release().map_err(recovery_error);
        release?;
        checks_release?;
        result
    }

    pub(crate) fn recover_current_user_core(
        &self,
        apply: bool,
        still_stopped: impl Fn() -> Result<(), CoreRecoveryState>,
    ) -> CoreRecoveryState {
        self.with_user_recovery(|| {
            // Recheck after obtaining the existing fences. A prior Ready reply,
            // renderer state, or stop snapshot is never mutation authority.
            still_stopped()?;
            user_recovery_requirement()?;
            if self.manager().is_installed()
                && !matches!(
                    self.managed_core_status(),
                    Ok(ManagedCoreStatus::Stopped | ManagedCoreStatus::BirthEmpty)
                )
            {
                return Err(CoreRecoveryState::StopRequired);
            }
            let helper = super::resolve_recovery_helper_binary().map_err(recovery_error)?;
            let stage = core_identity::stage_recovery_installation(
                &helper,
                &self.dir,
                &AtomicBool::new(false),
            )
            .map_err(recovery_error)?;
            still_stopped()?;
            if !apply {
                // This explicit cold check validates the real selected archive,
                // all members, and same-package Helper/cleaner before offering Reset.
                return Ok(CoreRecoveryState::Ready);
            }
            let outcome =
                core_identity::user_store::install(&self.dir, &stage).map_err(recovery_error)?;
            Ok(match outcome {
                UserRecoveryState::Installed | UserRecoveryState::AlreadyPresent => {
                    CoreRecoveryState::Restored
                }
                UserRecoveryState::Unsupported => CoreRecoveryState::ReinstallRequired,
                UserRecoveryState::Pending => CoreRecoveryState::Pending,
            })
        })
        .unwrap_or_else(|state| state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::proxy::DirectCoreSlot;
    use std::sync::{Arc, Mutex};

    #[test]
    fn data_verifier_requires_real_damage_without_execution_admission() {
        let fixture = core_identity::RecoveryFixture::new("win");
        let stage = fixture.stage().unwrap();
        assert_eq!(
            data_requirement_in(&stage.directory, &fixture.selected),
            Err(CoreRecoveryState::NotNeeded)
        );
        let member = &fixture.selected.files[0];
        std::fs::write(
            stage.directory.join(&member.name),
            vec![b'x'; member.bytes as usize],
        )
        .unwrap();
        assert_eq!(
            data_requirement_in(&stage.directory, &fixture.selected),
            Ok(())
        );
        #[cfg(any(windows, target_os = "macos"))]
        assert!(
            polaris_core_supervisor::admitted_payload::check_selected_capability(
                core_identity::ExecPurpose::RunMain
            )
            .is_err()
        );
    }

    #[test]
    fn data_verifier_observes_missing_members_without_promoting_generic_recovery_errors() {
        let fixture = core_identity::RecoveryFixture::new("win");
        let stage = fixture.stage().unwrap();
        std::fs::remove_file(stage.directory.join(&fixture.selected.files[0].name)).unwrap();
        assert_eq!(
            data_requirement_in(&stage.directory, &fixture.selected),
            Ok(())
        );
        assert_eq!(
            recovery_error("CORE_RECOVERY_REQUIRED: unclassified error".into()),
            CoreRecoveryState::Unavailable
        );
    }

    #[test]
    fn recovery_source_errors_do_not_loop_through_stop_or_assert_fake_damage() {
        assert_eq!(
            recovery_error("CORE_BUNDLE_MISMATCH: missing archive".into()),
            CoreRecoveryState::ReinstallRequired
        );
        assert_eq!(
            recovery_error("CORE_RECOVERY_UNSUPPORTED: filesystem".into()),
            CoreRecoveryState::ReinstallRequired
        );
        assert_eq!(
            recovery_error("CORE_ADMISSION_Io: permission".into()),
            CoreRecoveryState::Unavailable
        );
        assert_eq!(
            recovery_error("CORE_ADMISSION_Unknown: retained work".into()),
            CoreRecoveryState::StopRequired
        );
        assert_eq!(
            recovery_error("CORE_RECOVERY_REQUIRED: unreadable namespace".into()),
            CoreRecoveryState::Unavailable
        );
    }

    #[test]
    fn consumer_keeps_existing_fences_through_failure_and_releases_its_own_reservation() {
        let runtime = HelperRuntime::never_installed_for_tests(std::env::temp_dir());
        let slot = Arc::new(Mutex::new(DirectCoreSlot::default()));
        runtime
            .bind_core_mutation_fence(Arc::downgrade(&slot))
            .unwrap();
        let result: Result<(), _> = runtime.with_user_recovery(|| {
            assert!(runtime.register_core_mutation().is_err());
            assert!(runtime.reserve_recovery().is_err());
            Err(CoreRecoveryState::Unavailable)
        });
        assert_eq!(result, Err(CoreRecoveryState::Unavailable));
        let mut reservation = runtime.reserve_recovery().unwrap();
        reservation.release().unwrap();
    }
}

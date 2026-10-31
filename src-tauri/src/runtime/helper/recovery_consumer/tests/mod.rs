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

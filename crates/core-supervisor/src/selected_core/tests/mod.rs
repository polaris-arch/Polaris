use super::*;

#[test]
fn installer_bootstrap_pin_does_not_admit_an_execution_selection() {
    let Ok(platform) = current_platform() else {
        return;
    };
    let formal_bootstrap = selection_lane() == SelectionLane::Formal
        && COMPILED_INSTALLATION == br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#;
    if formal_bootstrap {
        let expected = formal_manifest(platform).unwrap().id(sha256_hex).unwrap();
        assert_eq!(selected_installer_id(platform).unwrap(), expected);
        assert!(selected_manifest(platform).is_err());
        assert!(selected_id(platform).is_err());
        assert!(selected_check_source(platform).is_err());
        println!("POLARIS_TEST_FORMAL_PENDING_INSTALLER_PIN_ACCEPTED_EXECUTION_REJECTED");
    } else {
        // Installed formal and local lanes cannot acquire
        // an installer-only fallback or borrow a different compiled core id.
        assert_eq!(selected_installer_id(platform), selected_id(platform));
    }
    assert!(selected_installer_id("unsupported-platform").is_err());
}

#[test]
fn compiled_lane_never_uses_validation_receipts_as_local_authority() {
    #[cfg(feature = "formal-selection")]
    {
        assert_eq!(selection_lane(), SelectionLane::Formal);
        assert_eq!(COMPILED_DEVELOPMENT_SELECTION, b"null");
        assert_eq!(COMPILED_DEVELOPMENT_CONTRACT, b"null");
    }
    #[cfg(feature = "local-development-selection")]
    {
        assert_eq!(selection_lane(), SelectionLane::LocalDevelopment);
        let pure = selected_pure_manifest("linux").unwrap();
        let source = selected_source_commit("linux").unwrap();
        assert_ne!(source, formal_source_commit("linux").unwrap());
        let mut installed = pure.clone();
        installed.helper_sha256 = Some("f".repeat(64));
        assert_eq!(
            pure.id(sha256_hex).unwrap(),
            installed.id(sha256_hex).unwrap()
        );
        assert!(selected_pure_manifest("win").is_err());
        assert!(selected_pure_manifest("mac-arm64").is_err());
        assert_eq!(selected_check_source("linux").unwrap().0, source);
    }
}

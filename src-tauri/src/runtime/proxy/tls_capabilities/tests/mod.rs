use super::*;
use crate::runtime::helper::ManagedCoreStatus;
use polaris_helper_proto::core_manifest::CoreId;
use polaris_helper_proto::{HelperBirthTarget, Platform};
#[test]
fn mac_real_status_shape_requires_original_birth_and_selected_core_not_windows_fields() {
    let birth = HelperBirthTarget::parse_wire("4242", "00112233445566778899aabbccddeeff").unwrap();
    let core = CoreId::parse(&"a".repeat(64)).unwrap();
    let status = ManagedCoreStatus::NativeBirthRunning {
        target: birth,
        created: None,
        image: None,
        core: Some(core.clone()),
    };
    assert!(native_helper_birth_matches(
        Platform::Mac,
        &status,
        birth,
        &core,
        None
    ));
    assert!(!native_helper_birth_matches(
        Platform::Win,
        &status,
        birth,
        &core,
        None
    ));
    let other = HelperBirthTarget::parse_wire("4242", "10112233445566778899aabbccddeeff").unwrap();
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &status,
        other,
        &core,
        None
    ));
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &status,
        birth,
        &CoreId::parse(&"b".repeat(64)).unwrap(),
        None
    ));
    let legacy = ManagedCoreStatus::NativeBirthRunning {
        target: birth,
        created: None,
        image: None,
        core: None,
    };
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &legacy,
        birth,
        &core,
        None
    ));
    let windows = ManagedCoreStatus::NativeBirthRunning {
        target: birth,
        created: Some(99),
        image: Some("C:\\private\\sing-box.exe".into()),
        core: None,
    };
    assert!(native_helper_birth_matches(
        Platform::Win,
        &windows,
        birth,
        &core,
        None
    ));
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &windows,
        birth,
        &core,
        None
    ));
}

#[test]
fn non_native_helper_platforms_cannot_borrow_mac_or_windows_birth_authority() {
    let birth = HelperBirthTarget::parse_wire("4242", "00112233445566778899aabbccddeeff").unwrap();
    let core = CoreId::parse(&"a".repeat(64)).unwrap();
    let mac = ManagedCoreStatus::NativeBirthRunning {
        target: birth,
        created: None,
        image: None,
        core: Some(core.clone()),
    };
    let windows = ManagedCoreStatus::NativeBirthRunning {
        target: birth,
        created: Some(99),
        image: Some("C:\\private\\sing-box.exe".into()),
        core: Some(core.clone()),
    };
    for platform in [
        Platform::Linux,
        Platform::Android,
        Platform::Ios,
        Platform::Other,
    ] {
        assert!(!native_helper_birth_matches(
            platform,
            &mac,
            birth,
            &core,
            Some(&core)
        ));
        assert!(!native_helper_birth_matches(
            platform,
            &windows,
            birth,
            &core,
            Some(&core)
        ));
    }
}

#[test]
fn linux_requires_its_running_original_birth_and_started_receipt_core() {
    let birth = HelperBirthTarget::parse_wire("4242", "00112233445566778899aabbccddeeff").unwrap();
    let other = HelperBirthTarget::parse_wire("4242", "10112233445566778899aabbccddeeff").unwrap();
    let core = CoreId::parse(&"a".repeat(64)).unwrap();
    let wrong = CoreId::parse(&"b".repeat(64)).unwrap();
    let running = ManagedCoreStatus::BirthRunning { target: birth };
    let matches = |platform, status: &ManagedCoreStatus, expected, started| {
        native_helper_birth_matches(platform, status, expected, &core, started)
    };
    assert!(matches(Platform::Linux, &running, birth, Some(&core)));
    // BirthRejected records no CoreId; a missing or different marker never attests.
    assert!(!matches(Platform::Linux, &running, birth, None));
    assert!(!matches(Platform::Linux, &running, birth, Some(&wrong)));
    assert!(!matches(Platform::Linux, &running, other, Some(&core)));
    for status in [
        ManagedCoreStatus::BirthStopping { target: birth },
        ManagedCoreStatus::BirthUnknown { target: birth },
        ManagedCoreStatus::BirthEmpty,
        ManagedCoreStatus::NativeBirthRunning {
            target: birth,
            created: Some(99),
            image: Some("/private/sing-box".into()),
            core: Some(core.clone()),
        },
    ] {
        assert!(!matches(Platform::Linux, &status, birth, Some(&core)));
    }
    // The Linux shape grants nothing to the platforms that must report a core.
    for platform in [Platform::Mac, Platform::Win] {
        assert!(!matches(platform, &running, birth, Some(&core)));
    }
}

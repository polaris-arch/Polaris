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
        &core
    ));
    assert!(!native_helper_birth_matches(
        Platform::Win,
        &status,
        birth,
        &core
    ));
    let other = HelperBirthTarget::parse_wire("4242", "10112233445566778899aabbccddeeff").unwrap();
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &status,
        other,
        &core
    ));
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &status,
        birth,
        &CoreId::parse(&"b".repeat(64)).unwrap()
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
        &core
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
        &core
    ));
    assert!(!native_helper_birth_matches(
        Platform::Mac,
        &windows,
        birth,
        &core
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
        assert!(!native_helper_birth_matches(platform, &mac, birth, &core));
        assert!(!native_helper_birth_matches(
            platform, &windows, birth, &core
        ));
    }
}

use super::*;
#[test]
fn versions_and_protocols_never_downgrade_the_shared_helper() {
    assert!(!may_replace("1.1.0", 2, "1.0.0", 2).unwrap());
    assert!(!may_replace("1.0.0", 3, "1.0.0", 2).unwrap());
    assert!(may_replace("1.0.0", 2, "1.0.0", 2).unwrap());
    assert!(may_replace("1.0.0-alpha.2", 2, "1.0.0", 2).unwrap());
    assert!(!may_replace("1.0.0", 2, "1.0.0-alpha.2", 2).unwrap());
    assert!(may_replace("not-a-version", 2, "1.0.0", 2).is_err());
}

#[test]
fn compatible_helpers_survive_app_releases_but_required_fixes_upgrade() {
    for version in ["0.9.0", "1.0.0", "1.1.0"] {
        assert_eq!(
            replacement_policy(version, 5, "1.0.0", 5).unwrap(),
            "retain"
        );
        assert_eq!(
            replacement_policy(version, 6, "1.0.0", 5).unwrap(),
            "retain"
        );
    }
    assert_eq!(
        replacement_policy("1.0.0", 4, "1.0.0", 5).unwrap(),
        "replace"
    );
    assert_eq!(
        replacement_policy("0.9.0", 4, "1.0.0", 5).unwrap(),
        "replace"
    );
    assert!(replacement_policy("1.1.0", 4, "1.0.0", 5).is_err());
    assert!(replacement_policy("invalid", 5, "1.0.0", 5).is_err());
}

#[test]
fn platform_behavior_fixes_advance_once_then_reuse_compatible_helpers() {
    use polaris_helper_proto::{proto_version, Platform};
    for (platform, expected) in [(Platform::Win, 8), (Platform::Linux, 9)] {
        let current = proto_version::for_platform(platform);
        assert_eq!(current, expected, "{platform:?}");
        assert_eq!(
            replacement_policy("1.0.0", current - 1, "1.0.0", current).unwrap(),
            "replace"
        );
        assert_eq!(
            replacement_policy("1.0.0", current, "1.1.0", current).unwrap(),
            "retain"
        );
        assert_eq!(
            replacement_policy("1.1.0", current + 1, "1.0.0", current).unwrap(),
            "retain"
        );
        assert!(replacement_policy("1.1.0", current - 1, "1.0.0", current).is_err());
    }
}

#[test]
fn platform_behavior_fixes_preserve_the_macos_v7_helper() {
    let required =
        polaris_helper_proto::proto_version::for_platform(polaris_helper_proto::Platform::Mac);
    assert_eq!(required, 7);
    assert_eq!(
        replacement_policy("1.0.0", 7, "1.1.0", required).unwrap(),
        "retain"
    );
}

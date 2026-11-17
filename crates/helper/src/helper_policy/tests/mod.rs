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
fn windows_exact_object_fix_advances_once_then_reuses_compatible_helper() {
    let current =
        polaris_helper_proto::proto_version::for_platform(polaris_helper_proto::Platform::Win);
    assert_eq!(
        current, 8,
        "the exact-object fix requires a new compatibility generation"
    );
    // This must use the production platform generation, not two hand-written version numbers: otherwise
    // reverting the advertised generation to v7 would leave this regression green.
    assert_eq!(
        replacement_policy("1.0.0", 7, "1.0.0", current).unwrap(),
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
    // A real compatibility fix still cannot silently downgrade a newer binary.
    assert!(replacement_policy("1.1.0", 7, "1.0.0", current).is_err());
}

#[test]
fn windows_behavior_fix_does_not_replace_other_platform_v7_helpers() {
    for platform in [
        polaris_helper_proto::Platform::Mac,
        polaris_helper_proto::Platform::Linux,
    ] {
        let required = polaris_helper_proto::proto_version::for_platform(platform);
        assert_eq!(required, 7);
        assert_eq!(
            replacement_policy("1.0.0", 7, "1.1.0", required).unwrap(),
            "retain"
        );
    }
}

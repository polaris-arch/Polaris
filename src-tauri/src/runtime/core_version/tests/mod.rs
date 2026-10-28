use super::*;

#[test]
fn mobile_version_comes_from_build_contract() {
    assert_eq!(
        mobile_kernel_version(
            r#"{"schema":"polaris-mobile-fork-input-v1","version":"2.3.4-test.5"}"#
        ),
        "2.3.4-test.5"
    );
    for raw in [
        "{}",
        "invalid",
        r#"{"schema":"other","version":"1.2.3"}"#,
        r#"{"schema":"polaris-mobile-fork-input-v1","version":"1\r\nInjected: yes"}"#,
    ] {
        assert!(mobile_kernel_version(raw).is_empty());
    }
}

#[test]
fn embedded_mobile_contract_has_a_safe_version() {
    let raw = MOBILE_SOURCE_CONTRACT;
    let contract: serde_json::Value = serde_json::from_str(raw).unwrap();
    let version = mobile_kernel_version(raw);
    assert!(!version.is_empty());
    assert_eq!(Some(version.as_str()), contract["version"].as_str());
    for raw in [
        r#"{"schema":"polaris-mobile-fork-input-v1"}"#,
        r#"{"schema":"polaris-mobile-fork-input-v1","version":123}"#,
    ] {
        assert!(mobile_kernel_version(raw).is_empty());
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[test]
fn desktop_version_matches_admission_or_is_explicitly_unknown() {
    let selected = crate::runtime::core_identity::current_platform()
        .and_then(crate::runtime::core_identity::selected_manifest);
    match selected {
        Ok(manifest) => assert_eq!(selected_bundled_version(), manifest.version),
        Err(_) => assert_eq!(selected_bundled_version(), "unknown"),
    }
}

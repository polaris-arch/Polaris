use super::*;

fn release(version: &str, suffix: &str) -> Value {
    json!({
        "tag_name": version,
        "name": format!("release {version}"),
        "assets": [{
            "name": format!("Polaris_{}_{suffix}", polaris_updater::github::strip_v(version)),
            "browser_download_url": format!("https://example.com/{version}/{suffix}"),
            "size": 123,
            "digest": format!("sha256:{}", "ab".repeat(32)),
        }],
    })
}

#[test]
fn actual_host_dispatch_selects_current_independently_of_available_newer_release() {
    let body = json!([
        release("v3.0.0", "aarch64-mac.dmg"),
        release("v2.0.0", "aarch64-mac.dmg"),
        release("v1.0.0", "aarch64-mac.dmg"),
    ])
    .to_string();
    let target = (AssetPlatform::Macos, AssetArch::Arm64, false);
    let check = |include_current| {
        check_requested_app_release(
            &body,
            "2.0.0",
            false,
            include_current,
            Some("2.0.0"),
            target,
        )
        .unwrap()
    };
    let AppUpdateCheck::Available(newer) = check(false) else {
        panic!("ordinary update should select the newer release");
    };
    assert_eq!(newer.version, "v3.0.0");
    let AppUpdateCheck::Available(current) = check(true) else {
        panic!("explicit reinstall should independently find current, even if skipped");
    };
    assert_eq!(current.version, "v2.0.0");
    assert_eq!(current.title, "release v2.0.0");
    assert_eq!(
        current.download_url,
        "https://example.com/v2.0.0/aarch64-mac.dmg"
    );
    assert_eq!(current.sha256, Some("ab".repeat(32)));

    // Verify the IPC command uses this same tested dispatch before constructing the response.
    let source = src();
    let command = &source[source.find("pub async fn update_check(").unwrap()..];
    let command = &command[..command.find("\n}").unwrap()];
    let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("check_requested_app_release(&body,&current,include_pre,include_current,skipped.as_deref(),(platform,arch,loose_form),)"));
    assert!(!compact.contains("=check_app_update("));
    assert!(compact.contains("\"hasUpdate\":!include_current"));
    assert!(compact.contains("ifinclude_current{response[\"isCurrentVersion\"]=json!(true);}"));
}

#[test]
fn host_current_lookup_does_not_fall_back_to_other_android_releases_when_unavailable() {
    let body = json!([
        release("v3.0.0", "arm64-v8a-android.apk"),
        release("v2.0.0", "aarch64-mac.dmg"),
        release("v1.0.0", "arm64-v8a-android.apk"),
    ])
    .to_string();
    for version in ["2.0.0", "2.1.0"] {
        assert_eq!(
            check_requested_app_release(
                &body,
                version,
                false,
                true,
                None,
                (AssetPlatform::Android, AssetArch::Arm64, false),
            )
            .unwrap(),
            AppUpdateCheck::NoUpdate,
        );
    }
    // Android's normal update path remains available when the chosen highest version has an APK.
    let ordinary = check_requested_app_release(
        &body,
        "2.0.0",
        false,
        false,
        None,
        (AssetPlatform::Android, AssetArch::Arm64, false),
    )
    .unwrap();
    assert!(matches!(ordinary, AppUpdateCheck::Available(info) if info.version == "v3.0.0"));
    assert!(src().contains("if !include_current && platform == AssetPlatform::Android =>"));
}

#[test]
fn both_host_dispatch_paths_preserve_manifest_parse_errors() {
    for include_current in [false, true] {
        assert!(check_requested_app_release(
            "not json",
            "2.0.0",
            false,
            include_current,
            None,
            (AssetPlatform::Macos, AssetArch::Arm64, false),
        )
        .is_err());
    }
}

#[test]
fn release_only_fallback_keeps_highest_version_and_never_claims_a_current_installer() {
    let body = json!([
        release("v3.0.0", "aarch64-mac.dmg"),
        release("v2.0.0", "arm64-v8a-android.apk"),
    ])
    .to_string();
    assert_eq!(
        check_requested_app_release(
            &body,
            "1.0.0",
            false,
            false,
            None,
            (AssetPlatform::Android, AssetArch::Arm64, false),
        )
        .unwrap(),
        AppUpdateCheck::NoUpdate,
    );
    let fallback = release_only_response(&body, "1.0.0", false, None, false);
    assert!(fallback.success);
    let data = fallback.data.unwrap();
    assert_eq!(data["hasUpdate"], true);
    assert_eq!(data["updateInfo"]["version"], "v3.0.0");
    assert_eq!(data["updateInfo"]["downloadUrl"], "");

    // The no-package-platform branch cannot reinstall, even if other releases are available.
    let explicit_current = release_only_response(&body, "2.0.0", false, None, true);
    assert!(explicit_current.success);
    assert_eq!(
        explicit_current.data.unwrap(),
        json!({ "hasUpdate": false })
    );
    for include_current in [false, true] {
        assert!(!release_only_response("bad json", "2.0.0", false, None, include_current).success);
    }
}

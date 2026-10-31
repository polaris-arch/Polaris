use super::*;
use serde_json::{json, Value};

fn release(tag: &str, published_at: Option<&str>, prerelease: bool) -> Value {
    // Asset names follow the producer contract: no tag prefix or build metadata.
    let asset_version = strip_v(tag).split('+').next().unwrap_or_default();
    json!({
        "tag_name": tag,
        "name": format!("Release {tag}"),
        "body": format!("Notes {tag}"),
        "published_at": published_at,
        "prerelease": prerelease,
        "assets": [{
            "name": format!("Polaris_{asset_version}_aarch64-mac.dmg"),
            "browser_download_url": format!("https://example.com/{tag}/mac.dmg"),
            "size": 123,
            "digest": format!("sha256:{}", "ab".repeat(32)),
        }],
    })
}

fn check(releases: &[Value], include_pre: bool, skipped: Option<&str>) -> AppUpdateCheck {
    check_app_update(
        &serde_json::to_string(releases).unwrap(),
        "1.0.0",
        include_pre,
        skipped,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap()
}

fn check_release_only(
    releases: &[Value],
    include_pre: bool,
    skipped: Option<&str>,
) -> AppUpdateCheck {
    check_app_update_release_only(
        &serde_json::to_string(releases).unwrap(),
        "1.0.0",
        include_pre,
        skipped,
    )
    .unwrap()
}

fn current(releases: &[Value], version: &str, include_pre: bool) -> AppUpdateCheck {
    resolve_current_app_release(
        &serde_json::to_string(releases).unwrap(),
        version,
        include_pre,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap()
}

fn available(check: AppUpdateCheck) -> AppUpdateInfo {
    let AppUpdateCheck::Available(info) = check else {
        panic!("expected a selected release");
    };
    info
}

#[test]
fn higher_version_wins_over_later_lower_release_and_preserves_metadata() {
    let releases = [
        release("v2.0.0", Some("2026-01-01T00:00:00Z"), false),
        release("v1.0.0", Some("2026-10-28T00:00:00Z"), false),
        release("v0.9.0", Some("2099-01-01T00:00:00Z"), false),
    ];
    let info = available(check(&releases, false, None));
    assert_eq!(info.version, "v2.0.0");
    assert_eq!(info.title, "Release v2.0.0");
    assert_eq!(info.release_notes, "Notes v2.0.0");
    assert_eq!(info.published_at, "2026-01-01T00:00:00Z");
    assert!(!info.is_prerelease);
    assert_eq!(info.download_url, "https://example.com/v2.0.0/mac.dmg");
    assert_eq!(info.file_name, "Polaris_2.0.0_aarch64-mac.dmg");
    assert_eq!(info.file_size, 123);
    assert_eq!(info.sha256, Some("ab".repeat(32)));

    let release_only = available(check_release_only(&releases, false, None));
    assert_eq!(release_only.version, info.version);
    assert_eq!(release_only.title, info.title);
    assert_eq!(release_only.release_notes, info.release_notes);
    assert_eq!(release_only.published_at, info.published_at);
    assert_eq!(release_only.is_prerelease, info.is_prerelease);
    assert!(release_only.download_url.is_empty());
    assert!(release_only.file_name.is_empty());
    assert_eq!(release_only.file_size, 0);
    assert_eq!(release_only.sha256, None);
}

#[test]
fn channel_filtering_precedes_semver_ordering_including_stable_over_its_prerelease() {
    let releases = [
        release("v2.0.0-beta.10", Some("2099-01-01T00:00:00Z"), true),
        release("v2.0.0", None, false),
        release("v1.9.0", Some("2100-01-01T00:00:00Z"), false),
    ];
    for include_pre in [false, true] {
        assert_eq!(
            available(check(&releases, include_pre, None)).version,
            "v2.0.0"
        );
        assert_eq!(
            available(check_release_only(&releases, include_pre, None)).version,
            "v2.0.0"
        );
    }
    let releases = [
        release("v3.0.0-beta.2", Some("2020-01-01T00:00:00Z"), true),
        release("v3.0.0-beta.10", None, true),
        releases[1].clone(),
    ];
    for include_pre in [false, true] {
        let expected = if include_pre {
            "v3.0.0-beta.10"
        } else {
            "v2.0.0"
        };
        assert_eq!(
            available(check(&releases, include_pre, None)).version,
            expected
        );
        assert_eq!(
            available(check_release_only(&releases, include_pre, None)).version,
            expected
        );
    }
}

#[test]
fn malformed_tags_cannot_mask_a_valid_newer_release() {
    let releases = [
        release("v2.0.0", Some("2020-01-01T00:00:00Z"), false),
        release("", Some("2099-01-01T00:00:00Z"), false),
        release("nightly", Some("2100-01-01T00:00:00Z"), false),
        release("v999.garbage", Some("2101-01-01T00:00:00Z"), false),
        release("v999.0.0-alpha..1", None, true),
        release("v999.0.0+bad+build", None, false),
    ];
    for include_pre in [false, true] {
        assert_eq!(
            available(check(&releases, include_pre, None)).version,
            "v2.0.0"
        );
        assert_eq!(
            available(check_release_only(&releases, include_pre, None)).version,
            "v2.0.0"
        );
    }
    let malformed = &releases[1..];
    assert_eq!(check(malformed, true, None), AppUpdateCheck::NoUpdate);
    assert_eq!(
        check_release_only(malformed, true, None),
        AppUpdateCheck::NoUpdate
    );
    assert_eq!(current(malformed, "0.0.0", true), AppUpdateCheck::NoUpdate);
}

#[test]
fn skipping_the_highest_version_does_not_fall_back_to_an_older_update() {
    let releases = [
        release("v3.0.0", Some("2020-01-01T00:00:00Z"), false),
        release("v2.0.0", Some("2099-01-01T00:00:00Z"), false),
    ];
    assert_eq!(
        check(&releases, false, Some("3.0.0")),
        AppUpdateCheck::NoUpdate
    );
    assert_eq!(
        check_release_only(&releases, false, Some("3.0.0")),
        AppUpdateCheck::NoUpdate
    );
    assert_eq!(
        available(check(&releases, false, Some("2.0.0"))).version,
        "v3.0.0"
    );
}

#[test]
fn highest_version_missing_assets_does_not_fall_back_but_release_only_still_reports_it() {
    let mut highest = release("v3.0.0", None, false);
    highest["assets"] = json!([]);
    let releases = [
        highest,
        release("v2.0.0", Some("2099-01-01T00:00:00Z"), false),
    ];
    assert_eq!(check(&releases, false, None), AppUpdateCheck::NoUpdate);
    assert_eq!(
        available(check_release_only(&releases, false, None)).version,
        "v3.0.0"
    );
}

#[test]
fn current_reinstall_is_independent_of_other_versions_and_their_publish_times() {
    let releases = [
        release("v9.0.0", Some("2026-10-28T00:00:00Z"), false),
        release("v1.0.0", Some("2099-01-01T00:00:00Z"), false),
        release("v2.0.0", Some("2020-01-01T00:00:00Z"), false),
    ];
    assert_eq!(
        available(current(&releases, "2.0.0", false)).version,
        "v2.0.0"
    );
    assert_eq!(current(&releases, "3.0.0", false), AppUpdateCheck::NoUpdate);

    let releases = [release("v2.0.0-beta.1", None, true), releases[0].clone()];
    assert_eq!(
        current(&releases, "2.0.0-beta.1", false),
        AppUpdateCheck::NoUpdate
    );
    assert_eq!(
        available(current(&releases, "2.0.0-beta.1", true)).version,
        "v2.0.0-beta.1"
    );
}

#[test]
fn equivalent_versions_prefer_publish_time_then_tag_independently_of_api_order() {
    let mut releases = [
        release("v2.0.0+build.a", Some("2026-01-01T00:00:00Z"), false),
        release("v2.0.0+build.z", Some("2026-01-01T00:00:00Z"), false),
        release("v2.0.0+build.zz", None, false),
    ];
    for _ in 0..2 {
        let expected = "v2.0.0+build.z";
        assert_eq!(available(check(&releases, false, None)).version, expected);
        assert_eq!(
            available(check_release_only(&releases, false, None)).version,
            expected
        );
        assert_eq!(
            available(current(&releases, "2.0.0+installed", false)).version,
            expected
        );
        releases.reverse();
    }
    releases[0]["published_at"] = json!("2099-01-01T00:00:00Z");
    assert_eq!(
        available(check(&releases, false, None)).version,
        "v2.0.0+build.a"
    );
    assert_eq!(
        available(check_release_only(&releases, false, None)).version,
        "v2.0.0+build.a"
    );
    assert_eq!(
        available(current(&releases, "2.0.0", false)).version,
        "v2.0.0+build.a"
    );
}

#[test]
fn identical_tags_keep_first_response_entry_even_if_it_lacks_assets() {
    let mut first = release("v2.0.0", None, false);
    first["name"] = json!("first entry");
    first["assets"] = json!([]);
    let releases = [first, release("v2.0.0", None, false)];
    assert_eq!(
        available(check_release_only(&releases, false, None)).title,
        "first entry"
    );
    assert_eq!(check(&releases, false, None), AppUpdateCheck::NoUpdate);
    assert_eq!(current(&releases, "2.0.0", false), AppUpdateCheck::NoUpdate);
}

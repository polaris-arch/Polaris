use super::*;
use serde_json::json;

fn selection() -> Value {
    let pending: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/desktop-core/pending-release-selection.json"
    ))
    .unwrap();
    pending["selection"].clone()
}

#[test]
fn complete_release_identity_is_distinct_from_source_identity() {
    let value = selection();
    assert_eq!(
        desktop_release_identity(&value),
        Some((
            "1.15.0-polaris-alpha.12",
            "830f7256af499e5ad3e7da9a79f0a377ca38fc77"
        ))
    );
}

#[test]
fn final_polaris_alpha12_is_admitted_only_with_the_complete_exact_release_binding() {
    let value = &selection();
    assert_eq!(
        desktop_release_identity(value),
        Some((
            "1.15.0-polaris-alpha.12",
            "830f7256af499e5ad3e7da9a79f0a377ca38fc77"
        ))
    );
    let mut invalid = value.clone();
    invalid["tag"] = json!("v1.15.0-alpha.12.polaris.1");
    assert!(desktop_release_identity(&invalid).is_none());
    let mut invalid = value.clone();
    invalid["contractPath"] = json!("scripts/desktop-core/arbitrary.json");
    assert!(desktop_release_identity(&invalid).is_none());
}

// Synthetic admission fixtures do not claim that these releases exist. The
// public entry point must still reject them until exact contract bytes ship.
fn bound_fixture(version: &str, upstream: bool) -> (Value, String) {
    let mut value = selection();
    value["version"] = json!(version);
    value["tag"] = json!(format!("v{version}"));
    value["sourceCommit"] = json!("a".repeat(40));
    value["sourceTree"] = json!("b".repeat(40));
    value["tagObject"] = json!("c".repeat(40));
    value["releaseId"] = json!(17);
    let mut contract = json!({
        "schema": "polaris-box-release-consumer-contract-v1",
        "repository": "polaris-arch/polaris-box",
        "platforms": {
            "linux": {"sidecar": {"sha256": value["librarySha256"]["linux"]}},
            "win": {"sidecar": {"sha256": value["librarySha256"]["win"]}}
        }
    });
    if upstream {
        contract["schema"] = json!("polaris-upstream-native-release-consumer-v1");
        contract["providerContract"] = json!("polaris-upstream-native-release-v1");
        contract["repository"] = json!("polaris-arch/sing-box-polaris");
        contract["repositoryId"] = json!(1_421_657_509u64);
    }
    for key in [
        "version",
        "tag",
        "sourceCommit",
        "sourceTree",
        "tagObject",
        "releaseId",
    ] {
        contract[key] = value[key].clone();
    }
    let raw = serde_json::to_string(&contract).unwrap();
    value["contractSha256"] = json!(crate::sha256_hex(raw.as_bytes()));
    (value, raw)
}

#[test]
fn polaris_channels_require_exact_contract_identity_and_historical_names_remain_supported() {
    for (version, upstream) in [
        ("1.15.0-polaris-alpha.12", true),
        ("1.15.0-polaris-beta.0", true),
        ("2.0.1-polaris-rc.3", true),
        ("1.15.0-alpha.12.polaris.1", false),
    ] {
        let (value, raw) = bound_fixture(version, upstream);
        assert_eq!(
            desktop_release_identity_from_contract(&value, &raw),
            Some((version, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"))
        );
        assert!(desktop_release_identity(&value).is_none());
    }
    for version in [
        "1.15.0-polaris-alpha.01",
        "1.15.0-polaris-alpha.-1",
        "1.15.0-polaris-alpha.",
        "1.15.0-polaris-dev.1",
        "1.15.0-polaris-rc.1+build",
        "1.15.0-polaris-beta.1.extra",
        "1.15-polaris-alpha.1",
        "v1.15.0-polaris-alpha.1",
        "1.15.0-polaris-alpha.１２",
        "1.15.0-alpha.12",
        "1.15.0-alpha.12.polaris.1",
    ] {
        let (value, raw) = bound_fixture(version, true);
        assert!(
            desktop_release_identity_from_contract(&value, &raw).is_none(),
            "{version}"
        );
    }
    let (value, raw) = bound_fixture("1.15.0-polaris-alpha.12", false);
    assert!(desktop_release_identity_from_contract(&value, &raw).is_none());
}

#[test]
fn well_shaped_unbound_fields_and_pending_contract_aliases_withhold_identity() {
    let value = selection();
    for (key, bad) in [
        ("contractSha256", json!("a".repeat(64))),
        ("sourceCommit", json!("b".repeat(40))),
        ("sourceTree", json!("c".repeat(40))),
        ("tagObject", json!("d".repeat(40))),
        ("releaseId", json!(17)),
        (
            "contractPath",
            json!("scripts/desktop-core/pending-release-contract.json"),
        ),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(desktop_release_identity(&changed).is_none(), "{key}");
    }
    for platform in ["linux", "win"] {
        let mut changed = value.clone();
        changed["librarySha256"][platform] = json!("e".repeat(64));
        assert!(desktop_release_identity(&changed).is_none(), "{platform}");
    }
    let mut changed = value.clone();
    changed["version"] = json!("1.15.0-alpha.12");
    changed["tag"] = json!("v1.15.0-alpha.12");
    assert!(desktop_release_identity(&changed).is_none());
    let mut changed = value.clone();
    changed["extra"] = json!(true);
    assert!(desktop_release_identity(&changed).is_none());
}

#[test]
fn compiled_contract_hash_and_upstream_repository_are_checked_independently() {
    let (value, raw) = bound_fixture("1.15.0-polaris-alpha.12", true);
    assert!(desktop_release_identity_from_contract(&value, &(raw.clone() + "\n")).is_none());
    for (key, bad) in [
        ("schema", json!("polaris-native-dns-candidate-v1")),
        ("providerContract", json!("polaris-box-release-v1")),
        ("repository", json!("polaris-arch/polaris-box")),
        ("repositoryId", json!(1_407_705_262u64)),
    ] {
        let mut contract: Value = serde_json::from_str(&raw).unwrap();
        contract[key] = bad;
        let changed_raw = serde_json::to_string(&contract).unwrap();
        let mut changed = value.clone();
        changed["contractSha256"] = json!(crate::sha256_hex(changed_raw.as_bytes()));
        assert!(
            desktop_release_identity_from_contract(&changed, &changed_raw).is_none(),
            "{key}"
        );
    }
}

#[test]
fn every_public_binding_is_required_and_invalid_values_withhold_identity() {
    let value = selection();
    for key in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(desktop_release_identity(&missing).is_none(), "{key}");
        let mut null = value.clone();
        null[key] = Value::Null;
        assert!(desktop_release_identity(&null).is_none(), "{key}");
    }
    for bad in [json!(null), json!([]), json!("release")] {
        assert!(desktop_release_identity(&bad).is_none());
    }
    for (key, bad) in [
        ("releaseId", json!(0)),
        ("releaseId", json!(9_007_199_254_740_992u64)),
        ("releaseId", json!(1.5)),
        ("sourceCommit", json!("B".repeat(40))),
        ("tag", json!("v1.15.0-alpha.12.polaris.2")),
        ("version", json!("1.15.0-alpha.12.polaris.01")),
        ("version", json!("1.15.0-alpha.11.polaris.1")),
        ("contractPath", json!("../release-contract.json")),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(desktop_release_identity(&changed).is_none(), "{key}");
    }
}

#[test]
fn temporary_default_has_its_own_exact_identity_and_never_relabels_formal() {
    let manifest: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "src-tauri/core-manifest.json"
    ))
    .unwrap();
    let selection = &manifest["historicalDesktopTemporary"];
    assert_eq!(
        desktop_temporary_identity(selection),
        Some((
            "1.15.0-polaris-alpha.12",
            "9b2fe41af73c5768dccff079ded1e4a489cfa9e1"
        ))
    );
    assert!(desktop_release_identity(selection).is_none());
    for key in selection.as_object().unwrap().keys() {
        let mut changed = selection.clone();
        changed[key] = Value::Null;
        assert!(desktop_temporary_identity(&changed).is_none(), "{key}");
    }
    let mut changed = selection.clone();
    changed["sourceCommit"] = json!("68ffb209ded1f9337f9115741cba24f9f4c63421");
    assert!(desktop_temporary_identity(&changed).is_none());
    let mut changed = selection.clone();
    changed["producer"]["runId"] = json!(39110306601u64);
    assert!(desktop_temporary_identity(&changed).is_none());
}

#[test]
fn archived_formal_identity_cannot_replace_the_current_compiled_release() {
    let pending: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/desktop-core/pending-release-selection.json"
    ))
    .unwrap();
    let old = &pending["historicalSelection"];
    let raw = polaris_source_probe::repo_file!(
        "scripts/desktop-core/historical-release-contract-577d4b9.json"
    );
    assert_eq!(
        desktop_release_identity_from_contract(old, &raw),
        Some((
            "1.15.0-alpha.12",
            "072dacb999c07c7948816f10efaaaf371da45eb9"
        ))
    );
    assert!(desktop_release_identity(old).is_none());
}

#[test]
fn shared_version_corpus_preserves_raw_patch_identity_without_promoting_pins() {
    let cases: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/lib/polaris-version-fixtures.json"
    ))
    .unwrap();
    for (group, accepted) in [("accepted", true), ("rejected", false)] {
        for value in cases[group].as_array().unwrap() {
            let version = value.as_str().unwrap();
            assert_eq!(upstream_release_version(version), accepted, "{version:?}");
            let (selection, raw) = bound_fixture(version, true);
            let identity = desktop_release_identity_from_contract(&selection, &raw);
            assert_eq!(identity.is_some(), accepted, "{version:?}");
            if accepted {
                assert_eq!(identity.unwrap().0, version);
            }
            // Synthetic valid syntax is not the currently compiled release.
            assert!(desktop_release_identity(&selection).is_none());
        }
    }
}

#[test]
fn patch_identity_requires_independent_contract_tag_source_and_asset_bindings() {
    let (value, raw) = bound_fixture("1.15.0-polaris-alpha.12.1", true);
    for (key, bad) in [
        ("version", json!("1.15.0-polaris-alpha.12")),
        ("tag", json!("v1.15.0-polaris-alpha.12")),
        ("tagObject", json!("f".repeat(40))),
        ("sourceCommit", json!("f".repeat(40))),
        ("sourceTree", json!("f".repeat(40))),
        ("releaseId", json!(18)),
        ("contractSha256", json!("f".repeat(64))),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(
            desktop_release_identity_from_contract(&changed, &raw).is_none(),
            "{key}"
        );
    }
    for platform in ["linux", "win"] {
        let mut changed = value.clone();
        changed["librarySha256"][platform] = json!("f".repeat(64));
        assert!(desktop_release_identity_from_contract(&changed, &raw).is_none());
    }
}

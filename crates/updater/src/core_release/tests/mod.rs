use super::*;
use serde_json::json;

fn selection() -> Value {
    let manifest: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "src-tauri/core-manifest.json"
    ))
    .unwrap();
    manifest["desktopRelease"].clone()
}

// Synthetic admission fixtures do not claim that these releases exist. The
// public entry point must still reject them until exact contract bytes ship.
fn bound(contract: &Value) -> (Value, String) {
    let raw = contract.to_string();
    let mut value = selection();
    value["contractSha256"] = json!(crate::sha256_hex(raw.as_bytes()));
    (value, raw)
}

fn contract_fixture(version: &str) -> Value {
    json!({
        "schema": "polaris-upstream-native-release-consumer-v1",
        "providerContract": "polaris-upstream-native-release-v1",
        "repository": "polaris-arch/sing-box-polaris",
        "repositoryId": 1_421_657_509u64,
        "version": version,
        "tag": format!("v{version}"),
        "sourceTree": "b".repeat(40),
    })
}

#[test]
fn current_selection_admits_the_compiled_contract_identity() {
    let contract: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/desktop-core/release-contract.json"
    ))
    .unwrap();
    assert_ne!(contract["sourceCommit"], contract["sourceTree"]);
    assert_eq!(
        desktop_release_identity(&selection()),
        Some((
            contract["version"].as_str().unwrap().to_owned(),
            contract["sourceTree"].as_str().unwrap().to_owned()
        ))
    );
}

#[test]
fn every_selection_binding_is_required_and_exact() {
    let value = selection();
    for key in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(desktop_release_identity(&missing).is_none(), "{key}");
        let mut null = value.clone();
        null[key] = Value::Null;
        assert!(desktop_release_identity(&null).is_none(), "{key}");
    }
    for (key, bad) in [
        ("contractSha256", json!("a".repeat(64))),
        ("contractPath", json!("scripts/desktop-core/arbitrary.json")),
        (
            "schema",
            json!("polaris-app-temporary-desktop-selection-v1"),
        ),
        ("extra", json!(true)),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(desktop_release_identity(&changed).is_none(), "{key}");
    }
    for bad in [json!(null), json!([]), json!("release")] {
        assert!(desktop_release_identity(&bad).is_none());
    }
}

#[test]
fn compiled_contract_hash_and_upstream_repository_are_checked_independently() {
    let contract = contract_fixture("1.15.0-polaris-alpha.12");
    let (value, raw) = bound(&contract);
    assert!(desktop_release_identity_from_contract(&value, &raw).is_some());
    // Valid synthetic bytes are still not the compiled release.
    assert!(desktop_release_identity(&value).is_none());
    assert!(desktop_release_identity_from_contract(&value, &(raw.clone() + "\n")).is_none());
    for (key, bad) in [
        ("schema", json!("polaris-box-release-consumer-contract-v1")),
        ("providerContract", json!("polaris-box-release-v1")),
        ("repository", json!("polaris-arch/polaris-box")),
        ("repositoryId", json!(1_407_705_262u64)),
        ("tag", json!("v1.15.0-polaris-alpha.13")),
        ("sourceTree", Value::Null),
    ] {
        let mut changed = contract.clone();
        changed[key] = bad;
        let (value, raw) = bound(&changed);
        assert!(
            desktop_release_identity_from_contract(&value, &raw).is_none(),
            "{key}"
        );
    }
}

#[test]
fn shared_version_corpus_matches_contract_admission() {
    let cases: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/lib/polaris-version-fixtures.json"
    ))
    .unwrap();
    for (group, accepted) in [("accepted", true), ("rejected", false)] {
        for value in cases[group].as_array().unwrap() {
            let version = value.as_str().unwrap();
            assert_eq!(upstream_release_version(version), accepted, "{version:?}");
            let (selection, raw) = bound(&contract_fixture(version));
            let identity = desktop_release_identity_from_contract(&selection, &raw);
            assert_eq!(
                identity.map(|(version, _)| version).as_deref(),
                accepted.then_some(version),
                "{version:?}"
            );
        }
    }
}

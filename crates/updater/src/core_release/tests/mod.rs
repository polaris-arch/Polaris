use super::*;
use serde_json::json;

fn selection() -> Value {
    json!({"schema":"polaris-app-release-selection-v1",
        "contractPath":"scripts/desktop-core/release-contract.json",
        "contractSha256":"a".repeat(64), "sourceCommit":"b".repeat(40),
        "sourceTree":"c".repeat(40), "tagObject":"d".repeat(40), "releaseId":17,
        "version":"1.15.0-alpha.12.polaris.1", "tag":"v1.15.0-alpha.12.polaris.1",
        "librarySha256":{"linux":"e".repeat(64), "win":"f".repeat(64)}})
}

#[test]
fn complete_release_identity_is_distinct_from_source_identity() {
    let value = selection();
    assert_eq!(
        desktop_release_identity(&value),
        Some((
            "1.15.0-alpha.12.polaris.1",
            "cccccccccccccccccccccccccccccccccccccccc"
        ))
    );
}

#[test]
fn plain_alpha12_is_admitted_only_with_the_complete_exact_release_binding() {
    let pending: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "scripts/desktop-core/pending-release-selection.json"
    ))
    .unwrap();
    let value = &pending["selection"];
    assert_eq!(
        desktop_release_identity(value),
        Some((
            "1.15.0-alpha.12",
            "072dacb999c07c7948816f10efaaaf371da45eb9"
        ))
    );
    let mut invalid = value.clone();
    invalid["tag"] = json!("v1.15.0-alpha.12.polaris.1");
    assert!(desktop_release_identity(&invalid).is_none());
    let mut invalid = value.clone();
    invalid["contractPath"] = json!("scripts/desktop-core/arbitrary.json");
    assert!(desktop_release_identity(&invalid).is_none());
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

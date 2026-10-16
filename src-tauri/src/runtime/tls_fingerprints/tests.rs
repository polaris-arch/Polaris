use super::*;
use serde_json::json;

fn document() -> Value {
    json!({
        "schema":"polaris-tls-fingerprint-capabilities-v1", "enabled":true,
        "coreVersion":"fixture", "sourceCommit":"", "sourceModified":null,
        "goVersion":"go1.27.2", "requiredGoVersion":"go1.27.2", "buildTags":"with_utls",
        "os":"linux", "arch":"amd64", "modules":[{"path":"github.com/refraction-networking/utls","version":"fixture","sum":"h1:fixture"}],
        "defaultFingerprint":"chrome", "profiles":[
            {"id":"chrome_155","displayName":"Chrome 155","client":"Chrome","templateVersion":"155","selection":"profile","templateOffersX25519MLKEM768":true},
            {"id":"safari_16_0","displayName":"Safari 16.0","client":"Safari","templateVersion":"16.0","selection":"profile","templateOffersX25519MLKEM768":false}],
        "legacyChromeAliases":["chrome_pq"], "shadowTLSSessionIDWithECHSupported":false,
        "curvePreferencesSupported":false, "realityX25519MLKEM768":"removed before authentication",
        "randomSemantics":"process browser", "randomizedSemantics":"process seeded",
        "negotiationEvidence":"offer only"
    })
}

#[test]
fn loaded_identity_binds_bytes_and_report_without_inventing_source_evidence() {
    let raw = document().to_string();
    let first = Snapshot::from_loaded(&raw, &"a".repeat(64)).unwrap();
    assert_eq!(first.document.as_ref().unwrap().source_modified, None);
    assert_eq!(first.document.as_ref().unwrap().source_commit, "");
    assert_ne!(
        first.identity,
        Snapshot::from_loaded(&raw, &"b".repeat(64))
            .unwrap()
            .identity
    );
    let mut changed = document();
    changed["profiles"][0]["templateVersion"] = json!("different template");
    assert_ne!(
        first.identity,
        Snapshot::from_loaded(&changed.to_string(), &"a".repeat(64))
            .unwrap()
            .identity
    );
    assert_eq!(
        serde_json::to_value(first.document).unwrap()["profiles"][0]
            ["templateOffersX25519MLKEM768"],
        true
    );
}

#[test]
fn final_generated_profile_admission_and_legacy_fallback() {
    let snapshot = Snapshot::from_loaded(&document().to_string(), &"a".repeat(64)).unwrap();
    let config =
        |id: &str| json!({"outbounds":[{"tls":{"utls":{"enabled":true,"fingerprint":id}}}]});
    for id in ["", "chrome_155", "safari_16_0", "chrome_pq"] {
        snapshot.validate_config(&config(id)).unwrap();
    }
    assert!(snapshot.validate_config(&config("safari_26_3")).is_err());
    assert!(snapshot.validate_config(&config("fake")).is_err());
    Snapshot::unavailable("old CLI")
        .validate_config(&config("chrome"))
        .unwrap();
    let mut disabled = document();
    disabled["enabled"] = json!(false);
    disabled["profiles"] = json!([]);
    disabled["legacyChromeAliases"] = json!([]);
    let disabled = Snapshot::from_loaded(&disabled.to_string(), &"a".repeat(64)).unwrap();
    assert!(disabled.validate_config(&config("")).is_err());
    disabled
        .validate_config(&json!({"outbounds":[{"tls":{"utls":{"enabled":false}}}]}))
        .unwrap();
}

#[test]
fn malformed_or_future_schema_is_unknown_not_a_menu_inventory() {
    let mutations: [fn(&mut Value); 7] = [
        |v: &mut Value| v["schema"] = json!("future"),
        |v: &mut Value| v["profiles"][0]["id"] = json!("guessed_version"),
        |v: &mut Value| v["profiles"][0]["extra"] = json!(true),
        |v: &mut Value| {
            let duplicate = v["profiles"][0].clone();
            v["profiles"].as_array_mut().unwrap().push(duplicate);
        },
        |v: &mut Value| v["enabled"] = json!(false),
        |v: &mut Value| v["sourceCommit"] = json!("ABC"),
        |v: &mut Value| v["curvePreferencesSupported"] = json!(true),
    ];
    for mutate in mutations {
        let mut value = document();
        mutate(&mut value);
        assert!(Snapshot::from_loaded(&value.to_string(), &"a".repeat(64)).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn actual_query_is_bounded_and_bound_to_exact_selected_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("polaris-tls-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let core = dir.join("core");
    let raw = document().to_string();
    std::fs::write(&core, format!("#!/bin/sh\n[ \"$1 $2\" = 'tools tls-fingerprints' ] || exit 1\ncat <<'POLARIS_DOCUMENT'\n{raw}\nPOLARIS_DOCUMENT\n")).unwrap();
    std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o700)).unwrap();
    let first = query_cli(&core).await;
    assert!(first.document.is_some());
    std::fs::write(&core, "#!/bin/sh\nexit 1\n").unwrap();
    let old = query_cli(&core).await;
    assert!(old.document.is_none());
    assert_ne!(old.binary_sha256, first.binary_sha256);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn old_loaded_library_keeps_byte_identity_without_inventing_a_capability_report() {
    let digest = "a".repeat(64);
    let old = Snapshot::from_native(None, &digest, "loaded-jni-so");
    assert_eq!(old.binary_sha256.as_deref(), Some(digest.as_str()));
    assert!(old.document.is_none());
    assert!(old.identity.is_none());
    assert!(Snapshot::from_native(None, "bad", "loaded-jni-so")
        .binary_sha256
        .is_none());
}

#[tokio::test]
async fn file_proof_rejects_atomic_replacement_even_with_equal_length_and_restored_mtime() {
    let dir = std::env::temp_dir().join(format!("polaris-tls-identity-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("selected-core");
    std::fs::write(&path, b"original").unwrap();
    let proof = open_observation(&path).await.unwrap();
    let before = proof.snapshot.binary_sha256.clone();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = dir.join("replacement");
    std::fs::write(&replacement, b"replaced").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(
        !proof.still_current(),
        "a matching pathname/size/mtime does not identify the old bytes"
    );
    assert_ne!(
        open_observation(&path)
            .await
            .unwrap()
            .snapshot
            .binary_sha256,
        before
    );
    assert!(!proof.matches(&dir.join("another-selection")));
    drop(proof);
    std::fs::remove_dir_all(dir).unwrap();
}

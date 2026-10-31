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
fn native_report_keeps_actual_container_scope_and_unknown_capabilities_separate() {
    let raw = document().to_string();
    let digest = "a".repeat(64);
    let loaded = Snapshot::from_native(Some(&raw), &digest, "loaded-jni-so");
    assert!(loaded.document.is_some());
    assert_eq!(loaded.observation_scope, "loaded-libbox");
    assert_eq!(loaded.byte_identity_scope, "loaded-jni-so");
    assert_eq!(loaded.binary_sha256.as_deref(), Some(digest.as_str()));
    assert_ne!(
        loaded.identity,
        Snapshot::from_native(Some(&raw), &"b".repeat(64), "packet-tunnel-executable").identity
    );
    for raw in [None, Some("not a capability report")] {
        let unknown = Snapshot::from_native(raw, &digest, "packet-tunnel-executable");
        assert!(unknown.document.is_none());
        assert!(unknown.identity.is_none());
        assert_eq!(unknown.binary_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(unknown.byte_identity_scope, "packet-tunnel-executable");
        assert!(unknown.unavailable_reason.is_some());
    }
    let invalid = Snapshot::from_native(Some(&raw), "invalid-digest", "loaded-jni-so");
    assert!(invalid.document.is_none());
    assert!(invalid.binary_sha256.is_none());
    assert!(invalid.identity.is_none());
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
    let dir = std::env::temp_dir().join(format!("polaris-tls-denial-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("sing-box");
    let marker = dir.join("executed");
    crate::test_support::write_executable_stand_in(
        &path,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let snapshot = query_cli(&path).await;
    assert!(snapshot.document.is_none());
    assert!(snapshot.binary_sha256.is_none());
    assert!(
        !marker.exists(),
        "a foreign same-named candidate must have zero query births"
    );
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
    let dir = std::env::temp_dir().join(format!("polaris-tls-replace-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("sing-box");
    std::fs::write(&path, b"original").unwrap();
    assert!(open_observation(&path).await.is_err());
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let other = dir.join("other");
    std::fs::write(&other, b"replaced").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(other, &path).unwrap();
    assert!(open_observation(&path).await.is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn transient_failure_retries_the_same_bytes_after_recovery_without_rehashing_or_spawning_on_every_read(
) {
    let dir =
        std::env::temp_dir().join(format!("polaris-tls-cache-denial-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("sing-box");
    let marker = dir.join("attempts");
    crate::test_support::write_executable_stand_in(
        &path,
        format!("#!/bin/sh\nprintf x >> '{}'\n", marker.display()),
    );
    let cache = CliProbeCache::new();
    for _ in 0..3 {
        let snapshot = cache
            .probe_at(&path, std::time::Instant::now(), ProbeFile::ScriptFixture)
            .await;
        assert!(snapshot.document.is_none());
        assert!(cache.observed(&path).is_none());
    }
    assert!(
        !marker.exists(),
        "query retries may not bypass executable admission"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_explicit_old_cli_dispatch_rejection_is_stable_absence() {
    assert!(unsupported_capability_command(
        "",
        "Error: unknown command \"tls-fingerprints\" for \"sing-box tools\""
    ));
    assert!(unsupported_capability_command(
        "No help topic for 'tls-fingerprints'",
        ""
    ));
    for transient in [
        "validation admission is closing",
        "cleanup remains unconfirmed",
        "permission denied",
        "query timed out",
        "unexpected exit 7",
    ] {
        assert!(!unsupported_capability_command("", transient));
    }
}

#[tokio::test]
async fn missing_cli_observation_preserves_the_unavailable_snapshot() {
    let dir = std::env::temp_dir().join(format!("polaris-tls-missing-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("missing-core");
    let unavailable = probe_cli_observation(&path).await.unwrap_err();
    assert!(unavailable.unavailable_reason.is_some());
    assert!(unavailable.binary_sha256.is_none());
    assert!(unavailable.document.is_none());
    assert_eq!(
        serde_json::to_value(unavailable.as_ref()).unwrap(),
        serde_json::to_value(probe_cli(&path).await).unwrap(),
        "the boxed failure must retain the complete legacy Snapshot payload"
    );
    std::fs::remove_dir(dir).unwrap();
}

#[tokio::test]
async fn app_business_observation_fixture_cannot_reach_the_native_query_factory() {
    let dir = crate::test_support::TestDir::new("polaris-tls-business-observation-");
    let path = dir.path().join("fixture-core");
    let marker = dir.path().join("must-not-exist");
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf spawned > '{}'\n", marker.display()),
    )
    .unwrap();
    let entry = cache_fixture_observation(&path);
    assert!(entry.still_current());
    assert!(entry.admission.inventory_execution().is_err());
    let result = query_observation(&entry).await;
    assert!(result.snapshot.document.is_none());
    assert!(!marker.exists());
}

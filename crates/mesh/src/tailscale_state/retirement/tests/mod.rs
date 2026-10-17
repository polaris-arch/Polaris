use super::*;
#[test]
fn history_has_current_anchor_without_relaxing_legacy_single_digest_validator() {
    let current = "a".repeat(64);
    let historical = "b".repeat(64);
    let export: TailscaleStoreExport = serde_json::from_value(serde_json::json!({
        "contractVersion": "polaris-ts-auth-writer-retirement-v1",
        "globalCleanupEvidence": "CleanupUnknown",
        "instances": [
            {"runNonce":"1".repeat(64), "configDigest": historical, "terminal":"NoStoreConstruction", "censusComplete":true, "nodes":[]},
            {"runNonce":"2".repeat(64), "configDigest": current, "terminal":"NoStoreConstruction", "censusComplete":true, "nodes":[]}
        ]
    })).unwrap();
    assert!(export.validate_history(&current, true).is_ok());
    assert!(export.validate(&current, true).is_err());
    assert!(export.validate_history(&"c".repeat(64), true).is_err());
    let mut malformed = export.clone();
    malformed.instances[0].config_digest = "invalid".into();
    assert!(malformed.validate_history(&current, true).is_err());
    let mut unknown = export.clone();
    unknown.instances[0].terminal = TailscaleWriterState::Unknown;
    assert!(unknown.validate_history(&current, true).is_err());
    let mut duplicate = export.clone();
    duplicate.instances[1].run_nonce = duplicate.instances[0].run_nonce.clone();
    assert!(duplicate.validate_history(&current, true).is_err());
}

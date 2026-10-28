use super::*;
fn export() -> TailscaleDirectoryExport {
    serde_json::from_value(serde_json::json!({
        "contractVersion":"polaris-ts-directory-retirement-v1", "globalCleanupEvidence":"CleanupUnknown",
        "instances":[{"runNonce":"a".repeat(64),"configDigest":"b".repeat(64),"terminal":"SealedDrained","censusComplete":true,
        "nodes":[{"tag":"ts-node","stateDirectory":"/state/tailscale/node","directoryIdentity":"unix:1:2:1:3","incarnation":"c".repeat(32),"directoryEpoch":"e".repeat(32),"writerState":"SealedDrained","censusVersion":1,"failure":""}]}]
    })).unwrap()
}
#[test]
fn reload_history_preserves_each_digest_with_a_current_anchor() {
    let mut evidence = export();
    let mut historical = evidence.instances[0].clone();
    historical.run_nonce = "f".repeat(64);
    historical.config_digest = "e".repeat(64);
    evidence.instances.insert(0, historical);
    let original = evidence.observed_runs().unwrap();
    assert!(evidence.validate(&"b".repeat(64), false).is_ok());
    assert!(evidence.validate_observed_runs(&original, true).is_ok());
    assert!(evidence.validate(&"d".repeat(64), false).is_err());
    evidence.instances[0].config_digest = "d".repeat(64);
    assert!(evidence.validate_observed_runs(&original, true).is_err());
}
#[test]
fn original_census_is_exact() {
    let mut evidence = export();
    let original = evidence.observed_runs().unwrap();
    assert!(evidence.validate_observed_runs(&original, true).is_ok());
    evidence.instances[0].nodes[0].incarnation = "d".repeat(32);
    assert!(evidence.validate_observed_runs(&original, true).is_err());
}
#[test]
fn old_run_partial_unknown_and_wrong_abi_never_authorize() {
    let original = export().observed_runs().unwrap();
    for change in 0..5 {
        let mut evidence = export();
        match change {
            0 => evidence.instances[0].run_nonce = "d".repeat(64),
            1 => evidence.instances[0].census_complete = false,
            2 => evidence.instances[0].nodes[0].writer_state = DirectoryWriterState::Unknown,
            3 => evidence.instances[0].nodes[0].census_version = 2,
            _ => {
                let node = evidence.instances[0].nodes[0].clone();
                evidence.instances[0].nodes.push(node);
            }
        }
        assert!(evidence.validate_observed_runs(&original, true).is_err());
    }
}
#[test]
fn no_construction_is_not_a_directory_proof() {
    let mut evidence = export();
    let original = evidence.observed_runs().unwrap();
    evidence.instances[0].nodes.clear();
    evidence.instances[0].terminal = DirectoryWriterState::NoDirectoryConstruction;
    assert!(evidence.validate_observed_runs(&original, true).is_err());
    assert!(evidence.instances()[0].nodes.is_empty());
}
#[test]
fn auth_contract_and_global_clean_claim_are_rejected() {
    let mut evidence = export();
    let original = evidence.observed_runs().unwrap();
    evidence.contract_version = "polaris-ts-store-retirement-v1".into();
    assert!(evidence.validate_observed_runs(&original, true).is_err());
    let mut evidence = export();
    evidence.global_cleanup_evidence = "NoOwner".into();
    assert!(evidence.validate_observed_runs(&original, true).is_err());
}

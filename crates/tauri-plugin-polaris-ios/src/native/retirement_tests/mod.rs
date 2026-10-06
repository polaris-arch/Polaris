use super::*;
use serde_json::{json, Value};

fn digest() -> String {
    "d".repeat(64)
}
fn node(tag: &str, directory: &str) -> Value {
    json!({"tag":tag,"stateDirectory":directory,"stateFile":format!("{directory}/tailscaled.state"),
        "writerState":"SealedDrained","stateFileState":"Regular","stateFileRevision":"a".repeat(64),
        "profileState":"Bound","profileFingerprint":"b".repeat(64)})
}
fn export() -> Value {
    json!({"contractVersion":"polaris-ts-auth-writer-retirement-v1","globalCleanupEvidence":"CleanupUnknown",
        "instances":[
            {"runNonce":"1".repeat(64),"configDigest":digest(),"terminal":"SealedDrained","censusComplete":true,
                "nodes":[node("ts-a","/private/group/ts-a"),node("ts-b","/private/group/ts-b")]},
            {"runNonce":"2".repeat(64),"configDigest":digest(),"terminal":"NoStoreConstruction","censusComplete":true,"nodes":[]}]})
}
fn stopped() -> Value {
    json!({"status":1,"running":false,"active":false,"profileExists":true,"ownership":"ownedSessionReported",
        "sessionId":"session-a","requestId":"start-a","configDigest":digest(),"runtimeStopped":true,
        "cleanupEvidence":"CleanupUnknown","lifecycle":"stopped","extensionGeneration":8,
        "tailscaleStoreRetirement":{"contractVersion":"polaris-ios-ts-store-retirement-v1","sessionID":"session-a",
            "startRequestID":"start-a","configDigest":digest(),"sourceExtensionGeneration":7,"stopExtensionGeneration":8,
            "stopRequestID":"stop-a","observationNonce":"nonce-a","globalCleanupEvidence":"CleanupUnknown","storeRetirement":export()}})
}
fn receipt(value: Value) -> StoppedSessionReceipt {
    StoppedSessionReceipt::from_stop(serde_json::from_value(value).unwrap(), "stop-a", "nonce-a")
}
fn custody() -> StartSessionCustody {
    StartSessionCustody(Arc::new(NativeStartCell {
        owner: Arc::new(()),
        native_generation: 7,
        shared_generation: 9,
        request_id: "start-a".into(),
        config: "private-config-secret".into(),
        no_store: OnceLock::new(),
        dispatched: std::sync::atomic::AtomicBool::new(false),
    }))
}
fn no_store() -> NativeNoStoreTerminal {
    serde_json::from_value(json!({"contractVersion":"polaris-ios-native-no-store-v1","requestID":"start-a",
        "startIntentFinished":true,"submissionAttempted":false,"storeConstruction":"NoStoreConstruction"})).unwrap()
}

#[test]
fn complete_multi_run_multi_node_scoped_terminal_preserves_all_facts() {
    let result = receipt(stopped());
    let terminal = result.retirement().unwrap();
    assert_eq!(result.snapshot().cleanup_evidence, "CleanupUnknown");
    assert_eq!(terminal.session_id(), "session-a");
    assert_eq!(terminal.start_request_id(), "start-a");
    assert_eq!(terminal.config_digest(), digest());
    assert_eq!(terminal.source_extension_generation(), 7);
    assert_eq!(terminal.stop_extension_generation(), 8);
    assert_eq!(terminal.stop_request_id(), "stop-a");
    assert_eq!(terminal.observation_nonce(), "nonce-a");
    assert_eq!(terminal.instances().len(), 2);
    assert_eq!(terminal.instances()[0].nodes.len(), 2);
    assert_eq!(
        terminal.instances()[0].nodes[1].profile_fingerprint,
        "b".repeat(64)
    );
    assert_eq!(
        terminal.instances()[1].terminal,
        TailscaleWriterState::NoStoreConstruction
    );
}

#[test]
fn ordinary_disconnected_and_old_disk_report_never_issue_typed_terminal() {
    let mut value = stopped();
    value
        .as_object_mut()
        .unwrap()
        .remove("tailscaleStoreRetirement");
    let result = receipt(value);
    assert!(!result.snapshot().active);
    assert_eq!(result.snapshot().runtime_stopped, Some(true));
    assert!(result.retirement().is_none());
    assert!(StoppedSessionReceipt::from_stop(
        serde_json::from_value(stopped()).unwrap(),
        "stop-b",
        "nonce-a"
    )
    .retirement()
    .is_none());
    assert!(StoppedSessionReceipt::from_stop(
        serde_json::from_value(stopped()).unwrap(),
        "stop-a",
        "nonce-b"
    )
    .retirement()
    .is_none());
}

#[test]
fn mismatched_session_config_generation_operation_and_failed_stop_are_unknown() {
    for (pointer, value) in [
        ("/sessionId", json!("session-b")),
        ("/requestId", json!("start-b")),
        ("/configDigest", json!("e".repeat(64))),
        ("/extensionGeneration", json!(9)),
        ("/status", json!(3)),
        ("/running", json!(true)),
        ("/active", json!(true)),
        ("/runtimeStopped", json!(false)),
        ("/lifecycle", json!("uncertain")),
        ("/ownership", json!("profileMatchedUnattested")),
        ("/cleanupEvidence", json!("MeshNoOwner")),
        ("/cleanupError", json!("close failed")),
        ("/lastError", json!("timeout")),
        ("/uncertainSettingsGeneration", json!(5)),
        ("/tailscaleStoreRetirement/contractVersion", json!("old")),
        ("/tailscaleStoreRetirement/sessionID", json!("session-b")),
        ("/tailscaleStoreRetirement/startRequestID", json!("start-b")),
        (
            "/tailscaleStoreRetirement/configDigest",
            json!("e".repeat(64)),
        ),
        (
            "/tailscaleStoreRetirement/sourceExtensionGeneration",
            json!(0),
        ),
        (
            "/tailscaleStoreRetirement/sourceExtensionGeneration",
            json!(6),
        ),
        (
            "/tailscaleStoreRetirement/sourceExtensionGeneration",
            json!(u64::MAX),
        ),
        (
            "/tailscaleStoreRetirement/stopExtensionGeneration",
            json!(9),
        ),
        ("/tailscaleStoreRetirement/stopRequestID", json!("stop-b")),
        (
            "/tailscaleStoreRetirement/observationNonce",
            json!("nonce-b"),
        ),
        (
            "/tailscaleStoreRetirement/globalCleanupEvidence",
            json!("MeshNoOwner"),
        ),
    ] {
        let mut candidate = stopped();
        if let Some(field) = candidate.pointer_mut(pointer) {
            *field = value;
        } else {
            candidate
                .as_object_mut()
                .unwrap()
                .insert(pointer.trim_start_matches('/').into(), value);
        }
        assert!(
            receipt(candidate).retirement().is_none(),
            "accepted {pointer}"
        );
    }
}

#[test]
fn omitted_instances_duplicate_runs_partial_census_and_active_writers_are_unknown() {
    for (pointer, value) in [
        ("/instances", json!([])),
        ("/contractVersion", json!("old")),
        ("/globalCleanupEvidence", json!("MeshNoOwner")),
        ("/instances/1/runNonce", json!("1".repeat(64))),
        ("/instances/0/runNonce", json!("nonce")),
        ("/instances/0/configDigest", json!("e".repeat(64))),
        ("/instances/0/censusComplete", json!(false)),
        ("/instances/0/terminal", json!("Unknown")),
        ("/instances/0/terminal", json!("NoStoreConstruction")),
        ("/instances/0/nodes/0/writerState", json!("Unknown")),
        ("/instances/0/nodes/1/tag", json!("ts-a")),
        (
            "/instances/0/nodes/1/stateDirectory",
            json!("/private/group/ts-a"),
        ),
    ] {
        let mut candidate = export();
        *candidate.pointer_mut(pointer).unwrap() = value;
        let parsed: TailscaleStoreExport = serde_json::from_value(candidate).unwrap();
        assert!(
            parsed.validate(&digest(), true).is_err(),
            "accepted {pointer}"
        );
    }
    let mut candidate = export();
    candidate["instances"][0]["nodes"][0]["extraProof"] = json!(true);
    assert!(serde_json::from_value::<TailscaleStoreExport>(candidate).is_err());
    let mut candidate = export();
    candidate["instances"][0]["nodes"][0]["writerState"] = json!("Closed");
    assert!(serde_json::from_value::<TailscaleStoreExport>(candidate).is_err());
}

#[test]
fn canonical_scope_file_revision_and_profile_facts_cannot_be_aliased() {
    for directory in [
        "relative/ts-a",
        "/",
        "/private//ts-a",
        "/private/./ts-a",
        "/private/../ts-a",
        "/private/ts-a/",
        "/private/ts\0-a",
    ] {
        let mut candidate = export();
        candidate["instances"][0]["nodes"][0]["stateDirectory"] = json!(directory);
        candidate["instances"][0]["nodes"][0]["stateFile"] =
            json!(format!("{directory}/tailscaled.state"));
        let parsed: TailscaleStoreExport = serde_json::from_value(candidate).unwrap();
        assert!(
            parsed.validate(&digest(), true).is_err(),
            "accepted {directory:?}"
        );
    }
    for (field, value) in [
        ("stateFile", json!("/other/tailscaled.state")),
        ("stateFileRevision", json!("stale")),
        ("profileFingerprint", json!("B".repeat(64))),
        ("profileState", json!("None")),
        ("stateFileState", json!("Missing")),
    ] {
        let mut candidate = export();
        candidate["instances"][0]["nodes"][0][field] = value;
        let parsed: TailscaleStoreExport = serde_json::from_value(candidate).unwrap();
        assert!(
            parsed.validate(&digest(), true).is_err(),
            "accepted {field}"
        );
    }
}

#[test]
fn active_original_scopes_support_readiness_but_never_retirement() {
    let mut active = export();
    for instance in active["instances"].as_array_mut().unwrap() {
        instance["terminal"] = json!("Unknown");
        instance["censusComplete"] = json!(false);
        for node in instance["nodes"].as_array_mut().unwrap() {
            node["writerState"] = json!("Unknown");
            node["stateFileState"] = json!("Unknown");
            node["stateFileRevision"] = json!("");
            node["profileState"] = json!("Unknown");
            node["profileFingerprint"] = json!("");
        }
    }
    let parsed: TailscaleStoreExport = serde_json::from_value(active.clone()).unwrap();
    assert!(parsed.validate(&digest(), false).is_ok());
    assert!(parsed.validate(&digest(), true).is_err());
    let mut ready = stopped();
    ready
        .as_object_mut()
        .unwrap()
        .remove("tailscaleStoreRetirement");
    for (field, value) in [
        ("status", json!(3)),
        ("running", json!(true)),
        ("active", json!(true)),
        ("runtimeStopped", json!(false)),
        ("lifecycle", json!("running")),
        ("extensionGeneration", json!(7)),
        ("tailscaleStoreScope", active),
    ] {
        ready[field] = value;
    }
    let snapshot: TunnelStatus = serde_json::from_value(ready.clone()).unwrap();
    let receipt = ReadySessionReceipt::from_start(&snapshot, 3, 5, "start-a").unwrap();
    assert_eq!(receipt.original_instances(), parsed.instances());
    assert!(ReadySessionReceipt::from_start(&snapshot, 3, 5, "start-b").is_err());
    assert_eq!(
        ObservedSessionReceipt::from_live(&snapshot)
            .unwrap()
            .original_instances(),
        receipt.original_instances()
    );
    ready.as_object_mut().unwrap().remove("tailscaleStoreScope");
    assert!(ReadySessionReceipt::from_start(
        &serde_json::from_value(ready).unwrap(),
        3,
        5,
        "start-a"
    )
    .is_err());
}

#[test]
fn no_store_needs_original_finished_non_submission_and_private_owner_identity() {
    for changed in [0, 1, 2, 3, 4] {
        let original = custody();
        let mut terminal = no_store();
        match changed {
            0 => terminal.contract_version = "old".into(),
            1 => terminal.request_id = "start-b".into(),
            2 => terminal.start_intent_finished = false,
            3 => terminal.submission_attempted = true,
            _ => terminal.store_construction = "Unknown".into(),
        };
        original.retain_no_store(&terminal);
        assert!(original.no_store_terminal().is_none());
    }
    let original = custody();
    assert!(original.no_store_terminal().is_none());
    assert!(!format!("{original:?}").contains("private-config-secret"));
    original.retain_no_store(&no_store());
    let proof = original.no_store_terminal().unwrap();
    assert!(proof.belongs_to(&original));
    assert!(proof.belongs_to(&original.clone()));
    assert!(!proof.belongs_to(&custody())); // Identical public strings cannot recreate custody.
    let newer = custody();
    let late = original.clone();
    late.retain_no_store(&no_store());
    assert!(newer.no_store_terminal().is_none());
    assert!(proof.belongs_to(&late));
    let mut wrong = no_store();
    wrong.submission_attempted = true;
    original.retain_no_store(&wrong);
    assert!(original.no_store_terminal().unwrap().belongs_to(&original));
}

#[test]
fn generic_reject_timeout_revoke_ack_and_malformed_json_are_not_terminals() {
    for value in [
        json!({"error":"timeout"}),
        json!({"revoked":true}),
        json!({"cleanupEvidence":"CleanupUnknown"}),
        json!({"nativeNoStoreTerminal":{"requestID":"start-a"},"startError":"cancelled"}),
    ] {
        assert!(serde_json::from_value::<NativeStartResponse>(value).is_err());
    }
    let value = json!({"nativeNoStoreTerminal":{"contractVersion":"polaris-ios-native-no-store-v1","requestID":"start-a",
        "startIntentFinished":true,"submissionAttempted":false,"storeConstruction":"NoStoreConstruction"},"startError":"PermissionDenied: save"});
    assert!(matches!(
        serde_json::from_value::<NativeStartResponse>(value).unwrap(),
        NativeStartResponse::NoStore { .. }
    ));
    for pointer in [
        "/tailscaleStoreRetirement/sourceExtensionGeneration",
        "/tailscaleStoreRetirement/stopExtensionGeneration",
    ] {
        let mut value = stopped();
        *value.pointer_mut(pointer).unwrap() = json!(true);
        assert!(serde_json::from_value::<TunnelStatus>(value).is_err());
    }
}

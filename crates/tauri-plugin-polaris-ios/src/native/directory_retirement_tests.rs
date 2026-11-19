use super::*;
use serde_json::{json, Value};

fn scope(terminal: bool) -> Value {
    json!({"contractVersion":"polaris-ts-directory-retirement-v1", "globalCleanupEvidence":"CleanupUnknown",
        "instances":[{"runNonce":"b".repeat(64),"configDigest":"a".repeat(64),
        "terminal":if terminal {"SealedDrained"} else {"Unknown"},"censusComplete":true,
        "nodes":[{"tag":"ts-a","stateDirectory":"/managed/ts-a","directoryIdentity":"unix:1:2:1:3",
        "incarnation":"c".repeat(32),"directoryEpoch":"e".repeat(32),"writerState":if terminal {"SealedDrained"} else {"Unknown"},
        "censusVersion":1,"failure":""}]}]})
}
fn stopped() -> Value {
    json!({"status":1,"running":false,"active":false,"profileExists":true,"ownership":"ownedSessionReported",
        "sessionId":"session-a","requestId":"start-a","configDigest":"a".repeat(64),"runtimeStopped":true,
        "cleanupEvidence":"CleanupUnknown","lifecycle":"stopped","extensionGeneration":2,
        "tailscaleDirectoryRetirement":{"contractVersion":"polaris-ios-ts-directory-retirement-v1",
        "sessionID":"session-a","startRequestID":"start-a","configDigest":"a".repeat(64),
        "sourceExtensionGeneration":1,"stopExtensionGeneration":2,"stopRequestID":"stop-a",
        "observationNonce":"nonce-a","globalCleanupEvidence":"CleanupUnknown",
        "originalScope":scope(false),"directoryRetirement":scope(true)}})
}
fn receipt(value: Value) -> StoppedSessionReceipt {
    StoppedSessionReceipt::from_stop(serde_json::from_value(value).unwrap(), "stop-a", "nonce-a")
}
#[test]
fn directory_receipt_is_distinct_from_auth_and_global_cleanup() {
    let result = receipt(stopped());
    assert!(result.directory_retirement().is_some());
    assert!(result.retirement().is_none());
    assert_eq!(result.snapshot().cleanup_evidence, "CleanupUnknown");
}
#[test]
fn directory_receipt_rejects_stale_stop_binding_and_host_only_status() {
    for (path, value) in [
        (
            "/tailscaleDirectoryRetirement/observationNonce",
            json!("old"),
        ),
        ("/tailscaleDirectoryRetirement/stopRequestID", json!("old")),
        ("/tailscaleDirectoryRetirement/sessionID", json!("other")),
        (
            "/tailscaleDirectoryRetirement/sourceExtensionGeneration",
            json!(2),
        ),
        (
            "/tailscaleDirectoryRetirement/contractVersion",
            json!("polaris-ios-ts-store-retirement-v1"),
        ),
        ("/active", json!(true)),
        ("/runtimeStopped", json!(false)),
        ("/lifecycle", json!("uncertain")),
        ("/cleanupError", json!("timeout")),
    ] {
        let mut value_root = stopped();
        if path == "/cleanupError" {
            value_root["cleanupError"] = value;
        } else {
            *value_root.pointer_mut(path).unwrap() = value;
        }
        assert!(
            receipt(value_root).directory_retirement().is_none(),
            "{path}"
        );
    }
}
#[test]
fn directory_receipt_requires_exact_original_census_and_terminal() {
    for (field, value) in [
        ("incarnation", json!("d".repeat(32))),
        ("directoryEpoch", json!("f".repeat(32))),
        ("directoryIdentity", json!("unix:1:2:1:4")),
        ("writerState", json!("Unknown")),
        ("failure", json!("drain timeout")),
    ] {
        let mut root = stopped();
        root["tailscaleDirectoryRetirement"]["directoryRetirement"]["instances"][0]["nodes"][0]
            [field] = value;
        assert!(receipt(root).directory_retirement().is_none(), "{field}");
    }
    let mut root = stopped();
    root["tailscaleDirectoryRetirement"]["directoryRetirement"]["instances"][0]["censusComplete"] =
        json!(false);
    assert!(receipt(root).directory_retirement().is_none());
    let mut root = stopped();
    root["tailscaleDirectoryRetirement"]["originalScope"]["instances"] = json!([]);
    assert!(receipt(root).directory_retirement().is_none());
}

#[test]
fn empty_or_absent_directory_scope_cannot_mint_directory_receipt() {
    let mut root = stopped();
    for export in ["originalScope", "directoryRetirement"] {
        root["tailscaleDirectoryRetirement"][export]["instances"][0]["nodes"] = json!([]);
        root["tailscaleDirectoryRetirement"][export]["instances"][0]["terminal"] =
            json!("NoDirectoryConstruction");
    }
    assert!(receipt(root).directory_retirement().is_none());
    let mut root = stopped();
    root.as_object_mut()
        .unwrap()
        .remove("tailscaleDirectoryRetirement");
    assert!(receipt(root).directory_retirement().is_none());
}

#[test]
fn prior_run_digests_remain_independent_with_current_digest_anchor() {
    let mut root = stopped();
    for key in ["originalScope", "directoryRetirement"] {
        let runs = root["tailscaleDirectoryRetirement"][key]["instances"]
            .as_array_mut()
            .unwrap();
        let mut old = runs[0].clone();
        old["runNonce"] = json!("f".repeat(64));
        old["configDigest"] = json!("d".repeat(64));
        runs.insert(0, old);
    }
    assert!(receipt(root.clone()).directory_retirement().is_some());
    root["tailscaleDirectoryRetirement"]["originalScope"]["instances"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(receipt(root.clone()).directory_retirement().is_some());
    root["tailscaleDirectoryRetirement"]["directoryRetirement"]["instances"][0]["configDigest"] =
        json!("e".repeat(64));
    assert!(receipt(root).directory_retirement().is_none());
    let mut root = stopped();
    root["tailscaleDirectoryRetirement"]["originalScope"]["instances"][0]["configDigest"] =
        json!("e".repeat(64));
    assert!(receipt(root).directory_retirement().is_none());
}
#[test]
fn missing_epoch_is_not_a_legacy_directory_proof() {
    let mut root = stopped();
    root["tailscaleDirectoryRetirement"]["directoryRetirement"]["instances"][0]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("directoryEpoch");
    assert!(serde_json::from_value::<TunnelStatus>(root).is_err());
}

fn live_auth(generation: u64, run_count: usize) -> Value {
    let runs: Vec<Value> = (0..run_count).map(|index| json!({
        "runNonce":format!("{:064x}",index+1),"configDigest":"a".repeat(64),
        "terminal":"Unknown","censusComplete":false,"nodes":[{
            "tag":"ts-a","stateDirectory":"/managed/ts-a","stateFile":"/managed/ts-a/tailscaled.state",
            "writerState":"Unknown","stateFileState":"Unknown","stateFileRevision":"",
            "profileState":"Unknown","profileFingerprint":""}]})).collect();
    json!({"status":3,"running":true,"active":true,"profileExists":true,"ownership":"ownedSessionReported",
        "sessionId":"session-a","requestId":"start-a","configDigest":"a".repeat(64),"runtimeStopped":false,
        "cleanupEvidence":"CleanupUnknown","lifecycle":"running","extensionGeneration":generation,
        "tailscaleStoreScope":{"contractVersion":"polaris-ts-auth-writer-retirement-v1",
        "globalCleanupEvidence":"CleanupUnknown","instances":runs}})
}

fn managed_cold_auth() -> Value {
    let mut live = live_auth(1, 3);
    for (index, store) in live["tailscaleStoreScope"]["instances"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if index == 0 {
            store["nodes"] = json!([]);
        } else {
            let path = format!("/managed/ts-{index}");
            store["configDigest"] = json!("c".repeat(64));
            store["nodes"][0]["tag"] = json!(format!("ts-{index}"));
            store["nodes"][0]["stateDirectory"] = json!(path);
            store["nodes"][0]["stateFile"] = json!(format!("{path}/tailscaled.state"));
        }
    }
    attach_startup_cohort(&mut live);
    live
}

fn attach_startup_cohort(live: &mut Value) {
    let runs = live["tailscaleStoreScope"]["instances"].as_array().unwrap();
    let cohort = json!({"contractVersion":"polaris-ts-startup-cohort-v1", "censusComplete":true,
        "parent":runs[0], "initialChildren":runs[1..]});
    live["managedColdStoreCohort"] = json!(cohort.to_string());
}

#[test]
fn managed_cold_ready_binds_empty_parent_and_exact_physical_births() {
    let ready = |value| {
        ReadySessionReceipt::from_start(&serde_json::from_value(value).unwrap(), 7, 9, "start-a")
    };
    let live = managed_cold_auth();
    let original = ready(live.clone()).unwrap();
    assert!(original.original_instances()[0].nodes.is_empty());
    let nodes = original.managed_cold_store_scope().unwrap().nodes();
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.tag.as_str())
            .collect::<Vec<_>>(),
        ["ts-1", "ts-2"]
    );

    for (path, replacement) in [
        ("/contractVersion", json!("unknown")),
        ("/censusComplete", json!(false)),
        ("/parent/configDigest", json!("b".repeat(64))),
        ("/initialChildren/0/runNonce", json!("f".repeat(64))),
        ("/initialChildren/0/nodes", json!([])),
        (
            "/initialChildren/1/nodes/0/stateDirectory",
            json!("/managed/unrelated"),
        ),
        ("/initialChildren/1/terminal", json!("SealedDrained")),
    ] {
        let mut mismatched = live.clone();
        let mut cohort: Value =
            serde_json::from_str(live["managedColdStoreCohort"].as_str().unwrap()).unwrap();
        *cohort.pointer_mut(path).unwrap() = replacement;
        mismatched["managedColdStoreCohort"] = json!(cohort.to_string());
        assert!(ready(mismatched).is_err(), "accepted {path}");
    }

    let mut raced = live.clone();
    let mut foreign = live["tailscaleStoreScope"]["instances"][2].clone();
    foreign["runNonce"] = json!("f".repeat(64));
    raced["tailscaleStoreScope"]["instances"]
        .as_array_mut()
        .unwrap()
        .push(foreign);
    assert!(ready(raced).is_err());

    let mut unattested = live;
    unattested
        .as_object_mut()
        .unwrap()
        .remove("managedColdStoreCohort");
    assert!(ready(unattested)
        .unwrap()
        .managed_cold_store_scope()
        .is_none());

    let mut legacy = live_auth(1, 1);
    attach_startup_cohort(&mut legacy);
    let ready = ready(legacy).unwrap();
    assert!(ready.managed_cold_store_scope().is_none());
    assert_eq!(ready.original_instances()[0].nodes[0].tag, "ts-a");
}

#[test]
fn managed_cold_stop_requires_terminal_proof_for_every_original_birth() {
    let live = managed_cold_auth();
    let snapshot = serde_json::from_value(live.clone()).unwrap();
    let ready = ReadySessionReceipt::from_start(&snapshot, 7, 9, "start-a").unwrap();
    let target = StopSessionTarget {
        host_generation: Arc::new(std::sync::atomic::AtomicU64::new(7)),
        cleanup_proof_eligible: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        session_id: ready.session_id.clone(),
        request_id: ready.request_id.clone(),
        config_digest: ready.config_digest.clone(),
        extension_generation: ready.extension_generation,
        original_instances: ready.original_instances.clone(),
    };
    let mut export = live["tailscaleStoreScope"].clone();
    for instance in export["instances"].as_array_mut().unwrap() {
        let nodes = instance["nodes"].as_array_mut().unwrap();
        let terminal = if nodes.is_empty() {
            "NoStoreConstruction"
        } else {
            "SealedDrained"
        };
        for node in nodes {
            node["writerState"] = json!("SealedDrained");
        }
        instance["terminal"] = json!(terminal);
        instance["censusComplete"] = json!(true);
    }
    let mut stopped = stopped();
    stopped["tailscaleStoreRetirement"] = json!({"contractVersion":"polaris-ios-ts-store-retirement-v1",
        "sessionID":"session-a", "startRequestID":"start-a", "configDigest":ready.config_digest,
        "sourceExtensionGeneration":1, "stopExtensionGeneration":2, "stopRequestID":"stop-a",
        "observationNonce":"nonce-a", "globalCleanupEvidence":"CleanupUnknown", "storeRetirement":export});
    assert!(ready
        .verify_managed_cold_retirement(&target, receipt(stopped.clone()).retirement().unwrap())
        .is_ok());
    assert!(ready
        .managed_cold_retired_node(&target, &receipt(stopped.clone()), "ts-2")
        .is_err());

    for (field, terminal) in [("originalScope", false), ("directoryRetirement", true)] {
        let mut export = scope(terminal);
        let template = export["instances"][0]["nodes"][0].clone();
        let runs: Vec<_> = ready
            .original_instances()
            .iter()
            .map(|run| {
                let nodes: Vec<_> = run
                    .nodes
                    .iter()
                    .map(|store| {
                        let mut node = template.clone();
                        node["tag"] = json!(store.tag);
                        node["stateDirectory"] = json!(store.state_directory);
                        node
                    })
                    .collect();
                let state = if !terminal {
                    "Unknown"
                } else if nodes.is_empty() {
                    "NoDirectoryConstruction"
                } else {
                    "SealedDrained"
                };
                json!({"runNonce": run.run_nonce, "configDigest": run.config_digest,
                "terminal": state, "censusComplete": true, "nodes": nodes})
            })
            .collect();
        export["instances"] = json!(runs);
        stopped["tailscaleDirectoryRetirement"][field] = export;
    }
    let terminal = receipt(stopped.clone());
    let node = ready
        .managed_cold_retired_node(&target, &terminal, "ts-2")
        .unwrap()
        .unwrap();
    assert_eq!(node.state_directory, "/managed/ts-2");
    assert!(ready
        .managed_cold_retired_node(&target, &terminal, "other")
        .is_err());

    stopped["tailscaleStoreRetirement"]["storeRetirement"]["instances"][2]["runNonce"] =
        json!("f".repeat(64));
    assert!(ready
        .verify_managed_cold_retirement(&target, receipt(stopped).retirement().unwrap())
        .is_err());
}
#[test]
fn original_start_reload_stop_keeps_owner_and_complete_auth_census() {
    let start: TunnelStatus = serde_json::from_value(live_auth(1, 1)).unwrap();
    let ready = ReadySessionReceipt::from_start(&start, 7, 9, "start-a").unwrap();
    let mut live_value = live_auth(2, 2);
    live_value["tailscaleStoreScope"]["instances"][0]["terminal"] = json!("SealedDrained");
    live_value["tailscaleStoreScope"]["instances"][0]["censusComplete"] = json!(true);
    live_value["tailscaleStoreScope"]["instances"][0]["nodes"][0]["writerState"] =
        json!("SealedDrained");
    let live: TunnelStatus = serde_json::from_value(live_value).unwrap();
    let updated = ready.from_reload_observation(&live).unwrap();
    assert_eq!(updated.native_generation, ready.native_generation);
    assert_eq!(updated.shared_generation, ready.shared_generation);
    assert_eq!(updated.extension_generation(), 2);
    assert_eq!(updated.original_instances().len(), 2);
    let observed = ObservedSessionReceipt::from_live(&start).unwrap();
    assert_eq!(
        observed
            .from_reload_observation(&live)
            .unwrap()
            .original_instances()
            .len(),
        2
    );
    let mut terminal = live_auth(2, 2)["tailscaleStoreScope"].clone();
    for run in terminal["instances"].as_array_mut().unwrap() {
        run["terminal"] = json!("SealedDrained");
        run["censusComplete"] = json!(true);
        run["nodes"][0]["writerState"] = json!("SealedDrained");
    }
    let mut value = stopped();
    value
        .as_object_mut()
        .unwrap()
        .remove("tailscaleDirectoryRetirement");
    value["extensionGeneration"] = json!(3);
    value["tailscaleStoreRetirement"] = json!({"contractVersion":"polaris-ios-ts-store-retirement-v1",
        "sessionID":"session-a","startRequestID":"start-a","configDigest":"a".repeat(64),
        "sourceExtensionGeneration":2,"stopExtensionGeneration":3,"stopRequestID":"stop-a",
        "observationNonce":"nonce-a","globalCleanupEvidence":"CleanupUnknown","storeRetirement":terminal});
    let stopped = receipt(value);
    let auth = stopped.retirement().unwrap();
    assert_eq!(
        auth.source_extension_generation(),
        updated.extension_generation()
    );
    assert_ne!(
        auth.source_extension_generation(),
        ready.extension_generation()
    );
    assert_eq!(auth.instances().len(), updated.original_instances().len());
}
#[test]
fn reload_rejects_wrong_owner_generation_and_lost_original_auth_scope() {
    let start: TunnelStatus = serde_json::from_value(live_auth(1, 1)).unwrap();
    let ready = ReadySessionReceipt::from_start(&start, 7, 9, "start-a").unwrap();
    for (path, value) in [
        ("/sessionId", json!("other")),
        ("/requestId", json!("other")),
        ("/configDigest", json!("f".repeat(64))),
        ("/extensionGeneration", json!(1)),
        ("/extensionGeneration", json!(3)),
        ("/lifecycle", json!("reloadAccepted")),
        (
            "/tailscaleStoreScope/instances/0/runNonce",
            json!("f".repeat(64)),
        ),
        (
            "/tailscaleStoreScope/instances/0/nodes/0/stateDirectory",
            json!("/managed/other"),
        ),
    ] {
        let mut value_root = live_auth(2, 2);
        *value_root.pointer_mut(path).unwrap() = value;
        let snapshot: TunnelStatus = serde_json::from_value(value_root).unwrap();
        assert!(ready.from_reload_observation(&snapshot).is_err(), "{path}");
    }
    let unchanged: TunnelStatus = serde_json::from_value(live_auth(1, 1)).unwrap();
    assert!(ready.from_reload_observation(&unchanged).is_ok());
    let missing: TunnelStatus = serde_json::from_value(live_auth(2, 1)).unwrap();
    assert!(ready.from_reload_observation(&missing).is_err());
}

#[test]
fn revocation_keeps_only_stop_authority_for_the_same_idle_native_owner() {
    let mut lifecycle = LifecycleState::default();
    let generation = lifecycle.begin_start(9, "start-a".into()).unwrap();
    assert!(!lifecycle.accepts_stop_target(generation));
    assert!(lifecycle.native_terminal(generation));
    assert!(lifecycle.finish(generation, Some(true)));
    assert!(lifecycle.accepts_receipt(generation, 9));
    assert!(lifecycle.revoke_start_through(9).is_none());
    assert!(!lifecycle.accepts_receipt(generation, 9));
    assert!(lifecycle.accepts_stop_target(generation));
    let successor = lifecycle.begin_start(10, "start-b".into()).unwrap();
    assert!(!lifecycle.accepts_stop_target(generation));
    assert!(!lifecycle.accepts_stop_target(successor));
    assert!(lifecycle.native_terminal(successor));
    assert!(lifecycle.finish(successor, Some(true)));
    assert!(!lifecycle.accepts_stop_target(generation));
}

#[test]
fn stop_dispatch_survives_pending_reload_missing_or_replayed_observation() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let start: TunnelStatus = serde_json::from_value(live_auth(1, 1)).unwrap();
    let ready = ReadySessionReceipt::from_start(&start, 1, 9, "start-a").unwrap();
    let mut pending = live_auth(2, 2);
    pending["lifecycle"] = json!("reloadAccepted");
    let pending: TunnelStatus = serde_json::from_value(pending).unwrap();
    for observed in [
        None,
        Some(("wrong-nonce", &start)),
        Some(("fresh", &pending)),
    ] {
        assert!(refine_stop_scope(
            "session-a",
            "start-a",
            &"a".repeat(64),
            1,
            ready.original_instances(),
            "fresh",
            observed
        )
        .is_none());
        let mut state = LifecycleState::default();
        let first = state.begin_start(9, "start-a".into()).unwrap();
        state.native_terminal(first);
        state.finish(first, Some(true));
        state.revoke_start_through(9);
        let target = AtomicU64::new(first);
        // Actual production stop_call_bound admission, not readiness admission.
        let dispatch = begin_bound_stop(&mut state, Some(&target)).unwrap();
        assert_eq!(target.load(Ordering::SeqCst), dispatch);
        assert!(!state.accepts_receipt(first, 9));
        // Simulate the first Swift call failing before NE submission.
        assert!(state.finish(dispatch, None));
        let retry = begin_bound_stop(&mut state, Some(&target)).unwrap();
        assert!(retry > dispatch);
        assert!(state.finish(retry, None));
        let successor = state.begin_start(10, "start-b".into()).unwrap();
        state.native_terminal(successor);
        state.finish(successor, Some(true));
        assert!(begin_bound_stop(&mut state, Some(&target)).is_err());
    }
}

#[test]
fn stop_only_downgrade_is_shared_and_never_revives_ready() {
    use std::sync::atomic::{AtomicBool, AtomicU64};
    let target = StopSessionTarget {
        host_generation: Arc::new(AtomicU64::new(1)),
        cleanup_proof_eligible: Arc::new(AtomicBool::new(true)),
        session_id: "session-a".into(),
        request_id: "start-a".into(),
        config_digest: "a".repeat(64),
        extension_generation: 1,
        original_instances: vec![],
    };
    let retained = target.clone();
    let ordinary = target.without_cleanup_proof();
    assert!(!ordinary.cleanup_proof_eligible());
    assert!(!retained.cleanup_proof_eligible());
    assert_eq!(ordinary.session_id(), "session-a");
}

#[test]
fn directory_source_identity_requires_exact_migrated_repository() {
    let source = json!({"schema":"polaris-mobile-fork-input-v1",
        "repository":"polaris-arch/sing-box-polaris", "repositoryId":1421657509u64,
        "sourceURL":"https://github.com/polaris-arch/sing-box-polaris",
        "sourceOverlay":0, "patches":[], "sourceCommit":"a".repeat(40)});
    assert_eq!(directory_source_identity(&source).unwrap(), "a".repeat(40));
    for (field, wrong) in [
        ("repository", json!("polaris-arch/polaris-box")),
        ("repositoryId", json!(1407705262u64)),
        (
            "sourceURL",
            json!("https://github.com/polaris-arch/polaris-box"),
        ),
        ("repository", json!("SagerNet/sing-box")),
        ("sourceCommit", json!("not-an-immutable-commit")),
        ("sourceOverlay", json!(1)),
    ] {
        let mut changed = source.clone();
        changed[field] = wrong;
        assert!(
            directory_source_identity(&changed).is_err(),
            "accepted {field}"
        );
    }
}

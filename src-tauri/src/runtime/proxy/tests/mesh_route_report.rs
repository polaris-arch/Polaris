use super::*;
use crate::runtime::tailscale_status::{TailscaleStatusDetails, TailscaleStatusEvent};
use polaris_config_engine::builder::endpoint_routes::{
    ForceRouteLeg, MeshRouteCandidate, MeshRouteEmissionCandidate, MeshRouteLoadEvidence,
    MeshRouteScope, MeshRouteSource, MeshRouteUnknownReason, SourcedCidr,
};
use polaris_config_engine::user_config::app_config::UserConfig;

fn ts_config(ids: &[&str], resolve_by_name: bool) -> (serde_json::Value, UserConfig) {
    let servers: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "protocol": "tailscale",
                "tailscaleSettings": { "resolveByName": resolve_by_name }
            })
        })
        .collect();
    let raw = serde_json::json!({
        "servers": servers,
        "selectedServerId": "__direct__",
        "proxyMode": "smart",
        "proxyModeType": "tun"
    });
    let config = serde_json::from_value(raw.clone()).unwrap();
    (raw, config)
}

fn status_frame(id: &str, address: &str) -> TailscaleStatusEvent {
    TailscaleStatusEvent {
        server_id: id.into(),
        backend_state: "Running".into(),
        logged_in: true,
        auth_url: None,
        tailscale_ips: vec![address.into()],
        expired: false,
        peers: Vec::new(),
        details: TailscaleStatusDetails::default(),
        can_share_files: false,
        waiting_file_count: 0,
        receiving_file_count: 0,
        unread_file_count: 0,
    }
}

#[tokio::test]
async fn final_gate_inline_absorption_keeps_unemitted_request() {
    let (rt, dir) = test_runtime();
    let (raw, config) = ts_config(&["ts-a", "ts-b"], false);
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let core_path = dir.join("core.json");
    let mut peeled = BTreeMap::new();
    let gate = rt
        .generate_and_gate(&config, &deps, &core_path, None, &mut peeled)
        .await
        .unwrap();
    assert_eq!(gate.mesh_route_candidates.len(), 2);
    assert_eq!(
        gate.mesh_route_candidates[1].candidate.leg,
        ForceRouteLeg::Inline
    );
    assert!(gate.mesh_route_candidates[1].emitted_inline.is_empty());
    let candidates_during_write = gate.mesh_route_candidates.clone();
    let core_before = std::fs::read_to_string(&core_path).unwrap();
    let evidence = rt.prepare_mesh_route_run(
        gate.mesh_route_candidates,
        gate.mesh_route_total_candidate_count,
        gate.mesh_route_diagnostics_limited,
        gate.mesh_route_dns_owner_server_id.clone(),
        &gate.invalid_nodes,
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    assert_eq!(evidence.load_evidence, MeshRouteLoadEvidence::Unknown);
    assert!(evidence.candidates[1].candidate.generated);
    rt.publish_mesh_route_run(evidence, 9);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(9),
        ..Default::default()
    };
    let report = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(report.snapshot.scope, MeshRouteScope::Applied);
    assert_eq!(
        report.results[1].coverage,
        polaris_config_engine::builder::endpoint_routes::MeshRouteCoverage::None
    );
    assert_eq!(report.results[1].effective, Some(Vec::new()));
    assert!(!report.results[1].requested.is_empty());
    assert_eq!(report.results[1].blocked_by[0].server_id, "ts-a");
    assert_eq!(std::fs::read_to_string(&core_path).unwrap(), core_before);

    // 取材时写者已进入奇数代；即使 ready 前写完，也不能补授加载 ACK。
    rt.tailnet_file_write_epoch.fetch_add(1, Ordering::SeqCst);
    let prepared_during_write = rt.prepare_mesh_route_run(
        candidates_during_write,
        2,
        false,
        None,
        &[],
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    assert_eq!(
        prepared_during_write.load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged
    );
    rt.tailnet_file_write_epoch.fetch_add(1, Ordering::SeqCst);
    rt.publish_mesh_route_run(prepared_during_write, 9);
    let unacknowledged = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(
        unacknowledged.snapshot.scope,
        MeshRouteScope::PersistedUnknown
    );
    assert_eq!(unacknowledged.results[1].effective, None);
}

#[tokio::test]
async fn final_gate_external_reads_retained_file_not_new_observation_projection() {
    let (rt, dir) = test_runtime();
    let (raw, config) = ts_config(&["ts-a"], true);
    rt.write_tailnet_rule_files(&config).await;
    rt.sync_tailnet_rule_files(&[status_frame("ts-a", "32.0.0.28")]);
    let rule_path = rt.tailnet_rules_dir().join(format!(
        "{}.json",
        polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base("ts-a")
    ));
    let rule_before = std::fs::read_to_string(&rule_path).unwrap();
    assert!(rule_before.contains("100.64.0.0/10"));
    assert!(rule_before.contains("32.0.0.28/32"));
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let core_path = dir.join("core.json");
    let mut peeled = BTreeMap::new();
    let gate = rt
        .generate_and_gate(&config, &deps, &core_path, None, &mut peeled)
        .await
        .unwrap();
    assert_eq!(
        gate.mesh_route_candidates[0].candidate.leg,
        ForceRouteLeg::ExternalRuleSet
    );
    let candidates_for_failure = gate.mesh_route_candidates.clone();
    let evidence = rt.prepare_mesh_route_run(
        gate.mesh_route_candidates,
        gate.mesh_route_total_candidate_count,
        gate.mesh_route_diagnostics_limited,
        gate.mesh_route_dns_owner_server_id.clone(),
        &gate.invalid_nodes,
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    assert_eq!(evidence.load_evidence, MeshRouteLoadEvidence::Unknown);
    rt.publish_mesh_route_run(evidence, 11);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(11),
        ..Default::default()
    };
    let report = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(report.snapshot.scope, MeshRouteScope::Applied);
    assert_eq!(report.snapshot.dns_owner_server_id.as_deref(), Some("ts-a"));
    assert!(report.results[0].requested.iter().any(|entry| {
        entry.cidr == "100.64.0.0/10" && entry.source == MeshRouteSource::Retained
    }));
    assert!(report.results[0].requested.iter().any(|entry| {
        entry.cidr == "32.0.0.28/32" && entry.source == MeshRouteSource::Observed
    }));
    assert_eq!(std::fs::read_to_string(&rule_path).unwrap(), rule_before);

    std::fs::remove_file(&rule_path).unwrap();
    let failed = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(failed.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        failed.snapshot.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert_eq!(failed.results[0].effective, None);

    let missing_before_spawn = rt.prepare_mesh_route_run(
        candidates_for_failure.clone(),
        1,
        false,
        gate.mesh_route_dns_owner_server_id.clone(),
        &[],
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    assert_eq!(
        missing_before_spawn.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert!(!missing_before_spawn.eligible_for_ack);
    assert!(missing_before_spawn.candidates[0]
        .candidate
        .unknown_reasons
        .contains(&MeshRouteUnknownReason::FileReadFailed));
    assert_eq!(
        missing_before_spawn.candidates[0].candidate.match_cidrs,
        None
    );

    let oversized = format!(
        "{{\"version\":1,\"rules\":[{{\"ip_cidr\":[\"{}\"]}}]}}",
        "a".repeat(1024 * 1024)
    );
    std::fs::write(&rule_path, oversized).unwrap();
    let limited_before_spawn = rt.prepare_mesh_route_run(
        candidates_for_failure,
        1,
        false,
        gate.mesh_route_dns_owner_server_id.clone(),
        &[],
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    assert_eq!(
        limited_before_spawn.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert!(!limited_before_spawn.eligible_for_ack);
    assert!(limited_before_spawn.candidates[0]
        .candidate
        .unknown_reasons
        .contains(&MeshRouteUnknownReason::ResourceLimitExceeded));
}

#[tokio::test]
async fn dns_owner_exists_without_any_force_route_claimant() {
    let (rt, dir) = test_runtime();
    let (mut raw, _) = ts_config(&["ts-dns-only"], true);
    raw["servers"][0]["tailscaleSettings"]["alwaysRouteSubnets"] = serde_json::Value::Bool(false);
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let core_path = dir.join("core.json");
    let mut peeled = BTreeMap::new();
    let gate = rt
        .generate_and_gate(&config, &deps, &core_path, None, &mut peeled)
        .await
        .unwrap();
    assert!(gate.mesh_route_candidates.is_empty());
    assert_eq!(gate.mesh_route_total_candidate_count, 0);
    assert_eq!(
        gate.mesh_route_dns_owner_server_id.as_deref(),
        Some("ts-dns-only")
    );
    let evidence = rt.prepare_mesh_route_run(
        gate.mesh_route_candidates,
        gate.mesh_route_total_candidate_count,
        gate.mesh_route_diagnostics_limited,
        gate.mesh_route_dns_owner_server_id,
        &gate.invalid_nodes,
        &config,
        &gate.config,
        &core_path,
        &gate.config_json,
        rt.gate.generation(),
    );
    rt.publish_mesh_route_run(evidence, 13);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(13),
        ..Default::default()
    };
    let report = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(report.snapshot.scope, MeshRouteScope::Applied);
    assert_eq!(
        report.snapshot.dns_owner_server_id.as_deref(),
        Some("ts-dns-only")
    );
    assert!(report.results.is_empty());
}

fn candidate(path: &std::path::Path) -> MeshRouteEmissionCandidate {
    MeshRouteEmissionCandidate {
        candidate: MeshRouteCandidate {
            server_id: "ts-a".into(),
            tag: "endpoint-ts-a".into(),
            leg: ForceRouteLeg::ExternalRuleSet,
            generated: true,
            generation_reason: None,
            engaged_reason: "alwaysRouteSubnets".into(),
            configured_cidrs: vec!["10.1.0.0/16".into()],
            observed_hosts: Vec::new(),
            referenced_file: None,
            match_cidrs: Some(vec![SourcedCidr {
                cidr: "10.1.0.0/16".into(),
                source: MeshRouteSource::Declared,
            }]),
            unknown_reasons: Vec::new(),
        },
        external_path: Some(path.to_string_lossy().into_owned()),
        emitted_inline: Vec::new(),
    }
}

fn ready_evidence(
    rt: &ProxyRuntime,
    core_path: &std::path::Path,
    rule_path: &std::path::Path,
    rule_content: &str,
) -> MeshRouteRunEvidence {
    MeshRouteRunEvidence {
        candidates: vec![candidate(rule_path)],
        total_candidate_count: 1,
        diagnostics_limited: false,
        file_baselines: BTreeMap::from([(
            rule_path.to_string_lossy().into_owned(),
            rule_content.into(),
        )]),
        core_config_path: core_path.to_string_lossy().into_owned(),
        core_config_sha256: Some(polaris_updater::verify::sha256_hex(b"{}")),
        write_epoch: rt.tailnet_file_write_epoch.load(Ordering::SeqCst),
        run_generation: rt.gate.generation(),
        ready_at_ms: None,
        load_evidence: MeshRouteLoadEvidence::Unknown,
        eligible_for_ack: true,
        dns_owner_server_id: None,
        preceding_exceptions: Vec::new(),
    }
}

#[test]
fn startup_ack_is_revoked_even_when_file_content_is_restored() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();
    let evidence = ready_evidence(&rt, &core_path, &rule_path, &original);
    rt.publish_mesh_route_run(evidence, 7);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(7),
        ..Default::default()
    };
    let applied = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(applied.snapshot.scope, MeshRouteScope::Applied);
    assert_eq!(
        applied.results[0].effective,
        Some(vec!["10.1.0.0/16".into()])
    );

    let changed = super::super::tailnet_rules::tailnet_rule_file_json(&["10.2.0.0/16".into()]);
    rt.write_tailnet_file_with_epoch(&rule_path, &changed, false)
        .unwrap();
    rt.write_tailnet_file_with_epoch(&rule_path, &original, false)
        .unwrap();
    let restored = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(restored.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        restored.snapshot.load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged
    );
    assert_eq!(restored.results[0].effective, None);
    assert!(restored.results[0].blocked_by.is_empty());

    rt.set_error("synthetic core failure", "TEST");
    let failed = rt.mesh_route_report(None);
    assert!(failed.snapshot.candidates.is_empty());
    assert!(failed
        .unknown_reasons
        .contains(&MeshRouteUnknownReason::RunningArtifactUnavailable));
}

#[test]
fn externally_changed_file_cannot_regain_startup_ack_after_restore() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();
    rt.publish_mesh_route_run(ready_evidence(&rt, &core_path, &rule_path, &original), 7);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(7),
        ..Default::default()
    };
    assert_eq!(
        rt.mesh_route_report(Some("r-version".into()))
            .snapshot
            .scope,
        MeshRouteScope::Applied
    );

    let changed = super::super::tailnet_rules::tailnet_rule_file_json(&["10.2.0.0/16".into()]);
    std::fs::write(&rule_path, changed).unwrap();
    let changed_report = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(
        changed_report.snapshot.scope,
        MeshRouteScope::PersistedUnknown
    );
    assert_eq!(
        changed_report.snapshot.load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged
    );
    assert_eq!(changed_report.results[0].effective, None);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), 0);

    std::fs::write(&rule_path, original).unwrap();
    let restored = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(restored.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        restored.snapshot.load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged
    );
    assert_eq!(restored.results[0].effective, None);
}

#[test]
fn unreadable_core_artifact_cannot_regain_startup_ack_after_restore() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();
    rt.publish_mesh_route_run(ready_evidence(&rt, &core_path, &rule_path, &original), 7);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(7),
        ..Default::default()
    };
    assert_eq!(
        rt.mesh_route_report(Some("r-version".into()))
            .snapshot
            .scope,
        MeshRouteScope::Applied
    );

    std::fs::remove_file(&core_path).unwrap();
    let missing = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(missing.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        missing.snapshot.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );

    std::fs::write(&core_path, "{}").unwrap();
    let restored = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(restored.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        restored.snapshot.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert_eq!(restored.results[0].effective, None);
}

#[test]
fn earlier_report_cannot_use_an_ack_revoked_by_another_reader() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();
    rt.publish_mesh_route_run(ready_evidence(&rt, &core_path, &rule_path, &original), 7);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(7),
        ..Default::default()
    };

    // Pause the first report just before its final ACK check. A second report
    // observes the external change and revokes the stored claim.
    let old_evidence = rt.mesh_route_run.read().unwrap().as_ref().unwrap().clone();
    let mut old_snapshot = rt.mesh_route_report(Some("r-version".into())).snapshot;
    assert_eq!(old_snapshot.scope, MeshRouteScope::Applied);
    let initial_snapshot = old_snapshot.clone();
    let changed = super::super::tailnet_rules::tailnet_rule_file_json(&["10.2.0.0/16".into()]);
    std::fs::write(&rule_path, changed).unwrap();
    assert_eq!(
        rt.mesh_route_report(Some("r-version".into()))
            .snapshot
            .scope,
        MeshRouteScope::PersistedUnknown
    );
    rt.reconcile_mesh_route_report_ack(&old_evidence, &mut old_snapshot);
    assert_eq!(old_snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        old_snapshot.load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged
    );

    // An old reader cannot revoke or regain a replacement startup's claim.
    let mut new_run = ready_evidence(&rt, &core_path, &rule_path, &original);
    new_run.run_generation += 1;
    new_run.ready_at_ms = Some(8);
    new_run.load_evidence = MeshRouteLoadEvidence::StartupReady;
    *rt.mesh_route_run.write().unwrap() = Some(new_run);
    let mut replaced_snapshot = initial_snapshot;
    rt.reconcile_mesh_route_report_ack(&old_evidence, &mut replaced_snapshot);
    assert!(replaced_snapshot.snapshot_stale);
    assert_eq!(replaced_snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        replaced_snapshot.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert_eq!(
        rt.mesh_route_run
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .load_evidence,
        MeshRouteLoadEvidence::StartupReady
    );
}

#[test]
fn absent_prepare_core_baseline_is_unknown_not_a_claim_of_file_write() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();
    let mut evidence = ready_evidence(&rt, &core_path, &rule_path, &original);
    evidence.core_config_sha256 = None;
    evidence.eligible_for_ack = false;
    rt.publish_mesh_route_run(evidence, 7);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        start_time: Some(7),
        ..Default::default()
    };
    let report = rt.mesh_route_report(Some("r-version".into()));
    assert_eq!(report.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(
        report.snapshot.load_evidence,
        MeshRouteLoadEvidence::Unknown
    );
    assert_eq!(report.results[0].effective, None);
}

#[test]
fn prepare_unacknowledged_and_old_generation_cannot_become_ready() {
    let (rt, dir) = test_runtime();
    let core_path = dir.join("config.json");
    let rule_path = dir.join("tailnet.json");
    let original = super::super::tailnet_rules::tailnet_rule_file_json(&["10.1.0.0/16".into()]);
    std::fs::write(&core_path, "{}").unwrap();
    std::fs::write(&rule_path, &original).unwrap();

    let mut during_prepare = ready_evidence(&rt, &core_path, &rule_path, &original);
    during_prepare.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
    rt.publish_mesh_route_run(during_prepare, 7);
    assert_eq!(
        rt.mesh_route_run
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .load_evidence,
        MeshRouteLoadEvidence::FileWrittenUnacknowledged,
    );

    *rt.mesh_route_run.write().unwrap() = None;
    let superseded = ready_evidence(&rt, &core_path, &rule_path, &original);
    rt.gate.bump_generation();
    rt.publish_mesh_route_run(superseded, 8);
    assert!(rt.mesh_route_run.read().unwrap().is_none());
}

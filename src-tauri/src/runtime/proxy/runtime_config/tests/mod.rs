use super::*;
use polaris_config_engine::user_config::dns_constants::DIRECT_SERVER_ID;
use serde_json::json;

#[test]
fn exit_ip_probe_requires_unique_manual_leaf_and_the_exact_core_receipt() {
    let mut snapshot = super::super::hot_switch::SwitchSnapshot::default();
    snapshot.id_to_tag.insert("p".into(), "leaf".into());
    let config = json!({
        "proxyMode": "smart",
        "selectedServerId": "p",
        "servers": [{"id": "p"}, {"id": "other"}],
    });
    let leaf = pinned_exit_ip_leaf(&config, &snapshot).unwrap();
    assert_eq!(leaf.server_id, "p");
    assert_eq!(leaf.tag, "leaf");
    for (field, incompatible) in [
        ("selectionIntent", json!({"mode":"auto","scope":"global"})),
        ("proxyMode", json!("direct")),
        ("selectedServerId", json!(DIRECT_SERVER_ID)),
        ("selectedServerId", json!("other")),
        ("servers", json!([{"id":"p"}, {"id":"p"}])),
    ] {
        let mut changed = config.clone();
        changed[field] = incompatible;
        assert!(pinned_exit_ip_leaf(&changed, &snapshot).is_none());
    }
    snapshot.endpoint_tags.insert("leaf".into());
    assert!(pinned_exit_ip_leaf(&config, &snapshot).is_none());
    snapshot.endpoint_tags.clear();
    snapshot.static_detours.insert("leaf".into(), "hop".into());
    assert!(pinned_exit_ip_leaf(&config, &snapshot).is_none());

    let mut caps = paired_capabilities();
    let active = caps.active.clone().unwrap();
    caps.control_revision = 8;
    assert!(!exit_ip_graph_matches(&caps, &active, 8));
    caps.exit_ip_probe_version = EXIT_IP_PROBE_VERSION;
    assert!(exit_ip_graph_matches(&caps, &active, 8));
    assert!(exit_ip_graph_matches(&caps, &active, 7));
    assert!(!exit_ip_graph_matches(&caps, &active, 9));
    caps.active.as_mut().unwrap().applied_revision += 1;
    assert!(!exit_ip_graph_matches(&caps, &active, 8));
    caps.active.as_mut().unwrap().applied_revision -= 1;
    caps.active.as_mut().unwrap().effective_digest = "d".repeat(64);
    assert!(!exit_ip_graph_matches(&caps, &active, 8));

    let request = daemon::ProbeExitIpRequest {
        version: EXIT_IP_PROBE_VERSION,
        expected: Some(active),
        expected_control_revision: 8,
        leaf_tag: "leaf".into(),
    };
    let trace = b"ip=192.0.2.8\nloc=US\n".to_vec();
    let receipt = daemon::ProbeExitIpResponse {
        active: request.expected.clone(),
        control_revision: request.expected_control_revision,
        leaf_tag: request.leaf_tag.clone(),
        trace_body: trace.clone(),
    };
    assert_eq!(
        verified_exit_ip_trace(&request, receipt.clone()),
        Some(trace)
    );
    for mutate in 0..6 {
        let mut changed = receipt.clone();
        match mutate {
            0 => changed.control_revision += 1,
            1 => changed.leaf_tag = "other".to_string(),
            2 => changed.active.as_mut().unwrap().incarnation.push('x'),
            3 => changed.active.as_mut().unwrap().applied_revision += 1,
            4 => changed.active.as_mut().unwrap().effective_digest.push('x'),
            5 => changed.active = None,
            _ => unreachable!(),
        }
        assert!(verified_exit_ip_trace(&request, changed).is_none());
    }
}

#[cfg(not(target_os = "android"))]
#[tokio::test]
async fn exit_ip_routes_reject_active_switch_and_stale_graphs() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let generation = pending.generation;
    runtime.status.write().unwrap().mixed_port = 7890;
    let receipt = committed_ts_receipt(&pending);
    let active = receipt.active.clone();
    let (_fixture, server) = native_retirement_rpc_peer(&runtime, &pending, receipt).await;
    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .observed_active = active;
    *runtime.current_config.write().unwrap() = Some(json!({
        "proxyMode": "smart",
        "selectedServerId": "p",
        "servers": [{"id":"p"}],
    }));
    *runtime.switch_snapshot.write().unwrap() = Some(super::super::hot_switch::SwitchSnapshot {
        id_to_tag: BTreeMap::from([("p".into(), "leaf".into())]),
        probe_proxy_port: Some(31002),
        ..Default::default()
    });
    let route = runtime.exit_ip_http_route(generation).unwrap();
    assert_eq!(route.proxy.port, 31002);
    assert_eq!(route.proxy.inbound, super::super::LocalInbound::ProbeProxy);
    assert!(runtime.exit_ip_http_route(generation + 1).is_none());
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), Some(()));
    let pinned = runtime.pinned_exit_ip_route().unwrap();
    let switch = runtime.switch_serial.try_lock().unwrap();
    assert!(runtime.pinned_exit_ip_route().is_none());
    assert_eq!(runtime.with_pinned_exit_ip_route(&pinned, || ()), None);
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), None);
    let wait = runtime.wait_for_exit_ip_http_route(generation);
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
    drop(switch);
    assert_eq!(wait.await.unwrap().proxy.port, 31002);
    assert_eq!(runtime.with_pinned_exit_ip_route(&pinned, || ()), Some(()));

    let switch = runtime.switch_serial.try_lock().unwrap();
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), None);
    let wait = runtime.wait_for_exit_ip_http_route(generation);
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .observed_control_revision += 1;
    runtime.gen_changed.notify_waiters();
    assert!(futures::poll!(&mut wait).is_pending());
    drop(switch);
    let updated_route = wait.await.unwrap();
    let failed_snapshot = json!({"proxy": null, "proxyReachability": "unreachable"});
    assert_eq!(
        runtime.with_exit_ip_http_route(&route, || &failed_snapshot),
        None
    );
    assert_eq!(
        runtime.with_exit_ip_http_route(&updated_route, || &failed_snapshot),
        Some(&failed_snapshot)
    );
    let route = updated_route;
    let pinned = runtime.pinned_exit_ip_route().unwrap();

    runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), None);
    assert_eq!(runtime.with_pinned_exit_ip_route(&pinned, || ()), None);
    assert!(runtime.exit_ip_http_route(generation).is_none());

    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .observed_active = pending.plan.expected.clone();
    pending.published = true;
    runtime.runtime_config_apply.lock().unwrap().pending = Some(pending);
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), None);
    assert_eq!(runtime.with_pinned_exit_ip_route(&pinned, || ()), None);
    let route = runtime.exit_ip_http_route(generation).unwrap();
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), Some(()));
    let switch = runtime.switch_serial.try_lock().unwrap();
    let wait = runtime.wait_for_exit_ip_http_route(generation);
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
    runtime.register_selector_intent();
    drop(switch);
    assert!(wait.await.is_none());

    let route = runtime.exit_ip_http_route(generation).unwrap();
    runtime.set_login_fallback(true, Some("pending-login".into()));
    assert_eq!(runtime.with_exit_ip_http_route(&route, || ()), None);
    let switch = runtime.switch_serial.try_lock().unwrap();
    let wait = runtime.wait_for_exit_ip_http_route(generation);
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
    runtime.bump_generation();
    assert!(matches!(
        futures::poll!(&mut wait),
        std::task::Poll::Ready(None)
    ));
    drop(switch);
    server.abort();
}

pub(super) fn paired_capabilities() -> daemon::RuntimeCapabilities {
    daemon::RuntimeCapabilities {
        contract_version: 1,
        connected_hot_enabled: true,
        implementation_manifest_digest: "a".repeat(64),
        active: Some(daemon::RuntimeIdentity {
            incarnation: "owned-incarnation".into(),
            applied_revision: 1,
            effective_digest: "c".repeat(64),
        }),
        fields: vec![daemon::RuntimeFieldCapability {
            effective_path: "runtime.protocol".into(),
            variant: "polaris-daily-runtime-v1".into(),
            platform_build: runtime_platform_build(),
            update_class: 0,
            adapter_version: "1".into(),
            conditions: RUNTIME_PROTOCOL_METHODS
                .iter()
                .map(|value| (*value).into())
                .collect(),
        }],
        ..Default::default()
    }
}

fn instance_reason(variant: &str, path: &str) -> daemon::RuntimeFieldCapability {
    daemon::RuntimeFieldCapability {
        effective_path: "runtime.instance".into(),
        variant: variant.into(),
        conditions: vec![path.into()],
        ..Default::default()
    }
}

#[test]
fn paired_runtime_requires_exact_protocol_not_only_a_capability_switch() {
    let valid = paired_capabilities();
    assert!(observe_runtime_pairing(&valid).is_ok());
    for change in 0..8 {
        let mut value = valid.clone();
        match change {
            0 => value.contract_version = 2,
            1 => value.fields.clear(),
            2 => value.fields.push(value.fields[0].clone()),
            3 => value.fields[0].variant = "foreign-runtime".into(),
            4 => value.fields[0].adapter_version = "2".into(),
            5 => value.fields[0].platform_build = "foreign/target".into(),
            6 => value.fields[0].conditions.swap(0, 1),
            _ => value.implementation_manifest_digest.clear(),
        }
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
    }
}

#[test]
fn paired_runtime_preserves_partial_support_and_unbound_owner_warning() {
    let mut value = paired_capabilities();
    assert!(!value.full_effective_schema_supported);
    value.fields.push(instance_reason(
        "OWNER_PERSISTENCE_UNBOUND",
        "runtime.persistence",
    ));
    value.fields.push(daemon::RuntimeFieldCapability {
        effective_path: "runtime.build".into(),
        variant: "with_naive_outbound".into(),
        conditions: vec!["state=unknown".into()],
        ..Default::default()
    });
    assert!(observe_runtime_pairing(&value).is_ok());
}

#[test]
fn paired_runtime_binds_incarnation_and_manifest_but_not_mutable_revisions() {
    let mut value = paired_capabilities();
    let pairing = observe_runtime_pairing(&value).unwrap();
    value.active.as_mut().unwrap().applied_revision += 1;
    value.active.as_mut().unwrap().effective_digest = "d".repeat(64);
    value.control_revision += 1;
    assert!(runtime_pairing_matches(&pairing, &value));
    value.active.as_mut().unwrap().incarnation = "replacement".into();
    assert!(!runtime_pairing_matches(&pairing, &value));
    value.active.as_mut().unwrap().incarnation = "owned-incarnation".into();
    value.implementation_manifest_digest = "b".repeat(64);
    assert!(!runtime_pairing_matches(&pairing, &value));
}

#[test]
fn paired_runtime_rejects_missing_or_zero_started_identity() {
    for change in 0..6 {
        let mut value = paired_capabilities();
        match change {
            0 => value.active = None,
            1 => value.active.as_mut().unwrap().incarnation.clear(),
            2 => value.active.as_mut().unwrap().applied_revision = 0,
            3 => value.active.as_mut().unwrap().effective_digest.clear(),
            4 => value.active.as_mut().unwrap().effective_digest = "next".into(),
            _ => value.active.as_mut().unwrap().effective_digest = "A".repeat(64),
        }
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
    }
}

#[test]
fn paired_runtime_only_waits_for_noncontradictory_starting_state() {
    let mut value = paired_capabilities();
    value
        .fields
        .push(instance_reason("RUNTIME_STARTING", "runtime"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.connected_hot_enabled = false;
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Starting)
    );
    value
        .fields
        .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
}

#[test]
fn paired_runtime_never_admits_stopped_or_unavailable_graph() {
    for reason in [
        "RUNTIME_STOPPED",
        "RUNTIME_GRAPH_UNAVAILABLE",
        "INITIAL_CONFIG_ENCODING_FAILED",
    ] {
        let mut value = paired_capabilities();
        value.fields.push(instance_reason(reason, "runtime"));
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
        value.connected_hot_enabled = false;
        value.active = None;
        value
            .fields
            .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
    }
}

#[test]
fn unsupported_graph_needs_completed_start_and_cannot_adopt_an_incarnation() {
    let mut value = paired_capabilities();
    value.connected_hot_enabled = false;
    value.active = None;
    value
        .fields
        .push(instance_reason("PENDING_ADAPTER", "services"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value
        .fields
        .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
    let pairing = observe_runtime_pairing(&value).unwrap();
    assert_eq!(pairing.incarnation, None);
    assert_eq!(unsupported_runtime_paths(&value), ["services"]);
    let supported = paired_capabilities();
    assert!(!runtime_pairing_matches(&pairing, &supported));
}

#[test]
fn malformed_or_contradictory_dynamic_records_are_not_admission() {
    let mut value = paired_capabilities();
    value
        .fields
        .push(instance_reason("OWNER_PERSISTENCE_UNBOUND", "wrong.path"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.fields.pop();
    value
        .fields
        .push(instance_reason("PENDING_ADAPTER", "services"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.connected_hot_enabled = false;
    value.active = None;
    value
        .fields
        .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
    assert!(observe_runtime_pairing(&value).is_ok());
    value
        .fields
        .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.fields.pop();
    value.fields[1].conditions.clear();
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.fields.remove(1);
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
}

#[test]
fn startup_pairing_precedes_running_and_uses_original_remaining_deadline() {
    let source = crate::test_support::crate_code("runtime/proxy/startup.rs");
    let gate = source
        .find(".confirm_startup_runtime_pairing(&singbox_config, my_gen, ready_deadline)")
        .unwrap();
    assert!(gate < source.find("let new_status = ProxyStatus {").unwrap());
    assert!(
        gate < source
            .find("self.capture_runtime_config_baseline(")
            .unwrap()
    );
    assert!(source.contains("t_ready + Duration::from_millis(ready_budget_ms)"));
    let body = crate::commands::guard_scan::impl_method_body(
        &source,
        "    async fn confirm_startup_runtime_pairing(",
    );
    let compact: String = body.split_whitespace().collect();
    assert!(compact.contains("client.get_runtime_capabilities().await"));
    assert!(body.contains("tokio::time::timeout_at("));
    assert!(body.contains("self.gate.generation() != generation"));
    assert!(!body.contains("cfg(test)"));
}

#[test]
fn unverified_peers_never_receive_a_compatibility_restart_choice() {
    let source = crate::test_support::crate_code("runtime/proxy/runtime_config.rs");
    let body = crate::commands::guard_scan::impl_method_body(
        &source,
        "    async fn prepare_runtime_candidate(",
    );
    assert!(body.contains("paired_runtime_capabilities(&client, &baseline.pairing)"));
    assert!(!body.contains("unsupportedCore"));
    assert!(!body.contains("RuntimeConfigError::Unsupported"));
}

#[test]
fn pairing_mismatch_is_explicit_without_authorizing_restart() {
    let result = runtime_pairing_mismatch();
    assert_eq!(result.status, "unconfirmed");
    assert_eq!(result.reasons[0].code, "CORE_BUNDLE_MISMATCH");
    assert_eq!(result.reasons[0].path, "runtime.protocol");
    assert!(!result.ok);
    assert!(!result.restart_required);
    assert!(result.restart_confirmation.is_none());
}

#[test]
fn runtime_instance_text_must_be_concrete_and_bounded() {
    for incarnation in ["   ".into(), "x".repeat(257)] {
        let mut value = paired_capabilities();
        value.active.as_mut().unwrap().incarnation = incarnation;
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
    }
    for path in ["", "   ", "runtime"] {
        let mut value = paired_capabilities();
        value.connected_hot_enabled = false;
        value.active = None;
        value
            .fields
            .push(instance_reason("RUNTIME_STARTED_UNSUPPORTED", "runtime"));
        value.fields.push(instance_reason("PENDING_ADAPTER", path));
        assert_eq!(
            observe_runtime_pairing(&value),
            Err(RuntimePairingError::Invalid)
        );
    }
    let mut value = paired_capabilities();
    value.connected_hot_enabled = false;
    value
        .fields
        .push(instance_reason("RUNTIME_STARTING", "wrong.path"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
    value.fields.last_mut().unwrap().conditions = vec!["runtime".into()];
    value
        .fields
        .push(instance_reason("UNKNOWN_FAILURE", "runtime"));
    assert_eq!(
        observe_runtime_pairing(&value),
        Err(RuntimePairingError::Invalid)
    );
}

#[test]
fn cached_results_owner_ack_and_retirement_revalidate_current_pairing() {
    let source = crate::test_support::crate_code("runtime/proxy/runtime_config.rs");
    for head in [
        "    pub(crate) async fn retry_runtime_persistence(",
        "    async fn runtime_apply_status_inner(",
        "    fn spawn_runtime_retirement_reconcile(",
    ] {
        let body = crate::commands::guard_scan::impl_method_body(&source, head);
        assert!(body.contains("verify_cached_runtime_pairing("), "{head}");
    }
    for head in [
        "    async fn acknowledge_regeneration_owner(",
        "    fn spawn_runtime_retirement_reconcile(",
    ] {
        let body = crate::commands::guard_scan::impl_method_body(&source, head);
        assert!(body.contains("paired_runtime_capabilities("), "{head}");
    }
}

#[test]
fn restart_choice_keeps_the_original_observation_generation_and_intent() {
    let source = crate::test_support::crate_code("runtime/proxy/runtime_config.rs");
    let body = crate::commands::guard_scan::impl_method_body(
        &source,
        "    fn restart_choice_at_revision(",
    );
    assert!(body.contains("selector_operation_is_current(generation, intent)"));
    assert!(!body.contains("let generation = self.gate.generation()"));
    assert!(!body.contains("let intent = self.selector_reconcile.intent_generation()"));
}

fn group(tag: &str, selected: &str, members: &[&str]) -> daemon::Group {
    daemon::Group {
        tag: tag.into(),
        selected: selected.into(),
        items: members
            .iter()
            .map(|tag| daemon::GroupItem {
                tag: (*tag).into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
fn graph(default: &str, members: &[&str]) -> Value {
    json!({"outbounds":[{"type":"selector","tag":"scope","default":default,"outbounds":members}]})
}
#[test]
fn unchanged_controls_preserve_actual_winner() {
    let config = graph("a", &["a", "b"]);
    assert!(
        selector_intents(&config, &config, &[group("scope", "b", &["a", "b"])], false)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn deleted_active_member_binds_fallback_to_observed_path() {
    let intents = selector_intents(
        &graph("a", &["a", "b"]),
        &graph("a", &["a"]),
        &[group("scope", "b", &["a", "b"])],
        false,
    )
    .unwrap();
    assert_eq!(intents[0].selected, "a");
    assert_eq!(intents[0].expected_selection_path, ["scope", "b"]);
    assert!(!intents[0].pin_current_tcp);
}
#[test]
fn freeze_requires_exact_tcp_terminal() {
    let old = graph("scope-test", &["scope-test", "a", "b"]);
    let groups = [
        group("scope", "scope-test", &["scope-test", "a", "b"]),
        group("scope-test", "a", &["a", "b"]),
    ];
    assert!(selector_intents(&old, &graph("b", &["a", "b"]), &groups, true).is_err());
    let intents = selector_intents(&old, &graph("a", &["a", "b"]), &groups, true).unwrap();
    assert!(intents[0].pin_current_tcp);
    assert_eq!(
        intents[0].expected_selection_path,
        ["scope", "scope-test", "a"]
    );
}
#[test]
fn invalid_members_and_cycles_are_rejected() {
    assert!(selector_intents(&graph("a", &["a"]), &graph("b", &[]), &[], false).is_err());
    assert!(selector_intents(
        &graph("a", &["a"]),
        &graph("b", &["b"]),
        &[group("scope", "scope", &["scope"])],
        false
    )
    .is_err());
}
#[test]
fn unchanged_tun_and_cache_are_not_restart_reasons() {
    let old =
        json!({"tunConfig":{"mtu":1500},"cachePath":"same","dnsConfig":{"optimisticCache":false}});
    let mut next = old.clone();
    next["dnsConfig"]["optimisticCache"] = json!(true);
    assert!(!structural_runtime_change(&old, &next, Platform::Linux));
    next["tunConfig"]["mtu"] = json!(1400);
    assert!(structural_runtime_change(&old, &next, Platform::Linux));
}

fn host_dns_restart_fixture() -> (
    Arc<ProxyRuntime>,
    crate::test_support::TestDir,
    Value,
    Value,
) {
    let (rt, dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let mut raw = runnable_config_fixture();
    raw["proxyModeType"] = "tun".into();
    raw["dnsConfig"]["takeoverSystemDns"] = true.into();
    rt.config.save_full(&raw).unwrap();
    let running = rt.config.current().unwrap();
    let generation = rt.bump_generation();
    *rt.status.write().unwrap() = super::super::ProxyStatus {
        running: true,
        main_generation: generation,
        ..Default::default()
    };
    *rt.current_config.write().unwrap() = Some(running.clone());
    *rt.startup_snapshot.write().unwrap() = Some(running.clone());
    let mut candidate = running.clone();
    candidate["dnsConfig"]["takeoverSystemDns"] = false.into();
    rt.config.save_full(&candidate).unwrap();
    let candidate = rt.config.current().unwrap();
    (rt, dir, running, candidate)
}

#[tokio::test]
async fn host_dns_consent_and_failed_admission_do_not_accept_the_saved_host_effect() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let generation = rt.gate.generation();
    let intent = rt.register_selector_intent();
    let host_plan = host_effects::HostEffectPlan {
        effect_digest: "dns-effect".into(),
        physical_facts_digest: "dns-facts".into(),
        host_epoch: 0,
        effects: vec![host_effects::HostEffectPreview {
            kind: host_effects::HostEffectKind::Dns,
            scope_digest: "dns-scope".into(),
            before_digest: "before".into(),
            target_digest: "target".into(),
        }],
        strategy: host_effects::HostEffectStrategy::NeedsSnapshotBeforeCore,
        recipe: Value::Null,
    };
    let result = rt.host_effect_restart_choice(&candidate, &host_plan, generation, intent, None);
    assert_eq!(result.status, "restartRequired");
    assert!(result.restart_confirmation.is_some());
    assert_eq!(result.reasons[0].code, "hostEffectsRequireRestart");
    assert_eq!(result.reasons[0].path, "dnsConfig.takeoverSystemDns");
    // Declining confirmation sends no operation: D and its debt remain, R stays old.
    assert_eq!(rt.current_config_snapshot(), Some(running.clone()));
    assert_eq!(rt.config.current().unwrap(), candidate);
    assert!(rt.pending_changes().pending_apply);
    assert!(rt.pending_force_restart.read().unwrap().is_none());

    let rejected = rt
        .apply_pending_transaction(Some("wrong-token".into()))
        .await;
    assert_eq!(rejected.status, "conflict");
    assert_eq!(rt.current_config_snapshot(), Some(running.clone()));
    assert!(rt.pending_changes().pending_apply);
    // No admitted runtime baseline: the real Apply entry fails before any RPC/spawn.
    let failed = rt.apply_pending_transaction(None).await;
    assert_eq!(failed.status, "unconfirmed");
    assert_eq!(rt.current_config_snapshot(), Some(running));
    assert_eq!(rt.config.current().unwrap(), candidate);
    assert!(rt.pending_changes().pending_apply);
    assert!(rt.pending_force_restart.read().unwrap().is_none());
}

fn prepare_log_edit(rt: &ProxyRuntime) -> PreparedConfigEdit {
    let (_, edit) = rt
        .config
        .prepare_config_edit(|candidate| {
            candidate["logLevel"] = json!("debug");
            crate::runtime::config::Decision::Write(())
        })
        .unwrap();
    edit.unwrap()
}

#[tokio::test]
async fn hot_config_edit_without_pairing_never_writes_or_publishes_the_candidate() {
    let (rt, _dir, running, prior_saved) = host_dns_restart_fixture();
    let prior_owner = StagedSaveOwner::capture(&rt, rt.config.saved_revision());
    let prior_choice = rt.restart_choice(
        &prior_saved,
        "hostEffectsRequireRestart",
        vec![],
        prior_owner.generation,
        prior_owner.intent,
    );
    assert_eq!(prior_choice.status, "restartRequired");
    rt.config.save_full(&running).unwrap();
    let baseline = ts_pending_raw_fixture(&rt, running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
        &baseline.typed,
        &baseline.generated,
        &baseline.deps,
    ));
    rt.config.set_staged_pending(true);
    let edit = prepare_log_edit(&rt);
    let owner = StagedSaveOwner::capture(&rt, edit.revision);
    assert!(rt.runtime_config_apply.lock().unwrap().restart.is_some());
    assert!(rt.hot_config_edit_blocker(&edit.previous, owner).is_none());
    assert!(matches!(
        rt.hot_config_edit_blocker(&edit.previous, prior_owner),
        Some(HotConfigEditOutcome::BlockedByPending),
    ));
    assert_ne!(edit.candidate, edit.previous);
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Retryable,
    ));
    assert_eq!(rt.config.saved_revision(), edit.revision);
    assert_eq!(rt.config.current().unwrap(), running);
    assert_eq!(rt.current_config_snapshot(), Some(running.clone()));
    assert!(!rt.pending_changes().pending_apply);
    assert!(rt.runtime_config_apply.lock().unwrap().last.is_none());
    assert!(rt.pending_force_restart.read().unwrap().is_none());

    let mut changed = running.clone();
    changed["logLevel"] = json!("warn");
    rt.config.save_full(&changed).unwrap();
    rt.config.save_full(&running).unwrap();
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Conflict,
    ));
    assert_eq!(rt.config.current().unwrap(), running);
}

#[tokio::test]
async fn hot_config_edit_while_stopped_saves_for_next_start_without_a_runtime_receipt() {
    let (rt, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    rt.config.save_full(&runnable_config_fixture()).unwrap();
    let edit = prepare_log_edit(&rt);
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Saved {
            runtime_apply: None,
            ..
        },
    ));
    assert_eq!(rt.config.current().unwrap(), edit.candidate);
    assert_eq!(rt.current_config_snapshot(), None);
    assert!(!rt.pending_changes().pending_apply);
}

#[tokio::test]
async fn hot_config_edit_while_stopped_consumes_deferred_deletions() {
    let (rt, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let mut raw = runnable_config_fixture();
    raw["servers"] = json!([{
        "id": "warp-1", "name": "WARP", "protocol": "wireguard",
        "address": "engage.cloudflareclient.com", "port": 2408,
        "wireguardSettings": {
            "privateKey": "private", "localAddress": ["172.16.0.2/32"],
            "peerPublicKey": "peer", "allowedIPs": ["0.0.0.0/0"],
            "warpDevice": { "deviceId": "dev-1", "token": "tok-1" }
        }
    }]);
    rt.config.save_full(&raw).unwrap();
    let (_, edit) = rt
        .config
        .prepare_config_edit(|candidate| {
            candidate["servers"] = json!([]);
            crate::runtime::config::Decision::Write(())
        })
        .unwrap();
    assert!(matches!(
        rt.apply_hot_config_edit(&edit.unwrap()).await.unwrap(),
        HotConfigEditOutcome::Saved {
            saved_offline: true,
            ..
        },
    ));
    assert!(!rt
        .config
        .dir()
        .join("pending-config-deletions.json")
        .exists());
    let queue = std::fs::read_to_string(rt.mesh.warp_queue_path()).unwrap();
    assert!(queue.contains("dev-1"));
}

#[tokio::test]
async fn hot_config_edit_retries_only_before_local_apply_admission() {
    let (rt, _dir, _, saved) = host_dns_restart_fixture();
    let expires = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 5000) as i64;
    let owner = StagedSaveOwner::capture(&rt, rt.config.saved_revision());
    let (held, holding) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = {
        let rt = Arc::clone(&rt);
        tokio::task::spawn_blocking(move || {
            rt.config
                .update_with_cleanup_revision(false, |_| {
                    held.send(()).unwrap();
                    released.recv().unwrap();
                    crate::runtime::config::Decision::Skip(())
                })
                .unwrap();
        })
    };
    holding.await.unwrap();
    assert_eq!(
        rt.config.try_with_saved_commit_classified(|_| ()),
        Err(SavedCommitError::WriterBusy),
    );
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let attempt = rt.try_runtime_saved_commit(Some(owner), Some(expires), |config| {
        calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(config, &saved);
        Ok(true)
    });
    let release = async move {
        tokio::time::sleep(HOT_EDIT_WRITER_RETRY_INTERVAL * 2).await;
        release.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(attempt, release);
    writer.await.unwrap();
    assert_eq!(result, Some(true));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_none());

    let expired = rt
        .try_runtime_saved_commit(Some(owner), Some(0), |_| {
            panic!("expired ticket cannot claim local admission")
        })
        .await;
    assert_eq!(expired, None::<()>);
    rt.register_selector_intent();
    let superseded = rt
        .try_runtime_saved_commit(Some(owner), Some(expires), |_| {
            panic!("superseded ticket cannot claim local admission")
        })
        .await;
    assert_eq!(superseded, None::<()>);
}

#[tokio::test]
async fn hot_config_edit_retries_a_contended_publication_claim_for_its_exact_owner() {
    use crate::runtime::config::Decision;
    use polaris_config_engine::user_config::dns_constants::BLOCK_SERVER_ID;

    let (rt, _dir, running, _) = host_dns_restart_fixture();
    let generation = rt.automatic_selection_generation();
    let mut selection = rt
        .config
        .update(|disk| {
            disk["selectedServerId"] = json!(DIRECT_SERVER_ID);
            let intent = rt.register_selector_intent();
            Decision::Write(
                rt.own_automatic_selection(DIRECT_SERVER_ID, intent, generation)
                    .unwrap(),
            )
        })
        .unwrap()
        .0;
    selection.mark_committed();
    let saved = rt.config.current().unwrap();
    let mut pending = ts_pending_raw_fixture(&rt, saved);
    pending.saved_revision = Some(rt.config.saved_revision());
    let owner = pending.staged_save_owner().unwrap();
    let expiry = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 5000) as i64;
    let retry_owner =
        rt.automatic_admission_owner(Some(owner), &pending.desired, &pending.candidate);
    let mut other_target = pending.candidate.clone();
    other_target["selectedServerId"] = json!(BLOCK_SERVER_ID);
    assert!(rt
        .automatic_admission_owner(Some(owner), &pending.desired, &other_target)
        .is_none());
    for plan_expiry in [Some(expiry), None] {
        let publication = rt.gate.lock_generation_publication();
        let admitted = std::cell::Cell::new(0);
        assert_eq!(
            rt.with_runtime_candidate_claim(true, owner.generation, owner.intent, || ()),
            Err(RuntimeClaimError::Contended)
        );
        let attempt = rt.try_runtime_saved_commit(retry_owner, plan_expiry, |_| {
            rt.with_runtime_candidate_claim(true, owner.generation, owner.intent, || {
                admitted.set(admitted.get() + 1);
            })
        });
        let release = async {
            tokio::time::sleep(HOT_EDIT_WRITER_RETRY_INTERVAL * 2).await;
            assert_eq!(admitted.get(), 0);
            drop(publication);
        };
        assert_eq!(tokio::join!(attempt, release).0, Some(()));
        assert_eq!(admitted.get(), 1);
    }

    // Exercise the R publisher used after a verified Core commit. The Plan
    // ticket can expire now, because no Core Apply is sent by this retry.
    pending.plan.expires_at_unix_milli = 0;
    rt.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    let receipt = committed_ts_receipt(&pending);
    let publication = rt.gate.lock_generation_publication();
    let attempt = rt.publish_committed_runtime_candidate(&pending, &receipt);
    let release = async {
        tokio::time::sleep(HOT_EDIT_WRITER_RETRY_INTERVAL * 2).await;
        assert_eq!(rt.current_config_snapshot().as_ref(), Some(&running));
        drop(publication);
    };
    assert!(tokio::join!(attempt, release).0.is_some());
    assert_eq!(
        rt.current_config_snapshot().as_ref(),
        Some(&pending.candidate)
    );

    // A selector change during contention is terminal for the old owner.
    let publication = rt.gate.lock_generation_publication();
    let attempt = rt.try_runtime_saved_commit(retry_owner, Some(expiry), |_| {
        rt.with_runtime_candidate_claim(true, owner.generation, owner.intent, || {
            panic!("superseded work must not enter its saved authority")
        })
    });
    let supersede = async {
        tokio::time::sleep(HOT_EDIT_WRITER_RETRY_INTERVAL * 2).await;
        rt.register_selector_intent();
        drop(publication);
    };
    assert_eq!(tokio::join!(attempt, supersede).0, None::<()>);
}

#[test]
fn hot_config_edit_pending_projection_does_not_consume_independent_or_failed_work() {
    let (rt, _dir, running, _) = host_dns_restart_fixture();
    rt.config.save_full(&running).unwrap();
    let baseline = ts_pending_raw_fixture(&rt, running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
        &baseline.typed,
        &baseline.generated,
        &baseline.deps,
    ));
    let edit = prepare_log_edit(&rt);
    let mut executing = AutomaticConfigEdit {
        runtime: &rt,
        owner: None,
    };
    let owner = StagedSaveOwner::capture(&rt, edit.revision);
    let revision = rt
        .config
        .commit_prepared_config_edit(
            &edit,
            || true,
            |revision| executing.publish(StagedSaveOwner { revision, ..owner }),
        )
        .unwrap()
        .unwrap();
    assert_eq!(rt.config.current().unwrap(), edit.candidate);
    let mut pending = ts_pending_raw_fixture(&rt, edit.candidate.clone());
    pending.saved_revision = Some(revision);
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending);
    assert!(!rt.pending_changes().pending_apply);
    assert!(!rt.staged_save_recovery_pending());

    // Once dispatch returns without definitive success, its exact debt is real.
    drop(executing);
    assert!(rt.pending_changes().pending_apply);
    assert!(rt.staged_save_recovery_pending());

    // Equal bytes from a later writer cannot inherit the executing owner.
    let mut executing = AutomaticConfigEdit {
        runtime: &rt,
        owner: None,
    };
    executing.publish(StagedSaveOwner { revision, ..owner });
    rt.config.save_full(&running).unwrap();
    rt.config.save_full(&edit.candidate).unwrap();
    assert!(rt.pending_changes().pending_apply);
    assert!(rt.staged_save_recovery_pending());
}

#[test]
fn selected_graph_apply_only_reports_recovery_after_its_exact_owner_releases() {
    use crate::runtime::config::Decision;
    use polaris_config_engine::user_config::dns_constants::BLOCK_SERVER_ID;

    for prior_debt in [false, true] {
        let (rt, _dir, mut running, _) = host_dns_restart_fixture();
        running["selectedServerId"] = json!(BLOCK_SERVER_ID);
        *rt.current_config.write().unwrap() = Some(running.clone());
        *rt.startup_snapshot.write().unwrap() = Some(running.clone());
        let mut saved = running.clone();
        if prior_debt {
            saved["logLevel"] = json!("debug");
        }
        rt.config.save_full(&saved).unwrap();
        let generation = rt.automatic_selection_generation();
        let mut owner = rt
            .config
            .update(|disk| {
                disk["selectedServerId"] = json!(DIRECT_SERVER_ID);
                let intent = rt.register_selector_intent();
                Decision::Write(
                    rt.own_automatic_selection(DIRECT_SERVER_ID, intent, generation)
                        .unwrap(),
                )
            })
            .unwrap()
            .0;
        owner.mark_committed();

        running["selectedServerId"] = json!(DIRECT_SERVER_ID);
        let mut pending = ts_pending_raw_fixture(&rt, running);
        pending.desired = rt.config.current().unwrap();
        pending.saved_revision = Some(rt.config.saved_revision());
        *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
            &pending.typed,
            &pending.generated,
            &pending.deps,
        ));
        rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
        assert!(!rt.staged_save_recovery_pending());
        let observed = rt.pending_changes();
        assert_eq!(observed.pending_apply, prior_debt);
        assert_eq!(observed.restart_deferred, prior_debt);

        let mut wrong_target = pending.clone();
        wrong_target.candidate["selectedServerId"] = json!(BLOCK_SERVER_ID);
        rt.runtime_config_apply.lock().unwrap().pending = Some(wrong_target);
        assert!(rt.staged_save_recovery_pending());
        rt.runtime_config_apply.lock().unwrap().pending = Some(pending);

        drop(owner);
        assert!(rt.staged_save_recovery_pending());
        assert!(rt.pending_changes().pending_apply);
    }
}

#[tokio::test]
async fn pending_projection_while_subscription_refresh_waits_for_core_admission() {
    use crate::runtime::config::Decision;

    let (rt, _dir, running, _) = host_dns_restart_fixture();
    rt.config.save_full(&running).unwrap();
    let baseline = ts_pending_raw_fixture(&rt, running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
        &baseline.typed,
        &baseline.generated,
        &baseline.deps,
    ));
    let (_, edit) = rt
        .config
        .prepare_config_edit(|candidate| {
            candidate["servers"].as_array_mut().unwrap().push(json!({
                "id": "refreshed-node",
                "name": "Refreshed node",
                "protocol": "socks",
                "address": "192.0.2.18",
                "port": 1080
            }));
            Decision::Write(())
        })
        .unwrap();
    let edit = edit.unwrap();
    assert!(edit.candidate["servers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["id"] == "refreshed-node"));
    let admission = rt.switch_serial.lock().await;
    let mut prospective = Box::pin(rt.apply_hot_config_edit(&edit));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(prospective.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(rt.config.current().unwrap(), running);
    assert!(!rt.pending_changes().pending_apply);
    drop(prospective);

    // The old post-save path exposed this diff while waiting on the very same gate.
    assert!(rt
        .config
        .commit_prepared_config_edit(&edit, || true, |_| {})
        .unwrap()
        .is_some());
    let mut post_save = Box::pin(rt.apply_staged_save_if_current(
        edit.previous,
        edit.candidate,
        StagedSaveOwner::capture(&rt, rt.config.saved_revision()),
        |_| {},
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(post_save.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let pending = rt.pending_changes();
    assert!(pending.pending_apply);
    assert_eq!(pending.added, ["refreshed-node"]);
    drop(post_save);
    drop(admission);
}

#[tokio::test]
async fn staged_save_preserves_earlier_saved_debt_and_rejects_superseded_candidates() {
    let (rt, _dir, running, prior_saved) = host_dns_restart_fixture();
    let baseline = ts_pending_raw_fixture(&rt, running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
        &baseline.typed,
        &baseline.generated,
        &baseline.deps,
    ));
    let mut next = prior_saved.clone();
    next["logLevel"] = json!("debug");
    rt.config.save_full(&next).unwrap();
    let next = rt.config.current().unwrap();
    let owner = StagedSaveOwner {
        generation: rt.core_generation(),
        intent: rt.register_selector_intent(),
        revision: rt.config.saved_revision(),
    };
    let mut projected = false;
    let result = rt
        .apply_staged_save_if_current(prior_saved.clone(), next.clone(), owner, |_| {
            projected = true
        })
        .await;
    assert!(
        projected,
        "current App projections remain independent of runtime debt"
    );
    assert!(result.is_none());
    assert!(
        rt.runtime_config_apply.lock().unwrap().last.is_none(),
        "older debt must stop before Core Plan"
    );
    assert_eq!(rt.current_config_snapshot(), Some(running.clone()));
    assert_eq!(rt.config.current().unwrap(), next);
    assert!(rt.pending_changes().pending_apply);

    let late = rt
        .apply_staged_save_if_current(running, prior_saved, owner, |_| {
            panic!("stale saved candidate cannot change App projections")
        })
        .await;
    assert!(late.is_none());
    assert!(rt.pending_force_restart.read().unwrap().is_none());
}

#[tokio::test]
async fn staged_save_preserves_earlier_rule_auto_target_swaps() {
    for collection in ["trafficRules", "appRules"] {
        let (rt, _dir, mut running, _) = host_dns_restart_fixture();
        running["autoSelectEnabled"] = json!(true);
        running["servers"] = json!([{"id":"node","name":"node","protocol":"socks","address":"127.0.0.1","port":1080}]);
        running["proxyMode"] = json!("smart");
        running["appRoutingEnabled"] = json!(true);
        running[collection] = if collection == "trafficRules" {
            json!([
                {"id":"first","type":"domain","values":["one.example"],"action":"proxy","enabled":true,"targetAutoSelect":{"scope":"global"}},
                {"id":"second","type":"domain","values":["two.example"],"action":"proxy","enabled":true,"targetAutoSelect":{"scope":"manual"}}
            ])
        } else {
            json!([
                {"appId":"youtube","action":"proxy","enabled":true,"targetAutoSelect":{"scope":"global"}},
                {"appId":"telegram","action":"proxy","enabled":true,"targetAutoSelect":{"scope":"manual"}}
            ])
        };
        let baseline = ts_pending_raw_fixture(&rt, running.clone());
        *rt.current_config.write().unwrap() = Some(running.clone());
        *rt.startup_snapshot.write().unwrap() = Some(running.clone());
        *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
            &baseline.typed,
            &baseline.generated,
            &baseline.deps,
        ));
        assert!(!rt.pending_changes_against(Some(&running)).pending_apply);

        let mut prior_saved = running.clone();
        prior_saved[collection][0]["targetAutoSelect"]["scope"] = json!("manual");
        prior_saved[collection][1]["targetAutoSelect"]["scope"] = json!("global");
        rt.config.save_full(&prior_saved).unwrap();
        let prior_saved = rt.config.current().unwrap();
        assert!(
            rt.pending_changes().pending_apply,
            "swapping existing auto targets in {collection} is pending work"
        );
        assert!(!rt.pending_changes().restart_required);

        let mut next = prior_saved.clone();
        next["logLevel"] = json!("debug");
        rt.config.save_full(&next).unwrap();
        let next = rt.config.current().unwrap();
        let owner = StagedSaveOwner::capture(&rt, rt.config.saved_revision());
        let result = rt
            .apply_staged_save_if_current(prior_saved, next, owner, |_| {})
            .await;
        assert!(result.is_none());
        assert!(rt.runtime_config_apply.lock().unwrap().last.is_none());
        assert_eq!(rt.current_config_snapshot(), Some(running));
    }
}

#[tokio::test]
async fn staged_save_never_inherits_its_grant_from_equal_bytes_written_later() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let owner = StagedSaveOwner {
        generation: rt.core_generation(),
        intent: rt.register_selector_intent(),
        revision: rt.config.saved_revision(),
    };
    rt.config.save_full(&running).unwrap();
    rt.config.save_full(&candidate).unwrap();
    assert_eq!(rt.config.current().unwrap(), candidate);
    assert!(rt.selector_intent_is_current(owner.intent));
    assert!(rt.config.saved_revision() > owner.revision);
    let receipt = rt
        .apply_staged_save_if_current(running.clone(), candidate, owner, |_| {
            panic!("equal bytes from a later writer do not own this Save")
        })
        .await;
    assert!(receipt.is_none());
    assert_eq!(rt.current_config_snapshot(), Some(running));
}

#[tokio::test]
async fn takeover_apply_rejects_later_saved_revisions_including_equal_bytes() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let (owner, _) = rt.takeover_preview().expect("saved takeover owner");
    let mut newer = candidate.clone();
    newer["logLevel"] = json!("warn");
    for saved in [&newer, &candidate] {
        rt.config.save_full(saved).unwrap();
        let result = rt.apply_owned_takeover(owner).await;
        assert_eq!(result.status, "conflict");
        assert!(result.restart_confirmation.is_none());
        assert!(result.impact_confirmation.is_none());
        assert!(rt.runtime_config_apply.lock().unwrap().pending.is_none());
        assert_eq!(rt.current_config_snapshot().as_ref(), Some(&running));
    }
}

#[test]
fn takeover_plan_cannot_issue_choices_after_a_saved_revision_is_replaced() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let (owner, _) = rt.takeover_preview().expect("saved takeover owner");
    let mut pending = ts_pending_raw_fixture(&rt, candidate.clone());
    pending.saved_revision = Some(owner.saved_revision);
    pending.plan.expires_at_unix_milli = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        + std::time::Duration::from_secs(60))
    .as_millis() as i64;
    rt.config.save_full(&running).unwrap();
    rt.config.save_full(&candidate).unwrap();
    let restart = rt.restart_choice_at_revision(
        &candidate,
        "structuralChange",
        vec!["proxyModeType".into()],
        owner.generation,
        owner.intent,
        Some(owner.saved_revision),
    );
    assert_eq!(restart.status, "conflict");
    assert!(restart.restart_confirmation.is_none());
    let impact = rt.prepare_runtime_impact_choice(pending);
    assert_eq!(impact.status, "conflict");
    assert!(impact.impact_confirmation.is_none());
    let ledger = rt.runtime_config_apply.lock().unwrap();
    assert!(ledger.restart.is_none());
    assert!(ledger.impact.is_none());
}

#[tokio::test]
async fn staged_save_leaves_same_id_node_debt_when_verified_fingerprints_are_unavailable() {
    let (rt, _dir, mut running, _) = host_dns_restart_fixture();
    running["servers"] =
        json!([{"id":"node","name":"node","protocol":"socks","address":"127.0.0.1","port":1080}]);
    rt.config.save_full(&running).unwrap();
    let running = rt.config.current().unwrap();
    *rt.current_config.write().unwrap() = Some(running.clone());
    *rt.startup_snapshot.write().unwrap() = Some(running.clone());
    let mut older = running.clone();
    older["servers"][0]["port"] = json!(1081);
    rt.config.save_full(&older).unwrap();
    let older = rt.config.current().unwrap();
    assert!(
        !rt.pending_changes_against(Some(&older)).pending_apply,
        "display projection cannot classify a changed same-ID node without accepted fingerprints"
    );
    let mut candidate = older.clone();
    candidate["logLevel"] = json!("debug");
    rt.config.save_full(&candidate).unwrap();
    let candidate = rt.config.current().unwrap();
    let owner = StagedSaveOwner {
        generation: rt.core_generation(),
        intent: rt.register_selector_intent(),
        revision: rt.config.saved_revision(),
    };
    let result = rt
        .apply_staged_save_if_current(older, candidate, owner, |_| {})
        .await;
    assert!(result.is_none());
    assert!(
        rt.runtime_config_apply.lock().unwrap().last.is_none(),
        "missing fingerprints prevent even planning"
    );
    assert_eq!(rt.current_config_snapshot(), Some(running));
    rt.selector_reconcile.mark_required();
    assert!(rt.pending_changes().pending_apply);
}

#[tokio::test]
async fn selector_reconciliation_alone_preserves_the_save_fence_without_config_debt() {
    let (rt, _dir, running, _) = host_dns_restart_fixture();
    rt.config.save_full(&running).unwrap();
    let pending = ts_pending_raw_fixture(&rt, running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
        &pending.typed,
        &pending.generated,
        &pending.deps,
    ));
    rt.selector_reconcile.mark_required();
    let summary = rt.pending_changes();
    assert!(!summary.pending_apply);
    assert!(!summary.restart_deferred);
    assert!(rt.selector_reconcile.is_required());

    let mut candidate = running.clone();
    candidate["logLevel"] = json!("debug");
    rt.config.save_full(&candidate).unwrap();
    let candidate = rt.config.current().unwrap();
    let owner = StagedSaveOwner::capture(&rt, rt.config.saved_revision());
    assert!(rt
        .apply_staged_save_if_current(running.clone(), candidate, owner, |_| {})
        .await
        .is_none());
    assert!(
        rt.pending_changes().pending_apply,
        "the genuine saved log-level change stays visible"
    );
    assert!(rt.selector_reconcile.is_required());
    assert_eq!(rt.current_config_snapshot(), Some(running));
}

#[tokio::test]
async fn staged_save_retains_the_exact_recovery_owner_after_supersession() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let mut pending = ts_pending_raw_fixture(&rt, candidate);
    pending.saved_revision = Some(rt.config.saved_revision());
    let owner = pending.staged_save_owner().unwrap();
    let mut operation = listener_retirement_receipt(&pending, "DRAINED");
    operation.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
    operation.durable = false;
    operation.persistence_mode = "exact_configuration_file".into();
    let (rpc, server) = native_retirement_rpc_peer(&rt, &pending, operation.clone()).await;
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    rt.config.save_full(&running).unwrap();
    rt.register_selector_intent();
    let id = pending.plan.transaction_id.clone();
    let mut receipt = RuntimeApplyResult::status("hotDurabilityPending");
    receipt.transaction_id = Some(id.clone());
    receipt.runtime_generation = Some(owner.generation);
    receipt.selection_intent_generation = Some(owner.intent);
    assert!(rt.retain_staged_save_recovery_receipt(owner, &mut receipt));
    let poll = rt.runtime_apply_status(id.clone()).await;
    let retry = rt.retry_runtime_persistence(id.clone()).await;
    for result in [receipt, poll, retry] {
        assert_eq!(result.status, "unconfirmed");
        assert!(!result.ok);
        assert_eq!(result.transaction_id.as_deref(), Some(id.as_str()));
        assert_eq!(result.runtime_generation, Some(owner.generation));
        assert_eq!(result.selection_intent_generation, Some(owner.intent));
        assert_eq!(result.persistence_retryable, Some(false));
    }
    assert!(rt.pending_changes().pending_apply);
    assert!(rt.staged_save_recovery_pending());
    assert!(!rpc.requests.lock().unwrap().is_empty());
    assert_eq!(rpc.mutations.load(Ordering::SeqCst), 0);
    assert!(rpc.acknowledgements.lock().unwrap().is_empty());

    operation.state = daemon::RuntimeOperationState::Committed as i32;
    operation.durable = true;
    *rpc.status.lock().unwrap() = native_retirement_status(operation);
    let terminal = rt.runtime_apply_status(id.clone()).await;
    assert_eq!(terminal.status, "conflict");
    assert_eq!(terminal.configuration_durable, Some(true));
    assert!(!rt.staged_save_recovery_pending());
    let cached = rt.runtime_apply_status(id.clone()).await;
    assert_eq!(cached.status, "conflict");
    assert_eq!(cached.configuration_durable, Some(true));
    assert_eq!(cached.transaction_id.as_deref(), Some(id.as_str()));
    assert!(!cached.ok);
    assert_eq!(rt.retry_runtime_persistence(id).await.status, "conflict");
    assert_eq!(rt.current_config_snapshot().as_ref(), Some(&running));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn staged_save_stale_terminal_native_observer_cannot_report_success() {
    let (rt, _dir, running, candidate) = host_dns_restart_fixture();
    let mut pending = ts_pending_raw_fixture(&rt, candidate);
    pending.saved_revision = Some(rt.config.saved_revision());
    let operation = listener_retirement_receipt(&pending, "UNKNOWN");
    let (rpc, server) = native_retirement_rpc_peer(&rt, &pending, operation).await;
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    rt.config.save_full(&running).unwrap();
    let id = pending.plan.transaction_id.clone();

    for receipt in [
        rt.runtime_apply_status(id.clone()).await,
        rt.runtime_apply_status(id.clone()).await,
        rt.runtime_native_retirement_status(id).await,
    ] {
        assert_eq!(receipt.status, "conflict");
        assert_eq!(receipt.configuration_durable, Some(true));
        assert_original_native_debt(&receipt, &pending);
    }
    assert_eq!(rt.current_config_snapshot().as_ref(), Some(&running));
    assert_eq!(rpc.mutations.load(Ordering::SeqCst), 0);
    assert!(!rt.staged_save_recovery_pending());
    server.abort();
    let _ = server.await;
}

#[test]
fn stale_host_dns_restart_choices_preserve_current_state_and_debt() {
    for stale in [
        "disk",
        "disk-aba",
        "running",
        "generation",
        "intent",
        "stopped",
        "busy",
    ] {
        let (rt, _dir, running, candidate) = host_dns_restart_fixture();
        let result = rt.restart_choice(
            &candidate,
            "structuralChange",
            vec!["dnsConfig.takeoverSystemDns".into()],
            rt.gate.generation(),
            rt.register_selector_intent(),
        );
        assert_eq!(result.status, "restartRequired");
        let choice = rt
            .runtime_config_apply
            .lock()
            .unwrap()
            .restart
            .take()
            .unwrap();
        match stale {
            "disk" | "disk-aba" => {
                let mut newer = candidate.clone();
                newer["logLevel"] = "warn".into();
                rt.config.save_full(&newer).unwrap();
                if stale == "disk-aba" {
                    rt.config.save_full(&candidate).unwrap();
                    assert_eq!(rt.config.current().unwrap(), candidate);
                    assert_eq!(rt.selector_reconcile.intent_generation(), choice.intent);
                }
            }
            "running" => {
                let mut newer = running.clone();
                newer["logLevel"] = "warn".into();
                *rt.current_config.write().unwrap() = Some(newer);
            }
            "generation" => {
                rt.bump_generation();
            }
            "intent" => {
                rt.register_selector_intent();
            }
            "stopped" => {
                rt.status.write().unwrap().running = false;
            }
            _ => {
                rt.gate.begin();
            }
        }
        let accepted = rt.current_config_snapshot();
        let saved = rt.config.current().unwrap();
        let result = rt.commit_confirmed_runtime_restart(choice);
        assert_eq!(result.status, "conflict", "{stale}");
        assert_eq!(rt.current_config_snapshot(), accepted, "{stale}");
        assert_eq!(rt.config.current().unwrap(), saved, "{stale}");
        assert!(rt.pending_changes().pending_apply, "{stale}");
        assert!(
            rt.pending_force_restart.read().unwrap().is_none(),
            "{stale}"
        );
        if stale == "busy" {
            rt.gate.end(polaris_core_supervisor::LifecycleKind::Start);
        }
    }
}
#[test]
fn stale_intent_does_not_release_generic_persistence_debt() {
    let mut operation = daemon::RuntimeConfigOperation {
        state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32,
        durable: false,
        ..Default::default()
    };
    assert!(!stale_receipt_releases_pending(&operation, false));
    // Only the separate verified-newer-D check may authorize correction.
    assert!(stale_receipt_releases_pending(&operation, true));
    operation.durable = true;
    assert!(!stale_receipt_releases_pending(&operation, false));
    operation.state = daemon::RuntimeOperationState::Committed as i32;
    assert!(stale_receipt_releases_pending(&operation, false));
}

#[test]
fn completed_poll_can_satisfy_only_the_same_current_persistence_retry() {
    let mut result = RuntimeApplyResult::status("hotApplied");
    result.transaction_id = Some("original".into());
    result.runtime_generation = Some(10);
    result.selection_intent_generation = Some(7);
    assert!(terminal_persistence_receipt(Some(&result), "original", 10, 7).is_some());
    assert!(terminal_persistence_receipt(Some(&result), "other", 10, 7).is_none());
    assert!(terminal_persistence_receipt(Some(&result), "original", 11, 7).is_none());
    assert!(terminal_persistence_receipt(Some(&result), "original", 10, 8).is_none());
    result.status = "hotDurabilityPending";
    assert!(terminal_persistence_receipt(Some(&result), "original", 10, 7).is_none());
    assert!(terminal_persistence_receipt(None, "original", 10, 7).is_none());
}

#[test]
fn persistence_retry_is_bound_to_the_original_graph_control_and_mode() {
    let before = daemon::RuntimeIdentity {
        incarnation: "one".into(),
        applied_revision: 3,
        effective_digest: "before".into(),
    };
    let active = daemon::RuntimeIdentity {
        incarnation: "one".into(),
        applied_revision: 4,
        effective_digest: "after".into(),
    };
    let plan = daemon::RuntimeConfigPlan {
        transaction_id: "original".into(),
        expected: Some(before.clone()),
        candidate_digest: "after".into(),
        impact_digest: "impact".into(),
        expected_control_revision: 7,
        ..Default::default()
    };
    let operation = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: "original".into(),
        before: Some(before),
        active: Some(active.clone()),
        candidate_digest: "after".into(),
        impact_digest: "impact".into(),
        control_revision: 8,
        state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32,
        persistence_mode: "exact_configuration_file".into(),
        ..Default::default()
    };
    let status = daemon::RuntimeConfigStatus {
        contract_version: 1,
        lookup_state: daemon::RuntimeLookupState::Found as i32,
        active: Some(active),
        control_revision: 8,
        operation: Some(operation),
        ..Default::default()
    };
    let request = persistence_retry_request(&plan, &status).unwrap();
    assert_eq!(request.transaction_id, "original");
    assert_eq!(request.control_revision, 8);
    assert_eq!(request.persistence_mode, "exact_configuration_file");
    let mut changed = status.clone();
    changed.control_revision += 1;
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut changed = status.clone();
    changed.active.as_mut().unwrap().applied_revision += 1;
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut changed = status.clone();
    changed.operation.as_mut().unwrap().transaction_id = "newer".into();
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut changed = status.clone();
    changed.operation.as_mut().unwrap().candidate_digest = "different".into();
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut changed = status.clone();
    changed.operation.as_mut().unwrap().persistence_mode = "unknown".into();
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut changed = status.clone();
    changed.lookup_state = daemon::RuntimeLookupState::UnknownOrExpired as i32;
    assert!(persistence_retry_request(&plan, &changed).is_none());
    let mut noop = plan.clone();
    noop.no_op = true;
    assert!(persistence_retry_request(&noop, &status).is_none());
    let mut regeneration = status;
    regeneration.operation.as_mut().unwrap().persistence_mode =
        "app_authoritative_regeneration".into();
    assert_eq!(
        persistence_retry_request(&plan, &regeneration)
            .unwrap()
            .persistence_mode,
        "app_authoritative_regeneration"
    );
}

#[test]
fn identity_needs_all_three_axes() {
    let a = daemon::RuntimeIdentity {
        incarnation: "one".into(),
        applied_revision: 3,
        effective_digest: "digest".into(),
    };
    assert!(identity_equal(Some(&a), Some(&a)));
    let mut b = a.clone();
    b.applied_revision += 1;
    assert!(!identity_equal(Some(&a), Some(&b)));
    assert!(!identity_equal(None, None));
}

// New-install defaults are valid saved data, but a running graph needs an
// explicit exit. Construct the complete candidate before saving/typing/building
// it; the raw pending helper must never repair only a generated clone.
pub(super) fn runnable_config_fixture() -> Value {
    let mut raw = polaris_store::default_config();
    raw["selectedServerId"] = json!(DIRECT_SERVER_ID);
    raw
}

fn ts_pending_fixture(rt: &Arc<ProxyRuntime>, authority: &str) -> PendingOperation {
    let mut raw = runnable_config_fixture();
    raw["servers"] = json!([{"id":"ts","name":"ts","protocol":"tailscale","tailscaleSettings":{"controlUrl":authority}}]);
    raw["proxyModeType"] = json!("manual");
    ts_pending_raw_fixture(rt, raw)
}

pub(super) fn ts_pending_raw_fixture(rt: &Arc<ProxyRuntime>, raw: Value) -> PendingOperation {
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated =
        polaris_config_engine::builder::generate_sing_box_config(&typed, &BTreeMap::new(), &deps)
            .unwrap();
    let pairing = observe_runtime_pairing(&paired_capabilities()).unwrap();
    let generation = rt.gate.generation();
    let mesh_evidence = rt.prepare_mesh_route_run_with_receipt(
        vec![],
        0,
        false,
        None,
        &[],
        &typed,
        &generated,
        &rt.runtime_config_path(),
        "{}",
        generation,
        true,
    );
    let plan = daemon::RuntimeConfigPlan {
        contract_version: 1,
        transaction_id: "ts-operation".into(),
        // Identity of the actual generated fixture bytes, shared by Plan,
        // committed receipt and RPC capabilities; not a Core admission claim.
        candidate_digest: polaris_updater::sha256_hex(&serde_json::to_vec(&generated).unwrap()),
        impact_digest: "impact".into(),
        expected: paired_capabilities().active,
        expected_control_revision: 7,
        ..Default::default()
    };
    let tailnet_bindings = rt.tailnet_bindings(&raw, &generated);
    let targets = Arc::new(super::super::prerequisite::final_targets(
        &typed,
        &serde_json::to_value(&generated).unwrap(),
    ));
    *rt.status.write().unwrap() = crate::runtime::proxy::ProxyStatus {
        running: true,
        main_generation: generation,
        ..Default::default()
    };
    PendingOperation {
        host_plan: None,
        api_handoff: None,
        pairing,
        desired: raw.clone(),
        saved_revision: None,
        deletion_scope: vec![],
        physical_route_config: typed.clone(),
        protected_physical_root: None,
        protected_binding_plan: None,
        #[cfg(target_os = "android")]
        android_runtime_writer: None,
        generation,
        intent: rt.selector_reconcile.intent_generation(),
        candidate: raw,
        typed,
        generated,
        targets,
        deps,
        plan,
        network_canary: None,
        published: false,
        binding_plan: Default::default(),
        mesh_evidence,
        tailnet_suspended_token: None,
        tailnet_bindings,
    }
}

#[test]
fn incoming_plan_effects_require_known_scope_and_consistent_explicit_policy() {
    let mut plan = daemon::RuntimeConfigPlan {
        applicable: true,
        client_impact_policy_version: CLIENT_IMPACT_POLICY_VERSION,
        impact_confirmation_required: true,
        impacts: vec![daemon::RuntimeImpact {
            update_class: 2,
            reconnect_required: true,
            affected_session_classes: vec![
                "existing TUN TCP flows".into(),
                "existing TUN UDP associations".into(),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(runtime_plan_impacts_supported(&plan));
    assert!(runtime_plan_needs_local_confirmation(&plan));
    let mut bad = plan.clone();
    bad.impacts[0].update_class = 999;
    assert!(!runtime_plan_impacts_supported(&bad));
    bad = plan.clone();
    bad.impact_confirmation_required = false;
    assert!(!runtime_plan_impacts_supported(&bad));
    bad.impacts[0].reconnect_required = false;
    assert!(
        !runtime_plan_impacts_supported(&bad),
        "known session resets cannot be disguised by clearing both producer flags"
    );
    bad.impacts[0].affected_session_classes.clear();
    assert!(
        !runtime_plan_impacts_supported(&bad),
        "an unscoped effect does not authorize a live Apply"
    );
    bad = plan.clone();
    bad.impacts[0].affected_session_classes = vec!["new admissions".into()];
    assert!(!runtime_plan_impacts_supported(&bad));
    bad = plan.clone();
    bad.impacts[0]
        .affected_session_classes
        .push("future-reset-v2".into());
    assert!(!runtime_plan_impacts_supported(&bad));
    bad = plan.clone();
    bad.no_op = true;
    assert!(!runtime_plan_impacts_supported(&bad));
    // Core returns rejected mixed plans before issuing a confirmation bit.
    // Preserve their precise restart path instead of treating rejection as consent.
    let mut restart = plan.clone();
    restart.applicable = false;
    restart.impact_confirmation_required = false;
    restart.impacts.push(daemon::RuntimeImpact {
        update_class: daemon::RuntimeUpdateClass::RestartRequired as i32,
        effective_path: "dns.servers".into(),
        reconnect_required: true,
        ..Default::default()
    });
    assert!(runtime_plan_impacts_supported(&restart));
    assert!(!runtime_plan_needs_local_confirmation(&restart));
    restart.impact_confirmation_required = true;
    assert!(!runtime_plan_impacts_supported(&restart));
    // This is a recognized rejected reply vocabulary, never an applicable
    // authorization. Check both negotiated and legacy producer contracts.
    for policy in [0, CLIENT_IMPACT_POLICY_VERSION] {
        let mut rejected_only = daemon::RuntimeConfigPlan {
            applicable: true,
            client_impact_policy_version: policy,
            impacts: vec![daemon::RuntimeImpact {
                update_class: daemon::RuntimeUpdateClass::LocalReplace as i32,
                effective_path: "outbounds".into(),
                affected_session_classes: vec!["none; candidate not applied".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!runtime_plan_impacts_supported(&rejected_only));
        rejected_only.applicable = false;
        rejected_only.impacts[0].update_class = daemon::RuntimeUpdateClass::RestartRequired as i32;
        assert!(runtime_plan_impacts_supported(&rejected_only));
        assert!(!runtime_plan_needs_local_confirmation(&rejected_only));
    }
    // Legacy producers do not promise the v1 confirmation bit. The new client
    // still binds its own explicit choice to a known disruptive legacy plan.
    plan.client_impact_policy_version = 0;
    plan.impact_confirmation_required = false;
    assert!(runtime_plan_impacts_supported(&plan));
    assert!(runtime_plan_needs_local_confirmation(&plan));
    let noop = daemon::RuntimeConfigPlan {
        no_op: true,
        applicable: true,
        client_impact_policy_version: CLIENT_IMPACT_POLICY_VERSION,
        ..Default::default()
    };
    assert!(runtime_plan_impacts_supported(&noop));
    assert!(!runtime_plan_needs_local_confirmation(&noop));
}

#[test]
fn verified_kernel_receipt_survives_unrelated_last_request_and_keeps_its_owner() {
    let mut state = RuntimeConfigApplyState::default();
    let active = daemon::RuntimeIdentity {
        incarnation: "kernel".into(),
        applied_revision: 2,
        effective_digest: "e".repeat(64),
    };
    state.kernel = Some(RuntimeKernelReceipt {
        generation: 7,
        intent: 11,
        publication: RuntimeReceiptPublication::Current,
        operation: daemon::RuntimeConfigOperation {
            transaction_id: "original".into(),
            active: Some(active.clone()),
            state: daemon::RuntimeOperationState::Committed as i32,
            durable: true,
            ..Default::default()
        },
    });
    let mut unrelated = RuntimeApplyResult::status("conflict");
    unrelated.transaction_id = Some("original".into());
    unrelated.runtime_generation = Some(7);
    unrelated.selection_intent_generation = Some(12);
    state.last = Some(unrelated);
    let receipt = state.core_receipt_for("original", 7).unwrap();
    assert_eq!(receipt.status, "hotApplied");
    assert_eq!(receipt.configuration_durable, Some(true));
    assert_eq!(receipt.selection_intent_generation, Some(11));
    assert!(state.core_receipt_for("different", 7).is_none());
    assert!(state.core_receipt_for("original", 8).is_none());
    let mut plan = daemon::RuntimeConfigPlan {
        no_op: true,
        expected: Some(active),
        ..Default::default()
    };
    assert!(!state.requires_regeneration_owner_restart(7, &plan));
    state.kernel.as_mut().unwrap().publication = RuntimeReceiptPublication::RegenerationSuperseded;
    assert!(state.requires_regeneration_owner_restart(7, &plan));
    assert_eq!(
        state.core_receipt_for("original", 7).unwrap().status,
        "conflict"
    );
    assert!(!state.requires_regeneration_owner_restart(8, &plan));
    plan.expected.as_mut().unwrap().applied_revision += 1;
    assert!(!state.requires_regeneration_owner_restart(7, &plan));
    plan.expected.as_mut().unwrap().applied_revision -= 1;
    plan.no_op = false;
    assert!(!state.requires_regeneration_owner_restart(7, &plan));
}

#[tokio::test]
async fn stale_intent_keeps_verified_core_fact_without_publishing_or_rebinding() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let running = json!({"oldRunningSnapshot": true});
    *runtime.current_config.write().unwrap() = Some(running.clone());
    runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    let mut active = pending.plan.expected.clone().unwrap();
    active.applied_revision += 1;
    active.effective_digest = pending.plan.candidate_digest.clone();
    let operation = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: Some(active),
        control_revision: pending.plan.expected_control_revision,
        state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32,
        durable: false,
        persistence_mode: "exact_configuration_file".into(),
        ..Default::default()
    };
    assert_ne!(runtime.selector_reconcile.register_intent(), pending.intent);
    let result = runtime.settle_runtime_operation(&pending, operation).await;
    assert_eq!(result.status, "conflict");
    assert_eq!(runtime.current_config_snapshot(), Some(running));
    let state = runtime.runtime_config_apply.lock().unwrap();
    let receipt = state
        .core_receipt_for(&pending.plan.transaction_id, pending.generation)
        .unwrap();
    assert_eq!(receipt.selection_intent_generation, Some(pending.intent));
    assert_eq!(receipt.configuration_durable, Some(false));
    assert_eq!(
        state.pending.as_ref().unwrap().plan.transaction_id,
        pending.plan.transaction_id
    );
}

#[test]
fn host_only_publication_preserves_the_original_kernel_persistence_owner() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut original = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    original.published = true;
    original.plan.transaction_id = "original-kernel-transaction".into();
    *runtime.current_config.write().unwrap() = Some(original.candidate.clone());
    runtime.capture_runtime_config_baseline(
        original.generation,
        original.deps.clone(),
        original.generated.clone(),
        original.typed.clone(),
        original.pairing.clone(),
    );
    let mut active = original.plan.expected.clone().unwrap();
    active.applied_revision += 1;
    active.effective_digest = "e".repeat(64);
    let operation = daemon::RuntimeConfigOperation {
        transaction_id: original.plan.transaction_id.clone(),
        active: Some(active.clone()),
        control_revision: 9,
        durable: false,
        state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32,
        ..Default::default()
    };
    let mut retry = original.clone();
    retry.plan.no_op = true;
    retry.plan.transaction_id = "unused-noop-plan".into();
    retry.plan.expected = Some(active.clone());
    retry.plan.candidate_digest = active.effective_digest.clone();
    retry.plan.expected_control_revision = 9;
    retry.plan.expires_at_unix_milli = i64::MAX;
    retry.host_plan = Some(host_effects::HostEffectPlan {
        effect_digest: "effect".into(),
        physical_facts_digest: "physical".into(),
        host_epoch: 2,
        effects: vec![host_effects::HostEffectPreview {
            kind: host_effects::HostEffectKind::Dns,
            scope_digest: "scope".into(),
            before_digest: "before".into(),
            target_digest: "after".into(),
        }],
        strategy: host_effects::HostEffectStrategy::HostOnly,
        recipe: json!({}),
    });
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        state.pending = Some(original.clone());
        state.kernel = Some(RuntimeKernelReceipt {
            generation: original.generation,
            intent: original.intent,
            operation,
            publication: RuntimeReceiptPublication::Current,
        });
        assert!(state.host_only_preserves_pending_core(&retry));
        let mut changed = retry.clone();
        changed.candidate["mixedPort"] = json!(3080);
        assert!(!state.host_only_preserves_pending_core(&changed));
        changed = retry.clone();
        changed.plan.no_op = false;
        assert!(!state.host_only_preserves_pending_core(&changed));
    }
    // Exercise the real publication reservation with a mocked already-verified
    // NoOp receipt. No RPC, persistence callback, or native worker is invoked.
    let (_, _, execution) = runtime
        .publish_prepared_runtime_candidate(&retry, None, Some(active), 9, None)
        .expect("same-candidate host admission");
    let execution = execution.expect("real independent host execution reservation");
    assert!(execution.transaction_id.is_none());
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert_eq!(
        state.pending.as_ref().unwrap().plan.transaction_id,
        "original-kernel-transaction"
    );
    assert_eq!(
        state.kernel.as_ref().unwrap().operation.transaction_id,
        "original-kernel-transaction"
    );
}

// V04/V05: exercise the actual receipt settlement, with the original paused
// token, not a separate test-only state machine. No RPC/native action runs here.
#[tokio::test]
async fn ts_apply_unknown_stays_paused_only_proven_unapplied_resumes() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let original = ts_pending_fixture(&rt, "https://one.example");
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(original.generation, original.tailnet_bindings.clone());
    }
    let old = rt.tailnet_context().unwrap();
    let mut pending = ts_pending_fixture(&rt, "https://two.example");
    pending.tailnet_suspended_token = rt.suspend_tailnet_context(&pending.tailnet_bindings);
    assert_eq!(pending.tailnet_suspended_token, Some(old.token));
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    let receipt = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: pending.plan.expected.clone(),
        control_revision: 7,
        state: daemon::RuntimeOperationState::AbortedUnapplied as i32,
        errors: [
            "PREPARATION_FAILED",
            "PREPARATION_DEADLINE_EXCEEDED",
            "PREPARATION_DEADLINE_EXCEEDED",
            "UNRECOGNIZED_CORE_DETAIL",
        ]
        .into_iter()
        .map(|code| daemon::RuntimePlanError {
            code: code.into(),
            effective_path: "raw Core detail".into(),
            message: "raw Core message".into(),
            ..Default::default()
        })
        .collect(),
        ..Default::default()
    };
    let mut aborting = receipt.clone();
    aborting.state = daemon::RuntimeOperationState::AbortingUnapplied as i32;
    let unconfirmed = rt.settle_runtime_operation(&pending, aborting).await;
    assert_eq!(unconfirmed.status, "unconfirmed");
    assert!(unconfirmed.reasons.is_empty());
    assert!(rt.tailnet_context().unwrap().suspended);
    let mut unknown = receipt.clone();
    unknown.candidate_digest = "wrong".into();
    assert_eq!(
        rt.settle_runtime_operation(&pending, unknown).await.status,
        "unconfirmed"
    );
    assert!(rt.tailnet_context().unwrap().suspended);
    let mut contradictory = receipt.clone();
    contradictory.active.as_mut().unwrap().effective_digest = "changed".into();
    assert_eq!(
        rt.settle_runtime_operation(&pending, contradictory)
            .await
            .status,
        "unconfirmed"
    );
    assert!(rt.tailnet_context().unwrap().suspended);
    let mut replaced = pending.clone();
    replaced.plan.transaction_id = "new-pending-operation".into();
    rt.runtime_config_apply.lock().unwrap().pending = Some(replaced);
    assert_eq!(
        rt.settle_runtime_operation(&pending, receipt.clone())
            .await
            .status,
        "unconfirmed"
    );
    assert!(rt.tailnet_context().unwrap().suspended);
    assert_eq!(
        rt.runtime_config_apply
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .plan
            .transaction_id,
        "new-pending-operation"
    );
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    let rejected = rt.settle_runtime_operation(&pending, receipt).await;
    assert_eq!(rejected.status, "rejected");
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_none());
    assert_eq!(
        rejected.reasons,
        ["PREPARATION_FAILED", "PREPARATION_DEADLINE_EXCEEDED"].map(|code| RuntimeApplyReason {
            code,
            path: "runtime.prepare".into(),
        })
    );
    assert!(!rt.tailnet_context().unwrap().suspended);
    assert_eq!(rt.tailnet_context().unwrap().token, old.token);
    assert!(rt
        .suspend_tailnet_context(&original.tailnet_bindings)
        .is_none());
    {
        let _publication = rt.gate.lock_generation_publication();
        assert!(!rt.activate_tailnet_context(original.generation, original.tailnet_bindings));
    }
    assert_eq!(rt.tailnet_context().unwrap().token, old.token);
}

// I1: ABORTING_UNAPPLIED keeps the original pending only until Core settles.
// The bounded background read of the same receipt releases it on
// ABORTED_UNAPPLIED, so the next edit is no longer declined by pending.
#[tokio::test]
async fn aborting_apply_settles_in_background_and_releases_pending() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&rt, runnable_config_fixture());
    rt.config.save_full(&pending.desired).unwrap();
    let mut operation = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: pending.plan.expected.clone(),
        control_revision: pending.plan.expected_control_revision,
        state: daemon::RuntimeOperationState::AbortingUnapplied as i32,
        ..Default::default()
    };
    let (rpc, server) = native_retirement_rpc_peer(&rt, &pending, operation.clone()).await;
    *rpc.apply_response.lock().unwrap() = Some(operation.clone());
    let client = rt
        .runtime_management_client(pending.generation)
        .await
        .unwrap();
    let result = rt
        .execute_prepared_runtime_candidate(client, pending.clone(), None, false, 0, false)
        .await;
    assert_eq!(result.status, "unconfirmed");
    tokio::time::timeout(Duration::from_secs(5), async {
        while rpc.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_some());
    operation.state = daemon::RuntimeOperationState::AbortedUnapplied as i32;
    *rpc.status.lock().unwrap() = native_retirement_status(operation);
    tokio::time::timeout(Duration::from_secs(10), async {
        while rt.runtime_config_apply.lock().unwrap().pending.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rpc.mutations.load(Ordering::SeqCst), 0);
    assert_eq!(
        rt.runtime_config_apply
            .lock()
            .unwrap()
            .last
            .as_ref()
            .unwrap()
            .status,
        "rejected"
    );
    server.abort();
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "hold the saved writer through the rejection receipt"
)]
async fn background_broadcast_reports_only_the_exhausted_apply_rejection() {
    let (rt, _dir, errors) = crate::runtime::proxy::tests::test_runtime_recording_errors();
    let mut running = runnable_config_fixture();
    running["proxyMode"] = json!("global");
    running["proxyModeType"] = json!("manual");
    running["clashApiSecret"] = json!("native-retirement-test");
    *rt.current_config.write().unwrap() = Some(running.clone());
    *rt.startup_snapshot.write().unwrap() = Some(running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(Default::default());
    let pending = ts_pending_raw_fixture(&rt, running.clone());
    let (rpc, server) = native_retirement_rpc_peer(
        &rt,
        &pending,
        daemon::RuntimeConfigOperation {
            active: pending.plan.expected.clone(),
            control_revision: pending.plan.expected_control_revision,
            ..Default::default()
        },
    )
    .await;
    *rpc.selected_group_member.lock().unwrap() = Some("direct".into());
    *rpc.rejected_apply_transactions.lock().unwrap() = Some(Vec::new());
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    *rpc.exhausted_apply_hold.lock().unwrap() = Some((entered, release_rx));

    let mut desired = running.clone();
    desired["logLevel"] = json!("debug");
    desired["privacyPasswordHash"] = json!("salt$hash");
    desired["builtinGeoMeta"] = json!({"geoip-cn": {"updatedAt": "2026-11-24T12:00:00Z"}});
    let mut broadcast = desired.clone();
    crate::commands::config::strip_privacy_secrets(&mut broadcast);
    desired["builtinGeoMeta"]["geoip-cn"]["updatedAt"] = json!("2026-11-24T12:00:01Z");
    rt.config.save_full(&desired).unwrap();
    let revision = rt.config.saved_revision();

    let intent = rt.register_selector_intent();
    let apply = crate::commands::config::apply_config_broadcast_core(
        &rt,
        broadcast,
        false,
        intent,
        |_| {},
        |_, outcome| {
            assert_eq!(outcome, Some(SwitchOutcome::Deferred));
        },
    );
    tokio::pin!(apply);
    tokio::select! {
        entered = entered_rx => entered.unwrap(),
        () = &mut apply => panic!("broadcast returned before the final rejection"),
    }
    // A different edit can be preparing under the writer without publishing a revision.
    {
        let _writer = rt.config.hold_write_lock_for_test();
        release.send(()).unwrap();
        apply.await;
    }
    assert_eq!(rt.config.saved_revision(), revision);
    let transactions = rpc.rejected_apply_transactions.lock().unwrap();
    assert_eq!(
        transactions.as_deref().unwrap(),
        ["transaction-1", "transaction-2", "transaction-3"],
        "last result: {:?}, gRPC readback: {:?}",
        rt.runtime_config_apply
            .lock()
            .unwrap()
            .last
            .as_ref()
            .map(|result| (result.status, result.reasons.clone())),
        rpc.probe_select_calls.lock().unwrap(),
    );
    let errors = errors.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].1, super::super::code::RUNTIME_APPLY_REJECTED);
    assert!(rt.status().running);
    assert!(rt.status().error_code.is_none());
    assert_eq!(rt.current_config_snapshot().as_ref(), Some(&running));
    assert!(rt.pending_changes().pending_apply);
    assert!(!rt.gate.pending().restart_pending);
    server.abort();
}

// Core's source refresh can publish between the capability read and Plan.
// Prospective prepare replans against the advanced identity without writing D.
#[tokio::test]
async fn hot_config_edit_replans_when_core_publishes_before_plan() {
    let (rt, _dir, _) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut running = runnable_config_fixture();
    running["proxyMode"] = json!("global");
    running["proxyModeType"] = json!("manual");
    running["clashApiSecret"] = json!("native-retirement-test");
    rt.config.save_full(&running).unwrap();
    let running = rt.config.current().unwrap();
    *rt.current_config.write().unwrap() = Some(running.clone());
    *rt.startup_snapshot.write().unwrap() = Some(running.clone());
    *rt.switch_snapshot.write().unwrap() = Some(Default::default());
    let pending = ts_pending_raw_fixture(&rt, running.clone());
    let (rpc, server) = native_retirement_rpc_peer(
        &rt,
        &pending,
        daemon::RuntimeConfigOperation {
            active: pending.plan.expected.clone(),
            control_revision: pending.plan.expected_control_revision,
            ..Default::default()
        },
    )
    .await;
    *rpc.selected_group_member.lock().unwrap() = Some("direct".into());
    *rpc.rejected_apply_transactions.lock().unwrap() = Some(Vec::new());
    let edit = prepare_log_edit(&rt);

    use polaris_singbox_grpc::tonic::Code;
    // A stale identity is replanned only within the bound; one more would succeed.
    *rpc.failed_plans.lock().unwrap() =
        vec![Code::FailedPrecondition; usize::from(RUNTIME_APPLY_REPLANS) + 1];
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Retryable,
    ));
    assert!(rpc.failed_plans.lock().unwrap().is_empty());
    assert_eq!(rt.config.saved_revision(), edit.revision);
    assert_eq!(rt.config.current().unwrap(), running);
    assert!(rpc
        .rejected_apply_transactions
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_empty());

    // An unknown Plan outcome is never replanned: a retry would succeed here.
    *rpc.failed_plans.lock().unwrap() = vec![Code::DeadlineExceeded];
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Retryable,
    ));
    assert!(rpc.failed_plans.lock().unwrap().is_empty());
    assert_eq!(rt.config.current().unwrap(), running);

    *rpc.failed_plans.lock().unwrap() = vec![Code::FailedPrecondition];
    assert!(matches!(
        rt.apply_hot_config_edit(&edit).await.unwrap(),
        HotConfigEditOutcome::Saved {
            runtime_apply: Some(_),
            ..
        },
    ));
    assert!(rpc.failed_plans.lock().unwrap().is_empty());
    assert_eq!(rt.config.current().unwrap(), edit.candidate);
    server.abort();
}

// An unconsumed plan is UNKNOWN to Core until it is applied, and Core may still
// admit it before expiry. Only an expired plan with the unchanged expected
// identity proves the transaction never published.
#[tokio::test]
async fn unknown_receipt_needs_expiry_or_exact_unapplied_rejection() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&rt, runnable_config_fixture());
    pending.plan.expires_at_unix_milli = i64::try_from(
        (SystemTime::now() + Duration::from_secs(60))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let operation = daemon::RuntimeConfigOperation {
        active: pending.plan.expected.clone(),
        control_revision: pending.plan.expected_control_revision,
        ..Default::default()
    };
    let (rpc, server) = native_retirement_rpc_peer(&rt, &pending, operation).await;
    let unknown = daemon::RuntimeConfigStatus {
        contract_version: 1,
        active: pending.plan.expected.clone(),
        lookup_state: daemon::RuntimeLookupState::UnknownOrExpired as i32,
        control_revision: pending.plan.expected_control_revision,
        ..Default::default()
    };
    *rpc.status.lock().unwrap() = unknown.clone();
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    let id = pending.plan.transaction_id.clone();
    assert_eq!(
        rt.runtime_apply_status(id.clone()).await.status,
        "unconfirmed"
    );
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_some());
    let mut advanced = pending.plan.expected.clone().unwrap();
    advanced.applied_revision += 1;
    *rpc.status.lock().unwrap() = daemon::RuntimeConfigStatus {
        contract_version: 1,
        active: Some(advanced),
        lookup_state: daemon::RuntimeLookupState::Found as i32,
        operation: Some(daemon::RuntimeConfigOperation {
            contract_version: 1,
            transaction_id: id.clone(),
            candidate_digest: pending.plan.candidate_digest.clone(),
            impact_digest: pending.plan.impact_digest.clone(),
            before: pending.plan.expected.clone(),
            active: pending.plan.expected.clone(),
            control_revision: pending.plan.expected_control_revision,
            state: daemon::RuntimeOperationState::RejectedUnapplied as i32,
            errors: vec![daemon::RuntimePlanError {
                code: PLAN_PRECONDITION_CHANGED.into(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = rt.runtime_apply_status(id.clone()).await;
    assert_eq!(result.status, "rejected");
    assert_eq!(result.reasons[0].code, PLAN_PRECONDITION_CHANGED);
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_none());
    *rpc.status.lock().unwrap() = unknown;
    let ticket_lifetime = Duration::from_millis(500);
    let expiry = i64::try_from(
        (SystemTime::now() + ticket_lifetime)
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    pending.plan.expires_at_unix_milli = expiry;
    rt.runtime_config_apply.lock().unwrap().pending = Some(pending);
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    *rpc.status_hold.lock().unwrap() = Some((entered, release_rx));
    let read_started = Instant::now();
    let read = tokio::spawn({
        let rt = Arc::clone(&rt);
        let id = id.clone();
        async move { rt.runtime_apply_status(id).await }
    });
    entered_rx.await.unwrap();
    assert!(runtime_plan_ticket_is_live(expiry));
    tokio::time::sleep(ticket_lifetime).await;
    assert!(!runtime_plan_ticket_is_live(expiry));
    release.send(()).unwrap();
    assert_eq!(read.await.unwrap().status, "unconfirmed");
    assert!(read_started.elapsed() < Duration::from_secs(2));
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_some());
    assert_eq!(rt.runtime_apply_status(id).await.status, "rejected");
    assert!(rt.runtime_config_apply.lock().unwrap().pending.is_none());
    server.abort();
}

#[tokio::test]
async fn confirmed_ts_binding_commit_replaces_token_in_same_main_generation() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let original = ts_pending_fixture(&rt, "https://one.example");
    rt.capture_runtime_config_baseline(
        original.generation,
        original.deps.clone(),
        original.generated.clone(),
        original.typed.clone(),
        original.pairing.clone(),
    );
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(original.generation, original.tailnet_bindings.clone());
    }
    let old = rt.tailnet_context().unwrap();
    let mut pending = ts_pending_fixture(&rt, "https://two.example");
    pending.tailnet_suspended_token = rt.suspend_tailnet_context(&pending.tailnet_bindings);
    let mut active = pending.plan.expected.clone().unwrap();
    active.applied_revision += 1;
    active.effective_digest = pending.plan.candidate_digest.clone();
    let receipt = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: Some(active),
        control_revision: 8,
        durable: true,
        state: daemon::RuntimeOperationState::Committed as i32,
        ..Default::default()
    };
    assert_eq!(
        rt.settle_runtime_operation(&pending, receipt).await.status,
        "hotApplied"
    );
    let current = rt.tailnet_context().unwrap();
    assert_eq!(current.generation, old.generation);
    assert_ne!(current.token, old.token);
    assert!(!current.suspended);
    let empty = daemon::TailscaleStatusUpdate { endpoints: vec![] };
    assert!(!rt.apply_ts_status_frame(&empty, &old.tag_to_id(), old.generation, old.token));
    assert!(rt.apply_ts_status_frame(
        &empty,
        &current.tag_to_id(),
        current.generation,
        current.token
    ));
}

#[test]
fn tailnet_activation_can_read_the_published_baseline() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let original = ts_pending_fixture(&rt, "https://one.example");
    rt.capture_runtime_config_baseline(
        original.generation,
        original.deps.clone(),
        original.generated.clone(),
        original.typed.clone(),
        original.pairing.clone(),
    );
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(original.generation, original.tailnet_bindings);
    }
    let old = rt.tailnet_context().unwrap();
    let pending = ts_pending_fixture(&rt, "https://two.example");
    let rule_directory = pending.deps.tailnet_rules_dir.clone();
    let generation = pending.generation;

    // Pause the real publisher after it enters its baseline write section.
    let tailnet = rt.tailnet_routes.lock().unwrap();
    let binding = rt.runtime_binding_state.lock().unwrap();
    let publisher = Arc::clone(&rt);
    let publish = std::thread::spawn(move || {
        publisher.publish_prepared_runtime_candidate(&pending, Some("new-binding"), None, 8, None)
    });
    let wait = |ready: &mut dyn FnMut() -> bool, message: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !ready() {
            assert!(std::time::Instant::now() < deadline, "{message}");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    wait(
        &mut || {
            matches!(
                rt.runtime_config_baseline.try_read(),
                Err(std::sync::TryLockError::WouldBlock)
            )
        },
        "publication did not enter the baseline write section",
    );
    drop(binding);
    // Android's preparation reads these paths under the tailnet lock. Probe
    // with try_read so regressing to a self-deadlock still fails in bounded time.
    wait(
        &mut || {
            rt.runtime_config_baseline.try_read().is_ok_and(|baseline| {
                baseline.as_ref().is_some_and(|live| {
                    live.generation == generation
                        && live.deps.tailnet_rules_dir == rule_directory
                        && live.committed_transaction.as_deref() == Some("new-binding")
                })
            })
        },
        "tailnet activation still owns the baseline writer while it needs to read",
    );
    drop(tailnet);
    publish.join().unwrap().unwrap();
    let current = rt.tailnet_context().unwrap();
    assert_eq!(current.generation, old.generation);
    assert_ne!(current.token, old.token);
}

#[tokio::test]
async fn confirmed_non_ts_graph_change_preserves_ts_token_and_known_hosts() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_fixture(&rt, "https://one.example");
    rt.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(pending.generation, pending.tailnet_bindings.clone());
    }
    let context = rt.tailnet_context().unwrap();
    let update = daemon::TailscaleStatusUpdate {
        endpoints: vec![daemon::TailscaleEndpointStatus {
            endpoint_tag: context.tag_to_id().keys().next().unwrap().clone(),
            self_: Some(daemon::TailscalePeer {
                tailscale_i_ps: vec!["32.0.0.28".into()],
                ..Default::default()
            }),
            ..Default::default()
        }],
    };
    assert!(rt.apply_ts_status_frame(
        &update,
        &context.tag_to_id(),
        context.generation,
        context.token
    ));
    let hosts = rt.observed_tailnet_snapshot();
    let mut active = pending.plan.expected.clone().unwrap();
    active.applied_revision += 1;
    active.effective_digest = pending.plan.candidate_digest.clone();
    let receipt = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: Some(active),
        control_revision: 8,
        durable: true,
        state: daemon::RuntimeOperationState::Committed as i32,
        ..Default::default()
    };
    assert_eq!(
        rt.settle_runtime_operation(&pending, receipt).await.status,
        "hotApplied"
    );
    assert_eq!(rt.tailnet_context().unwrap().token, context.token);
    assert_eq!(rt.observed_tailnet_snapshot(), hosts);
}

fn ts_identity_raw_for_projection() -> Value {
    let mut raw = runnable_config_fixture();
    raw["servers"] = json!([{"id":"ts","name":"ts","protocol":"tailscale","tailscaleSettings":{"controlUrl":"https://one.example"}}]);
    raw["proxyModeType"] = json!("manual");
    raw["meshRouteState"] = json!({"schemaVersion":1,"localId":"local","revision":"1",
        "identities":[{"serverId":"ts","identityEpoch":"epoch-one","controlAuthority":"https://one.example","selfStableId":"self-one","bindingState":"bound","evidenceSource":"status"}],
        "observations":[],"reservations":[],"intent":{"revision":"1","desiredRun":"stopped"},"identityEffects":[]});
    raw
}

// Exercise the production accepted-H projection and actual generated Plan payload
// with the previous/CAS projection left on its accepted dependency snapshot.
#[tokio::test]
async fn runtime_plan_identity_changes_filter_candidate_without_replacing_previous_or_importing_fresh(
) {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let raw = ts_identity_raw_for_projection();
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated =
        polaris_config_engine::builder::generate_sing_box_config(&typed, &BTreeMap::new(), &deps)
            .unwrap();
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(rt.gate.generation(), rt.tailnet_bindings(&raw, &generated));
    }
    let context = rt.tailnet_context().unwrap();
    let update = |address: &str| daemon::TailscaleStatusUpdate {
        endpoints: vec![daemon::TailscaleEndpointStatus {
            endpoint_tag: context.tag_to_id().keys().next().unwrap().clone(),
            self_: Some(daemon::TailscalePeer {
                tailscale_i_ps: vec![address.into()],
                ..Default::default()
            }),
            ..Default::default()
        }],
    };
    assert!(rt.apply_ts_status_frame(
        &update("32.0.0.28"),
        &context.tag_to_id(),
        context.generation,
        context.token
    ));
    let accepted = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    assert_eq!(accepted.observed_tailnet_addresses["ts"], vec!["32.0.0.28"]);
    assert!(rt.apply_ts_status_frame(
        &update("32.0.0.29"),
        &context.tag_to_id(),
        context.generation,
        context.token
    ));
    let fresh = rt.fresh_tailnet_snapshot(context.generation);
    let previous = polaris_config_engine::builder::generate_sing_box_config(
        &typed,
        &BTreeMap::new(),
        &accepted,
    )
    .unwrap();
    let previous_json = serde_json::to_value(&previous).unwrap();
    assert!(previous_json.to_string().contains("32.0.0.28/32"));
    assert!(!previous_json.to_string().contains("32.0.0.29/32"));
    for change in [
        "authority",
        "epoch",
        "stable",
        "retired",
        "same-binding",
        "selector",
        "non-ts",
    ] {
        let mut target = raw.clone();
        match change {
            "authority" => {
                target["servers"][0]["tailscaleSettings"]["controlUrl"] =
                    json!("https://two.example");
                target["meshRouteState"]["identities"][0]["controlAuthority"] =
                    json!("https://two.example");
            }
            "epoch" => {
                target["meshRouteState"]["identities"][0]["identityEpoch"] = json!("epoch-two")
            }
            "stable" => {
                target["meshRouteState"]["identities"][0]["selfStableId"] = json!("self-two")
            }
            "retired" => {
                target["meshRouteState"]["identities"][0]["bindingState"] = json!("retired")
            }
            "selector" => target["selectedServerId"] = json!("ts"),
            "non-ts" => target["logLevel"] = json!("debug"),
            _ => {}
        }
        let mut candidate_deps = accepted.clone();
        candidate_deps.observed_tailnet_addresses =
            rt.accepted_tailnet_for_config(&raw, &target, &accepted.observed_tailnet_addresses);
        let compatible = matches!(change, "same-binding" | "selector" | "non-ts");
        assert_eq!(
            candidate_deps.observed_tailnet_addresses.contains_key("ts"),
            compatible,
            "{change}"
        );
        let target: UserConfig = serde_json::from_value(target).unwrap();
        let candidate = polaris_config_engine::builder::generate_sing_box_config(
            &target,
            &BTreeMap::new(),
            &candidate_deps,
        )
        .unwrap();
        let request = daemon::PlanRuntimeConfigRequest {
            contract_version: 1,
            config_content: serde_json::to_string(&candidate).unwrap(),
            ..Default::default()
        };
        assert_eq!(
            request.config_content.contains("32.0.0.28/32"),
            compatible,
            "{change}"
        );
        assert!(!request.config_content.contains("32.0.0.29/32"));
        assert!(!tailnet_tun_projection_changed(&previous, &candidate));
        assert_eq!(
            serde_json::to_value(
                polaris_config_engine::builder::generate_sing_box_config(
                    &typed,
                    &BTreeMap::new(),
                    &accepted
                )
                .unwrap()
            )
            .unwrap(),
            previous_json
        );
        assert_eq!(rt.tailnet_context().unwrap().token, context.token);
        assert_eq!(rt.fresh_tailnet_snapshot(context.generation), fresh);
    }
    assert_eq!(accepted.observed_tailnet_addresses["ts"], vec!["32.0.0.28"]);
}

#[test]
fn filtered_tailnet_physical_tun_projection_requires_existing_restart_choice() {
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut raw = ts_identity_raw_for_projection();
    raw["proxyModeType"] = json!("tun");
    raw["tunConfig"] = json!({"inboundExcludeCidrs":["32.0.0.0/24"]});
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let mut accepted = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    accepted.platform = "darwin".into();
    accepted.observed_tailnet_addresses = BTreeMap::from([("ts".into(), vec!["32.0.0.28".into()])]);
    let previous = polaris_config_engine::builder::generate_sing_box_config(
        &typed,
        &BTreeMap::new(),
        &accepted,
    )
    .unwrap();
    let mut target = raw.clone();
    target["meshRouteState"]["identities"][0]["identityEpoch"] = json!("epoch-two");
    let mut candidate_deps = accepted.clone();
    candidate_deps.observed_tailnet_addresses =
        rt.accepted_tailnet_for_config(&raw, &target, &accepted.observed_tailnet_addresses);
    assert!(candidate_deps.observed_tailnet_addresses.is_empty());
    let typed: UserConfig = serde_json::from_value(target.clone()).unwrap();
    let candidate = polaris_config_engine::builder::generate_sing_box_config(
        &typed,
        &BTreeMap::new(),
        &candidate_deps,
    )
    .unwrap();
    assert!(tailnet_tun_projection_changed(&previous, &candidate));
    let generation = rt.bump_generation();
    let intent = rt.register_selector_intent();
    *rt.status.write().unwrap() = super::super::ProxyStatus {
        running: true,
        main_generation: generation,
        ..Default::default()
    };
    *rt.current_config.write().unwrap() = Some(raw);
    let staged = rt.classify_staged(&target);
    assert!(staged.pending_apply);
    assert!(
        !staged.restart_required,
        "preview cannot attest the physical generated TUN"
    );
    let result = rt.restart_choice(
        &target,
        "structuralChange",
        vec!["inbounds.tun.route_exclude_address".into()],
        generation,
        intent,
    );
    assert_eq!(result.status, "restartRequired");
    assert_eq!(rt.observed_tailnet_snapshot(), BTreeMap::new());
    assert!(rt.tailnet_context().is_none());
}

#[test]
fn tailnet_restart_choice_rejects_stopped_busy_and_stale_owners() {
    for state in ["stopped", "busy", "stale-generation", "stale-intent"] {
        let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let raw = ts_identity_raw_for_projection();
        let generation = rt.bump_generation();
        let intent = rt.register_selector_intent();
        *rt.current_config.write().unwrap() = Some(raw.clone());
        *rt.status.write().unwrap() = super::super::ProxyStatus {
            running: state != "stopped",
            main_generation: generation,
            ..Default::default()
        };
        if state == "busy" {
            rt.gate.begin();
        }
        let result = rt.restart_choice(
            &raw,
            "structuralChange",
            vec!["inbounds.tun.route_exclude_address".into()],
            if state == "stale-generation" {
                generation - 1
            } else {
                generation
            },
            if state == "stale-intent" {
                intent - 1
            } else {
                intent
            },
        );
        assert_eq!(result.status, "conflict", "{state}");
        assert!(result.restart_confirmation.is_none(), "{state}");
        assert_eq!(rt.current_config_snapshot(), Some(raw));
        if state == "busy" {
            rt.gate.end(polaris_core_supervisor::LifecycleKind::Start);
        }
    }
}

pub(super) fn committed_ts_receipt(pending: &PendingOperation) -> daemon::RuntimeConfigOperation {
    let mut active = pending.plan.expected.clone().unwrap();
    active.applied_revision += 1;
    active.effective_digest = pending.plan.candidate_digest.clone();
    daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: Some(active),
        control_revision: 8,
        durable: true,
        state: daemon::RuntimeOperationState::Committed as i32,
        ..Default::default()
    }
}

#[tokio::test]
async fn regeneration_ack_rejects_receipts_outside_the_original_plan() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let mut original = committed_ts_receipt(&pending);
    original.durable = false;
    original.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
    original.persistence_mode = "app_authoritative_regeneration".into();
    original.owner_reload_acknowledged = true;
    assert!(pending.matches_receipt(&original));
    assert!(pending.committed_identity(&original).is_some());
    for axis in ["contract", "impact", "control", "active"] {
        let mut operation = original.clone();
        match axis {
            "contract" => operation.contract_version = 0,
            "impact" => operation.impact_digest.push('x'),
            "control" => operation.control_revision = 0,
            "active" => operation.active.as_mut().unwrap().applied_revision = 0,
            _ => unreachable!(),
        }
        let result = runtime
            .acknowledge_regeneration_owner(&pending, operation)
            .await;
        assert_eq!(result.unwrap_err().status, "unconfirmed", "{axis}");
    }
}

// Begin with actual legacy bytes and an admitted C1, deliberately without any
// STATUS. Only production confirmed settlement may install C2 and rewrite files.
#[tokio::test]
async fn weak_legacy_held_context_filters_preview_candidate_and_confirmed_source_together() {
    use polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base;
    for change in [
        "authority",
        "epoch",
        "stable",
        "same-binding",
        "selector",
        "non-ts",
    ] {
        let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let raw = ts_identity_raw_for_projection();
        let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
        let path = rt
            .tailnet_rules_dir()
            .join(format!("{}.json", tailnet_rule_file_base("ts")));
        std::fs::create_dir_all(rt.tailnet_rules_dir()).unwrap();
        std::fs::write(
            &path,
            crate::runtime::proxy::tailnet_rules::tailnet_rule_file_json(&["32.0.0.99/32".into()]),
        )
        .unwrap();
        rt.write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
            .await
            .unwrap();
        let original = ts_pending_raw_fixture(&rt, raw.clone());
        rt.capture_runtime_config_baseline(
            original.generation,
            original.deps.clone(),
            original.generated.clone(),
            original.typed.clone(),
            original.pairing.clone(),
        );
        {
            let _publication = rt.gate.lock_generation_publication();
            rt.activate_tailnet_context(original.generation, original.tailnet_bindings.clone());
        }
        let context = rt.tailnet_context().unwrap();
        let before = std::fs::read(&path).unwrap();
        let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
        assert_eq!(rt.observed_tailnet_snapshot()["ts"], vec!["32.0.0.99"]);
        assert!(rt.fresh_tailnet_snapshot(context.generation).is_empty());
        assert!(rt.mesh.tailscale_status_snapshot(true).statuses.is_empty());
        let mut target = raw.clone();
        match change {
            "authority" => {
                target["servers"][0]["tailscaleSettings"]["controlUrl"] =
                    json!("https://two.example");
                target["meshRouteState"]["identities"][0]["controlAuthority"] =
                    json!("https://two.example");
            }
            "epoch" => {
                target["meshRouteState"]["identities"][0]["identityEpoch"] = json!("epoch-two")
            }
            "stable" => {
                target["meshRouteState"]["identities"][0]["selfStableId"] = json!("self-two")
            }
            "selector" => target["selectedServerId"] = json!("ts"),
            "non-ts" => target["logLevel"] = json!("debug"),
            _ => {}
        }
        let compatible = matches!(change, "same-binding" | "selector" | "non-ts");
        assert_eq!(
            rt.observed_tailnet_for_config(&target).contains_key("ts"),
            compatible,
            "preview {change}"
        );
        let projected = rt.accepted_tailnet_for_config(
            &raw,
            &target,
            &original.deps.observed_tailnet_addresses,
        );
        assert_eq!(
            projected.contains_key("ts"),
            compatible,
            "candidate {change}"
        );
        let mut pending = ts_pending_raw_fixture(&rt, target.clone());
        assert_eq!(pending.deps.observed_tailnet_addresses, projected);
        assert!(rt.tailnet_bindings_within_limit(&pending.tailnet_bindings));
        {
            let _publication = rt.gate.lock_generation_publication();
            pending.tailnet_suspended_token = rt.suspend_tailnet_context(&pending.tailnet_bindings);
        }
        assert_eq!(pending.tailnet_suspended_token.is_none(), compatible);
        // A suspended C1 remains negative knowledge until actual confirmation.
        assert_eq!(
            rt.observed_tailnet_for_config(&target).contains_key("ts"),
            compatible
        );
        assert_eq!(
            rt.settle_runtime_operation(&pending, committed_ts_receipt(&pending))
                .await
                .status,
            "hotApplied"
        );
        let next = rt.tailnet_context().unwrap();
        assert_eq!(next.generation, context.generation);
        assert_eq!(next.token == context.token, compatible);
        assert_eq!(
            rt.observed_tailnet_for_config(&target).contains_key("ts"),
            compatible
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content.contains("32.0.0.99/32"),
            compatible,
            "confirmed file {change}"
        );
        if compatible {
            assert_eq!(content.as_bytes(), before);
            assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
        } else {
            assert!(rt.observed_tailnet_snapshot().is_empty());
        }
        assert!(rt.fresh_tailnet_snapshot(next.generation).is_empty());
        assert!(rt.mesh.tailscale_status_snapshot(true).statuses.is_empty());
        let baseline = rt.runtime_config_baseline.read().unwrap();
        assert_eq!(
            baseline.as_ref().unwrap().deps.observed_tailnet_addresses,
            projected
        );
    }
}

#[tokio::test]
async fn weak_legacy_context_filter_applies_before_capacity_and_real_confirmed_activation() {
    use polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base;
    let (rt, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let raw = ts_identity_raw_for_projection();
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let path = rt
        .tailnet_rules_dir()
        .join(format!("{}.json", tailnet_rule_file_base("ts")));
    std::fs::create_dir_all(rt.tailnet_rules_dir()).unwrap();
    let legacy: Vec<String> = (0..35000)
        .map(|n| format!("10.{}.{}.{}/32", (n >> 16) & 255, (n >> 8) & 255, n & 255))
        .collect();
    let content = crate::runtime::proxy::tailnet_rules::tailnet_rule_file_json(&legacy);
    assert!(content.len() < crate::runtime::proxy::tailnet_rules::MAX_TAILNET_RULE_BYTES);
    std::fs::write(&path, content).unwrap();
    rt.write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
        .await
        .unwrap();
    let original = ts_pending_raw_fixture(&rt, raw.clone());
    rt.capture_runtime_config_baseline(
        original.generation,
        original.deps.clone(),
        original.generated.clone(),
        original.typed.clone(),
        original.pairing.clone(),
    );
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(original.generation, original.tailnet_bindings.clone());
    }
    let mut target = raw.clone();
    target["servers"][0]["tailscaleSettings"]["routes"] = json!((0..10000)
        .map(|n| format!("192.{}.{}.{}/32", (n >> 16) & 255, (n >> 8) & 255, n & 255))
        .collect::<Vec<_>>());
    let same_context = ts_pending_raw_fixture(&rt, target.clone());
    assert!(!rt.tailnet_bindings_within_limit(&same_context.tailnet_bindings));
    target["meshRouteState"]["identities"][0]["identityEpoch"] = json!("epoch-two");
    assert!(rt.observed_tailnet_for_config(&target).is_empty());
    let mut next = ts_pending_raw_fixture(&rt, target);
    assert!(next.deps.observed_tailnet_addresses.is_empty());
    assert!(rt.tailnet_bindings_within_limit(&next.tailnet_bindings));
    {
        let _publication = rt.gate.lock_generation_publication();
        next.tailnet_suspended_token = rt.suspend_tailnet_context(&next.tailnet_bindings);
    }
    assert_eq!(
        rt.settle_runtime_operation(&next, committed_ts_receipt(&next))
            .await
            .status,
        "hotApplied"
    );
    let content = std::fs::read_to_string(path).unwrap();
    assert!(!content.contains("10.0.0.0/32"));
    assert!(content.contains("192.0.0.0/32"));
    assert!(content.len() <= crate::runtime::proxy::tailnet_rules::MAX_TAILNET_RULE_BYTES);
    assert!(rt.observed_tailnet_snapshot().is_empty());
    assert!(rt.fresh_tailnet_snapshot(next.generation).is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn committed_receipt_with_blocked_physical_proof_keeps_kernel_fact_without_publication() {
    for expired in [true, false] {
        for durable in [true, false] {
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
            let accepted = ts_pending_fixture(&runtime, "https://accepted.example");
            *runtime.current_config.write().unwrap() = Some(accepted.candidate.clone());
            *runtime.switch_snapshot.write().unwrap() = Some(ProxyRuntime::build_switch_snapshot(
                &accepted.typed,
                &accepted.generated,
                &accepted.deps,
            ));
            runtime.capture_runtime_config_baseline(
                accepted.generation,
                accepted.deps.clone(),
                accepted.generated.clone(),
                accepted.typed.clone(),
                accepted.pairing.clone(),
            );
            let accepted_bindings = runtime.runtime_binding_state.lock().unwrap().plan.clone();
            let mut pending = ts_pending_fixture(&runtime, "https://candidate.example");
            pending.host_plan = Some(host_effects::HostEffectPlan {
                effect_digest: "candidate-effect".into(),
                physical_facts_digest: "physical".into(),
                host_epoch: 2,
                effects: vec![host_effects::HostEffectPreview {
                    kind: host_effects::HostEffectKind::Dns,
                    scope_digest: "scope".into(),
                    before_digest: "before".into(),
                    target_digest: "after".into(),
                }],
                strategy: host_effects::HostEffectStrategy::AfterCoreCommit,
                recipe: json!({}),
            });
            let epoch = runtime.network_epoch().expect("desktop watcher epoch");
            let (owner, proof) =
                crate::runtime::route_binding::blocked_physical_binding_proof_for_test(
                    pending.generation,
                    epoch,
                    expired,
                );
            pending.protected_physical_root = Some(owner);
            pending.protected_binding_plan = Some(proof);
            runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
            let mut committed = committed_ts_receipt(&pending);
            committed.durable = durable;
            committed.persistence_mode = "exact_configuration_file".into();
            committed.state = if durable {
                daemon::RuntimeOperationState::Committed as i32
            } else {
                daemon::RuntimeOperationState::CommittedDurabilityPending as i32
            };
            // Exercise production verification + original settlement. The
            // fixture is expired/revoked and has no native seal, so no OS/RPC
            // or host-effect worker can be reached by this committed receipt.
            let result = runtime
                .settle_runtime_operation_with_physical_verification(&pending, committed.clone())
                .await;
            assert_eq!(result.status, "unconfirmed");
            assert_eq!(result.core_state, Some("committed"));
            assert_eq!(result.configuration_durable, Some(durable));
            assert!(
                runtime.switch_snapshot.read().unwrap().is_none(),
                "a committed but unpublished graph cannot retain the old cheap-selector classification"
            );
            assert_eq!(
                result.persistence_mode.as_deref(),
                Some("exact_configuration_file")
            );
            assert!(result
                .reasons
                .iter()
                .any(|reason| reason.code == "PHYSICAL_ROUTE_FACTS_CHANGED"));
            assert_eq!(
                *runtime.current_config.read().unwrap(),
                Some(accepted.candidate.clone())
            );
            assert_eq!(
                runtime.runtime_binding_state.lock().unwrap().plan,
                accepted_bindings
            );
            let state = runtime.runtime_config_apply.lock().unwrap();
            let kernel = state
                .kernel
                .as_ref()
                .expect("real matching committed fact retained");
            assert_eq!(kernel.operation.transaction_id, pending.plan.transaction_id);
            assert_eq!(kernel.operation.active, committed.active);
            assert_eq!(kernel.operation.state, committed.state);
            assert_eq!(kernel.operation.durable, durable);
            let fact = state
                .core_receipt_for(&pending.plan.transaction_id, pending.generation)
                .expect("K stays queryable independently of R");
            assert_eq!(fact.core_state, Some("committed"));
            assert_eq!(fact.configuration_durable, Some(durable));
            assert_eq!(
                fact.persistence_mode.as_deref(),
                Some("exact_configuration_file")
            );
            let retained = state.pending.as_ref().expect("unpublished owner retained");
            assert_eq!(retained.plan.transaction_id, pending.plan.transaction_id);
            assert!(!retained.published);
            assert!(
                !state.host.has_in_flight(),
                "physical rejection cannot reserve host execution"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn blocked_noop_publication_preserves_original_kernel_persistence_owner() {
    for expired in [true, false] {
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let mut original = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
        original.published = true;
        original.plan.transaction_id = "original-kernel-transaction".into();
        *runtime.current_config.write().unwrap() = Some(original.candidate.clone());
        runtime.capture_runtime_config_baseline(
            original.generation,
            original.deps.clone(),
            original.generated.clone(),
            original.typed.clone(),
            original.pairing.clone(),
        );
        let mut operation = committed_ts_receipt(&original);
        operation.durable = false;
        operation.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
        operation.persistence_mode = "exact_configuration_file".into();
        let active = operation.active.clone().unwrap();
        let control_revision = operation.control_revision;
        let mut retry = original.clone();
        retry.published = false;
        retry.plan.no_op = true;
        retry.plan.transaction_id = "unused-noop-plan".into();
        retry.plan.expected = Some(active.clone());
        retry.plan.candidate_digest = active.effective_digest.clone();
        retry.plan.expected_control_revision = control_revision;
        retry.plan.expires_at_unix_milli = i64::MAX;
        retry.host_plan = Some(host_effects::HostEffectPlan {
            effect_digest: "effect".into(),
            physical_facts_digest: "physical".into(),
            host_epoch: 2,
            effects: vec![host_effects::HostEffectPreview {
                kind: host_effects::HostEffectKind::Dns,
                scope_digest: "scope".into(),
                before_digest: "before".into(),
                target_digest: "after".into(),
            }],
            strategy: host_effects::HostEffectStrategy::HostOnly,
            recipe: json!({}),
        });
        let (owner, proof) = crate::runtime::route_binding::blocked_physical_binding_proof_for_test(
            retry.generation,
            runtime.network_epoch().expect("desktop watcher epoch"),
            expired,
        );
        retry.protected_physical_root = Some(owner);
        retry.protected_binding_plan = Some(proof);
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            state.pending = Some(original.clone());
            state.kernel = Some(RuntimeKernelReceipt {
                generation: original.generation,
                intent: original.intent,
                operation: operation.clone(),
                publication: RuntimeReceiptPublication::Current,
            });
            assert!(
                state.host_only_preserves_pending_core(&retry),
                "same-candidate NoOp admission precondition"
            );
        }
        // Invoke the actual publication reservation, not a parallel test state
        // machine. Its memory proof gate must reject before host reservation.
        assert!(runtime
            .publish_prepared_runtime_candidate(&retry, None, Some(active), control_revision, None,)
            .is_err());
        assert_eq!(
            *runtime.current_config.read().unwrap(),
            Some(original.candidate.clone())
        );
        let state = runtime.runtime_config_apply.lock().unwrap();
        let retained = state
            .pending
            .as_ref()
            .expect("original persistence owner retained");
        assert_eq!(retained.plan.transaction_id, original.plan.transaction_id);
        assert!(retained.published);
        let kernel = state
            .kernel
            .as_ref()
            .expect("original Core receipt retained");
        assert_eq!(
            kernel.operation.transaction_id,
            original.plan.transaction_id
        );
        assert_eq!(kernel.operation.active, operation.active);
        assert_eq!(
            kernel.operation.control_revision,
            operation.control_revision
        );
        assert_eq!(kernel.operation.state, operation.state);
        assert!(!kernel.operation.durable);
        let fact = state
            .core_receipt_for(&original.plan.transaction_id, original.generation)
            .unwrap();
        assert_eq!(fact.core_state, Some("committed"));
        assert_eq!(fact.configuration_durable, Some(false));
        assert_eq!(
            fact.persistence_mode.as_deref(),
            Some("exact_configuration_file")
        );
        assert!(state
            .core_receipt_for(&retry.plan.transaction_id, retry.generation)
            .is_none());
        assert!(
            !state.host.has_in_flight(),
            "blocked NoOp cannot reserve host execution"
        );
    }
}

// R13: exercise the real K/D settlement and original native retirement observer.
// These are synthetic authenticated RPC receipts, not native Close evidence.
fn listener_retirement_receipt(
    pending: &PendingOperation,
    retirement: &str,
) -> daemon::RuntimeConfigOperation {
    let mut operation = committed_ts_receipt(pending);
    operation.persistence_mode = "exact_configuration_file".into();
    operation.retirement_state = retirement.into();
    if retirement == "UNKNOWN" {
        operation.errors.push(daemon::RuntimePlanError {
            code: "LISTENER_RETIREMENT_FAILED".into(),
            effective_path: "inbounds".into(),
            ..Default::default()
        });
    }
    operation
}

fn native_retirement_status(
    operation: daemon::RuntimeConfigOperation,
) -> daemon::RuntimeConfigStatus {
    daemon::RuntimeConfigStatus {
        contract_version: 1,
        active: operation.active.clone(),
        lookup_state: daemon::RuntimeLookupState::Found as i32,
        control_revision: operation.control_revision,
        operation: Some(operation),
        ..Default::default()
    }
}

fn assert_original_native_debt(result: &RuntimeApplyResult, pending: &PendingOperation) {
    assert!(!result.ok);
    assert!(result.pending_apply);
    let wire = serde_json::to_value(result).unwrap();
    assert_eq!(
        wire["nativeRetirements"],
        json!([{
            "transactionId": pending.plan.transaction_id,
            "runtimeGeneration": pending.generation,
            "selectionIntentGeneration": pending.intent,
            "retirementState": "UNKNOWN",
            "reason": {"code": "LISTENER_RETIREMENT_FAILED", "path": "inbounds"}
        }])
    );
}

#[test]
fn cached_kernel_receipt_retains_native_unknown_without_undoing_durable_commit() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let state = RuntimeConfigApplyState {
        kernel: Some(RuntimeKernelReceipt {
            generation: pending.generation,
            intent: pending.intent,
            publication: RuntimeReceiptPublication::Current,
            operation: listener_retirement_receipt(&pending, "UNKNOWN"),
        }),
        ..Default::default()
    };
    let result = state
        .core_receipt_for(&pending.plan.transaction_id, pending.generation)
        .unwrap();
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.status, "nativeCleanupUnknown");
    assert_eq!(result.core_state, Some("committed"));
    assert_eq!(result.configuration_durable, Some(true));
    assert_eq!(
        result.transaction_id.as_deref(),
        Some(pending.plan.transaction_id.as_str())
    );
    assert_eq!(result.selection_intent_generation, Some(pending.intent));
    assert_eq!(result.retirement_state.as_deref(), Some("UNKNOWN"));
    assert_eq!(
        result.persistence_mode.as_deref(),
        Some("exact_configuration_file")
    );
}

#[tokio::test]
async fn real_settlement_preserves_native_unknown_with_empty_scope_and_no_host_work() {
    let (runtime, _dir, calls) = crate::runtime::proxy::tests::test_runtime_recording();
    runtime
        .config
        .save_full(&runnable_config_fixture())
        .unwrap();
    let pending = ts_pending_raw_fixture(&runtime, runtime.config.current().unwrap());
    runtime.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    *runtime.current_config.write().unwrap() = Some(pending.desired.clone());
    assert!(pending.deletion_scope.is_empty());
    assert!(pending.host_plan.is_none());
    *runtime.startup_snapshot.write().unwrap() = Some(pending.desired.clone());
    runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
    let result = runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"))
        .await;
    let result = runtime.remember_runtime_result(result);
    assert_original_native_debt(&result, &pending);
    assert!(
        !runtime.restart_deferred.load(Ordering::SeqCst),
        "native cleanup alone is not an unapplied desired-state change"
    );
    assert_eq!(result.status, "nativeCleanupUnknown");
    assert_eq!(result.core_state, Some("committed"));
    assert_eq!(result.configuration_durable, Some(true));
    assert_eq!(
        runtime.current_config_snapshot(),
        Some(pending.candidate.clone())
    );
    assert!(result.host_effects.is_empty());
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(
        state.pending.is_none(),
        "native cleanup does not invent a D persistence debt"
    );
    assert!(!state.host.has_in_flight());
    assert_eq!(state.native_retirements.len(), 1);
    assert_original_native_debt(
        &state
            .core_receipt_for(&pending.plan.transaction_id, pending.generation)
            .unwrap(),
        &pending,
    );
    drop(state);
    let summary = runtime.pending_changes();
    assert!(summary.added.is_empty() && summary.modified.is_empty() && summary.removed.is_empty());
    assert!(summary.pending_apply);
    assert_eq!(
        serde_json::to_value(summary).unwrap()["nativeRetirements"],
        serde_json::to_value(result).unwrap()["nativeRetirements"]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepted_carrier_pending_remains_success_and_durability_keeps_its_own_truth() {
    for retirement in ["PENDING", "DRAINING"] {
        for durable in [true, false] {
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
            let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
            pending.published = true;
            let mut operation = listener_retirement_receipt(&pending, retirement);
            operation.durable = durable;
            if !durable {
                operation.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
            }
            let result = runtime.settle_runtime_operation(&pending, operation).await;
            assert_eq!(
                result.status,
                if durable {
                    "hotApplied"
                } else {
                    "hotDurabilityPending"
                }
            );
            assert!(result.ok);
            assert_eq!(result.core_state, Some("committed"));
            assert_eq!(result.configuration_durable, Some(durable));
            assert_eq!(result.retirement_state.as_deref(), Some(retirement));
            assert!(serde_json::to_value(result)
                .unwrap()
                .get("nativeRetirements")
                .is_none_or(|v| v.as_array().unwrap().is_empty()));
            let state = runtime.runtime_config_apply.lock().unwrap();
            assert_eq!(state.pending.is_some(), !durable);
            assert_eq!(
                state.native_retirements.len(),
                1,
                "empty deletion scope still retains the original observer"
            );
        }
    }
}

#[test]
fn original_retirement_refresh_rejects_each_mismatched_receipt_axis() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let original_operation = listener_retirement_receipt(&pending, "PENDING");
    for change in 0..20 {
        let mut state = RuntimeConfigApplyState::default();
        assert!(state.observe_native_retirement(&pending, &original_operation));
        let original = state.native_retirements[&pending.plan.transaction_id].clone();
        let mut status = native_retirement_status(listener_retirement_receipt(&pending, "UNKNOWN"));
        match change {
            0 => status.contract_version = 2,
            1 => status.lookup_state = daemon::RuntimeLookupState::UnknownOrExpired as i32,
            2 => status.operation = None,
            3 => status.active.as_mut().unwrap().incarnation = "replacement".into(),
            _ => {
                let operation = status.operation.as_mut().unwrap();
                match change {
                    4 => operation.contract_version = 2,
                    5 => operation.transaction_id = "host-B-or-new-D".into(),
                    6 => operation.candidate_digest = "other-candidate".into(),
                    7 => operation.impact_digest = "other-impact".into(),
                    8 => operation.before.as_mut().unwrap().incarnation = "other-before".into(),
                    9 => operation.before.as_mut().unwrap().applied_revision += 1,
                    10 => {
                        operation.before.as_mut().unwrap().effective_digest = "other-before".into()
                    }
                    11 => operation.before = None,
                    12 => operation.active.as_mut().unwrap().incarnation = "other-active".into(),
                    13 => operation.active.as_mut().unwrap().applied_revision += 1,
                    14 => {
                        operation.active.as_mut().unwrap().effective_digest = "other-active".into()
                    }
                    15 => operation.active = None,
                    16 => operation.control_revision += 1,
                    17 => operation.state = daemon::RuntimeOperationState::RejectedUnapplied as i32,
                    18 => operation.durable = false,
                    _ => operation.retirement_state = "FUTURE_STATE".into(),
                }
            }
        }
        assert!(
            state
                .refresh_native_retirement(&original, &status)
                .is_none(),
            "axis {change}"
        );
        assert_eq!(
            state.native_retirements[&pending.plan.transaction_id]
                .receipt
                .operation
                .retirement_state,
            "PENDING",
            "axis {change}"
        );
    }
    for change in 0..3 {
        let mut state = RuntimeConfigApplyState::default();
        state.observe_native_retirement(&pending, &original_operation);
        let mut borrowed = state.native_retirements[&pending.plan.transaction_id].clone();
        match change {
            0 => borrowed.receipt.generation += 1,
            1 => borrowed.receipt.intent += 1,
            _ => borrowed.pairing.implementation_manifest_digest = "b".repeat(64),
        }
        assert!(state
            .refresh_native_retirement(
                &borrowed,
                &native_retirement_status(listener_retirement_receipt(&pending, "UNKNOWN"))
            )
            .is_none());
    }
}

#[test]
fn original_refresh_keeps_first_unknown_and_does_not_borrow_successor_kernel_owner() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let mut state = RuntimeConfigApplyState::default();
    state.observe_native_retirement(&pending, &listener_retirement_receipt(&pending, "PENDING"));
    let original = state.native_retirements[&pending.plan.transaction_id].clone();
    let mut successor = listener_retirement_receipt(&pending, "DRAINED");
    successor.transaction_id = "later-core-owner".into();
    successor.active.as_mut().unwrap().applied_revision += 1;
    state.kernel = Some(RuntimeKernelReceipt {
        generation: pending.generation,
        intent: pending.intent + 1,
        publication: RuntimeReceiptPublication::Current,
        operation: successor.clone(),
    });
    let mut status = native_retirement_status(listener_retirement_receipt(&pending, "UNKNOWN"));
    status.active = successor.active.clone();
    let result = state.refresh_native_retirement(&original, &status).unwrap();
    assert_original_native_debt(&result, &pending);
    assert_eq!(state.kernel.as_ref().unwrap().operation, successor);
    let first = state.native_retirements[&pending.plan.transaction_id]
        .receipt
        .operation
        .clone();
    for retirement in ["DRAINED", "PENDING", "UNKNOWN"] {
        let mut status =
            native_retirement_status(listener_retirement_receipt(&pending, retirement));
        status.operation.as_mut().unwrap().errors.clear();
        assert_original_native_debt(
            &state.refresh_native_retirement(&original, &status).unwrap(),
            &pending,
        );
        assert_eq!(
            state.native_retirements[&pending.plan.transaction_id]
                .receipt
                .operation,
            first
        );
    }
}

#[tokio::test]
async fn stale_verified_commit_still_observes_native_failure_without_publishing_over_new_intent() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let newer = json!({"newerRunningOwner": true});
    *runtime.current_config.write().unwrap() = Some(newer.clone());
    runtime.selector_reconcile.register_intent();
    let result = runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"))
        .await;
    assert_eq!(runtime.current_config_snapshot(), Some(newer));
    let remembered = runtime.remember_runtime_result(result);
    assert_original_native_debt(&remembered, &pending);
    assert_eq!(remembered.status, "conflict");
    assert_eq!(
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .native_retirements[&pending.plan.transaction_id]
            .receipt
            .intent,
        pending.intent
    );
}

#[tokio::test]
async fn native_unknown_survives_same_generation_baseline_recapture_until_teardown() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    pending.published = true;
    *runtime.startup_snapshot.write().unwrap() = Some(pending.desired.clone());
    runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"))
        .await;
    runtime.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    assert!(runtime.native_retirement_pending());
    assert_eq!(
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .native_retirements
            .len(),
        1
    );
    // Lifecycle claims/bump happen before native stop; a still-running old
    // process has not released its FD merely because the gate value changed.
    runtime.gate.bump_generation();
    assert!(runtime.core_running());
    assert!(runtime.native_retirement_pending());
    assert!(runtime.pending_changes().pending_apply);
    // This lifecycle fixture has no RPC peer; fail closed before connecting to
    // the generated management port and still expose the retained native debt.
    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .management = None;
    let result = runtime
        .runtime_apply_status(pending.plan.transaction_id.clone())
        .await;
    assert_original_native_debt(&result, &pending);
    let newer_generation = runtime.gate.generation();
    runtime.capture_runtime_config_baseline(
        newer_generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    assert!(
        runtime.native_retirement_pending(),
        "recapture cannot certify old native teardown"
    );
    assert_eq!(
        runtime.native_retirement_debts()[0].runtime_generation,
        pending.generation
    );
    // This is the production baseline teardown invoked after actual process stop;
    // the synthetic fixture makes no claim about an OS/native stop having run.
    runtime.clear_runtime_config_baseline();
    assert!(!runtime.native_retirement_pending());
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .native_retirements
        .is_empty());
    runtime.gate.bump_generation();
    runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"))
        .await;
    assert!(
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .native_retirements
            .is_empty(),
        "old-generation callbacks cannot resurrect retired debt"
    );
}

type ProbeGraphCalls = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;
type ProbeSelectCallback = Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;
type RuntimeRpcReplyHold = Arc<
    std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
>;

#[derive(Clone)]
struct NativeRetirementRpcFixture {
    capabilities: Arc<std::sync::Mutex<daemon::RuntimeCapabilities>>,
    status: Arc<std::sync::Mutex<daemon::RuntimeConfigStatus>>,
    requests: Arc<std::sync::Mutex<Vec<daemon::RuntimeConfigStatusRequest>>>,
    status_hold: RuntimeRpcReplyHold,
    probe_selection: Arc<std::sync::atomic::AtomicBool>,
    probe_select_first_unavailable: Arc<std::sync::atomic::AtomicBool>,
    probe_select_calls: ProbeGraphCalls,
    on_probe_select: ProbeSelectCallback,
    probe_select_hold: RuntimeRpcReplyHold,
    selected_group_member: Arc<std::sync::Mutex<Option<String>>>,
    capability_reads: Arc<std::sync::atomic::AtomicUsize>,
    mutations: Arc<std::sync::atomic::AtomicUsize>,
    acknowledgement_response: Arc<std::sync::Mutex<Option<daemon::RuntimeConfigOperation>>>,
    acknowledgements: Arc<std::sync::Mutex<Vec<daemon::RuntimePersistenceAcknowledgementRequest>>>,
    apply_response: Arc<std::sync::Mutex<Option<daemon::RuntimeConfigOperation>>>,
    rejected_apply_transactions: Arc<std::sync::Mutex<Option<Vec<String>>>>,
    exhausted_apply_hold: RuntimeRpcReplyHold,
    // RPC errors answered, in order, before the normal Plan reply.
    failed_plans: Arc<std::sync::Mutex<Vec<polaris_singbox_grpc::tonic::Code>>>,
}

#[derive(Clone)]
struct ProbeSelectorPeer {
    runtime: daemon::runtime_config_service_server::RuntimeConfigServiceServer<
        NativeRetirementRpcFixture,
    >,
    fixture: NativeRetirementRpcFixture,
}

struct ProbeGroupReadback(String);

impl polaris_singbox_grpc::tonic::server::ServerStreamingService<()> for ProbeGroupReadback {
    type Response = daemon::Groups;
    type ResponseStream = futures::stream::Iter<
        std::array::IntoIter<Result<Self::Response, polaris_singbox_grpc::tonic::Status>, 1>,
    >;
    type Future = std::future::Ready<
        Result<
            polaris_singbox_grpc::tonic::Response<Self::ResponseStream>,
            polaris_singbox_grpc::tonic::Status,
        >,
    >;

    fn call(&mut self, _: polaris_singbox_grpc::tonic::Request<()>) -> Self::Future {
        let groups = daemon::Groups {
            group: vec![daemon::Group {
                tag: "proxy-selector".into(),
                selected: self.0.clone(),
                ..Default::default()
            }],
        };
        std::future::ready(Ok(polaris_singbox_grpc::tonic::Response::new(
            futures::stream::iter([Ok(groups)]),
        )))
    }
}

impl
    polaris_singbox_grpc::tonic::codegen::Service<
        polaris_singbox_grpc::tonic::codegen::http::Request<
            polaris_singbox_grpc::tonic::body::Body,
        >,
    > for ProbeSelectorPeer
{
    type Response = polaris_singbox_grpc::tonic::codegen::http::Response<
        polaris_singbox_grpc::tonic::body::Body,
    >;
    type Error = std::convert::Infallible;
    type Future = polaris_singbox_grpc::tonic::codegen::BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(
        &mut self,
        request: polaris_singbox_grpc::tonic::codegen::http::Request<
            polaris_singbox_grpc::tonic::body::Body,
        >,
    ) -> Self::Future {
        use polaris_singbox_grpc::tonic::{self, Status};
        if request
            .uri()
            .path()
            .starts_with("/daemon.RuntimeConfigService/")
        {
            return self.runtime.call(request);
        }
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer native-retirement-test"
        );
        let path = request.uri().path();
        let graph = request
            .headers()
            .get("x-polaris-probe-graph")
            .map(|value| value.to_str().unwrap().to_owned());
        self.fixture
            .probe_select_calls
            .lock()
            .unwrap()
            .push((path.into(), graph));
        let selected = self.fixture.selected_group_member.lock().unwrap().clone();
        if let Some(selected) = selected {
            if path == "/daemon.StartedService/SubscribeGroups" {
                return Box::pin(async move {
                    let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
                    Ok(grpc
                        .server_streaming(ProbeGroupReadback(selected), request)
                        .await)
                });
            }
            if path == "/daemon.StartedService/SelectOutbound" {
                return Box::pin(async {
                    let mut response = Status::new(tonic::Code::Ok, "").into_http();
                    *response.body_mut() =
                        tonic::body::Body::new(reqwest::Body::from(vec![0u8; 5]));
                    Ok(response)
                });
            }
        }
        let (status, hold) = if path == "/daemon.StartedService/SelectOutbound" {
            let callback = self.fixture.on_probe_select.lock().unwrap().take();
            if let Some(callback) = callback {
                callback();
            }
            let status = if self
                .fixture
                .probe_select_first_unavailable
                .swap(false, Ordering::SeqCst)
            {
                Status::unavailable("selector result unknown")
            } else {
                Status::failed_precondition("active graph changed before write")
            };
            (
                status,
                self.fixture.probe_select_hold.lock().unwrap().take(),
            )
        } else {
            (Status::unimplemented("no readback"), None)
        };
        Box::pin(async move {
            if let Some((entered, release)) = hold {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            Ok(status.into_http())
        })
    }
}

impl NativeRetirementRpcFixture {
    fn forbidden(&self) -> polaris_singbox_grpc::tonic::Status {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        polaris_singbox_grpc::tonic::Status::failed_precondition(
            "native retirement observation cannot mutate Core or acknowledge persistence",
        )
    }
}

#[polaris_singbox_grpc::tonic::async_trait]
impl daemon::runtime_config_service_server::RuntimeConfigService for NativeRetirementRpcFixture {
    async fn lookup_public_dns(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::LookupPublicDnsRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::LookupPublicDnsResponse>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented(
            "public DNS lookup",
        ))
    }

    async fn authorize_pool_probe(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::AuthorizePoolProbeRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::AuthorizePoolProbeResponse>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented(
            "pool probe ticket",
        ))
    }

    async fn probe_exit_ip(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::ProbeExitIpRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::ProbeExitIpResponse>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented(
            "exit IP probe",
        ))
    }

    async fn get_runtime_capabilities(
        &self,
        _: polaris_singbox_grpc::tonic::Request<()>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeCapabilities>,
        polaris_singbox_grpc::tonic::Status,
    > {
        self.capability_reads.fetch_add(1, Ordering::SeqCst);
        Ok(polaris_singbox_grpc::tonic::Response::new(
            self.capabilities.lock().unwrap().clone(),
        ))
    }
    async fn plan_runtime_config(
        &self,
        request: polaris_singbox_grpc::tonic::Request<daemon::PlanRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigPlan>,
        polaris_singbox_grpc::tonic::Status,
    > {
        let failure = {
            let mut failures = self.failed_plans.lock().unwrap();
            (!failures.is_empty()).then(|| failures.remove(0))
        };
        if let Some(code) = failure {
            return Err(polaris_singbox_grpc::tonic::Status::new(
                code,
                "stale or stopped runtime",
            ));
        }
        if let Some(transactions) = self.rejected_apply_transactions.lock().unwrap().as_ref() {
            let next = transactions.len() + 1;
            let request = request.into_inner();
            return Ok(polaris_singbox_grpc::tonic::Response::new(
                daemon::RuntimeConfigPlan {
                    contract_version: request.contract_version,
                    transaction_id: format!("transaction-{next}"),
                    plan_id: format!("plan-{next}"),
                    expected: request.expected,
                    expected_control_revision: request.expected_control_revision,
                    candidate_digest: polaris_updater::sha256_hex(
                        request.config_content.as_bytes(),
                    ),
                    impact_digest: "impact".into(),
                    applicable: true,
                    expires_at_unix_milli: i64::MAX,
                    ..Default::default()
                },
            ));
        }
        Err(self.forbidden())
    }
    async fn apply_runtime_config(
        &self,
        request: polaris_singbox_grpc::tonic::Request<daemon::ApplyRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        let rejection_count = self
            .rejected_apply_transactions
            .lock()
            .unwrap()
            .as_mut()
            .map(|transactions| {
                transactions.push(request.get_ref().transaction_id.clone());
                transactions.len()
            });
        if let Some(count) = rejection_count {
            if count == usize::from(RUNTIME_APPLY_REPLANS) + 1 {
                let hold = self.exhausted_apply_hold.lock().unwrap().take();
                if let Some((entered, release)) = hold {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            let request = request.into_inner();
            return Ok(polaris_singbox_grpc::tonic::Response::new(
                daemon::RuntimeConfigOperation {
                    contract_version: request.contract_version,
                    transaction_id: request.transaction_id,
                    candidate_digest: request.candidate_digest,
                    impact_digest: request.accepted_impact_digest,
                    before: request.expected.clone(),
                    active: request.expected,
                    control_revision: request.expected_control_revision,
                    state: daemon::RuntimeOperationState::RejectedUnapplied as i32,
                    errors: vec![daemon::RuntimePlanError {
                        code: PLAN_PRECONDITION_CHANGED.into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ));
        }
        match self.apply_response.lock().unwrap().clone() {
            Some(response) => Ok(polaris_singbox_grpc::tonic::Response::new(response)),
            None => Err(self.forbidden()),
        }
    }
    async fn get_runtime_config_status(
        &self,
        request: polaris_singbox_grpc::tonic::Request<daemon::RuntimeConfigStatusRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigStatus>,
        polaris_singbox_grpc::tonic::Status,
    > {
        self.requests.lock().unwrap().push(request.into_inner());
        let mut reply =
            polaris_singbox_grpc::tonic::Response::new(self.status.lock().unwrap().clone());
        let hold = self.status_hold.lock().unwrap().take();
        if let Some((entered, release)) = hold {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        if self.probe_selection.load(Ordering::SeqCst) {
            reply
                .metadata_mut()
                .insert("x-polaris-probe-select-condition", "1".parse().unwrap());
        }
        Ok(reply)
    }
    async fn retry_runtime_persistence(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::RuntimePersistenceRetryRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(self.forbidden())
    }
    async fn acknowledge_runtime_persistence(
        &self,
        request: polaris_singbox_grpc::tonic::Request<
            daemon::RuntimePersistenceAcknowledgementRequest,
        >,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        self.acknowledgements
            .lock()
            .unwrap()
            .push(request.into_inner());
        // An explicit static response is test data. Production retains all
        // durable-owner admission, identity construction and response checks.
        match self.acknowledgement_response.lock().unwrap().clone() {
            Some(response) => Ok(polaris_singbox_grpc::tonic::Response::new(response)),
            None => Err(self.forbidden()),
        }
    }
}

// Reuse the existing API-handoff tests' tonic peer pattern. Only an authenticated
// captured transport and synthetic status are supplied; no native runtime starts.
async fn native_retirement_rpc_peer(
    runtime: &Arc<ProxyRuntime>,
    pending: &PendingOperation,
    operation: daemon::RuntimeConfigOperation,
) -> (
    NativeRetirementRpcFixture,
    tokio::task::JoinHandle<Result<(), polaris_singbox_grpc::tonic::transport::Error>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut capabilities = paired_capabilities();
    capabilities.active = operation.active.clone();
    capabilities.control_revision = operation.control_revision;
    assert!(runtime_pairing_matches(&pending.pairing, &capabilities));
    let fixture = NativeRetirementRpcFixture {
        capabilities: Arc::new(std::sync::Mutex::new(capabilities)),
        status: Arc::new(std::sync::Mutex::new(native_retirement_status(operation))),
        requests: Arc::default(),
        status_hold: Arc::default(),
        probe_selection: Arc::default(),
        probe_select_first_unavailable: Arc::default(),
        probe_select_calls: Arc::default(),
        on_probe_select: Arc::default(),
        probe_select_hold: Arc::default(),
        selected_group_member: Arc::default(),
        capability_reads: Arc::default(),
        mutations: Arc::default(),
        acknowledgement_response: Arc::default(),
        acknowledgements: Arc::default(),
        apply_response: Arc::default(),
        rejected_apply_transactions: Arc::default(),
        exhausted_apply_hold: Arc::default(),
        failed_plans: Arc::default(),
    };
    let incoming = futures::stream::unfold(listener, |listener| async move {
        let next = listener.accept().await.map(|(stream, _)| stream);
        Some((next, listener))
    });
    let service = ProbeSelectorPeer {
        runtime: daemon::runtime_config_service_server::RuntimeConfigServiceServer::new(
            fixture.clone(),
        ),
        fixture: fixture.clone(),
    };
    let server = tokio::spawn(async move {
        polaris_singbox_grpc::tonic::transport::Server::builder()
            .serve_with_incoming(service, incoming)
            .await
    });
    runtime.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .management = Some(ApiEndpoint {
        host: "127.0.0.1".into(),
        port,
        secret: "native-retirement-test".into(),
    });
    let control = SingBoxApiClient::connect_captured(
        Endpoint::new("127.0.0.1", port),
        "native-retirement-test",
    )
    .await
    .unwrap();
    let observed = control.get_runtime_capabilities().await.unwrap();
    assert!(runtime_pairing_matches(&pending.pairing, &observed));
    assert!(control.captured_transport_live());
    runtime.bind_runtime_api_control(pending.generation, Arc::new(control));
    (fixture, server)
}

#[tokio::test]
async fn pool_graph_capture_pins_the_paired_startup_and_observes_only_active_status() {
    use crate::commands::speedtest::{capture_pool_graph, read_pool_graph};

    let (runtime, _dir, _) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let incarnation = "a".repeat(64);
    pending.pairing.incarnation = Some(incarnation.clone());
    pending.plan.expected.as_mut().unwrap().incarnation = incarnation.clone();
    let mut operation = listener_retirement_receipt(&pending, "PENDING");
    let active = operation.active.as_mut().unwrap();
    active.incarnation = incarnation.clone();
    active.applied_revision = 1;
    let active = active.clone();
    let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, operation).await;
    let port = runtime
        .runtime_config_baseline
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .management
        .as_ref()
        .unwrap()
        .port;
    runtime.status.write().unwrap().clash_api_port = port;
    let generation = pending.generation;
    let graph = format!("{incarnation}/1");
    let next = format!("{incarnation}/2");
    assert!(runtime.published_pool_graph(generation).is_none());
    runtime
        .runtime_config_baseline
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .observed_active = Some(active.clone());
    assert_eq!(
        runtime.published_pool_graph(generation).as_deref(),
        Some(graph.as_str())
    );
    *rpc.status.lock().unwrap() = daemon::RuntimeConfigStatus {
        contract_version: 1,
        active: Some(active.clone()),
        lookup_state: daemon::RuntimeLookupState::ActiveOnly as i32,
        ..Default::default()
    };
    let scope = capture_pool_graph(&runtime, generation, &graph)
        .await
        .unwrap();
    assert_eq!(scope.graph.as_ref(), graph);
    assert!(
        !scope.can_select,
        "status from an older Core has no selector capability"
    );
    let requested = rpc.requests.lock().unwrap().clone();
    assert_eq!(
        requested,
        [daemon::RuntimeConfigStatusRequest {
            contract_version: 1,
            ..Default::default()
        }]
    );
    assert_eq!(
        rpc.capability_reads.load(Ordering::SeqCst),
        1,
        "only peer setup reads inventory"
    );
    let legacy = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), "legacy")
        .await
        .unwrap();
    assert!(read_pool_graph(&legacy).await.is_none());
    assert_eq!(rpc.requests.lock().unwrap().len(), 1);
    rpc.probe_selection.store(true, Ordering::SeqCst);
    assert!(
        capture_pool_graph(&runtime, generation, &graph)
            .await
            .unwrap()
            .can_select
    );

    let set_active = |identity| rpc.status.lock().unwrap().active = Some(identity);
    let publish = |identity| {
        runtime
            .runtime_config_baseline
            .write()
            .unwrap()
            .as_mut()
            .unwrap()
            .observed_active = Some(identity);
    };
    set_active(daemon::RuntimeIdentity {
        incarnation: "b".repeat(64),
        ..active.clone()
    });
    assert!(capture_pool_graph(&runtime, generation, &graph)
        .await
        .is_none());
    publish(active.clone());
    let native = daemon::RuntimeIdentity {
        applied_revision: 2,
        ..active.clone()
    };
    set_active(native.clone());
    assert_eq!(
        capture_pool_graph(&runtime, generation, &graph)
            .await
            .unwrap()
            .graph
            .as_ref(),
        next
    );
    assert_eq!(
        runtime.published_pool_graph(generation).as_deref(),
        Some(graph.as_str())
    );
    assert_eq!(
        runtime.current_pool_graph(generation).as_deref(),
        Some(next.as_str())
    );
    assert!(runtime.pool_graph_is_current(generation, &next));
    assert!(!runtime.pool_graph_is_current(generation, &graph));

    set_active(active);
    assert!(capture_pool_graph(&runtime, generation, &graph)
        .await
        .is_none());
    let replacement = daemon::RuntimeIdentity {
        applied_revision: 3,
        effective_digest: "b".repeat(64),
        ..native.clone()
    };
    set_active(replacement.clone());
    assert!(capture_pool_graph(&runtime, generation, &graph)
        .await
        .is_none());
    runtime.runtime_config_apply.lock().unwrap().pending = Some(pending);
    assert!(runtime.published_pool_graph(generation).is_none());
    set_active(native);
    assert!(capture_pool_graph(&runtime, generation, &graph)
        .await
        .is_none());
    runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .as_mut()
        .unwrap()
        .published = true;
    publish(replacement.clone());
    set_active(replacement);
    assert!(capture_pool_graph(&runtime, generation, &graph)
        .await
        .is_none());
    let replacement = format!("{incarnation}/3");
    assert!(capture_pool_graph(&runtime, generation, &replacement)
        .await
        .is_some());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn pool_graph_guarded_selection_never_restores_against_a_new_graph() {
    use crate::runtime::auto_switch::RuntimeCandidate;
    use crate::runtime::config::Decision;
    use crate::runtime::management_api::GrpcManagementApi;
    use crate::runtime::proxy::auto_switch::AutoHotSwitchOutcome;

    let select = "/daemon.StartedService/SelectOutbound";
    let groups = "/daemon.StartedService/SubscribeGroups";
    for (can_select, unavailable, newer_intent, held_stop, attested) in [
        (false, false, false, false, false),
        (true, false, false, false, false),
        (true, false, true, false, false),
        (true, true, false, false, false),
        (true, true, false, true, false),
        (true, false, false, false, true),
    ] {
        let (runtime, _dir, _) = crate::runtime::proxy::tests::test_runtime_recording();
        let mut raw = runnable_config_fixture();
        raw["servers"] = json!([{
            "id": "candidate", "name": "measured", "protocol": "shadowsocks",
            "address": "127.0.0.1", "port": 9,
            "shadowsocksSettings": { "method": "aes-128-gcm", "password": "fixture" }
        }]);
        raw["proxyMode"] = json!("global");
        raw["proxyModeType"] = json!("manual");
        raw["interruptConnectionsOnSwitch"] = json!(false);
        runtime.config.save_full(&raw).unwrap();
        let mut pending = ts_pending_raw_fixture(&runtime, raw.clone());
        let incarnation = "a".repeat(64);
        pending.pairing.incarnation = Some(incarnation.clone());
        pending.plan.expected.as_mut().unwrap().incarnation = incarnation.clone();
        let mut operation = listener_retirement_receipt(&pending, "PENDING");
        let active = operation.active.as_mut().unwrap();
        active.incarnation = incarnation.clone();
        active.applied_revision = 1;
        let active = active.clone();
        let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, operation).await;
        let snapshot =
            ProxyRuntime::build_switch_snapshot(&pending.typed, &pending.generated, &pending.deps);
        let candidate = RuntimeCandidate {
            id: "candidate".into(),
            name: "measured".into(),
            tag: snapshot.id_to_tag["candidate"].clone(),
        };
        *runtime.switch_snapshot.write().unwrap() = Some(snapshot);
        *runtime.current_config.write().unwrap() = Some(raw);
        let generation = pending.generation;
        {
            let mut baseline = runtime.runtime_config_baseline.write().unwrap();
            let baseline = baseline.as_mut().unwrap();
            runtime.status.write().unwrap().clash_api_port =
                baseline.management.as_ref().unwrap().port;
            baseline.observed_active = Some(active.clone());
        }
        *rpc.status.lock().unwrap() = daemon::RuntimeConfigStatus {
            contract_version: 1,
            active: Some(active),
            lookup_state: daemon::RuntimeLookupState::ActiveOnly as i32,
            ..Default::default()
        };
        rpc.probe_selection.store(can_select, Ordering::SeqCst);
        rpc.probe_select_first_unavailable
            .store(unavailable, Ordering::SeqCst);
        if attested {
            *rpc.selected_group_member.lock().unwrap() = Some(candidate.tag.clone());
        }
        if newer_intent {
            let runtime = Arc::clone(&runtime);
            *rpc.on_probe_select.lock().unwrap() = Some(Box::new(move || {
                runtime
                    .config
                    .update(|raw| {
                        runtime.register_selector_intent();
                        raw["selectedServerId"] = json!("candidate");
                        Decision::Write(())
                    })
                    .unwrap();
            }));
        }
        let fingerprint = &runtime.speed_measurement_fingerprints()["candidate"];
        let graph = format!("{incarnation}/1");
        let wait_for_stop = async {
            if held_stop {
                let (enter, entered) = tokio::sync::oneshot::channel();
                let (release, waiting) = tokio::sync::oneshot::channel();
                *rpc.probe_select_hold.lock().unwrap() = Some((enter, waiting));
                tokio::time::timeout(Duration::from_secs(2), entered)
                    .await
                    .unwrap()
                    .unwrap();
                // Ordinary Start reads saved config before claiming its lifecycle.
                assert_eq!(
                    runtime.config.current().unwrap()["selectedServerId"],
                    DIRECT_SERVER_ID
                );
                assert!(runtime
                    .claim_generation(Some(generation), crate::runtime::proxy::LifecycleKind::Stop)
                    .is_some());
                release.send(()).unwrap();
            }
        };
        let legacy_api = GrpcManagementApi::not_ready();
        let ((), outcome) = tokio::join!(
            wait_for_stop,
            runtime.auto_hot_switch_transaction_for_graph(
                generation,
                DIRECT_SERVER_ID,
                &candidate,
                fingerprint,
                None,
                &legacy_api,
                Some(&graph),
            ),
        );
        assert_eq!(
            outcome,
            if attested {
                AutoHotSwitchOutcome::Applied
            } else if unavailable && !held_stop {
                AutoHotSwitchOutcome::Failed
            } else {
                AutoHotSwitchOutcome::Superseded
            }
        );
        assert_eq!(
            runtime.selector_reconcile.is_required(),
            unavailable && !held_stop
        );
        assert_eq!(
            runtime.config.current().unwrap()["selectedServerId"],
            if newer_intent || attested {
                "candidate"
            } else {
                DIRECT_SERVER_ID
            }
        );
        assert_eq!(
            runtime.current_config.read().unwrap().as_ref().unwrap()["selectedServerId"],
            if attested {
                "candidate"
            } else {
                DIRECT_SERVER_ID
            }
        );
        let guarded_put = (select.to_string(), Some(graph));
        let expected = if attested {
            vec![guarded_put, (groups.to_string(), None)]
        } else if unavailable && !held_stop {
            vec![guarded_put.clone(), guarded_put, (groups.to_string(), None)]
        } else if can_select {
            vec![guarded_put]
        } else {
            vec![]
        };
        assert_eq!(*rpc.probe_select_calls.lock().unwrap(), expected);
        assert_eq!(rpc.requests.lock().unwrap().len(), 1);
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn actual_status_refreshes_original_pending_without_deletion_scope_and_keeps_unknown_sticky()
{
    for terminal in ["UNKNOWN", "DRAINED"] {
        let (runtime, _dir, calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
        pending.published = true;
        assert!(pending.deletion_scope.is_empty() && pending.host_plan.is_none());
        let operation = listener_retirement_receipt(&pending, "PENDING");
        let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, operation.clone()).await;
        runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
        assert_eq!(
            runtime
                .settle_runtime_operation(&pending, operation)
                .await
                .status,
            "hotApplied"
        );
        assert!(runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .pending
            .is_none());
        *rpc.status.lock().unwrap() =
            native_retirement_status(listener_retirement_receipt(&pending, terminal));
        // A fresh original-owner read is still required after the selector intent
        // and desired state move on. Neither becomes ownership of this old Close.
        runtime.selector_reconcile.register_intent();
        let result = runtime
            .runtime_apply_status(pending.plan.transaction_id.clone())
            .await;
        assert_eq!(
            result.transaction_id.as_deref(),
            Some(pending.plan.transaction_id.as_str())
        );
        assert_eq!(result.runtime_generation, Some(pending.generation));
        assert_eq!(result.selection_intent_generation, Some(pending.intent));
        assert_eq!(result.core_state, Some("committed"));
        assert_eq!(result.configuration_durable, Some(true));
        assert_eq!(result.retirement_state.as_deref(), Some(terminal));
        if terminal == "UNKNOWN" {
            assert_eq!(result.status, "nativeCleanupUnknown");
            assert_original_native_debt(&result, &pending);
            *rpc.status.lock().unwrap() =
                native_retirement_status(listener_retirement_receipt(&pending, "DRAINED"));
            for _ in 0..2 {
                assert_original_native_debt(
                    &runtime
                        .runtime_apply_status(pending.plan.transaction_id.clone())
                        .await,
                    &pending,
                );
            }
        } else {
            assert_eq!(result.status, "hotApplied");
            assert!(result.ok);
            assert!(!runtime.native_retirement_pending());
        }
        {
            let requests = rpc.requests.lock().unwrap();
            assert!(
                !requests.is_empty(),
                "cached PENDING must actually refresh over the production status RPC"
            );
            for request in requests.iter() {
                assert_eq!(request.contract_version, 1);
                assert_eq!(request.transaction_id, pending.plan.transaction_id);
                assert_eq!(
                    request.runtime_incarnation,
                    pending.plan.expected.as_ref().unwrap().incarnation
                );
            }
        }
        assert_eq!(
            rpc.mutations.load(Ordering::SeqCst),
            0,
            "status never plans/applies/retries/ACKs"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn actual_status_rejects_replacement_pairing_but_gate_bump_keeps_original_live_owner_observable(
) {
    for wrong_generation in [false, true] {
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
        pending.published = true;
        let operation = listener_retirement_receipt(&pending, "PENDING");
        let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, operation.clone()).await;
        runtime.settle_runtime_operation(&pending, operation).await;
        *rpc.status.lock().unwrap() =
            native_retirement_status(listener_retirement_receipt(&pending, "UNKNOWN"));
        if wrong_generation {
            runtime.gate.bump_generation();
        } else {
            rpc.capabilities
                .lock()
                .unwrap()
                .implementation_manifest_digest = "b".repeat(64);
        }
        let result = runtime
            .runtime_apply_status(pending.plan.transaction_id.clone())
            .await;
        assert!(!result.ok);
        if wrong_generation {
            assert_original_native_debt(&result, &pending);
            assert!(!rpc.requests.lock().unwrap().is_empty(), "a gate claim is not native process termination; authenticate and observe the original live owner");
            assert_eq!(
                runtime
                    .runtime_config_apply
                    .lock()
                    .unwrap()
                    .native_retirements[&pending.plan.transaction_id]
                    .receipt
                    .operation
                    .retirement_state,
                "UNKNOWN"
            );
        } else {
            assert!(
                rpc.requests.lock().unwrap().is_empty(),
                "a replacement pairing cannot authenticate the original process"
            );
            assert_eq!(
                runtime
                    .runtime_config_apply
                    .lock()
                    .unwrap()
                    .native_retirements[&pending.plan.transaction_id]
                    .receipt
                    .operation
                    .retirement_state,
                "PENDING"
            );
        }
        assert_eq!(rpc.mutations.load(Ordering::SeqCst), 0);
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn accepted_host_only_and_late_host_callbacks_cannot_discharge_native_unknown() {
    use host_effects::{
        HostEffectExecution, HostEffectKind, HostEffectOutcome, HostEffectPlan, HostEffectPreview,
        HostEffectReceipt, HostEffectResult, HostEffectStrategy,
    };
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    pending.published = true;
    runtime.capture_runtime_config_baseline(
        pending.generation,
        pending.deps.clone(),
        pending.generated.clone(),
        pending.typed.clone(),
        pending.pairing.clone(),
    );
    let operation = listener_retirement_receipt(&pending, "UNKNOWN");
    runtime
        .settle_runtime_operation(&pending, operation.clone())
        .await;
    let execution = HostEffectExecution {
        execution_id: "accepted-host-only-B".into(),
        transaction_id: None,
        generation: pending.generation,
        intent: pending.intent,
        desired: pending.desired.clone(),
        core_active: operation.active.clone(),
        core_control_revision: operation.control_revision,
        plan: HostEffectPlan {
            effect_digest: "host-effect".into(),
            physical_facts_digest: "host-physical".into(),
            host_epoch: 4,
            strategy: HostEffectStrategy::HostOnly,
            recipe: json!({}),
            effects: [HostEffectKind::Dns, HostEffectKind::SystemProxy]
                .into_iter()
                .map(|kind| HostEffectPreview {
                    kind,
                    scope_digest: "host-scope".into(),
                    before_digest: "host-before".into(),
                    target_digest: "host-target".into(),
                })
                .collect(),
        },
    };
    let receipt = HostEffectReceipt {
        effect_digest: execution.plan.effect_digest.clone(),
        physical_facts_digest: execution.plan.physical_facts_digest.clone(),
        host_epoch: execution.plan.host_epoch,
        effects: execution
            .plan
            .effects
            .iter()
            .cloned()
            .map(|preview| HostEffectResult {
                preview,
                outcome: HostEffectOutcome::Applied,
                receipt_id: Some("owned-host-receipt".into()),
                cleanup_pending: false,
                retryable: false,
                reason_code: None,
            })
            .collect(),
    };
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &pending.desired));
    runtime.record_runtime_host_effect_result(&execution, receipt.clone());
    assert!(
        !runtime.host_effect_pending(),
        "host receipt is an actually accepted independent positive control"
    );
    let mut result = RuntimeApplyResult::status("hostEffectsFailed");
    result.core_state = Some("unchanged");
    result.configuration_durable = Some(true);
    result.runtime_generation = Some(pending.generation);
    result.selection_intent_generation = Some(pending.intent);
    result.host_execution_id = Some(execution.execution_id.clone());
    let result = runtime.remember_runtime_result(result);
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.configuration_durable, Some(true));
    assert_eq!(result.host_effects.len(), 2);
    let mut next = pending.clone();
    next.plan.transaction_id = "later-noop-plan".into();
    next.plan.no_op = true;
    next.plan.expected = operation.active.clone();
    next.plan.candidate_digest = operation.active.as_ref().unwrap().effective_digest.clone();
    next.plan.expected_control_revision = operation.control_revision;
    let mut noop = operation.clone();
    noop.transaction_id = next.plan.transaction_id.clone();
    noop.before = next.plan.expected.clone();
    noop.active = next.plan.expected.clone();
    noop.retirement_state = "DRAINED".into();
    noop.errors.clear();
    let noop_result = runtime.settle_runtime_operation(&next, noop).await;
    assert_original_native_debt(&noop_result, &pending);
    assert_eq!(noop_result.status, "nativeCleanupUnknown");
    assert_eq!(
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .kernel
            .as_ref()
            .unwrap()
            .operation
            .transaction_id,
        "later-noop-plan"
    );
    assert_eq!(noop_result.core_state, Some("committed"));
    assert_eq!(noop_result.configuration_durable, Some(true));
    runtime.selector_reconcile.register_intent();
    let mut late = execution.clone();
    late.execution_id = "late-unregistered-host".into();
    runtime.record_runtime_host_effect_result(&late, receipt);
    let mut later = RuntimeApplyResult::status("hotApplied");
    later.core_state = Some("unchanged");
    later.configuration_durable = Some(true);
    later.runtime_generation = Some(pending.generation);
    later.selection_intent_generation = Some(runtime.selector_reconcile.intent_generation());
    let later = runtime.remember_runtime_result(later);
    assert_original_native_debt(&later, &pending);
    assert_eq!(
        later.selection_intent_generation,
        Some(runtime.selector_reconcile.intent_generation()),
        "native debt retains its own original intent without relabeling the current result"
    );
}

#[test]
fn native_failure_requires_exact_unknown_listener_code_and_inbounds_path() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    for change in 0..4 {
        let mut operation = listener_retirement_receipt(&pending, "UNKNOWN");
        match change {
            0 => operation.retirement_state = "PENDING".into(),
            1 => operation.errors.clear(),
            2 => operation.errors[0].code = "OTHER_RETIREMENT_FAILURE".into(),
            _ => operation.errors[0].effective_path = "outbounds".into(),
        }
        let receipt = RuntimeKernelReceipt {
            generation: pending.generation,
            intent: pending.intent,
            publication: RuntimeReceiptPublication::Current,
            operation,
        };
        let result = receipt.result();
        assert_eq!(result.status, "hotApplied", "proof axis {change}");
        assert!(result.ok);
        assert!(result.native_retirements.is_empty());
    }
}

#[tokio::test]
async fn superseded_regeneration_owner_still_retains_original_native_failure() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    runtime
        .config
        .save_full(&runnable_config_fixture())
        .unwrap();
    let saved = runtime.config.current().unwrap();
    let mut obsolete = saved.clone();
    obsolete["logLevel"] = json!("debug");
    let mut pending = ts_pending_raw_fixture(&runtime, obsolete);
    pending.published = true;
    let mut operation = listener_retirement_receipt(&pending, "UNKNOWN");
    operation.durable = false;
    operation.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
    operation.persistence_mode = "app_authoritative_regeneration".into();
    operation.owner_reload_acknowledged = true;
    assert!(runtime.regeneration_owner_superseded(&pending, &operation));
    let result = runtime.settle_runtime_operation(&pending, operation).await;
    let result = runtime.remember_runtime_result(result);
    assert_eq!(result.status, "conflict");
    assert!(result
        .reasons
        .iter()
        .any(|reason| reason.code == "OWNER_STATE_SUPERSEDED"));
    assert_original_native_debt(&result, &pending);
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(
        state.kernel.as_ref().unwrap().publication
            == RuntimeReceiptPublication::RegenerationSuperseded
    );
    assert!(state.pending.is_none());
    assert_eq!(
        state.native_retirements[&pending.plan.transaction_id]
            .receipt
            .intent,
        pending.intent
    );
}

#[test]
fn native_observation_capacity_preserves_unsettled_owners_and_only_reclaims_drained_slots() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    let mut state = RuntimeConfigApplyState::default();
    for index in 0..NATIVE_RETIREMENT_OBSERVATION_LIMIT {
        let mut owner = pending.clone();
        owner.plan.transaction_id = format!("native-owner-{index}");
        let operation =
            listener_retirement_receipt(&owner, if index == 0 { "UNKNOWN" } else { "PENDING" });
        assert!(state.observe_native_retirement(&owner, &operation));
    }
    assert!(!state.native_retirement_capacity_available());
    assert_eq!(
        state.native_retirements.len(),
        NATIVE_RETIREMENT_OBSERVATION_LIMIT
    );
    assert_eq!(state.native_retirement_debts().len(), 1);
    let first = state.native_retirements["native-owner-0"]
        .receipt
        .operation
        .clone();
    let drained = state.native_retirements["native-owner-1"].clone();
    let mut operation = drained.receipt.operation.clone();
    operation.retirement_state = "DRAINED".into();
    assert!(state
        .refresh_native_retirement(&drained, &native_retirement_status(operation))
        .is_some());
    assert!(state.native_retirement_capacity_available());
    let mut next = pending.clone();
    next.plan.transaction_id = "next-native-owner".into();
    assert!(state.observe_native_retirement(&next, &listener_retirement_receipt(&next, "PENDING")));
    assert_eq!(
        state.native_retirements.len(),
        NATIVE_RETIREMENT_OBSERVATION_LIMIT
    );
    assert!(!state.native_retirements.contains_key("native-owner-1"));
    assert_eq!(
        state.native_retirements["native-owner-0"].receipt.operation,
        first
    );
}

#[tokio::test]
async fn late_same_owner_drained_settlement_cannot_launder_cached_first_unknown() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    pending.published = true;
    let first = listener_retirement_receipt(&pending, "UNKNOWN");
    assert_original_native_debt(
        &runtime
            .settle_runtime_operation(&pending, first.clone())
            .await,
        &pending,
    );
    let late = listener_retirement_receipt(&pending, "DRAINED");
    let result = runtime.settle_runtime_operation(&pending, late).await;
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.core_state, Some("committed"));
    assert_eq!(result.configuration_durable, Some(true));
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert_eq!(
        state.native_retirements[&pending.plan.transaction_id]
            .receipt
            .operation,
        first
    );
    let cached = state
        .core_receipt_for(&pending.plan.transaction_id, pending.generation)
        .unwrap();
    assert_original_native_debt(&cached, &pending);
    assert_eq!(cached.status, "nativeCleanupUnknown");
    assert_eq!(cached.core_state, Some("committed"));
    assert_eq!(cached.configuration_durable, Some(true));
}

#[tokio::test]
async fn verified_later_durability_updates_d_without_discharging_first_native_unknown() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    pending.published = true;
    let mut first = listener_retirement_receipt(&pending, "UNKNOWN");
    first.durable = false;
    first.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
    let result = runtime
        .settle_runtime_operation(&pending, first.clone())
        .await;
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.configuration_durable, Some(false));
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .is_some());
    let result = runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "DRAINED"))
        .await;
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.configuration_durable, Some(true));
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.pending.is_none());
    let original = &state.native_retirements[&pending.plan.transaction_id].receipt;
    assert_eq!(original.intent, pending.intent);
    assert_eq!(original.generation, pending.generation);
    assert!(original.operation.durable);
    assert_eq!(
        original.operation.state,
        daemon::RuntimeOperationState::Committed as i32
    );
    assert_eq!(original.operation.retirement_state, "UNKNOWN");
    assert_eq!(original.operation.errors, first.errors);
    let cached = state
        .core_receipt_for(&pending.plan.transaction_id, pending.generation)
        .unwrap();
    assert_original_native_debt(&cached, &pending);
    assert_eq!(cached.configuration_durable, Some(true));
}

#[tokio::test]
async fn successor_saved_desired_and_committed_kernel_keep_the_original_native_owner() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut original = ts_pending_raw_fixture(&runtime, runnable_config_fixture());
    original.published = true;
    let first = listener_retirement_receipt(&original, "UNKNOWN");
    runtime
        .settle_runtime_operation(&original, first.clone())
        .await;
    let mut desired = original.desired.clone();
    desired["logLevel"] = json!("debug");
    runtime.config.save_full(&desired).unwrap();
    runtime.selector_reconcile.register_intent();
    let mut successor = ts_pending_raw_fixture(&runtime, runtime.config.current().unwrap());
    successor.published = true;
    successor.plan.transaction_id = "successor-durable-core-owner".into();
    successor.plan.expected = first.active.clone();
    successor.plan.expected_control_revision = first.control_revision;
    let result = runtime
        .settle_runtime_operation(
            &successor,
            listener_retirement_receipt(&successor, "DRAINED"),
        )
        .await;
    let result = runtime.remember_runtime_result(result);
    assert_original_native_debt(&result, &original);
    assert_eq!(result.core_state, Some("committed"));
    assert_eq!(result.configuration_durable, Some(true));
    assert_eq!(runtime.config.current().unwrap(), successor.desired);
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert_eq!(
        state.kernel.as_ref().unwrap().operation.transaction_id,
        successor.plan.transaction_id
    );
    let cached = state
        .core_receipt_for(&successor.plan.transaction_id, successor.generation)
        .unwrap();
    assert_original_native_debt(&cached, &original);
    assert_eq!(
        cached.transaction_id.as_deref(),
        Some(successor.plan.transaction_id.as_str())
    );
    assert_eq!(cached.selection_intent_generation, Some(successor.intent));
    assert_ne!(cached.selection_intent_generation, Some(original.intent));
    assert_eq!(
        state.native_retirements[&original.plan.transaction_id]
            .receipt
            .operation,
        first
    );
}

#[tokio::test]
async fn native_unknown_status_with_independent_d_pending_never_acknowledges_persistence() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    runtime
        .config
        .save_full(&runnable_config_fixture())
        .unwrap();
    let mut pending = ts_pending_raw_fixture(&runtime, runtime.config.current().unwrap());
    pending.published = true;
    let mut operation = listener_retirement_receipt(&pending, "UNKNOWN");
    operation.durable = false;
    operation.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
    operation.persistence_mode = "app_authoritative_regeneration".into();
    operation.owner_reload_acknowledged = true;
    let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, operation.clone()).await;
    runtime.settle_runtime_operation(&pending, operation).await;
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .is_some());
    let result = runtime
        .runtime_native_retirement_status(pending.plan.transaction_id.clone())
        .await;
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.configuration_durable, Some(false));
    assert_eq!(result.selection_intent_generation, Some(pending.intent));
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .is_some());
    assert!(
        rpc.requests.lock().unwrap().is_empty(),
        "first UNKNOWN requires no further Close/status attempt"
    );
    assert!(rpc.acknowledgements.lock().unwrap().is_empty());
    assert_eq!(
        rpc.mutations.load(Ordering::SeqCst),
        0,
        "checking native cleanup cannot retry or ACK the separate D debt"
    );
    assert!(matches!(
        ProxyRuntime::runtime_result_outcome(&result),
        SwitchOutcome::Pending
    ));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn generic_original_d_status_acknowledges_only_its_full_saved_owner_and_keeps_native_unknown()
{
    for superseding_desired in [false, true] {
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        runtime
            .config
            .save_full(&runnable_config_fixture())
            .unwrap();
        let saved = runtime.config.current().unwrap();
        let mut pending = ts_pending_raw_fixture(&runtime, saved.clone());
        pending.published = true;
        let mut not_ready = listener_retirement_receipt(&pending, "UNKNOWN");
        not_ready.durable = false;
        not_ready.state = daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
        not_ready.persistence_mode = "app_authoritative_regeneration".into();
        not_ready.owner_reload_acknowledged = false;
        let (rpc, server) = native_retirement_rpc_peer(&runtime, &pending, not_ready.clone()).await;
        runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
        // Reach the actual generic status -> original owner acknowledgement
        // helper -> settlement before provider readiness, rather than seeding
        // the native account through a separate test state machine.
        let initial = runtime
            .runtime_apply_status(pending.plan.transaction_id.clone())
            .await;
        assert_original_native_debt(&initial, &pending);
        assert_eq!(initial.configuration_durable, Some(false));
        assert!(runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .pending
            .is_some());
        assert!(rpc.acknowledgements.lock().unwrap().is_empty());
        assert!(!rpc.requests.lock().unwrap().is_empty());
        let owner_runtime = Arc::clone(&runtime);
        let owner_desired = saved.clone();
        let (digest, revision) = tokio::task::spawn_blocking(move || {
            owner_runtime
                .config
                .with_durable_runtime_owner(&owner_desired, |digest, revision| (digest, revision))
        })
        .await
        .unwrap()
        .unwrap();
        let mut ready = not_ready.clone();
        ready.owner_reload_acknowledged = true;
        *rpc.status.lock().unwrap() = native_retirement_status(ready.clone());
        let mut acknowledged = ready.clone();
        acknowledged.durable = true;
        acknowledged.state = daemon::RuntimeOperationState::Committed as i32;
        acknowledged.desired_state_digest = digest.clone();
        acknowledged.desired_state_revision = revision.clone();
        *rpc.acknowledgement_response.lock().unwrap() = Some(acknowledged);
        if superseding_desired {
            let mut newer = saved.clone();
            newer["logLevel"] = json!("debug");
            runtime.config.save_full(&newer).unwrap();
            assert_ne!(runtime.config.current().unwrap(), saved);
        }
        let completed = runtime
            .runtime_apply_status(pending.plan.transaction_id.clone())
            .await;
        assert_original_native_debt(&completed, &pending);
        assert_eq!(
            completed.transaction_id.as_deref(),
            Some(pending.plan.transaction_id.as_str())
        );
        assert_eq!(completed.runtime_generation, Some(pending.generation));
        assert_eq!(completed.selection_intent_generation, Some(pending.intent));
        assert_eq!(completed.core_state, Some("committed"));
        {
            let acknowledgements = rpc.acknowledgements.lock().unwrap();
            if superseding_desired {
                assert!(
                    acknowledgements.is_empty(),
                    "original A cannot acknowledge durability using newer saved B evidence"
                );
                assert_eq!(completed.configuration_durable, Some(false));
                assert_eq!(completed.status, "conflict");
                assert!(completed
                    .reasons
                    .iter()
                    .any(|reason| reason.code == "OWNER_STATE_SUPERSEDED"));
            } else {
                assert_eq!(
                    acknowledgements.len(),
                    1,
                    "generic original D status must retain its automatic original-owner ACK"
                );
                let request = &acknowledgements[0];
                assert_eq!(request.contract_version, 1);
                assert_eq!(request.transaction_id, pending.plan.transaction_id);
                assert_eq!(request.active, ready.active);
                assert_eq!(request.control_revision, ready.control_revision);
                assert_eq!(request.persistence_mode, "app_authoritative_regeneration");
                assert_eq!(request.desired_state_digest, digest);
                assert_eq!(request.desired_state_revision, revision);
                assert_eq!(completed.configuration_durable, Some(true));
                assert_eq!(completed.status, "nativeCleanupUnknown");
                assert!(runtime
                    .runtime_config_apply
                    .lock()
                    .unwrap()
                    .pending
                    .is_none());
                let state = runtime.runtime_config_apply.lock().unwrap();
                let original = &state.native_retirements[&pending.plan.transaction_id].receipt;
                assert!(original.operation.durable);
                assert_eq!(original.operation.retirement_state, "UNKNOWN");
                assert_eq!(original.operation.errors, not_ready.errors);
            }
        }
        assert_eq!(
            rpc.mutations.load(Ordering::SeqCst),
            0,
            "status recovery must never Plan/Apply/retry or send an unconfigured ACK"
        );
        for request in rpc.requests.lock().unwrap().iter() {
            assert_eq!(request.transaction_id, pending.plan.transaction_id);
            assert_eq!(
                request.runtime_incarnation,
                pending.plan.expected.as_ref().unwrap().incarnation
            );
        }
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn native_unknown_does_not_authorize_publication_without_its_exact_baseline() {
    for mismatch in ["missing", "generation", "pairing"] {
        let (runtime, _dir, calls) = crate::runtime::proxy::tests::test_runtime_recording();
        runtime
            .config
            .save_full(&runnable_config_fixture())
            .unwrap();
        let running = runtime.config.current().unwrap();
        let original = ts_pending_raw_fixture(&runtime, running.clone());
        runtime.capture_runtime_config_baseline(
            original.generation,
            original.deps.clone(),
            original.generated.clone(),
            original.typed.clone(),
            original.pairing.clone(),
        );
        *runtime.current_config.write().unwrap() = Some(running.clone());
        *runtime.startup_snapshot.write().unwrap() = Some(running.clone());
        let mut desired = running.clone();
        desired["logLevel"] = json!("debug");
        runtime.config.save_full(&desired).unwrap();
        let pending = ts_pending_raw_fixture(&runtime, runtime.config.current().unwrap());
        assert_ne!(pending.desired, running);
        assert!(pending.deletion_scope.is_empty());
        assert!(pending.host_plan.is_none());
        assert!(!pending.published);
        {
            let mut baseline = runtime.runtime_config_baseline.write().unwrap();
            let actual = baseline.as_ref().unwrap();
            assert_eq!(actual.generation, pending.generation);
            assert_eq!(actual.pairing, pending.pairing);
            match mismatch {
                "missing" => *baseline = None,
                "generation" => baseline.as_mut().unwrap().generation += 1,
                "pairing" => {
                    baseline
                        .as_mut()
                        .unwrap()
                        .pairing
                        .implementation_manifest_digest = "b".repeat(64)
                }
                _ => unreachable!(),
            }
        }
        runtime.runtime_config_apply.lock().unwrap().pending = Some(pending.clone());
        let result = runtime
            .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"))
            .await;
        let result = runtime.remember_runtime_result(result);
        assert_original_native_debt(&result, &pending);
        assert_eq!(result.status, "unconfirmed", "{mismatch}");
        assert_eq!(result.core_state, Some("committed"));
        assert_eq!(result.configuration_durable, Some(true));
        assert!(result
            .reasons
            .iter()
            .any(|reason| reason.code == "RUNTIME_PUBLICATION_UNCONFIRMED"));
        assert!(runtime.restart_deferred.load(Ordering::SeqCst));
        assert_eq!(runtime.current_config_snapshot(), Some(running.clone()));
        assert_eq!(*runtime.startup_snapshot.read().unwrap(), Some(running));
        assert_eq!(runtime.config.current().unwrap(), pending.desired);
        assert!(result.host_effects.is_empty());
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.pending.is_some());
        assert!(!state.host.has_in_flight());
        assert_eq!(state.native_retirements.len(), 1);
        assert_original_native_debt(
            &state
                .core_receipt_for(&pending.plan.transaction_id, pending.generation)
                .unwrap(),
            &pending,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

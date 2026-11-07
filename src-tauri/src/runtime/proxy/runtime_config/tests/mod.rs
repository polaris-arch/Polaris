use super::*;
use polaris_config_engine::user_config::dns_constants::DIRECT_SERVER_ID;
use serde_json::json;
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
        "    pub(super) async fn apply_runtime_candidate(",
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
    let body = crate::commands::guard_scan::impl_method_body(&source, "    fn restart_choice(");
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
    let result = rt.host_effect_restart_choice(&candidate, &host_plan, generation, intent);
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

#[test]
fn stale_host_dns_restart_choices_preserve_current_state_and_debt() {
    for stale in ["disk", "running", "generation", "intent", "stopped", "busy"] {
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
            "disk" => {
                let mut newer = candidate.clone();
                newer["logLevel"] = "warn".into();
                rt.config.save_full(&newer).unwrap();
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
        deletion_scope: vec![],
        physical_route_config: typed.clone(),
        protected_physical_root: None,
        protected_binding_plan: None,
        generation,
        intent: rt.selector_reconcile.intent_generation(),
        candidate: raw,
        typed,
        generated,
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
        superseded: false,
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
    state.kernel.as_mut().unwrap().superseded = true;
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

#[test]
fn stale_intent_keeps_verified_core_fact_without_publishing_or_rebinding() {
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
    let result = runtime.settle_runtime_operation(&pending, operation);
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
            superseded: false,
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
    let receipt = daemon::RuntimeConfigOperation {
        contract_version: 1,
        transaction_id: pending.plan.transaction_id.clone(),
        candidate_digest: pending.plan.candidate_digest.clone(),
        impact_digest: pending.plan.impact_digest.clone(),
        before: pending.plan.expected.clone(),
        active: pending.plan.expected.clone(),
        control_revision: 7,
        state: daemon::RuntimeOperationState::RejectedUnapplied as i32,
        ..Default::default()
    };
    let mut unknown = receipt.clone();
    unknown.candidate_digest = "wrong".into();
    assert_eq!(
        rt.settle_runtime_operation(&pending, unknown).status,
        "unconfirmed"
    );
    assert!(rt.tailnet_context().unwrap().suspended);
    let mut contradictory = receipt.clone();
    contradictory.active.as_mut().unwrap().effective_digest = "changed".into();
    assert_eq!(
        rt.settle_runtime_operation(&pending, contradictory).status,
        "unconfirmed"
    );
    assert!(rt.tailnet_context().unwrap().suspended);
    assert_eq!(
        rt.settle_runtime_operation(&pending, receipt).status,
        "rejected"
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
        rt.settle_runtime_operation(&pending, receipt).status,
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
        rt.settle_runtime_operation(&pending, receipt).status,
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

fn committed_ts_receipt(pending: &PendingOperation) -> daemon::RuntimeConfigOperation {
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
                superseded: false,
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
            .is_none());
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
            superseded: false,
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
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"));
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
            let result = runtime.settle_runtime_operation(&pending, operation);
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
        superseded: false,
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
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"));
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
    runtime.settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"));
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
    runtime.settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"));
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

#[derive(Clone)]
struct NativeRetirementRpcFixture {
    capabilities: Arc<std::sync::Mutex<daemon::RuntimeCapabilities>>,
    status: Arc<std::sync::Mutex<daemon::RuntimeConfigStatus>>,
    requests: Arc<std::sync::Mutex<Vec<daemon::RuntimeConfigStatusRequest>>>,
    mutations: Arc<std::sync::atomic::AtomicUsize>,
    acknowledgement_response: Arc<std::sync::Mutex<Option<daemon::RuntimeConfigOperation>>>,
    acknowledgements: Arc<std::sync::Mutex<Vec<daemon::RuntimePersistenceAcknowledgementRequest>>>,
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
    async fn get_runtime_capabilities(
        &self,
        _: polaris_singbox_grpc::tonic::Request<()>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeCapabilities>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Ok(polaris_singbox_grpc::tonic::Response::new(
            self.capabilities.lock().unwrap().clone(),
        ))
    }
    async fn plan_runtime_config(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::PlanRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigPlan>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(self.forbidden())
    }
    async fn apply_runtime_config(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::ApplyRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(self.forbidden())
    }
    async fn get_runtime_config_status(
        &self,
        request: polaris_singbox_grpc::tonic::Request<daemon::RuntimeConfigStatusRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigStatus>,
        polaris_singbox_grpc::tonic::Status,
    > {
        self.requests.lock().unwrap().push(request.into_inner());
        Ok(polaris_singbox_grpc::tonic::Response::new(
            self.status.lock().unwrap().clone(),
        ))
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
        mutations: Arc::default(),
        acknowledgement_response: Arc::default(),
        acknowledgements: Arc::default(),
    };
    let incoming = futures::stream::unfold(listener, |listener| async move {
        let next = listener.accept().await.map(|(stream, _)| stream);
        Some((next, listener))
    });
    let service = fixture.clone();
    let server = tokio::spawn(async move {
        polaris_singbox_grpc::tonic::transport::Server::builder()
            .serve_with_incoming(
                daemon::runtime_config_service_server::RuntimeConfigServiceServer::new(service),
                incoming,
            )
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
            runtime.settle_runtime_operation(&pending, operation).status,
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
        runtime.settle_runtime_operation(&pending, operation);
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
    runtime.settle_runtime_operation(&pending, operation.clone());
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
    let noop_result = runtime.settle_runtime_operation(&next, noop);
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
            superseded: false,
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
    let result = runtime.settle_runtime_operation(&pending, operation);
    let result = runtime.remember_runtime_result(result);
    assert_eq!(result.status, "conflict");
    assert!(result
        .reasons
        .iter()
        .any(|reason| reason.code == "OWNER_STATE_SUPERSEDED"));
    assert_original_native_debt(&result, &pending);
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.kernel.as_ref().unwrap().superseded);
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
        &runtime.settle_runtime_operation(&pending, first.clone()),
        &pending,
    );
    let late = listener_retirement_receipt(&pending, "DRAINED");
    let result = runtime.settle_runtime_operation(&pending, late);
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
    let result = runtime.settle_runtime_operation(&pending, first.clone());
    assert_original_native_debt(&result, &pending);
    assert_eq!(result.configuration_durable, Some(false));
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .is_some());
    let result = runtime
        .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "DRAINED"));
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
    runtime.settle_runtime_operation(&original, first.clone());
    let mut desired = original.desired.clone();
    desired["logLevel"] = json!("debug");
    runtime.config.save_full(&desired).unwrap();
    runtime.selector_reconcile.register_intent();
    let mut successor = ts_pending_raw_fixture(&runtime, runtime.config.current().unwrap());
    successor.published = true;
    successor.plan.transaction_id = "successor-durable-core-owner".into();
    successor.plan.expected = first.active.clone();
    successor.plan.expected_control_revision = first.control_revision;
    let result = runtime.settle_runtime_operation(
        &successor,
        listener_retirement_receipt(&successor, "DRAINED"),
    );
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
    runtime.settle_runtime_operation(&pending, operation);
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
            .settle_runtime_operation(&pending, listener_retirement_receipt(&pending, "UNKNOWN"));
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

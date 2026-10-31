use super::*;
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
    for change in 0..4 {
        let mut value = paired_capabilities();
        match change {
            0 => value.active = None,
            1 => value.active.as_mut().unwrap().incarnation.clear(),
            2 => value.active.as_mut().unwrap().applied_revision = 0,
            _ => value.active.as_mut().unwrap().effective_digest.clear(),
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
    assert!(!structural_runtime_change(&old, &next));
    next["tunConfig"]["mtu"] = json!(1400);
    assert!(structural_runtime_change(&old, &next));
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

fn ts_pending_fixture(rt: &Arc<ProxyRuntime>, authority: &str) -> PendingOperation {
    let raw = json!({"servers":[{"id":"ts","name":"ts","protocol":"tailscale","tailscaleSettings":{"controlUrl":authority}}],"selectedServerId":"__direct__","proxyMode":"smart","proxyModeType":"manual"});
    ts_pending_raw_fixture(rt, raw)
}

fn ts_pending_raw_fixture(rt: &Arc<ProxyRuntime>, raw: Value) -> PendingOperation {
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
        candidate_digest: "next".into(),
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
        api_handoff: None,
        pairing,
        desired: raw.clone(),
        deletion_scope: vec![],
        physical_route_config: typed.clone(),
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
    json!({"servers":[{"id":"ts","name":"ts","protocol":"tailscale","tailscaleSettings":{"controlUrl":"https://one.example"}}],
        "selectedServerId":"__direct__","proxyMode":"smart","proxyModeType":"manual",
        "meshRouteState":{"schemaVersion":1,"localId":"local","revision":"1",
            "identities":[{"serverId":"ts","identityEpoch":"epoch-one","controlAuthority":"https://one.example","selfStableId":"self-one","bindingState":"bound","evidenceSource":"status"}],
            "observations":[],"reservations":[],"intent":{"revision":"1","desiredRun":"stopped"},"identityEffects":[]}})
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

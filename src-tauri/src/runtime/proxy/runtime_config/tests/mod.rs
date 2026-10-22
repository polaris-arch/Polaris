use super::*;
use serde_json::json;
fn paired_capabilities() -> daemon::RuntimeCapabilities {
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

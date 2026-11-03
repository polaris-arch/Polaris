use super::*;
use polaris_helper_proto::Platform;
use serde_json::json;
use std::sync::Arc;

fn execution(runtime: &Arc<ProxyRuntime>, id: &str) -> HostEffectExecution {
    let generation = runtime.gate.generation();
    {
        let mut status = runtime.status.write().unwrap();
        status.running = true;
        status.main_generation = generation;
    }
    let mut desired = polaris_store::default_config();
    desired["mixedPort"] = json!(2080);
    desired["dnsConfig"]["takeoverSystemDns"] = json!(true);
    let deps = runtime.generate_deps(9090, 0, 0, None, &[], &desired, false);
    runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .seed(generation, deps);
    HostEffectExecution {
        execution_id: id.into(),
        transaction_id: None,
        generation,
        intent: runtime.selector_reconcile.intent_generation(),
        desired,
        core_active: None,
        core_control_revision: 7,
        plan: HostEffectPlan {
            effect_digest: "effect".into(),
            physical_facts_digest: "physical".into(),
            host_epoch: 4,
            strategy: HostEffectStrategy::HostOnly,
            recipe: json!({}),
            effects: [HostEffectKind::Dns, HostEffectKind::SystemProxy]
                .into_iter()
                .map(|kind| HostEffectPreview {
                    kind,
                    scope_digest: "scope".into(),
                    before_digest: "before".into(),
                    target_digest: "after".into(),
                })
                .collect(),
        },
    }
}

fn receipt(execution: &HostEffectExecution, outcomes: [HostEffectOutcome; 2]) -> HostEffectReceipt {
    HostEffectReceipt {
        effect_digest: execution.plan.effect_digest.clone(),
        physical_facts_digest: execution.plan.physical_facts_digest.clone(),
        host_epoch: execution.plan.host_epoch,
        effects: execution
            .plan
            .effects
            .iter()
            .cloned()
            .zip(outcomes)
            .map(|(preview, outcome)| HostEffectResult {
                preview,
                outcome,
                receipt_id: Some("opaque-receipt".into()),
                cleanup_pending: outcome == HostEffectOutcome::Unknown,
                retryable: outcome == HostEffectOutcome::FailedRestored,
                reason_code: None,
            })
            .collect(),
    }
}

#[test]
fn partial_host_receipts_preserve_core_and_only_advance_the_accepted_kind() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let execution = execution(&runtime, "first");
    let running =
        json!({"mixedPort":1080,"dnsConfig":{"takeoverSystemDns":false,"coreResolver":"old"}});
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &running));
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    // R_core may already be D. Host failure must not roll it back or hide debt.
    *runtime.current_config.write().unwrap() = Some(execution.desired.clone());
    runtime.record_runtime_host_effect_result(
        &execution,
        receipt(
            &execution,
            [
                HostEffectOutcome::Applied,
                HostEffectOutcome::FailedRestored,
            ],
        ),
    );
    assert!(runtime.host_effect_pending());
    assert!(runtime.current_config_snapshot().as_ref() == Some(&execution.desired));
    let accepted = runtime
        .runtime_accepted_host_config(execution.generation)
        .unwrap();
    assert_eq!(accepted["mixedPort"], 1080);
    assert_eq!(accepted["dnsConfig"]["takeoverSystemDns"], true);
    assert_eq!(accepted["dnsConfig"]["coreResolver"], "old");
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert!(
        !runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .host
            .current
            .as_ref()
            .unwrap()
            .in_flight
    );
}

#[test]
fn late_and_duplicate_host_callbacks_cannot_accept_a_successor() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let first = execution(&runtime, "first");
    let running = json!({"mixedPort":1080,"dnsConfig":{"takeoverSystemDns":false}});
    assert!(runtime.begin_runtime_host_execution(first.clone(), &running));
    let mut next = first.clone();
    next.execution_id = "next".into();
    assert!(!runtime.begin_runtime_host_execution(next.clone(), &running));
    runtime.record_runtime_host_effect_result(
        &first,
        receipt(&first, [HostEffectOutcome::FailedRestored; 2]),
    );
    assert!(runtime.begin_runtime_host_execution(next.clone(), &running));
    runtime.record_runtime_host_effect_result(
        &first,
        receipt(&first, [HostEffectOutcome::Applied; 2]),
    );
    assert!(runtime.host_effect_pending());
    assert_eq!(
        runtime
            .runtime_accepted_host_config(first.generation)
            .unwrap(),
        running
    );
    runtime
        .record_runtime_host_effect_result(&next, receipt(&next, [HostEffectOutcome::Applied; 2]));
    assert!(!runtime.host_effect_pending());
    runtime
        .record_runtime_host_effect_result(&next, receipt(&next, [HostEffectOutcome::Unknown; 2]));
    assert!(!runtime.host_effect_pending());
}

#[test]
fn changed_intent_and_mismatched_receipts_never_advance_host_acceptance() {
    for mismatch in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let execution = execution(&runtime, "owned");
        let running = json!({"mixedPort":1080,"dnsConfig":{"takeoverSystemDns":false}});
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &running));
        let mut result = receipt(&execution, [HostEffectOutcome::Applied; 2]);
        if mismatch {
            result.physical_facts_digest = "foreign".into();
        } else {
            runtime.register_selector_intent();
        }
        runtime.record_runtime_host_effect_result(&execution, result);
        assert!(runtime.host_effect_pending());
        assert_eq!(
            runtime
                .runtime_accepted_host_config(execution.generation)
                .unwrap(),
            running
        );
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
    }
}

#[test]
fn host_poll_completion_does_not_clear_independent_persistence_debt() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let execution = execution(&runtime, "host-only");
    assert!(execution.transaction_id.is_none());
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let mut result = RuntimeApplyResult::status("hotDurabilityPending");
    result.core_state = Some("committed");
    result.configuration_durable = Some(false);
    result.runtime_generation = Some(execution.generation);
    runtime.annotate_runtime_host_result(&mut result);
    assert_eq!(result.status, "hostEffectsPending");
    runtime.record_runtime_host_effect_result(
        &execution,
        receipt(&execution, [HostEffectOutcome::Applied; 2]),
    );
    runtime.annotate_runtime_host_result(&mut result);
    assert_eq!(result.status, "hotDurabilityPending");
    assert_eq!(result.configuration_durable, Some(false));
    assert!(result.pending_apply);
}

#[test]
fn unrelated_failed_requests_cannot_erase_the_host_execution_result() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let execution = execution(&runtime, "retained-host");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let mut initial = RuntimeApplyResult::status("hotApplied");
    initial.core_state = Some("unchanged");
    initial.configuration_durable = Some(true);
    initial.runtime_generation = Some(execution.generation);
    initial.selection_intent_generation = Some(execution.intent);
    runtime.remember_runtime_result(initial);
    runtime.remember_runtime_result(RuntimeApplyResult::status("conflict"));
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert_eq!(state.last.as_ref().unwrap().status, "conflict");
    let retained = state
        .host
        .current
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap();
    assert_eq!(retained.host_execution_id.as_deref(), Some("retained-host"));
    assert_eq!(retained.status, "hostEffectsPending");
    assert!(retained.transaction_id.is_none());
}

#[test]
fn original_core_durability_and_host_only_execution_keep_separate_owners() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let mut execution = execution(&runtime, "host-retry");
    let identity = super::super::tests::paired_capabilities().active;
    execution.core_active = identity.clone();
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        state.kernel = Some(super::super::RuntimeKernelReceipt {
            generation: execution.generation,
            intent: execution.intent,
            superseded: false,
            operation: daemon::RuntimeConfigOperation {
                transaction_id: "original-core".into(),
                active: identity.clone(),
                state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32,
                durable: false,
                ..Default::default()
            },
        });
    }
    let mut result = RuntimeApplyResult::status("hotApplied");
    result.core_state = Some("unchanged");
    result.configuration_durable = Some(true);
    result.runtime_generation = Some(execution.generation);
    runtime.annotate_runtime_host_result(&mut result);
    assert_eq!(result.configuration_durable, Some(false));
    assert_eq!(
        result.persistence_transaction_id.as_deref(),
        Some("original-core")
    );
    assert!(result.transaction_id.is_none());
    runtime.record_runtime_host_effect_result(
        &execution,
        receipt(&execution, [HostEffectOutcome::FailedRestored; 2]),
    );
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        let kernel = state.kernel.as_mut().unwrap();
        kernel.operation.durable = true;
        kernel.operation.state = daemon::RuntimeOperationState::Committed as i32;
        let authority = kernel.result();
        assert_eq!(authority.transaction_id.as_deref(), Some("original-core"));
        assert_eq!(
            authority.selection_intent_generation,
            Some(execution.intent)
        );
    }
    runtime.annotate_runtime_host_result(&mut result);
    assert_eq!(result.configuration_durable, Some(true));
    assert!(result.persistence_transaction_id.is_none());
    assert_eq!(result.status, "hostEffectsFailed");
    assert!(result.pending_apply);
    // The original Core's receipt may update durability, but does not
    // turn this later HostOnly execution into a new Core publication.
    result.transaction_id = Some("original-core".into());
    result.core_state = Some("committed");
    result.selection_intent_generation = Some(execution.intent);
    let mut state = runtime.runtime_config_apply.lock().unwrap();
    state.host.remember_result(&result);
    let host_result = state
        .host
        .current
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap();
    assert!(host_result.transaction_id.is_none());
    assert_eq!(host_result.core_state, Some("unchanged"));
    assert_eq!(host_result.configuration_durable, Some(true));
}
#[test]
fn native_listener_owner_survives_host_success_failure_and_absent_host_paths() {
    for outcome in [
        HostEffectOutcome::Applied,
        HostEffectOutcome::FailedRestored,
        HostEffectOutcome::Unknown,
    ] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let execution = execution(&runtime, "successor-host-B");
        let generation = execution.generation;
        let intent = execution.intent;
        let pairing =
            super::super::observe_runtime_pairing(&super::super::tests::paired_capabilities())
                .unwrap();
        let operation = daemon::RuntimeConfigOperation {
            transaction_id: "original-native-A".into(),
            state: daemon::RuntimeOperationState::Committed as i32,
            durable: true,
            retirement_state: "UNKNOWN".into(),
            errors: vec![daemon::RuntimePlanError {
                code: "LISTENER_RETIREMENT_FAILED".into(),
                effective_path: "inbounds".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .native_retirements
            .insert(
                operation.transaction_id.clone(),
                super::super::NativeRetirementObservation {
                    pairing,
                    receipt: super::super::RuntimeKernelReceipt {
                        generation,
                        intent,
                        superseded: false,
                        operation,
                    },
                },
            );
        // No host owner used to short-circuit annotation before the native
        // projection could run. Preserve the successor result's own K/D.
        let mut no_host = RuntimeApplyResult::status("hotApplied");
        runtime.annotate_runtime_host_result(&mut no_host);
        assert_eq!(no_host.status, "nativeCleanupUnknown");
        assert!(!no_host.ok);
        assert_eq!(
            no_host.native_retirements[0].transaction_id,
            "original-native-A"
        );
        assert!(no_host.transaction_id.is_none());
        assert!(no_host.configuration_durable.is_none());
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        let mut result = RuntimeApplyResult::status("hotApplied");
        result.core_state = Some("unchanged");
        result.configuration_durable = Some(true);
        result.runtime_generation = Some(generation);
        result.selection_intent_generation = Some(intent);
        runtime.annotate_runtime_host_result(&mut result);
        assert_eq!(result.status, "hostEffectsPending");
        assert!(result.pending_apply);
        assert!(!result.ok);
        runtime.record_runtime_host_effect_result(&execution, receipt(&execution, [outcome; 2]));
        runtime.annotate_runtime_host_result(&mut result);
        assert_eq!(result.configuration_durable, Some(true));
        assert!(result.transaction_id.is_none());
        assert_eq!(
            result.host_execution_id.as_deref(),
            Some("successor-host-B")
        );
        assert_eq!(result.native_retirements.len(), 1);
        assert_eq!(
            result.native_retirements[0].transaction_id,
            "original-native-A"
        );
        assert_eq!(result.native_retirements[0].runtime_generation, generation);
        assert_eq!(
            result.native_retirements[0].selection_intent_generation,
            intent
        );
        assert!(result.pending_apply);
        assert!(!result.ok);
        assert_eq!(
            result.status,
            if outcome == HostEffectOutcome::Applied {
                "nativeCleanupUnknown"
            } else {
                "hostEffectsFailed"
            }
        );
        assert!(runtime.native_retirement_pending());
    }
}

#[derive(Default)]
struct LedgerDnsFaults {
    fail_marker_reads: std::sync::atomic::AtomicBool,
    fail_marker_after_write: std::sync::atomic::AtomicBool,
}
struct LedgerDnsOps {
    values: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>>,
    faults: Arc<LedgerDnsFaults>,
}
impl polaris_system_integration::dns_ops::SystemDnsOps for LedgerDnsOps {
    fn takeover_supported(&self) -> bool {
        true
    }
    fn list_targets(
        &self,
    ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError> {
        Ok(self.values.lock().unwrap().keys().cloned().collect())
    }
    fn read_dns(
        &self,
        name: &str,
    ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError> {
        self.values
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| {
                polaris_system_integration::error::SystemIntegrationError::dns("missing service")
            })
    }
    fn apply_dns(
        &self,
        name: &str,
        values: &[String],
    ) -> Result<(), polaris_system_integration::error::SystemIntegrationError> {
        self.values
            .lock()
            .unwrap()
            .insert(name.into(), values.into());
        if self
            .faults
            .fail_marker_after_write
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.faults
                .fail_marker_reads
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    fn read_effective_resolvers(
        &self,
    ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError> {
        Ok(vec!["192.168.1.1".into()])
    }
}
#[derive(Default)]
struct LedgerDnsFs {
    raw: std::sync::Mutex<Option<String>>,
    faults: Arc<LedgerDnsFaults>,
}
impl polaris_system_integration::proxy::MarkerFs for LedgerDnsFs {
    fn write_marker(&self, _: &str, raw: &str) -> std::io::Result<()> {
        *self.raw.lock().unwrap() = Some(raw.into());
        Ok(())
    }
    fn read_marker(&self, _: &str) -> Option<String> {
        self.raw.lock().unwrap().clone()
    }
    fn read_marker_checked(&self, path: &str) -> std::io::Result<Option<String>> {
        if self
            .faults
            .fail_marker_reads
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(std::io::Error::other("injected exact-marker read failure"))
        } else {
            Ok(self.read_marker(path))
        }
    }
    fn remove_marker(&self, _: &str) -> std::io::Result<()> {
        *self.raw.lock().unwrap() = None;
        Ok(())
    }
}
type LedgerDnsValues = Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>>;
type LedgerDnsController =
    polaris_system_integration::dns_ops::SystemDnsController<LedgerDnsOps, LedgerDnsFs>;
fn dns_execution_fixture(
    runtime: &Arc<ProxyRuntime>,
    id: &str,
) -> (
    HostEffectExecution,
    LedgerDnsController,
    polaris_system_integration::dns_ops::PreparedDnsEffect,
    LedgerDnsValues,
) {
    let (execution, controller, prepared, values, _) =
        dns_execution_fixture_with_faults(runtime, id);
    (execution, controller, prepared, values)
}
fn dns_execution_fixture_with_faults(
    runtime: &Arc<ProxyRuntime>,
    id: &str,
) -> (
    HostEffectExecution,
    LedgerDnsController,
    polaris_system_integration::dns_ops::PreparedDnsEffect,
    LedgerDnsValues,
    Arc<LedgerDnsFaults>,
) {
    let mut execution = execution(runtime, id);
    execution.desired["proxyModeType"] = json!("tun");
    let values = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::from([(
        "A".into(),
        vec!["192.168.1.1".into()],
    )])));
    let faults = Arc::new(LedgerDnsFaults::default());
    let controller = polaris_system_integration::dns_ops::SystemDnsController::new(
        LedgerDnsOps {
            values: values.clone(),
            faults: faults.clone(),
        },
        polaris_system_integration::dns_ops::DnsMarker::new(
            LedgerDnsFs {
                raw: Default::default(),
                faults: faults.clone(),
            },
            "/ledger-dns.json",
        ),
    );
    let prepared = controller.prepare_effect(true).unwrap();
    let digest = |value: Value| polaris_updater::sha256_hex(value.to_string().as_bytes());
    execution.plan.effects = vec![HostEffectPreview {
        kind: HostEffectKind::Dns,
        scope_digest: "exact-DNS-scope".into(),
        before_digest: digest(serde_json::to_value(prepared.before()).unwrap()),
        target_digest: digest(serde_json::to_value(prepared.target()).unwrap()),
    }];
    execution.plan.recipe = json!({"generation":execution.generation,"intent":execution.intent,
        "desired_digest":digest(execution.desired.clone()),"physical":null,"dns":serde_json::to_value(&prepared).unwrap(),"linux_dns":null,"proxy":null,"proxy_port":null});
    *runtime.current_config.write().unwrap() = Some(execution.desired.clone());
    (execution, controller, prepared, values, faults)
}
fn actual_dns_receipt(execution: &HostEffectExecution, leaf: TakeoverReceipt) -> HostEffectReceipt {
    HostEffectReceipt {
        effect_digest: execution.plan.effect_digest.clone(),
        physical_facts_digest: execution.plan.physical_facts_digest.clone(),
        host_epoch: execution.plan.host_epoch,
        effects: vec![super::super::super::dns_takeover::host_result(
            execution.plan.effects[0].clone(),
            &execution.execution_id,
            leaf,
        )],
    }
}

#[test]
fn known_dns_marker_io_debt_settles_after_exact_restore_without_accepting_policy() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values, faults) =
        dns_execution_fixture_with_faults(&runtime, "known-unreadable-E");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    faults
        .fail_marker_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut pending = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(pending.outcome, TakeoverOutcome::RecoveryPending);
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
    pending.marker_id = controller.exact_marker_operation_id();
    assert!(
        pending.marker_id.is_none(),
        "actual post-write marker IO failed"
    );
    let receipt = actual_dns_receipt(&execution, pending);
    assert!(receipt.effects[0].cleanup_pending);
    assert!(receipt.effects[0].receipt_id.is_none());
    runtime.record_runtime_host_effect_result(&execution, receipt.clone());
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(!state.host.dns_unknown_cleanup);
        let owner = state.host.dns_unknown_owner.as_ref().unwrap();
        assert_eq!(owner.execution_id, execution.execution_id);
        assert_eq!(owner.generation, execution.generation);
        assert_eq!(owner.plan.recipe, execution.plan.recipe);
    }
    runtime.record_runtime_dns_restore_result("foreign", true);
    runtime.record_runtime_dns_restore_result(&execution.execution_id, false);
    assert!(runtime.host_effect_pending());
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_unknown_owner
        .is_some());
    faults
        .fail_marker_after_write
        .store(false, std::sync::atomic::Ordering::SeqCst);
    faults
        .fail_marker_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let recovered_owner = controller.exact_marker_operation_id().unwrap();
    assert_eq!(recovered_owner, execution.execution_id);
    controller.restore_dns();
    assert!(!controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    runtime.record_runtime_dns_restore_result(&recovered_owner, true);
    assert_eq!(
        runtime.runtime_host_dns_physical_active(execution.generation),
        Some(false)
    );
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    assert!(
        runtime.host_effect_pending(),
        "physical off does not accept required DNS policy"
    );
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.host.dns_unknown_owner.is_none());
        assert!(!state.host.dns_unknown_cleanup);
        let owner = state.host.dns_owner.as_ref().unwrap();
        assert!(owner.restored && owner.policy_required && !owner.cleanup_pending);
        let observed = &state
            .host
            .current
            .as_ref()
            .unwrap()
            .receipt
            .as_ref()
            .unwrap()
            .effects[0];
        assert!(observed.outcome == HostEffectOutcome::FailedRestored);
        assert!(!observed.cleanup_pending);
        // An older pending receipt must not recreate E's Native account at
        // reset. Old policy retirement is distinct from new R acceptance.
        let mut old = RuntimeHostState {
            current: Some(HostExecutionRecord {
                execution: execution.clone(),
                receipt: Some(receipt),
                in_flight: false,
                accepted: false,
                result: None,
            }),
            dns_restored: state.host.dns_restored.clone(),
            dns_owner: state.host.dns_owner.clone(),
            ..Default::default()
        };
        let mut next = RuntimeHostState::default();
        next.carry_dns_cleanup_from(&mut old);
        assert!(next.dns_unknown_owner.is_none());
        assert!(!next.dns_unknown_cleanup);
        assert!(
            !next.dns_pending(),
            "retiring old R cannot resurrect physical cleanup"
        );
    }
    let mut next = execution.clone();
    next.execution_id = "actual-reaccept-after-E-restore".into();
    let fresh = controller.prepare_effect(true).unwrap();
    next.plan.recipe["dns"] = serde_json::to_value(&fresh).unwrap();
    assert!(runtime.begin_runtime_host_execution(next.clone(), &execution.desired));
    let reapplied = controller.apply_effect(&fresh, &next.execution_id);
    assert_eq!(reapplied.outcome, TakeoverOutcome::Applied);
    runtime.record_runtime_host_effect_result(&next, actual_dns_receipt(&next, reapplied));
    assert!(!runtime.host_effect_pending());
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert_eq!(
        runtime.current_config_snapshot().unwrap(),
        execution.desired
    );
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
}

#[test]
fn known_dns_marker_io_debt_reset_and_late_orders_keep_exact_native_account() {
    for receipt_before_reset in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let claimed = runtime
            .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
            .unwrap();
        assert!(
            claimed > 0,
            "running owner must differ from reset generation zero"
        );
        let (execution, mut controller, prepared, values, faults) =
            dns_execution_fixture_with_faults(&runtime, "late-unreadable-E");
        assert_eq!(execution.generation, claimed);
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        faults
            .fail_marker_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut pending = controller.apply_effect(&prepared, &execution.execution_id);
        assert_eq!(pending.outcome, TakeoverOutcome::RecoveryPending);
        pending.marker_id = controller.exact_marker_operation_id();
        assert!(pending.marker_id.is_none());
        let receipt = actual_dns_receipt(&execution, pending);
        if receipt_before_reset {
            runtime.record_runtime_host_effect_result(&execution, receipt.clone());
        }
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            let mut previous = std::mem::take(&mut state.host);
            state.host.carry_dns_cleanup_from(&mut previous);
            assert!(
                state.host.dns_pending(),
                "reset cannot lose an unproven write"
            );
        }
        runtime.status.write().unwrap().running = false;
        // The original callback may arrive after reset but before restore.
        runtime.record_runtime_host_effect_result(&execution, receipt.clone());
        assert!(runtime.host_effect_pending());
        runtime.record_runtime_dns_restore_result("successor-not-E", true);
        assert!(runtime.host_effect_pending());
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let owner = controller.exact_marker_operation_id().unwrap();
        assert_eq!(owner, execution.execution_id);
        controller.restore_dns();
        assert!(!controller.has_marker());
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
        runtime.record_runtime_dns_restore_result(&owner, true);
        assert!(!runtime.host_effect_pending());
        // A repeated late pending receipt cannot reintroduce the same debt.
        runtime.record_runtime_host_effect_result(&execution, receipt);
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            let mut previous = std::mem::take(&mut state.host);
            state.host.carry_dns_cleanup_from(&mut previous);
            assert!(!state.host.dns_pending());
            assert!(state.host.dns_unknown_owner.is_none());
            assert!(!state.host.dns_unknown_cleanup);
        }
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
    }
}

#[test]
fn checked_restore_before_current_dns_callback_cannot_accept_late_applied_or_pending() {
    for completion in ["applied", "pending", "unchanged-prior"] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (mut execution, mut controller, mut prepared, values, faults) =
            dns_execution_fixture_with_faults(&runtime, "restored-before-current-callback");
        if completion == "unchanged-prior" {
            assert_eq!(
                controller.apply_effect(&prepared, "prior-A").outcome,
                TakeoverOutcome::Applied
            );
            prepared = controller.prepare_effect(true).unwrap();
            assert_eq!(
                prepared.prior_marker_operation_id().as_deref(),
                Some("prior-A")
            );
            execution.plan.recipe["dns"] = serde_json::to_value(&prepared).unwrap();
            execution.plan.effects[0].before_digest =
                execution.plan.effects[0].target_digest.clone();
        }
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        faults
            .fail_marker_after_write
            .store(completion == "pending", std::sync::atomic::Ordering::SeqCst);
        let mut leaf = controller.apply_effect(&prepared, &execution.execution_id);
        if completion == "pending" {
            assert_eq!(leaf.outcome, TakeoverOutcome::RecoveryPending);
            leaf.marker_id = controller.exact_marker_operation_id();
            assert!(leaf.marker_id.is_none());
        } else if completion == "unchanged-prior" {
            assert_eq!(leaf.outcome, TakeoverOutcome::Unchanged);
        } else {
            assert_eq!(leaf.outcome, TakeoverOutcome::Applied);
        }
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let owner = controller.exact_marker_operation_id().unwrap();
        controller.restore_dns();
        assert!(!controller.has_marker());
        runtime.record_runtime_dns_restore_result(&owner, true);
        assert!(runtime.core_running());
        runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, leaf));
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.host.dns_unknown_owner.is_none());
        assert!(!state.host.dns_unknown_cleanup);
        assert!(state.host.dns_owner.as_ref().unwrap().restored);
        assert!(!state.host.current.as_ref().unwrap().accepted);
        assert!(
            state.host.pending(),
            "R still requires DNS even though Native cleanup finished"
        );
        assert_eq!(state.host.accepted_dns_takeover, None);
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    }
}

#[test]
fn actual_dns_reconcile_resolves_only_its_own_unreadable_write() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values, faults) =
        dns_execution_fixture_with_faults(&runtime, "reconcile-known-E");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    faults
        .fail_marker_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut uncertain = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(uncertain.outcome, TakeoverOutcome::RecoveryPending);
    uncertain.marker_id = controller.exact_marker_operation_id();
    assert!(uncertain.marker_id.is_none());
    runtime
        .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, uncertain));
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_unknown_owner
        .is_some());
    faults
        .fail_marker_after_write
        .store(false, std::sync::atomic::Ordering::SeqCst);
    faults
        .fail_marker_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let reconciled = controller.reconcile_dns_checked().unwrap();
    assert_eq!(reconciled.outcome, TakeoverOutcome::Applied);
    let own_readback = controller.checked_owned_receipt().unwrap().unwrap();
    assert_eq!(own_readback.operation_id, execution.execution_id);
    let mut foreign = own_readback.clone();
    foreign.operation_id = "foreign-readback".into();
    foreign.marker_id = Some(foreign.operation_id.clone());
    runtime.record_runtime_dns_reconcile_result(execution.generation, execution.intent, foreign);
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_unknown_owner
        .is_some());
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        execution.intent,
        own_readback,
    );
    assert!(!runtime.host_effect_pending());
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_unknown_owner
        .is_none());
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
}

#[test]
fn unreadable_dns_owner_eviction_and_foreign_marker_are_never_washed_by_restore() {
    for foreign_marker in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (execution, mut controller, prepared, values, faults) =
            dns_execution_fixture_with_faults(&runtime, "recoverable-newest-E");
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        if !foreign_marker {
            let mut older = execution.clone();
            older.execution_id = "evicted-original-owner".into();
            older.generation = execution.generation.saturating_sub(1);
            older.plan.recipe["generation"] = json!(older.generation);
            // Ledger capacity semantics: only the retained original recipe
            // can be recovered. Its eviction must never imply cleanup.
            runtime
                .runtime_config_apply
                .lock()
                .unwrap()
                .host
                .retain_unreadable_dns_owner(&older);
        }
        faults
            .fail_marker_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut pending = controller.apply_effect(&prepared, &execution.execution_id);
        assert_eq!(pending.outcome, TakeoverOutcome::RecoveryPending);
        pending.marker_id = controller.exact_marker_operation_id();
        assert!(pending.marker_id.is_none());
        let mut receipt = actual_dns_receipt(&execution, pending);
        if foreign_marker {
            receipt.effects[0].receipt_id =
                Some(format!("{}:foreign-marker", execution.execution_id));
        }
        runtime.record_runtime_host_effect_result(&execution, receipt);
        assert!(
            runtime
                .runtime_config_apply
                .lock()
                .unwrap()
                .host
                .dns_unknown_cleanup
        );
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let owner = controller.exact_marker_operation_id().unwrap();
        controller.restore_dns();
        assert!(!controller.has_marker());
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
        runtime.record_runtime_dns_restore_result(&owner, true);
        assert!(
            runtime
                .runtime_config_apply
                .lock()
                .unwrap()
                .host
                .dns_unknown_cleanup
        );
        assert!(runtime.host_effect_pending());
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            let mut previous = std::mem::take(&mut state.host);
            state.host.carry_dns_cleanup_from(&mut previous);
            assert!(state.host.dns_unknown_cleanup);
        }
        runtime.record_runtime_dns_restore_result("evicted-original-owner", true);
        runtime.record_runtime_dns_restore_result("foreign-marker", true);
        assert!(
            runtime.host_effect_pending(),
            "neither successor nor foreign proof discharges overflow"
        );
    }
}

#[test]
fn unreadable_dns_recipe_prior_owner_restore_closes_only_that_execution() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (mut execution, mut controller, prepared, values, faults) =
        dns_execution_fixture_with_faults(&runtime, "no-proof-E-with-prior-A");
    let first = controller.apply_effect(&prepared, "original-A");
    assert_eq!(first.outcome, TakeoverOutcome::Applied);
    let prior = controller.prepare_effect(true).unwrap();
    assert_eq!(
        prior.prior_marker_operation_id().as_deref(),
        Some("original-A")
    );
    execution.plan.recipe["dns"] = serde_json::to_value(&prior).unwrap();
    execution.plan.effects[0].before_digest = execution.plan.effects[0].target_digest.clone();
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    faults
        .fail_marker_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(controller.exact_marker_operation_id().is_none());
    // Model the production worker-join/no-completion receipt, not a made-up
    // successful write. Only the actual marker below can prove restoration.
    let receipt = actual_dns_receipt(
        &execution,
        TakeoverReceipt {
            operation_id: execution.execution_id.clone(),
            outcome: TakeoverOutcome::Unknown,
            marker_id: None,
            cleanup_pending: true,
            reason: Some("worker completion unavailable".into()),
        },
    );
    runtime.record_runtime_host_effect_result(&execution, receipt.clone());
    runtime.record_runtime_dns_restore_result("unrelated-prior", true);
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_unknown_owner
        .is_some());
    faults
        .fail_marker_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let owner = controller.exact_marker_operation_id().unwrap();
    assert_eq!(owner, "original-A");
    controller.restore_dns();
    assert!(!controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    runtime.record_runtime_dns_restore_result(&owner, true);
    assert_eq!(
        runtime.runtime_host_dns_physical_active(execution.generation),
        Some(false)
    );
    assert!(
        runtime.host_effect_pending(),
        "unchanged required policy is not accepted"
    );
    runtime.record_runtime_host_effect_result(&execution, receipt);
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.host.dns_unknown_owner.is_none());
    assert!(!state.host.dns_unknown_cleanup);
    assert!(state.host.dns_execution_restored(&execution));
    assert!(!state.host.current.as_ref().unwrap().accepted);
}

#[test]
fn exact_dns_restore_projects_off_for_only_its_recipe_predecessor_not_a_successor() {
    for successor in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (mut execution, mut controller, prepared, values, faults) =
            dns_execution_fixture_with_faults(&runtime, "disable-E");
        assert_eq!(
            controller.apply_effect(&prepared, "original-A").outcome,
            TakeoverOutcome::Applied
        );
        let disable = controller.prepare_effect(false).unwrap();
        execution.desired["dnsConfig"]["takeoverSystemDns"] = json!(false);
        execution.plan.recipe["dns"] = serde_json::to_value(&disable).unwrap();
        execution.plan.recipe["desired_digest"] = json!(polaris_updater::sha256_hex(
            execution.desired.to_string().as_bytes()
        ));
        execution.plan.effects[0].before_digest = polaris_updater::sha256_hex(
            serde_json::to_value(disable.before())
                .unwrap()
                .to_string()
                .as_bytes(),
        );
        execution.plan.effects[0].target_digest = polaris_updater::sha256_hex(
            serde_json::to_value(disable.target())
                .unwrap()
                .to_string()
                .as_bytes(),
        );
        *runtime.current_config.write().unwrap() = Some(execution.desired.clone());
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            state.host.dns_owner = Some(DnsMarkerOwner {
                generation: execution.generation,
                marker_operation_id: if successor {
                    "unrelated-successor"
                } else {
                    "original-A"
                }
                .into(),
                cleanup_pending: successor,
                restored: false,
                policy_required: true,
            });
            state.host.accepted_dns_takeover = Some(true);
        }
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        faults
            .fail_marker_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut pending = controller.apply_effect(&disable, &execution.execution_id);
        assert_eq!(pending.outcome, TakeoverOutcome::RecoveryPending);
        pending.marker_id = controller.exact_marker_operation_id();
        assert!(pending.marker_id.is_none());
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, pending));
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let marker = controller.exact_marker_operation_id().unwrap();
        assert_eq!(marker, execution.execution_id);
        controller.restore_dns();
        assert!(!controller.has_marker());
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
        runtime.record_runtime_dns_restore_result(&marker, true);
        assert!(
            runtime.host_effect_pending(),
            "restore does not accept a new disabled policy"
        );
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.host.dns_unknown_owner.is_none());
        assert!(!state.host.dns_unknown_cleanup);
        if successor {
            let owner = state.host.dns_owner.as_ref().unwrap();
            assert_eq!(owner.marker_operation_id, "unrelated-successor");
            assert!(owner.cleanup_pending && !owner.restored && owner.policy_required);
            assert_eq!(state.host.accepted_dns_takeover, Some(true));
        } else {
            assert!(
                state.host.dns_owner.is_none(),
                "proven predecessor scope is physically retired"
            );
            assert_eq!(state.host.accepted_dns_takeover, None);
        }
    }
}

#[test]
fn exact_stop_restore_before_h_callback_cannot_be_resurrected_by_late_applied() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "stop-before-H");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    assert_eq!(
        controller.exact_marker_operation_id().as_deref(),
        Some(execution.execution_id.as_str())
    );
    let original_owner = controller.exact_marker_operation_id().unwrap();
    controller.restore_dns();
    assert!(!controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    // Actual caller releases its controller mutex before this ledger hook.
    runtime.record_runtime_dns_restore_result(&original_owner, true);
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        let mut previous = std::mem::take(&mut state.host);
        state.host.carry_dns_cleanup_from(&mut previous);
    }
    runtime.status.write().unwrap().running = false;
    runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
    assert!(!runtime.host_effect_pending());
    assert!(runtime.accepted_host_dns_takeover().is_none());
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.host.dns_owner.is_none());
    assert!(state.host.dns_retiring.is_empty());
    assert_eq!(
        state
            .host
            .dns_restored
            .iter()
            .filter(|(id, _)| id == &execution.execution_id)
            .count(),
        1
    );
    drop(state);
    runtime.record_runtime_dns_restore_result(&original_owner, true);
    assert!(!runtime.host_effect_pending());
}

#[test]
fn late_applied_without_a_real_restore_retains_native_cleanup_across_reset() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "late-active-DNS");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        let mut previous = std::mem::take(&mut state.host);
        state.host.carry_dns_cleanup_from(&mut previous);
    }
    runtime.status.write().unwrap().running = false;
    runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
    assert!(runtime.host_effect_pending());
    assert!(controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        let owner = state.host.dns_owner.as_ref().unwrap();
        assert_eq!(owner.marker_operation_id, execution.execution_id);
        assert!(owner.cleanup_pending && !owner.policy_required);
    }
    controller.restore_dns();
    assert!(!controller.has_marker());
    runtime.record_runtime_dns_restore_result(&execution.execution_id, true);
    assert!(!runtime.host_effect_pending());
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .dns_owner
        .is_none());
}

#[test]
fn proxy_only_success_does_not_clear_dns_reconcile_debt_or_borrow_its_scope() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "DNS-owned");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
    assert!(!runtime.host_effect_pending());
    let mut proxy = execution.clone();
    proxy.execution_id = "later-proxy-only".into();
    proxy.plan.effects = vec![HostEffectPreview {
        kind: HostEffectKind::SystemProxy,
        scope_digest: "proxy".into(),
        before_digest: "before".into(),
        target_digest: "after".into(),
    }];
    assert!(runtime.begin_runtime_host_execution(proxy.clone(), &execution.desired));
    runtime.record_runtime_host_effect_result(
        &proxy,
        receipt(&proxy, [HostEffectOutcome::Applied; 2]),
    );
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    let failed = controller.reconcile_dns_checked().unwrap();
    assert_eq!(failed.outcome, TakeoverOutcome::RecoveryPending);
    runtime.record_runtime_dns_reconcile_result(execution.generation, execution.intent, failed);
    assert!(runtime.host_effect_pending());
    assert!(runtime.runtime_dns_effect_retry_required(execution.generation, execution.intent));
    let mut projected = RuntimeApplyResult::status("hostEffectsPending");
    projected.host_execution_id = Some(proxy.execution_id.clone());
    projected.runtime_generation = Some(proxy.generation);
    projected.selection_intent_generation = Some(proxy.intent);
    projected.core_state = Some("unchanged");
    runtime.annotate_runtime_host_result(&mut projected);
    assert!(
        projected.pending_apply,
        "a completed proxy execution cannot clear independent DNS debt"
    );
    assert_eq!(projected.host_effects.len(), 1);
    assert_eq!(
        projected.host_effects[0].preview.kind,
        HostEffectKind::SystemProxy
    );
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        let current = state.host.current.as_ref().unwrap();
        assert!(current.accepted);
        assert_eq!(current.receipt.as_ref().unwrap().effects.len(), 1);
        assert_eq!(
            current.receipt.as_ref().unwrap().effects[0].preview.kind,
            HostEffectKind::SystemProxy
        );
        assert!(current.receipt.as_ref().unwrap().effects[0].outcome == HostEffectOutcome::Applied);
        assert_eq!(
            state.host.dns_owner.as_ref().unwrap().marker_operation_id,
            execution.execution_id
        );
    }
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["8.8.8.8".into()]);
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        execution.intent,
        controller.reconcile_dns_checked().unwrap(),
    );
    assert!(!runtime.host_effect_pending());
    assert_eq!(
        runtime.current_config_snapshot().unwrap(),
        execution.desired
    );
}

#[test]
fn stale_reconcile_and_foreign_restore_cannot_clear_or_rebind_current_dns_owner() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "fixed-DNS-owner");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        execution.intent,
        controller.reconcile_dns_checked().unwrap(),
    );
    assert!(runtime.host_effect_pending());
    runtime.record_runtime_dns_restore_result("foreign", true);
    assert!(runtime.host_effect_pending());
    runtime.register_selector_intent();
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["8.8.8.8".into()]);
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        execution.intent,
        controller.reconcile_dns_checked().unwrap(),
    );
    assert!(runtime.host_effect_pending());
    let intent = runtime.selector_reconcile.intent_generation();
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        intent,
        controller.reconcile_dns_checked().unwrap(),
    );
    assert!(!runtime.host_effect_pending());
    assert_eq!(
        runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .host
            .dns_owner
            .as_ref()
            .unwrap()
            .marker_operation_id,
        execution.execution_id
    );
    controller.restore_dns();
    assert!(!controller.has_marker());
    runtime.record_runtime_dns_restore_result(&execution.execution_id, true);
    assert_eq!(
        runtime.runtime_host_dns_physical_active(execution.generation),
        Some(false)
    );
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    assert!(
        runtime.host_effect_pending(),
        "running DNS policy is still owed after physical cleanup"
    );
    let deps = runtime
        .runtime_accepted_host_deps(execution.generation)
        .unwrap();
    assert!(
        !runtime
            .runtime_host_dns_generation_deps(execution.generation, &deps)
            .unwrap()
            .system_dns_takeover_active
    );
}

#[test]
fn current_published_dns_disable_blocks_watcher_even_when_startup_and_h_still_enable() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "published-policy");
    assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    runtime.record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
    *runtime.startup_snapshot.write().unwrap() = Some(execution.desired.clone());
    let mut current = execution.desired.clone();
    current["dnsConfig"]["takeoverSystemDns"] = json!(false);
    *runtime.current_config.write().unwrap() = Some(current);
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert!(!runtime.reconcile_system_dns_locked());
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    runtime.record_runtime_dns_reconcile_result(
        execution.generation,
        execution.intent,
        controller.reconcile_dns_checked().unwrap(),
    );
    assert!(
        !runtime.host_effect_pending(),
        "foreign-to-published-policy callbacks cannot authorize or inject new scope"
    );
}

#[test]
fn generation_uses_only_observed_host_dns_projection_and_preserves_other_core_deps() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let execution = execution(&runtime, "deps");
    let mut core = runtime
        .runtime_accepted_host_deps(execution.generation)
        .unwrap();
    core.system_dns_takeover_active = false;
    core.lan_resolver_for_dns = Some("192.168.1.1".into());
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        state
            .host
            .accepted_deps
            .as_mut()
            .unwrap()
            .system_dns_takeover_active = true;
    }
    assert!(
        !runtime
            .runtime_host_dns_generation_deps(execution.generation, &core)
            .unwrap()
            .system_dns_takeover_active
    );
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        state.host.accepted_dns_takeover = Some(true);
    }
    let projected = runtime
        .runtime_host_dns_generation_deps(execution.generation, &core)
        .unwrap();
    assert!(projected.system_dns_takeover_active);
    assert_eq!(projected.lan_resolver_for_dns, core.lan_resolver_for_dns);
    assert!(runtime
        .runtime_host_dns_generation_deps(execution.generation + 1, &core)
        .is_none());
}

#[test]
fn cold_checked_dns_accepts_h_under_busy_start_without_a_host_or_core_transaction() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    runtime.gate.begin();
    let (execution, mut controller, prepared, _values) =
        dns_execution_fixture(&runtime, "dns-startup:real");
    assert!(runtime.gate.is_busy());
    assert!(runtime.begin_runtime_cold_dns(
        execution.generation,
        execution.intent,
        &execution.execution_id
    ));
    assert!(runtime.runtime_host_execution_in_flight());
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .current
        .is_none());
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, applied);
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert!(!runtime.runtime_host_execution_in_flight());
    assert!(!runtime.host_effect_pending());
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(
        state.host.current.is_none(),
        "cold receipt is not a fabricated HostExecution/Core transaction"
    );
    assert_eq!(
        state.host.dns_owner.as_ref().unwrap().marker_operation_id,
        execution.execution_id
    );
    assert!(!state.host.dns_owner.as_ref().unwrap().cleanup_pending);
}

#[test]
fn cold_restore_before_h_callback_survives_reset_without_reviving_native_cleanup() {
    for existing_owner in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let claimed = runtime
            .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
            .unwrap();
        assert!(claimed > 0);
        runtime.gate.begin();
        let (execution, mut controller, prepared, values) =
            dns_execution_fixture(&runtime, "dns-startup:restore-first");
        assert_eq!(execution.generation, claimed);
        let prepared = if existing_owner {
            assert_eq!(
                controller.apply_effect(&prepared, "older-marker").outcome,
                TakeoverOutcome::Applied
            );
            controller.prepare_effect(true).unwrap()
        } else {
            prepared
        };
        assert!(runtime.begin_runtime_cold_dns(
            execution.generation,
            execution.intent,
            &execution.execution_id
        ));
        assert!(runtime.bind_runtime_cold_dns_marker(
            &execution.execution_id,
            prepared.prior_marker_operation_id()
        ));
        let mut applied = controller.apply_effect(&prepared, &execution.execution_id);
        if existing_owner {
            assert_eq!(applied.outcome, TakeoverOutcome::Unchanged);
            applied.marker_id = controller
                .checked_owned_receipt()
                .unwrap()
                .unwrap()
                .marker_id;
        } else {
            assert_eq!(applied.outcome, TakeoverOutcome::Applied);
        }
        let owner = controller.exact_marker_operation_id().unwrap();
        controller.restore_dns();
        assert!(!controller.has_marker());
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
        runtime.record_runtime_dns_restore_result(&owner, true);
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            let mut previous = std::mem::take(&mut state.host);
            state.host.carry_dns_cleanup_from(&mut previous);
        }
        runtime.status.write().unwrap().running = false;
        runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, applied);
        assert!(!runtime.host_effect_pending());
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
        assert!(runtime
            .runtime_config_apply
            .lock()
            .unwrap()
            .host
            .dns_owner
            .is_none());
    }
}

#[test]
fn cold_late_applied_without_checked_restore_retains_native_debt_across_reset() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    runtime.gate.begin();
    let (execution, mut controller, prepared, _values) =
        dns_execution_fixture(&runtime, "dns-startup:late");
    assert!(runtime.begin_runtime_cold_dns(
        execution.generation,
        execution.intent,
        &execution.execution_id
    ));
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    {
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        let mut previous = std::mem::take(&mut state.host);
        state.host.carry_dns_cleanup_from(&mut previous);
    }
    runtime.status.write().unwrap().running = false;
    runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, applied);
    assert!(runtime.host_effect_pending());
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    runtime.record_runtime_dns_restore_result("foreign", true);
    assert!(runtime.host_effect_pending());
    controller.restore_dns();
    assert!(!controller.has_marker());
    runtime.record_runtime_dns_restore_result(&execution.execution_id, true);
    assert!(!runtime.host_effect_pending());
}

#[test]
fn cold_readonly_adoption_requires_owned_readback_and_failure_never_accepts_startup_target() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    runtime.gate.begin();
    let (execution, mut controller, prepared, values) =
        dns_execution_fixture(&runtime, "dns-startup:adoption");
    assert!(controller.checked_owned_receipt().unwrap().is_none());
    let applied = controller.apply_effect(&prepared, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    let checked = controller.checked_owned_receipt().unwrap().unwrap();
    assert_eq!(checked.operation_id, execution.execution_id);
    assert_eq!(checked.outcome, TakeoverOutcome::Unchanged);
    runtime
        .gate
        .end(polaris_core_supervisor::LifecycleKind::Start);
    runtime.record_runtime_dns_reconcile_result(execution.generation, execution.intent, checked);
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .host
        .current
        .is_none());
    values
        .lock()
        .unwrap()
        .insert("A".into(), vec!["9.9.9.9".into()]);
    assert!(controller.checked_owned_receipt().is_err());

    let (failed_runtime, _failed_dir) =
        crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    failed_runtime.gate.begin();
    let (failed, _controller, _prepared, empty) =
        dns_execution_fixture(&failed_runtime, "dns-startup:readonly-failure");
    empty.lock().unwrap().clear();
    assert!(failed_runtime.begin_runtime_cold_dns(
        failed.generation,
        failed.intent,
        &failed.execution_id
    ));
    let reason = match _controller.prepare_effect(true) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("missing service scope was admitted"),
    };
    assert!(!_controller.has_marker());
    let failure = TakeoverReceipt {
        operation_id: failed.execution_id.clone(),
        outcome: TakeoverOutcome::FailedUnchanged,
        marker_id: None,
        cleanup_pending: false,
        reason: Some(reason),
    };
    failed_runtime.record_runtime_cold_dns_result(failed.generation, failed.intent, failure);
    assert_eq!(failed_runtime.accepted_host_dns_takeover(), None);
    assert!(
        failed_runtime.host_effect_pending(),
        "startup prospective deps are not H acceptance"
    );
    assert!(!failed_runtime.runtime_host_execution_in_flight());
    let core = failed_runtime
        .runtime_accepted_host_deps(failed.generation)
        .unwrap();
    assert!(
        failed_runtime
            .runtime_host_dns_generation_deps(failed.generation, &core)
            .is_none(),
        "busy startup must not probe/adopt or borrow its target"
    );
    failed_runtime
        .gate
        .end(polaris_core_supervisor::LifecycleKind::Start);
    assert_eq!(failed_runtime.accepted_host_dns_takeover(), None);
}

#[test]
fn dns_policy_uses_effective_mode_only_in_supported_host_domains() {
    for platform in Platform::ALL.iter().copied() {
        for mode in ["systemProxy", "tun", "manual"] {
            for enabled in [false, true] {
                let mut desired = polaris_store::default_config();
                desired["proxyModeType"] = json!(mode);
                desired["dnsConfig"]["takeoverSystemDns"] = json!(enabled);
                let supported = matches!(platform, Platform::Mac | Platform::Linux);
                let typed =
                    serde_json::from_value::<polaris_config_engine::user_config::UserConfig>(
                        desired.clone(),
                    )
                    .unwrap();
                if platform == Platform::Android {
                    assert!(typed.proxy_mode_type.effective_on(platform).is_tun());
                }
                assert_eq!(
                    dns_config_requires_takeover(&desired, platform),
                    supported && mode == "tun" && enabled,
                    "{platform:?} {mode} takeover={enabled}"
                );
            }
        }
        assert!(!dns_config_requires_takeover(
            &json!({"proxyModeType":"tun"}),
            platform
        ));
    }
}

#[test]
fn unsupported_host_dns_domain_never_admits_cold_write_or_policy_debt() {
    for platform in [
        Platform::Win,
        Platform::Other,
        Platform::Android,
        Platform::Ios,
    ] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(platform);
        runtime
            .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
            .unwrap();
        runtime.gate.begin();
        let mut execution = execution(&runtime, "unsupported-cold-domain");
        execution.desired["proxyModeType"] = json!("tun");
        *runtime.current_config.write().unwrap() = Some(execution.desired);
        assert!(!runtime.set_system_dns_locked());
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
        assert!(!runtime.host_effect_pending());
        assert!(!runtime.runtime_host_execution_in_flight());
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.host.dns_cold.is_none());
        assert!(state.host.dns_policy_unconfirmed.is_none());
        assert!(state.host.dns_owner.is_none());
    }
}

#[test]
fn cold_prior_restore_keeps_replacement_reserved_until_its_actual_receipt() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    runtime
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    runtime.gate.begin();
    let (execution, mut controller, prepared, values, faults) =
        dns_execution_fixture_with_faults(&runtime, "dns-startup:replacement");
    faults
        .fail_marker_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let partial = controller.apply_effect(&prepared, "prior-partial-owner");
    assert_eq!(partial.outcome, TakeoverOutcome::RecoveryPending);
    faults
        .fail_marker_after_write
        .store(false, std::sync::atomic::Ordering::SeqCst);
    faults
        .fail_marker_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let retry = controller.prepare_effect(true).unwrap();
    assert!(retry.needs_prior_restore());
    assert_eq!(
        retry.prior_marker_operation_id().as_deref(),
        Some("prior-partial-owner")
    );
    assert!(controller.checked_owned_receipt().unwrap().is_none());
    assert!(runtime.begin_runtime_cold_dns(
        execution.generation,
        execution.intent,
        &execution.execution_id
    ));
    assert!(runtime
        .bind_runtime_cold_dns_marker(&execution.execution_id, retry.prior_marker_operation_id()));
    runtime.record_runtime_dns_restore_result("foreign-owner", true);
    runtime.record_runtime_dns_restore_result("prior-partial-owner", false);
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert!(cold.in_flight);
        assert_eq!(cold.prior_marker.as_deref(), Some("prior-partial-owner"));
        assert_eq!(cold.operation_id, execution.execution_id);
    }
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    controller.restore_dns();
    assert!(!controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    runtime.record_runtime_dns_restore_result("prior-partial-owner", true);
    // Duplicate and foreign restore callbacks cannot retire the successor reservation.
    runtime.record_runtime_dns_restore_result("prior-partial-owner", true);
    runtime.record_runtime_dns_restore_result("foreign-owner", true);
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert!(cold.in_flight);
        assert_eq!(cold.operation_id, execution.execution_id);
        assert!(cold.prior_marker.is_none());
        assert!(state.host.dns_owner.is_none());
        assert!(state.host.current.is_none());
    }
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    assert!(runtime.host_effect_pending());
    assert!(runtime.bind_runtime_cold_dns_marker(&execution.execution_id, None));
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state
            .host
            .dns_cold
            .as_ref()
            .unwrap()
            .restored_prior_marker
            .is_none());
    }
    let next = controller.prepare_after_restore(&retry).unwrap();
    let applied = controller.apply_effect(&next, &execution.execution_id);
    assert_eq!(applied.outcome, TakeoverOutcome::Applied);
    runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, applied);
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    assert!(!runtime.host_effect_pending());
    assert!(!runtime.runtime_host_execution_in_flight());
    runtime.record_runtime_dns_restore_result("prior-partial-owner", true);
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.host.dns_cold.is_none());
    assert_eq!(
        state.host.dns_owner.as_ref().unwrap().marker_operation_id,
        execution.execution_id
    );
    assert_eq!(state.host.accepted_dns_takeover, Some(true));
    assert_eq!(
        controller.exact_marker_operation_id().as_deref(),
        Some(execution.execution_id.as_str())
    );
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
}

// Construct the real prior marker and the same cold reservation/binding used by
// the producer. No alternate settlement logic is implemented in this fixture.
fn cold_reserved_prior_fixture(
    runtime: &Arc<ProxyRuntime>,
    id: &str,
    partial: bool,
) -> (
    HostEffectExecution,
    LedgerDnsController,
    polaris_system_integration::dns_ops::PreparedDnsEffect,
    LedgerDnsValues,
    Arc<LedgerDnsFaults>,
) {
    let generation = runtime
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    assert!(generation > 0);
    runtime.gate.begin();
    let (execution, mut controller, prepared, values, faults) =
        dns_execution_fixture_with_faults(runtime, id);
    assert_eq!(execution.generation, generation);
    faults
        .fail_marker_after_write
        .store(partial, std::sync::atomic::Ordering::SeqCst);
    let prior = controller.apply_effect(&prepared, "cold-prior-owner");
    assert_eq!(
        prior.outcome,
        if partial {
            TakeoverOutcome::RecoveryPending
        } else {
            TakeoverOutcome::Applied
        }
    );
    faults
        .fail_marker_after_write
        .store(false, std::sync::atomic::Ordering::SeqCst);
    faults
        .fail_marker_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let prepared = controller.prepare_effect(true).unwrap();
    assert_eq!(prepared.needs_prior_restore(), partial);
    assert_eq!(
        prepared.prior_marker_operation_id().as_deref(),
        Some("cold-prior-owner")
    );
    assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
    assert!(runtime.begin_runtime_cold_dns(
        execution.generation,
        execution.intent,
        &execution.execution_id
    ));
    assert!(runtime.bind_runtime_cold_dns_marker(
        &execution.execution_id,
        prepared.prior_marker_operation_id()
    ));
    (execution, controller, prepared, values, faults)
}

#[test]
fn restored_borrowed_cold_receipts_cannot_accept_h_or_revive_cleanup_after_reset() {
    for partial in [false, true] {
        for reset in [false, true] {
            let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
            let (execution, mut controller, prepared, values, _) =
                cold_reserved_prior_fixture(&runtime, "borrowed-cold-result", partial);
            let mut receipt = controller.apply_effect(&prepared, &execution.execution_id);
            if partial {
                assert_eq!(receipt.outcome, TakeoverOutcome::RecoveryPending);
                assert!(receipt.cleanup_pending);
                assert_eq!(receipt.marker_id.as_deref(), Some("cold-prior-owner"));
                // The actual partial-plan guard must have performed no new
                // marker or DNS mutation before borrowing the old footprint.
                assert!(controller.effect_facts_match(&prepared).unwrap());
            } else {
                assert_eq!(receipt.outcome, TakeoverOutcome::Unchanged);
                assert!(!receipt.cleanup_pending);
                receipt.marker_id = controller
                    .checked_owned_receipt()
                    .unwrap()
                    .unwrap()
                    .marker_id;
            }
            assert_eq!(runtime.accepted_host_dns_takeover(), None);
            controller.restore_dns();
            assert!(!controller.has_marker());
            assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
            runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
            runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
            runtime.record_runtime_dns_restore_result("foreign-marker", true);
            {
                let state = runtime.runtime_config_apply.lock().unwrap();
                let cold = state.host.dns_cold.as_ref().unwrap();
                assert!(cold.in_flight);
                assert_eq!(
                    cold.restored_prior_marker
                        .as_ref()
                        .map(|proof| proof.marker_operation_id.as_str()),
                    Some("cold-prior-owner")
                );
            }
            if reset {
                let mut state = runtime.runtime_config_apply.lock().unwrap();
                let mut previous = std::mem::take(&mut state.host);
                state.host.carry_dns_cleanup_from(&mut previous);
                assert_eq!(
                    state
                        .host
                        .dns_cold
                        .as_ref()
                        .unwrap()
                        .restored_prior_marker
                        .as_ref()
                        .map(|proof| proof.marker_operation_id.as_str()),
                    Some("cold-prior-owner")
                );
                drop(state);
                runtime.status.write().unwrap().running = false;
            }
            runtime.record_runtime_cold_dns_result(
                execution.generation,
                execution.intent,
                receipt.clone(),
            );
            runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, receipt);
            runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
            assert_eq!(runtime.accepted_host_dns_takeover(), None);
            assert_eq!(runtime.host_effect_pending(), !reset);
            let state = runtime.runtime_config_apply.lock().unwrap();
            assert!(state.host.dns_cold.is_none());
            assert!(state.host.dns_owner.is_none());
            assert!(!state.host.dns_unknown_cleanup);
            assert!(state.host.dns_unknown_owner.is_none());
            assert_eq!(
                state.host.dns_policy_unconfirmed,
                (!reset).then_some(execution.generation)
            );
        }
    }
}

#[test]
fn borrowed_cold_cleanup_requires_exact_restore_and_full_reservation_identity() {
    for mismatch in [
        "unrestored",
        "failed-restore",
        "foreign-restore",
        "generation",
        "intent",
        "operation",
        "marker",
        "missing-marker",
        "unknown-outcome",
    ] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (execution, mut controller, prepared, values, faults) =
            cold_reserved_prior_fixture(&runtime, "exact-borrowed-cold", true);
        let mut receipt = controller.apply_effect(&prepared, &execution.execution_id);
        assert_eq!(receipt.outcome, TakeoverOutcome::RecoveryPending);
        assert_eq!(receipt.marker_id.as_deref(), Some("cold-prior-owner"));
        assert!(controller.effect_facts_match(&prepared).unwrap());
        let confirmed = !matches!(
            mismatch,
            "unrestored" | "failed-restore" | "foreign-restore"
        );
        if confirmed {
            controller.restore_dns();
            assert!(!controller.has_marker());
            assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
            runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        } else if mismatch == "failed-restore" {
            faults
                .fail_marker_reads
                .store(true, std::sync::atomic::Ordering::SeqCst);
            controller.restore_dns();
            runtime.record_runtime_dns_restore_result("cold-prior-owner", false);
            faults
                .fail_marker_reads
                .store(false, std::sync::atomic::Ordering::SeqCst);
            assert!(controller.has_marker());
        } else if mismatch == "foreign-restore" {
            runtime.record_runtime_dns_restore_result("foreign-marker", true);
            assert!(controller.has_marker());
        }
        {
            let state = runtime.runtime_config_apply.lock().unwrap();
            assert_eq!(
                state
                    .host
                    .dns_cold
                    .as_ref()
                    .unwrap()
                    .restored_prior_marker
                    .as_ref()
                    .map(|proof| proof.marker_operation_id.as_str()),
                confirmed.then_some("cold-prior-owner")
            );
        }
        let original_pending = receipt.clone();
        let mut generation = execution.generation;
        let mut intent = execution.intent;
        match mismatch {
            "generation" => {
                generation = runtime
                    .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
                    .unwrap()
            }
            "intent" => {
                runtime.selector_reconcile.register_intent();
                intent = runtime.selector_reconcile.intent_generation();
            }
            "operation" => receipt.operation_id = "foreign-operation".into(),
            "marker" => receipt.marker_id = Some("foreign-marker".into()),
            "missing-marker" => receipt.marker_id = None,
            "unknown-outcome" => receipt.outcome = TakeoverOutcome::Unknown,
            _ => {}
        }
        if mismatch == "generation" {
            assert_ne!(generation, execution.generation);
        }
        if mismatch == "intent" {
            assert_ne!(intent, execution.intent);
        }
        runtime.record_runtime_cold_dns_result(generation, intent, receipt);
        if mismatch == "unknown-outcome" {
            runtime.record_runtime_cold_dns_result(
                execution.generation,
                execution.intent,
                original_pending,
            );
        }
        assert!(runtime.host_effect_pending(), "{mismatch}");
        assert_eq!(runtime.accepted_host_dns_takeover(), None, "{mismatch}");
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert_eq!(cold.generation, execution.generation);
        assert_eq!(cold.intent, execution.intent);
        assert_eq!(cold.operation_id, execution.execution_id);
        if matches!(mismatch, "generation" | "intent" | "operation") {
            assert!(cold.in_flight);
        } else {
            assert!(!cold.in_flight);
            assert!(cold.cleanup_pending);
        }
        assert!(state.host.dns_owner.is_none());
    }
}

#[test]
fn replacement_bind_ends_prior_only_proof_before_real_applied_or_partial_write() {
    for partial in [false, true] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (execution, mut controller, prepared, values, faults) =
            cold_reserved_prior_fixture(&runtime, "real-cold-replacement", true);
        controller.restore_dns();
        assert!(!controller.has_marker());
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        {
            let state = runtime.runtime_config_apply.lock().unwrap();
            assert_eq!(
                state
                    .host
                    .dns_cold
                    .as_ref()
                    .unwrap()
                    .restored_prior_marker
                    .as_ref()
                    .map(|proof| proof.marker_operation_id.as_str()),
                Some("cold-prior-owner")
            );
        }
        assert!(runtime.bind_runtime_cold_dns_marker(&execution.execution_id, None));
        // Both real producer arms perform this bind before prepare_after_restore
        // and apply. A duplicated prior restore cannot put the proof back.
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        runtime.record_runtime_dns_restore_result("foreign-marker", true);
        {
            let state = runtime.runtime_config_apply.lock().unwrap();
            let cold = state.host.dns_cold.as_ref().unwrap();
            assert!(cold.in_flight);
            assert!(cold.prior_marker.is_none());
            assert!(cold.restored_prior_marker.is_none());
        }
        let next = controller.prepare_after_restore(&prepared).unwrap();
        faults
            .fail_marker_after_write
            .store(partial, std::sync::atomic::Ordering::SeqCst);
        let mut receipt = controller.apply_effect(&next, &execution.execution_id);
        assert_eq!(
            receipt.outcome,
            if partial {
                TakeoverOutcome::RecoveryPending
            } else {
                TakeoverOutcome::Applied
            }
        );
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        receipt.marker_id = controller.exact_marker_operation_id();
        assert_eq!(
            receipt.marker_id.as_deref(),
            Some(execution.execution_id.as_str())
        );
        assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, receipt);
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        assert_eq!(
            runtime.accepted_host_dns_takeover(),
            (!partial).then_some(true)
        );
        assert_eq!(runtime.host_effect_pending(), partial);
        {
            let state = runtime.runtime_config_apply.lock().unwrap();
            assert!(state.host.dns_cold.is_none());
            let owner = state.host.dns_owner.as_ref().unwrap();
            assert_eq!(owner.marker_operation_id, execution.execution_id);
            assert_eq!(owner.cleanup_pending, partial);
        }
        controller.restore_dns();
        assert!(!controller.has_marker());
        runtime.record_runtime_dns_restore_result(&execution.execution_id, true);
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
        assert_eq!(
            runtime.runtime_host_dns_physical_active(execution.generation),
            Some(false)
        );
        assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    }
}

#[test]
fn new_partial_write_cannot_borrow_prior_restoration_from_ambiguous_marker_metadata() {
    for marker in [None, Some("cold-prior-owner"), Some("foreign-marker")] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (execution, mut controller, prepared, values, faults) =
            cold_reserved_prior_fixture(&runtime, "ambiguous-new-write", true);
        controller.restore_dns();
        assert!(!controller.has_marker());
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        assert!(runtime.bind_runtime_cold_dns_marker(&execution.execution_id, None));
        let next = controller.prepare_after_restore(&prepared).unwrap();
        faults
            .fail_marker_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut receipt = controller.apply_effect(&next, &execution.execution_id);
        assert_eq!(receipt.outcome, TakeoverOutcome::RecoveryPending);
        assert!(receipt.cleanup_pending);
        faults
            .fail_marker_after_write
            .store(false, std::sync::atomic::Ordering::SeqCst);
        faults
            .fail_marker_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            controller.exact_marker_operation_id().as_deref(),
            Some(execution.execution_id.as_str())
        );
        assert_eq!(values.lock().unwrap()["A"], vec!["8.8.8.8"]);
        // Change one metadata axis of a real partial-write receipt. Missing or
        // even old-marker text cannot erase a newly written physical footprint.
        receipt.marker_id = marker.map(str::to_owned);
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, receipt);
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        assert!(runtime.host_effect_pending());
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert!(!cold.in_flight);
        assert!(cold.cleanup_pending);
        assert!(cold.restored_prior_marker.is_none());
        assert!(state.host.dns_owner.is_none());
        assert_eq!(
            controller.exact_marker_operation_id().as_deref(),
            Some(execution.execution_id.as_str())
        );
    }
}

#[test]
fn borrowed_cold_restore_proof_keeps_original_owner_generation() {
    let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
    let old_generation = runtime
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    assert!(old_generation > 0);
    runtime.gate.begin();
    let (old, mut controller, prepared, values) = dns_execution_fixture(&runtime, "old-cold-owner");
    assert!(runtime.begin_runtime_cold_dns(old.generation, old.intent, &old.execution_id));
    assert!(runtime.bind_runtime_cold_dns_marker(&old.execution_id, None));
    let old_receipt = controller.apply_effect(&prepared, &old.execution_id);
    assert_eq!(old_receipt.outcome, TakeoverOutcome::Applied);
    runtime.record_runtime_cold_dns_result(old.generation, old.intent, old_receipt);
    assert_eq!(runtime.accepted_host_dns_takeover(), Some(true));
    runtime
        .gate
        .end(polaris_core_supervisor::LifecycleKind::Start);
    let new_generation = runtime
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    assert!(new_generation > old_generation);
    runtime.gate.begin();
    {
        // Use the real reset carry path: old physical custody survives without
        // borrowing its previous generation's accepted policy.
        let mut state = runtime.runtime_config_apply.lock().unwrap();
        let mut previous = std::mem::take(&mut state.host);
        state.host.carry_dns_cleanup_from(&mut previous);
        let owner = state.host.dns_owner.as_ref().unwrap();
        assert_eq!(owner.generation, old_generation);
        assert!(owner.cleanup_pending);
        assert!(!owner.policy_required);
    }
    let mut successor = execution(&runtime, "new-cold-reservation");
    successor.desired["proxyModeType"] = json!("tun");
    *runtime.current_config.write().unwrap() = Some(successor.desired.clone());
    assert!(runtime.begin_runtime_cold_dns(
        successor.generation,
        successor.intent,
        &successor.execution_id
    ));
    let prepared = controller.prepare_effect(true).unwrap();
    assert!(!prepared.needs_prior_restore());
    assert_eq!(
        prepared.prior_marker_operation_id().as_deref(),
        Some(old.execution_id.as_str())
    );
    assert!(runtime.bind_runtime_cold_dns_marker(
        &successor.execution_id,
        prepared.prior_marker_operation_id()
    ));
    let mut borrowed = controller.apply_effect(&prepared, &successor.execution_id);
    assert_eq!(borrowed.outcome, TakeoverOutcome::Unchanged);
    borrowed.marker_id = controller
        .checked_owned_receipt()
        .unwrap()
        .unwrap()
        .marker_id;
    controller.restore_dns();
    assert!(!controller.has_marker());
    assert_eq!(values.lock().unwrap()["A"], vec!["192.168.1.1"]);
    runtime.record_runtime_dns_restore_result(&old.execution_id, true);
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert_eq!(cold.generation, new_generation);
        assert_eq!(cold.intent, successor.intent);
        assert_eq!(cold.operation_id, successor.execution_id);
        let proof = cold.restored_prior_marker.as_ref().unwrap();
        assert_eq!(proof.marker_operation_id, old.execution_id);
        assert_eq!(proof.restored_generation, old_generation);
        assert!(state
            .host
            .dns_restored
            .iter()
            .any(|(id, generation)| id == &old.execution_id && *generation == old_generation));
        assert!(!state
            .host
            .dns_restored
            .iter()
            .any(|(id, generation)| id == &old.execution_id && *generation == new_generation));
    }
    runtime.record_runtime_cold_dns_result(successor.generation, successor.intent, borrowed);
    assert_eq!(runtime.accepted_host_dns_takeover(), None);
    assert!(runtime.host_effect_pending());
    let state = runtime.runtime_config_apply.lock().unwrap();
    assert!(state.host.dns_cold.is_none());
    assert_eq!(state.host.dns_policy_unconfirmed, Some(new_generation));
    assert!(state.host.dns_owner.is_none());
}

#[test]
fn borrowed_cold_restore_proof_requires_its_exact_bounded_history_entry() {
    for mismatch in ["missing", "generation", "marker", "evicted"] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Mac);
        let (execution, mut controller, prepared, _, _) =
            cold_reserved_prior_fixture(&runtime, "bounded-borrowed-history", true);
        let receipt = controller.apply_effect(&prepared, &execution.execution_id);
        assert_eq!(receipt.outcome, TakeoverOutcome::RecoveryPending);
        controller.restore_dns();
        assert!(!controller.has_marker());
        runtime.record_runtime_dns_restore_result("cold-prior-owner", true);
        {
            let mut state = runtime.runtime_config_apply.lock().unwrap();
            assert_eq!(state.host.dns_restored.len(), 1);
            assert_eq!(
                state.host.dns_restored[0],
                ("cold-prior-owner".into(), execution.generation)
            );
            assert_eq!(
                state
                    .host
                    .dns_cold
                    .as_ref()
                    .unwrap()
                    .restored_prior_marker
                    .as_ref()
                    .unwrap()
                    .restored_generation,
                execution.generation
            );
            // Start with real restore evidence, then remove or perturb exactly
            // its history axis. The eviction case calls the actual bounded
            // ledger method; these extra entries do not claim OS restoration.
            match mismatch {
                "missing" => state.host.dns_restored.clear(),
                "generation" => state.host.dns_restored[0].1 += 1,
                "marker" => state.host.dns_restored[0].0 = "other-history-marker".into(),
                "evicted" => {
                    for index in 0..8 {
                        state.host.remember_dns_restore(
                            &format!("bounded-entry-{index}"),
                            execution.generation,
                        );
                    }
                    assert_eq!(state.host.dns_restored.len(), 8);
                    assert!(!state
                        .host
                        .dns_restored
                        .iter()
                        .any(|(id, _)| id == "cold-prior-owner"));
                }
                _ => unreachable!(),
            }
        }
        runtime.record_runtime_cold_dns_result(execution.generation, execution.intent, receipt);
        assert!(runtime.host_effect_pending(), "{mismatch}");
        assert_eq!(runtime.accepted_host_dns_takeover(), None);
        let state = runtime.runtime_config_apply.lock().unwrap();
        let cold = state.host.dns_cold.as_ref().unwrap();
        assert!(!cold.in_flight);
        assert!(cold.cleanup_pending);
        assert!(state.host.dns_owner.is_none());
        assert!(state.host.dns_restored.len() <= 8);
    }
}

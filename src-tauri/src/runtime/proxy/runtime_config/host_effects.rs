//! Private host recipes and typed results. Physical snapshots and owner markers
//! stay inside the retained App owner; IPC receives digest-only previews.
use super::super::ProxyRuntime;
use super::{identity_equal, RuntimeApplyResult};
use polaris_config_engine::builder::GenerateConfigDeps;
use polaris_singbox_grpc::daemon;
use polaris_system_integration::proxy_ops::{TakeoverOutcome, TakeoverReceipt};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum HostEffectKind {
    Dns,
    SystemProxy,
    NativeTunnel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum HostEffectStrategy {
    HostOnly,
    AfterCoreCommit,
    NeedsSnapshotBeforeCore,
    BridgeOrRestart,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostEffectPreview {
    pub(crate) kind: HostEffectKind,
    pub(crate) scope_digest: String,
    pub(crate) before_digest: String,
    pub(crate) target_digest: String,
}

// No Serialize or Debug: recipe may contain complete private OS snapshots.
#[derive(Clone)]
pub(crate) struct HostEffectPlan {
    pub(crate) effect_digest: String,
    pub(crate) physical_facts_digest: String,
    pub(crate) host_epoch: u64,
    pub(crate) effects: Vec<HostEffectPreview>,
    pub(crate) strategy: HostEffectStrategy,
    pub(crate) recipe: Value,
}

#[derive(Clone)]
pub(crate) struct HostEffectExecution {
    pub(crate) execution_id: String,
    pub(crate) transaction_id: Option<String>,
    pub(crate) generation: u64,
    pub(crate) intent: u64,
    pub(crate) plan: HostEffectPlan,
    pub(crate) desired: Value,
    pub(crate) core_active: Option<daemon::RuntimeIdentity>,
    pub(crate) core_control_revision: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum HostEffectOutcome {
    Unchanged,
    Applied,
    FailedUnchanged,
    FailedRestored,
    RecoveryPending,
    Unknown,
    Superseded,
    Unsupported,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostEffectResult {
    pub(crate) preview: HostEffectPreview,
    pub(crate) outcome: HostEffectOutcome,
    pub(crate) receipt_id: Option<String>,
    pub(crate) cleanup_pending: bool,
    pub(crate) retryable: bool,
    pub(crate) reason_code: Option<String>,
}

#[derive(Clone)]
pub(crate) struct HostEffectReceipt {
    pub(crate) effect_digest: String,
    pub(crate) physical_facts_digest: String,
    pub(crate) host_epoch: u64,
    pub(crate) effects: Vec<HostEffectResult>,
}

#[derive(Clone)]
struct HostExecutionRecord {
    execution: HostEffectExecution,
    receipt: Option<HostEffectReceipt>,
    in_flight: bool,
    accepted: bool,
    result: Option<RuntimeApplyResult>,
}

// A late callback is retained as evidence, never as authority over a successor.
// Retired entries intentionally exclude the private desired config and recipe.
struct RetiredHostReceipt {
    execution_id: String,
    generation: u64,
    intent: u64,
    receipt: HostEffectReceipt,
}

/// Bounded DNS cleanup account, independent of the latest host execution.
/// The ID is the actual exact-marker owner, never a proxy-only execution ID.
#[derive(Clone)]
struct ColdDnsOwner {
    generation: u64,
    intent: u64,
    operation_id: String,
    prior_marker: Option<String>,
    in_flight: bool,
    cleanup_pending: bool,
}

#[derive(Clone)]
struct DnsMarkerOwner {
    generation: u64,
    marker_operation_id: String,
    cleanup_pending: bool,
    restored: bool,
    policy_required: bool,
}

#[derive(Default)]
pub(super) struct RuntimeHostState {
    generation: u64,
    accepted_deps: Option<GenerateConfigDeps>,
    accepted_config: Option<Value>,
    accepted_dns_takeover: Option<bool>,
    current: Option<HostExecutionRecord>,
    retired: VecDeque<RetiredHostReceipt>,
    dns_owner: Option<DnsMarkerOwner>,
    dns_cold: Option<ColdDnsOwner>,
    dns_policy_unconfirmed: Option<u64>,
    // Unattributable/evicted native cleanup stays conservative. An exact
    // execution with a temporarily unreadable marker belongs in the bounded
    // recoverable slot below, never in this irreversible overflow account.
    dns_unknown_cleanup: bool,
    dns_unknown_owner: Option<HostEffectExecution>,
    dns_retiring: VecDeque<HostEffectExecution>,
    dns_restored: VecDeque<(String, u64)>,
}

impl RuntimeHostState {
    pub(super) fn seed(&mut self, generation: u64, deps: GenerateConfigDeps) {
        self.generation = generation;
        self.accepted_deps = Some(deps);
        // Startup has not completed its physical owner transaction at capture.
        // Its desired config is not an observed host acceptance receipt.
        self.accepted_config = None;
    }

    fn dns_execution_restored(&self, execution: &HostEffectExecution) -> bool {
        self.dns_restored.iter().any(|(id, generation)| {
            id == &execution.execution_id && *generation == execution.generation
        })
    }

    fn retain_unreadable_dns_owner(&mut self, execution: &HostEffectExecution) {
        if self.dns_execution_restored(execution) {
            return;
        }
        // One recoverable private recipe is retained. Replacing a different
        // account cannot discard its cleanup: eviction becomes anonymous debt.
        if self.dns_unknown_owner.as_ref().is_some_and(|old| {
            old.execution_id != execution.execution_id
                || old.generation != execution.generation
                || old.intent != execution.intent
                || old.transaction_id != execution.transaction_id
                || old.core_active != execution.core_active
                || old.core_control_revision != execution.core_control_revision
                || old.desired != execution.desired
                || old.plan.effect_digest != execution.plan.effect_digest
                || old.plan.physical_facts_digest != execution.plan.physical_facts_digest
                || old.plan.host_epoch != execution.plan.host_epoch
                || old.plan.effects != execution.plan.effects
                || old.plan.strategy != execution.plan.strategy
                || old.plan.recipe != execution.plan.recipe
        }) {
            self.dns_unknown_cleanup = true;
        }
        self.dns_unknown_owner = Some(execution.clone());
    }

    fn remember_dns_restore(&mut self, operation_id: &str, generation: u64) {
        if !self
            .dns_restored
            .iter()
            .any(|(id, old)| id == operation_id && *old == generation)
        {
            if self.dns_restored.len() == 8 {
                self.dns_restored.pop_front();
            }
            self.dns_restored
                .push_back((operation_id.into(), generation));
        }
    }

    fn settle_retiring_dns(
        &mut self,
        execution: &HostEffectExecution,
        receipt: &HostEffectReceipt,
    ) {
        let Some(index) = self.dns_retiring.iter().position(|old| {
            old.execution_id == execution.execution_id
                && old.generation == execution.generation
                && old.intent == execution.intent
                && old.plan.effect_digest == execution.plan.effect_digest
                && old.plan.physical_facts_digest == execution.plan.physical_facts_digest
                && old.plan.recipe == execution.plan.recipe
                && valid_receipt(old, receipt)
        }) else {
            return;
        };
        self.dns_retiring.remove(index);
        if self.dns_restored.iter().any(|(id, generation)| {
            id == &execution.execution_id && *generation == execution.generation
        }) {
            return; // real original-owner restoration preceded this metadata
        }
        let Some(dns) = receipt
            .effects
            .iter()
            .find(|effect| effect.preview.kind == HostEffectKind::Dns)
        else {
            return;
        };
        let marker = dns
            .receipt_id
            .as_deref()
            .and_then(|id| id.strip_prefix(&format!("{}:", execution.execution_id)));
        if dns.outcome == HostEffectOutcome::Applied
            && !dns.cleanup_pending
            && !dns_execution_active(execution)
            && marker == Some(execution.execution_id.as_str())
        {
            let prior = super::super::dns_takeover::runtime_dns_plan_prior_owner(&execution.plan);
            if let Some(owner) = self.dns_owner.as_mut().filter(|owner| {
                owner.generation == execution.generation
                    && prior.as_deref() == Some(owner.marker_operation_id.as_str())
            }) {
                owner.cleanup_pending = false;
                owner.restored = true;
                owner.policy_required = false;
            }
            return; // actual complete disable, never a new running H acceptance
        }
        if dns.cleanup_pending || dns.outcome == HostEffectOutcome::Applied {
            if marker == Some(execution.execution_id.as_str()) {
                // Stale acceptance is not H authority, but its actual OS write
                // still owns cleanup until a checked restore proves otherwise.
                if self
                    .dns_owner
                    .as_ref()
                    .is_none_or(|owner| owner.generation <= execution.generation)
                {
                    self.dns_owner = Some(DnsMarkerOwner {
                        generation: execution.generation,
                        marker_operation_id: execution.execution_id.clone(),
                        cleanup_pending: true,
                        restored: false,
                        policy_required: false,
                    });
                }
            } else if dns.receipt_id.is_none() {
                self.retain_unreadable_dns_owner(execution);
            } else {
                // A supplied foreign/malformed marker is not unreadable own
                // evidence and cannot borrow this execution's cleanup proof.
                self.dns_unknown_cleanup = true;
            }
        }
    }

    fn retain(&mut self, execution: &HostEffectExecution, receipt: HostEffectReceipt) {
        let same_current = self.current.as_ref().is_some_and(|current| {
            current.execution.execution_id == execution.execution_id
                && current.execution.generation == execution.generation
                && current.execution.intent == execution.intent
                && current.execution.plan.recipe == execution.plan.recipe
                && valid_receipt(&current.execution, &receipt)
        });
        if same_current
            && receipt
                .effects
                .iter()
                .any(|effect| effect.preview.kind == HostEffectKind::Dns)
            && !self.dns_restored.iter().any(|(id, generation)| {
                id == &execution.execution_id && *generation == execution.generation
            })
            && !self
                .dns_retiring
                .iter()
                .any(|old| old.execution_id == execution.execution_id)
        {
            if self.dns_retiring.len() == 8 {
                self.dns_retiring.pop_front();
                self.dns_unknown_cleanup = true;
            }
            self.dns_retiring.push_back(execution.clone());
        }
        self.settle_retiring_dns(execution, &receipt);
        if self
            .retired
            .iter()
            .any(|old| old.execution_id == execution.execution_id)
        {
            return;
        }
        if self.retired.len() == 8 {
            self.retired.pop_front();
        }
        self.retired.push_back(RetiredHostReceipt {
            execution_id: execution.execution_id.clone(),
            generation: execution.generation,
            intent: execution.intent,
            receipt,
        });
    }

    fn dns_pending(&self) -> bool {
        self.dns_policy_unconfirmed == Some(self.generation)
            || self
                .dns_cold
                .as_ref()
                .is_some_and(|cold| cold.in_flight || cold.cleanup_pending)
            || self.dns_unknown_cleanup
            || self.dns_unknown_owner.is_some()
            || !self.dns_retiring.is_empty()
            || self.dns_owner.as_ref().is_some_and(|owner| {
                owner.cleanup_pending || owner.restored && owner.policy_required
            })
    }

    fn pending(&self) -> bool {
        self.dns_pending()
            || self
                .current
                .as_ref()
                .is_some_and(|current| !current.accepted)
    }

    /// Used only by the central reset after a real Core retirement/new baseline.
    /// Resetting generation cannot discharge unresolved native cleanup.
    pub(super) fn carry_dns_cleanup_from(&mut self, previous: &mut Self) {
        self.dns_restored = std::mem::take(&mut previous.dns_restored);
        self.dns_cold = previous.dns_cold.take();
        self.dns_retiring = std::mem::take(&mut previous.dns_retiring);
        self.dns_owner = previous.dns_owner.take().and_then(|mut owner| {
            owner.cleanup_pending |= owner.policy_required && !owner.restored;
            owner.policy_required = false;
            owner.cleanup_pending.then_some(owner)
        });
        self.dns_unknown_cleanup = previous.dns_unknown_cleanup;
        self.dns_unknown_owner = previous.dns_unknown_owner.take();
        if let Some(current) = previous.current.as_ref().filter(|current| {
            current.receipt.as_ref().is_some_and(|receipt| {
                receipt.effects.iter().any(|effect| {
                    effect.preview.kind == HostEffectKind::Dns
                        && effect.cleanup_pending
                        && effect.receipt_id.is_none()
                })
            })
        }) {
            // Restored E must not be recreated from its older pending receipt.
            self.retain_unreadable_dns_owner(&current.execution);
        }
        if let Some(current) = previous.current.as_ref().filter(|current| {
            current.in_flight
                && current
                    .execution
                    .plan
                    .effects
                    .iter()
                    .any(|preview| preview.kind == HostEffectKind::Dns)
        }) {
            if !self.dns_restored.iter().any(|(id, generation)| {
                id == &current.execution.execution_id && *generation == current.execution.generation
            }) && !self
                .dns_retiring
                .iter()
                .any(|old| old.execution_id == current.execution.execution_id)
            {
                if self.dns_retiring.len() == 8 {
                    self.dns_retiring.pop_front();
                    self.dns_unknown_cleanup = true;
                }
                self.dns_retiring.push_back(current.execution.clone());
            }
        }
    }

    pub(super) fn has_in_flight(&self) -> bool {
        self.dns_cold.as_ref().is_some_and(|cold| cold.in_flight)
            || self
                .current
                .as_ref()
                .is_some_and(|current| current.in_flight)
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending()
    }

    pub(super) fn remember_result(&mut self, result: &RuntimeApplyResult) {
        if let Some(current) = self.current.as_mut().filter(|current| {
            result.host_execution_id.as_ref() == Some(&current.execution.execution_id)
                && result.runtime_generation == Some(current.execution.generation)
                && result.selection_intent_generation == Some(current.execution.intent)
                && result.core_state.is_some()
        }) {
            let mut retained = result.clone();
            // A Core persistence retry can update this account without turning
            // a HostOnly execution into a newly issued Core transaction.
            retained.transaction_id = current.execution.transaction_id.clone();
            if retained.transaction_id.is_none() {
                retained.core_state = Some("unchanged");
            }
            current.result = Some(retained);
        }
    }
}

fn dns_config_requires_takeover(config: &Value) -> bool {
    config
        .get("proxyModeType")
        .cloned()
        .and_then(|value| {
            serde_json::from_value::<polaris_config_engine::user_config::ProxyModeType>(value).ok()
        })
        .is_some_and(|mode| mode.is_tun())
        && super::super::dns_takeover::dns_takeover_enabled(config) != Some(false)
}

fn dns_execution_active(execution: &HostEffectExecution) -> bool {
    dns_config_requires_takeover(&execution.desired)
}

fn valid_receipt(execution: &HostEffectExecution, receipt: &HostEffectReceipt) -> bool {
    receipt.effect_digest == execution.plan.effect_digest
        && receipt.physical_facts_digest == execution.plan.physical_facts_digest
        && receipt.host_epoch == execution.plan.host_epoch
        && receipt.effects.len() == execution.plan.effects.len()
        && receipt
            .effects
            .iter()
            .zip(&execution.plan.effects)
            .all(|(result, preview)| {
                &result.preview == preview
                    && (result.outcome != HostEffectOutcome::Applied
                        || result.receipt_id.as_ref().is_some_and(|id| !id.is_empty()))
                    && (result.outcome != HostEffectOutcome::Unchanged
                        || preview.before_digest == preview.target_digest)
            })
}

fn effect_accepted(result: &HostEffectResult) -> bool {
    !result.cleanup_pending
        && matches!(
            result.outcome,
            HostEffectOutcome::Unchanged | HostEffectOutcome::Applied
        )
}

fn merge_accepted_host_kind(accepted: &mut Value, desired: &Value, kind: HostEffectKind) {
    let Some(accepted) = accepted.as_object_mut() else {
        return;
    };
    let keys: &[&str] = match kind {
        HostEffectKind::SystemProxy => &[
            "proxyModeType",
            "mixedPort",
            "httpPort",
            "bypassLAN",
            "bypassLANList",
        ],
        HostEffectKind::Dns => &[],
        HostEffectKind::NativeTunnel => return,
    };
    for key in keys {
        if let Some(value) = desired.get(*key) {
            accepted.insert((*key).into(), value.clone());
        } else {
            accepted.remove(*key);
        }
    }
    if kind == HostEffectKind::Dns {
        let dns = accepted
            .entry("dnsConfig")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(dns) = dns.as_object_mut() {
            if let Some(value) = desired
                .get("dnsConfig")
                .and_then(|dns| dns.get("takeoverSystemDns"))
            {
                dns.insert("takeoverSystemDns".into(), value.clone());
            } else {
                dns.remove("takeoverSystemDns");
            }
        }
    }
}

impl ProxyRuntime {
    pub(in crate::runtime::proxy) fn runtime_host_execution_in_flight(&self) -> bool {
        self.runtime_config_apply
            .lock()
            .map(|state| state.host.has_in_flight())
            .unwrap_or(true)
    }

    pub(crate) fn host_effect_pending(&self) -> bool {
        self.runtime_config_apply
            .lock()
            .map(|state| state.host.pending())
            .unwrap_or(true)
    }

    pub(in crate::runtime::proxy) fn begin_runtime_cold_dns(
        &self,
        generation: u64,
        intent: u64,
        operation_id: &str,
    ) -> bool {
        if operation_id.is_empty()
            || !self.gate.is_busy()
            || !self.selector_operation_is_current(generation, intent)
        {
            return false;
        }
        let Some(running) = self.current_config_snapshot() else {
            return false;
        };
        if !dns_config_requires_takeover(&running) {
            return false;
        }
        let Ok(mut state) = self.runtime_config_apply.lock() else {
            return false;
        };
        if state.host.generation != generation
            || state.host.has_in_flight()
            || state.host.dns_cold.is_some()
        {
            return false;
        }
        state.host.dns_policy_unconfirmed = Some(generation);
        state.host.dns_cold = Some(ColdDnsOwner {
            generation,
            intent,
            operation_id: operation_id.into(),
            prior_marker: None,
            in_flight: true,
            cleanup_pending: false,
        });
        true
    }

    /// Prepared marker custody is registered outside the controller mutex,
    /// before its exact CAS/write. This also covers an unchanged older marker.
    pub(in crate::runtime::proxy) fn bind_runtime_cold_dns_marker(
        &self,
        operation_id: &str,
        prior_marker: Option<String>,
    ) -> bool {
        let Ok(mut state) = self.runtime_config_apply.lock() else {
            return false;
        };
        let Some(cold) = state
            .host
            .dns_cold
            .as_mut()
            .filter(|cold| cold.operation_id == operation_id && cold.in_flight)
        else {
            return false;
        };
        cold.prior_marker = prior_marker;
        true
    }

    /// C7 runs after real Core readiness while the lifecycle gate is still busy.
    /// This standalone OS receipt cannot become a HostExecution/Core transaction.
    pub(in crate::runtime::proxy) fn record_runtime_cold_dns_result(
        &self,
        generation: u64,
        intent: u64,
        receipt: TakeoverReceipt,
    ) {
        self.selector_reconcile.with_intent_claim(|current_intent| {
            let _publication = self.gate.lock_generation_publication();
            let current = self.gate.generation() == generation
                && current_intent == intent
                && self.core_running()
                && self
                    .current_config_snapshot()
                    .as_ref()
                    .is_some_and(dns_config_requires_takeover);
            let Ok(mut state) = self.runtime_config_apply.lock() else {
                return;
            };
            if state
                .host
                .dns_restored
                .iter()
                .any(|(id, old)| id == &receipt.operation_id && *old == generation)
            {
                if state
                    .host
                    .dns_cold
                    .as_ref()
                    .is_some_and(|cold| cold.operation_id == receipt.operation_id)
                {
                    state.host.dns_cold = None;
                }
                return;
            }
            if !state.host.dns_cold.as_ref().is_some_and(|cold| {
                cold.generation == generation
                    && cold.intent == intent
                    && cold.operation_id == receipt.operation_id
            }) {
                return;
            }
            let success = !receipt.cleanup_pending
                && matches!(
                    receipt.outcome,
                    TakeoverOutcome::Applied | TakeoverOutcome::Unchanged
                )
                && receipt.marker_id.as_ref().is_some_and(|id| !id.is_empty());
            if success {
                let marker = receipt.marker_id.as_ref().unwrap().clone();
                if current
                    || state
                        .host
                        .dns_owner
                        .as_ref()
                        .is_none_or(|owner| owner.generation <= generation)
                {
                    state.host.dns_owner = Some(DnsMarkerOwner {
                        generation,
                        marker_operation_id: marker,
                        cleanup_pending: !current,
                        restored: false,
                        policy_required: current,
                    });
                }
                state.host.dns_cold = None;
                if current {
                    state.host.dns_policy_unconfirmed = None;
                    state.host.accepted_dns_takeover = Some(true);
                    if let Some(deps) = state.host.accepted_deps.as_mut() {
                        deps.system_dns_takeover_active =
                            super::super::dns_takeover::system_dns_takeover_active(
                                self.helper.platform(),
                                true,
                                Some(true),
                            );
                    }
                }
            } else if receipt.cleanup_pending {
                if receipt.marker_id.as_deref() == Some(receipt.operation_id.as_str())
                    && state
                        .host
                        .dns_owner
                        .as_ref()
                        .is_none_or(|owner| owner.generation <= generation)
                {
                    state.host.dns_owner = Some(DnsMarkerOwner {
                        generation,
                        marker_operation_id: receipt.operation_id.clone(),
                        cleanup_pending: true,
                        restored: false,
                        policy_required: current,
                    });
                    state.host.dns_cold = None;
                } else if let Some(cold) = state.host.dns_cold.as_mut() {
                    cold.in_flight = false;
                    cold.cleanup_pending = true;
                }
            } else {
                state.host.dns_cold = None; // typed no-write/restoration, never H acceptance
            }
        });
        self.push_pending_changes();
    }

    pub(in crate::runtime::proxy) fn runtime_dns_effect_retry_required(
        &self,
        generation: u64,
        intent: u64,
    ) -> bool {
        if !self.selector_operation_is_current(generation, intent) {
            return false;
        }
        self.runtime_config_apply
            .lock()
            .map(|state| {
                state.host.dns_pending()
                    || state.host.current.as_ref().is_some_and(|current| {
                        current.execution.generation == generation
                            && current.receipt.as_ref().is_some_and(|receipt| {
                                receipt.effects.iter().any(|effect| {
                                    effect.preview.kind == HostEffectKind::Dns
                                        && !effect_accepted(effect)
                                })
                            })
                    })
            })
            .unwrap_or(true)
    }

    /// Called after the controller lock has been released. The leaf has observed
    /// the exact marker and service/link scope; this does not create a Core ACK.
    pub(in crate::runtime::proxy) fn record_runtime_dns_reconcile_result(
        &self,
        generation: u64,
        intent: u64,
        receipt: TakeoverReceipt,
    ) {
        let marker = receipt
            .marker_id
            .as_deref()
            .filter(|marker| !marker.is_empty());
        if marker != Some(receipt.operation_id.as_str()) {
            return;
        }
        self.selector_reconcile.with_intent_claim(|current_intent| {
            let _publication = self.gate.lock_generation_publication();
            if current_intent != intent || self.gate.generation() != generation
                || self.gate.is_busy() || !self.core_running() { return; }
            let Some(running) = self.current_config_snapshot() else { return; };
            let Ok(typed) = serde_json::from_value::<polaris_config_engine::user_config::UserConfig>(running.clone()) else { return; };
            let dns_required = typed.proxy_mode_type.effective_on(self.helper.platform()).is_tun()
                && super::super::dns_takeover::dns_takeover_enabled(&running) != Some(false);
            if !dns_required { return; }
            let Ok(mut state) = self.runtime_config_apply.lock() else { return; };
            if state.host.generation != generation || state.host.has_in_flight() { return; }
            let success = !receipt.cleanup_pending && matches!(receipt.outcome, TakeoverOutcome::Applied | TakeoverOutcome::Unchanged);
            let current_dns = state.host.current.as_ref().is_some_and(|current|
                current.execution.execution_id == receipt.operation_id && current.execution.generation == generation
                    && current.execution.plan.effects.iter().any(|effect| effect.kind == HostEffectKind::Dns));
            let known_owner = state.host.dns_owner.as_ref().is_some_and(|owner|
                owner.generation == generation && owner.marker_operation_id == receipt.operation_id);
            if !known_owner && !(success && (state.host.dns_owner.is_none() || current_dns)) { return; }
            state.host.dns_owner = Some(DnsMarkerOwner { generation, marker_operation_id: receipt.operation_id.clone(),
                cleanup_pending: !success, restored: false, policy_required: true });
            if let Some(current) = state.host.current.as_mut().filter(|current|
                current.execution.execution_id == receipt.operation_id && current.execution.generation == generation) {
                if let Some(own_receipt) = current.receipt.as_mut() {
                    for effect in own_receipt.effects.iter_mut().filter(|effect| effect.preview.kind == HostEffectKind::Dns) {
                        effect.outcome = if success { HostEffectOutcome::Applied } else { HostEffectOutcome::RecoveryPending };
                        effect.receipt_id = Some(format!("{}:{}", receipt.operation_id, receipt.operation_id));
                        effect.cleanup_pending = !success;
                        effect.retryable = !success;
                        effect.reason_code = (!success).then_some("HOST_DNS_RECONCILE_PENDING".into());
                    }
                    current.accepted = own_receipt.effects.iter().all(effect_accepted);
                }
            }
            if success {
                // This leaf read back the complete exact E scope. It resolves
                // E's uncertain write, not any foreign/evicted cleanup account.
                if state.host.dns_unknown_owner.as_ref().is_some_and(|owner| {
                    owner.execution_id == receipt.operation_id && owner.generation == generation
                }) {
                    state.host.dns_unknown_owner = None;
                }
                state.host.dns_policy_unconfirmed = None;
                state.host.accepted_dns_takeover = Some(true);
                if let Some(config) = state.host.accepted_config.as_mut() {
                    merge_accepted_host_kind(config, &running, HostEffectKind::Dns);
                }
                if let Some(deps) = state.host.accepted_deps.as_mut() {
                    deps.system_dns_takeover_active = super::super::dns_takeover::system_dns_takeover_active(
                        self.helper.platform(), true, Some(true));
                }
            }
        });
        self.push_pending_changes();
    }

    /// Resource cleanup is distinct from accepting a running DNS policy.
    /// A stale/foreign owner cannot clear a successor's debt.
    pub(in crate::runtime::proxy) fn record_runtime_dns_restore_result(
        &self,
        marker_operation_id: &str,
        restored: bool,
    ) {
        if marker_operation_id.is_empty() || !restored {
            return;
        }
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            let known = state
                .host
                .dns_owner
                .as_ref()
                .filter(|owner| owner.marker_operation_id == marker_operation_id)
                .map(|owner| owner.generation);
            let current = state
                .host
                .current
                .as_ref()
                .filter(|current| {
                    current
                        .execution
                        .plan
                        .effects
                        .iter()
                        .any(|preview| preview.kind == HostEffectKind::Dns)
                        && (current.execution.execution_id == marker_operation_id
                            || super::super::dns_takeover::runtime_dns_plan_prior_owner(
                                &current.execution.plan,
                            )
                            .as_deref()
                                == Some(marker_operation_id))
                })
                .map(|current| current.execution.generation);
            let unknown = state
                .host
                .dns_unknown_owner
                .as_ref()
                .filter(|old| {
                    old.execution_id == marker_operation_id
                        || super::super::dns_takeover::runtime_dns_plan_prior_owner(&old.plan)
                            .as_deref()
                            == Some(marker_operation_id)
                })
                .cloned();
            let restored_execution = state
                .host
                .current
                .as_ref()
                .filter(|current| {
                    current.execution.generation == state.host.generation
                        && current
                            .execution
                            .plan
                            .effects
                            .iter()
                            .any(|effect| effect.kind == HostEffectKind::Dns)
                        && (current.execution.execution_id == marker_operation_id
                            || super::super::dns_takeover::runtime_dns_plan_prior_owner(
                                &current.execution.plan,
                            )
                            .as_deref()
                                == Some(marker_operation_id))
                })
                .map(|current| current.execution.clone())
                .or_else(|| {
                    unknown
                        .as_ref()
                        .filter(|old| old.generation == state.host.generation)
                        .cloned()
                });
            let cold = state
                .host
                .dns_cold
                .as_ref()
                .filter(|cold| {
                    cold.operation_id == marker_operation_id
                        || cold.prior_marker.as_deref() == Some(marker_operation_id)
                })
                .map(|cold| cold.generation);
            let retiring = state
                .host
                .dns_retiring
                .iter()
                .find(|old| {
                    old.execution_id == marker_operation_id
                        || super::super::dns_takeover::runtime_dns_plan_prior_owner(&old.plan)
                            .as_deref()
                            == Some(marker_operation_id)
                })
                .map(|old| old.generation);
            if let Some(generation) = known
                .or(current)
                .or(retiring)
                .or_else(|| unknown.as_ref().map(|old| old.generation))
                .or(cold)
            {
                state
                    .host
                    .remember_dns_restore(marker_operation_id, generation);
                if let Some(old) = unknown.as_ref() {
                    // The recovered marker may be the original recipe's prior
                    // owner. Also close E so late metadata/reset cannot revive it.
                    state
                        .host
                        .remember_dns_restore(&old.execution_id, old.generation);
                }
                state
                    .host
                    .dns_retiring
                    .retain(|old| old.execution_id != marker_operation_id);
                if unknown.is_some() {
                    state.host.dns_unknown_owner = None;
                }
                if cold.is_some() {
                    state.host.dns_cold = None;
                }
                if let Some(restored_execution) = restored_execution.filter(|execution| {
                    state.host.dns_owner.as_ref().is_none_or(|owner| {
                        owner.marker_operation_id == marker_operation_id
                            || (execution.execution_id == marker_operation_id
                                && owner.generation <= execution.generation
                                && super::super::dns_takeover::runtime_dns_plan_prior_owner(
                                    &execution.plan,
                                )
                                .as_deref()
                                    == Some(owner.marker_operation_id.as_str()))
                    })
                }) {
                    // E's checked original restoration also covers its recipe's
                    // recorded predecessor. An unrelated successor cannot match.
                    state.host.dns_owner = Some(DnsMarkerOwner {
                        generation: state.host.generation,
                        marker_operation_id: marker_operation_id.into(),
                        cleanup_pending: false,
                        restored: true,
                        policy_required: dns_execution_active(&restored_execution),
                    });
                }
                // Keep the receipt's target unaccepted while projecting the
                // now-proven physical cleanup. No Core/persistence ACK is made.
                if let Some(current) = state.host.current.as_mut().filter(|current| {
                    current.execution.execution_id == marker_operation_id
                        || unknown.as_ref().is_some_and(|old| {
                            current.execution.execution_id == old.execution_id
                                && current.execution.generation == old.generation
                                && current.execution.plan.recipe == old.plan.recipe
                        })
                }) {
                    if let Some(receipt) = current.receipt.as_mut() {
                        for effect in receipt
                            .effects
                            .iter_mut()
                            .filter(|effect| effect.preview.kind == HostEffectKind::Dns)
                        {
                            effect.outcome = HostEffectOutcome::FailedRestored;
                            effect.cleanup_pending = false;
                            effect.retryable = true;
                            effect.reason_code = Some("HOST_DNS_RECONCILE_PENDING".into());
                        }
                    }
                    current.accepted = false;
                }
                let invalidates_h = state.host.dns_owner.as_ref().is_some_and(|owner| {
                    owner.generation == state.host.generation
                        && owner.marker_operation_id == marker_operation_id
                });
                if let Some(owner) = state
                    .host
                    .dns_owner
                    .as_mut()
                    .filter(|owner| owner.marker_operation_id == marker_operation_id)
                {
                    owner.cleanup_pending = false;
                    owner.restored = true;
                }
                if invalidates_h {
                    state.host.accepted_dns_takeover = None;
                }
                if state.host.dns_owner.as_ref().is_some_and(|owner| {
                    !owner.cleanup_pending && owner.restored && !owner.policy_required
                }) {
                    state.host.dns_owner = None;
                }
            }
        }
        self.push_pending_changes();
    }

    /// A checked restore is observed physical off, not acceptance of a new
    /// running policy. Keep the original owner/policy debt independently.
    pub(in crate::runtime::proxy) fn runtime_host_dns_physical_active(
        &self,
        generation: u64,
    ) -> Option<bool> {
        let state = self.runtime_config_apply.lock().ok()?;
        if state.host.generation != generation {
            return None;
        }
        if state
            .host
            .dns_owner
            .as_ref()
            .is_some_and(|owner| owner.generation == generation && owner.restored)
        {
            Some(false)
        } else {
            state.host.accepted_dns_takeover
        }
    }

    /// Candidate generation may reuse an observed H projection. It never
    /// converts the requested DNS flag into an already-accepted physical fact.
    pub(super) fn runtime_host_dns_generation_deps(
        &self,
        generation: u64,
        core_deps: &GenerateConfigDeps,
    ) -> Option<GenerateConfigDeps> {
        let physical_active = self.runtime_host_dns_physical_active(generation);
        let current_dns_required = self
            .current_config_snapshot()
            .as_ref()
            .is_some_and(dns_config_requires_takeover);
        let needs_observation = {
            let state = self.runtime_config_apply.lock().ok()?;
            if state.host.generation != generation {
                return None;
            }
            physical_active.is_none()
                && current_dns_required
                && matches!(
                    self.helper.platform(),
                    polaris_helper_proto::Platform::Mac | polaris_helper_proto::Platform::Linux
                )
        };
        if needs_observation && !self.gate.is_busy() && !self.runtime_host_execution_in_flight() {
            let observed = if self.helper.platform() == polaris_helper_proto::Platform::Linux {
                self.linux_resolved_controller
                    .lock()
                    .ok()
                    .and_then(|controller| controller.checked_owned_receipt().ok().flatten())
            } else {
                self.dns_controller
                    .lock()
                    .ok()
                    .and_then(|controller| controller.checked_owned_receipt().ok().flatten())
            };
            if let Some(receipt) = observed {
                self.record_runtime_dns_reconcile_result(
                    generation,
                    self.selector_reconcile.intent_generation(),
                    receipt,
                );
            }
        }
        let state = self.runtime_config_apply.lock().ok()?;
        if state.host.generation != generation
            || needs_observation && state.host.accepted_dns_takeover.is_none()
        {
            return None;
        }
        let mut projected = core_deps.clone();
        if state
            .host
            .dns_owner
            .as_ref()
            .is_some_and(|owner| owner.generation == generation && owner.restored)
        {
            projected.system_dns_takeover_active = false;
        } else if state.host.accepted_dns_takeover.is_some() {
            projected.system_dns_takeover_active = state
                .host
                .accepted_deps
                .as_ref()?
                .system_dns_takeover_active;
        }
        Some(projected)
    }

    pub(crate) fn runtime_accepted_host_deps(&self, generation: u64) -> Option<GenerateConfigDeps> {
        self.runtime_config_apply.lock().ok().and_then(|state| {
            (state.host.generation == generation)
                .then(|| state.host.accepted_deps.clone())
                .flatten()
        })
    }

    pub(crate) fn runtime_accepted_host_config(&self, generation: u64) -> Option<Value> {
        self.runtime_config_apply.lock().ok().and_then(|state| {
            (state.host.generation == generation)
                .then(|| state.host.accepted_config.clone())
                .flatten()
        })
    }

    pub(crate) fn accepted_host_dns_takeover(&self) -> Option<bool> {
        self.runtime_config_apply.lock().ok().and_then(|state| {
            if state.host.generation != self.gate.generation() {
                return None;
            }
            state.host.accepted_dns_takeover
        })
    }

    pub(crate) fn runtime_host_execution_is_current(
        &self,
        execution: &HostEffectExecution,
    ) -> bool {
        if self.gate.is_busy()
            || !self.core_running()
            || !self.selector_operation_is_current(execution.generation, execution.intent)
        {
            return false;
        }
        let core_matches = self.runtime_config_baseline.read().is_ok_and(|baseline| {
            baseline.as_ref().is_some_and(|baseline| {
                baseline.generation == execution.generation
                    && identity_equal(
                        baseline.observed_active.as_ref(),
                        execution.core_active.as_ref(),
                    )
                    && baseline.observed_control_revision == execution.core_control_revision
            })
        });
        if !core_matches {
            return false;
        }
        // No controller calls or D reads under this lock: workers invoke this
        // check while holding the original DNS/proxy controller owner.
        self.runtime_config_apply.lock().is_ok_and(|state| {
            state.host.current.as_ref().is_some_and(|current| {
                current.in_flight
                    && current.execution.execution_id == execution.execution_id
                    && current.execution.generation == execution.generation
                    && current.execution.intent == execution.intent
                    && current.execution.transaction_id == execution.transaction_id
                    && current.execution.core_active == execution.core_active
                    && current.execution.core_control_revision == execution.core_control_revision
                    && current.execution.desired == execution.desired
                    && current.execution.plan.effect_digest == execution.plan.effect_digest
                    && current.execution.plan.physical_facts_digest
                        == execution.plan.physical_facts_digest
                    && current.execution.plan.host_epoch == execution.plan.host_epoch
                    && current.execution.plan.strategy == execution.plan.strategy
                    && current.execution.plan.effects == execution.plan.effects
                    && current.execution.plan.recipe == execution.plan.recipe
            })
        })
    }

    pub(crate) fn runtime_host_listener_ready(
        &self,
        execution: &HostEffectExecution,
        port: u16,
    ) -> bool {
        if port == 0 || !self.runtime_host_execution_is_current(execution) {
            return false;
        }
        let ready = self.runtime_config_baseline.read().is_ok_and(|baseline| {
            baseline.as_ref().is_some_and(|baseline| {
                baseline.generation == execution.generation
                    && identity_equal(
                        baseline.observed_active.as_ref(),
                        execution.core_active.as_ref(),
                    )
                    && baseline.observed_control_revision == execution.core_control_revision
                    && execution.transaction_id.as_ref().is_none_or(|transaction| {
                        baseline.committed_transaction.as_ref() == Some(transaction)
                    })
                    && baseline.generated.inbounds.iter().any(|inbound| {
                        inbound.tag == "mixed-in"
                            && inbound.type_field == "mixed"
                            && inbound.listen_port == Some(port)
                            && matches!(
                                inbound.listen.as_deref(),
                                Some("127.0.0.1" | "0.0.0.0" | "::")
                            )
                    })
            })
        });
        ready
            && self
                .status
                .read()
                .is_ok_and(|status| status.mixed_port == port)
            && self.runtime_host_execution_is_current(execution)
    }

    pub(super) fn begin_runtime_host_execution(
        &self,
        execution: HostEffectExecution,
        running: &Value,
    ) -> bool {
        if !self.selector_operation_is_current(execution.generation, execution.intent) {
            return false;
        }
        let Ok(mut state) = self.runtime_config_apply.lock() else {
            return false;
        };
        if state.host.generation != execution.generation
            || state
                .host
                .current
                .as_ref()
                .is_some_and(|current| current.in_flight)
        {
            return false;
        }
        if let Some(old) = state.host.current.take() {
            if let Some(receipt) = old.receipt {
                state.host.retain(&old.execution, receipt);
            }
        }
        // Freeze the prior host input projection before R advances. Physical
        // acceptance still requires the controller receipt below, even at startup.
        if state.host.accepted_config.is_none() {
            state.host.accepted_config = Some(running.clone());
        }
        state.host.current = Some(HostExecutionRecord {
            execution,
            receipt: None,
            in_flight: true,
            accepted: false,
            result: None,
        });
        true
    }

    pub(crate) fn record_runtime_host_effect_result(
        &self,
        execution: &HostEffectExecution,
        receipt: HostEffectReceipt,
    ) {
        self.selector_reconcile.with_intent_claim(|intent| {
            let _publication = self.gate.lock_generation_publication();
            let current_owner = intent == execution.intent
                && self.gate.generation() == execution.generation
                && !self.gate.is_busy()
                && self.core_running();
            let Ok(mut state) = self.runtime_config_apply.lock() else {
                return;
            };
            let owns_record = state.host.current.as_ref().is_some_and(|current| {
                current.execution.execution_id == execution.execution_id
                    && current.execution.generation == execution.generation
                    && current.execution.intent == execution.intent
            });
            if !owns_record {
                state.host.retain(execution, receipt);
                return;
            }
            // Duplicate completion cannot replace a receipt, clear debt, or replay H.
            if state
                .host
                .current
                .as_ref()
                .is_some_and(|current| !current.in_flight)
            {
                return;
            }
            let registered = state.host.current.as_ref().unwrap().execution.clone();
            let verified = registered.transaction_id == execution.transaction_id
                && registered.desired == execution.desired
                && registered.core_active == execution.core_active
                && registered.core_control_revision == execution.core_control_revision
                && registered.plan.effect_digest == execution.plan.effect_digest
                && registered.plan.physical_facts_digest == execution.plan.physical_facts_digest
                && registered.plan.host_epoch == execution.plan.host_epoch
                && registered.plan.effects == execution.plan.effects
                && registered.plan.strategy == execution.plan.strategy
                && registered.plan.recipe == execution.plan.recipe
                && valid_receipt(&registered, &receipt);
            let dns_restored = state.host.dns_execution_restored(&registered)
                || (verified
                    && receipt.effects.iter().any(|effect| {
                        effect.preview.kind == HostEffectKind::Dns
                            && effect.outcome == HostEffectOutcome::Unchanged
                    })
                    && super::super::dns_takeover::runtime_dns_plan_prior_owner(&registered.plan)
                        .is_some_and(|prior| {
                            state.host.dns_restored.iter().any(|(id, _)| id == &prior)
                        }));
            if verified && dns_restored {
                // An Unchanged receipt owns no new write: restoring its exact
                // prior marker before this callback also closes E's account.
                state
                    .host
                    .remember_dns_restore(&registered.execution_id, registered.generation);
            }
            let accepted = current_owner
                && verified
                && receipt.effects.iter().all(effect_accepted)
                && !(dns_restored
                    && receipt
                        .effects
                        .iter()
                        .any(|effect| effect.preview.kind == HostEffectKind::Dns));
            if let Some(current) = state.host.current.as_mut() {
                current.in_flight = false;
                current.accepted = accepted;
                current.receipt = Some(if verified {
                    let mut observed = receipt.clone();
                    if dns_restored {
                        for effect in observed
                            .effects
                            .iter_mut()
                            .filter(|effect| effect.preview.kind == HostEffectKind::Dns)
                        {
                            effect.outcome = HostEffectOutcome::FailedRestored;
                            effect.cleanup_pending = false;
                            effect.retryable = true;
                            effect.reason_code = Some("HOST_DNS_RECONCILE_PENDING".into());
                        }
                    }
                    observed
                } else {
                    HostEffectReceipt {
                        effect_digest: registered.plan.effect_digest.clone(),
                        physical_facts_digest: registered.plan.physical_facts_digest.clone(),
                        host_epoch: registered.plan.host_epoch,
                        effects: registered
                            .plan
                            .effects
                            .iter()
                            .cloned()
                            .map(|preview| HostEffectResult {
                                preview,
                                outcome: HostEffectOutcome::Unknown,
                                receipt_id: None,
                                cleanup_pending: true,
                                retryable: false,
                                reason_code: Some("HOST_RECEIPT_OWNER_MISMATCH".into()),
                            })
                            .collect(),
                    }
                });
            }
            if current_owner && verified {
                // Each OS owner advances independently: a successful DNS write is
                // still accepted when the proxy failed. Keep the other H inputs.
                for effect in receipt.effects.iter().filter(|effect| {
                    effect_accepted(effect)
                        && !(dns_restored && effect.preview.kind == HostEffectKind::Dns)
                }) {
                    if let Some(config) = state.host.accepted_config.as_mut() {
                        merge_accepted_host_kind(config, &execution.desired, effect.preview.kind);
                    }
                    if effect.preview.kind == HostEffectKind::Dns {
                        state.host.dns_policy_unconfirmed = None;
                        let required = serde_json::from_value::<
                            polaris_config_engine::user_config::UserConfig,
                        >(execution.desired.clone())
                        .is_ok_and(|typed| {
                            typed
                                .proxy_mode_type
                                .effective_on(self.helper.platform())
                                .is_tun()
                                && super::super::dns_takeover::dns_takeover_enabled(
                                    &execution.desired,
                                ) != Some(false)
                        });
                        if effect.outcome == HostEffectOutcome::Applied {
                            // A fresh checked DNS acceptance covers its complete
                            // restoration scope. Proxy-only/Unchanged cannot do this.
                            if state
                                .host
                                .dns_unknown_owner
                                .as_ref()
                                .is_some_and(|unknown| {
                                    unknown.generation == execution.generation
                                        && super::super::dns_takeover::runtime_dns_plan_prior_owner(
                                            &execution.plan,
                                        )
                                        .as_deref()
                                            == Some(unknown.execution_id.as_str())
                                })
                            {
                                state.host.dns_unknown_owner = None;
                            }
                            state.host.dns_owner = required.then(|| DnsMarkerOwner {
                                generation: execution.generation,
                                marker_operation_id: execution.execution_id.clone(),
                                cleanup_pending: false,
                                restored: false,
                                policy_required: true,
                            });
                        }
                        if required
                            && effect.outcome == HostEffectOutcome::Unchanged
                            && state.host.dns_owner.is_none()
                        {
                            if let Some(original_owner) =
                                super::super::dns_takeover::runtime_dns_plan_prior_owner(
                                    &execution.plan,
                                )
                            {
                                state.host.dns_owner = Some(DnsMarkerOwner {
                                    generation: execution.generation,
                                    marker_operation_id: original_owner,
                                    cleanup_pending: false,
                                    restored: false,
                                    policy_required: true,
                                });
                            }
                        }
                        if !required && effect.outcome == HostEffectOutcome::Unchanged {
                            if let Some(owner) = state.host.dns_owner.as_mut() {
                                owner.policy_required = false;
                            }
                        }
                        state.host.accepted_dns_takeover = Some(
                            super::super::dns_takeover::dns_takeover_enabled(&execution.desired)
                                != Some(false),
                        );
                        if let Ok(typed) = serde_json::from_value::<
                            polaris_config_engine::user_config::UserConfig,
                        >(execution.desired.clone())
                        {
                            if let Some(deps) = state.host.accepted_deps.as_mut() {
                                deps.system_dns_takeover_active =
                                    super::super::dns_takeover::system_dns_takeover_active(
                                        self.helper.platform(),
                                        typed
                                            .proxy_mode_type
                                            .effective_on(self.helper.platform())
                                            .is_tun(),
                                        super::super::dns_takeover::dns_takeover_enabled(
                                            &execution.desired,
                                        ),
                                    );
                            }
                        }
                    }
                }
            }
            if current_owner && verified {
                for effect in receipt.effects.iter().filter(|effect| {
                    effect.preview.kind == HostEffectKind::Dns
                        && effect.cleanup_pending
                        && !dns_restored
                }) {
                    let prefix = format!("{}:", execution.execution_id);
                    let marker = effect
                        .receipt_id
                        .as_deref()
                        .and_then(|id| id.strip_prefix(&prefix));
                    let retained_owner = state
                        .host
                        .dns_owner
                        .as_ref()
                        .map(|owner| owner.marker_operation_id.as_str());
                    if let Some(marker) = marker.filter(|marker| {
                        !marker.is_empty()
                            && (*marker == execution.execution_id
                                || Some(*marker) == retained_owner)
                    }) {
                        state.host.dns_owner = Some(DnsMarkerOwner {
                            generation: execution.generation,
                            marker_operation_id: marker.into(),
                            cleanup_pending: true,
                            restored: false,
                            policy_required: true,
                        });
                    } else if effect.receipt_id.is_none() {
                        // The marker is unreadable, but this registered E and
                        // its original recipe are known recovery authority.
                        state.host.retain_unreadable_dns_owner(&registered);
                    } else {
                        state.host.dns_unknown_cleanup = true;
                    }
                }
            }
            if !current_owner || !verified {
                state.host.retain(execution, receipt);
            }
            drop(state);
        });
        self.push_pending_changes();
    }

    pub(super) fn annotate_runtime_host_result(&self, result: &mut RuntimeApplyResult) {
        if result.core_state.is_none()
            && result.host_execution_id.is_none()
            && result.transaction_id.is_none()
        {
            return;
        }
        let Ok(state) = self.runtime_config_apply.lock() else {
            return;
        };
        let Some(current) = state.host.current.as_ref().filter(|current| {
            current.execution.generation == self.gate.generation()
                && result
                    .host_execution_id
                    .as_ref()
                    .is_none_or(|id| id == &current.execution.execution_id)
                && result
                    .runtime_generation
                    .is_none_or(|generation| generation == current.execution.generation)
                && result
                    .selection_intent_generation
                    .is_none_or(|intent| intent == current.execution.intent)
                && result.transaction_id.as_ref().is_none_or(|transaction| {
                    current.execution.transaction_id.as_ref() == Some(transaction)
                        || state.kernel.as_ref().is_some_and(|kernel| {
                            kernel.generation == current.execution.generation
                                && kernel.operation.transaction_id == *transaction
                                && identity_equal(
                                    kernel.operation.active.as_ref(),
                                    current.execution.core_active.as_ref(),
                                )
                        })
                })
        }) else {
            return;
        };
        result.host_execution_id = Some(current.execution.execution_id.clone());
        if let Some(kernel) = state.kernel.as_ref().filter(|kernel| {
            kernel.generation == current.execution.generation
                && !kernel.superseded
                && identity_equal(
                    kernel.operation.active.as_ref(),
                    current.execution.core_active.as_ref(),
                )
        }) {
            let durable = kernel.operation.durable
                && kernel.operation.state == daemon::RuntimeOperationState::Committed as i32;
            result.configuration_durable = Some(durable);
            result.persistence_transaction_id =
                (!durable).then(|| kernel.operation.transaction_id.clone());
            result.persistence_mode = Some(kernel.operation.persistence_mode.clone());
            result.retirement_state = Some(kernel.operation.retirement_state.clone());
        }
        if let Some(receipt) = &current.receipt {
            result.host_effects = receipt.effects.clone();
        }
        if state.host.dns_pending() {
            result.pending_apply = true;
            if !result
                .reasons
                .iter()
                .any(|reason| reason.code == "HOST_DNS_RECONCILE_PENDING")
            {
                result.reasons.push(super::RuntimeApplyReason {
                    code: "HOST_DNS_RECONCILE_PENDING",
                    path: "dnsConfig.takeoverSystemDns".into(),
                });
            }
        }
        if !current.accepted {
            result.pending_apply = true;
            if matches!(
                result.status,
                "hotApplied" | "hotDurabilityPending" | "hostEffectsPending" | "hostEffectsFailed"
            ) {
                result.status = if current.in_flight {
                    "hostEffectsPending"
                } else {
                    "hostEffectsFailed"
                };
                result.ok = false;
            }
        } else if matches!(result.status, "hostEffectsPending" | "hostEffectsFailed") {
            result.status = if result.configuration_durable == Some(false) {
                "hotDurabilityPending"
            } else {
                "hotApplied"
            };
            result.ok = true;
            result.pending_apply =
                result.configuration_durable == Some(false) || state.host.dns_pending();
        }
    }

    pub(crate) async fn runtime_host_apply_status(
        self: &std::sync::Arc<Self>,
        execution_id: String,
    ) -> RuntimeApplyResult {
        let generation = self.gate.generation();
        let cached = self.runtime_config_apply.lock().ok().and_then(|state| {
            if let Some(current) = state.host.current.as_ref().filter(|current| {
                current.execution.execution_id == execution_id
                    && current.execution.generation == generation
                    && self.selector_operation_is_current(generation, current.execution.intent)
            }) {
                return current
                    .result
                    .as_ref()
                    .filter(|last| {
                        last.host_execution_id.as_deref() == Some(execution_id.as_str())
                            && last.runtime_generation == Some(current.execution.generation)
                    })
                    .cloned();
            }
            state
                .host
                .retired
                .iter()
                .find(|old| old.execution_id == execution_id)
                .map(|old| {
                    let mut result = RuntimeApplyResult::status("conflict");
                    result.host_execution_id = Some(old.execution_id.clone());
                    result.runtime_generation = Some(old.generation);
                    result.selection_intent_generation = Some(old.intent);
                    // Retained evidence describes that old execution only.
                    result.pending_apply = old
                        .receipt
                        .effects
                        .iter()
                        .any(|effect| effect.cleanup_pending);
                    result
                })
        });
        let Some(mut result) = cached else {
            return RuntimeApplyResult::status("conflict");
        };
        if result.runtime_generation != Some(generation)
            || !self.core_running()
            || self.gate.is_busy()
        {
            return RuntimeApplyResult::status("conflict");
        }
        if let Err(error) = self.verify_cached_runtime_pairing(generation).await {
            return *error;
        }
        self.annotate_runtime_host_result(&mut result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn receipt(
        execution: &HostEffectExecution,
        outcomes: [HostEffectOutcome; 2],
    ) -> HostEffectReceipt {
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        assert_eq!(
            runtime
                .runtime_config_apply
                .lock()
                .unwrap()
                .host
                .current
                .as_ref()
                .unwrap()
                .in_flight,
            false
        );
    }

    #[test]
    fn late_and_duplicate_host_callbacks_cannot_accept_a_successor() {
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        runtime.record_runtime_host_effect_result(
            &next,
            receipt(&next, [HostEffectOutcome::Applied; 2]),
        );
        assert!(!runtime.host_effect_pending());
        runtime.record_runtime_host_effect_result(
            &next,
            receipt(&next, [HostEffectOutcome::Unknown; 2]),
        );
        assert!(!runtime.host_effect_pending());
    }

    #[test]
    fn changed_intent_and_mismatched_receipts_never_advance_host_acceptance() {
        for mismatch in [false, true] {
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError>
        {
            Ok(self.values.lock().unwrap().keys().cloned().collect())
        }
        fn read_dns(
            &self,
            name: &str,
        ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError>
        {
            self.values
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .ok_or_else(|| {
                    polaris_system_integration::error::SystemIntegrationError::dns(
                        "missing service",
                    )
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
        ) -> Result<Vec<String>, polaris_system_integration::error::SystemIntegrationError>
        {
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
        use sha2::Digest as _;
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
        let digest =
            |value: Value| format!("{:x}", sha2::Sha256::digest(value.to_string().as_bytes()));
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
    fn actual_dns_receipt(
        execution: &HostEffectExecution,
        leaf: TakeoverReceipt,
    ) -> HostEffectReceipt {
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
            let (execution, mut controller, prepared, values, faults) =
                dns_execution_fixture_with_faults(&runtime, "late-unreadable-E");
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
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
            runtime.record_runtime_host_effect_result(
                &execution,
                actual_dns_receipt(&execution, leaf),
            );
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        runtime.record_runtime_host_effect_result(
            &execution,
            actual_dns_receipt(&execution, uncertain),
        );
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
        runtime.record_runtime_dns_reconcile_result(
            execution.generation,
            execution.intent,
            foreign,
        );
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
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        use sha2::Digest as _;
        for successor in [false, true] {
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
            let (mut execution, mut controller, prepared, values, faults) =
                dns_execution_fixture_with_faults(&runtime, "disable-E");
            assert_eq!(
                controller.apply_effect(&prepared, "original-A").outcome,
                TakeoverOutcome::Applied
            );
            let disable = controller.prepare_effect(false).unwrap();
            execution.desired["dnsConfig"]["takeoverSystemDns"] = json!(false);
            execution.plan.recipe["dns"] = serde_json::to_value(&disable).unwrap();
            execution.plan.recipe["desired_digest"] = json!(format!(
                "{:x}",
                sha2::Sha256::digest(execution.desired.to_string().as_bytes())
            ));
            execution.plan.effects[0].before_digest = format!(
                "{:x}",
                sha2::Sha256::digest(
                    serde_json::to_value(disable.before())
                        .unwrap()
                        .to_string()
                        .as_bytes()
                )
            );
            execution.plan.effects[0].target_digest = format!(
                "{:x}",
                sha2::Sha256::digest(
                    serde_json::to_value(disable.target())
                        .unwrap()
                        .to_string()
                        .as_bytes()
                )
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
            runtime.record_runtime_host_effect_result(
                &execution,
                actual_dns_receipt(&execution, pending),
            );
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let (execution, mut controller, prepared, values) =
            dns_execution_fixture(&runtime, "DNS-owned");
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        let applied = controller.apply_effect(&prepared, &execution.execution_id);
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
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
            assert!(
                current.receipt.as_ref().unwrap().effects[0].outcome == HostEffectOutcome::Applied
            );
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let (execution, mut controller, prepared, values) =
            dns_execution_fixture(&runtime, "fixed-DNS-owner");
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        let applied = controller.apply_effect(&prepared, &execution.execution_id);
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let (execution, mut controller, prepared, values) =
            dns_execution_fixture(&runtime, "published-policy");
        assert!(runtime.begin_runtime_host_execution(execution.clone(), &json!({})));
        let applied = controller.apply_effect(&prepared, &execution.execution_id);
        runtime
            .record_runtime_host_effect_result(&execution, actual_dns_receipt(&execution, applied));
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
            let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
            runtime.gate.begin();
            let (execution, mut controller, prepared, values) =
                dns_execution_fixture(&runtime, "dns-startup:restore-first");
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
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
        runtime.record_runtime_dns_reconcile_result(
            execution.generation,
            execution.intent,
            checked,
        );
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

        let (failed_runtime, _failed_dir, _failed_calls) =
            crate::runtime::proxy::tests::test_runtime_recording();
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
}

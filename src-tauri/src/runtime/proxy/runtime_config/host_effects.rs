//! Private host recipes and typed results. Physical snapshots and owner markers
//! stay inside the retained App owner; IPC receives digest-only previews.
use super::super::ProxyRuntime;
use super::{identity_equal, RuntimeApplyResult};
use polaris_config_engine::builder::helpers::MIXED_INBOUND_TAG;
use polaris_config_engine::builder::GenerateConfigDeps;
use polaris_config_engine::user_config::UserConfig;
use polaris_helper_proto::Platform;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum HostEffectStrategy {
    HostOnly,
    AfterCoreCommit,
    NeedsSnapshotBeforeCore,
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
        platform: Platform,
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
            && !dns_execution_active(execution, platform)
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

    fn retain(
        &mut self,
        execution: &HostEffectExecution,
        receipt: HostEffectReceipt,
        platform: Platform,
    ) {
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
        self.settle_retiring_dns(execution, &receipt, platform);
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

fn dns_config_requires_takeover(config: &Value, platform: Platform) -> bool {
    // Host DNS custody is separate from a native TUN's effective mode. Only
    // these two platforms have a checked host-DNS adapter in this owner.
    match platform {
        Platform::Mac | Platform::Linux => {}
        Platform::Win | Platform::Android | Platform::Ios | Platform::Other => return false,
    }
    serde_json::from_value::<UserConfig>(config.clone()).is_ok_and(|typed| {
        typed.proxy_mode_type.effective_on(platform).is_tun()
            && super::super::dns_takeover::dns_takeover_enabled(config) != Some(false)
    })
}

fn dns_execution_active(execution: &HostEffectExecution, platform: Platform) -> bool {
    dns_config_requires_takeover(&execution.desired, platform)
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
        if !dns_config_requires_takeover(&running, self.helper.platform()) {
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
                    .is_some_and(|config| {
                        dns_config_requires_takeover(config, self.helper.platform())
                    });
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
            if current_intent != intent
                || self.gate.generation() != generation
                || self.gate.is_busy()
                || !self.core_running()
            {
                return;
            }
            let Some(running) = self.current_config_snapshot() else {
                return;
            };
            let dns_required = dns_config_requires_takeover(&running, self.helper.platform());
            if !dns_required {
                return;
            }
            let Ok(mut state) = self.runtime_config_apply.lock() else {
                return;
            };
            if state.host.generation != generation || state.host.has_in_flight() {
                return;
            }
            let success = !receipt.cleanup_pending
                && matches!(
                    receipt.outcome,
                    TakeoverOutcome::Applied | TakeoverOutcome::Unchanged
                );
            let current_dns = state.host.current.as_ref().is_some_and(|current| {
                current.execution.execution_id == receipt.operation_id
                    && current.execution.generation == generation
                    && current
                        .execution
                        .plan
                        .effects
                        .iter()
                        .any(|effect| effect.kind == HostEffectKind::Dns)
            });
            let known_owner = state.host.dns_owner.as_ref().is_some_and(|owner| {
                owner.generation == generation && owner.marker_operation_id == receipt.operation_id
            });
            if !known_owner && !(success && (state.host.dns_owner.is_none() || current_dns)) {
                return;
            }
            state.host.dns_owner = Some(DnsMarkerOwner {
                generation,
                marker_operation_id: receipt.operation_id.clone(),
                cleanup_pending: !success,
                restored: false,
                policy_required: true,
            });
            if let Some(current) = state.host.current.as_mut().filter(|current| {
                current.execution.execution_id == receipt.operation_id
                    && current.execution.generation == generation
            }) {
                if let Some(own_receipt) = current.receipt.as_mut() {
                    for effect in own_receipt
                        .effects
                        .iter_mut()
                        .filter(|effect| effect.preview.kind == HostEffectKind::Dns)
                    {
                        effect.outcome = if success {
                            HostEffectOutcome::Applied
                        } else {
                            HostEffectOutcome::RecoveryPending
                        };
                        effect.receipt_id =
                            Some(format!("{}:{}", receipt.operation_id, receipt.operation_id));
                        effect.cleanup_pending = !success;
                        effect.retryable = !success;
                        effect.reason_code =
                            (!success).then_some("HOST_DNS_RECONCILE_PENDING".into());
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
                    deps.system_dns_takeover_active =
                        super::super::dns_takeover::system_dns_takeover_active(
                            self.helper.platform(),
                            true,
                            Some(true),
                        );
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
                if let Some(replacement) = state.host.dns_cold.as_mut().filter(|cold| {
                    cold.in_flight
                        && cold.operation_id != marker_operation_id
                        && cold.prior_marker.as_deref() == Some(marker_operation_id)
                }) {
                    // Exact prior restoration does not retire the in-flight
                    // replacement's reservation or accept its requested policy.
                    replacement.prior_marker = None;
                } else if cold.is_some() {
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
                        policy_required: dns_execution_active(
                            &restored_execution,
                            self.helper.platform(),
                        ),
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
            .is_some_and(|config| dns_config_requires_takeover(config, self.helper.platform()));
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
                        inbound.tag == MIXED_INBOUND_TAG
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
                state
                    .host
                    .retain(&old.execution, receipt, self.helper.platform());
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
                state
                    .host
                    .retain(execution, receipt, self.helper.platform());
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
                state
                    .host
                    .retain(execution, receipt, self.helper.platform());
            }
            drop(state);
        });
        self.push_pending_changes();
    }

    pub(super) fn annotate_runtime_host_result(&self, result: &mut RuntimeApplyResult) {
        self.annotate_runtime_host_result_inner(result);
        // Native retirement has an independent original Core owner. Every host
        // path, including no current host and accepted callbacks, retains it.
        self.annotate_runtime_native_result(result);
    }

    fn annotate_runtime_host_result_inner(&self, result: &mut RuntimeApplyResult) {
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
                "hotApplied"
                    | "hotDurabilityPending"
                    | "hostEffectsPending"
                    | "hostEffectsFailed"
                    | "nativeCleanupUnknown"
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
            return self.runtime_native_result(RuntimeApplyResult::status("conflict"));
        };
        if result.runtime_generation != Some(generation)
            || !self.core_running()
            || self.gate.is_busy()
        {
            return self.runtime_native_result(RuntimeApplyResult::status("conflict"));
        }
        if let Err(error) = self.verify_cached_runtime_pairing(generation).await {
            return self.runtime_native_result(*error);
        }
        self.annotate_runtime_host_result(&mut result);
        result
    }
}

#[cfg(test)]
mod tests;

//! C7 系统 DNS 接管 owner：接管/还原/热插拔重灌的同步核与 async 包装、OS DNS 缓存刷新、
//! `takeoverSystemDns` 用户开关的三态读取。
//!
//! 底层操作（mac `networksetup`/`scutil`、Linux helper→`resolvectl`、Windows no-op）由
//! system-integration/helper 承担；本模块是**接线**层（L1，只依赖 façade 定义与 `set_nonfatal_error`）。

use std::sync::Arc;

use super::runtime_config::host_effects::{
    HostEffectExecution, HostEffectKind, HostEffectOutcome, HostEffectPlan, HostEffectPreview,
    HostEffectReceipt, HostEffectResult, HostEffectStrategy,
};
use super::runtime_config::{RuntimeApplyReason, RuntimeApplyResult};
use polaris_config_engine::user_config::{app_config::UserConfig, ProxyModeType};
use polaris_helper_proto::Platform;
use polaris_system_integration::dns_ops::PreparedDnsEffect;
use polaris_system_integration::linux_resolved::PreparedLinuxDnsEffect;
use polaris_system_integration::proxy_ops::{
    PreparedProxyEffect, TakeoverOutcome, TakeoverReceipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{code, ProxyRuntime};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeHostRecipe {
    generation: u64,
    intent: u64,
    desired_digest: String,
    physical: Value,
    dns: Option<PreparedDnsEffect>,
    linux_dns: Option<PreparedLinuxDnsEffect>,
    proxy: Option<PreparedProxyEffect>,
    proxy_port: Option<u16>,
}

pub(super) fn runtime_dns_plan_prior_owner(plan: &HostEffectPlan) -> Option<String> {
    let recipe: RuntimeHostRecipe = serde_json::from_value(plan.recipe.clone()).ok()?;
    recipe
        .dns
        .as_ref()
        .and_then(PreparedDnsEffect::prior_marker_operation_id)
        .or_else(|| {
            recipe
                .linux_dns
                .as_ref()
                .and_then(PreparedLinuxDnsEffect::prior_marker_operation_id)
        })
}

fn host_digest(value: &Value) -> String {
    polaris_updater::sha256_hex(value.to_string().as_bytes())
}

fn host_plan_digest(plan: &HostEffectPlan) -> String {
    host_digest(&serde_json::json!({
        "hostEpoch": plan.host_epoch, "strategy": plan.strategy,
        "physicalFactsDigest": plan.physical_facts_digest,
        "effects": plan.effects, "recipe": plan.recipe,
    }))
}

fn host_rejection(code: &'static str, path: &str) -> RuntimeApplyResult {
    let mut result = RuntimeApplyResult::status("restartRequired");
    result.reasons.push(RuntimeApplyReason {
        code,
        path: path.to_owned(),
    });
    result
}

fn host_physical_facts(
    deps: &polaris_config_engine::builder::GenerateConfigDeps,
    generation: u64,
) -> Value {
    // Captured before TUN. Do not launder the desired takeover flag into an OS fact.
    serde_json::json!({
        "generation": generation, "platform": deps.platform,
        "lanResolver": deps.lan_resolver_for_dns, "ownLanCidrs": deps.own_lan_cidrs,
    })
}

pub(super) fn merge_runtime_host_projection(
    running: &Value,
    accepted_host: Option<&Value>,
) -> Value {
    let mut merged = running.clone();
    let (Some(target), Some(host)) = (
        merged.as_object_mut(),
        accepted_host.and_then(Value::as_object),
    ) else {
        return merged;
    };
    for key in [
        "proxyModeType",
        "mixedPort",
        "httpPort",
        "bypassLAN",
        "bypassLANList",
    ] {
        match host.get(key) {
            Some(value) => {
                target.insert(key.into(), value.clone());
            }
            None => {
                target.remove(key);
            }
        }
    }
    let takeover = host
        .get("dnsConfig")
        .and_then(|dns| dns.get("takeoverSystemDns"));
    if let Some(value) = takeover {
        let dns = target
            .entry("dnsConfig")
            .or_insert_with(|| serde_json::json!({}));
        if !dns.is_object() {
            *dns = serde_json::json!({});
        }
        dns.as_object_mut()
            .expect("DNS object created above")
            .insert("takeoverSystemDns".into(), value.clone());
    } else if let Some(dns) = target.get_mut("dnsConfig").and_then(Value::as_object_mut) {
        dns.remove("takeoverSystemDns");
    }
    merged
}

pub(super) fn dns_effect_requires_precore_snapshot(
    platform: Platform,
    before: &UserConfig,
    after: &UserConfig,
    before_active: bool,
    target_active: bool,
    dhcp_suppressed: bool,
) -> bool {
    use polaris_config_engine::builder::network_env::{resolved_probe, ProbeFacts};
    if platform != Platform::Mac || before_active == target_active {
        return false;
    }
    let before_facts = ProbeFacts {
        platform,
        tun: true,
        takeover_active: before_active,
        dhcp_suppressed,
    };
    let target_facts = ProbeFacts {
        takeover_active: target_active,
        ..before_facts
    };
    let changed_in = |config: &UserConfig| {
        let mut seen = std::collections::BTreeSet::new();
        config
            .network_profiles
            .iter()
            .filter(|profile| !profile.id.trim().is_empty() && seen.insert(profile.id.clone()))
            .any(|profile| {
                let before = resolved_probe(profile, &before_facts);
                let target = resolved_probe(profile, &target_facts);
                before.available != target.available
                    || before.available && before.probe_source != target.probe_source
            })
    };
    changed_in(before) || changed_in(after)
}

pub(super) fn host_result(
    preview: HostEffectPreview,
    expected_operation: &str,
    receipt: TakeoverReceipt,
) -> HostEffectResult {
    if receipt.operation_id != expected_operation
        || receipt.outcome == TakeoverOutcome::Applied
            && receipt.marker_id.as_deref().is_none_or(str::is_empty)
    {
        return HostEffectResult {
            preview,
            outcome: HostEffectOutcome::Unknown,
            receipt_id: None,
            cleanup_pending: true,
            retryable: false,
            reason_code: Some("HOST_RECEIPT_OWNER_MISMATCH".into()),
        };
    }
    if let Some(reason) = &receipt.reason {
        log::warn!(
            "host effect {} {:?} {:?}: {reason}",
            receipt.operation_id,
            preview.kind,
            receipt.outcome
        );
    }
    let outcome = match receipt.outcome {
        TakeoverOutcome::Unchanged => HostEffectOutcome::Unchanged,
        TakeoverOutcome::Applied => HostEffectOutcome::Applied,
        TakeoverOutcome::FailedUnchanged => HostEffectOutcome::FailedUnchanged,
        TakeoverOutcome::FailedRestored => HostEffectOutcome::FailedRestored,
        TakeoverOutcome::RecoveryPending => HostEffectOutcome::RecoveryPending,
        TakeoverOutcome::Unknown => HostEffectOutcome::Unknown,
        TakeoverOutcome::Superseded => HostEffectOutcome::Superseded,
        TakeoverOutcome::Unsupported => HostEffectOutcome::Unsupported,
    };
    HostEffectResult {
        preview,
        outcome,
        receipt_id: receipt
            .marker_id
            .map(|marker| format!("{}:{marker}", receipt.operation_id)),
        cleanup_pending: receipt.cleanup_pending,
        retryable: matches!(
            receipt.outcome,
            TakeoverOutcome::FailedUnchanged | TakeoverOutcome::FailedRestored
        ),
        reason_code: receipt.reason.map(|_| {
            match receipt.outcome {
                TakeoverOutcome::RecoveryPending => "HOST_RECOVERY_PENDING",
                TakeoverOutcome::Unknown => "HOST_WORKER_UNCONFIRMED",
                TakeoverOutcome::Superseded => "HOST_EXECUTION_SUPERSEDED",
                TakeoverOutcome::Unsupported => "HOST_EFFECT_UNSUPPORTED",
                _ => "HOST_EFFECT_FAILED",
            }
            .to_owned()
        }),
    }
}

fn host_leaf_failure(
    execution_id: &str,
    outcome: TakeoverOutcome,
    reason: &str,
) -> TakeoverReceipt {
    TakeoverReceipt {
        operation_id: execution_id.to_owned(),
        outcome,
        marker_id: None,
        cleanup_pending: matches!(
            outcome,
            TakeoverOutcome::Unknown | TakeoverOutcome::RecoveryPending
        ),
        reason: Some(reason.to_owned()),
    }
}

#[cfg(test)]
pub(crate) type TestDnsRestore = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

impl ProxyRuntime {
    /// Only captures controller snapshots. No DNS/proxy mutation and no fresh
    /// physical route/resolver probe through the currently active TUN.
    pub(super) fn prepare_runtime_host_effects(
        &self,
        desired: &Value,
        running: &Value,
        generation: u64,
        intent: u64,
        core_changes: bool,
    ) -> Result<HostEffectPlan, RuntimeApplyResult> {
        if !self.selector_operation_is_current(generation, intent) {
            return Err(RuntimeApplyResult::status("conflict"));
        }
        let platform = self.helper.platform();
        let accepted_host = self.runtime_accepted_host_config(generation);
        let mut old_host = merge_runtime_host_projection(running, accepted_host.as_ref());
        if let Some(active) = self.runtime_host_dns_physical_active(generation) {
            if let Some(object) = old_host.as_object_mut() {
                let dns = object
                    .entry("dnsConfig")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(fields) = dns.as_object_mut() {
                    fields.insert("takeoverSystemDns".into(), Value::Bool(active));
                }
            }
        }
        let before: UserConfig = serde_json::from_value(old_host.clone())
            .map_err(|_| RuntimeApplyResult::status("rejected"))?;
        let after: UserConfig = serde_json::from_value(desired.clone())
            .map_err(|_| RuntimeApplyResult::status("rejected"))?;
        let old_request = super::system_takeover::runtime_system_proxy_request(&before, platform);
        let new_request = super::system_takeover::runtime_system_proxy_request(&after, platform);
        let proxy_changed = old_request != new_request;
        let dns_changed = host_dns_takeover_changed(platform, &old_host, desired)
            || self.runtime_dns_effect_retry_required(generation, intent);
        let host_epoch = self.system_proxy.host_epoch();
        if host_epoch & 1 != 0 {
            return Err(RuntimeApplyResult::status("conflict"));
        }
        let mut strategy = if core_changes {
            HostEffectStrategy::AfterCoreCommit
        } else {
            HostEffectStrategy::HostOnly
        };
        let mut effects = Vec::new();
        let mut recipe = RuntimeHostRecipe {
            generation,
            intent,
            desired_digest: host_digest(desired),
            physical: Value::Null,
            dns: None,
            linux_dns: None,
            proxy: None,
            proxy_port: None,
        };
        if dns_changed || proxy_changed {
            let deps = self.runtime_accepted_host_deps(generation).ok_or_else(|| {
                host_rejection("HOST_PHYSICAL_BASELINE_UNAVAILABLE", "runtime.hostEffects")
            })?;
            recipe.physical = host_physical_facts(&deps, generation);
            if before.proxy_mode_type.effective_on(platform).is_tun()
                != after.proxy_mode_type.effective_on(platform).is_tun()
            {
                return Err(host_rejection(
                    "HOST_CARRIER_TRANSITION_REQUIRES_RESTART",
                    "proxyModeType",
                ));
            }
        }
        if dns_changed {
            let target_active = after.proxy_mode_type.effective_on(platform).is_tun()
                && dns_takeover_enabled(desired) != Some(false);
            if platform == Platform::Linux {
                let prepared = self
                    .linux_resolved_controller
                    .lock()
                    .map_err(|_| RuntimeApplyResult::status("unconfirmed"))?
                    .prepare_effect(target_active)
                    .map_err(|_| {
                        host_rejection(
                            "LINUX_DNS_EXACT_CAPTURE_UNAVAILABLE",
                            "dnsConfig.takeoverSystemDns",
                        )
                    })?;
                effects.push(HostEffectPreview {
                    kind: HostEffectKind::Dns,
                    scope_digest: host_digest(&serde_json::json!({"kind":"linuxDns","marker":prepared.scope(),
                        "birth":prepared.before().target().map(|target| (target.pid.get(), target.birth.to_wire())),
                        "link":prepared.before().link})),
                    before_digest: host_digest(&prepared.before_facts()),
                    target_digest: host_digest(&prepared.target_facts()),
                });
                recipe.linux_dns = Some(prepared);
            } else if platform == Platform::Mac {
                let before_active = before.proxy_mode_type.effective_on(platform).is_tun()
                    && dns_takeover_enabled(&old_host) != Some(false);
                let dhcp_suppressed = self
                    .runtime_accepted_host_deps(generation)
                    .ok_or_else(|| RuntimeApplyResult::status("unconfirmed"))?
                    .netenv_dhcp_suppressed;
                if dns_effect_requires_precore_snapshot(
                    platform,
                    &before,
                    &after,
                    before_active,
                    target_active,
                    dhcp_suppressed,
                ) {
                    strategy = HostEffectStrategy::NeedsSnapshotBeforeCore;
                }
                let physical_lan_resolver = self
                    .runtime_accepted_host_deps(generation)
                    .and_then(|deps| deps.lan_resolver_for_dns);
                let prepared = self
                    .dns_controller
                    .lock()
                    .map_err(|_| RuntimeApplyResult::status("unconfirmed"))?
                    .prepare_effect(target_active)
                    .map_err(|_| {
                        host_rejection(
                            "HOST_DNS_EXACT_CAPTURE_UNAVAILABLE",
                            "dnsConfig.takeoverSystemDns",
                        )
                    })?
                    .with_physical_lan_resolver(physical_lan_resolver);
                effects.push(HostEffectPreview {
                kind: HostEffectKind::Dns,
                scope_digest: host_digest(&serde_json::json!({"kind":"dns","marker":prepared.scope(),"services":prepared.before().keys().collect::<Vec<_>>()})),
                before_digest: host_digest(&serde_json::to_value(prepared.before()).map_err(|_| RuntimeApplyResult::status("rejected"))?),
                target_digest: host_digest(&serde_json::to_value(prepared.target()).map_err(|_| RuntimeApplyResult::status("rejected"))?),
            });
                recipe.dns = Some(prepared);
            } else {
                return Err(host_rejection(
                    "HOST_DNS_TAKEOVER_UNSUPPORTED",
                    "dnsConfig.takeoverSystemDns",
                ));
            }
        }
        if proxy_changed {
            let prepared = self
                .system_proxy
                .prepare_host_effect(new_request.as_ref())
                .map_err(|_| {
                    host_rejection("HOST_PROXY_EXACT_CAPTURE_UNAVAILABLE", "systemProxy")
                })?;
            if let Some(request) = &new_request {
                recipe.proxy_port = Some(request.http_port);
                // core_changes was established by the real Plan. A different
                // accepted host pointer can be retrying an already-current Core
                // listener; that HostOnly retry must not fabricate another Apply.
            }
            effects.push(HostEffectPreview {
                kind: HostEffectKind::SystemProxy,
                scope_digest: host_digest(&serde_json::json!({"kind":"systemProxy","marker":prepared.scope(),"before":prepared.before()})),
                before_digest: host_digest(&serde_json::to_value(prepared.before()).map_err(|_| RuntimeApplyResult::status("rejected"))?),
                target_digest: host_digest(&serde_json::to_value(prepared.target()).map_err(|_| RuntimeApplyResult::status("rejected"))?),
            });
            recipe.proxy = Some(prepared);
        }
        let physical_facts_digest = host_digest(&recipe.physical);
        let mut plan = HostEffectPlan {
            effect_digest: String::new(),
            physical_facts_digest,
            host_epoch,
            effects,
            strategy,
            recipe: serde_json::to_value(recipe)
                .map_err(|_| RuntimeApplyResult::status("rejected"))?,
        };
        plan.effect_digest = host_plan_digest(&plan);
        if !self.runtime_host_facts_match(&plan) {
            return Err(RuntimeApplyResult::status("conflict"));
        }
        Ok(plan)
    }

    pub(super) fn runtime_host_facts_match(&self, plan: &HostEffectPlan) -> bool {
        if host_plan_digest(plan) != plan.effect_digest
            || self.system_proxy.host_epoch() != plan.host_epoch
            || plan.host_epoch & 1 != 0
        {
            return false;
        }
        let Ok(recipe) = serde_json::from_value::<RuntimeHostRecipe>(plan.recipe.clone()) else {
            return false;
        };
        if !self.selector_operation_is_current(recipe.generation, recipe.intent) {
            return false;
        }
        if !plan.effects.is_empty() {
            let Some(deps) = self.runtime_accepted_host_deps(recipe.generation) else {
                return false;
            };
            if host_physical_facts(&deps, recipe.generation) != recipe.physical
                || host_digest(&recipe.physical) != plan.physical_facts_digest
            {
                return false;
            }
        }
        if let Some(prepared) = &recipe.dns {
            if !self
                .dns_controller
                .lock()
                .is_ok_and(|controller| controller.effect_facts_match(prepared).unwrap_or(false))
            {
                return false;
            }
        }
        if let Some(prepared) = &recipe.linux_dns {
            if !self
                .linux_resolved_controller
                .lock()
                .is_ok_and(|controller| controller.effect_facts_match(prepared).unwrap_or(false))
            {
                return false;
            }
        }
        recipe.proxy.as_ref().is_none_or(|prepared| {
            self.system_proxy
                .host_effect_facts_match(prepared, plan.host_epoch)
        })
    }

    /// The detached task owns Arc/runtime, execution and blocking lease. Dropping
    /// an IPC waiter cannot cancel queued/running host mutation. Stop/restore uses
    /// these same controller mutexes; revocation retains custody and does not claim a join.
    pub(super) fn spawn_runtime_host_effects(self: &Arc<Self>, execution: HostEffectExecution) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let worker_runtime = runtime.clone();
            let worker_execution = execution.clone();
            let lease = runtime.config.retain_active_legacy_start_lease();
            let result = tokio::task::spawn_blocking(move || {
                let _lease = lease;
                worker_runtime.execute_runtime_host_effects(&worker_execution)
            })
            .await;
            let receipt = match result {
                Ok(receipt) => receipt,
                Err(_) => HostEffectReceipt {
                    effect_digest: execution.plan.effect_digest.clone(),
                    physical_facts_digest: execution.plan.physical_facts_digest.clone(),
                    host_epoch: execution.plan.host_epoch,
                    effects: execution
                        .plan
                        .effects
                        .iter()
                        .cloned()
                        .map(|preview| {
                            host_result(
                                preview,
                                &execution.execution_id,
                                host_leaf_failure(
                                    &execution.execution_id,
                                    TakeoverOutcome::Unknown,
                                    "host worker failed",
                                ),
                            )
                        })
                        .collect(),
                },
            };
            runtime.record_runtime_host_effect_result(&execution, receipt);
        });
    }

    fn execute_runtime_mac_dns_effect(
        &self,
        execution: &HostEffectExecution,
        prepared: &PreparedDnsEffect,
    ) -> TakeoverReceipt {
        let mut after_restore = false;
        if prepared.needs_prior_restore() {
            let (owner, restored) = match self.dns_controller.lock() {
                Ok(mut controller)
                    if self.runtime_host_execution_is_current(execution)
                        && controller.effect_facts_match(prepared).unwrap_or(false) =>
                {
                    let owner = controller.exact_marker_operation_id();
                    controller.restore_dns();
                    (owner, !controller.has_marker())
                }
                _ => {
                    return host_leaf_failure(
                        &execution.execution_id,
                        TakeoverOutcome::Superseded,
                        "prior DNS cleanup scope changed",
                    )
                }
            };
            if let Some(owner) = &owner {
                self.record_runtime_dns_restore_result(owner, restored);
            }
            if !restored {
                let mut result = host_leaf_failure(
                    &execution.execution_id,
                    TakeoverOutcome::RecoveryPending,
                    "prior DNS owner restoration is unconfirmed",
                );
                result.marker_id = owner;
                return result;
            }
            after_restore = true;
        }
        match self.dns_controller.lock() {
            Ok(mut controller) if self.runtime_host_execution_is_current(execution) => {
                let next;
                let prepared = if after_restore {
                    next = match controller.prepare_after_restore(prepared) {
                        Ok(next) => next,
                        Err(_) => {
                            return host_leaf_failure(
                                &execution.execution_id,
                                TakeoverOutcome::FailedRestored,
                                "DNS scope changed after confirmed prior-owner restoration",
                            )
                        }
                    };
                    &next
                } else {
                    prepared
                };
                let mut result = controller.apply_effect(prepared, &execution.execution_id);
                if result.cleanup_pending {
                    result.marker_id = controller.exact_marker_operation_id();
                }
                if after_restore
                    && matches!(
                        result.outcome,
                        TakeoverOutcome::Superseded | TakeoverOutcome::FailedUnchanged
                    )
                {
                    result.outcome = TakeoverOutcome::FailedRestored;
                }
                result
            }
            _ => host_leaf_failure(
                &execution.execution_id,
                if after_restore {
                    TakeoverOutcome::FailedRestored
                } else {
                    TakeoverOutcome::Superseded
                },
                "DNS owner superseded",
            ),
        }
    }

    fn execute_runtime_linux_dns_effect(
        &self,
        execution: &HostEffectExecution,
        prepared: &PreparedLinuxDnsEffect,
    ) -> TakeoverReceipt {
        let mut after_restore = false;
        if prepared.needs_prior_restore() {
            let (owner, restored) = match self.linux_resolved_controller.lock() {
                Ok(mut controller)
                    if self.runtime_host_execution_is_current(execution)
                        && controller.effect_facts_match(prepared).unwrap_or(false) =>
                {
                    let owner = controller.exact_marker_operation_id();
                    let restored = controller.restore().is_ok() && !controller.has_marker();
                    (owner, restored)
                }
                _ => {
                    return host_leaf_failure(
                        &execution.execution_id,
                        TakeoverOutcome::Superseded,
                        "prior Linux DNS cleanup scope changed",
                    )
                }
            };
            if let Some(owner) = &owner {
                self.record_runtime_dns_restore_result(owner, restored);
            }
            if !restored {
                let mut result = host_leaf_failure(
                    &execution.execution_id,
                    TakeoverOutcome::RecoveryPending,
                    "prior Linux DNS restoration is unconfirmed",
                );
                result.marker_id = owner;
                return result;
            }
            after_restore = true;
        }
        match self.linux_resolved_controller.lock() {
            Ok(mut controller) if self.runtime_host_execution_is_current(execution) => {
                let next;
                let prepared = if after_restore {
                    next =
                        match controller.prepare_after_restore(prepared) {
                            Ok(next) => next,
                            Err(_) => return host_leaf_failure(
                                &execution.execution_id,
                                TakeoverOutcome::FailedRestored,
                                "Linux DNS birth/link/baseline changed after confirmed restoration",
                            ),
                        };
                    &next
                } else {
                    prepared
                };
                let mut result = controller.apply_effect(prepared, &execution.execution_id);
                if result.cleanup_pending {
                    result.marker_id = controller.exact_marker_operation_id();
                }
                if after_restore
                    && matches!(
                        result.outcome,
                        TakeoverOutcome::Superseded | TakeoverOutcome::FailedUnchanged
                    )
                {
                    result.outcome = TakeoverOutcome::FailedRestored;
                }
                result
            }
            _ => host_leaf_failure(
                &execution.execution_id,
                if after_restore {
                    TakeoverOutcome::FailedRestored
                } else {
                    TakeoverOutcome::Superseded
                },
                "Linux DNS owner superseded",
            ),
        }
    }

    fn execute_runtime_host_effects(&self, execution: &HostEffectExecution) -> HostEffectReceipt {
        let mut receipt = HostEffectReceipt {
            effect_digest: execution.plan.effect_digest.clone(),
            physical_facts_digest: execution.plan.physical_facts_digest.clone(),
            host_epoch: execution.plan.host_epoch,
            effects: Vec::new(),
        };
        let recipe =
            serde_json::from_value::<RuntimeHostRecipe>(execution.plan.recipe.clone()).ok();
        let valid = recipe.as_ref().is_some_and(|recipe| {
            recipe.generation == execution.generation
                && recipe.intent == execution.intent
                && recipe.desired_digest == host_digest(&execution.desired)
        }) && self.runtime_host_execution_is_current(execution)
            && self.runtime_host_facts_match(&execution.plan)
            && matches!(
                execution.plan.strategy,
                HostEffectStrategy::HostOnly | HostEffectStrategy::AfterCoreCommit
            );
        for preview in &execution.plan.effects {
            let blocked_strategy = matches!(
                execution.plan.strategy,
                HostEffectStrategy::NeedsSnapshotBeforeCore | HostEffectStrategy::BridgeOrRestart
            );
            let mut result = host_leaf_failure(
                &execution.execution_id,
                if blocked_strategy {
                    TakeoverOutcome::Unsupported
                } else {
                    TakeoverOutcome::Superseded
                },
                "host recipe superseded or requires pre-Core bridge",
            );
            if valid && self.runtime_host_execution_is_current(execution) {
                if let Some(recipe) = &recipe {
                    match preview.kind {
                        HostEffectKind::Dns => {
                            if let Some(prepared) = &recipe.linux_dns {
                                result = self.execute_runtime_linux_dns_effect(execution, prepared);
                            } else if let Some(prepared) = &recipe.dns {
                                result = self.execute_runtime_mac_dns_effect(execution, prepared);
                            }
                        }
                        HostEffectKind::SystemProxy => {
                            if let Some(prepared) = &recipe.proxy {
                                let target_ready = recipe.proxy_port.is_none_or(|port| {
                                    self.runtime_host_listener_ready(execution, port)
                                });
                                if target_ready {
                                    result = self.system_proxy.apply_host_effect_when(
                                        prepared,
                                        &execution.execution_id,
                                        execution.plan.host_epoch,
                                        || {
                                            self.runtime_host_execution_is_current(execution)
                                                && recipe.proxy_port.is_none_or(|port| {
                                                    self.runtime_host_listener_ready(
                                                        execution, port,
                                                    )
                                                })
                                        },
                                    );
                                } else {
                                    result = host_leaf_failure(
                                        &execution.execution_id,
                                        TakeoverOutcome::FailedUnchanged,
                                        "committed listener not confirmed ready",
                                    );
                                }
                            }
                        }
                        HostEffectKind::NativeTunnel => {
                            result = host_leaf_failure(
                                &execution.execution_id,
                                TakeoverOutcome::Unsupported,
                                "native tunnel uses its platform owner",
                            );
                        }
                    }
                }
            }
            receipt.effects.push(host_result(
                preview.clone(),
                &execution.execution_id,
                result,
            ));
        }
        receipt
    }

    // ── C7 系统 DNS 接管/还原/刷缓存（装配 + 命令收口 + 生命周期接线）─────────────────────────
    //
    // 底层操作（mac `networksetup`/`scutil`、Linux helper→`resolvectl`、Windows no-op）由
    // system-integration/helper 承担。本层是**接线**：控制器装配 + 命令/生命周期收口。

    /// C7：系统 DNS 接管的同步核（持锁 → `set_dns` → 报告是否留下接管 marker）。
    ///
    /// best-effort：`set_dns` 内部失败仅告警 + 回滚，**绝不抛**（DNS 治理降级不阻断 TUN 启动）。锁中毒 → 跳过。
    /// 命令层直调；async 生命周期经 [`set_system_dns_best_effort`](Self::set_system_dns_best_effort) 的 spawn_blocking 包。
    pub(crate) fn set_system_dns_locked(&self) -> bool {
        let generation = self.gate.generation();
        let intent = self.selector_reconcile.intent_generation();
        let operation_id = format!("dns-startup:{}", uuid::Uuid::new_v4());
        if !self.begin_runtime_cold_dns(generation, intent, &operation_id) {
            return false;
        }
        // Only this pre-TUN captured resolver fact is reused; the startup target
        // takeover bit is never accepted as H.
        let physical_lan_resolver = self
            .runtime_accepted_host_deps(generation)
            .and_then(|deps| deps.lan_resolver_for_dns);
        let receipt = match self.helper.platform() {
            Platform::Linux => {
                let prepared = self
                    .linux_resolved_controller
                    .lock()
                    .map_err(|_| "Linux DNS controller poisoned".to_string())
                    .and_then(|controller| controller.prepare_effect(true));
                match prepared {
                    Ok(prepared)
                        if self.bind_runtime_cold_dns_marker(
                            &operation_id,
                            prepared.prior_marker_operation_id(),
                        ) =>
                    {
                        match self.linux_resolved_controller.lock() {
                            Ok(mut controller)
                                if self.selector_operation_is_current(generation, intent) =>
                            {
                                let mut receipt = controller.apply_effect(&prepared, &operation_id);
                                if receipt.outcome == TakeoverOutcome::Unchanged {
                                    receipt.marker_id = controller
                                        .checked_owned_receipt()
                                        .ok()
                                        .flatten()
                                        .and_then(|checked| checked.marker_id);
                                } else if receipt.cleanup_pending {
                                    receipt.marker_id = controller.exact_marker_operation_id();
                                }
                                receipt
                            }
                            _ => host_leaf_failure(
                                &operation_id,
                                TakeoverOutcome::Superseded,
                                "cold Linux DNS owner changed before write",
                            ),
                        }
                    }
                    Ok(_) => host_leaf_failure(
                        &operation_id,
                        TakeoverOutcome::Superseded,
                        "cold Linux DNS reservation retired before write",
                    ),
                    Err(error) => {
                        host_leaf_failure(&operation_id, TakeoverOutcome::FailedUnchanged, &error)
                    }
                }
            }
            Platform::Mac => {
                let prepared = self
                    .dns_controller
                    .lock()
                    .map_err(|_| "DNS controller poisoned".to_string())
                    .and_then(|controller| {
                        controller
                            .prepare_effect(true)
                            .map(|prepared| {
                                prepared.with_physical_lan_resolver(physical_lan_resolver)
                            })
                            .map_err(|error| error.to_string())
                    });
                match prepared {
                    Ok(prepared)
                        if self.bind_runtime_cold_dns_marker(
                            &operation_id,
                            prepared.prior_marker_operation_id(),
                        ) =>
                    {
                        match self.dns_controller.lock() {
                            Ok(mut controller)
                                if self.selector_operation_is_current(generation, intent) =>
                            {
                                let mut receipt = controller.apply_effect(&prepared, &operation_id);
                                if receipt.outcome == TakeoverOutcome::Unchanged {
                                    receipt.marker_id = controller
                                        .checked_owned_receipt()
                                        .ok()
                                        .flatten()
                                        .and_then(|checked| checked.marker_id);
                                } else if receipt.cleanup_pending {
                                    receipt.marker_id = controller.exact_marker_operation_id();
                                }
                                let displaced = controller.displaced_resolvers();
                                if !displaced.is_empty() {
                                    log::warn!("系统 DNS 接管挤掉了 {} 个非公网解析器（可能属于其它 VPN/组网客户端）：{}",displaced.len(),displaced.join(", "));
                                }
                                receipt
                            }
                            _ => host_leaf_failure(
                                &operation_id,
                                TakeoverOutcome::Superseded,
                                "cold DNS owner changed before write",
                            ),
                        }
                    }
                    Ok(_) => host_leaf_failure(
                        &operation_id,
                        TakeoverOutcome::Superseded,
                        "cold DNS reservation retired before write",
                    ),
                    Err(error) => {
                        host_leaf_failure(&operation_id, TakeoverOutcome::FailedUnchanged, &error)
                    }
                }
            }
            _ => host_leaf_failure(
                &operation_id,
                TakeoverOutcome::Unsupported,
                "platform has no checked system DNS adapter",
            ),
        };
        let accepted = !receipt.cleanup_pending
            && receipt.marker_id.is_some()
            && matches!(
                receipt.outcome,
                TakeoverOutcome::Applied | TakeoverOutcome::Unchanged
            );
        if !accepted {
            self.set_nonfatal_error(
                &format!(
                    "系统 DNS 接管未确认：{}",
                    receipt
                        .reason
                        .as_deref()
                        .unwrap_or("checked DNS result unavailable")
                ),
                code::SYSTEM_DNS_TAKEOVER_FAILED,
            );
        }
        // Preparation binding and H/cleanup callbacks run outside controller locks.
        self.record_runtime_cold_dns_result(generation, intent, receipt);
        accepted
            && self.selector_operation_is_current(generation, intent)
            && self.accepted_host_dns_takeover() == Some(true)
    }

    /// 本次接管挤掉的非公网解析器（快照拷贝；锁中毒/未接管 → 空）。
    ///
    /// 只读，供 `dns_takeover_report` 命令回给设置页。Linux 走 `linux_resolved` 那条腿，
    /// 不经本控制器，故恒空 —— 与 `takeover_supported` 的平台面一致。
    pub(crate) fn displaced_dns_resolvers(&self) -> Vec<String> {
        match self.dns_controller.lock() {
            Ok(c) => c.displaced_resolvers().to_vec(),
            Err(error) => {
                log::error!("dns_controller 锁中毒: {error} → 无法读取被挤掉的解析器");
                Vec::new()
            }
        }
    }

    /// C7：系统 DNS 还原的同步核（持锁 → `restore_dns` → 报告 marker 是否已清）。
    pub(crate) fn restore_system_dns_locked(&self) -> bool {
        match self.restore_system_dns_confirmed_locked() {
            Ok(()) => true,
            Err(error) => {
                log::error!("系统 DNS 恢复未确认: {error}");
                false
            }
        }
    }

    fn restore_system_dns_confirmed_locked(&self) -> Result<(), String> {
        #[cfg(test)]
        {
            let restore = self
                .test_dns_restore
                .lock()
                .map_err(|_| "DNS test port poisoned")?
                .clone();
            if let Some(restore) = restore {
                return restore();
            }
        }
        if Platform::current() == Platform::Linux {
            let (owner, result, retired) = {
                let mut controller = self
                    .linux_resolved_controller
                    .lock()
                    .map_err(|error| format!("Linux DNS controller poisoned: {error}"))?;
                let owner = controller.exact_marker_operation_id();
                let result = controller.restore();
                let retired = result.is_ok() && !controller.has_marker();
                (owner, result, retired)
            };
            if let Some(owner) = owner {
                self.record_runtime_dns_restore_result(&owner, retired);
            }
            result?;
            return if retired {
                Ok(())
            } else {
                Err("Linux DNS restoration marker remains".into())
            };
        }
        let (owner, retired) = {
            let mut controller = self
                .dns_controller
                .lock()
                .map_err(|error| format!("DNS controller poisoned: {error}"))?;
            let owner = controller.exact_marker_operation_id();
            controller.restore_dns();
            (owner, !controller.has_marker())
        };
        if let Some(owner) = owner {
            self.record_runtime_dns_restore_result(&owner, retired);
        }
        if !retired {
            return Err("DNS restoration or marker retirement unconfirmed".into());
        }
        Ok(())
    }

    /// Strict exit/account retirement must retain any unresolved recovery authority.
    pub(super) async fn restore_system_dns_confirmed(self: &Arc<Self>) -> Result<(), String> {
        let this = Arc::clone(self);
        let blocking_lease = self.config.retain_active_legacy_start_lease();
        tokio::task::spawn_blocking(move || {
            let _blocking_lease = blocking_lease;
            this.restore_system_dns_confirmed_locked()
        })
        .await
        .map_err(|error| format!("DNS restore worker failed: {error}"))?
    }

    pub(super) async fn restore_system_takeover_confirmed(self: &Arc<Self>) -> Result<(), String> {
        let dns = self.restore_system_dns_confirmed().await;
        let proxy = self.restore_system_proxy_for_exit().await;
        let errors: Vec<String> = [dns, proxy].into_iter().filter_map(Result::err).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_dns_restore(&self, restore: TestDnsRestore) {
        *self.test_dns_restore.lock().unwrap() = Some(restore);
    }

    /// C7：是否存在系统 DNS 接管 marker（命令层/诊断查询）。
    #[must_use]
    pub(crate) fn system_dns_has_marker(&self) -> bool {
        if Platform::current() == Platform::Linux {
            return self
                .linux_resolved_controller
                .lock()
                .map(|controller| controller.has_marker())
                .unwrap_or(false);
        }
        self.dns_controller
            .lock()
            .map(|c| c.has_marker())
            .unwrap_or(false)
    }

    /// C7：TUN 起核尾接管系统 DNS（best-effort，失败不阻断起核）。
    /// 同步控制器（mac exec / Linux helper IPC）挪进 `spawn_blocking`，锁绝不跨 await；调用方等待结果，
    /// 只在接管成功时启动链路 watcher。
    pub(super) async fn set_system_dns_best_effort(self: &Arc<Self>) -> bool {
        let this = Arc::clone(self);
        let blocking_lease = self.config.retain_active_legacy_start_lease();
        match tokio::task::spawn_blocking(move || {
            let _blocking_lease = blocking_lease;
            this.set_system_dns_locked()
        })
        .await
        {
            Ok(applied) => applied,
            Err(error) => {
                log::error!("系统 DNS 接管 spawn_blocking join 失败: {error}");
                false
            }
        }
    }

    /// C7：停核/启动自愈尾还原系统 DNS（best-effort）。无 marker（fresh / 已还原）→ 惰性。
    pub(super) async fn restore_system_dns_best_effort(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let blocking_lease = self.config.retain_active_legacy_start_lease();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            let _blocking_lease = blocking_lease;
            this.restore_system_dns_locked()
        })
        .await
        {
            log::error!("系统 DNS 还原 spawn_blocking join 失败: {e}");
        }
    }

    /// row33：DNS 热插拔重灌的门控判定（纯逻辑，便于单测 + 变异）。`should_reconcile_dns` 的运行时适配：
    /// 仅当前配置仍 TUN 模式（切走 TUN → 虽 marker 在也不再重灌）+ **用户未关 `takeoverSystemDns`** +
    /// 有接管 marker 才放行。三条与起核尾的接管门（[`dns_takeover_enabled`] + `is_tun`）同口径。
    pub(super) fn dns_reconcile_should_run(
        is_tun: bool,
        takeover: Option<bool>,
        has_marker: bool,
    ) -> bool {
        polaris_system_integration::dns_watcher::should_reconcile_dns(
            if is_tun { Some("tun") } else { None },
            takeover,
            has_marker,
        )
    }

    /// row33：DNS 接口热插拔重灌的同步核（持锁 → 门控 → `reconcile_dns`）。best-effort，绝不抛。
    /// 门控（[`Self::dns_reconcile_should_run`]）：当前配置 TUN + 接管 marker 在。锁中毒 / 门未过 → 跳过。
    pub(crate) fn reconcile_system_dns_locked(&self) -> bool {
        if self.runtime_host_execution_in_flight() || self.gate.is_busy() || !self.core_running() {
            return false;
        }
        let generation = self.gate.generation();
        let intent = self.selector_reconcile.intent_generation();
        let raw = self.current_config_snapshot();
        let is_tun = raw
            .clone()
            .and_then(|value| serde_json::from_value::<UserConfig>(value).ok())
            .is_some_and(|config| {
                config
                    .proxy_mode_type
                    .effective_on(self.helper.platform())
                    .is_tun()
            });
        // Both published R and accepted H must permit a watcher write. A failed
        // disable must not admit new services under the old H policy.
        if raw.as_ref().and_then(dns_takeover_enabled) == Some(false) {
            return false;
        }
        let takeover = self
            .accepted_host_dns_takeover()
            .or_else(|| raw.as_ref().and_then(dns_takeover_enabled));
        if !Self::dns_reconcile_should_run(is_tun, takeover, self.system_dns_has_marker()) {
            return false;
        }
        let current = || {
            !self.runtime_host_execution_in_flight()
                && !self.gate.is_busy()
                && self.core_running()
                && self.selector_operation_is_current(generation, intent)
        };
        let observed = if Platform::current() == Platform::Linux {
            match self.linux_resolved_controller.lock() {
                Ok(mut controller) if current() => controller.reconcile_checked(),
                _ => return false,
            }
        } else {
            match self.dns_controller.lock() {
                Ok(mut controller) if current() => Ok(controller.reconcile_dns_checked()),
                _ => return false,
            }
        };
        // Never take the H ledger while retaining the controller lock.
        match observed {
            Ok(Some(receipt)) => {
                let success = !receipt.cleanup_pending
                    && matches!(
                        receipt.outcome,
                        TakeoverOutcome::Applied | TakeoverOutcome::Unchanged
                    );
                if !success {
                    self.set_nonfatal_error(
                        receipt
                            .reason
                            .as_deref()
                            .unwrap_or("DNS exact reconcile remains unconfirmed"),
                        code::SYSTEM_DNS_TAKEOVER_FAILED,
                    );
                }
                self.record_runtime_dns_reconcile_result(generation, intent, receipt);
                success
            }
            Ok(None) => true, // legacy attempt carries no typed H acceptance
            Err(error) => {
                self.set_nonfatal_error(
                    &format!("DNS reconcile failed: {error}"),
                    code::SYSTEM_DNS_TAKEOVER_FAILED,
                );
                false
            }
        }
    }

    /// row33：DNS 热插拔重灌（async 包装，spawn_blocking 持锁；锁绝不跨 await）。watcher 去抖后调。
    pub(super) async fn reconcile_system_dns_best_effort(self: &Arc<Self>) {
        let this = Arc::clone(self);
        if let Err(e) =
            tokio::task::spawn_blocking(move || this.reconcile_system_dns_locked()).await
        {
            log::error!("DNS 热插拔重灌 spawn_blocking join 失败: {e}");
        }
    }

    /// C7：核 start/stop 尾刷 OS DNS 缓存（fire-and-forget、best-effort、永不阻塞代理生命周期）。
    ///
    /// 语义对齐 上游 `flushOsDnsCacheBestEffort`：mac 优先 root helper（`flush-dns`：dscacheutil + HUP
    /// mDNSResponder 两层全清）→ 不可用降级用户级 `dscacheutil`；win `ipconfig /flushdns`；linux `resolvectl
    /// flush-caches`。动机：核 start/stop 跨越「系统解析器受控/还原」边界时清缓存里残留另一侧记录（TUN+FakeIP
    /// 会话期假 IP 停核后仍命中 → 直连撞墙，反向同理）。
    ///
    /// **真机门**：真刷宿主 DNS 缓存**触碰宿主**（本机 Linux 会真跑 `resolvectl`）——故仅在**真跑 app** 时发生，
    /// 单测也会走 start/stop；测试执行器只记录语义结果，绝不执行宿主命令。
    pub(super) fn flush_os_dns_cache_best_effort(self: &Arc<Self>, context: &'static str) {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            // 特权 helper flush 通道（mac root / win SYSTEM；linux 不经此腿，见 `flush_os_dns_cache`
            // 平台分派）。helper 未装/旧 helper 不认这条命令时该调用回 `ok:false`，由分派腿就地降级。
            let helper_flush = || this.helper.flush_dns();
            // Windows 腿的前置判据（spec §3.1「if helper ready」，见 `flush_os_dns_cache` 头注）：
            // 没装 helper 的机器结构上就没有特权通道，每次起停核都发一条「不可用」是纯噪音。
            // 取 token 在位这条**零副作用**的判据而非 `status().ready`，理由见
            // [`HelperRuntime::client_token_present`]。
            let helper_ready = this.helper.client_token_present();
            #[cfg(not(test))]
            let flushed = polaris_system_integration::production_flush_os_dns_cache(
                Some(&helper_flush),
                helper_ready,
                &mut |m| log::info!("[dns-flush:{context}] {m}"),
            );
            #[cfg(test)]
            let flushed = polaris_system_integration::dns_flush::flush_os_dns_cache(
                Platform::current(),
                &super::tests::TestDnsFlushExec,
                Some(&helper_flush),
                helper_ready,
                &mut |m| log::info!("[dns-flush:{context}] {m}"),
            );
            if Platform::current() == Platform::Linux && !flushed {
                this.set_nonfatal_error(
                    "Linux 系统 DNS 缓存刷新失败；旧的失败缓存可能继续影响域名解析",
                    code::SYSTEM_DNS_TAKEOVER_FAILED,
                );
            }
        });
    }
}

/// **C7 用户开关**：原始 config JSON 的 `dnsConfig.takeoverSystemDns` 三态读取（纯函数）。
///
/// **为何从裸 JSON 读**：该字段不在 config-engine 的 `DnsConfig` 结构体里（前端契约
/// `ui/src/contracts/types.ts:324` 有、Rust 侧无建模），与 `restartOnNodeChange` / `autoSwitchNode` /
/// `meshLoginFallbackDirect` 同法 —— 不为一个纯运行期开关去改共享的配置结构体（那会波及 norm/生成/快照
/// 四条链）。宿主接管效果由独立的有效意图比较进入待应用，不伪装成 Core DNS 字段。
///
/// 返回**三态**而非 bool：调用方一律按 上游的 `!== false` 口径判（`Some(false)` 才算关），
/// 缺省与非布尔都等价于「未显式关」。若在此折成 bool，`None`（缺省=开）与 `Some(true)` 的区别就没了，
/// 下游想改默认方向时会误把「用户没表态」当成「用户选了开」。
pub(super) fn dns_takeover_enabled(config: &Value) -> Option<bool> {
    config
        .get("dnsConfig")
        .and_then(|d| d.get("takeoverSystemDns"))
        .and_then(Value::as_bool)
}

/// Host DNS is outside typed Core configuration. Compare effective intent on
/// both sides so save-only cannot accept an unapplied host effect as a NoOp.
pub(super) fn host_dns_takeover_changed(platform: Platform, old: &Value, new: &Value) -> bool {
    if !matches!(platform, Platform::Mac | Platform::Linux) {
        return false;
    }
    let active = |config: &Value| {
        let mode = config
            .get("proxyModeType")
            .cloned()
            .and_then(|value| serde_json::from_value::<ProxyModeType>(value).ok())
            .unwrap_or_default();
        mode.effective_on(platform).is_tun() && dns_takeover_enabled(config) != Some(false)
    };
    active(old) != active(new)
}

/// 网络场景 auto 探测源要的运行期事实「macOS + TUN + 接管系统 DNS 生效」（spec §4.4）。
///
/// 与起核尾 C7 接管门同一组输入、同一口径（`is_tun && takeover != Some(false)`），只多一个平台前提：
/// Linux 的接管加在 TUN link 上、不影响物理网卡 link 的 `local` 取值，Windows TUN 下不接管 —— 只有 mac
/// 的 `networksetup` 接管会让 `local` 看到受控 IP。起核腿与「本机解析后的探测源」查询接口都只调它。
pub(super) fn system_dns_takeover_active(
    platform: polaris_helper_proto::Platform,
    is_tun: bool,
    takeover: Option<bool>,
) -> bool {
    platform == polaris_helper_proto::Platform::Mac && is_tun && takeover != Some(false)
}

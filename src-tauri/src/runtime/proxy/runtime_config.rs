//! Additive, capability-negotiated configuration transactions. A failed transaction
//! never authorizes a core restart. Startup facts are retained, not reallocated.
use std::collections::BTreeMap;
use std::sync::{atomic::Ordering, Arc};

use super::{ProxyRuntime, SwitchOutcome};
use polaris_config_engine::builder::custom_rule_files::build_custom_rule_files;
use polaris_config_engine::builder::{
    generate_sing_box_config_with_report_and_runtime_bindings, GenerateConfigDeps,
};
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::UserConfig;
use polaris_singbox_grpc::{daemon, Endpoint, RuntimeConfigError, SingBoxApiClient};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone)]
pub(super) struct RuntimeConfigBaseline {
    generation: u64,
    committed_transaction: Option<String>,
    physical_route_config: UserConfig,
    deps: GenerateConfigDeps,
    generated: SingBoxConfig,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeApplyReason {
    code: &'static str,
    path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeApplyResult {
    pub status: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_intent_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_confirmation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_mode: Option<String>,
    pub reasons: Vec<RuntimeApplyReason>,
    pub pending_apply: bool,
    pub restart_required: bool,
}
impl RuntimeApplyResult {
    fn status(status: &'static str) -> Self {
        Self {
            status,
            ok: matches!(status, "hotApplied" | "hotDurabilityPending" | "applied"),
            apply_id: None,
            transaction_id: None,
            runtime_generation: None,
            selection_intent_generation: None,
            restart_confirmation: None,
            persistence_mode: None,
            reasons: vec![],
            pending_apply: !matches!(status, "hotApplied" | "hotDurabilityPending" | "skipped"),
            restart_required: status == "restartRequired",
        }
    }
}

#[derive(Clone)]
struct RestartChoice {
    token: String,
    candidate: Value,
    generation: u64,
    intent: u64,
    running: Value,
}
#[derive(Clone)]
struct PendingOperation {
    desired: Value,
    deletion_scope: Vec<crate::runtime::config::DeferredConfigDeletion>,
    physical_route_config: UserConfig,
    generation: u64,
    intent: u64,
    candidate: Value,
    typed: UserConfig,
    generated: SingBoxConfig,
    deps: GenerateConfigDeps,
    plan: daemon::RuntimeConfigPlan,
    network_canary: Option<polaris_config_engine::builder::network_env::NetworkCanaryPlan>,
    published: bool,
    binding_plan: polaris_platform_events::RuntimeBindingPlan,
    mesh_evidence: super::mesh_route_report::MeshRouteRunEvidence,
}
#[derive(Default)]
pub(super) struct RuntimeConfigApplyState {
    restart: Option<RestartChoice>,
    pending: Option<PendingOperation>,
    last: Option<RuntimeApplyResult>,
}

fn identity_equal(
    a: Option<&daemon::RuntimeIdentity>,
    b: Option<&daemon::RuntimeIdentity>,
) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            !a.incarnation.is_empty()
                && a.incarnation == b.incarnation
                && a.applied_revision == b.applied_revision
                && a.effective_digest == b.effective_digest
        }
        _ => false,
    }
}

fn stale_receipt_releases_pending(
    operation: &daemon::RuntimeConfigOperation,
    verified_regeneration_superseded: bool,
) -> bool {
    (operation.durable && operation.state == daemon::RuntimeOperationState::Committed as i32)
        || verified_regeneration_superseded
}

fn terminal_persistence_receipt(
    last: Option<&RuntimeApplyResult>,
    transaction_id: &str,
    generation: u64,
    intent: u64,
) -> Option<RuntimeApplyResult> {
    last.filter(|last| {
        last.status == "hotApplied"
            && last.transaction_id.as_deref() == Some(transaction_id)
            && last.runtime_generation == Some(generation)
            && last.selection_intent_generation == Some(intent)
    })
    .cloned()
}

/// Never rebase an old persistence owner onto a newer graph or cheap control.
/// Status is sampled before the explicit retry; the kernel repeats this CAS.
fn persistence_retry_request(
    plan: &daemon::RuntimeConfigPlan,
    status: &daemon::RuntimeConfigStatus,
) -> Option<daemon::RuntimePersistenceRetryRequest> {
    let operation = status.operation.as_ref()?;
    let active = operation.active.as_ref()?;
    let before = operation.before.as_ref()?;
    if status.contract_version != 1
        || status.lookup_state != daemon::RuntimeLookupState::Found as i32
        || operation.contract_version != 1
        || plan.no_op
        || plan.transaction_id.is_empty()
        || operation.transaction_id != plan.transaction_id
        || operation.candidate_digest != plan.candidate_digest
        || operation.impact_digest != plan.impact_digest
        || !identity_equal(operation.before.as_ref(), plan.expected.as_ref())
        || active.incarnation != before.incarnation
        || active.applied_revision < before.applied_revision
        || active.effective_digest != plan.candidate_digest
        || !identity_equal(status.active.as_ref(), Some(active))
        || status.control_revision != operation.control_revision
        || operation.control_revision < plan.expected_control_revision
        || !matches!(operation.state, x if x == daemon::RuntimeOperationState::Committed as i32
            || x == daemon::RuntimeOperationState::CommittedDurabilityPending as i32)
        || !matches!(
            operation.persistence_mode.as_str(),
            "exact_configuration_file" | "app_authoritative_regeneration"
        )
    {
        return None;
    }
    Some(daemon::RuntimePersistenceRetryRequest {
        contract_version: 1,
        transaction_id: plan.transaction_id.clone(),
        active: Some(active.clone()),
        control_revision: operation.control_revision,
        persistence_mode: operation.persistence_mode.clone(),
    })
}

/// Only generated custom rules are made private inline snapshots. User resource
/// paths retain their native source identity and are captured by kernel Plan.
fn inline_custom_rules(value: &mut Value, user: &UserConfig, directory: &str) -> Result<(), ()> {
    let expected = build_custom_rule_files(user);
    let Some(sets) = value
        .pointer_mut("/route/rule_set")
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for set in sets {
        let Some(path) = set.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        if std::path::Path::new(path).parent() != Some(std::path::Path::new(directory)) {
            continue;
        }
        let Some(source) = expected.get(name) else {
            return Err(());
        };
        let parsed: Value = serde_json::from_str(source).map_err(|_| ())?;
        let rules = parsed
            .get("rules")
            .filter(|v| v.is_array())
            .cloned()
            .ok_or(())?;
        let object = set.as_object_mut().ok_or(())?;
        object.insert("type".into(), Value::String("inline".into()));
        object.insert("rules".into(), rules);
        object.remove("path");
        object.remove("format");
    }
    Ok(())
}

/// Explicit desired-control deltas only. Config-only edits preserve each live
/// selector independently. A removed live member requires a validated fallback.
fn selector_intents(
    previous: &Value,
    candidate: &Value,
    groups: &[daemon::Group],
    freeze_auto: bool,
) -> Result<Vec<daemon::RuntimeSelectorIntent>, ()> {
    let old = previous
        .get("outbounds")
        .and_then(Value::as_array)
        .ok_or(())?;
    let next = candidate
        .get("outbounds")
        .and_then(Value::as_array)
        .ok_or(())?;
    let mut intents = Vec::new();
    for outbound in next
        .iter()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("selector"))
    {
        let tag = outbound.get("tag").and_then(Value::as_str).ok_or(())?;
        let selected = outbound.get("default").and_then(Value::as_str).ok_or(())?;
        let members = outbound
            .get("outbounds")
            .and_then(Value::as_array)
            .ok_or(())?;
        if !members.iter().any(|v| v.as_str() == Some(selected)) {
            return Err(());
        }
        let old_default = old
            .iter()
            .find(|v| v.get("tag").and_then(Value::as_str) == Some(tag))
            .and_then(|v| v.get("default"))
            .and_then(Value::as_str);
        let live = groups.iter().find(|group| group.tag == tag);
        let removed = live.is_some_and(|group| {
            !members
                .iter()
                .any(|v| v.as_str() == Some(group.selected.as_str()))
        });
        if old_default == Some(selected) && !removed && live.is_some() {
            continue;
        }
        let mut path = Vec::new();
        if live.is_some() {
            let mut current = tag.to_owned();
            loop {
                if path.contains(&current) {
                    return Err(());
                }
                path.push(current.clone());
                let Some(group) = groups.iter().find(|group| group.tag == current) else {
                    break;
                };
                if group.selected.is_empty()
                    || !group.items.iter().any(|item| item.tag == group.selected)
                {
                    return Err(());
                }
                current = group.selected.clone();
            }
        }
        let pin_current_tcp = freeze_auto && old_default == Some(format!("{tag}-test").as_str());
        if pin_current_tcp && path.last().map(String::as_str) != Some(selected) {
            return Err(());
        }
        intents.push(daemon::RuntimeSelectorIntent {
            pin_current_tcp,
            group: tag.to_owned(),
            selected: selected.to_owned(),
            expected_selection_path: path,
        });
    }
    Ok(intents)
}

impl ProxyRuntime {
    pub(super) fn runtime_physical_targets_ready(&self, candidate: &Value) -> bool {
        let Ok(new) = serde_json::from_value::<UserConfig>(candidate.clone()) else {
            return false;
        };
        if !new
            .proxy_mode_type
            .effective_on(self.helper.platform())
            .is_tun()
        {
            return true;
        }
        let Some(old) = self
            .current_config_snapshot()
            .and_then(|value| serde_json::from_value::<UserConfig>(value).ok())
        else {
            return false;
        };
        let Some(plan) = self
            .runtime_binding_state
            .lock()
            .ok()
            .map(|state| state.plan.clone())
        else {
            return false;
        };
        self.runtime_route_targets_reusable(&old, &new, &plan)
    }

    /// Selector-only paths must use the same trusted startup target facts as
    /// graph Apply; a prior inactive host edit cannot make coverage authoritative.
    pub(super) fn runtime_route_targets_reusable(
        &self,
        old: &UserConfig,
        new: &UserConfig,
        plan: &polaris_platform_events::RuntimeBindingPlan,
    ) -> bool {
        let baseline = self.runtime_config_baseline.read().ok();
        let trusted = baseline
            .as_ref()
            .and_then(|value| value.as_ref())
            .map_or(old, |value| &value.physical_route_config);
        crate::runtime::route_binding::runtime_binding_targets_reusable(trusted, new, plan)
    }

    pub(super) fn capture_runtime_config_baseline(
        &self,
        generation: u64,
        deps: GenerateConfigDeps,
        generated: SingBoxConfig,
        physical_route_config: UserConfig,
    ) {
        if let Ok(mut baseline) = self.runtime_config_baseline.write() {
            *baseline = Some(RuntimeConfigBaseline {
                generation,
                committed_transaction: None,
                physical_route_config,
                deps,
                generated,
            });
        }
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            *state = RuntimeConfigApplyState::default();
        }
    }
    pub(super) fn clear_runtime_config_baseline(&self) {
        if let Ok(mut baseline) = self.runtime_config_baseline.write() {
            *baseline = None;
        }
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            *state = RuntimeConfigApplyState::default();
        }
    }
    fn remember_runtime_result(&self, result: RuntimeApplyResult) -> RuntimeApplyResult {
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            state.last = Some(result.clone());
        }
        if result.pending_apply {
            self.restart_deferred.store(true, Ordering::SeqCst);
        }
        self.push_pending_changes();
        result
    }
    pub(super) fn defer_runtime_apply(&self) {
        self.restart_deferred.store(true, Ordering::SeqCst);
        self.push_pending_changes();
    }
    fn restart_choice(
        &self,
        candidate: &Value,
        code: &'static str,
        paths: Vec<String>,
    ) -> RuntimeApplyResult {
        let Some(running) = self.current_config_snapshot() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        if running == *candidate && code == "unsupportedCore" {
            // Old kernels cannot attest external source identity. Do not turn an
            // unchanged desired projection into permission to restart.
            return RuntimeApplyResult::status("unconfirmed");
        }
        let generation = self.gate.generation();
        let intent = self.selector_reconcile.intent_generation();
        let token = format!(
            "{}:{}:{}",
            generation,
            intent,
            self.force_restart_seq.fetch_add(1, Ordering::SeqCst)
        );
        let choice = RestartChoice {
            token: token.clone(),
            candidate: candidate.clone(),
            generation,
            intent,
            running,
        };
        let mut result = RuntimeApplyResult::status("restartRequired");
        result.restart_confirmation = Some(token);
        result.reasons = paths
            .into_iter()
            .map(|path| RuntimeApplyReason { code, path })
            .collect();
        if result.reasons.is_empty() {
            result.reasons.push(RuntimeApplyReason {
                code,
                path: String::new(),
            });
        }
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            state.restart = Some(choice);
        }
        result
    }

    /// Called with switch_serial held. Does not save D or touch live source files.
    pub(super) async fn apply_runtime_candidate(
        self: &Arc<Self>,
        candidate: Value,
    ) -> RuntimeApplyResult {
        let generation = self.gate.generation();
        let intent = self.selector_reconcile.intent_generation();
        if self.gate.is_busy() {
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        if !self.core_running() {
            self.process_deferred_config_deletions().await;
            return RuntimeApplyResult::status("skipped");
        }
        let desired = match self.config.current() {
            Ok(value) => value,
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        let baseline = self
            .runtime_config_baseline
            .read()
            .ok()
            .and_then(|s| s.clone());
        let Some(mut baseline) = baseline.filter(|s| s.generation == generation) else {
            return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"));
        };
        // Authenticate against the accepted running service, never an edited D secret.
        let Some(service) = baseline
            .generated
            .services
            .as_ref()
            .and_then(|services| services.iter().find(|service| service.type_field == "api"))
        else {
            return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"));
        };
        let port = service.listen_port;
        let secret = service.secret.clone().unwrap_or_default();
        let client = match SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await
        {
            Ok(client) => client,
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        // An ambiguous previous Apply is reconciled by its original transaction ID.
        // Never send it again or replace it with a newer candidate blindly.
        let pending = self
            .runtime_config_apply
            .lock()
            .ok()
            .and_then(|s| s.pending.clone());
        if let Some(pending) = pending {
            let result = self.reconcile_runtime_operation(&client, pending).await;
            let superseded = result
                .reasons
                .iter()
                .any(|reason| reason.code == "OWNER_STATE_SUPERSEDED");
            if !superseded || self.config.current().ok().as_ref() != Some(&candidate) {
                return self.remember_runtime_result(result);
            }
            // Only an explicit Apply of the latest saved D may correct a typed
            // regeneration supersession. Generic callback failures stay pending.
            baseline = match self
                .runtime_config_baseline
                .read()
                .ok()
                .and_then(|value| value.clone())
            {
                Some(value) if value.generation == generation => value,
                _ => {
                    return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
                }
            };
        }
        let capabilities = match client.get_runtime_capabilities().await {
            Ok(value) => value,
            Err(RuntimeConfigError::Unsupported(_)) => {
                let result = self.restart_choice(&candidate, "unsupportedCore", vec![]);
                return self.remember_runtime_result(result);
            }
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        if capabilities.contract_version != 1 || !capabilities.connected_hot_enabled {
            let result = self.restart_choice(&candidate, "unsupportedCore", vec![]);
            return self.remember_runtime_result(result);
        }
        if !self.selector_operation_is_current(generation, intent) {
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        let typed: UserConfig = match serde_json::from_value(candidate.clone()) {
            Ok(value) => value,
            Err(_) => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        if self
            .validate_required_bind_interfaces(&typed)
            .await
            .is_err()
        {
            return self.remember_runtime_result(RuntimeApplyResult::status("rejected"));
        }
        let mut deps = baseline.deps.clone();
        let (log_level, disable_log_file) = super::core_log::log_axes_from_config(&candidate);
        deps.log_level = log_level;
        deps.disable_log_file = disable_log_file;
        deps.node_dns_decoy_override = self.node_dns_decoy_override();
        let Some(binding_plan) = self
            .runtime_binding_state
            .lock()
            .ok()
            .map(|state| state.plan.clone())
        else {
            return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"));
        };
        let previous_bindings = binding_plan.bindings.clone();
        // This planner's route probes are only trustworthy before TUN startup.
        // Retain accepted physical facts for ordinary graph edits. New automatic
        // targets need a safe pre-TUN plan, never a probe through our own tunnel.
        if typed
            .proxy_mode_type
            .effective_on(self.helper.platform())
            .is_tun()
        {
            if !crate::runtime::route_binding::runtime_binding_targets_reusable(
                &baseline.physical_route_config,
                &typed,
                &binding_plan,
            ) {
                let result = self.restart_choice(
                    &candidate,
                    "structuralChange",
                    vec!["outbounds.physical_route_binding".into()],
                );
                return self.remember_runtime_result(result);
            }
        }
        if !self.selector_operation_is_current(generation, intent) {
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        let bindings = binding_plan.bindings.clone();
        if baseline.deps.network_canary_port.is_none()
            && !typed.network_profiles.is_empty()
            && self
                .current_config_snapshot()
                .and_then(|value| serde_json::from_value::<UserConfig>(value).ok())
                .is_some_and(|old| old.network_profiles.is_empty())
        {
            let result = self.restart_choice(
                &candidate,
                "structuralChange",
                vec!["inbounds.network-canary.listen_port".into()],
            );
            return self.remember_runtime_result(result);
        }
        let outcome = match generate_sing_box_config_with_report_and_runtime_bindings(
            &typed,
            &BTreeMap::new(),
            &deps,
            &bindings,
        ) {
            Ok(value)
                if value.invalid_nodes.is_empty()
                    && value.pruned_rule_set_tags.is_empty()
                    && value.pruned_env_rules.iter().all(|rule| rule.is_warning()) =>
            {
                value
            }
            _ => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        let network_canary = outcome.network_canary;
        let mut generated = outcome.config;
        generated.polaris_windows_directory = baseline.generated.polaris_windows_directory.clone();
        let mut content = match serde_json::to_value(&generated) {
            Ok(value) => value,
            Err(_) => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        // Native directory custody is host-issued, not recreated by the graph builder.
        if let Ok(previous) = serde_json::to_value(&baseline.generated) {
            if let Some(custody) = previous.get("polaris_windows_directory") {
                content["polaris_windows_directory"] = custody.clone();
            }
        }
        if inline_custom_rules(&mut content, &typed, &deps.custom_rules_dir).is_err() {
            return self.remember_runtime_result(RuntimeApplyResult::status("rejected"));
        }
        // Compare with the current R projection, not startup defaults, which may
        // have diverged after explicit live selection. Snapshot paths are CAS-bound
        // by the kernel, including automatic URLTest winners.
        let previous: UserConfig = match self
            .current_config_snapshot()
            .and_then(|v| serde_json::from_value(v).ok())
        {
            Some(value) => value,
            None => return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed")),
        };
        let previous = match generate_sing_box_config_with_report_and_runtime_bindings(
            &previous,
            &BTreeMap::new(),
            &deps,
            &previous_bindings,
        )
        .ok()
        .and_then(|value| serde_json::to_value(value.config).ok())
        {
            Some(value) => value,
            None => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        let groups = match client.first_groups_snapshot().await {
            Ok(groups) => groups,
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        let selector_intents = match selector_intents(
            &previous,
            &content,
            &groups,
            self.current_config_snapshot()
                .is_some_and(|v| v.get("autoSelectEnabled").and_then(Value::as_bool) == Some(true))
                && !typed.auto_select_enabled,
        ) {
            Ok(intents) => intents,
            Err(_) => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        let config_content = match serde_json::to_string(&content) {
            Ok(value) => value,
            Err(_) => return self.remember_runtime_result(RuntimeApplyResult::status("rejected")),
        };
        let mesh_evidence = self.prepare_mesh_route_run_with_receipt(
            outcome.mesh_route_candidates,
            outcome.mesh_route_total_candidate_count,
            outcome.mesh_route_diagnostics_limited,
            outcome.mesh_route_dns_owner_server_id,
            &[],
            &typed,
            &generated,
            &self.runtime_config_path(),
            &config_content,
            generation,
            true,
        );
        // Omission inherits the kernel's immutable startup override snapshot.
        // Explicit empty options would erase platform projection on newer hosts.
        let overrides = None;
        let deletion_scope = match self.config.runtime_deletion_scope(
            &desired,
            &self.current_config_snapshot().unwrap_or(Value::Null),
            &candidate,
        ) {
            Ok(scope) => scope,
            Err(_) => return self.remember_runtime_result(RuntimeApplyResult::status("conflict")),
        };
        let plan = match client
            .plan_runtime_config(daemon::PlanRuntimeConfigRequest {
                contract_version: 1,
                expected: capabilities.active.clone(),
                config_content,
                overrides,
                expected_control_revision: capabilities.control_revision,
                selector_intents,
            })
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        if plan.contract_version != 1
            || !identity_equal(plan.expected.as_ref(), capabilities.active.as_ref())
            || plan.expected_control_revision != capabilities.control_revision
            || !self.selector_operation_is_current(generation, intent)
        {
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        if !plan.applicable {
            let paths: Vec<String> = plan
                .impacts
                .iter()
                .filter(|i| i.update_class == daemon::RuntimeUpdateClass::RestartRequired as i32)
                .map(|i| i.effective_path.clone())
                .collect();
            let result = if paths.is_empty() {
                RuntimeApplyResult::status("rejected")
            } else {
                self.restart_choice(&candidate, "structuralChange", paths)
            };
            return self.remember_runtime_result(result);
        }
        if plan.plan_id.is_empty()
            || plan.transaction_id.is_empty()
            || plan.candidate_digest.is_empty()
            || plan.impact_digest.is_empty()
            || !plan.errors.is_empty()
        {
            return self.remember_runtime_result(RuntimeApplyResult::status("rejected"));
        }
        let pending = PendingOperation {
            desired,
            deletion_scope,
            physical_route_config: baseline.physical_route_config.clone(),
            generation,
            intent,
            candidate,
            typed,
            generated,
            deps,
            plan,
            network_canary,
            published: false,
            binding_plan,
            mesh_evidence,
        };
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            state.pending = Some(pending.clone());
            state.restart = None;
        } else {
            return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"));
        }
        let result = client
            .apply_runtime_config(daemon::ApplyRuntimeConfigRequest {
                contract_version: 1,
                transaction_id: pending.plan.transaction_id.clone(),
                plan_id: pending.plan.plan_id.clone(),
                expected: pending.plan.expected.clone(),
                candidate_digest: pending.plan.candidate_digest.clone(),
                accepted_impact_digest: pending.plan.impact_digest.clone(),
                expected_control_revision: pending.plan.expected_control_revision,
            })
            .await;
        let transaction_id = pending.plan.transaction_id.clone();
        let mut result = match result {
            Ok(operation) => {
                let operation = self
                    .acknowledge_regeneration_owner(&pending, operation)
                    .await;
                self.settle_runtime_operation(&pending, operation)
            }
            Err(_) => self.reconcile_runtime_operation(&client, pending).await,
        };
        result.transaction_id = Some(transaction_id);
        result.runtime_generation = Some(generation);
        result.selection_intent_generation = Some(intent);
        self.remember_runtime_result(result)
    }

    async fn reconcile_runtime_operation(
        self: &Arc<Self>,
        client: &SingBoxApiClient,
        pending: PendingOperation,
    ) -> RuntimeApplyResult {
        let Some(identity) = pending.plan.expected.as_ref() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let mut result = match client
            .get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
                contract_version: 1,
                transaction_id: pending.plan.transaction_id.clone(),
                runtime_incarnation: identity.incarnation.clone(),
            })
            .await
        {
            Ok(status)
                if status.contract_version == 1
                    && status.lookup_state == daemon::RuntimeLookupState::Found as i32 =>
            {
                match status.operation {
                    Some(operation) => {
                        let operation = self
                            .acknowledge_regeneration_owner(&pending, operation)
                            .await;
                        self.settle_runtime_operation(&pending, operation)
                    }
                    None => RuntimeApplyResult::status("unconfirmed"),
                }
            }
            _ => RuntimeApplyResult::status("unconfirmed"),
        };
        result.transaction_id = Some(pending.plan.transaction_id.clone());
        result.runtime_generation = Some(pending.generation);
        result.selection_intent_generation = Some(pending.intent);
        result
    }

    /// iOS retains its established desired-profile regeneration contract. The
    /// provider acknowledges reload bytes; this original App owner separately
    /// attests durable D while excluding writers through the bounded RPC.
    async fn acknowledge_regeneration_owner(
        self: &Arc<Self>,
        pending: &PendingOperation,
        operation: daemon::RuntimeConfigOperation,
    ) -> daemon::RuntimeConfigOperation {
        if operation.persistence_mode != "app_authoritative_regeneration"
            || !operation.owner_reload_acknowledged
            || operation.durable
            || operation.state != daemon::RuntimeOperationState::CommittedDurabilityPending as i32
            || operation.transaction_id != pending.plan.transaction_id
            || operation.candidate_digest != pending.plan.candidate_digest
            || !identity_equal(operation.before.as_ref(), pending.plan.expected.as_ref())
        {
            return operation;
        }
        let Some(active) = operation
            .active
            .clone()
            .filter(|active| active.effective_digest == pending.plan.candidate_digest)
        else {
            return operation;
        };
        let Some(service) = pending
            .generated
            .services
            .as_ref()
            .and_then(|services| services.iter().find(|service| service.type_field == "api"))
        else {
            return operation;
        };
        let endpoint = (
            service.listen_port,
            service.secret.clone().unwrap_or_default(),
        );
        let me = Arc::clone(self);
        let pending = pending.clone();
        let transaction = operation.transaction_id.clone();
        let control_revision = operation.control_revision;
        let handle = tokio::runtime::Handle::current();
        let acknowledged = tokio::task::spawn_blocking(move || {
            let client = handle
                .block_on(SingBoxApiClient::connect(
                    Endpoint::new("127.0.0.1", endpoint.0),
                    endpoint.1,
                ))
                .ok()?;
            me.config
                .with_durable_runtime_owner(&pending.desired, |digest, revision| {
                    me.selector_reconcile.with_intent_claim(|intent| {
                        if intent != pending.intent
                            || me.gate.generation() != pending.generation
                            || me.gate.is_busy()
                            || !me.core_running()
                        {
                            return None;
                        }
                        let response = handle
                            .block_on(client.acknowledge_runtime_persistence(
                                daemon::RuntimePersistenceAcknowledgementRequest {
                                    contract_version: 1,
                                    transaction_id: transaction.clone(),
                                    active: Some(active.clone()),
                                    control_revision,
                                    persistence_mode: "app_authoritative_regeneration".into(),
                                    desired_state_digest: digest.clone(),
                                    desired_state_revision: revision.clone(),
                                },
                            ))
                            .ok()?;
                        (response.transaction_id == transaction
                            && identity_equal(response.active.as_ref(), Some(&active))
                            && response.control_revision == control_revision
                            && response.persistence_mode == "app_authoritative_regeneration"
                            && response.owner_reload_acknowledged
                            && response.desired_state_digest == digest
                            && response.desired_state_revision == revision)
                            .then_some(response)
                    })
                })
                .ok()
                .flatten()
        })
        .await
        .ok()
        .flatten();
        acknowledged.unwrap_or(operation)
    }

    fn settle_runtime_operation(
        self: &Arc<Self>,
        pending: &PendingOperation,
        operation: daemon::RuntimeConfigOperation,
    ) -> RuntimeApplyResult {
        if operation.contract_version != 1
            || operation.transaction_id != pending.plan.transaction_id
            || operation.candidate_digest != pending.plan.candidate_digest
            || operation.impact_digest != pending.plan.impact_digest
            || !identity_equal(operation.before.as_ref(), pending.plan.expected.as_ref())
        {
            return RuntimeApplyResult::status("unconfirmed");
        }
        let committed = operation.state == daemon::RuntimeOperationState::Committed as i32
            || operation.state == daemon::RuntimeOperationState::CommittedDurabilityPending as i32;
        if !committed {
            if matches!(
                daemon::RuntimeOperationState::try_from(operation.state),
                Ok(daemon::RuntimeOperationState::RejectedUnapplied
                    | daemon::RuntimeOperationState::AbortedUnapplied)
            ) {
                if let Ok(mut state) = self.runtime_config_apply.lock() {
                    state.pending = None;
                }
                return RuntimeApplyResult::status("rejected");
            }
            return RuntimeApplyResult::status("unconfirmed");
        }
        let Some(active) = operation.active.as_ref() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let Some(before) = operation.before.as_ref() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        if active.incarnation != before.incarnation
            || active.effective_digest != pending.plan.candidate_digest
            || active.applied_revision < before.applied_revision
            || operation.control_revision < pending.plan.expected_control_revision
        {
            return RuntimeApplyResult::status("unconfirmed");
        }
        if pending.plan.no_op
            && (!identity_equal(operation.active.as_ref(), operation.before.as_ref())
                || operation.control_revision != pending.plan.expected_control_revision)
        {
            return RuntimeApplyResult::status("unconfirmed");
        }
        // The receipt proves kernel state, but obsolete local intent cannot publish R
        // over a newer owner. Consume terminal state without claiming local publication.
        if !self.selector_operation_is_current(pending.generation, pending.intent) {
            // A terminal receipt must not publish over a newer owner. Preserve
            // nondurable generic debt; only verified regeneration supersession
            // permits a corrective latest-D transaction. Invalidate stale cheap
            // selector classification until a fresh graph transaction reconciles.
            let superseded = self.regeneration_owner_superseded(pending, &operation);
            // A committed graph with generic persistence failure remains owned
            // pending debt even after selection intent changes. Only an actually
            // durable receipt or verified regeneration supersession releases it.
            if stale_receipt_releases_pending(&operation, superseded) {
                if let Ok(mut state) = self.runtime_config_apply.lock() {
                    state.pending = None;
                }
            }
            if self.gate.generation() == pending.generation {
                if let Ok(mut snapshot) = self.switch_snapshot.write() {
                    *snapshot = None;
                }
                self.selector_reconcile.mark_required();
            }
            let mut result = RuntimeApplyResult::status("conflict");
            if superseded {
                result.persistence_mode = Some(operation.persistence_mode.clone());
                result.reasons.push(RuntimeApplyReason {
                    code: "OWNER_STATE_SUPERSEDED",
                    path: "desired".into(),
                });
            }
            return result;
        }
        let published = pending.published
            || self.selector_reconcile.with_intent_claim(|intent| {
                if intent != pending.intent
                    || self.gate.generation() != pending.generation
                    || self.gate.is_busy()
                {
                    return false;
                }
                let Ok(mut current) = self.current_config.write() else {
                    return false;
                };
                let Ok(mut snapshot) = self.switch_snapshot.write() else {
                    return false;
                };
                let Ok(mut startup) = self.startup_snapshot.write() else {
                    return false;
                };
                let Ok(mut baseline) = self.runtime_config_baseline.write() else {
                    return false;
                };
                let Ok(mut binding_state) = self.runtime_binding_state.lock() else {
                    return false;
                };
                self.coex
                    .commit_config(&mut current, pending.candidate.clone());
                *snapshot = Some(Self::build_switch_snapshot(
                    &pending.typed,
                    &pending.generated,
                    &pending.deps,
                ));
                *startup = Some(pending.candidate.clone());
                *baseline = Some(RuntimeConfigBaseline {
                    generation: pending.generation,
                    committed_transaction: Some(pending.plan.transaction_id.clone()),
                    physical_route_config: pending.physical_route_config.clone(),
                    deps: pending.deps.clone(),
                    generated: pending.generated.clone(),
                });
                binding_state.plan = pending.binding_plan.clone();
                binding_state.explicit_unavailable = Default::default();
                true
            });
        if !published {
            return RuntimeApplyResult::status("conflict");
        }
        if !pending.published {
            if !pending.plan.no_op {
                self.arm_network_canary(pending.generation, pending.network_canary.clone());
            }
            self.publish_mesh_route_runtime_receipt(pending.mesh_evidence.clone());
            self.spawn_runtime_retirement_reconcile(pending, &operation);
        }
        // The provider accepted reload bytes, but a newer durably saved D is
        // authoritative for the next App bootstrap. Keep the old kernel receipt
        // nondurable; expose explicit supersession and allow a corrective latest-D
        // transaction instead of falsely acknowledging the obsolete version.
        if self.regeneration_owner_superseded(pending, &operation) {
            if let Ok(mut state) = self.runtime_config_apply.lock() {
                state.pending = None;
            }
            let mut result = RuntimeApplyResult::status("conflict");
            result.persistence_mode = Some(operation.persistence_mode.clone());
            result.reasons.push(RuntimeApplyReason {
                code: "OWNER_STATE_SUPERSEDED",
                path: "desired".into(),
            });
            return result;
        }
        if let Ok(mut state) = self.runtime_config_apply.lock() {
            state.pending = if operation.durable
                && operation.state == daemon::RuntimeOperationState::Committed as i32
            {
                None
            } else {
                let mut retained = pending.clone();
                retained.published = true;
                Some(retained)
            };
        }
        self.restart_deferred.store(false, Ordering::SeqCst);
        let mut result = RuntimeApplyResult::status(
            if operation.durable
                && operation.state == daemon::RuntimeOperationState::Committed as i32
            {
                "hotApplied"
            } else {
                "hotDurabilityPending"
            },
        );
        result.persistence_mode = Some(operation.persistence_mode);
        result
    }

    fn regeneration_owner_superseded(
        &self,
        pending: &PendingOperation,
        operation: &daemon::RuntimeConfigOperation,
    ) -> bool {
        if operation.durable
            || !operation.owner_reload_acknowledged
            || operation.persistence_mode != "app_authoritative_regeneration"
        {
            return false;
        }
        let newer = self
            .config
            .current()
            .ok()
            .filter(|desired| desired != &pending.desired);
        newer.as_ref().is_some_and(|desired| {
            self.config
                .with_durable_runtime_owner(desired, |_, _| ())
                .is_ok()
        })
    }

    /// Deferred node-state deletion waits for the kernel's old graph retirement,
    /// independently of durable config success. Never infer DRAINED from COMMITTED.
    fn spawn_runtime_retirement_reconcile(
        self: &Arc<Self>,
        pending: &PendingOperation,
        operation: &daemon::RuntimeConfigOperation,
    ) {
        let me = Arc::clone(self);
        let generation = pending.generation;
        let transaction_id = pending.plan.transaction_id.clone();
        let Some(expected) = pending.plan.expected.clone() else {
            return;
        };
        let candidate_digest = pending.plan.candidate_digest.clone();
        let initial = operation.retirement_state.clone();
        let candidate = pending.candidate.clone();
        let scope = pending.deletion_scope.clone();
        if scope.is_empty() {
            return;
        }
        let endpoint = pending
            .generated
            .services
            .as_ref()
            .and_then(|services| services.iter().find(|service| service.type_field == "api"))
            .map(|service| {
                (
                    service.listen_port,
                    service.secret.clone().unwrap_or_default(),
                )
            });
        tokio::spawn(async move {
            let mut retirement = initial;
            loop {
                if me.gate.generation() != generation || !me.core_running() {
                    return;
                }
                if retirement == "DRAINED" {
                    let _serial = me.switch_serial.lock().await;
                    if me.gate.generation() == generation
                        && me.core_running()
                        && me.current_config_snapshot().as_ref() == Some(&candidate)
                        && me
                            .runtime_config_baseline
                            .read()
                            .ok()
                            .and_then(|baseline| {
                                baseline
                                    .as_ref()
                                    .and_then(|baseline| baseline.committed_transaction.clone())
                            })
                            .as_deref()
                            == Some(transaction_id.as_str())
                    {
                        me.process_runtime_deletions(&scope).await;
                    }
                    return;
                }
                if retirement != "PENDING" {
                    return;
                }
                tokio::select! {
                    _ = me.gen_changed.notified() => { if me.gate.generation() != generation { return; } }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                }
                let Some((port, secret)) = endpoint.as_ref() else {
                    return;
                };
                let Ok(client) =
                    SingBoxApiClient::connect(Endpoint::new("127.0.0.1", *port), secret.clone())
                        .await
                else {
                    continue;
                };
                match client
                    .get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
                        contract_version: 1,
                        transaction_id: transaction_id.clone(),
                        runtime_incarnation: expected.incarnation.clone(),
                    })
                    .await
                {
                    Ok(status)
                        if status.contract_version == 1
                            && status.lookup_state == daemon::RuntimeLookupState::Found as i32 =>
                    {
                        let Some(operation) = status.operation else {
                            return;
                        };
                        if operation.transaction_id != transaction_id
                            || operation.candidate_digest != candidate_digest
                            || !identity_equal(operation.before.as_ref(), Some(&expected))
                            || !matches!(operation.state, x if x == daemon::RuntimeOperationState::Committed as i32 || x == daemon::RuntimeOperationState::CommittedDurabilityPending as i32)
                        {
                            return;
                        }
                        retirement = operation.retirement_state;
                    }
                    Ok(_) => return, // Expired/unknown receipts leave the journal intact.
                    Err(_) => continue,
                }
            }
        });
    }

    /// User choice is bound to the exact candidate, running projection, generation
    /// and selection intent. Confirmation cannot silently apply a later edit.
    pub(crate) async fn apply_pending_transaction(
        self: &Arc<Self>,
        confirmation: Option<String>,
    ) -> RuntimeApplyResult {
        let _serial = self.switch_serial.lock().await;
        if let Some(token) = confirmation {
            let choice = self
                .runtime_config_apply
                .lock()
                .ok()
                .and_then(|mut state| state.restart.take());
            let Some(choice) = choice.filter(|choice| choice.token == token) else {
                return RuntimeApplyResult::status("conflict");
            };
            let candidate_current = self
                .config
                .with_current(|value| value == &choice.candidate)
                .unwrap_or(false);
            if !candidate_current
                || self.current_config_snapshot().as_ref() != Some(&choice.running)
                || !self.selector_operation_is_current(choice.generation, choice.intent)
                || self.gate.is_busy()
            {
                return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
            }
            let id = self.force_restart_seq.fetch_add(1, Ordering::SeqCst);
            let accepted = self.selector_reconcile.with_intent_claim(|intent| {
                if intent != choice.intent
                    || self.gate.generation() != choice.generation
                    || self.gate.is_busy()
                {
                    return false;
                }
                let Ok(mut pending) = self.pending_force_restart.write() else {
                    return false;
                };
                self.debounced.invalidate_pending();
                *pending = Some((
                    id,
                    choice.candidate,
                    super::ForceRestartSource::Confirmed {
                        intent_generation: choice.intent,
                    },
                ));
                self.gate.set_force_restart(id);
                true
            });
            if !accepted {
                return RuntimeApplyResult::status("conflict");
            }
            self.schedule_restart_for_generation(choice.generation);
            let mut result = RuntimeApplyResult::status("applied");
            result.apply_id = Some(id);
            return result;
        }
        let candidate = match self.config.current() {
            Ok(candidate) => candidate,
            Err(_) => return RuntimeApplyResult::status("rejected"),
        };
        self.apply_runtime_candidate(candidate).await
    }

    #[cfg(test)]
    pub(super) async fn apply_confirmed_for_test(self: &Arc<Self>) -> (&'static str, Option<u64>) {
        let candidate = self.config.current().expect("test desired configuration");
        let choice = self.restart_choice(&candidate, "structuralChange", vec!["inbounds".into()]);
        let result = match choice.restart_confirmation {
            Some(token) => self.apply_pending_transaction(Some(token)).await,
            None => choice,
        };
        (result.status, result.apply_id)
    }

    /// Explicit user recovery of one retained transaction. Never saves new D,
    /// replans, replays Apply, restarts, or retries a callback from the polling path.
    pub(crate) async fn retry_runtime_persistence(
        self: &Arc<Self>,
        transaction_id: String,
    ) -> RuntimeApplyResult {
        let _serial = self.switch_serial.lock().await;
        let pending = self
            .runtime_config_apply
            .lock()
            .ok()
            .and_then(|state| state.pending.clone());
        let Some(pending) = pending else {
            // A status poll may have completed the same receipt while this
            // explicit Retry waited for switch_serial. Return that exact terminal
            // result without invoking any callback or applying a different graph.
            if self.core_running() && !self.gate.is_busy() {
                if let Some(result) = self.runtime_config_apply.lock().ok().and_then(|state| {
                    terminal_persistence_receipt(
                        state.last.as_ref(),
                        &transaction_id,
                        self.gate.generation(),
                        self.selector_reconcile.intent_generation(),
                    )
                }) {
                    return result;
                }
            }
            return RuntimeApplyResult::status("conflict");
        };
        if pending.plan.transaction_id != transaction_id {
            return RuntimeApplyResult::status("conflict");
        }
        let mut result = self.retry_runtime_persistence_inner(&pending).await;
        result.transaction_id = Some(transaction_id);
        result.runtime_generation = Some(pending.generation);
        result.selection_intent_generation = Some(pending.intent);
        self.remember_runtime_result(result)
    }

    async fn retry_runtime_persistence_inner(
        self: &Arc<Self>,
        pending: &PendingOperation,
    ) -> RuntimeApplyResult {
        if self.gate.is_busy()
            || !self.core_running()
            || !self.selector_operation_is_current(pending.generation, pending.intent)
        {
            return RuntimeApplyResult::status("conflict");
        }
        let baseline = self
            .runtime_config_baseline
            .read()
            .ok()
            .and_then(|value| value.clone());
        let Some(service) = baseline
            .as_ref()
            .filter(|baseline| baseline.generation == pending.generation)
            .and_then(|baseline| baseline.generated.services.as_ref())
            .and_then(|services| services.iter().find(|service| service.type_field == "api"))
        else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let Ok(client) = SingBoxApiClient::connect(
            Endpoint::new("127.0.0.1", service.listen_port),
            service.secret.clone().unwrap_or_default(),
        )
        .await
        else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let Some(expected) = pending.plan.expected.as_ref() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let status = match client
            .get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
                contract_version: 1,
                transaction_id: pending.plan.transaction_id.clone(),
                runtime_incarnation: expected.incarnation.clone(),
            })
            .await
        {
            Ok(status) => status,
            Err(_) => return RuntimeApplyResult::status("unconfirmed"),
        };
        let Some(request) = persistence_retry_request(&pending.plan, &status) else {
            return RuntimeApplyResult::status("conflict");
        };
        if self.gate.is_busy()
            || !self.selector_operation_is_current(pending.generation, pending.intent)
        {
            return RuntimeApplyResult::status("conflict");
        }
        let Some(operation) = status.operation else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        // A successful regeneration callback needs only the separate durable-D
        // acknowledgement. Otherwise invoke the host with NO D/intent claim held.
        let operation = if operation.durable || operation.owner_reload_acknowledged {
            operation
        } else {
            match client.retry_runtime_persistence(request.clone()).await {
                Ok(operation) => operation,
                Err(RuntimeConfigError::Unsupported(_)) => {
                    let mut result = RuntimeApplyResult::status("unconfirmed");
                    result.reasons.push(RuntimeApplyReason {
                        code: "PERSISTENCE_RETRY_UNSUPPORTED",
                        path: "persistence".into(),
                    });
                    return result;
                }
                Err(_) => {
                    // Unknown outcome: only read the same receipt, never resend.
                    return self
                        .reconcile_runtime_operation(&client, pending.clone())
                        .await;
                }
            }
        };
        if !identity_equal(operation.active.as_ref(), request.active.as_ref())
            || operation.control_revision != request.control_revision
            || operation.persistence_mode != request.persistence_mode
        {
            return RuntimeApplyResult::status("unconfirmed");
        }
        let operation = self
            .acknowledge_regeneration_owner(pending, operation)
            .await;
        self.settle_runtime_operation(pending, operation)
    }

    /// Read-only reconciliation of the original transaction. Never replans or
    /// reapplies; suitable for a bounded UI poll while durability is pending.
    pub(crate) async fn runtime_apply_status(
        self: &Arc<Self>,
        transaction_id: String,
    ) -> RuntimeApplyResult {
        let mut result = self
            .runtime_apply_status_inner(transaction_id.clone())
            .await;
        result.transaction_id = Some(transaction_id);
        result
    }

    async fn runtime_apply_status_inner(
        self: &Arc<Self>,
        transaction_id: String,
    ) -> RuntimeApplyResult {
        let _serial = self.switch_serial.lock().await;
        if !self.core_running() {
            return RuntimeApplyResult::status("skipped");
        }
        let pending = self
            .runtime_config_apply
            .lock()
            .ok()
            .and_then(|state| state.pending.clone());
        let Some(pending) = pending else {
            return self
                .runtime_config_apply
                .lock()
                .ok()
                .and_then(|state| state.last.clone())
                .filter(|result| result.transaction_id.as_deref() == Some(transaction_id.as_str()))
                .unwrap_or_else(|| RuntimeApplyResult::status("conflict"));
        };
        if pending.plan.transaction_id != transaction_id
            || pending.generation != self.gate.generation()
        {
            return RuntimeApplyResult::status("conflict");
        }
        let baseline = self
            .runtime_config_baseline
            .read()
            .ok()
            .and_then(|value| value.clone());
        let Some(service) = baseline
            .as_ref()
            .and_then(|baseline| baseline.generated.services.as_ref())
            .and_then(|services| services.iter().find(|service| service.type_field == "api"))
        else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let client = match SingBoxApiClient::connect(
            Endpoint::new("127.0.0.1", service.listen_port),
            service.secret.clone().unwrap_or_default(),
        )
        .await
        {
            Ok(client) => client,
            Err(_) => return RuntimeApplyResult::status("unconfirmed"),
        };
        let result = self.reconcile_runtime_operation(&client, pending).await;
        self.remember_runtime_result(result)
    }

    pub(super) fn runtime_result_outcome(result: &RuntimeApplyResult) -> SwitchOutcome {
        match result.status {
            "hotApplied" | "hotDurabilityPending" => SwitchOutcome::HotSwitched,
            "skipped" => SwitchOutcome::NotRunning,
            "conflict" | "unconfirmed" => SwitchOutcome::Pending,
            _ => SwitchOutcome::Deferred,
        }
    }
}

/// Preview only. Kernel Plan remains authoritative; presence of a TUN/cache
/// field is never enough. These are changed physical host/service controls.
pub(super) fn structural_runtime_change(old: &Value, new: &Value) -> bool {
    [
        "proxyModeType",
        "tunConfig",
        "mixedPort",
        "httpPort",
        "allowLan",
        "clashApiSecret",
        "singboxDashboard",
    ]
    .iter()
    .any(|key| old.get(*key) != new.get(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
        let old = json!({"tunConfig":{"mtu":1500},"cachePath":"same","dnsConfig":{"optimisticCache":false}});
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
}

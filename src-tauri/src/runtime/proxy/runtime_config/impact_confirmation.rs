use super::{
    identity_equal, paired_runtime_capabilities, PendingOperation, ProxyRuntime, RuntimeApplyResult,
};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeImpactPreview {
    pub(crate) path: String,
    pub(crate) update_class: i32,
    pub(crate) reason: String,
    pub(crate) affected_session_classes: Vec<String>,
    pub(crate) reconnect_required: bool,
}

// Exactly one private recipe is retained. No config, credential, Plan or OS
// snapshot is accepted back from IPC, serialized into a token, or regenerated
// after the user confirms. The original Core Apply checks remain authoritative.
#[derive(Clone)]
pub(super) struct PreparedImpactChoice {
    pub(super) token: String,
    pub(super) pending: PendingOperation,
    pub(super) running: Value,
    expires: Instant,
}

impl ProxyRuntime {
    pub(super) fn prepare_runtime_impact_choice(
        &self,
        pending: PendingOperation,
    ) -> RuntimeApplyResult {
        let Some(running) = self.current_config_snapshot() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let expires = pending.plan.expires_at_unix_milli;
        if expires <= 0 || expires as u128 <= now.as_millis() {
            return RuntimeApplyResult::status("conflict");
        }
        let lifetime =
            Duration::from_millis((expires as u128 - now.as_millis()).min(300_000) as u64);
        let mut result = RuntimeApplyResult::status("impactConfirmationRequired");
        let token = uuid::Uuid::new_v4().to_string();
        result.impact_confirmation = Some(token.clone());
        result.runtime_generation = Some(pending.generation);
        result.selection_intent_generation = Some(pending.intent);
        result.core_impacts = pending
            .plan
            .impacts
            .iter()
            .map(|impact| RuntimeImpactPreview {
                path: impact.effective_path.clone(),
                update_class: impact.update_class,
                reason: impact.reason.clone(),
                affected_session_classes: impact.affected_session_classes.clone(),
                reconnect_required: impact.reconnect_required,
            })
            .collect();
        result.host_effect_previews = pending
            .host_plan
            .as_ref()
            .map(|plan| plan.effects.clone())
            .unwrap_or_default();
        if self.gate.is_busy()
            || !self.selector_operation_is_current(pending.generation, pending.intent)
            || self.config.current().ok().as_ref() != Some(&pending.desired)
        {
            return RuntimeApplyResult::status("conflict");
        }
        let Ok(mut state) = self.runtime_config_apply.lock() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        if !state.host_only_preserves_pending_core(&pending) {
            return RuntimeApplyResult::status("conflict");
        }
        state.restart = None;
        state.impact = Some(PreparedImpactChoice {
            token,
            pending,
            running,
            expires: Instant::now() + lifetime,
        });
        result
    }

    pub(crate) fn cancel_runtime_impact_choice(&self, token: &str) -> RuntimeApplyResult {
        let Ok(mut state) = self.runtime_config_apply.lock() else {
            return RuntimeApplyResult::status("unconfirmed");
        };
        if !state
            .impact
            .as_ref()
            .is_some_and(|choice| choice.token == token)
        {
            return RuntimeApplyResult::status("conflict");
        }
        state.impact = None;
        RuntimeApplyResult::status("cancelled")
    }

    pub(crate) async fn confirm_runtime_impact_choice(
        self: &Arc<Self>,
        token: String,
    ) -> RuntimeApplyResult {
        let _serial = self.switch_serial.lock().await;
        let choice = self
            .runtime_config_apply
            .lock()
            .ok()
            .and_then(|state| state.impact.clone())
            .filter(|choice| choice.token == token);
        let Some(choice) = choice else {
            return RuntimeApplyResult::status("conflict");
        };
        let pending = &choice.pending;
        let local_matches = || {
            Instant::now() < choice.expires
                && !self.gate.is_busy()
                && self.core_running()
                && self.selector_operation_is_current(pending.generation, pending.intent)
                && self.config.current().ok().as_ref() == Some(&pending.desired)
                && self.current_config_snapshot().as_ref() == Some(&choice.running)
        };
        if !local_matches() {
            self.cancel_runtime_impact_choice(&token);
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        let client = match self.runtime_management_client(pending.generation).await {
            Ok(client) => client,
            Err(_) => {
                return self.remember_runtime_result(RuntimeApplyResult::status("unconfirmed"))
            }
        };
        let capabilities = match paired_runtime_capabilities(&client, &pending.pairing).await {
            Ok(capabilities) => capabilities,
            Err(result) => return self.remember_runtime_result(*result),
        };
        if !local_matches()
            || !identity_equal(capabilities.active.as_ref(), pending.plan.expected.as_ref())
            || capabilities.control_revision != pending.plan.expected_control_revision
            || pending
                .host_plan
                .as_ref()
                .is_none_or(|plan| !self.runtime_host_facts_match(plan))
        {
            self.cancel_runtime_impact_choice(&token);
            return self.remember_runtime_result(RuntimeApplyResult::status("conflict"));
        }
        self.execute_prepared_runtime_candidate(client, choice.pending, Some(token))
            .await
    }
}

#[cfg(test)]
mod tests;

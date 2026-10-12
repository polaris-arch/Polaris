//! Read-only UI projection of the one measurement ledger. Purpose, instance and URL
//! remain separate; a numeric event is never an independent source of currentness.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::{global, ledger_stale, LedgerEntry, LedgerStale, MeasurementLedger, ReadContext};
use crate::commands::{
    config::config_version,
    speedtest::{CoreInstance, MeasurePath, StaleReason},
};
use crate::response::ApiResponse;
use crate::runtime::{
    measurement_scheduler::{self, entry_freshness_cap_ms, LedgerView},
    AppRuntime,
};

static REVISION: AtomicU64 = AtomicU64::new(0);
static PROCESS: OnceLock<String> = OnceLock::new();

fn process_identity() -> &'static str {
    PROCESS.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn stale_code(reason: LedgerStale) -> &'static str {
    match reason {
        LedgerStale::Identity(StaleReason::Instance) => "instanceChanged",
        LedgerStale::Identity(StaleReason::NodeFingerprint) => "nodeChanged",
        LedgerStale::Identity(StaleReason::NetworkEpoch) => "networkChanged",
        LedgerStale::Expired => "expired",
        LedgerStale::ForegroundEpoch => "foregroundChanged",
    }
}

impl MeasurementLedger {
    /// At most the ledger's existing hard bound plus bounded skip markers. Missing
    /// nodes are unmeasured in the UI, not failed. All record reads hold one lock.
    pub(crate) fn display_rows(
        &self,
        context: &ReadContext<'_>,
        emission_digest: Option<&str>,
    ) -> Value {
        let inner = self.lock();
        let mut latest: BTreeMap<(&str, MeasurePath), &LedgerEntry> = BTreeMap::new();
        for (key, entry) in &inner.entries {
            if !context.fingerprints.contains_key(&key.node_id) {
                continue;
            }
            let slot = (key.node_id.as_str(), key.path);
            let rank = |e: &LedgerEntry| {
                (
                    e.identity.url_digest == context.url_digest,
                    e.identity.run,
                    e.identity.seq,
                )
            };
            if latest
                .get(&slot)
                .is_none_or(|previous| rank(entry) > rank(previous))
            {
                latest.insert(slot, entry);
            }
        }
        let mut rows = serde_json::Map::new();
        for ((id, purpose), entry) in latest {
            let wrong_url = entry.identity.url_digest != context.url_digest;
            let incomplete = entry.identity.node_fingerprint.is_none()
                || matches!(entry.identity.instance, CoreInstance::Main { .. })
                    && emission_digest.is_none();
            let changed_config = matches!(entry.identity.instance, CoreInstance::Main { .. })
                && entry.identity.config_digest.as_deref() != emission_digest;
            let stale = ledger_stale(entry, id, context);
            let state = if wrong_url || incomplete || changed_config || stale.is_some() {
                "expired"
            } else if entry.measured.is_ok() {
                "success"
            } else {
                "failed"
            };
            let reason = if wrong_url {
                Some("urlChanged")
            } else if incomplete {
                Some("identityUnknown")
            } else if changed_config {
                Some("configurationChanged")
            } else {
                stale.map(stale_code)
            };
            let cap = (context.freshness_cap_ms)(id, entry);
            let value = json!({
                "state": state, "reason": reason,
                "latency": entry.measured.ok(), "failure": entry.measured.err().map(|e|e.to_json()),
                "identity": entry.identity.to_json(),
                "disconnected": entry.identity.instance == CoreInstance::Temp,
                "bindingMismatch": entry.identity.binding == Some(crate::commands::speedtest::BindingVerdict::Mismatch),
                "expiresAt": entry.identity.measured_at.saturating_add(cap),
            });
            rows.entry(id.to_owned()).or_insert_with(|| json!({}))[purpose.as_str()] = value;
        }
        for (id, reason) in inner.skipped.iter().take(super::HARD_CAP) {
            if context.fingerprints.contains_key(id) {
                rows.entry(id.clone()).or_insert_with(|| json!({}))["notIncluded"] = json!(reason);
            }
        }
        Value::Object(rows)
    }
}

/// Configuration and runtime are sampled on both sides. A changing input returns
/// no apparently current rows; the window feed will read again on its next event.
pub(crate) fn snapshot(app: &AppHandle) -> ApiResponse<Value> {
    let runtime = app.state::<AppRuntime>();
    let proxy = runtime.proxy();
    let config = match runtime.config().current() {
        Ok(config) => config,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let before = proxy.status();
    let generation = before.running.then(|| proxy.core_generation());
    let network = proxy.network_epoch();
    let ledger = global();
    let foreground = ledger.foreground_epoch();
    let ledger_version = ledger.version();
    let view = LedgerView::new(
        &config,
        generation,
        network,
        polaris_helper_proto::Platform::current(),
    );
    let auto_scope = config["selectionIntent"]["subscriptionId"]
        .as_str()
        .map_or_else(BTreeSet::new, |id| BTreeSet::from([id.to_owned()]));
    let plan = measurement_scheduler::plan_subscriptions(&config, &auto_scope);
    let cap = |id: &str, entry: &LedgerEntry| entry_freshness_cap_ms(&plan, id, entry);
    let now = crate::runtime::subscription_scheduler::now_ms();
    let emission_digest =
        generation.and_then(|generation| proxy.ready_main_emission_digest(generation));
    let rows = ledger.display_rows(&view.read(&cap, ledger, now), emission_digest.as_deref());
    let mut frontend = config.clone();
    crate::commands::config::apply_frontend_view(&mut frontend);
    let frontend_config_digest = polaris_updater::sha256_hex(
        polaris_config_engine::builder::orchestration::stable_stringify(&frontend).as_bytes(),
    );
    let schedule = measurement_scheduler::status(app);
    let after = proxy.status();
    let consistent = runtime
        .config()
        .current()
        .is_ok_and(|value| value == config)
        && before.running == after.running
        && before.start_time == after.start_time
        && generation == after.running.then(|| proxy.core_generation())
        && network == proxy.network_epoch()
        && foreground == ledger.foreground_epoch()
        && emission_digest
            == generation.and_then(|generation| proxy.ready_main_emission_digest(generation))
        && ledger_version == ledger.version();
    ApiResponse::ok(json!({
        "schema": "polaris-measurement-display-v1", "process": process_identity(),
        "revision": REVISION.fetch_add(1, Ordering::AcqRel).to_string(),
        "configVersion": config_version(&config), "consistent": consistent,
        "frontendConfigDigest": frontend_config_digest,
        "generation": generation, "startTime": after.start_time,
        "emissionDigest": emission_digest, "networkEpoch": network, "foregroundEpoch": foreground, "now": now,
        "schedule": schedule, "rows": if consistent { rows } else { json!({}) },
    }))
}

#[cfg(test)]
mod tests;

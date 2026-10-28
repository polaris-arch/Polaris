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
    measurement_scheduler::{
        self, freshness_period_index, indexed_entry_freshness_cap_ms, LedgerView,
    },
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

#[derive(Clone)]
pub(super) struct Skipped {
    pub(super) reason: &'static str,
    pub(super) identity: crate::commands::speedtest::ResultIdentity,
    connection_digest: String,
    foreground_epoch: u64,
}

/// Display IPC has its own digest-only identity. Existing event and subscription
/// identity contracts remain unchanged; their credential-bearing legacy string
/// is never forwarded by this read command.
fn display_identity(entry: &LedgerEntry) -> Value {
    let mut identity = entry.identity.to_json();
    identity
        .as_object_mut()
        .expect("identity object")
        .remove("nodeFingerprint");
    identity["nodeFingerprintDigest"] = json!(entry
        .identity
        .node_fingerprint
        .as_ref()
        .map(|value| polaris_updater::sha256_hex(value.as_bytes())));
    identity["connectionDigest"] = json!(entry.connection_digest);
    identity
}

impl MeasurementLedger {
    pub(crate) fn set_display_skipped(
        &self,
        members: &[String],
        skipped: &[(String, &'static str)],
        identity: crate::commands::speedtest::ResultIdentity,
        connections: &BTreeMap<String, String>,
    ) {
        let mut inner = self.lock();
        for id in members {
            if inner
                .display_skipped
                .get(id)
                .is_some_and(|old| old.identity.run <= identity.run)
            {
                inner.display_skipped.remove(id);
            }
        }
        for (id, reason) in skipped.iter().take(super::HARD_CAP) {
            let Some(digest) = connections.get(id) else {
                continue;
            };
            if inner
                .display_skipped
                .get(id)
                .is_some_and(|old| old.identity.run > identity.run)
                || inner
                    .entries
                    .iter()
                    .any(|(key, entry)| key.node_id == *id && entry.identity.run > identity.run)
            {
                continue;
            }
            inner.display_skipped.insert(
                id.clone(),
                Skipped {
                    reason,
                    identity: identity.clone(),
                    connection_digest: digest.clone(),
                    foreground_epoch: self.foreground_epoch(),
                },
            );
        }
        while inner.display_skipped.len() > super::HARD_CAP {
            let Some(id) = inner
                .display_skipped
                .iter()
                .min_by_key(|(_, value)| value.identity.run)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            inner.display_skipped.remove(&id);
        }
        drop(inner);
        self.bump();
    }

    /// At most the ledger's existing hard bound plus bounded skip markers. Missing
    /// nodes are unmeasured in the UI, not failed. All record reads hold one lock.
    pub(crate) fn display_rows(
        &self,
        context: &ReadContext<'_>,
        emission_digest: Option<&str>,
        connections: &crate::runtime::node_fingerprints::MeasurementConnections,
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
            let incomplete = entry.connection_digest.is_none()
                || entry.identity.node_fingerprint.is_none()
                || matches!(entry.identity.instance, CoreInstance::Main { .. })
                    && emission_digest.is_none();
            let changed_connection = entry.connection_digest.as_ref()
                != connections
                    .for_temp(entry.identity.instance == CoreInstance::Temp)
                    .get(id);
            let changed_config = matches!(entry.identity.instance, CoreInstance::Main { .. })
                && entry.identity.config_digest.as_deref() != emission_digest;
            let stale = ledger_stale(entry, id, context);
            let state = if wrong_url
                || incomplete
                || changed_config
                || changed_connection
                || stale.is_some()
            {
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
            } else if stale.is_some() {
                stale.map(stale_code)
            } else if changed_connection {
                Some("connectionChanged")
            } else {
                None
            };
            let cap = (context.freshness_cap_ms)(id, entry);
            let value = json!({
                "state": state, "reason": reason,
                "latency": entry.measured.ok(), "failure": entry.measured.err().map(|e|e.to_json()),
                "identity": display_identity(entry),
                "disconnected": entry.identity.instance == CoreInstance::Temp,
                "bindingMismatch": entry.identity.binding == Some(crate::commands::speedtest::BindingVerdict::Mismatch),
                "expiresAt": entry.identity.measured_at.saturating_add(cap),
            });
            rows.entry(id.to_owned()).or_insert_with(|| json!({}))[purpose.as_str()] = value;
        }
        for (id, skip) in &inner.display_skipped {
            let valid_instance = match skip.identity.instance {
                CoreInstance::Temp => context.main_generation.is_none(),
                CoreInstance::Main { generation, .. } => {
                    context.main_generation == Some(generation)
                        && skip.identity.config_digest.as_deref() == emission_digest
                        && emission_digest.is_some()
                }
            };
            if connections
                .for_temp(skip.identity.instance == CoreInstance::Temp)
                .get(id)
                == Some(&skip.connection_digest)
                && valid_instance
                && skip.identity.url_digest == context.url_digest
                && skip.identity.network_epoch == context.network_epoch
                && context
                    .foreground_epoch
                    .is_none_or(|epoch| epoch == skip.foreground_epoch)
                && context.now_ms >= skip.identity.measured_at
                && context.now_ms.saturating_sub(skip.identity.measured_at) <= 30 * 60 * 1000
            {
                rows.entry(id.clone()).or_insert_with(|| json!({}))["notIncluded"] =
                    json!(skip.reason);
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
    )
    .with_core_binary_sha256(proxy.measurement_core_sha256());
    let auto_scope = config["selectionIntent"]["subscriptionId"]
        .as_str()
        .map_or_else(BTreeSet::new, |id| BTreeSet::from([id.to_owned()]));
    let plan = measurement_scheduler::plan_subscriptions(&config, &auto_scope);
    let periods = freshness_period_index(&plan);
    let cap = |id: &str, entry: &LedgerEntry| indexed_entry_freshness_cap_ms(&periods, id, entry);
    let now = crate::runtime::subscription_scheduler::now_ms();
    let emission_digest =
        generation.and_then(|generation| proxy.ready_main_emission_digest(generation));
    let connections = crate::runtime::node_fingerprints::measurement_connections_json(&config);
    let mut rows = ledger.display_rows(
        &view.read(&cap, ledger, now),
        emission_digest.as_deref(),
        &connections,
    );
    // One authoritative UI projector; native samples carry distinct provenance.
    if let Some(native) = proxy
        .native_measurement_rows(&config, now, emission_digest.as_deref())
        .as_object()
    {
        for (id, record) in native {
            rows.as_object_mut()
                .expect("display rows object")
                .entry(id.clone())
                .or_insert_with(|| json!({}))["nativeHttp"] = record.clone();
        }
    }
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

//! Native groups own ranking and measurement cadence. This layer only resolves
//! references, attests runtime choices, and transactionally changes wrapper mode.
use super::ProxyRuntime;
use crate::runtime::config::Decision;
use polaris_config_engine::user_config::{
    auto_select::{self as model, AutoSelectPin, AutoSelectTarget},
    UserConfig,
};
use polaris_singbox_grpc::{daemon::Group, Endpoint, ReconnectConfig, SingBoxApiClient};
use polaris_switch_engine::ManagementApi;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAutoGroup {
    pub target: AutoSelectTarget,
    pub actual_server_id: Option<String>,
    pub confirmed: bool,
    pub state: &'static str,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeMeasurement {
    pub server_id: String,
    pub latency_ms: u32,
    pub measured_at: u64,
    pub generation: u64,
    pub url_digest: String,
    pub source: &'static str,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAutoStatus {
    pub feature_enabled: bool,
    /// Action-only receipt. Saved D is never evidence that a queued reload applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_outcome: Option<&'static str>,
    pub generation: u64,
    pub native_auto_groups: Vec<NativeAutoGroup>,
    pub native_measurements: Vec<NativeMeasurement>,
    /// TCP leaf of proxy-selector. No UDP choice is implied by this field.
    pub actual_server_id: Option<String>,
}
#[derive(Default)]
pub(super) struct NativeObservation {
    generation: u64,
    revision: u64,
    groups: Vec<Group>,
}

/// Resolve immediate TCP group selections recursively. Missing groups, cycles,
/// and invented tags are unknown, never a desired-config fallback.
pub(super) fn tcp_leaf<'a>(
    tag: &'a str,
    groups: &'a [Group],
    nodes: &BTreeMap<String, String>,
) -> Option<String> {
    let by_tag: BTreeMap<&str, &Group> = groups
        .iter()
        .map(|group| (group.tag.as_str(), group))
        .collect();
    let mut tag = tag;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(tag) {
            return None;
        }
        if tag == "direct" {
            return Some("__direct__".into());
        }
        if let Some((id, _)) = nodes.iter().find(|(_, node_tag)| node_tag.as_str() == tag) {
            return Some(id.clone());
        }
        let group = by_tag.get(tag)?;
        if group.selected.is_empty() {
            return None;
        }
        // A malformed readback cannot claim a selection outside its member list.
        if !group.items.iter().any(|item| item.tag == group.selected) {
            return None;
        }
        tag = &group.selected;
    }
}

/// Block is a generated reject policy, not a selector member. Both applied R and
/// this generation's startup policy must attest it; desired D is never consulted.
fn runtime_exit_id(
    running: Option<&Value>,
    startup: Option<&Value>,
    groups: &[Group],
    nodes: &BTreeMap<String, String>,
) -> Option<String> {
    let blocked = |value: &Value| {
        value.get("selectionIntent").is_none_or(Value::is_null)
            && value.get("selectedServerId").and_then(Value::as_str) == Some("__block__")
            && value.get("proxyMode").and_then(Value::as_str) != Some("direct")
    };
    if running.is_some_and(blocked) {
        return startup.is_some_and(blocked).then(|| "__block__".into());
    }
    tcp_leaf("proxy-selector", groups, nodes)
}

pub(crate) fn native_owned_members(config: &Value) -> BTreeSet<String> {
    let Ok(config) = serde_json::from_value::<UserConfig>(config.clone()) else {
        return BTreeSet::new();
    };
    if !config.auto_select_enabled {
        return BTreeSet::new();
    }
    model::referenced_targets(&config)
        .iter()
        .flat_map(|target| model::eligible_members(&config, target))
        .collect()
}

/// A verified node ID ceases to be a valid pin when its connection/dependency
/// identity or scope changes. This runs under ConfigManager's writer fence.
pub(crate) fn invalidate_changed_pins(previous: &Value, incoming: &mut Value) {
    let Some(pins) = incoming.get("autoSelectPins").and_then(Value::as_array) else {
        return;
    };
    if pins.is_empty() {
        return;
    }
    let old = crate::runtime::node_fingerprints::measurement_table_json(previous);
    let new = crate::runtime::node_fingerprints::measurement_table_json(incoming);
    let typed = serde_json::from_value::<UserConfig>(incoming.clone()).ok();
    let kept: Vec<Value> = pins
        .iter()
        .filter(|value| {
            let Ok(pin) = serde_json::from_value::<AutoSelectPin>((*value).clone()) else {
                return false;
            };
            old.get(&pin.server_id)
                .is_some_and(|identity| new.get(&pin.server_id) == Some(identity))
                && typed.as_ref().is_some_and(|config| {
                    model::eligible_members(config, &pin.target).contains(&pin.server_id)
                })
        })
        .cloned()
        .collect();
    incoming["autoSelectPins"] = Value::Array(kept);
}

fn project(
    config: &UserConfig,
    generation: u64,
    groups: Option<&[Group]>,
    nodes: &BTreeMap<String, String>,
    clean: &BTreeSet<String>,
    url_digest: &str,
    now: u64,
) -> NativeAutoStatus {
    let referenced = model::referenced_targets(config);
    let entries = referenced
        .iter()
        .map(|target| {
            let members = model::eligible_members(config, target);
            let wrapper = model::wrapper_tag(target);
            let actual = groups
                .and_then(|groups| tcp_leaf(&wrapper, groups, nodes))
                .filter(|id| members.contains(id) && clean.contains(id));
            let expected = if config.auto_select_enabled {
                Some(model::urltest_tag(target))
            } else {
                model::pin(config, target)
                    .and_then(|id| nodes.get(id))
                    .cloned()
            };
            let aligned = groups
                .and_then(|groups| groups.iter().find(|group| group.tag == wrapper))
                .zip(expected.as_deref())
                .is_some_and(|(group, expected)| group.selected == expected);
            let measured = actual
                .as_ref()
                .and_then(|id| nodes.get(id))
                .is_some_and(|tag| {
                    groups
                        .into_iter()
                        .flatten()
                        .flat_map(|group| &group.items)
                        .any(|item| {
                            &item.tag == tag
                                && item.url_test_time > 0
                                && item.url_test_delay >= 0
                                && {
                                    let time = (item.url_test_time as u64).saturating_mul(1000);
                                    now >= time
                                        && now - time
                                            <= model::interval_minutes(config, target) * 60_000
                                }
                        })
                });
            let confirmed =
                actual.is_some() && aligned && (!config.auto_select_enabled || measured);
            NativeAutoGroup {
                target: target.clone(),
                confirmed,
                actual_server_id: actual,
                state: if members.is_empty() {
                    "empty"
                } else if !confirmed {
                    "pending"
                } else if config.auto_select_enabled {
                    "auto"
                } else {
                    "pinned"
                },
            }
        })
        .collect();
    let owned: BTreeSet<String> = if config.auto_select_enabled {
        referenced
            .iter()
            .flat_map(|target| model::eligible_members(config, target))
            .collect()
    } else {
        BTreeSet::new()
    };
    let reverse: BTreeMap<&str, &str> = nodes
        .iter()
        .map(|(id, tag)| (tag.as_str(), id.as_str()))
        .collect();
    let mut measurements: BTreeMap<String, NativeMeasurement> = BTreeMap::new();
    for item in groups
        .into_iter()
        .flatten()
        .flat_map(|group| group.items.iter())
    {
        let Some(id) = reverse.get(item.tag.as_str()).copied() else {
            continue;
        };
        if !owned.contains(id)
            || !clean.contains(id)
            || item.url_test_time <= 0
            || item.url_test_delay < 0
        {
            continue;
        }
        let measured_at = (item.url_test_time as u64).saturating_mul(1000);
        let entry = NativeMeasurement {
            server_id: id.to_owned(),
            latency_ms: item.url_test_delay as u32,
            measured_at,
            generation,
            url_digest: url_digest.into(),
            source: "nativeHttp",
        };
        if measurements
            .get(id)
            .is_none_or(|old| old.measured_at < measured_at)
        {
            measurements.insert(id.to_owned(), entry);
        }
    }
    NativeAutoStatus {
        feature_enabled: config.auto_select_enabled,
        apply_outcome: None,
        generation,
        native_auto_groups: entries,
        native_measurements: measurements.into_values().collect(),
        actual_server_id: groups.and_then(|groups| tcp_leaf("proxy-selector", groups, nodes)),
    }
}

/// All-or-nothing preflight. No PUT or saved flag mutation is possible before
/// every independently referenced group supplies a fresh, eligible TCP receipt.
fn freeze_plan(
    config: &UserConfig,
    status: &NativeAutoStatus,
    groups: &[Group],
    nodes: &BTreeMap<String, String>,
) -> Result<Vec<(AutoSelectPin, String, String)>, &'static str> {
    let mut plan = Vec::new();
    for target in model::referenced_targets(config) {
        let actual = status
            .native_auto_groups
            .iter()
            .find(|group| group.target == target)
            .filter(|group| group.confirmed)
            .and_then(|group| group.actual_server_id.as_ref())
            .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
        if !model::eligible_members(config, &target).contains(actual) {
            return Err("AUTO_SELECT_NO_ACTUAL_EXIT");
        }
        let wrapper = model::wrapper_tag(&target);
        let tag = nodes
            .get(actual)
            .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?
            .clone();
        let group = groups
            .iter()
            .find(|group| group.tag == wrapper)
            .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
        if !group.items.iter().any(|item| item.tag == tag) {
            return Err("AUTO_SELECT_NO_ACTUAL_EXIT");
        }
        plan.push((
            AutoSelectPin {
                target,
                server_id: actual.clone(),
            },
            wrapper,
            tag,
        ));
    }
    Ok(plan)
}

fn updates_attested(groups: &[Group], updates: &[(String, String)]) -> bool {
    updates.iter().all(|(wrapper, member)| {
        groups.iter().any(|group| {
            &group.tag == wrapper
                && &group.selected == member
                && group.items.iter().any(|item| &item.tag == member)
        })
    })
}

impl ProxyRuntime {
    async fn native_read_groups(
        &self,
        api: &crate::runtime::management_api::GrpcManagementApi,
    ) -> Result<Vec<Group>, polaris_switch_engine::ManagementError> {
        #[cfg(test)]
        if let Some(sink) = self
            .management_api_stub
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone))
        {
            if let Some(callback) = sink.on_groups.lock().unwrap().take() {
                callback();
            }
            let groups = sink.native_groups.lock().unwrap().clone();
            return groups.ok_or(polaris_switch_engine::ManagementError::NotReady);
        }
        api.native_groups_snapshot().await
    }

    async fn native_put(
        &self,
        api: &crate::runtime::management_api::GrpcManagementApi,
        wrapper: &str,
        member: &str,
    ) -> Result<(), polaris_switch_engine::ManagementError> {
        #[cfg(test)]
        if let Some(sink) = self
            .management_api_stub
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone))
        {
            return sink.select_outbound(wrapper, member).await;
        }
        api.select_outbound(wrapper, member).await
    }

    pub(crate) fn native_owns_any(&self, ids: &[String]) -> bool {
        self.config
            .with_current(|config| {
                let owned = native_owned_members(config);
                ids.iter().any(|id| owned.contains(id))
            })
            .unwrap_or(false)
    }
    fn native_status_with(
        &self,
        config: &Value,
        generation: u64,
        groups: Option<&[Group]>,
    ) -> NativeAutoStatus {
        let typed = serde_json::from_value::<UserConfig>(config.clone()).unwrap_or_default();
        let snapshot = self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let nodes = snapshot
            .as_ref()
            .map(|snapshot| snapshot.id_to_tag.clone())
            .unwrap_or_default();
        let desired = crate::runtime::node_fingerprints::modified_table_json(config);
        let connections = crate::runtime::node_fingerprints::measurement_table_json(config);
        let clean = snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .fingerprints
                    .iter()
                    .filter(|(id, fingerprint)| {
                        desired.get(*id) == Some(*fingerprint)
                            && connections.get(*id).is_some_and(|expected| {
                                snapshot.measurement_fingerprints.get(*id) == Some(expected)
                            })
                    })
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        // Native history has no URL key. Only the actual running input can name it;
        // if D changed the endpoint, old samples are not exposed as current.
        let running_url = self
            .current_config_snapshot()
            .map(|running| crate::commands::speedtest::resolve_speed_test_url(&running));
        let desired_url = crate::commands::speedtest::resolve_speed_test_url(config);
        let url_matches = crate::commands::speedtest::validate_speed_test_url(config).is_ok()
            && running_url.as_deref() == Some(desired_url.as_str());
        let mut status = project(
            &typed,
            generation,
            groups,
            &nodes,
            &clean,
            &polaris_updater::sha256_hex(desired_url.as_bytes()),
            crate::runtime::subscription_scheduler::now_ms(),
        );
        status.actual_server_id = groups.and_then(|groups| {
            let running = self.current_config_snapshot();
            let startup = self
                .startup_snapshot
                .read()
                .ok()
                .and_then(|snapshot| snapshot.clone());
            runtime_exit_id(running.as_ref(), startup.as_ref(), groups, &nodes)
        });
        if !url_matches {
            status.native_measurements.clear();
            if typed.auto_select_enabled {
                for group in &mut status.native_auto_groups {
                    group.confirmed = false;
                    if group.state != "empty" {
                        group.state = "pending";
                    }
                }
            }
        }
        status
    }

    pub(crate) fn native_auto_cached_status(&self) -> NativeAutoStatus {
        let config = self.config.current().unwrap_or(Value::Null);
        let generation = self.core_generation();
        let observed = self
            .native_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let groups = (self.core_running() && observed.generation == generation)
            .then_some(observed.groups.as_slice());
        self.native_status_with(&config, generation, groups)
    }

    pub(crate) async fn native_auto_status(&self) -> NativeAutoStatus {
        let generation = self.core_generation();
        if self.core_running() {
            let revision = self.native_observation_revision();
            let api = self.management_api().await;
            match self.native_read_groups(&api).await {
                Ok(groups) => {
                    self.publish_native_groups(generation, revision, groups);
                }
                Err(_) => {
                    self.publish_native_groups(generation, revision, Vec::new());
                }
            }
        }
        self.native_auto_cached_status()
    }

    pub(super) fn native_observation_revision(&self) -> u64 {
        self.native_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    /// Compare-and-publish a request begun at `revision`. Relay connections retain
    /// the returned revision; an intervening independent readback retires that
    /// connection, whose queued frames have no server-side revision to order them.
    pub(super) fn publish_native_groups(
        &self,
        generation: u64,
        revision: u64,
        groups: Vec<Group>,
    ) -> Option<u64> {
        let publication = self.gate.lock_generation_publication();
        if !self.core_running() || self.core_generation() != generation {
            return None;
        }
        let before = self.native_actual_server_id();
        let selection_changed = {
            let mut state = self
                .native_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.revision != revision {
                return None;
            }
            let old: BTreeMap<&str, &str> = state
                .groups
                .iter()
                .map(|group| (group.tag.as_str(), group.selected.as_str()))
                .collect();
            let changed = state.generation != generation
                || groups.iter().any(|group| {
                    old.get(group.tag.as_str()).copied() != Some(group.selected.as_str())
                });
            *state = NativeObservation {
                generation,
                revision: revision
                    .checked_add(1)
                    .expect("native observation revision exhausted"),
                groups,
            };
            changed
        };
        drop(publication);
        let status = self.native_auto_cached_status();
        if selection_changed {
            self.invalidate_unlock_cache(true, false);
        }
        if before != status.actual_server_id && status.actual_server_id.is_some() {
            self.schedule_exit_ip_refresh(true);
        }
        self.auto_select_announce();
        Some(revision + 1)
    }

    /// This observes native events only. It never schedules a measurement/evaluation.
    pub(super) fn spawn_native_auto_relay(self: &Arc<Self>, generation: u64, api_port: u16) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            'reconnect: loop {
                if me.core_generation() != generation || !me.core_running() {
                    return;
                }
                let mut revision = me.native_observation_revision();
                if let Ok(client) = SingBoxApiClient::connect(
                    Endpoint::new("127.0.0.1", api_port),
                    me.clash_api_secret(),
                )
                .await
                {
                    if let Ok(mut stream) = client.open_groups().await {
                        loop {
                            let changed = me.gen_changed.notified();
                            if me.core_generation() != generation || !me.core_running() {
                                return;
                            }
                            tokio::select! {
                                _=changed=>{ if me.core_generation()!=generation || !me.core_running() { return; } }
                                frame=stream.message()=>match frame {
                                    Ok(Some(frame))=>match me.publish_native_groups(generation, revision, frame.group) {
                                        Some(next) => revision = next,
                                        // An independent readback won. Reopen for a fresh
                                        // first frame rather than replay this stream's queue.
                                        None => continue 'reconnect,
                                    },
                                    _=>break,
                                }
                            }
                        }
                    }
                }
                let _ = me.publish_native_groups(generation, revision, Vec::new());
                // Transport reconnection backoff, not a measurement/ranking timer.
                tokio::select! {
                    _=me.gen_changed.notified()=>{},
                    _=tokio::time::sleep(std::time::Duration::from_secs(2))=>{},
                }
            }
        });
    }

    async fn restore_native_wrappers(
        &self,
        generation: u64,
        intent: u64,
        previous: &[(String, String)],
    ) {
        let api = self.management_api().await;
        for (wrapper, selected) in previous {
            // Compensation belongs to this operation too. A newer selection or
            // lifecycle owner must never be overwritten by an old rollback.
            if !self.selector_operation_is_current(generation, intent) || self.gate.is_busy() {
                break;
            }
            let _ = self.native_put(&api, wrapper, selected).await;
        }
        self.selector_reconcile.mark_required();
        if self.selector_operation_is_current(generation, intent) && !self.gate.is_busy() {
            let revision = self.native_observation_revision();
            if let Ok(groups) = self.native_read_groups(&api).await {
                self.publish_native_groups(generation, revision, groups);
            }
            self.reconcile_emitted_exit_route(generation).await;
        }
    }

    /// OFF freezes each scope's last verified TCP winner through its outer
    /// selector. This is independent per-scope control, not a simultaneous global
    /// snapshot: native URLTest may advance between observation and PUT. Every
    /// wrapper must read back correctly before OFF is persisted. Failed/partial
    /// operations remain visible; compensation never authorizes a restart.
    pub(crate) async fn native_auto_set_enabled(
        self: &Arc<Self>,
        enabled: bool,
    ) -> Result<NativeAutoStatus, String> {
        let _serial = self.switch_serial.lock().await;
        if self.gate.is_busy() {
            return Err("AUTO_SELECT_BUSY".into());
        }
        let generation = self.core_generation();
        let running = self.core_running();
        let intent_generation = self.selector_reconcile.intent_generation();
        let original = self.config.current().map_err(|error| error.to_string())?;
        let typed: UserConfig =
            serde_json::from_value(original.clone()).map_err(|_| "AUTO_SELECT_INVALID_CONFIG")?;
        let targets = model::referenced_targets(&typed);
        let default_target = polaris_store::auto_select::target(&original);
        let mut frozen_default = None;
        if enabled
            && targets
                .iter()
                .any(|target| model::eligible_members(&typed, target).is_empty())
        {
            return Err("AUTO_SELECT_SCOPE_EMPTY".into());
        }
        if running {
            let running = self
                .current_config_snapshot()
                .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
            let runtime: UserConfig = serde_json::from_value(running.clone())
                .map_err(|_| "AUTO_SELECT_INVALID_CONFIG")?;
            if model::referenced_targets(&runtime) != targets
                || polaris_store::auto_select::target(&running) != default_target
                || (!targets.is_empty() && model::test_url(&runtime) != model::test_url(&typed))
                || targets.iter().any(|target| {
                    model::eligible_members(&runtime, target)
                        != model::eligible_members(&typed, target)
                })
            {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
            let old = crate::runtime::node_fingerprints::measurement_table_json(&running);
            let new = crate::runtime::node_fingerprints::measurement_table_json(&original);
            if targets
                .iter()
                .flat_map(|target| model::eligible_members(&typed, target))
                .any(|id| !old.contains_key(&id) || old.get(&id) != new.get(&id))
            {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
        }
        let mut next = original.clone();
        next["autoSelectEnabled"] = json!(enabled);
        if running && !self.runtime_physical_targets_ready(&next) {
            return Err("AUTO_SELECT_REQUIRES_APPLY".into());
        }
        let mut pins = typed.auto_select_pins.clone();
        let mut previous = Vec::new();
        let mut puts = Vec::new();
        let mut needs_restart = false;
        if !targets.is_empty() && running {
            let api = self.management_api().await;
            let groups = self
                .native_read_groups(&api)
                .await
                .map_err(|_| "AUTO_SELECT_NO_ACTUAL_EXIT")?;
            let status = self.native_status_with(&original, generation, Some(&groups));
            let nodes = self
                .switch_snapshot
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|snapshot| snapshot.id_to_tag.clone()))
                .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
            if !enabled {
                for (pin, wrapper, tag) in freeze_plan(&typed, &status, &groups, &nodes)? {
                    let group = groups
                        .iter()
                        .find(|group| group.tag == wrapper)
                        .expect("preflight group");
                    previous.push((wrapper.clone(), group.selected.clone()));
                    if default_target.as_ref() == Some(&pin.target) {
                        frozen_default = Some(pin.server_id.clone());
                    }
                    pins.retain(|old| old.target != pin.target);
                    pins.push(pin);
                    puts.push((wrapper, tag));
                }
                if let Some(target) = &default_target {
                    let actual = frozen_default
                        .as_ref()
                        .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
                    let tag = nodes.get(actual).ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
                    let proxy = groups
                        .iter()
                        .find(|group| {
                            group.tag == "proxy-selector"
                                && group.selected == model::wrapper_tag(target)
                                && group.items.iter().any(|item| &item.tag == tag)
                        })
                        .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
                    // OFF is a mode change, not a user exit switch: detach the
                    // default from Auto at the same frozen leaf, without closes.
                    // Otherwise enabling a shared rule wrapper would rebind it.
                    previous.push(("proxy-selector".into(), proxy.selected.clone()));
                    puts.push(("proxy-selector".into(), tag.clone()));
                    next.as_object_mut()
                        .ok_or("AUTO_SELECT_INVALID_CONFIG")?
                        .remove(polaris_store::SELECTION_INTENT_KEY);
                    next["selectedServerId"] = json!(actual);
                }
            } else {
                for target in &targets {
                    let wrapper = model::wrapper_tag(target);
                    let Some(group) = groups.iter().find(|group| group.tag == wrapper) else {
                        needs_restart = true;
                        continue;
                    };
                    previous.push((wrapper.clone(), group.selected.clone()));
                    let native = model::urltest_tag(target);
                    if !groups.iter().any(|group| group.tag == native)
                        || !group.items.iter().any(|item| item.tag == native)
                    {
                        needs_restart = true;
                    }
                    puts.push((wrapper, native));
                }
            }
            next["autoSelectPins"] =
                serde_json::to_value(&pins).map_err(|error| error.to_string())?;
            if !self.runtime_physical_targets_ready(&next) {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
            if !self.selector_operation_is_current(generation, intent_generation)
                || self
                    .config
                    .with_current(|latest| latest != &original)
                    .unwrap_or(true)
            {
                return Err("AUTO_SELECT_SWITCH_FAILED".into());
            }
            if !needs_restart {
                for (wrapper, member) in &puts {
                    if !self.selector_operation_is_current(generation, intent_generation)
                        || self
                            .config
                            .with_current(|latest| latest != &original)
                            .unwrap_or(true)
                        || self.native_put(&api, wrapper, member).await.is_err()
                    {
                        self.restore_native_wrappers(generation, intent_generation, &previous)
                            .await;
                        return Err("AUTO_SELECT_SWITCH_FAILED".into());
                    }
                }
                let revision = self.native_observation_revision();
                let groups = self
                    .native_read_groups(&api)
                    .await
                    .map_err(|_| "AUTO_SELECT_SWITCH_FAILED");
                if !self.selector_operation_is_current(generation, intent_generation)
                    || self
                        .config
                        .with_current(|latest| latest != &original)
                        .unwrap_or(true)
                    || !groups
                        .as_ref()
                        .is_ok_and(|groups| updates_attested(groups, &puts))
                {
                    self.restore_native_wrappers(generation, intent_generation, &previous)
                        .await;
                    return Err("AUTO_SELECT_SWITCH_FAILED".into());
                }
                if self
                    .publish_native_groups(generation, revision, groups.unwrap())
                    .is_none()
                {
                    self.restore_native_wrappers(generation, intent_generation, &previous)
                        .await;
                    return Err("AUTO_SELECT_SWITCH_FAILED".into());
                }
            }
        } else if !enabled {
            for target in &targets {
                let Some(pin) = model::pin(&typed, target) else {
                    return Err("AUTO_SELECT_NO_ACTUAL_EXIT".into());
                };
                if !model::eligible_members(&typed, target)
                    .iter()
                    .any(|id| id == pin)
                {
                    return Err("AUTO_SELECT_NO_ACTUAL_EXIT".into());
                }
            }
            if let Some(target) = &default_target {
                let pin = model::pin(&typed, target).ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
                frozen_default = Some(pin.to_owned());
                next.as_object_mut()
                    .ok_or("AUTO_SELECT_INVALID_CONFIG")?
                    .remove(polaris_store::SELECTION_INTENT_KEY);
                next["selectedServerId"] = json!(pin);
            }
        }
        next["autoSelectPins"] = serde_json::to_value(pins).map_err(|error| error.to_string())?;
        // The writer registers its intent before persistence; retain that token
        // even if disk persistence fails so compensation cannot use a newer owner.
        let mut written_intent = None;
        let saved = self.config.update(|latest| {
            if *latest != original
                || self.core_generation() != generation
                || self.core_running() != running
                || self.selector_reconcile.intent_generation() != intent_generation
            {
                return Decision::Skip(None);
            }
            *latest = next.clone();
            let intent = self.register_selector_intent();
            written_intent = Some(intent);
            Decision::Write(Some(intent))
        });
        let new_intent = match saved {
            Ok((Some(intent), _)) => intent,
            _ => {
                if !previous.is_empty() && !needs_restart {
                    self.restore_native_wrappers(
                        generation,
                        written_intent.unwrap_or(intent_generation),
                        &previous,
                    )
                    .await;
                }
                return Err("AUTO_SELECT_SWITCH_FAILED".into());
            }
        };
        let mut apply_outcome = "saved";
        if running {
            let mut projected = self
                .current_config_snapshot()
                .ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
            projected["autoSelectEnabled"] = json!(enabled);
            projected["autoSelectPins"] = next["autoSelectPins"].clone();
            if let Some(actual) = &frozen_default {
                projected
                    .as_object_mut()
                    .ok_or("AUTO_SELECT_INVALID_CONFIG")?
                    .remove(polaris_store::SELECTION_INTENT_KEY);
                projected["selectedServerId"] = json!(actual);
            }
            let outcome = self.commit_native_mode_projection(
                &projected,
                generation,
                new_intent,
                needs_restart,
            );
            if outcome.is_none() {
                // Stop/new lifecycle can arrive after disk persistence but before the
                // runtime claim. Undo only our exact still-current write, never a newer intent.
                let _ = self.config.update(|latest| {
                    if *latest == next && self.selector_reconcile.intent_generation() == new_intent
                    {
                        *latest = original.clone();
                        Decision::Write(())
                    } else {
                        Decision::Skip(())
                    }
                });
                self.restore_native_wrappers(generation, new_intent, &previous)
                    .await;
                return Err("AUTO_SELECT_SWITCH_FAILED".into());
            }
            apply_outcome = match outcome.expect("claimed mode projection") {
                super::SwitchOutcome::Restarting => "restarting",
                super::SwitchOutcome::Pending => "pending",
                super::SwitchOutcome::Deferred => "deferred",
                super::SwitchOutcome::NotRunning => "saved",
                _ => "applied",
            };
            if !needs_restart {
                self.selector_reconcile.clear_required();
                self.reconcile_emitted_exit_route(generation).await;
            }
        }
        if apply_outcome == "applied"
            && (!self.selector_operation_is_current(generation, new_intent)
                || self.current_config_snapshot().is_none_or(|runtime| {
                    runtime.get("autoSelectEnabled").and_then(Value::as_bool) != Some(enabled)
                }))
        {
            apply_outcome = "pending";
        }
        let mut status = self.native_auto_cached_status();
        status.apply_outcome = Some(apply_outcome);
        Ok(status)
    }

    fn with_native_projection_claim(
        &self,
        generation: u64,
        intent: u64,
        apply: impl FnOnce(),
    ) -> bool {
        self.selector_reconcile.with_intent_claim(|current| {
            if current != intent
                || self.core_generation() != generation
                || self.gate.is_busy()
                || !self.core_running()
            {
                return false;
            }
            apply();
            true
        })
    }

    pub(super) fn commit_native_mode_projection(
        self: &Arc<Self>,
        projected: &Value,
        generation: u64,
        intent: u64,
        restart: bool,
    ) -> Option<super::SwitchOutcome> {
        let freezes_default = projected.get("autoSelectEnabled").and_then(Value::as_bool)
            == Some(false)
            && polaris_store::auto_select::target(projected).is_none()
            && self
                .current_config_snapshot()
                .as_ref()
                .is_some_and(|running| polaris_store::auto_select::target(running).is_some());
        let mut existing = None;
        let current = self.with_native_projection_claim(generation, intent, || {
            let mut pending = self
                .pending_force_restart
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            existing = pending.as_mut().map(|(id, config, source)| {
                if matches!(source, super::ForceRestartSource::Confirmed { .. }) {
                    return *id;
                }
                config["autoSelectEnabled"] = projected["autoSelectEnabled"].clone();
                config["autoSelectPins"] = projected["autoSelectPins"].clone();
                // OFF also makes the default concrete. Do not let a queued,
                // unconfirmed projection resurrect its removed Auto reference.
                if freezes_default {
                    super::hot_switch::copy_selection_projection(config, projected);
                }
                if matches!(source, super::ForceRestartSource::Selected { .. }) {
                    *source = super::ForceRestartSource::Selected {
                        intent_generation: intent,
                    };
                }
                *id
            });
            drop(pending);
            if !restart {
                self.commit_coex_current_config(projected.clone());
            }
        });
        if !current || (!restart && self.current_config_snapshot().as_ref() != Some(projected)) {
            return None;
        }
        if restart {
            // Reusing an old queued identifier is not authorization for this
            // new Auto projection to restart the core.
            let _ = existing;
            return Some(self.defer_selected_projection(projected, generation, intent));
        }
        Some(super::SwitchOutcome::HotSwitched)
    }

    pub(crate) async fn native_auto_select_default(
        self: &Arc<Self>,
        target: AutoSelectTarget,
    ) -> Result<NativeAutoStatus, String> {
        use polaris_config_engine::builder::hotswitch::{
            HotSwitchKind, HotSwitchPlan, HotSwitchPut,
        };
        use polaris_switch_engine::HotSwitchOutcome;
        let serial = self.switch_serial.lock().await;
        if self.gate.is_busy() {
            return Err("AUTO_SELECT_BUSY".into());
        }
        let saved = self.config.current().map_err(|error| error.to_string())?;
        let started_generation = self.core_generation();
        let started_running = self.core_running();
        let started_intent = self.selector_reconcile.intent_generation();
        let typed: UserConfig =
            serde_json::from_value(saved.clone()).map_err(|_| "AUTO_SELECT_INVALID_CONFIG")?;
        if !typed.auto_select_enabled {
            return Err("AUTO_SELECT_SWITCHED_OFF".into());
        }
        if !target.valid() || model::eligible_members(&typed, &target).is_empty() {
            return Err("AUTO_SELECT_SCOPE_EMPTY".into());
        }
        let mut projected = self.current_config_snapshot();
        // A user selecting Auto from a concrete node changes the logical exit,
        // even when URLTest currently resolves to that same physical node.
        // Keep Auto feature ON/OFF and Auto-to-Auto scope changes unchanged.
        let interrupt_manual_exit = typed.interrupt_connections_on_switch == Some(true)
            && projected
                .as_ref()
                .is_some_and(|running| polaris_store::auto_select::target(running).is_none());
        if self.core_running() {
            let runtime: UserConfig =
                serde_json::from_value(projected.clone().ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?)
                    .map_err(|_| "AUTO_SELECT_INVALID_CONFIG")?;
            if !runtime.auto_select_enabled {
                return Err("AUTO_SELECT_BUSY".into());
            }
            let requested = model::eligible_members(&typed, &target);
            if model::eligible_members(&runtime, &target) != requested {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
            let fingerprints = crate::runtime::node_fingerprints::modified_table_json(&saved);
            let connections = crate::runtime::node_fingerprints::measurement_table_json(&saved);
            let clean = self.switch_snapshot.read().ok().is_some_and(|guard| {
                guard.as_ref().is_some_and(|snapshot| {
                    requested.iter().all(|id| {
                        snapshot.fingerprints.get(id) == fingerprints.get(id)
                            && snapshot.id_to_tag.contains_key(id)
                            && connections.get(id).is_some_and(|expected| {
                                snapshot.measurement_fingerprints.get(id) == Some(expected)
                            })
                    })
                })
            });
            if !clean {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
        }
        let seed = if self.core_running() {
            self.native_auto_status()
                .await
                .actual_server_id
                .filter(|id| model::eligible_members(&typed, &target).contains(id))
        } else {
            None
        };
        let intent = polaris_store::auto_select::intent(&target);
        if self.core_running() {
            let mut target_projection = projected.clone().ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
            target_projection[polaris_store::SELECTION_INTENT_KEY] = intent.clone();
            target_projection["autoSelectEnabled"] = json!(true);
            if !self.runtime_physical_targets_ready(&target_projection) {
                return Err("AUTO_SELECT_REQUIRES_APPLY".into());
            }
        }
        let (generation, _) = self
            .config
            .update(|latest| {
                if *latest != saved
                    || self.core_generation() != started_generation
                    || self.core_running() != started_running
                    || self.selector_reconcile.intent_generation() != started_intent
                {
                    return Decision::Skip(None);
                }
                latest[polaris_store::SELECTION_INTENT_KEY] = intent.clone();
                if let Some(seed) = &seed {
                    latest["selectedServerId"] = json!(seed);
                }
                Decision::Write(Some(self.register_selector_intent()))
            })
            .map_err(|error| error.to_string())?;
        let generation = generation.ok_or("AUTO_SELECT_SWITCH_FAILED")?;
        if self.core_running() {
            let projected = projected.as_mut().ok_or("AUTO_SELECT_NO_ACTUAL_EXIT")?;
            projected[polaris_store::SELECTION_INTENT_KEY] = intent;
            if let Some(seed) = &seed {
                projected["selectedServerId"] = json!(seed);
            }
            projected["autoSelectEnabled"] = json!(true);
            if projected.get("selectedServerId").is_none() {
                projected["selectedServerId"] = Value::Null;
            }
            let core_generation = started_generation;
            if !self.selector_operation_is_current(core_generation, generation) {
                return Err("AUTO_SELECT_SWITCH_FAILED".into());
            }
            let wrapper = model::wrapper_tag(&target);
            let api = self.management_api().await;
            let native = model::urltest_tag(&target);
            let initial_groups = self.native_read_groups(&api).await;
            let present = initial_groups.as_ref().is_ok_and(|groups| {
                groups.iter().any(|group| {
                    group.tag == wrapper && group.items.iter().any(|item| item.tag == native)
                }) && groups.iter().any(|group| group.tag == native)
                    && groups.iter().any(|group| {
                        group.tag == "proxy-selector"
                            && group.items.iter().any(|item| item.tag == wrapper)
                    })
            });
            if present && self.selector_operation_is_current(core_generation, generation) {
                let aligned = initial_groups.as_ref().is_ok_and(|groups| {
                    updates_attested(
                        groups,
                        &[
                            (wrapper.clone(), native.clone()),
                            ("proxy-selector".into(), wrapper.clone()),
                        ],
                    )
                });
                let updates = vec![
                    (wrapper.clone(), native),
                    ("proxy-selector".into(), wrapper),
                ];
                let previous: Vec<_> = initial_groups
                    .as_ref()
                    .unwrap()
                    .iter()
                    .filter(|group| updates.iter().any(|(tag, _)| tag == &group.tag))
                    .map(|group| (group.tag.clone(), group.selected.clone()))
                    .collect();
                let mut pending_disconnect = None;
                if !aligned {
                    let plan = HotSwitchPlan {
                        kind: HotSwitchKind::Global,
                        must_restart: false,
                        puts: updates
                            .iter()
                            .map(|(tag, member)| HotSwitchPut {
                                selector_tag: tag.clone(),
                                member_tag: member.clone(),
                                // Only the default logical exit is being replaced.
                                // Never acquire the independent rule wrapper's scope.
                                old_member_tag: if tag == "proxy-selector" {
                                    previous
                                        .iter()
                                        .find(|(old, _)| old == tag)
                                        .map(|(_, member)| member.clone())
                                } else {
                                    None
                                },
                            })
                            .collect(),
                    };
                    let (outcome, captured) = self
                        .execute_hot_switch_plan_with_pre_switch_snapshot(
                            &plan,
                            interrupt_manual_exit,
                            core_generation,
                            generation,
                        )
                        .await;
                    if !matches!(outcome, HotSwitchOutcome::Applied { .. }) {
                        self.restore_native_wrappers(core_generation, generation, &previous)
                            .await;
                        return Err("AUTO_SELECT_SWITCH_FAILED".into());
                    }
                    pending_disconnect = Some(captured);
                }
                let revision = self.native_observation_revision();
                let groups = self.native_read_groups(&api).await;
                if !self.selector_operation_is_current(core_generation, generation)
                    || !groups
                        .as_ref()
                        .is_ok_and(|groups| updates_attested(groups, &updates))
                {
                    self.restore_native_wrappers(core_generation, generation, &previous)
                        .await;
                    return Err("AUTO_SELECT_SWITCH_FAILED".into());
                }
                if self
                    .publish_native_groups(core_generation, revision, groups.unwrap())
                    .is_none()
                {
                    self.restore_native_wrappers(core_generation, generation, &previous)
                        .await;
                    return Err("AUTO_SELECT_SWITCH_FAILED".into());
                }
                if !self.with_native_projection_claim(core_generation, generation, || {
                    self.commit_selected_projection(projected, generation)
                }) || self.current_config_snapshot().as_ref() != Some(&*projected)
                {
                    self.restore_native_wrappers(core_generation, generation, &previous)
                        .await;
                    return Err("AUTO_SELECT_SWITCH_FAILED".into());
                }
                if let Some(captured) = pending_disconnect {
                    self.finish_logical_exit_disconnect(captured, core_generation, generation)
                        .await;
                }
                self.reconcile_emitted_exit_route(core_generation).await;
            } else {
                // New reference adds native outbounds. Keep it pending for explicit
                // graph Apply; never consume unrelated saved D or restart implicitly.
                self.defer_selected_projection(projected, core_generation, generation);
            }
            let _ = &serial;
        }
        Ok(self.native_auto_status().await)
    }
    /// Pin a clean existing node through the existing hot-switch executor.
    /// Auto and a concrete node are different logical exits, including equal TCP leaves.
    pub(super) async fn native_manual_exit(
        self: &Arc<Self>,
        projected: &Value,
        server_id: &str,
        intent: u64,
    ) -> Option<super::hot_switch::SwitchOutcome> {
        use super::hot_switch::SwitchOutcome;
        use polaris_config_engine::builder::{
            hotswitch::{HotSwitchKind, HotSwitchPlan, HotSwitchPut},
            mesh_mode::{selected_mode, NORMAL},
        };
        use polaris_switch_engine::HotSwitchOutcome;
        let running = self.current_config_snapshot()?;
        polaris_store::auto_select::target(&running)?;
        let typed: UserConfig = serde_json::from_value(projected.clone()).ok()?;
        // Mode transitions and split-only mesh exits retain their existing planner.
        if selected_mode(&typed) != NORMAL {
            return None;
        }
        if !self.runtime_physical_targets_ready(projected) {
            return Some(SwitchOutcome::Deferred);
        }
        let generation = self.core_generation();
        let api = self.management_api().await;
        let groups = self.native_read_groups(&api).await.ok()?;
        let nodes = self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|snapshot| snapshot.id_to_tag.clone()))?;
        tcp_leaf("proxy-selector", &groups, &nodes)?;
        let tag = nodes.get(server_id)?;
        let proxy = groups.iter().find(|group| {
            group.tag == "proxy-selector" && group.items.iter().any(|item| &item.tag == tag)
        })?;
        let plan = HotSwitchPlan {
            kind: HotSwitchKind::Global,
            must_restart: false,
            puts: vec![HotSwitchPut {
                selector_tag: "proxy-selector".into(),
                member_tag: tag.clone(),
                old_member_tag: Some(proxy.selected.clone()),
            }],
        };
        let (outcome, captured) = self
            .execute_hot_switch_plan_with_pre_switch_snapshot(
                &plan,
                typed.interrupt_connections_on_switch == Some(true),
                generation,
                intent,
            )
            .await;
        if !matches!(outcome, HotSwitchOutcome::Applied { .. }) {
            self.selector_reconcile.mark_required();
            self.reconcile_emitted_exit_route(generation).await;
            return Some(SwitchOutcome::Pending);
        }
        let revision = self.native_observation_revision();
        let groups = self.native_read_groups(&api).await;
        if !self.selector_operation_is_current(generation, intent)
            || !groups.as_ref().is_ok_and(|groups| {
                updates_attested(groups, &[("proxy-selector".into(), tag.clone())])
            })
        {
            self.restore_native_wrappers(
                generation,
                intent,
                &[("proxy-selector".into(), proxy.selected.clone())],
            )
            .await;
            return Some(SwitchOutcome::Pending);
        }
        if self
            .publish_native_groups(generation, revision, groups.unwrap())
            .is_none()
            || !self.with_selected_projection_claim(projected, generation, intent, || {
                self.commit_selected_projection(projected, intent)
            })
            || self.current_config_snapshot().as_ref() != Some(projected)
        {
            self.restore_native_wrappers(
                generation,
                intent,
                &[("proxy-selector".into(), proxy.selected.clone())],
            )
            .await;
            return Some(SwitchOutcome::Pending);
        }
        self.finish_logical_exit_disconnect(captured, generation, intent)
            .await;
        self.reconcile_emitted_exit_route(generation).await;
        Some(SwitchOutcome::HotSwitched)
    }

    /// Existing daemon URLTest is asynchronous: acknowledgement is not a winner
    /// receipt. Return the observed status; subsequent SubscribeGroups frames carry
    /// completed measurements and any native winner change.
    pub(crate) async fn native_auto_retest(&self) -> Result<NativeAutoStatus, String> {
        let config = self.config.current().map_err(|error| error.to_string())?;
        let typed: UserConfig =
            serde_json::from_value(config).map_err(|_| "AUTO_SELECT_INVALID_CONFIG")?;
        if !typed.auto_select_enabled {
            return Err("AUTO_SELECT_SWITCHED_OFF".into());
        }
        if !self.core_running() {
            return Err("AUTO_SELECT_NO_ACTUAL_EXIT".into());
        }
        let generation = self.core_generation();
        let api = self.management_api().await;
        for target in model::referenced_targets(&typed) {
            if self.core_generation() != generation || !self.core_running() {
                return Err("AUTO_SELECT_SWITCH_FAILED".into());
            }
            api.url_test(&model::urltest_tag(&target))
                .await
                .map_err(|_| "AUTO_SELECT_SWITCH_FAILED")?;
        }
        Ok(self.native_auto_status().await)
    }

    /// Lightweight TCP truth used by frequent proxy status reads.
    pub(crate) fn native_actual_server_id(&self) -> Option<String> {
        if !self.core_running() {
            return None;
        }
        self.native_actual_server_id_memory(self.core_generation())
    }

    /// Memory-only projection for a captured status generation. In particular,
    /// do not consult Android's native bridge while saved/intent guards are held.
    pub(super) fn native_actual_server_id_memory(&self, generation: u64) -> Option<String> {
        if self.core_generation() != generation
            || !self
                .status
                .read()
                .map(|status| status.running)
                .unwrap_or(false)
        {
            return None;
        }
        let state = self
            .native_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != generation {
            return None;
        }
        let snapshot = self.switch_snapshot.read().ok()?;
        let running = self.current_config_snapshot();
        let startup = self
            .startup_snapshot
            .read()
            .ok()
            .and_then(|snapshot| snapshot.clone());
        runtime_exit_id(
            running.as_ref(),
            startup.as_ref(),
            &state.groups,
            &snapshot.as_ref()?.id_to_tag,
        )
    }

    /// Native history is projected through the same display IPC as App HTTP
    /// records. It retains its own metric and never enters the App ranking ledger.
    pub(crate) fn native_measurement_rows(
        &self,
        config: &Value,
        now: u64,
        emission_digest: Option<&str>,
    ) -> Value {
        let Ok(typed) = serde_json::from_value::<UserConfig>(config.clone()) else {
            return json!({});
        };
        if !self.core_running() || !typed.auto_select_enabled {
            return json!({});
        }
        let status = self.native_auto_cached_status();
        let owned = native_owned_members(config);
        let fingerprints = crate::commands::speedtest::current_server_fingerprints(config);
        let connections = crate::runtime::node_fingerprints::measurement_connections_json(config);
        let mut caps: BTreeMap<String, u64> = BTreeMap::new();
        for target in model::referenced_targets(&typed) {
            let cap = model::interval_minutes(&typed, &target) * 60_000;
            for id in model::eligible_members(&typed, &target) {
                caps.entry(id)
                    .and_modify(|old| *old = (*old).min(cap))
                    .or_insert(cap);
            }
        }
        let samples: BTreeMap<&str, &NativeMeasurement> = status
            .native_measurements
            .iter()
            .map(|sample| (sample.server_id.as_str(), sample))
            .collect();
        let mut rows = serde_json::Map::new();
        for id in owned {
            let cap = caps.get(&id).copied().unwrap_or(30 * 60_000);
            let sample = samples.get(id.as_str()).copied();
            let measured_at = sample.map_or(0, |sample| sample.measured_at);
            let expires_at = measured_at.saturating_add(cap);
            let current = sample.is_some()
                && emission_digest.is_some()
                && now >= measured_at
                && now <= expires_at;
            rows.insert(id.clone(),json!({
                "state": if current {"success"} else {"expired"},
                "reason": if sample.is_none() {Some("identityUnknown")} else if !current {Some("expired")} else {None},
                "latency":sample.map(|sample|sample.latency_ms), "failure":null,
                "disconnected":false,"bindingMismatch":false,"expiresAt":expires_at,
                "identity":{
                    "run":measured_at.to_string(),"seq":0,"origin":"nativeAuto","scope":null,
                    "path":"nativeHttp","metric":"native_http_v1","context":"connected",
                    "instance":{"kind":"main","generation":status.generation,"startTime":self.status.read().ok().and_then(|status|status.start_time)},
                    "urlDigest":polaris_updater::sha256_hex(model::test_url(&typed).as_bytes()),
                    "configDigest":emission_digest,"coreBinarySha256":self.measurement_core_sha256(),
                    "nodeFingerprintDigest":fingerprints.get(&id).map(|value|polaris_updater::sha256_hex(value.as_bytes())),
                    "connectionDigest":connections.for_temp(false).get(&id),
                    "networkEpoch":null,"measuredAt":measured_at
                }
            }));
        }
        Value::Object(rows)
    }

    /// Project independent measurement frames against current desired identity
    /// and ownership. Never publish these frames into global actual-exit state.
    pub(super) fn native_measure_frame_status(
        &self,
        generation: u64,
        expected_url: &str,
        groups: &[Group],
    ) -> Option<NativeAutoStatus> {
        let latest = self.config.current().ok()?;
        if self.core_generation() != generation
            || !self.core_running()
            || !polaris_store::auto_select::enabled(&latest)
            || crate::commands::speedtest::resolve_speed_test_url(&latest) != expected_url
        {
            return None;
        }
        Some(self.native_status_with(&latest, generation, Some(groups)))
    }

    /// Reuse completed native HTTP history. Missing/expired samples ask the native
    /// owner for one round and await its events, never launch competing App probes.
    /// A timeout/failed test leaves a node absent; it is not a fabricated -1 result.
    pub(crate) async fn native_measure_requested(
        &self,
        requested: &[String],
        cancel: &crate::commands::speedtest::SpeedTestCancel,
    ) -> BTreeMap<String, u32> {
        if cancel.reason().is_some() {
            return BTreeMap::new();
        }
        let Ok(raw) = self.config.current() else {
            return BTreeMap::new();
        };
        let Ok(config) = serde_json::from_value::<UserConfig>(raw.clone()) else {
            return BTreeMap::new();
        };
        let generation = self.core_generation();
        if !config.auto_select_enabled
            || self.current_config_snapshot().is_none_or(|running| {
                crate::commands::speedtest::resolve_speed_test_url(&running)
                    != model::test_url(&config)
            })
        {
            return BTreeMap::new();
        }
        let wanted: BTreeSet<String> = requested.iter().cloned().collect();
        let mut groups: Vec<_> = model::referenced_targets(&config)
            .into_iter()
            .map(|target| {
                let members: BTreeSet<String> = model::eligible_members(&config, &target)
                    .into_iter()
                    .collect();
                (target, members)
            })
            .collect();
        groups.sort_by_key(|(_, members)| members.len());
        let mut cap: BTreeMap<String, u64> = BTreeMap::new();
        for (target, members) in &groups {
            let ttl = model::interval_minutes(&config, target) * 60_000;
            for id in members {
                cap.entry(id.clone())
                    .and_modify(|value| *value = (*value).min(ttl))
                    .or_insert(ttl);
            }
        }
        let read = |status: NativeAutoStatus| -> BTreeMap<String, u32> {
            let now = crate::runtime::subscription_scheduler::now_ms();
            status
                .native_measurements
                .into_iter()
                .filter(|sample| {
                    wanted.contains(&sample.server_id)
                        && sample.generation == generation
                        && now >= sample.measured_at
                        && now - sample.measured_at
                            <= cap.get(&sample.server_id).copied().unwrap_or(0)
                })
                .map(|sample| (sample.server_id, sample.latency_ms))
                .collect()
        };
        let status = self.native_auto_status().await;
        if cancel.reason().is_some() || self.core_generation() != generation {
            return BTreeMap::new();
        }
        let mut results = read(status);
        if results.len() == wanted.len() || !self.core_running() {
            return results;
        }
        if cancel.reason().is_some() {
            return BTreeMap::new();
        }
        let Some((port, secret)) = self.management_endpoint() else {
            return results;
        };
        let connected = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await;
        if cancel.reason().is_some() {
            return BTreeMap::new();
        }
        let Ok(client) = connected else {
            return results;
        };
        let mut stream = client.subscribe_groups(ReconnectConfig::default());
        let mut scheduled: BTreeSet<String> = results.keys().cloned().collect();
        for (target, members) in groups {
            if cancel.reason().is_some() {
                return BTreeMap::new();
            }
            if members
                .iter()
                .any(|id| wanted.contains(id) && !scheduled.contains(id))
            {
                if self.core_generation() != generation
                    || !self.core_running()
                    || self
                        .config
                        .with_current(|latest| {
                            !polaris_store::auto_select::enabled(latest)
                                || crate::commands::speedtest::resolve_speed_test_url(latest)
                                    != model::test_url(&config)
                        })
                        .unwrap_or(true)
                {
                    return BTreeMap::new();
                }
                if cancel.reason().is_some() {
                    return BTreeMap::new();
                }
                let measured = client.url_test(model::urltest_tag(&target)).await;
                if cancel.reason().is_some() {
                    return BTreeMap::new();
                }
                if measured.is_ok() {
                    scheduled.extend(members);
                }
            }
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while results.len() < wanted.len() {
            tokio::select! {
                _=cancel.cancelled()=>break,
                _=tokio::time::sleep_until(deadline)=>break,
                frame=stream.recv()=>{
                    let Some(frame)=frame else { break; };
                    // Fresh desired identity/ownership is required after every
                    // await; the request's original config is not frame authority.
                    let Some(status) = self.native_measure_frame_status(
                        generation, model::test_url(&config), &frame.group,
                    ) else { return BTreeMap::new(); };
                    results=read(status);
                }
            }
        }
        if cancel.reason().is_some() {
            return BTreeMap::new();
        }
        results
    }
}

#[cfg(test)]
mod tests;

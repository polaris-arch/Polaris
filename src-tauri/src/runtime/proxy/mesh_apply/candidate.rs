//! A sealed generation candidate for the first conservative desktop
//! direct/VLESS subset. Port allocation probes and live read-only facts are
//! captured, but no shared config, old core, or sidecar is mutated. The old
//! core's persistent cache is never an immutable artifact or birth permit.

use super::materialize::{materialize_local_rule_sets, MaterializedEmission};
use crate::runtime::config::ApplyInputSnapshot;
use crate::runtime::proxy::mesh_apply::plan_digest;
use crate::runtime::proxy::ProxyRuntime;
use polaris_config_engine::builder::endpoint_routes::MeshRouteEmissionCandidate;
use polaris_config_engine::builder::generate::{
    generate_sing_box_config_with_report_and_runtime_bindings, GenerateConfigDeps, InvalidNode,
};
use polaris_config_engine::builder::managed_mesh_plan::{
    ManagedMeshPlanInput, ManagedMeshRoutePlan,
};
use polaris_config_engine::builder::network_env::{NetworkCanaryPlan, PrunedEnvRule};
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::proxy_mode::{ProxyMode, ProxyModeType};
use polaris_config_engine::user_config::server_config::Protocol;
use polaris_config_engine::user_config::UserConfig;
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateError {
    SnapshotChanged,
    SnapshotMismatch,
    Unsupported,
    Builder,
    Materialize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedCachePolicy {
    /// Bundled b609 (1.15.0-alpha.8) keeps implicit selector persistence:
    /// `LoadSelected` may override config `default`, and `StoreSelected` may
    /// write the runtime choice to this DB. No override is emitted. A config
    /// hash does not attest the current selection; birth needs exclusive DB
    /// writer handoff and runtime selector readback.
    ImplicitCoreDefaultPreserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheOwnership {
    /// The DB belongs to the existing application data directory and can be
    /// mutated by the old core. Candidate generation makes no ownership claim.
    PersistentMutablePreserve,
}

/// This is a reference to the existing persistent, mutable cache. Never hash,
/// copy, delete, create, or open it during candidate generation.
struct PersistentMutableCache {
    ownership: CacheOwnership,
    path: PathBuf,
    enabled: bool,
    cache_id: Option<String>,
    store_fakeip: Option<bool>,
    store_dns: Option<bool>,
    selected_policy: SelectedCachePolicy,
}

/// Captured before the one full-builder call. Contains loopback credentials
/// where applicable, so it deliberately implements neither Debug nor Serialize.
pub(crate) struct EffectiveCandidateFacts {
    raw_document_sha256: String,
    plan_digest: String,
    config_version: String,
    input_state_revision: String,
    deps: GenerateConfigDeps,
    runtime_bind_interfaces: BTreeMap<String, String>,
    cache: PersistentMutableCache,
    source_config_sha256: String,
    final_config_sha256: String,
    immutable_rule_sha256: BTreeMap<String, String>,
    mesh_route_candidates: Vec<MeshRouteEmissionCandidate>,
    mesh_route_total_candidate_count: usize,
    mesh_route_diagnostics_limited: bool,
    mesh_route_dns_owner_server_id: Option<String>,
    invalid_nodes: Vec<InvalidNode>,
    pruned_rule_set_tags: Vec<String>,
    pruned_env_rules: Vec<PrunedEnvRule>,
    network_canary: Option<NetworkCanaryPlan>,
}

/// Its config and rule bytes came from the same full-builder invocation and
/// D1 materialization. No `sing-box check` or manifest publication occurs here.
pub(crate) struct SealedCandidate {
    materialized: MaterializedEmission,
    facts: EffectiveCandidateFacts,
}

mod check;

thread_local! {
    static BUILDER_CALLED_UNSUPPORTED_DEPENDENCY: Cell<bool> = const { Cell::new(false) };
}

fn reject_dependency() {
    BUILDER_CALLED_UNSUPPORTED_DEPENDENCY.with(|called| called.set(true));
}

fn no_candidate_log(level: polaris_config_engine::user_config::LogLevel, message: &str) {
    let _ = message;
    if matches!(
        level,
        polaris_config_engine::user_config::LogLevel::Warn
            | polaris_config_engine::user_config::LogLevel::Error
            | polaris_config_engine::user_config::LogLevel::Fatal
    ) {
        reject_dependency();
    }
}

fn no_candidate_degraded() {
    reject_dependency();
}

fn no_candidate_srs_probe(_: &str) -> bool {
    reject_dependency();
    false
}

fn conservative_user_input(config: &UserConfig) -> bool {
    config.proxy_mode == ProxyMode::Direct
        && config.proxy_mode_type == ProxyModeType::SystemProxy
        && config.servers.len() == 1
        && config.servers[0].protocol == Protocol::Vless
        && config.selected_server_id.as_deref() == Some(config.servers[0].id.as_str())
        && config.servers[0].detour.is_none()
        && config.servers[0].custom_settings.is_none()
        && config.servers[0].bind_interface.is_none()
        && config.subscriptions.is_empty()
        && config.network_interfaces.is_none()
        && config.custom_rules.is_empty()
        && config.route_rule_order.is_empty()
        && config.dns_rule_order.is_empty()
        && config
            .policy_rules
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        && config
            .traffic_rules
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        && config.dns_rules.as_deref().unwrap_or_default().is_empty()
        && config.app_rules.is_empty()
        && config.custom_app_presets.is_empty()
        && config.rule_resources.is_empty()
        && config.network_profiles.is_empty()
        && config.region_routing.is_none()
        && config.dns_config.is_none()
        && config.dns_servers == UserConfig::default().dns_servers
        && config.dns_server_groups.is_empty()
        && config.dns_defaults.is_none()
        && config.route_defaults.is_none()
        && config.singbox_dashboard != Some(true)
}

fn conservative_raw_server(raw: &Value) -> bool {
    let Some(server) = raw
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| (servers.len() == 1).then(|| &servers[0]))
        .and_then(Value::as_object)
    else {
        return false;
    };
    // `ServerConfig` is intentionally tolerant of older and future fields.
    // Do not let an unmodelled file path or protocol escape hatch silently
    // disappear during deserialize and thereby enter this narrower contract.
    if !server.keys().all(|key| {
        matches!(
            key.as_str(),
            "id" | "name"
                | "protocol"
                | "address"
                | "port"
                | "uuid"
                | "encryption"
                | "flow"
                | "packetEncoding"
                | "network"
                | "security"
                | "tlsSettings"
                | "realitySettings"
                | "createdAt"
                | "updatedAt"
        )
    }) || !matches!(
        server.get("network").and_then(Value::as_str),
        None | Some("tcp")
    ) {
        return false;
    }
    let tls_keys = [
        "serverName",
        "allowInsecure",
        "alpn",
        "fingerprint",
        "ech",
        "echConfig",
        "fragment",
        "engine",
        "spoofSni",
        "spoofMethod",
        "certificateSha256",
        "certificatePublicKeySha256",
    ];
    let reality_keys = ["publicKey", "shortId"];
    let known_settings = [
        ("tlsSettings", tls_keys.as_slice()),
        ("realitySettings", reality_keys.as_slice()),
    ]
    .into_iter()
    .all(|(field, allowed)| {
        server.get(field).is_none_or(|value| {
            value
                .as_object()
                .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
        })
    });
    known_settings
}

fn resources_are_supported(
    config: &SingBoxConfig,
    cache_path: &Path,
) -> Result<PersistentMutableCache, CandidateError> {
    if config.log.output.is_some()
        || config
            .endpoints
            .as_deref()
            .is_some_and(|endpoints| !endpoints.is_empty())
        || config
            .services
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|service| service.type_field != "api" || service.dashboard.is_some())
        || config
            .inbounds
            .iter()
            .any(|inbound| !matches!(inbound.type_field.as_str(), "mixed" | "socks" | "http"))
        || config
            .route
            .as_ref()
            .and_then(|route| route.rule_set.as_ref())
            .is_some_and(|rules| !rules.is_empty())
        || config.outbounds.iter().any(|outbound| {
            !matches!(
                outbound.type_field.as_str(),
                "direct" | "block" | "selector" | "vless"
            ) || !outbound.extra.is_empty()
                || outbound.path.is_some()
                || outbound.transport.is_some()
                || outbound.private_key_path.is_some()
                || outbound.executable_path.is_some()
                || outbound.data_directory.is_some()
        })
        || config.dns.as_ref().is_some_and(|dns| {
            dns.servers.iter().any(|server| {
                !matches!(
                    server.type_field.as_deref(),
                    Some("https" | "local" | "fakeip" | "mdns" | "udp")
                ) || (server.path.is_some() && server.type_field.as_deref() != Some("https"))
                    || server.endpoint.is_some()
            }) || dns
                .rules
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|rule| rule.rule_set.is_some())
        })
    {
        return Err(CandidateError::Unsupported);
    }
    let cache = config
        .experimental
        .as_ref()
        .and_then(|experimental| experimental.cache_file.as_ref())
        .ok_or(CandidateError::Unsupported)?;
    if !cache.enabled
        || !cache_path.is_absolute()
        || cache_path.to_str().is_none_or(|path| cache.path != path)
        || cache.cache_id.as_deref() != Some("polaris-dns-v2")
        || cache.store_fakeip != Some(true)
        || cache.store_dns != Some(true)
    {
        return Err(CandidateError::Unsupported);
    }
    Ok(PersistentMutableCache {
        ownership: CacheOwnership::PersistentMutablePreserve,
        path: cache_path.to_path_buf(),
        enabled: cache.enabled,
        cache_id: cache.cache_id.clone(),
        store_fakeip: cache.store_fakeip,
        store_dns: cache.store_dns,
        selected_policy: SelectedCachePolicy::ImplicitCoreDefaultPreserve,
    })
}

/// Generate one conservative candidate without calling legacy start/gate,
/// touching the old Child or sidecar, or opening its persistent cache. Port
/// resolution probes free ports but reserves none. The current producer has
/// no birth continuation: the later strict check is separately sealed and
/// cannot establish exclusive writer handoff or managed launch readiness.
pub(crate) fn generate_direct_vless_candidate(
    runtime: &ProxyRuntime,
    snapshot: &ApplyInputSnapshot,
    input: &ManagedMeshPlanInput,
    plan: &ManagedMeshRoutePlan,
) -> Result<SealedCandidate, CandidateError> {
    runtime
        .config
        .admit_mesh_apply_snapshot(snapshot)
        .map_err(|_| CandidateError::SnapshotChanged)?;
    if input.policy != *snapshot.policy()
        || input.state != *snapshot.state()
        || input.config_version != snapshot.config_version()
        || input.plan_id != plan.plan_id
        || input.state.revision != plan.input_state_revision
        || input.config_version != plan.config_version
        || !input.candidates.is_empty()
        || !input.scopeable_rule_matchers.is_empty()
    {
        return Err(CandidateError::SnapshotMismatch);
    }
    let user_config: UserConfig =
        serde_json::from_value(snapshot.raw().clone()).map_err(|_| CandidateError::Unsupported)?;
    if !conservative_user_input(&user_config) || !conservative_raw_server(snapshot.raw()) {
        return Err(CandidateError::Unsupported);
    }
    let control_port =
        polaris_config_engine::user_config::proxy_ports::control_api_port(&user_config);
    let (api_port, update_port, subscription_port, probe_port, pool_ports) = runtime
        .resolve_start_ports(&user_config, control_port)
        .map_err(|_| CandidateError::Unsupported)?;
    let mut deps = runtime.generate_deps(
        api_port,
        update_port,
        subscription_port,
        probe_port,
        &pool_ports,
        snapshot.raw(),
        false,
    );
    if deps.race_server_port != 0
        || !deps.race_upstream_ips.is_empty()
        || !deps.race_upstream_ports.is_empty()
        || !deps.observed_tailnet_addresses.is_empty()
        || deps.log_file_path.is_some()
        || deps.network_canary_port.is_some()
    {
        return Err(CandidateError::Unsupported);
    }
    // A full builder is only a pure producer for this subset if it never
    // reaches a live SRS file probe or a shared degraded callback. Its normal
    // informational logs are inert here; a warning or error rejects the
    // candidate. None of these callbacks can write shared runtime state.
    deps.log = no_candidate_log;
    deps.on_degraded = no_candidate_degraded;
    deps.is_valid_srs_fn = no_candidate_srs_probe;
    let expected_cache = runtime.config.dir().join("cache.db");
    if expected_cache
        .to_str()
        .is_none_or(|path| deps.cache_path != path)
    {
        return Err(CandidateError::Unsupported);
    }
    let runtime_bind_interfaces = BTreeMap::new(); // non-TUN + no explicit bindings
    let previous_callback_state =
        BUILDER_CALLED_UNSUPPORTED_DEPENDENCY.with(|called| called.replace(false));
    let generated = generate_sing_box_config_with_report_and_runtime_bindings(
        &user_config,
        &BTreeMap::new(),
        &deps,
        &runtime_bind_interfaces,
    );
    let unsupported_dependency_called = BUILDER_CALLED_UNSUPPORTED_DEPENDENCY
        .with(|called| called.replace(previous_callback_state));
    if unsupported_dependency_called {
        return Err(CandidateError::Unsupported);
    }
    let generated = generated.map_err(|_| CandidateError::Builder)?;
    let cache = resources_are_supported(&generated.config, &expected_cache)?;
    if !generated.invalid_nodes.is_empty()
        || !generated.pruned_rule_set_tags.is_empty()
        || !generated.pruned_env_rules.is_empty()
        || generated.network_canary.is_some()
        || !generated.mesh_route_candidates.is_empty()
        || generated.mesh_route_total_candidate_count != 0
        || generated.mesh_route_diagnostics_limited
        || generated.mesh_route_dns_owner_server_id.is_some()
    {
        return Err(CandidateError::Unsupported);
    }
    let source_config_sha256 = polaris_updater::verify::sha256_hex(
        &serde_json::to_vec_pretty(&generated.config).map_err(|_| CandidateError::Builder)?,
    );
    let materialized =
        materialize_local_rule_sets(runtime.config.dir(), plan, input, &generated.config, &[])
            .map_err(|_| CandidateError::Materialize)?;
    let final_cache = resources_are_supported(&materialized.emission.config, &expected_cache)?;
    let final_config_sha256 = materialized.closure.config_sha256.clone();
    let immutable_rule_sha256 = materialized.closure.rule_file_sha256.clone();
    let facts = EffectiveCandidateFacts {
        raw_document_sha256: snapshot.raw_document_sha256().into(),
        plan_digest: plan_digest(plan).map_err(|_| CandidateError::SnapshotMismatch)?,
        config_version: snapshot.config_version().into(),
        input_state_revision: snapshot.state().revision.clone(),
        deps,
        runtime_bind_interfaces,
        cache: final_cache,
        source_config_sha256,
        final_config_sha256,
        immutable_rule_sha256,
        mesh_route_candidates: generated.mesh_route_candidates,
        mesh_route_total_candidate_count: generated.mesh_route_total_candidate_count,
        mesh_route_diagnostics_limited: generated.mesh_route_diagnostics_limited,
        mesh_route_dns_owner_server_id: generated.mesh_route_dns_owner_server_id,
        invalid_nodes: generated.invalid_nodes,
        pruned_rule_set_tags: generated.pruned_rule_set_tags,
        pruned_env_rules: generated.pruned_env_rules,
        network_canary: generated.network_canary,
    };
    // Both references are classified from typed configs, never from a hash.
    debug_assert_eq!(cache.path, facts.cache.path);
    runtime
        .config
        .admit_mesh_apply_snapshot(snapshot)
        .map_err(|_| CandidateError::SnapshotChanged)?;
    Ok(SealedCandidate {
        materialized,
        facts,
    })
}

#[cfg(test)]
mod tests;

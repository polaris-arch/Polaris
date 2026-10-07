//! A sealed generation candidate for the first conservative desktop
//! direct/VLESS and userspace multi-Tailscale subsets. Port probes and live
//! read-only facts are captured, but no shared config, old core, or sidecar is mutated. The old
//! core's persistent cache is never an immutable artifact or birth permit.

use super::materialize::{
    capture_local_rule_sets, materialize_owned_rule_sets, relocate_owned_rule_sets,
    MaterializedEmission, OwnedRuleSources,
};
use crate::runtime::config::ApplyInputSnapshot;
use crate::runtime::proxy::mesh_apply::plan_digest;
use crate::runtime::proxy::ProxyRuntime;
use polaris_config_engine::builder::endpoint_routes::{ForceRouteLeg, MeshRouteEmissionCandidate};
use polaris_config_engine::builder::generate::{
    generate_sing_box_config_with_report_and_runtime_bindings, GenerateConfigDeps, InvalidNode,
};
use polaris_config_engine::builder::managed_mesh_emission::emit_managed_mesh_config;
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshCandidate, ManagedMeshPlanInput, ManagedMeshRoutePlan,
};
use polaris_config_engine::builder::network_env::{NetworkCanaryPlan, PrunedEnvRule};
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::effective_view::tailscale_control_authority;
use polaris_config_engine::user_config::mesh_identity_reconcile::canonical_control_authority;
use polaris_config_engine::user_config::mesh_route_state::{MeshBindingState, MeshOwnerRef};
use polaris_config_engine::user_config::proxy_mode::{ProxyMode, ProxyModeType};
use polaris_config_engine::user_config::server_config::Protocol;
use polaris_config_engine::user_config::UserConfig;
use polaris_helper_proto::Platform;
use serde_json::Value;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
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
#[derive(PartialEq, Eq)]
struct PersistentMutableCache {
    ownership: CacheOwnership,
    path: PathBuf,
    enabled: bool,
    cache_id: Option<String>,
    store_fakeip: Option<bool>,
    store_dns: Option<bool>,
    selected_policy: SelectedCachePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateProfile {
    DesktopNonTunDirectVlessV1,
    ManagedMultiTsUnsupported,
    DesktopNonTunUserspaceMultiTsV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortsProvenance {
    ProbedNumbersNotReserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubsetApplicability {
    NotRequiredByValidatedNonTunSubset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckSupport {
    LinuxX8664PinnedPlainTcpOnly,
    UnsupportedProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestPublication {
    Unpublished,
}

/// Non-secret preparation scope, never a managed/OS readiness capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateMetadata {
    pub profile: CandidateProfile,
    pub target_os: String,
    pub target_arch: String,
    pub ports: PortsProvenance,
    pub interface_binding: SubsetApplicability,
    pub dns_sidecar: SubsetApplicability,
    pub check_support: CheckSupport,
    pub manifest: ManifestPublication,
    pub cache_writer_lease_and_selector_readback_required: bool,
    pub managed_launch_unsupported: bool,
    pub tailscale_state_and_taildrop_handoff_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateIntegrityError {
    SnapshotMismatch,
    PlanMismatch,
    ProfileMismatch,
    EffectiveFactsMismatch,
    ClosureMismatch,
    EmissionMismatch,
    UnsupportedResources,
}

/// Captured before the one full-builder call. Contains loopback credentials
/// where applicable, so it deliberately implements neither Debug nor Serialize.
pub(crate) struct EffectiveCandidateFacts {
    effective_user_config: UserConfig,
    effective_user_config_sha256: String,
    plan: ManagedMeshRoutePlan,
    plan_input: ManagedMeshPlanInput,
    metadata: CandidateMetadata,
    raw_document_sha256: String,
    tailnet_file_write_epoch: u64,
    plan_digest: String,
    config_version: String,
    input_state_revision: String,
    deps: GenerateConfigDeps,
    deps_sha256: String,
    runtime_bind_interfaces: BTreeMap<String, String>,
    cache: PersistentMutableCache,
    source_config_sha256: String,
    source_config_bytes: Vec<u8>,
    source_config: SingBoxConfig,
    final_config_sha256: String,
    immutable_rule_sha256: BTreeMap<String, String>,
    owned_rules: OwnedRuleSources,
    mesh_route_evidence_sha256: String,
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
#[path = "protected_inputs.rs"]
pub(crate) mod protected_inputs;

fn value_hash(value: &impl serde::Serialize) -> Result<String, CandidateIntegrityError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| CandidateIntegrityError::EffectiveFactsMismatch)?;
    Ok(polaris_updater::verify::sha256_hex(&bytes))
}

/// Hash every value input, excluding the three callbacks which are inert
/// constructor choices, not runtime facts. Exhaustive destructuring forces a
/// newly added GenerateConfigDeps field to be classified here too. The value
/// can contain credentials and is never returned or logged.
fn deps_hash(deps: &GenerateConfigDeps) -> Result<String, CandidateIntegrityError> {
    let GenerateConfigDeps {
        platform,
        arch,
        race_server_port,
        probe_direct_port,
        probe_proxy_port,
        debug_probe_mixed_udp,
        update_in_port,
        subscription_update_in_port,
        loopback_auth,
        probe_pool_ports,
        lan_resolver_for_dns,
        race_upstream_ips,
        race_upstream_ports,
        has_cronet,
        cronet_copy_failed,
        has_management_api,
        privacy_mode,
        log_level,
        disable_log_file,
        dashboard_serve_dir,
        tailscale_api_port,
        cache_path,
        log_file_path,
        runtime_rules_dir,
        rule_resources_path,
        custom_rules_dir,
        tailnet_rules_dir,
        tailscale_state_dir_prefix,
        own_lan_cidrs,
        system_dns_takeover_active,
        netenv_dhcp_suppressed,
        network_canary_port,
        observed_tailnet_addresses,
        is_valid_srs_fn: _,
        log: _,
        on_degraded: _,
    } = deps;
    value_hash(&serde_json::json!({
        "platform":platform, "arch":arch, "raceServerPort":race_server_port,
        "probeDirectPort":probe_direct_port, "probeProxyPort":probe_proxy_port,
        "debugProbeMixedUdp":debug_probe_mixed_udp,
        "updateInPort":update_in_port, "subscriptionUpdateInPort":subscription_update_in_port,
        "loopbackAuth":loopback_auth, "probePoolPorts":probe_pool_ports,
        "lanResolverForDns":lan_resolver_for_dns, "raceUpstreamIps":race_upstream_ips,
        "raceUpstreamPorts":race_upstream_ports, "hasCronet":has_cronet,
        "cronetCopyFailed":cronet_copy_failed, "hasManagementApi":has_management_api,
        "privacyMode":privacy_mode, "logLevel":log_level, "disableLogFile":disable_log_file,
        "dashboardServeDir":dashboard_serve_dir, "tailscaleApiPort":tailscale_api_port,
        "cachePath":cache_path, "logFilePath":log_file_path, "runtimeRulesDir":runtime_rules_dir,
        "ruleResourcesPath":rule_resources_path, "customRulesDir":custom_rules_dir,
        "tailnetRulesDir":tailnet_rules_dir, "tailscaleStateDirPrefix":tailscale_state_dir_prefix,
        "ownLanCidrs":own_lan_cidrs, "systemDnsTakeoverActive":system_dns_takeover_active,
        "netenvDhcpSuppressed":netenv_dhcp_suppressed, "networkCanaryPort":network_canary_port,
        "observedTailnetAddresses":observed_tailnet_addresses,
    }))
}

fn metadata_for(config: &UserConfig, deps: &GenerateConfigDeps) -> CandidateMetadata {
    let userspace_mesh = config
        .servers
        .iter()
        .all(|s| s.protocol == Protocol::Tailscale);
    let server = &config.servers[0]; // validated candidate subset
    let plain_tcp = server.tls_settings.is_none()
        && server.reality_settings.is_none()
        && matches!(
            server.security,
            None | Some(polaris_config_engine::user_config::server_config::SecurityMode::None)
        );
    CandidateMetadata {
        profile: if userspace_mesh {
            CandidateProfile::DesktopNonTunUserspaceMultiTsV1
        } else {
            CandidateProfile::DesktopNonTunDirectVlessV1
        },
        target_os: deps.platform.clone(),
        target_arch: deps.arch.clone(),
        ports: PortsProvenance::ProbedNumbersNotReserved,
        interface_binding: SubsetApplicability::NotRequiredByValidatedNonTunSubset,
        dns_sidecar: SubsetApplicability::NotRequiredByValidatedNonTunSubset,
        check_support: if !userspace_mesh
            && deps.platform == "linux"
            && deps.arch == "x86_64"
            && plain_tcp
        {
            CheckSupport::LinuxX8664PinnedPlainTcpOnly
        } else {
            CheckSupport::UnsupportedProfile
        },
        manifest: ManifestPublication::Unpublished,
        cache_writer_lease_and_selector_readback_required: true,
        managed_launch_unsupported: true,
        tailscale_state_and_taildrop_handoff_required: userspace_mesh,
    }
}

impl SealedCandidate {
    pub(crate) fn metadata(&self) -> &CandidateMetadata {
        &self.facts.metadata
    }

    pub(crate) fn plan(&self) -> &ManagedMeshRoutePlan {
        &self.facts.plan
    }

    pub(crate) fn config_bytes(&self) -> &[u8] {
        &self.materialized.closure.config_bytes
    }

    /// Recheck only the original document, marker, sampled live projection and
    /// source identities. These separate reads are not a global atomic snapshot
    /// or a resource lease; consumers still need exclusive handoff before birth.
    pub(crate) fn validate_source_inputs(
        &self,
        runtime: &ProxyRuntime,
        snapshot: &ApplyInputSnapshot,
    ) -> Result<(), CandidateError> {
        let f = &self.facts;
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateError::SnapshotChanged)?;
        if f.raw_document_sha256 != snapshot.raw_document_sha256()
            || runtime
                .tailnet_file_write_epoch
                .load(std::sync::atomic::Ordering::SeqCst)
                != f.tailnet_file_write_epoch
            || runtime.observed_tailnet_snapshot() != f.deps.observed_tailnet_addresses
            || runtime.dns_race.config_projection()
                != (
                    f.deps.race_server_port,
                    f.deps.race_upstream_ips.clone(),
                    f.deps.race_upstream_ports.clone(),
                )
        {
            return Err(CandidateError::SnapshotChanged);
        }
        f.owned_rules
            .verify_sources_unchanged()
            .map_err(|_| CandidateError::SnapshotChanged)?;
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateError::SnapshotChanged)?;
        if runtime
            .tailnet_file_write_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            != f.tailnet_file_write_epoch
        {
            return Err(CandidateError::SnapshotChanged);
        }
        Ok(())
    }

    /// Pure consistency validation of this owned full-builder result. It
    /// performs no live reads, port/auth generation, I/O, or full-builder call.
    pub(crate) fn validate_same_candidate(
        &self,
        snapshot: &ApplyInputSnapshot,
        plan: &ManagedMeshRoutePlan,
        expected_profile: CandidateProfile,
    ) -> Result<(), CandidateIntegrityError> {
        let f = &self.facts;
        if f.raw_document_sha256 != snapshot.raw_document_sha256()
            || f.config_version != snapshot.config_version()
            || f.input_state_revision != snapshot.state().revision
            || f.plan_input.policy != *snapshot.policy()
            || f.plan_input.state != *snapshot.state()
        {
            return Err(CandidateIntegrityError::SnapshotMismatch);
        }
        if f.plan != *plan
            || f.plan_digest
                != plan_digest(plan).map_err(|_| CandidateIntegrityError::PlanMismatch)?
            || f.plan_input.plan_id != plan.plan_id
            || f.config_version != plan.config_version
            || f.input_state_revision != plan.input_state_revision
            || self.materialized.closure.plan_digest != f.plan_digest
        {
            return Err(CandidateIntegrityError::PlanMismatch);
        }
        if !profile_input_supported(
            expected_profile,
            &f.effective_user_config,
            snapshot.raw(),
            Platform::parse(&f.deps.platform),
        ) || f.metadata.profile != expected_profile
            || f.metadata != metadata_for(&f.effective_user_config, &f.deps)
        {
            return Err(CandidateIntegrityError::ProfileMismatch);
        }
        if value_hash(&f.effective_user_config)? != f.effective_user_config_sha256
            || deps_hash(&f.deps)? != f.deps_sha256
            || route_evidence_hash(&f.mesh_route_candidates)? != f.mesh_route_evidence_sha256
        {
            return Err(CandidateIntegrityError::EffectiveFactsMismatch);
        }
        let userspace_mesh = expected_profile == CandidateProfile::DesktopNonTunUserspaceMultiTsV1;
        if !f.runtime_bind_interfaces.is_empty()
            || f.deps.race_server_port != 0
            || !f.deps.race_upstream_ips.is_empty()
            || !f.deps.race_upstream_ports.is_empty()
            || f.deps.log_file_path.is_some()
            || f.deps.network_canary_port.is_some()
            || f.mesh_route_diagnostics_limited
            || !f.invalid_nodes.is_empty()
            || !f.pruned_rule_set_tags.is_empty()
            || !f.pruned_env_rules.is_empty()
            || f.network_canary.is_some()
            || (!userspace_mesh
                && (!f.deps.observed_tailnet_addresses.is_empty()
                    || !f.mesh_route_candidates.is_empty()
                    || f.mesh_route_total_candidate_count != 0
                    || f.mesh_route_dns_owner_server_id.is_some()))
        {
            return Err(CandidateIntegrityError::UnsupportedResources);
        }
        if userspace_mesh {
            let input = managed_input_from_builder(
                snapshot,
                &f.effective_user_config,
                &f.deps,
                &f.source_config,
                &f.mesh_route_candidates,
                f.mesh_route_total_candidate_count,
                f.mesh_route_diagnostics_limited,
                &f.owned_rules,
                &f.plan_input.plan_id,
            )
            .map_err(|_| CandidateIntegrityError::EffectiveFactsMismatch)?;
            if input.candidates != f.plan_input.candidates
                || input.scopeable_rule_matchers != f.plan_input.scopeable_rule_matchers
                || compile_managed_mesh_plan(input)
                    .map_err(|_| CandidateIntegrityError::PlanMismatch)?
                    != f.plan
            {
                return Err(CandidateIntegrityError::PlanMismatch);
            }
        }
        let closure = &self.materialized.closure;
        if (!userspace_mesh
            && (!closure.rule_files.is_empty()
                || !closure.rule_file_sha256.is_empty()
                || !f.immutable_rule_sha256.is_empty()))
            || closure.rule_file_sha256 != f.immutable_rule_sha256
            || closure.config_sha256 != f.final_config_sha256
            || polaris_updater::verify::sha256_hex(&closure.config_bytes) != f.final_config_sha256
            || polaris_updater::verify::sha256_hex(&f.source_config_bytes) != f.source_config_sha256
        {
            return Err(CandidateIntegrityError::ClosureMismatch);
        }
        let source_bytes = serde_json::to_vec_pretty(&f.source_config)
            .map_err(|_| CandidateIntegrityError::EmissionMismatch)?;
        let final_bytes = serde_json::to_vec_pretty(&self.materialized.emission.config)
            .map_err(|_| CandidateIntegrityError::EmissionMismatch)?;
        if source_bytes != f.source_config_bytes || final_bytes != closure.config_bytes {
            return Err(CandidateIntegrityError::EmissionMismatch);
        }
        let (relocated, payloads) =
            relocate_owned_rule_sets(&closure.data_dir, &f.plan, &f.source_config, &f.owned_rules)
                .map_err(|_| CandidateIntegrityError::ClosureMismatch)?;
        let rule_files = payloads
            .into_iter()
            .map(|payload| (payload.relative_path, payload.bytes))
            .collect::<Vec<_>>();
        if rule_files != closure.rule_files
            || rule_files
                .iter()
                .map(|(path, bytes)| (path.clone(), polaris_updater::verify::sha256_hex(bytes)))
                .collect::<BTreeMap<_, _>>()
                != closure.rule_file_sha256
        {
            return Err(CandidateIntegrityError::ClosureMismatch);
        }
        // D1 verification uses the same owned relocation, with no file read,
        // live dependency lookup, full-builder call or second DNS algorithm.
        let emitted = emit_managed_mesh_config(&relocated, &f.plan_input, &f.plan)
            .map_err(|_| CandidateIntegrityError::EmissionMismatch)?;
        if relocated != self.materialized.source_config
            || emitted != self.materialized.emission
            || resources_are_supported_for_profile(&emitted.config, &f.cache.path, expected_profile)
                .map_err(|_| CandidateIntegrityError::UnsupportedResources)?
                != f.cache
        {
            return Err(CandidateIntegrityError::EmissionMismatch);
        }
        Ok(())
    }
}

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

fn conservative_user_input(config: &UserConfig, platform: Platform) -> bool {
    config.proxy_mode == ProxyMode::Direct
        && config.proxy_mode_type.effective_on(platform) == ProxyModeType::SystemProxy
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

fn ignore_candidate_log(_: polaris_config_engine::user_config::LogLevel, _: &str) {}

fn route_evidence_hash(
    candidates: &[MeshRouteEmissionCandidate],
) -> Result<String, CandidateIntegrityError> {
    value_hash(
        &candidates
            .iter()
            .map(|emitted| {
                (
                    &emitted.candidate,
                    &emitted.external_path,
                    &emitted.emitted_inline,
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn profile_input_supported(
    profile: CandidateProfile,
    config: &UserConfig,
    raw: &Value,
    platform: Platform,
) -> bool {
    match profile {
        CandidateProfile::DesktopNonTunDirectVlessV1 => {
            conservative_user_input(config, platform) && conservative_raw_server(raw)
        }
        CandidateProfile::DesktopNonTunUserspaceMultiTsV1 => {
            userspace_mesh_input(config, raw, platform)
        }
        CandidateProfile::ManagedMultiTsUnsupported => false,
    }
}

fn userspace_mesh_input(config: &UserConfig, raw: &Value, platform: Platform) -> bool {
    matches!(platform, Platform::Linux | Platform::Mac | Platform::Win)
        && !config.proxy_mode_type.effective_on(platform).is_tun()
        && config.servers.len() == 2
        && config.servers.iter().all(|server| server.protocol == Protocol::Tailscale
            && !polaris_config_engine::builder::endpoint_routes::mesh_uses_system_interface(server)
            && server.bind_interface.is_none() && server.detour.is_none() && server.custom_settings.is_none()
            // D1 does not yet admit explicit mesh ACL inbound pins. Preserve
            // ordinary builder capability by rejecting this candidate profile.
            && server.mesh_inbound_policy.is_none())
        && config.subscriptions.is_empty() && config.network_interfaces.is_none()
        && config.network_profiles.is_empty()
        && polaris_dns_race::plan_upstreams(config.dns_config.as_ref(), config.proxy_mode_type.effective_on(platform)).is_none()
        && config.singbox_dashboard != Some(true)
        && raw.get("servers").and_then(Value::as_array).is_some_and(|servers| servers.len() == 2 && servers.iter().all(|server| {
            server.as_object().is_some_and(|object| object.keys().all(|key| matches!(key.as_str(),
                "id" | "name" | "protocol" | "address" | "port" | "tailscaleSettings" | "onDemand" | "meshInboundPolicy" | "createdAt" | "updatedAt")))
            && server.get("tailscaleSettings").is_none_or(|settings| settings.as_object().is_some_and(|object|
                object.keys().all(|key| polaris_config_engine::user_config::server_config::TAILSCALE_CANDIDATE_SETTINGS_FIELDS.contains(&key.as_str()))))
        }))
}

fn normalized_cidrs(
    values: impl IntoIterator<Item = String>,
) -> Result<BTreeSet<String>, CandidateError> {
    values
        .into_iter()
        .map(|value| {
            polaris_config_engine::user_config::cidr::normalize_cidr(&value)
                .ok_or(CandidateError::Unsupported)
        })
        .collect()
}

// Only the existing headless tailnet JSON shape supplies CIDR evidence. A
// general rule resource is owned for D1, but cannot be interpreted as a route
// owner merely because it has a hash.
fn owned_tailnet_cidrs(bytes: &[u8]) -> Result<BTreeSet<String>, CandidateError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| CandidateError::Unsupported)?;
    let object = value.as_object().ok_or(CandidateError::Unsupported)?;
    if object.len() != 2 || value["version"].as_u64() != Some(1) {
        return Err(CandidateError::Unsupported);
    }
    let rules = value["rules"]
        .as_array()
        .ok_or(CandidateError::Unsupported)?;
    let mut cidrs = Vec::new();
    for rule in rules {
        if rule.as_object().is_none_or(|object| object.len() != 1) {
            return Err(CandidateError::Unsupported);
        }
        for cidr in rule["ip_cidr"]
            .as_array()
            .ok_or(CandidateError::Unsupported)?
        {
            let text = cidr.as_str().ok_or(CandidateError::Unsupported)?;
            if text.len() > polaris_config_engine::builder::endpoint_routes::MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES
                || cidrs.len() >= polaris_config_engine::builder::endpoint_routes::MAX_MESH_ROUTE_REPORT_CIDRS { return Err(CandidateError::Unsupported); }
            cidrs.push(text.to_owned());
        }
    }
    normalized_cidrs(cidrs)
}

#[allow(
    clippy::too_many_arguments,
    reason = "each argument is an owned axis of the one full-builder outcome; no live reads or second builder"
)]
fn managed_input_from_builder(
    snapshot: &ApplyInputSnapshot,
    user_config: &UserConfig,
    deps: &GenerateConfigDeps,
    config: &SingBoxConfig,
    emitted: &[MeshRouteEmissionCandidate],
    total: usize,
    limited: bool,
    owned_rules: &OwnedRuleSources,
    plan_id: &str,
) -> Result<ManagedMeshPlanInput, CandidateError> {
    if limited || total != 2 || emitted.len() != total || !snapshot.policy().overrides.is_empty() {
        return Err(CandidateError::Unsupported);
    }
    let endpoints = config
        .endpoints
        .as_deref()
        .ok_or(CandidateError::Unsupported)?;
    if endpoints.len() != total {
        return Err(CandidateError::Unsupported);
    }
    let mut ids = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut candidates = Vec::with_capacity(total);
    for audit in emitted {
        let report = &audit.candidate;
        let server = user_config
            .servers
            .iter()
            .find(|server| server.id == report.server_id)
            .ok_or(CandidateError::SnapshotMismatch)?;
        if !ids.insert(&report.server_id)
            || !tags.insert(&report.tag)
            || !report.generated
            || report.generation_reason.is_some()
            || !report.unknown_reasons.is_empty()
            || report.match_cidrs.is_none()
            || report.leg == ForceRouteLeg::PreferredBy
            || endpoints
                .iter()
                .filter(|endpoint| endpoint.tag == report.tag && endpoint.type_field == "tailscale")
                .count()
                != 1
            || config
                .outbounds
                .iter()
                .any(|outbound| outbound.tag == report.tag)
            || report.configured_cidrs
                != server
                    .tailscale_settings
                    .as_ref()
                    .map(|settings| settings.routes.as_slice())
                    .unwrap_or_default()
            || report.observed_hosts.as_slice()
                != deps
                    .observed_tailnet_addresses
                    .get(&report.server_id)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
        {
            return Err(CandidateError::SnapshotMismatch);
        }
        let mut identities = snapshot.state().identities.iter().filter(|identity| {
            identity.server_id == report.server_id
                && matches!(
                    identity.binding_state,
                    MeshBindingState::Bound | MeshBindingState::Unbound
                )
        });
        let identity = identities.next().ok_or(CandidateError::SnapshotMismatch)?;
        if identities.next().is_some() {
            return Err(CandidateError::SnapshotMismatch);
        }
        let raw_server = snapshot.raw()["servers"]
            .as_array()
            .and_then(|servers| {
                servers
                    .iter()
                    .find(|server| server["id"] == report.server_id)
            })
            .ok_or(CandidateError::SnapshotMismatch)?;
        if canonical_control_authority(&identity.control_authority)
            .map_err(|_| CandidateError::SnapshotMismatch)?
            != tailscale_control_authority(raw_server)
                .map_err(|_| CandidateError::SnapshotMismatch)?
        {
            return Err(CandidateError::SnapshotMismatch);
        }
        let owner_ref = MeshOwnerRef {
            server_id: report.server_id.clone(),
            identity_epoch: identity.identity_epoch.clone(),
        };
        // STATUS memory is not epoch-bound. It can only influence this candidate
        // if it agrees exactly with the same document's bound-epoch evidence.
        let observed = report
            .observed_hosts
            .iter()
            .map(|host| {
                polaris_config_engine::builder::helpers::host_to_exclude_cidr(host)
                    .ok_or(CandidateError::Unsupported)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let recorded = snapshot
            .state()
            .observations
            .iter()
            .filter(|observation| observation.owner_ref == owner_ref)
            .flat_map(|observation| observation.raw_hosts.clone());
        if !observed.is_empty()
            && (identity.binding_state != MeshBindingState::Bound
                || observed != normalized_cidrs(recorded)?)
        {
            return Err(CandidateError::SnapshotMismatch);
        }
        let matches = normalized_cidrs(
            report
                .match_cidrs
                .as_ref()
                .unwrap()
                .iter()
                .map(|cidr| cidr.cidr.clone()),
        )?;
        if let Some(path) = &audit.external_path {
            if report.leg != ForceRouteLeg::ExternalRuleSet
                || owned_tailnet_cidrs(
                    owned_rules
                        .bytes_at(path)
                        .ok_or(CandidateError::Unsupported)?,
                )? != matches
            {
                return Err(CandidateError::SnapshotMismatch);
            }
        } else if report.leg != ForceRouteLeg::Inline {
            return Err(CandidateError::Unsupported);
        }
        // Configured declarations and the original epoch-bound observation
        // ledger drive Q. The default bootstrap pools never become ownership.
        candidates.push(ManagedMeshCandidate {
            owner_ref,
            configured_cidrs: report.configured_cidrs.clone(),
            endpoint_tag: Some(report.tag.clone()),
            evidence_complete: true,
        });
    }
    if deps
        .observed_tailnet_addresses
        .iter()
        .any(|(id, hosts)| !hosts.is_empty() && !ids.contains(id))
    {
        return Err(CandidateError::SnapshotMismatch);
    }
    Ok(ManagedMeshPlanInput {
        plan_id: plan_id.into(),
        config_version: snapshot.config_version().into(),
        policy: snapshot.policy().clone(),
        state: snapshot.state().clone(),
        candidates,
        scopeable_rule_matchers: BTreeMap::new(),
    })
}

fn resources_are_supported_for_profile(
    config: &SingBoxConfig,
    cache_path: &Path,
    profile: CandidateProfile,
) -> Result<PersistentMutableCache, CandidateError> {
    let userspace_mesh = profile == CandidateProfile::DesktopNonTunUserspaceMultiTsV1;
    if config.log.output.is_some()
        || config.endpoints.as_deref().is_some_and(|endpoints| {
            if userspace_mesh {
                endpoints.len() != 2
                    || endpoints.iter().any(|endpoint| {
                        endpoint.type_field != "tailscale"
                            || endpoint.system_interface == Some(true)
                            || !endpoint.extra.is_empty()
                            || endpoint
                                .state_directory
                                .as_deref()
                                .is_none_or(|path| !Path::new(path).is_absolute())
                            || endpoint
                                .taildrop_directory
                                .as_deref()
                                .is_none_or(|path| !Path::new(path).is_absolute())
                    })
            } else {
                !endpoints.is_empty()
            }
        })
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
            .is_some_and(|rules| !userspace_mesh && !rules.is_empty())
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
                ) && !(userspace_mesh && server.type_field.as_deref() == Some("tailscale"))
                    || (server.path.is_some() && server.type_field.as_deref() != Some("https"))
                    || (!userspace_mesh && server.endpoint.is_some())
            }) || dns
                .rules
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|rule| !userspace_mesh && rule.rule_set.is_some())
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
    generate_candidate(
        runtime,
        snapshot,
        &input.plan_id,
        CandidateProfile::DesktopNonTunDirectVlessV1,
        Some((input, plan)),
    )
}

/// The plan is derived from the exact full-builder outcome, never caller
/// supplied endpoint tags. This unpublished userspace result cannot be passed
/// to the plain-TCP checker or mint a managed birth/NoOwner capability.
pub(crate) fn generate_userspace_mesh_candidate(
    runtime: &ProxyRuntime,
    snapshot: &ApplyInputSnapshot,
    plan_id: &str,
) -> Result<SealedCandidate, CandidateError> {
    generate_candidate(
        runtime,
        snapshot,
        plan_id,
        CandidateProfile::DesktopNonTunUserspaceMultiTsV1,
        None,
    )
}

fn generate_candidate(
    runtime: &ProxyRuntime,
    snapshot: &ApplyInputSnapshot,
    plan_id: &str,
    profile: CandidateProfile,
    expected: Option<(&ManagedMeshPlanInput, &ManagedMeshRoutePlan)>,
) -> Result<SealedCandidate, CandidateError> {
    runtime
        .config
        .admit_mesh_apply_snapshot(snapshot)
        .map_err(|_| CandidateError::SnapshotChanged)?;
    if let Some((input, plan)) = expected {
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
    }
    let mesh_epoch = runtime
        .tailnet_file_write_epoch
        .load(std::sync::atomic::Ordering::SeqCst);
    if !mesh_epoch.is_multiple_of(2) {
        return Err(CandidateError::SnapshotChanged);
    }
    let user_config: UserConfig =
        serde_json::from_value(snapshot.raw().clone()).map_err(|_| CandidateError::Unsupported)?;
    if !profile_input_supported(
        profile,
        &user_config,
        snapshot.raw(),
        runtime.helper.platform(),
    ) {
        return Err(CandidateError::Unsupported);
    }
    let mut deps = runtime
        .generate_candidate_deps(&user_config, snapshot.raw())
        .map_err(|_| CandidateError::Unsupported)?;
    if deps.race_server_port != 0
        || !deps.race_upstream_ips.is_empty()
        || !deps.race_upstream_ports.is_empty()
        || (profile == CandidateProfile::DesktopNonTunDirectVlessV1
            && !deps.observed_tailnet_addresses.is_empty())
        || deps.log_file_path.is_some()
        || deps.network_canary_port.is_some()
    {
        return Err(CandidateError::Unsupported);
    }
    // A full builder is only a pure producer for this subset if it never
    // reaches a live SRS file probe or a shared degraded callback. Its normal
    // logs are inert here. The VLESS subset rejects warnings; mesh overlap
    // diagnostics are consumed from the same outcome below. Degraded or file
    // probe callbacks reject both profiles and cannot write runtime state.
    deps.log = if profile == CandidateProfile::DesktopNonTunDirectVlessV1 {
        no_candidate_log
    } else {
        ignore_candidate_log
    };
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
    let cache = resources_are_supported_for_profile(&generated.config, &expected_cache, profile)?;
    if !generated.invalid_nodes.is_empty()
        || !generated.pruned_rule_set_tags.is_empty()
        || !generated.pruned_env_rules.is_empty()
        || generated.network_canary.is_some()
        || (profile == CandidateProfile::DesktopNonTunDirectVlessV1
            && (!generated.mesh_route_candidates.is_empty()
                || generated.mesh_route_total_candidate_count != 0
                || generated.mesh_route_dns_owner_server_id.is_some()))
        || generated.mesh_route_diagnostics_limited
    {
        return Err(CandidateError::Unsupported);
    }
    let source_config_bytes =
        serde_json::to_vec_pretty(&generated.config).map_err(|_| CandidateError::Builder)?;
    let source_config_sha256 = polaris_updater::verify::sha256_hex(&source_config_bytes);
    let roots = if profile == CandidateProfile::DesktopNonTunUserspaceMultiTsV1 {
        [
            deps.runtime_rules_dir.as_str(),
            deps.rule_resources_path.as_str(),
            deps.custom_rules_dir.as_str(),
            deps.tailnet_rules_dir.as_str(),
        ]
        .into_iter()
        .map(PathBuf::from)
        .filter(|root| root.exists())
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let owned_rules = capture_local_rule_sets(&generated.config, &roots)
        .map_err(|_| CandidateError::Materialize)?;
    let (input, plan) = if let Some((input, plan)) = expected {
        (input.clone(), plan.clone())
    } else {
        let input = managed_input_from_builder(
            snapshot,
            &user_config,
            &deps,
            &generated.config,
            &generated.mesh_route_candidates,
            generated.mesh_route_total_candidate_count,
            generated.mesh_route_diagnostics_limited,
            &owned_rules,
            plan_id,
        )?;
        let plan = compile_managed_mesh_plan(input.clone())
            .map_err(|_| CandidateError::SnapshotMismatch)?;
        (input, plan)
    };
    let materialized = materialize_owned_rule_sets(
        runtime.config.dir(),
        &plan,
        &input,
        &generated.config,
        &owned_rules,
    )
    .map_err(|_| CandidateError::Materialize)?;
    let final_cache = resources_are_supported_for_profile(
        &materialized.emission.config,
        &expected_cache,
        profile,
    )?;
    let final_config_sha256 = materialized.closure.config_sha256.clone();
    let immutable_rule_sha256 = materialized.closure.rule_file_sha256.clone();
    let facts = EffectiveCandidateFacts {
        effective_user_config_sha256: value_hash(&user_config)
            .map_err(|_| CandidateError::Builder)?,
        metadata: metadata_for(&user_config, &deps),
        effective_user_config: user_config,
        plan: plan.clone(),
        plan_input: input.clone(),
        raw_document_sha256: snapshot.raw_document_sha256().into(),
        tailnet_file_write_epoch: mesh_epoch,
        plan_digest: plan_digest(&plan).map_err(|_| CandidateError::SnapshotMismatch)?,
        config_version: snapshot.config_version().into(),
        input_state_revision: snapshot.state().revision.clone(),
        deps_sha256: deps_hash(&deps).map_err(|_| CandidateError::Builder)?,
        deps,
        runtime_bind_interfaces,
        cache: final_cache,
        source_config_sha256,
        source_config_bytes,
        source_config: generated.config,
        final_config_sha256,
        immutable_rule_sha256,
        owned_rules,
        mesh_route_evidence_sha256: route_evidence_hash(&generated.mesh_route_candidates)
            .map_err(|_| CandidateError::Builder)?,
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
    if cache != facts.cache {
        return Err(CandidateError::Materialize);
    }
    facts
        .owned_rules
        .verify_sources_unchanged()
        .map_err(|_| CandidateError::SnapshotChanged)?;
    if runtime
        .tailnet_file_write_epoch
        .load(std::sync::atomic::Ordering::SeqCst)
        != mesh_epoch
    {
        return Err(CandidateError::SnapshotChanged);
    }
    runtime
        .config
        .admit_mesh_apply_snapshot(snapshot)
        .map_err(|_| CandidateError::SnapshotChanged)?;
    let candidate = SealedCandidate {
        materialized,
        facts,
    };
    candidate.validate_source_inputs(runtime, snapshot)?;
    candidate
        .validate_same_candidate(snapshot, &plan, profile)
        .map_err(|_| CandidateError::Materialize)?;
    Ok(candidate)
}

#[cfg(test)]
mod tests;

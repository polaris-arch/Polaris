//! A value-level boundary between one compiled mesh plan and the exact bytes
//! offered to immutable staging. Final DNS and route are independently
//! regenerated through D1's sole emitter; a caller-created hash receipt alone
//! cannot prove either was emitted from this plan. No production caller yet.

use super::artifact::{artifact_paths, validate_relative};
use crate::runtime::proxy::mesh_apply::plan_digest;
use polaris_config_engine::builder::managed_mesh_emission::emit_managed_mesh_config;
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshPlanInput, ManagedMeshRoutePlan, ManagedPlanTarget,
};
use polaris_config_engine::singbox::SingBoxConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosureError {
    SnapshotMismatch,
    PlanMismatch,
    EmissionMismatch,
    ConfigHashMismatch,
    EndpointMismatch,
    RuleSetOutsidePlan,
    RuleSetUnsupported,
    RuleFileMismatch,
    InvalidPath,
    Serialization,
}

/// Emitted alongside the final config and external bytes, from one immutable
/// builder snapshot. The config itself is independently regenerated below.
/// Rule-file hashes still need a trusted materializer; a caller-created map
/// is only a consistency check, never sufficient for durable `Prepared`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedEmission {
    pub plan_id: String,
    pub plan_digest: String,
    pub config_version: String,
    pub input_state_revision: String,
    pub config_sha256: String,
    /// Compact serialization of each typed endpoint keyed by generated tag.
    pub endpoint_sha256: BTreeMap<String, String>,
    /// Exact external bytes keyed by path relative to this plan directory.
    pub rule_file_sha256: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RulePayload {
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

/// Only `validate_closure` may construct this. Staging must use these owned
/// bytes directly, then compare its manifest against the recorded hashes.
#[derive(Debug)]
pub(crate) struct ValidatedClosure {
    pub(super) data_dir: PathBuf,
    pub(super) plan_digest: String,
    pub(super) config_bytes: Vec<u8>,
    pub(super) rule_files: Vec<(String, Vec<u8>)>,
    pub(super) config_sha256: String,
    pub(super) rule_file_sha256: BTreeMap<String, String>,
}

fn sha256(bytes: &[u8]) -> String {
    polaris_updater::verify::sha256_hex(bytes)
}

fn exact_endpoint_hashes(
    input: &ManagedMeshPlanInput,
    config: &SingBoxConfig,
) -> Result<BTreeMap<String, String>, ClosureError> {
    let mut required = BTreeMap::new();
    for candidate in &input.candidates {
        if let Some(tag) = &candidate.endpoint_tag {
            if tag.is_empty()
                || required
                    .insert(tag.as_str(), &candidate.owner_ref)
                    .is_some()
            {
                return Err(ClosureError::EndpointMismatch);
            }
        }
    }
    let mut actual = BTreeMap::new();
    for endpoint in config.endpoints.as_deref().unwrap_or_default() {
        if required.contains_key(endpoint.tag.as_str()) {
            if endpoint.type_field != "tailscale" || actual.contains_key(&endpoint.tag) {
                return Err(ClosureError::EndpointMismatch);
            }
            let bytes = serde_json::to_vec(endpoint).map_err(|_| ClosureError::Serialization)?;
            actual.insert(endpoint.tag.clone(), sha256(&bytes));
        }
    }
    if actual.len() != required.len()
        || config
            .outbounds
            .iter()
            .any(|outbound| required.contains_key(outbound.tag.as_str()))
    {
        return Err(ClosureError::EndpointMismatch);
    }
    Ok(actual)
}

fn plan_owner_tags(plan: &ManagedMeshRoutePlan) -> BTreeSet<&str> {
    let mut tags: BTreeSet<&str> = plan
        .owner_routes
        .iter()
        .map(|route| route.endpoint_tag.as_str())
        .collect();
    for override_rule in &plan.overrides {
        if let ManagedPlanTarget::Owner { endpoint_tag, .. } = &override_rule.target {
            tags.insert(endpoint_tag);
        }
    }
    tags
}

fn config_rule_paths(
    root: &Path,
    config: &SingBoxConfig,
) -> Result<BTreeSet<String>, ClosureError> {
    let mut paths = BTreeSet::new();
    let Some(route) = &config.route else {
        return Err(ClosureError::RuleSetUnsupported);
    };
    for rule_set in route.rule_set.as_deref().unwrap_or_default() {
        // Existing builtin SRS and remote rule sets have no immutable bytes in
        // this plan. Until the final emitter relocates them, block Apply.
        if rule_set.type_field != "local"
            || !matches!(rule_set.format.as_str(), "source" | "binary")
            || rule_set.url.is_some()
        {
            return Err(ClosureError::RuleSetUnsupported);
        }
        let path = rule_set
            .path
            .as_deref()
            .ok_or(ClosureError::RuleSetUnsupported)?;
        let relative = Path::new(path)
            .strip_prefix(root)
            .map_err(|_| ClosureError::RuleSetOutsidePlan)?;
        let parts = relative
            .components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str().ok_or(ClosureError::InvalidPath),
                _ => Err(ClosureError::InvalidPath),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let name = parts.join("/");
        validate_relative(&name).map_err(|_| ClosureError::InvalidPath)?;
        if matches!(
            name.as_str(),
            "config.json" | "plan.json" | "manifest.json" | "manifest.pending"
        ) {
            return Err(ClosureError::InvalidPath);
        }
        paths.insert(name);
    }
    Ok(paths)
}

/// Validate all bindings before creating a plan directory. The caller must
/// provide the emitter's expected hashes, not derive them from these inputs;
/// the independent D1 re-emission proves final DNS/route and endpoint shape.
pub(crate) fn validate_closure(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    input: &ManagedMeshPlanInput,
    legacy: &SingBoxConfig,
    config: &SingBoxConfig,
    expected: &ExpectedEmission,
    rules: Vec<RulePayload>,
) -> Result<ValidatedClosure, ClosureError> {
    if expected.plan_id != plan.plan_id
        || expected.config_version != plan.config_version
        || expected.input_state_revision != plan.input_state_revision
        || input.plan_id != plan.plan_id
        || input.config_version != plan.config_version
        || input.state.revision != plan.input_state_revision
    {
        return Err(ClosureError::SnapshotMismatch);
    }
    let rebuilt =
        compile_managed_mesh_plan(input.clone()).map_err(|_| ClosureError::PlanMismatch)?;
    let digest = plan_digest(plan).map_err(|_| ClosureError::Serialization)?;
    if rebuilt != *plan || expected.plan_digest != digest {
        return Err(ClosureError::PlanMismatch);
    }
    let emitted = emit_managed_mesh_config(legacy, input, plan)
        .map_err(|_| ClosureError::EmissionMismatch)?;
    if emitted.config != *config {
        return Err(ClosureError::EmissionMismatch);
    }
    let (root, _) =
        artifact_paths(data_dir, &plan.plan_id).map_err(|_| ClosureError::InvalidPath)?;
    let config_bytes =
        serde_json::to_vec_pretty(config).map_err(|_| ClosureError::Serialization)?;
    let config_sha256 = sha256(&config_bytes);
    if expected.config_sha256 != config_sha256 {
        return Err(ClosureError::ConfigHashMismatch);
    }
    let endpoints = exact_endpoint_hashes(input, config)?;
    if endpoints != expected.endpoint_sha256
        || !plan_owner_tags(plan)
            .iter()
            .all(|tag| endpoints.contains_key(*tag))
    {
        return Err(ClosureError::EndpointMismatch);
    }
    let referenced = config_rule_paths(&root, config)?;
    let expected_paths: BTreeSet<_> = expected.rule_file_sha256.keys().cloned().collect();
    if referenced != expected_paths {
        return Err(ClosureError::RuleFileMismatch);
    }
    let mut actual = BTreeMap::new();
    let mut payloads = Vec::with_capacity(rules.len());
    for rule in rules {
        validate_relative(&rule.relative_path).map_err(|_| ClosureError::InvalidPath)?;
        if actual
            .insert(rule.relative_path.clone(), sha256(&rule.bytes))
            .is_some()
        {
            return Err(ClosureError::RuleFileMismatch);
        }
        payloads.push((rule.relative_path, rule.bytes));
    }
    if actual != expected.rule_file_sha256 {
        return Err(ClosureError::RuleFileMismatch);
    }
    Ok(ValidatedClosure {
        data_dir: data_dir.to_path_buf(),
        plan_digest: digest,
        config_bytes,
        rule_files: payloads,
        config_sha256,
        rule_file_sha256: actual,
    })
}

#[cfg(test)]
mod tests;

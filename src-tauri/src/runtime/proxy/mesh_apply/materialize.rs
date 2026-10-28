//! Snapshot every local rule set from one generated legacy config, relocate
//! those bytes into the plan's private generation, then invoke D1's sole
//! managed emitter. No artifact directory is created here. The caller must
//! obtain legacy, plan input, and source roots from one config snapshot.

use super::artifact::{artifact_paths, MAX_FILES, MAX_TOTAL_BYTES};
use super::closure::{
    validate_closure, ClosureError, ExpectedEmission, RulePayload, ValidatedClosure,
};
use super::file_snapshot::FileSnapshot;
use crate::runtime::proxy::mesh_apply::plan_digest;
use polaris_config_engine::builder::managed_mesh_emission::{
    emit_managed_mesh_config, ManagedMeshEmission,
};
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshPlanInput, ManagedMeshRoutePlan,
};
use polaris_config_engine::singbox::SingBoxConfig;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::fs;
use std::fs::File;
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterializeError {
    SnapshotMismatch,
    InvalidSourceRoot,
    UntrustedRuleSource,
    MissingRuleSource,
    UnsupportedRuleSet,
    DuplicateRuleSetTag,
    ResourceBudget,
    SourceIo,
    SourceChanged,
    Emission,
    Closure(ClosureError),
}

impl From<ClosureError> for MaterializeError {
    fn from(error: ClosureError) -> Self {
        Self::Closure(error)
    }
}

#[derive(Debug)]
pub(crate) struct MaterializedEmission {
    pub emission: ManagedMeshEmission,
    pub closure: ValidatedClosure,
    /// Relocated legacy config used by the sole D1 emitter, not another builder.
    pub source_config: SingBoxConfig,
}

/// Local files captured once before plan compilation. Their values are owned;
/// their source paths remain mutable and do not constitute a runtime lease.
#[derive(Debug)]
pub(crate) struct OwnedRuleSources {
    sources: Vec<OwnedRuleSource>,
    rule_sets_sha256: String,
}

#[derive(Debug)]
struct OwnedRuleSource {
    path: PathBuf,
    identity: FileSnapshot,
    directories: Vec<(PathBuf, FileSnapshot)>,
    bytes: Vec<u8>,
}

impl OwnedRuleSources {
    pub(crate) fn bytes_at(&self, path: &str) -> Option<&[u8]> {
        self.sources
            .iter()
            .find(|source| source.path == Path::new(path))
            .map(|source| source.bytes.as_slice())
    }

    pub(crate) fn verify_sources_unchanged(&self) -> Result<(), MaterializeError> {
        for source in &self.sources {
            let current =
                FileSnapshot::path(&source.path).map_err(|_| MaterializeError::SourceChanged)?;
            if current.is_reparse()
                || !current.is_file()
                || !source.identity.same_snapshot(&current)
            {
                return Err(MaterializeError::SourceChanged);
            }
            for (path, before) in &source.directories {
                let after =
                    FileSnapshot::path(path).map_err(|_| MaterializeError::SourceChanged)?;
                if after.is_reparse() || !after.is_dir() || !before.same_identity(&after) {
                    return Err(MaterializeError::SourceChanged);
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn capture_local_rule_sets(
    config: &SingBoxConfig,
    trusted_source_roots: &[PathBuf],
) -> Result<OwnedRuleSources, MaterializeError> {
    validate_source_roots(trusted_source_roots)?;
    let rules = config
        .route
        .as_ref()
        .ok_or(MaterializeError::Emission)?
        .rule_set
        .as_deref()
        .unwrap_or_default();
    if rules.len() > MAX_FILES {
        return Err(MaterializeError::ResourceBudget);
    }
    let mut tags = BTreeSet::new();
    let mut sources = Vec::with_capacity(rules.len());
    let mut total = 0usize;
    for rule in rules {
        if !tags.insert(&rule.tag) {
            return Err(MaterializeError::DuplicateRuleSetTag);
        }
        if rule.type_field != "local"
            || !matches!(rule.format.as_str(), "source" | "binary")
            || rule.url.is_some()
        {
            return Err(MaterializeError::UnsupportedRuleSet);
        }
        let path = PathBuf::from(
            rule.path
                .as_deref()
                .ok_or(MaterializeError::UnsupportedRuleSet)?,
        );
        let source = read_trusted_source(&path, trusted_source_roots)?;
        total = total
            .checked_add(source.bytes.len())
            .ok_or(MaterializeError::ResourceBudget)?;
        if total > MAX_TOTAL_BYTES {
            return Err(MaterializeError::ResourceBudget);
        }
        sources.push(source);
    }
    let owned = OwnedRuleSources {
        sources,
        rule_sets_sha256: sha256(
            &serde_json::to_vec(&config.route.as_ref().unwrap().rule_set)
                .map_err(|_| MaterializeError::Emission)?,
        ),
    };
    owned.verify_sources_unchanged()?;
    Ok(owned)
}

fn sha256(bytes: &[u8]) -> String {
    polaris_updater::verify::sha256_hex(bytes)
}

fn checked_directory_chain(path: &Path) -> Result<Vec<(PathBuf, FileSnapshot)>, MaterializeError> {
    let mut chain = Vec::new();
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata =
            FileSnapshot::path(ancestor).map_err(|_| MaterializeError::MissingRuleSource)?;
        if metadata.is_reparse() || !metadata.is_dir() {
            return Err(MaterializeError::UntrustedRuleSource);
        }
        chain.push((ancestor.to_path_buf(), metadata));
    }
    Ok(chain)
}

fn validate_source_roots(roots: &[PathBuf]) -> Result<(), MaterializeError> {
    for root in roots {
        if !root.is_absolute()
            || root.components().any(|part| {
                !matches!(
                    part,
                    Component::RootDir | Component::Prefix(_) | Component::Normal(_)
                )
            })
        {
            return Err(MaterializeError::InvalidSourceRoot);
        }
        checked_directory_chain(root).map_err(|_| MaterializeError::InvalidSourceRoot)?;
    }
    Ok(())
}

fn open_source(path: &Path) -> Result<File, MaterializeError> {
    #[cfg(windows)]
    return FileSnapshot::open_path(path).map_err(|_| MaterializeError::SourceIo);
    #[cfg(not(windows))]
    {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(nix::libc::O_NOFOLLOW);
        options.open(path).map_err(|_| MaterializeError::SourceIo)
    }
}

fn read_trusted_source(
    path: &Path,
    roots: &[PathBuf],
) -> Result<OwnedRuleSource, MaterializeError> {
    read_trusted_source_after_lstat(path, roots, || {})
}

fn read_trusted_source_after_lstat(
    path: &Path,
    roots: &[PathBuf],
    after_lstat: impl FnOnce(),
) -> Result<OwnedRuleSource, MaterializeError> {
    if !path.is_absolute() {
        return Err(MaterializeError::UntrustedRuleSource);
    }
    for root in roots {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let parts = relative.components().collect::<Vec<_>>();
        if parts.is_empty()
            || parts
                .iter()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(MaterializeError::UntrustedRuleSource);
        }
        let current = path.to_path_buf();
        // These pre/post identity checks detect ordinary concurrent updates.
        // They are not an atomic openat walk: an adversary able to replace an
        // ancestor and restore it between checks could escape this boundary.
        // Production must use private, permission-restricted source roots (or
        // upgrade this to fd-relative traversal before admitting shared roots).
        let directories = checked_directory_chain(
            current
                .parent()
                .ok_or(MaterializeError::UntrustedRuleSource)?,
        )?;
        let metadata =
            FileSnapshot::path(&current).map_err(|_| MaterializeError::MissingRuleSource)?;
        if metadata.is_reparse() || !metadata.is_file() {
            return Err(MaterializeError::UntrustedRuleSource);
        }
        if metadata.len() > MAX_TOTAL_BYTES as u64 {
            return Err(MaterializeError::ResourceBudget);
        }
        after_lstat();
        let mut file = open_source(&current)?;
        let opened = FileSnapshot::opened(&file).map_err(|_| MaterializeError::SourceIo)?;
        if opened.is_reparse() || !opened.is_file() || !metadata.same_snapshot(&opened) {
            return Err(MaterializeError::SourceChanged);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.by_ref()
            .take(MAX_TOTAL_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| MaterializeError::SourceIo)?;
        if bytes.len() > MAX_TOTAL_BYTES {
            return Err(MaterializeError::ResourceBudget);
        }
        let end_opened = FileSnapshot::opened(&file).map_err(|_| MaterializeError::SourceIo)?;
        let end_path = FileSnapshot::path(&current).map_err(|_| MaterializeError::SourceChanged)?;
        if end_path.is_reparse()
            || !end_path.is_file()
            || bytes.len() as u64 != metadata.len()
            || !metadata.same_snapshot(&end_opened)
            || !metadata.same_snapshot(&end_path)
        {
            return Err(MaterializeError::SourceChanged);
        }
        for (dir, before) in &directories {
            let after = FileSnapshot::path(dir).map_err(|_| MaterializeError::SourceChanged)?;
            if after.is_reparse() || !after.is_dir() || !before.same_identity(&after) {
                return Err(MaterializeError::SourceChanged);
            }
        }
        return Ok(OwnedRuleSource {
            path: current,
            identity: metadata,
            directories,
            bytes,
        });
    }
    Err(MaterializeError::UntrustedRuleSource)
}

/// Materialize a complete local `route.rule_set` closure. Existing global
/// builtin/custom files are snapshotted and rewritten to immutable per-plan
/// paths. Remote rule sets and any source outside explicit trusted roots
/// block Apply; they cannot be silently reused by a managed generation.
pub(crate) fn materialize_local_rule_sets(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    input: &ManagedMeshPlanInput,
    raw_legacy: &SingBoxConfig,
    trusted_source_roots: &[PathBuf],
) -> Result<MaterializedEmission, MaterializeError> {
    if input.plan_id != plan.plan_id
        || input.config_version != plan.config_version
        || input.state.revision != plan.input_state_revision
        || compile_managed_mesh_plan(input.clone())
            .map_err(|_| MaterializeError::SnapshotMismatch)?
            != *plan
    {
        return Err(MaterializeError::SnapshotMismatch);
    }
    let owned = capture_local_rule_sets(raw_legacy, trusted_source_roots)?;
    materialize_owned_rule_sets(data_dir, plan, input, raw_legacy, &owned)
}

/// Relocation consumes exactly the captured bytes used for candidate evidence.
/// No file is reread and no shared file, cache or artifact directory is written.
pub(crate) fn materialize_owned_rule_sets(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    input: &ManagedMeshPlanInput,
    raw_legacy: &SingBoxConfig,
    owned: &OwnedRuleSources,
) -> Result<MaterializedEmission, MaterializeError> {
    if compile_managed_mesh_plan(input.clone()).map_err(|_| MaterializeError::SnapshotMismatch)?
        != *plan
    {
        return Err(MaterializeError::SnapshotMismatch);
    }
    owned.verify_sources_unchanged()?;
    let (legacy, payloads) = relocate_owned_rule_sets(data_dir, plan, raw_legacy, owned)?;
    let emission =
        emit_managed_mesh_config(&legacy, input, plan).map_err(|_| MaterializeError::Emission)?;
    let digest = plan_digest(plan).map_err(|_| MaterializeError::Emission)?;
    let config_bytes =
        serde_json::to_vec_pretty(&emission.config).map_err(|_| MaterializeError::Emission)?;
    let required_tags: BTreeSet<_> = input
        .candidates
        .iter()
        .filter_map(|candidate| candidate.endpoint_tag.as_deref())
        .collect();
    let endpoint_sha256: BTreeMap<_, _> = emission
        .config
        .endpoints
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|endpoint| required_tags.contains(endpoint.tag.as_str()))
        .map(|endpoint| {
            let bytes = serde_json::to_vec(endpoint).map_err(|_| MaterializeError::Emission)?;
            Ok((endpoint.tag.clone(), sha256(&bytes)))
        })
        .collect::<Result<_, MaterializeError>>()?;
    let expected = ExpectedEmission {
        plan_id: plan.plan_id.clone(),
        plan_digest: digest,
        config_version: input.config_version.clone(),
        input_state_revision: input.state.revision.clone(),
        config_sha256: sha256(&config_bytes),
        endpoint_sha256,
        rule_file_sha256: payloads
            .iter()
            .map(|rule| (rule.relative_path.clone(), sha256(&rule.bytes)))
            .collect(),
    };
    let closure = validate_closure(
        data_dir,
        plan,
        input,
        &legacy,
        &emission.config,
        &expected,
        payloads,
    )?;
    Ok(MaterializedEmission {
        emission,
        closure,
        source_config: legacy,
    })
}

/// Pure reconstruction from already owned values, also used by candidate
/// consistency validation. It does not probe the current source pathname.
pub(crate) fn relocate_owned_rule_sets(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    raw_legacy: &SingBoxConfig,
    owned: &OwnedRuleSources,
) -> Result<(SingBoxConfig, Vec<RulePayload>), MaterializeError> {
    let raw_rules = &raw_legacy
        .route
        .as_ref()
        .ok_or(MaterializeError::Emission)?
        .rule_set;
    if sha256(&serde_json::to_vec(raw_rules).map_err(|_| MaterializeError::Emission)?)
        != owned.rule_sets_sha256
    {
        return Err(MaterializeError::SnapshotMismatch);
    }
    let (root, _) = artifact_paths(data_dir, &plan.plan_id)
        .map_err(|_| MaterializeError::UntrustedRuleSource)?;
    let mut legacy = raw_legacy.clone();
    let rules = legacy
        .route
        .as_mut()
        .ok_or(MaterializeError::Emission)?
        .rule_set
        .as_mut();
    let mut payloads = Vec::new();
    let rule_count = rules.as_ref().map_or(0, |rules| rules.len());
    if rule_count != owned.sources.len() {
        return Err(MaterializeError::SnapshotMismatch);
    }
    if let Some(rules) = rules {
        for (index, (rule, source)) in rules.iter_mut().zip(&owned.sources).enumerate() {
            if rule.path.as_deref().map(Path::new) != Some(source.path.as_path()) {
                return Err(MaterializeError::SnapshotMismatch);
            }
            let extension = match (rule.type_field.as_str(), rule.format.as_str()) {
                ("local", "source") if rule.url.is_none() => "json",
                ("local", "binary") if rule.url.is_none() => "srs",
                _ => return Err(MaterializeError::UnsupportedRuleSet),
            };
            let relative_path = format!("rules/rs-{index:04}.{extension}");
            rule.path = Some(root.join(&relative_path).to_string_lossy().into_owned());
            payloads.push(RulePayload {
                relative_path,
                bytes: source.bytes.clone(),
            });
        }
    }
    Ok((legacy, payloads))
}

#[cfg(test)]
mod tests;

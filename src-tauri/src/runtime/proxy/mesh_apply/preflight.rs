//! Strict managed Apply preflight. Payload bytes are durable but unreferenced
//! while `sing-box check` runs; only an Accepted verdict publishes the
//! manifest. This does not start a core, acquire a lifecycle generation, or
//! constitute a core-ready/platform ACK.

use super::artifact::{
    artifact_paths, publish_manifest, stage_payload, verify_artifact_contents, ArtifactError,
    StagedArtifacts,
};
use super::closure::ValidatedClosure;
use crate::runtime::proxy::mesh_apply::plan_digest;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_core_supervisor::{run_config_check, ConfigCheckVerdict, ValidationLifecycleError};
use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreflightError {
    ClosureChanged,
    Artifact(ArtifactError),
    BinaryUnavailable,
    BinaryChanged,
    CoreRejected,
    CoreUnattributable,
    CoreUnavailable,
    ValidationLifecycle(ValidationLifecycleError),
    CandidateMismatch,
    InvalidConfig,
}

impl From<ArtifactError> for PreflightError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

#[derive(Debug)]
pub(crate) struct CoreCheckEvidence {
    /// The future spawn hook must rehash its selected executable and compare.
    pub binary_sha256: String,
    pub config_sha256: String,
    pub plan_digest: String,
    pub manifest_ref: String,
}

#[derive(Debug)]
pub(crate) struct CheckedArtifacts {
    staged: StagedArtifacts,
    core_check: CoreCheckEvidence,
}

impl CheckedArtifacts {
    pub(crate) fn staged(&self) -> &StagedArtifacts {
        &self.staged
    }

    pub(crate) fn core_check(&self) -> &CoreCheckEvidence {
        &self.core_check
    }
}

/// The current preflight does not freeze the effective user config, generated
/// deps (including race/probe ports and auth), runtime bindings or canary.
/// Until those facts are captured with the same final config, this proof is
/// deliberately not a launch input. There is no fallback to recomputing them
/// after the old core stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactStartReadiness {
    UnsupportedRuntimeFacts,
}

/// Owned, read-only proof of the exact staged candidate. Config and rule
/// bytes are retained so a future under-gate launch can reverify them and the
/// paths immediately before spawn. No path in this type is currently exposed
/// to a spawner; validation here alone does not close a later path-swap race.
pub(crate) struct VerifiedExactStart {
    plan: ManagedMeshRoutePlan,
    plan_digest: String,
    manifest_ref: String,
    /// Hash of the checked manifest's canonical published encoding.
    manifest_sha256: String,
    config_sha256: String,
    binary_sha256: String,
    readiness: ExactStartReadiness,
    config_path: PathBuf,
    config_bytes: Vec<u8>,
    rule_files: Vec<(String, Vec<u8>)>,
    config: SingBoxConfig,
}

impl VerifiedExactStart {
    pub(in crate::runtime::proxy::mesh_apply) fn plan(&self) -> &ManagedMeshRoutePlan {
        &self.plan
    }

    pub(in crate::runtime::proxy::mesh_apply) fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    pub(in crate::runtime::proxy::mesh_apply) fn manifest_ref(&self) -> &str {
        &self.manifest_ref
    }

    pub(in crate::runtime::proxy::mesh_apply) fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub(in crate::runtime::proxy::mesh_apply) fn config_sha256(&self) -> &str {
        &self.config_sha256
    }

    pub(in crate::runtime::proxy::mesh_apply) fn binary_sha256(&self) -> &str {
        &self.binary_sha256
    }

    pub(in crate::runtime::proxy::mesh_apply) fn readiness(&self) -> ExactStartReadiness {
        self.readiness
    }

    /// Recheck the frozen metadata against owned bytes at the birth boundary.
    pub(crate) fn config_hash_matches_owned_bytes(&self) -> bool {
        polaris_updater::verify::sha256_hex(&self.config_bytes) == self.config_sha256
    }
}

fn binary_digest(binary: &Path) -> Result<String, PreflightError> {
    let file = File::open(binary).map_err(|_| PreflightError::BinaryUnavailable)?;
    polaris_updater::verify::sha256_reader_hex(file).map_err(|_| PreflightError::BinaryUnavailable)
}

fn compare_closure(
    closure: &ValidatedClosure,
    plan: &ManagedMeshRoutePlan,
    staged: &super::artifact::PendingArtifacts,
) -> Result<(), PreflightError> {
    let digest = plan_digest(plan).map_err(|_| PreflightError::ClosureChanged)?;
    let manifest = staged.manifest();
    let rule_hashes: BTreeMap<_, _> = manifest
        .rule_files
        .iter()
        .map(|file| (file.relative_path.clone(), file.sha256.clone()))
        .collect();
    if digest != closure.plan_digest
        || manifest.plan_digest != closure.plan_digest
        || manifest.config.sha256 != closure.config_sha256
        || rule_hashes != closure.rule_file_sha256
    {
        return Err(PreflightError::ClosureChanged);
    }
    Ok(())
}

/// Narrow seam for an injected checker. Core-supervisor already tests the
/// real child-process argv/timeout/kill path; these tests assert the Apply
/// ordering and its stricter verdict mapping without starting a real core.
pub(super) async fn checked_stage_with<F, Fut, G>(
    plan: &ManagedMeshRoutePlan,
    closure: ValidatedClosure,
    generator_version: &str,
    check: F,
    after_check: G,
) -> Result<StagedArtifacts, PreflightError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<ConfigCheckVerdict, ValidationLifecycleError>>,
    G: FnOnce() -> Result<(), PreflightError>,
{
    if plan_digest(plan).map_err(|_| PreflightError::ClosureChanged)? != closure.plan_digest {
        return Err(PreflightError::ClosureChanged);
    }
    let pending = stage_payload(
        &closure.data_dir,
        plan,
        &closure.config_bytes,
        &closure.rule_files,
        generator_version,
    )?;
    compare_closure(&closure, plan, &pending)?;
    match check(pending.config_path.clone())
        .await
        .map_err(PreflightError::ValidationLifecycle)?
    {
        ConfigCheckVerdict::Accepted => {}
        ConfigCheckVerdict::Rejected(_) => return Err(PreflightError::CoreRejected),
        ConfigCheckVerdict::Unattributable(_) => return Err(PreflightError::CoreUnattributable),
        ConfigCheckVerdict::Unavailable(_) => return Err(PreflightError::CoreUnavailable),
    }
    after_check()?;
    Ok(publish_manifest(pending, plan)?)
}

/// Run the repository's single bounded `sing-box check` subprocess against
/// staged config and rule files. Only Accepted can publish a manifest. The
/// caller supplies the exact trusted executable it will later spawn; its hash
/// is checked before and after this await and returned for the spawn gate.
pub(crate) async fn stage_checked_with_core(
    plan: &ManagedMeshRoutePlan,
    closure: ValidatedClosure,
    generator_version: &str,
    binary: &Path,
) -> Result<CheckedArtifacts, PreflightError> {
    let before = binary_digest(binary)?;
    let binary_path = binary.to_path_buf();
    let staged = checked_stage_with(
        plan,
        closure,
        generator_version,
        move |config_path| async move { run_config_check(&binary_path, &config_path).await },
        || {
            if binary_digest(binary)? == before {
                Ok(())
            } else {
                Err(PreflightError::BinaryChanged)
            }
        },
    )
    .await?;
    Ok(CheckedArtifacts {
        core_check: CoreCheckEvidence {
            binary_sha256: before,
            config_sha256: staged.manifest.config.sha256.clone(),
            plan_digest: staged.manifest.plan_digest.clone(),
            manifest_ref: staged.manifest_ref.clone(),
        },
        staged,
    })
}

/// Re-read the published candidate and executable using the artifact reader's
/// no-follow, bounded, hash-checked path. This binds plan id/version/revision
/// through the plan digest, plus the exact manifest/config/rules/check evidence.
/// A later launch must repeat this proof under the held TS gate at the spawn
/// boundary; neither this read nor an earlier Accepted check pins a pathname.
pub(crate) fn verify_exact_start(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    checked: &CheckedArtifacts,
    binary: &Path,
) -> Result<VerifiedExactStart, PreflightError> {
    let digest = plan_digest(plan).map_err(|_| PreflightError::CandidateMismatch)?;
    let (root, manifest_ref) = artifact_paths(data_dir, &plan.plan_id)?;
    let evidence = &checked.core_check;
    if checked.staged.manifest.plan_id != plan.plan_id
        || checked.staged.manifest.plan_digest != digest
        || checked.staged.manifest_ref != manifest_ref
        || evidence.plan_digest != digest
        || evidence.manifest_ref != manifest_ref
        || evidence.config_sha256 != checked.staged.manifest.config.sha256
    {
        return Err(PreflightError::CandidateMismatch);
    }
    let before = binary_digest(binary)?;
    if before != evidence.binary_sha256 {
        return Err(PreflightError::BinaryChanged);
    }
    let contents = verify_artifact_contents(data_dir, plan)?;
    if contents.manifest != checked.staged.manifest {
        return Err(PreflightError::CandidateMismatch);
    }
    let config: SingBoxConfig = serde_json::from_slice(&contents.config_bytes)
        .map_err(|_| PreflightError::InvalidConfig)?;
    let original: serde_json::Value = serde_json::from_slice(&contents.config_bytes)
        .map_err(|_| PreflightError::InvalidConfig)?;
    if serde_json::to_value(&config).map_err(|_| PreflightError::InvalidConfig)? != original {
        return Err(PreflightError::InvalidConfig);
    }
    if binary_digest(binary)? != before {
        return Err(PreflightError::BinaryChanged);
    }
    Ok(VerifiedExactStart {
        plan: plan.clone(),
        plan_digest: digest,
        manifest_ref,
        manifest_sha256: polaris_updater::verify::sha256_hex(
            &serde_json::to_vec(&contents.manifest)
                .map_err(|_| PreflightError::CandidateMismatch)?,
        ),
        config_sha256: evidence.config_sha256.clone(),
        binary_sha256: before,
        readiness: ExactStartReadiness::UnsupportedRuntimeFacts,
        config_path: root.join(&contents.manifest.config.relative_path),
        config_bytes: contents.config_bytes,
        rule_files: contents.rule_files,
        config,
    })
}

#[cfg(test)]
mod tests;

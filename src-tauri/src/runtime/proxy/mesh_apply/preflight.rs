//! Strict managed Apply preflight. Payload bytes are durable but unreferenced
//! while `sing-box check` runs; only an Accepted verdict publishes the
//! manifest. This does not start a core, acquire a lifecycle generation, or
//! constitute a core-ready/platform ACK.

use super::artifact::{publish_manifest, stage_payload, ArtifactError, StagedArtifacts};
use super::closure::ValidatedClosure;
use super::plan_digest;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use polaris_core_supervisor::{run_config_check, ConfigCheckVerdict};
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
    pub staged: StagedArtifacts,
    pub core_check: CoreCheckEvidence,
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
    Fut: Future<Output = ConfigCheckVerdict>,
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
    match check(pending.config_path.clone()).await {
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

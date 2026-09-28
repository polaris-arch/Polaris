//! Per-plan, immutable artifact staging. The manifest is published last. No
//! existing plan directory is reused, including a crash-left incomplete one.
//! This module does not start a core or claim platform protection.

use super::{plan_digest, safe_plan_id};
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use polaris_store::fs::DurableWriteGuarantee;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const ARTIFACT_SCHEMA_VERSION: u32 = 1;
const MAX_FILES: usize = 512;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MANIFEST_NAME: &str = "manifest.json";
const CONFIG_NAME: &str = "config.json";
const PLAN_NAME: &str = "plan.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArtifactError {
    Invalid(&'static str),
    Io(String),
    /// Manifest rename may have happened. Caller must re-read/verify before
    /// deciding whether it is published; never reuse the same planId.
    CommitUncertain(String),
}

impl From<std::io::Error> for ArtifactError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArtifactFile {
    pub relative_path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArtifactManifest {
    pub schema_version: u32,
    pub plan_id: String,
    pub plan_digest: String,
    pub generator_version: String,
    pub config: ArtifactFile,
    pub plan: ArtifactFile,
    pub rule_files: Vec<ArtifactFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedArtifacts {
    pub manifest: ArtifactManifest,
    pub manifest_ref: String,
    pub durability: DurableWriteGuarantee,
}

pub(super) fn validate_relative(path: &str) -> Result<(), ArtifactError> {
    let safe_segment = |part: &str| {
        let device = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (device.len() == 4
                && (device.starts_with("COM") || device.starts_with("LPT"))
                && device.as_bytes()[3].is_ascii_digit()
                && device.as_bytes()[3] != b'0');
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.ends_with('.')
            && !reserved
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if path.is_empty()
        || path.contains('\\')
        || path.contains(':')
        || path.bytes().any(|byte| byte == 0)
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.split('/').any(|part| !safe_segment(part))
    {
        return Err(ArtifactError::Invalid("unsafe artifact relative path"));
    }
    Ok(())
}

fn artifact_file(path: &str, bytes: &[u8]) -> ArtifactFile {
    ArtifactFile {
        relative_path: path.into(),
        sha256: polaris_updater::verify::sha256_hex(bytes),
        bytes: bytes.len() as u64,
    }
}

pub(super) fn artifact_paths(
    data_dir: &Path,
    plan_id: &str,
) -> Result<(PathBuf, String), ArtifactError> {
    if !data_dir.is_absolute() || !safe_plan_id(plan_id) {
        return Err(ArtifactError::Invalid("invalid data directory or planId"));
    }
    validate_relative(plan_id)?;
    let relative = format!("mesh-routes/plans/{plan_id}");
    Ok((
        data_dir.join(&relative),
        format!("{relative}/{MANIFEST_NAME}"),
    ))
}

fn inspect_real_directory(path: &Path) -> Result<(), ArtifactError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ArtifactError::Invalid(
            "artifact directory is not a real directory",
        ));
    }
    Ok(())
}

fn inspect_directory(path: &Path) -> Result<(), ArtifactError> {
    inspect_real_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ArtifactError::Invalid(
                "artifact directory permissions are too broad",
            ));
        }
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), ArtifactError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    inspect_directory(path)
}

fn ensure_private_child(parent: &Path, name: &str) -> Result<PathBuf, ArtifactError> {
    inspect_real_directory(parent)?;
    let child = parent.join(name);
    match fs::symlink_metadata(&child) {
        Ok(_) => inspect_directory(&child)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(&child)?;
            sync_dir(parent)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(child)
}

fn sync_dir(path: &Path) -> Result<(), ArtifactError> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn checked_file_path(
    root: &Path,
    relative: &str,
    create_parents: bool,
) -> Result<PathBuf, ArtifactError> {
    validate_relative(relative)?;
    inspect_directory(root)?;
    let components: Vec<_> = relative.split('/').collect();
    let mut parent = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        parent = if create_parents {
            ensure_private_child(&parent, component)?
        } else {
            let next = parent.join(component);
            inspect_directory(&next)?;
            next
        };
    }
    let target = parent.join(components[components.len() - 1]);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(ArtifactError::Invalid("artifact target already exists"));
    }
    Ok(target)
}

fn write_new_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), ArtifactError> {
    let path = checked_file_path(root, relative, true)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_dir(
        path.parent()
            .ok_or(ArtifactError::Invalid("file has no parent"))?,
    )?;
    Ok(())
}

fn read_checked(root: &Path, relative: &str, max_bytes: u64) -> Result<Vec<u8>, ArtifactError> {
    validate_relative(relative)?;
    inspect_directory(root)?;
    let path = root.join(relative);
    let mut parent = root.to_path_buf();
    let components: Vec<_> = relative.split('/').collect();
    for component in &components[..components.len() - 1] {
        parent.push(component);
        inspect_directory(&parent)?;
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ArtifactError::Invalid("artifact is not a regular file"));
    }
    if metadata.len() > max_bytes {
        return Err(ArtifactError::Invalid("artifact size budget exceeded"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ArtifactError::Invalid(
                "artifact file permissions are too broad",
            ));
        }
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > max_bytes {
        return Err(ArtifactError::Invalid("artifact size budget exceeded"));
    }
    Ok(bytes)
}

fn verify_entry(root: &Path, entry: &ArtifactFile) -> Result<Vec<u8>, ArtifactError> {
    if entry.bytes > MAX_TOTAL_BYTES as u64 {
        return Err(ArtifactError::Invalid("artifact size budget exceeded"));
    }
    let bytes = read_checked(root, &entry.relative_path, entry.bytes)?;
    if bytes.len() as u64 != entry.bytes
        || polaris_updater::verify::sha256_hex(&bytes) != entry.sha256
    {
        return Err(ArtifactError::Invalid("artifact hash or size mismatch"));
    }
    Ok(bytes)
}

/// Verify the published manifest and every referenced byte before any core is
/// allowed to consume the directory. There is no fallback to an old plan.
pub(crate) fn verify_artifacts(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
) -> Result<ArtifactManifest, ArtifactError> {
    let (root, _) = artifact_paths(data_dir, &plan.plan_id)?;
    inspect_real_directory(data_dir)?;
    inspect_directory(&data_dir.join("mesh-routes"))?;
    inspect_directory(&data_dir.join("mesh-routes/plans"))?;
    inspect_directory(&root)?;
    let manifest_bytes = read_checked(&root, MANIFEST_NAME, MAX_MANIFEST_BYTES)?;
    let manifest: ArtifactManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ArtifactError::Invalid("invalid artifact manifest"))?;
    if manifest.schema_version != ARTIFACT_SCHEMA_VERSION
        || manifest.plan_id != plan.plan_id
        || manifest.plan_digest
            != plan_digest(plan).map_err(|_| ArtifactError::Invalid("plan serialization"))?
        || manifest.generator_version.is_empty()
        || manifest.generator_version.len() > 128
        || manifest.config.relative_path != CONFIG_NAME
        || manifest.plan.relative_path != PLAN_NAME
        || manifest.rule_files.len() > MAX_FILES
    {
        return Err(ArtifactError::Invalid("artifact manifest binding mismatch"));
    }
    let total = manifest
        .rule_files
        .iter()
        .chain([&manifest.config, &manifest.plan])
        .try_fold(0u64, |sum, entry| sum.checked_add(entry.bytes))
        .ok_or(ArtifactError::Invalid("artifact size overflow"))?;
    if total > MAX_TOTAL_BYTES as u64 {
        return Err(ArtifactError::Invalid("artifact size budget exceeded"));
    }
    let plan_bytes = verify_entry(&root, &manifest.plan)?;
    if polaris_updater::verify::sha256_hex(&plan_bytes) != manifest.plan_digest {
        return Err(ArtifactError::Invalid("plan artifact digest mismatch"));
    }
    verify_entry(&root, &manifest.config)?;
    let mut seen = BTreeSet::from([CONFIG_NAME, PLAN_NAME, MANIFEST_NAME]);
    for entry in &manifest.rule_files {
        validate_relative(&entry.relative_path)?;
        if !seen.insert(&entry.relative_path) {
            return Err(ArtifactError::Invalid("duplicate artifact path"));
        }
        verify_entry(&root, entry)?;
    }
    Ok(manifest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagePoint {
    AfterPayload,
    BeforeManifestRename,
    AfterManifestRename,
}

/// Payload exists but no manifest does. A failed core check leaves this
/// generation unusable; its planId cannot be retried or silently recycled.
#[derive(Debug)]
pub(crate) struct PendingArtifacts {
    pub(super) data_dir: PathBuf,
    pub(super) config_path: PathBuf,
    root: PathBuf,
    manifest_ref: String,
    manifest: ArtifactManifest,
}

impl PendingArtifacts {
    pub(super) fn manifest(&self) -> &ArtifactManifest {
        &self.manifest
    }
}

/// Test-only convenience for the original S4b staging assertions. Production
/// callers must stage payload, run a strict core check, then publish.
#[cfg(test)]
pub(crate) fn stage_artifacts(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    final_config: &[u8],
    rule_files: &[(String, Vec<u8>)],
    generator_version: &str,
) -> Result<StagedArtifacts, ArtifactError> {
    stage_artifacts_with_hook(
        data_dir,
        plan,
        final_config,
        rule_files,
        generator_version,
        |_| Ok(()),
    )
}

#[cfg(test)]
fn stage_artifacts_with_hook(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    final_config: &[u8],
    rule_files: &[(String, Vec<u8>)],
    generator_version: &str,
    mut hook: impl FnMut(StagePoint) -> Result<(), ArtifactError>,
) -> Result<StagedArtifacts, ArtifactError> {
    let pending = stage_payload_with_hook(
        data_dir,
        plan,
        final_config,
        rule_files,
        generator_version,
        &mut hook,
    )?;
    publish_manifest_with_hook(pending, plan, &mut hook)
}

/// Write and fsync every immutable payload byte without making the generation
/// referenceable. No old generation is ever overwritten.
pub(crate) fn stage_payload(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    final_config: &[u8],
    rule_files: &[(String, Vec<u8>)],
    generator_version: &str,
) -> Result<PendingArtifacts, ArtifactError> {
    stage_payload_with_hook(
        data_dir,
        plan,
        final_config,
        rule_files,
        generator_version,
        &mut |_| Ok(()),
    )
}

fn stage_payload_with_hook(
    data_dir: &Path,
    plan: &ManagedMeshRoutePlan,
    final_config: &[u8],
    rule_files: &[(String, Vec<u8>)],
    generator_version: &str,
    hook: &mut impl FnMut(StagePoint) -> Result<(), ArtifactError>,
) -> Result<PendingArtifacts, ArtifactError> {
    if generator_version.is_empty() || generator_version.len() > 128 || rule_files.len() > MAX_FILES
    {
        return Err(ArtifactError::Invalid(
            "invalid artifact generator or file count",
        ));
    }
    let plan_bytes =
        serde_json::to_vec(plan).map_err(|_| ArtifactError::Invalid("plan serialization"))?;
    let total = final_config
        .len()
        .checked_add(plan_bytes.len())
        .and_then(|n| {
            rule_files
                .iter()
                .try_fold(n, |sum, (_, bytes)| sum.checked_add(bytes.len()))
        })
        .ok_or(ArtifactError::Invalid("artifact size overflow"))?;
    if total > MAX_TOTAL_BYTES {
        return Err(ArtifactError::Invalid("artifact size budget exceeded"));
    }
    let mut seen = BTreeSet::from([
        CONFIG_NAME.to_string(),
        PLAN_NAME.to_string(),
        MANIFEST_NAME.to_string(),
        "manifest.pending".to_string(),
    ]);
    for (path, _) in rule_files {
        validate_relative(path)?;
        if !seen.insert(path.clone()) {
            return Err(ArtifactError::Invalid("duplicate artifact path"));
        }
    }
    let (root, manifest_ref) = artifact_paths(data_dir, &plan.plan_id)?;
    inspect_real_directory(data_dir)?;
    let mesh = ensure_private_child(data_dir, "mesh-routes")?;
    let plans = ensure_private_child(&mesh, "plans")?;
    create_private_dir(&root)?; // create_new semantics: never reuse an old or incomplete generation
    sync_dir(&plans)?;
    write_new_file(&root, CONFIG_NAME, final_config)?;
    write_new_file(&root, PLAN_NAME, &plan_bytes)?;
    let mut entries = Vec::with_capacity(rule_files.len());
    for (path, bytes) in rule_files {
        write_new_file(&root, path, bytes)?;
        entries.push(artifact_file(path, bytes));
    }
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    hook(StagePoint::AfterPayload)?;
    let manifest = ArtifactManifest {
        schema_version: ARTIFACT_SCHEMA_VERSION,
        plan_id: plan.plan_id.clone(),
        plan_digest: polaris_updater::verify::sha256_hex(&plan_bytes),
        generator_version: generator_version.into(),
        config: artifact_file(CONFIG_NAME, final_config),
        plan: artifact_file(PLAN_NAME, &plan_bytes),
        rule_files: entries,
    };
    Ok(PendingArtifacts {
        data_dir: data_dir.to_path_buf(),
        config_path: root.join(CONFIG_NAME),
        root,
        manifest_ref,
        manifest,
    })
}

fn verify_pending_payload(
    pending: &PendingArtifacts,
    plan: &ManagedMeshRoutePlan,
) -> Result<(), ArtifactError> {
    if pending.manifest.plan_id != plan.plan_id
        || pending.manifest.plan_digest
            != plan_digest(plan).map_err(|_| ArtifactError::Invalid("plan serialization"))?
    {
        return Err(ArtifactError::Invalid("pending plan binding mismatch"));
    }
    inspect_real_directory(&pending.data_dir)?;
    inspect_directory(&pending.data_dir.join("mesh-routes"))?;
    inspect_directory(&pending.data_dir.join("mesh-routes/plans"))?;
    inspect_directory(&pending.root)?;
    let plan_bytes = verify_entry(&pending.root, &pending.manifest.plan)?;
    if polaris_updater::verify::sha256_hex(&plan_bytes) != pending.manifest.plan_digest {
        return Err(ArtifactError::Invalid("pending plan digest mismatch"));
    }
    verify_entry(&pending.root, &pending.manifest.config)?;
    for entry in &pending.manifest.rule_files {
        verify_entry(&pending.root, entry)?;
    }
    Ok(())
}

/// Publish only after an external strict check has accepted this exact
/// config. The caller owns that proof and rechecks its state claim afterward.
pub(crate) fn publish_manifest(
    pending: PendingArtifacts,
    plan: &ManagedMeshRoutePlan,
) -> Result<StagedArtifacts, ArtifactError> {
    publish_manifest_with_hook(pending, plan, &mut |_| Ok(()))
}

fn publish_manifest_with_hook(
    pending: PendingArtifacts,
    plan: &ManagedMeshRoutePlan,
    hook: &mut impl FnMut(StagePoint) -> Result<(), ArtifactError>,
) -> Result<StagedArtifacts, ArtifactError> {
    verify_pending_payload(&pending, plan)?;
    let root = pending.root;
    let manifest = pending.manifest;
    let data_dir = pending.data_dir;
    let manifest_ref = pending.manifest_ref;
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|_| ArtifactError::Invalid("manifest serialization"))?;
    // Temp is in the same private generation. A crash before rename leaves no
    // manifest; a post-rename sync failure has an explicitly uncertain result.
    write_new_file(&root, "manifest.pending", &bytes)?;
    hook(StagePoint::BeforeManifestRename)?;
    if fs::symlink_metadata(root.join(MANIFEST_NAME)).is_ok() {
        return Err(ArtifactError::Invalid("manifest target already exists"));
    }
    fs::rename(root.join("manifest.pending"), root.join(MANIFEST_NAME))?;
    hook(StagePoint::AfterManifestRename)
        .map_err(|error| ArtifactError::CommitUncertain(format!("{error:?}")))?;
    sync_dir(&root).map_err(|error| ArtifactError::CommitUncertain(format!("{error:?}")))?;
    let verified = verify_artifacts(&data_dir, plan)
        .map_err(|error| ArtifactError::CommitUncertain(format!("{error:?}")))?;
    if verified != manifest {
        return Err(ArtifactError::CommitUncertain(
            "artifact manifest changed after publish".into(),
        ));
    }
    Ok(StagedArtifacts {
        manifest,
        manifest_ref,
        durability: if cfg!(unix) {
            DurableWriteGuarantee::FileAndDirectory
        } else {
            DurableWriteGuarantee::FileOnly
        },
    })
}

#[cfg(test)]
mod tests;

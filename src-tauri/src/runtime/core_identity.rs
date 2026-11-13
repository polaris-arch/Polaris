//! B2b: compiled selected content identity, independent of a writable package manifest.
use std::path::{Path, PathBuf};

use polaris_helper_proto::core_manifest::{CoreId, CoreManifest};

pub(crate) use polaris_core_supervisor::selected_core::current_platform;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod tls_inventory;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) use tls_inventory::expected_tls_support;
const APP_INSTALLATION: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/selected-core-manifest.json"));
const APP_DEVELOPMENT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/local-development-selection.json"
));
const APP_DEVELOPMENT_CONTRACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/local-development-contract.json"));
fn check_compiled_selection() -> Result<(), String> {
    if APP_DEVELOPMENT != polaris_core_supervisor::selected_core::COMPILED_DEVELOPMENT_SELECTION
        || APP_DEVELOPMENT_CONTRACT
            != polaris_core_supervisor::selected_core::COMPILED_DEVELOPMENT_CONTRACT
        || APP_INSTALLATION != polaris_core_supervisor::selected_core::COMPILED_INSTALLATION
    {
        return Err(
            "CORE_BUNDLE_MISMATCH: App and execution gate compiled selections differ".into(),
        );
    }
    Ok(())
}
pub(crate) fn selected_manifest(platform: &str) -> Result<CoreManifest, String> {
    check_compiled_selection()?;
    polaris_core_supervisor::selected_core::selected_manifest(platform)
}
pub(crate) fn selected_id(platform: &str) -> Result<CoreId, String> {
    check_compiled_selection()?;
    polaris_core_supervisor::selected_core::selected_id(platform)
}
pub(crate) fn selected_payload_dirs(formal: &[&str]) -> Result<Vec<String>, String> {
    check_compiled_selection()?;
    polaris_core_supervisor::selected_core::selected_payload_dirs(formal)
}
pub(crate) fn selected_source_commit(platform: &str) -> Result<String, String> {
    check_compiled_selection()?;
    polaris_core_supervisor::selected_core::selected_source_commit(platform)
}
#[cfg(test)]
use polaris_core_supervisor::selected_core::{
    formal_manifest, formal_provider_fingerprint, COMPILED_INSTALLATION, RELEASE_CONTRACT,
};

pub(crate) use polaris_core_supervisor::admitted_payload::{
    AdmissionError, AdmittedPayload, ExecPurpose, PreparedExecution,
};
pub(crate) fn acquire_at(
    binary: &Path,
    purpose: ExecPurpose,
) -> Result<std::sync::Arc<AdmittedPayload>, AdmissionError> {
    check_compiled_selection().map_err(|detail| AdmissionError {
        kind: polaris_core_supervisor::admitted_payload::AdmissionErrorKind::Mismatch,
        purpose: Some(purpose),
        member: None,
        recovery: polaris_core_supervisor::admitted_payload::RecoveryAction::ReinstallCurrentApp,
        detail,
    })?;
    polaris_core_supervisor::admitted_payload::admit_selected(
        polaris_core_supervisor::admitted_payload::UntrustedPayload::bundled(binary),
        purpose,
    )
}
pub(crate) fn prepare(
    binary: &Path,
    purpose: ExecPurpose,
) -> Result<PreparedExecution, AdmissionError> {
    let payload = acquire_at(binary, purpose)?;
    prepare_admitted(&payload, purpose)
}
pub(crate) fn selection_error(detail: String, purpose: ExecPurpose) -> AdmissionError {
    use polaris_core_supervisor::admitted_payload::{AdmissionErrorKind as K, RecoveryAction as R};
    // Candidate selection only reports a missing payload; anything else is unknown.
    let kind = if detail.starts_with("CORE_ADMISSION_Missing:") {
        K::Missing
    } else {
        K::Unknown
    };
    AdmissionError {
        kind,
        purpose: Some(purpose),
        member: None,
        recovery: if kind.is_payload_damage() {
            R::ReinstallCurrentApp
        } else if kind == K::Busy || kind == K::Unknown {
            R::RetryAfterStop
        } else {
            R::None
        },
        detail,
    }
}
pub(crate) fn prepare_admitted(
    payload: &std::sync::Arc<AdmittedPayload>,
    purpose: ExecPurpose,
) -> Result<PreparedExecution, AdmissionError> {
    payload.prepare(purpose)
}
#[derive(Debug)]
pub(crate) struct InstallStage {
    pub(crate) directory: PathBuf,
    pub(crate) manifest: polaris_helper_proto::core_manifest::InstalledCoreManifest,
}
impl Drop for InstallStage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Private, bounded, complete copy for the explicit administrator transaction.
/// Start/status never invokes this function and never transports payload bytes.
pub(crate) fn stage_installation(
    binary: &Path,
    helper: &Path,
    config: &Path,
    selected: &CoreManifest,
) -> Result<InstallStage, String> {
    use polaris_helper_proto::core_manifest::InstalledCoreManifest;
    #[cfg(unix)]
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    let platform = selected.platform.as_str();
    let expected_helper = selected
        .helper_sha256
        .as_deref()
        .ok_or("CORE_BUNDLE_MISMATCH: compiled installer input unavailable")?;
    let source = binary
        .parent()
        .ok_or("CORE_BUNDLE_MISMATCH: payload directory missing")?;
    if helper.parent() != Some(source) {
        return Err("CORE_BUNDLE_MISMATCH: helper belongs to another payload".into());
    }
    verify_directory(source, selected)?;
    let helper_name = if platform == "win" {
        "polaris-helper.exe"
    } else {
        "polaris-helper"
    };
    if helper.file_name().and_then(|name| name.to_str()) != Some(helper_name) {
        return Err("CORE_BUNDLE_MISMATCH: unexpected helper name".into());
    }
    #[cfg(unix)]
    let owner = {
        use std::os::unix::fs::MetadataExt;
        Some(
            fs::symlink_metadata(source)
                .map_err(|error| error.to_string())?
                .uid(),
        )
    };
    #[cfg(not(unix))]
    let owner = None;
    let objects = polaris_helper::core_objects::PayloadDir::open(source, owner)
        .map_err(|error| error.to_string())?;
    let (file, size) = objects
        .open_regular("core-manifest.json")
        .map_err(|error| error.to_string())?;
    if size > 64 * 1024 {
        return Err("CORE_BUNDLE_MISMATCH: installation manifest too large".into());
    }
    let mut raw = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| error.to_string())?;
    let manifest = InstalledCoreManifest::parse(&raw, polaris_updater::sha256_hex)
        .map_err(|error| error.to_string())?;
    if manifest.id
        != selected
            .id(polaris_updater::sha256_hex)
            .map_err(|error| error.to_string())?
            .as_str()
        || manifest.core != *selected
    {
        return Err("CORE_BUNDLE_MISMATCH: installation selection differs".into());
    }
    let parent = private_stage_parent(config)?;
    let directory = parent.join(uuid::Uuid::new_v4().to_string());
    create_private_directory(&directory)?;
    let stage = InstallStage {
        directory,
        manifest,
    };
    let cleaner = if platform == "win" {
        Some((
            "polaris-helper-setup.exe",
            None,
            selected
                .cleaner_sha256
                .as_deref()
                .ok_or("CORE_BUNDLE_MISMATCH: compiled cleaner hash unavailable")?,
        ))
    } else {
        None
    };
    for (name, expected_bytes, expected_hash) in selected
        .files
        .iter()
        .map(|file| (file.name.as_str(), Some(file.bytes), file.sha256.as_str()))
        .chain(std::iter::once((helper_name, None, expected_helper)))
        .chain(cleaner)
    {
        let (input, bytes) = objects
            .open_regular(name)
            .map_err(|error| error.to_string())?;
        if bytes == 0
            || bytes > 512 * 1024 * 1024
            || expected_bytes.is_some_and(|expected| bytes != expected)
        {
            return Err("CORE_BUNDLE_MISMATCH: installation file size differs".into());
        }
        let path = stage.directory.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        let mut output = polaris_helper::core_store::create_current_user_data_file(&path)
            .map_err(|error| error.to_string())?;
        #[cfg(not(windows))]
        let mut output = options.open(&path).map_err(|error| error.to_string())?;
        let copied = std::io::copy(&mut input.take(bytes + 1), &mut output)
            .map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        if copied != bytes
            || polaris_updater::verify::sha256_reader_hex(
                std::fs::File::open(&path).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
                != expected_hash
        {
            return Err("CORE_BUNDLE_MISMATCH: installation file bytes differ".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(
                    if name == helper_name
                        || name == "polaris-helper-setup.exe"
                        || name.starts_with("sing-box")
                    {
                        0o700
                    } else {
                        0o600
                    },
                ),
            )
            .map_err(|error| error.to_string())?;
        }
    }
    let raw = stage
        .manifest
        .to_json()
        .map_err(|error| error.to_string())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    let mut output = polaris_helper::core_store::create_current_user_data_file(
        &stage.directory.join("core-manifest.json"),
    )
    .map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    let mut output = options
        .open(stage.directory.join("core-manifest.json"))
        .map_err(|error| error.to_string())?;
    output.write_all(&raw).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    Ok(stage)
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        polaris_helper::core_store::create_current_user_data_directory(path)
            .map_err(|_| "CORE_RECOVERY_IO: private staging unavailable".into())
    }
    #[cfg(not(windows))]
    {
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|_| "CORE_RECOVERY_IO: private staging unavailable".into())
    }
}
fn private_stage_parent(config: &Path) -> Result<PathBuf, String> {
    // Hold/open the existing app-owned root; never chmod a foreign/linked parent.
    let _root = open_payload_directory(config)?;
    let parent = config.join("helper-install-stage");
    if !parent
        .try_exists()
        .map_err(|_| "CORE_RECOVERY_IO: staging state unknown")?
    {
        create_private_directory(&parent)?;
    }
    let meta =
        std::fs::symlink_metadata(&parent).map_err(|_| "CORE_RECOVERY_IO: staging unavailable")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("CORE_RECOVERY_IO: invalid staging directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != nix::unistd::getuid().as_raw() || meta.mode() & 0o7777 != 0o700 {
            return Err("CORE_RECOVERY_IO: staging owner or privacy differs".into());
        }
    }
    let _held = open_payload_directory(&parent)?;
    Ok(parent)
}

fn open_payload_directory(dir: &Path) -> Result<polaris_helper::core_objects::PayloadDir, String> {
    #[cfg(unix)]
    let owner = {
        use std::os::unix::fs::MetadataExt;
        Some(
            std::fs::symlink_metadata(dir)
                .map_err(|_| "CORE_BUNDLE_MISMATCH: directory unavailable")?
                .uid(),
        )
    };
    #[cfg(not(unix))]
    let owner = None;
    polaris_helper::core_objects::PayloadDir::open(dir, owner)
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid payload directory".to_owned())
}

// A maintenance tool is intentionally independent of all core files and
// sidecars. Verify the exact open object against compiled selection metadata;
// the existing privileged sealed copy checks this SAME expected pin again.
pub(crate) const MAX_MAINTENANCE_HELPER_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) fn maintenance_utility_from(
    helper: &Path,
    platform: &str,
    selected: &CoreManifest,
) -> Result<polaris_helper_client::MaintenanceUtility, String> {
    let reject = || "CORE_BUNDLE_MISMATCH: untrusted maintenance helper".to_owned();
    if !matches!(platform, "linux" | "mac-arm64" | "mac-x64")
        || selected.platform != platform
        || !helper.is_absolute()
        || helper.file_name().and_then(|name| name.to_str()) != Some("polaris-helper")
    {
        return Err(reject());
    }
    selected.validate().map_err(|_| reject())?;
    let pin = selected.helper_sha256.as_deref().ok_or_else(reject)?;
    if CoreId::parse(pin).is_err() {
        return Err(reject());
    }
    let directory = helper.parent().ok_or_else(reject)?;
    let objects = open_payload_directory(directory)?;
    let (file, bytes) = objects
        .open_regular("polaris-helper")
        .map_err(|_| reject())?;
    if bytes == 0 || bytes > MAX_MAINTENANCE_HELPER_BYTES {
        return Err(reject());
    }
    let digest = polaris_updater::verify::sha256_reader_hex(std::io::Read::take(file, bytes + 1))
        .map_err(|_| reject())?;
    if digest != pin {
        return Err(reject());
    }
    Ok(polaris_helper_client::MaintenanceUtility {
        src_binary: helper.to_path_buf(),
        helper_sha256: pin.to_owned(),
    })
}

/// Hash the same open regular object, not a path reopened after validation.
pub(crate) fn verify_directory(dir: &Path, manifest: &CoreManifest) -> Result<(), String> {
    let objects = open_payload_directory(dir)?;
    manifest
        .validate()
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid manifest")?;
    for expected in &manifest.files {
        let (file, bytes) = objects
            .open_regular(&expected.name)
            .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid payload object")?;
        if bytes != expected.bytes {
            return Err("CORE_BUNDLE_MISMATCH: payload size differs".into());
        }
        let digest = polaris_updater::verify::sha256_reader_hex(std::io::Read::take(
            file,
            expected.bytes + 1,
        ))
        .map_err(|_| "CORE_BUNDLE_MISMATCH: payload read failed")?;
        if digest != expected.sha256 {
            return Err("CORE_BUNDLE_MISMATCH: payload bytes differ".into());
        }
    }
    Ok(())
}

/// A prior full content verification may be reused only as a lightweight
/// invalidation observation, never as permission to launch/install a payload.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct BundleObservation {
    files: Vec<BundleFileObservation>,
}
#[cfg(test)]
#[derive(Clone, Debug)]
struct BundleFileObservation {
    path: PathBuf,
    file: std::sync::Arc<std::fs::File>,
    stamp: String,
    #[cfg(windows)]
    identity: (u64, [u8; 16]),
}
#[cfg(test)]
fn bundle_file_stamp(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.mode(),
            metadata.uid()
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}",
            metadata.creation_time(),
            metadata.last_write_time(),
            metadata.file_size(),
            metadata.file_attributes()
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        format!("{:?}", metadata)
    }
}
#[cfg(test)]
impl BundleObservation {
    fn capture(dir: &Path, manifest: &CoreManifest) -> Result<Self, String> {
        // Linux has a real bundled sidecar. Never open it by a blocking path
        // before validation: reuse the original directory-relative, no-follow,
        // nonblocking regular-file primitive and retain that exact handle.
        #[cfg(target_os = "linux")]
        let objects = open_payload_directory(dir)?;
        let files = manifest
            .files
            .iter()
            .map(|expected| {
                let path = dir.join(&expected.name);
                #[cfg(target_os = "linux")]
                let (file, _) = objects
                    .open_regular(&expected.name)
                    .map_err(|e| e.to_string())?;
                #[cfg(not(target_os = "linux"))]
                let file = {
                    let mut options = std::fs::OpenOptions::new();
                    options.read(true);
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::OpenOptionsExt;
                        options.share_mode(5); // No in-place writes; normal atomic replacement remains possible.
                    }
                    options.open(&path).map_err(|e| e.to_string())?
                };
                let stamp = bundle_file_stamp(&file.metadata().map_err(|e| e.to_string())?);
                #[cfg(windows)]
                let identity =
                    crate::windows_file_id::identity(&file).map_err(|e| e.to_string())?;
                Ok(BundleFileObservation {
                    path,
                    file: std::sync::Arc::new(file),
                    stamp,
                    #[cfg(windows)]
                    identity,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { files })
    }
    #[cfg_attr(
        not(windows),
        allow(
            clippy::needless_bool,
            reason = "the Windows file identity check follows the stamp check"
        )
    )]
    pub(crate) fn is_current(&self) -> bool {
        self.files.iter().all(|observed| {
            if !observed
                .file
                .metadata()
                .is_ok_and(|m| bundle_file_stamp(&m) == observed.stamp)
            {
                return false;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&observed.path) else {
                return false;
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || bundle_file_stamp(&metadata) != observed.stamp
            {
                return false;
            }
            #[cfg(windows)]
            {
                let actual = std::fs::File::open(&observed.path)
                    .and_then(|file| crate::windows_file_id::identity(&file));
                if actual.ok() != Some(observed.identity) {
                    return false;
                }
            }
            true
        })
    }
}

#[cfg(test)]
pub(crate) fn verify_directory_observed(
    dir: &Path,
    manifest: &CoreManifest,
) -> Result<BundleObservation, String> {
    // Bracket the existing full core + sidecar content check with the same
    // retained objects. A replacement during verification cannot become a receipt.
    manifest
        .validate()
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid manifest")?;
    let observation = BundleObservation::capture(dir, manifest)
        .map_err(|reason| format!("CORE_BUNDLE_MISMATCH: cannot observe payload: {reason}"))?;
    verify_directory(dir, manifest)?;
    if !observation.is_current() {
        return Err("CORE_BUNDLE_MISMATCH: payload changed during verification".into());
    }
    Ok(observation)
}

pub(crate) fn verify_bundled(core: &Path) -> Result<std::sync::Arc<AdmittedPayload>, String> {
    acquire_at(core, ExecPurpose::CheckConfig).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;

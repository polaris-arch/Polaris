//! B2b: compiled selected content identity, independent of a writable package manifest.
use std::path::{Path, PathBuf};

use polaris_helper_proto::core_manifest::{CoreFile, CoreId, CoreManifest};

const RELEASE_CONTRACT: &str = include_str!("../../../scripts/desktop-core/release-contract.json");
const COMPILED_INSTALLATION: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/selected-core-manifest.json"));

pub(crate) fn selected_manifest(platform: &str) -> Result<CoreManifest, String> {
    if COMPILED_INSTALLATION != b"null" {
        let selected = polaris_helper_proto::core_manifest::InstalledCoreManifest::parse(
            COMPILED_INSTALLATION,
            polaris_updater::sha256_hex,
        )
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid compiled installation identity")?;
        if selected.core.platform != platform {
            return Err("CORE_BUNDLE_MISMATCH: compiled platform differs".into());
        }
        return Ok(selected.core);
    }
    let contract: serde_json::Value = serde_json::from_str(RELEASE_CONTRACT)
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid compiled identity")?;
    let row = &contract["platforms"][platform];
    let read_file = |value: &serde_json::Value| -> Result<CoreFile, String> {
        Ok(CoreFile {
            name: value["member"]
                .as_str()
                .ok_or("CORE_BUNDLE_MISMATCH: missing member")?
                .to_string(),
            bytes: value["bytes"]
                .as_u64()
                .ok_or("CORE_BUNDLE_MISMATCH: missing size")?,
            sha256: value["sha256"]
                .as_str()
                .ok_or("CORE_BUNDLE_MISMATCH: missing hash")?
                .to_string(),
        })
    };
    let mut files = vec![read_file(&row["core"])?];
    if !row["sidecar"].is_null() {
        files.push(read_file(&row["sidecar"])?);
    }
    let manifest = CoreManifest {
        platform: platform.to_string(),
        version: contract["version"]
            .as_str()
            .ok_or("CORE_BUNDLE_MISMATCH: missing version")?
            .to_string(),
        provider_fingerprint: row["providerFingerprint"]
            .as_str()
            .ok_or("CORE_BUNDLE_MISMATCH: missing provider")?
            .to_string(),
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    manifest
        .validate()
        .map_err(|reason| format!("CORE_BUNDLE_MISMATCH: {reason}"))?;
    Ok(manifest)
}

pub(crate) fn selected_id(platform: &str) -> Result<CoreId, String> {
    selected_manifest(platform)?
        .id(polaris_updater::sha256_hex)
        .map_err(|reason| format!("CORE_BUNDLE_MISMATCH: {reason}"))
}

pub(crate) fn current_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux"),
        ("windows", "x86_64") => Ok("win"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        _ => Err("CORE_BUNDLE_MISMATCH: no desktop selected identity".into()),
    }
}

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
) -> Result<InstallStage, String> {
    use polaris_helper_proto::core_manifest::InstalledCoreManifest;
    use std::fs::{self, OpenOptions};
    use std::io::{Read, Write};
    let platform = current_platform()?;
    let selected = selected_manifest(platform)?;
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
    verify_directory(source, &selected)?;
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
        || manifest.core != selected
    {
        return Err("CORE_BUNDLE_MISMATCH: installation selection differs".into());
    }
    let parent = config.join("helper-install-stage");
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    if fs::symlink_metadata(&parent)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("invalid helper staging directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let directory = parent.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let stage = InstallStage {
        directory,
        manifest,
    };
    let cleaner = if platform == "win" {
        Some((
            "polaris-cleaner.exe",
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
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
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
                        || name == "polaris-cleaner.exe"
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
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.directory.join("core-manifest.json"))
        .map_err(|error| error.to_string())?;
    output.write_all(&raw).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    Ok(stage)
}

/// Hash the same open regular object, not a path reopened after validation.
pub(crate) fn verify_directory(dir: &Path, manifest: &CoreManifest) -> Result<(), String> {
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
    let objects = polaris_helper::core_objects::PayloadDir::open(dir, owner)
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid payload directory")?;
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
#[derive(Clone, Debug)]
pub(crate) struct BundleObservation {
    files: Vec<BundleFileObservation>,
}
#[derive(Clone, Debug)]
struct BundleFileObservation {
    path: PathBuf,
    file: std::sync::Arc<std::fs::File>,
    stamp: String,
    #[cfg(windows)]
    identity: (u64, [u8; 16]),
}
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
impl BundleObservation {
    fn capture(dir: &Path, manifest: &CoreManifest) -> Result<Self, String> {
        let files = manifest
            .files
            .iter()
            .map(|expected| {
                let path = dir.join(&expected.name);
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.share_mode(5); // No in-place writes; normal atomic replacement remains possible.
                }
                let file = options.open(&path).map_err(|e| e.to_string())?;
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

pub(crate) fn verify_bundled(core: &Path) -> Result<BundleObservation, String> {
    let dir = core
        .parent()
        .ok_or("CORE_BUNDLE_MISMATCH: missing directory")?;
    let platform = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("CORE_BUNDLE_MISMATCH: unknown platform")?;
    let manifest = selected_manifest(platform)?;
    if core.file_name().and_then(|name| name.to_str())
        != manifest
            .files
            .iter()
            .find(|file| file.name == "sing-box" || file.name == "sing-box.exe")
            .map(|file| file.name.as_str())
    {
        return Err("CORE_BUNDLE_MISMATCH: unexpected binary name".into());
    }
    verify_directory_observed(dir, &manifest)
}

#[cfg(test)]
mod tests;

//! Explicit recovery input preparation. No execution owner and no bundle writes.
use super::{CoreManifest, InstallStage};
use polaris_helper_proto::core_manifest::InstalledCoreManifest;
use polaris_updater::core_recovery::{stage_core_recovery, CoreRecoveryError, CoreRecoverySpec};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const REINSTALL: &str = "CORE_BUNDLE_MISMATCH: trusted current-app recovery inputs unavailable; reinstall the same trusted app";
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("CORE_RECOVERY_CANCELLED: recovery cancelled".into())
    } else {
        Ok(())
    }
}
pub(super) fn create_private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| "CORE_RECOVERY_IO: private staging unavailable".into())
}
pub(super) fn private_stage_parent(config: &Path) -> Result<PathBuf, String> {
    // Hold/open the existing app-owned root; never chmod a foreign/linked parent.
    let _root = super::open_payload_directory(config)?;
    let parent = config.join("helper-install-stage");
    if !parent
        .try_exists()
        .map_err(|_| "CORE_RECOVERY_IO: staging state unknown")?
    {
        create_private_directory(&parent)?;
    }
    let meta =
        fs::symlink_metadata(&parent).map_err(|_| "CORE_RECOVERY_IO: staging unavailable")?;
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
    let _held = super::open_payload_directory(&parent)?;
    Ok(parent)
}

/// Only explicit install/repair calls this entry. Compiled metadata chooses all
/// hashes; the caller chooses neither an archive nor a different CoreId.
pub(crate) fn stage_recovery_installation(
    helper: &Path,
    config: &Path,
    cancel: &AtomicBool,
) -> Result<InstallStage, String> {
    super::check_compiled_selection().map_err(|_| REINSTALL.to_owned())?;
    let platform = super::current_platform()?;
    let selected = super::selected_manifest(platform).map_err(|_| REINSTALL.to_owned())?;
    let spec = polaris_core_supervisor::selected_core::selected_recovery_spec(platform)
        .and_then(|v| serde_json::from_value(v).map_err(|_| REINSTALL.to_owned()))
        .map_err(|_| REINSTALL.to_owned())?;
    stage_from_bundle(helper, config, &selected, &spec, cancel)
}

fn stage_from_bundle(
    helper: &Path,
    config: &Path,
    selected: &CoreManifest,
    spec: &CoreRecoverySpec,
    cancel: &AtomicBool,
) -> Result<InstallStage, String> {
    cancelled(cancel)?;
    selected.validate().map_err(|_| REINSTALL.to_owned())?;
    let id = selected
        .id(polaris_updater::sha256_hex)
        .map_err(|_| REINSTALL.to_owned())?;
    if spec.core_id != id.as_str() || spec.platform != selected.platform || !helper.is_absolute() {
        return Err(REINSTALL.into());
    }
    let platform_dir = helper.parent().ok_or(REINSTALL)?;
    if platform_dir.file_name().and_then(|s| s.to_str()) != Some(selected.platform.as_str()) {
        return Err(REINSTALL.into());
    }
    let root = platform_dir.parent().ok_or(REINSTALL)?;
    let prefix = format!("core-recovery/{}/", selected.platform);
    let leaf = spec
        .archive
        .resource
        .strip_prefix(&prefix)
        .ok_or(REINSTALL)?;
    if !matches!(
        Path::new(leaf).components().next(),
        Some(Component::Normal(_))
    ) || Path::new(leaf).components().count() != 1
        || leaf.contains(['\\', '\0'])
    {
        return Err(REINSTALL.into());
    }
    let source =
        super::open_payload_directory(&root.join("core-recovery").join(&selected.platform))
            .map_err(|_| REINSTALL.to_owned())?;
    let (mut archive, size) = source
        .open_regular(leaf)
        .map_err(|_| REINSTALL.to_owned())?;
    if size != spec.archive.bytes {
        return Err(REINSTALL.into());
    }
    // The strict backend binds parsing to its own bounded verified byte snapshot.
    let parent = private_stage_parent(config)?;
    let data = stage_core_recovery(&mut archive, spec, &parent, || {
        cancel.load(Ordering::Acquire)
    })
    .map_err(|e| match e {
        CoreRecoveryError::Cancelled => "CORE_RECOVERY_CANCELLED: recovery cancelled".to_owned(),
        CoreRecoveryError::Io(_) => "CORE_RECOVERY_IO: recovery storage failed".to_owned(),
        _ => REINSTALL.to_owned(),
    })?;
    if data.core_id() != id.as_str() {
        return Err(REINSTALL.into());
    }
    let helper_name = if selected.platform == "win" {
        "polaris-helper.exe"
    } else {
        "polaris-helper"
    };
    if helper.file_name().and_then(|n| n.to_str()) != Some(helper_name) {
        return Err(REINSTALL.into());
    }
    let helpers = super::open_payload_directory(platform_dir).map_err(|_| REINSTALL.to_owned())?;
    for (name, hash) in std::iter::once((helper_name, selected.helper_sha256.as_deref())).chain(
        (selected.platform == "win")
            .then_some(("polaris-cleaner.exe", selected.cleaner_sha256.as_deref())),
    ) {
        cancelled(cancel)?;
        let expected = hash.ok_or(REINSTALL)?;
        let (input, bytes) = helpers
            .open_regular(name)
            .map_err(|_| REINSTALL.to_owned())?;
        if bytes == 0 || bytes > 512 * 1024 * 1024 {
            return Err(REINSTALL.into());
        }
        let path = data.directory().join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options
            .open(&path)
            .map_err(|_| "CORE_RECOVERY_IO: staging copy unavailable")?;
        let copied = std::io::copy(&mut input.take(bytes + 1), &mut output)
            .map_err(|_| "CORE_RECOVERY_IO: staging copy failed")?;
        output
            .sync_all()
            .map_err(|_| "CORE_RECOVERY_IO: staging sync failed")?;
        if copied != bytes
            || polaris_updater::verify::sha256_reader_hex(
                fs::File::open(&path).map_err(|_| REINSTALL)?,
            )
            .map_err(|_| REINSTALL)?
                != expected
        {
            return Err(REINSTALL.into());
        }
    }
    cancelled(cancel)?;
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.as_str().into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        core: selected.clone(),
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(data.directory().join("core-manifest.json"))
        .map_err(|_| "CORE_RECOVERY_IO: manifest staging failed")?;
    file.write_all(&manifest.to_json().map_err(|_| REINSTALL)?)
        .map_err(|_| "CORE_RECOVERY_IO: manifest staging failed")?;
    file.sync_all()
        .map_err(|_| "CORE_RECOVERY_IO: manifest sync failed")?;
    // The old installer independently rehashes this complete same-directory
    // core+real sidecars+compiled Helper(+cleaner) into its normal InstallStage.
    let binary = selected
        .files
        .iter()
        .find(|f| f.name == "sing-box" || f.name == "sing-box.exe")
        .ok_or(REINSTALL)?;
    let stage = super::stage_installation(
        &data.directory().join(&binary.name),
        &data.directory().join(helper_name),
        config,
        selected,
    )?;
    cancelled(cancel)?;
    Ok(stage)
}

#[cfg(test)]
pub(super) mod tests;

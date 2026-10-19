//! Ephemeral release admission and byte custody for executable updates.
//! Renderer values and persisted paths never constitute installation authority.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use serde_json::Value;

const RECHECK: &str = "Update selection expired or changed; check for updates again";
const REDOWNLOAD: &str = "Update package is untrusted or changed; check and download again";
const MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
struct Receipt {
    path: PathBuf,
    hash: String,
    bytes: u64,
}

#[derive(Default)]
pub(crate) struct UpdateCustody {
    selected: Mutex<Option<Value>>,
    downloaded: Mutex<Option<Receipt>>,
}

impl UpdateCustody {
    /// Only the backend's successful platform/version selector may call this.
    pub(crate) fn select(&self, info: Value) -> Result<(), String> {
        *self.selected.lock().map_err(|_| RECHECK.to_string())? = Some(info);
        Ok(())
    }

    pub(crate) fn admit(&self, requested: &Value) -> Result<Value, String> {
        let selected = self.selected.lock().map_err(|_| RECHECK.to_string())?;
        let selected = selected.as_ref().ok_or(RECHECK)?;
        // Progress replays intentionally omit title/releaseNotes. Neither is executable identity.
        for key in ["version", "downloadUrl", "fileName", "fileSize", "sha256", "isPrerelease"] {
            if selected.get(key) != requested.get(key) {
                return Err(RECHECK.into());
            }
        }
        let hash = selected.get("sha256").and_then(Value::as_str).ok_or(
            "Release has no trusted SHA-256; use the release page for manual installation",
        )?;
        if !polaris_updater::verify::is_valid_sha256_hex(hash) {
            return Err(RECHECK.into());
        }
        let name = selected.get("fileName").and_then(Value::as_str).ok_or(RECHECK)?;
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(RECHECK.into());
        }
        let url = reqwest::Url::parse(selected.get("downloadUrl").and_then(Value::as_str).ok_or(RECHECK)?)
            .map_err(|_| RECHECK.to_string())?;
        let (owner, repo) = polaris_updater::APP_UPDATE_REPO;
        let asset_prefix = format!("/{owner}/{repo}/releases/download/");
        if url.scheme() != "https" || url.host_str() != Some("github.com")
            || !url.username().is_empty() || url.password().is_some()
            || !url.path().starts_with(&asset_prefix)
        {
            return Err(RECHECK.into());
        }
        Ok(selected.clone())
    }

    pub(crate) fn record(&self, path: &Path, hash: &str, bytes: u64) -> Result<(), String> {
        if bytes == 0 || bytes > MAX_BYTES || !polaris_updater::verify::is_valid_sha256_hex(hash) {
            return Err(REDOWNLOAD.into());
        }
        *self.downloaded.lock().map_err(|_| REDOWNLOAD.to_string())? = Some(Receipt {
            path: path.to_path_buf(), hash: hash.to_owned(), bytes,
        });
        Ok(())
    }

    fn receipt(&self, path: &Path) -> Result<Receipt, String> {
        self.downloaded.lock().map_err(|_| REDOWNLOAD.to_string())?
            .as_ref().filter(|receipt| receipt.path == path).cloned().ok_or_else(|| REDOWNLOAD.into())
    }

    /// Cheap authority check before any advisory or system open. Full byte validation is below.
    pub(crate) fn admits_install(&self, path: &Path) -> Result<(), String> {
        self.receipt(path).map(|_| ())
    }

    /// Freeze the exact checked bytes before stopping the app or invoking the OS.
    /// The same UID can still rewrite private files; this is not a sandbox against that UID.
    pub(crate) fn stage(&self, path: &Path) -> Result<InstallStage, String> {
        let receipt = self.receipt(path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|_| REDOWNLOAD.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(REDOWNLOAD.into());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
        }
        let input = options.open(path).map_err(|_| REDOWNLOAD.to_string())?;
        let opened = input.metadata().map_err(|_| REDOWNLOAD.to_string())?;
        if !opened.is_file() || opened.len() != receipt.bytes {
            return Err(REDOWNLOAD.into());
        }
        let parent = path.parent().ok_or(REDOWNLOAD)?;
        let dir = parent.join(format!("polaris-install-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).map_err(|_| "Cannot create private update stage".to_string())?;
        let stage = InstallStage { file: dir.join(path.file_name().ok_or(REDOWNLOAD)?), dir: Some(dir) };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
        }
        let mut output = options.open(&stage.file).map_err(|_| REDOWNLOAD.to_string())?;
        let copied = std::io::copy(&mut input.take(receipt.bytes + 1), &mut output)
            .map_err(|_| REDOWNLOAD.to_string())?;
        output.sync_all().map_err(|_| REDOWNLOAD.to_string())?;
        drop(output);
        let actual = polaris_updater::verify::sha256_reader_hex(
            std::fs::File::open(&stage.file).map_err(|_| REDOWNLOAD.to_string())?,
        ).map_err(|_| REDOWNLOAD.to_string())?;
        if copied != receipt.bytes || polaris_updater::verify::verify_hex_digest(&actual, &receipt.hash).is_err() {
            return Err(REDOWNLOAD.into());
        }
        Ok(stage)
    }
}

pub(crate) struct InstallStage {
    file: PathBuf,
    dir: Option<PathBuf>,
}
impl InstallStage {
    pub(crate) fn path(&self) -> &Path { &self.file }
    /// Only after successful OS/script handoff: the consumer outlives this command.
    pub(crate) fn hand_off(mut self) { self.dir = None; }
}
impl Drop for InstallStage {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_file(&self.file);
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests;

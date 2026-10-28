//! A helper-owned copy of every read-only object used by an exact launch.
//!
//! The app's plan directory is only an input. The core receives paths in a new directory
//! created by the privileged helper. An unsupported local input fails before the directory is
//! published or any process is started. Runtime-writable state is a separate ownership problem;
//! until a plan binds that owner/generation, configurations containing it are rejected here.

use polaris_config_engine::singbox::SingBoxConfig;
use polaris_helper_proto::ExactBinding;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const MAX_FILES: usize = 512;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_CONFIG_BYTES: usize = 32 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const CLOSURE_VERSION: &[u8] = b"polaris-exact-launch-closure-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    Unsupported,
    InvalidManifest,
    HashMismatch,
    ResourceBudget,
    Io,
}

#[derive(Debug)]
pub struct LaunchSnapshot {
    pub config_path: PathBuf,
    pub executable_path: PathBuf,
    pub launched_config_sha256: String,
    pub launch_closure_digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestFile {
    relative_path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    plan_id: String,
    plan_digest: String,
    generator_version: String,
    config: ManifestFile,
    plan: ManifestFile,
    rule_files: Vec<ManifestFile>,
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':'])
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
}

fn bounded_read(path: &Path, maximum: usize) -> Result<Vec<u8>, SnapshotError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    options.custom_flags(nix::libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(|_| SnapshotError::Io)?;
    let metadata = file.metadata().map_err(|_| SnapshotError::Io)?;
    if !metadata.is_file() {
        return Err(SnapshotError::Unsupported);
    }
    if metadata.len() > maximum as u64 {
        return Err(SnapshotError::ResourceBudget);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SnapshotError::Io)?;
    if bytes.len() > maximum {
        return Err(SnapshotError::ResourceBudget);
    }
    Ok(bytes)
}

fn manifest_bytes(
    root: &Path,
    file: &ManifestFile,
    maximum: usize,
) -> Result<Vec<u8>, SnapshotError> {
    if !safe_relative(&file.relative_path) || file.bytes > maximum as u64 {
        return Err(SnapshotError::InvalidManifest);
    }
    let bytes = bounded_read(&root.join(&file.relative_path), maximum)?;
    if bytes.len() as u64 != file.bytes || hash(&bytes) != file.sha256 {
        return Err(SnapshotError::HashMismatch);
    }
    Ok(bytes)
}

fn inspect_config(bytes: &[u8]) -> Result<SingBoxConfig, SnapshotError> {
    let original: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SnapshotError::Unsupported)?;
    let config: SingBoxConfig =
        serde_json::from_value(original.clone()).map_err(|_| SnapshotError::Unsupported)?;
    if serde_json::to_value(&config).map_err(|_| SnapshotError::Unsupported)? != original
        || config.inbounds.iter().any(|inbound| {
            !matches!(
                inbound.type_field.as_str(),
                "tun" | "mixed" | "http" | "socks" | "direct"
            )
        })
        || config.outbounds.iter().any(|outbound| {
            !outbound.extra.is_empty()
                || !matches!(
                    outbound.type_field.as_str(),
                    "direct"
                        | "block"
                        | "dns"
                        | "selector"
                        | "urltest"
                        | "shadowsocks"
                        | "vless"
                        | "vmess"
                        | "trojan"
                        | "hysteria"
                        | "hysteria2"
                        | "tuic"
                        | "naive"
                        | "snell"
                        | "anytls"
                        | "socks"
                        | "http"
                        | "ssh"
                        | "wireguard"
                )
                || outbound.executable_path.is_some()
                || outbound.data_directory.is_some()
                || outbound.torrc.is_some()
                || outbound.type_field == "tor"
                || outbound.plugin.is_some()
                || outbound.plugin_opts.is_some()
                || outbound.private_key_path.is_some()
        })
        || config
            .endpoints
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|endpoint| {
                !endpoint.extra.is_empty()
                    || endpoint.type_field != "wireguard"
                    || endpoint.state_directory.is_some()
                    || endpoint.taildrop_directory.is_some()
            })
        || config.log.output.is_some()
        || config
            .experimental
            .as_ref()
            .is_some_and(|experimental| experimental.cache_file.is_some())
        || config
            .services
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|service| service.type_field != "api" || service.dashboard.is_some())
        || config.dns.as_ref().is_some_and(|dns| {
            dns.servers.iter().any(|server| {
                !matches!(
                    server.type_field.as_deref(),
                    Some(
                        "https"
                            | "tls"
                            | "udp"
                            | "tcp"
                            | "local"
                            | "fakeip"
                            | "mdns"
                            | "hosts"
                            | "tailscale"
                    )
                ) || server.type_field.as_deref() == Some("hosts") && server.path.is_some()
                    || server.path.is_some()
                        && !matches!(server.type_field.as_deref(), Some("https") | Some("hosts"))
            })
        })
    {
        return Err(SnapshotError::Unsupported);
    }
    Ok(config)
}

fn new_file(path: &Path, bytes: &[u8], executable: bool) -> Result<(), SnapshotError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| SnapshotError::Io)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    options.mode(if executable { 0o500 } else { 0o400 });
    let mut file = options.open(path).map_err(|_| SnapshotError::Io)?;
    use std::io::Write;
    file.write_all(bytes).map_err(|_| SnapshotError::Io)?;
    file.sync_all().map_err(|_| SnapshotError::Io)?;
    Ok(())
}

fn update_field(hasher: &mut Sha256, name: &str, digest: &str) {
    hasher.update((name.len() as u64).to_be_bytes());
    hasher.update(name.as_bytes());
    hasher.update(digest.as_bytes());
}

/// Stage from the published app manifest into a fresh directory. This function is deliberately
/// private: only `stage_protected` may expose a snapshot to a privileged launch path.
fn stage(
    source_config: &Path,
    source_executable: &Path,
    destination: &Path,
    binding: &ExactBinding,
) -> Result<LaunchSnapshot, SnapshotError> {
    if !binding.valid() || destination.exists() || !destination.is_absolute() {
        return Err(SnapshotError::Unsupported);
    }
    let root = source_config.parent().ok_or(SnapshotError::Unsupported)?;
    let manifest_raw = bounded_read(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    if hash(&manifest_raw) != binding.artifact_digest {
        return Err(SnapshotError::HashMismatch);
    }
    let manifest: Manifest =
        serde_json::from_slice(&manifest_raw).map_err(|_| SnapshotError::InvalidManifest)?;
    if manifest.schema_version != 1
        || manifest.plan_id != binding.plan_id
        || manifest.plan_digest != binding.plan_digest
        || manifest.generator_version.is_empty()
        || manifest.generator_version.len() > 128
        || manifest.config.relative_path != "config.json"
        || source_config != root.join("config.json")
        || manifest.plan.relative_path != "plan.json"
        || manifest.rule_files.len() > MAX_FILES
    {
        return Err(SnapshotError::InvalidManifest);
    }
    let config_raw = manifest_bytes(root, &manifest.config, MAX_CONFIG_BYTES)?;
    if hash(&config_raw) != binding.config_sha256 {
        return Err(SnapshotError::HashMismatch);
    }
    let plan = manifest_bytes(root, &manifest.plan, MAX_CONFIG_BYTES)?;
    if hash(&plan) != binding.plan_digest {
        return Err(SnapshotError::HashMismatch);
    }
    let mut config = inspect_config(&config_raw)?;
    let executable = bounded_read(source_executable, MAX_TOTAL_BYTES)?;
    if hash(&executable) != binding.core_sha256 {
        return Err(SnapshotError::HashMismatch);
    }
    // Cronet is loaded beside the executable on Linux/Windows. Include the exact installed
    // object in this generation; macOS embeds it statically. Absence is recorded too.
    let sidecar_name = if cfg!(target_os = "linux") {
        Some("libcronet.so")
    } else if cfg!(windows) {
        Some(crate::core_install::CRONET_DLL_NAME_WIN)
    } else {
        None
    };
    let sidecar = if let Some(name) = sidecar_name {
        let path = source_executable
            .parent()
            .ok_or(SnapshotError::Unsupported)?
            .join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                Some((name, bounded_read(&path, MAX_TOTAL_BYTES)?))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Ok(_) => return Err(SnapshotError::Unsupported),
            Err(_) => return Err(SnapshotError::Io),
        }
    } else {
        None
    };
    let mut rule_bytes = BTreeMap::new();
    let mut total = config_raw
        .len()
        .checked_add(plan.len())
        .and_then(|size| size.checked_add(executable.len()))
        .ok_or(SnapshotError::ResourceBudget)?;
    if let Some((_, bytes)) = &sidecar {
        total = total
            .checked_add(bytes.len())
            .ok_or(SnapshotError::ResourceBudget)?;
        if total > MAX_TOTAL_BYTES {
            return Err(SnapshotError::ResourceBudget);
        }
    }
    for entry in &manifest.rule_files {
        if !entry.relative_path.starts_with("rules/")
            || entry.relative_path["rules/".len()..].contains('/')
            || rule_bytes.contains_key(&entry.relative_path)
        {
            return Err(SnapshotError::InvalidManifest);
        }
        let bytes = manifest_bytes(root, entry, MAX_TOTAL_BYTES)?;
        total = total
            .checked_add(bytes.len())
            .ok_or(SnapshotError::ResourceBudget)?;
        if total > MAX_TOTAL_BYTES {
            return Err(SnapshotError::ResourceBudget);
        }
        rule_bytes.insert(entry.relative_path.clone(), bytes);
    }
    let mut referenced = BTreeSet::new();
    if let Some(route) = &mut config.route {
        for rule in route.rule_set.as_deref_mut().unwrap_or_default() {
            if rule.type_field != "local"
                || !matches!(rule.format.as_str(), "source" | "binary")
                || rule.url.is_some()
            {
                return Err(SnapshotError::Unsupported);
            }
            let original = rule.path.as_deref().ok_or(SnapshotError::Unsupported)?;
            let relative = Path::new(original)
                .strip_prefix(root)
                .map_err(|_| SnapshotError::Unsupported)?;
            if relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(SnapshotError::Unsupported);
            }
            let name = relative.to_str().ok_or(SnapshotError::Unsupported)?;
            if !rule_bytes.contains_key(name) || !referenced.insert(name.to_owned()) {
                return Err(SnapshotError::Unsupported);
            }
            rule.path = Some(destination.join(name).to_string_lossy().into_owned());
        }
    }
    if referenced.len() != rule_bytes.len() {
        return Err(SnapshotError::Unsupported);
    }
    let launched = serde_json::to_vec_pretty(&config).map_err(|_| SnapshotError::Unsupported)?;
    let launched_hash = hash(&launched);
    let executable_name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    // Source checks above finish before the first destination mutation. A failed write leaves a
    // private, unpublished directory; a fresh UUID must be used on retry.
    fs::create_dir(destination).map_err(|_| SnapshotError::Io)?;
    new_file(&destination.join(executable_name), &executable, true)?;
    if let Some((name, bytes)) = &sidecar {
        new_file(&destination.join(name), bytes, false)?;
    }
    for (name, bytes) in &rule_bytes {
        new_file(&destination.join(name), bytes, false)?;
    }
    new_file(&destination.join("config.json"), &launched, false)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if !rule_bytes.is_empty() {
            fs::set_permissions(destination.join("rules"), fs::Permissions::from_mode(0o500))
                .map_err(|_| SnapshotError::Io)?;
        }
        fs::set_permissions(destination, fs::Permissions::from_mode(0o500))
            .map_err(|_| SnapshotError::Io)?;
    }
    let mut closure = Sha256::new();
    closure.update(CLOSURE_VERSION);
    update_field(&mut closure, "source-config", &binding.config_sha256);
    update_field(&mut closure, "manifest", &binding.artifact_digest);
    update_field(&mut closure, "launch-config", &launched_hash);
    update_field(&mut closure, executable_name, &binding.core_sha256);
    if let Some(name) = sidecar_name {
        update_field(
            &mut closure,
            name,
            &sidecar
                .as_ref()
                .map_or_else(|| "absent".into(), |(_, bytes)| hash(bytes)),
        );
    }
    for (name, bytes) in &rule_bytes {
        update_field(&mut closure, name, &hash(bytes));
    }
    Ok(LaunchSnapshot {
        config_path: destination.join("config.json"),
        executable_path: destination.join(executable_name),
        launched_config_sha256: launched_hash,
        launch_closure_digest: hex::encode(closure.finalize()),
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn root_owned_chain(path: &Path) -> Result<(), SnapshotError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(SnapshotError::Unsupported);
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| SnapshotError::Io)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(SnapshotError::Unsupported);
        }
    }
    Ok(())
}

/// Entrypoint for a Unix root daemon. `protected_root` is installed outside the app's writable
/// tree; each ancestor and the executable object must still be root-owned and non-writable to
/// the app at call time. Linux must grant its dropped UID read/execute ACLs after this returns;
/// macOS runs the child as root. The public wire remains disabled until that launch path exists.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn stage_protected(
    source_config: &Path,
    source_executable: &Path,
    protected_root: &Path,
    destination: &Path,
    binding: &ExactBinding,
) -> Result<LaunchSnapshot, SnapshotError> {
    if nix::unistd::geteuid().as_raw() != 0 || destination.parent() != Some(protected_root) {
        return Err(SnapshotError::Unsupported);
    }
    root_owned_chain(protected_root)?;
    root_owned_chain(
        source_executable
            .parent()
            .ok_or(SnapshotError::Unsupported)?,
    )?;
    let executable = fs::symlink_metadata(source_executable).map_err(|_| SnapshotError::Io)?;
    if !executable.is_file()
        || executable.file_type().is_symlink()
        || executable.uid() != 0
        || executable.permissions().mode() & 0o022 != 0
    {
        return Err(SnapshotError::Unsupported);
    }
    stage(source_config, source_executable, destination, binding)
}

/// Linux runs the core as the authenticated peer UID. Give it search/read/execute access to the
/// root-owned snapshot without ever making that UID an owner or granting write/delete rights.
/// A missing `setfacl` is an unsupported platform capability, never a reason to chown the tree.
#[cfg(target_os = "linux")]
pub fn grant_linux_reader(snapshot: &LaunchSnapshot, uid: u32) -> Result<(), SnapshotError> {
    let directory = snapshot
        .config_path
        .parent()
        .ok_or(SnapshotError::Unsupported)?;
    let protected_root = directory.parent().ok_or(SnapshotError::Unsupported)?;
    if nix::unistd::geteuid().as_raw() != 0 || snapshot.executable_path.parent() != Some(directory)
    {
        return Err(SnapshotError::Unsupported);
    }
    root_owned_chain(protected_root)?;
    let grant = |path: &Path, rights: &str| -> Result<(), SnapshotError> {
        let status = std::process::Command::new("/usr/bin/setfacl")
            .env_clear()
            .arg("--modify")
            .arg(format!("u:{uid}:{rights}"))
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| SnapshotError::Unsupported)?;
        if !status.success() {
            return Err(SnapshotError::Unsupported);
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| SnapshotError::Io)?;
        if metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(SnapshotError::Unsupported);
        }
        Ok(())
    };
    grant(protected_root, "--x")?;
    grant(directory, "--x")?;
    grant(&snapshot.config_path, "r--")?;
    grant(&snapshot.executable_path, "r-x")?;
    let sidecar = directory.join("libcronet.so");
    if sidecar.exists() {
        grant(&sidecar, "r--")?;
    }
    let rules = directory.join("rules");
    if rules.exists() {
        grant(&rules, "--x")?;
        for entry in fs::read_dir(rules).map_err(|_| SnapshotError::Io)? {
            let entry = entry.map_err(|_| SnapshotError::Io)?;
            let metadata = entry.metadata().map_err(|_| SnapshotError::Io)?;
            if !metadata.is_file() {
                return Err(SnapshotError::Unsupported);
            }
            grant(&entry.path(), "r--")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

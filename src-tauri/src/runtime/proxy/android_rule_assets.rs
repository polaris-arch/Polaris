use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use polaris_config_engine::builder::endpoint_routes::TAILNET_RULES_DIR_NAME;
use serde_json::Value;

const GENERATIONS_DIR: &str = "android-rule-generations";
const CUSTOM_RULES_DIR: &str = "custom-rules";
const MAX_SAVED_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

pub(super) struct AndroidRuleAssets {
    directory: PathBuf,
    pub(super) custom: PathBuf,
    pub(super) tailnet: PathBuf,
    tailnet_history: BTreeSet<PathBuf>,
    dispatched: bool,
}

impl AndroidRuleAssets {
    /// The caller holds the exclusive, unlaunched Android main reservation.
    pub(super) fn new(config_dir: &Path, saved: &Path, run_id: &str) -> io::Result<Self> {
        if !valid_run_id(OsStr::new(run_id)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid Android generation",
            ));
        }
        let generations = config_dir.join(GENERATIONS_DIR);
        let mkdir = fs::DirBuilder::new();
        #[cfg(unix)]
        let mkdir = {
            use std::os::unix::fs::DirBuilderExt;
            let mut mkdir = mkdir;
            mkdir.mode(0o700);
            mkdir
        };
        match mkdir.create(&generations) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&generations)?.is_dir() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        let directory = generations.join(run_id);
        mkdir.create(&directory)?;
        let tailnet_history = saved_rule_sources(saved, &generations)
            .map(|sources| sources.tailnet_files)
            .unwrap_or_default();
        Ok(Self {
            custom: directory.join(CUSTOM_RULES_DIR),
            tailnet: directory.join(TAILNET_RULES_DIR_NAME),
            directory,
            tailnet_history,
            dispatched: false,
        })
    }

    pub(super) fn tailnet_history_file(&self, name: &str) -> Option<PathBuf> {
        let directory = self.tailnet_history.first()?.parent()?;
        let file = self.tailnet_history.get(&directory.join(name))?;
        (real_directory(directory)
            && directory.parent().is_some_and(real_directory)
            && fs::symlink_metadata(file).is_ok_and(|entry| entry.is_file()))
        .then(|| file.clone())
    }

    pub(super) fn sync_directories(&self) -> io::Result<()> {
        // Atomic file writes sync their own directories. Make the new ancestors
        // durable before the Kotlin owner publishes JSON that refers to them.
        #[cfg(unix)]
        for directory in self.directory.ancestors().take(3) {
            fs::File::open(directory)?.sync_all()?;
        }
        Ok(())
    }

    pub(super) fn set_dispatched(&mut self, dispatched: bool) {
        self.dispatched = dispatched;
    }

    /// Call only after the exact Android Ready receipt; uncertain JSON retains all.
    pub(super) fn reclaim_retired(&self, saved: &Path) -> io::Result<()> {
        let generations = self
            .directory
            .parent()
            .expect("rule generation has a parent");
        if !fs::symlink_metadata(generations)?.is_dir() {
            return Ok(());
        }
        let Some(sources) = saved_rule_sources(saved, generations) else {
            return Ok(());
        };
        for entry in fs::read_dir(generations)? {
            let entry = entry?;
            let name = entry.file_name();
            if self.directory.file_name() != Some(name.as_os_str())
                && valid_run_id(&name)
                && !sources.generations.contains(&name)
                && entry.file_type()?.is_dir()
            {
                // remove_dir_all does not follow symlinks. This direct child
                // was created in the private generation directory namespace.
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

impl Drop for AndroidRuleAssets {
    fn drop(&mut self) {
        if !self.dispatched
            && self.directory.parent().is_some_and(real_directory)
            && real_directory(&self.directory)
        {
            // This exact directory was created exclusively for a request that
            // never reached native launch. The bridge preparation is still held.
            if let Err(error) = fs::remove_dir_all(&self.directory) {
                log::warn!("Android 未启动的规则目录清理失败：{error}");
            }
        }
    }
}

fn real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|entry| entry.is_dir())
}

fn valid_run_id(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix("legacy-"))
        .is_some_and(|suffix| {
            suffix.len() == 24
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

#[derive(Default)]
struct SavedRuleSources {
    generations: BTreeSet<OsString>,
    tailnet_files: BTreeSet<PathBuf>,
}

fn saved_rule_sources(saved: &Path, generations: &Path) -> Option<SavedRuleSources> {
    let legacy_tailnet = generations.parent()?.join(TAILNET_RULES_DIR_NAME);
    let metadata = fs::symlink_metadata(saved).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SAVED_CONFIG_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(saved)
        .ok()?
        .take(MAX_SAVED_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_SAVED_CONFIG_BYTES {
        return None;
    }
    let config: Value = serde_json::from_slice(&bytes).ok()?;
    let mut sources = SavedRuleSources::default();
    let Some(route) = config.as_object()?.get("route") else {
        return Some(sources);
    };
    let Some(rule_sets) = route.as_object()?.get("rule_set") else {
        return Some(sources);
    };
    for set in rule_sets.as_array()? {
        if set.as_object()?.get("type")?.as_str()? != "local" {
            continue;
        }
        let path = Path::new(set.get("path")?.as_str()?);
        if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
            return None;
        }
        let Ok(relative) = path.strip_prefix(generations) else {
            if path.parent() == Some(legacy_tailnet.as_path()) {
                sources.tailnet_files.insert(path.to_path_buf());
            }
            continue;
        };
        let mut parts = relative.components();
        let (
            Some(Component::Normal(id)),
            Some(Component::Normal(kind)),
            Some(Component::Normal(_)),
            None,
        ) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if !valid_run_id(id) || (kind != CUSTOM_RULES_DIR && kind != TAILNET_RULES_DIR_NAME) {
            return None;
        }
        sources.generations.insert(id.to_owned());
        if kind == TAILNET_RULES_DIR_NAME {
            sources.tailnet_files.insert(path.to_path_buf());
        }
    }
    // Current runtime JSON can name many Tailnet files, all from one directory.
    let mut tailnet = sources.tailnet_files.iter();
    if let Some(first) = tailnet.next() {
        if tailnet.any(|file| file.parent() != first.parent()) {
            return None;
        }
    }
    Some(sources)
}

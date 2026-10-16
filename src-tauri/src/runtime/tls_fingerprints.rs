//! Actual selected CLI / loaded libbox capabilities. No manifest-version inference.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

const MAX_DOCUMENT: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Profile {
    pub id: String,
    pub display_name: String,
    pub client: String,
    pub template_version: String,
    pub selection: String,
    #[serde(rename = "templateOffersX25519MLKEM768")]
    pub template_offers_x25519_mlkem768: Option<bool>,
}

/// Preserve source/module/modified/Go evidence verbatim, including unknowns.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Capabilities {
    pub schema: String,
    pub enabled: bool,
    pub core_version: String,
    pub source_commit: String,
    pub source_modified: Option<bool>,
    pub go_version: String,
    pub required_go_version: String,
    pub build_tags: String,
    pub os: String,
    pub arch: String,
    pub modules: Vec<Module>,
    pub default_fingerprint: String,
    pub profiles: Vec<Profile>,
    pub legacy_chrome_aliases: Vec<String>,
    #[serde(rename = "shadowTLSSessionIDWithECHSupported")]
    pub shadow_tls_session_id_with_ech_supported: bool,
    pub curve_preferences_supported: bool,
    #[serde(rename = "realityX25519MLKEM768")]
    pub reality_x25519_mlkem768: String,
    pub random_semantics: String,
    pub randomized_semantics: String,
    pub negotiation_evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Module {
    path: String,
    version: String,
    sum: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    replace: Option<Box<Module>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    /// CLI random selection belongs to the probe process, not the running main.
    pub observation_scope: String,
    pub byte_identity_scope: String,
    /// Hash of executable bytes + capability document; never a version label.
    pub identity: Option<String>,
    pub binary_sha256: Option<String>,
    pub document: Option<Capabilities>,
    pub unavailable_reason: Option<String>,
}

impl Snapshot {
    pub(crate) fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            observation_scope: "unavailable".into(),
            byte_identity_scope: "unavailable".into(),
            identity: None,
            binary_sha256: None,
            document: None,
            unavailable_reason: Some(reason.into()),
        }
    }
    pub(crate) fn from_native(raw: Option<&str>, binary_sha256: &str, scope: &str) -> Self {
        if binary_sha256.len() != 64
            || !binary_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Self::unavailable("Loaded core byte identity is invalid");
        }
        let mut snapshot = match raw {
            Some(raw) => Self::from_loaded(raw, binary_sha256).unwrap_or_else(Self::unavailable),
            None => Self::unavailable("Loaded core has no TLS capability report"),
        };
        snapshot.binary_sha256 = Some(binary_sha256.into());
        snapshot.byte_identity_scope = scope.into();
        snapshot
    }

    pub(crate) fn from_loaded(raw: &str, binary_sha256: &str) -> Result<Self, String> {
        if raw.len() > MAX_DOCUMENT
            || binary_sha256.len() != 64
            || !binary_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("Invalid loaded-core capability identity".into());
        }
        let raw_value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        let required = [
            "schema",
            "enabled",
            "coreVersion",
            "sourceCommit",
            "sourceModified",
            "goVersion",
            "requiredGoVersion",
            "buildTags",
            "os",
            "arch",
            "modules",
            "defaultFingerprint",
            "profiles",
            "legacyChromeAliases",
            "shadowTLSSessionIDWithECHSupported",
            "curvePreferencesSupported",
            "realityX25519MLKEM768",
            "randomSemantics",
            "randomizedSemantics",
            "negotiationEvidence",
        ];
        if required.iter().any(|key| raw_value.get(key).is_none())
            || raw_value["profiles"].as_array().is_some_and(|profiles| {
                profiles
                    .iter()
                    .any(|p| p.get("templateOffersX25519MLKEM768").is_none())
            })
        {
            return Err("Incomplete TLS capability report".into());
        }
        let doc: Capabilities =
            serde_json::from_str(raw).map_err(|e| format!("Invalid TLS capability report: {e}"))?;
        let ids: BTreeSet<&str> = doc.profiles.iter().map(|p| p.id.as_str()).collect();
        let aliases: BTreeSet<&str> = doc
            .legacy_chrome_aliases
            .iter()
            .map(String::as_str)
            .collect();
        if doc.schema != "polaris-tls-fingerprint-capabilities-v1"
            || doc.profiles.len() > 16
            || doc.modules.len() > 2
            || ids.len() != doc.profiles.len()
            || aliases.len() != doc.legacy_chrome_aliases.len()
            || doc.legacy_chrome_aliases.len() > 5
            || doc.shadow_tls_session_id_with_ech_supported
            || doc.curve_preferences_supported
            || (!doc.enabled && (!ids.is_empty() || !aliases.is_empty()))
            || (!doc.source_commit.is_empty()
                && (doc.source_commit.len() != 40
                    || !doc
                        .source_commit
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())))
            || doc.profiles.iter().any(|p| {
                !polaris_config_engine::user_config::utls::SUPPORTED_PROFILES
                    .contains(&p.id.as_str())
                    || [
                        "chrome_psk",
                        "chrome_psk_shuffle",
                        "chrome_padding_psk_shuffle",
                        "chrome_pq",
                        "chrome_pq_psk",
                    ]
                    .contains(&p.id.as_str())
                    || !["profile", "process-browser", "process-seeded-generator"]
                        .contains(&p.selection.as_str())
            })
            || aliases.iter().any(|id| {
                ![
                    "chrome_psk",
                    "chrome_psk_shuffle",
                    "chrome_padding_psk_shuffle",
                    "chrome_pq",
                    "chrome_pq_psk",
                ]
                .contains(id)
            })
        {
            return Err("Unsupported TLS capability schema or inventory".into());
        }
        let identity = polaris_updater::sha256_hex(
            &serde_json::to_vec(&(binary_sha256, &doc)).map_err(|e| e.to_string())?,
        );
        Ok(Self {
            observation_scope: "loaded-libbox".into(),
            byte_identity_scope: "loaded-native-container".into(),
            identity: Some(identity),
            binary_sha256: Some(binary_sha256.into()),
            document: Some(doc),
            unavailable_reason: None,
        })
    }

    /// Check only generated uTLS fields; QUIC/custom TLS policies stay untouched.
    pub(crate) fn validate_config(&self, value: &Value) -> Result<(), String> {
        let Some(doc) = &self.document else {
            // Old cores retain their existing settings and go through real check.
            return Ok(());
        };
        fn visit(value: &Value, doc: &Capabilities) -> Result<(), String> {
            match value {
                Value::Object(map) => {
                    if let Some(tls) = map.get("tls").and_then(Value::as_object) {
                        if let Some(utls) = tls.get("utls") {
                            if utls["enabled"].as_bool() == Some(true) {
                                let id = utls["fingerprint"].as_str().unwrap_or("");
                                if !doc.enabled
                                    || (!id.is_empty()
                                        && !doc.profiles.iter().any(|p| p.id == id)
                                        && !doc.legacy_chrome_aliases.iter().any(|a| a == id))
                                {
                                    return Err(format!("The selected core does not support uTLS fingerprint {id:?}. Choose a supported fingerprint or the core default."));
                                }
                            }
                        }
                    }
                    for child in map.values() {
                        visit(child, doc)?;
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        visit(child, doc)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        visit(value, doc)
    }
}

#[cfg(test)]
pub(crate) fn binary_digest(path: &Path) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    polaris_updater::verify::sha256_reader_hex(f).map_err(|e| e.to_string())
}

/// One bounded shared observation per file identity. The open Windows handle
/// excludes in-place writes but permits atomic replacement; Unix also checks
/// device/inode/ctime, not just mtime.
#[derive(Clone, Debug)]
pub(crate) struct CliObservation {
    pub snapshot: Snapshot,
    path: std::path::PathBuf,
    file: std::sync::Arc<std::fs::File>,
    stamp: String,
    #[cfg(windows)]
    file_id: (u64, [u8; 16]),
}

fn file_stamp(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        format!(
            "{}:{}:{}",
            metadata.creation_time(),
            metadata.last_write_time(),
            metadata.file_size()
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        format!("{:?}", metadata)
    }
}
impl CliObservation {
    pub(crate) fn matches(&self, path: &Path) -> bool {
        #[cfg(windows)]
        if std::fs::File::open(path)
            .and_then(|file| crate::windows_file_id::identity(&file))
            .ok()
            != Some(self.file_id)
        {
            return false;
        }
        path == self.path
            && self
                .file
                .metadata()
                .is_ok_and(|m| file_stamp(&m) == self.stamp)
            && std::fs::metadata(path).is_ok_and(|m| file_stamp(&m) == self.stamp)
    }
    pub(crate) fn still_current(&self) -> bool {
        self.matches(&self.path)
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
static CLI_CACHE: std::sync::Mutex<Vec<CliObservation>> = std::sync::Mutex::new(Vec::new());
static CLI_QUERY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn cached_cli(path: &Path) -> Option<CliObservation> {
    CLI_CACHE
        .lock()
        .ok()?
        .iter()
        .find(|entry| entry.matches(path))
        .cloned()
}
pub(crate) fn cached_digest_matches(path: &Path, expected: &str) -> bool {
    cached_cli(path).is_some_and(|entry| entry.snapshot.binary_sha256.as_deref() == Some(expected))
}

async fn open_observation(path: &Path) -> Result<CliObservation, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(5); // READ | DELETE: no in-place writes; allow normal atomic updater replacement.
        }
        let file = options.open(&path).map_err(|e| e.to_string())?;
        let stamp = file_stamp(&file.metadata().map_err(|e| e.to_string())?);
        let digest = polaris_updater::verify::sha256_reader_hex(
            file.try_clone().map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut snapshot = Snapshot::unavailable("Core has no TLS capability report");
        snapshot.binary_sha256 = Some(digest);
        snapshot.byte_identity_scope = "cli-executable".into();
        #[cfg(windows)]
        let file_id = crate::windows_file_id::identity(&file).map_err(|e| e.to_string())?;
        let entry = CliObservation {
            snapshot,
            path,
            file: std::sync::Arc::new(file),
            stamp,
            #[cfg(windows)]
            file_id,
        };
        if !entry.still_current() {
            return Err("Core file identity changed while hashing".into());
        }
        Ok(entry)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn probe_cli(path: &Path) -> Snapshot {
    if let Some(entry) = cached_cli(path) {
        return entry.snapshot;
    }
    let _query = CLI_QUERY.lock().await;
    if let Some(entry) = cached_cli(path) {
        return entry.snapshot;
    }
    let mut entry = match open_observation(path).await {
        Ok(entry) => entry,
        Err(reason) => return Snapshot::unavailable(reason),
    };
    use std::io::{Read, Seek};
    let mut magic = [0_u8; 2];
    let wrapper = entry.file.try_clone().is_ok_and(|mut file| {
        file.rewind().is_ok() && file.read_exact(&mut magic).is_ok() && &magic == b"#!"
    });
    entry.snapshot = if wrapper {
        Snapshot {
            binary_sha256: entry.snapshot.binary_sha256.clone(),
            byte_identity_scope: "cli-executable".into(),
            ..Snapshot::unavailable("Native-core capabilities unavailable for a script wrapper")
        }
    } else {
        query_observation(&entry).await
    };
    if !entry.still_current() {
        return Snapshot::unavailable("Core changed during capability query");
    }
    if let Ok(mut cache) = CLI_CACHE.lock() {
        cache.retain(|old| old.path != path);
        if cache.len() >= 4 {
            cache.remove(0);
        }
        cache.push(entry.clone());
    }
    entry.snapshot
}

#[cfg(test)]
async fn query_cli(path: &Path) -> Snapshot {
    match open_observation(path).await {
        Ok(entry) => query_observation(&entry).await,
        Err(reason) => Snapshot::unavailable(reason),
    }
}

async fn query_observation(entry: &CliObservation) -> Snapshot {
    let path = entry.path();
    let mut observed_digest = None;
    let result = async {
        if !entry.still_current() {
            return Err("Core file identity changed before query".into());
        }
        let before = entry
            .snapshot
            .binary_sha256
            .clone()
            .ok_or("Missing core digest")?;
        observed_digest = Some(before.clone());
        let result = polaris_core_supervisor::run_tls_fingerprint_query(
            path,
            std::time::Duration::from_secs(5),
        )
        .await
        .map_err(|e| e.to_string())?;
        if !entry.still_current() {
            observed_digest = None;
            return Err("Core bytes changed during capability query".into());
        }
        let raw = match result {
            polaris_core_supervisor::RawCheck::Done {
                success: true,
                stdout,
                ..
            } => stdout,
            other => {
                return Err(format!(
                    "Selected core TLS capability query unavailable: {other:?}"
                ))
            }
        };
        let mut snapshot = Snapshot::from_loaded(&raw, &before)?;
        snapshot.observation_scope = "cli-probe-process".into();
        snapshot.byte_identity_scope = "cli-executable".into();
        Ok(snapshot)
    };
    match result.await {
        Ok(snapshot) => snapshot,
        Err(reason) => Snapshot {
            binary_sha256: observed_digest,
            byte_identity_scope: "cli-executable".into(),
            ..Snapshot::unavailable(reason)
        },
    }
}

#[cfg(test)]
mod tests;

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

pub(crate) fn binary_digest(path: &Path) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    polaris_updater::verify::sha256_reader_hex(f).map_err(|e| e.to_string())
}

/// Fresh hash before/after a bounded query. No mtime-only or version-only cache.
pub(crate) async fn probe_cli(path: &Path) -> Snapshot {
    // A script/wrapper's hash does not identify the native core it may execute.
    // It also must not be run as a speculative capability query.
    use std::io::Read;
    let mut magic = [0_u8; 2];
    if std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"#!"
    {
        return Snapshot {
            binary_sha256: binary_digest(path).ok(),
            ..Snapshot::unavailable("Native-core capabilities unavailable for a script wrapper")
        };
    }
    query_cli(path).await
}

async fn digest_off_thread(path: &Path) -> Result<String, String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || binary_digest(&path))
        .await
        .map_err(|e| e.to_string())?
}

async fn query_cli(path: &Path) -> Snapshot {
    use tokio::io::AsyncReadExt;
    let mut observed_digest = None;
    let mut command = tokio::process::Command::new(path);
    command
        .args(["tools", "tls-fingerprints"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let result = async {
        let before = digest_off_thread(path).await?;
        observed_digest = Some(before.clone());
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("Missing capability stdout")?
            .take((MAX_DOCUMENT + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_DOCUMENT {
            return Err("TLS capability report exceeds budget".into());
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        let after = match digest_off_thread(path).await {
            Ok(after) => after,
            Err(reason) => {
                observed_digest = None;
                return Err(reason);
            }
        };
        if after != before {
            observed_digest = None;
            return Err("Core bytes changed during capability query".into());
        }
        if !status.success() {
            return Err("Selected core has no TLS capability query".into());
        }
        let raw = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let mut snapshot = Snapshot::from_loaded(raw, &before)?;
        snapshot.observation_scope = "cli-probe-process".into();
        snapshot.byte_identity_scope = "cli-executable".into();
        Ok(snapshot)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), result).await {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(reason)) => Snapshot {
            binary_sha256: observed_digest,
            byte_identity_scope: "cli-executable".into(),
            ..Snapshot::unavailable(reason)
        },
        Err(_) => Snapshot {
            ..Snapshot::unavailable("TLS capability query timed out")
        },
    }
}

#[cfg(test)]
mod tests;

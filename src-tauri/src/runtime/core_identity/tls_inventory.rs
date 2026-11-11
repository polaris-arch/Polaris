//! Source-reviewed support for the expected compiled selection. This data does
//! not observe installed files, a running process, execution custody or a TLS handshake.
//! Build identity comes from the compiled release contract; the catalog holds only
//! the stable support table reviewed from `common/tls/fingerprint_profiles.go`.
use polaris_helper_proto::core_manifest::CoreManifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::OnceLock;

const CATALOG: &str = include_str!("../../../../scripts/desktop-core/tls-support-catalog.json");
const MAX_CATALOG: usize = 64 * 1024;
const KIND: &str = "compiled-selected-support-v1";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct CorePin {
    member: String,
    bytes: u64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    schema: String,
    stable_support: StableSupport,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StableSupport {
    default_fingerprint: String,
    profiles: Vec<StableProfile>,
    legacy_chrome_aliases: Vec<String>,
    random_semantics: String,
    randomized_semantics: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StableProfile {
    id: String,
    display_name: String,
    selection: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupportDirectory {
    evidence_kind: &'static str,
    selected_spec: Option<SelectedSpec>,
    document: Option<SupportDocument>,
    unavailable_reason: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectedSpec {
    platform: String,
    target: String,
    source_commit: String,
    source_tree: String,
    version: String,
    contract_sha256: String,
    core_bytes: u64,
    core_sha256: String,
    data_origin: &'static str,
    observation_stdout_sha256: Option<String>,
    source_archive_sha256: String,
    source_index_sha256: String,
    go_version: String,
    required_go_version: String,
    build_tags: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportDocument {
    schema: &'static str,
    enabled: bool,
    os: String,
    arch: String,
    default_fingerprint: String,
    profiles: Vec<SupportProfile>,
    legacy_chrome_aliases: Vec<String>,
    random_semantics: String,
    randomized_semantics: String,
    negotiation_evidence: &'static str,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportProfile {
    id: String,
    display_name: String,
    selection: String,
    client: Option<String>,
    template_version: Option<String>,
    #[serde(rename = "templateOffersX25519MLKEM768")]
    template_offers_x25519_mlkem768: Option<bool>,
}
impl SupportDirectory {
    fn unavailable(reason: String) -> Self {
        Self {
            evidence_kind: KIND,
            selected_spec: None,
            document: None,
            unavailable_reason: Some(reason),
        }
    }
}
fn reject() -> String {
    "Selected TLS support directory is unavailable".into()
}
fn hex(value: &str, count: usize) -> bool {
    value.len() == count
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_stable(support: &StableSupport) -> Result<(), String> {
    let ids: BTreeSet<&str> = support.profiles.iter().map(|p| p.id.as_str()).collect();
    let aliases: BTreeSet<&str> = support
        .legacy_chrome_aliases
        .iter()
        .map(String::as_str)
        .collect();
    let expected_aliases: BTreeSet<&str> = [
        "chrome_psk",
        "chrome_psk_shuffle",
        "chrome_padding_psk_shuffle",
        "chrome_pq",
        "chrome_pq_psk",
    ]
    .into_iter()
    .collect();
    let all: BTreeSet<&str> = ids.union(&aliases).copied().collect();
    if support.default_fingerprint != "chrome"
        || support.profiles.len() > 16
        || ids.len() != support.profiles.len()
        || aliases != expected_aliases
        || aliases.len() != support.legacy_chrome_aliases.len()
        || !ids.is_disjoint(&aliases)
        || all
            != polaris_config_engine::user_config::utls::SUPPORTED_PROFILES
                .iter()
                .copied()
                .collect()
        || support.random_semantics.is_empty()
        || support.randomized_semantics.is_empty()
    {
        return Err(reject());
    }
    for profile in &support.profiles {
        let selection = match profile.id.as_str() {
            "random" => "process-browser",
            "randomized" => "process-seeded-generator",
            _ => "profile",
        };
        if profile.selection != selection || profile.display_name.is_empty() {
            return Err(reject());
        }
    }
    Ok(())
}
fn directory_for(
    raw: &str,
    contract_raw: &str,
    platform: &str,
    selected: &CoreManifest,
) -> Result<SupportDirectory, String> {
    if raw.len() > MAX_CATALOG {
        return Err(reject());
    }
    let catalog: Catalog = serde_json::from_str(raw).map_err(|_| reject())?;
    let contract: Value = serde_json::from_str(contract_raw).map_err(|_| reject())?;
    let row = &contract["platforms"][platform];
    let core: CorePin = serde_json::from_value(row["core"].clone()).map_err(|_| reject())?;
    let target = match platform {
        "linux" => "linux/amd64",
        "win" => "windows/amd64",
        "mac-x64" => "darwin/amd64",
        "mac-arm64" => "darwin/arm64",
        _ => return Err(reject()),
    };
    let text = |value: &Value| value.as_str().map(str::to_owned).ok_or_else(reject);
    let required_go_version = format!("go{}", text(&contract["sourcePolicy"]["goVersion"])?);
    let build_tags = text(&row["tags"])?;
    if contract["schema"] != "polaris-upstream-native-release-consumer-v1"
        || catalog.schema != "polaris-selected-tls-support-catalog-v1"
        || row["target"] != target
        || row["sourceCommit"] != contract["sourceCommit"]
        || row["sourceTree"] != contract["sourceTree"]
        || row["version"] != contract["version"]
        || row["goVersion"] != required_go_version.as_str()
        || !build_tags.split(',').any(|tag| tag == "with_utls")
        || core.bytes == 0
        || !hex(&core.sha256, 64)
        || selected.platform != platform
        || contract["version"] != selected.version.as_str()
        || !selected.files.iter().any(|file| {
            file.name == core.member && file.bytes == core.bytes && file.sha256 == core.sha256
        })
    {
        return Err(reject());
    }
    validate_stable(&catalog.stable_support)?;
    let support = &catalog.stable_support;
    let (os, arch) = target.split_once('/').ok_or_else(reject)?;
    Ok(SupportDirectory {
        evidence_kind: KIND,
        selected_spec: Some(SelectedSpec {
            platform: platform.into(),
            target: target.into(),
            source_commit: text(&contract["sourceCommit"])?,
            source_tree: text(&contract["sourceTree"])?,
            version: selected.version.clone(),
            contract_sha256: polaris_updater::sha256_hex(contract_raw.as_bytes()),
            core_bytes: core.bytes,
            core_sha256: core.sha256,
            data_origin: "source-derived-stable-catalog",
            observation_stdout_sha256: None,
            source_archive_sha256: text(&contract["sourceArchive"]["sha256"])?,
            source_index_sha256: text(&contract["assetIndex"]["sha256"])?,
            go_version: required_go_version.clone(),
            required_go_version,
            build_tags,
        }),
        document: Some(SupportDocument {
            schema: "polaris-tls-support-directory-v1",
            enabled: true,
            os: os.into(),
            arch: arch.into(),
            default_fingerprint: support.default_fingerprint.clone(),
            profiles: support
                .profiles
                .iter()
                .map(|p| SupportProfile {
                    id: p.id.clone(),
                    display_name: p.display_name.clone(),
                    selection: p.selection.clone(),
                    client: None,
                    template_version: None,
                    template_offers_x25519_mlkem768: None,
                })
                .collect(),
            legacy_chrome_aliases: support.legacy_chrome_aliases.clone(),
            random_semantics: support.random_semantics.clone(),
            randomized_semantics: support.randomized_semantics.clone(),
            negotiation_evidence: "Support directory only; no runtime or handshake observation.",
        }),
        unavailable_reason: None,
    })
}

pub(crate) fn expected_tls_support() -> &'static SupportDirectory {
    static DIRECTORY: OnceLock<SupportDirectory> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let read = || {
            super::check_compiled_selection()?;
            if polaris_core_supervisor::selected_core::selection_lane()
                != polaris_core_supervisor::selected_core::SelectionLane::Formal
            {
                return Err(reject());
            }
            let platform = super::current_platform()?;
            let selected = super::selected_manifest(platform)?;
            directory_for(
                CATALOG,
                polaris_core_supervisor::selected_core::RELEASE_CONTRACT,
                platform,
                &selected,
            )
        };
        read().unwrap_or_else(SupportDirectory::unavailable)
    })
}

#[cfg(test)]
mod tests;

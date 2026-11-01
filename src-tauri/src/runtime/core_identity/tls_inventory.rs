//! Source-reviewed support for the expected compiled selection. This data does
//! not observe installed files, a running process, execution custody or a TLS handshake.
use crate::runtime::tls_fingerprints::{Capabilities, Module};
use polaris_helper_proto::core_manifest::CoreManifest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
    source_commit: String,
    source_tree: String,
    version: String,
    contract_sha256: String,
    required_go_version: String,
    source_archive_sha256: String,
    source_index_sha256: String,
    source_files: Vec<SourceFile>,
    stable_support: StableSupport,
    bindings: Vec<Binding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    platform: String,
    target: String,
    core: CorePin,
    go_version: String,
    build_tags: String,
    modules: Vec<Module>,
    provenance: Provenance,
}
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Provenance {
    #[serde(rename = "observed-cli-metadata")]
    ObservedCli {
        #[serde(rename = "stdoutSha256")]
        stdout_sha256: String,
        stdout: String,
    },
    #[serde(rename = "source-derived-stable-catalog")]
    SourceDerived {
        #[serde(rename = "sourceTablePath")]
        source_table_path: String,
    },
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
fn module_map(modules: &[Module]) -> Result<BTreeMap<String, Value>, String> {
    let mut result = BTreeMap::new();
    for value in serde_json::to_value(modules)
        .map_err(|_| reject())?
        .as_array()
        .ok_or_else(reject)?
    {
        let path = value["path"].as_str().ok_or_else(reject)?.to_owned();
        if result.insert(path, value.clone()).is_some() {
            return Err(reject());
        }
    }
    Ok(result)
}
fn row_modules(row: &Value) -> Result<BTreeMap<String, Value>, String> {
    let mut modules = BTreeMap::new();
    for key in ["normalModules", "localReplacements"] {
        for module in row["buildGraph"][key].as_array().ok_or_else(reject)? {
            let path = module["module"].as_str().ok_or_else(reject)?;
            if ![
                "github.com/refraction-networking/utls",
                "github.com/metacubex/utls",
            ]
            .contains(&path)
            {
                continue;
            }
            let mut value = json!({"path":path,"version":module["version"],"sum":module["h1"]});
            if let Some(replacement) = module.get("replacement") {
                value["replace"] = json!({"path":replacement["module"],"version":replacement["version"],"sum":replacement["h1"]});
            }
            if modules.insert(path.into(), value).is_some() {
                return Err(reject());
            }
        }
    }
    Ok(modules)
}
// Public source references document the reviewed derivation. They are metadata,
// not an external authentication service: runtime trusts compiled source and
// matches the selected contract without opening an archive or source file.
fn validate_source_references(files: &[SourceFile]) -> Result<(), String> {
    let expected: BTreeSet<&str> = [
        "cmd/sing-box/cmd_tools_tls_fingerprints.go",
        "common/tls/fingerprint_capabilities.go",
        "common/tls/fingerprint_profiles.go",
        "common/tls/fingerprint_profiles_stub.go",
        "common/tls/utls_client.go",
        "common/tls/utls_stub.go",
        "option/tls.go",
    ]
    .into_iter()
    .collect();
    let mut paths = BTreeSet::new();
    for file in files {
        if !hex(&file.sha256, 64) || !paths.insert(file.path.as_str()) {
            return Err(reject());
        }
    }
    if paths != expected {
        return Err(reject());
    }
    Ok(())
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
fn validate_binding(binding: &Binding, catalog: &Catalog, row: &Value) -> Result<(), String> {
    let expected: CorePin = serde_json::from_value(row["core"].clone()).map_err(|_| reject())?;
    let target = match binding.platform.as_str() {
        "linux" => "linux/amd64",
        "win" => "windows/amd64",
        "mac-x64" => "darwin/amd64",
        "mac-arm64" => "darwin/arm64",
        _ => return Err(reject()),
    };
    if binding.core != expected
        || expected.bytes == 0
        || !hex(&expected.sha256, 64)
        || binding.target != target
        || row["target"] != binding.target
        || row["sourceCommit"] != catalog.source_commit
        || row["sourceTree"] != catalog.source_tree
        || row["version"] != catalog.version
        || row["goVersion"] != binding.go_version
        || catalog.required_go_version != binding.go_version
        || row["tags"] != binding.build_tags
        || !binding.build_tags.split(',').any(|tag| tag == "with_utls")
        || module_map(&binding.modules)? != row_modules(row)?
    {
        return Err(reject());
    }
    match &binding.provenance {
        Provenance::ObservedCli {
            stdout_sha256,
            stdout,
        } => {
            if !hex(stdout_sha256, 64)
                || polaris_updater::sha256_hex(stdout.as_bytes()) != *stdout_sha256
            {
                return Err(reject());
            }
            let doc = Capabilities::parse(stdout).map_err(|_| reject())?;
            let profiles: Vec<StableProfile> = doc
                .profiles
                .into_iter()
                .map(|p| StableProfile {
                    id: p.id,
                    display_name: p.display_name,
                    selection: p.selection,
                })
                .collect();
            if !doc.enabled
                || doc.source_commit != catalog.source_commit
                || doc.source_modified != Some(false)
                || doc.core_version != catalog.version
                || format!("{}/{}", doc.os, doc.arch) != binding.target
                || doc.go_version != binding.go_version
                || doc.required_go_version != catalog.required_go_version
                || doc.build_tags != binding.build_tags
                || module_map(&doc.modules)? != module_map(&binding.modules)?
                || profiles != catalog.stable_support.profiles
                || doc.default_fingerprint != catalog.stable_support.default_fingerprint
                || doc.legacy_chrome_aliases != catalog.stable_support.legacy_chrome_aliases
                || doc.random_semantics != catalog.stable_support.random_semantics
                || doc.randomized_semantics != catalog.stable_support.randomized_semantics
            {
                return Err(reject());
            }
        }
        Provenance::SourceDerived { source_table_path } => {
            if source_table_path != "common/tls/fingerprint_profiles.go" {
                return Err(reject());
            }
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
    if contract["schema"] != "polaris-upstream-native-release-consumer-v1"
        || catalog.schema != "polaris-selected-tls-support-catalog-v1"
        || !hex(&catalog.source_commit, 40)
        || !hex(&catalog.source_tree, 40)
        || !hex(&catalog.contract_sha256, 64)
        || !hex(&catalog.source_index_sha256, 64)
        || contract["assetIndex"]["sha256"] != catalog.source_index_sha256
        || !hex(&catalog.source_archive_sha256, 64)
        || contract["sourceArchive"]["sha256"] != catalog.source_archive_sha256
        || catalog.contract_sha256 != polaris_updater::sha256_hex(contract_raw.as_bytes())
        || contract["sourceCommit"] != catalog.source_commit
        || contract["sourceTree"] != catalog.source_tree
        || contract["version"] != catalog.version
        || catalog.bindings.is_empty()
        || catalog.bindings.len() > 4
        || selected.platform != platform
        || selected.version != catalog.version
    {
        return Err(reject());
    }
    validate_source_references(&catalog.source_files)?;
    validate_stable(&catalog.stable_support)?;
    let mut seen = BTreeSet::new();
    let mut found = None;
    for binding in &catalog.bindings {
        if !seen.insert(binding.platform.as_str()) {
            return Err(reject());
        }
        validate_binding(binding, &catalog, &contract["platforms"][&binding.platform])?;
        if binding.platform == platform {
            if !selected.files.iter().any(|file| {
                file.name == binding.core.member
                    && file.bytes == binding.core.bytes
                    && file.sha256 == binding.core.sha256
            }) {
                return Err(reject());
            }
            found = Some(binding);
        }
    }
    let binding = found.ok_or_else(reject)?;
    let (origin, stdout_sha) = match &binding.provenance {
        Provenance::ObservedCli { stdout_sha256, .. } => {
            ("observed-cli-metadata", Some(stdout_sha256.clone()))
        }
        Provenance::SourceDerived { .. } => ("source-derived-stable-catalog", None),
    };
    let (os, arch) = binding.target.split_once('/').ok_or_else(reject)?;
    let support = &catalog.stable_support;
    Ok(SupportDirectory {
        evidence_kind: KIND,
        selected_spec: Some(SelectedSpec {
            platform: platform.into(),
            target: binding.target.clone(),
            source_commit: catalog.source_commit.clone(),
            source_tree: catalog.source_tree.clone(),
            version: catalog.version.clone(),
            contract_sha256: catalog.contract_sha256.clone(),
            core_bytes: binding.core.bytes,
            core_sha256: binding.core.sha256.clone(),
            data_origin: origin,
            observation_stdout_sha256: stdout_sha,
            source_archive_sha256: catalog.source_archive_sha256.clone(),
            source_index_sha256: catalog.source_index_sha256.clone(),
            go_version: binding.go_version.clone(),
            required_go_version: catalog.required_go_version.clone(),
            build_tags: binding.build_tags.clone(),
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
                || super::APP_SOURCE != b"null"
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

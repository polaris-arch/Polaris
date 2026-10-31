//! Read-only compiled selection. Returned manifests are inputs, never execution authority.
use crate::admitted_payload::sha256_hex;
use polaris_helper_proto::core_manifest::{CoreFile, CoreId, CoreManifest};
#[cfg(all(feature = "formal-selection", feature = "local-development-selection"))]
compile_error!("FormalSelection and LocalDevelopmentSelection are mutually exclusive");
#[cfg(not(any(feature = "formal-selection", feature = "local-development-selection")))]
compile_error!("an explicit compiled selection is required");
#[cfg(all(feature = "local-development-selection", not(target_os = "linux")))]
compile_error!("local development exact-object execution is supported only on Linux");
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionLane {
    Formal,
    LocalDevelopment,
}
pub fn selection_lane() -> SelectionLane {
    #[cfg(feature = "local-development-selection")]
    {
        std::hint::black_box(LOCAL_DEVELOPMENT_BINARY_MARKER);
        return SelectionLane::LocalDevelopment;
    }
    #[cfg(not(feature = "local-development-selection"))]
    {
        SelectionLane::Formal
    }
}
#[cfg(feature = "local-development-selection")]
pub const LOCAL_DEVELOPMENT_BINARY_MARKER: &[u8] = b"POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1";
pub const COMPILED_DEVELOPMENT_CONTRACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/local-development-contract.json"));
/// Build-time verified data; neither paths nor this marker grant execution authority.
pub const COMPILED_DEVELOPMENT_SELECTION: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/local-development-selection.json"
));
#[cfg(feature = "local-development-selection")]
pub(crate) mod development;
pub(crate) fn selected_contract() -> Result<&'static str, String> {
    #[cfg(feature = "local-development-selection")]
    {
        return development::contract_raw();
    }
    #[cfg(not(feature = "local-development-selection"))]
    {
        Ok(RELEASE_CONTRACT)
    }
}
pub(crate) fn selected_pure_manifest(platform: &str) -> Result<CoreManifest, String> {
    #[cfg(feature = "local-development-selection")]
    {
        return development::manifest(platform);
    }
    #[cfg(not(feature = "local-development-selection"))]
    {
        formal_manifest(platform)
    }
}
pub const RELEASE_CONTRACT: &str =
    include_str!("../../../scripts/desktop-core/release-contract.json");
pub const COMPILED_INSTALLATION: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/selected-core-manifest.json"));

pub const COMPILED_VALIDATION_SOURCE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/validation-core-source.json"));

/// Data only; the archive/member hashes still come exclusively from the same
/// compiled release contract. This inventory cannot grant execution authority.
pub const COMPILED_RECOVERY_INVENTORY: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/selected-core-recovery-inventory.json"
));

pub fn selected_recovery_spec(platform: &str) -> Result<serde_json::Value, String> {
    if COMPILED_VALIDATION_SOURCE != b"null" || COMPILED_RECOVERY_INVENTORY == b"null" {
        return Err("CORE_RECOVERY_UNAVAILABLE: no compiled selected recovery inventory".into());
    }
    recovery_spec_with_contract(
        platform,
        COMPILED_RECOVERY_INVENTORY,
        selected_contract()?,
        selected_pure_manifest(platform)?,
    )
}
fn recovery_spec_from_inventory(platform: &str, raw: &[u8]) -> Result<serde_json::Value, String> {
    recovery_spec_with_contract(platform, raw, RELEASE_CONTRACT, formal_manifest(platform)?)
}
fn recovery_spec_with_contract(
    platform: &str,
    raw: &[u8],
    contract_raw: &str,
    selected: CoreManifest,
) -> Result<serde_json::Value, String> {
    use serde_json::{json, Value};
    let reject = || "CORE_RECOVERY_UNAVAILABLE: compiled recovery inventory differs".to_owned();
    let exact = |value: &Value, keys: &[&str]| -> bool {
        value.as_object().is_some_and(|object| {
            object.len() == keys.len() && keys.iter().all(|k| object.contains_key(*k))
        })
    };
    let inventory: Value = serde_json::from_slice(raw).map_err(|_| reject())?;
    if !exact(
        &inventory,
        &[
            "schema",
            "platform",
            "contractSha256",
            "archive",
            "memberBytes",
        ],
    ) || inventory["schema"] != "polaris-core-recovery-inventory-v1"
        || inventory["platform"] != platform
        || inventory["contractSha256"] != sha256_hex(contract_raw.as_bytes())
    {
        return Err(reject());
    }
    let contract: Value = serde_json::from_str(contract_raw).map_err(|_| reject())?;
    let archive = &contract["platforms"][platform]["archive"];
    let name = archive["name"].as_str().ok_or_else(reject)?;
    let root = archive["root"].as_str().ok_or_else(reject)?;
    let leaf =
        |v: &str| !v.is_empty() && !matches!(v, "." | "..") && !v.contains(['/', '\\', '\0']);
    if !leaf(name) || !leaf(root) {
        return Err(reject());
    }
    let format = if name.ends_with(".tar.gz") {
        "tar.gz"
    } else if name.ends_with(".zip") {
        "zip"
    } else {
        return Err(reject());
    };
    let observed = &inventory["archive"];
    if !exact(observed, &["name", "format", "bytes", "sha256", "root"])
        || observed["name"] != name
        || observed["root"] != root
        || observed["format"] != format
        || observed["sha256"] != archive["sha256"]
        || observed["bytes"] != archive["bytes"]
    {
        return Err(reject());
    }
    let bytes = archive["bytes"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(reject)?;
    CoreId::parse(archive["sha256"].as_str().ok_or_else(reject)?).map_err(|_| reject())?;
    let expected = archive["members"].as_object().ok_or_else(reject)?;
    let sizes = inventory["memberBytes"].as_object().ok_or_else(reject)?;
    if expected.is_empty() || expected.len() != sizes.len() {
        return Err(reject());
    }
    let mut members = Vec::new();
    for (name, hash) in expected {
        let member = name
            .strip_prefix(&format!("{root}/"))
            .filter(|v| leaf(v))
            .ok_or_else(reject)?;
        let _ = member;
        let sha = hash.as_str().ok_or_else(reject)?;
        CoreId::parse(sha).map_err(|_| reject())?;
        let size = sizes
            .get(name)
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .ok_or_else(reject)?;
        members.push(json!({"name":name,"bytes":size,"sha256":sha}));
    }
    let id = selected.id(sha256_hex).map_err(|_| reject())?;
    let mut payload = Vec::new();
    for file in &selected.files {
        let name = format!("{root}/{}", file.name);
        if expected.get(&name).and_then(Value::as_str) != Some(file.sha256.as_str())
            || sizes.get(&name).and_then(Value::as_u64) != Some(file.bytes)
        {
            return Err(reject());
        }
        payload.push(name);
    }
    Ok(
        json!({"schema":"polaris-core-recovery-v1","platform":platform,"coreId":id.as_str(),"archive":{"resource":format!("core-recovery/{platform}/{name}"),"format":format,"bytes":bytes,"sha256":archive["sha256"],"root":root},"members":members,"payloadMembers":payload}),
    )
}

pub fn compiled_source(compiled: &[u8], platform: &str) -> Result<serde_json::Value, String> {
    let selected: serde_json::Value =
        serde_json::from_slice(compiled).map_err(|_| "invalid compiled validation source")?;
    let manual = selected["schema"] == "polaris-native-dns-compiled-source-v1"
        && selected["validationOnly"] == true;
    let temporary = selected["schema"] == "polaris-native-dns-compiled-source-v2"
        && selected["validationOnly"] == false
        && selected["purpose"] == "temporary-default"
        && selected["context"]["mode"] == "temporary-default";
    if !(manual || temporary)
        || selected["AppAccepted"] != false
        || selected["publicationEligible"] != false
        || !selected["platforms"]
            .as_array()
            .is_some_and(|platforms| platforms.iter().any(|value| value == platform))
    {
        return Err("compiled validation source selection differs".into());
    }
    Ok(selected)
}

pub fn selected_source_from(compiled: &[u8], platform: &str) -> Result<String, String> {
    let source = if compiled == b"null" {
        let contract: serde_json::Value = serde_json::from_str(RELEASE_CONTRACT)
            .map_err(|_| "invalid compiled source contract")?;
        contract["platforms"][platform]["sourceCommit"].clone()
    } else {
        let selected = compiled_source(compiled, platform)?;
        selected["sourceCommit"].clone()
    };
    let source = source.as_str().ok_or("missing compiled source identity")?;
    if source.len() != 40
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid compiled source identity".into());
    }
    Ok(source.to_owned())
}

pub fn selected_source_commit(platform: &str) -> Result<String, String> {
    #[cfg(feature = "local-development-selection")]
    {
        return development::source(platform);
    }
    selected_source_from(COMPILED_VALIDATION_SOURCE, platform)
}

/// Private check-only admission consumes compiled identity, never an environment
/// digest. Missing/analysis-only installation identities cannot select a profile.
pub fn selected_check_source(platform: &str) -> Result<(String, String), String> {
    let selected = selected_manifest(platform)?;
    let source = selected_source_commit(platform)?;
    let contract_sha256 = if COMPILED_VALIDATION_SOURCE == b"null" {
        sha256_hex(selected_contract()?.as_bytes())
    } else {
        let compiled = compiled_source(COMPILED_VALIDATION_SOURCE, platform)?;
        let digest = compiled["bindingSha256"]
            .as_str()
            .ok_or("missing compiled check contract")?;
        if digest != selected.provider_fingerprint {
            return Err("compiled check contract differs".into());
        }
        digest.to_owned()
    };
    if contract_sha256.len() != 64
        || !contract_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid compiled check contract".into());
    }
    Ok((source, contract_sha256))
}

/// Candidate builds resolve the complete verified carrier for both core and
/// helper. Runtime environment cannot switch this compiled choice or fall back
/// to an old formal carrier. Dashboard/data resource lookup is unchanged.
pub fn selected_payload_dirs(formal: &[&str]) -> Result<Vec<String>, String> {
    #[cfg(feature = "local-development-selection")]
    {
        development::manifest(current_platform()?)?;
        return Ok(vec![".local-development/linux".into()]);
    }

    if COMPILED_VALIDATION_SOURCE == b"null" {
        return Ok(formal.iter().map(|value| (*value).to_owned()).collect());
    }
    let platform = current_platform()?;
    let selected = selected_manifest(platform)?;
    if selected.helper_sha256.is_none() || (platform == "win" && selected.cleaner_sha256.is_none())
    {
        return Err("CORE_BUNDLE_MISMATCH: candidate carrier is incomplete".into());
    }
    let source = compiled_source(COMPILED_VALIDATION_SOURCE, platform)?;
    Ok(vec![
        if source["schema"] == "polaris-native-dns-compiled-source-v2" {
            platform.to_owned()
        } else {
            format!(".native-dns-app/{platform}")
        },
    ])
}

pub fn selected_manifest(platform: &str) -> Result<CoreManifest, String> {
    if COMPILED_INSTALLATION != b"null" {
        let selected = polaris_helper_proto::core_manifest::InstalledCoreManifest::parse(
            COMPILED_INSTALLATION,
            sha256_hex,
        )
        .map_err(|_| "CORE_BUNDLE_MISMATCH: invalid compiled installation identity")?;
        if selected.core.platform != platform {
            return Err("CORE_BUNDLE_MISMATCH: compiled platform differs".into());
        }
        if COMPILED_VALIDATION_SOURCE != b"null" {
            let source = compiled_source(COMPILED_VALIDATION_SOURCE, platform)?;
            if source["bindingSha256"].as_str() != Some(selected.core.provider_fingerprint.as_str())
            {
                return Err("CORE_BUNDLE_MISMATCH: compiled provider differs".into());
            }
        } else {
            let expected = selected_pure_manifest(platform)?;
            let selected_id = selected
                .core
                .id(sha256_hex)
                .map_err(|_| "invalid selected formal id")?;
            let expected_id = expected
                .id(sha256_hex)
                .map_err(|_| "invalid expected formal id")?;
            if selected_id != expected_id {
                return Err("CORE_BUNDLE_MISMATCH: compiled formal selection differs".into());
            }
        }
        return Ok(selected.core);
    }
    if COMPILED_VALIDATION_SOURCE != b"null" {
        return Err("CORE_BUNDLE_MISMATCH: candidate installation identity missing".into());
    }
    #[cfg(feature = "installed-development-selection")]
    {
        return Err("CORE_BUNDLE_MISMATCH: local App installation identity missing".into());
    }
    selected_pure_manifest(platform)
}

pub fn formal_manifest(platform: &str) -> Result<CoreManifest, String> {
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
        provider_fingerprint: formal_provider_fingerprint(RELEASE_CONTRACT, &contract, row)?,
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    manifest
        .validate()
        .map_err(|reason| format!("CORE_BUNDLE_MISMATCH: {reason}"))?;
    Ok(manifest)
}

pub fn formal_provider_fingerprint(
    raw: &str,
    contract: &serde_json::Value,
    row: &serde_json::Value,
) -> Result<String, String> {
    if contract["schema"] == "polaris-upstream-native-release-consumer-v1" {
        if row.get("providerFingerprint").is_some() || row.get("providerIdentity").is_some() {
            return Err("CORE_BUNDLE_MISMATCH: unverified upstream producer identity".into());
        }
        let mut bytes = b"polaris-upstream-native-installation-v1\0".to_vec();
        bytes.extend_from_slice(raw.as_bytes());
        Ok(sha256_hex(&bytes))
    } else {
        row["providerFingerprint"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "CORE_BUNDLE_MISMATCH: missing provider".into())
    }
}

pub fn selected_id(platform: &str) -> Result<CoreId, String> {
    selected_manifest(platform)?
        .id(sha256_hex)
        .map_err(|reason| format!("CORE_BUNDLE_MISMATCH: {reason}"))
}

pub fn current_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux"),
        ("windows", "x86_64") => Ok("win"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        _ => Err("CORE_BUNDLE_MISMATCH: no desktop selected identity".into()),
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    fn inventory(platform: &str) -> serde_json::Value {
        use serde_json::json;
        let contract: serde_json::Value = serde_json::from_str(RELEASE_CONTRACT).unwrap();
        let archive = &contract["platforms"][platform]["archive"];
        let mut sizes = serde_json::Map::new();
        // Non-payload sizes here are header fixtures, not archive verification.
        // The non-executing decoder must still verify every actual byte/hash.
        for name in archive["members"].as_object().unwrap().keys() {
            sizes.insert(name.clone(), json!(1));
        }
        for file in formal_manifest(platform).unwrap().files {
            sizes.insert(
                format!("{}/{}", archive["root"].as_str().unwrap(), file.name),
                json!(file.bytes),
            );
        }
        json!({"schema":"polaris-core-recovery-inventory-v1","platform":platform,"contractSha256":sha256_hex(RELEASE_CONTRACT.as_bytes()),"archive":{"name":archive["name"],"format":if platform=="win" {"zip"}else{"tar.gz"},"bytes":archive["bytes"],"sha256":archive["sha256"],"root":archive["root"]},"memberBytes":sizes})
    }
    #[test]
    fn compiled_lane_never_uses_validation_receipts_as_local_authority() {
        #[cfg(feature = "formal-selection")]
        {
            assert_eq!(selection_lane(), SelectionLane::Formal);
            assert_eq!(COMPILED_DEVELOPMENT_SELECTION, b"null");
            assert_eq!(COMPILED_DEVELOPMENT_CONTRACT, b"null");
        }
        #[cfg(feature = "local-development-selection")]
        {
            assert_eq!(selection_lane(), SelectionLane::LocalDevelopment);
            assert_eq!(COMPILED_VALIDATION_SOURCE, b"null");
            let pure = selected_pure_manifest("linux").unwrap();
            let source = selected_source_commit("linux").unwrap();
            assert_ne!(source, selected_source_from(b"null", "linux").unwrap());
            let mut installed = pure.clone();
            installed.helper_sha256 = Some("f".repeat(64));
            assert_eq!(
                pure.id(sha256_hex).unwrap(),
                installed.id(sha256_hex).unwrap()
            );
            assert!(selected_pure_manifest("win").is_err());
            assert!(selected_pure_manifest("mac-arm64").is_err());
            assert_eq!(selected_check_source("linux").unwrap().0, source);
        }
    }

    #[test]
    fn recovery_data_uses_the_one_compiled_hash_set_and_derives_core_id() {
        for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
            let inventory = inventory(platform);
            let spec =
                recovery_spec_from_inventory(platform, &serde_json::to_vec(&inventory).unwrap())
                    .unwrap();
            assert_eq!(
                spec["coreId"],
                formal_manifest(platform)
                    .unwrap()
                    .id(sha256_hex)
                    .unwrap()
                    .as_str()
            );
            assert_eq!(
                spec["payloadMembers"].as_array().unwrap().len(),
                formal_manifest(platform).unwrap().files.len()
            );
            assert_eq!(
                spec["members"].as_array().unwrap().len(),
                inventory["memberBytes"].as_object().unwrap().len()
            );
        }
    }
    #[test]
    fn wrong_or_extra_inventory_fields_cannot_select_another_archive_or_member() {
        for path in [
            vec!["platform"],
            vec!["contractSha256"],
            vec!["archive", "sha256"],
            vec!["archive", "root"],
            vec!["archive", "name"],
            vec!["archive", "format"],
            vec!["archive", "bytes"],
        ] {
            let mut bad = inventory("linux");
            let mut value = &mut bad;
            for part in path {
                value = &mut value[part];
            }
            *value = serde_json::json!("foreign");
            assert!(
                recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err()
            );
        }
        let mut bad = inventory("linux");
        bad["memberBytes"]["foreign/member"] = serde_json::json!(1);
        assert!(recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = inventory("linux");
        let key = bad["memberBytes"]
            .as_object()
            .unwrap()
            .keys()
            .find(|key| key.ends_with("/sing-box"))
            .unwrap()
            .clone();
        bad["memberBytes"][key] = serde_json::json!(1);
        assert!(recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err());
    }
}

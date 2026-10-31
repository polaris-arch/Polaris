//! The compiler selected this lane. Serialized data never selects it.
use super::*;
#[path = "development_config.rs"]
mod configuration;
use serde_json::Value;
use std::sync::OnceLock;
fn document() -> Result<&'static Value, String> {
    static VALUE: OnceLock<Result<Value, String>> = OnceLock::new();
    VALUE
        .get_or_init(|| {
            std::hint::black_box(LOCAL_DEVELOPMENT_BINARY_MARKER);
            let pin = configuration::COMPILED
                .as_ref()
                .ok_or("no compiled development selection")?;
            if sha256_hex(COMPILED_DEVELOPMENT_SELECTION) != pin.selection_sha256
                || sha256_hex(COMPILED_DEVELOPMENT_CONTRACT) != pin.contract_sha256
            {
                return Err("development data differs from compiler pins".into());
            }
            let value: Value = serde_json::from_slice(COMPILED_DEVELOPMENT_SELECTION)
                .map_err(|_| "invalid compiled development selection")?;
            let fields = [
                "schema",
                "configurationSha256",
                "context",
                "row",
                "proof",
                "policy",
                "providerFingerprint",
                "contractSha256",
            ];
            if !value.as_object().is_some_and(|o| {
                o.len() == fields.len() && fields.iter().all(|k| o.contains_key(*k))
            }) || value["schema"] != "polaris-local-development-selection-v1"
            {
                return Err("development selection shape differs".into());
            }
            for key in [
                "configurationSha256",
                "providerFingerprint",
                "contractSha256",
            ] {
                CoreId::parse(value[key].as_str().ok_or("missing development digest")?)
                    .map_err(|_| "invalid development digest")?;
            }
            if value["contractSha256"] != sha256_hex(COMPILED_DEVELOPMENT_CONTRACT) {
                return Err("development contract identity differs".into());
            }
            let row = &value["row"];
            let context = &value["context"];
            if context["coreSource"]["commit"] != pin.source_commit
                || context["coreSource"]["tree"] != pin.source_tree
                || context["producerSource"]["commit"] != pin.producer_commit
                || context["producerSource"]["tree"] != pin.producer_tree
                || row["target"] != pin.target
                || row["goVersion"] != pin.go_version
                || row["tags"] != pin.tags
                || row["core"]["bytes"].as_u64() != Some(pin.core_bytes)
                || row["core"]["sha256"] != pin.core_sha256
                || row["sidecar"]["bytes"].as_u64() != Some(pin.sidecar_bytes)
                || row["sidecar"]["sha256"] != pin.sidecar_sha256
                || value["policy"]["purposes"] != serde_json::json!(pin.purposes)
            {
                return Err("development compiled facts differ".into());
            }
            Ok(value)
        })
        .as_ref()
        .map_err(Clone::clone)
}
pub(super) fn contract_raw() -> Result<&'static str, String> {
    document()?;
    std::str::from_utf8(COMPILED_DEVELOPMENT_CONTRACT)
        .map_err(|_| "invalid development contract encoding".into())
}
pub(super) fn source(platform: &str) -> Result<String, String> {
    manifest(platform)?;
    Ok(document()?["context"]["coreSource"]["commit"]
        .as_str()
        .ok_or("missing development source")?
        .into())
}
pub(super) fn manifest(platform: &str) -> Result<CoreManifest, String> {
    if platform != "linux" {
        return Err("local development platform is Unsupported".into());
    }
    let d = document()?;
    let mut files = Vec::new();
    for (key, name) in [("core", "sing-box"), ("sidecar", "libcronet.so")] {
        let v = &d["row"][key];
        if v["member"] != name {
            return Err("development member differs".into());
        }
        files.push(CoreFile {
            name: name.into(),
            bytes: v["bytes"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("invalid development member bytes")?,
            sha256: v["sha256"]
                .as_str()
                .ok_or("missing development member digest")?
                .into(),
        });
    }
    let selected = CoreManifest {
        platform: platform.into(),
        version: d["row"]["version"]
            .as_str()
            .ok_or("missing development version")?
            .into(),
        provider_fingerprint: d["providerFingerprint"]
            .as_str()
            .ok_or("missing development provider")?
            .into(),
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    selected
        .id(sha256_hex)
        .map_err(|e| format!("invalid development identity: {e}"))?;
    let contract: Value = serde_json::from_slice(COMPILED_DEVELOPMENT_CONTRACT)
        .map_err(|_| "invalid development contract")?;
    let row = &contract["platforms"][platform];
    if contract["schema"] != "polaris-local-development-core-contract-v1"
        || contract["version"] != d["row"]["version"]
        || row["core"] != d["row"]["core"]
        || row["sidecar"] != d["row"]["sidecar"]
        || row["trustedExecution"] != d["policy"]
        || row["providerFingerprint"] != d["providerFingerprint"]
        || row["sourceCommit"] != d["context"]["coreSource"]["commit"]
        || row["sourceTree"] != d["context"]["coreSource"]["tree"]
    {
        return Err("development contract binding differs".into());
    }
    Ok(selected)
}

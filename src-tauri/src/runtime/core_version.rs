//! Selected bundled kernel identity shared by HTTP UA and version displays.
//! This is build identity, never evidence that a core process is currently running.
use std::sync::OnceLock;

#[cfg(any(target_os = "android", target_os = "ios", test))]
const MOBILE_SOURCE_CONTRACT: &str =
    include_str!("../../../scripts/mobile-core/source-contract.json");

pub(crate) fn selected_bundled_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let version = read_selected_kernel_version();
        if version.is_empty() {
            "unknown".to_owned()
        } else {
            version
        }
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn read_selected_kernel_version() -> String {
    // Exactly the compiled installation selection used by desktop core admission,
    // including validation/temporary overrides. Never fall back to a stale baseline.
    crate::runtime::core_identity::current_platform()
        .and_then(crate::runtime::core_identity::selected_manifest)
        .map(|manifest| manifest.version)
        .unwrap_or_default()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn read_selected_kernel_version() -> String {
    mobile_kernel_version(MOBILE_SOURCE_CONTRACT)
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
fn mobile_kernel_version(raw: &str) -> String {
    // The mobile builder injects this exact version into constant.Version via
    // fork_input.py's ldflags. The old updater baseline is not the mobile build.
    let Ok(contract) = serde_json::from_str::<serde_json::Value>(raw) else {
        return String::new();
    };
    if contract["schema"] != "polaris-mobile-fork-input-v1" {
        return String::new();
    }
    let version = contract["version"].as_str().unwrap_or_default();
    if version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return String::new();
    }
    version.to_owned()
}

#[cfg(test)]
mod tests;

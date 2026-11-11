//! Admission for the compiled desktop release selection; artifact verification
//! remains the build consumer's responsibility. No filesystem or HTTP access.
use serde_json::Value;

/// A valid selection names exactly the compiled contract by hash; its public
/// identity comes from those contract bytes. Any other selection withholds identity.
pub fn desktop_release_identity(selection: &Value) -> Option<(String, String)> {
    const RAW: &str = include_str!("../../../scripts/desktop-core/release-contract.json");
    desktop_release_identity_from_contract(selection, RAW)
}

fn desktop_release_identity_from_contract(
    selection: &Value,
    raw: &str,
) -> Option<(String, String)> {
    if selection["schema"] != "polaris-app-release-selection-v1"
        || selection["contractPath"] != "scripts/desktop-core/release-contract.json"
        || selection["contractSha256"] != crate::sha256_hex(raw.as_bytes())
        || selection.as_object()?.len() != 3
    {
        return None;
    }
    let contract: Value = serde_json::from_str(raw).ok()?;
    let version = contract["version"].as_str()?;
    if contract["schema"] != "polaris-upstream-native-release-consumer-v1"
        || contract["providerContract"] != "polaris-upstream-native-release-v1"
        || contract["repository"] != "polaris-arch/sing-box-polaris"
        || contract["repositoryId"] != 1_421_657_509u64
        || !upstream_release_version(version)
        || contract["tag"].as_str()? != format!("v{version}")
    {
        return None;
    }
    Some((
        version.to_owned(),
        contract["sourceTree"].as_str()?.to_owned(),
    ))
}

// Match scripts/lib/polaris-version.json; shared fixture tests enforce parity.
// One optional Polaris revision retains the full upstream prerelease baseline.
// Version syntax alone is never enough to admit an identity.
fn upstream_release_version(version: &str) -> bool {
    fn digits(value: &str) -> bool {
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && (value == "0" || !value.starts_with('0'))
    }
    let Some((triplet, suffix)) = version.split_once("-polaris-") else {
        return false;
    };
    let numbers: Vec<_> = triplet.split('.').collect();
    let Some((channel, number)) = suffix.split_once('.') else {
        return false;
    };
    let revisions: Vec<_> = number.split('.').collect();
    numbers.len() == 3
        && numbers.iter().all(|number| digits(number))
        && matches!(channel, "alpha" | "beta" | "rc")
        && (1..=2).contains(&revisions.len())
        && revisions.iter().all(|revision| digits(revision))
}

#[cfg(test)]
mod tests;

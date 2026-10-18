//! Admission for the compiled desktop release selection; artifact verification
//! remains the build consumer's responsibility. No filesystem or HTTP access.
use serde_json::Value;

/// Display the independently selected temporary default. This does not admit
/// its staging evidence as a formal release or authorize an update artifact.
pub fn desktop_temporary_identity(selection: &Value) -> Option<(&str, &str)> {
    const RAW: &str = include_str!("../../../scripts/desktop-core/native-dns-candidate.json");
    let binding: Value = serde_json::from_str(RAW).ok()?;
    if selection["schema"] != "polaris-app-temporary-desktop-selection-v1"
        || selection["contractPath"] != "scripts/desktop-core/native-dns-candidate.json"
        || selection["contractSha256"] != crate::sha256_hex(RAW.as_bytes())
        || selection.as_object()?.len() != 11
    {
        return None;
    }
    for key in [
        "version",
        "tag",
        "tagObject",
        "sourceCommit",
        "sourceTree",
        "releaseId",
        "producer",
    ] {
        if selection[key] != binding[key] {
            return None;
        }
    }
    if selection["librarySha256"].as_object()?.len() != 2 {
        return None;
    }
    for platform in ["linux", "win"] {
        if selection["librarySha256"][platform]
            != binding["platforms"][platform]["sidecar"]["sha256"]
        {
            return None;
        }
    }
    Some((
        selection["version"].as_str()?,
        selection["sourceTree"].as_str()?,
    ))
}

/// Valid selections carry independently frozen public identity and contract hash.
/// A present invalid selection must withhold identity, rather than use source pins.
pub fn desktop_release_identity(selection: &Value) -> Option<(&str, &str)> {
    // Only this contract is compiled into this consumer. An unbound pending path
    // must not borrow its bytes, even when all displayed identity fields match.
    const RAW: &str = include_str!("../../../scripts/desktop-core/release-contract.json");
    desktop_release_identity_from_contract(selection, RAW)
}

fn desktop_release_identity_from_contract<'a>(
    selection: &'a Value,
    raw: &str,
) -> Option<(&'a str, &'a str)> {
    fn hex(value: &Value, length: usize) -> bool {
        value.as_str().is_some_and(|s| {
            s.len() == length
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    }
    if selection["schema"] != "polaris-app-release-selection-v1"
        || selection["contractPath"] != "scripts/desktop-core/release-contract.json"
        || selection["contractSha256"] != crate::sha256_hex(raw.as_bytes())
        || selection.as_object()?.len() != 10
        || !hex(&selection["sourceCommit"], 40)
        || !hex(&selection["sourceTree"], 40)
        || !hex(&selection["tagObject"], 40)
        || !selection["releaseId"]
            .as_u64()
            .is_some_and(|id| id > 0 && id <= 9_007_199_254_740_991)
        || !hex(&selection["librarySha256"]["linux"], 64)
        || !hex(&selection["librarySha256"]["win"], 64)
        || selection["librarySha256"].as_object()?.len() != 2
    {
        return None;
    }
    let binding: Value = serde_json::from_str(raw).ok()?;
    let version = selection["version"].as_str()?;
    let historical_version = version == "1.15.0-alpha.12"
        || version
            .strip_prefix("1.15.0-alpha.12.polaris.")
            .is_some_and(|suffix| {
                suffix
                    .bytes()
                    .next()
                    .is_some_and(|b| (b'1'..=b'9').contains(&b))
                    && suffix.bytes().all(|b| b.is_ascii_digit())
            });
    let historical = binding["schema"] == "polaris-box-release-consumer-contract-v1"
        && binding["repository"] == "polaris-arch/polaris-box"
        && historical_version;
    let upstream = binding["schema"] == "polaris-upstream-native-release-consumer-v1"
        && binding["providerContract"] == "polaris-upstream-native-release-v1"
        && binding["repository"] == "polaris-arch/sing-box-polaris"
        && binding["repositoryId"] == 1_421_657_509u64
        && upstream_release_version(version);
    if !(historical || upstream) || selection["tag"].as_str()? != format!("v{version}") {
        return None;
    }
    for key in [
        "version",
        "tag",
        "tagObject",
        "sourceCommit",
        "sourceTree",
        "releaseId",
    ] {
        if selection[key] != binding[key] {
            return None;
        }
    }
    for platform in ["linux", "win"] {
        if selection["librarySha256"][platform]
            != binding["platforms"][platform]["sidecar"]["sha256"]
        {
            return None;
        }
    }
    Some((version, selection["sourceTree"].as_str()?))
}

// Match the upstream release validator's numeric triplet and Polaris channel.
// Version syntax alone is never enough to admit an identity.
fn upstream_release_version(version: &str) -> bool {
    fn digits(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
    }
    let Some((triplet, suffix)) = version.split_once("-polaris-") else {
        return false;
    };
    let numbers: Vec<_> = triplet.split('.').collect();
    let Some((channel, number)) = suffix.split_once('.') else {
        return false;
    };
    numbers.len() == 3
        && numbers.iter().all(|number| digits(number))
        && matches!(channel, "alpha" | "beta" | "rc")
        && digits(number)
        && (number == "0" || !number.starts_with('0'))
}

#[cfg(test)]
mod tests;

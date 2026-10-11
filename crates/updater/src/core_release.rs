//! Admission for the compiled desktop release selection; artifact verification
//! remains the build consumer's responsibility. No filesystem or HTTP access.
use serde_json::Value;

/// Valid selections carry independently frozen public identity and contract hash.
/// A present invalid selection must withhold identity, rather than use source pins.
pub fn desktop_release_identity(selection: &Value) -> Option<(&str, &str)> {
    fn hex(value: &Value, length: usize) -> bool {
        value.as_str().is_some_and(|s| {
            s.len() == length
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    }
    if selection["schema"] != "polaris-app-release-selection-v1"
        || selection["contractPath"] != "scripts/desktop-core/release-contract.json"
        || !hex(&selection["contractSha256"], 64)
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
    let version = selection["version"].as_str()?;
    let suffix = version.strip_prefix("1.15.0-alpha.12.polaris.")?;
    if !suffix
        .bytes()
        .next()
        .is_some_and(|b| (b'1'..=b'9').contains(&b))
        || !suffix.bytes().all(|b| b.is_ascii_digit())
        || selection["tag"].as_str()? != format!("v{version}")
    {
        return None;
    }
    Some((version, selection["sourceTree"].as_str()?))
}

#[cfg(test)]
mod tests;

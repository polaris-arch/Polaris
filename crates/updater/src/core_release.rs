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
mod tests {
    use super::*;
    use serde_json::json;

    fn selection() -> Value {
        json!({"schema":"polaris-app-release-selection-v1",
            "contractPath":"scripts/desktop-core/release-contract.json",
            "contractSha256":"a".repeat(64), "sourceCommit":"b".repeat(40),
            "sourceTree":"c".repeat(40), "tagObject":"d".repeat(40), "releaseId":17,
            "version":"1.15.0-alpha.12.polaris.1", "tag":"v1.15.0-alpha.12.polaris.1",
            "librarySha256":{"linux":"e".repeat(64), "win":"f".repeat(64)}})
    }

    #[test]
    fn complete_release_identity_is_distinct_from_source_identity() {
        let value = selection();
        assert_eq!(
            desktop_release_identity(&value),
            Some((
                "1.15.0-alpha.12.polaris.1",
                "cccccccccccccccccccccccccccccccccccccccc"
            ))
        );
    }

    #[test]
    fn every_public_binding_is_required_and_invalid_values_withhold_identity() {
        let value = selection();
        for key in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(desktop_release_identity(&missing).is_none(), "{key}");
            let mut null = value.clone();
            null[key] = Value::Null;
            assert!(desktop_release_identity(&null).is_none(), "{key}");
        }
        for bad in [json!(null), json!([]), json!("release")] {
            assert!(desktop_release_identity(&bad).is_none());
        }
        for (key, bad) in [
            ("releaseId", json!(0)),
            ("releaseId", json!(9_007_199_254_740_992u64)),
            ("releaseId", json!(1.5)),
            ("sourceCommit", json!("B".repeat(40))),
            ("tag", json!("v1.15.0-alpha.12.polaris.2")),
            ("version", json!("1.15.0-alpha.12.polaris.01")),
            ("version", json!("1.15.0-alpha.11.polaris.1")),
            ("contractPath", json!("../release-contract.json")),
        ] {
            let mut changed = value.clone();
            changed[key] = bad;
            assert!(desktop_release_identity(&changed).is_none(), "{key}");
        }
    }
}

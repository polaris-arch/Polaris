use super::super::*;
#[test]
fn external_matcher_values_do_not_hide_outer_policy_changes() {
    let base = serde_json::json!({"proxyMode":"smart","proxyModeType":"systemProxy","servers":[],"customRules":[{"id":"r1","type":"domain","values":["a.example"],"action":"proxy","enabled":true}]});
    let config = |v| serde_json::from_value::<UserConfig>(v).unwrap();
    let original = config(base.clone());
    let norm = config_generation_norm(&original, None);
    assert!(norm.contains("\"__ext\":1"));
    let mut matcher = base.clone();
    matcher["customRules"][0]["values"] = serde_json::json!(["b.example"]);
    assert_eq!(norm, config_generation_norm(&config(matcher), None));
    let mut target = base.clone();
    target["customRules"][0]["targetServerId"] = serde_json::json!("another");
    assert_eq!(norm, config_generation_norm(&config(target), None));
    let mut network = base.clone();
    network["customRules"][0]["networkProfileId"] = serde_json::json!("profile");
    assert_ne!(norm, config_generation_norm(&config(network), None));
    let mut spoof = base;
    spoof["customRules"][0]["tlsSpoof"] = serde_json::json!("cover.example");
    assert_ne!(norm, config_generation_norm(&config(spoof), None));
}

use super::super::*;
use serde_json::json;

#[test]
fn identities_are_plane_local_but_duplicates_in_each_input_are_rejected() {
    assert!(validate_raw_rule_ids(&json!({
        "trafficRules": [{"id":"same"}], "dnsRules": [{"id":"same"}]
    }))
    .is_ok());
    for key in ["trafficRules", "dnsRules", "policyRules", "customRules"] {
        assert!(validate_raw_rule_ids(&json!({key: [{"id":"same"}, {"id":"same"}]})).is_err());
    }
}

#[test]
fn batch_or_import_rejects_duplicates_before_sanitizer_can_hide_them() {
    let mut input = crate::default_config();
    let rule = json!({"id":"same", "type":"domain", "values":["example.com"],
        "action":"proxy", "enabled":true});
    input["trafficRules"] = json!([rule.clone()]);
    // Positive control: unrelated shape/default errors must not satisfy this test.
    assert!(crate::ConfigStore::canonicalize_for_save(&input).is_ok());
    input["trafficRules"] = json!([rule.clone(), rule]);
    assert!(crate::ConfigStore::canonicalize_for_save(&input).is_err());
    assert_eq!(input["trafficRules"].as_array().unwrap().len(), 2);
}

#[test]
fn persisted_legacy_duplicates_remain_readable_and_complete_repair_can_save() {
    let mut input = crate::default_config();
    let rule = json!({"id":"same", "type":"domain", "values":["example.com"],
        "action":"proxy", "enabled":true});
    input["configSchemaVersion"] = json!(4);
    for key in ["trafficRules", "policyRules", "customRules"] {
        input[key] = json!([rule.clone(), rule.clone()]);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("polaris-rule-ids-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("config.json");
    let bytes = serde_json::to_vec(&input).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let loaded = crate::ConfigStore::load(&crate::StdFs, &path);
    assert!(loaded.loaded_from_disk);
    assert!(loaded.error.is_none());
    assert_eq!(loaded.config["trafficRules"].as_array().unwrap().len(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(crate::ConfigStore::canonicalize_for_save(&loaded.config).is_err());
    let mut repaired = loaded.config;
    for key in ["trafficRules", "policyRules", "customRules"] {
        repaired[key] = json!([rule.clone()]);
    }
    assert!(crate::ConfigStore::canonicalize_for_save(&repaired).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

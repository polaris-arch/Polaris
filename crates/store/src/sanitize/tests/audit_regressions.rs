use super::super::*;
#[test]
fn malformed_collection_is_not_erased_by_unrelated_migration() {
    for key in ["servers", "subscriptions"] {
        for malformed in [
            serde_json::json!({"recoverable":"node"}),
            serde_json::json!("recoverable"),
            Value::Null,
        ] {
            let mut value = serde_json::json!({"servers":[],"subscriptions":[],"keepTrayMenuWarmDefaultMigrated":false});
            value[key] = malformed;
            assert!(sanitize_config(&value.to_string()).is_err());
            assert!(crate::ConfigStore::canonicalize_for_save(&value).is_err());
        }
    }
    assert!(sanitize_config("{}").is_ok());
}

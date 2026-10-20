use super::*;
use serde_json::json;
#[test]
fn references_are_strict_and_scopes_cannot_collide() {
    assert!(validate_raw(&json!({"autoSelectEnabled": true, "trafficRules":[{"targetAutoSelect":{"scope":"global"}}]})).is_ok());
    for invalid in [
        json!({"scope":"global","subscriptionId":"global"}),
        json!({"scope":"subscription","subscriptionId":""}),
        json!({"scope":"unknown"}),
    ] {
        assert!(validate_raw(&json!({"appRules":[{"targetAutoSelect":invalid}]})).is_err());
    }
    assert!(validate_raw(
        &json!({"appRules":[{"targetAutoSelect":{"scope":"global"},"targetServerId":"a"}]})
    )
    .is_err());
}
#[test]
fn default_off_and_pins_do_not_guess_or_deduplicate_silently_on_save() {
    assert!(!enabled(&json!({})));
    let pin = json!({"target":{"scope":"global"},"serverId":"a"});
    assert!(validate_raw(&json!({"autoSelectPins":[pin.clone(),pin]})).is_err());
}
#[test]
fn legacy_opt_in_migrates_once_without_inventing_a_pin() {
    let original = json!({"selectionIntent":{"mode":"auto","scope":"subscription","subscriptionId":"s"},"subscriptions":[{"id":"s"}],"selectedServerId":"stale-desired"});
    let mut config = original.clone();
    assert!(migrate_feature_flag(&mut config));
    assert_eq!(config[ENABLED_KEY], true);
    assert_eq!(config[PINS_KEY], json!([]));
    assert!(!migrate_feature_flag(&mut config));
    config[ENABLED_KEY] = Value::Bool(false);
    assert!(!migrate_feature_flag(&mut config));
    assert_eq!(config[ENABLED_KEY], false);
    for mut config in [
        json!({}),
        json!({"selectedServerId":"a"}),
        json!({"selectionIntent":{"mode":"future","scope":"subscription","subscriptionId":"s"},"subscriptions":[{"id":"s"}]}),
        json!({"selectionIntent":{"mode":"auto","scope":"global"}}),
        json!({"selectionIntent":{"mode":"auto","scope":"subscription","subscriptionId":"deleted"}}),
    ] {
        let intent = config.get(crate::SELECTION_INTENT_KEY).cloned();
        assert!(migrate_feature_flag(&mut config));
        assert_eq!(config[ENABLED_KEY], false);
        assert_eq!(config.get(crate::SELECTION_INTENT_KEY).cloned(), intent);
        assert_eq!(config[PINS_KEY], json!([]));
    }
}

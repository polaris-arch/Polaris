//! Native Auto storage boundary. References and verified pins are distinct values.
use std::collections::BTreeSet;
use polaris_config_engine::user_config::auto_select::{AutoSelectPin, AutoSelectTarget, SelectionIntent};
use serde_json::{Map, Value};
use crate::StoreError;

pub const ENABLED_KEY: &str = "autoSelectEnabled";
pub const PINS_KEY: &str = "autoSelectPins";

pub fn enabled(config: &Value) -> bool {
    config.get(ENABLED_KEY).and_then(Value::as_bool) == Some(true)
}

pub fn target(config: &Value) -> Option<AutoSelectTarget> {
    let intent: SelectionIntent = serde_json::from_value(config.get(crate::SELECTION_INTENT_KEY)?.clone()).ok()?;
    (intent.mode == "auto" && intent.target.valid()).then_some(intent.target)
}

pub fn intent(target: &AutoSelectTarget) -> Value {
    serde_json::to_value(SelectionIntent { mode: "auto".into(), target: target.clone() })
        .expect("Auto target serializes")
}

fn parse_target(value: &Value) -> Result<AutoSelectTarget, StoreError> {
    let target: AutoSelectTarget = serde_json::from_value(value.clone())
        .map_err(|_| StoreError::validation("invalid Auto target"))?;
    if !target.valid() { return Err(StoreError::validation("Auto subscriptionId must not be empty")); }
    Ok(target)
}

fn validate_route(route: &Value) -> Result<(), StoreError> {
    if let Some(target) = route.get("targetAutoSelect") {
        parse_target(target)?;
        if route.get("targetServerId").is_some_and(|value| !value.is_null()) {
            return Err(StoreError::validation("targetServerId and targetAutoSelect are mutually exclusive"));
        }
    }
    Ok(())
}

/// Before tolerant sanitization: malformed explicit routing must never turn into
/// follow-default, and duplicate pins must never depend on array order.
pub fn validate_raw(config: &Value) -> Result<(), StoreError> {
    if config.get(ENABLED_KEY).is_some_and(|value| !value.is_boolean()) {
        return Err(StoreError::validation("autoSelectEnabled must be boolean"));
    }
    if let Some(value) = config.get(PINS_KEY) {
        let pins = value.as_array().ok_or_else(|| StoreError::validation("autoSelectPins must be an array"))?;
        let mut seen = BTreeSet::new();
        for value in pins {
            let pin: AutoSelectPin = serde_json::from_value(value.clone())
                .map_err(|_| StoreError::validation("invalid Auto pin"))?;
            if !pin.target.valid() || pin.server_id.trim().is_empty() || !seen.insert(pin.target) {
                return Err(StoreError::validation("invalid or duplicate Auto pin"));
            }
        }
    }
    if let Some(value) = config.get(crate::SELECTION_INTENT_KEY) {
        if value.get("mode").and_then(Value::as_str) == Some("auto") {
            let mut target = value.clone();
            if let Some(object) = target.as_object_mut() { object.remove("mode"); }
            parse_target(&target)?;
        }
    }
    for key in ["customRules", "trafficRules", "policyRules", "appRules"] {
        for rule in config.get(key).and_then(Value::as_array).into_iter().flatten() {
            validate_route(rule)?;
            if let Some(route) = rule.get("effects").and_then(|effects| effects.get("route")) {
                validate_route(route)?;
            }
        }
    }
    Ok(())
}

/// Preserve an existing opt-in once, only for the exact legacy subscription
/// contract and an extant subscription. Explicit false is never overridden.
pub(crate) fn migrate_feature_flag(config: &mut Value) -> bool {
    if config.get(ENABLED_KEY).is_some() { return false; }
    let prior_opt_in=target(config).is_some_and(|target|match target {
        AutoSelectTarget::Subscription{subscription_id} => config.get("subscriptions").and_then(Value::as_array)
            .is_some_and(|subs|subs.iter().any(|sub|sub.get("id").and_then(Value::as_str)==Some(subscription_id.as_str()))),
        _=>false,
    });
    let Some(object)=config.as_object_mut() else { return false; };
    object.insert(ENABLED_KEY.into(),Value::Bool(prior_opt_in));
    object.entry(PINS_KEY).or_insert_with(||Value::Array(vec![]));
    true
}

/// Loading damaged storage may discard invalid pins, but never replaces an absent
/// scoped pin with a different node. Generation then explicitly rejects that scope.
pub(crate) fn sanitize(object: &mut Map<String, Value>) {
    if object.get(ENABLED_KEY).is_some_and(|value| !value.is_boolean()) {
        object.insert(ENABLED_KEY.into(), Value::Bool(false));
    }
    if let Some(value) = object.get_mut(PINS_KEY) {
        let mut seen = BTreeSet::new();
        let pins = value.as_array().into_iter().flatten().filter_map(|value| {
            let pin: AutoSelectPin = serde_json::from_value(value.clone()).ok()?;
            (pin.target.valid() && !pin.server_id.trim().is_empty() && seen.insert(pin.target.clone()))
                .then_some(value.clone())
        }).collect();
        *value = Value::Array(pins);
    }
}

#[cfg(test)]
mod tests {
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
        assert!(validate_raw(&json!({"appRules":[{"targetAutoSelect":{"scope":"global"},"targetServerId":"a"}]})).is_err());
    }
    #[test]
    fn default_off_and_pins_do_not_guess_or_deduplicate_silently_on_save() {
        assert!(!enabled(&json!({})));
        let pin = json!({"target":{"scope":"global"},"serverId":"a"});
        assert!(validate_raw(&json!({"autoSelectPins":[pin.clone(),pin]})).is_err());
    }
    #[test]
    fn legacy_opt_in_migrates_once_without_inventing_a_pin() {
        let original=json!({"selectionIntent":{"mode":"auto","scope":"subscription","subscriptionId":"s"},"subscriptions":[{"id":"s"}],"selectedServerId":"stale-desired"});
        let mut config=original.clone();
        assert!(migrate_feature_flag(&mut config));
        assert_eq!(config[ENABLED_KEY],true);
        assert_eq!(config[PINS_KEY],json!([]));
        assert!(!migrate_feature_flag(&mut config));
        config[ENABLED_KEY]=Value::Bool(false);
        assert!(!migrate_feature_flag(&mut config));
        assert_eq!(config[ENABLED_KEY],false);
        for mut config in [json!({}),json!({"selectedServerId":"a"}),
            json!({"selectionIntent":{"mode":"future","scope":"subscription","subscriptionId":"s"},"subscriptions":[{"id":"s"}]}),
            json!({"selectionIntent":{"mode":"auto","scope":"global"}}),
            json!({"selectionIntent":{"mode":"auto","scope":"subscription","subscriptionId":"deleted"}})] {
            let intent=config.get(crate::SELECTION_INTENT_KEY).cloned();
            assert!(migrate_feature_flag(&mut config));
            assert_eq!(config[ENABLED_KEY],false);
            assert_eq!(config.get(crate::SELECTION_INTENT_KEY).cloned(),intent);
            assert_eq!(config[PINS_KEY],json!([]));
        }
    }

}

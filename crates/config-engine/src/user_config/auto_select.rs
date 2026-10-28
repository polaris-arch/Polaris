//! Stable Auto references shared by default routing, rules, generation and runtime.
//! The feature flag controls execution; a reference never controls another group's state.
use super::{server_config::is_mesh_node, UserConfig};
use crate::builder::endpoint_routes::{is_mesh_node_unroutable, mesh_node_carries_full_tunnel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const TAG_PREFIX: &str = "polaris-auto-";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "camelCase", deny_unknown_fields)]
pub enum AutoSelectTarget {
    Global,
    Manual,
    Mesh,
    Subscription {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
}

impl AutoSelectTarget {
    pub fn valid(&self) -> bool {
        match self {
            Self::Subscription { subscription_id } => !subscription_id.trim().is_empty(),
            _ => true,
        }
    }
    fn key(&self) -> String {
        match self {
            Self::Global => "global".into(),
            Self::Manual => "manual".into(),
            Self::Mesh => "mesh".into(),
            Self::Subscription { subscription_id } => format!(
                "sub-{}",
                subscription_id
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSelectPin {
    pub target: AutoSelectTarget,
    pub server_id: String,
}

/// Legacy wire shape retained; only recognized `mode=auto` acts as a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionIntent {
    pub mode: String,
    #[serde(flatten)]
    pub target: AutoSelectTarget,
}

pub fn wrapper_tag(target: &AutoSelectTarget) -> String {
    format!("{TAG_PREFIX}{}", target.key())
}
pub fn urltest_tag(target: &AutoSelectTarget) -> String {
    format!("{}-test", wrapper_tag(target))
}
pub fn pin<'a>(config: &'a UserConfig, target: &AutoSelectTarget) -> Option<&'a str> {
    config
        .auto_select_pins
        .iter()
        .find(|p| &p.target == target)
        .map(|p| p.server_id.as_str())
}
pub fn eligible_members(config: &UserConfig, target: &AutoSelectTarget) -> Vec<String> {
    if !target.valid() {
        return vec![];
    }
    let subscriptions: BTreeSet<&str> = config
        .subscriptions
        .iter()
        .map(|sub| sub.id.as_str())
        .collect();
    config
        .servers
        .iter()
        .filter(|server| {
            let owned_subscription = server
                .subscription_id
                .as_ref()
                .filter(|id| subscriptions.contains(id.as_str()));
            let scope_matches = match target {
                AutoSelectTarget::Global => true,
                AutoSelectTarget::Manual => owned_subscription.is_none() && !is_mesh_node(server),
                AutoSelectTarget::Mesh => owned_subscription.is_none() && is_mesh_node(server),
                AutoSelectTarget::Subscription { subscription_id } => {
                    owned_subscription == Some(subscription_id)
                }
            };
            scope_matches
                && !is_mesh_node_unroutable(server)
                && mesh_node_carries_full_tunnel(server)
        })
        .map(|s| s.id.clone())
        .collect()
}
pub fn referenced_targets(config: &UserConfig) -> Vec<AutoSelectTarget> {
    let mut targets = BTreeSet::new();
    if let Some(intent) = &config.selection_intent {
        if intent.mode == "auto" {
            targets.insert(intent.target.clone());
        }
    }
    for rule in config.effective_traffic_rules() {
        if rule.enabled {
            if let Some(target) = rule.route_target_auto_select() {
                targets.insert(target.clone());
            }
        }
    }
    for rule in &config.app_rules {
        if rule.enabled && rule.action == super::RuleAction::Proxy {
            if let Some(target) = &rule.target_auto_select {
                targets.insert(target.clone());
            }
        }
    }
    targets.into_iter().collect()
}

/// Same interval as measurement scheduling; global follows its fastest member cohort.
pub fn interval_minutes(config: &UserConfig, target: &AutoSelectTarget) -> u64 {
    let intervals: std::collections::BTreeMap<&str, u64> = config
        .subscriptions
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.speed_test_interval_minutes
                    .filter(|n| (5..=360).contains(n))
                    .unwrap_or(30),
            )
        })
        .collect();
    let interval = |id: &str| intervals.get(id).copied().unwrap_or(30);
    match target {
        AutoSelectTarget::Subscription { subscription_id } => interval(subscription_id),
        AutoSelectTarget::Global => config
            .servers
            .iter()
            .filter(|s| !is_mesh_node_unroutable(s) && mesh_node_carries_full_tunnel(s))
            .map(|s| s.subscription_id.as_deref().map(interval).unwrap_or(30))
            .min()
            .unwrap_or(30),
        _ => 30,
    }
}
/// Selection is separate from admission: only absent/blank selects the default.
/// Invalid explicit values remain visible and can never turn into another URL.
pub fn test_url(config: &UserConfig) -> &str {
    configured_test_url(config.speed_test_url.as_deref())
}
pub fn configured_test_url(raw: Option<&str>) -> &str {
    raw.map(str::trim)
        .filter(|raw| !raw.is_empty())
        .unwrap_or("http://www.gstatic.com/generate_204")
}
pub const SPEED_TEST_URL_INVALID: &str = "SPEED_TEST_URL_INVALID";
pub fn validate_test_url(raw: Option<&str>) -> Result<(), &'static str> {
    let raw = configured_test_url(raw);
    if url::Url::parse(raw).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.port_or_known_default().is_some()
    }) {
        Ok(())
    } else {
        Err(SPEED_TEST_URL_INVALID)
    }
}

/// Reject ambiguous references before any route can silently follow the default.
pub fn validate_references(config: &UserConfig) -> Result<(), String> {
    let mut pins = BTreeSet::new();
    for value in &config.auto_select_pins {
        if !value.target.valid() || value.server_id.trim().is_empty() || !pins.insert(&value.target)
        {
            return Err("AUTO_SELECT_INVALID_PIN".into());
        }
    }
    for target in referenced_targets(config) {
        if !target.valid() {
            return Err("AUTO_SELECT_INVALID_SCOPE".into());
        }
    }
    for rule in config.effective_traffic_rules() {
        if rule.route_target_auto_select().is_some() && rule.route_target_server_id().is_some() {
            return Err("AUTO_SELECT_AMBIGUOUS_TARGET".into());
        }
    }
    for rule in &config.app_rules {
        if rule.target_auto_select.is_some() && rule.target_server_id.is_some() {
            return Err("AUTO_SELECT_AMBIGUOUS_TARGET".into());
        }
    }
    Ok(())
}

/// Projection only: unknown future intent remains untouched in raw persisted config.
pub fn deserialize_intent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SelectionIntent>, D::Error> {
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value::<SelectionIntent>(raw)
        .ok()
        .filter(|i| i.mode == "auto" && i.target.valid()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_namespace_is_injective_and_subscription_required() {
        assert_ne!(
            wrapper_tag(&AutoSelectTarget::Global),
            wrapper_tag(&AutoSelectTarget::Subscription {
                subscription_id: "global".into()
            })
        );
        assert_ne!(
            wrapper_tag(&AutoSelectTarget::Subscription {
                subscription_id: "a/b".into()
            }),
            wrapper_tag(&AutoSelectTarget::Subscription {
                subscription_id: "a-b".into()
            })
        );
        assert!(serde_json::from_value::<AutoSelectTarget>(
            serde_json::json!({"scope":"subscription"})
        )
        .is_err());
        assert!(serde_json::from_value::<AutoSelectTarget>(
            serde_json::json!({"scope":"global","subscriptionId":"sub"})
        )
        .is_err());
    }
    #[test]
    fn feature_defaults_off_and_unknown_intent_does_not_break_projection() {
        let config = UserConfig::default();
        assert!(!config.auto_select_enabled);
        let mut raw = serde_json::to_value(config).unwrap();
        raw["selectionIntent"] = serde_json::json!({"mode":"future","scope":"future"});
        let projected: UserConfig = serde_json::from_value(raw).unwrap();
        assert!(projected.selection_intent.is_none());
    }
    #[test]
    fn legacy_subscription_intent_remains_a_reference() {
        let mut raw = serde_json::to_value(UserConfig::default()).unwrap();
        raw["selectionIntent"] =
            serde_json::json!({"mode":"auto","scope":"subscription","subscriptionId":"s"});
        let projected: UserConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(
            projected.selection_intent.unwrap().target,
            AutoSelectTarget::Subscription {
                subscription_id: "s".into()
            }
        );
    }
    #[test]
    fn invalid_explicit_endpoint_is_rejected_without_substitution() {
        let mut config = UserConfig::default();
        for invalid in ["http://", "tcp://example.com", "https://example.com:99999"] {
            config.speed_test_url = Some(invalid.into());
            assert_eq!(test_url(&config), invalid);
            assert_eq!(
                validate_test_url(config.speed_test_url.as_deref()),
                Err(SPEED_TEST_URL_INVALID)
            );
        }
        for default in [None, Some(""), Some("  ")] {
            assert!(validate_test_url(default).is_ok());
            assert_eq!(
                configured_test_url(default),
                "http://www.gstatic.com/generate_204"
            );
        }
        config.speed_test_url = Some("https://example.com/check".into());
        assert_eq!(test_url(&config), "https://example.com/check");
    }
}

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
fn every_scope_rejects_foreign_fields_and_round_trips_through_intent() {
    for scope in ["global", "manual", "mesh", "subscription"] {
        let mut target = serde_json::json!({"scope": scope});
        if scope == "subscription" {
            target["subscriptionId"] = serde_json::json!("s");
        }
        let decoded: AutoSelectTarget = serde_json::from_value(target.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), target);
        let mut intent = target.clone();
        intent["mode"] = serde_json::json!("auto");
        assert_eq!(
            serde_json::from_value::<SelectionIntent>(intent)
                .unwrap()
                .target,
            decoded
        );
        target["foreign"] = serde_json::json!(true);
        assert!(serde_json::from_value::<AutoSelectTarget>(target).is_err());
        if scope != "subscription" {
            for extra in [serde_json::json!("s"), serde_json::Value::Null] {
                assert!(serde_json::from_value::<AutoSelectTarget>(
                    serde_json::json!({"scope":scope,"subscriptionId":extra})
                )
                .is_err());
            }
        }
    }
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

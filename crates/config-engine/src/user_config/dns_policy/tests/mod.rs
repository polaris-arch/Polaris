use super::*;

#[test]
fn outbound_is_a_strict_tagged_union() {
    let node: DnsServerOutbound = serde_json::from_str(r#"{"type":"node","nodeId":"n1"}"#).unwrap();
    assert_eq!(
        node,
        DnsServerOutbound::Node {
            node_id: "n1".into()
        }
    );
    assert!(serde_json::from_str::<DnsServerOutbound>(r#"{"type":"unknown"}"#).is_err());
}

#[test]
fn hosts_first_roundtrips_without_losing_fallback() {
    let action = DnsPolicyAction::HostsFirst {
        hosts_server_id: "hosts".into(),
        fallback: Box::new(DnsPolicyAction::Server {
            server_id: "real".into(),
        }),
    };
    let json = serde_json::to_value(&action).unwrap();
    assert_eq!(
        serde_json::from_value::<DnsPolicyAction>(json).unwrap(),
        action
    );
}

#[test]
fn real_query_fallback_is_optional_and_round_trips_explicit_choices() {
    let legacy: DnsPolicyDefaults = serde_json::from_str("{}").unwrap();
    assert!(legacy.real_query_fallback_action.is_none());
    assert!(serde_json::to_value(&legacy)
        .unwrap()
        .get("realQueryFallbackAction")
        .is_none());

    for action in [
        serde_json::json!({"type":"server","serverId":"builtin-remote"}),
        serde_json::json!({"type":"reject","method":"default"}),
        serde_json::json!({"type":"reject","method":"drop"}),
    ] {
        let raw = serde_json::json!({"realQueryFallbackAction":action});
        let defaults: DnsPolicyDefaults = serde_json::from_value(raw).unwrap();
        assert!(defaults
            .real_query_fallback_action
            .as_ref()
            .unwrap()
            .is_valid_real_query_fallback());
        assert_eq!(
            serde_json::to_value(&defaults).unwrap()["realQueryFallbackAction"],
            action
        );
    }
}

#[test]
fn real_query_fallback_excludes_composite_and_route_dependent_actions() {
    for raw in [
        serde_json::json!({"type":"fakeIp"}),
        serde_json::json!({"type":"group","groupId":"race"}),
        serde_json::json!({"type":"hostsFirst","hostsServerId":"hosts",
            "fallback":{"type":"server","serverId":"builtin-remote"}}),
        serde_json::json!({"type":"predefined","rcode":"NOERROR"}),
        serde_json::json!({"type":"followRouteDefault"}),
    ] {
        let action: DnsPolicyAction = serde_json::from_value(raw).unwrap();
        assert!(!action.is_valid_real_query_fallback(), "{action:?}");
    }
}

mod audit_regressions;

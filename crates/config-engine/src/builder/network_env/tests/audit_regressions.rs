use super::super::*;
#[test]
fn default_dns_alone_requires_available_dhcp_resolver() {
    let config: UserConfig = serde_json::from_value(serde_json::json!({"configSchemaVersion":4,"servers":[],"proxyMode":"smart","proxyModeType":"systemProxy","dnsDefaults":{"unmatchedAction":{"type":"server","serverId":"builtin-netenv-dhcp"}}})).unwrap();
    assert!(default_dns_uses_netenv(&config));
    let available = ProbeFacts {
        platform: Platform::Win,
        tun: false,
        takeover_active: false,
        dhcp_suppressed: false,
    };
    assert!(netenv_required(&config, &available));
    assert!(!netenv_required(
        &config,
        &ProbeFacts {
            dhcp_suppressed: true,
            ..available
        }
    ));
    assert!(!netenv_required(
        &config,
        &ProbeFacts {
            platform: Platform::Linux,
            ..available
        }
    ));
}

#[test]
fn default_dns_group_fallback_is_part_of_dependency_traversal() {
    let config: UserConfig = serde_json::from_value(serde_json::json!({"configSchemaVersion":4,"servers":[],"proxyMode":"smart","proxyModeType":"systemProxy","dnsDefaults":{"unmatchedAction":{"type":"group","groupId":"g"}},"dnsServerGroups":[{"id":"g","members":[],"fallbackServerId":"builtin-netenv-dhcp"}]})).unwrap();
    assert!(default_dns_uses_netenv(&config));
    let mut disabled = config.clone();
    disabled.dns_server_groups[0].enabled = false;
    assert!(!default_dns_uses_netenv(&disabled));
    let mut legacy = config;
    legacy.config_schema_version = Some(1);
    assert!(!default_dns_uses_netenv(&legacy));
}

#[test]
fn fakeip_real_query_fallback_participates_only_when_active() {
    let mut config: UserConfig = serde_json::from_value(serde_json::json!({
        "configSchemaVersion": 4, "servers": [], "proxyMode": "direct",
        "dnsDefaults": {
            "unmatchedAction": {"type": "fakeIp"},
            "realQueryFallbackAction": {"type": "server", "serverId": "builtin-netenv-dhcp"}
        }
    }))
    .unwrap();
    assert!(default_dns_uses_netenv(&config));
    let available = ProbeFacts {
        platform: Platform::Win,
        tun: false,
        takeover_active: false,
        dhcp_suppressed: false,
    };
    assert!(netenv_required(&config, &available));
    assert!(!netenv_required(
        &config,
        &ProbeFacts {
            dhcp_suppressed: true,
            ..available
        }
    ));
    config.dns_defaults.as_mut().unwrap().unmatched_action = Some(DnsPolicyAction::Server {
        server_id: "builtin-remote".into(),
    });
    assert!(!default_dns_uses_netenv(&config));
    config.dns_defaults.as_mut().unwrap().unmatched_action = Some(DnsPolicyAction::FakeIp);
    config.config_schema_version = Some(1);
    assert!(!default_dns_uses_netenv(&config));
}

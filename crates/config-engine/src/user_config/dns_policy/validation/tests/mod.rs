use super::*;

#[test]
fn protected_resources_cannot_be_removed_or_disabled_by_direct_input() {
    for id in [
        BUILTIN_DOMESTIC_DNS_ID,
        BUILTIN_REMOTE_DNS_ID,
        BUILTIN_BOOTSTRAP_DNS_ID,
    ] {
        for remove in [true, false] {
            let mut config = UserConfig::default();
            if remove {
                config.dns_servers.retain(|server| server.id != id);
            } else {
                config
                    .dns_servers
                    .iter_mut()
                    .find(|server| server.id == id)
                    .unwrap()
                    .enabled = false;
            }
            let before = serde_json::to_value(&config).unwrap();
            assert!(validate_dns_dependencies(&config).unwrap_err().contains(id));
            assert_eq!(
                serde_json::to_value(&config).unwrap(),
                before,
                "validation must not repair the input"
            );
        }
    }
}

#[test]
fn custom_defaults_and_unused_disabled_custom_resources_remain_valid() {
    let mut config = UserConfig::default();
    config.dns_servers.push(
        serde_json::from_value(serde_json::json!({
            "id":"custom", "name":"Custom", "enabled":true, "type":"udp",
            "endpoint":{"host":"9.9.9.9","port":53}, "outbound":{"type":"direct"}
        }))
        .unwrap(),
    );
    config.dns_servers.push(
        serde_json::from_value(serde_json::json!({
            "id":"unused", "name":"Unused", "enabled":false, "type":"udp",
            "outbound":{"type":"direct"}
        }))
        .unwrap(),
    );
    config.dns_defaults = Some(
        serde_json::from_value(serde_json::json!({
            "directServerId":"custom", "proxyServerId":"custom",
            "unmatchedAction":{"type":"server","serverId":"custom"},
            "realQueryFallbackAction":{"type":"server","serverId":"custom"}
        }))
        .unwrap(),
    );
    assert!(validate_dns_dependencies(&config).is_ok());
}

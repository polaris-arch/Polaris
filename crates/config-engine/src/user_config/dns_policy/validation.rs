//! Shared persistence/compiler boundary for explicit DNS dependencies.
//! Never repair a missing policy dependency by silently choosing another resolver.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::user_config::app_config::UserConfig;
use crate::user_config::network_profile::BUILTIN_NETENV_DHCP_ID;

pub fn validate_dns_dependencies(config: &UserConfig) -> Result<(), String> {
    let servers: BTreeMap<&str, &DnsServerResource> = config
        .dns_servers
        .iter()
        .map(|server| (server.id.as_str(), server))
        .collect();
    let require_server = |owner: &str, id: &str| -> Result<(), String> {
        // This transport is generated from current network facts, not persisted.
        if id == BUILTIN_NETENV_DHCP_ID {
            return Ok(());
        }
        if servers.get(id).is_some_and(|server| server.enabled) {
            Ok(())
        } else {
            Err(format!("DNS {owner} 引用的解析器「{id}」不存在或已禁用"))
        }
    };

    // The three protected resources are the shared bootstrap/default carriers:
    // existing UI disallows deleting or disabling them. Enforce the same boundary
    // for direct saves, restores and every full compiler entry, without inventing
    // an endpoint here. Legacy load migration materializes these explicitly.
    for id in [
        BUILTIN_DOMESTIC_DNS_ID,
        BUILTIN_REMOTE_DNS_ID,
        BUILTIN_BOOTSTRAP_DNS_ID,
    ] {
        require_server("受保护内置资源", id)?;
    }

    // All enabled transports are emitted even when not used by the default action.
    // Check roots and cycles once; a disabled dependency must not reach the kernel.
    let node_ids: BTreeSet<&str> = config.servers.iter().map(|node| node.id.as_str()).collect();
    for server in config.dns_servers.iter().filter(|server| server.enabled) {
        if let DnsServerOutbound::Node { node_id } = &server.outbound {
            if !node_ids.contains(node_id.as_str()) {
                return Err(format!(
                    "DNS 解析器「{}」引用的出口节点「{node_id}」不存在",
                    server.id
                ));
            }
        }
    }
    let mut complete = BTreeSet::new();
    for server in config.dns_servers.iter().filter(|server| server.enabled) {
        let mut path = BTreeSet::new();
        let mut current = server.id.as_str();
        while !complete.contains(current) {
            if !path.insert(current) {
                return Err(format!(
                    "DNS 解析器「{}」的 Bootstrap 依赖存在循环（{current}）",
                    server.id
                ));
            }
            let resource = servers[current];
            if resource.id == BUILTIN_BOOTSTRAP_DNS_ID && !is_valid_bootstrap_dns_resource(resource)
            {
                return Err("DNS builtin-bootstrap 必须使用系统解析或直连 IP 端点".into());
            }
            if matches!(resource.kind, DnsServerKind::Local | DnsServerKind::Hosts) {
                break;
            }
            let endpoint = resource
                .endpoint
                .as_ref()
                .filter(|endpoint| !endpoint.host.trim().is_empty())
                .ok_or_else(|| format!("DNS 解析器「{}」缺少端点", resource.id))?;
            if endpoint.port == Some(0) {
                return Err(format!("DNS 解析器「{}」端口不能为 0", resource.id));
            }
            if endpoint
                .host
                .trim()
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok()
            {
                break;
            }
            let bootstrap = resource
                .bootstrap_server_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| format!("DNS 解析器「{}」缺少 Bootstrap", resource.id))?;
            require_server(&format!("解析器「{}」Bootstrap", resource.id), bootstrap)?;
            if bootstrap == BUILTIN_NETENV_DHCP_ID {
                return Err(format!(
                    "DNS 解析器「{}」不能以动态 DHCP 作为 Bootstrap",
                    resource.id
                ));
            }
            if servers[bootstrap].kind == DnsServerKind::Hosts {
                return Err(format!(
                    "DNS 解析器「{}」的 Bootstrap「{bootstrap}」不能是 Hosts",
                    resource.id
                ));
            }
            current = bootstrap;
        }
        complete.extend(path);
    }

    let groups: BTreeMap<&str, &DnsServerGroup> = config
        .dns_server_groups
        .iter()
        .map(|group| (group.id.as_str(), group))
        .collect();
    for group in config
        .dns_server_groups
        .iter()
        .filter(|group| group.enabled)
    {
        let owner = format!("组「{}」", group.id);
        if group.members.is_empty() {
            return Err(format!("DNS {owner} 没有成员"));
        }
        for member in &group.members {
            require_server(&owner, member)?;
        }
        if let Some(fallback) = &group.fallback_server_id {
            require_server(&owner, fallback)?;
        }
    }

    let validate_action = |owner: &str, action: &DnsPolicyAction| -> Result<(), String> {
        let mut action = action;
        loop {
            match action {
                DnsPolicyAction::Server { server_id } => return require_server(owner, server_id),
                DnsPolicyAction::Group { group_id } => {
                    return if groups
                        .get(group_id.as_str())
                        .is_some_and(|group| group.enabled)
                    {
                        Ok(())
                    } else {
                        Err(format!("DNS {owner} 引用的组「{group_id}」不存在或已禁用"))
                    };
                }
                DnsPolicyAction::HostsFirst {
                    hosts_server_id,
                    fallback,
                } => {
                    require_server(owner, hosts_server_id)?;
                    if servers
                        .get(hosts_server_id.as_str())
                        .is_none_or(|server| server.kind != DnsServerKind::Hosts)
                    {
                        return Err(format!(
                            "DNS {owner} 的 Hosts 资源「{hosts_server_id}」类型无效"
                        ));
                    }
                    action = fallback;
                }
                _ => return Ok(()),
            }
        }
    };
    if let Some(defaults) = &config.dns_defaults {
        for (name, id) in [
            ("默认直连", &defaults.direct_server_id),
            ("默认代理", &defaults.proxy_server_id),
        ] {
            if !id.is_empty() {
                require_server(name, id)?;
            }
        }
        for (name, action) in [
            ("默认策略", &defaults.unmatched_action),
            ("真实查询回退", &defaults.real_query_fallback_action),
        ] {
            if let Some(action) = action {
                validate_action(name, action)?;
            }
        }
    }
    for rule in config
        .effective_dns_rules()
        .iter()
        .filter(|rule| rule.enabled)
    {
        if let Some(action) = rule.dns_effect().and_then(|effect| effect.action) {
            validate_action(&format!("规则「{}」", rule.id), &action)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
}

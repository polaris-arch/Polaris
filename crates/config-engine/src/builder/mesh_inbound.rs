//! 用户态 endpoint 入站授权。只消费最终已发射的 endpoint，不推测配置请求会否降级为 userspace。

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::builder::system_interfaces::endpoint_requests_system_interface;
use crate::singbox::{DnsRule, Endpoint, OneOrMany, RouteRule, SingBoxConfig};
use crate::user_config::app_config::UserConfig;
use crate::user_config::rules::parse_port_values;
use crate::user_config::server_config::{
    lands_in_endpoints, validate_mesh_inbound_policy, MeshInboundGrant, MeshInboundNetwork,
    MeshInboundPolicy, MeshInboundTarget,
};

/// 上层只据此固定生成错误分类，绝不解析自由文本或内核 stderr。
pub const SYSTEM_INTERFACE_POLICY_ERROR: &str =
    "mesh-inbound-system-interface-requires-external-firewall";

fn ingress(tag: &str) -> OneOrMany<String> {
    OneOrMany::Many(vec![tag.to_owned()])
}

fn reject(tag: &str, destinations: Option<Vec<String>>) -> RouteRule {
    RouteRule {
        inbound: Some(ingress(tag)),
        ip_cidr: destinations,
        action: Some("reject".into()),
        no_drop: Some(true),
        ..RouteRule::default()
    }
}

fn legacy_local_direct(tag: &str) -> RouteRule {
    // 五种 userspace endpoint 的内核实现都会把「本机分配地址」映射到双栈回环。
    // 这条只作用于该 endpoint 入站，避免选中全隧道节点时把 ::1 再拨回自身。
    RouteRule {
        inbound: Some(ingress(tag)),
        ip_cidr: Some(vec!["127.0.0.1/32".into(), "::1/128".into()]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        ..RouteRule::default()
    }
}

/// 原有 DNS sniff/hijack 与用户 traffic/app 规则之后、通用出口与 mesh force-route 之前插入。
/// 只依据 outbounds builder 已发射的 endpoint；未发射/system/WARP 都没有可承接的用户态入站。
pub(crate) fn legacy_userspace_local_routes(
    config: &UserConfig,
    id_to_tag: &BTreeMap<String, String>,
    emitted: &[Endpoint],
) -> Vec<RouteRule> {
    config
        .servers
        .iter()
        .filter(|server| {
            server.mesh_inbound_policy.is_none()
                && lands_in_endpoints(server.protocol)
                && !crate::warp::is_warp_server(server)
        })
        .filter_map(|server| {
            let tag = id_to_tag.get(&server.id)?;
            let endpoint = emitted.iter().find(|endpoint| &endpoint.tag == tag)?;
            (!endpoint_requests_system_interface(endpoint)).then(|| legacy_local_direct(tag))
        })
        .collect()
}

fn grants(tag: &str, rule: &MeshInboundGrant) -> Vec<RouteRule> {
    let network = match rule.network {
        MeshInboundNetwork::Tcp => vec!["tcp".into()],
        MeshInboundNetwork::Udp => vec!["udp".into()],
        MeshInboundNetwork::Both => vec!["tcp".into(), "udp".into()],
    };
    let destinations = match rule.target {
        MeshInboundTarget::Local => vec!["127.0.0.1/32".into(), "::1/128".into()],
        MeshInboundTarget::Forward => rule
            .target_cidrs
            .iter()
            .map(|c| c.trim().to_owned())
            .collect(),
    };
    // 单端口与范围分开：不依赖 sing-box 对 port + port_range 同时出现时的组合语义。
    rule.ports
        .iter()
        .map(|token| {
            let (ports, ranges) = parse_port_values(std::slice::from_ref(token));
            RouteRule {
                inbound: Some(ingress(tag)),
                source_ip_cidr: Some(
                    rule.source_cidrs
                        .iter()
                        .map(|c| c.trim().to_owned())
                        .collect(),
                ),
                ip_cidr: Some(destinations.clone()),
                network: Some(network.clone()),
                port: (!ports.is_empty()).then_some(OneOrMany::Many(ports)),
                port_range: (!ranges.is_empty()).then_some(ranges),
                action: Some("route".into()),
                outbound: Some("direct".into()),
                ..RouteRule::default()
            }
        })
        .collect()
}

/// 在 network canary 与所有通用规则写完后调用；最终 `route.rules` / `dns.rules` 均绝对置顶。
pub fn apply_mesh_inbound_policies(
    config: &UserConfig,
    id_to_tag: &BTreeMap<String, String>,
    singbox: &mut SingBoxConfig,
) -> Result<(), String> {
    let mut route_head = Vec::new();
    let mut dns_head = Vec::new();
    for server in &config.servers {
        let Some(policy) = server.mesh_inbound_policy.as_ref() else {
            continue;
        };
        validate_mesh_inbound_policy(server).map_err(str::to_owned)?;
        let Some(tag) = id_to_tag.get(&server.id) else {
            // 被 detour/配置门剔除的节点没有可接收的 endpoint；既有 invalid report 负责告知。
            continue;
        };
        let Some(endpoint) = singbox
            .endpoints
            .as_ref()
            .and_then(|all| all.iter().find(|ep| &ep.tag == tag))
        else {
            // 未发射的 endpoint 无入站路径（已由构造门另行报告），不发孤儿授权规则。
            continue;
        };
        if endpoint_requests_system_interface(endpoint) {
            return Err(SYSTEM_INTERFACE_POLICY_ERROR.into());
        }
        if let MeshInboundPolicy::Allowlist { rules } = policy {
            for rule in rules
                .iter()
                .filter(|r| r.target == MeshInboundTarget::Local)
            {
                route_head.extend(grants(tag, rule));
            }
            // 即使目标段是 0/0，也不能借目标 CIDR 授权穿透隧道地址→loopback 的本机服务。
            route_head.push(reject(
                tag,
                Some(vec![
                    "127.0.0.0/8".into(),
                    "::1/128".into(),
                    "::ffff:127.0.0.0/104".into(),
                    "0.0.0.0/32".into(),
                    "::/128".into(),
                    "::ffff:0.0.0.0/128".into(),
                ]),
            ));
            for rule in rules
                .iter()
                .filter(|r| r.target == MeshInboundTarget::Forward)
            {
                route_head.extend(grants(tag, rule));
            }
        }
        route_head.push(reject(tag, None));
        // NewDNSPacket 不走 route；先于缓存与通用 DNS 规则拒绝该 endpoint 的解析请求。
        dns_head.push(DnsRule {
            inbound: Some(ingress(tag)),
            action: Some("reject".into()),
            method: Some("default".into()),
            no_drop: Some(true),
            ..DnsRule::default()
        });
    }
    if !route_head.is_empty() {
        let route = singbox.route.as_mut().ok_or("mesh-inbound-route-missing")?;
        route_head.append(&mut route.rules);
        route.rules = route_head;
    }
    if !dns_head.is_empty() {
        let dns = singbox.dns.as_mut().ok_or("mesh-inbound-dns-missing")?;
        dns_head.extend(dns.rules.take().unwrap_or_default());
        dns.rules = Some(dns_head);
    }
    Ok(())
}

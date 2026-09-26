use super::{num, str};
use polaris_config_engine::builder::endpoint_routes::{has_catch_all, strip_catch_all};
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, WireGuardSettings,
};
use serde_yaml::Value;
use std::net::IpAddr;

fn required_text(node: &Value, key: &str) -> Result<String, String> {
    node.get(key)
        .and_then(str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("wireguard 缺 {key}"))
}

fn port(node: &Value) -> Result<u16, String> {
    node.get("port")
        .and_then(num)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| "wireguard 缺合法 port".into())
}

fn local_address(node: &Value, key: &str, ipv6: bool) -> Result<Option<String>, String> {
    let Some(raw) = node.get(key).and_then(str) else {
        return Ok(None);
    };
    let raw = raw.trim();
    let (ip, prefix) = match raw.split_once('/') {
        Some((ip, prefix)) => (ip, prefix.parse::<u8>().ok()),
        None => (raw, Some(if ipv6 { 128 } else { 32 })),
    };
    let parsed = ip
        .parse::<IpAddr>()
        .map_err(|_| format!("wireguard {key} 不是 IP 地址"))?;
    if parsed.is_ipv6() != ipv6 || prefix.is_none_or(|p| p > if ipv6 { 128 } else { 32 }) {
        return Err(format!("wireguard {key} CIDR 非法"));
    }
    Ok(Some(format!("{ip}/{}", prefix.unwrap())))
}

fn string_list(node: &Value, key: &str) -> Result<Vec<String>, String> {
    match node.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("wireguard {key} 必须是字符串数组"))
            })
            .collect(),
        _ => Err(format!("wireguard {key} 必须是字符串数组")),
    }
}

pub(super) fn parse_wireguard_proxy(
    node: &Value,
    subscription_id: &str,
    now: &str,
    id: String,
) -> Result<ServerConfig, String> {
    if node.get("amnezia-wg-option").is_some() {
        return Err("wireguard 的 AmneziaWG 变体与 sing-box WireGuard 不兼容".into());
    }
    if node.get("remote-dns-resolve").and_then(super::bool_val) == Some(true)
        || node.get("dns").is_some()
    {
        return Err("wireguard 的 mihomo 专属远端 DNS 设置无法保真".into());
    }
    if node.get("ip-stack").is_some() || node.get("refresh-server-ip-interval").is_some() {
        return Err("wireguard 的 mihomo IP 栈/刷新选项无法保真".into());
    }
    if node.get("udp").and_then(super::bool_val) == Some(false) {
        return Err("wireguard 的 udp:false 无法在 Polaris 单 peer 模型保真".into());
    }

    let mut addresses = Vec::new();
    if let Some(ip) = local_address(node, "ip", false)? {
        addresses.push(ip);
    }
    if let Some(ip) = local_address(node, "ipv6", true)? {
        addresses.push(ip);
    }
    if addresses.is_empty() {
        return Err("wireguard 缺 ip/ipv6 本地地址".into());
    }

    let private_key = required_text(node, "private-key")?;
    let workers = match node.get("workers") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .or_else(|| value.as_str().and_then(|s| s.parse::<u32>().ok()))
                .ok_or("wireguard workers 必须是非负整数")?,
        ),
    };
    let peers = match node.get("peers") {
        None => None,
        Some(Value::Sequence(peers)) => Some(peers),
        Some(_) => return Err("wireguard peers 必须是数组".into()),
    };
    let peer = match peers.map(Vec::as_slice) {
        None => node,
        Some([single]) => single,
        Some(_) => return Err("wireguard 多 peer 无法映射到 Polaris 单 peer 节点".into()),
    };
    let server = required_text(peer, "server")?;
    let server_port = port(peer)?;
    let peer_public_key = required_text(peer, "public-key")?;
    let pre_shared_key = peer
        .get("pre-shared-key")
        .and_then(str)
        .filter(|s| !s.is_empty());
    let allowed = string_list(peer, "allowed-ips")?;
    if peers.is_some() && allowed.is_empty() {
        return Err("wireguard peers[0] 缺 allowed-ips".into());
    }
    let allow_internet = peers.is_none() || has_catch_all(&allowed);
    let specific = strip_catch_all(&allowed);
    let mut settings = WireGuardSettings {
        private_key: Some(private_key),
        local_address: addresses,
        peer_public_key: Some(peer_public_key),
        pre_shared_key,
        allowed_ips: specific,
        allow_internet: Some(allow_internet),
        // mihomo 未设置时关闭保活；Polaris 默认 25s，显式 0 才能保真。
        persistent_keepalive: Some(node.get("persistent-keepalive").and_then(num).unwrap_or(0)),
        mtu: node.get("mtu").and_then(num),
        workers,
        ..Default::default()
    };
    if let Some(raw) = peer.get("reserved") {
        let values = raw
            .as_sequence()
            .ok_or("wireguard reserved 必须是三个字节")?;
        if values.len() != 3 {
            return Err("wireguard reserved 必须是三个字节".into());
        }
        settings.reserved = values
            .iter()
            .map(|v| {
                num(v)
                    .filter(|n| *n <= 255)
                    .ok_or_else(|| "wireguard reserved 必须是三个字节".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(ServerConfig {
        id,
        name: node
            .get("name")
            .and_then(str)
            .unwrap_or_else(|| format!("{server}:{server_port}")),
        protocol: Protocol::Wireguard,
        address: server,
        port: server_port,
        wireguard_settings: Some(Box::new(settings)),
        subscription_id: Some(subscription_id.to_string()),
        created_at: Some(now.to_string()),
        updated_at: Some(now.to_string()),
        ..Default::default()
    })
}

//! WireGuard/Tailscale endpoint 构造（上游 `buildWireGuardEndpoint` + `buildTailscaleEndpoint`）。

#![forbid(unsafe_code)]

use crate::builder::endpoint_routes::{
    mesh_node_carries_full_tunnel, mesh_uses_system_interface, wireguard_peer_allowed_ips,
    TS_SYSTEM_INTERFACE_NAME, WG_SYSTEM_INTERFACE_NAME,
};
use crate::singbox::{DomainResolver, Endpoint, WireGuardPeer};
use crate::user_config::ip::is_ip_literal;
use crate::user_config::server_config::{Protocol, ServerConfig};

/// Taildrop 收件目录相对 `state_dir` 的子目录名。取与内核默认值相同的字面量（`"Taildrop"`），
/// 差别只在**我们把它锚成绝对路径**、不让它跟着 CWD 漂 —— 见
/// [`crate::singbox::Endpoint::taildrop_directory`]。
const TAILDROP_SUBDIR: &str = "Taildrop";

/// OpenConnect / OpenVPN 客户端 endpoint 的共享构造器。
/// 两者的设置结构 serde 名就是 sing-box wire 键，因此整体 flatten 到 `extra`；主核与临时测速核
/// 必须走同一入口，防止后者只生成 `type/tag` 空壳却仍被误判为“可测”。
pub fn build_vpn_client_endpoint(
    server: &ServerConfig,
    tag: &str,
    domain_resolver: Option<&DomainResolver>,
) -> Result<Endpoint, String> {
    let payload = match server.protocol {
        Protocol::Openconnect => server
            .openconnect_settings
            .as_ref()
            .and_then(|settings| serde_json::to_value(settings).ok()),
        Protocol::OpenvpnClient => server
            .openvpn_client_settings
            .as_ref()
            .and_then(|settings| serde_json::to_value(settings).ok()),
        _ => return Err("节点不是 OpenConnect/OpenVPN 客户端 endpoint".to_owned()),
    };
    let extra = match payload {
        Some(serde_json::Value::Object(extra)) => extra,
        _ => serde_json::Map::new(),
    };
    Ok(Endpoint {
        type_field: crate::builder::outbound::protocol_str(server.protocol),
        tag: tag.to_owned(),
        domain_resolver: domain_resolver.cloned(),
        extra,
        ..Default::default()
    })
}

/// MASQUE 节点 `path` 非法（非空且不以 `/` 开头）的 reason token。
pub const INVALID_REASON_MASQUE_PATH: &str = "masque-path-invalid";
/// MASQUE 节点 `version` 不在内核接受的 0–3 之内的 reason token。
pub const INVALID_REASON_MASQUE_VERSION: &str = "masque-version-invalid";

/// 生成侧恒不下发、透传袋里出现也剥掉的键。
///
/// - `system` / `name`：系统网卡模式与 Polaris 自己的 TUN（默认路由归它）、helper 提权模型
///   （系统代理与测速临时核都不提权起核）冲突，且内核这支不装路由、没有流量会主动进那块网卡；
///   固定走内部栈（未设置 = 内核缺省 false）。
/// - `advertise_routes`：请服务端把这些前缀打进本机当入站处理，而本仓 route 规则不按 endpoint 入站
///   区分 ⇒ 平白多出一条入站暴露面；它还在 `CARRY_TRAFFIC_KEYS` 里，会改变承流判定。
/// - 其余是生成侧自己写的键（顶层字段 / endpoint 具名字段）：袋里再留一份会与具名值重复或打架。
const MASQUE_STRIPPED_KEYS: &[&str] = &[
    "system",
    "name",
    "advertise_routes",
    "type",
    "tag",
    "server",
    "server_port",
    "username",
    "password",
    "tls",
    "detour",
    "domain_resolver",
];

/// HTTP/2 调优键（sing-box `option/http.go` 的 HTTP2Options，以 v1.15.0-alpha.7 为准）。
/// `version: 1` 下出现即 decode 失败、整核起不来（随包核实测）。
const MASQUE_H2_KEYS: &[&str] = &[
    "idle_timeout",
    "keep_alive_period",
    "stream_receive_window",
    "connection_receive_window",
    "max_concurrent_streams",
];
/// QUIC 独有键（QUICOptions 在 HTTP2Options 之上多出的两个）。`version: 1/2` 下出现即 decode 失败。
const MASQUE_QUIC_ONLY_KEYS: &[&str] = &["initial_packet_size", "disable_path_mtu_discovery"];

/// 该版本下内核不认、必须剥掉的调优键。缺省 / 0 / 3 同时接受两组（schema 把两者铺平）。
fn masque_incompatible_keys(version: Option<u32>) -> Vec<&'static str> {
    match version {
        Some(1) => [MASQUE_H2_KEYS, MASQUE_QUIC_ONLY_KEYS].concat(),
        Some(2) => MASQUE_QUIC_ONLY_KEYS.to_vec(),
        _ => Vec::new(),
    }
}

/// MASQUE 客户端 endpoint 构造。主核发射腿、测速临时核、`selected_server_precludes_selector_fallback`
/// 共用本函数：剔节点的判据只有这一份，三处不会漂移。
///
/// 失败即关闭：`Err(reason token)` 表示该节点会让**整份配置**起不来（内核 initialize/decode 失败），
/// 调用方须剔除并上报，而不是下发。版本与调优键不兼容**不剔节点**，只剥键并经 `log` 记 Warn ——
/// 用户切版本是合法操作，残留的调优键不该让节点消失。
///
/// 缺 `masqueClientSettings` 按缺省处理：必需内容都在顶层，空设置本来就是完整节点。
pub fn build_masque_endpoint(
    server: &ServerConfig,
    tag: &str,
    domain_resolver: Option<&DomainResolver>,
    detour_tag: Option<&str>,
    log: fn(crate::user_config::log_level::LogLevel, &str),
) -> Result<Endpoint, &'static str> {
    let settings = server.masque_client_settings.as_deref();
    if settings
        .and_then(|s| s.path.as_deref())
        .is_some_and(|p| !p.is_empty() && !p.starts_with('/'))
    {
        return Err(INVALID_REASON_MASQUE_PATH);
    }
    let version = settings.and_then(|s| s.version);
    if version.is_some_and(|v| v > 3) {
        return Err(INVALID_REASON_MASQUE_VERSION);
    }
    // 设置结构的 serde 名即内核键名 ⇒ 序列化结果 = 建模键 ∪ 透传袋，顺序固定为
    // 「袋 → 剥禁止键与不兼容键 → 具名字段覆盖」（同 Tor 腿）。
    let mut extra = match serde_json::to_value(settings) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    for k in MASQUE_STRIPPED_KEYS {
        extra.remove(*k);
    }
    let dropped: Vec<&str> = masque_incompatible_keys(version)
        .into_iter()
        .filter(|k| extra.remove(*k).is_some())
        .collect();
    if !dropped.is_empty() {
        log(
            crate::user_config::log_level::LogLevel::Warn,
            &format!(
                "节点「{tag}」：已忽略与 HTTP/{} 不兼容的键 {}（内核在该版本下不认，下发会让整核起不来）",
                version.unwrap_or_default(),
                dropped.join(", ")
            ),
        );
    }

    extra.insert("server".into(), server.address.clone().into());
    extra.insert("server_port".into(), server.port.into());
    for (key, value) in [
        ("username", &server.username),
        ("password", &server.password),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            extra.insert(key.into(), v.into());
        }
    }
    // TLS 恒开：v3 缺 TLS 整核失败；h1/h2 明文虽合法，但 Basic 凭据会明文过线，没有产品场景。
    // 只取 SNI / insecure / 两种 pin；ALPN 由 version 决定，uTLS/ECH/分片本期不下发。
    let tls = server.tls_settings.as_ref();
    let tls = crate::singbox::outbound::OutboundTls {
        enabled: true,
        server_name: Some(
            tls.and_then(|t| t.server_name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| server.address.clone()),
        ),
        insecure: Some(tls.and_then(|t| t.allow_insecure).unwrap_or(false)),
        alpn: None,
        engine: None,
        spoof: None,
        spoof_method: None,
        utls: None,
        reality: None,
        ech: None,
        fragment: None,
        certificate_sha256: crate::user_config::tls_pin::cert_pins_for_kernel(
            tls.and_then(|t| t.certificate_sha256.as_deref()),
        ),
        certificate_public_key_sha256: crate::user_config::tls_pin::cert_pins_for_kernel(
            tls.and_then(|t| t.certificate_public_key_sha256.as_deref()),
        ),
    };
    if let Ok(v) = serde_json::to_value(tls) {
        extra.insert("tls".into(), v);
    }

    Ok(Endpoint {
        type_field: crate::builder::outbound::protocol_str(Protocol::MasqueClient),
        tag: tag.to_owned(),
        domain_resolver: domain_resolver.cloned(),
        detour: detour_tag.map(String::from),
        extra,
        ..Default::default()
    })
}

/// WireGuard endpoint 构造。上游 `buildWireGuardEndpoint`。
/// domain_resolver + platform + tailscale_state_dir（路径）注入。
///
/// `domain_resolver` **纯透传**（#335）：本函数不构造也不给默认值，调用方用
/// [`get_node_dial_domain_resolver`](crate::builder::helpers::get_node_dial_domain_resolver) 备好。
/// 类型是 [`DomainResolver`] 而非 `&str`，新增 call site 塞裸 tag 会编译失败而非静默回落未修形态。
/// `None` 仍表示「不下发」（IP 直拨节点、以及 `endpoint_routes` 的可构造性预检）。
///
/// `detour_tag` = 前置代理的 **outbound tag**（已由调用方经 id→tag 映射解析 + 排除 endpoint 目标，
/// 见 `builder/outbounds.rs#resolve_detour_tag`；本函数不做解析，也不接受 server id）。
/// 这是对 上游的**有意偏离**（上游的 WG 表单与 `SingBoxEndpoint` 都没有 detour），
/// 语义实测与「前置代理必须支持 UDP 转发」这条硬约束见 `singbox/endpoint.rs` 的 `Endpoint::detour`。
pub fn build_wireguard_endpoint(
    server: &ServerConfig,
    tag: &str,
    domain_resolver: Option<&DomainResolver>,
    platform: &str,
    detour_tag: Option<&str>,
) -> Result<Endpoint, String> {
    let s = server
        .wireguard_settings
        .as_ref()
        .ok_or("WireGuard 配置缺失 wireguardSettings")?;
    let private_key = s.private_key.clone().ok_or("WireGuard 缺少 privateKey")?;
    let peer_public_key = s
        .peer_public_key
        .clone()
        .ok_or("WireGuard 缺少 peerPublicKey")?;
    if s.local_address.is_empty() {
        return Err("WireGuard 缺少 localAddress".into());
    }

    let allowed_ips = wireguard_peer_allowed_ips(server).ok_or_else(|| {
        "WireGuard 节点无可路由网段（关外网或 system 内核接口且无具体段）：空 allowed_ips 致 FATAL".to_string()
    })?;

    // 域名 server 才需 domain_resolver（IP 直拨无需）。
    let needs_resolver = domain_resolver.is_some() && !is_ip_literal(&server.address);
    let uses_system = mesh_uses_system_interface(server);

    let mut ep = Endpoint {
        type_field: "wireguard".into(),
        tag: tag.to_string(),
        domain_resolver: None,
        detour: detour_tag.map(String::from),
        // 线格式占位：按需连接的**策略**由 `builder::outbounds::apply_on_demand` 在 push 前统一注入，
        // 不在各 endpoint 构造器里各读一次 `server.on_demand`（四条腿各读一次 = 四个可遗漏点）。
        on_demand: None,
        extra: serde_json::Map::new(),
        system: None,
        mtu: None,
        address: None,
        private_key: None,
        listen_port: None,
        peers: None,
        udp_timeout: None,
        workers: None,
        auth_key: None,
        state_directory: None,
        control_url: None,
        hostname: None,
        exit_node: None,
        exit_node_allow_lan_access: None,
        accept_routes: None,
        ephemeral: None,
        advertise_routes: None,
        system_interface: None,
        system_interface_name: None,
        name: None,
        advertise_tags: None,
        ssh_server: None,
        relay_server_port: None,
        taildrop_directory: None,
    };

    if needs_resolver {
        ep.domain_resolver = domain_resolver.cloned();
    }
    ep.system = Some(uses_system);
    if uses_system && platform != "darwin" {
        ep.name = Some(WG_SYSTEM_INTERFACE_NAME.to_string());
    }
    let default_mtu = if crate::warp::is_warp_server(server) {
        crate::warp::WARP_MTU
    } else {
        1408
    };
    ep.mtu = Some(s.mtu.filter(|mtu| *mtu > 0).unwrap_or(default_mtu));
    ep.workers = s.workers.filter(|workers| *workers > 0);
    ep.address = Some(s.local_address.clone());
    ep.private_key = Some(private_key);
    let mut peer = WireGuardPeer {
        address: server.address.clone(),
        port: server.port,
        public_key: peer_public_key,
        pre_shared_key: s.pre_shared_key.clone(),
        allowed_ips,
        // 缺省按 Polaris 既有策略回落 25 秒；显式 0 遵循 WireGuard 语义关闭保活。
        persistent_keepalive_interval: Some(s.persistent_keepalive.unwrap_or(25)),
        reserved: None,
    };
    if s.reserved.len() == 3 {
        peer.reserved = Some(s.reserved.clone());
    }
    ep.peers = Some(vec![peer]);

    Ok(ep)
}

/// Tailscale endpoint 构造。上游 `buildTailscaleEndpoint`。
/// state_dir 注入（生产 = UserData/tailscale/`<id>`，对拍 = 固定假路径）。
///
/// `detour_tag` 同 [`build_wireguard_endpoint`]：已解析好的 outbound tag，本函数不做解析。
/// 对 上游的有意偏离；TS 侧经前置代理的是**控制面 / DERP 的 TCP 拨号**（异于 WG 的 UDP），
/// 实测见 `singbox/endpoint.rs` 的 `Endpoint::detour`。
pub fn build_tailscale_endpoint(
    server: &ServerConfig,
    tag: &str,
    state_dir: &str,
    platform: &str,
    detour_tag: Option<&str>,
) -> Endpoint {
    // 只读，故借而不拷（与 `builder/outbound.rs` 的 snell 那处同型、同修法）。
    // 此前写的是 `.clone().unwrap_or_default()` —— 装箱后 Some/None **两支各多一次堆分配**
    // （`Box::clone` 先 alloc 再深拷；`Box::<T>::default()` 也 alloc），而 `ts` 在本函数里
    // 全程只读（10 个字段访问点，零写入）。调用点 `builder/outbounds.rs` 在每节点循环里。
    let fallback;
    let ts = match server.tailscale_settings.as_deref() {
        Some(ts) => ts,
        None => {
            fallback = crate::user_config::server_config::TailscaleSettings::default();
            &fallback
        }
    };
    let mut ep = Endpoint {
        type_field: "tailscale".into(),
        tag: tag.to_string(),
        domain_resolver: None,
        detour: detour_tag.map(String::from),
        // 线格式占位：按需连接的**策略**由 `builder::outbounds::apply_on_demand` 在 push 前统一注入，
        // 不在各 endpoint 构造器里各读一次 `server.on_demand`（四条腿各读一次 = 四个可遗漏点）。
        on_demand: None,
        extra: serde_json::Map::new(),
        system: None,
        mtu: None,
        address: None,
        private_key: None,
        listen_port: None,
        peers: None,
        udp_timeout: None,
        workers: None,
        auth_key: ts
            .auth_key
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        state_directory: Some(state_dir.to_string()),
        control_url: ts
            .control_url
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        hostname: ts
            .hostname
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        exit_node: None,
        exit_node_allow_lan_access: None,
        accept_routes: None,
        ephemeral: None,
        advertise_routes: None,
        system_interface: None,
        system_interface_name: None,
        name: None,
        advertise_tags: None,
        ssh_server: None,
        relay_server_port: None,
        // 恒填绝对路径，绝不留给内核默认值 —— 默认是相对的 `Taildrop`，按核进程 CWD 解析后
        // 无条件 mkdir。为什么这是硬约束（含 Windows helper 那条 CWD 腿）见
        // [`crate::singbox::Endpoint::taildrop_directory`]。
        // 落在 state_dir 之下而不是与之并列：state_dir 已按节点 id 分好、随节点删除一起清理，
        // 收件目录跟着走即天然隔离；同时它是 state_dir 的**子目录**，peer 送来的文件名不可能
        // 撞上 `tailscaled.state` 这类密钥文件。
        taildrop_directory: Some(format!("{state_dir}/{TAILDROP_SUBDIR}")),
    };

    // exit_node 仅承载全隧道时下发。
    if mesh_node_carries_full_tunnel(server) {
        if let Some(en) = &ts.exit_node {
            let en = en.trim();
            if !en.is_empty() {
                ep.exit_node = Some(en.to_string());
                ep.exit_node_allow_lan_access = if ts.exit_node_allow_lan_access == Some(true) {
                    Some(true)
                } else {
                    None
                };
            }
        }
    }
    ep.accept_routes = if ts.accept_routes == Some(true) {
        Some(true)
    } else {
        None
    };
    ep.ephemeral = if ts.ephemeral == Some(true) {
        Some(true)
    } else {
        None
    };
    let adv: Vec<String> = ts
        .advertise_routes
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if !adv.is_empty() {
        ep.advertise_routes = Some(adv);
    }
    let adv_tags: Vec<String> = ts
        .advertise_tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if !adv_tags.is_empty() {
        ep.advertise_tags = Some(adv_tags);
    }
    ep.ssh_server = if ts.ssh_server == Some(true) {
        Some(true)
    } else {
        None
    };
    if let Some(p) = ts.relay_server_port {
        if p > 0 {
            ep.relay_server_port = Some(p);
        }
    }
    // `0` = 内核语义里的「自动选端口」（= 不设）。与 relay_server_port 同一口径：不把等价于默认值的
    // 显式 0 写进配置，免得日后上游改默认时磁盘上躺着一份冻结的旧默认。
    if let Some(p) = ts.listen_port {
        if p > 0 {
            ep.listen_port = Some(p);
        }
    }
    // Phase 2 reverseMesh → system_interface。
    if mesh_uses_system_interface(server) {
        ep.system_interface = Some(true);
        if platform != "darwin" {
            ep.system_interface_name = Some(TS_SYSTEM_INTERFACE_NAME.to_string());
        }
    }

    ep
}

#[cfg(test)]
mod tests;

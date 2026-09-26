//! sing-box JSON `outbounds[]` / `endpoints[]` → [`ServerConfig`] 解析
//! （上游 `SubscriptionService.parseSingboxOutbounds` + `makeCustomNode` 移植，纯逻辑）。
//!
//! 与 xray（[`crate::xray_import`]）的差异：sing-box outbound 用**扁平 `type`** + 同级字段
//! （`server`/`server_port`/`tls`/`transport`/...），xray 用 `protocol`+`settings`+`streamSettings`。
//! 二者共用 `outbounds` 键 → 靠 [`crate::xray_import::looks_like_xray`] 区分。
//!
//! 逐条处理，单条失败不影响其它（对齐 xray/clash 分支容错分层）。**不支持传输 / 缺 server·port**
//! 一律**整节点跳过**（不静默降级裸 TCP 产假节点，与分享链 #263 纪律一致）。**未建模的 type**
//! 按 [`ImportOrigin`] 分流：本机文件透传为 custom 逃生舱、远端订阅跳过。
//!
//! **边界归一（R4）协同**：`flow` / `vmessSecurity` / `fingerprint` / `network` 直接构造 `ServerConfig`
//! 时不经 serde `de_opt_token` 钩子 → 显式过 [`normalize_token`]（与 [`crate::xray_import`] 同口径），
//! 否则 `"Chrome"` / `"AES-128-GCM"` 大小写变体让 sing-box FATAL。`security` 走 [`SecurityMode`] 类型化归一。

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde_json::Value;

use polaris_config_engine::builder::endpoint_routes::{has_catch_all, strip_catch_all};
use polaris_config_engine::builder::endpoints::build_masque_endpoint;
use polaris_config_engine::builder::outbound::build_proxy_outbound;
use polaris_config_engine::legacy_keys::migrate_hysteria_v1_legacy_keys;
use polaris_config_engine::singbox::DomainResolver;
use polaris_config_engine::user_config::normalize::normalize_token;
use polaris_config_engine::user_config::protocol_settings::{
    custom_outbound_type, tailcat_emit_check, AnyTlsSettings, CustomSettings, GrpcSettings,
    HttpSettings, Hysteria2ObfsSettings, Hysteria2Settings, HysteriaSettings, MasqueClientSettings,
    MultiplexSettings, NaiveSettings, OpenconnectSettings, OpenvpnClientSettings,
    OpenvpnTlsSettings, RealitySettings, ShadowsocksSettings, SnellSettings, SshSettings,
    TailcatSettings, TlsSettings, TorSettings, TuicSettings, WebSocketSettings,
};
use polaris_config_engine::user_config::server_config::{
    Protocol, SecurityMode, ServerConfig, WireGuardSettings,
};
use polaris_config_engine::user_config::tls_pin::keep_valid_cert_pins;

use crate::clash_parser::ClashParseResult;

/// 导入来源 —— 决定**未建模 type 是否透传为 custom 逃生舱**。
///
/// # 为什么这是个类型而不是 `bool`
///
/// custom 逃生舱把**原始 outbound JSON 逐字下发内核**。这在本机文件上是特性（用户自己的配置，
/// 换 fork 内核就能用未建模协议），在**远端订阅**上是一条任意 JSON 注入通道 —— 实测（随包核
/// 1.14.0-beta.7，`sing-box check` rc=0）：
///
/// ```text
/// {"type":"tor","tag":"t","executable_path":"/bin/false","extra_args":["--x"],"data_directory":"/tmp/nope"}
/// ```
///
/// 即内核**按订阅下发的路径拉起任意本机可执行文件**。故两条腿的信任级不同，误把订阅当本机文件
/// 是提权级缺陷 —— 裸 `bool` 参数在调用点看不出方向（`parse_subscription(t, id, now, g, false)`），
/// 枚举让调用点自陈其信任级。
///
/// 上游 同口径：custom 透传只在 `parseLocalContent`（本机导入）里做，
/// `parseSingboxOutbounds` 走订阅时不做（`SubscriptionService.ts:1258-1265`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOrigin {
    /// 远端订阅正文：未建模 type **跳过**（不开任意 JSON 通道）。
    RemoteSubscription,
    /// 用户本机文件 / 粘贴内容：未建模 type 透传为 custom（上游 `makeCustomNode`）。
    LocalFile,
}

/// 可直接映射的 sing-box outbound `type`（对齐 上游 `SINGBOX_SUPPORTED_TYPES`）。
const SINGBOX_SUPPORTED_TYPES: &[&str] = &[
    // 2026-08-11：hysteria(v1) / tor 进建模协议后一并进本表。
    // tor 在 `map_singbox_outbound` 里有**前置分支**（无 server/port），不走公共守卫。
    "hysteria",
    "tor",
    "shadowsocks",
    "vless",
    "trojan",
    "hysteria2",
    "naive",
    "vmess",
    "tuic",
    "anytls",
    "snell",
    "socks",
    "http",
    "ssh",
];
/// sing-box transport.type → ServerConfig.network 可承载的传输（其余整节点跳过）。上游 `SINGBOX_SUPPORTED_TRANSPORTS`。
const SINGBOX_SUPPORTED_TRANSPORTS: &[&str] = &["ws", "grpc", "http", "httpupgrade"];
/// sing-box 非代理内部 outbound type（忽略，不计丢弃噪声）。上游 `SINGBOX_INTERNAL_TYPES`。
const SINGBOX_INTERNAL_TYPES: &[&str] = &["direct", "block", "dns", "selector", "urltest"];

// ── 标量规整（对齐 xray-import 的 str/num）────────────────────────────────────────

/// 字符串/数字/布尔 → `String`，其余 → `None`。
fn str_val(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 空串归 `None`（上游 `str(v) || 'x'` 语义）。
fn str_ne(v: Option<&Value>) -> Option<String> {
    str_val(v).filter(|s| !s.is_empty())
}

/// 数字（整数值）/ 数字串 → `u32`。
fn num_val(v: Option<&Value>) -> Option<u32> {
    match v? {
        Value::Number(n) => n.as_u64().and_then(|x| u32::try_from(x).ok()).or_else(|| {
            n.as_f64().and_then(|f| {
                (f.fract() == 0.0 && f >= 0.0 && f <= f64::from(u32::MAX)).then_some(f as u32)
            })
        }),
        Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    }
}

/// 端口规整（0/越界拒，防 `as u16` 静默截断）。
fn port_val(p: u32) -> Option<u16> {
    (1..=u32::from(u16::MAX)).contains(&p).then_some(p as u16)
}

fn bool_true(v: Option<&Value>) -> bool {
    v.and_then(Value::as_bool) == Some(true)
}

/// tuic heartbeat 时长规整（上游 `normalizeDuration`）：纯数字(毫秒)补 `ms`，否则透传；空/缺 → `None`。
/// 防 sing-box `ParseDuration "missing unit"`。
fn normalize_duration(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Number(n) => Some(format!("{n}ms")),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else if t.chars().all(|c| c.is_ascii_digit()) {
                Some(format!("{t}ms"))
            } else {
                Some(t.to_string())
            }
        }
        _ => None,
    }
}

/// 有效端口：`server_port` ?? `server_ports[0]` 低位（Hy2 端口跳跃无 server_port 时从范围首个推导）。
fn effective_port(ob: &Value) -> Option<u16> {
    if let Some(p) = num_val(ob.get("server_port")) {
        return port_val(p);
    }
    // server_ports: ["20000:30000", ...] → 首范围低位端口。
    let first = ob.get("server_ports")?.as_array()?.first()?.as_str()?;
    let low = first.split(':').next()?.trim().parse::<u32>().ok()?;
    port_val(low)
}

/// sing-box transport headers（`{k: v | [v..]}`）→ `{k: v}`（取字符串或数组首元素）。
fn transport_headers(v: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let obj = v?.as_object()?;
    let mut m = BTreeMap::new();
    for (k, val) in obj {
        let s = match val {
            Value::Array(a) => a.first().and_then(|x| str_val(Some(x))),
            other => str_val(Some(other)),
        };
        if let Some(s) = s {
            m.insert(k.clone(), s);
        }
    }
    (!m.is_empty()).then_some(m)
}

fn host_header(host: String) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("Host".to_string(), host);
    m
}

// ── TLS / 传输 / 多路复用 ─────────────────────────────────────────────────────────

/// sing-box `tls.ech.config`（ECHConfigList）→ 归一多行字符串。1.14 schema 为字符串数组
/// （每行一段 PEM），亦容忍单个多行字符串；trim + 去空行后 `\n` 拼接，与导出侧
/// `apply_anti_censorship_options` 的 `lines()` split 对称（数组 ←→ 多行字符串 round-trip 闭合）。
fn ech_config_str(ech: Option<&Value>) -> Option<String> {
    let cfg = ech?.get("config")?;
    let raw: Vec<String> = match cfg {
        Value::Array(a) => a.iter().filter_map(|x| str_val(Some(x))).collect(),
        Value::String(s) => s.lines().map(str::to_string).collect(),
        _ => return None,
    };
    let joined = raw
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!joined.is_empty()).then_some(joined)
}

/// sing-box `tls.certificate_sha256` / `certificate_public_key_sha256` → 逗号串。内核类型是
/// `badoption.Listable[[]byte]`（单个 base64 串或其数组，`option/tls.go` v1.15.0-alpha.7 :121-122），
/// 两种形态都收；base64 原文保留，生成侧 `tls_pin::cert_pins_for_kernel` 认得它 ⇒ 往返逐字闭合。
fn cert_pins_str(v: Option<&Value>) -> Option<String> {
    let joined = match v? {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| str_val(Some(x)))
            .collect::<Vec<_>>()
            .join(","),
        other => str_val(Some(other))?,
    };
    keep_valid_cert_pins(&joined)
}

/// TLS/Reality 层（上游：`ob.tls && ob.tls.enabled !== false`）。
fn apply_tls(server: &mut ServerConfig, tls: Option<&Value>) {
    let Some(tls) = tls.filter(|v| v.is_object()) else {
        return;
    };
    // enabled !== false（缺省视作开）。
    if tls.get("enabled").and_then(Value::as_bool) == Some(false) {
        return;
    }
    let reality = tls.get("reality");
    let has_reality = reality
        .map(|r| bool_true(r.get("enabled")))
        .unwrap_or(false)
        && str_ne(reality.and_then(|r| r.get("public_key"))).is_some();

    server.security = Some(if has_reality {
        SecurityMode::Reality
    } else {
        SecurityMode::Tls
    });
    let alpn = tls
        .get("alpn")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| str_val(Some(x))).collect());
    // ECH：enabled 决定开关；开启时一并读 config（ECHConfigList），与导出侧对称（生成什么解析回什么）。
    let ech = tls.get("ech");
    let ech_enabled = ech.map(|e| bool_true(e.get("enabled"))).unwrap_or(false);
    server.tls_settings = Some(TlsSettings {
        server_name: str_val(tls.get("server_name")),
        allow_insecure: Some(bool_true(tls.get("insecure"))),
        alpn,
        fingerprint: str_ne(tls.get("utls").and_then(|u| u.get("fingerprint")))
            .and_then(|f| normalize_token(&f)),
        ech: ech_enabled.then_some(true),
        ech_config: if ech_enabled {
            ech_config_str(ech)
        } else {
            None
        },
        fragment: bool_true(tls.get("fragment")).then_some(true),
        certificate_sha256: cert_pins_str(tls.get("certificate_sha256")),
        certificate_public_key_sha256: cert_pins_str(tls.get("certificate_public_key_sha256")),
        ..Default::default()
    });
    if has_reality {
        if let Some(r) = reality {
            server.reality_settings = Some(RealitySettings {
                public_key: str_val(r.get("public_key")).unwrap_or_default(),
                short_id: str_val(r.get("short_id")),
            });
        }
    }
}

/// 传输层（ws/grpc/http/httpupgrade）。调用前已过 [`SINGBOX_SUPPORTED_TRANSPORTS`] 闸。
fn apply_transport(server: &mut ServerConfig, transport: &Value) {
    let Some(t) = str_ne(transport.get("type")) else {
        return;
    };
    let host = str_ne(transport.get("host"));
    let headers = transport.get("headers");
    match t.as_str() {
        "ws" | "httpupgrade" => {
            server.network = Some(t.clone());
            server.ws_settings = Some(Box::new(WebSocketSettings {
                path: str_val(transport.get("path")),
                headers: host.map(host_header).or_else(|| transport_headers(headers)),
                ..Default::default()
            }));
        }
        "grpc" => {
            server.network = Some("grpc".to_string());
            server.grpc_settings = Some(GrpcSettings {
                service_name: str_val(transport.get("service_name")),
                ..Default::default()
            });
        }
        "http" => {
            server.network = Some("http".to_string());
            server.http_settings = Some(Box::new(HttpSettings {
                path: str_val(transport.get("path")),
                ..Default::default()
            }));
        }
        _ => {}
    }
}

/// Multiplex（vless/trojan/vmess/ss）。
fn apply_multiplex(server: &mut ServerConfig, mux: Option<&Value>) {
    let Some(mux) = mux.filter(|v| bool_true(v.get("enabled"))) else {
        return;
    };
    server.multiplex_settings = Some(MultiplexSettings {
        enabled: Some(true),
        protocol: Some(str_ne(mux.get("protocol")).unwrap_or_else(|| "h2mux".to_string())),
        max_connections: num_val(mux.get("max_connections")),
        min_streams: num_val(mux.get("min_streams")),
        padding: mux.get("padding").and_then(Value::as_bool),
    });
}

// ── 单 outbound → ServerConfig ───────────────────────────────────────────────────

fn new_server(
    id_gen: &mut impl FnMut() -> String,
    ob: &Value,
    protocol: Protocol,
    address: String,
    port: u16,
    sub_id: &str,
    now: &str,
) -> ServerConfig {
    let name = str_ne(ob.get("tag")).unwrap_or_else(|| format!("{address}:{port}"));
    let mut s = ServerConfig {
        id: id_gen(),
        name,
        protocol,
        address,
        port,
        subscription_id: Some(sub_id.to_string()),
        created_at: Some(now.to_string()),
        updated_at: Some(now.to_string()),
        ..Default::default()
    };
    // vless/vmess UDP 封装：显式携带时透传（缺省不写，由生成侧默认 xudp）。
    if ob.get("packet_encoding").is_some() {
        s.packet_encoding = str_val(ob.get("packet_encoding"));
    }
    s
}

/// 未建模 type 能否包成 custom 逃生舱 —— 形状判据**复用生成侧单一真值**
/// [`custom_outbound_type`]，额外只加一条 上游 就有的「type 非空白」。
///
/// # 为什么不在这里另写一份
///
/// 生成侧两条腿（`builder/outbound.rs` / `builder/outbounds.rs`）与「测试内核兼容性」按钮
/// （`commands/proxy.rs::validate_probe_outbound`）已统一到 [`custom_outbound_type`]。导入侧
/// 若自己判「有 type 就行」，`{"type": 42}` 这种就会**导得进、生成时被剔**（数字不是 string）
/// —— 第三份判据 = 第三种分叉。
///
/// # 为什么额外要求 type 非空白（这不是白名单）
///
/// 生成侧刻意放行 `{"type":""}`（“这个 type 内核认不认”是 `sing-box check` 的活，不在生成侧
/// 复刻协议白名单）。导入侧多这一条的理由不是协议判定，而是**落盘门**：
/// `store::validate::protocol_requirement_ok("custom")` 要求 `customSettings.outbound.type`
/// 非空 ⇒ 空 type 的节点造出来也会被 `sanitize_servers` 剔掉，用户只看见「导入了却没有」。
/// 上游 同口径（`if (typeof t !== 'string' || !t.trim()) continue;`）。
/// **仍然没有任何协议名白名单** —— 认不认那个 type 依旧由内核 probe 说了算。
fn custom_wrappable(ob: &Value) -> bool {
    custom_outbound_type(ob).is_some_and(|t| !t.trim().is_empty())
}

/// 未建模 type → custom 透传节点（上游 `makeCustomNode`，`SubscriptionService.ts:1398-1411`）。
/// 调用前须过 [`custom_wrappable`]。
///
/// **原始 JSON 逐字进 `customSettings.outbound`** —— 这是逃生舱的全部价值：换 fork 内核 /
/// 内核后续版本支持该 type 时，节点无需重导即可用；用户也能在节点弹窗里直接编辑那份 JSON。
/// 生成侧已改为 `#[serde(flatten)] extra` 真透传（`098b41e`），故这里保住的字段
/// （hy2-v1 的 `auth_str`、tor 的 `executable_path`…）是真能原样下发内核的，不再被窄 struct 吃掉。
///
/// 字段口径逐条对齐 上游：
/// - `name` = `tag`（trim 后非空）→ 否则 `type` → 否则 `"custom"`；
/// - `address` = `server`（**仅字符串**，非串按缺省空）；`port` = `server_port`（仅数字，否则 0）。
///   空 address / port 0 **不是坏数据**：`crates/store/src/sanitize.rs` 对 `custom` 显式豁免
///   address/port 校验，落点在 `customSettings.outbound` 里那份原文，顶层两字段只作列表展示。
///
/// `is_endpoint` = 该 type 属顶层 `endpoints[]`（openconnect / openvpn-*）而非 `outbounds[]`；
/// 生成侧据此决定把原文塞进哪个数组（`builder/outbounds.rs` 的 custom 两条腿）。
fn make_custom_node(
    ob: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
    is_endpoint: bool,
) -> ServerConfig {
    // 已过 `custom_wrappable` ⇒ type 必是非空白 string。
    let ty = custom_outbound_type(ob)
        .unwrap_or_default()
        .trim()
        .to_string();
    let name = str_ne(ob.get("tag"))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| (!ty.is_empty()).then(|| ty.clone()))
        .unwrap_or_else(|| "custom".to_string());
    ServerConfig {
        id: id_gen(),
        name,
        protocol: Protocol::Custom,
        // 上游 `typeof ob.server === 'string' ? ob.server : ''`：数字 server 不当地址用
        // （与建模腿的 `str_ne` 有意不同 —— 这里只是展示用回显，真值在 outbound 原文里）。
        address: ob
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        port: ob
            .get("server_port")
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(0),
        custom_settings: Some(CustomSettings {
            outbound: ob.clone(),
            is_endpoint: is_endpoint.then_some(true),
            secret_keys: None,
        }),
        subscription_id: Some(sub_id.to_string()),
        created_at: Some(now.to_string()),
        updated_at: Some(now.to_string()),
        ..Default::default()
    }
}

/// 单条 outbound 映射结果。
enum MapOutcome {
    Server(Box<ServerConfig>),
    /// 受支持 type 但字段缺失/配置非法（计 failed）。
    Fail,
    /// 不支持传输（计 skipped，带 transport 名）。
    SkipTransport(String),
}

/// 单条 sing-box outbound → [`ServerConfig`]（已确认 type 受支持）。
fn map_singbox_outbound(
    ob: &Value,
    ty: &str,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
    origin: ImportOrigin,
) -> MapOutcome {
    // ── tor：**无地址协议**，必须排在下面的 server/port 前置守卫之前（2026-08-11）──
    // 实测给内核传 `server` 得 `outbounds[0].server: json: unknown field "server"`，
    // 故它天生没有这两个键，走公共前置只会被判 Fail 而静默丢掉整个节点。
    // address/port 落空由 `store::sanitize` 的 `addressless` 豁免接住（tailscale 同族）。
    if ty == "tor" {
        let mut s = new_server(id_gen, ob, Protocol::Tor, String::new(), 0, sub_id, now);
        const MODELED_TOR: &[&str] = &[
            "type",
            "tag",
            "executable_path",
            "data_directory",
            "extra_args",
            "torrc",
            "domain_resolver",
            "detour",
        ];
        let mut extra = serde_json::Map::new();
        if let Some(obj) = ob.as_object() {
            for (k, v) in obj {
                if !MODELED_TOR.contains(&k.as_str()) {
                    extra.insert(k.clone(), v.clone());
                }
            }
        }
        s.tor_settings = Some(Box::new(TorSettings {
            executable_path: str_ne(ob.get("executable_path")),
            data_directory: str_ne(ob.get("data_directory")),
            extra_args: str_array(ob.get("extra_args")).unwrap_or_default(),
            torrc: ob
                .get("torrc")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| str_val(Some(v)).map(|s| (k.clone(), s)))
                        .collect()
                })
                .unwrap_or_default(),
            extra,
        }));
        return MapOutcome::Server(Box::new(s));
    }

    // 公共前置：server + 有效端口。
    let (Some(server_addr), Some(port)) = (str_ne(ob.get("server")), effective_port(ob)) else {
        return MapOutcome::Fail;
    };
    // 传输闸：不支持的 transport（quic 等）整节点跳过（防裸 TCP 假节点）。
    if let Some(tt) = str_ne(ob.get("transport").and_then(|t| t.get("type"))) {
        if !SINGBOX_SUPPORTED_TRANSPORTS.contains(&tt.as_str()) {
            return MapOutcome::SkipTransport(tt);
        }
    }

    let protocol = match ty {
        "shadowsocks" => Protocol::Shadowsocks,
        "vless" => Protocol::Vless,
        "trojan" => Protocol::Trojan,
        "hysteria2" => Protocol::Hysteria2,
        "hysteria" => Protocol::Hysteria,
        "naive" => Protocol::Naive,
        "vmess" => Protocol::Vmess,
        "tuic" => Protocol::Tuic,
        "anytls" => Protocol::Anytls,
        "snell" => Protocol::Snell,
        "socks" => Protocol::Socks,
        "http" => Protocol::Http,
        "ssh" => Protocol::Ssh,
        _ => return MapOutcome::Fail,
    };
    let mut s = new_server(id_gen, ob, protocol, server_addr, port, sub_id, now);

    // 通用 TLS / 传输 / 多路复用（ssh 显式覆盖 security/network，见下）。
    apply_tls(&mut s, ob.get("tls"));
    if let Some(transport) = ob.get("transport").filter(|v| v.is_object()) {
        apply_transport(&mut s, transport);
    }
    apply_multiplex(&mut s, ob.get("multiplex"));

    // Protocol-specific。
    match ty {
        "shadowsocks" => {
            s.shadowsocks_settings = Some(Box::new(ShadowsocksSettings {
                method: str_ne(ob.get("method")).unwrap_or_else(|| "aes-256-gcm".to_string()),
                password: str_val(ob.get("password")).unwrap_or_default(),
                plugin: str_ne(ob.get("plugin")),
                plugin_opts: str_ne(ob.get("plugin_opts")),
            }));
        }
        "vless" => {
            s.uuid = Some(str_val(ob.get("uuid")).unwrap_or_default());
            s.flow = str_ne(ob.get("flow")).and_then(|f| normalize_token(&f));
        }
        "trojan" => {
            s.password = Some(str_val(ob.get("password")).unwrap_or_default());
        }
        // Hysteria **v1**（2026-08-11）：与 hy2 同名不同义的两处必须各写各的 ——
        // obfs 是裸口令串（不是 {type,password} 对象）、认证走 auth_str（不是 password）。
        //
        // 未建模的键**原样进透传袋**：表单是精选子集，没有袋子时「导入 → 编辑 → 保存」
        // 会把 recv_window 之类静默丢掉，配置从能连变成连不上且无提示。
        "hysteria" => {
            s.security = Some(SecurityMode::Tls); // v1 恒 TLS（后端 TLS_PROTOCOLS 含它）
            let mut hy = HysteriaSettings {
                auth_str: str_ne(ob.get("auth_str")),
                auth: str_ne(ob.get("auth")),
                up_mbps: num_val(ob.get("up_mbps")),
                down_mbps: num_val(ob.get("down_mbps")),
                obfs: str_ne(ob.get("obfs")),
                hop_interval: str_ne(ob.get("hop_interval")),
                ..Default::default()
            };
            hy.server_ports = ob
                .get("server_ports")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| str_ne(Some(v)));
            // 透传袋 = 原文减去「本结构建模过的键」与「已落在 ServerConfig 别处的公共键」。
            //
            // 🔴 **只对本地文件填充**（2026-08-11）：袋子把原文里**任意**键带进下发配置，
            // 而 sing-box 对未知字段是 **decode 阶段拒收** ⇒ 远端订阅塞一个乱键就能让**整个核起不来**，
            // 不止坏掉那个节点。袋子的语义是「用户自己的文件，原样保全」；远端输入只收我们看得懂的字段。
            // 与 tor/openconnect 的「命令执行向量只许本地文件」同一条信任判据，不是两套规则。
            const MODELED: &[&str] = &[
                "type",
                "tag",
                "server",
                "server_port",
                "auth_str",
                "auth",
                "up_mbps",
                "down_mbps",
                "obfs",
                "server_ports",
                "hop_interval",
                "tls",
                "detour",
            ];
            if origin == ImportOrigin::LocalFile {
                if let Some(map) = ob.as_object() {
                    for (k, v) in map {
                        if !MODELED.contains(&k.as_str()) {
                            hy.extra.insert(k.clone(), v.clone());
                        }
                    }
                }
                // Hysteria v1 五旧键**改名替换**：随包 1.14 出站收三键，另两键是
                // misplaced 入站键；上游 docs/changelog 已定 1.16 移除。判据与「为什么不并写」见
                // [`polaris_config_engine::legacy_keys`]。落在入袋之后：袋子收的是用户文件的
                // 原文，此处是它进入本仓数据结构的第一道，早改一步则后续「导入 → 编辑 → 保存」
                // 全程看到的都是新名。生成侧另有同一函数兜住**改动之前就已落盘**的旧配置。
                migrate_hysteria_v1_legacy_keys(&mut hy.extra);
            }
            s.hysteria_settings = Some(Box::new(hy));
            apply_tls(&mut s, ob.get("tls"));
        }
        "hysteria2" => {
            s.password = Some(str_val(ob.get("password")).unwrap_or_default());
            s.security = Some(SecurityMode::Tls);
            let mut hy2 = Hysteria2Settings::default();
            // obfs：salamander（type+password）/ gecko（+min/max_packet_size 随机填充，仅 gecko）。
            // 与导出侧 buildOutbound 对称：需 type+password 双备；未知 type 不设（graceful #263）。
            let obfs = ob.get("obfs");
            if let (Some(ty), Some(pw)) = (
                str_ne(obfs.and_then(|o| o.get("type"))),
                str_ne(obfs.and_then(|o| o.get("password"))),
            ) {
                match ty.as_str() {
                    "salamander" => {
                        hy2.obfs = Some(Hysteria2ObfsSettings {
                            type_field: Some("salamander".to_string()),
                            password: Some(pw),
                            ..Default::default()
                        });
                    }
                    "gecko" => {
                        hy2.obfs = Some(Hysteria2ObfsSettings {
                            type_field: Some("gecko".to_string()),
                            password: Some(pw),
                            min_packet_size: num_val(obfs.and_then(|o| o.get("min_packet_size"))),
                            max_packet_size: num_val(obfs.and_then(|o| o.get("max_packet_size"))),
                        });
                    }
                    _ => {}
                }
            }
            // bbr_profile（1.14）：仅 standard/aggressive/conservative 合法，空/未知不设（对齐导出侧枚举域）。
            if let Some(bp) = str_ne(ob.get("bbr_profile")) {
                if matches!(bp.as_str(), "standard" | "aggressive" | "conservative") {
                    hy2.bbr_profile = Some(bp);
                }
            }
            if let Some(ports) = ob.get("server_ports").and_then(Value::as_array) {
                if !ports.is_empty() {
                    let joined = ports
                        .iter()
                        .filter_map(|x| str_val(Some(x)))
                        .collect::<Vec<_>>()
                        .join(",");
                    hy2.server_ports = Some(joined);
                    hy2.hop_interval = str_ne(ob.get("hop_interval"));
                }
            }
            if hy2 != Hysteria2Settings::default() {
                s.hysteria2_settings = Some(Box::new(hy2));
            }
        }
        "naive" => {
            s.username = Some(str_val(ob.get("username")).unwrap_or_default());
            s.password = Some(str_val(ob.get("password")).unwrap_or_default());
            // quic:true → HTTP/3 传输。
            if bool_true(ob.get("quic")) {
                s.naive_settings = Some(NaiveSettings {
                    use_http3: Some(true),
                });
            }
        }
        "vmess" => {
            s.uuid = Some(str_val(ob.get("uuid")).unwrap_or_default());
            s.alter_id = num_val(ob.get("alter_id"));
            s.vmess_security = str_ne(ob.get("security")).and_then(|v| normalize_token(&v));
        }
        "tuic" => {
            s.uuid = Some(str_val(ob.get("uuid")).unwrap_or_default());
            s.password = Some(str_val(ob.get("password")).unwrap_or_default());
            let ts = TuicSettings {
                congestion_control: str_ne(ob.get("congestion_control")),
                udp_relay_mode: str_ne(ob.get("udp_relay_mode")),
                zero_rtt_handshake: ob.get("zero_rtt_handshake").and_then(Value::as_bool),
                heartbeat: normalize_duration(ob.get("heartbeat")),
            };
            if ts != TuicSettings::default() {
                s.tuic_settings = Some(ts);
            }
        }
        "anytls" => {
            s.password = Some(str_val(ob.get("password")).unwrap_or_default());
            let a = AnyTlsSettings {
                idle_session_check_interval: str_ne(ob.get("idle_session_check_interval")),
                idle_session_timeout: str_ne(ob.get("idle_session_timeout")),
                min_idle_session: num_val(ob.get("min_idle_session")),
            };
            if a != AnyTlsSettings::default() {
                s.any_tls_settings = Some(a);
            }
        }
        "socks" => {
            s.username = str_ne(ob.get("username"));
            s.password = str_ne(ob.get("password"));
        }
        "http" => {
            s.username = str_ne(ob.get("username"));
            s.password = str_ne(ob.get("password"));
        }
        "ssh" => {
            // ssh：固定 tcp/none（private_key_path 是本机路径、跨设备订阅无意义，刻意不映射）。
            s.network = Some("tcp".to_string());
            s.security = Some(SecurityMode::None);
            let ssh = SshSettings {
                user: str_ne(ob.get("user")),
                password: str_ne(ob.get("password")),
                private_key: str_ne(ob.get("private_key")),
                private_key_passphrase: str_ne(ob.get("private_key_passphrase")),
                host_key: str_array(ob.get("host_key")),
                host_key_algorithms: str_array(ob.get("host_key_algorithms")),
                client_version: str_ne(ob.get("client_version")),
                cipher: str_array(ob.get("cipher")),
                mac: str_array(ob.get("mac")),
                kex_algorithm: str_array(ob.get("kex_algorithm")),
                ..Default::default()
            };
            if ssh != SshSettings::default() {
                s.ssh_settings = Some(Box::new(ssh));
            }
        }
        "snell" => {
            // 官方 snell（1.14.0-alpha.38+）：version 仅 4/6；psk 复用 password 落点。
            let version = num_val(ob.get("version"));
            if version != Some(4) && version != Some(6) {
                return MapOutcome::Fail; // 版本不受支持 → 跳过（validateConfig 会拒坏节点连累整份订阅）
            }
            let Some(psk) = str_ne(ob.get("psk")) else {
                return MapOutcome::Fail; // 缺 psk
            };
            let version = version.unwrap();
            let mut snell = SnellSettings {
                version,
                ..Default::default()
            };
            // obfs 仅 v4。
            if version == 4 && str_ne(ob.get("obfs_mode")).as_deref() == Some("http") {
                snell.obfs_mode = Some("http".to_string());
                snell.obfs_host = str_ne(ob.get("obfs_host"));
            }
            if version == 6 {
                if let Some(m) = str_ne(ob.get("mode")) {
                    if m == "unshaped" || m == "unsafe-raw" {
                        snell.mode = Some(m);
                    }
                }
            }
            if bool_true(ob.get("reuse")) {
                snell.reuse = Some(true);
            }
            if let Some(net) = str_ne(ob.get("network")) {
                if net == "tcp" || net == "udp" {
                    snell.network = Some(net);
                }
            }
            snell.userkey = str_ne(ob.get("userkey"));
            s.password = Some(psk);
            s.snell_settings = Some(Box::new(snell));
        }
        _ => return MapOutcome::Fail,
    }

    MapOutcome::Server(Box::new(s))
}

/// 字符串数组 → `Vec<String>`（空数组 → None，对齐 上游 `x.length > 0`）。
fn str_array(v: Option<&Value>) -> Option<Vec<String>> {
    let arr = v?.as_array()?;
    let out: Vec<String> = arr.iter().filter_map(|x| str_val(Some(x))).collect();
    (!out.is_empty()).then_some(out)
}

// ── 入口 ─────────────────────────────────────────────────────────────────────────

/// sing-box `outbounds[]` → 节点集 + 统计（上游 `parseSingboxOutbounds` + `parseLocalContent`
/// 的 custom 透传腿合并）。
///
/// 逐条：`SINGBOX_INTERNAL_TYPES` 忽略；不支持 transport → skipped；缺 server/port / 配置非法 →
/// failed；**未建模 type** 按 `origin` 分流 —— [`ImportOrigin::LocalFile`] 且过 `custom_wrappable`
/// 则透传为 custom（计入 `servers`），[`ImportOrigin::RemoteSubscription`] → skipped。
/// `type` 空 / 缺 / 非字符串 **两条腿都 skipped**（理由见 `custom_wrappable`）。
///
/// `sub_id` 挂到每个节点（本地导入传空串，命令层剥离）。
pub fn parse_singbox_outbounds(
    outbounds: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
    origin: ImportOrigin,
) -> ClashParseResult {
    let mut r = ClashParseResult::default();
    let Some(arr) = outbounds.as_array() else {
        return r;
    };

    let mut skip_by_type: Vec<(String, usize)> = Vec::new();
    let mut unsafe_rejections = Vec::new();
    let mut skip_by_transport: Vec<(String, usize)> = Vec::new();
    let mut tor_skipped = 0usize;
    let mut missing_fields = 0usize;

    for ob in arr {
        if !ob.is_object() {
            r.failed += 1;
            continue;
        }
        let ty = str_val(ob.get("type"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        // ── 信任级分流：命令执行向量类协议**只许本地文件**（2026-08-11）──
        //
        // `tor` 收 `executable_path` / `extra_args` ⇒ 能造出「起任意本机程序」的配置。
        // 该向量此前由「远端订阅绝不产 custom」那条闸挡住（见
        // `remote_subscription_never_wraps_custom`）；把 tor 改成**建模协议**会让向量
        // 从 custom 路径**转移到建模路径**，绕过原闸 —— 这不是新洞，是同一个洞换了条路。
        // 故在此显式复用同一条信任判据，而不是依赖「它现在是建模协议了」。
        if ty == "tor" && origin != ImportOrigin::LocalFile {
            bump(&mut skip_by_type, ty.clone());
            tor_skipped += 1;
            r.skipped += 1;
            continue;
        }
        // Tailcat 没有本地执行向量；远端只接收已知可移植字段，拒绝透传袋。
        // 无 server/port（同 tor），不走 `map_singbox_outbound` 的公共前置守卫。
        if ty == "tailcat" {
            if origin == ImportOrigin::RemoteSubscription {
                if let Some(key) = remote_endpoint_rejection(ob, TAILCAT_REMOTE_KEYS, None) {
                    unsafe_rejections.push(format!("tailcat.{key}"));
                    r.skipped += 1;
                    continue;
                }
            }
            match map_tailcat_outbound(ob, sub_id, now, id_gen) {
                Some((s, ignored)) => {
                    r.warnings.extend(ignored_keys_warning(&s.name, &ignored));
                    r.servers.push(s);
                }
                None => {
                    missing_fields += 1;
                    r.failed += 1;
                }
            }
            continue;
        }
        if !SINGBOX_SUPPORTED_TYPES.contains(&ty.as_str()) {
            // direct/block/selector 等内部 outbound 不计噪声。
            if SINGBOX_INTERNAL_TYPES.contains(&ty.as_str()) {
                continue;
            }
            // 本机文件 + 形状合法 → custom 逃生舱（原文逐字保留、可编辑、换核即用）。
            // 形状判据与生成侧同源，见 [`custom_wrappable`]。
            if origin == ImportOrigin::LocalFile && custom_wrappable(ob) {
                r.servers
                    .push(make_custom_node(ob, sub_id, now, id_gen, false));
                continue;
            }
            let key = if ty.is_empty() {
                "(empty)".to_string()
            } else {
                ty.clone()
            };
            bump(&mut skip_by_type, key);
            r.skipped += 1;
            continue;
        }
        match map_singbox_outbound(ob, &ty, sub_id, now, id_gen, origin) {
            MapOutcome::Server(s) => r.servers.push(*s),
            MapOutcome::Fail => {
                missing_fields += 1;
                r.failed += 1;
            }
            MapOutcome::SkipTransport(tt) => {
                bump(&mut skip_by_transport, tt);
                r.skipped += 1;
            }
        }
    }

    if !unsafe_rejections.is_empty() {
        r.warnings.push(format!(
            "远程订阅节点含本地依赖或不可安全转换的字段，已跳过: {}",
            unsafe_rejections.join(", ")
        ));
    }
    if tor_skipped > 0 {
        r.warnings.push(format!(
            "跳过 {tor_skipped} 个 tor outbound：远端订阅可设置 executable_path/extra_args 拉起本机程序，仅接受本地文件导入"
        ));
    }
    if !skip_by_type.is_empty() {
        r.warnings.push(format!(
            "跳过不支持的 outbound 类型: {}",
            fmt_counts(&skip_by_type)
        ));
    }
    if !skip_by_transport.is_empty() {
        r.warnings.push(format!(
            "跳过不支持的传输层类型: {}",
            fmt_counts(&skip_by_transport)
        ));
    }
    if missing_fields > 0 {
        r.warnings.push(format!(
            "跳过 {missing_fields} 个缺 server/port 或配置非法的 outbound"
        ));
    }
    r.finish()
}

// ── endpoints[] ─────────────────────────────────────────────────────────────────

/// sing-box `endpoints[]` → 节点集 + 统计。
///
/// # 内核 type 域（实测，随包核 `resources/linux/sing-box` = 1.14.0-beta.7）
///
/// 随包核可解码 `wireguard` / `tailscale` / `masque-client` / `masque-server` /
/// `openconnect` / `openvpn-client` / `openvpn-server` 七种 endpoint；
/// 与 `outbounds[]` 的 type 域**不相交**（`wireguard` 作 outbound 已于 1.13 移除、`tailscale`
/// 作 outbound 即 unknown）。故本函数与 [`parse_singbox_outbounds`] 各管一个数组，无重复计数。
/// Polaris 建模其中五种（`wireguard` / `tailscale` / `masque-client` / `openconnect` /
/// `openvpn-client`）；剩余服务端类型走 custom 逃生舱。
///
/// # 分流
///
/// - `wireguard` → [`Protocol::Wireguard`] 建模映射（见 `map_wireguard_endpoint`）。
/// - `tailscale` → **恒 skipped**（不建模、也不透传 custom），原因见函数体内该 match 臂的注释。
/// - `masque-client` / `openconnect` / `openvpn-client` → 建模映射；远端逐字段筛选本地依赖。
/// - 其余类型 → 按 `origin` 走 custom 逃生舱（`isEndpoint = true`）/ skipped，
///   与 [`parse_singbox_outbounds`] 同一条信任级判据。
pub fn parse_singbox_endpoints(
    endpoints: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
    origin: ImportOrigin,
) -> ClashParseResult {
    let mut r = ClashParseResult::default();
    let Some(arr) = endpoints.as_array() else {
        return r;
    };

    let mut skip_by_type: Vec<(String, usize)> = Vec::new();
    let mut unsafe_rejections = Vec::new();
    let mut tailscale_skipped = 0usize;
    let mut missing_fields = 0usize;
    let mut multi_peer_skipped = 0usize;
    let mut multi_remote_skipped = 0usize;
    let mut masque_failed = 0usize;

    for ep in arr {
        if !ep.is_object() {
            r.failed += 1;
            continue;
        }
        let ty = str_val(ep.get("type"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ty == "openvpn-client" && ep.get("servers").is_some() {
            // 核心要求 server 与 servers 互斥；当前节点地址/端口编辑只表达一个远端。
            // 不能取首项，也不能将两种字段同时下发制造核拒绝的配置。
            multi_remote_skipped += 1;
            r.skipped += 1;
            continue;
        }
        if origin == ImportOrigin::RemoteSubscription {
            let allowed = match ty.as_str() {
                "masque-client" => Some((MASQUE_REMOTE_KEYS, Some(MASQUE_TLS_KEYS))),
                "openconnect" => Some((OPENCONNECT_REMOTE_KEYS, Some(OPENCONNECT_TLS_KEYS))),
                "openvpn-client" => Some((OPENVPN_REMOTE_KEYS, Some(OPENVPN_TLS_KEYS))),
                _ => None,
            };
            if let Some((keys, tls_keys)) = allowed {
                if let Some(key) = remote_endpoint_rejection(ep, keys, tls_keys) {
                    unsafe_rejections.push(format!("{ty}.{key}"));
                    r.skipped += 1;
                    continue;
                }
            }
        }
        match ty.as_str() {
            "wireguard"
                if ep
                    .get("peers")
                    .and_then(Value::as_array)
                    .is_some_and(|peers| peers.len() > 1) =>
            {
                multi_peer_skipped += 1;
                r.skipped += 1;
            }
            "wireguard" => match map_wireguard_endpoint(ep, sub_id, now, id_gen) {
                Some(s) => {
                    let ignored: Vec<String> = [
                        "system",
                        "listen_port",
                        "udp_timeout",
                        "name",
                        "detour",
                        "domain_resolver",
                    ]
                    .iter()
                    .filter(|key| ep.get(**key).is_some())
                    .map(|key| (*key).to_string())
                    .collect();
                    r.warnings.extend(ignored_keys_warning(&s.name, &ignored));
                    r.servers.push(s);
                }
                None => {
                    missing_fields += 1;
                    r.failed += 1;
                }
            },
            // ── tailscale endpoint 恒不导入（账号授权须由本机发起）─────────────────────────
            // 1. **账号加入须明确授权**：`auth_key` 会让本机加入对应 tailnet；目前导入流程没有
            //    提供账号加入确认或本机状态初始化。即使文件来自用户本人，也不能仅凭节点列表代做此事。
            // 2. **状态目录不可移植**：`state_directory` 由 Polaris 生成时注入本机路径
            //    （`builder/endpoints.rs` 的 `build_tailscale_endpoint`），文件里那份对本机无意义。
            // 3. **实测无内容可导**：`{"type":"tailscale","tag":"x"}`（零字段）`sing-box check`
            //    rc=0 —— 没有任何必填的、可移植的、非凭据字段。
            // **也不走 custom 逃生舱**：这会绕过本机账号与状态目录的授权流程。
            "tailscale" => {
                tailscale_skipped += 1;
                r.skipped += 1;
            }
            // ── 端点族 VPN 客户端（2026-08-11）──
            // 它们的凭据用于本机主动连远端服务器，故可按远端字段安全策略导入。
            // 未建模的键原样进透传袋 —— 表单是精选子集（openconnect 61 键 / openvpn 78 键，
            // 多数是调优旋钮），没有袋子时「导入 → 编辑 → 保存」会静默丢掉它们。
            // openconnect 的 `csd` / `tncc` 等外部脚本键在远端入口被拒。
            "masque-client" => match map_endpoint_masque(ep, sub_id, now, id_gen) {
                Some((s, ignored)) => {
                    r.warnings.extend(ignored_keys_warning(&s.name, &ignored));
                    r.servers.push(s);
                }
                None => {
                    masque_failed += 1;
                    r.failed += 1;
                }
            },
            "openconnect" | "openvpn-client" => {
                match map_endpoint_vpn_client(&ty, ep, sub_id, now, id_gen) {
                    Some(s) => {
                        let ignored: Vec<String> = ["detour", "domain_resolver"]
                            .iter()
                            .filter(|key| ep.get(**key).is_some())
                            .map(|key| (*key).to_string())
                            .collect();
                        r.warnings.extend(ignored_keys_warning(&s.name, &ignored));
                        r.servers.push(s);
                    }
                    None => {
                        missing_fields += 1;
                        r.failed += 1;
                    }
                }
            }
            _ => {
                if origin == ImportOrigin::LocalFile && custom_wrappable(ep) {
                    r.servers
                        .push(make_custom_node(ep, sub_id, now, id_gen, true));
                    continue;
                }
                let key = if ty.is_empty() {
                    "(empty)".to_string()
                } else {
                    ty.clone()
                };
                bump(&mut skip_by_type, key);
                r.skipped += 1;
            }
        }
    }

    if !unsafe_rejections.is_empty() {
        r.warnings.push(format!(
            "远程订阅节点含本地依赖或不可安全转换的字段，已跳过: {}",
            unsafe_rejections.join(", ")
        ));
    }
    if tailscale_skipped > 0 {
        r.warnings.push(format!(
            "跳过 {tailscale_skipped} 个 tailscale endpoint：账号加入与本机状态目录目前须经本机配置流程，\
             导入流程尚未实现该授权和状态初始化"
        ));
    }
    if multi_peer_skipped > 0 {
        r.warnings.push(format!(
            "跳过 {multi_peer_skipped} 个 wireguard endpoint：Polaris 仅支持单 peer，不能无损导入多 peer 配置"
        ));
    }
    if multi_remote_skipped > 0 {
        r.warnings.push(format!(
            "跳过 {multi_remote_skipped} 个 openvpn-client endpoint：servers 多远端轮换尚未映射到节点编辑模型，不能取首项或同时下发 server 与 servers"
        ));
    }
    if !skip_by_type.is_empty() {
        r.warnings.push(format!(
            "跳过不支持的 endpoint 类型: {}",
            fmt_counts(&skip_by_type)
        ));
    }
    if missing_fields > 0 {
        r.warnings.push(format!(
            "跳过 {missing_fields} 个缺 private_key / peers 必填字段的 wireguard endpoint"
        ));
    }
    if masque_failed > 0 {
        r.warnings.push(format!(
            "跳过 {masque_failed} 个缺 server / server_port，或 path / version 非法（会让内核整核起不来）\
             的 masque-client endpoint"
        ));
    }
    r.finish()
}

/// `endpoints[].{type:"wireguard"}` → [`Protocol::Wireguard`] 节点。必填缺失 → `None`（计 failed）。
///
/// # 字段对位（逆向 [`build_wireguard_endpoint`]，单 peer 模型）
///
/// | sing-box endpoint | ServerConfig |
/// |---|---|
/// | `tag` | `name`（缺省 `addr:port`，与 [`new_server`] 同口径） |
/// | `peers[0].address` / `.port` | `address` / `port` |
/// | `private_key` | `wireguardSettings.privateKey` |
/// | `address[]` | `wireguardSettings.localAddress` |
/// | `peers[0].public_key` | `wireguardSettings.peerPublicKey` |
/// | `peers[0].pre_shared_key` | `wireguardSettings.preSharedKey` |
/// | `peers[0].allowed_ips` | catch-all → `allowInternet`；具体段 → `allowedIPs` |
/// | `peers[0].persistent_keepalive_interval` | `persistentKeepalive`（>0 才写） |
/// | `peers[0].reserved` | `reserved`（**恰 3 项**才写，与生成侧 `s.reserved.len() == 3` 对称） |
/// | `mtu` | `mtu`（>0 才写） |
/// | `on_demand` / `bind_interface` | 顶层 `onDemand` / `bindInterface`（[`ENDPOINT_TOP_LEVEL_KEYS`]，同 MASQUE） |
///
/// `allowed_ips` 的拆分口径与粘贴 wg-quick `.conf` 那条腿逐字同源
/// （`ui/src/components/dialogs/wg-logic.ts#draftFromParsed`）：全网段是「全隧道意图」、由
/// `allowInternet` 承载，`allowedIPs` 只留具体段 —— 生成侧 `wireguard_peer_allowed_ips` 会按
/// `allowInternet` 把 0/0 加回去，不这样拆就会双份。
///
/// # 刻意不映射（非遗漏）
///
/// - **`system` → `reverseMesh`**：`system:true` 要抢内核 utun（需提权、与主 TUN 冲突，
///   `builder/endpoint_routes.rs:112-128` 记有 WARP 恒否决与 `resource busy` FATAL 实证）。
///   外部文件不该能翻这个开关；且 wg-quick 导入腿同样恒 false（`wg-logic.ts:170`）。
///   导入时保持用户态默认值；用户态并不禁止入站，是否可反向接入由路由与对端配置决定。
/// - **`listen_port` / `udp_timeout` / `name`**：`WireGuardSettings` 无落点；
///   导入汇合处应向用户报告这些字段未保留。
/// - **`detour`**：`ServerConfig.detour` 存的是**本地节点 id**，外部 tag 无从解析。
///
/// [`build_wireguard_endpoint`]: polaris_config_engine::builder::endpoints::build_wireguard_endpoint
/// `openconnect` / `openvpn-client` endpoint → [`ServerConfig`]。
///
/// 载荷形态与生成侧**对称**：生成时把设置结构整体序列化 flatten 进 endpoint，
/// 故导入时反过来 —— 建模键各归各位，其余原样进透传袋。
/// 两侧共用同一份「哪些键建模了」的清单（下面的 `MODELED_*`），复制第二份必然漂移。
/// `on_demand` / `bind_interface` 例外：生成侧读顶层，故提到顶层（[`ENDPOINT_TOP_LEVEL_KEYS`]，同 MASQUE）。
///
/// 地址/端口：openconnect 的 `server` 是 `host:port` **单串**，openvpn 才有独立的 `server_port`。
/// `ServerConfig` 的 address/port 是落盘门 `sanitize_servers` 的必填项，故从各自形态里拆出来。
fn map_endpoint_vpn_client(
    ty: &str,
    ep: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
) -> Option<ServerConfig> {
    const MODELED_OC: &[&str] = &[
        "type",
        "tag",
        "server",
        "username",
        "password",
        "flavor",
        "auth_group",
        "token",
        "mtu",
        "no_udp",
        "pfs",
        "allow_insecure_crypto",
        "user_agent",
        "reported_os",
        "system",
        "domain_resolver",
        "detour",
    ];
    const MODELED_OV: &[&str] = &[
        "type",
        "tag",
        "server",
        "server_port",
        "username",
        "password",
        "network",
        "cipher",
        "auth",
        "mtu",
        "redirect_gateway",
        "system",
        "tls",
        "domain_resolver",
        "detour",
    ];

    let raw_server = str_ne(ep.get("server"))?;
    let (addr, port) = if ty == "openconnect" {
        // OpenConnect 的 server 是 host:port；IPv6 必须按括号形式解析，裸 IPv6 默认 443。
        if raw_server.contains("://") {
            let uri = url::Url::parse(&raw_server).ok()?;
            if !matches!(uri.scheme(), "http" | "https") {
                return None;
            }
            let host = match uri.host()? {
                url::Host::Ipv6(ip) => ip.to_string(),
                host => host.to_string(),
            };
            // url::Url 会规范化掉 http 的显式 :80；原生 Go URL.Port() 会保留它。
            // 因此端口从原始 authority 读取，未写时恒回落 443。
            let authority = raw_server
                .split_once("://")?
                .1
                .split(['/', '?', '#'])
                .next()?;
            let authority = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let explicit_port = if authority.starts_with('[') {
                authority.split_once(']')?.1.strip_prefix(':')
            } else {
                authority.rsplit_once(':').map(|(_, port)| port)
            };
            let port = match explicit_port {
                Some(port) => port_val(port.parse::<u32>().ok()?)?,
                None => 443,
            };
            (host, port)
        } else if let Some(rest) = raw_server.strip_prefix('[') {
            let (host, suffix) = rest.split_once(']')?;
            let port = if let Some(port) = suffix.strip_prefix(':') {
                port_val(port.parse::<u32>().ok()?)?
            } else if suffix.is_empty() {
                443
            } else {
                return None;
            };
            (host.to_string(), port)
        } else if raw_server.matches(':').count() > 1 {
            (raw_server.clone(), 443)
        } else if let Some((host, port)) = raw_server.rsplit_once(':') {
            (host.to_string(), port_val(port.parse::<u32>().ok()?)?)
        } else {
            (raw_server.clone(), 443)
        }
    } else {
        (
            raw_server.clone(),
            port_val(num_val(ep.get("server_port"))?)?,
        )
    };

    let protocol = if ty == "openconnect" {
        Protocol::Openconnect
    } else {
        Protocol::OpenvpnClient
    };
    let mut s = new_server(id_gen, ep, protocol, addr, port, sub_id, now);
    lift_endpoint_top_level_keys(&mut s, ep);

    if ty == "openconnect" {
        s.openconnect_settings = Some(Box::new(OpenconnectSettings {
            server: Some(raw_server),
            username: str_ne(ep.get("username")),
            password: str_ne(ep.get("password")),
            flavor: str_ne(ep.get("flavor")),
            auth_group: str_ne(ep.get("auth_group")),
            token: ep.get("token").cloned(),
            mtu: num_val(ep.get("mtu")),
            no_udp: ep.get("no_udp").map(|v| bool_true(Some(v))),
            pfs: ep.get("pfs").map(|v| bool_true(Some(v))),
            allow_insecure_crypto: ep.get("allow_insecure_crypto").map(|v| bool_true(Some(v))),
            user_agent: str_ne(ep.get("user_agent")),
            reported_os: str_ne(ep.get("reported_os")),
            system: ep.get("system").map(|v| bool_true(Some(v))),
            extra: endpoint_bag(ep, MODELED_OC),
        }));
    } else {
        let pem = |k: &str| -> Vec<String> {
            ep.get("tls")
                .and_then(|t| t.get(k))
                .map(|value| match value {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                })
                .unwrap_or_default()
        };
        s.openvpn_client_settings = Some(Box::new(OpenvpnClientSettings {
            server: Some(raw_server),
            server_port: Some(port),
            username: str_ne(ep.get("username")),
            password: str_ne(ep.get("password")),
            network: str_ne(ep.get("network")),
            cipher: str_ne(ep.get("cipher")),
            auth: str_ne(ep.get("auth")),
            mtu: num_val(ep.get("mtu")),
            redirect_gateway: ep.get("redirect_gateway").map(|v| bool_true(Some(v))),
            system: ep.get("system").map(|v| bool_true(Some(v))),
            tls: (ep.get("mode").and_then(Value::as_str) != Some("static_key")).then(|| {
                OpenvpnTlsSettings {
                    certificate: pem("certificate"),
                    client_certificate: pem("client_certificate"),
                    client_key: pem("client_key"),
                    // 嵌套袋：tls 下未建模的子键（peer_fingerprint / server_name / version_* …）
                    extra: ep
                        .get("tls")
                        .and_then(Value::as_object)
                        .map(|m| {
                            m.iter()
                                .filter(|(k, _)| {
                                    !matches!(
                                        k.as_str(),
                                        "certificate" | "client_certificate" | "client_key"
                                    )
                                })
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }),
            extra: endpoint_bag(ep, MODELED_OV),
        }));
    }
    Some(s)
}

/// 端点族（WireGuard / MASQUE / OpenConnect / OpenVPN Client）里生成侧按 `ServerConfig` **顶层**写的键。
///
/// 生成侧装配层对每条 endpoint 腿统一调 `apply_on_demand` / `apply_bind_interface`，读的是顶层
/// `onDemand` / `bindInterface`：袋里的 `on_demand` 会原样下发而 UI 读顶层（界面显示关、内核实开），
/// 袋里的 `bind_interface` 会被 `apply_bind_interface` 静默删掉。故导入时一律提到顶层、不进袋。
const ENDPOINT_TOP_LEVEL_KEYS: &[&str] = &["on_demand", "bind_interface"];

/// 把 [`ENDPOINT_TOP_LEVEL_KEYS`] 提到节点顶层。
fn lift_endpoint_top_level_keys(s: &mut ServerConfig, ep: &Value) {
    s.on_demand = ep.get("on_demand").and_then(Value::as_bool);
    s.bind_interface = str_ne(ep.get("bind_interface"));
}

/// endpoint 的透传袋：`modeled` 与 [`ENDPOINT_TOP_LEVEL_KEYS`] 之外的键原样保留。
fn endpoint_bag(ep: &Value, modeled: &[&str]) -> serde_json::Map<String, Value> {
    ep.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| {
            !modeled.contains(&k.as_str()) && !ENDPOINT_TOP_LEVEL_KEYS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// 远端 endpoint 只接收明确映射或经随包核 schema 确认的可移植字段。不能把本地文件的
// `extra` 逃生舱直接放给订阅：其中的 *_path、脚本认证、系统接口等会读写本机资源。
const MASQUE_REMOTE_KEYS: &[&str] = &[
    "type",
    "tag",
    "server",
    "server_port",
    "username",
    "password",
    "tls",
    "path",
    "headers",
    "version",
    "mtu",
    "on_demand",
    "connect_timeout",
    "idle_timeout",
    "keep_alive_period",
    "stream_receive_window",
    "connection_receive_window",
    "max_concurrent_streams",
    "initial_packet_size",
    "disable_path_mtu_discovery",
    "disable_version_fallback",
    "fallback_delay",
    "fallback_network_type",
    "network_strategy",
    "network_type",
    "tcp_fast_open",
    "tcp_multi_path",
    "tcp_keep_alive",
    "tcp_keep_alive_interval",
    "disable_tcp_keep_alive",
    "udp_fragment",
    "udp_timeout",
    "udp_mapping",
    "udp_filtering",
    "udp_nat_max",
    "reuse_addr",
    "bind_address_no_port",
    // 这些键被 mapper/生成器剥离并报告，保留现有本地导入的诊断语义。
    "system",
    "name",
    "advertise_routes",
    "detour",
    "domain_resolver",
];
const MASQUE_TLS_KEYS: &[&str] = &[
    "enabled",
    "server_name",
    "insecure",
    "certificate_sha256",
    "certificate_public_key_sha256",
];
const TAILCAT_REMOTE_KEYS: &[&str] = &[
    "type",
    "tag",
    "server_public_key",
    "server_disco_key",
    "pre_shared_key",
    "private_key",
    "derp_region",
    "derp_map_url",
    "derp_servers",
    "connect_timeout",
    "fallback_delay",
    "fallback_network_type",
    "network_strategy",
    "network_type",
    "tcp_fast_open",
    "tcp_multi_path",
    "tcp_keep_alive",
    "tcp_keep_alive_interval",
    "disable_tcp_keep_alive",
    "udp_fragment",
    "reuse_addr",
    "bind_address_no_port",
    "http_client",
    "detour",
    "domain_resolver",
];
const OPENCONNECT_REMOTE_KEYS: &[&str] = &[
    "type",
    "tag",
    "server",
    "username",
    "password",
    "flavor",
    "auth_group",
    "token",
    "mtu",
    "no_udp",
    "pfs",
    "allow_insecure_crypto",
    "user_agent",
    "reported_os",
    "on_demand",
    "tls",
    "cookie",
    "dtls_local_port",
    "form_entries",
    "fortinet_host_check",
    "base_mtu",
    "compression_disabled",
    "compression_mode",
    "connect_timeout",
    "dpd_interval",
    "external_auth_disabled",
    "http_keepalive_disabled",
    "ipv6_disabled",
    "local_hostname",
    "mobile",
    "password_authentication_disabled",
    "queue_length",
    "reconnect_timeout",
    "tcp_keep_alive_enabled",
    "trojan_interval",
    "version",
    "xml_post_disabled",
    "fallback_delay",
    "fallback_network_type",
    "network_strategy",
    "network_type",
    "tcp_fast_open",
    "tcp_multi_path",
    "tcp_keep_alive",
    "tcp_keep_alive_interval",
    "disable_tcp_keep_alive",
    "udp_fragment",
    "udp_timeout",
    "udp_mapping",
    "udp_filtering",
    "udp_nat_max",
    "reuse_addr",
    "bind_address_no_port",
    "detour",
    "domain_resolver",
];
const OPENVPN_REMOTE_KEYS: &[&str] = &[
    "type",
    "tag",
    "address",
    "server",
    "server_port",
    "username",
    "password",
    "network",
    "cipher",
    "auth",
    "mtu",
    "redirect_gateway",
    "tls",
    "on_demand",
    "data_ciphers",
    "data_ciphers_fallback",
    "ping_interval",
    "ping_restart",
    "handshake_window",
    "key_direction",
    "compression_lzo",
    "allow_compression",
    "compression",
    "auth_retry",
    "static_challenge",
    "static_challenge_echo",
    "block_ipv6",
    "connect_timeout",
    "explicit_exit_notify",
    "fragment",
    "mode",
    "mss_fix",
    "mss_fix_disabled",
    "mss_fix_mode",
    "peer_address",
    "peer_address_ipv6",
    "ping_restart_disabled",
    "redirect_gateway_flags",
    "redirect_private",
    "remote_random",
    "renegotiate_bytes",
    "renegotiate_disabled",
    "renegotiate_interval",
    "renegotiate_packets",
    "replay_window",
    "replay_window_time",
    "route_gateway",
    "route_metric",
    "route_no_pull",
    "routes",
    "pull_filters",
    "static_key",
    "tls_timeout",
    "topology",
    "fallback_delay",
    "fallback_network_type",
    "network_strategy",
    "network_type",
    "tcp_fast_open",
    "tcp_multi_path",
    "tcp_keep_alive",
    "tcp_keep_alive_interval",
    "disable_tcp_keep_alive",
    "udp_fragment",
    "udp_timeout",
    "udp_mapping",
    "udp_filtering",
    "udp_nat_max",
    "reuse_addr",
    "bind_address_no_port",
    "detour",
    "domain_resolver",
];
const OPENCONNECT_TLS_KEYS: &[&str] = &[
    "certificate_authority",
    "client_certificate",
    "client_key",
    "client_key_password",
    "insecure",
    "mca_certificate",
    "mca_key",
    "mca_key_password",
    "peer_fingerprint",
    "server_name",
    "system_trust_disabled",
];
const OPENVPN_TLS_KEYS: &[&str] = &[
    "certificate",
    "client_certificate",
    "client_key",
    "peer_fingerprint",
    "server_name",
    "control_wrap",
    "certificate_profile",
    "cipher",
    "groups",
    "ns_certificate_type",
    "remote_certificate_eku",
    "remote_certificate_ku",
    "remote_certificate_tls",
    "server_name_type",
    "version_max",
    "version_min",
];

/// 返回拒绝的键名供聚合告警使用；绝不把订阅字段值（可能是凭据）写进日志。
fn remote_endpoint_rejection(
    node: &Value,
    allowed: &[&str],
    tls_allowed: Option<&[&str]>,
) -> Option<String> {
    let obj = node.as_object()?;
    if obj.get("type").and_then(Value::as_str) == Some("openvpn-client") {
        if obj.contains_key("mode") && !obj.get("mode").is_some_and(Value::is_string) {
            return Some("mode".into());
        }
        let mode = obj.get("mode").and_then(Value::as_str).unwrap_or("tls");
        if !matches!(mode, "tls" | "static_key") {
            return Some("mode".into());
        }
        if mode == "static_key" {
            for (key, valid) in [
                (
                    "address",
                    obj.get("address").is_some_and(nonempty_listable_strings),
                ),
                (
                    "cipher",
                    obj.get("cipher")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty()),
                ),
                (
                    "auth",
                    obj.get("auth")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty()),
                ),
                (
                    "static_key",
                    obj.get("static_key").is_some_and(nonempty_listable_strings),
                ),
            ] {
                if !valid {
                    return Some(format!("static_key.{key}"));
                }
            }
            let addresses: Vec<&str> = match obj.get("address")? {
                Value::String(address) => vec![address],
                Value::Array(addresses) => addresses.iter().filter_map(Value::as_str).collect(),
                _ => return Some("static_key.address".into()),
            };
            let mut has_ipv4 = false;
            let mut has_ipv6 = false;
            for address in addresses {
                let Some((ip, prefix)) = address.split_once('/') else {
                    return Some("static_key.address".into());
                };
                let (Ok(ip), Ok(prefix)) = (ip.parse::<std::net::IpAddr>(), prefix.parse::<u8>())
                else {
                    return Some("static_key.address".into());
                };
                if prefix > if ip.is_ipv4() { 32 } else { 128 } {
                    return Some("static_key.address".into());
                }
                has_ipv4 |= ip.is_ipv4();
                has_ipv6 |= ip.is_ipv6();
            }
            for (key, ipv6, has_family) in [
                ("peer_address", false, has_ipv4),
                ("peer_address_ipv6", true, has_ipv6),
            ] {
                match obj.get(key) {
                    None if !has_family => {}
                    Some(value)
                        if has_family
                            && value
                                .as_str()
                                .and_then(|ip| ip.parse::<std::net::IpAddr>().ok())
                                .is_some_and(|ip| ip.is_ipv6() == ipv6) => {}
                    _ => return Some(format!("static_key.{key}")),
                }
            }
            if obj.contains_key("tls") {
                return Some("static_key.tls".into());
            }
        } else if obj.contains_key("static_key") {
            return Some("mode".into());
        }
    }
    for (key, value) in obj {
        if !allowed.contains(&key.as_str()) {
            return Some(key.clone());
        }
        if !matches!(
            key.as_str(),
            "tls"
                | "headers"
                | "derp_servers"
                | "derp_map_url"
                | "token"
                | "mobile"
                | "form_entries"
                | "fortinet_host_check"
                | "pull_filters"
        ) && !remote_scalar_shape(
            key,
            value,
            obj.get("type").and_then(Value::as_str).unwrap_or_default(),
        ) {
            return Some(key.clone());
        }
        if key == "tls" {
            let Some(tls) = value.as_object() else {
                return Some("tls".into());
            };
            for (nested, val) in tls {
                if !tls_allowed.is_some_and(|keys| keys.contains(&nested.as_str())) {
                    return Some(format!("tls.{nested}"));
                }
                if matches!(
                    nested.as_str(),
                    "certificate"
                        | "certificate_authority"
                        | "client_certificate"
                        | "client_key"
                        | "mca_certificate"
                        | "mca_key"
                ) && !listable_strings(val)
                {
                    return Some(format!("tls.{nested}"));
                }
                if nested == "control_wrap" {
                    let Some(wrap) = val.as_object() else {
                        return Some("tls.control_wrap".into());
                    };
                    if wrap
                        .keys()
                        .any(|k| !["type", "key", "direction"].contains(&k.as_str()))
                        || !wrap.get("key").is_some_and(nonempty_listable_strings)
                        || !wrap.get("type").and_then(Value::as_str).is_some_and(|ty| {
                            matches!(ty, "tls_auth" | "tls_crypt" | "tls_crypt_v2")
                        })
                        || wrap.get("direction").is_some_and(|v| {
                            !v.as_str().is_some_and(|d| matches!(d, "client" | "server"))
                        })
                    {
                        return Some("tls.control_wrap".into());
                    }
                } else if matches!(
                    nested.as_str(),
                    "enabled" | "insecure" | "system_trust_disabled"
                ) && !val.is_boolean()
                    || matches!(
                        nested.as_str(),
                        "groups"
                            | "remote_certificate_ku"
                            | "peer_fingerprint"
                            | "certificate_sha256"
                            | "certificate_public_key_sha256"
                    ) && !listable_strings(val)
                    || !matches!(
                        nested.as_str(),
                        "control_wrap"
                            | "enabled"
                            | "insecure"
                            | "system_trust_disabled"
                            | "certificate"
                            | "certificate_authority"
                            | "client_certificate"
                            | "client_key"
                            | "mca_certificate"
                            | "mca_key"
                            | "groups"
                            | "remote_certificate_ku"
                            | "peer_fingerprint"
                            | "certificate_sha256"
                            | "certificate_public_key_sha256"
                    ) && !val.is_string()
                {
                    return Some(format!("tls.{nested}"));
                }
            }
        }
    }
    if obj.get("derp_map_url").is_some_and(|value| {
        let Some(raw) = value.as_str() else {
            return true;
        };
        url::Url::parse(raw).map_or(true, |u| {
            !matches!(u.scheme(), "http" | "https")
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
        })
    }) {
        return Some("derp_map_url".into());
    }
    if obj.get("derp_servers").is_some_and(|value| {
        let servers: Vec<&Value> = match value {
            Value::Array(items) => items.iter().collect(),
            Value::String(_) | Value::Object(_) => vec![value],
            _ => return true,
        };
        !servers.iter().all(|server| match server {
            Value::String(host) => !host.is_empty(),
            Value::Object(fields) => {
                fields
                    .get("host")
                    .and_then(Value::as_str)
                    .is_some_and(|h| !h.is_empty())
                    && fields.keys().all(|key| {
                        [
                            "host",
                            "ipv4",
                            "ipv6",
                            "derp_port",
                            "stun_port",
                            "cert_name",
                        ]
                        .contains(&key.as_str())
                    })
                    && ["ipv4", "ipv6", "cert_name"]
                        .iter()
                        .all(|key| fields.get(*key).is_none_or(Value::is_string))
                    && ["derp_port", "stun_port"].iter().all(|key| {
                        fields
                            .get(*key)
                            .is_none_or(|port| num_val(Some(port)).and_then(port_val).is_some())
                    })
            }
            _ => false,
        })
    }) {
        return Some("derp_servers".into());
    }
    if obj.get("headers").is_some_and(|value| {
        !value
            .as_object()
            .is_some_and(|headers| headers.values().all(listable_strings))
    }) {
        return Some("headers".into());
    }
    if obj.get("token").is_some_and(|value| {
        !value.as_object().is_some_and(|token| {
            token.keys().all(|key| {
                ["mode", "secret", "counter", "pin", "password", "device_id"]
                    .contains(&key.as_str())
            }) && token.iter().all(|(key, value)| {
                if key == "counter" {
                    value.as_u64().is_some()
                } else if key == "mode" {
                    value
                        .as_str()
                        .is_some_and(|mode| matches!(mode, "hotp" | "oidc" | "stoken" | "totp"))
                } else {
                    value.is_string()
                }
            })
        })
    }) {
        return Some("token".into());
    }
    if obj.get("mobile").is_some_and(|value| {
        !value.as_object().is_some_and(|mobile| {
            mobile.keys().all(|key| {
                ["device_type", "device_unique_id", "platform_version"].contains(&key.as_str())
            }) && mobile.values().all(Value::is_string)
        })
    }) {
        return Some("mobile".into());
    }
    if obj.get("form_entries").is_some_and(|value| {
        !value.as_array().is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry.as_object().is_some_and(|fields| {
                    fields.iter().all(|(key, value)| match key.as_str() {
                        "form_id" | "submission_key" | "name" | "value" => value.is_string(),
                        "promote" => value.is_boolean(),
                        _ => false,
                    }) && !(fields.get("promote").and_then(Value::as_bool) == Some(true)
                        && fields
                            .get("value")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.is_empty()))
                })
            })
        })
    }) {
        return Some("form_entries".into());
    }
    if obj.get("fortinet_host_check").is_some_and(|value| {
        !value.as_object().is_some_and(|fields| {
            fields.iter().all(|(key, value)| {
                matches!(key.as_str(), "hostcheck" | "check_virtual_desktop") && value.is_string()
            })
        })
    }) {
        return Some("fortinet_host_check".into());
    }
    if obj.get("pull_filters").is_some_and(|value| {
        !value.as_array().is_some_and(|filters| {
            filters.iter().all(|filter| {
                filter.as_object().is_some_and(|fields| {
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "action" | "text"))
                        && fields.get("text").is_some_and(Value::is_string)
                        && fields
                            .get("action")
                            .and_then(Value::as_str)
                            .is_some_and(|action| matches!(action, "accept" | "ignore" | "reject"))
                })
            })
        })
    }) {
        return Some("pull_filters".into());
    }
    None
}

fn listable_strings(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Array(values) => values.iter().all(Value::is_string),
        _ => false,
    }
}

fn nonempty_listable_strings(value: &Value) -> bool {
    match value {
        Value::String(s) => !s.is_empty(),
        Value::Array(values) => {
            !values.is_empty()
                && values
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        }
        _ => false,
    }
}

fn remote_scalar_shape(key: &str, value: &Value, ty: &str) -> bool {
    // 丢弃字段不进入配置；内部 tag 没有本机解析语义。
    if matches!(
        key,
        "system" | "name" | "advertise_routes" | "detour" | "domain_resolver" | "http_client"
    ) {
        return true;
    }
    if matches!(
        key,
        "on_demand"
            | "no_udp"
            | "pfs"
            | "allow_insecure_crypto"
            | "compression_disabled"
            | "external_auth_disabled"
            | "http_keepalive_disabled"
            | "ipv6_disabled"
            | "password_authentication_disabled"
            | "tcp_keep_alive_enabled"
            | "xml_post_disabled"
            | "redirect_gateway"
            | "block_ipv6"
            | "ping_restart_disabled"
            | "redirect_private"
            | "remote_random"
            | "renegotiate_disabled"
            | "route_no_pull"
            | "disable_tcp_keep_alive"
            | "tcp_fast_open"
            | "tcp_multi_path"
            | "udp_fragment"
            | "reuse_addr"
            | "bind_address_no_port"
            | "disable_path_mtu_discovery"
            | "disable_version_fallback"
            | "mss_fix_disabled"
            | "static_challenge_echo"
    ) {
        return value.is_boolean();
    }
    if matches!(
        key,
        "server_port"
            | "dtls_local_port"
            | "mtu"
            | "base_mtu"
            | "queue_length"
            | "max_concurrent_streams"
            | "initial_packet_size"
            | "udp_nat_max"
            | "fragment"
            | "derp_region"
            | "replay_window"
            | "mss_fix"
            | "explicit_exit_notify"
    ) {
        return num_val(Some(value)).is_some();
    }
    if matches!(key, "renegotiate_bytes" | "renegotiate_packets") {
        return value.as_u64().is_some();
    }
    if key == "route_metric" {
        return value.as_i64().is_some();
    }
    if key == "version" && ty == "masque-client" {
        return num_val(Some(value)).is_some();
    }
    if matches!(key, "stream_receive_window" | "connection_receive_window") {
        return value.is_string() || value.is_number();
    }
    if key == "udp_timeout" && ty == "openvpn-client" {
        return value.is_string() || value.is_number();
    }
    if matches!(
        key,
        "data_ciphers"
            | "redirect_gateway_flags"
            | "address"
            | "routes"
            | "static_key"
            | "network_type"
            | "fallback_network_type"
    ) {
        return listable_strings(value);
    }
    value.is_string()
}

/// 导入时丢弃的键 → 一条告警（无则 `None`）。
///
/// 被丢的都是「存下来也不会下发」的形态：生成侧会剥掉（`system` / `advertise_routes` / 版本不兼容的
/// 调优键 / `http_client` …）或生成侧不读（MASQUE 的 `tls.alpn` 等）。悄悄存下，用户在编辑器里会以为
/// 它生效；悄悄丢掉，用户不知道文件里那份设置没被照做。故丢，并在导入预览里说出来。
fn ignored_keys_warning(name: &str, ignored: &[String]) -> Option<String> {
    (!ignored.is_empty()).then(|| {
        format!(
            "节点「{name}」导入时忽略了 Polaris 不会下发的键：{}",
            ignored.join(", ")
        )
    })
}

/// 透传袋里生成器不会下发的键 → 移出袋子并记进 `ignored`。
///
/// 「会不会下发」**问生成器本身**（`emitted` = 同一节点经生成侧构造器得到的键集），不在导入侧另抄
/// 一份剥键表：剥键常量（`MASQUE_STRIPPED_KEYS`、按版本的 H2/QUIC 键、`TAILCAT_GENERATED_KEYS`）
/// 以后怎么改，导入侧自动跟随，两边不会漂移。
fn drop_unemitted_bag_keys(
    bag: &mut serde_json::Map<String, Value>,
    emitted: &serde_json::Map<String, Value>,
    ignored: &mut Vec<String>,
) {
    bag.retain(|k, _| {
        let kept = emitted.contains_key(k);
        if !kept {
            ignored.push(k.clone());
        }
        kept
    });
}

/// `endpoints[].{type:"masque-client"}` → [`Protocol::MasqueClient`]。`None` = 计 failed。
///
/// # 映射（与生成侧 `build_masque_endpoint` 对位）
///
/// - `server` / `server_port` / `username` / `password` → 顶层（生成侧读顶层，不另存第二份）；
/// - `on_demand` / `bind_interface` → 顶层同名字段（生成侧装配层按顶层写，袋里那份会被覆盖或删掉）；
/// - `tls` 只收生成侧读的四项（`server_name` / `insecure` / 两种 pin），pin 走与其它协议同一个
///   [`keep_valid_cert_pins`]；`security` 恒 `Tls`（生成侧恒开 TLS）——不复用 [`apply_tls`]：它会把
///   `alpn` / `utls` / `ech` / `reality` 存进 `tlsSettings`，而 MASQUE 生成侧一个都不下发（`reality` 还会
///   让导入汇合点 `drop_unemitted_cert_pins` 把 pin 当成不生效而删掉）；
/// - `path` / `headers` / `version` / `mtu` → `masqueClientSettings` 具名字段，其余进透传袋；
/// - `detour` / `domain_resolver` 指向文件内的 tag，对本机无意义，静默丢（与其余导入腿同口径）。
///
/// # 与生成侧同一判据
///
/// 装配完的节点直接喂给 `build_masque_endpoint`：`Err`（path / version 非法，内核整核起不来）⇒ 计 failed，
/// 而不是导入一个生成时必被剔除的节点；袋里被它剥掉的键（`system` / `name` / `advertise_routes` /
/// 版本不兼容的调优键）⇒ 不存并告警。
fn map_endpoint_masque(
    ep: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
) -> Option<(ServerConfig, Vec<String>)> {
    const MODELED: &[&str] = &[
        "type",
        "tag",
        "server",
        "server_port",
        "username",
        "password",
        "tls",
        "path",
        "headers",
        "version",
        "mtu",
        "detour",
        "domain_resolver",
    ];
    let addr = str_ne(ep.get("server"))?;
    let port = port_val(num_val(ep.get("server_port"))?)?;
    let mut s = new_server(id_gen, ep, Protocol::MasqueClient, addr, port, sub_id, now);
    s.username = str_ne(ep.get("username"));
    s.password = str_ne(ep.get("password"));
    lift_endpoint_top_level_keys(&mut s, ep);
    s.security = Some(SecurityMode::Tls);

    let mut ignored = Vec::new();
    if let Some(tls) = ep.get("tls").and_then(Value::as_object) {
        for (k, v) in tls {
            match k.as_str() {
                "server_name"
                | "insecure"
                | "certificate_sha256"
                | "certificate_public_key_sha256" => {}
                // `enabled:false` 也记：生成侧恒开 TLS，文件里的明文意图不会被照做。
                "enabled" if v.as_bool() != Some(false) => {}
                _ => ignored.push(format!("tls.{k}")),
            }
        }
        s.tls_settings = Some(TlsSettings {
            server_name: str_ne(tls.get("server_name")),
            allow_insecure: Some(bool_true(tls.get("insecure"))),
            certificate_sha256: cert_pins_str(tls.get("certificate_sha256")),
            certificate_public_key_sha256: cert_pins_str(tls.get("certificate_public_key_sha256")),
            ..Default::default()
        });
    }
    let headers = ep.get("headers");
    if headers.is_some_and(|h| !h.is_object()) {
        ignored.push("headers".into());
    }
    let extra = endpoint_bag(ep, MODELED);
    s.masque_client_settings = Some(Box::new(MasqueClientSettings {
        path: str_ne(ep.get("path")),
        headers: headers
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        version: num_val(ep.get("version")),
        mtu: num_val(ep.get("mtu")),
        extra,
    }));

    let emitted = build_masque_endpoint(&s, "import", None, None, |_, _| {}).ok()?;
    let bag = &mut s.masque_client_settings.as_mut()?.extra;
    drop_unemitted_bag_keys(bag, &emitted.extra, &mut ignored);
    Some((s, ignored))
}

/// `outbounds[].{type:"tailcat"}` → [`Protocol::Tailcat`]。`None` = 计 failed。
///
/// 三把 key、psk、DERP 字段进 `tailcatSettings` 具名字段，其余进透传袋；`bind_interface` → 顶层；
/// `http_client` 丢弃并告警（拉图出口由生成侧决定：`direct` 或前置代理，写成 selector 会自锁）；
/// `detour` / `domain_resolver` 静默丢（文件内 tag，同其余导入腿）。
///
/// 与生成侧同一判据：`tailcat_emit_check` 不过（缺服务端 key / key 形态错 / DERP 冲突，内核整核起不来）
/// ⇒ 计 failed；servers 模式下残留的 `derp_region` / `derp_map_url` 生成侧不下发 ⇒ 不存并告警；
/// 袋里被生成器剥掉的键同样问 `build_proxy_outbound` 本身。
fn map_tailcat_outbound(
    ob: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
) -> Option<(ServerConfig, Vec<String>)> {
    const MODELED: &[&str] = &[
        "type",
        "tag",
        "server_public_key",
        "server_disco_key",
        "pre_shared_key",
        "private_key",
        "derp_region",
        "derp_map_url",
        "derp_servers",
        "bind_interface",
        "http_client",
        "detour",
        "domain_resolver",
    ];
    let mut s = new_server(id_gen, ob, Protocol::Tailcat, String::new(), 0, sub_id, now);
    s.bind_interface = str_ne(ob.get("bind_interface"));
    let mut ignored = Vec::new();
    if ob.get("http_client").is_some() {
        ignored.push("http_client".to_string());
    }
    let mut extra = serde_json::Map::new();
    if let Some(obj) = ob.as_object() {
        for (k, v) in obj {
            if !MODELED.contains(&k.as_str()) {
                extra.insert(k.clone(), v.clone());
            }
        }
    }
    let mut t = TailcatSettings {
        server_public_key: str_ne(ob.get("server_public_key")),
        server_disco_key: str_ne(ob.get("server_disco_key")),
        pre_shared_key: str_ne(ob.get("pre_shared_key")),
        private_key: str_ne(ob.get("private_key")),
        derp_region: ob.get("derp_region").and_then(Value::as_i64),
        derp_map_url: str_ne(ob.get("derp_map_url")),
        derp_servers: match ob.get("derp_servers") {
            Some(Value::Array(items)) => items.clone(),
            Some(value @ (Value::String(_) | Value::Object(_))) => vec![value.clone()],
            _ => Vec::new(),
        },
        extra,
    };
    tailcat_emit_check(Some(&t)).ok()?;
    if !t.derp_servers.is_empty() {
        // 过了判据 ⇒ 这里的 region 必 <= 0（内核视同未设）；两者生成侧在 servers 模式下都不写。
        if t.derp_region.take().is_some() {
            ignored.push("derp_region".into());
        }
        if t.derp_map_url.take().is_some() {
            ignored.push("derp_map_url".into());
        }
    }
    s.tailcat_settings = Some(Box::new(t));

    let emitted = build_proxy_outbound(&s, "import", &DomainResolver::Tag(String::new()), "", "");
    let bag = &mut s.tailcat_settings.as_mut()?.extra;
    drop_unemitted_bag_keys(bag, &emitted.extra, &mut ignored);
    Some((s, ignored))
}

fn map_wireguard_endpoint(
    ep: &Value,
    sub_id: &str,
    now: &str,
    id_gen: &mut impl FnMut() -> String,
) -> Option<ServerConfig> {
    // Polaris 单 peer 模型不能保真多 peer：取首个会静默丢掉其余路由与密钥。
    let peers = ep.get("peers")?.as_array()?;
    if peers.len() != 1 {
        return None;
    }
    let peer = &peers[0];
    // 必填：落盘门 `validate::protocol_requirement_ok("wireguard")` 要 privateKey + peerPublicKey
    // + 非空 localAddress；`sanitize_servers` 另要非空 address + port∈1..=65535。缺任一造出来也会
    // 被剔除 —— 与其静默入库再消失，不如此处计 failed 并聚合告警。
    let private_key = str_ne(ep.get("private_key"))?;
    let peer_public_key = str_ne(peer.get("public_key"))?;
    let local_address = str_array(ep.get("address"))?;
    let server_addr = str_ne(peer.get("address"))?;
    let port = port_val(num_val(peer.get("port"))?)?;

    let allowed = str_array(peer.get("allowed_ips")).unwrap_or_default();
    let specific = strip_catch_all(&allowed);
    let mut wg = WireGuardSettings {
        private_key: Some(private_key),
        local_address,
        peer_public_key: Some(peer_public_key),
        pre_shared_key: str_ne(peer.get("pre_shared_key")),
        allowed_ips: specific,
        allow_internet: Some(has_catch_all(&allowed)),
        ..Default::default()
    };
    if let Some(k) = num_val(peer.get("persistent_keepalive_interval")).filter(|k| *k > 0) {
        wg.persistent_keepalive = Some(k);
    }
    if let Some(m) = num_val(ep.get("mtu")).filter(|m| *m > 0) {
        wg.mtu = Some(m);
    }
    if let Some(workers) = num_val(ep.get("workers")).filter(|workers| *workers > 0) {
        wg.workers = Some(workers);
    }
    // reserved 恰 3 项才承载（与生成侧 `if s.reserved.len() == 3` 对称；残值等价缺席）。
    if let Some(rs) = peer.get("reserved").and_then(Value::as_array) {
        let nums: Vec<u32> = rs.iter().filter_map(|v| num_val(Some(v))).collect();
        if nums.len() == 3 && rs.len() == 3 {
            wg.reserved = nums;
        }
    }

    let name = str_ne(ep.get("tag")).unwrap_or_else(|| format!("{server_addr}:{port}"));
    let mut s = ServerConfig {
        id: id_gen(),
        name,
        protocol: Protocol::Wireguard,
        address: server_addr,
        port,
        wireguard_settings: Some(Box::new(wg)),
        subscription_id: Some(sub_id.to_string()),
        created_at: Some(now.to_string()),
        updated_at: Some(now.to_string()),
        ..Default::default()
    };
    lift_endpoint_top_level_keys(&mut s, ep);
    Some(s)
}

fn bump(counts: &mut Vec<(String, usize)>, key: String) {
    match counts.iter_mut().find(|(k, _)| *k == key) {
        Some((_, c)) => *c += 1,
        None => counts.push((key, 1)),
    }
}

fn fmt_counts(counts: &[(String, usize)]) -> String {
    counts
        .iter()
        .map(|(k, c)| format!("{k}({c})"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;

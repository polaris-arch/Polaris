//! Strict Tailscale import shared by sing-box JSON endpoints and Mihomo proxies.
//! Diagnostics contain field names only, never auth keys or control URL values.
#![forbid(unsafe_code)]

use polaris_config_engine::user_config::control_url::tailscale_control_url_reject;
use polaris_config_engine::user_config::server_config::TailscaleSettings;
use serde_json::{Map, Value};

use crate::singbox_import::ImportOrigin;

pub struct ImportedTailscale {
    pub source_tag: String,
    pub settings: TailscaleSettings,
    pub on_demand: Option<bool>,
    pub bind_interface: Option<String>,
    pub warnings: Vec<String>,
}

pub fn rejection_reason(field: &str, origin: ImportOrigin) -> &'static str {
    match field {
        "auth_key" if origin == ImportOrigin::RemoteSubscription => {
            "远程订阅不能代替本机账号授权；请在本机登录或从本地文件导入"
        }
        "udp" => "当前仅支持显式 udp: true；TCP-only Tailscale 不能等价转换",
        "relay_server_port.zero" => "随机中继端口当前不可表达，不能静默关闭或开启本机中继",
        "ssh_server.restrictions" => "无法保留 SSH 的细粒度限制，不能降级为普通开关",
        "exit_node.auto" => "固定内核不支持自动选择出口节点",
        _ => "字段无法安全或无损导入",
    }
}

fn string(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok((!s.trim().is_empty()).then(|| s.trim().to_string())),
        _ => Err(key.to_string()),
    }
}

fn boolean(obj: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(key.to_string()),
    }
}

fn port(obj: &Map<String, Value>, key: &str) -> Result<Option<u16>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| key.to_string()),
        _ => Err(key.to_string()),
    }
}

fn ssh_server(obj: &Map<String, Value>) -> Result<Option<bool>, String> {
    match obj.get("ssh_server") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Object(settings)) => {
            if settings.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "enabled" | "disable_pty" | "disable_sftp" | "disable_forwarding"
                )
            }) {
                return Err("ssh_server".into());
            }
            let enabled = settings
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("ssh_server".to_string())?;
            for key in ["disable_pty", "disable_sftp", "disable_forwarding"] {
                match settings.get(key) {
                    None | Some(Value::Bool(false)) => {}
                    Some(Value::Bool(true)) => return Err("ssh_server.restrictions".into()),
                    _ => return Err("ssh_server".into()),
                }
            }
            Ok(Some(enabled))
        }
        _ => Err("ssh_server".into()),
    }
}

fn strings(obj: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(vec![s.trim().to_string()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
                _ => Err(key.to_string()),
            })
            .collect(),
        _ => Err(key.to_string()),
    }
}

fn cidr_array(obj: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let values = match obj.get(key) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(values)) => values,
        _ => return Err(key.to_string()),
    };
    values
        .iter()
        .map(|value| {
            let cidr = value.as_str().ok_or(key.to_string())?.trim();
            if !cidr.contains('/')
                || !polaris_config_engine::user_config::rule_validate::is_valid_ip_cidr(cidr)
            {
                return Err(key.to_string());
            }
            if cidr
                .split_once('/')
                .is_some_and(|(_, prefix)| prefix.parse::<u8>() == Ok(0))
            {
                return Err(key.to_string());
            }
            Ok(cidr.to_string())
        })
        .collect()
}

const SINGBOX_KEYS: &[&str] = &[
    "type",
    "tag",
    "auth_key",
    "state_directory",
    "control_url",
    "hostname",
    "exit_node",
    "exit_node_allow_lan_access",
    "accept_routes",
    "ephemeral",
    "advertise_routes",
    "system_interface",
    "system_interface_name",
    "name",
    "advertise_tags",
    "ssh_server",
    "relay_server_port",
    "listen_port",
    "taildrop_directory",
    "on_demand",
    "bind_interface",
    "detour",
    "domain_resolver",
    "advertise_exit_node",
];

pub fn parse_singbox_tailscale(
    node: &Value,
    origin: ImportOrigin,
) -> Result<ImportedTailscale, String> {
    let obj = node.as_object().ok_or("endpoint".to_string())?;
    if let Some(key) = obj.keys().find(|k| !SINGBOX_KEYS.contains(&k.as_str())) {
        return Err(key.clone());
    }
    let source_tag = string(obj, "tag")?.ok_or("tag".to_string())?;
    let control_url = string(obj, "control_url")?;
    if let Some(url) = control_url.as_deref() {
        if tailscale_control_url_reject(url).is_some() {
            return Err("control_url".into());
        }
        let parsed = url::Url::parse(url).map_err(|_| "control_url".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("control_url".into());
        }
    }
    for key in ["detour", "domain_resolver", "name"] {
        if string(obj, key)?.is_some() {
            return Err(key.to_string());
        }
    }
    if boolean(obj, "advertise_exit_node")? == Some(true) {
        return Err("advertise_exit_node".into());
    }
    let mut warnings = Vec::new();
    if string(obj, "system_interface_name")?.is_some() {
        if origin == ImportOrigin::RemoteSubscription {
            return Err("system_interface_name".into());
        }
        warnings.push("Tailscale system_interface_name 未搬迁，改用 Polaris 管理的接口名".into());
    }
    for key in ["state_directory", "taildrop_directory"] {
        let path = string(obj, key)?;
        if path.is_some() {
            if origin == ImportOrigin::RemoteSubscription {
                return Err(key.to_string());
            }
            warnings.push(format!(
                "Tailscale 本机路径字段 {key} 未搬迁，改用 Polaris 管理的节点目录"
            ));
        }
    }
    let auth_key = string(obj, "auth_key")?;
    let advertise_routes = cidr_array(obj, "advertise_routes")?;
    let advertise_tags = strings(obj, "advertise_tags")?;
    let system_interface = boolean(obj, "system_interface")?;
    let ssh_server = ssh_server(obj)?;
    let relay_server_port = port(obj, "relay_server_port")?;
    // Core treats a present zero as "enable relay on a random port"; Polaris currently omits zero.
    if relay_server_port == Some(0) {
        return Err("relay_server_port.zero".into());
    }
    let listen_port = port(obj, "listen_port")?;
    let bind_interface = string(obj, "bind_interface")?;
    if origin == ImportOrigin::RemoteSubscription {
        for (key, active) in [
            ("auth_key", auth_key.is_some()),
            ("advertise_routes", !advertise_routes.is_empty()),
            ("advertise_tags", !advertise_tags.is_empty()),
            ("system_interface", system_interface == Some(true)),
            ("ssh_server", ssh_server == Some(true)),
            (
                "relay_server_port",
                relay_server_port.is_some_and(|p| p != 0),
            ),
            ("listen_port", listen_port.is_some_and(|p| p != 0)),
            ("bind_interface", bind_interface.is_some()),
        ] {
            if active {
                return Err(key.to_string());
            }
        }
    }
    let exit_node = string(obj, "exit_node")?;
    if exit_node
        .as_deref()
        .is_some_and(|s| s.to_ascii_lowercase().starts_with("auto:"))
    {
        return Err("exit_node.auto".into());
    }
    let settings = TailscaleSettings {
        source_tag: Some(source_tag.clone()),
        auth_key,
        control_url,
        hostname: string(obj, "hostname")?,
        exit_node,
        exit_node_allow_lan_access: boolean(obj, "exit_node_allow_lan_access")?,
        accept_routes: boolean(obj, "accept_routes")?,
        ephemeral: boolean(obj, "ephemeral")?,
        advertise_routes,
        advertise_tags,
        reverse_mesh: system_interface,
        ssh_server,
        relay_server_port,
        listen_port,
        ..Default::default()
    };
    Ok(ImportedTailscale {
        source_tag,
        settings,
        on_demand: boolean(obj, "on_demand")?,
        bind_interface,
        warnings,
    })
}

/// Mihomo Alpha uses hyphenated keys and defaults `udp` to false. Polaris cannot represent
/// TCP-only Tailscale endpoints, so only an explicit `udp: true` is equivalent.
pub fn parse_mihomo_tailscale(
    node: &serde_yaml::Value,
    origin: ImportOrigin,
) -> Result<ImportedTailscale, String> {
    let raw = serde_json::to_value(node).map_err(|_| "node".to_string())?;
    let obj = raw.as_object().ok_or("node".to_string())?;
    if obj.get("udp") != Some(&Value::Bool(true)) {
        return Err("udp".into());
    }
    let mut mapped = Map::new();
    mapped.insert("type".into(), Value::String("tailscale".into()));
    for (from, to) in [
        ("name", "tag"),
        ("auth-key", "auth_key"),
        ("state-dir", "state_directory"),
        ("control-url", "control_url"),
        ("hostname", "hostname"),
        ("ephemeral", "ephemeral"),
        ("accept-routes", "accept_routes"),
        ("exit-node", "exit_node"),
        ("exit-node-allow-lan-access", "exit_node_allow_lan_access"),
    ] {
        if let Some(value) = obj.get(from) {
            mapped.insert(to.into(), value.clone());
        }
    }
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "type"
                | "udp"
                | "name"
                | "auth-key"
                | "state-dir"
                | "control-url"
                | "hostname"
                | "ephemeral"
                | "accept-routes"
                | "exit-node"
                | "exit-node-allow-lan-access"
        ) {
            continue;
        }
        // BasicOption's inert defaults are harmless; active host/routing knobs cannot be mapped.
        if matches!(key.as_str(), "dialer-proxy" | "interface-name")
            && value.as_str().is_some_and(str::is_empty)
        {
            continue;
        }
        if key == "routing-mark" && value.as_u64() == Some(0) {
            continue;
        }
        if key == "ip-version" && value.as_str().is_some_and(|s| s.is_empty() || s == "dual") {
            continue;
        }
        return Err(key.clone());
    }
    parse_singbox_tailscale(&Value::Object(mapped), origin)
}

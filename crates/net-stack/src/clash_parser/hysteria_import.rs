use super::{bool_val, clash_cert_pin, str, to_alpn_fn};
use polaris_config_engine::user_config::protocol_settings::{HysteriaSettings, TlsSettings};
use polaris_config_engine::user_config::server_config::{SecurityMode, ServerConfig};
use serde_yaml::Value;

/// mihomo 的带宽字符串按 Mbps 规整；小于 1 Mbps 或不能整除的值无法用当前模型保真。
fn mbps(value: Option<&Value>) -> Option<u32> {
    let text = str(value?)?;
    let text = text.trim().to_ascii_lowercase().replace(' ', "");
    let (number, factor) = if let Some(n) = text.strip_suffix("gbps") {
        (n, 1000.0)
    } else if let Some(n) = text.strip_suffix("mbps") {
        (n, 1.0)
    } else if let Some(n) = text.strip_suffix("kbps") {
        (n, 0.001)
    } else if let Some(n) = text.strip_suffix("bps") {
        (n, 0.000001)
    } else {
        (text.as_str(), 1.0)
    };
    let value = number.parse::<f64>().ok()? * factor;
    (value.is_finite() && value >= 1.0 && value <= f64::from(u32::MAX) && value.fract() == 0.0)
        .then_some(value as u32)
}

fn ports(text: &str) -> Option<String> {
    let mut segments = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        let (start, end) = part.split_once('-').unwrap_or((part, part));
        let start = start.parse::<u16>().ok().filter(|p| *p != 0)?;
        let end = end.parse::<u16>().ok().filter(|p| *p >= start)?;
        segments.push(format!("{start}:{end}"));
    }
    (!segments.is_empty()).then(|| segments.join(","))
}

pub(super) fn apply_hysteria_proxy(node: &Value, config: &mut ServerConfig) -> Result<(), String> {
    let name = &config.name;
    for key in ["protocol", "obfs-protocol"] {
        if node
            .get(key)
            .and_then(str)
            .is_some_and(|value| !value.is_empty() && value != "udp")
        {
            return Err(format!(
                "hysteria 节点「{name}」的 {key} 与 sing-box v1 不兼容"
            ));
        }
    }
    for key in ["certificate", "private-key", "ech-opts"] {
        if node.get(key).is_some() {
            return Err(format!("hysteria 节点「{name}」的 {key} 无法保真转换"));
        }
    }
    if node.get("fast-open").and_then(bool_val) == Some(true) {
        return Err(format!("hysteria 节点「{name}」的 fast-open 无法保真转换"));
    }
    let up = mbps(node.get("up-speed"))
        .or_else(|| mbps(node.get("up")))
        .ok_or_else(|| format!("hysteria 节点「{name}」缺可保真的 up 带宽"))?;
    let down = mbps(node.get("down-speed"))
        .or_else(|| mbps(node.get("down")))
        .ok_or_else(|| format!("hysteria 节点「{name}」缺可保真的 down 带宽"))?;
    let mut hy = HysteriaSettings {
        // mihomo 同时有 auth/auth-str 时 auth(base64) 优先。
        auth: node.get("auth").and_then(str).filter(|s| !s.is_empty()),
        auth_str: node.get("auth-str").and_then(str).filter(|s| !s.is_empty()),
        up_mbps: Some(up),
        down_mbps: Some(down),
        obfs: node.get("obfs").and_then(str).filter(|s| !s.is_empty()),
        ..Default::default()
    };
    if hy.auth.is_some() {
        hy.auth_str = None;
    }
    if let Some(raw) = node.get("ports").and_then(str) {
        hy.server_ports =
            Some(ports(&raw).ok_or_else(|| format!("hysteria 节点「{name}」ports 格式非法"))?);
    }
    if let Some(hop) = node.get("hop-interval") {
        let seconds = super::num(hop)
            .ok_or_else(|| format!("hysteria 节点「{name}」hop-interval 必须为秒数"))?;
        hy.hop_interval = Some(format!("{seconds}s"));
    }
    if let Some(window) = node.get("recv-window-conn").and_then(super::num) {
        hy.extra
            .insert("stream_receive_window".into(), window.into());
    }
    if let Some(window) = node.get("recv-window").and_then(super::num) {
        hy.extra
            .insert("connection_receive_window".into(), window.into());
    }
    if let Some(disable) = node.get("disable-mtu-discovery").and_then(bool_val) {
        hy.extra
            .insert("disable_path_mtu_discovery".into(), disable.into());
    }
    config.hysteria_settings = Some(Box::new(hy));
    config.security = Some(SecurityMode::Tls);
    let mut tls = TlsSettings::default();
    let sni = node
        .get("sni")
        .and_then(str)
        .or_else(|| node.get("servername").and_then(str));
    let cert_name = node.get("name-cert-verify").and_then(str);
    if sni.is_some() && cert_name.is_some() && sni != cert_name {
        return Err(format!(
            "hysteria 节点「{name}」的 sni 与 name-cert-verify 不同，无法保真"
        ));
    }
    tls.server_name = cert_name.or(sni);
    tls.allow_insecure = node.get("skip-cert-verify").and_then(bool_val);
    tls.alpn = node.get("alpn").and_then(to_alpn_fn);
    tls.certificate_sha256 = clash_cert_pin(node);
    if tls.server_name.is_some()
        || tls.allow_insecure.is_some()
        || tls.alpn.is_some()
        || tls.certificate_sha256.is_some()
    {
        config.tls_settings = Some(tls);
    }
    Ok(())
}

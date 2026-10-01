//! mihomo `type: openvpn` → sing-box `openvpn-client` endpoint.
//!
//! The two cores do not share a wire schema. Only fields with a known endpoint equivalent are
//! emitted; a non-equivalent setting fails this node instead of making a misleading VPN entry.

use super::*;
use polaris_config_engine::user_config::protocol_settings::{
    OpenvpnClientSettings, OpenvpnTlsSettings,
};
use serde_json::{json, Map as JsonMap};

pub(super) fn parse_openvpn_proxy(
    proxy: &Value,
    mut server: ServerConfig,
) -> Result<ServerConfig, String> {
    let fields = proxy
        .as_mapping()
        .ok_or_else(|| "OpenVPN 节点必须是对象".to_owned())?;
    for (key, value) in fields {
        let key = key
            .as_str()
            .ok_or_else(|| "OpenVPN 节点字段名必须是字符串".to_owned())?;
        if !matches!(
            key,
            "name"
                | "type"
                | "server"
                | "port"
                | "proto"
                | "dev"
                | "cipher"
                | "data-ciphers"
                | "data-ciphers-fallback"
                | "auth"
                | "comp-lzo"
                | "ca"
                | "cert"
                | "key"
                | "tls-auth"
                | "key-direction"
                | "tls-crypt"
                | "tls-crypt-v2"
                | "username"
                | "password"
                | "ping"
                | "ping-restart"
                | "mtu"
                | "udp"
                | "tfo"
                | "mptcp"
        ) && !value.is_null()
        {
            return Err(format!("OpenVPN 字段 `{key}` 无法安全转换到 sing-box"));
        }
    }

    let address = required_string(proxy, "server")?;
    if address.trim().is_empty() {
        return Err("OpenVPN `server` 不能为空".to_owned());
    }
    let port = unsigned(proxy, "port")?.ok_or("OpenVPN 缺 `port`")?;
    let port = u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .ok_or("OpenVPN `port` 必须在 1..=65535")?;
    server.address = address.clone();
    server.port = port;

    let proto = optional_string(proxy, "proto")?
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let network = match proto.as_str() {
        "" | "udp" | "udp4" => "udp",
        "tcp" | "tcp-client" | "tcp4" | "tcp4-client" => "tcp",
        _ => return Err(format!("OpenVPN `proto` 不支持 `{proto}`")),
    };
    if let Some(dev) = optional_string(proxy, "dev")? {
        if !dev.eq_ignore_ascii_case("tun") {
            return Err("OpenVPN `dev` 仅支持 tun".to_owned());
        }
    }
    // mihomo's OpenVPN `udp` is outbound UDP capability, not transport. Its source dials by
    // ClientConfig.Proto and sets Base.UDP=true regardless of this field.
    if proxy
        .get("udp")
        .is_some_and(|v| !matches!(v, Value::Bool(_)))
    {
        return Err("OpenVPN `udp` 必须是布尔值".to_owned());
    }

    let ca = inline_material(proxy, "ca", "-----BEGIN CERTIFICATE-----")?
        .ok_or("OpenVPN 缺内联 `ca` 证书")?;
    let cert = inline_material(proxy, "cert", "-----BEGIN CERTIFICATE-----")?;
    let key = inline_material(proxy, "key", "-----BEGIN ")?;
    if let Some(key) = &key {
        let marker = key.trim().lines().next().unwrap_or_default();
        if !matches!(
            marker,
            "-----BEGIN PRIVATE KEY-----"
                | "-----BEGIN RSA PRIVATE KEY-----"
                | "-----BEGIN EC PRIVATE KEY-----"
        ) {
            return Err("OpenVPN `key` 必须是未加密的内联 PEM 私钥".to_owned());
        }
    }
    if cert.is_some() != key.is_some() {
        return Err("OpenVPN `cert` 与 `key` 必须同时提供".to_owned());
    }
    let username = optional_string(proxy, "username")?;
    let password = optional_string(proxy, "password")?;
    if cert.is_none()
        && username
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err("OpenVPN 需要 `username` 或内联 `cert`+`key`".to_owned());
    }

    let tls_auth = inline_material(proxy, "tls-auth", "-----BEGIN OpenVPN Static key V1-----")?;
    let tls_crypt = inline_material(proxy, "tls-crypt", "-----BEGIN OpenVPN Static key V1-----")?;
    let tls_crypt_v2 = inline_material(
        proxy,
        "tls-crypt-v2",
        "-----BEGIN OpenVPN tls-crypt-v2 client key-----",
    )?;
    if [
        tls_auth.is_some(),
        tls_crypt.is_some(),
        tls_crypt_v2.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
        > 1
    {
        return Err("OpenVPN `tls-auth` / `tls-crypt` / `tls-crypt-v2` 互斥".to_owned());
    }
    let key_direction = match proxy.get("key-direction") {
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => optional_string(proxy, "key-direction")?,
    };
    if key_direction.is_some() && tls_auth.is_none() {
        return Err("OpenVPN `key-direction` 仅能与 `tls-auth` 一起使用".to_owned());
    }
    let direction = match key_direction.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("0") => Some("server"),
        Some("1") => Some("client"),
        _ => return Err("OpenVPN `key-direction` 仅支持 0 或 1".to_owned()),
    };

    let mut tls = OpenvpnTlsSettings {
        certificate: vec![ca],
        client_certificate: cert.into_iter().collect(),
        client_key: key.into_iter().collect(),
        ..Default::default()
    };
    if let Some((kind, key)) = tls_auth
        .map(|key| ("tls_auth", key))
        .or_else(|| tls_crypt.map(|key| ("tls_crypt", key)))
        .or_else(|| tls_crypt_v2.map(|key| ("tls_crypt_v2", key)))
    {
        let mut wrap = json!({"type": kind, "key": [key]});
        if let Some(direction) = direction {
            wrap["direction"] = json!(direction);
        }
        tls.extra.insert("control_wrap".to_owned(), wrap);
    }

    let cipher = optional_string(proxy, "cipher")?
        .map(|cipher| canonical_cipher(&cipher))
        .transpose()?;
    let fallback = optional_string(proxy, "data-ciphers-fallback")?
        .map(|cipher| canonical_cipher(&cipher))
        .transpose()?;
    let primary_cipher = cipher.unwrap_or_else(|| "AES-128-GCM".to_owned());
    if fallback
        .as_deref()
        .is_some_and(|value| value != primary_cipher.as_str())
    {
        return Err("OpenVPN `cipher` 与不同的 `data-ciphers-fallback` 无法等价转换".to_owned());
    }
    let data_ciphers = match proxy.get("data-ciphers") {
        None | Some(Value::Null) => None,
        Some(Value::Sequence(values)) if !values.is_empty() => Some(
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| "OpenVPN `data-ciphers` 必须是非空字符串数组".to_owned())
                        .and_then(canonical_cipher)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => return Err("OpenVPN `data-ciphers` 必须是非空字符串数组".to_owned()),
    };
    let auth = optional_string(proxy, "auth")?
        .map(|auth| {
            let auth = auth.trim().to_ascii_uppercase().replace("SHA-1", "SHA1");
            if matches!(
                auth.as_str(),
                "MD5" | "SHA1" | "SHA256" | "SHA384" | "SHA512"
            ) {
                Ok(auth)
            } else {
                Err("OpenVPN `auth` 算法不支持".to_owned())
            }
        })
        .transpose()?
        .unwrap_or_else(|| "SHA256".to_owned());

    let mut extra = JsonMap::new();
    for (from, to) in [("tfo", "tcp_fast_open"), ("mptcp", "tcp_multi_path")] {
        match proxy.get(from) {
            None | Some(Value::Null | Value::Bool(false)) => {}
            Some(Value::Bool(true)) => {
                extra.insert(to.to_owned(), json!(true));
            }
            _ => return Err(format!("OpenVPN `{from}` 必须是布尔值")),
        }
    }
    if let Some(data_ciphers) = data_ciphers {
        extra.insert("data_ciphers".to_owned(), json!(data_ciphers));
    }
    // TLS mode rejects `cipher`. The legacy cipher is its closest equivalent as a fallback;
    // retaining it in the advertised list also allows a server to push that cipher explicitly.
    let fallback_cipher = fallback.unwrap_or_else(|| primary_cipher.clone());
    if !extra.contains_key("data_ciphers") {
        extra.insert("data_ciphers".to_owned(), json!([primary_cipher]));
    }
    extra.insert("data_ciphers_fallback".to_owned(), json!(fallback_cipher));
    if let Some(comp_lzo) = optional_string(proxy, "comp-lzo")? {
        let value = match comp_lzo.trim().to_ascii_lowercase().as_str() {
            "yes" | "adaptive" => "yes",
            "no" => "no",
            _ => return Err("OpenVPN `comp-lzo` 仅支持 yes/no/adaptive".to_owned()),
        };
        extra.insert("compression_lzo".to_owned(), json!(value));
        if value == "yes" {
            extra.insert("allow_compression".to_owned(), json!("yes"));
        }
    }
    for (from, to) in [("ping", "ping_interval"), ("ping-restart", "ping_restart")] {
        if let Some(seconds) = unsigned(proxy, from)? {
            let _ = i64::try_from(seconds)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1_000_000_000))
                .ok_or_else(|| format!("OpenVPN `{from}` 秒数过大"))?;
            if seconds > 0 {
                extra.insert(to.to_owned(), json!(format!("{seconds}s")));
            }
        }
    }
    let mtu = unsigned(proxy, "mtu")?
        .map(|mtu| {
            u32::try_from(mtu)
                .ok()
                .filter(|mtu| *mtu > 0)
                .ok_or("OpenVPN `mtu` 必须为正的 32 位整数")
        })
        .transpose()?;

    server.protocol = Protocol::OpenvpnClient;
    server.openvpn_client_settings = Some(Box::new(OpenvpnClientSettings {
        server: Some(address),
        server_port: Some(port),
        username,
        password,
        network: Some(network.to_owned()),
        auth: Some(auth),
        mtu,
        tls: Some(tls),
        extra,
        ..Default::default()
    }));
    Ok(server)
}

fn optional_string(proxy: &Value, key: &str) -> Result<Option<String>, String> {
    match proxy.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(format!("OpenVPN `{key}` 必须是非空字符串")),
    }
}

fn required_string(proxy: &Value, key: &str) -> Result<String, String> {
    optional_string(proxy, key)?.ok_or_else(|| format!("OpenVPN 缺 `{key}`"))
}

fn inline_material(proxy: &Value, key: &str, prefix: &str) -> Result<Option<String>, String> {
    let value = optional_string(proxy, key)?;
    if let Some(value) = &value {
        let value = value.trim();
        let begin = value.lines().next().unwrap_or_default();
        let end = begin.replacen("BEGIN", "END", 1);
        if !begin.starts_with(prefix) || !value.lines().any(|line| line.trim() == end) {
            return Err(format!(
                "OpenVPN `{key}` 必须是内联 PEM 内容，不能是本机文件路径"
            ));
        }
    }
    Ok(value)
}

fn unsigned(proxy: &Value, key: &str) -> Result<Option<u64>, String> {
    match proxy.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("OpenVPN `{key}` 必须是非负整数")),
        Some(Value::String(text)) => text
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("OpenVPN `{key}` 必须是非负整数")),
        _ => Err(format!("OpenVPN `{key}` 必须是非负整数")),
    }
}

fn canonical_cipher(cipher: &str) -> Result<String, String> {
    let cipher = cipher.trim().to_ascii_uppercase();
    let cipher = if cipher == "AES-CBC" {
        "AES-128-CBC".to_owned()
    } else {
        cipher
    };
    if matches!(
        cipher.as_str(),
        "AES-128-GCM"
            | "AES-192-GCM"
            | "AES-256-GCM"
            | "AES-128-CBC"
            | "AES-192-CBC"
            | "AES-256-CBC"
            | "CHACHA20-POLY1305"
    ) {
        Ok(cipher)
    } else {
        Err("OpenVPN 加密算法不受 mihomo/sing-box 共同支持".to_owned())
    }
}

#[cfg(test)]
mod tests;

//! TLS 证书固定（sing-box 出站 `tls.certificate_sha256` / `tls.certificate_public_key_sha256`）。
//!
//! # 为什么生成侧必须转码，不能原样下发
//!
//! 两个内核字段是 Go `badoption.Listable[[]byte]`，JSON 里 `[]byte` 是**标准 base64**
//! （`option/tls.go` v1.15.0-alpha.7 :121-122）。而用户/订阅手里的几乎都是 hex：mihomo
//! `fingerprint`、hysteria2 `pinSHA256`、Xray `pinnedPeerCertSha256`/`pcs`、`openssl x509 -fingerprint`
//! 的冒号 hex。实测（随包核 1.15.0-alpha.7 `sing-box check`）：
//!  - 64 位 hex **rc=0 照收** —— hex 字符恰好都在 base64 字母表里，被解成 48 字节垃圾；
//!  - `"AAAA"`（3 字节）同样 rc=0 —— check 只做 decode，不校长度；
//!  - 冒号 hex / 无填充 base64 / url-safe base64 → decode FATAL（整份配置起不来）。
//!
//! 而校验处 `VerifyPinnedCertificate`（`common/tls/std_client.go` :279-300）是 `bytes.Equal` 比 32 字节
//! 摘要 ⇒ 原样下发 hex = **每次握手都失败，且 check 与起核都不报错**。故本模块是这件事唯一的门。
//!
//! # 输入口径
//!
//! 逗号分隔多条（Xray `pinnedPeerCertSha256` 与 v2rayN `pcs` 即逗号串，内核字段本身是列表）。每条接受：
//!  - hex，可带 `:`/`-` 分隔（mihomo 去 `:`、hysteria 去 `:` 与 `-`，见各导入点注释）；
//!  - 标准 base64（44 字符带填充）—— sing-box JSON 导入原样带来的形态，也是内核报错
//!    `unrecognized peer certificate: sha256 <base64>` 打印的形态，用户会直接抄它。
//!
//! 两种形态按长度天然可分（32 字节 = 64 hex = 44 base64），无歧义。解码后不是 32 字节的条目一律视为非法。

#![forbid(unsafe_code)]

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 单条 pin → 32 字节 SHA-256 摘要；形态不合法或长度不对 → `None`。
fn decode_pin(item: &str) -> Option<[u8; 32]> {
    let item = item.trim();
    let hex: String = item.chars().filter(|c| *c != ':' && *c != '-').collect();
    let bytes = if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?
    } else {
        decode_std_base64(item)?
    };
    bytes.try_into().ok()
}

/// 严格标准 base64（带填充）。只为识别 44 字符那一种形态，故不容忍空白/无填充/url-safe ——
/// 与内核 decode 的口径一致（后三者内核同样 FATAL）。
fn decode_std_base64(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.is_empty() || !s.len().is_multiple_of(4) {
        return None;
    }
    let pad = s.iter().rev().take_while(|&&b| b == b'=').count();
    if pad > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    for (i, &b) in s[..s.len() - pad].iter().enumerate() {
        let v = B64.iter().position(|&c| c == b)? as u32;
        acc = (acc << 6) | v;
        if i % 4 == 3 {
            out.extend_from_slice(&acc.to_be_bytes()[1..]);
            acc = 0;
        }
    }
    match pad {
        2 => out.push((acc >> 4) as u8),
        1 => out.extend_from_slice(&((acc >> 2) as u16).to_be_bytes()),
        _ => {}
    }
    Some(out)
}

fn encode_std_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, &b| (a << 8) | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// 生成侧：用户/订阅串 → 内核形态（每条标准 base64）。
///
/// Explicit malformed constraints fail closed, including mixed valid/invalid lists.
/// Empty editor fields mean unset; invalid nonempty input never becomes no pins.
pub fn cert_pins_for_kernel(raw: Option<&str>) -> Result<Option<Vec<String>>, &'static str> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Ok(None);
    };
    let pins = raw
        .split(',')
        .map(|item| {
            decode_pin(item)
                .map(|d| encode_std_base64(&d))
                .ok_or("TLS_PIN_INVALID")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(pins))
}

/// Importers preserve nonempty constraints until shared validation can reject the
/// node with an explicit failure. Dropping malformed input would remove security.
pub fn preserve_cert_pins(raw: &str) -> Option<String> {
    (!raw.trim().is_empty()).then(|| raw.trim().to_owned())
}

pub fn validate_server_cert_pins(
    server: &crate::user_config::server_config::ServerConfig,
) -> Result<(), &'static str> {
    if pin_is_emitted(server) {
        if let Some(tls) = &server.tls_settings {
            cert_pins_for_kernel(tls.certificate_sha256.as_deref())?;
            cert_pins_for_kernel(tls.certificate_public_key_sha256.as_deref())?;
        }
    }
    Ok(())
}

/// Validation-only helper retained for the share-link exporter. Importers must use
/// preserve_cert_pins plus validate_server_cert_pins; filtering import constraints
/// would silently weaken authentication when all supplied values are malformed.
pub fn keep_valid_cert_pins(raw: &str) -> Option<String> {
    let kept: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|item| decode_pin(item).is_some())
        .collect();
    (!kept.is_empty()).then(|| kept.join(","))
}

/// Applicability of the modeled tlsSettings pin fields. Raw custom/VPN payloads
/// have separate schemas; they do not consume these modeled fields. MASQUE builds
/// its own ordinary TLS block regardless of the generic security selector.
pub fn modeled_cert_pins_are_emitted(
    protocol: crate::user_config::server_config::Protocol,
    security: Option<&crate::user_config::server_config::SecurityMode>,
) -> bool {
    use crate::user_config::server_config::Protocol;
    match protocol {
        Protocol::MasqueClient => true,
        Protocol::Naive
        | Protocol::Tor
        | Protocol::Tailcat
        | Protocol::Ssh
        | Protocol::Custom
        | Protocol::Wireguard
        | Protocol::Tailscale
        | Protocol::Openconnect
        | Protocol::OpenvpnClient => false,
        Protocol::Vless
        | Protocol::Trojan
        | Protocol::Hysteria2
        | Protocol::Shadowsocks
        | Protocol::Anytls
        | Protocol::Tuic
        | Protocol::Vmess
        | Protocol::Snell
        | Protocol::Socks
        | Protocol::Http
        | Protocol::Hysteria => !security.is_some_and(|security| security.is_reality()),
    }
}

fn pin_is_emitted(s: &crate::user_config::server_config::ServerConfig) -> bool {
    modeled_cert_pins_are_emitted(s.protocol, s.security.as_ref())
}

/// 导入侧：剔除生成侧不会消费的建模 pin，复用同一能力判据。
///
/// 这类值存下来只会「存着、看不到（编辑器在 reality 下隐藏、naive 不提供）、不生效」，
/// 用户也无从处理 —— 不存。放在导入汇合点调用，而不是各导入器里各判一次：安全层（reality）
/// 与 pin 在不同导入器里解析顺序不同，汇合点拿到的是装配完的节点。
pub fn drop_unemitted_cert_pins(servers: &mut [crate::user_config::server_config::ServerConfig]) {
    for s in servers.iter_mut().filter(|s| !pin_is_emitted(s)) {
        if let Some(t) = s.tls_settings.as_mut() {
            t.certificate_sha256 = None;
            t.certificate_public_key_sha256 = None;
        }
    }
}

/// 导入预览提示：本批里会真正下发 pin 的节点数 > 0 时给一句话，否则 `None`。
///
/// 为什么要提示：mihomo / Xray 的 pin 命中证书链上**任一张**即可（含上级 CA），sing-box 只比
/// **服务器证书本身**（`common/tls/std_client.go` v1.15.0-alpha.7 `VerifyPinnedCertificate` 只看
/// `rawCerts[0]`）。订阅固定的若是 CA，导入后该节点握手必败（失败关闭，不放行）——而用户在导入
/// 这一刻最容易把「连不上」和「订阅刚导入」联系起来，故提示放在导入预览。
pub fn cert_pin_import_warning(
    servers: &[crate::user_config::server_config::ServerConfig],
) -> Option<String> {
    let n = servers
        .iter()
        .filter(|s| pin_is_emitted(s))
        .filter(|s| {
            s.tls_settings.as_ref().is_some_and(|t| {
                t.certificate_sha256.is_some() || t.certificate_public_key_sha256.is_some()
            })
        })
        .count();
    (n > 0).then(|| {
        format!(
            "{n} 个节点带证书固定：Polaris 只比对服务器证书本身，若订阅固定的是上级（CA）证书，\
             这些节点会连不上——可在节点编辑里改填服务器证书的指纹或清空"
        )
    })
}

#[cfg(test)]
mod tests;

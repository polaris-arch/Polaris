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
/// **非法条目丢弃、合法条目保留**：UI 保存时已拒绝非法值（`proto-codec.ts` 的 `certPinInvalid`），
/// 走到这里的非法值只可能来自手改配置文件。逐条丢而不是整字段丢，是因为整字段丢会让该节点
/// 退回「不固定」（若同时开了 insecure 即任意证书放行），逐条丢只是少一个可匹配项、仍然 fail-closed。
/// 全部非法/为空 → `None`（不产生键）。
pub fn cert_pins_for_kernel(raw: Option<&str>) -> Option<Vec<String>> {
    let pins: Vec<String> = raw?
        .split(',')
        .filter_map(decode_pin)
        .map(|d| encode_std_base64(&d))
        .collect();
    (!pins.is_empty()).then_some(pins)
}

/// 导入侧：只保留合法条目的**原文**（逗号拼接），无合法条目 → `None`。
///
/// 保留原文而不是转成 base64：UI 里给用户看的是订阅给的那个 hex，转码只在生成侧做一次。
/// 过滤非法条目是为了不把一个导入进来的死值留给 UI —— 否则该节点在编辑器里保存必被拒
/// （典型：老 Clash 配置把浏览器名 `chrome` 误写进 `fingerprint`，mihomo 对它是报错而非忽略）。
pub fn keep_valid_cert_pins(raw: &str) -> Option<String> {
    let kept: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|item| decode_pin(item).is_some())
        .collect();
    (!kept.is_empty()).then(|| kept.join(","))
}

#[cfg(test)]
mod tests;

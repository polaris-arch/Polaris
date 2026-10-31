//! Tailscale `control_url` 的本地格式策略，与前端 `domain/control-url.ts` 同步。
//!
//! 已核验的 Polaris alpha.12.6 (`bfcdf96b...`) 的 legacy constructor 固定以
//! `RemoteIsDomain: true, ResolverOnDetour: true` 创建 resolver wrapper；physical 路径使用
//! runtime dialer，binding 则检查 `ResolveDialer` 断言。旧“IP host 必 panic”依据已不成立。
//! resolver wrapper 将 IP destination 直接下放，不要求把控制面伪装成域名。
//!
//! 本策略允许四段 IPv4、正确方括号 IPv6（含 v4-mapped）；前导零 IPv4、括号 IPv4、裸 IPv6、
//! URI-zone 仍沿原拒绝边界。缺 scheme/host、畸形 URL 仍在生成前剔除，原域名/userinfo/scheme
//! 处理不变。此处不改 DNS/SNI/CA，也不保证控制面可达、TLS 证书匹配或账户授权成功。

#![forbid(unsafe_code)]

use crate::user_config::ip::{is_ip_literal, strip_brackets};

/// `control_url` 被拒的成因。取值经 [`reject_token`] 转成**稳定机器 token** 下发前端换 i18n 文案，
/// 故枚举项的语义不得复用（要新增成因就加新项，别把旧项的含义改掉）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlUrlReject {
    /// 未受本策略支持的 IP 写法；保留原稳定 token，规范 IPv4 / IPv6 不再返回它。
    IpLiteral,
    /// 缺合法的 `scheme://`。
    MissingScheme,
    /// 有 scheme 但 host 为空（`http://`、`http://:8080`）。
    NoHost,
    /// host 畸形（裸 IPv6、方括号不配平、内嵌空白等）。
    Malformed,
}

/// [`ControlUrlReject`] → 稳定机器 token。
///
/// 前端按 token 查 i18n 文案（`ui/src/domain/invalid-node-reason.ts`），**不渲染 token 本身**；
/// `ui/src/contracts/invalid-node-reason-coverage.test.ts` 双向对账本函数与前端映射表。
pub fn reject_token(reject: ControlUrlReject) -> &'static str {
    match reject {
        ControlUrlReject::IpLiteral => "control-url-ip",
        ControlUrlReject::MissingScheme => "control-url-scheme",
        ControlUrlReject::NoHost | ControlUrlReject::Malformed => "control-url-invalid",
    }
}

/// 分类原有不支持的 IP 写法，保留其拒绝 token；zone 不得漏成普通域名。
/// 公共 IP 谓词刻意容许前导零/宽松 v6，真正放行须另经标准库严格解析。
fn is_ip_host(host: &str) -> bool {
    let h = strip_brackets(host);
    let h = h.split('%').next().unwrap_or(h);
    is_ip_literal(h) || h.parse::<std::net::IpAddr>().is_ok()
}

/// 端口后缀（`:8080`）判定。空端口（`host:`）不算合法后缀。
fn is_port_suffix(s: &str) -> bool {
    match s.strip_prefix(':') {
        Some(p) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// host 是否**可能**是域名。
///
/// 刻意只列**否定**字符（`: / ? # @ [ ] \` 与控制字符），不做 LDH 白名单：headscale 用 IDN 域名
/// 完全合法，白名单会把它误伤成非法。走到本函数时 `/ ? # @` 已在上游切走，剩下的主要是裸 IPv6 的冒号
/// 与不配平的方括号。
fn is_hostname_like(host: &str) -> bool {
    !host.is_empty()
        && !host
            .chars()
            .any(|c| matches!(c, ':' | '/' | '?' | '#' | '@' | '[' | ']' | '\\') || c.is_control())
}

/// Tailscale `control_url` 的前置校验：`None` = 可下发，`Some(_)` = **必须拦在下发之前**。
///
/// 空串 / 全空白 → `None`（用户没填 = 官方 controlplane）。
pub fn tailscale_control_url_reject(raw: &str) -> Option<ControlUrlReject> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // 内嵌空白 → Go `url.Parse` 直接报错（实测 `parse control URL`），FATAL 掉整个核。
    if s.chars().any(char::is_whitespace) {
        return Some(ControlUrlReject::Malformed);
    }

    // scheme：必须有 `://`，且 scheme 本身合法（ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )）。
    // 沿用原 scheme 语法，不在本批增加协议种类/连通性策略。
    let Some(pos) = s.find("://") else {
        return Some(ControlUrlReject::MissingScheme);
    };
    let scheme = &s[..pos];
    if !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return Some(ControlUrlReject::MissingScheme);
    }

    // authority = scheme 之后、首个 `/ ? #` 之前；再剥 userinfo（内核 `url.Hostname()` 同样只取 host）。
    let rest = &s[pos + 3..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };

    // 只新增严格方括号 IPv6。标准库拒 zone / 括号 IPv4 / 畸形压缩，维持原拒绝边界。
    if let Some(stripped) = hostport.strip_prefix('[') {
        let Some(end) = stripped.find(']') else {
            return Some(ControlUrlReject::Malformed);
        };
        let inner = &stripped[..end];
        let after = &stripped[end + 1..];
        if !after.is_empty() && !is_port_suffix(after) {
            return Some(ControlUrlReject::Malformed);
        }
        if inner.parse::<std::net::Ipv6Addr>().is_ok() {
            return None;
        }
        return Some(if is_ip_host(inner) {
            ControlUrlReject::IpLiteral
        } else {
            ControlUrlReject::Malformed
        });
    }

    // 非方括号：末段全数字才当端口剥掉；否则残留的冒号意味着裸 IPv6 之类的畸形。
    let host = match hostport.rfind(':') {
        Some(i) if is_port_suffix(&hostport[i..]) => &hostport[..i],
        Some(_) => return Some(ControlUrlReject::Malformed),
        None => hostport,
    };

    if host.is_empty() {
        return Some(ControlUrlReject::NoHost);
    }
    if is_ip_host(host) {
        return if host.parse::<std::net::Ipv4Addr>().is_ok() {
            None
        } else {
            Some(ControlUrlReject::IpLiteral)
        };
    }
    if !is_hostname_like(host) {
        return Some(ControlUrlReject::Malformed);
    }
    None
}

#[cfg(test)]
mod tests;

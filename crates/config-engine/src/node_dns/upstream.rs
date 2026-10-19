//! 节点域名解析上游模型 —— 上游 `shared/node-resolver-upstreams.ts` 1:1 移植。
//!
//! 纯逻辑、无 I/O、可逐项单测：
//! - 内置上游单一真值（`ali` / `dnspod` / `system`），IP 取自 [`DOH_ALIDNS_IP`] / [`DOH_DNSPOD_IP`]，
//!   单测同时保留独立引导 DNS 的内置地址一致性护栏。
//! - 自定义上游**强制纯 IP**（`parse_dns_server_spec().is_domain` 拒绝）：零 bootstrap + 直连放行确定。
//! - Tier1（加密 DoH）抢跑、上限 3；Tier2（明文 UDP / system）兜底，不占额度、不与 Tier1 抢跑。
//! - canonical 去重：内置与等价自定义合并（**先去重再数上限**，否则重复项会挤掉真上游）。

#![forbid(unsafe_code)]

use crate::user_config::dns_config::{CustomDnsUpstream, DnsConfig};
use crate::user_config::dns_spec::{parse_dns_server_spec, DnsServerType};
use crate::user_config::proxy_mode::ProxyModeType;

/// AliDNS IP-DoH 上游地址。**不变量**：∈ `BOOTSTRAP_DIRECT_DNS_IPS`（单测护栏）。上游 `DOH_ALIDNS_IP`。
pub const DOH_ALIDNS_IP: &str = "223.5.5.5";
/// DNSPod IP-DoH 上游地址。**不变量**：同上。上游 `DOH_DNSPOD_IP`。
pub const DOH_DNSPOD_IP: &str = "1.12.12.12";

/// Tier1 抢跑上游上限（设计 §9.1：2 见顶、第 3 冗余；只数 Tier1，Tier2 不占额度）。
pub const MAX_TIER1_UPSTREAMS: usize = 3;

/// 竞速 on 的默认上游池。上游 `DEFAULT_POOL_IDS`。
pub const DEFAULT_POOL_IDS: &[&str] = &["ali", "dnspod"];
/// 竞速 off 的默认单上游 id。上游 `DEFAULT_SINGLE_ID`。
pub const DEFAULT_SINGLE_ID: &str = "ali";

/// 上游解析方式。上游 `ResolveUpstream.kind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamKind {
    /// DoH（https，POST application/dns-message）/ DoT（tls，见 [`ResolveUpstream::dot`]）。
    Doh,
    /// 明文 UDP:53。
    Udp,
    /// 系统解析器（无 IP，走 OS resolver）。
    System,
}

/// 一个解析上游 = 一种解析方式 + 其 Tier。上游 `ResolveUpstream`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveUpstream {
    /// `ali` / `dnspod` / `system` 或自定义 id。
    pub id: String,
    pub kind: UpstreamKind,
    /// 纯 IP（doh/udp 有；system 无）。
    pub ip: Option<String>,
    pub port: Option<u16>,
    /// 仅 DoH(https) 有。
    pub path: Option<String>,
    /// `kind == Doh` 下是否为 DoT(tls)。**当前恒 false**：DoT 二期，[`parse_custom_upstream`] 直接拒
    /// `tls://`（见该函数注释：接受但永远 FAIL 比拒绝更糟）。
    pub dot: bool,
    /// 1 = 抢跑（加密）；2 = 兜底（明文 UDP / system）。
    pub tier: u8,
}

/// 内置上游单一真值（id → 上游）。上游 `BUILTIN_UPSTREAMS`。
#[must_use]
pub fn builtin_upstream(id: &str) -> Option<ResolveUpstream> {
    match id {
        "ali" => Some(ResolveUpstream {
            id: "ali".into(),
            kind: UpstreamKind::Doh,
            ip: Some(DOH_ALIDNS_IP.into()),
            port: Some(443),
            path: Some("/dns-query".into()),
            dot: false,
            tier: 1,
        }),
        "dnspod" => Some(ResolveUpstream {
            id: "dnspod".into(),
            kind: UpstreamKind::Doh,
            ip: Some(DOH_DNSPOD_IP.into()),
            port: Some(443),
            path: Some("/dns-query".into()),
            dot: false,
            tier: 1,
        }),
        "system" => Some(ResolveUpstream {
            id: "system".into(),
            kind: UpstreamKind::System,
            ip: None,
            port: None,
            path: None,
            dot: false,
            tier: 2,
        }),
        _ => None,
    }
}

/// 自定义上游 spec → [`ResolveUpstream`]；**强制纯 IP**，非法 / 域名 / `tls://` → `None`。
/// 上游 `parseCustomUpstream`。
///
/// - `https://` → Tier1 加密抢跑；`udp://` / 裸 IP → Tier2 明文兜底。
/// - `tls://`（DoT）二期未实现：查询侧对 `dot` 直接 Err（永远 FAIL）。**此处拒绝**，避免 UI 接受
///   `tls://` 上游、用户以为生效却静默全 FAIL。待 DoT 落地后改回 `dot: type == Tls`。
#[must_use]
pub fn parse_custom_upstream(c: &CustomDnsUpstream) -> Option<ResolveUpstream> {
    if c.id.is_empty() || c.spec.is_empty() {
        return None;
    }
    let p = parse_dns_server_spec(Some(&c.spec))?;
    if p.is_domain || p.server.parse::<std::net::IpAddr>().is_err() {
        return None; // 纯 IP 强制
    }
    match p.server_type {
        DnsServerType::Udp => Some(ResolveUpstream {
            id: c.id.clone(),
            kind: UpstreamKind::Udp,
            ip: Some(p.server),
            port: Some(p.port),
            path: None,
            dot: false,
            tier: 2,
        }),
        DnsServerType::Tls => None, // DoT 二期，见函数文档
        DnsServerType::Https => Some(ResolveUpstream {
            id: c.id.clone(),
            kind: UpstreamKind::Doh,
            ip: Some(p.server),
            port: Some(p.port),
            path: Some(p.path.unwrap_or_else(|| "/dns-query".into())),
            dot: false,
            tier: 1,
        }),
    }
}

/// UI 校验：自定义 spec 是否合法（纯 IP DoH / UDP）。上游 `isValidCustomUpstreamSpec`。
#[must_use]
pub fn is_valid_custom_upstream_spec(spec: &str) -> bool {
    parse_custom_upstream(&CustomDnsUpstream {
        id: "_probe".into(),
        spec: spec.to_string(),
    })
    .is_some()
}

/// canonical 去重 key：`system` 唯一；其余按 `(kind, IP, port, path)`。
/// udp 与 doh 即便同 IP 也不同（协议/端口不同）。上游 `upstreamCanonicalKey`。
#[must_use]
pub fn upstream_canonical_key(u: &ResolveUpstream) -> String {
    if u.kind == UpstreamKind::System {
        return "system".into();
    }
    let kind = match u.kind {
        UpstreamKind::Doh => "doh",
        UpstreamKind::Udp => "udp",
        UpstreamKind::System => "system",
    };
    format!(
        "{kind}:{}:{}:{}",
        u.ip.as_deref().unwrap_or(""),
        u.port.map(|p| p.to_string()).unwrap_or_default(),
        u.path.as_deref().unwrap_or("")
    )
}

/// 分桶后的上游集。上游 `ResolvedUpstreams`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedUpstreams {
    /// 抢跑层（去重 + 上限 [`MAX_TIER1_UPSTREAMS`]）。
    pub tier1: Vec<ResolveUpstream>,
    /// 兜底层（不抢跑、不占额度）。
    pub tier2: Vec<ResolveUpstream>,
}

/// 池里一个 id → 上游（内置优先，回退同 id 的自定义项）。`None` = 无效 id / 自定义 spec 非纯 IP。
///
/// 抽成具名函数是为了让 [`plan_upstreams`] 的 INV-1 过滤与 [`resolve_upstreams`] 的分桶**用同一套
/// 解析口径**：过滤若自己按 id 字符串猜 kind（如 `id == "system"`），将来多一个产 `System` 的来源
/// 就会漏筛 —— 而漏筛的后果是 TUN 下的递归放大，不是少一个上游。
fn lookup_upstream(id: &str, custom: &[CustomDnsUpstream]) -> Option<ResolveUpstream> {
    builtin_upstream(id).or_else(|| {
        custom
            .iter()
            .find(|c| c.id == id)
            .and_then(parse_custom_upstream)
    })
}

/// 上游 id 列表 → Tier1/Tier2 分桶 + canonical 去重 + Tier1 上限。上游 `resolveUpstreams`。
///
/// Generation rejects unsupported explicit selections before reaching this planner.
/// This low-level projection never substitutes defaults for an unsupported selection.
/// Empty/default and supported Tier2-only selections retain the existing default
/// Tier1 policy. Any explicit unsupported entry produces no substituted plan.
#[must_use]
pub fn resolve_upstreams(ids: &[String], custom: &[CustomDnsUpstream]) -> ResolvedUpstreams {
    if ids.iter().any(|id| lookup_upstream(id, custom).is_none()) {
        return ResolvedUpstreams::default();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut tier1: Vec<ResolveUpstream> = Vec::new();
    let mut tier2: Vec<ResolveUpstream> = Vec::new();
    for id in ids {
        let Some(up) = lookup_upstream(id, custom) else {
            continue; // 无效 id / 自定义非纯 IP → 跳过
        };
        let key = upstream_canonical_key(&up);
        if seen.contains(&key) {
            continue; // 去重（内置与等价自定义合并）
        }
        seen.push(key);
        if up.tier == 1 {
            if tier1.len() < MAX_TIER1_UPSTREAMS {
                tier1.push(up); // 上限（去重后才数）
            }
        } else {
            tier2.push(up);
        }
    }
    if tier1.is_empty() && ids.iter().all(|id| lookup_upstream(id, custom).is_some()) {
        // 竞速至少要有一个抢跑上游；去重防与已选 Tier2 重复（system 在 Tier2，不会撞）。
        for id in DEFAULT_POOL_IDS {
            if let Some(up) = builtin_upstream(id) {
                let key = upstream_canonical_key(&up);
                if !seen.contains(&key) {
                    seen.push(key);
                    tier1.push(up);
                }
            }
        }
    }
    ResolvedUpstreams { tier1, tier2 }
}

/// Validate only the enabled selection at generation time, not hydration. Legacy
/// unsupported entries remain editable; they cannot silently choose other DNS.
pub fn validate_selected_upstreams(dns: Option<&DnsConfig>) -> Result<(), String> {
    let Some(dns) = dns else { return Ok(()); };
    if dns.resolve_node_domains_ahead == Some(false) { return Ok(()); }
    let custom = dns.node_resolver_custom.as_deref().unwrap_or(&[]);
    for id in dns.node_resolver_pool.as_deref().unwrap_or(&[]) {
        if lookup_upstream(id, custom).is_none() {
            // No user-controlled resolver ID or URL in the error.
            return Err("NODE_RESOLVER_UNSUPPORTED: Select an IP-based HTTPS or UDP node resolver in DNS settings".into());
        }
    }
    Ok(())
}

/// The single immutable planning entry point. Saved false keeps the legacy
/// explicit single resolver; omitted/true keeps the existing default pool.
/// Effective TUN excludes OS system resolution before tiering/cap/defaulting,
/// preventing system DNS from feeding back through platform DNS interception.
#[must_use]
pub fn plan_upstreams(
    dns: Option<&DnsConfig>,
    proxy_mode_type: ProxyModeType,
) -> Option<ResolvedUpstreams> {
    if dns.and_then(|d| d.resolve_node_domains_ahead) == Some(false) {
        return None;
    }
    let owned_default: Vec<String>;
    let mut ids: &[String] = match dns.and_then(|d| d.node_resolver_pool.as_deref()) {
        Some(p) => p,
        None => {
            owned_default = DEFAULT_POOL_IDS.iter().map(|s| (*s).to_string()).collect();
            &owned_default
        }
    };
    let custom: &[CustomDnsUpstream] = dns
        .and_then(|d| d.node_resolver_custom.as_deref())
        .unwrap_or(&[]);
    let tun_filtered: Vec<String>;
    if proxy_mode_type.is_tun() {
        // INV-1：TUN 接管期把 `system` 从池里摘除（见本函数文档）。
        tun_filtered = ids
            .iter()
            .filter(|id| {
                !matches!(
                    lookup_upstream(id, custom).map(|u| u.kind),
                    Some(UpstreamKind::System)
                )
            })
            .cloned()
            .collect();
        ids = &tun_filtered;
    }
    Some(resolve_upstreams(ids, custom))
}

#[cfg(test)]
mod tests;

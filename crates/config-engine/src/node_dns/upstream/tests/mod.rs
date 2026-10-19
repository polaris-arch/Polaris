use super::*;
use crate::user_config::dns_constants::is_bootstrap_direct_dns;

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn custom(id: &str, spec: &str) -> CustomDnsUpstream {
    CustomDnsUpstream {
        id: id.into(),
        spec: spec.into(),
    }
}

#[test]
fn builtin_doh_ips_are_direct_allowed_by_route() {
    // 不变量护栏：两个内置 DoH IP 必须 ∈ BOOTSTRAP_DIRECT_DNS_IPS，否则 TUN 下它们的 :443
    // 不被直连放行 → native policy 的 DoH 请求经 TUN 回到内核 → 内核又等 native policy 解析 = 死环。
    assert!(is_bootstrap_direct_dns(DOH_ALIDNS_IP));
    assert!(is_bootstrap_direct_dns(DOH_DNSPOD_IP));
}

#[test]
fn default_pool_is_two_tier1_doh() {
    let r = resolve_upstreams(&ids(DEFAULT_POOL_IDS), &[]);
    assert_eq!(r.tier1.len(), 2);
    assert!(r.tier2.is_empty());
    assert_eq!(
        r.tier1
            .iter()
            .chain(r.tier2.iter())
            .filter_map(|u| u.ip.clone())
            .collect::<Vec<_>>(),
        vec![DOH_ALIDNS_IP, DOH_DNSPOD_IP]
    );
}

#[test]
fn system_goes_to_tier2_and_does_not_consume_tier1_quota() {
    let r = resolve_upstreams(&ids(&["system", "ali", "dnspod"]), &[]);
    assert_eq!(r.tier1.len(), 2, "system 不占 Tier1 额度");
    assert_eq!(r.tier2.len(), 1);
    assert_eq!(r.tier2[0].kind, UpstreamKind::System);
    assert!(
        !r.tier1
            .iter()
            .chain(r.tier2.iter())
            .filter_map(|u| u.ip.as_ref())
            .any(|s| s.is_empty()),
        "system 无 IP，不进 candidate IPs"
    );
}

#[test]
fn tier1_capped_at_three_after_dedupe() {
    let cs = vec![
        custom("c1", "https://9.9.9.9/dns-query"),
        custom("c2", "https://8.8.8.8/dns-query"),
        custom("c3", "https://1.1.1.1/dns-query"),
    ];
    let r = resolve_upstreams(&ids(&["ali", "dnspod", "c1", "c2", "c3"]), &cs);
    assert_eq!(r.tier1.len(), MAX_TIER1_UPSTREAMS);
    assert_eq!(
        r.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["ali", "dnspod", "c1"],
        "按池顺序取前 3"
    );
}

#[test]
fn duplicate_of_builtin_is_merged_before_counting_cap() {
    // 自定义写成与内置等价（同 kind/IP/port/path）→ 去重合并，不占第二个额度。
    let cs = vec![
        custom("dup", "https://223.5.5.5/dns-query"),
        custom("c2", "https://8.8.8.8/dns-query"),
        custom("c3", "https://9.9.9.9/dns-query"),
    ];
    let r = resolve_upstreams(&ids(&["ali", "dup", "c2", "c3"]), &cs);
    assert_eq!(
        r.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["ali", "c2", "c3"],
        "dup 被合并，额度留给真正不同的上游"
    );
}

#[test]
fn empty_tier1_falls_back_to_defaults() {
    // Supported system-only selection retains the existing fallback; invalid IDs do not.
    let r = resolve_upstreams(&ids(&["system"]), &[]);
    assert_eq!(
        r.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["ali", "dnspod"]
    );
    assert_eq!(r.tier2.len(), 1);
    let r2 = resolve_upstreams(&ids(&["nope", "gone"]), &[]);
    assert!(r2.tier1.is_empty()); // Explicit unknown IDs must never select other resolvers.
}

#[test]
fn custom_domain_and_dot_specs_are_rejected() {
    assert!(parse_custom_upstream(&custom("d", "https://dns.google/dns-query")).is_none());
    assert!(parse_custom_upstream(&custom("d", "tls://1.1.1.1:853")).is_none());
    assert!(parse_custom_upstream(&custom("", "https://1.1.1.1/dns-query")).is_none());
    assert!(parse_custom_upstream(&custom("d", "")).is_none());
    assert!(!is_valid_custom_upstream_spec(
        "https://dns.google/dns-query"
    ));
    assert!(is_valid_custom_upstream_spec("https://1.1.1.1/dns-query"));
    assert!(
        is_valid_custom_upstream_spec("8.8.4.4"),
        "裸 IP → Tier2 udp"
    );
}

#[test]
fn bare_ip_custom_is_tier2_udp() {
    let u = parse_custom_upstream(&custom("x", "8.8.4.4")).expect("裸 IP 合法");
    assert_eq!(u.kind, UpstreamKind::Udp);
    assert_eq!(u.tier, 2);
    assert_eq!(u.port, Some(53));
}

/// 非 TUN（系统代理）—— 绝大多数既有断言的基线口径。
const NON_TUN: ProxyModeType = ProxyModeType::SystemProxy;

#[test]
fn plan_returns_none_when_race_disabled() {
    // 【不变式：竞速 off 不走池】关掉总开关 → None，即便池里有一堆上游。
    let dns = DnsConfig {
        resolve_node_domains_ahead: Some(false),
        node_resolver_pool: Some(ids(&["ali", "dnspod", "system"])),
        ..Default::default()
    };
    assert!(plan_upstreams(Some(&dns), NON_TUN).is_none());
    assert!(
        plan_upstreams(Some(&dns), ProxyModeType::Tun).is_none(),
        "总开关优先于 INV-1 过滤：竞速 off 一律不起 native policy"
    );
}

// ── INV-1：TUN 接管期 system 不得入竞速池 ────────────────────────────────

fn pool_with_system() -> DnsConfig {
    DnsConfig {
        node_resolver_pool: Some(ids(&["ali", "dnspod", "system"])),
        ..Default::default()
    }
}

/// 【INV-1 正向】TUN 下 `system` 必须被摘除；【反向】非 TUN 下仍可入池（Tier2 兜底）。
///
/// 两侧一起断言才是不变量：只测 TUN 会被「无论什么模式都摘掉 system」这种过度修法蒙混过关，
/// 而那会白白砍掉系统代理模式下唯一的本地兜底腿（那里没有 `hijack-dns`，不存在递归）。
///
/// **变异锁**：① 删掉 `plan_upstreams` 里的 `if proxy_mode_type.is_tun()` 过滤块 → TUN 断言转红；
/// ② 把过滤条件写成恒真（不判模式）→ 非 TUN 断言转红。
#[test]
fn plan_drops_system_under_tun_and_keeps_it_otherwise() {
    let dns = pool_with_system();
    let tun = plan_upstreams(Some(&dns), ProxyModeType::Tun).expect("竞速开");
    assert!(
        !tun.tier1
            .iter()
            .chain(tun.tier2.iter())
            .any(|u| u.kind == UpstreamKind::System),
        "TUN 接管期 system 必须离池（INV-1）：它没有 IP ⇒ 无法被 route 直连放行 ⇒ \
         OS resolver 的明文 :53 被 hijack-dns 抓回内核 → 又指回 native node DNS，逐级放大"
    );
    assert_eq!(
        tun.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["ali", "dnspod"],
        "摘 system 不得影响 Tier1"
    );

    let sysproxy = plan_upstreams(Some(&dns), NON_TUN).expect("竞速开");
    assert!(
        sysproxy
            .tier2
            .iter()
            .any(|u| u.kind == UpstreamKind::System),
        "非 TUN 无 hijack-dns、不存在自递归 → system 仍是合法的 Tier2 兜底"
    );
    assert_eq!(sysproxy.tier1.len(), 2);
    // `system` 无 IP ⇒ 两种模式的直连放行集逐字节相同（金样零变化的根据）。
    assert_eq!(tun.tier1, sysproxy.tier1);
}

/// 只勾了 `system` 的池 + TUN：摘完 Tier1 空 → 必须落进既有的默认池回退闸，而不是产出空计划。
///
/// 这正是「摘在 `resolve_upstreams` 之前」的理由 —— 若改成对结果切 Tier，这里会得到一个
/// **零上游**的 native policy：所有节点域名解析恒 SERVFAIL，且日志上看不出是被谁摘的。
///
/// **变异锁**：把过滤挪到 `resolve_upstreams` 之后（对 `tier2` retain）→ 本测仍绿但
/// `plan_drops_system_under_tun_and_keeps_it_otherwise` 不受影响；真正钉死的是下面这条
/// 「Tier1 非空」断言在「过滤 + 不回退」写法下会转红。
#[test]
fn plan_falls_back_to_defaults_when_tun_filter_empties_the_pool() {
    let dns = DnsConfig {
        node_resolver_pool: Some(ids(&["system"])),
        ..Default::default()
    };
    let r = plan_upstreams(Some(&dns), ProxyModeType::Tun).expect("竞速开");
    assert_eq!(
        r.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["ali", "dnspod"],
        "摘空后须回退默认池（否则 TUN + 只勾 system = 零上游，节点域名全 SERVFAIL）"
    );
    assert!(r.tier2.is_empty(), "system 已被 INV-1 摘除，Tier2 应为空");
}

/// 过滤按**解析出来的 kind** 走，不按 id 字符串：同 id 的自定义项遮不住内置 `system`，
/// 而合法的自定义 UDP 上游（IP-literal native transport）在 TUN 下必须留下。
///
/// **变异锁**：把过滤条件换成 `id == "system"` 的字符串比对 → 本测的「自定义 UDP 仍在」这条仍绿，
/// 但一旦将来多一个产 `System` kind 的来源即静默漏筛；把条件写成「摘掉所有 Tier2」→ 转红。
#[test]
fn tun_filter_keeps_custom_udp_tier2_upstreams() {
    let dns = DnsConfig {
        node_resolver_pool: Some(ids(&["ali", "system", "my-udp"])),
        node_resolver_custom: Some(vec![custom("my-udp", "8.8.4.4")]),
        ..Default::default()
    };
    let r = plan_upstreams(Some(&dns), ProxyModeType::Tun).expect("竞速开");
    assert_eq!(
        r.tier2.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["my-udp"],
        "自定义 UDP 使用 IP-literal native transport，不经过 OS resolver，仍保留"
    );
    assert!(
        r.tier1
            .iter()
            .chain(r.tier2.iter())
            .any(|u| u.ip.as_deref() == Some("8.8.4.4")),
        "保留 UDP 上游的原始 IP 端点"
    );
}

#[test]
fn plan_uses_pool_when_race_on_and_defaults_when_absent() {
    let on = DnsConfig {
        resolve_node_domains_ahead: Some(true),
        node_resolver_pool: Some(ids(&["dnspod", "system"])),
        ..Default::default()
    };
    let r = plan_upstreams(Some(&on), NON_TUN).expect("on → Some");
    assert_eq!(
        r.tier1.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        vec!["dnspod"]
    );
    assert_eq!(r.tier2.len(), 1);

    // 缺省（老配置无 resolveNodeDomainsAhead / 无 pool）→ 开 + 默认池。
    let r2 = plan_upstreams(Some(&DnsConfig::default()), NON_TUN).expect("缺省视为开");
    assert!(r2.tier1.is_empty()); // Explicit unknown IDs must never select other resolvers.
    let r3 = plan_upstreams(None, NON_TUN).expect("无 dnsConfig 也视为开");
    assert_eq!(r3.tier1.len(), 2);
}

#[test]
fn candidate_endpoints_preserve_nonstandard_ports_and_cap() {
    let cs = vec![
        custom("a", "https://8.8.8.8:8443/q"),
        custom("b", "https://1.1.1.1:8444/q"),
        custom("c", "https://9.9.9.9:9999/q"),
        custom("udp", "udp://9.9.9.9:5353"),
    ];
    let r = resolve_upstreams(&ids(&["ali", "a", "b", "c", "udp", "system"]), &cs);
    assert_eq!(
        r.tier1.iter().map(|u| u.port).collect::<Vec<_>>(),
        vec![Some(443), Some(8443), Some(8444)]
    );
    assert_eq!(r.tier2[0].port, Some(5353));
    assert_eq!(r.tier2[1].kind, UpstreamKind::System);
    assert!(r.tier2[1].ip.is_none() && r.tier2[1].port.is_none());
}

#[test]
fn plan_consumes_custom_pool_entry_end_to_end() {
    // 配置字段消费面闭环：nodeResolverCustom 的 spec 真的变成 Tier1 上游 + candidate IPs。
    let dns = DnsConfig {
        node_resolver_pool: Some(ids(&["ali", "my-doh"])),
        node_resolver_custom: Some(vec![custom("my-doh", "https://9.9.9.9:8443/q")]),
        ..Default::default()
    };
    let r = plan_upstreams(Some(&dns), NON_TUN).expect("on");
    let mine = r
        .tier1
        .iter()
        .find(|u| u.id == "my-doh")
        .expect("自定义生效");
    assert_eq!(mine.ip.as_deref(), Some("9.9.9.9"));
    assert_eq!(mine.port, Some(8443));
    assert_eq!(mine.path.as_deref(), Some("/q"));
    assert!(
        r.tier1
            .iter()
            .chain(r.tier2.iter())
            .any(|u| u.ip.as_deref() == Some("9.9.9.9")),
        "自定义 IP 须进直连放行"
    );
}

#[test]
fn explicit_unsupported_selection_fails_generation_validation_without_hydration_changes() {
    let mut dns = DnsConfig {
        node_resolver_custom: Some(vec![custom("tls", "tls://1.1.1.1")]),
        node_resolver_pool: Some(ids(&["tls"])),
        ..Default::default()
    };
    assert!(validate_selected_upstreams(Some(&dns))
        .unwrap_err()
        .starts_with("NODE_RESOLVER_UNSUPPORTED:"));
    assert!(resolve_upstreams(
        dns.node_resolver_pool.as_ref().unwrap(),
        dns.node_resolver_custom.as_ref().unwrap()
    )
    .tier1
    .is_empty());
    dns.node_resolver_pool = Some(ids(&["ali"]));
    assert!(validate_selected_upstreams(Some(&dns)).is_ok()); // Legacy unselected entry remains editable.
    dns.node_resolver_pool = Some(ids(&["tls"]));
    dns.resolve_node_domains_ahead = Some(false);
    assert!(validate_selected_upstreams(Some(&dns)).is_ok());
    assert!(validate_selected_upstreams(None).is_ok());
    assert!(validate_selected_upstreams(Some(&DnsConfig::default())).is_ok());
    assert!(!is_valid_custom_upstream_spec("https://[:::]/dns-query"));
}

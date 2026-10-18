use super::*;
use crate::user_config::app_config::UserConfig;
use crate::user_config::dns_config::DnsConfig as UserDnsConfig;
use crate::user_config::proxy_mode::{ProxyMode, ProxyModeType};
use crate::user_config::region_routing::RegionRoutingConfig;
use crate::user_config::rule::{
    CombineMode, Rule, RuleAction, RuleCondition, RuleDnsAnswerMode, RuleDnsEffect,
    RuleDnsResolver, RuleEffects, RuleType,
};
use crate::user_config::server_config::{Protocol, ServerConfig, TailscaleSettings};
use crate::user_config::tun_config::TunModeConfig;
use crate::user_config::DnsServerEndpoint;
use std::collections::BTreeMap;

/// 构造最小 deps（Linux 平台、无 endpoint、固定假路径、FS 全 false = 所有 .srs 缺失）。
fn deps_false() -> DnsConfigDeps {
    DnsConfigDeps {
        lan_resolver_for_dns: None,
        pending_endpoints: vec![],
        log: |_, _| {},
        selected_server_tag: "proxy-selector".into(),
        node_dns_decoy_override: None,
        probe_pool_ports: vec![],
        probe_proxy_port: None,
        platform: "linux".into(),
        custom_rules_dir: "/fake/custom-rules/".into(),
        runtime_rules_dir: "/fake/runtime-rules/".into(),
        rule_resources_path: "/fake/rule-resources/".into(),
        is_valid_srs_fn: |_| false,
        exists_fn: |_| false,
        system_dns_takeover_active: false,
        netenv_dhcp_suppressed: false,
    }
}

/// 构造最小 UserConfig（smart + systemProxy + 单节点）。
fn base_config() -> UserConfig {
    UserConfig {
        servers: vec![ServerConfig {
            id: "s1".into(),
            name: "HK".into(),
            protocol: Protocol::Vless,
            address: "hk.example.com".into(),
            port: 443,
            ..Default::default()
        }],
        selected_server_id: Some("s1".into()),
        proxy_mode: ProxyMode::Smart,
        proxy_mode_type: ProxyModeType::SystemProxy,
        ..Default::default()
    }
}

fn dns_effect_rule(
    id: &str,
    type_field: RuleType,
    values: &[&str],
    resolver: RuleDnsResolver,
    answer_mode: RuleDnsAnswerMode,
) -> Rule {
    Rule {
        id: id.into(),
        type_field,
        values: values.iter().map(|value| value.to_string()).collect(),
        conditions: None,
        combine_mode: None,
        effects: Some(RuleEffects {
            route: None,
            dns: Some(RuleDnsEffect {
                enabled: true,
                action: None,
                migrated_implicit_resolve: false,
                resolver,
                answer_mode,
            }),
        }),
        action: RuleAction::Direct,
        enabled: true,
        bypass_fakeip: None,
        target_server_id: None,
        remarks: None,
        tls_spoof: None,
        tls_spoof_method: None,
        network_profile_id: None,
    }
}

/// 收集 server tag 列表。
fn server_tags(c: &DnsConfig) -> Vec<String> {
    c.servers.iter().map(|s| s.tag.clone()).collect()
}

#[test]
fn always_present_bootstrap_servers() {
    // 无论配置如何，5 个基础 server 恒在：bootstrap/node/local/domestic/remote。
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    let tags = server_tags(&c);
    assert!(tags.contains(&"dns-bootstrap".into()));
    assert!(tags.contains(&"dns-node".into()));
    assert!(tags.contains(&"dns-local".into()));
    assert!(tags.contains(&"dns-domestic".into()));
    assert!(tags.contains(&"dns-remote".into()));
    // dns-remote detour = selected_server_tag。
    let remote = c.servers.iter().find(|s| s.tag == "dns-remote").unwrap();
    assert_eq!(remote.detour.as_deref(), Some("proxy-selector"));
}

#[test]
fn native_dns_effect_is_active_in_smart_global_and_direct_modes() {
    for mode in [ProxyMode::Smart, ProxyMode::Global, ProxyMode::Direct] {
        let mut config = base_config();
        config.proxy_mode = mode;
        config.dns_config = Some(UserDnsConfig {
            enable_fake_ip: Some(false),
            ..Default::default()
        });
        config.custom_rules = vec![dns_effect_rule(
            "dns-all-modes",
            RuleType::DomainSuffix,
            &["example.com"],
            RuleDnsResolver::Proxy,
            RuleDnsAnswerMode::Real,
        )];
        let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
        let effect = built
            .rules
            .as_ref()
            .unwrap()
            .iter()
            .find(|rule| {
                rule.domain_suffix
                    .as_ref()
                    .is_some_and(|suffixes| suffixes.contains(&".example.com".to_string()))
            })
            .expect("统一 DNS 规则应在所有代理模式生成");
        assert_eq!(effect.server.as_deref(), Some("dns-remote"));
    }
}

#[test]
fn native_dns_and_conditions_share_one_rule_and_fakeip_is_native() {
    let mut config = base_config();
    config.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    let mut rule = dns_effect_rule(
        "dns-and",
        RuleType::Domain,
        &["api.example.com"],
        RuleDnsResolver::Direct,
        RuleDnsAnswerMode::FakeIp,
    );
    rule.conditions = Some(vec![
        RuleCondition {
            type_field: RuleType::Domain,
            values: vec!["api.example.com".into()],
        },
        RuleCondition {
            type_field: RuleType::DomainRegex,
            values: vec!["^api\\.".into()],
        },
    ]);
    rule.combine_mode = Some(CombineMode::And);
    config.custom_rules = vec![rule];

    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    let effect_rules: Vec<_> = built
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|rule| rule.type_field.as_deref() == Some("logical"))
        .collect();
    assert_eq!(effect_rules.len(), 1);
    let actual = serde_json::to_value(effect_rules[0]).unwrap();
    // Fixed core 577d4b9a: destinationAddressItems uses Any; logical AND requires every
    // child (rule_abstract.go:126-129,204-221). api.example.com satisfies both children;
    // api.other.com satisfies only the regex and therefore must not select FakeIP.
    assert_eq!(
        actual,
        serde_json::json!({
            "type": "logical", "mode": "and",
            "rules": [
                {"domain": ["api.example.com"]},
                {"domain_regex": ["^api\\."]},
                {"query_type": ["A", "AAAA"]}
            ],
            "action": "route", "server": "fakeip"
        })
    );
    let restored: DnsRule = serde_json::from_value(actual).unwrap();
    assert_eq!(&restored, effect_rules[0]);
}

#[test]
fn v2_hosts_first_emits_preferred_hosts_then_explicit_fallback_before_fakeip() {
    let mut config = base_config();
    config.config_schema_version = Some(2);
    config.dns_servers = vec![DnsServerResource {
        id: "hosts-corp".into(),
        name: "Corp hosts".into(),
        enabled: true,
        kind: DnsServerKind::Hosts,
        paths: vec!["/etc/hosts".into(), "/opt/corp.hosts".into()],
        predefined: BTreeMap::from([("git.corp.example".into(), vec!["10.0.0.8".into()])]),
        ..Default::default()
    }];
    config.dns_defaults = Some(DnsPolicyDefaults {
        direct_server_id: "builtin-domestic".into(),
        proxy_server_id: "builtin-remote".into(),
        unmatched_action: Some(DnsPolicyAction::FakeIp),
        connection_resolution: Default::default(),
        real_query_fallback_action: None,
    });
    let mut rule = dns_effect_rule(
        "hosts-first",
        RuleType::DomainSuffix,
        &["corp.example"],
        RuleDnsResolver::Direct,
        RuleDnsAnswerMode::Real,
    );
    rule.effects.as_mut().unwrap().dns.as_mut().unwrap().action =
        Some(DnsPolicyAction::HostsFirst {
            hosts_server_id: "hosts-corp".into(),
            fallback: Box::new(DnsPolicyAction::Server {
                server_id: "builtin-remote".into(),
            }),
        });
    config.policy_rules = Some(vec![rule]);

    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    let hosts = built
        .servers
        .iter()
        .find(|server| server.tag == "dns-user-hosts-corp")
        .expect("hosts resource emitted");
    assert_eq!(
        hosts.path,
        Some(OneOrMany::Many(vec![
            "/etc/hosts".into(),
            "/opt/corp.hosts".into()
        ]))
    );
    assert!(hosts
        .predefined
        .as_ref()
        .is_some_and(|records| records.contains_key("git.corp.example")));

    let rules = built.rules.as_ref().unwrap();
    let preferred_index = rules
        .iter()
        .position(|rule| {
            rule.preferred_by
                .as_ref()
                .is_some_and(|tags| tags.contains(&"dns-user-hosts-corp".to_string()))
        })
        .expect("hosts preferred_by rule");
    let fallback_index = rules
        .iter()
        .enumerate()
        .skip(preferred_index + 1)
        .find(|(_, rule)| {
            rule.domain_suffix
                .as_ref()
                .is_some_and(|suffixes| suffixes.contains(&".corp.example".to_string()))
                && rule.server.as_deref() == Some("dns-remote")
        })
        .map(|(index, _)| index)
        .expect("explicit real-IP fallback");
    let fake_index = rules
        .iter()
        .position(|rule| rule.server.as_deref() == Some("fakeip"))
        .expect("default fakeip rule");
    assert!(preferred_index < fallback_index && fallback_index < fake_index);
}

#[test]
fn v2_race_group_uses_native_evaluate_respond_and_node_egress_is_preserved() {
    let mut config = base_config();
    config.config_schema_version = Some(2);
    config.dns_servers = vec![DnsServerResource {
        id: "node-dns".into(),
        name: "Node DNS".into(),
        enabled: true,
        kind: DnsServerKind::Udp,
        endpoint: Some(DnsServerEndpoint {
            host: "9.9.9.9".into(),
            port: Some(53),
            path: None,
        }),
        outbound: DnsServerOutbound::Node {
            node_id: "s1".into(),
        },
        ..Default::default()
    }];
    config.dns_server_groups = vec![DnsServerGroup {
        id: "fastest".into(),
        name: "Fastest".into(),
        enabled: true,
        mode: DnsServerGroupMode::Race,
        members: vec!["builtin-domestic".into(), "node-dns".into()],
        fallback_server_id: Some("builtin-remote".into()),
    }];
    config.dns_defaults = Some(DnsPolicyDefaults {
        direct_server_id: "builtin-domestic".into(),
        proxy_server_id: "builtin-remote".into(),
        unmatched_action: Some(DnsPolicyAction::Server {
            server_id: "builtin-domestic".into(),
        }),
        connection_resolution: Default::default(),
        real_query_fallback_action: None,
    });
    let mut rule = dns_effect_rule(
        "race",
        RuleType::Domain,
        &["race.example"],
        RuleDnsResolver::Direct,
        RuleDnsAnswerMode::Real,
    );
    rule.effects.as_mut().unwrap().dns.as_mut().unwrap().action = Some(DnsPolicyAction::Group {
        group_id: "fastest".into(),
    });
    config.policy_rules = Some(vec![rule]);
    let id_map = BTreeMap::from([("s1".into(), "node-hk".into())]);

    let built = build_dns_config(&config, &id_map, &deps_false());
    assert_eq!(
        built
            .servers
            .iter()
            .find(|server| server.tag == "dns-user-node-dns")
            .and_then(|server| server.detour.as_deref()),
        Some("node-hk")
    );
    let rules = built.rules.as_ref().unwrap();
    let evaluates: Vec<_> = rules
        .iter()
        .filter(|rule| rule.resolver_policy.is_none() && rule.action.as_deref() == Some("evaluate"))
        .collect();
    let responds: Vec<_> = rules
        .iter()
        .filter(|rule| rule.resolver_policy.is_none() && rule.action.as_deref() == Some("respond"))
        .collect();
    assert_eq!(evaluates.len(), 2);
    assert_eq!(evaluates[0].speculative, None);
    assert_eq!(evaluates[1].speculative, Some(true));
    assert_eq!(responds.len(), 2);
    assert!(responds.iter().all(|rule| rule.race == Some(true)));
    assert!(rules.iter().any(|rule| {
        rule.domain
            .as_ref()
            .is_some_and(|domains| domains.contains(&"race.example".to_string()))
            && rule.server.as_deref() == Some("dns-remote")
    }));
}

#[test]
fn fakeip_off_adds_reverse_mapping() {
    // 关 FakeIP → reverse_mapping=true；无 fakeip server。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.reverse_mapping, Some(true));
    assert!(!server_tags(&c).contains(&"fakeip".into()));
}

#[test]
fn only_the_fakeip_catch_all_rewrites_ttl() {
    // FakeIP 合成应答必须带 rewrite_ttl（压错配窗口，理由见 FAKEIP_REWRITE_TTL）；
    // 其余 DNS 规则一条都不许带 —— 它们的应答来自真实上游，改写 TTL 是纯粹的越权。
    //
    // 判据故意写成「按 server 分区的全量对账」而不是「找到那条断言它有」：后者对
    // 「rewrite_ttl 被顺手撒到别的规则上」这类回归是瞎的。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let rules = c.rules.as_ref().expect("开 FakeIP 必有 DNS 规则");
    let with_ttl: Vec<_> = rules.iter().filter(|r| r.rewrite_ttl.is_some()).collect();
    assert_eq!(
        with_ttl.len(),
        1,
        "带 rewrite_ttl 的规则应恰为 1 条，实际 {}",
        with_ttl.len()
    );
    assert_eq!(with_ttl[0].server.as_deref(), Some("fakeip"));
    assert_eq!(with_ttl[0].rewrite_ttl, Some(FAKEIP_REWRITE_TTL));
    // 反向：关 FakeIP 的世界里一条都不该有。
    let mut off = base_config();
    off.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    let c_off = build_dns_config(&off, &BTreeMap::new(), &deps_false());
    assert!(
        c_off
            .rules
            .as_ref()
            .is_none_or(|rs| rs.iter().all(|r| r.rewrite_ttl.is_none())),
        "关 FakeIP 时不该有任何 rewrite_ttl"
    );
}

#[test]
fn fakeip_on_default_adds_fakeip_server_v4_only() {
    // 开 FakeIP（缺省 true）→ fakeip server 仅 inet4_range（关 IPv6）。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let fakeip = c
        .servers
        .iter()
        .find(|s| s.tag == "fakeip")
        .expect("fakeip server present");
    assert_eq!(fakeip.inet4_range.as_deref(), Some("198.18.0.0/15"));
    assert!(fakeip.inet6_range.is_none(), "关 IPv6 不分配 v6 段");
    // 开 FakeIP → 不加 reverse_mapping。
    assert_eq!(c.reverse_mapping, None);
}

#[test]
fn fakeip_on_with_ipv6_adds_v6_range() {
    let mut cfg = base_config();
    cfg.enable_ipv6 = Some(true);
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let fakeip = c.servers.iter().find(|s| s.tag == "fakeip").unwrap();
    assert_eq!(fakeip.inet6_range.as_deref(), Some("2001:2::/48"));
}

#[test]
fn strategy_follows_ipv6() {
    // 开 IPv6 → prefer_ipv4；关 → ipv4_only。
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    assert_eq!(c.strategy.as_deref(), Some("ipv4_only"));
    let mut cfg = base_config();
    cfg.enable_ipv6 = Some(true);
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.strategy.as_deref(), Some("prefer_ipv4"));
}

#[test]
fn lan_resolver_adds_dns_lan_udp() {
    // 注入地址必须真正成为 UDP 上游，而不是丢弃地址后改用 DHCP。
    let mut deps = deps_false();
    deps.lan_resolver_for_dns = Some("192.168.1.1".into());
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps);
    let lan = c.servers.iter().find(|s| s.tag == "dns-lan");
    assert!(lan.is_some(), "dns-lan server present");
    assert_eq!(lan.unwrap().type_field.as_deref(), Some("udp"));
    assert_eq!(lan.unwrap().server.as_deref(), Some("192.168.1.1"));
}

#[test]
fn node_domain_rule1_emits_when_server_has_domain() {
    // 节点 address=域名 → rule1 含 domain + domain_suffix（exact + .suffix）。
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    let rule1 = c.rules.as_ref().unwrap().iter().find(|r| {
        r.domain
            .as_ref()
            .map(|d| d.contains(&"hk.example.com".to_string()))
            .unwrap_or(false)
    });
    assert!(rule1.is_some(), "rule1 node-domain present");
    let r = rule1.unwrap();
    let suffix = r.domain_suffix.as_ref().unwrap();
    assert!(suffix.contains(&"hk.example.com".into()));
    assert!(suffix.contains(&".hk.example.com".into()));
}

#[test]
fn node_domain_rule1_skips_ip_literals() {
    // address=IPv4 → 不进 rule1。
    let mut cfg = base_config();
    cfg.servers[0].address = "1.2.3.4".into();
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let has_node_rule = c.rules.as_ref().unwrap().iter().any(|r| {
        r.domain
            .as_ref()
            .map(|d| d.contains(&"1.2.3.4".to_string()))
            .unwrap_or(false)
    });
    assert!(!has_node_rule, "IP 字面量不进 rule1");
}

#[test]
fn bootstrap_rule_includes_only_explicitly_referenced_domains() {
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    let bootstrap_rule = c
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .find(|r| r.server.as_deref() == Some("dns-bootstrap"))
        .unwrap();
    let domains = bootstrap_rule.domain.as_ref().unwrap();
    assert!(domains.contains(&"doh.pub".into()));
    assert!(domains.contains(&"dns.google".into()));
    assert!(!domains.contains(&"cloudflare-dns.com".into()));
    assert!(!domains.contains(&"one.one.one.one".into()));
}

#[test]
fn lan_dns_is_private_or_fails_closed_on_every_platform() {
    for platform in ["win32", "darwin", "linux", "android", "ios"] {
        for mode in [ProxyModeType::Tun, ProxyModeType::SystemProxy] {
            for candidate in [
                None,
                Some("192.168.1.1"),
                Some("fd00::53"),
                Some("100.100.100.100"),
                Some("8.8.8.8"),
                Some("127.0.0.1"),
                Some("::1"),
                Some("2001:4860:4860::8888"),
                Some("bad"),
            ] {
                let mut cfg = base_config();
                cfg.proxy_mode_type = mode;
                cfg.servers[0].address = "proxy.lan".into();
                cfg.custom_rules = vec![dns_effect_rule(
                    "override",
                    RuleType::DomainSuffix,
                    &["lan"],
                    RuleDnsResolver::Proxy,
                    RuleDnsAnswerMode::Real,
                )];
                let mut deps = deps_false();
                deps.platform = platform.into();
                deps.lan_resolver_for_dns = candidate.map(str::to_string);
                deps.probe_proxy_port = Some(12345);
                deps.probe_pool_ports = vec![12346];
                let built = build_dns_config(&cfg, &BTreeMap::new(), &deps);
                let rules = built.rules.as_ref().unwrap();
                assert_eq!(rules[0].server.as_deref(), Some("dns-mdns"));
                assert!(rules[0]
                    .domain_suffix
                    .as_ref()
                    .unwrap()
                    .contains(&"local".into()));
                let lan = &rules[1];
                for suffix in ["lan", "arpa", "home.arpa", "internal", "home"] {
                    assert!(lan.domain_suffix.as_ref().unwrap().contains(&suffix.into()));
                }
                assert_eq!(lan.domain_regex, Some(vec![r"^[^.]+\.?$".into()]));
                // 独立预期，不调用生产谓词自证。
                let private = matches!(
                    candidate,
                    Some("192.168.1.1" | "fd00::53" | "100.100.100.100")
                );
                if private {
                    assert_eq!(lan.server.as_deref(), Some("dns-lan"));
                    assert!(lan.action.is_none());
                    let upstream = built.servers.iter().find(|s| s.tag == "dns-lan").unwrap();
                    assert_eq!(upstream.type_field.as_deref(), Some("udp"));
                    assert_eq!(upstream.server.as_deref(), candidate);
                } else {
                    assert!(lan.server.is_none());
                    assert_eq!(lan.action.as_deref(), Some("predefined"));
                    assert_eq!(lan.rcode.as_deref(), Some("SERVFAIL"));
                    assert!(!built.servers.iter().any(|s| s.tag == "dns-lan"));
                }
                let mdns = built.servers.iter().find(|s| s.tag == "dns-mdns").unwrap();
                assert_eq!(mdns.type_field.as_deref(), Some("mdns"));
                assert!(mdns.server.is_none());
                assert!(mdns.detour.is_none());
                let bank = rules
                    .iter()
                    .find(|r| {
                        r.domain_suffix
                            .as_ref()
                            .is_some_and(|s| s.contains(&".microdone.cn".into()))
                    })
                    .unwrap();
                assert_eq!(
                    bank.server.as_deref(),
                    Some(if platform == "win32" && mode == ProxyModeType::Tun {
                        "dns-domestic"
                    } else {
                        "dns-local"
                    })
                );
            }
        }
    }
}

#[test]
fn fakeip_default_filter_rules() {
    // 开 FakeIP + 未编辑 filterList → captive + ntp/keyword 两条规则。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let rules = c.rules.as_ref().unwrap();
    // captive 规则（domain → internalResolverTag=dns-local，因非 Win 无 lan）。
    let captive = rules.iter().find(|r| {
        r.domain
            .as_ref()
            .map(|d| d.contains(&"captive.apple.com".to_string()))
            .unwrap_or(false)
    });
    assert!(captive.is_some(), "captive filter rule present");
    assert_eq!(captive.unwrap().server.as_deref(), Some("dns-local"));
    // ntp/keyword 规则（domain_suffix + domain_keyword → dns-domestic）。
    let ntp = rules.iter().find(|r| {
        r.domain_keyword
            .as_ref()
            .map(|k| k.contains(&"ntp".to_string()))
            .unwrap_or(false)
    });
    assert!(ntp.is_some(), "ntp/stun keyword rule present");
    let ntp_r = ntp.unwrap();
    assert_eq!(ntp_r.server.as_deref(), Some("dns-domestic"));
    // ntp suffix 含 [ntp.org, .ntp.org] 形态。
    let suf = ntp_r.domain_suffix.as_ref().unwrap();
    assert!(suf.contains(&"ntp.org".into()));
    assert!(suf.contains(&".ntp.org".into()));
}

#[test]
fn fakeip_filter_disabled_no_filter_rules() {
    // fakeIpFilter=false → 完全不生成 captive/ntp filter。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    cfg.fake_ip_filter = Some(false);
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let has_captive = c.rules.as_ref().unwrap().iter().any(|r| {
        r.domain
            .as_ref()
            .map(|d| d.contains(&"captive.apple.com".to_string()))
            .unwrap_or(false)
    });
    assert!(!has_captive, "fakeIpFilter=false 关闭 filter");
}

#[test]
fn fakeip_edited_filter_list_splits_captive_and_others() {
    // 用户编辑过 filterList：内置 captive 仍走 internalResolver；其余走 dns-domestic。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(true),
        ..Default::default()
    });
    cfg.fake_ip_filter_list = Some(vec![
        "captive.apple.com".into(),  // 内置 captive
        "custom.example.com".into(), // 其它
    ]);
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let rules = c.rules.as_ref().unwrap();
    // captive → dns-local（internalResolver）。
    let captive = rules.iter().find(|r| {
        r.domain
            .as_ref()
            .map(|d| d.contains(&"captive.apple.com".to_string()))
            .unwrap_or(false)
    });
    assert_eq!(captive.unwrap().server.as_deref(), Some("dns-local"));
    // others → dns-domestic（domain_suffix 含 [custom.example.com, .custom.example.com]）。
    let others = rules.iter().find(|r| {
        r.domain_suffix
            .as_ref()
            .map(|d| d.contains(&"custom.example.com".to_string()))
            .unwrap_or(false)
    });
    assert!(others.is_some(), "others suffix rule present");
    // ntp/stun keyword 始终兜底。
    let has_keyword = rules.iter().any(|r| {
        r.domain_keyword
            .as_ref()
            .map(|k| k.contains(&"stun".to_string()))
            .unwrap_or(false)
    });
    assert!(has_keyword, "ntp/stun keyword 始终保留");
}

#[test]
fn smart_non_fakeip_global_mode_dns_remote_query_type() {
    // global + 非 FakeIP → query_type A/AAAA → dns-remote。
    let mut cfg = base_config();
    cfg.proxy_mode = ProxyMode::Global;
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let remote_qt = c
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .find(|r| r.query_type.is_some() && r.server.as_deref() == Some("dns-remote"));
    assert!(
        remote_qt.is_some(),
        "global non-fakeip → dns-remote query_type"
    );
}

#[test]
fn smart_non_fakeip_final_stays_domestic_forward() {
    // smart 非-FakeIP + 正向 region（默认 cn reverse=false）→ final 保持 dns-domestic。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.final_server.as_deref(), Some("dns-domestic"));
}

#[test]
fn smart_non_fakeip_reverse_flips_final_to_remote() {
    // smart 非-FakeIP + reverse=true → final 翻为 dns-remote。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    cfg.region_routing = Some(RegionRoutingConfig {
        enabled: true,
        region: "cn".into(),
        reverse: true,
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.final_server.as_deref(), Some("dns-remote"));
}

#[test]
fn optimistic_cache_and_timeout_emitted() {
    // optimisticCache=true → optimistic=true；dnsTimeoutMs>0 → timeout="<n>ms"。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        optimistic_cache: Some(true),
        dns_timeout_ms: Some(5000.0),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.optimistic, Some(true));
    assert_eq!(c.timeout.as_deref(), Some("5000ms"));
}

#[test]
fn timeout_zero_or_invalid_omitted() {
    // dnsTimeoutMs=0 / NaN / 负 → 不下发 timeout。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        dns_timeout_ms: Some(0.0),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.timeout, None);
    // NaN
    cfg.dns_config.as_mut().unwrap().dns_timeout_ms = Some(f64::NAN);
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.timeout, None);
}

#[test]
fn timeout_rounds_to_int_ms() {
    // 4999.6 → "5000ms"（round）。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        dns_timeout_ms: Some(4999.6),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    assert_eq!(c.timeout.as_deref(), Some("5000ms"));
}

#[test]
fn native_candidate_servers_exist_without_runtime_port() {
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    let candidate = c
        .servers
        .iter()
        .find(|s| s.tag == "dns-node-native-1-0")
        .unwrap();
    assert_eq!(candidate.type_field.as_deref(), Some("https"));
    assert_eq!(candidate.server.as_deref(), Some("223.5.5.5"));
    assert!(candidate.detour.is_none());
}

#[test]
fn race_server_skipped_when_resolve_disabled() {
    // 保存配置显式关闭原生节点策略。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        resolve_node_domains_ahead: Some(false),
        ..Default::default()
    });
    let deps = deps_false();
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps);
    assert!(c
        .servers
        .iter()
        .all(|s| !s.tag.starts_with("dns-node-native-")));
}

#[test]
fn probe_pool_emits_servers_and_leading_rules() {
    // probePoolPorts=[1,2] → 2 个 dns-probe-exit-{0,1} server + 2 条 inbound probe-in 规则置顶。
    let mut deps = deps_false();
    deps.probe_pool_ports = vec![5354, 5355];
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps);
    let tags = server_tags(&c);
    assert!(tags.contains(&"dns-probe-exit-0".into()));
    assert!(tags.contains(&"dns-probe-exit-1".into()));
    // 规则置顶：LAN 保护之后两条是 probe-in-{0,1}。
    let rules = c.rules.as_ref().unwrap();
    assert_eq!(
        rules[2].inbound,
        Some(OneOrMany::Many(vec!["probe-in-0".into()]))
    );
    assert_eq!(
        rules[3].inbound,
        Some(OneOrMany::Many(vec!["probe-in-1".into()]))
    );
    assert_eq!(rules[2].disable_cache, Some(true));
}

#[test]
fn probe_proxy_emits_server_and_leading_rule() {
    // probeProxyPort>0 → dns-probe-exit-proxy server + probe-proxy-in 规则置顶。
    let mut deps = deps_false();
    deps.probe_proxy_port = Some(5356);
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps);
    let proxy_srv = c.servers.iter().find(|s| s.tag == "dns-probe-exit-proxy");
    assert!(proxy_srv.is_some());
    assert_eq!(proxy_srv.unwrap().detour.as_deref(), Some("proxy-selector"));
    // 规则[2] = probe-proxy-in。
    assert_eq!(
        c.rules.as_ref().unwrap()[2].inbound,
        Some(OneOrMany::Many(vec!["probe-proxy-in".into()]))
    );
}

#[test]
fn neighbor_domains_attached_to_dns_local_on_linux() {
    // Linux + neighborDomains → dns-local.neighbor_domain 归一化（.lan）。
    let mut cfg = base_config();
    cfg.tun_config = Some(TunModeConfig {
        neighbor_domains: Some(vec!["lan".into(), "home.arpa".into()]),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let local = c.servers.iter().find(|s| s.tag == "dns-local").unwrap();
    let nd = local.neighbor_domain.as_ref().unwrap();
    assert!(nd.contains(&".lan".into()));
    assert!(nd.contains(&".home.arpa".into()));
}

#[test]
fn neighbor_domains_skipped_on_win32() {
    // win32 不支持 source device match → 不附 neighbor_domain。
    let mut cfg = base_config();
    cfg.tun_config = Some(TunModeConfig {
        neighbor_domains: Some(vec!["lan".into()]),
        ..Default::default()
    });
    let mut deps = deps_false();
    deps.platform = "win32".into();
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps);
    let local = c.servers.iter().find(|s| s.tag == "dns-local").unwrap();
    assert!(local.neighbor_domain.is_none());
}

#[test]
fn tailscale_resolve_by_name_emits_server_and_preferred_by_rule() {
    // resolveByName=true + endpoint 已发射 → dns-tailscale server + preferred_by 规则。
    let mut cfg = base_config();
    cfg.servers.push(ServerConfig {
        id: "ts1".into(),
        name: "TS".into(),
        protocol: Protocol::Tailscale,
        address: "".into(),
        port: 0,
        tailscale_settings: Some(Box::new(TailscaleSettings {
            resolve_by_name: Some(true),
            accept_default_resolvers: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    });
    let mut id_map = BTreeMap::new();
    id_map.insert("ts1".into(), "TS".into());
    let mut deps = deps_false();
    deps.pending_endpoints = vec![Endpoint {
        type_field: "tailscale".into(),
        tag: "TS".into(),
        ..Default::default()
    }];
    let c = build_dns_config(&cfg, &id_map, &deps);
    // dns-tailscale server。
    let ts_srv = c.servers.iter().find(|s| s.tag == TS_NAME_DNS_TAG);
    assert!(ts_srv.is_some());
    assert_eq!(ts_srv.unwrap().endpoint.as_deref(), Some("TS"));
    assert_eq!(ts_srv.unwrap().accept_default_resolvers, Some(true));
    // preferred_by 规则（命中 preferred_by）。
    let preferred = c.rules.as_ref().unwrap().iter().find(|r| {
        r.preferred_by
            .as_ref()
            .map(|p| p.contains(&TS_NAME_DNS_TAG.to_string()))
            .unwrap_or(false)
    });
    assert!(preferred.is_some(), "preferred_by rule present");
}

#[test]
fn tailscale_resolve_skipped_when_endpoint_not_emitted() {
    // resolveByName=true 但 endpoint 未在 pendingEndpoints → 不生成。
    let mut cfg = base_config();
    cfg.servers.push(ServerConfig {
        id: "ts1".into(),
        name: "TS".into(),
        protocol: Protocol::Tailscale,
        address: "".into(),
        port: 0,
        tailscale_settings: Some(Box::new(TailscaleSettings {
            resolve_by_name: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    });
    let mut id_map = BTreeMap::new();
    id_map.insert("ts1".into(), "TS".into());
    let deps = deps_false(); // 无 pendingEndpoints
    let c = build_dns_config(&cfg, &id_map, &deps);
    assert!(c.servers.iter().all(|s| s.tag != TS_NAME_DNS_TAG));
}

#[test]
fn direct_mode_no_geo_rules() {
    // direct 模式 → 不生成 smart/global 分流规则（无 fakeip/geo catch-all）。
    let mut cfg = base_config();
    cfg.proxy_mode = ProxyMode::Direct;
    cfg.dns_config = Some(UserDnsConfig {
        enable_fake_ip: Some(false),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    // 不应有 query_type→fakeip 或 query_type→dns-remote(global) 的 catch-all。
    let has_qt_fakeip = c
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .any(|r| r.query_type.is_some() && r.server.as_deref() == Some("fakeip"));
    assert!(!has_qt_fakeip);
}

#[test]
fn custom_bypass_fakeip_inline_rule() {
    // bypassFakeIP=true + 文件缺失（FS=false）→ inline 合并 rule。
    //
    // 判据是「解析器与该规则的去向一致」，不是某个固定 tag：走代理的域名必须拿境外解析器，
    // 否则 bypassFakeIP 这个逃生口就是朝里开的（上游 #347 暴露的正是这一点）。
    // 本测试此前断言 proxy 规则 → dns-bootstrap，即把缺陷本身当成了基线。
    let bypass_server_for = |action: RuleAction| -> String {
        let mut cfg = base_config();
        cfg.dns_config = Some(UserDnsConfig {
            enable_fake_ip: Some(true),
            ..Default::default()
        });
        cfg.custom_rules = vec![Rule {
            id: "r1".into(),
            type_field: RuleType::Domain,
            values: vec!["blocked.example.com".into()],
            conditions: None,
            combine_mode: None,
            effects: None,
            action,
            enabled: true,
            bypass_fakeip: Some(true),
            target_server_id: None,
            remarks: None,
            tls_spoof: None,
            tls_spoof_method: None,
            network_profile_id: None,
        }];
        let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
        c.rules
            .as_ref()
            .unwrap()
            .iter()
            .find(|r| {
                r.domain
                    .as_ref()
                    .is_some_and(|d| d.contains(&"blocked.example.com".to_string()))
            })
            .and_then(|r| r.server.clone())
            .expect("bypassFakeIP 规则必须 emit 一条带该域名的 DNS 规则")
    };
    assert_eq!(
        bypass_server_for(RuleAction::Proxy),
        "dns-remote",
        "走代理的 bypass 域名必须用境外解析器（境内解析器正是要绕开的那条路）"
    );
    assert_eq!(
        bypass_server_for(RuleAction::Direct),
        "dns-bootstrap",
        "直连的 bypass 域名保持境内解析器"
    );
}

#[test]
fn invalid_domestic_dns_falls_back_default() {
    // 非法 domesticDns → 回退 doh.pub（server=doh.pub）。
    let mut cfg = base_config();
    cfg.dns_config = Some(UserDnsConfig {
        domestic_dns: Some("garbage text".into()),
        ..Default::default()
    });
    let c = build_dns_config(&cfg, &BTreeMap::new(), &deps_false());
    let domestic = c.servers.iter().find(|s| s.tag == "dns-domestic").unwrap();
    assert_eq!(domestic.server.as_deref(), Some("doh.pub"));
}

#[test]
fn rules_always_set_non_empty() {
    // rules 恒非空（至少 bootstrap + local 规则）。
    let c = build_dns_config(&base_config(), &BTreeMap::new(), &deps_false());
    assert!(!c.rules.as_ref().unwrap().is_empty());
}

#[test]
fn dns_and_preserves_same_type_conditions_and_actions_only_at_root() {
    let mut config = base_config();
    let mut rule = dns_effect_rule(
        "same-type-and",
        RuleType::Domain,
        &["a.example"],
        RuleDnsResolver::Direct,
        RuleDnsAnswerMode::Real,
    );
    rule.combine_mode = Some(CombineMode::And);
    rule.conditions = Some(vec![
        RuleCondition {
            type_field: RuleType::Domain,
            values: vec!["a.example".into(), "b.example".into()],
        },
        RuleCondition {
            type_field: RuleType::Domain,
            values: vec!["b.example".into(), "c.example".into()],
        },
    ]);
    rule.effects.as_mut().unwrap().dns.as_mut().unwrap().action = Some(DnsPolicyAction::Reject {
        method: DnsRejectMethod::Default,
    });
    config.custom_rules = vec![rule];
    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    let logical = built
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .find(|r| r.type_field.as_deref() == Some("logical"))
        .unwrap();
    assert_eq!(logical.action.as_deref(), Some("reject"));
    assert_eq!(logical.mode.as_deref(), Some("and"));
    let children = logical.rules.as_ref().unwrap();
    assert_eq!(children.len(), 2);
    assert!(children
        .iter()
        .all(|r| r.action.is_none() && r.server.is_none()));
    // Same-type condition intersection must be {b}, not the flattened union {a,b,c}.
    for (domain, expected) in [
        ("a.example", false),
        ("b.example", true),
        ("c.example", false),
    ] {
        assert_eq!(
            children
                .iter()
                .all(|r| r.domain.as_ref().unwrap().iter().any(|v| v == domain)),
            expected
        );
    }
    config.custom_rules[0].combine_mode = Some(CombineMode::Or);
    let or = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    let branches: Vec<_> = or
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| r.action.as_deref() == Some("reject"))
        .collect();
    assert_eq!(branches.len(), 2);
    assert!(branches
        .iter()
        .all(|r| r.type_field.is_none() && r.rules.is_none()));
}

#[test]
fn dns_and_hosts_first_and_group_keep_matcher_children_headless() {
    let mut config = base_config();
    config.dns_servers = vec![DnsServerResource {
        id: "hosts".into(),
        name: "Hosts".into(),
        enabled: true,
        kind: DnsServerKind::Hosts,
        predefined: BTreeMap::from([("api.example.com".into(), vec!["10.0.0.8".into()])]),
        ..Default::default()
    }];
    config.dns_server_groups = vec![DnsServerGroup {
        id: "race".into(),
        name: "Race".into(),
        enabled: true,
        mode: DnsServerGroupMode::Race,
        members: vec!["builtin-domestic".into(), "builtin-remote".into()],
        fallback_server_id: Some("builtin-domestic".into()),
    }];
    let mut rule = dns_effect_rule(
        "dns-and-actions",
        RuleType::Domain,
        &["api.example.com"],
        RuleDnsResolver::Direct,
        RuleDnsAnswerMode::Real,
    );
    rule.combine_mode = Some(CombineMode::And);
    rule.conditions = Some(vec![
        RuleCondition {
            type_field: RuleType::Domain,
            values: vec!["api.example.com".into()],
        },
        RuleCondition {
            type_field: RuleType::DomainRegex,
            values: vec!["^api\\.".into()],
        },
    ]);
    for action in [
        DnsPolicyAction::HostsFirst {
            hosts_server_id: "hosts".into(),
            fallback: Box::new(DnsPolicyAction::Server {
                server_id: "builtin-remote".into(),
            }),
        },
        DnsPolicyAction::Group {
            group_id: "race".into(),
        },
    ] {
        rule.effects.as_mut().unwrap().dns.as_mut().unwrap().action = Some(action);
        config.custom_rules = vec![rule.clone()];
        let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
        let logical: Vec<_> = built
            .rules
            .as_ref()
            .unwrap()
            .iter()
            .filter(|r| r.type_field.as_deref() == Some("logical"))
            .collect();
        assert!(!logical.is_empty());
        for root in logical {
            assert!(root.action.is_some());
            assert!(
                root.domain.is_none() && root.domain_regex.is_none() && root.preferred_by.is_none()
            );
            assert!(root
                .rules
                .as_ref()
                .unwrap()
                .iter()
                .all(|child| child.action.is_none() && child.server.is_none()));
            if root.server.as_deref() == Some("dns-user-hosts") {
                assert!(root
                    .rules
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|child| child.preferred_by == Some(vec!["dns-user-hosts".into()])));
            }
        }
        for response in built
            .rules
            .as_ref()
            .unwrap()
            .iter()
            .filter(|r| r.resolver_policy.is_none() && r.action.as_deref() == Some("respond"))
        {
            assert!(response.rules.is_none());
            assert!(response.match_response.is_some());
        }
    }
}

#[test]
fn default_dns_rule_serde_keeps_legacy_shape_without_logical_fields() {
    let legacy = serde_json::json!({
        "domain": ["legacy.example"],
        "action": "route",
        "server": "dns-domestic"
    });
    let decoded: DnsRule = serde_json::from_value(legacy.clone()).unwrap();
    assert!(decoded.mode.is_none() && decoded.rules.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
    assert_eq!(serde_json::to_string(&DnsRule::default()).unwrap(), "{}");
}

mod real_query_fallback;

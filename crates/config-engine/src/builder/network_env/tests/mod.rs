//! 网络场景 N1 单测（spec §8.1 U2–U13；U1 = `tests/golden_config_snapshot.rs` 零 diff，U14 在
//! `user_config/network_profile/tests` 与 `app_config/tests`）。
//!
//! 统一走 `generate_sing_box_config_with_report` 的**完整生成**：断言看最终 JSON，不看中间结构，
//! 这样 builder 注入、展开、剪枝三段是一起被测的。

use super::*;
use crate::builder::custom_rule_files::{build_custom_rule_files, custom_rule_file_base};
use crate::builder::generate::{generate_sing_box_config_with_report, GenerateConfigDeps};
use crate::singbox::DnsConfig;
use serde_json::{json, Value};

fn deps(platform: &str, takeover: bool, custom_rules_dir: &str) -> GenerateConfigDeps {
    GenerateConfigDeps {
        platform: platform.into(),
        arch: "x64".into(),
        race_server_port: 0,
        probe_direct_port: None,
        probe_proxy_port: None,
        update_in_port: None,
        subscription_update_in_port: None,
        probe_pool_ports: vec![],
        lan_resolver_for_dns: None,
        race_upstream_ips: vec![],
        race_upstream_ports: vec![],
        has_cronet: true,
        cronet_copy_failed: false,
        has_management_api: false,
        privacy_mode: false,
        log_level: crate::user_config::LogLevel::Info,
        disable_log_file: false,
        dashboard_serve_dir: None,
        tailscale_api_port: 0,
        cache_path: "/fake/cache.db".into(),
        log_file_path: None,
        runtime_rules_dir: "/fake/runtime-rules".into(),
        rule_resources_path: "/fake/rule-resources".into(),
        custom_rules_dir: custom_rules_dir.into(),
        tailscale_state_dir_prefix: "/fake/ts".into(),
        tailnet_rules_dir: "/fake/tailnet-rules".into(),
        observed_tailnet_addresses: Default::default(),
        is_valid_srs_fn: |_| false,
        own_lan_cidrs: vec![],
        system_dns_takeover_active: takeover,
        log: |_, _| {},
        on_degraded: || {},
    }
}

/// 基础配置 + 覆盖键。无节点（`__direct__` 哨兵），schema v4。
fn config(extra: Value) -> UserConfig {
    let mut base = json!({
        "configSchemaVersion": 4,
        "servers": [],
        "selectedServerId": "__direct__",
        "proxyMode": "smart",
        "proxyModeType": "systemProxy",
    });
    for (k, v) in extra.as_object().expect("extra 必须是 object") {
        base[k] = v.clone();
    }
    serde_json::from_value(base).expect("UserConfig 反序列化")
}

fn profile(id: &str, cidrs: &[&str], domains: &[&str], probe: &str) -> Value {
    json!({"id": id, "name": id, "enabled": true,
        "match": {"dnsServerCidrs": cidrs, "searchDomains": domains}, "probe": probe})
}

fn dns_rule(id: &str, suffix: &str, profile_id: Option<&str>, action: Value) -> Value {
    let mut rule = json!({"id": id, "type": "domainSuffix", "values": [suffix],
        "action": "direct", "enabled": true,
        "effects": {"dns": {"enabled": true, "action": action,
            "resolver": "inherit", "answerMode": "real"}}});
    if let Some(p) = profile_id {
        rule["networkProfileId"] = json!(p);
    }
    rule
}

fn traffic_rule(id: &str, suffix: &str, profile_id: Option<&str>, action: &str) -> Value {
    let mut rule = json!({"id": id, "type": "domainSuffix", "values": [suffix],
        "action": action, "enabled": true,
        "effects": {"route": {"enabled": true, "action": action}}});
    if let Some(p) = profile_id {
        rule["networkProfileId"] = json!(p);
    }
    rule
}

fn domestic() -> Value {
    json!({"type": "server", "serverId": "builtin-domestic"})
}

struct Out {
    json: Value,
    pruned: Vec<PrunedEnvRule>,
}

fn generate(cfg: &UserConfig, platform: &str, takeover: bool) -> Out {
    generate_in(cfg, platform, takeover, "/fake/custom-rules")
}

fn generate_in(cfg: &UserConfig, platform: &str, takeover: bool, dir: &str) -> Out {
    let outcome =
        generate_sing_box_config_with_report(cfg, &BTreeMap::new(), &deps(platform, takeover, dir))
            .expect("生成配置");
    Out {
        json: serde_json::to_value(&outcome.config).expect("序列化"),
        pruned: outcome.pruned_env_rules,
    }
}

/// 某平面里「文本里含 needle」的顶层规则（含 logical 子规则里的匹配值）。
fn rules_mentioning<'a>(out: &'a Out, plane: &str, needle: &str) -> Vec<&'a Value> {
    out.json[plane]["rules"]
        .as_array()
        .expect("rules 数组")
        .iter()
        .filter(|r| r.to_string().contains(needle))
        .collect()
}

fn server_tags(out: &Out) -> Vec<String> {
    out.json["dns"]["servers"]
        .as_array()
        .expect("dns.servers")
        .iter()
        .map(|s| s["tag"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn env_keys(rule: &Value) -> Vec<&str> {
    ["dns_server_address", "dns_search_domain"]
        .into_iter()
        .filter(|k| rule.get(*k).is_some())
        .collect()
}

// ── U2 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u2_system_source_address_only_yields_one_dns_rule_on_dns_local() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "auto")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
    }));
    let out = generate(&cfg, "linux", false);
    let rules = rules_mentioning(&out, "dns", "corp.example");
    assert_eq!(rules.len(), 1, "只填地址段 ⇒ 恰 1 条：{rules:?}");
    assert_eq!(
        rules[0]["dns_server_address"],
        json!({"dns-local": ["10.20.0.0/16"]})
    );
    assert!(rules[0].get("dns_search_domain").is_none());
    assert!(
        !server_tags(&out).contains(&NETENV_DNS_TAG.to_string()),
        "system 源不得生成 dns-netenv"
    );
    assert!(out.pruned.is_empty(), "{:?}", out.pruned);
}

// ── U3 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u3_address_or_search_expands_to_two_rules_per_plane_in_stable_order() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &["Corp.Example."], "system")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
        "trafficRules": [traffic_rule("r-rt", "corp.example", Some("np"), "direct")],
    }));
    let out = generate(&cfg, "linux", false);
    for plane in ["dns", "route"] {
        let rules = rules_mentioning(&out, plane, "corp.example");
        assert_eq!(rules.len(), 2, "{plane} 平面必须展开成 2 条：{rules:?}");
        assert_eq!(env_keys(rules[0]), ["dns_server_address"], "{plane}[0]");
        assert_eq!(env_keys(rules[1]), ["dns_search_domain"], "{plane}[1]");
        assert_eq!(
            rules[1]["dns_search_domain"],
            json!({"dns-local": ["corp.example."]}),
            "搜索域须规范化（小写、去首尾点）并补 FQDN 点"
        );
    }
    // 流量侧照旧带动作。
    let route = rules_mentioning(&out, "route", "corp.example");
    assert!(route.iter().all(|r| r["outbound"] == "direct"), "{route:?}");
}

// ── U4 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u4_win32_search_domain_auto_resolves_dhcp_and_emits_netenv() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &["corp.example"], "auto")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
    }));
    let out = generate(&cfg, "win32", false);
    let netenv: Vec<&Value> = out.json["dns"]["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["tag"] == NETENV_DNS_TAG)
        .collect();
    assert_eq!(netenv.len(), 1, "win32 + 搜索域 + auto ⇒ 生成 dns-netenv");
    assert_eq!(netenv[0]["type"], "dhcp");
    let rules = rules_mentioning(&out, "dns", "corp.example");
    assert_eq!(rules.len(), 2);
    for rule in rules {
        let key = env_keys(rule)[0];
        let tags: Vec<&String> = rule[key].as_object().unwrap().keys().collect();
        assert_eq!(tags, [NETENV_DNS_TAG], "环境项必须引用 dns-netenv：{rule}");
    }
}

// ── U5 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u5_darwin_tun_takeover_auto_resolves_dhcp_and_system_without_takeover() {
    let extra = json!({
        "proxyModeType": "tun",
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "auto")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
    });
    let cfg = config(extra);
    for (takeover, tag) in [(true, NETENV_DNS_TAG), (false, LOCAL_DNS_TAG)] {
        let out = generate(&cfg, "darwin", takeover);
        let rules = rules_mentioning(&out, "dns", "corp.example");
        assert_eq!(rules.len(), 1, "takeover={takeover}");
        assert_eq!(
            rules[0]["dns_server_address"],
            json!({tag: ["10.20.0.0/16"]}),
            "takeover={takeover}"
        );
        assert_eq!(
            server_tags(&out).contains(&NETENV_DNS_TAG.to_string()),
            takeover,
            "dns-netenv 只在解析为 dhcp 时生成（takeover={takeover}）"
        );
    }
}

// ── U6 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u6_linux_system_proxy_explicit_dhcp_is_pruned_as_unavailable() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "dhcp")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
        "trafficRules": [traffic_rule("r-rt", "corp.example", Some("np"), "direct")],
    }));
    let out = generate(&cfg, "linux", false);
    assert!(rules_mentioning(&out, "dns", "corp.example").is_empty());
    assert!(rules_mentioning(&out, "route", "corp.example").is_empty());
    assert!(!server_tags(&out).contains(&NETENV_DNS_TAG.to_string()));
    for id in ["r-rt", "r-dns"] {
        assert!(
            out.pruned.contains(&PrunedEnvRule {
                rule_id: Some(id.into()),
                reason: PRUNE_PROBE_UNAVAILABLE,
            }),
            "{id} 必须以 PROBE_UNAVAILABLE 入报告：{:?}",
            out.pruned
        );
    }
}

// ── U7 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u7_missing_or_disabled_profile_never_degrades_to_unconditional() {
    let mut disabled = profile("np-off", &["10.20.0.0/16"], &[], "system");
    disabled["enabled"] = json!(false);
    let cases = [
        ("np-missing", vec![]),
        ("np-off", vec![disabled]),
        (
            "np-empty",
            vec![profile("np-empty", &["not-a-cidr"], &[" . "], "system")],
        ),
    ];
    for (profile_id, profiles) in cases {
        let cfg = config(json!({
            "networkProfiles": profiles,
            "dnsRules": [dns_rule("r-dns", "corp.example", Some(profile_id), domestic())],
            "trafficRules": [traffic_rule("r-rt", "corp.example", Some(profile_id), "direct")],
        }));
        let out = generate(&cfg, "linux", false);
        for plane in ["dns", "route"] {
            let leaked = rules_mentioning(&out, plane, "corp.example");
            assert!(
                leaked.is_empty(),
                "{profile_id}：{plane} 平面出现了同 matcher 的规则（退化成无条件）：{leaked:?}"
            );
        }
        for id in ["r-rt", "r-dns"] {
            assert!(
                out.pruned.contains(&PrunedEnvRule {
                    rule_id: Some(id.into()),
                    reason: PRUNE_PROFILE_REF_INVALID,
                }),
                "{profile_id}/{id}：{:?}",
                out.pruned
            );
        }
    }
}

// ── U8 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u8_logical_rules_wrap_and_udp443_pairs_carry_env() {
    let logical = |id: &str, mode: &str| {
        json!({"id": id, "type": "domainSuffix", "values": [format!("{id}.example")],
            "conditions": [
                {"type": "domainSuffix", "values": [format!("{id}.example")]},
                {"type": "port", "values": ["8443"]}],
            "combineMode": mode, "networkProfileId": "np",
            "action": "proxy", "enabled": true,
            "effects": {"route": {"enabled": true, "action": "proxy"}}})
    };
    let cfg = config(json!({
        "blockQuic": true,
        // udp443 配对只在「有节点」时生成（`block_proxy_quic` 的前提）。
        "servers": [{"id": "s1", "name": "HK", "protocol": "vless",
            "address": "hk.example.com", "port": 443, "uuid": "u", "security": "tls"}],
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "system")],
        "trafficRules": [
            logical("land", "and"),
            logical("lor", "or"),
            traffic_rule("plain", "plain.example", Some("np"), "proxy"),
        ],
    }));
    let out = generate(&cfg, "linux", false);
    let env = json!({"dns-local": ["10.20.0.0/16"]});
    for (id, inner_mode) in [("land", "and"), ("lor", "or")] {
        let rules = rules_mentioning(&out, "route", &format!("{id}.example"));
        assert_eq!(rules.len(), 2, "{id}：udp443 配对 + 本体：{rules:?}");
        let (pair, main) = (rules[0], rules[1]);
        assert_eq!(main["type"], "logical");
        assert_eq!(main["mode"], "and", "{id} 外层必须包 AND");
        assert_eq!(main["rules"][0]["type"], "logical");
        assert_eq!(main["rules"][0]["mode"], inner_mode, "{id} 内层保留原 mode");
        assert!(main["rules"][0].get("action").is_none(), "内层不带 action");
        assert_eq!(main["rules"][1], json!({"dns_server_address": env}));
        assert_eq!(pair["action"], "reject");
        assert!(
            pair.to_string().contains("dns_server_address"),
            "{id} 的 udp443 配对必须同样带环境项（否则在家也拒 QUIC）：{pair}"
        );
    }
    let plain = rules_mentioning(&out, "route", "plain.example");
    assert_eq!(plain.len(), 2, "{plain:?}");
    for rule in plain {
        assert_eq!(
            rule["dns_server_address"], env,
            "默认规则与其 udp443 配对都带环境项：{rule}"
        );
    }
}

// ── U9 ─────────────────────────────────────────────────────────────────────────

#[test]
fn u9_externalized_rule_carries_env_on_outer_rule_set_rule_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_str = dir.path().display().to_string();
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &["corp.example"], "system")],
        "customRules": [traffic_rule("ext1", "corp.example", Some("np"), "direct")],
    }));
    let files = build_custom_rule_files(&cfg);
    assert!(!files.is_empty(), "夹具必须走外化腿，否则本测没有射程");
    for (name, content) in &files {
        assert!(
            !content.contains("dns_server_address") && !content.contains("dns_search_domain"),
            "落盘的 {name} 里出现了环境键（headless rule-set decode 即拒）：{content}"
        );
        std::fs::write(dir.path().join(name), content).expect("写外化文件");
    }
    let out = generate_in(&cfg, "linux", false, &dir_str);
    let base = custom_rule_file_base("ext1");
    let outer: Vec<&Value> = out.json["route"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule_set"] == json!([base]))
        .collect();
    assert_eq!(outer.len(), 2, "外化规则按判据展开 2 条：{outer:?}");
    assert_eq!(env_keys(outer[0]), ["dns_server_address"]);
    assert_eq!(env_keys(outer[1]), ["dns_search_domain"]);
    let inline = rules_mentioning(&out, "route", "corp.example");
    assert!(
        inline.iter().all(|r| r.get("domain_suffix").is_none()),
        "外化后域名值不得再内联进 route.rules：{inline:?}"
    );
}

// ── U10 ────────────────────────────────────────────────────────────────────────

#[test]
fn u10_group_action_env_on_every_evaluate_not_on_respond() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "system")],
        "dnsServerGroups": [{"id": "g1", "name": "g1", "enabled": true, "mode": "race",
            "members": ["builtin-domestic", "builtin-remote"]}],
        "dnsRules": [dns_rule("r-g", "corp.example", Some("np"),
            json!({"type": "group", "groupId": "g1"}))],
    }));
    let out = generate(&cfg, "linux", false);
    let rules = out.json["dns"]["rules"].as_array().unwrap();
    let evaluates: Vec<&Value> = rules.iter().filter(|r| r["action"] == "evaluate").collect();
    let responds: Vec<&Value> = rules
        .iter()
        .filter(|r| {
            r["action"] == "respond"
                && r["match_response"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("dns-eval-g1-"))
        })
        .collect();
    assert_eq!(evaluates.len(), 2, "{rules:?}");
    assert_eq!(responds.len(), 2, "{rules:?}");
    for e in evaluates {
        assert_eq!(
            e["dns_server_address"],
            json!({"dns-local": ["10.20.0.0/16"]})
        );
    }
    for r in responds {
        assert!(env_keys(r).is_empty(), "respond 不得带环境项：{r}");
    }
}

// ── U11 ────────────────────────────────────────────────────────────────────────

#[test]
fn u11_fakeip_action_keeps_query_type_alongside_env() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &[], "system")],
        "dnsRules": [dns_rule("r-f", "corp.example", Some("np"), json!({"type": "fakeIp"}))],
    }));
    let out = generate(&cfg, "linux", false);
    let rules = rules_mentioning(&out, "dns", "corp.example");
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!(rules[0]["server"], "fakeip");
    assert_eq!(rules[0]["query_type"], json!(["A", "AAAA"]));
    assert_eq!(
        rules[0]["dns_server_address"],
        json!({"dns-local": ["10.20.0.0/16"]})
    );
}

// ── U12 ────────────────────────────────────────────────────────────────────────

#[test]
fn u12_post_prune_drops_whole_rules_with_bad_env_refs() {
    let cfg = config(json!({}));
    let outcome =
        generate_sing_box_config_with_report(&cfg, &BTreeMap::new(), &deps("linux", false, "/x"))
            .expect("生成");
    let mut sb = outcome.config;
    let env = |tag: &str| {
        Some(BTreeMap::from([(
            tag.to_string(),
            vec!["10.0.0.0/8".into()],
        )]))
    };
    let route = sb.route.as_mut().unwrap();
    let before_route = route.rules.len();
    route.rules.push(RouteRule {
        domain_suffix: Some(vec!["unsupported.test".into()]),
        dns_server_address: env("dns-remote"),
        ..Default::default()
    });
    route.rules.push(RouteRule {
        type_field: Some("logical".into()),
        mode: Some("and".into()),
        rules: Some(vec![
            RouteRule {
                domain_suffix: Some(vec!["nested.test".into()]),
                ..Default::default()
            },
            RouteRule {
                dns_search_domain: env("dns-nope"),
                ..Default::default()
            },
        ]),
        ..Default::default()
    });
    route.rules.push(RouteRule {
        domain_suffix: Some(vec!["good.test".into()]),
        dns_server_address: env(LOCAL_DNS_TAG),
        ..Default::default()
    });
    let dns: &mut DnsConfig = sb.dns.as_mut().unwrap();
    let dns_rules = dns.rules.get_or_insert_with(Vec::new);
    dns_rules.push(DnsRule {
        domain_suffix: Some(vec!["missing.test".into()]),
        dns_search_domain: env("dns-nope"),
        ..Default::default()
    });

    let pruned = prune_invalid_env_condition_refs(&mut sb);
    let reasons: Vec<&str> = pruned.iter().map(|p| p.reason).collect();
    assert_eq!(
        reasons,
        [
            PRUNE_REF_UNSUPPORTED_TYPE,
            PRUNE_REF_MISSING,
            PRUNE_REF_MISSING
        ],
        "https transport ⇒ UNSUPPORTED_TYPE；不存在的 tag（含 logical 子规则里）⇒ MISSING"
    );
    let text = serde_json::to_string(&sb).unwrap();
    for gone in ["unsupported.test", "nested.test", "missing.test"] {
        assert!(!text.contains(gone), "{gone} 所在的整条顶层规则必须剔除");
    }
    assert!(
        text.contains("good.test"),
        "合格引用（dns-local）必须保留（正向对照）"
    );
    assert_eq!(sb.route.as_ref().unwrap().rules.len(), before_route + 1);
}

/// 第一道防线：builder 只在 tag 已生成且类型合格时注入（与后置剪枝同一判据函数）。
#[test]
fn u12b_builder_refuses_env_ref_to_absent_transport() {
    let profiles =
        vec![
            serde_json::from_value::<NetworkProfile>(profile("np", &["10.0.0.0/8"], &[], "dhcp"))
                .unwrap(),
        ];
    let rule = Rule {
        network_profile_id: Some("np".into()),
        ..Default::default()
    };
    let local = DnsServer {
        tag: LOCAL_DNS_TAG.into(),
        type_field: Some("local".into()),
        server: None,
        server_port: None,
        path: None,
        predefined: None,
        domain_resolver: None,
        detour: None,
        endpoint: None,
        accept_search_domain: None,
        accept_default_resolvers: None,
        neighbor_domain: None,
        address: None,
        address_resolver: None,
        inet4_range: None,
        inet6_range: None,
    };
    let env = NetworkEnv::new(
        &profiles,
        Platform::Win,
        false,
        false,
        std::slice::from_ref(&local),
    );
    assert_eq!(env.for_rule(&rule), RuleEnv::Skip(PRUNE_REF_MISSING));
    let wrong_type = DnsServer {
        tag: NETENV_DNS_TAG.into(),
        type_field: Some("udp".into()),
        ..local.clone()
    };
    let env = NetworkEnv::new(&profiles, Platform::Win, false, false, &[wrong_type]);
    assert_eq!(
        env.for_rule(&rule),
        RuleEnv::Skip(PRUNE_REF_UNSUPPORTED_TYPE)
    );
    let ok = DnsServer {
        tag: NETENV_DNS_TAG.into(),
        type_field: Some("dhcp".into()),
        ..local
    };
    let env = NetworkEnv::new(&profiles, Platform::Win, false, false, &[ok]);
    assert!(matches!(env.for_rule(&rule), RuleEnv::Conditions(_)));
}

// ── U13 ────────────────────────────────────────────────────────────────────────

/// 平台轴的唯一来源：`Platform` 枚举本身。加变体 ⇒ 下面的穷尽 match 编译失败 ⇒ 必须到此补轴。
fn all_platforms() -> Vec<Platform> {
    let all = vec![
        Platform::Mac,
        Platform::Win,
        Platform::Linux,
        Platform::Other,
    ];
    for p in &all {
        match p {
            Platform::Mac | Platform::Win | Platform::Linux | Platform::Other => {}
        }
    }
    all
}

fn np(probe: NetworkProbeSource, cidrs: &[&str], domains: &[&str]) -> NetworkProfile {
    NetworkProfile {
        id: "np".into(),
        probe,
        criteria: crate::user_config::NetworkProfileCriteria {
            dns_server_cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
            search_domains: domains.iter().map(|s| s.to_string()).collect(),
        },
        ..Default::default()
    }
}

#[test]
fn u13_resolve_probe_source_table() {
    use NetworkProbeSource::{Auto, Dhcp, System};
    use ProbeResolution as R;
    use ProbeUnavailable as U;
    let both: (&[&str], &[&str]) = (&["10.0.0.0/8"], &["corp.example"]);
    let addr: (&[&str], &[&str]) = (&["10.0.0.0/8"], &[]);
    let search: (&[&str], &[&str]) = (&[], &["corp.example"]);
    // (spec §4.4 行, 平台, tun, 接管, probe, 判据, 期望)
    let rows = [
        (
            "显式 system",
            Platform::Linux,
            false,
            false,
            System,
            addr,
            R::System,
        ),
        (
            "显式 dhcp（TUN 可用）",
            Platform::Linux,
            true,
            false,
            Dhcp,
            addr,
            R::Dhcp,
        ),
        (
            "win32 + 搜索域 → dhcp",
            Platform::Win,
            false,
            false,
            Auto,
            both,
            R::Dhcp,
        ),
        (
            "win32 仅地址 → system",
            Platform::Win,
            false,
            false,
            Auto,
            addr,
            R::System,
        ),
        (
            "darwin+TUN+接管 → dhcp",
            Platform::Mac,
            true,
            true,
            Auto,
            addr,
            R::Dhcp,
        ),
        (
            "darwin+TUN 未接管 → system",
            Platform::Mac,
            true,
            false,
            Auto,
            addr,
            R::System,
        ),
        (
            "darwin 非 TUN（接管位误给）→ system",
            Platform::Mac,
            false,
            true,
            Auto,
            addr,
            R::System,
        ),
        (
            "其余 → system",
            Platform::Linux,
            false,
            false,
            Auto,
            both,
            R::System,
        ),
        (
            "可用性：dhcp 且 linux 非 TUN",
            Platform::Linux,
            false,
            false,
            Dhcp,
            addr,
            R::Unavailable(U::DhcpNeedsPrivilege),
        ),
        (
            "可用性：system 且 win32 且地址段空",
            Platform::Win,
            false,
            false,
            System,
            search,
            R::Unavailable(U::SystemNoSearchDomain),
        ),
    ];
    for (row, platform, tun, takeover, probe, (cidrs, domains), want) in rows {
        let got = resolve_probe_source(&np(probe, cidrs, domains), platform, tun, takeover);
        assert_eq!(got, want, "§4.4 行「{row}」");
    }

    // 平台轴从枚举派生：显式 dhcp 的可用性 = 能绑 UDP 68（Win/Mac 恒可；Linux/未知仅 TUN）。
    for platform in all_platforms() {
        for tun in [false, true] {
            let got = resolve_probe_source(&np(Dhcp, addr.0, addr.1), platform, tun, false);
            let privileged = matches!(platform, Platform::Win | Platform::Mac) || tun;
            let want = if privileged {
                R::Dhcp
            } else {
                R::Unavailable(U::DhcpNeedsPrivilege)
            };
            assert_eq!(got, want, "{platform:?} tun={tun}");
        }
    }
}

/// 不挂场景的规则：无条件（老配置零变化的机制层断言；字节级由金样 U1 兜）。
#[test]
fn rule_without_profile_is_unconditional_and_emits_no_netenv() {
    let cfg = config(json!({
        "networkProfiles": [profile("np", &[], &["corp.example"], "dhcp")],
        "dnsRules": [dns_rule("r-dns", "corp.example", None, domestic())],
    }));
    let out = generate(&cfg, "win32", false);
    let rules = rules_mentioning(&out, "dns", "corp.example");
    assert_eq!(rules.len(), 1);
    assert!(env_keys(rules[0]).is_empty());
    assert!(
        !server_tags(&out).contains(&NETENV_DNS_TAG.to_string()),
        "场景未被任何规则引用 ⇒ 不生成 dns-netenv"
    );
}

/// D5：DNS 规则动作引用内置「当前网络 DHCP 下发的 DNS」⇒ 生成 dns-netenv 并路由到它。
#[test]
fn builtin_netenv_dhcp_action_emits_and_targets_dns_netenv() {
    let cfg = config(json!({
        "dnsRules": [dns_rule("r-d5", "corp.example", None,
            json!({"type": "server", "serverId": BUILTIN_NETENV_DHCP_ID}))],
    }));
    let out = generate(&cfg, "win32", false);
    assert!(server_tags(&out).contains(&NETENV_DNS_TAG.to_string()));
    let rules = rules_mentioning(&out, "dns", "corp.example");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["server"], NETENV_DNS_TAG);
}

/// 保留 id `builtin-netenv-dhcp` 被用户 DNS 资源占用（N2 写入校验本应拦住）：不得重复生成
/// `dns-netenv`（重复 tag 整核 FATAL），环境项引用那个非 dhcp 的同名 transport ⇒ builder 以类型判据剔除。
///
/// 这也是后置剪枝在生成链上唯一可构造的射程：删掉 builder 的类型校验时，由后置剪枝兜住（`rule_id=None`）。
#[test]
fn reserved_netenv_id_collision_never_duplicates_tag_nor_leaks_bad_ref() {
    let mut cfg = config(json!({
        "networkProfiles": [profile("np", &["10.20.0.0/16"], &["corp.example"], "dhcp")],
        "dnsRules": [dns_rule("r-dns", "corp.example", Some("np"), domestic())],
    }));
    cfg.dns_servers.push(
        serde_json::from_value(json!({"id": BUILTIN_NETENV_DHCP_ID, "name": "squat",
            "type": "udp", "enabled": true, "endpoint": {"host": "10.0.0.53", "port": 53}}))
        .unwrap(),
    );
    let out = generate(&cfg, "win32", false);
    let netenv: Vec<&Value> = out.json["dns"]["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["tag"] == NETENV_DNS_TAG)
        .collect();
    assert_eq!(netenv.len(), 1, "dns-netenv 不得重复：{netenv:?}");
    assert_eq!(
        netenv[0]["type"], "udp",
        "夹具前提：保留 id 被 udp 资源占用"
    );
    assert!(
        !out.json.to_string().contains("\"dns_se"),
        "引用非 dhcp 同名 transport 的环境项漏进了最终配置（Start 会 FATAL）"
    );
    assert_eq!(
        out.pruned,
        [PrunedEnvRule {
            rule_id: Some("r-dns".into()),
            reason: PRUNE_REF_UNSUPPORTED_TYPE,
        }],
        "应由 builder（第一道）以类型判据剔除并带 rule_id"
    );
}

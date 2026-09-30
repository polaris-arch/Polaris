use std::collections::BTreeMap;

use crate::runtime::proxy::rule_names::{
    build_named_rule_snapshot, count_bare_raw, silent, tags_of,
};
use polaris_config_engine::builder::{generate_sing_box_config_with_report, GenerateConfigDeps};
use polaris_config_engine::singbox::{OneOrMany, RouteRule, SingBoxConfig};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::dns_constants::DIRECT_SERVER_ID;
use polaris_config_engine::user_config::proxy_mode::ProxyMode;
use polaris_config_engine::user_config::rule::{
    Rule, RuleAction, RuleResource, RuleResourceFormat, RuleType,
};
use polaris_config_engine::user_config::LogLevel;
use polaris_stats_engine::RuleIdentity;

fn deps() -> GenerateConfigDeps {
    GenerateConfigDeps {
        platform: "linux".into(),
        arch: "x64".into(),
        race_server_port: 0,
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: vec![],
        lan_resolver_for_dns: None,
        race_upstream_ips: vec![],
        race_upstream_ports: vec![],
        has_cronet: true,
        cronet_copy_failed: false,
        has_management_api: false,
        privacy_mode: false,
        log_level: LogLevel::Info,
        disable_log_file: false,
        dashboard_serve_dir: None,
        tailscale_api_port: 15490,
        cache_path: "/fake/cache.db".into(),
        log_file_path: None,
        runtime_rules_dir: "/fake/runtime-rules".into(),
        rule_resources_path: "/fake/rule-resources".into(),
        custom_rules_dir: "/fake/custom-rules".into(),
        tailnet_rules_dir: "/fake/tailnet-rules".into(),
        tailscale_state_dir_prefix: "/fake/ts".into(),
        observed_tailnet_addresses: Default::default(),
        is_valid_srs_fn: |_| true,
        own_lan_cidrs: vec![],
        log: silent,
        on_degraded: || {},
        system_dns_takeover_active: false,
        netenv_dhcp_suppressed: false,
        network_canary_port: None,
    }
}

fn resource(id: &str) -> RuleResource {
    RuleResource {
        id: id.into(),
        name: id.into(),
        category: "test".into(),
        source_url: format!("https://example.invalid/{id}"),
        file_name: format!("{id}.srs"),
        format: RuleResourceFormat::Binary,
        size: 1,
        downloaded_at: String::new(),
    }
}

fn rule(id: &str, name: &str, resources: &[&str]) -> Rule {
    Rule {
        id: id.into(),
        remarks: Some(name.into()),
        type_field: RuleType::RuleSet,
        values: resources.iter().map(|id| format!("res:{id}")).collect(),
        action: RuleAction::Direct,
        enabled: true,
        ..Rule::default()
    }
}

fn running_config(rules: Vec<Rule>) -> UserConfig {
    UserConfig {
        selected_server_id: Some(DIRECT_SERVER_ID.into()),
        proxy_mode: ProxyMode::Smart,
        traffic_rules: Some(rules),
        rule_resources: vec![resource("a"), resource("b")],
        ..UserConfig::default()
    }
}

fn generated(user: &UserConfig, deps: &GenerateConfigDeps) -> SingBoxConfig {
    generate_sing_box_config_with_report(user, &BTreeMap::new(), deps)
        .expect("valid runtime config")
        .config
}

#[test]
fn names_a_live_multi_resource_rule_but_rejects_duplicate_owners() {
    let deps = deps();
    let user = running_config(vec![rule("r1", "流媒体", &["a", "b"])]);
    let route = generated(&user, &deps);
    let named = build_named_rule_snapshot(&user, &route, &deps, &BTreeMap::new());
    assert_eq!(
        named.get("rule_set=[local-rs-a local-rs-b]"),
        Some(&RuleIdentity {
            id: "r1".into(),
            name: "流媒体".into(),
        })
    );

    let user = running_config(vec![
        rule("r1", "流媒体", &["a"]),
        rule("r2", "另一名称", &["a"]),
    ]);
    let route = generated(&user, &deps);
    assert!(
        !build_named_rule_snapshot(&user, &route, &deps, &BTreeMap::new())
            .contains_key("rule_set=local-rs-a")
    );
}

#[test]
fn refuses_one_child_logical_and_any_non_running_candidate() {
    let deps = deps();
    let user = running_config(vec![rule("r1", "内网", &["a"])]);
    let mut route = generated(&user, &deps);
    let named = build_named_rule_snapshot(&user, &route, &deps, &BTreeMap::new());
    assert!(named.contains_key("rule_set=local-rs-a"));

    let child = route
        .route
        .as_ref()
        .unwrap()
        .rules
        .iter()
        .find(|r| tags_of(r) == vec!["local-rs-a"])
        .unwrap()
        .clone();
    route.route.as_mut().unwrap().rules.push(RouteRule {
        type_field: Some("logical".into()),
        mode: Some("or".into()),
        rules: Some(vec![RouteRule {
            action: None,
            outbound: None,
            ..child
        }]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        ..RouteRule::default()
    });
    assert!(
        !build_named_rule_snapshot(&user, &route, &deps, &BTreeMap::new())
            .contains_key("rule_set=local-rs-a")
    );

    // 已保存的新 D 规则不在当前运行产物 R 内；只以 R 的最终 route 作裁判。
    let mut draft = user.clone();
    draft
        .traffic_rules
        .as_mut()
        .unwrap()
        .push(rule("draft", "未应用", &["b"]));
    assert!(
        !build_named_rule_snapshot(&draft, &generated(&user, &deps), &deps, &BTreeMap::new())
            .contains_key("rule_set=local-rs-b")
    );
}

#[test]
fn raw_rule_set_text_collision_is_detected_across_distinct_tags() {
    let multi = RouteRule {
        rule_set: Some(OneOrMany::Many(vec!["a".into(), "b".into()])),
        ..RouteRule::default()
    };
    let encoded_single = RouteRule {
        rule_set: Some(OneOrMany::One("[a b]".into())),
        ..RouteRule::default()
    };
    let mut counts = BTreeMap::new();
    count_bare_raw(&multi, &mut counts);
    count_bare_raw(&encoded_single, &mut counts);
    assert_eq!(counts.get("rule_set=[a b]"), Some(&2));
}

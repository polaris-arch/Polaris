//! 运行核规则名称：只接受起核配置里能精确追溯到用户规则的 raw matcher。
//! sing-box Connection.rule 是 Rule.String() 条件文本，没有规则 ID；同条件异名必须放弃归属。

use std::collections::BTreeMap;

use polaris_config_engine::builder::network_env::{NetworkEnv, ProbeFacts};
use polaris_config_engine::builder::Platform;
use polaris_config_engine::builder::{build_custom_rules, CustomRulesDeps, GenerateConfigDeps};
use polaris_config_engine::singbox::{OneOrMany, RouteRule, SingBoxConfig};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::dns_constants::PROXY_SELECTOR_TAG;
use polaris_config_engine::user_config::proxy_mode::ProxyMode;
use polaris_config_engine::user_config::LogLevel;
use polaris_stats_engine::RuleIdentity;

fn silent(_level: LogLevel, _message: &str) {}

fn tags_of(rule: &RouteRule) -> Vec<&str> {
    match rule.rule_set.as_ref() {
        Some(OneOrMany::One(tag)) => vec![tag],
        Some(OneOrMany::Many(tags)) => tags.iter().map(String::as_str).collect(),
        None => vec![],
    }
}

fn count_tags(rule: &RouteRule, counts: &mut BTreeMap<String, usize>) {
    for tag in tags_of(rule) {
        *counts.entry(tag.to_string()).or_default() += 1;
    }
    if let Some(children) = &rule.rules {
        for child in children {
            count_tags(child, counts);
        }
    }
}

fn count_bare_raw(rule: &RouteRule, counts: &mut BTreeMap<String, usize>) {
    if let Some(raw) = bare_rule_set_text(rule) {
        *counts.entry(raw).or_default() += 1;
    }
    if let Some(children) = &rule.rules {
        for child in children {
            count_bare_raw(child, counts);
        }
    }
}

/// 上游 `RuleSetItem.String()` 的两种形态。只允许 matcher 除 rule_set 外为空；
/// 额外环境、端口或 logical 子规则的 String 顺序没有在这里重算。
fn bare_rule_set_text(rule: &RouteRule) -> Option<String> {
    let tags = tags_of(rule);
    if tags.is_empty() || tags.iter().any(|tag| tag.is_empty()) {
        return None;
    }
    let mut value = serde_json::to_value(rule).ok()?;
    let object = value.as_object_mut()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "rule_set" | "action" | "outbound" | "no_drop" | "tls_spoof" | "tls_spoof_method"
        )
    }) {
        return None;
    }
    Some(if tags.len() == 1 {
        format!("rule_set={}", tags[0])
    } else {
        format!("rule_set=[{}]", tags.join(" "))
    })
}

/// 从本次起核的 UserConfig 重跑单条规则 builder，且必须在**最终已剪枝** route.rules 中
/// 唯一出现。此处重跑的是配置发射，不是用请求元数据猜哪条规则命中。
pub(super) fn build_named_rule_snapshot(
    user: &UserConfig,
    generated: &SingBoxConfig,
    deps: &GenerateConfigDeps,
    id_to_tag: &BTreeMap<String, String>,
) -> BTreeMap<String, RuleIdentity> {
    if user.proxy_mode != ProxyMode::Smart {
        return BTreeMap::new();
    }
    let Some(route) = generated.route.as_ref() else {
        return BTreeMap::new();
    };
    let mut tag_counts = BTreeMap::new();
    let mut raw_counts = BTreeMap::new();
    for rule in &route.rules {
        count_tags(rule, &mut tag_counts);
        count_bare_raw(rule, &mut raw_counts);
    }
    let mut definitions = BTreeMap::new();
    for entry in route.rule_set.as_deref().unwrap_or(&[]) {
        *definitions.entry(entry.tag.as_str()).or_insert(0usize) += 1;
    }
    let custom_deps = CustomRulesDeps {
        runtime_rules_dir: deps.runtime_rules_dir.clone(),
        rule_resources_path: deps.rule_resources_path.clone(),
        custom_rules_dir: deps.custom_rules_dir.clone(),
        arch: deps.arch.clone(),
        platform: deps.platform.clone(),
        is_valid_srs_fn: deps.is_valid_srs_fn,
        exists_fn: polaris_config_engine::builder::custom_rule_files::ext_rule_file_exists,
        log: silent,
        network_env: NetworkEnv::new(
            &user.network_profiles,
            &ProbeFacts {
                platform: Platform::parse(deps.platform.as_str()),
                tun: user
                    .proxy_mode_type
                    .effective_on(Platform::parse(deps.platform.as_str()))
                    .is_tun(),
                takeover_active: deps.system_dns_takeover_active,
                dhcp_suppressed: deps.netenv_dhcp_suppressed,
            },
            generated
                .dns
                .as_ref()
                .map(|dns| dns.servers.as_slice())
                .unwrap_or(&[]),
        ),
    };
    let mut candidates: BTreeMap<String, Vec<RuleIdentity>> = BTreeMap::new();
    for user_rule in user.ordered_traffic_rules() {
        let Some(name) = user_rule
            .remarks
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !user_rule.enabled || user_rule.route_action().is_none() {
            continue;
        }
        let emitted = build_custom_rules(
            std::slice::from_ref(user_rule),
            user.selected_server_id.as_deref(),
            id_to_tag,
            PROXY_SELECTOR_TAG,
            &user.rule_resources,
            false,
            &custom_deps,
        );
        for candidate in &emitted.rules {
            let Some(raw) = bare_rule_set_text(candidate) else {
                continue;
            };
            let tags = tags_of(candidate);
            // `rule_set=[a b]` 也可能由单个含括号的 tag 编码出来；避免 String 歧义。
            if tags.iter().any(|tag| {
                tag.chars()
                    .any(|c| c.is_whitespace() || c == '[' || c == ']')
            }) || raw_counts.get(&raw) != Some(&1)
            {
                continue;
            }
            // 同一 tag 在别的顶层规则或 logical 子规则出现时，raw 可碰撞；一律拒绝。
            if tags
                .iter()
                .any(|tag| definitions.get(tag) != Some(&1) || tag_counts.get(*tag) != Some(&1))
            {
                continue;
            }
            if route
                .rules
                .iter()
                .filter(|actual| *actual == candidate)
                .count()
                != 1
            {
                continue;
            }
            candidates.entry(raw).or_default().push(RuleIdentity {
                id: user_rule.id.clone(),
                name: name.to_string(),
            });
        }
    }
    candidates
        .into_iter()
        .filter_map(|(raw, owners)| (owners.len() == 1).then(|| (raw, owners[0].clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::runtime::proxy::rule_names::{
        build_named_rule_snapshot, count_bare_raw, silent, tags_of,
    };
    use polaris_config_engine::builder::{
        generate_sing_box_config_with_report, GenerateConfigDeps,
    };
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
        assert!(!build_named_rule_snapshot(
            &draft,
            &generated(&user, &deps),
            &deps,
            &BTreeMap::new()
        )
        .contains_key("rule_set=local-rs-b"));
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
}

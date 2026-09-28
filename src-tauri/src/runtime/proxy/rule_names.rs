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
mod tests;

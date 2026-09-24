//! 网络场景 → 内核环境项（`dns_server_address` / `dns_search_domain`）的唯一解析点。
//!
//! 设计真值：`~/docs/polaris/design/polaris-network-aware-rules-spec-2026-09-25.md` §4.4 / §5。
//!
//! DNS 与流量两个 builder **只调这里**，不各写一份展开逻辑：
//! - [`resolve_probe_source`]：探测源解析表（auto → system/dhcp，外加本机可用性校验）。
//! - [`NetworkEnv`]：按场景预解析好的环境项；[`NetworkEnv::for_rule`] 给出单条规则的处置
//!   （无条件 / 按判据展开 / 整条剔除）。
//! - [`env_condition_ref_ok`]：「某 tag 能否被环境项引用」的**唯一判据**；builder 的第一道防线
//!   与生成末尾的后置剪枝 [`prune_invalid_env_condition_refs`]（第二道）用的是同一个函数。
//!
//! # 为什么引用失效一律剔除整条规则
//!
//! 退化成无条件规则 = 「公司直连规则在家里也生效」，与本特性的目的相反（spec D4）；而错误的 tag 引用
//! 在内核 Start 阶段会让整核 FATAL（`check` 拦不住，spec K3）。剔除一条场景规则，影响范围 = 用户出错
//! 的范围。

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use polaris_helper_proto::Platform;
use serde::Serialize;

use crate::singbox::{DnsRule, DnsServer, RouteRule, SingBoxConfig};
use crate::user_config::app_config::UserConfig;
use crate::user_config::network_profile::{
    normalize_search_domain, NetworkProbeSource, NetworkProfile, BUILTIN_NETENV_DHCP_ID,
};
use crate::user_config::proxy_mode::ProxyModeType;
use crate::user_config::rule::Rule;
use crate::user_config::{is_valid_ip_cidr, DnsPolicyAction, DnsServerGroup};

/// 系统 DNS transport（type local，DNS builder 恒生成）。
pub const LOCAL_DNS_TAG: &str = "dns-local";

/// 场景专用的 dhcp transport。**不复用 `dns-lan`**（spec §4.3 / D9）：`dns-lan` 一旦存在会改写所有
/// `.lan/.arpa`/captive 的解析路径，那是另一项行为变化。
pub const NETENV_DNS_TAG: &str = "dns-netenv";

/// 本仓会生成、且内核支持环境项的 DNS transport 类型（上游全集还包括
/// resolved/tailscale/openvpn/openconnect，本仓不作为环境源开放）。
pub const ENV_CONDITION_TRANSPORT_TYPES: &[&str] = &["local", "dhcp"];

/// 场景不存在 / 已停用 / 判据全空（spec §3.4-2）。
pub const PRUNE_PROFILE_REF_INVALID: &str = "NETWORK_PROFILE_REF_INVALID";
/// 探测源在本机不可用（spec §4.4 可用性校验）。
pub const PRUNE_PROBE_UNAVAILABLE: &str = "NETWORK_PROFILE_PROBE_UNAVAILABLE";
/// 环境项引用的 DNS server 未生成。
pub const PRUNE_REF_MISSING: &str = "NETWORK_ENV_REF_MISSING";
/// 环境项引用的 DNS server 类型不支持环境项。
pub const PRUNE_REF_UNSUPPORTED_TYPE: &str = "NETWORK_ENV_REF_UNSUPPORTED_TYPE";

/// 探测源在本机不可用的原因（UI「本机将使用：不可用（原因）」由后端给出，渲染端不重算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeUnavailable {
    /// dhcp 需要绑 UDP 68：Linux（及未知平台，按 Linux 保守处理）非 TUN 时核无特权 ⇒ EACCES，且失败粘滞。
    DhcpNeedsPrivilege,
    /// Windows 的系统 DNS 读不到搜索域，而场景只有搜索域判据。
    SystemNoSearchDomain,
}

/// [`resolve_probe_source`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "reason")]
pub enum ProbeResolution {
    System,
    Dhcp,
    Unavailable(ProbeUnavailable),
}

/// 场景判据经规范化后的有效值：`(合法 CIDR/IP, FQDN 形式的搜索域)`。
fn effective_criteria(profile: &NetworkProfile) -> (Vec<String>, Vec<String>) {
    let cidrs = profile
        .criteria
        .dns_server_cidrs
        .iter()
        .map(|v| v.trim())
        .filter(|v| is_valid_ip_cidr(v))
        .map(str::to_string)
        .collect();
    let mut domains: Vec<String> = Vec::new();
    for fqdn in profile
        .criteria
        .search_domains
        .iter()
        .filter_map(|v| normalize_search_domain(v))
        .map(|v| format!("{v}."))
    {
        if !domains.contains(&fqdn) {
            domains.push(fqdn);
        }
    }
    (cidrs, domains)
}

/// 探测源解析表（spec §4.4，按序取第一条命中），再过本机可用性校验。
///
/// `takeover_active` 是运行期事实「macOS + TUN + 接管系统 DNS 生效」，经
/// `GenerateConfigDeps::system_dns_takeover_active` 注入；本函数仍自带 darwin && tun 前提，注入方给
/// 多了也不会误判到别的平台。
#[must_use]
pub fn resolve_probe_source(
    profile: &NetworkProfile,
    platform: Platform,
    tun: bool,
    takeover_active: bool,
) -> ProbeResolution {
    let (cidrs, domains) = effective_criteria(profile);
    let dhcp = match profile.probe {
        NetworkProbeSource::System => false,
        NetworkProbeSource::Dhcp => true,
        NetworkProbeSource::Auto => {
            (platform == Platform::Win && !domains.is_empty())
                || (platform == Platform::Mac && tun && takeover_active)
        }
    };
    if dhcp {
        // Windows 普通用户 / SYSTEM 都能绑 68（N0 207 实测）；macOS 非特权可绑 <1024；Linux 只有
        // TUN（helper 给了 CAP_NET_BIND_SERVICE/RAW）才行。未知平台按 Linux 保守处理。
        let privileged = matches!(platform, Platform::Win | Platform::Mac) || tun;
        if privileged {
            ProbeResolution::Dhcp
        } else {
            ProbeResolution::Unavailable(ProbeUnavailable::DhcpNeedsPrivilege)
        }
    } else if platform == Platform::Win && cidrs.is_empty() {
        ProbeResolution::Unavailable(ProbeUnavailable::SystemNoSearchDomain)
    } else {
        ProbeResolution::System
    }
}

/// 环境项种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvKind {
    ServerAddress,
    SearchDomain,
}

impl EnvKind {
    /// 内核规则里的键名。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            EnvKind::ServerAddress => "dns_server_address",
            EnvKind::SearchDomain => "dns_search_domain",
        }
    }
}

/// 一个环境项：`{<key>: {<tag>: [values]}}`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCondition {
    pub kind: EnvKind,
    pub tag: String,
    pub values: Vec<String>,
}

impl EnvCondition {
    fn map(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(self.tag.clone(), self.values.clone())])
    }

    /// 写到一条 DNS 规则上（DNS builder 只产出默认规则，没有 logical）。
    pub fn apply_to_dns_rule(&self, rule: &mut DnsRule) {
        match self.kind {
            EnvKind::ServerAddress => rule.dns_server_address = Some(self.map()),
            EnvKind::SearchDomain => rule.dns_search_domain = Some(self.map()),
        }
    }

    /// 写到一条流量规则的字段表上。
    ///
    /// logical 规则顶层不接受 matcher 字段 ⇒ 包一层 `{type:logical, mode:and, rules:[原 logical 去掉 action,
    /// {环境项}]}`；原 logical 的 mode 原样保留在内层（随包核 `check` 实测接受，spec §1.2 #1）。
    pub fn apply_to_route_fields(&self, fields: &mut BTreeMap<String, serde_json::Value>) {
        let env_value = serde_json::to_value(self.map()).expect("BTreeMap<String, Vec<String>>");
        let is_logical = fields.get("type").and_then(serde_json::Value::as_str) == Some("logical");
        if !is_logical {
            fields.insert(self.kind.key().into(), env_value);
            return;
        }
        let action = fields.remove("action");
        let inner = serde_json::Value::Object(std::mem::take(fields).into_iter().collect());
        let env_rule = serde_json::json!({ self.kind.key(): env_value });
        if let Some(action) = action {
            fields.insert("action".into(), action);
        }
        fields.insert("type".into(), "logical".into());
        fields.insert("mode".into(), "and".into());
        fields.insert("rules".into(), serde_json::json!([inner, env_rule]));
    }
}

/// 单条规则的处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEnv {
    /// 没挂场景：照旧生成（老配置零变化）。
    Unconditional,
    /// 按判据展开：每个环境项一份完整规则序列（至多 2 份，spec D2）。
    Conditions(Vec<EnvCondition>),
    /// 整条不生成；值为原因码（`PRUNE_*`）。
    Skip(&'static str),
}

/// 按场景预解析好的环境项。缺省（空表）= 任何挂了场景的规则都按「引用失效」剔除（fail-closed）。
#[derive(Debug, Clone, Default)]
pub struct NetworkEnv {
    profiles: BTreeMap<String, Result<Vec<EnvCondition>, &'static str>>,
}

impl NetworkEnv {
    /// `servers` = 本次已生成的 DNS server；环境项只引用其中类型合格者（第一道防线）。
    #[must_use]
    pub fn new(
        profiles: &[NetworkProfile],
        platform: Platform,
        tun: bool,
        takeover_active: bool,
        servers: &[DnsServer],
    ) -> Self {
        let mut resolved = BTreeMap::new();
        for profile in profiles {
            if profile.id.trim().is_empty() || resolved.contains_key(&profile.id) {
                continue;
            }
            let entry = resolve_profile(profile, platform, tun, takeover_active, servers);
            resolved.insert(profile.id.clone(), entry);
        }
        Self { profiles: resolved }
    }

    #[must_use]
    pub fn for_rule(&self, rule: &Rule) -> RuleEnv {
        let Some(profile_id) = rule.network_profile_id.as_deref() else {
            return RuleEnv::Unconditional;
        };
        match self.profiles.get(profile_id) {
            None => RuleEnv::Skip(PRUNE_PROFILE_REF_INVALID),
            Some(Err(reason)) => RuleEnv::Skip(reason),
            Some(Ok(conditions)) => RuleEnv::Conditions(conditions.clone()),
        }
    }
}

fn resolve_profile(
    profile: &NetworkProfile,
    platform: Platform,
    tun: bool,
    takeover_active: bool,
    servers: &[DnsServer],
) -> Result<Vec<EnvCondition>, &'static str> {
    let (cidrs, domains) = effective_criteria(profile);
    if !profile.enabled || (cidrs.is_empty() && domains.is_empty()) {
        return Err(PRUNE_PROFILE_REF_INVALID);
    }
    let tag = match resolve_probe_source(profile, platform, tun, takeover_active) {
        ProbeResolution::System => LOCAL_DNS_TAG,
        ProbeResolution::Dhcp => NETENV_DNS_TAG,
        ProbeResolution::Unavailable(_) => return Err(PRUNE_PROBE_UNAVAILABLE),
    };
    env_condition_ref_ok(servers, tag).map_err(EnvRefError::reason)?;
    let mut conditions = Vec::with_capacity(2);
    if !cidrs.is_empty() {
        conditions.push(EnvCondition {
            kind: EnvKind::ServerAddress,
            tag: tag.to_string(),
            values: cidrs,
        });
    }
    if !domains.is_empty() {
        conditions.push(EnvCondition {
            kind: EnvKind::SearchDomain,
            tag: tag.to_string(),
            values: domains,
        });
    }
    Ok(conditions)
}

/// 环境项引用不合格的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvRefError {
    Missing,
    UnsupportedType,
}

impl EnvRefError {
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            EnvRefError::Missing => PRUNE_REF_MISSING,
            EnvRefError::UnsupportedType => PRUNE_REF_UNSUPPORTED_TYPE,
        }
    }
}

/// 唯一判据：某 tag 能否被 `dns_server_address` / `dns_search_domain` 引用。
pub fn env_condition_ref_ok(servers: &[DnsServer], tag: &str) -> Result<(), EnvRefError> {
    let server = servers
        .iter()
        .find(|s| s.tag == tag)
        .ok_or(EnvRefError::Missing)?;
    match server.type_field.as_deref() {
        Some(kind) if ENV_CONDITION_TRANSPORT_TYPES.contains(&kind) => Ok(()),
        _ => Err(EnvRefError::UnsupportedType),
    }
}

/// 被剔除的一条场景规则（`GenerateOutcome::pruned_env_rules` 的元素）。
///
/// builder 阶段剔除的带 `rule_id`；后置剪枝看的是最终 JSON，内核规则上没有用户规则 id ⇒ `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrunedEnvRule {
    #[serde(rename = "ruleId", skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub reason: &'static str,
}

/// DNS builder 会处理的用户 DNS 规则（与 `builder::dns` 用户规则块的入口过滤同口径）。
#[must_use]
pub fn dns_rule_is_candidate(rule: &Rule) -> bool {
    rule.enabled && rule.effects.is_some() && rule.dns_effect().is_some()
}

fn action_refs_server(action: &DnsPolicyAction, groups: &[DnsServerGroup], id: &str) -> bool {
    match action {
        DnsPolicyAction::Server { server_id } => server_id == id,
        DnsPolicyAction::HostsFirst {
            hosts_server_id,
            fallback,
        } => hosts_server_id == id || action_refs_server(fallback, groups, id),
        DnsPolicyAction::Group { group_id } => groups
            .iter()
            .filter(|g| g.id == *group_id && g.enabled)
            .any(|g| {
                g.members.iter().any(|m| m == id) || g.fallback_server_id.as_deref() == Some(id)
            }),
        _ => false,
    }
}

/// 本次是否需要生成 `dns-netenv`（spec §4.2「只在需要时生成」）：某条会生成的规则挂了解析为 dhcp 的
/// 启用场景，或某条 DNS 规则的动作引用了 [`BUILTIN_NETENV_DHCP_ID`]。否则一个字节都不变。
#[must_use]
pub fn netenv_required(config: &UserConfig, platform: Platform, takeover_active: bool) -> bool {
    let tun = config.proxy_mode_type == ProxyModeType::Tun;
    let dns_rules: Vec<&Rule> = config
        .effective_dns_rules()
        .iter()
        .filter(|r| dns_rule_is_candidate(r))
        .collect();
    let by_action = dns_rules.iter().any(|rule| {
        rule.dns_effect()
            .and_then(|effect| effect.action)
            .is_some_and(|action| {
                action_refs_server(&action, &config.dns_server_groups, BUILTIN_NETENV_DHCP_ID)
            })
    });
    if by_action {
        return true;
    }
    let traffic = config
        .effective_traffic_rules()
        .iter()
        .filter(|r| crate::builder::route::traffic_rule_is_built(r, config));
    dns_rules
        .into_iter()
        .chain(traffic)
        .filter_map(|rule| rule.network_profile_id.as_deref())
        .filter_map(|id| config.network_profiles.iter().find(|p| p.id == id))
        .any(|profile| {
            let (cidrs, domains) = effective_criteria(profile);
            profile.enabled
                && !(cidrs.is_empty() && domains.is_empty())
                && resolve_probe_source(profile, platform, tun, takeover_active)
                    == ProbeResolution::Dhcp
        })
}

/// builder 阶段按 [`NetworkEnv::for_rule`] 剔除的规则（与两个 builder 同一判据、同一输入重放，
/// 只为出报告；不参与生成）。
#[must_use]
pub fn builder_skipped_rules(config: &UserConfig, env: &NetworkEnv) -> Vec<PrunedEnvRule> {
    let traffic = config
        .ordered_traffic_rules()
        .into_iter()
        .filter(|r| crate::builder::route::traffic_rule_is_built(r, config));
    let dns = config
        .ordered_dns_rules()
        .into_iter()
        .filter(|r| dns_rule_is_candidate(r));
    let mut out: Vec<PrunedEnvRule> = Vec::new();
    for rule in traffic.chain(dns) {
        if let RuleEnv::Skip(reason) = env.for_rule(rule) {
            let entry = PrunedEnvRule {
                rule_id: Some(rule.id.clone()),
                reason,
            };
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
    }
    out
}

fn env_tags<'a>(
    address: Option<&'a BTreeMap<String, Vec<String>>>,
    search: Option<&'a BTreeMap<String, Vec<String>>>,
) -> impl Iterator<Item = &'a String> {
    address.into_iter().chain(search).flat_map(BTreeMap::keys)
}

fn first_bad_ref<'a>(
    servers: &[DnsServer],
    mut tags: impl Iterator<Item = &'a String>,
) -> Option<EnvRefError> {
    tags.find_map(|tag| env_condition_ref_ok(servers, tag).err())
}

fn route_rule_bad_ref(servers: &[DnsServer], rule: &RouteRule) -> Option<EnvRefError> {
    first_bad_ref(
        servers,
        env_tags(
            rule.dns_server_address.as_ref(),
            rule.dns_search_domain.as_ref(),
        ),
    )
    .or_else(|| {
        rule.rules
            .iter()
            .flatten()
            .find_map(|sub| route_rule_bad_ref(servers, sub))
    })
}

/// 生成末尾的后置剪枝（第二道防线）：遍历 `route.rules`（递归进 logical）与 `dns.rules`，任何环境项
/// 引用不合格 ⇒ 剔除**整条顶层规则**。看的是最终 JSON，与哪条 builder 路径产出无关，兜住将来新增的规则腿。
pub fn prune_invalid_env_condition_refs(cfg: &mut SingBoxConfig) -> Vec<PrunedEnvRule> {
    let servers: Vec<DnsServer> = cfg
        .dns
        .as_ref()
        .map(|dns| dns.servers.clone())
        .unwrap_or_default();
    let mut pruned = Vec::new();
    let mut record = |err: EnvRefError| {
        pruned.push(PrunedEnvRule {
            rule_id: None,
            reason: err.reason(),
        });
    };
    if let Some(route) = cfg.route.as_mut() {
        route
            .rules
            .retain(|rule| match route_rule_bad_ref(&servers, rule) {
                Some(err) => {
                    record(err);
                    false
                }
                None => true,
            });
    }
    if let Some(rules) = cfg.dns.as_mut().and_then(|dns| dns.rules.as_mut()) {
        rules.retain(|rule| {
            match first_bad_ref(
                &servers,
                env_tags(
                    rule.dns_server_address.as_ref(),
                    rule.dns_search_domain.as_ref(),
                ),
            ) {
                Some(err) => {
                    record(err);
                    false
                }
                None => true,
            }
        });
    }
    pruned
}

#[cfg(test)]
mod tests;

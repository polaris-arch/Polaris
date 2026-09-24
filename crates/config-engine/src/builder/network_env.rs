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

use crate::builder::dns::follow_route_default_server_id;
use crate::singbox::{DnsRule, DnsServer, RouteRule, SingBoxConfig};
use crate::user_config::app_config::UserConfig;
use crate::user_config::network_profile::{
    normalize_search_domain, NetworkProbeSource, NetworkProfile, BUILTIN_NETENV_DHCP_ID,
};
use crate::user_config::rule::{Rule, RuleAction};
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
/// 本次起核因 `missing monitor for auto DHCP` 失败后，运行时剔除 dhcp transport 重试（spec R4）：
/// 解析为 dhcp 的场景规则与引用内置 `builtin-netenv-dhcp` 的 DNS 规则本次都不生成。
pub const PRUNE_DHCP_MONITOR_MISSING: &str = "NETWORK_PROFILE_DHCP_MONITOR_MISSING";
/// **告警，不剪枝**：探测源为 dhcp 而地址段判据只有 IPv6。DHCPv4 只下发 IPv4 DNS（N0 207 实测），
/// 地址段判据永不命中；规则照常生成（与剪掉行为等价但保留用户意图），只让它可见。
pub const WARN_DHCP_IPV6_ONLY: &str = "NETWORK_PROFILE_DHCP_IPV6_ONLY";

/// 报告里「只告警、规则仍生成」的原因码；其余原因码 = 该规则本次未生成。
pub const ENV_REPORT_WARNINGS: &[&str] = &[WARN_DHCP_IPV6_ONLY];

/// 探测源判定的全部本机输入（生成侧与「本机解析后的探测源」查询接口共用同一份，渲染端不重算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeFacts {
    pub platform: Platform,
    pub tun: bool,
    /// 运行期事实「macOS + TUN + 接管系统 DNS 生效」（见 [`resolve_probe_source`]）。
    pub takeover_active: bool,
    /// 本次会话 dhcp transport 已被运行时剔除（spec R4：起核报 `missing monitor for auto DHCP`）。
    pub dhcp_suppressed: bool,
}

impl ProbeFacts {
    /// dhcp transport（`dns-netenv`）本机本次不可用的原因；`None` = 可用。
    ///
    /// dhcp 源场景与 D5 内置解析器 `builtin-netenv-dhcp` 共用这一个判据：二者是同一个 transport。
    #[must_use]
    pub fn dhcp_unavailable(&self) -> Option<ProbeReason> {
        if !dhcp_privileged(self.platform, self.tun) {
            Some(ProbeReason::DhcpNeedsPrivilege)
        } else if self.dhcp_suppressed {
            Some(ProbeReason::DhcpMonitorMissing)
        } else {
            None
        }
    }
}

/// 内置解析器 `builtin-netenv-dhcp`（D5）在本机本次是否可用：IPC `network_profile_builtin_dhcp_status`
/// 的返回体（JSON camelCase）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BuiltinDhcpStatus {
    pub available: bool,
    pub reason: Option<ProbeReason>,
}

/// 内置解析器可用性：与生成侧 B（[`NetworkEnv::dns_action_skip`]）同一个判据
/// [`ProbeFacts::dhcp_unavailable`]，不另写一份。
#[must_use]
pub fn builtin_dhcp_status(facts: &ProbeFacts) -> BuiltinDhcpStatus {
    let reason = facts.dhcp_unavailable();
    BuiltinDhcpStatus {
        available: reason.is_none(),
        reason,
    }
}

/// 场景 / 探测源的不可用或告警原因。IPC（[`ResolvedProbe::reason`]）按 camelCase 序列化这个枚举；
/// 生成报告与非致命信号用 [`Self::report_code`] 的原因码（同一原因的两种形态，由本枚举单点映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeReason {
    /// 场景已停用，或两项判据规范化后都为空 ⇒ 引用它的规则不生成。
    ProfileInvalid,
    /// dhcp 需要绑 UDP 68 而核无特权（Linux / 未知平台的非 TUN）。
    DhcpNeedsPrivilege,
    /// Windows 的系统 DNS 读不到搜索域，而场景只有搜索域判据。
    SystemNoSearchDomain,
    /// 本次起核报 `missing monitor for auto DHCP`，运行时已剔除 dhcp transport（spec R4）。
    DhcpMonitorMissing,
    /// **告警**：dhcp 源而地址段判据只有 IPv6（DHCPv4 只带 IPv4 DNS，地址段永不命中）；规则仍生成。
    DhcpIpv6Only,
}

impl ProbeReason {
    /// 生成报告 / 非致命信号里的原因码。
    #[must_use]
    pub fn report_code(self) -> &'static str {
        match self {
            ProbeReason::ProfileInvalid => PRUNE_PROFILE_REF_INVALID,
            ProbeReason::DhcpNeedsPrivilege | ProbeReason::SystemNoSearchDomain => {
                PRUNE_PROBE_UNAVAILABLE
            }
            ProbeReason::DhcpMonitorMissing => PRUNE_DHCP_MONITOR_MISSING,
            ProbeReason::DhcpIpv6Only => WARN_DHCP_IPV6_ONLY,
        }
    }
}

impl From<ProbeUnavailable> for ProbeReason {
    fn from(value: ProbeUnavailable) -> Self {
        match value {
            ProbeUnavailable::DhcpNeedsPrivilege => ProbeReason::DhcpNeedsPrivilege,
            ProbeUnavailable::SystemNoSearchDomain => ProbeReason::SystemNoSearchDomain,
        }
    }
}

/// dhcp 需要绑 UDP 68：Windows 普通用户 / SYSTEM 都能绑（N0 207 实测）；macOS 非特权可绑 <1024；
/// Linux 只有 TUN（helper 给了 CAP_NET_BIND_SERVICE/RAW）才行。未知平台按 Linux 保守处理。
fn dhcp_privileged(platform: Platform, tun: bool) -> bool {
    matches!(platform, Platform::Win | Platform::Mac) || tun
}

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

/// 本机实际使用的探测源（「不可用」时也给出按解析表**本应**使用的那一个，供 UI 说明原因）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeSourceKind {
    System,
    Dhcp,
}

/// 「本机解析后的探测源」：IPC `network_profile_resolved_sources` 的元素（JSON camelCase）。
///
/// 与生成侧同一判据（[`resolved_probe`]），渲染端只显示、不重算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedProbe {
    pub profile_id: String,
    pub probe_source: ProbeSourceKind,
    /// `false` = 引用本场景的规则本次不生成。
    pub available: bool,
    /// 不可用原因或告警原因（camelCase，见 [`ProbeReason`]）；可用且无告警时为 `null`。
    pub reason: Option<ProbeReason>,
    /// 内核 canary 探针（spec §6.3 方案 2）给出的「当前是否处在该网络」：`true` 命中 / `false` 未命中 /
    /// `null` 未知（核未运行、本场景本次没有 canary、网络刚变化还没探完）。本函数恒给 `None`，由运行时
    /// 按运行核的探测结果填入 —— 判据只有内核那一份，这里不推算。
    pub matched: Option<bool>,
}

/// 单个场景在本机的处置：解析表 + 可用性 + 场景自身有效性 + 告警，**唯一判据**。
///
/// 生成侧（[`NetworkEnv`] / [`netenv_required`]）与查询接口都只调它。
#[must_use]
pub fn resolved_probe(profile: &NetworkProfile, facts: &ProbeFacts) -> ResolvedProbe {
    let (cidrs, domains) = effective_criteria(profile);
    let resolution =
        resolve_probe_source(profile, facts.platform, facts.tun, facts.takeover_active);
    let probe_source = match resolution {
        ProbeResolution::System
        | ProbeResolution::Unavailable(ProbeUnavailable::SystemNoSearchDomain) => {
            ProbeSourceKind::System
        }
        ProbeResolution::Dhcp
        | ProbeResolution::Unavailable(ProbeUnavailable::DhcpNeedsPrivilege) => {
            ProbeSourceKind::Dhcp
        }
    };
    let unavailable = if !profile.enabled || (cidrs.is_empty() && domains.is_empty()) {
        Some(ProbeReason::ProfileInvalid)
    } else {
        match resolution {
            ProbeResolution::Unavailable(why) => Some(why.into()),
            ProbeResolution::Dhcp => facts.dhcp_unavailable(),
            ProbeResolution::System => None,
        }
    };
    // 地址段判据只有 IPv6 且走 dhcp：DHCPv4 只带 IPv4 DNS ⇒ 地址段永不命中（有效 CIDR 已过校验，
    // 含 `:` 即 IPv6）。
    let v6_only_on_dhcp = probe_source == ProbeSourceKind::Dhcp
        && !cidrs.is_empty()
        && cidrs.iter().all(|cidr| cidr.contains(':'));
    ResolvedProbe {
        profile_id: profile.id.clone(),
        probe_source,
        available: unavailable.is_none(),
        reason: unavailable.or(v6_only_on_dhcp.then_some(ProbeReason::DhcpIpv6Only)),
        matched: None,
    }
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
        if dhcp_privileged(platform, tun) {
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
    profiles: BTreeMap<String, Result<ProfileEnv, &'static str>>,
    /// dhcp transport 本机本次不可用的原因（D5 内置解析器动作据此剔除，见 [`Self::dns_action_skip`]）。
    /// 缺省 `None` 只出现在「没有任何场景输入」的默认值里，那里也不会有 DNS 规则经过它。
    dhcp_unavailable: Option<&'static str>,
}

/// 单个场景的预解析结果：环境项 + 告警（告警不影响生成）。
#[derive(Debug, Clone)]
struct ProfileEnv {
    conditions: Vec<EnvCondition>,
    warning: Option<&'static str>,
}

impl NetworkEnv {
    /// `servers` = 本次已生成的 DNS server；环境项只引用其中类型合格者（第一道防线）。
    #[must_use]
    pub fn new(profiles: &[NetworkProfile], facts: &ProbeFacts, servers: &[DnsServer]) -> Self {
        let mut resolved = BTreeMap::new();
        for profile in profiles {
            if profile.id.trim().is_empty() || resolved.contains_key(&profile.id) {
                continue;
            }
            let entry = resolve_profile(profile, facts, servers);
            resolved.insert(profile.id.clone(), entry);
        }
        Self {
            profiles: resolved,
            dhcp_unavailable: facts.dhcp_unavailable().map(ProbeReason::report_code),
        }
    }

    #[must_use]
    pub fn for_rule(&self, rule: &Rule) -> RuleEnv {
        let Some(profile_id) = rule.network_profile_id.as_deref() else {
            return RuleEnv::Unconditional;
        };
        match self.profiles.get(profile_id) {
            None => RuleEnv::Skip(PRUNE_PROFILE_REF_INVALID),
            Some(Err(reason)) => RuleEnv::Skip(reason),
            Some(Ok(entry)) => RuleEnv::Conditions(entry.conditions.clone()),
        }
    }

    /// 规则挂的场景带告警（规则仍生成）时的告警原因码。
    fn warning_for(&self, rule: &Rule) -> Option<&'static str> {
        let id = rule.network_profile_id.as_deref()?;
        self.profiles.get(id)?.as_ref().ok()?.warning
    }

    /// DNS 规则的动作用到内置 `builtin-netenv-dhcp`（含 group 成员/回退、hostsFirst 两腿、
    /// followRouteDefault 解出的默认），而 dhcp transport 本机本次不可用 ⇒ 整条剔除的原因码。
    ///
    /// 剔除而**不**退化成别的解析器（spec §4.4 口径）：Linux 系统代理下核无特权绑 68，查询必然失败，
    /// 悄悄换成别的解析器会把「公司内网解析」送去错误的上游。
    #[must_use]
    pub fn dns_action_skip(&self, rule: &Rule, config: &UserConfig) -> Option<&'static str> {
        self.dhcp_unavailable
            .filter(|_| dns_rule_uses_netenv(rule, config))
    }
}

fn resolve_profile(
    profile: &NetworkProfile,
    facts: &ProbeFacts,
    servers: &[DnsServer],
) -> Result<ProfileEnv, &'static str> {
    let probe = resolved_probe(profile, facts);
    if !probe.available {
        return Err(probe
            .reason
            .map_or(PRUNE_PROBE_UNAVAILABLE, ProbeReason::report_code));
    }
    let tag = match probe.probe_source {
        ProbeSourceKind::System => LOCAL_DNS_TAG,
        ProbeSourceKind::Dhcp => NETENV_DNS_TAG,
    };
    env_condition_ref_ok(servers, tag).map_err(EnvRefError::reason)?;
    let (cidrs, domains) = effective_criteria(profile);
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
    Ok(ProfileEnv {
        conditions,
        warning: probe.reason.map(ProbeReason::report_code),
    })
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

/// 场景规则报告的一条（`GenerateOutcome::pruned_env_rules` 的元素）：被剔除的规则，或带告警
/// 仍生成的规则（原因码在 [`ENV_REPORT_WARNINGS`] 里，见 [`Self::is_warning`]）。
///
/// builder 阶段剔除的带 `rule_id`；后置剪枝看的是最终 JSON，内核规则上没有用户规则 id ⇒ `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrunedEnvRule {
    #[serde(rename = "ruleId", skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub reason: &'static str,
}

impl PrunedEnvRule {
    /// `true` = 只告警、规则本次仍生成。
    #[must_use]
    pub fn is_warning(&self) -> bool {
        ENV_REPORT_WARNINGS.contains(&self.reason)
    }
}

/// DNS builder 会处理的用户 DNS 规则（与 `builder::dns` 用户规则块的入口过滤同口径）。
#[must_use]
pub fn dns_rule_is_candidate(rule: &Rule) -> bool {
    rule.enabled && rule.effects.is_some() && rule.dns_effect().is_some()
}

fn action_refs_server(
    action: &DnsPolicyAction,
    route_action: Option<RuleAction>,
    config: &UserConfig,
    id: &str,
) -> bool {
    let groups: &[DnsServerGroup] = &config.dns_server_groups;
    match action {
        DnsPolicyAction::Server { server_id } => server_id == id,
        DnsPolicyAction::FollowRouteDefault => {
            follow_route_default_server_id(route_action, config.dns_defaults.as_ref()) == id
        }
        DnsPolicyAction::HostsFirst {
            hosts_server_id,
            fallback,
        } => hosts_server_id == id || action_refs_server(fallback, route_action, config, id),
        DnsPolicyAction::Group { group_id } => groups
            .iter()
            .filter(|g| g.id == *group_id && g.enabled)
            .any(|g| {
                g.members.iter().any(|m| m == id) || g.fallback_server_id.as_deref() == Some(id)
            }),
        _ => false,
    }
}

/// DNS 规则的动作是否用到内置 [`BUILTIN_NETENV_DHCP_ID`]（`dns-netenv`）。
#[must_use]
pub fn dns_rule_uses_netenv(rule: &Rule, config: &UserConfig) -> bool {
    rule.dns_effect()
        .and_then(|effect| effect.action)
        .is_some_and(|action| {
            action_refs_server(&action, rule.route_action(), config, BUILTIN_NETENV_DHCP_ID)
        })
}

/// 本次是否需要生成 `dns-netenv`（spec §4.2「只在需要时生成」）：某条会生成的规则挂了解析为 dhcp 的
/// 启用场景，或某条 DNS 规则的动作用到了 [`BUILTIN_NETENV_DHCP_ID`]——两者都以 dhcp transport 本机
/// 本次可用为前提（不可用时那些规则整条剔除，生成它只会白绑一次 68）。否则一个字节都不变。
///
/// `facts.tun` 由调用方按 `config.proxy_mode_type` 给出（与 [`NetworkEnv::new`] 同一份）。
#[must_use]
pub fn netenv_required(config: &UserConfig, facts: &ProbeFacts) -> bool {
    let dns_rules: Vec<&Rule> = config
        .effective_dns_rules()
        .iter()
        .filter(|r| dns_rule_is_candidate(r))
        .collect();
    let by_action = facts.dhcp_unavailable().is_none()
        && dns_rules
            .iter()
            .any(|rule| dns_rule_uses_netenv(rule, config));
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
            let probe = resolved_probe(profile, facts);
            probe.available && probe.probe_source == ProbeSourceKind::Dhcp
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
    let mut push = |rule: &Rule, reason: &'static str| {
        let entry = PrunedEnvRule {
            rule_id: Some(rule.id.clone()),
            reason,
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    };
    // 与 DNS builder 用户规则块同序判定：场景处置先于动作处置（场景已剔除就不再看动作）。
    for (rule, is_dns) in traffic.map(|r| (r, false)).chain(dns.map(|r| (r, true))) {
        match env.for_rule(rule) {
            RuleEnv::Skip(reason) => push(rule, reason),
            _ => match is_dns.then(|| env.dns_action_skip(rule, config)).flatten() {
                Some(reason) => push(rule, reason),
                None => {
                    if let Some(warning) = env.warning_for(rule) {
                        push(rule, warning);
                    }
                }
            },
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

/// canary 探针的回环入站 tag（spec §6.3 方案 2）。
pub const NETWORK_CANARY_INBOUND_TAG: &str = "np-probe-in";
/// canary 域名后缀：RFC 6761 保留的 `.invalid`，与任何真实查询不相交。
pub const NETWORK_CANARY_SUFFIX: &str = "np-canary.polaris.invalid";

/// 一个场景的 canary：查询 `domain`，内核按该场景的环境项亲自求值（命中 ⇒ A 127.0.0.1，否则 NXDOMAIN）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkCanary {
    pub profile_id: String,
    pub domain: String,
}

/// 本次生成实际写进配置的 canary：回环端口 + 每个场景一项。`None`（见 [`apply_network_canaries`]）
/// = 本次没有 canary 入站与规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkCanaryPlan {
    pub port: u16,
    pub canaries: Vec<NetworkCanary>,
}

impl NetworkEnv {
    /// 能出 canary 的场景：本机可用、且环境项引用的 transport 本次确实生成了（与规则注入同一份预解析，
    /// 第一道防线同源）。按场景 id 排序，输出确定。
    fn canary_candidates(&self) -> impl Iterator<Item = (&String, &[EnvCondition])> {
        self.profiles.iter().filter_map(|(id, entry)| {
            let entry = entry.as_ref().ok()?;
            (!entry.conditions.is_empty()).then_some((id, entry.conditions.as_slice()))
        })
    }
}

/// 生成 canary 探针（spec §6.3 方案 2 / §5.4）：
///
/// - 每个候选场景一组 DNS 规则：每个环境项一条 `{domain, inbound, <环境项>} → predefined A 127.0.0.1`，
///   外加一条同名兜底 `→ predefined NXDOMAIN`；整组 `unshift` 到 `dns.rules` **绝对最前**（与探测池规则同一
///   插入点）。`inbound` 限定在探针入站，真实查询即使撞上这个域名也不会被它们答复。
/// - 一个只听 `127.0.0.1` 的 UDP `direct` 入站 + `route.rules` 最前的 `{inbound, hijack-dns}`。
///
/// 场景的地址段与搜索域之间是「任一命中」（D2），与规则展开同形：任一条正向规则命中即 A 记录。
/// `predefined` 不经 transport 缓存（spec K10），每次查询都重新求值。
///
/// 无端口、或没有任何候选场景 ⇒ 一个字节都不改（老配置零 diff），返回 `None`。
/// 候选只看场景本身，不看有没有规则引用它：system 源场景恒可出 canary；dhcp 源场景只有在 `dns-netenv`
/// 因规则需要而生成时才有（不为了显示命中态去多绑一次 UDP 68）。
pub fn apply_network_canaries(
    cfg: &mut SingBoxConfig,
    env: &NetworkEnv,
    port: Option<u16>,
) -> Option<NetworkCanaryPlan> {
    let port = port.filter(|p| *p > 0)?;
    let canaries: Vec<(NetworkCanary, &[EnvCondition])> = env
        .canary_candidates()
        .enumerate()
        .map(|(k, (id, conditions))| {
            let canary = NetworkCanary {
                profile_id: id.clone(),
                domain: format!("p{k}.{NETWORK_CANARY_SUFFIX}"),
            };
            (canary, conditions)
        })
        .collect();
    if canaries.is_empty() {
        return None;
    }
    let inbound = || {
        Some(crate::singbox::OneOrMany::Many(vec![
            NETWORK_CANARY_INBOUND_TAG.to_string(),
        ]))
    };
    let mut rules: Vec<DnsRule> = Vec::new();
    for (canary, conditions) in &canaries {
        let base = DnsRule {
            domain: Some(vec![canary.domain.clone()]),
            inbound: inbound(),
            action: Some("predefined".into()),
            ..Default::default()
        };
        for condition in *conditions {
            let mut hit = DnsRule {
                rcode: Some("NOERROR".into()),
                answer: Some(vec![format!("{}. IN A 127.0.0.1", canary.domain)]),
                ..base.clone()
            };
            condition.apply_to_dns_rule(&mut hit);
            rules.push(hit);
        }
        rules.push(DnsRule {
            rcode: Some("NXDOMAIN".into()),
            ..base
        });
    }
    if let Some(dns) = cfg.dns.as_mut() {
        let existing = dns.rules.get_or_insert_with(Vec::new);
        existing.splice(0..0, rules);
    }
    cfg.inbounds
        .push(crate::builder::inbounds::udp_direct_loopback(
            NETWORK_CANARY_INBOUND_TAG,
            port,
        ));
    if let Some(route) = cfg.route.as_mut() {
        route.rules.insert(
            0,
            RouteRule {
                inbound: inbound(),
                action: Some("hijack-dns".into()),
                ..Default::default()
            },
        );
    }
    Some(NetworkCanaryPlan {
        port,
        canaries: canaries.into_iter().map(|(c, _)| c).collect(),
    })
}

#[cfg(test)]
mod tests;

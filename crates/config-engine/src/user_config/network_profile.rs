//! 网络场景（Network Profile）：「仅在某个网络下生效」的可复用判据。
//!
//! 设计真值：`~/docs/polaris/design/polaris-network-aware-rules-spec-2026-09-25.md` §3.3 / §7。
//! 本模块只放**数据形态**与反序列化容错；判据怎么解析成内核环境项（探测源、tag、展开、剪枝）
//! 归 `builder::network_env`，两处不各写一份。

#![forbid(unsafe_code)]

use serde::{Deserialize, Deserializer, Serialize};

fn default_true() -> bool {
    true
}

/// DNS 规则动作可引用的内置解析器「当前网络 DHCP 下发的 DNS」（spec D5）。
///
/// 映射到的 transport tag 由 [`crate::user_config::dns_server_tag`] 单点给出（`dns-netenv`）；
/// 该 id 是保留 id，store 写入校验须禁止用户资源占用（N2）。
pub const BUILTIN_NETENV_DHCP_ID: &str = "builtin-netenv-dhcp";

/// 一个网络场景。
///
/// 所有字段都有缺省：缺键不能炸掉整份 `UserConfig`（部分消费腿对反序列化失败用
/// `unwrap_or_default()`，一失败就会把整份配置静默换成默认值；理由同 `dns_config.rs` 的注释）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 停用 = 引用它的规则**永不命中**（不生成），不是「无条件」。
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "match", default)]
    pub criteria: NetworkProfileCriteria,
    #[serde(default)]
    pub probe: NetworkProbeSource,
}

impl Default for NetworkProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            criteria: NetworkProfileCriteria::default(),
            probe: NetworkProbeSource::default(),
        }
    }
}

/// 场景判据。两种判据之间是「任一命中」（spec D2）；同一种判据内的多个值内核本就按 OR。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfileCriteria {
    /// 当前网络 DNS 服务器地址落在这些网段（CIDR 或裸 IP）里任一即命中。
    #[serde(
        rename = "dnsServerCidrs",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub dns_server_cidrs: Vec<String>,
    /// 当前网络的搜索域**精确**等于其中之一即命中（内核规范化后精确匹配，非后缀，spec K1）。
    #[serde(
        rename = "searchDomains",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub search_domains: Vec<String>,
}

/// 探测源：内核从哪个 DNS transport 读「当前网络」的状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkProbeSource {
    /// 由生成器按平台与模式确定性解析（`builder::network_env::resolve_probe_source`）。
    #[default]
    Auto,
    /// 系统 DNS（`dns-local`，type local）。
    System,
    /// 内核自发 DHCP DISCOVER（`dns-netenv`，type dhcp）。
    Dhcp,
}

/// 搜索域规范化：去首尾空白与首尾点、转小写；规范化后为空 ⇒ `None`。
#[must_use]
pub fn normalize_search_domain(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('.');
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
}

/// `networkProfiles` 的容错反序列化：**逐条**解析，坏条目（类型不对、未知 probe 值…）与缺 id 的条目
/// 丢弃，整份 `UserConfig` 照常读出；键的值不是数组时视为空。
///
/// 丢弃是 fail-closed 的：引用被丢条目的规则找不到场景 ⇒ 按「引用失效」不生成（spec §3.4-2）。
pub fn deserialize_network_profiles<'de, D>(
    deserializer: D,
) -> Result<Vec<NetworkProfile>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(items) = raw else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<NetworkProfile>(item).ok())
        .filter(|profile| !profile.id.trim().is_empty())
        .collect())
}

#[cfg(test)]
mod tests;

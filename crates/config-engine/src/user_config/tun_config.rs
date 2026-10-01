//! TUN 配置类型 + Windows 接口名解析 + FakeIP 段常量。
//!
//! 上游 `shared/types.ts TunModeConfig` + `shared/tun-interface.ts` + `shared/fakeip-filter.ts` 合并移植。

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::user_config::neighbor::TunMacFilterMode;

/// FakeIP IPv4 段（benchmarking 保留）。上游 `FAKEIP_INET4_RANGE`。
pub const FAKEIP_INET4_RANGE: &str = "198.18.0.0/15";

/// FakeIP IPv6 段。上游 `FAKEIP_INET6_RANGE`。
pub const FAKEIP_INET6_RANGE: &str = "2001:2::/48";

/// Windows TUN 接口名（缺省）。与外部 sing-box 默认 tun0 / 其它 VPN 网卡区分。
/// issue #327：起核后「适配器真建出来了没」正向探测的锚定名，须与探测侧同源
///（`polaris-` 前缀落在 `polaris_helper::platform::windows::wintun::PROBE_PREFIXES` 的可枚举面内，
/// 用户改成自定义名时探测转「不可断言」而非误判失败）。
pub const WIN_TUN_INTERFACE: &str = "polaris-tun0";

/// UDP NAT 类型档（用户语汇）。缺席 = 跟随内核默认。
///
/// # 为什么是「一个 NAT 类型档」，而不是把 `udp_mapping` / `udp_filtering` 两个原始字段各配一个控件
///
/// 内核那两个字段各有 3 个取值 ⇒ 9 种组合，其中**只有 4 种对应真实存在的 NAT 语义**（RFC 3489 的
/// 三种锥形 + 对称），其余 5 种是「过滤比映射还松」之类的无意义组合 —— 逐字段暴露等于把 5 个必然
/// 无意义的格子摆到用户面前，而用户认得的词是「NAT 类型」，不是 `udp_mapping`。这与本页既有的
/// `macFilterMode` 同形：那也是**一个**下拉（关闭/仅允许/排除）映射到内核**两个**互斥字段
/// (`include_mac_address` / `exclude_mac_address`)，不是把两个清单并排摆出来。
///
/// 另一条路（逐字段暴露）唯一的好处是「和内核 schema 一一对应、日后加值不用改 UI」；本页并不追求
/// 这条 —— `mtu` 已经是「留空即自动」，`macFilterMode` 已经是一颗下拉映射两个字段，语汇一直是
/// **意图级**而非字段级。
///
/// # 为什么没有「对称 NAT」档
///
/// 对称（mapping = `address_and_port_dependent`）比端口受限锥更严，但它对**本机出网**没有额外收益：
/// 端口受限锥已经把「只接受曾发过包的地址+端口」这条收紧做满，对称多出来的那一半是「每个目的地换一个
/// 源端口」，代价是彻底堵死一切打洞、收益只在「防端口预测」这种本机 TUN 上不存在的威胁模型里。
/// YAGNI：没有真实场景就不加档（加档的成本是 5 语文案 + 一格映射表 + 一条用户永远选错的路）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UdpNatType {
    /// 全锥：任何远端都能往回发（打洞最容易）。
    FullCone,
    /// 受限锥：只接受**曾发过包的地址**来的包。
    RestrictedCone,
    /// 端口受限锥：只接受**曾发过包的地址+端口**来的包（本档里最严，P2P/联机最容易失败）。
    PortRestrictedCone,
}

/// TUN 模式配置（上游 `TunModeConfig`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunModeConfig {
    /// TUN MTU。**`None` = 自动 = 交给内核**：生成期**不下发** `mtu` 键，由 sing-box 自己取默认。
    ///
    /// # 为什么 Polaris 不维护平台 → MTU 表（陈先生 2026-09-25 决策，覆盖 09-14「按平台取值」裁定）
    ///
    /// sing-box 1.15.0-alpha.3 起 TUN `stack` 弃用，Polaris 一律走 sing-tun 自有栈（不发 `stack`），
    /// 此前「栈 × 平台」的三档默认随自变量一起失去依据。上游 `protocol/tun/inbound.go`
    /// （v1.15.0-alpha.8 :117-127）在 `options.MTU == 0` 时按运行环境自己取值（Network Extension /
    /// Android / 其余各一档），内核是这张表唯一的持有者 —— Polaris 再抄一份只会在上游改值时静默分叉。
    /// 故缺席即**不发键**（`singbox::Inbound::mtu` 为 `None` 时 `skip_serializing_if` 略去），
    /// 语义与上游的 `MTU == 0` 相同。具体数值以上游源码为准，本仓不复述。
    ///
    /// 桌面此前显式发 65535，与上游桌面默认同值 ⇒ 改为不发键在桌面上行为中性。
    /// 桌面新栈吞吐实测（2026-09-13，65535 最优）见 vault
    /// `polaris/design/polaris-tun-go-stack-mtu-benchmark-2026-09-13.md`。
    ///
    /// # 为什么是 `Option` 而不是「默认值 + 哨兵」
    ///
    /// 此前是 `u32` + 默认 1350，且 `builder/inbounds.rs` 把 **`Some(9000)` 当「未设置」**回落平台默认
    /// （上游 `singbox-inbounds-builder.ts:393` 的同款哨兵）。哨兵有两个后果：① 用户一旦真想要 9000，
    /// 会被静默改写成 1350/1400，属「设了但不生效」；② 默认值随平台/栈变化时，无从区分「持久化的 1350
    /// 是旧默认」还是「用户就要 1350」。`Option` 两者都没有：缺席即自动，在场即用户意图，逐字下发。
    ///
    /// 存量配置的 `mtu` 一律由 `polaris-store` 的 `migrate_tun_mtu` 清成缺席 —— 判据是**本项在此之前
    /// 从未有过 UI 入口**，故磁盘上的任何值都是程序写的默认，没有一个承载用户意图。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    // 🔴 不再有 `stack`：上游弃用 TUN stack 后 Polaris 一律走新栈（见 [`TunModeConfig::mtu`]）。
    // 磁盘 / 备份里遗留的 `tunConfig.stack`（任意值）靠 serde 默认的「忽略未知键」读得进来，
    // 并由 `polaris-store` 的 `migrate_tun_stack` 删掉；生成侧的 `singbox::Inbound` 也没有该字段。
    #[serde(default = "default_true", rename = "autoRoute")]
    pub auto_route: bool,
    #[serde(default = "default_true", rename = "strictRoute")]
    pub strict_route: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(rename = "inet4Address", skip_serializing_if = "Option::is_none")]
    pub inet4_address: Option<String>,
    #[serde(rename = "inet6Address", skip_serializing_if = "Option::is_none")]
    pub inet6_address: Option<String>,
    #[serde(rename = "macFilterMode", skip_serializing_if = "Option::is_none")]
    pub mac_filter_mode: Option<TunMacFilterMode>,
    #[serde(
        default,
        rename = "macFilterList",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub mac_filter_list: Vec<String>,
    #[serde(rename = "neighborDomains", skip_serializing_if = "Option::is_none")]
    pub neighbor_domains: Option<Vec<String>>,
    #[serde(
        rename = "inboundExcludeCidrs",
        skip_serializing_if = "Option::is_none"
    )]
    pub inbound_exclude_cidrs: Option<Vec<String>>,
    /// 「NAT 类型」档 → TUN inbound 的 `udp_mapping` × `udp_filtering`（映射表见
    /// `builder::inbounds::udp_nat_behaviors`）。
    ///
    /// **`None` = 两个键一个都不下发 = 跟随内核默认**（beta.7 上游默认两项均 `endpoint_independent`，
    /// 即全锥）。这条不变量是本项的默认安全性所在：存量配置与金样 `fixtures/config-snapshot.json`
    /// 零 delta，且「没设过 NAT 类型的用户」拿到的仍是打洞最容易的那一档。
    ///
    /// 与 `mac_filter_mode` 同形（`Option` 而非带 `Auto` 变体的枚举）：那颗下拉的「关闭」档同样落成
    /// `None`、同样不发键 —— 默认档的语义就是**不发**。
    #[serde(rename = "udpNatType", skip_serializing_if = "Option::is_none")]
    pub udp_nat_type: Option<UdpNatType>,
}

fn default_true() -> bool {
    true
}

impl Default for TunModeConfig {
    fn default() -> Self {
        Self {
            mtu: None,
            auto_route: true,
            strict_route: true,
            interface_name: None,
            inet4_address: None,
            inet6_address: None,
            mac_filter_mode: None,
            mac_filter_list: vec![],
            neighbor_domains: None,
            inbound_exclude_cidrs: None,
            udp_nat_type: None,
        }
    }
}

/// 解析 Windows TUN 接口名：尊重自定义（合法才用），否则回落 Polaris 专属名。
/// 上游 `resolveWinTunInterfaceName`。
pub fn resolve_win_tun_interface_name(interface_name: Option<&str>) -> String {
    let custom = interface_name.unwrap_or("").trim();
    // 仅字母数字/连字符/下划线、1-32 字符（Windows 接口名约束）。
    if !custom.is_empty()
        && custom.len() <= 32
        && custom
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return custom.to_string();
    }
    WIN_TUN_INTERFACE.to_string()
}

#[cfg(test)]
mod tests;

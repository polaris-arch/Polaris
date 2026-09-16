//! TUN 配置类型 + Windows 接口名解析 + FakeIP 段常量。
//!
//! 上游 `shared/types.ts TunModeConfig` + `shared/tun-interface.ts` + `shared/fakeip-filter.ts` 合并移植。

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::user_config::neighbor::TunMacFilterMode;

/// 用户未填 MTU（[`TunModeConfig::mtu`] 为 `None`）时 Polaris 显式下发的 TUN MTU —— **与栈、平台都无关**。
///
/// # 为什么不再按「栈 × 平台」取
///
/// sing-box 1.15.0-alpha.3 起 TUN `stack` 弃用：`protocol/tun/inbound.go:84` 只要 `stack` 非空（含 `"go"`）
/// 就报 `deprecated.OptionTunStack`，1.16 起 CLI 须 `ENABLE_DEPRECATED_TUN_STACK=true` 才继续认，1.17 删除。
/// Polaris 因此**不再下发 `stack`**，一律走 sing-tun 新栈（`stack.go`：`case "", "go": return NewGo(options)`）。
/// 此前的三档默认（gvisor+Windows 65535 / gvisor 其余 9000 / system·mixed 4064）全是**旧栈上的实测函数**，
/// 自变量没了，那张表也就失去依据，整体删除而不是改写。
///
/// # 为什么是 65535
///
/// 这是上游 `inbound.go` 在 `options.MTU == 0` 时给桌面平台的默认（Network Extension 取 4064、Android 取 9000，
/// 均不在 Polaris 的发行面内），即「不发 `mtu` 键」时新栈实际拿到的值。仍然**显式下发**而不是省略键：
/// 生成的配置里 `mtu` 恒在场，设置页占位符才有一个与内核一致的具体数可显示。渲染端副本是
/// `ui/src/domain/tun-mtu.ts` 的 `DEFAULT_TUN_MTU`，其 parity 测试逐字读本常量对拍。
///
/// # 判据（2026-09-13 实测，sing-box 1.15.0-alpha.3，Windows VM 207 + Linux VM 185）
///
/// 预登记三条判据（起核/适配器 MTU/无弃用提示；TCP 不塌陷；UDP 512B·1400B 5/5）两平台全过：
///
/// | 平台 | 新栈 / 65535 | 新栈 / 1350 | 当日 gvisor / 65535 | 当日 system / 4064 |
/// |---|---|---|---|---|
/// | Windows | **716 Mbps**（≈ 用户态参照 101%） | 174 | 413 | 322 |
/// | Linux | 2342（各档区间重叠，无区分力） | 2435 | 2316 | 2090 |
///
/// Windows 上新栈吞吐随 MTU 单调上升、65535 最高，旧 system 栈在 65535 塌到 11 Mbps 的缺陷不复现。
/// 完整数据与方法学缺口见 vault `polaris/design/polaris-tun-go-stack-mtu-benchmark-2026-09-13.md`。
///
/// **GSO 与 MTU 无关**：`protocol/tun/inbound.go` 构造期虽按 `mtu < 49152` 预判 GSO，但启动期只要存在
/// TCP `FlowOutbound`（`direct` 即是，Polaris 配置恒有）就强制 `GSO = true`；实测 49151 与 65535 两格
/// `ethtool -k` 均 `tcp-segmentation-offload: on`、吞吐无差异。macOS 未测（上一轮 Mac 测试床无区分力）。
///
/// # 其它平台
///
/// Polaris 只发行 darwin / win32 / linux，本常量对三者一视同仁。旧实现「未知平台 → system → 4064」的
/// 兜底是 system 栈的产物，栈不存在后没有保留依据，故不再保留平台分支。
pub const DEFAULT_TUN_MTU: u32 = 65535;

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
    /// TUN MTU。**`None` = 自动**（生成期取 [`DEFAULT_TUN_MTU`]）。
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
    // 🔴 不再有 `stack`：上游弃用 TUN stack 后 Polaris 一律走新栈（见 [`DEFAULT_TUN_MTU`]）。
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

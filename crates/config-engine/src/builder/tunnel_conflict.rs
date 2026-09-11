//! 本机**其它**隧道（Tailscale / 公司 VPN / ZeroTier …）与 Polaris 的冲突判定。
//!
//! # 判据面为什么比"直觉上的冲突"窄得多
//!
//! 直觉版是「外来隧道的网段落在 Polaris auto_route 的捕获面里 ⇒ 冲突」。
//! **那个判据会对几乎所有情况报警，且大多是假的**：auto_route 装的是 `0.0.0.0/1` + `128.0.0.0/1`，
//! 覆盖整个地址空间；而外来隧道装的是更具体的前缀（Tailscale 的 `100.64.0.0/10`、
//! 自建控制面的 `32.0.0.0/24`），**更具体的路由本来就赢**，流量照常走它。
//!
//! 一个逢隧道必报的告警，最后的下场是被无视或被删掉 —— `plat-warn` 的历史已经演过一遍。
//! 故本模块只认**真正有两个声索人**的三类，每一类都能说出"哪一侧会坏"：
//!
//! | 类别 | 为什么是真冲突 |
//! |---|---|
//! | [`ConflictKind::FakeIpOverlap`] | Polaris 把 FakeIP 段的地址当成"自己发出去的假 IP"反查域名。外来隧道若真的在用这一段，它的真实流量会被当成 FakeIP 处理 —— 两边都错 |
//! | [`ConflictKind::MeshOverlap`] | Polaris 自己的组网节点已经为该段发了 force-route 规则，与外来隧道**争同一段**。两个声索人，路由表上谁赢取决于装载顺序 |
//! | [`ConflictKind::TunAddressOverlap`] | 外来隧道的段与 Polaris TUN 自己的接口地址相交，属地址空间直接撞车 |
//!
//! **"未被排除"刻意不算一类**：那是配置建议（要不要把该段排出 TUN），不是故障。
//! 它的表达面是 A-2 的「本平台生效的排除网段」——那里给的是事实，用户自己判断要不要加。
//!
//! # 本模块只做判定，不读路由表
//!
//! 探测（枚举本机隧道接口与其前缀）是平台相关的宿主动作，住在 `system-integration`；
//! 本模块是纯函数，输入是探测结果。分开是为了让判据能在本机跑单测，
//! 而不必等三个平台的真机。
//!
//! # 判据段从哪来
//!
//! [`emitted_conflict_criteria`] 从**本次发射给内核的那一份配置**里读回 FakeIP 段与 TUN 地址，
//! 组网段走 [`mesh_forced_route_cidrs`]（它读不回来，见该函数文档的对照表）。
//! 接线（后台探测腿 + warn 日志 + 只读 command）在 `src-tauri` 的
//! `runtime::proxy::tunnel_conflict`。

#![forbid(unsafe_code)]

use crate::builder::endpoint_routes::{mesh_forced_route_cidrs, ObservedTailnetAddresses};
use crate::singbox::SingBoxConfig;
use crate::user_config::cidr::cidr_overlaps_any;
use crate::user_config::server_config::ServerConfig;

/// 探测到的一条外来隧道路由。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ForeignTunnelRoute {
    /// 接口名（`utun4` / `tailscale0` / …）。仅用于告知用户"是谁"，不参与判据。
    pub interface: String,
    /// 该接口宣告的前缀（CIDR）。
    pub prefix: String,
}

/// 冲突类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// 与 FakeIP 段相交。
    FakeIpOverlap,
    /// 与 Polaris 自己的组网 force-route 段相交（两个声索人争同一段）。
    MeshOverlap,
    /// 与 Polaris TUN 的接口地址相交。
    TunAddressOverlap,
}

/// 一条判定结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TunnelConflict {
    pub interface: String,
    pub prefix: String,
    pub kind: ConflictKind,
}

/// 判据输入。三组段都来自**当前这份生成配置**，不是猜的常量 ——
/// FakeIP 是否启用、组网节点是否 engaged、TUN 地址是什么，都随配置变。
pub struct ConflictInput<'a> {
    /// 探测到的外来隧道路由（已剔除 Polaris 自己的 TUN 接口）。
    pub foreign: &'a [ForeignTunnelRoute],
    /// 本次生成实际发射的 FakeIP 段（未启用 ⇒ 空）。
    pub fakeip_ranges: &'a [String],
    /// 本次生成实际发射的组网 force-route 段（无 engaged 组网节点 ⇒ 空）。
    pub mesh_cidrs: &'a [String],
    /// Polaris TUN 的接口地址段。
    pub tun_addresses: &'a [String],
}

/// 判出真冲突。同一条前缀可能同时命中多类，逐类各出一条（用户要知道全部原因）。
///
/// 顺序：按输入顺序，同一前缀内按 FakeIP → Mesh → TunAddress。稳定顺序是为了让
/// UI 与快照测试不因 HashMap 迭代顺序抖动。
#[must_use]
pub fn detect_tunnel_conflicts(input: &ConflictInput<'_>) -> Vec<TunnelConflict> {
    let mut out = Vec::new();
    for route in input.foreign {
        let prefix = route.prefix.trim();
        if prefix.is_empty() {
            continue;
        }
        for (ranges, kind) in [
            (input.fakeip_ranges, ConflictKind::FakeIpOverlap),
            (input.mesh_cidrs, ConflictKind::MeshOverlap),
            (input.tun_addresses, ConflictKind::TunAddressOverlap),
        ] {
            if !ranges.is_empty() && cidr_overlaps_any(prefix, ranges) {
                out.push(TunnelConflict {
                    interface: route.interface.clone(),
                    prefix: prefix.to_string(),
                    kind,
                });
            }
        }
    }
    out
}

// ── 三组判据段从哪来（判据由代码持有，不留第二份可漂的表）──────────────────────────────

/// [`ConflictInput`] 的三组段的取值，**全部来自本次实际发射的那一份配置**。
///
/// 拆成一个可携带的结构体，是因为判定发生在**探测回来之后**（探测是异步后台腿），
/// 而判据段必须是**发射那一刻**的值：核起来之后用户接着改配置、观测地址又多了一条，
/// 都不该让这次的判定拿到一份与内核吃的那份不同的段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictCriteria {
    /// 见 [`ConflictInput::fakeip_ranges`]。
    pub fakeip_ranges: Vec<String>,
    /// 见 [`ConflictInput::mesh_cidrs`]。
    pub mesh_cidrs: Vec<String>,
    /// 见 [`ConflictInput::tun_addresses`]。
    pub tun_addresses: Vec<String>,
}

impl ConflictCriteria {
    /// 配上一份探测结果，组成 [`detect_tunnel_conflicts`] 的入参。
    #[must_use]
    pub fn with_foreign<'a>(&'a self, foreign: &'a [ForeignTunnelRoute]) -> ConflictInput<'a> {
        ConflictInput {
            foreign,
            fakeip_ranges: &self.fakeip_ranges,
            mesh_cidrs: &self.mesh_cidrs,
            tun_addresses: &self.tun_addresses,
        }
    }
}

/// 从**本次发射的 sing-box 配置**里读回三组判据段。
///
/// # 为什么两组读回、一组重算
///
/// | 段 | 来源 | 理由 |
/// |---|---|---|
/// | `fakeip_ranges` | `emitted.dns.servers[type=="fakeip"]` 的 `inet4_range`/`inet6_range` | 这就是内核吃到的那两个值。FakeIP 开没开经过 v2 动作派生 / legacy `enableFakeIp` 两条腿，重算就是复制那段判定 |
/// | `tun_addresses` | `emitted.inbounds[type=="tun"]` 的 `address` | 同上：默认值随平台分叉（darwin `/30`、其它 `/16`），用户还可覆盖 |
/// | `mesh_cidrs` | [`mesh_forced_route_cidrs`]`(servers, observed)` | **读不回来**：组网段有两条发射腿 —— inline `ip_cidr` 路由规则，以及 Tailscale 的外化 `rule_set` 文件（`builder::route` 块 0c）。后者在配置 JSON 里只剩一个文件路径，而自建 tailnet 的观测地址恰恰走的是那一条 |
///
/// `servers` 传**本次生成用的那套**（内核闸门剥离之后的 `effective_user_config.servers`），
/// `observed` 传**喂给本次 `GenerateConfigDeps` 的那份快照**。传空或现取一份，自建 tailnet
/// 的真实前缀（实测 `32.0.0.0/24`）就进不了 `mesh_cidrs`，`MeshOverlap` 一条都判不出来 ——
/// 那正是 2026-09-11 那次事故的形态，也是 `tailnet_observed_address_force_route_gate` 的
/// `observed_addresses_reach_mesh_cidrs_union` 钉住的供给面。
///
/// # 本函数不放宽判据面
///
/// 它只负责**取值**，不新增类别。「外来隧道未被排除」仍然刻意不算冲突（理由见模块头注）。
#[must_use]
pub fn emitted_conflict_criteria(
    emitted: &SingBoxConfig,
    servers: &[ServerConfig],
    observed: &ObservedTailnetAddresses,
) -> ConflictCriteria {
    let fakeip_ranges = emitted
        .dns
        .as_ref()
        .map(|dns| {
            dns.servers
                .iter()
                .filter(|server| server.type_field.as_deref() == Some("fakeip"))
                .flat_map(|server| [server.inet4_range.clone(), server.inet6_range.clone()])
                .flatten()
                .collect()
        })
        .unwrap_or_default();
    let tun_addresses = emitted
        .inbounds
        .iter()
        .filter(|inbound| inbound.type_field == "tun")
        .filter_map(|inbound| inbound.address.clone())
        .flatten()
        .collect();
    ConflictCriteria {
        fakeip_ranges,
        mesh_cidrs: mesh_forced_route_cidrs(servers, observed),
        tun_addresses,
    }
}

/// 本次发射的配置里 **Polaris 自己的** TUN 接口名。
///
/// 喂 `foreign_tunnel_routes` 的 `own_interfaces`：不剔除自己，整张自有路由表都会被当成
/// "别人的隧道"，与自己的 mesh/FakeIP 段一比就冒出一堆自指告警。
///
/// **macOS 上这里恒空**（内核分配 `utunN`，config-engine 不设 `interface_name`）——
/// 那条腿的自我识别只能靠运行期观测到的出口接口别名，由调用方补。
#[must_use]
pub fn emitted_tun_interface_names(emitted: &SingBoxConfig) -> Vec<String> {
    emitted
        .inbounds
        .iter()
        .filter(|inbound| inbound.type_field == "tun")
        .filter_map(|inbound| inbound.interface_name.clone())
        .collect()
}

#[cfg(test)]
mod tests;

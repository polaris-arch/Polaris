//! Windows 外来隧道探测的**进程内**取材面：注入缝（trait）+ 纯组装。
//!
//! # 为什么要有这一层（D1）
//!
//! [`crate::route_probe::ForeignTunnelProbeImpl`] 的 Windows 文本腿要跑**六个串行外部进程**
//! （`route.exe print -4/-6` + 四条 PowerShell `Get-NetIPAddress`/`Get-NetAdapter`/
//! `Get-VpnConnection`×2，各 5s 预算）。起核峰值窗口里 PowerShell 冷启动跑不完 ⇒ 整次探测
//! 超时 ⇒ 用户拿不到冲突提示。进程内 IP Helper / RAS API 把这六个进程压成三次系统调用
//! （空闲实测 `GetIpForwardTable2` ≤3ms）。
//!
//! # 为什么 FFI 不在本 crate（依赖成环，非风格取向）
//!
//! `crates/helper/Cargo.toml` 在 **macOS 下 helper 依赖本 crate**（root helper 复用同一份
//! SystemConfiguration 事务实现）。若本 crate 反过来调 `polaris_helper` 的 netinfo，macOS
//! 目标上就是一个环。故分三段：
//!
//! | 段 | 住哪 | 内容 |
//! |---|---|---|
//! | FFI | `polaris_helper::platform::windows::netinfo` | `GetIpForwardTable2` / `GetAdaptersAddresses` / `RasEnumConnectionsW` |
//! | 抽象 + 判据 | **本模块** | [`netinfo::WindowsNetInfoSource`] trait（纯签名，零 `windows-sys`）+ [`netinfo::assemble_windows_netinfo_probe`] 纯组装 |
//! | 注入 | `src-tauri` `runtime::proxy::platform_contracts` | impl trait，调 helper netinfo（先例：`enumerate_own_lan_cidrs` 的 Windows 腿已这么调） |
//!
//! 本 crate 由此保持 `windows-sys` 零依赖、`deny(unsafe_code)` 无破例。
//!
//! # 三腿失败的分界（不是一刀切「失败即空」）
//!
//! | 腿 | 失败 | 理由 |
//! |---|---|---|
//! | 路由表 | **整次探测 `Err`** | 没有路由表就没有任何事实。空结果会被下游读成「没有外来隧道」——[`crate::route_probe::TunnelProbeOutcome`] 分支存在的全部理由就是不让这件事发生 |
//! | 适配器 | **整次探测 `Err`** | 空名单 = 「这台机器上没有隧道」。与 [`crate::route_probe::parse_windows_tunnel_interfaces`] 对空名单必须报错同一条纪律 |
//! | RAS | **该腿空 + 调用方记 warn** | 「这台机器没有 VPN 连接」本来就是常态结果（与 [`crate::route_probe::parse_vpn_connection_names`] 的空输出同义）。文本腿正因为把这条也算失败，在没有 `VpnClient` 模块的 SKU 上整次探测退化成 `Err` —— 那是已知缺陷，不复刻 |

use thiserror::Error;

use super::{foreign_tunnel_routes, ForeignRoutes, ForeignTunnelSnapshot, RouteEntry};

/// 一张适配器的**类型**信息（`GetAdaptersAddresses` 的 `FriendlyName` + IANA `IfType`）。
///
/// 与文本腿的取材面对应关系：`alias` = `Get-NetAdapter` 的 `InterfaceAlias` 列，
/// `if_type` = `InterfaceType` 列。判据（[`super::WINDOWS_TUNNEL_IF_TYPES`]）两条腿共用一份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsAdapterKind {
    /// 接口别名，与 [`RouteEntry::interface`] 同一个命名空间（join 就靠它逐字相等）。
    pub alias: String,
    /// IANA ifType。
    pub if_type: u32,
}

/// 一条**活动的** RAS / VPN 连接（`RasEnumConnections` 的一行）。
///
/// # 与文本腿的两处语义差异（都是改善，但都需真机确认）
///
/// 1. **不需要状态过滤**：`Get-VpnConnection` 列的是配置（`Disconnected` 的连接照样在表里，
///    故文本腿必须按 `ConnectionStatus == Connected` 过滤），而 `RasEnumConnections` 枚举的
///    本就只有已建立的连接。
/// 2. **一次覆盖两个作用域**：文本腿要跑 `Get-VpnConnection` 与 `-AllUserConnection` 两条命令，
///    RAS API 一次枚举即覆盖，作用域由 [`Self::all_users`] 如实标注（Q8）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRasConnection {
    /// 连接名。连接建立后，承载流量那个接口的别名逐字就是它（w207 实测 `PolarisProbeL2TP`，
    /// ifIndex 35）—— 这也是它能与路由表的 `interface` 直接比的原因。
    pub name: String,
    /// 是不是「所有用户」作用域（`RASCONN.dwFlags` 含 `RASCF_AllUsers`）。
    ///
    /// **不作过滤判据**：两个作用域的连接都要报。它只用于日志里如实说明覆盖面 ——
    /// 少了这条，「一次枚举真的覆盖了 all-user 吗」在真机上无从对账。
    pub all_users: bool,
}

/// Windows 进程内网络信息源（三条只读枚举）。由 `src-tauri` 注入实现。
///
/// 三条都是**只读**枚举：不改路由 / 网卡 / DNS 任何状态，且都在 app 的用户会话作用域里
/// 免提权执行（与 helper 的 SYSTEM 身份无关 —— 住在 helper crate 只是因为 FFI 得住在
/// 有 `windows-sys` 的地方）。
pub trait WindowsNetInfoSource: Send + Sync {
    /// 内核路由表（IPv4 + IPv6），接口名已解成别名。
    ///
    /// # Errors
    ///
    /// 枚举失败的诊断串。**不得**折成空表（空表 = 「没有外来隧道」）。
    fn routes(&self) -> Result<Vec<RouteEntry>, String>;

    /// 全部适配器的别名 + ifType（隧道判据的取材面）。
    ///
    /// # Errors
    ///
    /// 同上，不得折成空表。
    fn adapters(&self) -> Result<Vec<WindowsAdapterKind>, String>;

    /// 活动的 RAS / VPN 连接。
    ///
    /// # Errors
    ///
    /// 枚举失败的诊断串。调用方对**本腿**的失败降级为空表（见模块头注的三腿分界表）。
    fn ras_connections(&self) -> Result<Vec<WindowsRasConnection>, String>;
}

/// 进程内取材面塌了的两种形态。**都不折成空快照**。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WindowsNetInfoError {
    /// 路由表一条都没有。内核路由表最少也有回环那几条，零条 = 读法塌了。
    #[error(
        "GetIpForwardTable2 返回 0 条路由 —— 一张空路由表会被读成「没有外来隧道」，故按失败报"
    )]
    EmptyRouteTable,
    /// 适配器一张都没有。同上，且空名单恰好长得像「这台机器上没有隧道」。
    #[error(
        "GetAdaptersAddresses 返回 0 张适配器 —— 空的隧道名单会被读成「这台机器上没有隧道」，故按失败报"
    )]
    EmptyAdapterTable,
}

/// 隧道接口名单 = ifType 白名单命中的适配器 ∪ 活动 RAS 连接名（**纯函数**）。
///
/// 两个来源互不覆盖，少任何一边都是一族真隧道静默失联：
///
/// - ifType 白名单（[`super::WINDOWS_TUNNEL_IF_TYPES`]）覆盖协议隧道（`131`）与用户态 VPN
///   虚拟网卡（`53`：wintun / TAP-Windows6）；
/// - RAS 族（L2TP / IKEv2 / SSTP / PPTP）连接建立后，承载流量的接口**根本不在**适配器枚举里
///   （w207 实测 ifIndex 35 在 `Get-NetAdapter -IncludeHidden` 里返回 0 条），只能问 RAS API。
///
/// 去重按名字：同一条连接可能既在适配器枚举里、又在 RAS 枚举里，重复名字会让 `foreign` 里
/// 同一条路由出现两遍（与文本腿 [`super::assemble_windows_probe`] 的去重同因）。
#[must_use]
pub fn windows_tunnel_interfaces(
    adapters: &[WindowsAdapterKind],
    ras_connections: &[WindowsRasConnection],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for adapter in adapters {
        if super::WINDOWS_TUNNEL_IF_TYPES.contains(&adapter.if_type)
            && !adapter.alias.is_empty()
            && !out.contains(&adapter.alias)
        {
            out.push(adapter.alias.clone());
        }
    }
    for conn in ras_connections {
        if !conn.name.is_empty() && !out.contains(&conn.name) {
            out.push(conn.name.clone());
        }
    }
    out
}

/// 路由表里**接口别名对不上任何适配器 / RAS 连接**的条数（**纯函数**）。
///
/// # 它守的是这条链最隐蔽的一种失效
///
/// 三条 API 的结果靠**别名逐字相等**join：路由的别名来自 `ConvertInterfaceLuidToAlias`，
/// 隧道名单的别名来自 `GetAdaptersAddresses.FriendlyName` 与 `RasEnumConnections.szEntryName`。
/// 两侧若不是同一个命名空间（本机 Linux **判不了**，列真机项），join 全落空 ——
/// 而落空的症状不是报错，是 `foreign` 为空、界面给出一句自信的「无冲突」。
///
/// **正常值不是 0**：`Loopback Pseudo-Interface 1` 这类隐藏接口在 `Get-NetAdapter` 里就没有
/// （w207 实测），它们的路由自然对不上。有信息量的是**全部**对不上 —— 调用方据此出声。
#[must_use]
pub fn routes_with_unknown_interface(
    routes: &[RouteEntry],
    adapters: &[WindowsAdapterKind],
    ras_connections: &[WindowsRasConnection],
) -> usize {
    routes
        .iter()
        .filter(|route| {
            !adapters.iter().any(|a| a.alias == route.interface)
                && !ras_connections.iter().any(|c| c.name == route.interface)
        })
        .count()
}

/// 三腿枚举结果 → 探测事实（**纯函数**，与文本腿 [`super::assemble_windows_probe`] 同一出口）。
///
/// 下游（[`foreign_tunnel_routes`] → [`ForeignTunnelSnapshot`] →
/// `config_engine::builder::tunnel_conflict::detect_tunnel_conflicts`）一个字都不改：
/// 本函数的全部职责就是把三条 API 的结果拼成与文本解析器**逐字相同**的中间结构。
///
/// # Errors
///
/// 见 [`WindowsNetInfoError`]：路由表或适配器表为空即整体失败，不产出半份事实。
/// RAS 那一腿的空是**结果**不是失败（绝大多数机器一条 VPN 连接都没有），故它不出现在错误里。
pub fn assemble_windows_netinfo_probe(
    routes: &[RouteEntry],
    adapters: &[WindowsAdapterKind],
    ras_connections: &[WindowsRasConnection],
    own_interfaces: &[String],
) -> Result<ForeignTunnelSnapshot, WindowsNetInfoError> {
    if routes.is_empty() {
        return Err(WindowsNetInfoError::EmptyRouteTable);
    }
    if adapters.is_empty() {
        return Err(WindowsNetInfoError::EmptyAdapterTable);
    }
    let tunnel_interfaces = windows_tunnel_interfaces(adapters, ras_connections);
    let ForeignRoutes {
        foreign,
        default_routes,
    } = foreign_tunnel_routes(routes, &tunnel_interfaces, own_interfaces);
    Ok(ForeignTunnelSnapshot {
        tunnel_interfaces,
        foreign,
        default_routes,
    })
}

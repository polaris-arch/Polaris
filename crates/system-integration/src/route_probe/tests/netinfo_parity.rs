//! 进程内取材腿（[`netinfo`]）与文本解析腿的**对照**，取材面是同一批真机抓取。
//!
//! # 这组能证明什么、不能证明什么（如实登记，别把它当成「D1 已验」）
//!
//! **能证明**：两条腿的**判据与组装**逐字一致 —— ifType 白名单命中面、RAS 名单并入、按名去重、
//! `own_interfaces` 剔除、默认路由分桶。把 [`netinfo::windows_tunnel_interfaces`] 的任一条改坏，
//! 本组红。
//!
//! **不能证明**：FFI 读回来的东西对不对。这三条只有 207 真机能验（spec §5.5）——
//!
//!  1. `GetIpForwardTable2(AF_UNSPEC)` 返回的前缀与 `route print -4/-6` 的活动路由表是否同一批；
//!  2. `ConvertInterfaceLuidToAlias` 给出的别名与 `Get-NetIPAddress` 的 `InterfaceAlias`
//!     是否**逐字同串**（不同串 ⇒ 路由与隧道名单 join 不上 ⇒ 一句自信的「无冲突」）；
//!  3. `RasEnumConnections` 是否真的一次覆盖 all-user 作用域（Q8），以及 RasMan 服务被禁用 /
//!     组策略受限时它的返回码（**不是**「没装 rasapi32 的 SKU」—— 那是导入表符号缺失，
//!     进程根本加载不起来，走不到降级腿，见 `polaris_helper::…::netinfo::NetInfoError` 头注）。
//!
//! # 为什么不能只比「两条腿相等」
//!
//! 两条腿在本组里都是从**同一份夹具**派生的：夹具被改坏，两边一起变、一起绿。故每份抓取另有
//! 一组**正面断言**（那台机器上到底该看见哪条隧道、哪条网段落在哪个桶里），夹具一坏它们就红。

use super::super::netinfo::{
    self, WindowsAdapterKind, WindowsNetInfoError, WindowsNetInfoSource, WindowsRasConnection,
};
use super::super::{
    assemble_windows_probe, parse_get_netipaddress, parse_route_print_routes, ForeignTunnelProbe,
    ForeignTunnelProbeImpl, ForeignTunnelSnapshot, FormatTable, RouteEntry, TunnelProbeOutcome,
};
use super::fixture_harness;
use polaris_helper_proto::Platform;
use std::sync::Arc;

/// 2026-09-12 w207：Tailscale 1.102.4 已装、wintun 在（ifType `53`）。
const WIN_WINTUN: &str = "windows-w207-wintun-present-2026-09-12.txt";
/// 2026-09-12 w207：wintun 已登录，30 条业务路由在它头上。
const WIN_TS_ON: &str = "windows-w207-ts-on-2026-09-12.txt";
/// 2026-09-13 w207：内置 L2TP 已连接，承载接口 `PolarisProbeL2TP` 抢默认路由。
const WIN_RAS: &str = "windows-w207-ras-l2tp-connected-2026-09-13.txt";
/// 2026-09-13 w207：OpenVPN TAP-Windows6 已连接（ifType 也是 `53`）。
const WIN_OVPN: &str = "windows-w207-ovpn-tun-connected-2026-09-13.txt";
/// 2026-09-13 w207：Hyper-V 虚拟交换机在（负样本，报 `6` 不是 `53`）。
const WIN_HYPERV: &str = "windows-w207-hyperv-present-2026-09-13.txt";
/// 2026-09-12 w207：Tailscale 装之前的对照。
const WIN_TS_OFF: &str = "windows-w207-routes-ts-off-2026-09-12.txt";
/// 2026-09-13 w207：虚拟化栈全开。
const WIN_VIRT: &str = "windows-w207-virtualization-stack-2026-09-13.txt";

/// 本组覆盖的全部 Windows 抓取。**逐字枚举而不是扫目录**：新抓取入库时这里不会自动跟上，
/// 由 `fixture_harness` 的 `windows_fixture_names` 那道门负责喊 —— 那条门数的是文件，
/// 本组数的是判据，两者不该互相顶替。
const ALL_WINDOWS_FIXTURES: [&str; 7] = [
    WIN_WINTUN, WIN_TS_ON, WIN_RAS, WIN_OVPN, WIN_HYPERV, WIN_TS_OFF, WIN_VIRT,
];

fn section(file: &str, id: &str) -> String {
    fixture_harness::section(&fixture_harness::read_fixture(file), id)
}

fn optional_section(file: &str, id: &str) -> String {
    fixture_harness::section_opt(&fixture_harness::read_fixture(file), id).unwrap_or_default()
}

/// 文本腿（六段 stdout → 快照）—— 对照基准。
fn text_leg(file: &str, own_interfaces: &[String]) -> ForeignTunnelSnapshot {
    assemble_windows_probe(
        &section(file, "ROUTE_PRINT_4"),
        &section(file, "ROUTE_PRINT_6"),
        &section(file, "GET_NETIPADDRESS"),
        &section(file, "GET_NETADAPTER"),
        &optional_section(file, "GET_VPNCONNECTION"),
        &optional_section(file, "GET_VPNCONNECTION_ALLUSER"),
        own_interfaces,
    )
    .unwrap_or_else(|e| panic!("{file}：文本腿应解析得动，实际 {e}"))
}

/// 把同一份抓取**改写成三条 FFI 会给出的形态**。
///
/// 🔴 这一步只做**形态转换**，不重实现判据：路由那腿直接用文本解析器的产物（`RouteEntry` 本就是
/// 「前缀 + 接口别名」，与 `GetIpForwardTable2` + `ConvertInterfaceLuidToAlias` 的产物同形），
/// 适配器与 RAS 两腿只从 `Format-Table` 里取列。判据（ifType 白名单 / 并名单 / 分桶）一条都不在这里，
/// 全在被测的 [`netinfo::assemble_windows_netinfo_probe`] 里 —— 否则这组对照就成了自证。
fn netinfo_inputs(
    file: &str,
) -> (
    Vec<RouteEntry>,
    Vec<WindowsAdapterKind>,
    Vec<WindowsRasConnection>,
) {
    let names = parse_get_netipaddress(&section(file, "GET_NETIPADDRESS"))
        .unwrap_or_else(|e| panic!("{file}：对照表应解析得动，实际 {e}"));
    let mut routes = parse_route_print_routes(&section(file, "ROUTE_PRINT_4"), &names)
        .unwrap_or_else(|e| panic!("{file}：v4 路由表应解析得动，实际 {e}"));
    routes.extend(
        parse_route_print_routes(&section(file, "ROUTE_PRINT_6"), &names)
            .unwrap_or_else(|e| panic!("{file}：v6 路由表应解析得动，实际 {e}")),
    );
    (routes, adapter_kinds(file), ras_connections(file))
}

/// `@@@GET_NETADAPTER` 的两列 → `GetAdaptersAddresses` 会给出的 (别名, ifType)。
fn adapter_kinds(file: &str) -> Vec<WindowsAdapterKind> {
    let stdout = section(file, "GET_NETADAPTER");
    let table = super::super::parse_format_table(&stdout, &["InterfaceAlias", "InterfaceType"])
        .unwrap_or_else(|| panic!("{file}：@@@GET_NETADAPTER 认不出列头"));
    let alias_idx = table
        .column("InterfaceAlias")
        .expect("列头里有 InterfaceAlias");
    let type_idx = table
        .column("InterfaceType")
        .expect("列头里有 InterfaceType");
    table
        .rows
        .iter()
        .filter_map(|row| {
            let alias = FormatTable::cell(row, alias_idx)?;
            let if_type = FormatTable::cell(row, type_idx)?.parse::<u32>().ok()?;
            Some(WindowsAdapterKind {
                alias: alias.to_string(),
                if_type,
            })
        })
        .collect()
}

/// 两段 `@@@GET_VPNCONNECTION*` → `RasEnumConnections` 会给出的活动连接。
///
/// **状态列在这里过滤**：`RasEnumConnections` 枚举的本就只有已建立的连接，而 `Get-VpnConnection`
/// 列的是配置 —— 把 `Disconnected` 的行折进来，派生出来的输入就不是 RAS API 会给的那份。
/// 作用域顺序（当前用户在前、all-user 在后）与文本腿的并入顺序一致，否则 `tunnel_interfaces`
/// 的**顺序**对不上，等值断言会因为一个与判据无关的差异而红。
fn ras_connections(file: &str) -> Vec<WindowsRasConnection> {
    let mut out: Vec<WindowsRasConnection> = Vec::new();
    for (id, all_users) in [
        ("GET_VPNCONNECTION", false),
        ("GET_VPNCONNECTION_ALLUSER", true),
    ] {
        let stdout = optional_section(file, id);
        if stdout.trim().is_empty() {
            continue;
        }
        let Some(table) = super::super::parse_format_table(&stdout, &["Name", "ConnectionStatus"])
        else {
            continue;
        };
        let (Some(name_idx), Some(status_idx)) =
            (table.column("Name"), table.column("ConnectionStatus"))
        else {
            continue;
        };
        for row in &table.rows {
            let (Some(name), Some(status)) = (
                FormatTable::cell(row, name_idx),
                FormatTable::cell(row, status_idx),
            ) else {
                continue;
            };
            if status != super::super::VPN_CONNECTED_STATUS || name.is_empty() {
                continue;
            }
            if out.iter().any(|c| c.name == name) {
                continue;
            }
            out.push(WindowsRasConnection {
                name: name.to_string(),
                all_users,
            });
        }
    }
    out
}

fn netinfo_leg(file: &str, own_interfaces: &[String]) -> ForeignTunnelSnapshot {
    let (routes, adapters, ras) = netinfo_inputs(file);
    netinfo::assemble_windows_netinfo_probe(&routes, &adapters, &ras, own_interfaces)
        .unwrap_or_else(|e| panic!("{file}：进程内腿应组装得出，实际 {e}"))
}

// ══════════════════════════ 对照：七份真机抓取，两条腿必须逐字相同 ══════════════════════════

/// 🔴 **同一份抓取，进程内腿与文本腿给出逐字相同的快照**（七份全覆盖）。
///
/// 三个字段全比（`tunnel_interfaces` / `foreign` / `default_routes`），含**顺序** ——
/// `foreign` 的顺序决定设置页里那张表的行序，两条腿换出不同的序就不是「同一份事实」。
///
/// 前提断言（缺了它这条对照可能只是「两边都是空的」）：每份抓取都至少解析出一条路由。
#[test]
fn the_in_process_leg_matches_the_text_leg_on_every_windows_capture() {
    for file in ALL_WINDOWS_FIXTURES {
        let text = text_leg(file, &[]);
        let ffi = netinfo_leg(file, &[]);
        let (routes, adapters, _) = netinfo_inputs(file);
        assert!(
            !routes.is_empty() && !adapters.is_empty(),
            "{file}：派生出来的取材面是空的 —— 下面那条等值断言没有信息量"
        );
        assert_eq!(
            ffi, text,
            "{file}：进程内腿与文本腿给出的探测事实不同 —— 判据在某一侧漂了"
        );
    }
}

/// 🔴 **`own_interfaces` 剔除在两条腿上同样生效**（且剔的是两个桶）。
///
/// 正面断言在前：不传 `own` 时 `Tailscale` 确实有路由；传了之后两个桶里都没有它。
/// 只写后半句的话，「压根没接上」会伪装成「剔干净了」。
#[test]
fn own_interface_removal_matches_on_both_legs() {
    let own = vec!["Tailscale".to_string()];
    let before = netinfo_leg(WIN_TS_ON, &[]);
    assert!(
        before.foreign.iter().any(|r| r.interface == "Tailscale"),
        "前提：未剔除时 Tailscale 应有业务路由：{:?}",
        before.foreign
    );
    let after = netinfo_leg(WIN_TS_ON, &own);
    assert_eq!(after, text_leg(WIN_TS_ON, &own), "剔除后两条腿仍须一致");
    assert!(
        !after.foreign.iter().any(|r| r.interface == "Tailscale")
            && !after
                .default_routes
                .iter()
                .any(|r| r.interface == "Tailscale"),
        "自己的隧道没被剔干净 —— 自指告警会与真告警混在一起：{after:?}"
    );
    assert!(
        after.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
        "`tunnel_interfaces` 是原始事实（本机有哪些隧道），剔除只作用在两个路由桶上"
    );
}

// ══════════════════════════ 正面断言：夹具被改坏时这些会红 ══════════════════════════

/// 🔴 **wintun（ifType `53`）进隧道名单**。判据只认 `131`（旧口径）时它会漏 —— 而它正是
/// 这条链唯一真要防的那一个。
#[test]
fn the_in_process_leg_sees_the_wintun_adapter() {
    for file in [WIN_WINTUN, WIN_TS_ON, WIN_RAS, WIN_OVPN] {
        let snapshot = netinfo_leg(file, &[]);
        assert!(
            snapshot.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
            "{file}：wintun 适配器 `Tailscale` 不在隧道名单里：{:?}",
            snapshot.tunnel_interfaces
        );
    }
    // 连接态那份上，业务网段真的落到了它头上（名单对了、路由没接上同样是失败）。
    let on = netinfo_leg(WIN_TS_ON, &[]);
    assert!(
        on.foreign
            .iter()
            .any(|r| r.interface == "Tailscale" && r.prefix == "100.100.100.100/32"),
        "已登录 wintun 的业务网段没进 foreign：{:?}",
        on.foreign
    );
}

/// 🔴 **RAS 承载接口（`Get-NetAdapter` 里根本没有它）进名单，且那条 `0.0.0.0/0` 落在
/// `default_routes` 而不是 `foreign`**。
///
/// 这一条是 RAS 腿存在的全部理由：`RasEnumConnections` 缺席时，装着企业 VPN 的机器又会拿到
/// 一句自信的「无冲突」。
#[test]
fn the_in_process_leg_sees_the_connected_ras_tunnel() {
    const RAS_IFACE: &str = "PolarisProbeL2TP";
    let snapshot = netinfo_leg(WIN_RAS, &[]);
    assert!(
        snapshot.tunnel_interfaces.iter().any(|t| t == RAS_IFACE),
        "已连接的 L2TP 隧道不在名单里：{:?}",
        snapshot.tunnel_interfaces
    );
    assert!(
        snapshot
            .foreign
            .iter()
            .any(|r| r.interface == RAS_IFACE && r.prefix == "10.55.0.10/32"),
        "RAS 隧道自身地址没进 foreign：{:?}",
        snapshot.foreign
    );
    assert!(
        !snapshot.foreign.iter().any(|r| r.prefix == "0.0.0.0/0"),
        "默认路由并进了 foreign —— 它与任何前缀相交，会把告警刷爆：{:?}",
        snapshot.foreign
    );
    assert!(
        snapshot
            .default_routes
            .iter()
            .any(|r| r.interface == RAS_IFACE && r.prefix == "0.0.0.0/0"),
        "那条抢默认路由的全隧道两个桶里都没有：{:?}",
        snapshot.default_routes
    );
}

/// 🔴 **反向对照：RAS 腿空掉，那条隧道就整条消失**。
///
/// 证明上一条里的 `PolarisProbeL2TP` 确实是**由 RAS 腿**带进来的，而不是恰好也被 ifType
/// 白名单收了（若是后者，RAS 腿就是可有可无的，而真机实测它在 `Get-NetAdapter` 里返回 0 条）。
#[test]
fn dropping_the_ras_leg_makes_the_l2tp_tunnel_disappear() {
    const RAS_IFACE: &str = "PolarisProbeL2TP";
    let (routes, adapters, ras) = netinfo_inputs(WIN_RAS);
    assert!(
        ras.iter().any(|c| c.name == RAS_IFACE),
        "前提：派生出来的 RAS 名单里应有它：{ras:?}"
    );
    let without_ras = netinfo::assemble_windows_netinfo_probe(&routes, &adapters, &[], &[])
        .expect("只少 RAS 一腿，另外两腿照常");
    assert!(
        !without_ras.tunnel_interfaces.iter().any(|t| t == RAS_IFACE),
        "RAS 腿空了它却还在名单里 —— 说明它是被别的判据收的，上一条对照没有信息量"
    );
    assert!(
        !without_ras
            .default_routes
            .iter()
            .any(|r| r.interface == RAS_IFACE),
        "RAS 腿空了，那条 0.0.0.0/0 就不该再被认成外来隧道的宣告"
    );
    // 另一半：别的隧道不能被这一腿的缺席连累。
    assert!(
        without_ras
            .tunnel_interfaces
            .iter()
            .any(|t| t == "Tailscale"),
        "RAS 腿缺席把 ifType 那一族也连累了：{:?}",
        without_ras.tunnel_interfaces
    );
}

/// 🔴 **变异对照：把 wintun 的 ifType 从 `53` 改成 `6`（真业务网卡的值），它必须整条消失**。
///
/// 变异先证明打上（改动处数 == 1），否则红绿都不算数。这条钉的是「白名单真的在判」——
/// 只比两条腿相等的话，判据整个失效（比如恒收）时两边会一起错、一起绿。
#[test]
fn mutating_the_wintun_iftype_removes_it_from_the_tunnel_list() {
    let (routes, mut adapters, ras) = netinfo_inputs(WIN_TS_ON);
    let before = netinfo::assemble_windows_netinfo_probe(&routes, &adapters, &ras, &[])
        .expect("未变异时应组装得出");
    assert!(
        before.tunnel_interfaces.iter().any(|t| t == "Tailscale")
            && before.foreign.iter().any(|r| r.interface == "Tailscale"),
        "前提：未变异时它在名单里且有路由"
    );

    let mut applied = 0usize;
    for adapter in &mut adapters {
        if adapter.alias == "Tailscale" {
            assert_eq!(adapter.if_type, 53, "wintun 的实测 ifType 是 53");
            adapter.if_type = 6; // 以太网 —— 同一份抓取里真业务网卡报的值
            applied += 1;
        }
    }
    assert_eq!(applied, 1, "变异打上的处数不是 1，红绿都不算数");

    let after = netinfo::assemble_windows_netinfo_probe(&routes, &adapters, &ras, &[])
        .expect("变异的只是 ifType，照样组装得出");
    assert!(
        !after.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
        "ifType 改成 6 之后它还在隧道名单里 —— 白名单没在判：{:?}",
        after.tunnel_interfaces
    );
    assert!(
        !after.foreign.iter().any(|r| r.interface == "Tailscale"),
        "它的路由也该跟着从 foreign 里消失：{:?}",
        after.foreign
    );
}

/// 🔴 **负向对照：Hyper-V 的两张虚拟适配器（实测 ifType `6`）不得进名单**。
///
/// 正面前提在前：那两张适配器确实在派生出来的取材面里（否则这条否定断言没有信息量）。
#[test]
fn hyperv_virtual_switches_are_not_tunnels_on_the_in_process_leg() {
    let (_, adapters, _) = netinfo_inputs(WIN_HYPERV);
    for alias in ["vSwitch (Default Switch)", "vEthernet (Default Switch)"] {
        let found = adapters
            .iter()
            .find(|a| a.alias == alias)
            .unwrap_or_else(|| panic!("抓取里没有 `{alias}` —— 下面那条否定断言没有信息量"));
        assert_eq!(found.if_type, 6, "`{alias}` 的实测 ifType 是 6");
    }
    let snapshot = netinfo_leg(WIN_HYPERV, &[]);
    for alias in ["vSwitch (Default Switch)", "vEthernet (Default Switch)"] {
        assert!(
            !snapshot.tunnel_interfaces.iter().any(|t| t == alias),
            "Hyper-V 的 `{alias}` 被判成隧道了 —— `6` 同时还是真业务网卡的值：{:?}",
            snapshot.tunnel_interfaces
        );
    }
}

// ══════════════════════════ 取材面塌了：不折成空快照 ══════════════════════════

/// 🔴 **空路由表 / 空适配器表都必须报错**，不产出一份「看过了，没有冲突」的空快照。
///
/// RAS 那一腿的空是**结果**不是失败（绝大多数机器一条 VPN 都没配）—— 正面断言那一半：
/// 三腿里只有 RAS 空时，照常给出 `Ok`。
#[test]
fn empty_route_or_adapter_tables_are_failures_not_empty_facts() {
    let (routes, adapters, _) = netinfo_inputs(WIN_WINTUN);
    assert_eq!(
        netinfo::assemble_windows_netinfo_probe(&[], &adapters, &[], &[]),
        Err(WindowsNetInfoError::EmptyRouteTable),
        "空路由表被折成了一份空快照 —— 下游会把它读成「没有外来隧道」"
    );
    assert_eq!(
        netinfo::assemble_windows_netinfo_probe(&routes, &[], &[], &[]),
        Err(WindowsNetInfoError::EmptyAdapterTable),
        "空适配器表被折成了一份空快照 —— 空的隧道名单长得就像「这台机器上没有隧道」"
    );
    // 反向对照：只有 RAS 空 ⇒ 照常成功（这台机器本来就没配 VPN 连接）。
    let ok = netinfo::assemble_windows_netinfo_probe(&routes, &adapters, &[], &[])
        .expect("RAS 空是常态结果，不是失败");
    assert!(
        ok.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
        "RAS 空不该影响 ifType 那一族：{:?}",
        ok.tunnel_interfaces
    );
}

/// 两个作用域的 RAS 连接都要进名单，且按名去重（**合成输入** —— 仓里没有一份带
/// all-user 连接的真机抓取，`@@@GET_VPNCONNECTION_ALLUSER` 在 RAS 那份里是空的）。
///
/// 作用域位（`all_users`）**不是过滤判据**：Q8 的决定是「一次枚举覆盖两个作用域，都报」。
/// 变异锁：谁把 `windows_tunnel_interfaces` 改成只收 `!all_users`（或只收 `all_users`），
/// 这里的两条名字断言各红一条。
#[test]
fn ras_connections_from_both_scopes_are_kept_and_deduped() {
    let adapters = vec![WindowsAdapterKind {
        alias: "以太网".into(),
        if_type: 6,
    }];
    let ras = vec![
        WindowsRasConnection {
            name: "个人 VPN".into(),
            all_users: false,
        },
        WindowsRasConnection {
            name: "公司下发 VPN".into(),
            all_users: true,
        },
        // 同名重复（同一条连接被两个作用域各列一次的形态）：并入时必须只留一份，
        // 否则 `foreign` 里同一条路由会出现两遍。
        WindowsRasConnection {
            name: "公司下发 VPN".into(),
            all_users: false,
        },
    ];
    let names = netinfo::windows_tunnel_interfaces(&adapters, &ras);
    assert_eq!(
        names,
        vec!["个人 VPN".to_string(), "公司下发 VPN".to_string()],
        "两个作用域的连接都要在、且按名去重；非隧道适配器（ifType 6）不得混进来"
    );
}

// ══════════════════════════ 接线：注入之后一个外部进程都不起 ══════════════════════════

/// 用真机抓取派生出来的三腿喂给 [`WindowsNetInfoSource`] 的替身。
struct FixtureNetInfo {
    routes: Vec<RouteEntry>,
    adapters: Vec<WindowsAdapterKind>,
    ras: Result<Vec<WindowsRasConnection>, String>,
}

impl WindowsNetInfoSource for FixtureNetInfo {
    fn routes(&self) -> Result<Vec<RouteEntry>, String> {
        Ok(self.routes.clone())
    }
    fn adapters(&self) -> Result<Vec<WindowsAdapterKind>, String> {
        Ok(self.adapters.clone())
    }
    fn ras_connections(&self) -> Result<Vec<WindowsRasConnection>, String> {
        self.ras.clone()
    }
}

fn fixture_source(file: &str) -> Arc<FixtureNetInfo> {
    let (routes, adapters, ras) = netinfo_inputs(file);
    Arc::new(FixtureNetInfo {
        routes,
        adapters,
        ras: Ok(ras),
    })
}

/// 🔴 **注入取材源之后，Windows 腿一个外部进程都不起**（D1 的全部目的）。
///
/// 反向对照在同一条测试里：**不**注入时它仍跑那六条命令 —— 少了这一半，「零命令」可能只是
/// 因为 mock runner 压根没被问过（比如平台分派错了、直接走进 `Unsupported`）。
#[test]
fn injecting_the_netinfo_source_replaces_all_six_external_commands() {
    // ── 注入：零命令 ──
    let injected = ForeignTunnelProbeImpl::with_platform(super::queued(&[]), Platform::Win)
        .with_windows_netinfo(fixture_source(WIN_RAS));
    let outcome = injected
        .probe_foreign_tunnels(&[])
        .expect("三腿都成功，应走 Probed");
    assert!(
        injected.runner.snapshot().is_empty(),
        "注入之后仍起了外部进程：{:?}",
        injected.runner.snapshot()
    );
    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("注入之后应走 Probed");
    };
    assert_eq!(
        snapshot,
        text_leg(WIN_RAS, &[]),
        "经生产分派拿到的快照与文本腿不同 —— 接线上漏了什么"
    );

    // ── 反向对照：不注入 ⇒ 逐字维持六条命令 ──
    let plain = ForeignTunnelProbeImpl::with_platform(
        super::queued(&[
            section(WIN_RAS, "ROUTE_PRINT_4").as_str(),
            section(WIN_RAS, "ROUTE_PRINT_6").as_str(),
            section(WIN_RAS, "GET_NETIPADDRESS").as_str(),
            section(WIN_RAS, "GET_NETADAPTER").as_str(),
            section(WIN_RAS, "GET_VPNCONNECTION").as_str(),
            optional_section(WIN_RAS, "GET_VPNCONNECTION_ALLUSER").as_str(),
        ]),
        Platform::Win,
    );
    plain.probe_foreign_tunnels(&[]).expect("命令腿照常");
    assert_eq!(
        plain.runner.snapshot().len(),
        6,
        "不注入时应逐字维持六条命令（注入失败的降级形态 = 回到本批之前的现状）"
    );
}

/// 🔴 **RAS 腿失败只降级它自己**：整次探测仍是 `Probed`，另外两腿的事实一条不少。
///
/// 这一条是本批点名要修的既有缺陷：文本腿把 `Get-VpnConnection` 的失败（没装 `VpnClient`
/// 模块的 SKU）算成整次探测失败，那些机器从「探得动」退化成 `Err`。
///
/// 反向对照：路由腿失败**必须**让整次探测 `Err`（拿不到路由表就没有任何事实）。
#[test]
fn a_failing_ras_leg_degrades_only_itself() {
    let (routes, adapters, ras) = netinfo_inputs(WIN_RAS);
    let probe = ForeignTunnelProbeImpl::with_platform(super::queued(&[]), Platform::Win)
        .with_windows_netinfo(Arc::new(FixtureNetInfo {
            routes: routes.clone(),
            adapters: adapters.clone(),
            ras: Err("RasEnumConnectionsW 失败（Win32 错误码 632）".into()),
        }));
    let outcome = probe
        .probe_foreign_tunnels(&[])
        .expect("RAS 一腿失败不该整次失败");
    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("RAS 腿失败时应仍走 Probed");
    };
    assert!(
        snapshot.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
        "另外两腿的事实被 RAS 那一腿的失败连累了：{:?}",
        snapshot.tunnel_interfaces
    );
    assert!(
        ras.iter().any(|c| c.name == "PolarisProbeL2TP")
            && !snapshot
                .tunnel_interfaces
                .iter()
                .any(|t| t == "PolarisProbeL2TP"),
        "前提对照：这份抓取本来有 RAS 连接，腿失败后它如实缺席（不是伪造出来的）"
    );

    // 反向对照：路由腿失败 ⇒ 整次 Err，绝不折成空的 Probed。
    struct DeadRoutes;
    impl WindowsNetInfoSource for DeadRoutes {
        fn routes(&self) -> Result<Vec<RouteEntry>, String> {
            Err("GetIpForwardTable2 失败（Win32 错误码 5）".into())
        }
        fn adapters(&self) -> Result<Vec<WindowsAdapterKind>, String> {
            Ok(Vec::new())
        }
        fn ras_connections(&self) -> Result<Vec<WindowsRasConnection>, String> {
            Ok(Vec::new())
        }
    }
    let dead = ForeignTunnelProbeImpl::with_platform(super::queued(&[]), Platform::Win)
        .with_windows_netinfo(Arc::new(DeadRoutes));
    let error = dead
        .probe_foreign_tunnels(&[])
        .expect_err("路由表拿不到 ⇒ 整次探测失败，不是「没有冲突」");
    assert!(
        error.to_string().contains("GetIpForwardTable2"),
        "错误串该带上是哪个 API 失败的（诊断只剩这一串）：{error}"
    );
}

/// 🔴 **别名 join 面塌了必须自曝**（[`netinfo::routes_with_unknown_interface`]）。
///
/// 正向对照在前：真机抓取上大多数路由都 join 得上（`unknown < 总数`）——
/// 不是 0，因为 `Loopback Pseudo-Interface 1` 这类隐藏接口本就不在 `Get-NetAdapter` 里。
///
/// 变异：把两侧的别名整体换成另一个命名空间（模拟 `ConvertInterfaceLuidToAlias` 返回 GUID
/// 而 `FriendlyName` 返回中文别名这种真机可能）→ 必须**全部**对不上，即调用方那句 warn 的
/// 触发条件。少了这一半，判据可能恒 0（比如把过滤条件写反）而正向那条照样绿。
#[test]
fn a_collapsed_alias_join_face_is_detectable() {
    let (routes, adapters, ras) = netinfo_inputs(WIN_RAS);
    let unknown = netinfo::routes_with_unknown_interface(&routes, &adapters, &ras);
    assert!(
        unknown < routes.len(),
        "正向对照：真机抓取上应有相当一部分路由 join 得上，实际 {unknown}/{} 全落空",
        routes.len()
    );

    let renamed: Vec<WindowsAdapterKind> = adapters
        .iter()
        .map(|a| WindowsAdapterKind {
            alias: format!("{{2F3A…}}{}", a.alias),
            if_type: a.if_type,
        })
        .collect();
    let renamed_ras: Vec<WindowsRasConnection> = ras
        .iter()
        .map(|c| WindowsRasConnection {
            name: format!("{{2F3A…}}{}", c.name),
            all_users: c.all_users,
        })
        .collect();
    assert_eq!(
        netinfo::routes_with_unknown_interface(&routes, &renamed, &renamed_ras),
        routes.len(),
        "两侧命名空间不一致时必须**全部**对不上 —— 那正是那句 warn 的触发条件"
    );
    // 后果对照：命名空间一错，隧道判定整体落空（`foreign` 空 = 一句「无冲突」）。
    let collapsed = netinfo::assemble_windows_netinfo_probe(&routes, &renamed, &renamed_ras, &[])
        .expect("两张表都非空，组装本身仍成立");
    assert!(
        collapsed.foreign.is_empty() && collapsed.default_routes.is_empty(),
        "别名对不上却仍判出了外来隧道路由 —— 说明 join 不是按别名做的，本条对照没有信息量"
    );
}

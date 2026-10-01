//! 🔴 「外来隧道抢默认路由」这一类的门 —— 真机抓取 → 真解析器 → 真判定，一条链跑通。
//!
//! # 这道门为什么必须存在
//!
//! 2026-09-13 w207 实测：Windows 内置 L2TP 连上之后，承载接口 `PolarisProbeL2TP` 宣告
//! `0.0.0.0/0` metric 1（一条抢全部出站流量的全隧道），而它在展示面上**几乎看不见** ——
//! 过完噪声过滤，同一接口只剩一条 `10.55.0.10/32`。用户读到的是「某个隧道宣告了一个 /32」，
//! 真实情况是「它要了全部流量」。
//!
//! # 两条判据，方向相反，缺一条这道门就只剩一半
//!
//! 1. **[`foreign_never_contains_a_default_route`]（回归门）**：默认路由**永远不许**并回
//!    `ForeignTunnelSnapshot::foreign`。它与**任何**前缀相交，并回去就是让
//!    `detect_tunnel_conflicts` 对我方每一条网段各报一次 —— 一条全隧道把告警刷爆，
//!    而刷爆的告警等于没有告警（`plat-warn` 已经演过一遍）。这是整个「两条通道」设计的理由，
//!    不钉住它，将来有人「顺手」合并回去，没有任何东西会红。
//! 2. **[`the_w207_ras_l2tp_tunnel_is_judged_to_contend_for_the_default_route`]（正向门）**：
//!    那条真实的全隧道必须被判出来并点名到接口。
//!
//! 两条都带**活输入的负向对照**（没装 TUN ⇒ 不报；不宣告默认路由的抓取 ⇒ 不报；
//! 我方自己的 TUN ⇒ 不自报），否则任一条都可能只是「什么都没发生」造成的假绿。

use super::fixture_harness::{read_fixture, section};
use super::{fixture_section, optional_fixture_section, windows_leg};
use crate::route_probe::{
    assemble_linux_probe, assemble_macos_probe, assemble_windows_probe, is_default_route,
    ForeignTunnelSnapshot, IpFamily, RouteEntry,
};
use polaris_config_engine::builder::tunnel_conflict::{
    detect_tunnel_conflicts, ConflictInput, ConflictKind, ForeignTunnelRoute, TunnelConflict,
};

const WIN_RAS: &str = "windows-w207-ras-l2tp-connected-2026-09-13.txt";
const WIN_HYPERV: &str = "windows-w207-hyperv-present-2026-09-13.txt";
const WIN_TAP: &str = "windows-w207-ovpn-tun-connected-2026-09-13.txt";
const WIN_TS_ON: &str = "windows-w207-ts-on-2026-09-12.txt";
const WIN_WINTUN: &str = "windows-w207-wintun-present-2026-09-12.txt";
const WIN_TS_OFF: &str = "windows-w207-routes-ts-off-2026-09-12.txt";
const LINUX_OVPN: &str = "linux-vm185-ovpn-dco-2026-09-13.txt";
const MAC_TS_OFF: &str = "macos-p101-routes-ts-off-2026-09-12.txt";
const MAC_TS_ON: &str = "macos-p101-routes-ts-on-2026-09-12.txt";

/// 那条 RAS 全隧道的接口名（= VPN 连接名，见 `parse_vpn_connection_names` 头注）。
const RAS_IFACE: &str = "PolarisProbeL2TP";

fn linux_leg(file: &str) -> ForeignTunnelSnapshot {
    let raw = read_fixture(file);
    assemble_linux_probe(
        &section(&raw, "V4_ROUTE"),
        &section(&raw, "V6_ROUTE"),
        &section(&raw, "LINK_TUN"),
        &section(&raw, "LINK_WIREGUARD"),
        &section(&raw, "LINK_OVPN"),
        &[],
    )
}

fn macos_leg(file: &str) -> ForeignTunnelSnapshot {
    let raw = read_fixture(file);
    assemble_macos_probe(
        &section(&raw, "V4"),
        &section(&raw, "V6"),
        &section(&raw, "IFCONFIG"),
        &[],
    )
    .expect("真机 mac 抓取应解析得动")
}

/// 一份抓取跑完真正的那条平台腿。
fn leg(file: &str) -> ForeignTunnelSnapshot {
    if file.starts_with("windows-") {
        windows_leg(file)
    } else if file.starts_with("linux-") {
        linux_leg(file)
    } else {
        macos_leg(file)
    }
}

fn wire(routes: &[RouteEntry]) -> Vec<ForeignTunnelRoute> {
    routes
        .iter()
        .map(|r| ForeignTunnelRoute {
            interface: r.interface.clone(),
            prefix: r.prefix.clone(),
        })
        .collect()
}

/// 本门统一用的那组判据段。
///
/// 三组都**取自真机**，不是为了凑绿编的：`198.18.0.0/15` 是 Polaris 的默认 FakeIP 段、
/// `32.0.0.0/24` + `fd7a:115c:a1e0::/48` 是 2026-09-11 自建 headscale 实测的 tailnet 段、
/// `10.8.0.1/24` 覆盖 w207 那台上 OpenVPN 的 `10.8.0.0/24`。
/// 三组都**能在这批抓取上真的命中**，前三类的「逐条不变」才有内容可比。
fn criteria_ranges() -> ([String; 2], [String; 2], [String; 1]) {
    (
        ["198.18.0.0/15".into(), "2001:2::/48".into()],
        ["32.0.0.0/24".into(), "fd7a:115c:a1e0::/48".into()],
        ["10.8.0.1/24".into()],
    )
}

/// 一份抓取跑完整条链 → 判定结果。`tun_addresses` 空 = 本轮没装 TUN inbound。
fn conflicts_of(snapshot: &ForeignTunnelSnapshot, with_tun: bool) -> Vec<TunnelConflict> {
    let (fakeip, mesh, tun) = criteria_ranges();
    let foreign = wire(&snapshot.foreign);
    let defaults = wire(&snapshot.default_routes);
    detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &defaults,
        fakeip_ranges: &fakeip,
        mesh_cidrs: &mesh,
        tun_addresses: if with_tun { &tun } else { &[] },
    })
}

fn count(conflicts: &[TunnelConflict], kind: ConflictKind) -> usize {
    conflicts.iter().filter(|c| c.kind == kind).count()
}

/// 每份抓取的期望：`foreign` 条数，以及四类各判出几条（装了 TUN 的那一档）。
///
/// # `foreign` 条数与前三类的计数**都是改动前的读数**
///
/// 2026-09-13 记于改动前的那一次基线跑（默认路由还在三个解析器里被 `continue` 掉时）：
/// 九份抓取的 `foreign` 条数逐份是 9 / 3 / 40 / 30 / 3 / 0 / 6 / 36 / 66。改动后它们一个没变 ——
/// 默认路由走了**另一个字段**，没有挤进这一批。这就是「现有三类判定结果逐条相同」的
/// 供给面证据：前三类只吃 `foreign`，输入不变、那段代码没动，输出就不会变。
///
/// 最后一列是新类别，改动前恒为 0（那时压根没有这一类）。
const EXPECTED: &[(&str, usize, usize, usize, usize, usize)] = &[
    // 文件, foreign 条数, FakeIp, Mesh, TunAddress, DefaultRouteContended
    (WIN_RAS, 9, 0, 0, 0, 1), // ← 本次要让它可见的那条 L2TP 全隧道
    (WIN_HYPERV, 3, 0, 0, 0, 0),
    (WIN_TAP, 40, 0, 30, 3, 0),
    (WIN_TS_ON, 30, 0, 29, 0, 0),
    (WIN_WINTUN, 3, 0, 0, 0, 0),
    (WIN_TS_OFF, 0, 0, 0, 0, 0),
    (LINUX_OVPN, 6, 1, 0, 1, 0),
    (MAC_TS_OFF, 36, 0, 0, 0, 0),
    (MAC_TS_ON, 66, 0, 27, 0, 0),
];

/// 🔴 **回归门：`foreign` 里永远不含默认路由，且前三类的判定逐份不变。**
///
/// 三半合起来才有信息量：
///  - **否定面**：九份抓取跑完真腿，`foreign` 里一条 `0.0.0.0/0` / `::/0` 都没有；
///  - **正向对照**：这批抓取里**确实有**默认路由（否则否定面是空跑）—— 逐份数原始输出里的
///    默认路由行，并断言至少有一份把它收进了 `default_routes`；
///  - **不变量**：`foreign` 条数与前三类的逐类计数，与改动前的基线逐份相等。
#[test]
fn foreign_never_contains_a_default_route() {
    let mut captures_with_default_rows = 0usize;
    let mut legs_with_foreign_defaults = 0usize;

    for (file, want_foreign, want_fakeip, want_mesh, want_tun, want_contended) in
        EXPECTED.iter().copied()
    {
        let snapshot = leg(file);

        // ── 否定面 ──
        let leaked: Vec<&RouteEntry> = snapshot
            .foreign
            .iter()
            .filter(|r| is_default_route(&r.prefix))
            .collect();
        assert!(
            leaked.is_empty(),
            "{file}：默认路由并进了 `foreign` —— 它与**任何**前缀相交，\
             判定会对我方每条网段各报一次冲突，一条全隧道就把告警刷爆。漏进来的：{leaked:?}"
        );

        // ── 正向对照①：这份抓取的原始输出里确实有默认路由行 ──
        if raw_capture_has_default_route_rows(file) {
            captures_with_default_rows += 1;
        }
        // ── 正向对照②：有没有哪一份真的把它收进了另一个桶 ──
        if !snapshot.default_routes.is_empty() {
            legs_with_foreign_defaults += 1;
            for entry in &snapshot.default_routes {
                assert!(
                    is_default_route(&entry.prefix),
                    "{file}：`default_routes` 里混进了非默认路由 {entry:?} —— 两个桶必须互斥"
                );
            }
        }

        // ── 不变量：条数与前三类的逐类计数 ──
        assert_eq!(
            snapshot.foreign.len(),
            want_foreign,
            "{file}：`foreign` 条数与改动前的基线对不上：{:?}",
            snapshot.foreign
        );
        let conflicts = conflicts_of(&snapshot, true);
        assert_eq!(
            (
                count(&conflicts, ConflictKind::FakeIpOverlap),
                count(&conflicts, ConflictKind::MeshOverlap),
                count(&conflicts, ConflictKind::TunAddressOverlap),
            ),
            (want_fakeip, want_mesh, want_tun),
            "{file}：前三类的判定结果与改动前的基线对不上：{conflicts:?}"
        );
        // 新类别逐份钉死：除 w207 那条全隧道外，**其余八份都必须是 0** ——
        // 「加了一类之后 mac/Linux 上开始逢隧道必报」正是这一列要拦的回归。
        assert_eq!(
            count(&conflicts, ConflictKind::DefaultRouteContended),
            want_contended,
            "{file}：新类别判出的条数与登记不符：{conflicts:?}"
        );
    }

    assert!(
        captures_with_default_rows >= 7,
        "只有 {captures_with_default_rows} 份抓取的原始输出里有默认路由行 —— \
         上面那条否定断言可能是空跑（连输入都没有）"
    );
    assert_eq!(
        legs_with_foreign_defaults, 1,
        "把默认路由收进 `default_routes` 的抓取份数不是 1 —— \
         这批抓取里只有 w207 那份 RAS 全隧道该落进来；数变了说明分桶判据漂了"
    );

    // ── 前三类**逐类都要真的命中过**，否则「不变」是在比一排 0 ──
    let totals = EXPECTED
        .iter()
        .fold((0, 0, 0), |acc, e| (acc.0 + e.2, acc.1 + e.3, acc.2 + e.4));
    assert!(
        totals.0 > 0 && totals.1 > 0 && totals.2 > 0,
        "前三类里有一类在这批抓取上一条都没命中过，「逐条不变」对它没有信息量：{totals:?}"
    );
}

/// 某份抓取的**原始输出**里有没有默认路由行（不过解析器，直接看文本）。
///
/// 判据故意写成三个平台的**原始字面量**：`0.0.0.0 0.0.0.0`（Windows `route print`）、
/// `::/0`、行首 `default`（Linux `ip route` / macOS `netstat -rn`）。
/// 走解析器去数就成了自证 —— 解析器把默认路由丢了的话，这条正向对照也跟着一起瞎。
fn raw_capture_has_default_route_rows(file: &str) -> bool {
    let raw = read_fixture(file);
    raw.lines().any(|line| {
        let t = line.trim();
        t.starts_with("default") || t.contains("0.0.0.0          0.0.0.0") || t.contains("::/0")
    })
}

/// 🔴 **正向门：w207 那条 L2TP 全隧道必须被判出来，并点名到接口。**
///
/// 这道门在实现落地前是红的：2026-09-13 改动前的那一次跑，同一份抓取 + 同一组判据段
/// 判出 **0 条冲突**（`foreign` 里那 9 条与三组段一条都不相交），而抓取里明摆着有
/// `0.0.0.0/0 PolarisProbeL2TP`。
#[test]
fn the_w207_ras_l2tp_tunnel_is_judged_to_contend_for_the_default_route() {
    // ── 前提：那条默认路由**两张表里都在**（解析器读的 `route print` 与独立读数 `Get-NetRoute`）──
    let route_print = fixture_section(WIN_RAS, "ROUTE_PRINT_4");
    assert!(
        route_print
            .lines()
            .any(|l| l.contains("0.0.0.0") && l.contains("10.55.0.10")),
        "`@@@ROUTE_PRINT_4` 里没有 {RAS_IFACE} 的默认路由 —— 本门没有输入"
    );
    let get_netroute = fixture_section(WIN_RAS, "GET_NETROUTE_4");
    assert!(
        get_netroute
            .lines()
            .any(|l| l.starts_with("0.0.0.0/0") && l.contains(RAS_IFACE)),
        "`@@@GET_NETROUTE_4` 这条独立读数里没有它 —— 两张表对不上时该先查抓取"
    );

    let snapshot = windows_leg(WIN_RAS);
    assert_eq!(
        snapshot.default_routes,
        vec![RouteEntry {
            prefix: "0.0.0.0/0".into(),
            interface: RAS_IFACE.into()
        }],
        "探测层没把那条全隧道收进 `default_routes`"
    );

    let conflicts = conflicts_of(&snapshot, true);
    let contended: Vec<&TunnelConflict> = conflicts
        .iter()
        .filter(|c| c.kind == ConflictKind::DefaultRouteContended)
        .collect();
    assert_eq!(
        contended.len(),
        1,
        "抢默认路由的全隧道没判出恰好一条：{conflicts:?}"
    );
    assert_eq!(contended[0].interface, RAS_IFACE, "没点名到接口");
    assert_eq!(
        contended[0].prefix, "0.0.0.0/0",
        "前缀要保留规范形，族信息不许在链路上丢掉"
    );
}

/// 🔴 **负向对照①：本轮没装 TUN inbound ⇒ 同一份抓取一条都不报。**
#[test]
fn without_a_tun_inbound_the_same_capture_reports_nothing() {
    let snapshot = windows_leg(WIN_RAS);
    // 正向对照先行，否则「不报」可能只是这条链压根没接上。
    assert_eq!(
        count(
            &conflicts_of(&snapshot, true),
            ConflictKind::DefaultRouteContended
        ),
        1,
        "前提：装了 TUN 时它确实会报"
    );
    assert_eq!(
        count(
            &conflicts_of(&snapshot, false),
            ConflictKind::DefaultRouteContended
        ),
        0,
        "没装 TUN inbound 时报了 —— 那时只有一个声索人，不是冲突"
    );
}

/// 🔴 **负向对照②：外来隧道不宣告默认路由的抓取 ⇒ 不报**，且**先证那份抓取里确实没有**。
///
/// 取三份互不相同的形态：Windows（装了 Hyper-V、默认路由全在物理网卡上）、
/// Linux（ovpn-dco，默认路由在 `ens18` 上）、macOS（八个 utun 的默认路由全是作用域路由）。
/// 每一份都先断言「原始输出里有默认路由行、但一条都没落到外来隧道头上」——
/// 少了这一半，「不报」与「这份抓取里压根没有默认路由」同形，本条就是空跑。
#[test]
fn captures_without_a_foreign_default_route_report_nothing() {
    for file in [WIN_HYPERV, LINUX_OVPN, MAC_TS_OFF] {
        assert!(
            raw_capture_has_default_route_rows(file),
            "{file}：原始输出里一条默认路由行都没有 —— 本条对照没有输入"
        );
        let snapshot = leg(file);
        assert!(
            snapshot.default_routes.is_empty(),
            "{file}：这份抓取里的默认路由不该落到外来隧道头上（物理网卡 / 作用域路由）：{:?}",
            snapshot.default_routes
        );
        assert_eq!(
            count(
                &conflicts_of(&snapshot, true),
                ConflictKind::DefaultRouteContended
            ),
            0,
            "{file}：没有外来默认路由却报了"
        );
    }
}

/// 🔴 **负向对照③：Polaris 自己的 TUN 宣告的默认路由不许自报。**
///
/// 输入是**活的**：同一份 w207 抓取，只把 `own_interfaces` 从空换成那条隧道的名字
/// （生产里这个名字来自本次发射的 `interface_name` + post-flight 观测到的出口别名）。
/// 剔除只作用在 `foreign` 上、忘了另一个桶的实现，会在这里红。
#[test]
fn our_own_tun_default_route_is_not_reported_back_at_us() {
    let sections = |id: &str| fixture_section(WIN_RAS, id);
    let probe = |own: &[String]| {
        assemble_windows_probe(
            &sections("ROUTE_PRINT_4"),
            &sections("ROUTE_PRINT_6"),
            &sections("GET_NETIPADDRESS"),
            &sections("GET_NETADAPTER"),
            &optional_fixture_section(WIN_RAS, "GET_VPNCONNECTION"),
            &optional_fixture_section(WIN_RAS, "GET_VPNCONNECTION_ALLUSER"),
            own,
        )
        .expect("真机抓取应解析得动")
    };

    // 正向对照先行：不剔除时它**确实**在 `default_routes` 里。
    assert_eq!(probe(&[]).default_routes.len(), 1, "前提：不剔除时它在列");

    let mine = probe(&[RAS_IFACE.to_string()]);
    assert!(
        mine.default_routes.is_empty(),
        "我方自己的 TUN 宣告的默认路由被当成了外来隧道 —— \
         自指告警与真告警混在一起，整条告警就废了：{:?}",
        mine.default_routes
    );
    assert!(
        !mine.foreign.iter().any(|r| r.interface == RAS_IFACE),
        "同一次剔除没作用在 `foreign` 上：{:?}",
        mine.foreign
    );
    assert_eq!(
        count(
            &conflicts_of(&mine, true),
            ConflictKind::DefaultRouteContended
        ),
        0,
        "剔除之后仍判出冲突"
    );
}

/// 🔴 **macOS 的作用域默认路由不是声索**（`Flags` 里的 `I` = `RTF_IFSCOPE`）。
///
/// # 这条不写，mac 上每次起核都会报 8 条
///
/// 任何一台 mac 的 `netstat -rn -f inet6` 里，每个 utun 上都挂着一条 `default`。
/// 2026-09-12 p101 那份**断开态**抓取里有 8 条，全带 `I`；同一张表里 `en0` 那条全局默认路由
/// **不带** `I`。收进来就是逢隧道必报 —— 而 `plat-warn` 的下场是被删掉。
///
/// # 变异对照：把 `I` 去掉，它们必须变成声索
///
/// 只断「现在不报」证不了判据在起作用（一个把 mac 默认路由全丢掉的实现同样绿）。
/// 故把那 8 处 `UGcIg` 逐字换成 `UGcg`（先证变异打上了：替换处数 == 8），
/// 其余一个字节不动，重跑同一条腿 —— 8 条必须全部现身。
#[test]
fn macos_interface_scoped_default_routes_are_not_claims() {
    let raw = read_fixture(MAC_TS_OFF);
    let v6 = section(&raw, "V6");

    // ── 前提：这份抓取里确实有一批带 `I` 的 utun 默认路由，以及一条不带 `I` 的全局默认路由 ──
    let scoped: Vec<&str> = v6
        .lines()
        .filter(|l| l.trim_start().starts_with("default") && l.contains("UGcIg"))
        .collect();
    assert_eq!(
        scoped.len(),
        8,
        "带 `I` 的 utun 默认路由不是 8 条 —— 取材面变了，本条的正负两半都要重核：{scoped:?}"
    );
    assert!(
        v6.lines()
            .any(|l| l.trim_start().starts_with("default") && !l.contains('I')),
        "这份抓取里没有不带 `I` 的全局默认路由 —— 判据的正样本那一半没了"
    );

    // ── 未变异：一条都不许进 `default_routes` ──
    let clean = macos_leg(MAC_TS_OFF);
    assert!(
        clean.default_routes.is_empty(),
        "作用域默认路由被当成了声索 —— 任何一台有 utun 的 mac 每次起核都会报 8 条：{:?}",
        clean.default_routes
    );

    // ── 变异：`I` 去掉之后它们必须现身（证明判据真的咬在这个标志位上）──
    let applied = v6.matches("UGcIg").count();
    assert_eq!(applied, 8, "变异要打的处数不是 8，红绿都不算数");
    let mutated = v6.replace("UGcIg", "UGcg");
    assert!(
        !mutated.contains("UGcIg") && mutated.contains("UGcg"),
        "变异没打上"
    );
    let loosened = assemble_macos_probe(
        &section(&raw, "V4"),
        &mutated,
        &section(&raw, "IFCONFIG"),
        &[],
    )
    .expect("变异的只是标志位列，解析器该照常解析得动");
    assert_eq!(
        loosened.default_routes.len(),
        8,
        "去掉 `I` 之后那 8 条没有现身 —— 说明「不报」不是 `RTF_IFSCOPE` 这条判据带来的，\
         而是默认路由在 mac 腿上压根没被解析出来：{:?}",
        loosened.default_routes
    );
    assert!(
        loosened
            .default_routes
            .iter()
            .all(|r| r.interface.starts_with("utun") && r.prefix == "::/0"),
        "现身的不是那 8 条 utun 的 v6 默认路由：{:?}",
        loosened.default_routes
    );
}

/// Linux 的默认路由行**逐字没有 `/0`**，族只能由调用方给 —— 两条腿各自规范成对的前缀。
///
/// 正样本取自 VM185 那份真机抓取：v4 是 `default via 192.168.10.1 dev ens18 …`、
/// v6 是 `default nhid … via fe80::… dev ens18 …`，两条都落在 `ens18`（不是隧道）上。
#[test]
fn linux_default_routes_are_normalized_per_family() {
    let raw = read_fixture(LINUX_OVPN);
    for (id, family, want) in [
        ("V4_ROUTE", IpFamily::V4, "0.0.0.0/0"),
        ("V6_ROUTE", IpFamily::V6, "::/0"),
    ] {
        let body = section(&raw, id);
        assert!(
            body.lines().any(|l| l.trim_start().starts_with("default")),
            "@@@{id} 里没有默认路由行 —— 本条没有输入"
        );
        let routes = crate::route_probe::parse_ip_routes(&body, family);
        assert!(
            routes.contains(&RouteEntry {
                prefix: want.into(),
                interface: "ens18".into()
            }),
            "@@@{id} 的默认路由没规范成 `{want}`：{routes:?}"
        );
        // 反向对照：另一族的前缀不许冒出来（族是调用方给的，不是从行里猜的）。
        let other = if want == "0.0.0.0/0" {
            "::/0"
        } else {
            "0.0.0.0/0"
        };
        assert!(
            !routes.iter().any(|r| r.prefix == other),
            "@@@{id} 里冒出了另一族的默认路由 `{other}`：{routes:?}"
        );
    }
    // 它落在物理网卡上 ⇒ 过完外来隧道过滤，一条都不剩。
    assert!(
        linux_leg(LINUX_OVPN).default_routes.is_empty(),
        "`ens18` 的默认路由被当成了外来隧道的宣告"
    );
}

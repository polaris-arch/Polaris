use super::*;
use crate::user_config::tun_config::{FAKEIP_INET4_RANGE, FAKEIP_INET6_RANGE};

fn route(interface: &str, prefix: &str) -> ForeignTunnelRoute {
    ForeignTunnelRoute {
        interface: interface.into(),
        prefix: prefix.into(),
    }
}

fn kinds(conflicts: &[TunnelConflict]) -> Vec<ConflictKind> {
    conflicts.iter().map(|c| c.kind).collect()
}

/// **本模块最重要的一条**：现场那台机器的形态**不该**报警。
///
/// macOS + 自建 tailnet `32.0.0.0/24` + Polaris 未配组网节点 + FakeIP 默认段
/// ⇒ 三组判据段一个都不相交 ⇒ 零告警。
///
/// 直觉版判据（"落在 auto_route 捕获面里就报"）在这里会报警，而那是**假的**：
/// `32.0.0.0/24` 比 `0.0.0.0/1` 更具体，路由表上本来就赢，流量照常走 Tailscale。
/// 逢隧道必报的告警最后会被无视 —— 这条就是把"不许乱报"钉死。
#[test]
fn self_hosted_tailnet_alone_is_not_a_conflict() {
    let foreign = vec![
        route("utun4", "32.0.0.0/24"),
        route("utun4", "fd7a:115c:a1e0::/48"),
    ];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[FAKEIP_INET4_RANGE.into(), FAKEIP_INET6_RANGE.into()],
        mesh_cidrs: &[],
        tun_addresses: &["172.19.0.1/30".into()],
    });
    assert!(
        conflicts.is_empty(),
        "外来隧道段与三组判据段都不相交，不该报警：{conflicts:?}"
    );
}

/// FakeIP 段撞车：外来隧道真的在用 `198.18.0.0/15` 里的地址。
#[test]
fn fakeip_overlap_is_reported() {
    let foreign = vec![route("utun6", "198.18.0.0/16")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[FAKEIP_INET4_RANGE.into()],
        mesh_cidrs: &[],
        tun_addresses: &[],
    });
    assert_eq!(kinds(&conflicts), vec![ConflictKind::FakeIpOverlap]);
    assert_eq!(conflicts[0].interface, "utun6");
}

/// 组网撞车：Polaris 自己也有一个 Tailscale 节点在为 tailnet 段发 force-route，
/// 而本机还跑着独立 Tailscale ⇒ 两个声索人争同一段。
#[test]
fn mesh_overlap_is_reported() {
    let foreign = vec![route("utun4", "100.64.0.0/10")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[],
        mesh_cidrs: &["100.64.0.0/10".into(), "fd7a:115c:a1e0::/48".into()],
        tun_addresses: &[],
    });
    assert_eq!(kinds(&conflicts), vec![ConflictKind::MeshOverlap]);
}

/// TUN 自身地址撞车。
#[test]
fn tun_address_overlap_is_reported() {
    let foreign = vec![route("utun9", "172.19.0.0/24")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[],
        mesh_cidrs: &[],
        tun_addresses: &["172.19.0.1/30".into()],
    });
    assert_eq!(kinds(&conflicts), vec![ConflictKind::TunAddressOverlap]);
}

/// 同一前缀命中多类 ⇒ 逐类各出一条，且顺序稳定（UI 与快照不因迭代顺序抖动）。
#[test]
fn multiple_kinds_on_one_prefix_are_all_reported_in_stable_order() {
    let foreign = vec![route("utun4", "100.64.0.0/10")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &["100.64.0.0/12".into()],
        mesh_cidrs: &["100.64.0.0/10".into()],
        tun_addresses: &["100.64.0.1/32".into()],
    });
    assert_eq!(
        kinds(&conflicts),
        vec![
            ConflictKind::FakeIpOverlap,
            ConflictKind::MeshOverlap,
            ConflictKind::TunAddressOverlap
        ]
    );
}

/// 判据段为空（FakeIP 关、无组网节点）⇒ 那一类恒不报，不能因为"空数组"退化成全匹配。
#[test]
fn empty_criteria_never_match() {
    let foreign = vec![route("utun4", "10.0.0.0/8")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[],
        mesh_cidrs: &[],
        tun_addresses: &[],
    });
    assert!(
        conflicts.is_empty(),
        "空判据段不该匹配任何东西：{conflicts:?}"
    );
}

/// 畸形/空前缀跳过，不 panic、不误报。
#[test]
fn malformed_prefixes_are_skipped() {
    let foreign = vec![
        route("utun4", ""),
        route("utun4", "   "),
        route("utun4", "not-a-cidr"),
        route("utun4", "198.18.0.0/16"),
    ];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[FAKEIP_INET4_RANGE.into()],
        mesh_cidrs: &[],
        tun_addresses: &[],
    });
    assert_eq!(
        conflicts.len(),
        1,
        "只有那条合法且相交的该报：{conflicts:?}"
    );
    assert_eq!(conflicts[0].prefix, "198.18.0.0/16");
}

// ══════════ 第四类：外来隧道抢默认路由 ══════════

/// 抢默认路由的全隧道 + 本轮确实装了 TUN inbound ⇒ 判出 [`ConflictKind::DefaultRouteContended`]。
///
/// 门控**不是**前缀相交：`0.0.0.0/0` 与三组判据段无一不相交，走 `cidr_overlaps_any`
/// 就是逢隧道必报。这里刻意把三组段全留空，只留 `tun_addresses` ——
/// 判出来了才说明门控咬的是「我方也是一个声索人」，不是「相交」。
#[test]
fn a_foreign_default_route_contends_with_our_tun() {
    let defaults = vec![route("PolarisProbeL2TP", "0.0.0.0/0")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &[],
        default_routes: &defaults,
        fakeip_ranges: &[],
        mesh_cidrs: &[],
        tun_addresses: &["172.19.0.1/30".into()],
    });
    assert_eq!(kinds(&conflicts), vec![ConflictKind::DefaultRouteContended]);
    assert_eq!(conflicts[0].interface, "PolarisProbeL2TP");
    assert_eq!(
        conflicts[0].prefix, "0.0.0.0/0",
        "前缀要保留规范形，族信息不能在这一层丢掉"
    );
}

/// v4 / v6 各一条 ⇒ 各出一条，族信息逐条留在前缀里。
#[test]
fn both_families_are_reported_separately() {
    let defaults = vec![
        route("utun9", "0.0.0.0/0"),
        route("utun9", "::/0"),
        route("", "   "),
    ];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &[],
        default_routes: &defaults,
        fakeip_ranges: &[],
        mesh_cidrs: &[],
        tun_addresses: &["172.19.0.1/30".into()],
    });
    assert_eq!(
        conflicts
            .iter()
            .map(|c| c.prefix.as_str())
            .collect::<Vec<_>>(),
        vec!["0.0.0.0/0", "::/0"],
        "空前缀那条该被跳过，两个族那两条该各出一条：{conflicts:?}"
    );
}

/// 🔴 **负向对照①：本轮没装 TUN inbound ⇒ 不报**。
///
/// `tun_addresses` 空 = 这次生成压根没发 TUN inbound（systemProxy / manual 模式）。
/// 那时外来隧道要全部流量与 Polaris **不争** —— 只有一个声索人，不是冲突。
/// 缺这条对照，上面那条绿可能只是「凡有默认路由就报」。
#[test]
fn without_a_tun_inbound_a_foreign_default_route_is_not_a_conflict() {
    let defaults = vec![route("PolarisProbeL2TP", "0.0.0.0/0")];
    let input = |tun: &[String]| {
        detect_tunnel_conflicts(&ConflictInput {
            foreign: &[],
            default_routes: &defaults,
            fakeip_ranges: &["198.18.0.0/15".into()],
            mesh_cidrs: &["32.0.0.0/24".into()],
            tun_addresses: tun,
        })
    };
    // 正向对照先行：同一条输入，只把 tun_addresses 填上就必须报。
    assert_eq!(
        kinds(&input(&["172.19.0.1/30".into()])),
        vec![ConflictKind::DefaultRouteContended],
        "前提：装了 TUN 时它确实会报"
    );
    assert!(
        input(&[]).is_empty(),
        "没装 TUN inbound 时不该报 —— 只有一个声索人：{:?}",
        input(&[])
    );
}

/// 🔴 **负向对照②：外来隧道不宣告默认路由 ⇒ 不报**，且不连累前三类。
#[test]
fn a_tunnel_without_a_default_route_is_not_contended() {
    let foreign = vec![route("utun4", "198.18.0.0/16")];
    let conflicts = detect_tunnel_conflicts(&ConflictInput {
        foreign: &foreign,
        default_routes: &[],
        fakeip_ranges: &[FAKEIP_INET4_RANGE.into()],
        mesh_cidrs: &[],
        tun_addresses: &["172.19.0.1/30".into()],
    });
    assert_eq!(
        kinds(&conflicts),
        vec![ConflictKind::FakeIpOverlap],
        "没有默认路由时只该剩前三类里命中的那条：{conflicts:?}"
    );
}

/// 🔴 **新增这一类不许改动前三类的逐条输出**：同一份 `foreign`，加不加 `default_routes`，
/// 前三类判出来的条目**逐条相同**（顺序也相同），新的那条只能追加在最后。
///
/// 没有这条，「顺便把默认路由并进 foreign」这种改法会让前三类各多报一遍，而本条正是它的绊线。
#[test]
fn adding_default_routes_does_not_perturb_the_first_three_kinds() {
    let foreign = vec![
        route("utun4", "198.18.0.0/16"),
        route("utun4", "100.64.0.0/10"),
        route("utun9", "172.19.0.0/24"),
    ];
    let criteria = |defaults: &[ForeignTunnelRoute]| {
        detect_tunnel_conflicts(&ConflictInput {
            foreign: &foreign,
            default_routes: defaults,
            fakeip_ranges: &[FAKEIP_INET4_RANGE.into()],
            mesh_cidrs: &["100.64.0.0/10".into()],
            tun_addresses: &["172.19.0.1/30".into()],
        })
    };
    let without = criteria(&[]);
    assert_eq!(
        kinds(&without),
        vec![
            ConflictKind::FakeIpOverlap,
            ConflictKind::MeshOverlap,
            ConflictKind::TunAddressOverlap
        ],
        "前提：三类各有一条命中，否则「逐条相同」是空跑"
    );
    let with = criteria(&[route("PolarisProbeL2TP", "0.0.0.0/0")]);
    assert_eq!(
        with[..without.len()],
        without[..],
        "前三类的逐条输出被新类别扰动了"
    );
    assert_eq!(
        with[without.len()..]
            .iter()
            .map(|c| c.kind)
            .collect::<Vec<_>>(),
        vec![ConflictKind::DefaultRouteContended],
        "新类别只能追加在最后"
    );
}

// ══════════ 判据段的取材面（从本次发射的产物读回）══════════

use crate::builder::endpoint_routes::ObservedTailnetAddresses;
use crate::singbox::SingBoxConfig;
use crate::user_config::server_config::{Protocol, ServerConfig, TailscaleSettings};

/// 2026-09-11 真控制面实测值：自建 headscale 的 `prefixes.v4` 非默认，self 拿到 `32.0.0.28`。
/// 与 `tests/tailnet_observed_address_force_route_gate.rs` 同一个真值，**刻意不另编一个**。
const OBSERVED_V4: &str = "32.0.0.28";

/// 缺省 `alwaysRouteSubnets`（= true）⇒ 恒 engaged，不依赖选中/规则点名。
fn ts_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        tailscale_settings: Some(Box::new(TailscaleSettings::default())),
        protocol: Protocol::Tailscale,
        ..Default::default()
    }
}

fn observed(id: &str, addrs: &[&str]) -> ObservedTailnetAddresses {
    let mut map = ObservedTailnetAddresses::new();
    map.insert(
        id.to_string(),
        addrs.iter().map(|a| (*a).to_string()).collect(),
    );
    map
}

/// 一份「发射给内核的」配置：TUN 入站 + fakeip DNS server。
///
/// 走 `serde_json` 反序列化而不是手搓结构体：读回腿认的是 **wire 上的键名**
/// （`type` / `inet4_range` / `address`），手搓结构体绕开 serde 就测不到那层。
fn emitted(dns: serde_json::Value, tun: serde_json::Value) -> SingBoxConfig {
    serde_json::from_value(serde_json::json!({
        "log": { "level": "info", "timestamp": true },
        "dns": dns,
        "inbounds": [tun],
        "outbounds": [],
    }))
    .expect("夹具应是合法的 SingBoxConfig")
}

/// FakeIP 段与 TUN 地址是**从产物读回**的，不是重算的。
///
/// 变异对照：把夹具里的 `inet4_range` 改成别的值 ⇒ 本条必须跟着变（说明它读的是产物，
/// 不是某个常量）。这里用一个**非默认**的 `inet4_range` 就地兑现了这条：若实现改成
/// 回落常量 `FAKEIP_INET4_RANGE`，断言当场红。
#[test]
fn criteria_are_read_back_from_the_emitted_config() {
    let config = emitted(
        serde_json::json!({
            "servers": [
                { "tag": "local", "type": "udp", "server": "223.5.5.5" },
                { "tag": "fakeip", "type": "fakeip", "inet4_range": "198.19.0.0/16", "inet6_range": "2001:2::/48" },
            ]
        }),
        serde_json::json!({
            "type": "tun", "tag": "tun-in",
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "interface_name": "polaris-tun0"
        }),
    );
    let criteria = emitted_conflict_criteria(&config, &[], &ObservedTailnetAddresses::new());
    assert_eq!(
        criteria.fakeip_ranges,
        vec!["198.19.0.0/16".to_string(), "2001:2::/48".to_string()],
        "FakeIP 段应逐字读回产物里的那两个值"
    );
    assert_eq!(
        criteria.tun_addresses,
        vec![
            "172.19.0.1/30".to_string(),
            "fdfe:dcba:9876::1/126".to_string()
        ]
    );
    assert_eq!(
        emitted_tun_interface_names(&config),
        vec!["polaris-tun0".to_string()]
    );
}

/// 没发射 fakeip server / 没发射 TUN 入站 ⇒ 对应判据段为空（那一类恒不报，不是"退化成全匹配"）。
#[test]
fn absent_emission_yields_empty_criteria() {
    let config = emitted(
        serde_json::json!({ "servers": [{ "tag": "local", "type": "udp", "server": "223.5.5.5" }] }),
        serde_json::json!({ "type": "mixed", "tag": "mixed-in", "listen_port": 7890 }),
    );
    let criteria = emitted_conflict_criteria(&config, &[], &ObservedTailnetAddresses::new());
    assert!(criteria.fakeip_ranges.is_empty(), "{criteria:?}");
    assert!(criteria.tun_addresses.is_empty(), "{criteria:?}");
    assert!(
        emitted_tun_interface_names(&config).is_empty(),
        "非 TUN 模式不该冒出接口名"
    );
}

/// 🔴 **判据 5（反向对照）**：`mesh_cidrs` 用的是**带观测的那一份**。
///
/// 实验组：观测到自建 tailnet 的 `32.0.0.28` ⇒ 本机那条 `32.0.0.0/24` 隧道判出 `MeshOverlap`。
/// 对照组：同一份配置、同一份探测结果，只把观测面换成空 ⇒ **必须一条都判不出**。
///
/// 对照组红了说明实现悄悄回落到了某个硬编码段；对照组绿而实验组也绿，说明观测面根本没接上 ——
/// 两侧一起看才排得掉「断言恒真」。
#[test]
fn mesh_criteria_use_the_observed_snapshot() {
    let config = emitted(
        serde_json::json!({ "servers": [] }),
        serde_json::json!({ "type": "tun", "tag": "tun-in", "address": ["172.19.0.1/30"] }),
    );
    let servers = [ts_node("ts1")];
    let foreign = vec![route("utun4", "32.0.0.0/24")];

    let with = emitted_conflict_criteria(&config, &servers, &observed("ts1", &[OBSERVED_V4]));
    assert!(
        with.mesh_cidrs.contains(&format!("{OBSERVED_V4}/32")),
        "带观测的 mesh 段并集里没有 {OBSERVED_V4}：{:?}",
        with.mesh_cidrs
    );
    assert_eq!(
        kinds(&detect_tunnel_conflicts(&with.with_foreign(&foreign, &[]))),
        vec![ConflictKind::MeshOverlap],
        "自建 tailnet 段与本机那条外来隧道争同一段，应判出 MeshOverlap"
    );

    let without = emitted_conflict_criteria(&config, &servers, &ObservedTailnetAddresses::new());
    assert!(
        detect_tunnel_conflicts(&without.with_foreign(&foreign, &[])).is_empty(),
        "无观测时 mesh 段只有硬编码的 {} 两段，与 32.0.0.0/24 不相交，不该判出任何冲突 —— \
         这里判出来了说明上面那条实验组的绿不是观测面带来的。实际段：{:?}",
        crate::builder::endpoint_routes::TAILNET_CGNAT,
        without.mesh_cidrs
    );
}

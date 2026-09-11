use super::*;

/// 2026-09-08 本机 `ip -o route show` 与 `ip -o -6 route show` 的**真实**输出片段。
///
/// 逐字保留（含行尾空格与多路径行里的字面量 `\` + 制表符）——
/// 这两样正是照着记忆写不出来的部分，一旦被"整理"掉，本组测试就退化成自证。
const REAL_V4: &str = "default via 192.168.10.1 dev eno1 proto dhcp src 192.168.10.218 metric 100 \n192.168.10.0/24 dev eno1 proto kernel scope link src 192.168.10.218 metric 100 \n";

const REAL_V6: &str = "2408:8248:4c30:c740::be1 dev eno1 proto kernel metric 100 pref medium\n2408:8248:4c30:c740::/64 dev eno1 proto ra metric 100 pref medium\n2408:8248:4c30:c740::/60 via fe80::290:27ff:fef4:29b3 dev eno1 proto ra metric 100 pref medium\n2409:8a34:4c13:fd41::/64 proto ra metric 105 pref medium\\\t nexthop via fe80::1840:87c8:c132:e8f4 dev eno1 weight 1 \\\t nexthop via fe80::14f5:aad9:6ac3:7957 dev eno1 weight 1 \n";

#[test]
fn parses_real_v4_output() {
    let routes = parse_ip_routes(REAL_V4);
    assert_eq!(
        routes,
        vec![RouteEntry {
            prefix: "192.168.10.0/24".into(),
            interface: "eno1".into()
        }],
        "default 行应被跳过，只剩具体网段那条"
    );
}

/// v6 的两种真实形态：主机路由无 `/前缀`、多路径路由顶层无 `dev`。
#[test]
fn parses_real_v6_output_including_host_route_and_multipath() {
    let routes = parse_ip_routes(REAL_V6);
    // 主机路由补 /128。
    assert!(
        routes.contains(&RouteEntry {
            prefix: "2408:8248:4c30:c740::be1/128".into(),
            interface: "eno1".into()
        }),
        "主机路由（无 /前缀）没被补成 /128：{routes:?}"
    );
    // 多路径行**不得**产出条目：它顶层没有 dev，`dev` 只在各 nexthop 段里。
    assert!(
        !routes
            .iter()
            .any(|r| r.prefix == "2409:8a34:4c13:fd41::/64"),
        "多路径路由不该被当成单一接口的宣告（会把 nexthop 的 dev 张冠李戴）：{routes:?}"
    );
    // 正常的 /64 与 /60 照收。
    assert!(routes
        .iter()
        .any(|r| r.prefix == "2408:8248:4c30:c740::/64"));
    assert!(routes
        .iter()
        .any(|r| r.prefix == "2408:8248:4c30:c740::/60"));
}

/// 负向对照：把多路径行的 `\` 分隔去掉（= 假装它是普通行），必须开始误判。
///
/// 没有这条，"跳过多路径"这件事就只是一句注释 —— 谁都不知道它有没有在起作用。
#[test]
fn multipath_guard_actually_does_something() {
    let flattened = REAL_V6.replace('\\', " ");
    let routes = parse_ip_routes(&flattened);
    assert!(
        routes
            .iter()
            .any(|r| r.prefix == "2409:8a34:4c13:fd41::/64" && r.interface == "eno1"),
        "去掉 `\\` 分隔后本该误判成 eno1 —— 若这里也不误判，说明跳过多路径的判据没在生效"
    );
}

/// 隧道接口宣告的网段（形态取自同一份 `ip -o route show`，只有 `dev` 换成隧道名）。
#[test]
fn tunnel_routes_are_extracted() {
    let stdout = "100.64.0.0/10 dev tailscale0 proto static scope link metric 1002 \n\
                  192.168.10.0/24 dev eno1 proto kernel scope link src 192.168.10.218 metric 100 \n";
    let routes = parse_ip_routes(stdout);
    let foreign = foreign_tunnel_routes(&routes, &["tailscale0".into()], &[]);
    assert_eq!(
        foreign,
        vec![RouteEntry {
            prefix: "100.64.0.0/10".into(),
            interface: "tailscale0".into()
        }]
    );
}

/// **必须排除 Polaris 自己的 TUN**，否则自己的路由会被当成"别人的隧道"，
/// 与自己的 mesh/FakeIP 段一比就冒出一堆自指告警。
#[test]
fn own_tun_is_excluded() {
    let stdout = "198.18.0.0/15 dev polaris-tun proto static metric 1 \n";
    let routes = parse_ip_routes(stdout);
    let all = foreign_tunnel_routes(&routes, &["polaris-tun".into()], &[]);
    assert_eq!(all.len(), 1, "前提：不排除时它确实在列");
    let foreign = foreign_tunnel_routes(&routes, &["polaris-tun".into()], &["polaris-tun".into()]);
    assert!(
        foreign.is_empty(),
        "自己的 TUN 被当成外来隧道了：{foreign:?}"
    );
}

/// `ip -o link show type tun` 的名字解析，含 `@父接口` 后缀。
#[test]
fn parses_link_names() {
    let stdout = "7: tailscale0: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1280 qdisc pfifo_fast state UNKNOWN mode DEFAULT group default qlen 500\\    link/none \n\
                  9: wg0@eno1: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420 qdisc noqueue state UNKNOWN mode DEFAULT group default \\    link/none \n";
    assert_eq!(
        parse_ip_link_names(stdout),
        vec!["tailscale0".to_string(), "wg0".to_string()]
    );
}

/// 本机实测的**阴性对照**：没有隧道接口时 `ip -o link show type tun` 输出为空 ⇒ 解析出空表。
/// 这条钉的是"空输入不许变成全匹配"。
#[test]
fn empty_link_output_yields_no_interfaces() {
    assert!(parse_ip_link_names("").is_empty());
    assert!(parse_ip_link_names("\n\n").is_empty());
}

// ══════════ 宿主探测接线（命令经 mock 注入；**绝不 spawn `ip`**）══════════

use crate::exec::exec_tests_helpers::MockRunner;
use std::cell::RefCell;

/// 本机 2026-09-11 实测：`ip -o link show type tun` 在**有**隧道时的形态（本机当时无隧道，
/// 故接口行沿用本文件既有的 `parses_link_names` 样例，只有路由行的 `dev` 换成隧道名）。
const TUN_LINKS: &str = "7: tailscale0: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1280 qdisc pfifo_fast state UNKNOWN mode DEFAULT group default qlen 500\\    link/none \n";
const WG_LINKS: &str = "9: wg0@eno1: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420 qdisc noqueue state UNKNOWN mode DEFAULT group default \\    link/none \n";

fn queued(stdouts: &[&str]) -> MockRunner {
    MockRunner {
        stdouts: RefCell::new(stdouts.iter().map(|s| (*s).to_string()).collect()),
        ..Default::default()
    }
}

/// Linux 腿跑的**就是那四条只读查询**，顺序与 argv 逐字钉死。
///
/// 钉 argv 而不只钉结果：解析器只认 `ip -o` 的一行一记录形态，谁把 `-o` 去掉、或把
/// `route show` 换成 `route list`（同义但输出不同）都会让解析静默退化成空表 —— 而空表
/// 恰好长得像「没有外来隧道」。
#[test]
fn linux_probe_runs_the_four_read_only_queries_in_order() {
    let runner = queued(&[
        "100.64.0.0/10 dev tailscale0 proto static scope link metric 1002 \n",
        "fd7a:115c:a1e0::/48 dev tailscale0 proto static metric 1002 pref medium\n",
        TUN_LINKS,
        WG_LINKS,
    ]);
    let probe = ForeignTunnelProbeImpl::with_platform(runner, Platform::Linux);
    let outcome = probe.probe_foreign_tunnels(&[]).expect("四条命令都成功");

    let calls: Vec<Vec<String>> = probe
        .runner
        .snapshot()
        .into_iter()
        .map(|c| {
            let mut argv = vec![c.program];
            argv.extend(c.args);
            argv
        })
        .collect();
    assert_eq!(
        calls,
        vec![
            vec!["ip", "-o", "route", "show"],
            vec!["ip", "-o", "-6", "route", "show"],
            vec!["ip", "-o", "link", "show", "type", "tun"],
            vec!["ip", "-o", "link", "show", "type", "wireguard"],
        ],
        "探测腿跑的命令与解析器的取材形态必须对得上"
    );

    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("Linux 有解析实现，应走 Probed 那一支");
    };
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec!["tailscale0".to_string(), "wg0".to_string()],
        "tun 与 wireguard 两张表要合并去重"
    );
    assert_eq!(
        snapshot.foreign,
        vec![
            RouteEntry {
                prefix: "100.64.0.0/10".into(),
                interface: "tailscale0".into()
            },
            RouteEntry {
                prefix: "fd7a:115c:a1e0::/48".into(),
                interface: "tailscale0".into()
            },
        ],
        "v4 与 v6 两份路由都要进结果"
    );
}

/// 🔴 **判据 3（平台缺席自曝）**：mac / win / 未知平台返回的是「未实现」，**不是空冲突列表**。
///
/// 断言写成两条：既要 `Unsupported` 成立，也要显式否掉 `Probed`（哪怕是空的 `Probed`）——
/// 只断言前者的话，把这支改成 `Probed(默认值)` 时 `matches!` 那一行会红、而"没有冲突"
/// 这个更危险的读法不会被点名。同时钉住**一条命令都没跑**：非 Linux 上没有可解析的命令，
/// 跑了也只是拿到一份读不懂的输出。
#[test]
fn unimplemented_platforms_report_unsupported_not_an_empty_list() {
    for platform in [Platform::Mac, Platform::Win, Platform::Other] {
        let probe = ForeignTunnelProbeImpl::with_platform(MockRunner::default(), platform);
        let outcome = probe
            .probe_foreign_tunnels(&[])
            .expect("未实现不是错误，是一种如实登记");
        assert_eq!(
            outcome,
            TunnelProbeOutcome::Unsupported(platform),
            "{platform:?} 没有解析实现，必须如实登记"
        );
        assert!(
            !matches!(outcome, TunnelProbeOutcome::Probed(_)),
            "{platform:?} 返回了 Probed —— 下游会把它读成「看过了，没有冲突」"
        );
        assert!(
            probe.runner.snapshot().is_empty(),
            "{platform:?} 上不该跑任何查询命令"
        );
    }
}

/// 命令失败 → `Err`，同样不是「无冲突」。反向对照：同一组输入不失败时是 `Ok`。
#[test]
fn command_failure_is_an_error_not_an_empty_list() {
    let failing = MockRunner {
        fail_programs: vec!["ip".to_string()],
        ..Default::default()
    };
    let probe = ForeignTunnelProbeImpl::with_platform(failing, Platform::Linux);
    assert!(
        probe.probe_foreign_tunnels(&[]).is_err(),
        "`ip` 跑不起来时必须报错，不能折成一份空的探测事实"
    );

    // 正向对照：命令不失败时同一条腿是 Ok —— 证明上面那条红的不是「Linux 腿根本走不通」。
    let ok = ForeignTunnelProbeImpl::with_platform(queued(&["", "", "", ""]), Platform::Linux);
    assert!(ok.probe_foreign_tunnels(&[]).is_ok());
}

/// 🔴 **判据 4（探测器不误吞自己）**：Polaris 自己的 TUN 不得被当成外来隧道。
///
/// 带正向对照（不传 `own_interfaces` 时它确实在列），否则"排除生效了"无从证伪。
#[test]
fn own_tun_is_excluded_through_the_probe_leg() {
    let stdouts = [
        "198.18.0.0/15 dev polaris-tun0 proto static metric 1 \n100.64.0.0/10 dev tailscale0 proto static scope link metric 1002 \n",
        "",
        "5: polaris-tun0: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 9000 qdisc fq state UNKNOWN mode DEFAULT group default qlen 500\\    link/none \n7: tailscale0: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1280 qdisc pfifo_fast state UNKNOWN mode DEFAULT group default qlen 500\\    link/none \n",
        "",
    ];

    // 正向对照：`own_interfaces` 为空 ⇒ 自己的 TUN 也在 foreign 里。
    let bare = ForeignTunnelProbeImpl::with_platform(queued(&stdouts), Platform::Linux);
    let TunnelProbeOutcome::Probed(without) = bare.probe_foreign_tunnels(&[]).unwrap() else {
        panic!("Linux 腿应走 Probed");
    };
    assert!(
        without
            .foreign
            .iter()
            .any(|r| r.interface == "polaris-tun0"),
        "前提：不排除时自己的 TUN 确实在列，否则下一条断言恒真：{:?}",
        without.foreign
    );

    let probe = ForeignTunnelProbeImpl::with_platform(queued(&stdouts), Platform::Linux);
    let TunnelProbeOutcome::Probed(with) = probe
        .probe_foreign_tunnels(&["polaris-tun0".to_string()])
        .unwrap()
    else {
        panic!("Linux 腿应走 Probed");
    };
    assert!(
        !with.foreign.iter().any(|r| r.interface == "polaris-tun0"),
        "自己的 TUN 被当成外来隧道了：{:?}",
        with.foreign
    );
    assert!(
        with.foreign.iter().any(|r| r.interface == "tailscale0"),
        "排除自己不该把别人的隧道一起丢掉：{:?}",
        with.foreign
    );
    assert!(
        with.tunnel_interfaces.contains(&"polaris-tun0".to_string()),
        "`tunnel_interfaces` 是「本机有哪些隧道」的原始事实，剔除只作用在 foreign 上"
    );
}

/// 本机实测的阴性对照：四条命令全空（本机当前形态）⇒ 探到的是**空的 Probed**，
/// 与「未实现」在类型上不同 —— 这条钉的正是那个区别不许被抹掉。
#[test]
fn empty_host_yields_empty_probed_not_unsupported() {
    let probe = ForeignTunnelProbeImpl::with_platform(queued(&["", "", "", ""]), Platform::Linux);
    assert_eq!(
        probe.probe_foreign_tunnels(&[]).unwrap(),
        TunnelProbeOutcome::Probed(ForeignTunnelSnapshot::default())
    );
}

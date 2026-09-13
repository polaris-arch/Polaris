use super::*;

/// 真机抓取样本 → 解析器的验证 harness（枚举 `tests/fixtures/`，缺席的平台逐字点名）。
mod fixture_harness;

/// 「外来隧道抢默认路由」那一类的门：回归门（默认路由永不进 `foreign`）+ 正向/负向对照。
mod default_route_gate;

/// 2026-09-08 本机 `ip -o route show` 与 `ip -o -6 route show` 的**真实**输出片段。
///
/// 逐字保留（含行尾空格与多路径行里的字面量 `\` + 制表符）——
/// 这两样正是照着记忆写不出来的部分，一旦被"整理"掉，本组测试就退化成自证。
const REAL_V4: &str = "default via 192.168.10.1 dev eno1 proto dhcp src 192.168.10.218 metric 100 \n192.168.10.0/24 dev eno1 proto kernel scope link src 192.168.10.218 metric 100 \n";

const REAL_V6: &str = "2408:8248:4c30:c740::be1 dev eno1 proto kernel metric 100 pref medium\n2408:8248:4c30:c740::/64 dev eno1 proto ra metric 100 pref medium\n2408:8248:4c30:c740::/60 via fe80::290:27ff:fef4:29b3 dev eno1 proto ra metric 100 pref medium\n2409:8a34:4c13:fd41::/64 proto ra metric 105 pref medium\\\t nexthop via fe80::1840:87c8:c132:e8f4 dev eno1 weight 1 \\\t nexthop via fe80::14f5:aad9:6ac3:7957 dev eno1 weight 1 \n";

#[test]
fn parses_real_v4_output() {
    let routes = parse_ip_routes(REAL_V4, IpFamily::V4);
    assert_eq!(
        routes,
        vec![
            // `default via …` 逐字没有 `/0`，规范成 v4 的默认路由前缀。族由调用方给
            // （见 `IpFamily` 头注：这一行里没有任何一个字节能区分 v4 与 v6）。
            RouteEntry {
                prefix: "0.0.0.0/0".into(),
                interface: "eno1".into()
            },
            RouteEntry {
                prefix: "192.168.10.0/24".into(),
                interface: "eno1".into()
            }
        ],
        "default 行应规范成 0.0.0.0/0 产出条目（分桶在 foreign_tunnel_routes 那一层做）"
    );
}

/// v6 的两种真实形态：主机路由无 `/前缀`、多路径路由顶层无 `dev`。
#[test]
fn parses_real_v6_output_including_host_route_and_multipath() {
    let routes = parse_ip_routes(REAL_V6, IpFamily::V6);
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
    let routes = parse_ip_routes(&flattened, IpFamily::V6);
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
    let routes = parse_ip_routes(stdout, IpFamily::V4);
    let split = foreign_tunnel_routes(&routes, &["tailscale0".into()], &[]);
    assert_eq!(
        split.foreign,
        vec![RouteEntry {
            prefix: "100.64.0.0/10".into(),
            interface: "tailscale0".into()
        }]
    );
    assert!(
        split.default_routes.is_empty(),
        "这份输入里没有默认路由，另一个桶该是空的：{:?}",
        split.default_routes
    );
}

/// **必须排除 Polaris 自己的 TUN**，否则自己的路由会被当成"别人的隧道"，
/// 与自己的 mesh/FakeIP 段一比就冒出一堆自指告警。
#[test]
fn own_tun_is_excluded() {
    let stdout = "198.18.0.0/15 dev polaris-tun proto static metric 1 \n";
    let routes = parse_ip_routes(stdout, IpFamily::V4);
    let all = foreign_tunnel_routes(&routes, &["polaris-tun".into()], &[]);
    assert_eq!(all.foreign.len(), 1, "前提：不排除时它确实在列");
    let split = foreign_tunnel_routes(&routes, &["polaris-tun".into()], &["polaris-tun".into()]);
    assert!(
        split.foreign.is_empty(),
        "自己的 TUN 被当成外来隧道了：{:?}",
        split.foreign
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

/// 2026-09-13 VM185 实测：`ip -o link show type ovpn` 的**逐字**输出（OpenVPN 2.7 的 ovpn-dco
/// 设备）。名字是 `tun0`/`tun1`，`link/none`，而 `type tun` 那张表里**一条都没有**它们。
///
/// 这里只取一条（`tun0`），两条那份在夹具里 ——
/// `fixture_harness::linux_leg_on_the_ovpn_dco_capture_sees_tun0_tun1_and_tap0`。
const OVPN_LINKS: &str = "4: tun0: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1500 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/none \n";

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
fn linux_probe_runs_the_five_read_only_queries_in_order() {
    let runner = queued(&[
        "100.64.0.0/10 dev tailscale0 proto static scope link metric 1002 \n10.8.0.0/24 dev tun0 proto kernel scope link src 10.8.0.1 \n",
        "fd7a:115c:a1e0::/48 dev tailscale0 proto static metric 1002 pref medium\n",
        TUN_LINKS,
        WG_LINKS,
        OVPN_LINKS,
    ]);
    let probe = ForeignTunnelProbeImpl::with_platform(runner, Platform::Linux);
    let outcome = probe.probe_foreign_tunnels(&[]).expect("五条命令都成功");

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
            // 🔴 第五条是 2026-09-13 补的：OpenVPN 2.7 默认走 ovpn-dco，设备 link type 是
            // `ovpn` 而**不是** `tun`（设备名却仍叫 `tunN`）。没有这条，装着 OpenVPN 的
            // Linux 机器上一条隧道都探不到 —— 又一句自信的「无冲突」。
            vec!["ip", "-o", "link", "show", "type", "ovpn"],
        ],
        "探测腿跑的命令与解析器的取材形态必须对得上"
    );

    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("Linux 有解析实现，应走 Probed 那一支");
    };
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec![
            "tailscale0".to_string(),
            "wg0".to_string(),
            "tun0".to_string()
        ],
        "tun / wireguard / ovpn 三张表要合并去重（顺序 = 查询序）"
    );
    assert_eq!(
        snapshot.foreign,
        vec![
            RouteEntry {
                prefix: "100.64.0.0/10".into(),
                interface: "tailscale0".into()
            },
            // 🔴 正向对照：ovpn 那张表带进来的接口，它宣告的网段也要进 `foreign`。
            // 只断 `tunnel_interfaces` 的话，「名字进了名单但路由没落上去」这半没有输入。
            RouteEntry {
                prefix: "10.8.0.0/24".into(),
                interface: "tun0".into()
            },
            RouteEntry {
                prefix: "fd7a:115c:a1e0::/48".into(),
                interface: "tailscale0".into()
            },
        ],
        "v4 与 v6 两份路由都要进结果"
    );
}

/// 🔴 **判据 3（平台缺席自曝）**：未知平台返回的是「未实现」，**不是空冲突列表**。
///
/// 断言写成两条：既要 `Unsupported` 成立，也要显式否掉 `Probed`（哪怕是空的 `Probed`）——
/// 只断言前者的话，把这支改成 `Probed(默认值)` 时 `matches!` 那一行会红、而"没有冲突"
/// 这个更危险的读法不会被点名。同时钉住**一条命令都没跑**：未知平台上没有可解析的读法，
/// 跑了也只是拿到一份读不懂的输出。
///
/// **`Platform::Mac` 与 `Platform::Win` 先后从这张名单里搬走了**（2026-09-12 两份真机抓取
/// 到位，两条腿都成了真实现），它们的正向断言在
/// `macos_probe_runs_the_three_read_only_queries_in_order` 与
/// `windows_probe_runs_the_four_read_only_queries_in_order`。名单上只剩 `Other`：
/// freebsd/openbsd 之类，仓里一份抓取都没有。
#[test]
fn unimplemented_platforms_report_unsupported_not_an_empty_list() {
    /// 名单写成常量而不是行内数组：它现在只剩一个，但「这是一张会增减的名单」这件事
    /// 得在代码里看得见 —— 下一个没有抓取的平台加在这里。
    const UNSUPPORTED: &[Platform] = &[Platform::Other];

    for platform in UNSUPPORTED.iter().copied() {
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
    let ok = ForeignTunnelProbeImpl::with_platform(queued(&["", "", "", "", ""]), Platform::Linux);
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
    let probe =
        ForeignTunnelProbeImpl::with_platform(queued(&["", "", "", "", ""]), Platform::Linux);
    assert_eq!(
        probe.probe_foreign_tunnels(&[]).unwrap(),
        TunnelProbeOutcome::Probed(ForeignTunnelSnapshot::default())
    );
}

// ══════════ macOS 腿（2026-09-12 起是真实现；输入仍是注入，绝不 spawn `netstat`）══════════

/// 2026-09-12 那份 macOS 全套抓取的某个分节。
fn macos_section(id: &str) -> String {
    fixture_section(MAC_TS_OFF, id)
}

/// 两份 macOS 全套抓取：**断开态** / **连接态**（`utun11` 连上自建 headscale）。
const MAC_TS_OFF: &str = "macos-p101-routes-ts-off-2026-09-12.txt";
const MAC_TS_ON: &str = "macos-p101-routes-ts-on-2026-09-12.txt";

/// 拿某份 macOS 抓取的三个判据分节，跑一遍**真正的** macOS 腿。
fn macos_leg(file: &str) -> ForeignTunnelSnapshot {
    let sections = [
        fixture_section(file, "V4"),
        fixture_section(file, "V6"),
        fixture_section(file, "IFCONFIG"),
    ];
    let args: Vec<&str> = sections.iter().map(String::as_str).collect();
    let probe = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Mac);
    match probe.probe_foreign_tunnels(&[]) {
        Ok(TunnelProbeOutcome::Probed(snapshot)) => snapshot,
        other => panic!("{file}：macOS 腿应走 Probed，实际 {other:?}"),
    }
}

/// macOS 腿跑的**就是那三条只读查询**，顺序与 argv 逐字钉死；喂的是**真机抓取**。
///
/// 钉 argv 的理由与 Linux 那条同：解析器照 `netstat -rn -f inet` 与**未过滤**的 `ifconfig -a`
/// 写的。谁把 `-f inet` 去掉（两张表连在一起）、或把 `ifconfig -a` 换成 `ifconfig utun0`
/// 之类的过滤版，解析会静默退化 —— 而退化的方向恰好是「隧道更少 / 路由更少」，
/// 也就是长得像「没有外来隧道」。
#[test]
fn macos_probe_runs_the_three_read_only_queries_in_order() {
    let v4 = macos_section("V4");
    let v6 = macos_section("V6");
    let ifconfig = macos_section("IFCONFIG");
    let probe = ForeignTunnelProbeImpl::with_platform(
        queued(&[v4.as_str(), v6.as_str(), ifconfig.as_str()]),
        Platform::Mac,
    );
    let outcome = probe.probe_foreign_tunnels(&[]).expect("三条命令都成功");

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
            vec!["netstat", "-rn", "-f", "inet"],
            vec!["netstat", "-rn", "-f", "inet6"],
            vec!["ifconfig", "-a"],
        ],
        "探测腿跑的命令与解析器的取材形态必须对得上（含**未过滤**的 `ifconfig -a`）"
    );

    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("mac 腿在 2026-09-12 之后是真实现，应走 Probed");
    };
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec![
            "gif0", "utun0", "utun1", "utun2", "utun3", "utun4", "utun5", "utun6", "utun7", "utun8"
        ],
        "隧道名单与真机 `ifconfig -a` 对不上"
    );
    assert!(
        snapshot
            .foreign
            .iter()
            .any(|r| r.prefix == "fe80::/64" && r.interface == "utun0"),
        "utun0 宣告的网段没进 foreign：{:?}",
        snapshot.foreign
    );
    assert!(
        snapshot
            .foreign
            .iter()
            .all(|r| r.interface.starts_with("utun")),
        "foreign 里混进了非隧道接口的路由：{:?}",
        snapshot.foreign
    );
}

/// 🔴 **断开态那份是噪声侧的负样本**：`foreign` 里只有各 utun 的 link-local 与组播，
/// 一条 tailnet 业务网段都没有。
///
/// 这条**不再是缺口登记**（业务网段那一半由 [`macos_leg_on_the_connected_capture_keeps_business_prefixes`]
/// 用连接态抓取证了）。它现在的职责是过滤器的**负样本**：一个「探过了、隧道也在、确实没有
/// 业务网段可报」的真机形态。两份合起来才是一对完整的正负样本 —— 只有正样本的话，
/// 「过滤器该收的都收了」这半没有任何输入。
///
/// ⚠️ 判「这份是不是断开态」**不能看 `@@@TAILSCALE`**：两份抓取的该分节都是「命令不在
/// PATH 上」哨兵（Mac App Store 版不装 CLI）。真值在路由表里。
#[test]
fn macos_probe_sample_is_tailscale_off_and_says_so() {
    let probe = ForeignTunnelProbeImpl::with_platform(
        queued(&[
            macos_section("V4").as_str(),
            macos_section("V6").as_str(),
            macos_section("IFCONFIG").as_str(),
        ]),
        Platform::Mac,
    );
    let TunnelProbeOutcome::Probed(snapshot) = probe.probe_foreign_tunnels(&[]).unwrap() else {
        panic!("mac 腿应走 Probed");
    };
    // 正向对照先行：utun 上**确实**有路由，否则下面那条否定断言是假绿。
    assert!(
        !snapshot.foreign.is_empty(),
        "foreign 是空的 —— 下面那条否定断言没有信息量"
    );
    // 判据取**单点实现**而不是本地再写一遍 `starts_with`：这条断言与生产展示面收的是同一族东西，
    // 两份口径迟早会漂（此前这里的 `starts_with("ff0")` 就比 `ff00::/8` 宽）。它顺带成了
    // `is_link_local_or_multicast` 在**真机形态**上的正向对照。
    let business: Vec<&RouteEntry> = snapshot
        .foreign
        .iter()
        .filter(|r| !is_link_local_or_multicast(&r.prefix))
        .collect();
    assert!(
        business.is_empty(),
        "这份抓取里出现了非 link-local / 非组播的隧道网段 {business:?} —— \
         「样本不含连接态 utun」这条登记已经过期，请同步更新 fixtures/README.md 的覆盖表，\
         并给「外来隧道的业务网段被摘出来」补一条真样本断言"
    );
}

/// 🔴 **连接态 macOS 抓取：外来隧道宣告的业务网段被摘出来，一条都不被噪声过滤器吃掉**。
///
/// 这是 macOS 侧「隧道宣告的业务网段 → 判定面」这条链路的**第一份真样本**。
///
/// # 三个照记忆写不出来的形态
///
/// 1. **逐 peer 的 `/32` 主机路由**，不是一条 `32.0.0.0/24` 汇总段；
/// 2. **本机自己那条 tailnet v6 `/128` 挂在 `lo0` 上**（BSD 把本地地址的主机路由装到环回），
///    于是它**压根进不了 `foreign`** —— `foreign_tunnel_routes` 按接口名过滤，`lo0` 不在隧道
///    名单里。Windows 侧相反：本机那条 `/32` 就在 wintun 适配器上
///    （见 [`windows_leg_on_the_connected_capture_keeps_business_prefixes`]）。
///    同一个 Tailscale、同一个 tailnet，两个平台的路由表形态不同；
/// 3. **`255.255.255.255/32` 也挂在 utun11 上**。这条抓取当初暴露了噪声块的一个缺口：
///    组播块 `224.0.0.0/4` 只盖 224–239，受限广播落在 `240.0.0.0/4` ⇒ 它一度作为
///    「业务网段」进展示面。2026-09-12 已把**那一个地址**（不是整个 `240.0.0.0/4`）收进
///    [`LINK_LOCAL_AND_MULTICAST_BLOCKS`]，理由见该常量的头注。
///    下面的业务网段名单因此**不含**它 —— 由
///    [`macos_connected_capture_treats_limited_broadcast_as_noise`] 正反两面钉住。
#[test]
fn macos_leg_on_the_connected_capture_keeps_business_prefixes() {
    let snapshot = macos_leg(MAC_TS_ON);
    assert!(
        snapshot.tunnel_interfaces.iter().any(|t| t == "utun11"),
        "连上 headscale 的 utun11 不在隧道名单里：{:?}",
        snapshot.tunnel_interfaces
    );

    let on_tunnel = |prefix: &str| {
        snapshot
            .foreign
            .iter()
            .any(|r| r.prefix == prefix && r.interface == "utun11")
    };
    // 逐 peer 主机路由：头、中、尾各点一个。
    for peer in ["32.0.0.1/32", "32.0.0.15/32", "32.0.0.29/32"] {
        assert!(on_tunnel(peer), "逐 peer 主机路由 {peer} 没进 foreign");
    }
    // 🔴 本机那条在 `netstat` 里逐字是 `32.0.0.30  32.0.0.30  UH`（**没有 `/32`**）——
    // 靠「主机路由补前缀」那条规则才落到 /32 上。
    assert!(
        on_tunnel("32.0.0.30/32"),
        "无 `/前缀` 的 UH 主机路由没补成 /32"
    );
    assert!(on_tunnel("100.100.100.100/32"), "MagicDNS 那条没进 foreign");
    // v6：ULA 段本体 + 一条 /128。
    for v6 in ["fd7a:115c:a1e0::/48", "fd7a:115c:a1e0::16/128"] {
        assert!(on_tunnel(v6), "tailnet v6 网段 {v6} 没进 foreign");
    }
    // 形态②：本机自己那条 v6 /128 在 lo0 上 ⇒ 不在 foreign 里。
    assert!(
        !snapshot
            .foreign
            .iter()
            .any(|r| r.prefix == "fd7a:115c:a1e0::d3/128"),
        "本机那条 /128 进了 foreign —— 它在这份抓取里挂的是 `lo0`，\
         出现在这里说明接口列取错了：{:?}",
        snapshot.foreign
    );
    // 形态①：没有汇总段（正向对照是上面那几条 /32）。
    assert!(
        !snapshot.foreign.iter().any(|r| r.prefix == "32.0.0.0/24"),
        "出现了 `32.0.0.0/24` 汇总段 —— 「Tailscale 装逐 peer /32」这条形态登记过期了"
    );

    // 🔴 业务网段逐条列全：过滤器收掉的是噪声、留下的是业务，两边都钉住。
    let mut business: Vec<&str> = snapshot
        .foreign
        .iter()
        .filter(|r| !is_link_local_or_multicast(&r.prefix))
        .map(|r| r.prefix.as_str())
        .collect();
    business.sort_unstable();
    let mut want: Vec<&str> = vec![
        "32.0.0.1/32",
        "32.0.0.2/32",
        "32.0.0.3/32",
        "32.0.0.4/32",
        "32.0.0.5/32",
        "32.0.0.7/32",
        "32.0.0.8/32",
        "32.0.0.9/32",
        "32.0.0.10/32",
        "32.0.0.12/32",
        "32.0.0.13/32",
        "32.0.0.14/32",
        "32.0.0.15/32",
        "32.0.0.17/32",
        "32.0.0.19/32",
        "32.0.0.20/32",
        "32.0.0.21/32",
        "32.0.0.23/32",
        "32.0.0.24/32",
        "32.0.0.25/32",
        "32.0.0.26/32",
        "32.0.0.27/32",
        "32.0.0.28/32",
        "32.0.0.29/32",
        "32.0.0.30/32",
        "100.100.100.100/32",
        "fd7a:115c:a1e0::/48",
        "fd7a:115c:a1e0::16/128",
    ];
    want.sort_unstable();
    assert_eq!(
        business, want,
        "连接态抓取的业务网段名单变了 —— 多出来的是过滤器漏收，少掉的是误杀"
    );
}

/// 🔴 受限广播是噪声 —— **正反两面都钉，且钉住「只收那一个地址」**。
///
/// 这条的由来是一个真实缺口：组播块 `224.0.0.0/4` 只盖 224–239，而 mac 连接态抓取里
/// `255.255.255.255/32` 挂在 `utun11` 上、落在 `240.0.0.0/4`，于是一度作为「业务网段」
/// 进了展示面。
///
/// 三条断言各防一种做错的方式：
///  - **前提**：那条路由确实在这份抓取的隧道接口上（否则「它是噪声」是空断言 —— 什么都没有
///    的时候任何过滤器都"正确");
///  - **正向**：判据认它是噪声，且它不在业务名单里；
///  - **未放宽**：`240.0.0.0/4` 里的其它地址**仍然**被当业务段。收整块等于凭「反正没人用」
///    放宽判据，而判据要能说出每一条为什么按定义不可能。
#[test]
fn macos_connected_capture_treats_limited_broadcast_as_noise() {
    let snapshot = macos_leg(MAC_TS_ON);

    // 前提：这份抓取里它真的在隧道接口上。
    assert!(
        snapshot
            .foreign
            .iter()
            .any(|r| r.prefix == "255.255.255.255/32"),
        "前提塌了：这份抓取的隧道接口上没有 255.255.255.255/32，下面两条断言就没有信息量"
    );

    // 正向：判据认它是噪声。
    assert!(
        is_link_local_or_multicast("255.255.255.255/32"),
        "受限广播必须被判成噪声"
    );

    // 未放宽：同在 240.0.0.0/4 里的其它地址不许被顺带收掉。
    for kept in [
        "240.0.0.1/32",
        "250.1.2.3/32",
        "255.255.255.254/32",
        "240.0.0.0/4",
    ] {
        assert!(
            !is_link_local_or_multicast(kept),
            "{kept} 被判成噪声了 —— 说明收的是整个 240.0.0.0/4 而不是受限广播那一个地址"
        );
    }
}

/// [`is_link_local_or_multicast`] 的判据面：两族收、其余一律留，**双向**各有正样本。
///
/// 最要紧的两条都在 `false` 那半：
///  - `fd7a:115c:a1e0::/48` 是 Tailscale 的 tailnet v6 段，住在 `fc00::/7` 里 —— 把「私网」
///    一起收掉的写法会在这里误杀掉唯一那条真要报的宣告；
///  - `224.0.0.0/3` / `::/0` 比块本身更宽 —— 判据若写成「相交」而不是「包含」，它们会被收掉，
///    而它们覆盖的可路由空间远不止组播。
#[test]
fn link_local_and_multicast_are_the_only_thing_classified_as_noise() {
    // 左半：2026-09-12 真机抓取里逐字出现的四种形态 + 两个块本体。
    for noisy in [
        "fe80::/64",
        "ff00::/8",
        "ff01::/32",
        "ff02::/32",
        "fe80::/10",
        "169.254.0.0/16",
        "169.254.0.0/24",
        "224.0.0.0/4",
        "224.0.0.0/24",
        "239.255.255.250/32",
    ] {
        assert!(
            is_link_local_or_multicast(noisy),
            "{noisy} 是 link-local / 组播，展示面该收掉"
        );
    }
    // 右半：一条都不许误杀。
    for business in [
        "fd7a:115c:a1e0::/48", // Tailscale tailnet v6（在 fc00::/7 里，但不是这两族）
        "fc00::/7",
        "fd00::/8",
        "32.0.0.0/24",   // 现场那台自建 headscale 的 tailnet v4
        "100.64.0.0/10", // Tailscale 默认 v4 段
        "198.18.0.0/15", // FakeIP 默认段
        "172.19.0.1/32",
        "192.168.10.0/24",
        "2001:2::/48",
        "224.0.0.0/3", // 比组播块更宽 —— 覆盖面不止组播，不许按「相交」收掉
        "::/0",
        "0.0.0.0/0",
    ] {
        assert!(
            !is_link_local_or_multicast(business),
            "{business} 被误判成噪声了 —— 展示面会把它藏掉"
        );
    }
}

/// 🔴 **判据 1 的探测侧一半（真机形态，不是构造的）**：2026-09-12 那份 p101 抓取过完整 mac 腿
/// 之后，`foreign` 恰好 36 条、**每一条**都是 link-local / 组播，10 个 utun 各 4 条。
///
/// 这三个数字是那台机器上的实测事实（Tailscale 断开态 = 任何一台有 utun 的 mac 的常态）。
/// 钉住它们，是因为展示面「零条」这句话的全部信息量都来自它：若哪天 `foreign` 本来就是空的，
/// 展示面为零就与过滤没有关系了。上面那条 `!snapshot.foreign.is_empty()` 是同一件事的弱形态，
/// 这里把它加强到确切条数与分布。
#[test]
fn macos_field_capture_is_36_routes_and_every_one_of_them_is_noise() {
    let snapshot = assemble_macos_probe(
        &macos_section("V4"),
        &macos_section("V6"),
        &macos_section("IFCONFIG"),
        &[],
    )
    .expect("真机抓取应解析得动");

    assert_eq!(
        snapshot.foreign.len(),
        36,
        "真机抓取的 foreign 条数变了 —— 夹具或解析器动过：{:?}",
        snapshot.foreign
    );
    let noisy = snapshot
        .foreign
        .iter()
        .filter(|r| is_link_local_or_multicast(&r.prefix))
        .count();
    assert_eq!(
        noisy, 36,
        "这份抓取里出现了业务网段 —— 判据 1 的前提（36 条全是噪声）已经不成立"
    );

    let mut per_interface: BTreeMap<&str, usize> = BTreeMap::new();
    for route in &snapshot.foreign {
        *per_interface.entry(route.interface.as_str()).or_default() += 1;
    }
    assert_eq!(
        per_interface,
        BTreeMap::from([
            ("utun0", 4),
            ("utun1", 4),
            ("utun2", 4),
            ("utun3", 4),
            ("utun4", 4),
            ("utun5", 4),
            ("utun6", 4),
            ("utun7", 4),
            ("utun8", 4),
        ]),
        "每个 utun 上恰好 fe80::/64 + ff00::/8 + ff01::/32 + ff02::/32 四条"
    );
}

/// 🔴 **半条腿不许伪装成整条腿**：`ifconfig` 那一半解析不动时整次探测是 `Err`，
/// **不是**一个隧道名单为空的 `Probed`（后者到了下游就是一句自信的「无冲突」）。
///
/// 正向对照在同一条里：同样两份路由输出、换上真 `ifconfig` 就是 `Ok`。
#[test]
fn macos_probe_fails_loudly_when_the_interface_half_is_unusable() {
    let v4 = macos_section("V4");
    let v6 = macos_section("V6");

    let blind = ForeignTunnelProbeImpl::with_platform(
        queued(&[v4.as_str(), v6.as_str(), ""]),
        Platform::Mac,
    );
    let err = blind
        .probe_foreign_tunnels(&[])
        .expect_err("`ifconfig` 空输出时必须报错，不能折成一份隧道名单为空的事实");
    assert!(
        err.to_string().contains("ifconfig -a"),
        "错误信息要说清是哪一半没读成：{err}"
    );

    let ok = ForeignTunnelProbeImpl::with_platform(
        queued(&[v4.as_str(), v6.as_str(), macos_section("IFCONFIG").as_str()]),
        Platform::Mac,
    );
    assert!(
        ok.probe_foreign_tunnels(&[]).is_ok(),
        "正向对照：三份都真时应成功 —— 否则上面那条红的可能只是「mac 腿压根走不通」"
    );
}

/// mac 腿上 `own_interfaces` 照样生效（与 Linux 那条同口径），带正向对照。
#[test]
fn own_tun_is_excluded_through_the_macos_leg() {
    let sections = [
        macos_section("V4"),
        macos_section("V6"),
        macos_section("IFCONFIG"),
    ];
    let args: Vec<&str> = sections.iter().map(String::as_str).collect();

    let bare = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Mac);
    let TunnelProbeOutcome::Probed(without) = bare.probe_foreign_tunnels(&[]).unwrap() else {
        panic!("mac 腿应走 Probed");
    };
    assert!(
        without.foreign.iter().any(|r| r.interface == "utun3"),
        "前提：不排除时 utun3 确实在列，否则下一条断言恒真：{:?}",
        without.foreign
    );

    let probe = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Mac);
    let TunnelProbeOutcome::Probed(with) =
        probe.probe_foreign_tunnels(&["utun3".to_string()]).unwrap()
    else {
        panic!("mac 腿应走 Probed");
    };
    assert!(
        !with.foreign.iter().any(|r| r.interface == "utun3"),
        "自己的 TUN 被当成外来隧道了：{:?}",
        with.foreign
    );
    assert!(
        with.foreign.iter().any(|r| r.interface == "utun0"),
        "排除自己不该把别人的隧道一起丢掉：{:?}",
        with.foreign
    );
    assert!(
        with.tunnel_interfaces.contains(&"utun3".to_string()),
        "`tunnel_interfaces` 是「本机有哪些隧道」的原始事实，剔除只作用在 foreign 上"
    );
}

// ══════════ Windows 腿（2026-09-12 重抓补上 InterfaceType 后是真实现）══════════

/// 2026-09-12 那份 Windows 全套抓取的某个分节（**装 Tailscale 之前**）。
fn windows_section(id: &str) -> String {
    fixture_harness::section(
        &fixture_harness::read_fixture("windows-w207-routes-ts-off-2026-09-12.txt"),
        id,
    )
}

/// 三份 Windows 抓取的文件名：**无 wintun** → **wintun up 未登录** → **wintun up 已登录**。
const WIN_NO_WINTUN: &str = "windows-w207-routes-ts-off-2026-09-12.txt";
const WIN_WINTUN_LOGGED_OUT: &str = "windows-w207-wintun-present-2026-09-12.txt";
const WIN_WINTUN_LOGGED_IN: &str = "windows-w207-ts-on-2026-09-12.txt";

/// 2026-09-13 同机、**L2TP 连接已建立**那份抓取（Windows RAS 族的第一份真机样本）。
const WIN_RAS_L2TP: &str = "windows-w207-ras-l2tp-connected-2026-09-13.txt";

/// 某份抓取的某个分节。
fn fixture_section(file: &str, id: &str) -> String {
    fixture_harness::section(&fixture_harness::read_fixture(file), id)
}

/// 某份抓取的某个分节，**不在就给空串**。
///
/// 只给采集脚本后来才加的分节用：老夹具里没有它们，而「老夹具缺这一节」与「新夹具的这一节是空的」
/// 对生产腿是同一件事（都没有已连接的 VPN 连接）。`fixture_section` 那条 panic 语义要留着 ——
/// 判据分节名写错时必须当场喊，不是静默空串。
fn optional_fixture_section(file: &str, id: &str) -> String {
    let raw = fixture_harness::read_fixture(file);
    fixture_harness::section_opt(&raw, id).unwrap_or_default()
}

/// 拿某份 Windows 抓取的四个判据分节，跑一遍**真正的** Windows 腿。
///
/// 走完整条腿（而不是只调解析器）是刻意的：`53` 那一支要证的不是「解析器认得这一行」，
/// 而是「这张适配器最终进了隧道名单、它宣告的路由最终进了 `foreign`」——
/// 中间还隔着 IP/索引 → 接口名的对照表，那一步漂掉的话判据认对了也没用。
fn windows_leg(file: &str) -> ForeignTunnelSnapshot {
    let sections = [
        fixture_section(file, "ROUTE_PRINT_4"),
        fixture_section(file, "ROUTE_PRINT_6"),
        fixture_section(file, "GET_NETIPADDRESS"),
        fixture_section(file, "GET_NETADAPTER"),
        // 2026-09-13 起多的两段：RAS 承载接口在 `Get-NetAdapter` 里**根本不存在**，
        // 判据只能问 Windows 自己（见 `parse_vpn_connection_names` 的头注）。
        // 老夹具里没有这两节 ⇒ 拿空串顶上：空 = 「查了，这台机器上没有 VPN 连接」，
        // 与「没查」由 `TunnelProbeOutcome` 在类型上分开，不由这个空串兼任。
        optional_fixture_section(file, "GET_VPNCONNECTION"),
        optional_fixture_section(file, "GET_VPNCONNECTION_ALLUSER"),
    ];
    let args: Vec<&str> = sections.iter().map(String::as_str).collect();
    let probe = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Win);
    match probe.probe_foreign_tunnels(&[]) {
        Ok(TunnelProbeOutcome::Probed(snapshot)) => snapshot,
        other => panic!("{file}：Windows 腿应走 Probed，实际 {other:?}"),
    }
}

/// Windows 腿跑的**就是那六条只读查询**，顺序与 argv 逐字钉死；喂的是**真机抓取**。
///
/// 钉 argv 的两条理由：
///  - 解析器照 `route print -4/-6`、**带 `InterfaceType` 的** `Get-NetAdapter`、以及
///    **两个作用域的** `Get-VpnConnection` 写的。谁把 `Select-Object` 里的 `InterfaceType` 去掉、
///    或把三列判据挪到右边（`Format-Table` 挤不下时从右边丢列），隧道名单就报缺列 ——
///    那是**要的**，但 argv 一漂移就没人知道为什么。
///  - 两条 exe 走 System32 绝对路径（PATH 缺 System32 的设备上裸名字解析不到），与
///    `route_ops` / `proxy_ops` 同一条纪律。
///
/// **第 5、6 条（`Get-VpnConnection`）2026-09-13 补**：RAS 族的承载接口不在 `Get-NetAdapter`
/// 里，少了它们，装着企业 VPN 的机器又会拿到那句自信的「无冲突」。两个作用域都钉：
/// 只跑不带参数那条，会漏掉管理员按「所有用户」下发的连接 —— 恰恰是企业配置的常见形态。
#[test]
fn windows_probe_runs_the_six_read_only_queries_in_order() {
    let v4 = windows_section("ROUTE_PRINT_4");
    let v6 = windows_section("ROUTE_PRINT_6");
    let addrs = windows_section("GET_NETIPADDRESS");
    let adapters = windows_section("GET_NETADAPTER");
    let probe = ForeignTunnelProbeImpl::with_platform(
        queued(&[v4.as_str(), v6.as_str(), addrs.as_str(), adapters.as_str()]),
        Platform::Win,
    );
    let outcome = probe.probe_foreign_tunnels(&[]).expect("六条命令都成功");

    let calls = probe.runner.snapshot();
    assert_eq!(calls.len(), 6, "应恰好跑六条：{calls:?}");
    assert!(
        calls[0].program.ends_with("\\System32\\route.exe")
            && calls[1].program.ends_with("\\System32\\route.exe"),
        "`route.exe` 应走 System32 绝对路径：{:?}",
        (&calls[0].program, &calls[1].program)
    );
    assert_eq!(calls[0].args, vec!["print", "-4"]);
    assert_eq!(calls[1].args, vec!["print", "-6"]);
    for c in &calls[2..] {
        assert!(
            c.program
                .ends_with("\\WindowsPowerShell\\v1.0\\powershell.exe"),
            "cmdlet 应走 System32 下的 powershell.exe 绝对路径：{}",
            c.program
        );
        assert_eq!(
            &c.args[..3],
            ["-NoProfile", "-NonInteractive", "-Command"],
            "与 `route_ops` 的 powershell 调用同形"
        );
        // 🔴 子进程输出编码必须显式设成 UTF-8：这台机器的控制台是 gb2312，不设的话
        // `以太网` 会被 `from_utf8_lossy` 打成一串 U+FFFD，而**不同的名字会塌成同一串**，
        // 于是「某条路由属于哪个适配器」张冠李戴。⚠️ 这一步需真机实测，本机验不了。
        assert!(
            c.args[3].starts_with("[Console]::OutputEncoding=[Text.Encoding]::UTF8;"),
            "powershell 脚本没先设输出编码：{}",
            c.args[3]
        );
    }
    assert!(
        calls[2].args[3].contains("Get-NetIPAddress")
            && calls[2].args[3].contains("InterfaceAlias")
            && calls[2].args[3].contains("InterfaceIndex"),
        "对照表那条的 Select-Object 与采集脚本对不上：{}",
        calls[2].args[3]
    );
    assert!(
        calls[3].args[3].contains("Get-NetAdapter -IncludeHidden")
            && calls[3].args[3].contains("InterfaceType"),
        "隧道名单那条必须 Select 出 InterfaceType —— 没它就是报缺列：{}",
        calls[3].args[3]
    );
    // 🔴 两个作用域的 `Get-VpnConnection`：管道与采集脚本逐字同形，且**第 6 条必须带
    // `-AllUserConnection`**（只跑第 5 条 = 漏掉企业下发的全局连接）。
    for i in [4, 5] {
        assert!(
            calls[i].args[3].contains("Get-VpnConnection")
                && calls[i].args[3].contains("ConnectionStatus")
                && calls[i].args[3].contains("TunnelType"),
            "第 {} 条的 Select-Object 与采集脚本对不上：{}",
            i + 1,
            calls[i].args[3]
        );
    }
    // 🔴 存在性守卫必须在场。`-ErrorAction SilentlyContinue` 管不了**命令不存在**
    // —— 那是解析期的 `CommandNotFoundException`，PowerShell 仍非零退出，而 `exec` 对非零
    // 一律 `Err` ⇒ 没装 `VpnClient` 模块的 SKU 上整次 Windows 探测从「探得动」退化成 `Err`。
    //
    // 这条断言是本门唯一钉住守卫的地方：上面那几条只比片段，把守卫整段删掉它们全都照旧绿
    // （实测过 —— 加守卫时 401 条一条没红，正是这个缺口）。
    for i in [4, 5] {
        assert!(
            calls[i].args[3].contains("Get-Command Get-VpnConnection"),
            "第 {} 条缺存在性守卫：没装 VpnClient 模块的机器上整次探测会变成 Err：{}",
            i + 1,
            calls[i].args[3]
        );
    }
    assert!(
        !calls[4].args[3].contains("-AllUserConnection"),
        "第 5 条不该带 -AllUserConnection（它是当前用户作用域那条）：{}",
        calls[4].args[3]
    );
    assert!(
        calls[5].args[3].contains("-AllUserConnection"),
        "第 6 条必须带 -AllUserConnection，否则两条查的是同一个作用域：{}",
        calls[5].args[3]
    );

    let TunnelProbeOutcome::Probed(snapshot) = outcome else {
        panic!("Windows 腿在 2026-09-12 重抓之后是真实现，应走 Probed");
    };
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec![
            "Teredo Tunneling Pseudo-Interface",
            "Microsoft IP-HTTPS Platform Interface",
            "6to4 Adapter",
        ],
        "隧道名单与真机 Get-NetAdapter 对不上"
    );
}

/// 🔴 **本机没有外来隧道在宣告网段 ⇒ 空的 `Probed`，与「没看」在类型上分得开**。
///
/// 那台机器的三个隧道适配器全是 `Not Present`（没起来），路由表上一条都没有 ⇒
/// `foreign` 为空。这是一句**断言**（看过了，真的没有），不是 `Unsupported`。
/// 两边都写：`foreign` 空、`tunnel_interfaces` 非空 —— 后者证明「看过了」这件事本身成立。
#[test]
fn windows_probe_yields_an_empty_but_asserted_foreign_list() {
    let probe = ForeignTunnelProbeImpl::with_platform(
        queued(&[
            windows_section("ROUTE_PRINT_4").as_str(),
            windows_section("ROUTE_PRINT_6").as_str(),
            windows_section("GET_NETIPADDRESS").as_str(),
            windows_section("GET_NETADAPTER").as_str(),
        ]),
        Platform::Win,
    );
    let TunnelProbeOutcome::Probed(snapshot) = probe.probe_foreign_tunnels(&[]).unwrap() else {
        panic!("Windows 腿应走 Probed");
    };
    assert!(
        !snapshot.tunnel_interfaces.is_empty(),
        "隧道名单是空的 —— 那下面那句「foreign 为空」就只是「没看见隧道」，不是断言"
    );
    assert!(
        snapshot.foreign.is_empty(),
        "这台机器的三个隧道适配器都是 Not Present、路由表上一条都没有，\
         foreign 本该为空：{:?}",
        snapshot.foreign
    );
    // 与「没看」在类型上分得开 —— 这正是 `TunnelProbeOutcome` 存在的全部理由。
    assert_ne!(
        TunnelProbeOutcome::Probed(snapshot),
        TunnelProbeOutcome::Unsupported(Platform::Win)
    );
}

/// 🔴 **三份 Windows 抓取走完整条腿，逐级台阶各钉一组数**（无 wintun → wintun up 未登录
/// → wintun up 已登录）。
///
/// | 抓取 | 隧道名单 | `foreign` | 其中**业务**网段 |
/// |---|---|---|---|
/// | 无 wintun | 3（只有 ifType `131` 那三个，且全是 `Not Present`） | 0 | 0 |
/// | wintun up、**未登录** | 4 | 3（全是 link-local 噪声） | **0** |
/// | wintun up、**已登录** | 4 | 30 | **30** |
///
/// 三行合起来同时否掉四种错，任何一格都不能省：
///  - 第一行 → 第二行的隧道名单 3→4 = 判据收到了 ifType `53`（旧判据在这里停在 3，
///    用户装着 Tailscale 却拿到一句「无冲突」）；
///  - 第二行的 `foreign=3 / 业务=0` = 过滤器**该收**的它收了（未登录态 wintun 上只有噪声）；
///  - 第三行的 `foreign=30 / 业务=30` = 过滤器**不该收**的它一条没碰 —— 这是仓里第一次有
///    「外来隧道宣告业务网段」的真样本；
///  - 第二行 → 第三行的隧道名单不变（4→4）= 判据不跟着连接态漂。
#[test]
fn the_three_windows_captures_step_from_no_wintun_to_business_prefixes() {
    let counts = |file: &str| -> [usize; 3] {
        let snapshot = windows_leg(file);
        let business = snapshot
            .foreign
            .iter()
            .filter(|r| !is_link_local_or_multicast(&r.prefix))
            .count();
        [
            snapshot.tunnel_interfaces.len(),
            snapshot.foreign.len(),
            business,
        ]
    };
    assert_eq!(
        counts(WIN_NO_WINTUN),
        [3, 0, 0],
        "无 wintun 那份：三个 131 隧道全是 Not Present，路由表上一条都没有"
    );
    assert_eq!(
        counts(WIN_WINTUN_LOGGED_OUT),
        [4, 3, 0],
        "wintun 未登录那份：判据要收到它（3→4），但它上面只有 link-local 噪声"
    );
    assert_eq!(
        counts(WIN_WINTUN_LOGGED_IN),
        [4, 30, 30],
        "wintun 已登录那份：30 条路由全是业务网段，一条都不许被噪声过滤器吃掉"
    );
}

/// 🔴 **未登录态那份是噪声侧的负样本**：wintun `Up` 且进了隧道名单，但它上面的 3 条路由
/// 全被展示面收掉。
///
/// 这条**不再是缺口登记**（业务网段那一半由 [`windows_leg_on_the_connected_capture_keeps_business_prefixes`]
/// 用登录态抓取证了）。它现在的职责是过滤器的**负样本**：一个「探过了、隧道也在、但确实
/// 没有业务网段可报」的真机形态 —— 与登录态那份合起来才是一对完整的正负样本。
///
/// 旧判据（只认 ifType `131`）在这份输入上的结局是：隧道名单里没有 `Tailscale`、`foreign`
/// 为空，也就是用户装着 Tailscale 却拿到一句自信的「无冲突」。
#[test]
fn windows_leg_on_the_logged_out_wintun_capture_sees_only_noise() {
    let snapshot = windows_leg(WIN_WINTUN_LOGGED_OUT);

    // 正向：隧道名单逐条列全，wintun（ifType 53）在里面。
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec![
            "Teredo Tunneling Pseudo-Interface",
            "Tailscale",
            "Microsoft IP-HTTPS Platform Interface",
            "6to4 Adapter",
        ],
        "隧道名单与真机 Get-NetAdapter 对不上（`Tailscale` 缺席 = 判据漏掉 ifType 53）"
    );

    // 正向对照：它宣告的路由真的进了 `foreign`，而且逐条都来自 wintun ——
    // 三个 131 隧道全是 `Not Present`，路由表上一条都没有。
    assert_eq!(
        snapshot
            .foreign
            .iter()
            .map(|r| (r.prefix.as_str(), r.interface.as_str()))
            .collect::<Vec<_>>(),
        [
            ("169.254.0.0/16", "Tailscale"),
            ("169.254.83.107/32", "Tailscale"),
            ("169.254.255.255/32", "Tailscale"),
        ],
        "未登录态 wintun 上的三条自动路由与这份抓取对不上：{:?}",
        snapshot.foreign
    );

    // 结论：这三条全是 link-local ⇒ 展示面一条不剩（`suppressedRoutes=3`）。
    let business: Vec<&RouteEntry> = snapshot
        .foreign
        .iter()
        .filter(|r| !is_link_local_or_multicast(&r.prefix))
        .collect();
    assert!(
        business.is_empty(),
        "未登录态 wintun 上出现了业务网段 {business:?} —— 这份抓取登记的是「服务已起、\
         tailscale status 逐字 `Logged out.`」态，若它真换成了登录态，请把它移到正样本那一侧"
    );
}

/// 🔴 **登录态 Windows 抓取：外来隧道宣告的业务网段被完整摘出来，一条都不被噪声过滤器吃掉**。
///
/// 这是 Windows 侧「隧道宣告的业务网段 → 判定面」这条链路的**第一份真样本**。
///
/// # 两个照记忆写不出来的形态
///
/// 1. **Tailscale 装的是逐 peer 的 `/32` / `/128` 主机路由**，不是一条 `32.0.0.0/24` 汇总段。
///    按「找一条汇总前缀」的直觉写断言会全落空。
/// 2. **本机自己那条 tailnet 地址的 `/32` 就在 wintun 适配器上**（`32.0.0.31` 在链路上）——
///    与 macOS 相反：那边本机的 `/128` 被 BSD 装到 `lo0` 上，压根进不了 `foreign`
///    （见 `fixture_harness::macos_tailnet_prefixes_appear_only_in_the_connected_capture`）。
///    同一个 Tailscale、同一个 tailnet，两个平台的路由表形态不同。
///
/// 另：未登录态那份 wintun 上的 3 条 `169.254.*` 噪声，在登录态这份里**一条都没有**
/// （适配器拿到真地址之后自动配置的那几条就撤了）—— 于是这份的 `foreign` 里业务网段占满。
#[test]
fn windows_leg_on_the_connected_capture_keeps_business_prefixes() {
    let snapshot = windows_leg(WIN_WINTUN_LOGGED_IN);
    assert!(
        snapshot.tunnel_interfaces.iter().any(|t| t == "Tailscale"),
        "wintun 不在隧道名单里：{:?}",
        snapshot.tunnel_interfaces
    );

    let on_tunnel = |prefix: &str| {
        snapshot
            .foreign
            .iter()
            .any(|r| r.prefix == prefix && r.interface == "Tailscale")
    };
    // 逐 peer 主机路由：第一条、中间一条、最后一条各点一个。
    for peer in ["32.0.0.1/32", "32.0.0.15/32", "32.0.0.30/32"] {
        assert!(on_tunnel(peer), "逐 peer 主机路由 {peer} 没进 foreign");
    }
    // 本机自己那条（Windows 侧在适配器上，不在环回上）。
    assert!(
        on_tunnel("32.0.0.31/32"),
        "本机 tailnet 地址那条没进 foreign"
    );
    // MagicDNS 解析器地址。
    assert!(on_tunnel("100.100.100.100/32"), "MagicDNS 那条没进 foreign");
    // v6：ULA 段本体 + 两条 /128。
    for v6 in [
        "fd7a:115c:a1e0::/48",
        "fd7a:115c:a1e0::f3/128",
        "fd7a:115c:a1e0::16/128",
    ] {
        assert!(on_tunnel(v6), "tailnet v6 网段 {v6} 没进 foreign");
    }
    // 形态：没有汇总段（正向对照就是上面那几条 /32）。
    assert!(
        !snapshot.foreign.iter().any(|r| r.prefix == "32.0.0.0/24"),
        "出现了 `32.0.0.0/24` 汇总段 —— 「Tailscale 装逐 peer /32」这条形态登记过期了"
    );

    // 🔴 结论：这 30 条**一条都不是** link-local / 组播 ⇒ 展示面一条都不许收掉。
    // 反过来说，噪声过滤器要是从「包含」漂成「相交」，`fd7a:115c:a1e0::/48`
    // （住在 `fc00::/7` 里）就是第一个被误杀的。
    let noise: Vec<&RouteEntry> = snapshot
        .foreign
        .iter()
        .filter(|r| is_link_local_or_multicast(&r.prefix))
        .collect();
    assert!(
        noise.is_empty(),
        "登录态 wintun 上的业务网段被噪声过滤器吃掉了 {noise:?} —— \
         判据该是「整条前缀落在某个块里面」，不是「与块相交」"
    );
    assert_eq!(
        snapshot.foreign.len(),
        30,
        "登录态 wintun 上的路由条数与这份抓取对不上（换样本了？）：{:?}",
        snapshot.foreign
    );
    assert!(
        snapshot.foreign.iter().all(|r| r.interface == "Tailscale"),
        "foreign 里混进了别的接口：{:?}",
        snapshot.foreign
    );
}

/// 🔴 **半条腿不许伪装成整条腿**：三半里任意一半读不成，整次探测是 `Err`。
///
/// 逐半试，每一半都带同一个正向对照（三半都真时是 `Ok`）——
/// 否则某条红的可能只是「Windows 腿压根走不通」。
#[test]
fn windows_probe_fails_loudly_when_any_half_is_unusable() {
    let v4 = windows_section("ROUTE_PRINT_4");
    let v6 = windows_section("ROUTE_PRINT_6");
    let addrs = windows_section("GET_NETIPADDRESS");
    let adapters = windows_section("GET_NETADAPTER");
    // netsh 那份是**真机抓取**，且结构上给不出 InterfaceType —— 拿它当适配器那一半，
    // 正好演一次「回退读法顶不上来」。
    let netsh = windows_section("NETSH_INTERFACE");

    for (label, stdouts, want_in_error) in [
        (
            "对照表那一半空了",
            [v4.as_str(), v6.as_str(), "", adapters.as_str()],
            "Get-NetIPAddress",
        ),
        (
            "路由表那一半空了",
            [v4.as_str(), "", addrs.as_str(), adapters.as_str()],
            "route print",
        ),
        (
            "适配器那一半换成顶不上来的 netsh",
            [v4.as_str(), v6.as_str(), addrs.as_str(), netsh.as_str()],
            "InterfaceType",
        ),
    ] {
        let probe = ForeignTunnelProbeImpl::with_platform(queued(&stdouts), Platform::Win);
        match probe.probe_foreign_tunnels(&[]) {
            Ok(outcome) => {
                panic!("{label}：竟然成功了，拿到 {outcome:?} —— 半份事实伪装成了整份")
            }
            Err(e) => assert!(
                e.to_string().contains(want_in_error),
                "{label}：错误信息没说清是哪一半没读成（期望含 `{want_in_error}`）：{e}"
            ),
        }
    }

    // 正向对照：四份都真时是 `Ok` —— 否则上面三条红的可能只是「Windows 腿压根走不通」。
    let ok = ForeignTunnelProbeImpl::with_platform(
        queued(&[v4.as_str(), v6.as_str(), addrs.as_str(), adapters.as_str()]),
        Platform::Win,
    );
    assert!(ok.probe_foreign_tunnels(&[]).is_ok(), "四份都真时应成功");
}

/// `own_interfaces` 在 Windows 腿上照样生效（名字带空格也不例外），带正向对照。
///
/// 用 `Teredo Tunneling Pseudo-Interface` 当「我方接口」是刻意的：Windows 的接口名
/// **合法地带空格**，排除逻辑是逐字比字符串，正好把这件事一起钉住。
#[test]
fn own_tun_is_excluded_through_the_windows_leg() {
    let sections = [
        windows_section("ROUTE_PRINT_4"),
        windows_section("ROUTE_PRINT_6"),
        windows_section("GET_NETIPADDRESS"),
        windows_section("GET_NETADAPTER"),
    ];
    let args: Vec<&str> = sections.iter().map(String::as_str).collect();
    let own = "Teredo Tunneling Pseudo-Interface".to_string();

    let bare = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Win);
    let TunnelProbeOutcome::Probed(without) = bare.probe_foreign_tunnels(&[]).unwrap() else {
        panic!("Windows 腿应走 Probed");
    };
    assert!(
        without.tunnel_interfaces.contains(&own),
        "前提：它确实在隧道名单里，否则下一条断言恒真：{:?}",
        without.tunnel_interfaces
    );

    let probe = ForeignTunnelProbeImpl::with_platform(queued(&args), Platform::Win);
    let TunnelProbeOutcome::Probed(with) = probe
        .probe_foreign_tunnels(std::slice::from_ref(&own))
        .unwrap()
    else {
        panic!("Windows 腿应走 Probed");
    };
    assert!(
        !with.foreign.iter().any(|r| r.interface == own),
        "我方接口被当成外来隧道了：{:?}",
        with.foreign
    );
    assert!(
        with.tunnel_interfaces.contains(&own),
        "`tunnel_interfaces` 是原始事实，剔除只作用在 foreign 上"
    );
}

// ══════════ Windows RAS（L2TP / IKEv2 / SSTP / PPTP）：ifType 白名单看不见它们 ══════════

/// 🔴 **已连接的 RAS 隧道必须进隧道接口集** —— 这是 2026-09-13 w207 实测坐实的一个真缺陷。
///
/// # 缺陷长什么样
///
/// L2TP 连接建立时，**承载流量的那个接口以 VPN 连接名为别名**（`PolarisProbeL2TP`，ifIndex 35），
/// 它宣告 `0.0.0.0/0` metric 1 —— 一条抢默认路由的全隧道。而：
///
/// ```text
/// Get-NetAdapter -IncludeHidden | Where InterfaceIndex -eq 35   →   0 条
/// ```
///
/// 也就是说隧道判据的整个取材面（`Get-NetAdapter` 的 `InterfaceType` 列）**根本看不到它**。
/// `Get-NetAdapter` 里那批 `WAN Miniport (L2TP/IKEv2/SSTP/PPTP)` 是恒 `Disconnected` 的
/// **协议模板**，它们的 ifType `131` 说明不了任何正在跑的连接（负向对照钉在
/// [`ras_protocol_template_miniports_announce_nothing`]）。
///
/// 逐个 cmdlet 问过之后的结论：**没有任何 cmdlet 给 RAS 接口一个 IANA ifType**。
/// `Get-NetIPInterface` / `netsh` / `Get-NetRoute` 看得见 ifIndex 35 但不带类型，
/// `Get-NetAdapter` / `MSFT_NetAdapter` / `Win32_NetworkAdapter` 连看都看不见。
///
/// # 那条 `0.0.0.0/0` 走的是**另一个字段**，不在 `foreign` 里
///
/// 这条隧道在抓取里最扎眼的就是那条 `0.0.0.0/0`。它是真实的宣告、也是最重的那一条，
/// 但它**不并进 `foreign`**：默认路由与**任何**前缀相交，并进去会让
/// `detect_tunnel_conflicts` 对我方每一条网段各报一次冲突。故它落在
/// [`ForeignTunnelSnapshot::default_routes`] 这条独立通道上 —— 两类事实，两个桶。
///
/// 所以本条断言的是：① 那条默认路由**确实在抓取里**；② 它**不在** `foreign`、而在
/// `default_routes` 里；③ 同一接口上**别的**网段进了 `foreign`（`10.55.0.10/32`）——
/// 缺了③，「这条隧道被看见了」就只剩名单那一半，路由那一半可能整条没接上。
///
/// 判定那一半（它被判成 `DefaultRouteContended`）由 `default_route_gate` 承担，不在这里重复。
#[test]
fn windows_leg_sees_the_connected_ras_tunnel() {
    const RAS_IFACE: &str = "PolarisProbeL2TP";
    let snapshot = windows_leg(WIN_RAS_L2TP);

    // ── ① 名单 ──
    assert!(
        snapshot.tunnel_interfaces.iter().any(|t| t == RAS_IFACE),
        "已连接的 L2TP 隧道 `{RAS_IFACE}` 不在隧道名单里 —— \
         `Get-NetAdapter` 的 ifType 白名单看不见 RAS 承载接口，判据得问 `Get-VpnConnection`。\
         名单：{:?}",
        snapshot.tunnel_interfaces
    );

    // ── ② 路由真的落到了它头上 ──
    let on_ras: Vec<&RouteEntry> = snapshot
        .foreign
        .iter()
        .filter(|r| r.interface == RAS_IFACE)
        .collect();
    assert!(
        on_ras.iter().any(|r| r.prefix == "10.55.0.10/32"),
        "`{RAS_IFACE}` 上那条隧道自身地址没进 foreign：{on_ras:?}"
    );

    // ── ③ 默认路由那条：先证它在抓取里，再证它落在另一个桶上 ──
    let raw_v4 = fixture_section(WIN_RAS_L2TP, "ROUTE_PRINT_4");
    assert!(
        raw_v4
            .lines()
            .any(|l| l.contains("0.0.0.0") && l.contains("10.55.0.10")),
        "抓取里没有 `{RAS_IFACE}` 的默认路由 —— 下面两条断言没有信息量"
    );
    assert!(
        !snapshot.foreign.iter().any(|r| r.prefix == "0.0.0.0/0"),
        "默认路由并进了 foreign —— 它与**任何**前缀相交，会让 detect_tunnel_conflicts 对我方\
         每条网段各报一次冲突：{:?}",
        snapshot.foreign
    );
    assert!(
        snapshot
            .default_routes
            .iter()
            .any(|r| r.interface == RAS_IFACE && r.prefix == "0.0.0.0/0"),
        "那条抢默认路由的全隧道两个桶里都没有 —— 它整条失联了：{:?}",
        snapshot.default_routes
    );
}

/// 🔴 **负向对照①：`Disconnected` 的 VPN 连接配置不得进隧道名单**。
///
/// `Get-VpnConnection` 列的是**配置**，断开的连接照样躺在里面。把整张表收进来就是
/// 「逢配置必报」—— 一台配了五个公司 VPN、一个都没连的机器会被判成有五条外来隧道。
///
/// 输入是**同形变异**：把那份真抓取里的 `Connected` 逐字换成 `Disconnected`，其余一字不动。
/// 变异先证明打上（替换次数 > 0），否则红绿都不算数。
#[test]
fn a_disconnected_vpn_connection_is_not_a_tunnel() {
    const RAS_IFACE: &str = "PolarisProbeL2TP";
    let raw = fixture_harness::read_fixture(WIN_RAS_L2TP);
    let vpn = fixture_harness::section(&raw, "GET_VPNCONNECTION");

    // 正向对照先行：未变异时它**确实**在名单里，否则下面那条否定断言可能只是「压根没接上」。
    assert!(
        windows_leg(WIN_RAS_L2TP)
            .tunnel_interfaces
            .iter()
            .any(|t| t == RAS_IFACE),
        "前提：未变异时 `{RAS_IFACE}` 在名单里"
    );

    let applied = vpn.matches("Connected").count();
    assert_eq!(
        applied, 1,
        "变异打上的处数不是 1 —— `Connected` 在这一节里应恰好出现一次（那一行的状态列）"
    );
    let mutated = vpn.replace("Connected", "Disconnected");
    assert!(mutated.contains("Disconnected"), "变异没打上，红绿都不算数");

    let snapshot = assemble_windows_probe(
        &fixture_section(WIN_RAS_L2TP, "ROUTE_PRINT_4"),
        &fixture_section(WIN_RAS_L2TP, "ROUTE_PRINT_6"),
        &fixture_section(WIN_RAS_L2TP, "GET_NETIPADDRESS"),
        &fixture_section(WIN_RAS_L2TP, "GET_NETADAPTER"),
        &mutated,
        &fixture_section(WIN_RAS_L2TP, "GET_VPNCONNECTION_ALLUSER"),
        &[],
    )
    .expect("变异的只是状态列，解析器该照常解析得动");
    assert!(
        !snapshot.tunnel_interfaces.iter().any(|t| t == RAS_IFACE),
        "断开的 VPN 连接配置被收进了隧道名单 —— 逢配置必报：{:?}",
        snapshot.tunnel_interfaces
    );
    // 同一次断言的另一半：别的隧道（Tailscale / TAP）不能被这条变异连累。
    for still in ["Tailscale", "OpenVPN TAP-Windows6"] {
        assert!(
            snapshot.tunnel_interfaces.iter().any(|t| t == still),
            "`{still}` 被 VPN 那条变异连累掉了：{:?}",
            snapshot.tunnel_interfaces
        );
    }
}

/// 🔴 **负向对照②：`WAN Miniport (...)` 那批协议模板不宣告任何网段**。
///
/// 它们的 ifType 是 `131`（L2TP/IKEv2/SSTP/PPTP）或 `23`（PPPOE）或 `6`（IP/IPv6/Network Monitor），
/// 其中 `131` 那批**会**被现有判据收进隧道名单 —— 这没问题，因为它们在路由表上一条都没有。
/// 但「没问题」得有人钉着：哪天它们开始带路由，或者判据把 `23` 也收了，这里会红。
///
/// 正向对照：先证明这批 miniport **确实在**隧道名单里（否则「它们不宣告网段」是废话）。
#[test]
fn ras_protocol_template_miniports_announce_nothing() {
    let snapshot = windows_leg(WIN_RAS_L2TP);
    let adapters = fixture_section(WIN_RAS_L2TP, "GET_NETADAPTER");

    // `本地连接* N` 是这批 miniport 在 zh-CN 机器上的 `InterfaceAlias`。
    let miniports: Vec<&String> = snapshot
        .tunnel_interfaces
        .iter()
        .filter(|t| t.starts_with("本地连接*"))
        .collect();
    assert_eq!(
        miniports.len(),
        4,
        "被判成隧道的 WAN Miniport 不是 4 个（L2TP / IKEv2 / SSTP / PPTP 四张 `131` 模板）：\
         {miniports:?}"
    );
    // PPPOE 那张报 `23`，**不该**在名单里 —— 它是接入协议不是隧道（与 PPPoE 拨号同型）。
    assert!(
        adapters.contains("ms_pppoeminiport"),
        "抓取里没有 PPPOE 那张 miniport —— 下一条否定断言没有信息量"
    );
    assert!(
        !snapshot
            .tunnel_interfaces
            .iter()
            .any(|t| t == "本地连接* 5"),
        "PPPOE 的 miniport（ifType `23`）进了隧道名单 —— `23` 与 PPPoE 拨号同型，不能收：{:?}",
        snapshot.tunnel_interfaces
    );

    // 本条的主体：这批模板在路由表上一条都没有。
    for mp in &miniports {
        assert!(
            !snapshot.foreign.iter().any(|r| &&r.interface == mp),
            "协议模板 `{mp}` 宣告了网段 —— 它是恒 Disconnected 的模板，不该有路由：{:?}",
            snapshot.foreign
        );
    }
}

//! C5 出口路由 op 纯逻辑门（argv 构造 / ifconfig 解析 / tailnet 反查）——真 OS 手术是 mac/Linux 真机门，
//! 但 argv 与解析是纯字符串→数据，可离线单测 + 变异（防真机门代码悄悄写错却无人守）。
use super::super::*;

/// 假 runner 记录的「真正被执行」的命令序列（失败后是否短路的唯一观测量）。
type RanCommands = Arc<Mutex<Vec<Vec<String>>>>;

/// 假 runner：按脚本返回成功/失败，并记录**真正被执行**的命令序列（验证失败后是否短路）。
fn scripted_runner(
    script: Vec<bool>,
) -> (
    impl Fn(Vec<String>) -> std::future::Ready<bool>,
    RanCommands,
) {
    let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);
    let script = Arc::new(Mutex::new(script.into_iter().collect::<Vec<_>>()));
    let run = move |argv: Vec<String>| {
        let idx = {
            let mut g = sink.lock().unwrap();
            g.push(argv);
            g.len() - 1
        };
        let ok = script.lock().unwrap().get(idx).copied().unwrap_or(true);
        std::future::ready(ok)
    };
    (run, calls)
}

#[tokio::test]
async fn linux_exit_routes_install_and_remove_the_rule_for_each_requested_family() {
    let dual = ["0.0.0.0/0".to_owned(), "::/0".to_owned()];
    for op in ["add", "del"] {
        for count in [1, 2] {
            let (run, calls) = scripted_runner(Vec::new());
            assert!(run_linux_route_seq(op, "polaris-ts", &dual[..count], run).await);
            let mut expected = Vec::new();
            for (family, cidr) in [("-4", "0.0.0.0/0"), ("-6", "::/0")]
                .into_iter()
                .take(count)
            {
                let rule = vec![
                    family,
                    "rule",
                    op,
                    "oif",
                    "polaris-ts",
                    "table",
                    "7732",
                    "priority",
                    "7732",
                ];
                let verb = if op == "add" { "replace" } else { "del" };
                let route = vec![
                    family,
                    "route",
                    verb,
                    cidr,
                    "dev",
                    "polaris-ts",
                    "table",
                    "7732",
                ];
                if op == "add" {
                    expected.extend([rule, route]);
                } else {
                    expected.extend([route, rule]);
                }
            }
            assert_eq!(*calls.lock().unwrap(), expected);
        }
    }
}

#[tokio::test]
async fn linux_add_stops_on_a_failed_rule_and_retains_prior_cleanup() {
    let cidrs = ["0.0.0.0/0".into(), "::/0".into()];
    for (script, installed, attempted) in [
        (vec![false], false, 1),
        (vec![true, true, false], true, 3),
        (vec![true, true, true, false], true, 4),
    ] {
        let (run, calls) = scripted_runner(script);
        assert_eq!(
            run_linux_route_seq("add", "polaris-ts", &cidrs, run).await,
            installed
        );
        assert_eq!(calls.lock().unwrap().len(), attempted);
    }
}

#[tokio::test]
async fn linux_del_reports_failure_but_attempts_every_command() {
    let (run, calls) = scripted_runner(vec![false, true, true, false]);
    let cidrs = ["0.0.0.0/0".into(), "::/0".into()];
    assert!(!run_linux_route_seq("del", "polaris-ts", &cidrs, run).await);
    assert_eq!(calls.lock().unwrap().len(), 4);
}

/// `ifconfig -l` → 仅 utun\d+ 名（滤掉 lo0/en0/非数字后缀 utunX）。
#[test]
fn parse_utun_list_filters_utun_only() {
    let s = "lo0 gif0 stf0 en0 utun0 utun4 utunfoo utun12 bridge0";
    let got = parse_utun_list(s);
    let mut v: Vec<_> = got.into_iter().collect();
    v.sort();
    assert_eq!(v, vec!["utun0", "utun12", "utun4"]);
}

/// 全量 ifconfig → 每 utun 的 v4 地址（忽略 inet6、缩进明细归属当前接口头）。
#[test]
fn parse_ifconfig_ifaces_groups_v4_by_utun() {
    let s = "\
en0: flags=8863<UP> mtu 1500
\tinet 192.168.1.10 netmask 0xffffff00
utun4: flags=8051<UP> mtu 1400
\tinet6 fe80::1 prefixlen 64
\tinet 100.64.0.7 --> 100.64.0.7 netmask 0xffffffff
utun5: flags=8051<UP> mtu 1280
\tinet 10.0.0.2 --> 10.0.0.2 netmask 0xffffffff
";
    let got = parse_ifconfig_ifaces(s);
    // en0 非 utun → 不入。
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "utun4");
    assert_eq!(got[0].1, vec!["100.64.0.7"]); // inet6 被忽略
    assert_eq!(got[1].0, "utun5");
    assert_eq!(got[1].1, vec!["10.0.0.2"]);
}

#[test]
fn non_utun_headers_end_address_ownership_even_when_their_address_is_tailnet() {
    let text = "utun4: flags=1\n\tinet6 fe80::1\nen9: flags=1\n\tinet 100.64.1.2 netmask 0xffffffff\nutun5: flags=1\n\tinet 100.64.2.3 netmask 0xffffffff\nbridge0: flags=1\n\tinet 100.64.3.4 netmask 0xffffffff\n";
    assert_eq!(
        parse_ifconfig_ifaces(text),
        vec![
            ("utun4".into(), Vec::<String>::new()),
            ("utun5".into(), vec!["100.64.2.3".into()]),
        ]
    );
}

/// tailnet 100.64.0.0/10 边界（100.64–100.127 命中；100.63/100.128/非 100 不命中）。
/// 变异：把范围写成 (64..=126) 或 0..=127 → 边界 case 转红。
#[test]
fn is_tailnet_addr_boundaries() {
    assert!(is_tailnet_addr("100.64.0.1"));
    assert!(is_tailnet_addr("100.127.255.255"));
    assert!(!is_tailnet_addr("100.63.0.1"));
    assert!(!is_tailnet_addr("100.128.0.1"));
    assert!(!is_tailnet_addr("192.168.1.1"));
    assert!(!is_tailnet_addr("10.64.0.1"));
}

/// 反查优先「起核后新增（不在 baseline）且带 tailnet 地址」的 utun。
/// 变异：删掉 baseline 过滤（filter）→ 会错命中 baseline 里的 Tailscale.app utun（utun3）→ 转红。
#[test]
fn pick_tailnet_iface_prefers_new_utun_over_baseline() {
    // utun3 = 起核前已存在的 Tailscale.app 接口（在 baseline，也带 tailnet 地址）；
    // utun7 = 起核后 sing-box 新建的 TS 接口（不在 baseline）。应挑 utun7。
    let ifaces = vec![
        ("utun3".to_string(), vec!["100.100.0.1".to_string()]),
        ("utun7".to_string(), vec!["100.64.0.9".to_string()]),
    ];
    let mut baseline = HashSet::new();
    baseline.insert("utun3".to_string());
    assert_eq!(
        pick_tailnet_iface(&ifaces, Some(&baseline)).as_deref(),
        Some("utun7"),
        "须优先起核后新增的 utun（时序 diff），不误命中 baseline 里的 Tailscale.app utun"
    );
}

/// 无 baseline → 退化为纯地址反推（Polaris 兜底）：取第一张带 tailnet 地址的 utun。
#[test]
fn pick_tailnet_iface_falls_back_to_address_when_no_baseline() {
    let ifaces = vec![
        ("utun5".to_string(), vec!["10.0.0.2".to_string()]),
        ("utun6".to_string(), vec!["100.96.1.2".to_string()]),
    ];
    assert_eq!(pick_tailnet_iface(&ifaces, None).as_deref(), Some("utun6"));
    // 无任何 tailnet 地址 → None。
    let none = vec![("utun5".to_string(), vec!["10.0.0.2".to_string()])];
    assert_eq!(pick_tailnet_iface(&none, None), None);
}

#[test]
fn linux_delete_absence_is_narrowly_idempotent() {
    let delete = vec!["rule".into(), "del".into()];
    let add = vec!["rule".into(), "add".into()];
    let absent = b"RTNETLINK answers: No such file or directory\n";
    assert!(ip_delete_already_absent(&delete, absent));
    assert!(!ip_delete_already_absent(&add, absent));
    for message in [
        b"RTNETLINK answers: Operation not permitted".as_slice(),
        b"Cannot find device",
        b"unknown failure",
    ] {
        assert!(!ip_delete_already_absent(&delete, message));
    }
}

#[test]
fn linux_disappeared_interface_requires_routes_and_policy_rule_absence() {
    let cidrs = vec!["0.0.0.0/0".into()];
    let empty = serde_json::json!([]);
    assert!(linux_exit_route_inventory_absent(
        &empty,
        &empty,
        "polaris-ts",
        &cidrs
    ));
    let leftover_rule = serde_json::json!([{"table":7732,"oif":"polaris-ts"}]);
    assert!(!linux_exit_route_inventory_absent(
        &empty,
        &leftover_rule,
        "polaris-ts",
        &cidrs
    ));
    let leftover_route = serde_json::json!([{"table":7732,"dst":"default","dev":"polaris-ts"}]);
    assert!(!linux_exit_route_inventory_absent(
        &leftover_route,
        &empty,
        "polaris-ts",
        &cidrs
    ));
    let other = serde_json::json!([{"table":254,"dst":"default","dev":"eth0"}]);
    assert!(linux_exit_route_inventory_absent(
        &other,
        &empty,
        "polaris-ts",
        &cidrs
    ));
    assert!(!linux_exit_route_inventory_absent(
        &serde_json::json!({}),
        &empty,
        "polaris-ts",
        &cidrs
    ));
    assert!(!linux_exit_route_inventory_absent(
        &empty,
        &serde_json::json!([{"oif":"polaris-ts"}]),
        "polaris-ts",
        &cidrs
    ));
}

#[test]
fn linux_cleanup_unknown_relevant_table_remains_debt() {
    let empty = serde_json::json!([]);
    let cidrs = vec!["0.0.0.0/0".to_string()];
    for table in [
        serde_json::Value::Null,
        serde_json::json!({}),
        serde_json::json!("garbage"),
        serde_json::json!("18446744073709551616"),
        serde_json::json!(4294967296_u64),
        serde_json::json!("4294967296"),
        serde_json::json!(-1),
    ] {
        let route = serde_json::json!([{"dev":"polaris-ts", "dst":"default", "table":table}]);
        let rule = serde_json::json!([{"oif":"polaris-ts", "table":table}]);
        assert!(!linux_exit_route_inventory_absent(
            &route,
            &empty,
            "polaris-ts",
            &cidrs
        ));
        assert!(!linux_exit_route_inventory_absent(
            &empty,
            &rule,
            "polaris-ts",
            &cidrs
        ));
    }
    let missing = serde_json::json!([{"dev":"polaris-ts", "dst":"default"}]);
    assert!(!linux_exit_route_inventory_absent(
        &missing,
        &empty,
        "polaris-ts",
        &cidrs
    ));
    for table in [serde_json::json!(254), serde_json::json!("254")] {
        let route = serde_json::json!([{"dev":"polaris-ts", "dst":"default", "table":table}]);
        let rule = serde_json::json!([{"oif":"polaris-ts", "table":table}]);
        assert!(linux_exit_route_inventory_absent(
            &route,
            &rule,
            "polaris-ts",
            &cidrs
        ));
    }
}

#[test]
fn linux_inventory_query_requires_detailed_numeric_tables() {
    for family in ["-4", "-6"] {
        let args = linux_exit_route_inventory_args(family);
        assert_eq!(
            args[0],
            vec![family, "-j", "-N", "-details", "route", "show", "table", "all"]
        );
        assert_eq!(
            args[1],
            vec![family, "-j", "-N", "-details", "rule", "show"]
        );
    }
    // iproute2's detailed numeric output explicitly identifies the main table.
    // The default destination alone must not make this an owned table-7732 route.
    let main = serde_json::json!([{
        "type":"unicast", "dst":"default", "gateway":"192.0.2.1",
        "dev":"eth0", "table":"254", "protocol":"3", "scope":"0", "flags":[]
    }]);
    let rules = serde_json::json!([{"priority":32766, "src":"all", "table":"254"}]);
    let cidrs = vec!["0.0.0.0/0".to_string()];
    assert!(linux_exit_route_inventory_absent(
        &main,
        &rules,
        "polaris-ts",
        &cidrs
    ));
    let mut missing_table = main;
    missing_table[0].as_object_mut().unwrap().remove("table");
    assert!(!linux_exit_route_inventory_absent(
        &missing_table,
        &rules,
        "polaris-ts",
        &cidrs
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_inventory_capture_rejects_incomplete_output_and_reaps_timeout() {
    for (script, expected) in [
        ("printf '[]'", Some(serde_json::json!([]))),
        ("printf '[]'; exit 1", None),
        ("printf '['", None),
        ("head -c 4194305 /dev/zero", None),
    ] {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", script]);
        assert_eq!(read_linux_exit_route_inventory(command).await, expected);
    }
    let directory = crate::test_support::TestDir::new("linux-inventory-timeout");
    let pid_file = directory.path().join("pid");
    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", "echo $$ > \"$PID_FILE\"; exec sleep 30"])
        .env("PID_FILE", &pid_file);
    let started = std::time::Instant::now();
    assert!(read_linux_exit_route_inventory(command).await.is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let pid = std::fs::read_to_string(pid_file).unwrap();
    assert!(!tokio::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .unwrap()
        .success());
}

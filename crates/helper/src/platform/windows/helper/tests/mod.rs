#![allow(clippy::too_many_lines)]

use super::*;
use crate::platform::windows::ops::{MockNetTableOps, MockProcOps};
use crate::token::StaticTokenStore;
use polaris_helper_proto::StartParams;

/// **已迁移**形态（P4 的目标态，也是安装脚本装出来的形态）：`--singbox` 就在
/// `<support>\core` 里，故实际执行面 == 派生的落盘目标面 ⇒ 起核自检真的会跑判定。
///
/// 这一点不是装饰：未迁移（两面不一致）时起核自检**按设计跳过**（spec §3.5「四象限无一 brick」，
/// 见 `start_skips_the_acl_check_when_the_helper_is_not_migrated_yet`）。若本构造仍用旧的
/// `C:\Program Files\Polaris\sing-box.exe` + `C:\ProgramData\Polaris`，下面每一条 ACL 行为断言
/// 都会因为「压根没跑判定」而变成假绿。
fn make_helper(
    proc_ops: MockProcOps,
    net_ops: MockNetTableOps,
) -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    WinHelper::new(
        StaticTokenStore::new("real-token"),
        proc_ops,
        net_ops,
        r"C:\ProgramData\Polaris\core\sing-box.exe",
        r"C:\Users\polaris\config",
        "PolarisHelper",
        r"C:\ProgramData\Polaris",
    )
}

/// **未迁移**形态：SCM ImagePath 的 `--singbox` 还指着安装目录，`<support>\core` 是另一处。
///
/// 可达路径不是理论 —— 安装脚本的 catch 回滚腿（icacls 守卫 throw → `Copy-Item $helperBackup`
/// 被文件锁挡住）会把机器停在「新 helper.exe + 旧 binPath」。
fn make_helper_unmigrated(
    proc_ops: MockProcOps,
) -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    WinHelper::new(
        StaticTokenStore::new("real-token"),
        proc_ops,
        MockNetTableOps::new(),
        r"C:\Users\bob\AppData\Roaming\Polaris\core_update\sing-box.exe",
        r"C:\Users\polaris\config",
        "PolarisHelper",
        r"C:\ProgramData\Polaris",
    )
}

fn make_helper_defaults() -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    make_helper(MockProcOps::new(), MockNetTableOps::new())
}

/// support_dir 可指定的构造（install-core 用：受保护内核目录是 `<support>\core`，
/// 本机跑测试必须指到 tempdir，不能用 `C:\ProgramData\Polaris` —— 在 Linux 上那是个相对路径，
/// 会在 cwd 底下真建目录）。
fn make_helper_with_support(
    proc_ops: MockProcOps,
    support_dir: &str,
) -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    WinHelper::new(
        StaticTokenStore::new("real-token"),
        proc_ops,
        MockNetTableOps::new(),
        r"C:\Program Files\Polaris\sing-box.exe",
        r"C:\Users\polaris\config",
        "PolarisHelper",
        support_dir,
    )
}

/// 造 install-core 源目录：`sing-box.exe` + 一个配套 DLL，返回 (dir, want_hash)。
fn make_win_src_dir(sb: &[u8]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sing-box.exe"), sb).unwrap();
    std::fs::write(dir.path().join("libcronet.dll"), b"cronet payload").unwrap();
    (dir, crate::core_install::sha256_hex(sb))
}

fn install_core_req(src: &std::path::Path, want_hash: &str) -> Request {
    Request::InstallCore(polaris_helper_proto::InstallCoreParams {
        src_dir: src.to_string_lossy().into_owned(),
        want_hash: want_hash.to_owned(),
    })
}

// ===== 鉴权 =====

#[test]
fn auth_failed_on_wrong_token() {
    let h = make_helper_defaults();
    let out = h.handle("wrong-token", Request::Ping);
    assert_eq!(out, HandleOutcome::AuthFailed);
}

#[test]
fn auth_failed_on_empty_token() {
    let h = make_helper_defaults();
    let out = h.handle("", Request::Ping);
    assert_eq!(out, HandleOutcome::AuthFailed);
}

#[test]
fn auth_passes_on_correct_token() {
    let h = make_helper_defaults();
    let out = h.handle("real-token", Request::Ping);
    let HandleOutcome::Respond(Response::Ok(ResponseKind::Pong(pong))) = out else {
        panic!("{out:?}");
    };
    // Windows uid 固定 0（helper.go:179）
    assert_eq!(pong.uid, 0);
    assert_eq!(pong.proto_version, crate::platform::windows::PROTO_VERSION);
}

// ===== ping/version/status =====

#[test]
fn version_returns_proto_version() {
    let h = make_helper_defaults();
    let out = h.handle("real-token", Request::Version);
    let HandleOutcome::Respond(Response::Ok(ResponseKind::Version { proto_version })) = out else {
        panic!("{out:?}");
    };
    assert_eq!(proto_version, crate::platform::windows::PROTO_VERSION);
}

#[test]
fn status_stopped_when_no_child() {
    let h = make_helper_defaults();
    let out = h.handle("real-token", Request::Status);
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    );
}

// ===== start =====

#[test]
fn start_with_empty_cfg_is_no_config() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: String::new(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::NoConfig);
}

#[test]
fn start_with_cfg_outside_confdir_is_denied() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Windows\evil.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::ConfigPathDenied);
}

/// log 与 cfg 走同一条白名单 —— 不校验就是「SYSTEM 在任意位置建文件并持续追加写」。
#[test]
fn start_with_log_outside_confdir_is_denied() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            // cfg 合法，只有 log 越界 —— 单独钉住 log 这一格。
            cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
            log: r"C:\Windows\System32\drivers\etc\hosts".to_owned(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::LogPathDenied);
}

/// 生产形态（cfg 与 log 同在 confDir）必须放行 —— 否则上面那条可能被「恒拒」满足。
#[test]
fn start_with_log_inside_confdir_is_allowed() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
            log: r"C:\Users\polaris\config\singbox-startup.log".to_owned(),
            fwd: false,
            parent_pid: None,
        }),
    );
    assert!(
        !matches!(
            &out,
            HandleOutcome::Respond(Response::Err(e))
                if e.code == polaris_helper_proto::ErrorCode::LogPathDenied
        ),
        "生产形态被误拒：{out:?}"
    );
}

/// 空 log = 不重定向（`win.rs` 的 `if !log_path.is_empty()`），必须放行。
#[test]
fn start_with_empty_log_is_allowed() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    assert!(
        !matches!(
            &out,
            HandleOutcome::Respond(Response::Err(e))
                if e.code == polaris_helper_proto::ErrorCode::LogPathDenied
        ),
        "空 log 被误拒：{out:?}"
    );
}

#[test]
fn start_with_valid_cfg_starts_and_records_pid() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Ok(ResponseKind::Start(
        polaris_helper_proto::Start::StartedTimed { pid, timing, .. },
    ))) = out
    else {
        panic!("{out:?}");
    };
    // mock next_pid + mock 阶段耗时。
    assert_eq!(pid, 1000);
    assert_eq!(timing.total_ms, 0);
    // status 应反映 running
    let out2 = h.handle("real-token", Request::Status);
    assert!(matches!(
        out2,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Running { pid: 1000, .. }
        )))
    ));
}

#[test]
fn status_clears_a_managed_pid_after_the_child_exits() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let request = Request::Start(StartParams {
        cfg: r"C:\Users\polaris\config\c.json".to_owned(),
        log: String::new(),
        fwd: false,
        parent_pid: None,
    });
    let _ = h.handle("real-token", request.clone());

    proc_ops.set_alive(false);
    assert!(matches!(
        h.handle("real-token", Request::Status),
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    ));

    proc_ops.set_alive(true);
    let restarted = h.handle("real-token", request);
    assert!(matches!(
        restarted,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Start(
            polaris_helper_proto::Start::StartedTimed { pid: 1001, .. }
        )))
    ));
}

#[test]
fn start_when_already_running_returns_already() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let cfg = r"C:\Users\polaris\config\c.json".to_owned();
    let req = Request::Start(StartParams {
        cfg,
        log: String::new(),
        fwd: false,
        parent_pid: None,
    });
    let _ = h.handle("real-token", req.clone());
    let out = h.handle("real-token", req);
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Start(
            polaris_helper_proto::Start::Already { pid: 1000 }
        )))
    ));
}

#[test]
fn start_failure_returns_err_start() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_start_error(std::io::Error::other("ENOENT"));
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::Start);
}

#[test]
fn start_with_fwd_calls_enable_ip_forwarding() {
    let proc_ops = MockProcOps::new();
    let snap_before = proc_ops.snapshot();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: true, // 应触发 enable_ip_forwarding
            parent_pid: None,
        }),
    );
    let snap_after = proc_ops.snapshot();
    assert!(snap_after.ip_forward_calls > snap_before.ip_forward_calls);
}

// ===== stop =====

#[test]
fn stop_when_not_running_is_idempotent() {
    let h = make_helper_defaults();
    let out = h.handle("real-token", Request::Stop { pid: None });
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
            polaris_helper_proto::Stop::NotRunning
        )))
    ));
}

#[test]
fn stop_reaps_running_child_and_clears_state() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    // 先 start
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let snap_before = proc_ops.snapshot();
    // stop
    let out = h.handle("real-token", Request::Stop { pid: None });
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
            polaris_helper_proto::Stop::Stopped { pid: 1000 }
        )))
    ));
    let snap_after = proc_ops.snapshot();
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls + 1);
    // status 现在应是 stopped
    let out2 = h.handle("real-token", Request::Status);
    assert!(matches!(
        out2,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    ));
}

/// **变异门（核心）**：身份不匹配 → 不摘不收割，child 状态原样留给新会话。
///
/// 变异（逃逸面穷举）：
/// - 删掉 `Request::Stop` 分支里的 `stop_pid_matches` 判据 → 响应变 `Stopped{1000}` +
///   `reap_calls` 涨 → 转红（那正是「杀掉用户刚连上的新核」）。
/// - 只改响应不改行为（回 Mismatch 但仍 `take()` + `reap_child`）→ 后两条断言转红。
/// - `parse_request` 里把身份行丢掉、恒 `pid: None` → 判据永不触发 → 转红（另有
///   `service/win::parse_request_stop_reads_optional_pid` 直接钉住解码侧）。
#[test]
fn stop_refuses_to_reap_when_managed_pid_is_another_session() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    // daemon 手里的是新会话的核（MockProcOps 的 start 固定报 1000）。
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let snap_before = proc_ops.snapshot();
    // 老 stop 腿声明它要停 4242。
    let out = h.handle("real-token", Request::Stop { pid: Some(4242) });
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
            polaris_helper_proto::Stop::Mismatch {
                want: 4242,
                current: 1000
            }
        ))),
        "身份不匹配 → 诚实 no-op，回报两个 pid"
    );
    assert_eq!(
        proc_ops.snapshot().reap_calls,
        snap_before.reap_calls,
        "绝不能收割：1000 是用户刚连上的新核"
    );
    assert!(
        matches!(
            h.handle("real-token", Request::Status),
            HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
                polaris_helper_proto::Status::Running { pid: 1000, .. }
            )))
        ),
        "child 记账必须原样留给新会话（摘掉 = 新核失联，daemon 再也停不掉它）"
    );
}

/// 反向失效门：身份匹配照常停（判据不能收得太紧，否则停核彻底失效）。
#[test]
fn stop_proceeds_when_managed_pid_matches() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let snap_before = proc_ops.snapshot();
    let out = h.handle("real-token", Request::Stop { pid: Some(1000) });
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
            polaris_helper_proto::Stop::Stopped { pid: 1000 }
        )))
    );
    assert_eq!(proc_ops.snapshot().reap_calls, snap_before.reap_calls + 1);
}

// ===== cleanup =====

#[test]
fn cleanup_reaps_child_and_kills_all_singbox() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_kill_all_return(2);
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    // 先 start
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let snap_before = proc_ops.snapshot();
    let out = h.handle("real-token", Request::Cleanup);
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Cleaned))
    );
    let snap_after = proc_ops.snapshot();
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls + 1);
    // kill_all_singbox 由 mock 的 kill_all_return 返回 2（调用计数不经此路径，但行为对齐 Go）
}

// ===== uninstall =====

#[test]
fn uninstall_spawns_self_uninstall_and_signals_exit() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let snap_before = proc_ops.snapshot();
    let out = h.handle("real-token", Request::Uninstall);
    match out {
        HandleOutcome::UninstallAndExit(Response::Ok(ResponseKind::Uninstalling)) => {}
        other => panic!("expected UninstallAndExit, got {other:?}"),
    }
    let snap_after = proc_ops.snapshot();
    assert_eq!(
        snap_after.spawn_uninstall_calls,
        snap_before.spawn_uninstall_calls + 1
    );
    // spawn 参数传了 service_name + support_dir
    let args = proc_ops.last_spawn_args();
    assert_eq!(
        args,
        Some((
            "PolarisHelper".to_owned(),
            r"C:\ProgramData\Polaris".to_owned()
        ))
    );
}

// ===== freeport =====

#[test]
fn freeport_free_when_no_listener() {
    let h = make_helper_defaults();
    let out = h.handle("real-token", Request::FreePort { port: 9090 });
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
            polaris_helper_proto::FreePort::Free
        )))
    );
}

#[test]
fn freeport_kills_locked_singbox_listener() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_image(1000, r"C:\ProgramData\Polaris\core\sing-box.exe");
    let net_ops = MockNetTableOps::new();
    net_ops.set_entries(vec![crate::platform::windows::logic::ListenEntry {
        pid: 1000,
        port: 9090,
    }]);
    let h = make_helper(proc_ops.clone(), net_ops);
    let out = h.handle("real-token", Request::FreePort { port: 9090 });
    let HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
        polaris_helper_proto::FreePort::Killed { pids },
    ))) = out
    else {
        panic!("{out:?}");
    };
    assert_eq!(pids, vec![1000]);
    // terminate_pid 被调用
    assert_eq!(proc_ops.snapshot().terminate_calls, 1);
}

#[test]
fn freeport_reports_foreign_listener_without_killing() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_image(2000, r"C:\Windows\System32\nginx.exe");
    let net_ops = MockNetTableOps::new();
    net_ops.set_entries(vec![crate::platform::windows::logic::ListenEntry {
        pid: 2000,
        port: 80,
    }]);
    let h = make_helper(proc_ops.clone(), net_ops);
    let out = h.handle("real-token", Request::FreePort { port: 80 });
    let HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
        polaris_helper_proto::FreePort::Foreign { names },
    ))) = out
    else {
        panic!("{out:?}");
    };
    assert_eq!(names, vec!["nginx.exe".to_owned()]);
    // 不应调 terminate_pid（foreign 不杀）
    assert_eq!(proc_ops.snapshot().terminate_calls, 0);
}

// ===== route-add / route-del =====

#[test]
fn route_add_denies_non_polaris_iface() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::RouteAdd(polaris_helper_proto::RouteParams {
            iface: "Ethernet0".to_owned(),
            cidrs: vec!["10.0.0.0/8".to_owned()],
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::IfaceDenied);
}

#[test]
fn route_add_allows_polaris_iface() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::RouteAdd(polaris_helper_proto::RouteParams {
            iface: "polaris-tun0".to_owned(),
            cidrs: vec!["10.0.0.0/8".to_owned()],
        }),
    );
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Route))
    );
}

// ===== route netsh 真派发（W9 修）=====

#[test]
fn route_add_dispatches_apply_route_for_each_valid_cidr() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle(
        "real-token",
        Request::RouteAdd(polaris_helper_proto::RouteParams {
            iface: "polaris-tun0".to_owned(),
            // 中间一项非法 CIDR + 一项空 → 应被跳过（Go: ParseCIDR err / "" → continue）。
            cidrs: vec![
                "10.0.0.0/8".to_owned(),
                "not-a-cidr".to_owned(),
                String::new(),
                "::/0".to_owned(),
            ],
        }),
    );
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Route))
    );
    // 仅 2 个合法 CIDR 触发 apply_route（非法/空跳过）。
    assert_eq!(proc_ops.snapshot().route_calls, 2);
    // 最后一次 = ::/0, del=false（add）。
    assert_eq!(
        proc_ops.last_route(),
        Some(("polaris-tun0".to_owned(), "::/0".to_owned(), false))
    );
}

#[test]
fn route_del_dispatches_apply_route_with_del_true() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle(
        "real-token",
        Request::RouteDel(polaris_helper_proto::RouteParams {
            iface: "polaris-tun0".to_owned(),
            cidrs: vec!["10.0.0.0/8".to_owned()],
        }),
    );
    assert_eq!(proc_ops.snapshot().route_calls, 1);
    assert_eq!(
        proc_ops.last_route(),
        Some(("polaris-tun0".to_owned(), "10.0.0.0/8".to_owned(), true))
    );
}

#[test]
fn route_denied_iface_runs_no_netsh() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle(
        "real-token",
        Request::RouteAdd(polaris_helper_proto::RouteParams {
            iface: "Ethernet0".to_owned(),
            cidrs: vec!["10.0.0.0/8".to_owned()],
        }),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::IfaceDenied);
    // iface 被拒 → 不应跑任何 netsh。
    assert_eq!(proc_ops.snapshot().route_calls, 0);
}

// ===== 父死看护接线（W15 修）=====

fn start_req_with_ppid(ppid: Option<u32>) -> Request {
    Request::Start(StartParams {
        cfg: r"C:\Users\polaris\config\c.json".to_owned(),
        log: String::new(),
        fwd: false,
        parent_pid: ppid,
    })
}

#[test]
fn start_with_ppid_wires_watch_parent() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle("real-token", start_req_with_ppid(Some(4242)));
    let snap = proc_ops.snapshot();
    assert_eq!(snap.watch_parent_calls, 1);
    // 传入 (ppid, child_pid)：child_pid = mock start 返回的 1000。
    assert_eq!(proc_ops.last_watch_args(), Some((4242, 1000)));
}

#[test]
fn start_without_ppid_does_not_wire_watch_parent() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle("real-token", start_req_with_ppid(None));
    assert_eq!(proc_ops.snapshot().watch_parent_calls, 0);
}

#[test]
fn start_with_zero_ppid_does_not_wire_watch_parent() {
    // Go: ppid <= 0 → 不启看护。parent_pid=Some(0) 等价（filter ppid>0 排除）。
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle("real-token", start_req_with_ppid(Some(0)));
    assert_eq!(proc_ops.snapshot().watch_parent_calls, 0);
}

#[test]
fn watch_parent_reaps_child_when_parent_dead() {
    // mock spawn_watch_parent 单次评估：父死（alive=false）+ child 仍当前 → on_parent_dead 收割 + 摘 child。
    let proc_ops = MockProcOps::new();
    proc_ops.set_alive(false);
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let snap_before = proc_ops.snapshot();
    let _ = h.handle("real-token", start_req_with_ppid(Some(4242)));
    let snap_after = proc_ops.snapshot();
    // on_parent_dead → proc.reap_child(pid) → reap_calls +1。
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls + 1);
    // child 已被摘 → status 现在 stopped。
    let out = h.handle("real-token", Request::Status);
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    ));
}

#[test]
fn watch_parent_keeps_child_when_parent_alive() {
    // 父存活（alive=true 默认）→ on_parent_dead 不触发 → child 仍在（running）。
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle("real-token", start_req_with_ppid(Some(4242)));
    let out = h.handle("real-token", Request::Status);
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Running { pid: 1000, .. }
        )))
    ));
}

// ===== iface-metric =====

#[test]
fn iface_metric_denies_non_polaris_iface() {
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::IfaceMetric {
            iface: "en0".to_owned(),
            metric: 999,
        },
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::IfaceDenied);
}

// ===== reap_child_on_exit =====

#[test]
fn reap_child_on_exit_reaps_when_child_present() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let snap_before = proc_ops.snapshot();
    h.reap_child_on_exit();
    let snap_after = proc_ops.snapshot();
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls + 1);
}

#[test]
fn reap_child_on_exit_killall_when_no_child() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.reap_child_on_exit(); // 无 child → 走 killAllSingbox 兜底
                            // kill_all_singbox 调用无独立计数器，但行为对齐 Go（Go 注释 helper.go:407-409）
}

// ===== unsupported commands =====

#[test]
fn mac_linux_commands_return_unknown() {
    // Windows helper 无 default-restore / linux-start / mac 代理事务三条（flush-dns 已由 D4 实现，
    // 见 `flush_dns_*`；install-core 已由 P4 实现，见 `install_core_*`）。
    // 断言体只发 default-restore —— 其余未支持命令的 `ERR unknown` 由本 arm 的 `|` 合并保证。
    let h = make_helper_defaults();
    let out = h.handle(
        "real-token",
        Request::DefaultRestore {
            gateway_ipv4: "192.168.1.1".to_owned(),
        }, // mac 专属
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::Unknown);
}

// ===== install-core（P4：S 受保护内核目录） =====

/// 本片的核心行为断言：install-core 不再落 `ERR unknown`，而是真把核写进 `<support>\core`。
#[test]
fn install_core_writes_the_protected_core_dir() {
    let support = tempfile::tempdir().unwrap();
    let sb: &[u8] = b"windows sing-box payload";
    let (src, hash) = make_win_src_dir(sb);
    let h = make_helper_with_support(MockProcOps::new(), support.path().to_str().unwrap());

    let out = h.handle("real-token", install_core_req(src.path(), &hash));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert_eq!(resp.to_wire_line(), "OK installed");

    // 正面断言：核与配套都落进受保护目录，字节与源一致（不是「响应对了但没写盘」）。
    let core = support.path().join("core");
    assert_eq!(std::fs::read(core.join("sing-box.exe")).unwrap(), sb);
    assert_eq!(
        std::fs::read(core.join("libcronet.dll")).unwrap(),
        b"cronet payload"
    );
}

/// sha256 闸有牙：hash 不符 → `ERR hash-mismatch`，且受保护目录压根不该被建出来。
///
/// 同时是「不再是 ERR unknown」的第二条证据 —— 这个 code 只可能从新分派里出来。
#[test]
fn install_core_hash_mismatch_leaves_the_core_dir_absent() {
    let support = tempfile::tempdir().unwrap();
    let (src, _hash) = make_win_src_dir(b"windows sing-box payload");
    let h = make_helper_with_support(MockProcOps::new(), support.path().to_str().unwrap());

    let out = h.handle("real-token", install_core_req(src.path(), &"0".repeat(64)));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert_eq!(resp.to_wire_line(), "ERR hash-mismatch");
    assert!(
        !support.path().join("core").exists(),
        "校验没过却建了 coreDir"
    );
}

/// 受管核在跑 → `ERR busy`，且受保护目录一个字节没动。
///
/// Windows rename 不动运行中的 exe / 已加载的 DLL；不挡就是「`.new` 写得进、rename 失败」的
/// 半吊子安装。判活与 handle_start 同一把锁同一个判据。
///
/// 正面对照见 `install_core_writes_the_protected_core_dir`：同样的请求在没核在跑时是会落盘的，
/// 所以这里的「没动」不是「什么都做不到」。
#[test]
fn install_core_is_busy_while_the_managed_core_runs() {
    let support = tempfile::tempdir().unwrap();
    let core = support.path().join("core");
    std::fs::create_dir_all(&core).unwrap();
    std::fs::write(core.join("sing-box.exe"), b"core already installed").unwrap();

    let (src, hash) = make_win_src_dir(b"windows sing-box payload");
    // MockProcOps 默认 process_alive=true —— 起一个受管核后 live_managed_pid 即为 Some。
    let h = make_helper_with_support(MockProcOps::new(), support.path().to_str().unwrap());
    let started = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(started) = started else {
        panic!("{started:?}");
    };
    assert!(
        started.to_wire_line().starts_with("OK started"),
        "前置条件没成立（核没起来）：{}",
        started.to_wire_line()
    );

    let out = h.handle("real-token", install_core_req(src.path(), &hash));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert_eq!(resp.to_wire_line(), "ERR busy");
    // 正面断言：原来那份核逐字还在（不是被写坏、也不是被删）。
    assert_eq!(
        std::fs::read(core.join("sing-box.exe")).unwrap(),
        b"core already installed"
    );
    // 新核的配套一个都没进来。
    assert!(!core.join("libcronet.dll").exists(), "busy 却写了配套文件");
}

// ===== P4：起核前的受保护核目录 ACL 自检（spec §3.4 · Q9）=====

/// `make_helper` 的 `--singbox` 派生出的三个**兜底**自检对象（目录 + 核 + 配套 DLL）。
/// 真正的覆盖面还含目录的实际条目（见 `start_refuses_when_a_file_only_found_by_enumeration_is_weakened`）。
const ACL_DIR: &str = r"C:\ProgramData\Polaris\core";
const ACL_BIN: &str = r"C:\ProgramData\Polaris\core\sing-box.exe";
const ACL_DLL: &str = r"C:\ProgramData\Polaris\core\libcronet.dll";

/// 造一个「Users 被授完全控制」的放宽态（最常见的真实放宽形态：有人 `icacls /grant Users:(F)`）。
fn users_full_control() -> crate::platform::windows::coreacl::ObjectSecurity {
    use crate::platform::windows::coreacl::{Ace, AceKind};
    let mut sec = crate::platform::windows::coreacl::locked_down_fixture();
    sec.dacl.as_mut().unwrap().push(Ace {
        sid: "S-1-5-32-545".to_owned(),
        mask: 0x001F_01FF,
        kind: AceKind::Allow,
        flags: 0x3,
    });
    sec
}

/// **防「第二条腿让旧门失去牙」**：断言自检确实**在生产起核路径上被咨询过**，且问的是
/// `--singbox` 的父目录（真正要被 exec 的那个）+ 该二进制 + 同目录配套 DLL。
///
/// mock 的兜底是「通过」，所以「自检根本没被调用」这个 bug 在其它任何断言下都是**不可见**的
/// —— 全部 ACL 行为断言都会照样绿。这条计数/取材面断言是唯一能把它照出来的东西。
///
/// 变异：把 `handle_start` 里的 `core_acl_gate()` 调用整行删掉 → `acl_queries()` 为空 → 本条红，
/// 而下面三条腿的断言里只有「拒起核」那条会跟着红（另两条会静默变成假绿）。
#[test]
fn start_consults_the_acl_self_check_on_the_exec_dir() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    assert_eq!(
        proc_ops.acl_queries(),
        Vec::<String>::new(),
        "起核前不该有查询"
    );
    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert!(resp.to_wire_line().starts_with("OK started"));
    assert_eq!(
        proc_ops.acl_queries(),
        vec![ACL_DIR.to_owned(), ACL_BIN.to_owned(), ACL_DLL.to_owned()],
        "自检没在起核路径上跑，或问错了对象（守错目标 = 门形同虚设）"
    );
}

/// 腿①**通过** → 照常起核（判据不能收得太紧，否则起核彻底失效）。
#[test]
fn start_proceeds_when_the_core_dir_acl_is_locked_down() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert!(
        resp.to_wire_line().starts_with("OK started"),
        "目标态被误拒：{}",
        resp.to_wire_line()
    );
    assert_eq!(proc_ops.snapshot().start_calls, 1);
}

/// 腿②**放宽 → 拒起核**，且 detail 说得出「哪个路径、哪条 ACE」。
///
/// 两条正面断言不可省：
/// - `start_calls == 0` —— 「拒」的含义是**核根本没被 exec**，不是「回了个错但还是起了」；
/// - detail 含路径 + SID —— 真机上只有这一行可用来定位。
#[test]
fn start_refuses_when_the_core_dir_acl_is_weakened() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl(ACL_DIR, Ok(users_full_control()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert!(
        e.detail.contains(ACL_DIR),
        "detail 没说哪个路径：{}",
        e.detail
    );
    assert!(
        e.detail.contains("S-1-5-32-545"),
        "detail 没说哪条 ACE：{}",
        e.detail
    );
    assert_eq!(
        proc_ops.snapshot().start_calls,
        0,
        "判了放宽却还是把核 exec 了"
    );
    // 状态里也不该留下受管 pid。
    assert!(matches!(
        h.handle("real-token", Request::Status),
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    ));
}

/// 覆盖面 = 目录 + **每个白名单文件**：目录锁得住、核二进制被单独放宽，照样拒起核。
///
/// 只判目录的门在生产上等于没门 —— `icacls <file> /grant` 是比改目录更省事的放宽方式。
#[test]
fn start_refuses_when_only_the_core_binary_acl_is_weakened() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl(ACL_BIN, Ok(users_full_control()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert!(e.detail.contains(ACL_BIN), "detail 指错对象：{}", e.detail);
    assert_eq!(proc_ops.snapshot().start_calls, 0);
}

/// 腿③**读不到 → warn 继续**（Q9 拍板：读不到 ≠ 被放宽）。
///
/// 把这条折进拒绝腿，一次 FFI 失败就让用户连不上网，而那台机器的 ACL 可能完全正常
/// （`libcronet.dll` 在不带 cronet 的核里本就缺席，恒落这条腿）。
#[test]
fn start_proceeds_when_the_acl_cannot_be_read() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl_default(Err("GetNamedSecurityInfoW failed: 5".to_owned()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert!(
        resp.to_wire_line().starts_with("OK started"),
        "读不到被当成了被放宽：{}",
        resp.to_wire_line()
    );
    assert_eq!(proc_ops.snapshot().start_calls, 1);
    // 反向对照：自检确实跑了（三个对象都问过），不是「跳过了所以过」。
    assert_eq!(proc_ops.acl_queries().len(), 3);
}

/// 读不到（其中一个）+ 另一个被放宽 → 仍然拒起核。
///
/// 两个篮子各自独立：读不到不会把放宽「冲淡」成 warn。
#[test]
fn an_unreadable_object_does_not_mask_a_weakened_one() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl(ACL_DLL, Err("GetNamedSecurityInfoW failed: 2".to_owned()));
    proc_ops.set_acl(ACL_BIN, Ok(users_full_control()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert_eq!(proc_ops.snapshot().start_calls, 0);
}

/// `ERR coredir-acl-weakened` 的 wire 行必须单行（detail 里带的是路径 + 判据文字，
/// 一个换行就把一帧劈成两行、把后半截当成下一个响应）。
#[test]
fn the_weakened_error_stays_on_one_wire_line() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl_default(Ok(users_full_control()));
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    let line = resp.to_wire_line();
    assert!(line.starts_with("ERR coredir-acl-weakened "), "{line}");
    assert!(!line.contains('\n'), "{line}");
    // 三个对象各一条发现，全都带上了（不是只报第一个就短路）。
    assert_eq!(line.matches("S-1-5-32-545").count(), 3, "{line}");
}

// ===== M2：取材面 = 兜底白名单 ∪ 目录实际条目 =====

/// `C:\ProgramData\Polaris\core\plugin.dll` —— 攻击者预创建、不在任何白名单里的第三个文件。
const ACL_PLANTED: &str = r"C:\ProgramData\Polaris\core\plugin.dll";

/// 目录里**只有枚举才看得见**的那个文件被放宽 ⇒ 必须拒起核。
///
/// 攻击路径全程 Medium IL、零特权：首装前预创建 `core` 目录，放一个文件，给它设
/// `SE_DACL_PROTECTED`（去继承）的 DACL 含 `Users:(F)`。安装脚本的 `/setowner /T` 收得走 owner，
/// 但 `/inheritance:r` + `/grant:r` 的传播**按定义跳过 protected DACL 的子项** ⇒ 那条 `Users:(F)`
/// 原样留着。取材面若是硬编码的两个文件名，自检问不到它 ⇒ 通过。
#[test]
fn start_refuses_when_a_file_only_found_by_enumeration_is_weakened() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_dir_entries(
        ACL_DIR,
        Ok(vec![
            "sing-box.exe".to_owned(),
            "libcronet.dll".to_owned(),
            "plugin.dll".to_owned(),
        ]),
    );
    proc_ops.set_acl(ACL_PLANTED, Ok(users_full_control()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert!(
        e.detail.contains("plugin.dll"),
        "detail 没指到那个预埋文件：{}",
        e.detail
    );
    assert_eq!(proc_ops.snapshot().start_calls, 0, "判了放宽却还是起了核");
    assert!(
        proc_ops.acl_queries().iter().any(|p| p == ACL_PLANTED),
        "枚举出的条目根本没被问过：{:?}",
        proc_ops.acl_queries()
    );
}

/// 枚举重报的白名单名必须去重 —— 否则同一对象被判两遍，findings 里出现重复条目。
///
/// 同时是上一条的正面对照：同样的枚举结果、没有任何对象被放宽时照常起核（不是「一枚举就拒」）。
#[test]
fn enumerated_entries_are_deduplicated_against_the_fallback_names() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_dir_entries(
        ACL_DIR,
        Ok(vec![
            // 真机 read_dir 报磁盘上的大小写，与我们拼的字面量不必逐字相同。
            "SING-BOX.EXE".to_owned(),
            "libcronet.dll".to_owned(),
            "plugin.dll".to_owned(),
        ]),
    );
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert!(resp.to_wire_line().starts_with("OK started"));
    assert_eq!(
        proc_ops.acl_queries(),
        vec![
            ACL_DIR.to_owned(),
            ACL_BIN.to_owned(),
            ACL_DLL.to_owned(),
            ACL_PLANTED.to_owned(),
        ],
        "取材面重复或漏项"
    );
}

/// 目录列不出来 ⇒ warn 继续（Q9：读不到 ≠ 被放宽），且覆盖面**退化成兜底白名单、不归零**。
#[test]
fn start_proceeds_when_the_core_dir_cannot_be_enumerated() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_dir_entries(ACL_DIR, Err("read_dir failed: 5".to_owned()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", start_req_with_ppid(None));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert!(
        resp.to_wire_line().starts_with("OK started"),
        "枚举失败被当成了被放宽：{}",
        resp.to_wire_line()
    );
    // 正面断言：三个兜底对象照样逐个问过（不是「枚举挂了就整条腿跳过」）。
    assert_eq!(
        proc_ops.acl_queries(),
        vec![ACL_DIR.to_owned(), ACL_BIN.to_owned(), ACL_DLL.to_owned()]
    );
    // 反向对照：同一台机器上，兜底面里的对象被放宽仍然拒得住。
    let proc_ops = MockProcOps::new();
    proc_ops.set_dir_entries(ACL_DIR, Err("read_dir failed: 5".to_owned()));
    proc_ops.set_acl(ACL_BIN, Ok(users_full_control()));
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let HandleOutcome::Respond(Response::Err(e)) =
        h.handle("real-token", start_req_with_ppid(None))
    else {
        panic!("枚举失败时兜底面失去了牙");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
}

// ===== M3：未迁移（实际执行面 != 派生落盘面）=====

/// **未迁移 ⇒ warn + 放行，且 ACL 判定根本不被调用**；已迁移 ⇒ 判定被调用。
///
/// 两半用**同一份「全都被放宽」预设**，只换 helper 的迁移状态 —— 否则「放行」可能只是因为预设太松。
///
/// 为什么未迁移不能拒：不一致唯一会发生的状态里 `dir(--singbox)` 就是
/// `%APPDATA%\…\core_update`，owner 是登录用户 ⇒ 必判 `OwnerNotPrivileged` ⇒ 拒起核。而那台机器
/// **本就还没被加固**，它要跑的核与本批之前跑的是同一个 —— 拒它换不来任何安全收益，只是把
/// 「加固未生效」变成「彻底不可用」（spec §3.5 四象限无一 brick / §3.6 未迁移用户不打断）。
/// 可达路径是安装脚本的 catch 回滚腿：icacls 守卫 throw → `Copy-Item $helperBackup` 撞文件锁 ⇒
/// 机器停在「新 helper.exe + 旧 binPath」⇒ 永久 `ERR coredir-acl-weakened`。
#[test]
fn start_skips_the_acl_check_only_when_the_helper_is_not_migrated_yet() {
    // 未迁移：全都放宽也照样起核，且一次 ACL 查询都没发生。
    let unmigrated = MockProcOps::new();
    unmigrated.set_acl_default(Ok(users_full_control()));
    let h = make_helper_unmigrated(unmigrated.clone());
    let HandleOutcome::Respond(resp) = h.handle("real-token", start_req_with_ppid(None)) else {
        panic!("未迁移的机器被 brick 了");
    };
    assert!(
        resp.to_wire_line().starts_with("OK started"),
        "未迁移的机器被拒起核 = brick：{}",
        resp.to_wire_line()
    );
    assert_eq!(unmigrated.snapshot().start_calls, 1);
    assert_eq!(
        unmigrated.acl_queries(),
        Vec::<String>::new(),
        "跳过了却仍然喂了判定（判定一旦跑就必判 OwnerNotPrivileged ⇒ brick）"
    );

    // 反向对照：**同一份预设**下，已迁移的机器判定照跑、照拒 —— 上面的放行不是「门被拆了」。
    let migrated = MockProcOps::new();
    migrated.set_acl_default(Ok(users_full_control()));
    let h = make_helper(migrated.clone(), MockNetTableOps::new());
    let HandleOutcome::Respond(Response::Err(e)) =
        h.handle("real-token", start_req_with_ppid(None))
    else {
        panic!("已迁移的机器没跑判定");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert_eq!(migrated.snapshot().start_calls, 0);
    assert_eq!(migrated.acl_queries().len(), 3);
}

// ===== S5：install-core 落盘前的自检（守的是**落盘目标面**，不是 exec 面）=====

/// 落盘目标被放宽 ⇒ `ERR coredir-acl-weakened`，且**一个字节都不写**。
///
/// 往一个普通用户能改的目录里落核，等于亲手把 sha256 闸校验过的字节交给别人替换。
///
/// 这条腿与起核那条守的是**不同的面**：起核守 `dir(--singbox)`，本条守 `<support>\core`。
/// 未迁移窗口里前者按设计跳过 —— 少了本条，那段窗口里落盘目标没有任何腿在守。
#[test]
fn install_core_refuses_when_the_protected_core_dir_acl_is_weakened() {
    let support = tempfile::tempdir().unwrap();
    let core = support.path().join("core");
    let (src, hash) = make_win_src_dir(b"windows sing-box payload");
    let proc_ops = MockProcOps::new();
    proc_ops.set_acl(&core.to_string_lossy(), Ok(users_full_control()));
    let h = make_helper_with_support(proc_ops.clone(), support.path().to_str().unwrap());

    let out = h.handle("real-token", install_core_req(src.path(), &hash));
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::CoredirAclWeakened);
    assert!(
        e.detail.contains("core"),
        "detail 没说是哪个路径：{}",
        e.detail
    );
    // 正面断言：「拒」的含义是没落盘，不是「回了个错但还是写了」。
    assert!(
        !core.join("sing-box.exe").exists(),
        "判了放宽却还是把核写了进去"
    );
    assert!(!core.join("libcronet.dll").exists());
}

/// 自检确实**在 install-core 的生产路径上被咨询过**，问的是 `<support>\core`（落盘目标面）。
///
/// mock 的兜底是「通过」，所以「这条腿根本没接上」在其它任何断言下都不可见 —— 上一条会照样绿
/// （它预设的那条放宽本来就不会被读到）。这条取材面断言是唯一能把它照出来的东西。
#[test]
fn install_core_consults_the_acl_self_check_on_the_derived_core_dir() {
    let support = tempfile::tempdir().unwrap();
    let core = support.path().join("core");
    let (src, hash) = make_win_src_dir(b"windows sing-box payload");
    let proc_ops = MockProcOps::new();
    let h = make_helper_with_support(proc_ops.clone(), support.path().to_str().unwrap());
    assert_eq!(proc_ops.acl_queries(), Vec::<String>::new());

    let out = h.handle("real-token", install_core_req(src.path(), &hash));
    let HandleOutcome::Respond(resp) = out else {
        panic!("{out:?}");
    };
    assert_eq!(resp.to_wire_line(), "OK installed");
    let dir = core.to_string_lossy().into_owned();
    assert_eq!(
        proc_ops.acl_queries(),
        vec![
            dir.clone(),
            crate::platform::windows::coreacl::join_win(&dir, "sing-box.exe"),
            crate::platform::windows::coreacl::join_win(&dir, "libcronet.dll"),
        ],
        "install-core 没查落盘目标面，或查的是 exec 面（守错目标 = 门形同虚设）"
    );
}

// ===== D2/D3 身份回传 =====

/// status 把 helper 手里那个句柄读到的两件事一并回传；start 只回传 created。
///
/// 两条 wire 断言各锁一个方向：
/// - status 有 `created=` + `image=`（app 据此发现 pid 复用、据此做内核自证）；
/// - start **没有** `image=` —— 那个 hex 路径不是 u64，会让旧 app 的 `parse_start_timing`
///   整段返回 None，五个 timing 字段一起丢。
#[test]
fn status_and_start_carry_the_managed_identity() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_identity(
        Some(133_600_000_000_000_000),
        Some(r"C:\ProgramData\Polaris\core\sing-box.exe"),
    );
    let h = make_helper(proc_ops, MockNetTableOps::new());

    let out = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(start) = out else {
        panic!("{out:?}");
    };
    let start_wire = start.to_wire_line();
    assert!(
        start_wire.contains("created=133600000000000000"),
        "start 未回传身份基线：{start_wire}"
    );
    assert!(
        !start_wire.contains("image="),
        "start 带上了 image ⇒ 旧 app 会丢掉全部 timing：{start_wire}"
    );

    let out = h.handle("real-token", Request::Status);
    let HandleOutcome::Respond(status) = out else {
        panic!("{out:?}");
    };
    let status_wire = status.to_wire_line();
    assert!(
        status_wire.contains("created=133600000000000000"),
        "status 未回传创建时间：{status_wire}"
    );
    // 路径 hex 编码后回来（含空格的路径不会破坏按空白切 token）。
    let Response::Ok(ResponseKind::Status(polaris_helper_proto::Status::Running { image, .. })) =
        polaris_helper_proto::Response::parse(&status_wire)
    else {
        panic!("{status_wire}");
    };
    assert_eq!(
        image.as_deref(),
        Some(r"C:\ProgramData\Polaris\core\sing-box.exe")
    );
}

/// 读不到身份（FFI 失败 / 非受管 pid）→ wire 回到**逐字**的旧形态，旧 app 与新 app 都不受影响。
///
/// 这条是「不可观测就说不可观测」的反向对照：mock 不预设身份即等价于 FFI 全部读失败。
#[test]
fn status_without_identity_falls_back_to_the_old_wire() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let _ = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        }),
    );
    let out = h.handle("real-token", Request::Status);
    let HandleOutcome::Respond(status) = out else {
        panic!("{out:?}");
    };
    assert_eq!(status.to_wire_line(), "OK running 1000");
}

// ===== D4 flush-dns =====

/// flush-dns 成功 → `OK flushed`，且真的调到了 ProcOps 那条腿（不是分派层自己回了个 OK）。
#[test]
fn flush_dns_reports_flushed_and_calls_the_ops_leg() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", Request::FlushDns);
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::FlushDns(
            polaris_helper_proto::FlushDns::Flushed
        )))
    );
    assert_eq!(proc_ops.flush_dns_calls(), 1);
}

/// flush-dns 失败 → `ERR ipconfig <detail>`，detail 原样带回 helper 侧自捕的 stdout 文本。
///
/// 「报错路径日志非空」是本条的验收点：ipconfig 的失败文字只在 stdout，若沿用共用 exec 的
/// 「只带 stderr」格式，这里会得到一条空 detail —— 失败了却说不出为什么。
#[test]
fn flush_dns_failure_carries_the_captured_stdout() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_flush_dns_error(
        "ipconfig /flushdns exit 1: Could not flush the DNS Resolver Cache: Function failed during execution.",
    );
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = h.handle("real-token", Request::FlushDns);
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::Ipconfig);
    assert!(
        e.detail.contains("Could not flush the DNS Resolver Cache"),
        "错误串丢了 stdout：{}",
        e.detail
    );
    // 反向对照：绝不折成 `ERR unknown` —— 那是「旧 helper 不认识这条命令」，app 会当能力缺失。
    assert_ne!(e.code, polaris_helper_proto::ErrorCode::Unknown);
    assert_eq!(proc_ops.flush_dns_calls(), 1);
}

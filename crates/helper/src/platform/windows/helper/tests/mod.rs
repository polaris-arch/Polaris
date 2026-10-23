#![allow(clippy::too_many_lines)]

use super::*;
use crate::platform::windows::ops::{MockNetTableOps, MockProcOps};
use crate::token::StaticTokenStore;
use polaris_helper_proto::{Platform, StartParams};

mod native_birth;
mod wire_gate;

fn native_fixture() -> TestNativeContext {
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest, InstalledCoreManifest};
    let directory = tempfile::tempdir().unwrap();
    let payload = directory.path().join("payload");
    std::fs::create_dir(&payload).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut files = Vec::new();
    for (name, raw) in [
        ("sing-box.exe", b"inert-core-fixture".as_slice()),
        ("libcronet.dll", b"inert-library-fixture".as_slice()),
    ] {
        std::fs::write(payload.join(name), raw).unwrap();
        files.push(CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(raw),
        });
    }
    let core = CoreManifest {
        platform: "win".into(),
        version: "1.15.0-alpha.12".into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: core
            .id(crate::core_hash::sha256_hex)
            .unwrap()
            .as_str()
            .into(),
        app_version: "1.0.0".into(),
        core,
    };
    let core = Arc::new(
        crate::core_store::VerifiedCore::fixture(
            &payload,
            &directory.path().join("lease"),
            manifest,
        )
        .unwrap(),
    );
    TestNativeContext {
        peer: crate::user_control::NativePeer {
            identity: polaris_helper_proto::core_protocol::CallerIdentity::windows(
                "S-1-5-21-1-2-3-1000",
            )
            .unwrap(),
            administrator: false,
        },
        core,
        config: r"C:\Users\polaris\config".into(),
        _directory: directory,
    }
}
fn make_helper(
    proc_ops: MockProcOps,
    net_ops: MockNetTableOps,
) -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    WinHelper::new(
        StaticTokenStore::new("real-token"),
        proc_ops,
        net_ops,
        r"C:\ProgramData\Polaris",
    )
    .with_native_fixture(native_fixture())
}
fn make_helper_defaults() -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    make_helper(MockProcOps::new(), MockNetTableOps::new())
}
fn pinned_start(
    h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>,
    common: StartParams,
) -> Request {
    Request::NativeStartPinned(polaris_helper_proto::core_protocol::PinnedStartParams {
        id: polaris_helper_proto::core_manifest::CoreId::parse(
            &h.test_context.as_ref().unwrap().core.manifest.id,
        )
        .unwrap(),
        common,
    })
}
fn handle_selected(
    h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>,
    token: &str,
    request: Request,
) -> HandleOutcome {
    match h.handle(token, request) {
        HandleOutcome::Respond(Response::Ok(ResponseKind::PinnedStart {
            id,
            start: polaris_helper_proto::core_protocol::PinnedBirthStart::Native(start),
        })) => {
            assert_eq!(
                id.as_str(),
                h.test_context.as_ref().unwrap().core.manifest.id
            );
            HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(start)))
        }
        other => other,
    }
}

// ===== 鉴权 =====

#[test]
fn auth_failed_on_wrong_token() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "wrong-token", Request::Ping);
    assert_eq!(out, HandleOutcome::AuthFailed);
}

#[test]
fn auth_failed_on_empty_token() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "", Request::Ping);
    assert_eq!(out, HandleOutcome::AuthFailed);
}

#[test]
fn auth_passes_on_correct_token() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "real-token", Request::Ping);
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
    let out = handle_selected(&h, "real-token", Request::Version);
    let HandleOutcome::Respond(Response::Ok(ResponseKind::Version { proto_version })) = out else {
        panic!("{out:?}");
    };
    assert_eq!(proto_version, crate::platform::windows::PROTO_VERSION);
}

#[test]
fn status_stopped_when_no_child() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "real-token", Request::Status);
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
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: String::new(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::NoConfig);
}

#[test]
fn start_with_cfg_outside_confdir_is_denied() {
    let h = make_helper_defaults();
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Windows\evil.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
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
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                // cfg 合法，只有 log 越界 —— 单独钉住 log 这一格。
                cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
                log: r"C:\Windows\System32\drivers\etc\hosts".to_owned(),
                fwd: false,
                parent_pid: None,
            },
        ),
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
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
                log: r"C:\Users\polaris\config\singbox-startup.log".to_owned(),
                fwd: false,
                parent_pid: None,
            },
        ),
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
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\singbox-runtime.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
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
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
        polaris_helper_proto::NativeBirthStart::Started {
            target,
            timing: Some(timing),
            ..
        },
    ))) = out
    else {
        panic!("{out:?}");
    };
    // mock next_pid + mock 阶段耗时。
    assert_eq!(target.pid.get(), 1000);
    assert_eq!(timing.total_ms, 0);
    // status 应反映 running
    let out2 = handle_selected(&h, "real-token", Request::Status);
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
    let request = pinned_start(
        &h,
        StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        },
    );
    let _ = handle_selected(&h, "real-token", request.clone());

    proc_ops.set_native_poll(crate::platform::windows::ops::NativeChildPoll::Exited);
    assert!(matches!(
        handle_selected(&h, "real-token", Request::Status),
        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
            polaris_helper_proto::Status::Stopped
        )))
    ));

    proc_ops.set_native_poll(crate::platform::windows::ops::NativeChildPoll::Running);
    let restarted = handle_selected(&h, "real-token", request);
    assert!(matches!(
        restarted,
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
            polaris_helper_proto::NativeBirthStart::Started { target, .. }
        ))) if target.pid.get() == 1001
    ));
}

#[test]
fn start_when_already_running_returns_already() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let cfg = r"C:\Users\polaris\config\c.json".to_owned();
    let req = pinned_start(
        &h,
        StartParams {
            cfg,
            log: String::new(),
            fwd: false,
            parent_pid: None,
        },
    );
    let _ = handle_selected(&h, "real-token", req.clone());
    let out = handle_selected(&h, "real-token", req);
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
            polaris_helper_proto::NativeBirthStart::Already { target }
        ))) if target.pid.get() == 1000
    ));
}

#[test]
fn start_failure_returns_err_start() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_start_error(std::io::Error::other("ENOENT"));
    let h = make_helper(proc_ops, MockNetTableOps::new());
    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::Start);
}

#[test]
fn legacy_fwd_boolean_does_not_create_a_router_operation() {
    use crate::platform::windows::ops::ProcOps;
    for fwd in [false, true] {
        for fail in [false, true] {
            let ops = MockProcOps::new();
            if fail {
                ops.set_start_error(std::io::Error::other("injected spawn failure"));
            }
            let result = ops.start_singbox("fixture-core", "fixture-config", "", fwd);
            assert_eq!(result.is_err(), fail);
            if let Ok(start) = result {
                assert_eq!(start.timing.forwarding_ms, 0);
            }
            assert_eq!(ops.snapshot().start_calls, 1);
            assert_eq!(ops.snapshot().route_calls, 0);
        }
    }
}

// ===== stop =====

#[test]
fn stop_when_not_running_is_idempotent() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "real-token", Request::Stop { pid: None });
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
    let mut h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.test_context.as_mut().unwrap().peer.administrator = true;
    // 先 start
    // Fixture for a pre-native legacy core; production legacy Start is refused.
    let old = proc_ops
        .start_singbox("fixture", "fixture", "", false)
        .unwrap();
    h.child_mu.lock().unwrap().pid = Some(old.pid);
    let snap_before = proc_ops.snapshot();
    // stop
    let out = handle_selected(&h, "real-token", Request::Stop { pid: None });
    assert!(matches!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
            polaris_helper_proto::Stop::Stopped { pid: 1000 }
        )))
    ));
    let snap_after = proc_ops.snapshot();
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls + 1);
    // status 现在应是 stopped
    let out2 = handle_selected(&h, "real-token", Request::Status);
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
    let mut h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.test_context.as_mut().unwrap().peer.administrator = true;
    // daemon 手里的是新会话的核（MockProcOps 的 start 固定报 1000）。
    // Fixture for a pre-native legacy core; production legacy Start is refused.
    let old = proc_ops
        .start_singbox("fixture", "fixture", "", false)
        .unwrap();
    h.child_mu.lock().unwrap().pid = Some(old.pid);
    let snap_before = proc_ops.snapshot();
    // 老 stop 腿声明它要停 4242。
    let out = handle_selected(&h, "real-token", Request::Stop { pid: Some(4242) });
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
            handle_selected(&h, "real-token", Request::Status),
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
    let mut h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.test_context.as_mut().unwrap().peer.administrator = true;
    // Fixture for a pre-native legacy core; production legacy Start is refused.
    let old = proc_ops
        .start_singbox("fixture", "fixture", "", false)
        .unwrap();
    h.child_mu.lock().unwrap().pid = Some(old.pid);
    let snap_before = proc_ops.snapshot();
    let out = handle_selected(&h, "real-token", Request::Stop { pid: Some(1000) });
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
fn cleanup_reaps_only_the_owned_native_birth() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    // 先 start
    let _ = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let snap_before = proc_ops.snapshot();
    let out = handle_selected(&h, "real-token", Request::Cleanup);
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::Cleaned))
    );
    let snap_after = proc_ops.snapshot();
    assert_eq!(proc_ops.native_stop_calls(), 1);
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls);
    assert_eq!(snap_after.terminate_calls, snap_before.terminate_calls);
}

// ===== uninstall =====

#[test]
fn uninstall_requires_bundled_uac_without_spawning_or_closing_helper() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let before = proc_ops.snapshot();
    let out = handle_selected(&h, "real-token", Request::Uninstall);
    match out {
        HandleOutcome::Respond(Response::Err(e)) => {
            assert!(e.detail.contains("native-cleaner-uac-required"));
        }
        other => panic!("expected explicit UAC fallback, got {other:?}"),
    }
    assert_eq!(
        proc_ops.snapshot().spawn_uninstall_calls,
        before.spawn_uninstall_calls
    );
    assert!(!h.child_mu.lock().unwrap().closing);
    assert!(h.dns_admission.launch(|| Ok(())).is_ok());
}

// ===== freeport =====

#[test]
fn freeport_free_when_no_listener() {
    let h = make_helper_defaults();
    let out = handle_selected(&h, "real-token", Request::FreePort { port: 9090 });
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
            polaris_helper_proto::FreePort::Free
        )))
    );
}

#[test]
fn freeport_preserves_a_legacy_named_listener() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_image(1000, r"C:\ProgramData\Polaris\core\sing-box.exe");
    let net_ops = MockNetTableOps::new();
    net_ops.set_entries(vec![crate::platform::windows::logic::ListenEntry {
        pid: 1000,
        port: 9090,
    }]);
    let h = make_helper(proc_ops.clone(), net_ops);
    let out = handle_selected(&h, "real-token", Request::FreePort { port: 9090 });
    let HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
        polaris_helper_proto::FreePort::Foreign { names },
    ))) = out
    else {
        panic!("{out:?}");
    };
    assert_eq!(names, vec!["sing-box.exe".to_owned()]);
    assert_eq!(proc_ops.snapshot().terminate_calls, 0);
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
    let out = handle_selected(&h, "real-token", Request::FreePort { port: 80 });
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
    let out = handle_selected(
        &h,
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
    let out = handle_selected(
        &h,
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
    let out = handle_selected(
        &h,
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
    let _ = handle_selected(
        &h,
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
    let out = handle_selected(
        &h,
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

fn start_req_with_ppid(
    h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>,
    ppid: Option<u32>,
) -> Request {
    pinned_start(
        h,
        StartParams {
            cfg: r"C:\Users\polaris\config\c.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: ppid,
        },
    )
}

#[test]
fn start_with_ppid_wires_watch_parent() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = handle_selected(&h, "real-token", start_req_with_ppid(&h, Some(4242)));
    let snap = proc_ops.snapshot();
    assert_eq!(snap.watch_parent_calls, 1);
    // 传入 (ppid, child_pid)：child_pid = mock start 返回的 1000。
    assert_eq!(proc_ops.last_watch_args(), Some((4242, 1000)));
}

#[test]
fn start_without_ppid_does_not_wire_watch_parent() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = handle_selected(&h, "real-token", start_req_with_ppid(&h, None));
    assert_eq!(proc_ops.snapshot().watch_parent_calls, 0);
}

#[test]
fn start_with_zero_ppid_does_not_wire_watch_parent() {
    // Go: ppid <= 0 → 不启看护。parent_pid=Some(0) 等价（filter ppid>0 排除）。
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = handle_selected(&h, "real-token", start_req_with_ppid(&h, Some(0)));
    assert_eq!(proc_ops.snapshot().watch_parent_calls, 0);
}

#[test]
fn watch_parent_reaps_child_when_parent_dead() {
    // mock spawn_watch_parent 单次评估：父死（alive=false）+ child 仍当前 → on_parent_dead 收割 + 摘 child。
    let proc_ops = MockProcOps::new();
    proc_ops.set_alive(false);
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let snap_before = proc_ops.snapshot();
    let _ = handle_selected(&h, "real-token", start_req_with_ppid(&h, Some(4242)));
    let snap_after = proc_ops.snapshot();
    // on_parent_dead → proc.reap_child(pid) → reap_calls +1。
    assert_eq!(proc_ops.native_stop_calls(), 1);
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls);
    // child 已被摘 → status 现在 stopped。
    let out = handle_selected(&h, "real-token", Request::Status);
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
    let _ = handle_selected(&h, "real-token", start_req_with_ppid(&h, Some(4242)));
    let out = handle_selected(&h, "real-token", Request::Status);
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
    let out = handle_selected(
        &h,
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
    let _ = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let snap_before = proc_ops.snapshot();
    h.reap_child_on_exit().unwrap();
    let snap_after = proc_ops.snapshot();
    assert_eq!(proc_ops.native_stop_calls(), 1);
    assert_eq!(snap_after.reap_calls, snap_before.reap_calls);
}

#[test]
fn reap_child_on_exit_confirms_both_empty_registries() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.reap_child_on_exit().unwrap();
    assert!(h.child_mu.lock().unwrap().closing);
    assert!(proc_ops.native_custody_empty());
    assert_eq!(proc_ops.snapshot().reap_calls, 0);
    assert_eq!(proc_ops.snapshot().terminate_calls, 0);
}

// ===== unsupported commands =====

#[test]
fn mac_linux_commands_return_unknown() {
    // Windows helper 无 default-restore / linux-start / mac 代理事务三条（flush-dns 已由 D4 实现，
    // 见 `flush_dns_*`；install-core 已由 P4 实现，见 `install_core_*`）。
    // 断言体只发 default-restore —— 其余未支持命令的 `ERR unknown` 由本 arm 的 `|` 合并保证。
    let h = make_helper_defaults();
    let out = handle_selected(
        &h,
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
fn every_retired_content_or_start_frame_is_unknown_and_does_not_spawn() {
    let ops = MockProcOps::new();
    let helper = make_helper(ops.clone(), MockNetTableOps::new());
    for command in [
        "install-core",
        "start",
        "start-reap-safe",
        "start-birth-safe",
        "start-native-birth-safe",
    ] {
        let before = ops.snapshot().start_calls;
        let reply = helper.handle_frame(&format!(
            "real-token\n{command}\n/user/source\n{}\n",
            "a".repeat(64)
        ));
        assert!(reply.line.starts_with("ERR unknown"));
        assert_eq!(ops.snapshot().start_calls, before);
    }
}

#[test]
fn status_and_start_carry_the_managed_identity() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_identity(
        Some(133_600_000_000_000_000),
        Some(r"C:\ProgramData\Polaris\core\sing-box.exe"),
    );
    let h = make_helper(proc_ops, MockNetTableOps::new());

    let out = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
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

    let out = handle_selected(&h, "real-token", Request::Status);
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
    let _ = handle_selected(
        &h,
        "real-token",
        pinned_start(
            &h,
            StartParams {
                cfg: r"C:\Users\polaris\config\c.json".to_owned(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
        ),
    );
    let out = handle_selected(&h, "real-token", Request::Status);
    let HandleOutcome::Respond(status) = out else {
        panic!("{out:?}");
    };
    assert_eq!(status.to_wire_line(), "OK running 1000");
}

// ===== D4 flush-dns =====

#[test]
fn shutdown_fences_queued_dns_flush_without_claiming_it_flushed() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    h.begin_shutdown().unwrap();
    let HandleOutcome::Respond(Response::Err(error)) =
        handle_selected(&h, "real-token", Request::FlushDns)
    else {
        panic!("shutdown must reject DNS work");
    };
    assert_eq!(error.code, polaris_helper_proto::ErrorCode::DnsCache);
    assert!(error.detail.contains("shutting down"));
    assert_eq!(proc_ops.flush_dns_calls(), 0);
}

#[test]
fn ordinary_cleanup_keeps_dns_flush_available() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let _ = handle_selected(&h, "real-token", Request::Cleanup);
    assert!(matches!(
        handle_selected(&h, "real-token", Request::FlushDns),
        HandleOutcome::Respond(Response::Ok(ResponseKind::FlushDns(_)))
    ));
    assert_eq!(proc_ops.flush_dns_calls(), 1);
}

/// flush-dns 成功 → `OK flushed`，且真的调到了 ProcOps 那条腿（不是分派层自己回了个 OK）。
#[test]
fn flush_dns_reports_flushed_and_calls_the_ops_leg() {
    let proc_ops = MockProcOps::new();
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = handle_selected(&h, "real-token", Request::FlushDns);
    assert_eq!(
        out,
        HandleOutcome::Respond(Response::Ok(ResponseKind::FlushDns(
            polaris_helper_proto::FlushDns::Flushed
        )))
    );
    assert_eq!(proc_ops.flush_dns_calls(), 1);
}

/// Native cache failure preserves its diagnostic and is never reported as a missing capability.
#[test]
fn flush_dns_failure_preserves_native_error() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_flush_dns_error(
        "native DNS cache flush failed: Could not flush the DNS Resolver Cache: Function failed during execution.",
    );
    let h = make_helper(proc_ops.clone(), MockNetTableOps::new());
    let out = handle_selected(&h, "real-token", Request::FlushDns);
    let HandleOutcome::Respond(Response::Err(e)) = out else {
        panic!("{out:?}");
    };
    assert_eq!(e.code, polaris_helper_proto::ErrorCode::DnsCache);
    assert!(
        e.detail.contains("Could not flush the DNS Resolver Cache"),
        "错误串丢了 stdout：{}",
        e.detail
    );
    // 反向对照：绝不折成 `ERR unknown` —— 那是「旧 helper 不认识这条命令」，app 会当能力缺失。
    assert_ne!(e.code, polaris_helper_proto::ErrorCode::Unknown);
    assert_eq!(proc_ops.flush_dns_calls(), 1);
}

#[test]
fn new_native_cache_wire_command_is_authenticated_and_reaches_the_native_ops_once() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    for (token, expected_calls) in [("wrong-token", 0), ("real-token", 1)] {
        let raw = String::from_utf8(polaris_helper_proto::codec::encode(
            Platform::Win,
            token,
            &Request::WindowsFlushDnsNative,
        ))
        .unwrap();
        let reply = h.handle_frame(&raw);
        assert_eq!(reply.line.starts_with("OK flushed"), expected_calls == 1);
        assert_eq!(ops.flush_dns_calls(), expected_calls);
    }
}

#[test]
fn uninstall_launch_failure_does_not_exit_or_report_uninstalling() {
    let proc_ops = MockProcOps::new();
    proc_ops.set_uninstall_error("missing fixed cleaner");
    let h = make_helper(proc_ops, MockNetTableOps::new());
    assert!(matches!(
        handle_selected(&h, "real-token", Request::Uninstall),
        HandleOutcome::Respond(Response::Err(_))
    ));
}

use super::*;
use crate::platform::windows::ops::NativeChildPoll;
use polaris_helper_proto::{
    HelperBirthTarget, NativeBirthStart, NativeBirthStatus, NativeBirthStop,
};

fn start(
    h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>,
    forwarding: bool,
) -> HelperBirthTarget {
    let reply = h.handle(
        "real-token",
        Request::NativeStartBirth(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".into(),
            log: String::new(),
            fwd: forwarding,
            parent_pid: None,
        }),
    );
    let HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
        NativeBirthStart::Started { target, .. },
    ))) = reply
    else {
        panic!("{reply:?}");
    };
    target
}

fn stop(
    h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>,
    target: HelperBirthTarget,
) -> NativeBirthStop {
    let reply = h.handle("real-token", Request::NativeStopBirth { target });
    let HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStop(stop))) = reply else {
        panic!("{reply:?}");
    };
    stop
}

#[test]
fn legacy_start_is_rejected_before_forwarding_or_process_creation() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let reply = h.handle(
        "real-token",
        Request::Start(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".into(),
            log: String::new(),
            fwd: true,
            parent_pid: None,
        }),
    );
    assert!(
        matches!(reply, HandleOutcome::Respond(Response::Err(error)) if error.detail.contains("client-upgrade-required"))
    );
    assert_eq!(ops.snapshot().start_calls, 0);
    assert_eq!(ops.snapshot().ip_forward_calls, 0);
}

#[test]
fn native_start_frame_round_trips_optional_parent_and_rejects_incomplete_or_extra_args() {
    for parent_pid in [None, Some(4242)] {
        let request = Request::NativeStartBirth(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".into(),
            log: String::new(),
            fwd: true,
            parent_pid,
        });
        let wire = String::from_utf8(polaris_helper_proto::codec::encode(
            polaris_helper_proto::Platform::Win,
            "real-token",
            &request,
        ))
        .unwrap();
        let frame = crate::platform::windows::logic::split_frame(&wire).unwrap();
        assert_eq!(
            crate::platform::windows::logic::parse_request(frame.command, &frame.args),
            Some(request)
        );
    }
    for args in [
        vec!["cfg", "log"],
        vec!["cfg", "log", "bad"],
        vec!["cfg", "log", "0", "0"],
        vec!["cfg", "log", "0", "bad"],
        vec!["cfg", "log", "0", "1", "2"],
    ] {
        assert!(
            crate::platform::windows::logic::parse_request("start-native-birth-safe", &args)
                .is_none()
        );
    }
}

#[test]
fn pending_and_wait_error_keep_exact_native_custody_and_block_replacement_side_effects() {
    for outcome in [NativeChildPoll::Running, NativeChildPoll::Unknown] {
        let ops = MockProcOps::new();
        let h = make_helper(ops.clone(), MockNetTableOps::new());
        let target = start(&h, false);
        ops.set_native_stop(outcome);
        assert_eq!(
            stop(&h, target),
            if outcome == NativeChildPoll::Running {
                NativeBirthStop::Pending { target }
            } else {
                NativeBirthStop::Unknown { target }
            }
        );
        assert_eq!(
            h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
            target
        );
        let reply = h.handle(
            "real-token",
            Request::NativeStartBirth(StartParams {
                cfg: r"C:\Users\polaris\config\other.json".into(),
                log: String::new(),
                fwd: true,
                parent_pid: None,
            }),
        );
        assert!(
            matches!(reply, HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(NativeBirthStart::NotAdmittedPending { target: observed } | NativeBirthStart::NotAdmittedUnknown { target: Some(observed) }))) if observed == target)
        );
        assert_eq!(ops.snapshot().start_calls, 1);
        assert_eq!(ops.snapshot().ip_forward_calls, 0);
        assert_eq!(
            ops.snapshot().reap_calls,
            0,
            "void legacy reap cannot mint native proof"
        );
        ops.set_native_stop(NativeChildPoll::Exited);
        assert_eq!(stop(&h, target), NativeBirthStop::Stopped { target });
        assert!(h.child_mu.lock().unwrap().native.is_none());
        let next = start(&h, false);
        assert_ne!(next.birth, target.birth);
    }
}

#[test]
fn old_terminal_receipt_never_signals_or_clears_the_replacement_birth() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let old = start(&h, false);
    assert_eq!(stop(&h, old), NativeBirthStop::Stopped { target: old });
    let current = start(&h, false);
    let calls = ops.native_stop_calls();
    assert_eq!(stop(&h, old), NativeBirthStop::Stopped { target: old });
    assert_eq!(ops.native_stop_calls(), calls);
    assert_eq!(
        h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
        current
    );
    let wrong = HelperBirthTarget {
        pid: current.pid,
        birth: old.birth,
    };
    assert_eq!(
        stop(&h, wrong),
        NativeBirthStop::Mismatch {
            requested: wrong,
            current
        }
    );
    assert_eq!(ops.native_stop_calls(), calls);
}

#[test]
fn native_exit_with_unconfirmed_tail_keeps_birth_and_refuses_new_forwarding_or_spawn() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let target = start(&h, false);
    ops.set_native_retire_allowed(false);
    assert_eq!(stop(&h, target), NativeBirthStop::Unknown { target });
    assert_eq!(
        h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
        target
    );
    let reply = h.handle(
        "real-token",
        Request::NativeStartBirth(StartParams {
            cfg: r"C:\Users\polaris\config\next.json".into(),
            log: String::new(),
            fwd: true,
            parent_pid: None,
        }),
    );
    assert!(
        matches!(reply, HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(NativeBirthStart::NotAdmittedUnknown { target: Some(observed) }))) if observed == target)
    );
    assert_eq!(ops.snapshot().start_calls, 1);
    assert_eq!(ops.snapshot().ip_forward_calls, 0);
    ops.set_native_retire_allowed(true);
    assert_eq!(stop(&h, target), NativeBirthStop::Stopped { target });
    assert_ne!(start(&h, false).birth, target.birth);
}

#[test]
fn bounded_terminal_cache_eviction_and_helper_restart_are_unknown() {
    let ops = MockProcOps::new();
    let h = make_helper(ops, MockNetTableOps::new());
    let old = start(&h, false);
    assert_eq!(stop(&h, old), NativeBirthStop::Stopped { target: old });
    for _ in 0..16 {
        let target = start(&h, false);
        let _ = stop(&h, target);
    }
    assert_eq!(stop(&h, old), NativeBirthStop::Unknown { target: old });
    assert_eq!(
        stop(&make_helper_defaults(), old),
        NativeBirthStop::Unknown { target: old }
    );
}

#[test]
fn native_status_is_read_only_capability_and_never_mints_a_birth_for_legacy_state() {
    let h = make_helper_defaults();
    h.child_mu.lock().unwrap().pid = Some(42);
    assert_eq!(
        h.handle("real-token", Request::NativeStatusBirth),
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(
            NativeBirthStatus::Unknown { target: None }
        )))
    );
    let bad = h.handle(
        "real-token",
        Request::NativeStartBirth(StartParams {
            cfg: r"C:\Users\polaris\config\c.json".into(),
            log: String::new(),
            fwd: true,
            parent_pid: None,
        }),
    );
    assert!(matches!(
        bad,
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
            NativeBirthStart::NotAdmittedUnknown { target: None }
        )))
    ));
}

#[test]
fn legacy_stop_cleanup_and_service_shutdown_cannot_detach_a_pending_native_object() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let target = start(&h, false);
    ops.set_native_stop(NativeChildPoll::Running);
    for request in [
        Request::Stop {
            pid: Some(target.pid.get()),
        },
        Request::Cleanup,
        Request::Uninstall,
    ] {
        assert!(matches!(
            h.handle("real-token", request),
            HandleOutcome::Respond(Response::Err(_))
        ));
        assert_eq!(
            h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
            target
        );
    }
    assert!(h.reap_child_on_exit().is_err());
    assert_eq!(
        h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
        target
    );
    assert_eq!(ops.snapshot().reap_calls, 0);
    assert_eq!(ops.snapshot().terminate_calls, 0);
    assert_eq!(ops.snapshot().spawn_uninstall_calls, 0);
}

#[test]
fn native_stop_wire_rejects_missing_duplicate_and_extra_identity_lines() {
    let h = make_helper_defaults();
    for args in [
        "",
        "42\n",
        "42\n00112233445566778899aabbccddeeff\nextra\n",
        "42\n00112233445566778899AABBCCDDEEFF\n",
        "42\n42\n",
    ] {
        let frame = format!("real-token\nstop-native-birth-safe\n{args}");
        assert_eq!(h.handle_frame(&frame).line, "ERR unknown\n");
    }
}

#[test]
fn native_windows_wait_source_cannot_upgrade_termination_ack_or_pid_probe_into_exit() {
    let source = include_str!("../../winproc/win.rs");
    let wait = source
        .split("fn native_wait(")
        .nth(1)
        .unwrap()
        .split("fn handle_alive(")
        .next()
        .unwrap();
    assert!(wait.contains("WaitForSingleObject(child.handle.as_raw_handle().cast()"));
    assert!(wait.contains("WAIT_OBJECT_0 =>"));
    assert!(wait.contains("WAIT_TIMEOUT => NativeChildPoll::Running"));
    assert!(!wait.contains("OpenProcess("));
    let stop = source
        .split("fn stop_native_child(")
        .nth(1)
        .unwrap()
        .split("fn retire_native_child(")
        .next()
        .unwrap();
    assert!(stop.contains("terminate_handle(child.handle.as_raw_handle().cast())"));
    assert!(stop.contains("native_wait(child, 2000)"));
    assert!(!stop.contains("take_managed_handle"));
    assert!(!stop.contains("terminate_pid_raw"));
    let spawn = source
        .split("fn spawn_tracked(")
        .nth(1)
        .unwrap()
        .split("/// ensureJob")
        .next()
        .unwrap();
    assert!(spawn.contains("spawn_pipe_loggers_with_preopened_files_custodied("));
    assert!(spawn.contains("spawn_pipe_drainers_custodied(stdout, stderr)"));
    assert!(
        !spawn.contains("std::thread::spawn"),
        "Fresh initialization cannot be delayed beyond admission"
    );
    let retire = source
        .split("fn retire_native_child(")
        .nth(1)
        .unwrap()
        .split("fn reap_child(")
        .next()
        .unwrap();
    assert!(retire.find("log_custody.revoke()").unwrap() < retire.find("*guard = None").unwrap());
}

#[test]
fn shutdown_pending_wait_error_and_logger_tail_failure_cannot_confirm_exit() {
    for outcome in [NativeChildPoll::Running, NativeChildPoll::Unknown] {
        let ops = MockProcOps::new();
        let h = make_helper(ops.clone(), MockNetTableOps::new());
        let target = start(&h, false);
        ops.set_native_stop(outcome);
        assert!(h.reap_child_on_exit().is_err());
        assert!(h.child_mu.lock().unwrap().closing);
        assert!(!ops.native_custody_empty());
        assert!(matches!(
            h.handle("real-token", Request::NativeStartBirth(StartParams {
                cfg: r"C:\Users\polaris\config\next.json".into(),
                log: String::new(), fwd: true, parent_pid: None,
            })),
            HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
                NativeBirthStart::NotAdmittedUnknown { target: Some(observed) }
            ))) if observed == target
        ));
        ops.set_native_stop(NativeChildPoll::Exited);
        ops.set_native_retire_allowed(false);
        assert!(
            h.reap_child_on_exit().is_err(),
            "native exit cannot skip logger retirement"
        );
        assert_eq!(
            h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
            target
        );
        assert!(!ops.native_custody_empty());
        ops.set_native_retire_allowed(true);
        h.reap_child_on_exit().unwrap();
        assert!(h.child_mu.lock().unwrap().native.is_none());
        assert!(ops.native_custody_empty());
        h.reap_child_on_exit().unwrap();
        assert_eq!(ops.snapshot().start_calls, 1);
        assert_eq!(ops.snapshot().ip_forward_calls, 0);
        assert_eq!(ops.snapshot().reap_calls, 0);
        assert_eq!(ops.snapshot().terminate_calls, 0);
    }
}

fn poison_child_state(h: &WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps>) {
    let state = h.child_mu.clone();
    let failed = std::thread::spawn(move || {
        let _guard = state.lock().unwrap();
        panic!("controlled helper state poison");
    })
    .join();
    assert!(failed.is_err());
}

#[test]
fn poisoned_empty_or_cached_slot_never_attests_empty_stopped_or_admission() {
    for with_cache in [false, true] {
        let ops = MockProcOps::new();
        let h = make_helper(ops.clone(), MockNetTableOps::new());
        let target = if with_cache {
            let target = start(&h, false);
            assert_eq!(stop(&h, target), NativeBirthStop::Stopped { target });
            target
        } else {
            HelperBirthTarget {
                pid: std::num::NonZeroU32::new(42).unwrap(),
                birth: polaris_helper_proto::HelperBirthToken::from_bytes([42; 16]),
            }
        };
        let before = ops.snapshot();
        let stops_before = ops.native_stop_calls();
        poison_child_state(&h);
        assert_eq!(stop(&h, target), NativeBirthStop::Unknown { target });
        assert_eq!(
            h.handle("real-token", Request::NativeStatusBirth),
            HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(
                NativeBirthStatus::Unknown { target: None }
            )))
        );
        assert!(matches!(
            h.handle(
                "real-token",
                Request::NativeStartBirth(StartParams {
                    cfg: r"C:\Users\polaris\config\next.json".into(),
                    log: String::new(),
                    fwd: true,
                    parent_pid: None,
                })
            ),
            HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(
                NativeBirthStart::NotAdmittedUnknown { target: None }
            )))
        ));
        for request in [
            Request::Status,
            Request::Stop { pid: None },
            Request::Cleanup,
            Request::Uninstall,
            Request::FreePort { port: 4321 },
            Request::InstallCore(polaris_helper_proto::InstallCoreParams {
                src_dir: r"C:\incoming".into(),
                want_hash: "a".repeat(64),
            }),
        ] {
            assert!(matches!(
                h.handle("real-token", request),
                HandleOutcome::Respond(Response::Err(_))
            ));
        }
        assert!(h.reap_child_on_exit().is_err());
        assert!(h.child_mu.lock().unwrap_err().get_ref().closing);
        assert_eq!(ops.snapshot(), before);
        assert_eq!(ops.native_stop_calls(), stops_before);
    }
}

#[test]
fn poisoned_live_slot_retains_exact_birth_and_cannot_retire_after_native_exit() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let target = start(&h, false);
    ops.set_native_poll(NativeChildPoll::Exited);
    poison_child_state(&h);
    assert_eq!(
        h.handle("real-token", Request::NativeStatusBirth),
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(
            NativeBirthStatus::Unknown {
                target: Some(target)
            }
        )))
    );
    assert_eq!(stop(&h, target), NativeBirthStop::Unknown { target });
    assert!(h.reap_child_on_exit().is_err());
    assert_eq!(
        h.child_mu
            .lock()
            .unwrap_err()
            .get_ref()
            .native
            .as_ref()
            .unwrap()
            .target,
        target
    );
    assert!(!ops.native_custody_empty());
    assert_eq!(ops.native_stop_calls(), 0);
}

#[test]
fn shutdown_cannot_guess_empty_from_missing_upper_entry_or_legacy_pid() {
    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    let birth = ops.mint_native_birth().unwrap();
    ops.start_native_singbox("bin", "cfg", "", false, birth)
        .unwrap();
    assert!(h.reap_child_on_exit().is_err());
    assert_eq!(
        h.handle("real-token", Request::NativeStatusBirth),
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(
            NativeBirthStatus::Unknown { target: None }
        )))
    );
    assert_eq!(ops.native_stop_calls(), 0);

    let ops = MockProcOps::new();
    let h = make_helper(ops.clone(), MockNetTableOps::new());
    h.child_mu.lock().unwrap().pid = Some(42);
    assert!(h.reap_child_on_exit().is_err());
    assert_eq!(h.child_mu.lock().unwrap().pid, Some(42));
    assert_eq!(ops.snapshot().reap_calls, 0);
    assert_eq!(ops.snapshot().terminate_calls, 0);
}

#[test]
fn shutdown_waits_for_start_publication_and_then_fences_replacement() {
    use std::sync::mpsc;
    let ops = MockProcOps::new();
    let h = Arc::new(make_helper(ops.clone(), MockNetTableOps::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    ops.pause_native_publication(entered_tx, resume_rx);
    let starting = h.clone();
    let spawned = std::thread::spawn(move || start(&starting, false));
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        h.child_mu.try_lock().is_err(),
        "Start owns admission through native publication"
    );
    let shutting_down = h.clone();
    let (fenced_tx, fenced_rx) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        shutting_down.begin_shutdown().unwrap();
        fenced_tx.send(()).unwrap();
    });
    assert!(matches!(
        fenced_rx.recv_timeout(std::time::Duration::from_millis(20)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    resume_tx.send(()).unwrap();
    let target = spawned.join().unwrap();
    fenced_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    shutdown.join().unwrap();
    assert_eq!(
        h.child_mu.lock().unwrap().native.as_ref().unwrap().target,
        target
    );
    h.reap_child_on_exit().unwrap();
    assert!(ops.native_custody_empty());
    assert!(h.child_mu.lock().unwrap().closing);
    assert_eq!(ops.snapshot().start_calls, 1);
}

use super::*;

#[test]
fn mock_proc_ops_records_calls() {
    let ops = MockProcOps::new();
    assert!(ops.process_alive(1234)); // 默认存活（宁漏勿误）
    let snap = ops.snapshot();
    let started = ops
        .start_singbox("/x/sing-box", "/c/cfg.json", "", false)
        .unwrap();
    assert_eq!(started.pid, 1000);
    assert_eq!(ops.snapshot().start_calls, snap.start_calls + 1);
    ops.reap_child(1000);
    assert_eq!(ops.last_reaped_pid(), 1000);
}

#[test]
fn mock_proc_ops_start_failure_returns_err() {
    let ops = MockProcOps::new();
    ops.set_start_error(std::io::Error::new(std::io::ErrorKind::NotFound, "ENOENT"));
    let r = ops.start_singbox("/x", "/c", "", false);
    assert!(r.is_err());
    // 第二次无预设错误 → 成功（模拟瞬时失败）
    assert!(ops.start_singbox("/x", "/c", "", false).is_ok());
}

#[test]
fn mock_net_table_listen_pids_filters_target_port() {
    let ops = MockNetTableOps::new();
    ops.set_entries(vec![
        ListenEntry {
            pid: 100,
            port: 9090,
        },
        ListenEntry {
            pid: 200,
            port: 9090,
        },
        ListenEntry { pid: 300, port: 80 },
    ]);
    assert_eq!(ops.listen_pids_for_port(9090).unwrap(), vec![100, 200]);
    assert!(ops.listen_pids_for_port(9999).unwrap().is_empty());
}
#[test]
fn dns_admission_holds_the_shutdown_lock_only_during_spawn() {
    let admission = super::DnsFlushAdmission::default();
    let value = admission
        .launch(|| {
            assert!(matches!(
                admission.0.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            Ok(7)
        })
        .unwrap();
    assert_eq!(value, 7);
    assert!(admission.0.try_lock().is_ok());
    admission.close().unwrap();
    let called = std::cell::Cell::new(false);
    assert!(admission
        .launch(|| {
            called.set(true);
            Ok(())
        })
        .is_err());
    assert!(!called.get());
}

#[test]
fn poisoned_dns_admission_rejects_spawn_and_still_closes() {
    let admission = super::DnsFlushAdmission::default();
    let _ = std::panic::catch_unwind(|| {
        let _: Result<(), String> = admission.launch(|| panic!("injected spawn panic"));
    });
    assert!(admission.close().is_err());
    let called = std::cell::Cell::new(false);
    assert!(admission
        .launch(|| {
            called.set(true);
            Ok(())
        })
        .is_err());
    assert!(!called.get());
}

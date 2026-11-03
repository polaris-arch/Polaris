use super::*;
use crate::proxy::proxy_tests_helpers::MemFs;
use std::sync::Mutex;

#[derive(Default)]
struct MockOps {
    calls: Mutex<Vec<&'static str>>,
    takeover_error: Option<String>,
    revert_error: Option<String>,
}

impl LinuxResolvedOps for MockOps {
    fn takeover(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("takeover");
        self.takeover_error.clone().map_or(Ok(()), Err)
    }

    fn revert(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("revert");
        self.revert_error.clone().map_or(Ok(()), Err)
    }
}

fn controller(ops: MockOps) -> LinuxResolvedController<MockOps, MemFs> {
    LinuxResolvedController::new(ops, MemFs::new(), "/linux-resolved.marker.json")
}

#[test]
fn takeover_writes_intent_before_apply() {
    let mut controller = controller(MockOps::default());
    controller.takeover().unwrap();
    assert!(controller.has_marker());
    assert_eq!(*controller.ops.calls.lock().unwrap(), ["takeover"]);
}

#[test]
fn failed_takeover_clears_marker_after_helper_rollback() {
    let mut controller = controller(MockOps {
        takeover_error: Some("unsupported old helper".to_owned()),
        ..Default::default()
    });
    assert!(controller.takeover().is_err());
    assert!(!controller.has_marker());
}

#[test]
fn restore_clears_only_after_success() {
    let mut controller = controller(MockOps::default());
    controller.takeover().unwrap();
    controller.restore().unwrap();
    assert!(!controller.has_marker());
    assert_eq!(
        *controller.ops.calls.lock().unwrap(),
        ["takeover", "revert"]
    );
}

#[test]
fn failed_restore_keeps_marker_for_crash_recovery() {
    let mut controller = controller(MockOps::default());
    controller.takeover().unwrap();
    controller.ops.revert_error = Some("temporary failure".to_owned());
    assert!(controller.restore().is_err());
    assert!(controller.has_marker());
}

#[test]
fn reconcile_is_gated_by_marker() {
    let mut controller = controller(MockOps::default());
    controller.reconcile().unwrap();
    assert!(controller.ops.calls.lock().unwrap().is_empty());
    controller.takeover().unwrap();
    controller.reconcile().unwrap();
    assert_eq!(
        *controller.ops.calls.lock().unwrap(),
        ["takeover", "takeover"]
    );
}

#[test]
fn checked_marker_distinguishes_absence_invalid_and_wrong_target() {
    let mut controller = controller(MockOps::default());
    assert!(controller.read_marker_checked().unwrap().is_none());
    controller
        .marker
        .fs
        .write_marker(&controller.marker.path, "not json")
        .unwrap();
    assert!(matches!(
        controller.read_marker_checked(),
        Err(LinuxResolvedMarkerError::Invalid(_))
    ));
    assert!(controller.restore().is_err());
    assert!(controller.reconcile().is_err());
    assert!(controller.ops.calls.lock().unwrap().is_empty());
    assert!(controller
        .marker
        .fs
        .read_marker(&controller.marker.path)
        .is_some());

    let wrong = LinuxResolvedMarkerData {
        interface_name: "other-tun".to_owned(),
        server_ip: CONTROLLED_DNS_IP.to_owned(),
        at: 0,
    };
    controller
        .marker
        .fs
        .write_marker(
            &controller.marker.path,
            &serde_json::to_string(&wrong).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        controller.read_marker_checked(),
        Err(LinuxResolvedMarkerError::WrongTarget)
    ));
    assert!(controller.restore().is_err());
    assert!(controller.ops.calls.lock().unwrap().is_empty());
    let wrong = LinuxResolvedMarkerData {
        interface_name: TUN_INTERFACE_NAME.to_owned(),
        server_ip: "127.0.0.2".to_owned(),
        at: 0,
    };
    controller
        .marker
        .fs
        .write_marker(
            &controller.marker.path,
            &serde_json::to_string(&wrong).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        controller.read_marker_checked(),
        Err(LinuxResolvedMarkerError::WrongTarget)
    ));
    assert!(controller.reconcile().is_err());
    assert!(controller.ops.calls.lock().unwrap().is_empty());
}

#[test]
fn checked_io_failure_is_not_the_legacy_missing_marker() {
    struct UnreadableFs;
    impl MarkerFs for UnreadableFs {
        fn write_marker(&self, _: &str, _: &str) -> std::io::Result<()> {
            unreachable!("read failure must not mutate marker")
        }
        fn read_marker(&self, _: &str) -> Option<String> {
            None
        }
        fn read_marker_checked(&self, _: &str) -> std::io::Result<Option<String>> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fixture denied",
            ))
        }
        fn remove_marker(&self, _: &str) -> std::io::Result<()> {
            unreachable!("read failure must not clear marker")
        }
    }
    let mut controller = LinuxResolvedController::new(
        MockOps::default(),
        UnreadableFs,
        "/linux-resolved.marker.json",
    );
    assert!(matches!(
        controller.read_marker_checked(),
        Err(LinuxResolvedMarkerError::Read(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied
    ));
    assert!(controller.restore().unwrap_err().contains("fixture denied"));
    assert!(controller
        .reconcile()
        .unwrap_err()
        .contains("fixture denied"));
    assert!(controller.ops.calls.lock().unwrap().is_empty());
}

#[test]
fn failed_takeover_and_failed_revert_preserve_recovery_marker() {
    let mut controller = controller(MockOps {
        takeover_error: Some("set partially failed".into()),
        revert_error: Some("native revert failed".into()),
        ..Default::default()
    });
    assert!(controller.takeover().is_err());
    assert!(controller.has_marker());
    assert_eq!(
        *controller.ops.calls.lock().unwrap(),
        ["takeover", "revert"]
    );
    controller.ops.revert_error = None;
    controller.restore().unwrap();
    assert!(!controller.has_marker());
}

fn exact_snapshot() -> Snapshot {
    Snapshot {
        pid: 77,
        birth: "01".repeat(16),
        link: polaris_helper_proto::linux_dns::LinkIdentity {
            ifindex: 8,
            sysfs_device: 1,
            sysfs_inode: 80,
            netns_inode: 9,
            tun_flags: 1,
        },
        settings: Settings {
            servers: vec![],
            domains: vec![],
            default_route: false,
            dnssec: "no".into(),
            dns_over_tls: "no".into(),
        },
        original: None,
        owned: false,
        cleanup_pending: false,
    }
}

struct CheckedOps {
    state: Mutex<Snapshot>,
    calls: Mutex<Vec<bool>>,
    no_change: bool,
    partial_set: bool,
    fail_restore: bool,
    gone: bool,
}

impl CheckedOps {
    fn new() -> Self {
        Self {
            state: Mutex::new(exact_snapshot()),
            calls: Mutex::new(vec![]),
            no_change: false,
            partial_set: false,
            fail_restore: false,
            gone: false,
        }
    }
}

impl LinuxResolvedOps for CheckedOps {
    fn takeover(&self) -> Result<(), String> {
        panic!("exact effects cannot downgrade to legacy SET")
    }
    fn revert(&self) -> Result<(), String> {
        panic!("exact effects cannot downgrade to legacy REVERT")
    }
    fn snapshot(&self) -> Result<Snapshot, String> {
        if self.gone {
            Err("native link gone".into())
        } else {
            Ok(self.state.lock().unwrap().clone())
        }
    }
    fn compare(&self, expected: &Snapshot, enabled: bool) -> Result<Change, String> {
        if !enabled && self.gone {
            return Ok(Change::Gone(polaris_helper_proto::linux_dns::Gone {
                pid: expected.pid,
                birth: expected.birth.clone(),
                link: expected.link.clone(),
            }));
        }
        let mut state = self.state.lock().unwrap();
        if self.no_change || &*state != expected {
            return Ok(Change::NoChange);
        }
        self.calls.lock().unwrap().push(enabled);
        if enabled {
            if state.original.is_none() {
                state.original = Some(state.settings.clone());
            }
            state.owned = true;
            state.settings.servers = vec![CONTROLLED_DNS_IP.into()];
            if self.partial_set {
                state.cleanup_pending = true;
                return Err("partial DNS write".into());
            }
            state.settings.domains = vec!["~.".into()];
            state.settings.default_route = true;
            state.settings.dnssec = "no".into();
            state.settings.dns_over_tls = "no".into();
        } else {
            if self.fail_restore {
                state.cleanup_pending = true;
                return Err("restore unavailable".into());
            }
            state.settings = state.original.take().ok_or("missing original settings")?;
            state.owned = false;
        }
        state.cleanup_pending = false;
        Ok(Change::Changed(Box::new(state.clone())))
    }
}

fn checked_controller() -> LinuxResolvedController<CheckedOps, MemFs> {
    LinuxResolvedController::new(CheckedOps::new(), MemFs::new(), "/exact-linux-dns.json")
}

#[test]
fn linux_live_prepare_is_read_only_and_checked_set_restore_preserve_original_scope() {
    let mut c = checked_controller();
    let before = c.ops.state.lock().unwrap().clone();
    let prepared = c.prepare_effect(true).unwrap();
    assert!(c.ops.calls.lock().unwrap().is_empty());
    assert!(!c.has_marker());
    assert_eq!(prepared.before(), &before);
    assert_ne!(prepared.before_facts(), prepared.target_facts());
    let result = c.apply_effect(&prepared, "linux-owner");
    assert_eq!(result.outcome, TakeoverOutcome::Applied);
    assert_eq!(
        c.exact_marker_operation_id().as_deref(),
        Some("linux-owner")
    );
    assert!(c.ops.state.lock().unwrap().settings.controlled());
    let unchanged = c.prepare_effect(true).unwrap();
    assert_eq!(unchanged.before_facts(), unchanged.target_facts());
    assert_eq!(
        c.apply_effect(&unchanged, "not-a-new-owner").outcome,
        TakeoverOutcome::Unchanged
    );
    assert_eq!(
        c.exact_marker_operation_id().as_deref(),
        Some("linux-owner")
    );
    c.restore().unwrap();
    assert!(!c.has_marker());
    assert_eq!(*c.ops.state.lock().unwrap(), before);
    assert_eq!(*c.ops.calls.lock().unwrap(), [true, false]);
}

#[test]
fn linux_live_birth_link_and_settings_drift_reject_before_native_mutation() {
    for field in ["birth", "link", "settings"] {
        let mut c = checked_controller();
        let prepared = c.prepare_effect(true).unwrap();
        {
            let mut state = c.ops.state.lock().unwrap();
            match field {
                "birth" => state.birth = "02".repeat(16),
                "link" => state.link.sysfs_inode += 1,
                _ => state.settings.servers = vec!["9.9.9.9".into()],
            }
        }
        assert_eq!(
            c.apply_effect(&prepared, "stale-plan").outcome,
            TakeoverOutcome::Superseded
        );
        assert!(c.ops.calls.lock().unwrap().is_empty());
        assert!(!c.has_marker());
    }
}

#[test]
fn linux_typed_no_write_rejection_restores_marker_without_claiming_application() {
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    c.ops.no_change = true;
    let result = c.apply_effect(&prepared, "no-write");
    assert_eq!(result.outcome, TakeoverOutcome::Superseded);
    assert!(!result.cleanup_pending);
    assert_eq!(*c.ops.state.lock().unwrap(), exact_snapshot());
    assert!(c.ops.calls.lock().unwrap().is_empty());
    assert!(!c.has_marker());
}

#[test]
fn linux_partial_write_and_failed_restore_retain_exact_recovery_phase_then_explicit_retry() {
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    c.ops.partial_set = true;
    c.ops.fail_restore = true;
    assert_eq!(
        c.apply_effect(&prepared, "partial-owner").outcome,
        TakeoverOutcome::RecoveryPending
    );
    let raw = c.raw().unwrap().unwrap();
    let marker = LiveDnsMarker::parse(&raw).unwrap().unwrap();
    assert_eq!(marker.host_effect.operation_id, "partial-owner");
    assert_eq!(marker.host_effect.original, exact_snapshot().settings);
    assert_eq!(
        c.reconcile_checked().unwrap().unwrap().outcome,
        TakeoverOutcome::RecoveryPending
    );
    let retry = c.prepare_effect(true).unwrap();
    assert!(retry.needs_prior_restore());
    c.ops.partial_set = false;
    c.ops.fail_restore = false;
    c.restore().unwrap();
    let next = c.prepare_after_restore(&retry).unwrap();
    assert_eq!(
        c.apply_effect(&next, "replacement-owner").outcome,
        TakeoverOutcome::Applied
    );
    assert_eq!(
        c.exact_marker_operation_id().as_deref(),
        Some("replacement-owner")
    );
    assert!(c.ops.state.lock().unwrap().settings.controlled());
}

#[test]
fn linux_partial_cleanup_followed_by_no_change_cannot_restore_a_false_owned_marker() {
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    c.ops.partial_set = true;
    c.ops.fail_restore = true;
    assert_eq!(
        c.apply_effect(&prepared, "first").outcome,
        TakeoverOutcome::RecoveryPending
    );
    c.ops.partial_set = false;
    c.ops.fail_restore = false;
    let retry = c.prepare_effect(true).unwrap();
    c.restore().unwrap();
    let next = c.prepare_after_restore(&retry).unwrap();
    c.ops.no_change = true;
    assert_eq!(
        c.apply_effect(&next, "replacement").outcome,
        TakeoverOutcome::Superseded
    );
    assert!(!c.has_marker());
    assert!(!c.ops.state.lock().unwrap().owned);
    assert_eq!(
        c.ops.state.lock().unwrap().settings,
        exact_snapshot().settings
    );
}

#[test]
fn linux_stop_absence_proof_is_for_original_kernel_resource_not_a_successor() {
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "old-birth").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.gone = true;
    c.restore().unwrap();
    assert!(!c.has_marker());
    // A present replacement link is never equivalent to original-object absence.
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "original").outcome,
        TakeoverOutcome::Applied
    );
    c.ops.state.lock().unwrap().link.sysfs_inode += 1;
    let calls = c.ops.calls.lock().unwrap().len();
    assert!(c.restore().is_err());
    assert!(c.has_marker());
    assert_eq!(c.ops.calls.lock().unwrap().len(), calls);
    let marker = LiveDnsMarker::parse(&c.raw().unwrap().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(marker.host_effect.operation_id, "original");
    assert_eq!(marker.host_effect.before.link, exact_snapshot().link);
    assert_eq!(marker.host_effect.phase, "restoring");
}

#[test]
fn linux_exact_restore_retries_lost_ack_only_after_real_baseline_readback() {
    let mut c = checked_controller();
    let prepared = c.prepare_effect(true).unwrap();
    assert_eq!(
        c.apply_effect(&prepared, "lost-ack").outcome,
        TakeoverOutcome::Applied
    );
    let current = c.ops.snapshot().unwrap();
    assert!(matches!(
        c.ops.compare(&current, false).unwrap(),
        Change::Changed(_)
    ));
    assert!(c.has_marker());
    c.restore().unwrap();
    assert!(!c.has_marker());
    assert!(!c.ops.snapshot().unwrap().owned);
}

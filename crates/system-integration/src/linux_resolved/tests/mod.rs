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

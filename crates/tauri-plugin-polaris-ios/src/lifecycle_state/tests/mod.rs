use super::*;

#[test]
fn timed_out_start_receipt_cannot_revive_stopped_or_new_session() {
    let mut state = LifecycleState::default();
    let start = state.begin().unwrap();
    assert!(state.finish(start, None));
    let stop = state.begin().unwrap();
    assert!(state.finish(stop, Some(false)));
    assert!(!state.finish(start, Some(true)));
    assert!(!state.running());
    let next = state.begin().unwrap();
    assert!(!state.observe(stop, Some(true)));
    assert!(!state.finish(start, Some(true)));
    assert!(state.finish(next, Some(true)));
    assert!(state.running());
}

#[test]
fn timeout_in_same_generation_is_terminal_for_that_receipt() {
    let mut state = LifecycleState::default();
    let start = state.begin().unwrap();
    assert!(state.finish(start, None));
    assert!(!state.finish(start, Some(true)));
    assert!(!state.running());
    assert!(state.observe(start, Some(true))); // fresh NE observation, not the late receipt
    assert!(state.running());
}

#[test]
fn cold_start_preserves_system_session_without_stop_request() {
    let mut state = LifecycleState::default();
    assert!(!state.running());
    assert!(state.observe(state.generation(), Some(true)));
    assert!(state.running());
    assert_eq!(state.generation(), 0);
}

#[test]
fn concurrent_status_cannot_overwrite_inflight_operation() {
    let mut state = LifecycleState::default();
    let request = state.begin().unwrap();
    assert!(state.begin().is_err());
    assert!(!state.observe(request, Some(true)));
    assert!(state.finish(request, Some(false)));
}

#[test]
fn failed_system_observation_does_not_leave_stale_running_cache() {
    let mut state = LifecycleState::default();
    assert!(state.observe(0, Some(true)));
    assert!(state.running());
    assert!(state.observe(0, None));
    assert!(!state.running());
}

#[test]
fn exact_stop_revokes_before_native_dispatch_and_rejects_late_receipt() {
    let mut state = LifecycleState::default();
    // Stop claimed before the old shared producer reached the iOS adapter.
    assert_eq!(state.revoke_start_through(7), None);
    assert!(state.begin_start(7, "old-not-dispatched".into()).is_err());
    let a = state.begin_start(9, "request-a".into()).unwrap();
    assert_eq!(state.revoke_start_through(9), Some("request-a".into()));
    assert!(!state.finish(a, Some(true)));
    assert!(!state.running());
    assert!(!state.accepts_receipt(a, 9));
}

#[test]
fn delayed_stop_a_never_revokes_pending_or_completed_b() {
    let mut state = LifecycleState::default();
    let a = state.begin_start(7, "request-a".into()).unwrap();
    assert_eq!(state.revoke_start_through(7), Some("request-a".into()));
    let b = state.begin_start(9, "request-b".into()).unwrap();
    assert_eq!(state.revoke_start_through(7), None);
    assert_eq!(state.generation(), b);
    assert!(!state.finish(a, Some(true)));
    assert!(state.finish(b, Some(true)));
    assert_eq!(state.revoke_start_through(7), None);
    assert!(state.accepts_receipt(b, 9));
    assert!(state.running());
}

#[test]
fn user_stop_claim_also_revokes_older_native_start_while_successor_waits_for_gate() {
    let mut state = LifecycleState::default();
    let a = state.begin_start(1, "native-a".into()).unwrap();
    // Shared Start B claimed 2 but has not reached native because A holds TS gate.
    // User Stop atomically claimed 3, so its captured preceding bound is 2.
    assert_eq!(state.revoke_start_through(2), Some("native-a".into()));
    assert!(!state.finish(a, Some(true)));
    assert!(state.begin_start(2, "late-native-b".into()).is_err());
    let successor = state.begin_start(4, "native-successor".into()).unwrap();
    assert_eq!(state.revoke_start_through(2), None);
    assert_eq!(state.generation(), successor);
    assert!(state.finish(successor, Some(true)));
}

#[test]
fn abandoned_or_timed_out_receiver_keeps_exact_native_start_for_user_stop() {
    for boundary in ["future-dropped", "host-timeout"] {
        let mut state = LifecycleState::default();
        let a = state.begin_start(7, boundary.into()).unwrap();
        assert!(state.abandon(a));
        assert!(!state.running());
        assert!(!state.observe(a, Some(true)));
        assert!(state.begin_start(9, "b-before-user-stop".into()).is_err());
        assert_eq!(state.revoke_start_through(7), Some(boundary.into()));
        let b = state.begin_start(9, "request-b".into()).unwrap();
        assert!(!state.native_terminal(a));
        assert!(!state.finish(a, Some(true)));
        assert_eq!(state.revoke_start_through(7), None);
        assert!(state.finish(b, Some(true)));
        assert!(state.accepts_receipt(b, 9));
    }
}

#[test]
fn native_terminal_after_receiver_loss_releases_registration_without_ready_receipt() {
    let mut state = LifecycleState::default();
    let a = state.begin_start(7, "request-a".into()).unwrap();
    assert!(state.abandon(a));
    assert!(state.native_terminal(a));
    assert!(!state.finish(a, Some(true))); // No receiver can promote a late native result.
    assert!(!state.running());
    assert!(state.begin_start(9, "request-b".into()).is_ok());
}

#[test]
fn stop_after_native_terminal_before_delivery_cannot_grant_a_ready_receipt() {
    let mut state = LifecycleState::default();
    let a = state.begin_start(7, "request-a".into()).unwrap();
    assert!(state.native_terminal(a));
    // Native returned on the detached task, but Rust has not received it yet.
    assert_eq!(state.revoke_start_through(7), None);
    assert!(state.finish(a, Some(true)));
    assert!(state.running()); // NE liveness remains an observation, never readiness.
    assert!(!state.accepts_receipt(a, 7));
}

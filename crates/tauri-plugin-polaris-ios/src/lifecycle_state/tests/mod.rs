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

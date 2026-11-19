use super::*;
fn target(id: &str) -> AndroidExactTarget {
    AndroidExactTarget {
        run_id: id.into(),
        birth_nonce: format!("birth-{id}"),
    }
}
fn event(id: &str, state: TerminalState) -> TerminalEvent {
    let t = target(id);
    TerminalEvent {
        run_id: t.run_id,
        birth_nonce: t.birth_nonce,
        state,
    }
}
#[test]
fn revoke_before_ack_cannot_resurrect_connected() {
    let mut s = Observation::default();
    s.begin("a");
    assert!(!s.observe(event("a", TerminalState::Unavailable)));
    assert!(s.confirm(target("a")));
    assert!(!s.running());
    assert_eq!(
        s.current.as_ref().unwrap().terminal,
        Some(TerminalState::Unavailable)
    );
    assert!(s.observe(event("a", TerminalState::Closed)));
    assert_eq!(
        s.current.as_ref().unwrap().terminal,
        Some(TerminalState::Closed)
    );
}
#[test]
fn old_event_and_late_stop_ack_cannot_clear_successor() {
    let mut s = Observation::default();
    s.begin("a");
    assert!(s.confirm(target("a")));
    let old = s.target();
    s.begin("b");
    assert!(s.confirm(target("b")));
    assert!(!s.observe(event("a", TerminalState::Closed)));
    s.clear_exact(old.as_ref());
    assert!(s.running());
    assert_eq!(s.target(), Some(target("b")));
    let mut wrong = event("b", TerminalState::Closed);
    wrong.birth_nonce = "foreign".into();
    assert!(!s.observe(wrong));
    assert!(s.running());
    let hint = serde_json::from_str::<LifecycleEvent>(r#""systemStartChanged""#).unwrap();
    assert!(matches!(
        hint,
        LifecycleEvent::SystemStatus(SystemStatusEvent::SystemStartChanged)
    ));
    assert!(serde_json::from_str::<LifecycleEvent>("{}").is_err());
    let terminal = serde_json::from_str::<LifecycleEvent>(
        r#"{"runId":"b","birthNonce":"birth-b","state":"closed"}"#,
    )
    .unwrap();
    let LifecycleEvent::Terminal(terminal) = terminal else {
        panic!("exact terminal is a separate frame")
    };
    assert!(s.observe(terminal));
    assert!(!s.running());
}
#[test]
fn rejected_start_does_not_clear_current_and_closed_never_downgrades() {
    let mut s = Observation::default();
    s.begin("a");
    assert!(s.confirm(target("a")));
    s.begin("b");
    s.abort("b");
    assert!(s.running());
    assert!(s.observe(event("a", TerminalState::Unavailable)));
    assert!(!s.observe(event("a", TerminalState::Unavailable)));
    assert!(s.observe(event("a", TerminalState::Closed)));
    assert!(!s.observe(event("a", TerminalState::Unavailable)));
    assert!(!s.running());
    assert_eq!(
        s.current.as_ref().unwrap().terminal,
        Some(TerminalState::Closed)
    );
}

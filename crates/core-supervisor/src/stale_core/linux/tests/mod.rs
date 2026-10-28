use super::*;
#[test]
fn stat_birth_handles_embedded_parentheses_and_rejects_dead_or_missing_identity() {
    let stat = |state| {
        format!(
            "42 (name ) with spaces) {state} {} 123",
            vec!["0"; 18].join(" ")
        )
    };
    assert_eq!(live_birth(stat("S").as_bytes()), Some(123));
    assert_eq!(live_birth(stat("Z").as_bytes()), None);
    assert_eq!(live_birth(b"broken"), None);
    assert!(!ProcessHold::default().has_identity());
    assert_eq!(ProcessHold::default().is_running(), None);
}

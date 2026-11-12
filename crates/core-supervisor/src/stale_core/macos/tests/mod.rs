use super::*;
use crate::process_killer::Signal;

#[test]
fn signals_only_while_the_pid_keeps_the_start_time_seen_at_scan() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = libc::pid_t::try_from(child.id()).unwrap();
    let born = birth(pid).unwrap();
    // Started at or after the scan instant: not the row `ps` listed.
    assert!(!hold(child.id(), born).has_identity());
    // Same PID, another start time: a reused PID is never signalled.
    let reused = HeldProcess {
        pid,
        birth: born - 1,
    };
    assert!(!reused.is_running());
    assert!(!reused.signal(Signal::Sigkill));
    assert!(child.try_wait().unwrap().is_none());

    let held = hold(child.id(), u64::MAX);
    assert!(held.has_identity());
    assert_eq!(held.is_running(), Some(true));
    assert!(held.signal(Signal::Sigkill));
    child.wait().unwrap();
    assert_eq!(held.is_running(), Some(false));
}

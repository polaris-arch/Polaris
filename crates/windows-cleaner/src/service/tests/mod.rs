use super::*;
use std::collections::VecDeque;
struct Fake {
    states: VecDeque<io::Result<State>>,
    stops: VecDeque<io::Result<Stop>>,
    calls: Vec<&'static str>,
    ticks: usize,
}
impl Fake {
    fn new(states: &[State], ticks: usize) -> Self {
        Self {
            states: states.iter().copied().map(Ok).collect(),
            stops: VecDeque::new(),
            calls: Vec::new(),
            ticks,
        }
    }
}
impl Ops for Fake {
    fn state(&mut self) -> io::Result<State> {
        self.calls.push("state");
        self.states
            .pop_front()
            .expect("unexpected extra observation")
    }
    fn stop(&mut self) -> io::Result<Stop> {
        self.calls.push("stop");
        self.stops.pop_front().unwrap_or(Ok(Stop::Requested))
    }
    fn delete(&mut self) -> io::Result<()> {
        self.calls.push("delete");
        Ok(())
    }
    fn pause(&mut self) -> bool {
        self.calls.push("pause");
        if self.ticks == 0 {
            false
        } else {
            self.ticks -= 1;
            true
        }
    }
}
#[test]
fn preexisting_delete_pending_waits_for_actual_absence_without_stop_or_delete() {
    let mut ops = Fake::new(
        &[State::DeletePending, State::DeletePending, State::Absent],
        4,
    );
    remove(&mut ops).unwrap();
    assert_eq!(ops.calls, ["state", "pause", "state", "pause", "state"]);
}
#[test]
fn starting_and_cannot_accept_control_are_retried_then_deletion_is_observed() {
    let mut ops = Fake::new(
        &[
            State::Starting,
            State::Running,
            State::Running,
            State::Stopping,
            State::Stopped,
            State::DeletePending,
            State::Absent,
        ],
        8,
    );
    ops.stops = [Ok(Stop::CannotAccept), Ok(Stop::Requested)].into();
    remove(&mut ops).unwrap();
    assert_eq!(ops.calls.iter().filter(|&&s| s == "stop").count(), 2);
    assert_eq!(ops.calls.iter().filter(|&&s| s == "delete").count(), 1);
    assert_eq!(ops.calls.last(), Some(&"state"));
}
#[test]
fn not_active_control_is_not_a_completion_receipt() {
    let mut ops = Fake::new(&[State::Running, State::Stopped, State::Absent], 4);
    ops.stops.push_back(Ok(Stop::NotActive));
    remove(&mut ops).unwrap();
    assert!(ops.calls.contains(&"delete"));
}
#[test]
fn pending_timeouts_never_become_success() {
    for (state, deleting) in [
        (State::DeletePending, true),
        (State::Starting, false),
        (State::Stopping, false),
    ] {
        let mut ops = Fake::new(&[state, state], 1);
        assert!(
            matches!(remove(&mut ops), Err(Failure::Timeout { deleting: actual }) if actual == deleting)
        );
        assert!(!ops.calls.contains(&"delete"));
    }
}
#[test]
fn service_access_denial_does_not_retry_or_mutate() {
    let mut ops = Fake::new(&[], 4);
    ops.states.push_back(Err(io::Error::from_raw_os_error(5)));
    assert!(matches!(remove(&mut ops), Err(Failure::Operation(e)) if e.raw_os_error() == Some(5)));
    assert_eq!(ops.calls, ["state"]);
}
#[test]
fn delayed_file_release_reruns_the_whole_attempt_before_success() {
    let mut plans = 0;
    retry_cleanup(
        || {
            plans += 1;
            if plans < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        || true,
    )
    .unwrap();
    assert_eq!(plans, 3);
}
#[test]
fn deletion_cannot_delete_retains_status_and_retries_only_its_provenance() {
    let mut attempts = 0;
    retry_cleanup(
        || {
            attempts += 1;
            if attempts == 1 {
                Err(deletion_failure(STATUS_CANNOT_DELETE, 5))
            } else {
                Ok(())
            }
        },
        || true,
    )
    .unwrap();
    assert_eq!(attempts, 2);
    let e = retry_cleanup(|| Err(deletion_failure(STATUS_CANNOT_DELETE, 5)), || false).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    let e = deletion_failure(0xc000_0022u32 as i32, 5);
    assert_eq!(
        e.get_ref()
            .unwrap()
            .downcast_ref::<DeleteStatus>()
            .unwrap()
            .status,
        0xc000_0022u32 as i32
    );
    let e = retry_cleanup(
        || Err(deletion_failure(0xc000_0022u32 as i32, 5)),
        || panic!("real NT access denial must not wait"),
    )
    .unwrap_err();
    assert!(!temporary_file_error(&e));
    assert!(!temporary_file_error(&io::Error::from_raw_os_error(5)));
}
#[test]
fn file_hold_timeout_and_access_denial_remain_failures() {
    for code in [32, 33, 1224] {
        let mut plans = 0;
        let error = retry_cleanup(
            || {
                plans += 1;
                Err(io::Error::from_raw_os_error(code))
            },
            || false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(plans, 1);
    }
    let mut plans = 0;
    let error = retry_cleanup(
        || {
            plans += 1;
            Err(io::Error::from_raw_os_error(5))
        },
        || panic!("access denied must not wait"),
    )
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(5));
    assert_eq!(plans, 1);
}

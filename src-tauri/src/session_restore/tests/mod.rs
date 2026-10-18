use super::*;

#[test]
fn confirmed_restore_reports_failure_and_success_without_a_late_action() {
    for result in [Ok(()), Err("registry denied".into())] {
        let expected = result.clone();
        let received = wait(Duration::from_secs(1), |send| {
            std::thread::spawn(move || complete(send, result, |_| panic!("observed receipt")));
        });
        assert_eq!(received.unwrap(), expected);
    }
}

#[test]
fn timeout_retains_restore_and_its_late_result_is_reconciled_once() {
    for result in [Ok(()), Err("late registry denial".into())] {
        let (release, held) = mpsc::channel();
        let (reported, report) = mpsc::channel();
        let expected = result.clone();
        assert!(matches!(
            wait(Duration::ZERO, |send| {
                std::thread::spawn(move || {
                    held.recv().unwrap(); // The registry call has not returned yet.
                    complete(send, result, |outcome| reported.send(outcome).unwrap());
                });
            }),
            Err(RecvTimeoutError::Timeout)
        ));
        assert!(
            report.try_recv().is_err(),
            "timeout must not fake completed cleanup"
        );
        release.send(()).unwrap();
        assert_eq!(
            report.recv_timeout(Duration::from_secs(1)).unwrap(),
            expected
        );
        assert!(report.recv().is_err(), "late cleanup must run exactly once");
    }
}

#[test]
fn completion_at_the_deadline_cannot_lose_success_or_skip_late_cleanup() {
    for _ in 0..100 {
        let (reported, report) = mpsc::channel();
        let outcome = wait(Duration::ZERO, |send| {
            std::thread::spawn(move || {
                complete(send, Ok(()), |result| reported.send(result).unwrap())
            });
        });
        match outcome {
            Ok(result) => {
                assert!(result.is_ok());
                assert!(report.recv_timeout(Duration::from_secs(1)).is_err());
            }
            Err(_) => assert!(report.recv_timeout(Duration::from_secs(1)).unwrap().is_ok()),
        }
    }
}

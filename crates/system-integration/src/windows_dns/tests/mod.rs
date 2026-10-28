use super::*;

#[test]
fn completed_native_result_is_reported_without_converting_failure_to_success() {
    for result in [Ok(()), Err("DNS service unavailable".into())] {
        let worker = FlushWorker::default();
        let expected = result.clone();
        assert_eq!(
            worker
                .begin(move || result)
                .unwrap()
                .wait(Duration::from_secs(1)),
            expected
        );
    }
}

#[test]
fn timed_out_or_dropped_caller_cannot_stack_workers_while_the_native_call_is_pending() {
    for timeout in [true, false] {
        let worker = FlushWorker::default();
        let (release, held) = mpsc::channel();
        let pending = worker
            .begin(move || {
                held.recv().unwrap();
                Ok(())
            })
            .unwrap();
        if timeout {
            assert!(pending
                .wait(Duration::ZERO)
                .unwrap_err()
                .contains("remains in flight"));
        } else {
            drop(pending);
        }
        assert!(worker
            .begin(|| panic!("busy native call must not be replaced"))
            .is_err());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while worker.busy.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not settle"
            );
            std::thread::yield_now();
        }
        worker
            .begin(|| Ok(()))
            .unwrap()
            .wait(Duration::from_secs(1))
            .unwrap();
    }
}

#[test]
fn panicked_worker_retains_no_busy_receipt_and_reports_no_success() {
    let worker = FlushWorker::default();
    let error = worker
        .begin(|| panic!("native adapter panic fixture"))
        .unwrap()
        .wait(Duration::from_secs(1))
        .unwrap_err();
    assert!(error.contains("without a result"));
    worker
        .begin(|| Ok(()))
        .unwrap()
        .wait(Duration::from_secs(1))
        .unwrap();
}

#[cfg(windows)]
#[test]
fn system_dns_export_can_be_loaded_without_flushing_the_host_cache() {
    // Read-only native CI coverage: resolve and release, never invoke flush().
    let _api = native::load().expect("supported Windows runner must expose the native DNS API");
}

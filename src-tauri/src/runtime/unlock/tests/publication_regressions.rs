use super::*;

struct HeldCheckerHttp {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    script: MockHttp,
}
#[async_trait::async_trait]
impl UnlockHttp for HeldCheckerHttp {
    async fn request(&self, req: &UnlockRequest) -> UnlockResponse {
        if req.url.contains("api.openai.com") {
            self.entered.notify_one();
            let permit = self.release.acquire().await.unwrap();
            permit.forget();
        }
        self.script.request(req).await
    }
}

#[tokio::test]
async fn held_old_checker_cannot_repopulate_stopped_or_new_node_badges() {
    for running in [false, true] {
        let runtime = runtime();
        let sink = RecordingSink::default();
        let http = HeldCheckerHttp {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            script: all_ok_mock(),
        };
        let run = runtime.run_with_budget(&http, &sink, false, || 1_000, BUDGET_UNBOUNDED_MS);
        tokio::pin!(run);
        tokio::select! {
            _ = http.entered.notified() => {},
            _ = &mut run => panic!("checker did not remain held"),
        }
        runtime.invalidate(&sink, running, false);
        let progress_after_invalidation = sink.progress_count();
        http.release.add_permits(1);
        let old = run.await;
        assert!(old.checked_at.is_none());
        assert_eq!(sink.progress_count(), progress_after_invalidation);
        assert!(sink.updated().is_empty());
        assert!(runtime.peek(1_001).is_none());
        // A subsequent legitimate generation still publishes normally.
        if running {
            let new = runtime.run(&all_ok_mock(), &sink, false, || 2_000).await;
            assert!(new.checked_at.is_some());
            assert_eq!(sink.updated(), vec![new]);
        }
    }
}

struct FifoSink(tokio::sync::mpsc::UnboundedSender<UnlockEvent>);
impl UnlockEventSink for FifoSink {
    fn progress(&self, service_id: &str, result: &UnlockResult) {
        self.0
            .send(UnlockEvent::Progress(UnlockProgress {
                service_id: service_id.into(),
                result: result.clone(),
            }))
            .unwrap();
    }
    fn updated(&self, snapshot: &UnlockSnapshot) {
        self.0.send(UnlockEvent::Updated(snapshot.clone())).unwrap();
    }
    fn invalidated(&self, running: bool, exit_blocked: bool) {
        self.0
            .send(UnlockEvent::Invalidated(InvalidatedPayload {
                running,
                exit_blocked,
            }))
            .unwrap();
    }
}

#[test]
fn held_event_delivery_orders_terminal_then_clear_and_rejects_old_blocked_result() {
    let runtime = runtime();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let sink = FifoSink(sender);
    let old_epoch = runtime.epoch();
    let old = UnlockSnapshot::blocked(polaris_unlock::UnlockBlockedReason::ProxyNotRunning);
    assert!(runtime.publish_terminal(&sink, old_epoch, &old));
    // Receiver deliberately held: start/invalidation is queued behind the earlier terminal.
    runtime.invalidate(&sink, true, false);
    assert!(!runtime.publish_terminal(&sink, old_epoch, &old));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        UnlockEvent::Updated(_)
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        UnlockEvent::Invalidated(InvalidatedPayload { running: true, .. })
    ));
    assert!(receiver.try_recv().is_err());
    let current = runtime.epoch();
    assert!(runtime.publish_terminal(&sink, current, &old));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        UnlockEvent::Updated(_)
    ));
}

#[test]
fn shutdown_event_queue_is_permanent_and_drops_pending_delivery() {
    let runtime = runtime();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    runtime.event_queue.lock().unwrap().sender = Some(sender);
    runtime.shutdown_events();
    assert!(runtime.event_queue.lock().unwrap().closed);
    assert!(runtime.event_queue.lock().unwrap().sender.is_none());
    drop(receiver);
}

#[tokio::test]
async fn queued_transport_keeps_capture_epoch_and_never_probes_new_session() {
    for running in [false, true] {
        let runtime = runtime();
        let sink = RecordingSink::default();
        let captured_epoch = runtime.epoch();
        let calls = std::sync::Arc::new(AtomicU64::new(0));
        let observed = calls.clone();
        let http = all_ok_mock().hook(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        let holder = runtime.run_lock.lock().await;
        let queued = runtime.run_at_epoch(&http, &sink, true, || 1_000, captured_epoch);
        tokio::pin!(queued);
        // Poll exactly once while the holder prevents admission; no timing race.
        assert!(futures::poll!(&mut queued).is_pending());
        runtime.invalidate(&sink, running, false);
        drop(holder);
        let result = queued.await;
        assert_eq!(result, UnlockSnapshot::default());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(sink.progress_count(), 0);
        assert!(sink.updated().is_empty());
        assert!(runtime.peek(1_001).is_none());
        assert!(runtime.last_snapshot().is_none());
        assert_eq!(runtime.last_run_at.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn stale_capture_is_rejected_even_before_lock_contention() {
    let runtime = runtime();
    let sink = RecordingSink::default();
    let epoch = runtime.epoch();
    runtime.invalidate(&sink, true, false);
    let http = all_ok_mock().hook(|| panic!("old transport must never be probed"));
    let result = runtime
        .run_at_epoch(&http, &sink, true, || 1_000, epoch)
        .await;
    assert_eq!(result, UnlockSnapshot::default());
    assert!(sink.updated().is_empty());
}

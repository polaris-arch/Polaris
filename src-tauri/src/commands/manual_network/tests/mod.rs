use super::*;
use std::sync::atomic::AtomicUsize;

#[cfg(not(target_os = "ios"))]
#[test]
fn detector_local_rejection_preserves_the_independent_unlock_commit() {
    let (proxy, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let slot = ManualSlot::default();
    let lease = slot.admit("network-check".into()).unwrap();
    let binding = BoundMain {
        proxy,
        ticket,
        request: Arc::clone(&lease.request),
        failure: Arc::new(Mutex::new(None)),
    };
    let unlock_binding = binding.clone();
    binding.check().unwrap();
    let local_calls = AtomicUsize::new(0);
    assert_eq!(
        binding
            .commit::<()>(|| {
                local_calls.fetch_add(1, Ordering::SeqCst);
                Err(ManualCheckFailure::new("superseded"))
            })
            .unwrap_err()
            .code,
        "superseded"
    );
    assert_eq!(local_calls.load(Ordering::SeqCst), 1);
    assert!(binding.failure.lock().unwrap().is_none());
    binding.check().unwrap();
    unlock_binding.check().unwrap();
    let unlock_leg: ManualCheckLeg<UnlockSnapshot> = unlock_binding
        .commit(|| {
            Ok(UnlockSnapshot {
                checked_at: Some(456),
                ..Default::default()
            })
        })
        .into();
    assert_eq!(unlock_leg.data.unwrap().checked_at, Some(456));
    assert!(unlock_leg.error.is_none());
    assert!(binding.failure.lock().unwrap().is_none());
}

#[cfg(not(target_os = "ios"))]
#[test]
fn completed_main_generation_fence_is_sticky_for_both_detector_legs() {
    let (proxy, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let slot = ManualSlot::default();
    let lease = slot.admit("network-check".into()).unwrap();
    let binding = BoundMain {
        proxy: Arc::clone(&proxy),
        ticket,
        request: Arc::clone(&lease.request),
        failure: Arc::new(Mutex::new(None)),
    };
    let other_binding = binding.clone();
    binding.check().unwrap();
    // Actual synchronous publication fence only; no shutdown/Stop transport is run.
    proxy.begin_shutdown().unwrap();
    let calls = AtomicUsize::new(0);
    assert_eq!(
        binding
            .commit(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "superseded"
    );
    assert_eq!(
        binding.failure.lock().unwrap().as_ref().unwrap().code,
        "superseded"
    );
    assert_eq!(binding.check().unwrap_err().code, "superseded");
    assert_eq!(other_binding.check().unwrap_err().code, "superseded");
    assert_eq!(
        other_binding
            .commit(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "superseded"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[cfg(not(target_os = "ios"))]
#[test]
fn actual_saved_config_commit_rejection_stays_sticky_after_config_restoration() {
    let (proxy, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let slot = ManualSlot::default();
    let lease = slot.admit("network-check".into()).unwrap();
    let binding = BoundMain {
        proxy: Arc::clone(&proxy),
        ticket,
        request: Arc::clone(&lease.request),
        failure: Arc::new(Mutex::new(None)),
    };
    let other_binding = binding.clone();
    binding.check().unwrap();
    let before = proxy.config_for_commit_test().current().unwrap();
    let mut changed = before.clone();
    changed["logLevel"] = serde_json::json!("debug");
    proxy
        .config_for_commit_test()
        .save_full_deferred_cleanup(&before, &changed)
        .unwrap();
    let calls = AtomicUsize::new(0);
    assert_eq!(
        binding
            .commit(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "configurationPending"
    );
    assert_eq!(
        binding.failure.lock().unwrap().as_ref().unwrap().code,
        "configurationPending"
    );
    proxy
        .config_for_commit_test()
        .save_full_deferred_cleanup(&changed, &before)
        .unwrap();
    proxy.check_ready_main(&binding.ticket).unwrap();
    assert_eq!(binding.check().unwrap_err().code, "configurationPending");
    assert_eq!(
        other_binding.check().unwrap_err().code,
        "configurationPending"
    );
    assert_eq!(
        other_binding
            .commit(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "configurationPending"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[cfg(not(target_os = "ios"))]
#[test]
fn completed_exact_request_cancel_prevents_the_production_memory_commit() {
    let (proxy, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let generation = ticket.generation();
    let slot = ManualSlot::default();
    let lease = slot.admit("network-check".into()).unwrap();
    let binding = BoundMain {
        proxy: Arc::clone(&proxy),
        ticket,
        request: Arc::clone(&lease.request),
        failure: Arc::new(Mutex::new(None)),
    };
    let writes = AtomicUsize::new(0);
    slot.cancel("different-request");
    binding
        .commit(|| {
            writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| slot.cancel("network-check")).join().unwrap();
    });
    assert_eq!(
        binding
            .commit(|| {
                writes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(
        binding.failure.lock().unwrap().as_ref().unwrap().code,
        "cancelled"
    );
    let other_binding = binding.clone();
    assert_eq!(binding.check().unwrap_err().code, "cancelled");
    assert_eq!(other_binding.check().unwrap_err().code, "cancelled");
    assert_eq!(
        other_binding
            .commit(|| {
                writes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.core_generation(), generation);
    assert!(
        proxy.core_running(),
        "action cancellation leaves normal connection running"
    );
}

#[cfg(not(target_os = "ios"))]
#[test]
fn cancellation_waits_only_for_the_already_valid_short_action_commit() {
    use std::sync::mpsc;
    let (proxy, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let slot = Arc::new(ManualSlot::default());
    let lease = slot.admit("network-check".into()).unwrap();
    let binding = BoundMain {
        proxy: Arc::clone(&proxy),
        ticket,
        request: Arc::clone(&lease.request),
        failure: Arc::new(Mutex::new(None)),
    };
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let committing = binding.clone();
    let commit = std::thread::spawn(move || {
        committing.commit(|| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok("committed-before-cancel")
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let cancelling = Arc::clone(&slot);
    let cancel = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        cancelling.cancel("network-check");
        done_tx.send(()).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
    release_tx.send(()).unwrap();
    assert_eq!(commit.join().unwrap().unwrap(), "committed-before-cancel");
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.join().unwrap();
    assert_eq!(binding.commit(|| Ok(())).unwrap_err().code, "cancelled");
    assert!(proxy.core_running());
}

struct MockOperation {
    preparation_error: Option<&'static str>,
    wait_for_ready: bool,
    ready: Notify,
    preparing: Notify,
    measured: AtomicBool,
    calls: AtomicUsize,
    together: tokio::sync::Barrier,
    ip_failure: bool,
    pending: bool,
    unlock_pending: bool,
}

impl MockOperation {
    fn new() -> Self {
        Self {
            preparation_error: None,
            wait_for_ready: false,
            ready: Notify::new(),
            preparing: Notify::new(),
            measured: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            together: tokio::sync::Barrier::new(2),
            ip_failure: false,
            pending: false,
            unlock_pending: false,
        }
    }
    async fn measure(&self) {
        assert!(
            self.measured.load(Ordering::SeqCst),
            "the normal producer must finish first"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.together.wait().await;
        if self.pending {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl ManualOperation for MockOperation {
    type Ready = ();
    async fn prepare(&self) -> Result<(), ManualCheckFailure> {
        self.preparing.notify_one();
        if self.wait_for_ready {
            self.ready.notified().await;
        }
        if let Some(error) = self.preparation_error {
            return Err(ManualCheckFailure::new(error));
        }
        self.measured.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn context(&self, (): &()) -> ManualCheckContext {
        ManualCheckContext {
            request_id: "action".into(),
            main_generation: 7,
            start_time: Some(123),
        }
    }
    async fn ip_info(&self, (): &()) -> Result<Value, ManualCheckFailure> {
        self.measure().await;
        if self.ip_failure {
            Err(ManualCheckFailure::new("ipInfoFailed"))
        } else {
            Ok(serde_json::json!({"proxy":{"ip":"1.1.1.1"}}))
        }
    }
    async fn unlock(&self, (): &()) -> Result<UnlockSnapshot, ManualCheckFailure> {
        self.measure().await;
        if self.unlock_pending {
            std::future::pending::<()>().await;
        }
        Ok(UnlockSnapshot {
            checked_at: Some(456),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn ready_is_required_then_both_original_legs_are_driven_concurrently() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.wait_for_ready = true;
    let (response, ()) = tokio::join!(perform_manual_check(&operation, &lease.request), async {
        operation.preparing.notified().await;
        assert_eq!(operation.calls.load(Ordering::SeqCst), 0);
        operation.ready.notify_one();
    });
    let response = response.unwrap();
    assert!(response.ip_info.data.is_some());
    assert!(response.unlock.data.is_some());
    assert_eq!(response.context.main_generation, 7);
    assert_eq!(operation.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn original_permission_denial_stop_and_config_failure_never_dispatch_detectors() {
    for code in [
        "IOS_PERMISSION_DENIED",
        "superseded",
        "configurationPending",
        "unsavedConfiguration",
    ] {
        let slot = ManualSlot::default();
        let lease = slot.admit("action".into()).unwrap();
        let mut operation = MockOperation::new();
        operation.preparation_error = Some(code);
        assert_eq!(
            perform_manual_check(&operation, &lease.request)
                .await
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(operation.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn ip_failure_does_not_swallow_unlock_result() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.ip_failure = true;
    let response = perform_manual_check(&operation, &lease.request)
        .await
        .unwrap();
    assert_eq!(response.ip_info.error.unwrap().code, "ipInfoFailed");
    assert_eq!(response.unlock.data.unwrap().checked_at, Some(456));
    assert!(operation.measured.load(Ordering::SeqCst));
}

#[tokio::test]
async fn exact_cancel_during_preparation_drops_only_the_action_waiter() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.wait_for_ready = true;
    let (response, ()) = tokio::join!(perform_manual_check(&operation, &lease.request), async {
        operation.preparing.notified().await;
        slot.cancel("another-action");
        assert!(lease.request.check().is_ok());
        slot.cancel("action");
    });
    assert_eq!(response.unwrap_err().code, "cancelled");
    assert_eq!(operation.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        slot.admit("second".into()).err().unwrap().code,
        "manualCheckBusy"
    );
    drop(lease);
    assert!(slot.admit("second".into()).is_ok());
}

#[tokio::test(start_paused = true)]
async fn target_deadline_does_not_clear_the_normal_connection() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.pending = true;
    let response = perform_manual_check(&operation, &lease.request)
        .await
        .unwrap();
    assert_eq!(response.ip_info.error.unwrap().code, "targetTimedOut");
    assert_eq!(response.unlock.error.unwrap().code, "targetTimedOut");
    assert!(operation.measured.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn one_completed_leg_is_preserved_when_the_other_reaches_its_deadline() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.unlock_pending = true;
    let response = perform_manual_check(&operation, &lease.request)
        .await
        .unwrap();
    assert!(response.ip_info.data.is_some());
    assert_eq!(response.unlock.error.unwrap().code, "targetTimedOut");
    assert!(operation.measured.load(Ordering::SeqCst));
}

#[tokio::test]
async fn cancel_during_detection_closes_both_action_legs_and_keeps_ready_main() {
    let slot = ManualSlot::default();
    let lease = slot.admit("action".into()).unwrap();
    let mut operation = MockOperation::new();
    operation.pending = true;
    let (response, ()) = tokio::join!(perform_manual_check(&operation, &lease.request), async {
        while operation.calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        slot.cancel("action");
    });
    let response = response.unwrap();
    assert_eq!(response.ip_info.error.unwrap().code, "cancelled");
    assert_eq!(response.unlock.error.unwrap().code, "cancelled");
    assert!(operation.measured.load(Ordering::SeqCst));
}

#[derive(Default)]
struct MockBinding(Mutex<Option<String>>);
#[async_trait]
impl ManualBindingGuard for MockBinding {
    fn check(&self) -> Result<(), ManualCheckFailure> {
        self.0
            .lock()
            .unwrap()
            .as_deref()
            .map_or(Ok(()), |code| Err(ManualCheckFailure::new(code)))
    }
}

struct MockTransport<'a> {
    binding: &'a MockBinding,
    calls: AtomicUsize,
    invalidate: &'static str,
}
impl HttpClient for MockTransport<'_> {
    async fn fetch(&self, _url: &str, _init: &FetchInit) -> Result<MinimalResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.binding.0.lock().unwrap() = Some(self.invalidate.into());
        Ok(MinimalResponse {
            status: 200,
            ..Default::default()
        })
    }
}
#[async_trait]
impl UnlockHttp for MockTransport<'_> {
    async fn request(&self, _request: &UnlockRequest) -> UnlockResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.binding.0.lock().unwrap() = Some(self.invalidate.into());
        UnlockResponse::ok(200, "success")
    }
}

#[tokio::test]
async fn actual_transports_reject_stop_successor_and_config_change_after_io() {
    for code in ["superseded", "configurationPending", "readyUnknown"] {
        let binding = MockBinding::default();
        let transport = MockTransport {
            binding: &binding,
            calls: AtomicUsize::new(0),
            invalidate: code,
        };
        let http = GuardedHttp {
            http: &transport,
            guard: &binding,
        };
        let init = FetchInit {
            user_agent: String::new(),
            headers: vec![],
            timeout_ms: None,
            max_body_bytes: None,
        };
        assert_eq!(
            http.fetch("https://example.invalid", &init)
                .await
                .unwrap_err(),
            code
        );
        assert_eq!(
            http.fetch("https://example.invalid", &init)
                .await
                .unwrap_err(),
            code
        );
        assert_eq!(
            transport.calls.load(Ordering::SeqCst),
            1,
            "no successor or direct retry may be dispatched"
        );
        *binding.0.lock().unwrap() = None;
        let unlock = GuardedUnlock {
            http: &transport,
            guard: &binding,
        };
        assert_eq!(
            unlock
                .request(&UnlockRequest::get("https://example.invalid"))
                .await
                .error
                .as_deref(),
            Some(code)
        );
        assert_eq!(
            unlock
                .request(&UnlockRequest::get("https://example.invalid"))
                .await
                .error
                .as_deref(),
            Some(code)
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn invalid_selected_target_is_rejected_before_start_and_context_is_metadata_only() {
    let mut saved = serde_json::to_value(UserConfig::default()).unwrap();
    saved["selectedServerId"] = serde_json::json!("missing");
    assert_eq!(
        selected_targets(&saved).unwrap_err().code,
        "targetNotInMain"
    );
    let value = serde_json::to_value(ManualCheckContext {
        request_id: "action".into(),
        main_generation: 7,
        start_time: None,
    })
    .unwrap();
    assert!(value.get("startTime").is_none());
    assert_eq!(value.as_object().unwrap().len(), 2);
}

#[test]
fn passive_and_automatic_entrypoints_do_not_request_normal_start() {
    use crate::commands::guard_scan::top_level_fn_body;
    use crate::test_support::crate_source;
    for (source, signature) in [
        (
            crate_source("commands/unlock.rs"),
            "pub async fn run_unlock_cycle(",
        ),
        (crate_source("commands/unlock.rs"), "pub fn unlock_get("),
        (
            crate_source("commands/misc/ipinfo.rs"),
            "pub async fn ipinfo_get(",
        ),
        (
            crate_source("commands/misc/ipinfo.rs"),
            "fn schedule_ipinfo_refresh_inner(",
        ),
    ] {
        let body = top_level_fn_body(&source, signature);
        assert!(
            !body.contains("await_normal_main")
                && !body.contains("normal_start_completion")
                && !body.contains("manual_network_check")
        );
    }
}

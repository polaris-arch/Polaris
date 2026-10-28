use super::*;
use polaris_unlock::{UnlockHttp, UnlockRequest, UnlockResponse};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

// Models the non-reentrant producer/action/failure fence held by BoundMain::commit.
// try_lock makes a regression fail immediately rather than hanging the test process.
struct NonReentrantMain(Mutex<()>);
#[async_trait::async_trait]
impl ManualBindingGuard for NonReentrantMain {
    fn check(&self) -> Result<(), ManualCheckFailure> {
        let _guard = self
            .0
            .try_lock()
            .expect("publication reentered the main fence");
        Ok(())
    }
    fn commit<T>(
        &self,
        commit: impl FnOnce() -> Result<T, ManualCheckFailure>,
    ) -> Result<T, ManualCheckFailure> {
        let _guard = self.0.lock().unwrap();
        commit()
    }
}
#[derive(Default)]
struct EnqueueSink {
    updated: AtomicUsize,
    progress: AtomicUsize,
}
impl UnlockEventSink for &EnqueueSink {
    fn progress(&self, _: &str, _: &polaris_unlock::UnlockResult) {
        self.progress.fetch_add(1, Ordering::SeqCst);
    }
    fn updated(&self, _: &UnlockSnapshot) {
        self.updated.fetch_add(1, Ordering::SeqCst);
    }
    fn invalidated(&self, _: bool, _: bool) {}
}
struct SyntheticHttp;
#[async_trait::async_trait]
impl UnlockHttp for SyntheticHttp {
    async fn request(&self, req: &UnlockRequest) -> UnlockResponse {
        if req.url.contains("/cdn-cgi/trace") {
            return UnlockResponse::ok(200, "ip=192.0.2.1\nloc=CN\n");
        }
        UnlockResponse::err("synthetic unavailable response")
    }
}

#[tokio::test]
async fn actual_bound_unlock_sink_publishes_under_main_fence_without_reentry() {
    let unlock = crate::runtime::unlock::UnlockRuntime::default();
    let main = NonReentrantMain(Mutex::new(()));
    let guard = UnlockBinding {
        main: &main,
        unlock: &unlock,
        epoch: unlock.epoch(),
    };
    let enqueued = EnqueueSink::default();
    // The real production wrapper and its with_current_commit implementation, with only
    // the final broadcast enqueue substituted: no AppHandle/native runtime is needed.
    let sink = BoundUnlockSink {
        sink: &enqueued,
        guard: &guard,
    };
    let snapshot = unlock.run(&SyntheticHttp, &sink, false, || 1_000).await;
    assert!(snapshot.checked_at.is_some());
    assert_eq!(enqueued.updated.load(Ordering::SeqCst), 1);
    assert_eq!(
        enqueued.progress.load(Ordering::SeqCst),
        polaris_unlock::ServiceId::ALL.len()
    );
    // TTL replay also travels through the same main fence.
    let cached = unlock.run(&SyntheticHttp, &sink, false, || 1_001).await;
    assert_eq!(cached, snapshot);
    assert_eq!(enqueued.updated.load(Ordering::SeqCst), 2);
}

#[test]
fn bound_blocked_terminal_is_epoch_admitted_without_reentering_main_fence() {
    let unlock = crate::runtime::unlock::UnlockRuntime::default();
    let main = NonReentrantMain(Mutex::new(()));
    let epoch = unlock.epoch();
    let guard = UnlockBinding {
        main: &main,
        unlock: &unlock,
        epoch,
    };
    let enqueued = EnqueueSink::default();
    let sink = BoundUnlockSink {
        sink: &enqueued,
        guard: &guard,
    };
    let blocked = UnlockSnapshot::blocked(polaris_unlock::UnlockBlockedReason::ProxyNotRunning);
    assert!(unlock.publish_terminal(&sink, epoch, &blocked));
    unlock.invalidate(&&enqueued, false, false);
    assert!(!unlock.publish_terminal(&sink, epoch, &blocked));
    assert_eq!(enqueued.updated.load(Ordering::SeqCst), 1);
}

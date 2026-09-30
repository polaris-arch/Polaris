//! Exact temporary Child/config custody across cancellation, failed closes and app exit.
use super::*;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tokio::sync::{mpsc, Semaphore};

async fn shutdown_pc_temp_custody(
    custody: &Arc<PcTempCoreCustody>,
    budget: Duration,
) -> Result<(), String> {
    custody.closing.store(true, Ordering::SeqCst);
    custody.request_close();
    tokio::time::timeout(budget, async {
        let _admission = custody.admission.lock().await;
        custody.retry_close().await
    })
    .await
    .map_err(|_| "退出时测速临时核关闭超时，退出未确认".to_owned())?
}

struct ControlledChild {
    wait_rx: mpsc::UnboundedReceiver<Result<(), String>>,
    close_rx: mpsc::UnboundedReceiver<Result<(), String>>,
    wait_entered: Arc<Semaphore>,
    close_entered: Arc<Semaphore>,
    reaped: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl LoginCoreChild for ControlledChild {
    fn pid(&self) -> Option<u32> {
        None
    }
    async fn wait(&mut self) {
        let _ = self.wait_result().await;
    }
    async fn wait_result(&mut self) -> Result<(), String> {
        self.wait_entered.add_permits(1);
        let result = self.wait_rx.recv().await.expect("wait script sender");
        if result.is_ok() {
            self.reaped.store(true, Ordering::SeqCst);
        }
        result
    }
    async fn terminate(&mut self) {
        let _ = self.close_confirmed().await;
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        self.close_entered.add_permits(1);
        let result = self.close_rx.recv().await.expect("close script sender");
        if result.is_ok() {
            self.reaped.store(true, Ordering::SeqCst);
        }
        result
    }
}

impl Drop for ControlledChild {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct ControlledSpawner {
    child: Mutex<Option<Box<dyn LoginCoreChild>>>,
    spawns: Arc<AtomicUsize>,
    terminated: Arc<AtomicUsize>,
}

#[async_trait]
impl LoginCoreSpawner for ControlledSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        if let StdioPolicy::Drain(sink) = req.stdio {
            sink(Box::new(tokio::io::empty()), Box::new(tokio::io::empty()));
        }
        Ok(self.child.lock().unwrap().take().unwrap_or_else(|| {
            Box::new(FakeChild {
                terminated: self.terminated.clone(),
                pid: None,
                die_after: None,
            })
        }))
    }
}

struct Script {
    harness: Arc<Harness>,
    wait_tx: mpsc::UnboundedSender<Result<(), String>>,
    close_tx: mpsc::UnboundedSender<Result<(), String>>,
    wait_entered: Arc<Semaphore>,
    close_entered: Arc<Semaphore>,
    probe_entered: Arc<Semaphore>,
    reaped: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

impl Script {
    fn new(ready: bool) -> Self {
        let mut h = harness(ready, false, vec![20001, 20002, 20003]);
        let (wait_tx, wait_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        let wait_entered = Arc::new(Semaphore::new(0));
        let close_entered = Arc::new(Semaphore::new(0));
        let probe_entered = Arc::new(Semaphore::new(0));
        let reaped = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        h.deps.spawner = Arc::new(ControlledSpawner {
            child: Mutex::new(Some(Box::new(ControlledChild {
                wait_rx,
                close_rx,
                wait_entered: wait_entered.clone(),
                close_entered: close_entered.clone(),
                reaped: reaped.clone(),
                dropped: dropped.clone(),
            }))),
            spawns: h.spawns.clone(),
            terminated: h.terminated.clone(),
        });
        h.deps.probe_port = {
            let probe_entered = probe_entered.clone();
            Arc::new(move |_| {
                probe_entered.add_permits(1);
                ready
            })
        };
        Self {
            harness: Arc::new(h),
            wait_tx,
            close_tx,
            wait_entered,
            close_entered,
            probe_entered,
            reaped,
            dropped,
        }
    }

    fn run(&self) -> tokio::task::JoinHandle<TempCoreOutcome> {
        let h = self.harness.clone();
        tokio::spawn(async move {
            TempCoreSession::run(
                &h.deps,
                &three_nodes(),
                &|| false,
                |_| std::future::pending::<Option<u32>>(),
                &mut |_, _| {},
            )
            .await
        })
    }

    fn assert_retained(&self) {
        assert!(self.harness.deps.pc_custody.current().unwrap().is_some());
        assert!(self.harness.dir.join(TEMP_CORE_CONFIG_NAME).exists());
        assert!(!self.reaped.load(Ordering::SeqCst));
        assert!(!self.dropped.load(Ordering::SeqCst));
    }

    async fn close_entered(&self) {
        acquire(&self.close_entered).await;
    }

    async fn retry_success(&self) {
        let custody = self.harness.deps.pc_custody.clone();
        let retry = tokio::spawn(async move { custody.retry_close().await });
        self.close_entered().await;
        self.close_tx.send(Ok(())).unwrap();
        retry.await.unwrap().unwrap();
        assert!(self.reaped.load(Ordering::SeqCst));
        assert!(self.dropped.load(Ordering::SeqCst));
        assert!(self.harness.deps.pc_custody.current().unwrap().is_none());
        assert!(!self.harness.dir.join(TEMP_CORE_CONFIG_NAME).exists());
        cleanup(&self.harness.dir);
    }
}

async fn acquire(signal: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(2), signal.acquire())
        .await
        .expect("controlled phase entered")
        .unwrap()
        .forget();
}

#[tokio::test]
async fn cancelled_readiness_retains_child_and_blocks_config_overwrite_until_close_passes() {
    let script = Script::new(false);
    let running = script.run();
    acquire(&script.probe_entered).await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    script.assert_retained();
    let config = std::fs::read(script.harness.dir.join(TEMP_CORE_CONFIG_NAME)).unwrap();
    let next = script.run();
    script.close_entered().await;
    script
        .close_tx
        .send(Err("native close wait failed".into()))
        .unwrap();
    assert!(matches!(
        next.await.unwrap(),
        TempCoreOutcome::CleanupUnknown(_)
    ));
    script.assert_retained();
    assert_eq!(script.harness.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(script.harness.dir.join(TEMP_CORE_CONFIG_NAME)).unwrap(),
        config
    );
    script.retry_success().await;
}

#[tokio::test]
async fn wait_error_and_cancelled_close_leave_the_same_physical_child_in_custody() {
    let script = Script::new(true);
    let running = script.run();
    acquire(&script.wait_entered).await;
    script
        .wait_tx
        .send(Err("native wait error".into()))
        .unwrap();
    script.close_entered().await;
    script.assert_retained();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    script.assert_retained();
    let next = script.run();
    script.close_entered().await;
    script
        .close_tx
        .send(Err("native reap still unknown".into()))
        .unwrap();
    assert!(matches!(
        next.await.unwrap(),
        TempCoreOutcome::CleanupUnknown(_)
    ));
    assert_eq!(script.harness.spawns.load(Ordering::SeqCst), 1);
    script.retry_success().await;
}

#[tokio::test]
async fn shutdown_notification_keeps_custody_until_same_child_close_and_prevents_restart() {
    let script = Script::new(true);
    let running = script.run();
    acquire(&script.wait_entered).await;
    let custody = script.harness.deps.pc_custody.clone();
    let shutdown =
        tokio::spawn(
            async move { shutdown_pc_temp_custody(&custody, Duration::from_secs(2)).await },
        );
    script.close_entered().await;
    script.assert_retained();
    assert!(!shutdown.is_finished());
    assert!(!running.is_finished());
    script.close_tx.send(Ok(())).unwrap();
    assert!(matches!(
        running.await.unwrap(),
        TempCoreOutcome::Ran {
            outcome: "interrupted",
            ..
        }
    ));
    shutdown.await.unwrap().unwrap();
    assert!(script.harness.deps.pc_custody.current().unwrap().is_none());
    assert!(script.dropped.load(Ordering::SeqCst));
    assert!(!script.harness.dir.join(TEMP_CORE_CONFIG_NAME).exists());
    assert!(matches!(
        script.run().await.unwrap(),
        TempCoreOutcome::CleanupUnknown(_)
    ));
    assert_eq!(script.harness.spawns.load(Ordering::SeqCst), 1);
    cleanup(&script.harness.dir);
}

#[tokio::test(start_paused = true)]
async fn shutdown_timeout_retains_unknown_child_config_and_closed_admission() {
    let script = Script::new(true);
    let running = script.run();
    acquire(&script.wait_entered).await;
    let custody = script.harness.deps.pc_custody.clone();
    let shutdown =
        tokio::spawn(
            async move { shutdown_pc_temp_custody(&custody, Duration::from_secs(1)).await },
        );
    script.close_entered().await;
    assert!(shutdown.await.unwrap().is_err());
    script.assert_retained();
    assert!(script
        .harness
        .deps
        .pc_custody
        .closing
        .load(Ordering::SeqCst));
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    script.retry_success().await;
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn exit_request_is_idempotent_and_keeps_pid_exclusion_until_confirmed_close() {
    let _lock = registry_guard();
    let h = harness(true, false, vec![20001, 20002, 20003]);
    let pid = 0xDEAD_BEEF;
    let mut child = h.deps.pc_custody.retain(
        Box::new(FakeChild {
            terminated: h.terminated.clone(),
            pid: Some(pid),
            die_after: None,
        }),
        h.dir.join(TEMP_CORE_CONFIG_NAME),
        false,
    );
    assert_eq!(h.deps.pc_custody.request_close(), 1);
    assert_eq!(h.deps.pc_custody.request_close(), 0);
    assert!(
        inflight_temp_core_pids().contains(&pid),
        "a request is not reap"
    );
    assert_eq!(
        h.terminated.load(Ordering::SeqCst),
        0,
        "no naked PID signal was issued"
    );
    child.close_confirmed().await.unwrap();
    assert_eq!(h.terminated.load(Ordering::SeqCst), 1);
    assert!(!inflight_temp_core_pids().contains(&pid));
    cleanup(&h.dir);
}

#[tokio::test]
async fn delayed_old_close_cannot_retire_successor_config_or_child() {
    let h = harness(true, false, vec![20001, 20002, 20003]);
    let path = h.dir.join(TEMP_CORE_CONFIG_NAME);
    std::fs::write(&path, "old config").unwrap();
    let mut old = h.deps.pc_custody.retain(
        Box::new(FakeChild {
            terminated: h.terminated.clone(),
            pid: None,
            die_after: None,
        }),
        path.clone(),
        false,
    );
    old.close_confirmed().await.unwrap();
    std::fs::write(&path, "successor config").unwrap();
    let mut next = h.deps.pc_custody.retain(
        Box::new(FakeChild {
            terminated: h.terminated.clone(),
            pid: None,
            die_after: None,
        }),
        path.clone(),
        false,
    );
    old.close_confirmed().await.unwrap();
    assert!(Arc::ptr_eq(
        h.deps.pc_custody.current().unwrap().as_ref().unwrap(),
        &next.birth
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "successor config");
    assert_eq!(h.terminated.load(Ordering::SeqCst), 1);
    next.close_confirmed().await.unwrap();
    cleanup(&h.dir);
}

struct UnprovenChild {
    terminated: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl LoginCoreChild for UnprovenChild {
    fn pid(&self) -> Option<u32> {
        None
    }
    async fn wait(&mut self) {
        std::future::pending::<()>().await;
    }
    async fn terminate(&mut self) {
        self.terminated.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for UnprovenChild {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn default_close_keeps_temporary_custody_and_configuration_unknown() {
    let h = harness(true, false, vec![20001, 20002, 20003]);
    let path = h.dir.join(TEMP_CORE_CONFIG_NAME);
    std::fs::write(&path, "still owned config").unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let mut child = h.deps.pc_custody.retain(
        Box::new(UnprovenChild {
            terminated: h.terminated.clone(),
            dropped: dropped.clone(),
        }),
        path.clone(),
        false,
    );
    assert!(child.close_confirmed().await.is_err());
    assert!(h.deps.pc_custody.retry_close().await.is_err());
    assert_eq!(
        h.terminated.load(Ordering::SeqCst),
        2,
        "void terminate was called but supplied no proof"
    );
    assert!(h.deps.pc_custody.current().unwrap().is_some());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "still owned config"
    );
    drop(child);
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the borrow's Drop cannot release Unknown Child"
    );
    // Test teardown only; production has no force-vacate operation.
    h.deps.pc_custody.birth.lock().unwrap().take();
    assert!(dropped.load(Ordering::SeqCst));
    cleanup(&h.dir);
}

struct DelayedSpawner {
    base: Arc<dyn LoginCoreSpawner>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl LoginCoreSpawner for DelayedSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.base.spawn(req).await
    }
}

#[tokio::test]
async fn shutdown_between_spawn_admission_and_publication_notifies_the_returned_birth() {
    let mut script = Script::new(false);
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let h = Arc::get_mut(&mut script.harness).unwrap();
    h.deps.spawner = Arc::new(DelayedSpawner {
        base: h.deps.spawner.clone(),
        entered: entered.clone(),
        release: release.clone(),
    });
    let running = script.run();
    acquire(&entered).await;
    assert!(script.harness.deps.pc_custody.current().unwrap().is_none());
    let custody = script.harness.deps.pc_custody.clone();
    let shutdown =
        tokio::spawn(
            async move { shutdown_pc_temp_custody(&custody, Duration::from_secs(2)).await },
        );
    while !script
        .harness
        .deps
        .pc_custody
        .closing
        .load(Ordering::SeqCst)
    {
        tokio::task::yield_now().await;
    }
    release.add_permits(1);
    script.close_entered().await;
    script.assert_retained();
    script.close_tx.send(Ok(())).unwrap();
    assert!(matches!(
        running.await.unwrap(),
        TempCoreOutcome::Superseded
    ));
    shutdown.await.unwrap().unwrap();
    assert!(script.harness.deps.pc_custody.current().unwrap().is_none());
    assert!(script.dropped.load(Ordering::SeqCst));
    cleanup(&script.harness.dir);
}

struct ValidationDebtChecker {
    checks: Arc<AtomicUsize>,
    error: polaris_core_supervisor::ValidationLifecycleError,
}

#[async_trait]
impl ConfigChecker for ValidationDebtChecker {
    async fn check(&self, _binary: &Path, _config: &Path) -> Result<(), String> {
        panic!("writer admission must preserve typed validation failure");
    }

    async fn check_for_spawn(
        &self,
        _binary: &Path,
        _config: &Path,
    ) -> Result<(), ConfigCheckFailure> {
        match self.checks.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(()),
            1 => Err(ConfigCheckFailure::Lifecycle(self.error.clone())),
            _ => panic!("another batch checked after validation lifecycle failure"),
        }
    }
}

struct ValidationDebtSpawner {
    inner: Arc<dyn LoginCoreSpawner>,
    attempts: Arc<AtomicUsize>,
    error: polaris_core_supervisor::ValidationLifecycleError,
}

struct LaterCapacityChecker {
    checks: Arc<AtomicUsize>,
}

#[async_trait]
impl ConfigChecker for LaterCapacityChecker {
    async fn check(&self, _: &Path, _: &Path) -> Result<(), String> {
        panic!("spawn admission must call the typed Android method");
    }

    async fn check_admitted(
        &self,
        _: &Path,
        _: &Path,
    ) -> Result<(), crate::runtime::proxy::android_capacity::CheckFailure> {
        use crate::runtime::proxy::android_capacity::{CapacityClosed, CheckFailure};
        match self.checks.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(()),
            1 => Err(CheckFailure::CapacityClosed(CapacityClosed)),
            _ => panic!("another batch checked after capacity admission closed"),
        }
    }
}

#[async_trait]
impl LoginCoreSpawner for ValidationDebtSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        match self.attempts.fetch_add(1, Ordering::SeqCst) {
            0 => self.inner.spawn(req).await,
            1 => Err(SpawnError::Spawn {
                bin: req.binary,
                source: std::io::Error::other(self.error.clone()),
            }),
            _ => panic!("another batch spawned after validation admission failure"),
        }
    }
}

#[tokio::test]
async fn successful_batch_then_check_lifecycle_failure_aborts_without_normal_done() {
    let nodes = naive_nodes(TEMP_CORE_BATCH_MAX_NODES * 3);
    assert!(plan_temp_core_batches(&nodes).len() >= 3);
    for error in [
        polaris_core_supervisor::ValidationLifecycleError::Closing,
        polaris_core_supervisor::ValidationLifecycleError::CleanupUnconfirmed(
            "native wait failed".into(),
        ),
    ] {
        let mut h = multi_batch_harness(nodes.len(), None);
        let checks = Arc::new(AtomicUsize::new(0));
        h.deps.checker = Arc::new(ValidationDebtChecker {
            checks: checks.clone(),
            error: error.clone(),
        });
        let (outcome, events) = run_round(&h, &nodes).await;
        assert!(
            matches!(outcome, TempCoreOutcome::CleanupUnknown(ref detail) if detail == &error.to_string()),
            "{outcome:?}"
        );
        assert!(
            events
                .iter()
                .any(|(event, _)| event == EVENT_SPEED_TEST_RESULT),
            "the first batch must actually measure"
        );
        assert!(!events
            .iter()
            .any(|(event, _)| event == EVENT_SPEED_TEST_DONE));
        assert_eq!(checks.load(Ordering::SeqCst), 2);
        assert_eq!(h.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(h.terminated.load(Ordering::SeqCst), 1);
        assert!(h.deps.pc_custody.current().unwrap().is_none());
        cleanup(&h.dir);
    }
}

#[tokio::test]
async fn successful_batch_then_admitted_capacity_failure_aborts_without_normal_done() {
    let nodes = naive_nodes(TEMP_CORE_BATCH_MAX_NODES * 3);
    assert!(plan_temp_core_batches(&nodes).len() >= 3);
    let mut h = multi_batch_harness(nodes.len(), None);
    let checks = Arc::new(AtomicUsize::new(0));
    h.deps.checker = Arc::new(LaterCapacityChecker {
        checks: checks.clone(),
    });
    let (outcome, events) = run_round(&h, &nodes).await;
    assert!(matches!(outcome, TempCoreOutcome::AndroidCapacityClosed(_)));
    assert!(events
        .iter()
        .any(|(event, _)| event == EVENT_SPEED_TEST_RESULT));
    assert!(!events
        .iter()
        .any(|(event, _)| event == EVENT_SPEED_TEST_DONE));
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(h.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(h.terminated.load(Ordering::SeqCst), 1);
    assert!(h.deps.pc_custody.current().unwrap().is_none());
    cleanup(&h.dir);
}

#[tokio::test]
async fn successful_batch_then_typed_spawn_admission_failure_cannot_become_partial_ran() {
    let nodes = naive_nodes(TEMP_CORE_BATCH_MAX_NODES * 3);
    assert!(plan_temp_core_batches(&nodes).len() >= 3);
    for error in [
        polaris_core_supervisor::ValidationLifecycleError::Closing,
        polaris_core_supervisor::ValidationLifecycleError::CleanupUnconfirmed(
            "old check custody retained".into(),
        ),
    ] {
        let mut h = multi_batch_harness(nodes.len(), None);
        let attempts = Arc::new(AtomicUsize::new(0));
        h.deps.spawner = Arc::new(ValidationDebtSpawner {
            inner: h.deps.spawner.clone(),
            attempts: attempts.clone(),
            error: error.clone(),
        });
        let (outcome, events) = run_round(&h, &nodes).await;
        assert!(
            matches!(outcome, TempCoreOutcome::CleanupUnknown(ref detail) if detail == &error.to_string()),
            "{outcome:?}"
        );
        assert!(events
            .iter()
            .any(|(event, _)| event == EVENT_SPEED_TEST_RESULT));
        assert!(!events
            .iter()
            .any(|(event, _)| event == EVENT_SPEED_TEST_DONE));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(h.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(h.terminated.load(Ordering::SeqCst), 1);
        assert!(h.deps.pc_custody.current().unwrap().is_none());
        cleanup(&h.dir);
    }
}

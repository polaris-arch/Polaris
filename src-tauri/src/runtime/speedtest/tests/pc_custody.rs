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

#[cfg(unix)]
fn native_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("crates/core-supervisor/src/config_gate/check_custody/tests/fixtures")
        .join(name)
}

#[cfg(unix)]
fn native_harness(name: &str) -> Harness {
    let mut h = harness(true, false, vec![20001, 20002, 20003]);
    let binary = native_fixture(name);
    h.deps.resolve_binary = Arc::new(move || Ok(binary.clone()));
    h.deps.spawner = Arc::new(TokioLoginCoreSpawner);
    h
}

#[cfg(unix)]
async fn native_child(h: &Harness, name: &str) -> PcCustodiedChild {
    let path = h.dir.join(TEMP_CORE_CONFIG_NAME);
    std::fs::write(&path, "owned native stand-in config").unwrap();
    let prepared = TokioLoginCoreSpawner.prepare_temp_native_birth().unwrap();
    let booked = h
        .deps
        .pc_custody
        .book_native(&prepared, path.clone(), false)
        .unwrap();
    booked
        .dispatch_native(
            &TokioLoginCoreSpawner,
            SpawnRequest::new(native_fixture(name), path, StdioPolicy::Discard),
            prepared,
        )
        .await
        .unwrap()
}

#[cfg(unix)]
async fn native_fact(child: &PcCustodiedChild) -> NativeTransientExit {
    child
        .birth
        .child
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native_exit()
        .unwrap()
}

#[cfg(unix)]
struct RecordingNativeSpawner {
    custody: Arc<PcTempCoreCustody>,
    births: Arc<Mutex<Vec<Arc<PcTempCoreBirth>>>>,
}

#[cfg(unix)]
#[async_trait]
impl LoginCoreSpawner for RecordingNativeSpawner {
    async fn spawn(&self, _: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        panic!("native preparation must not downgrade to ordinary spawn");
    }
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        TokioLoginCoreSpawner.prepare_temp_native_birth()
    }
    async fn spawn_with_temp_native_birth(
        &self,
        req: SpawnRequest,
        prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        assert!(prepared.is_some(), "native booking must stay strict");
        let birth = self
            .custody
            .current()
            .unwrap()
            .expect("original slot booked before factory");
        assert!(birth.native.is_some());
        assert!(birth.config_path.exists());
        assert!(
            birth.child.try_lock().is_err(),
            "original Child lock predates dispatch"
        );
        self.births.lock().unwrap().push(birth);
        TokioLoginCoreSpawner
            .spawn_with_temp_native_birth(req, prepared)
            .await
    }
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn actual_native_production_batches_retire_exit_zero_and_seven_and_admit_successor() {
    let _registry = registry_guard();
    for (name, code) in [("exit-success.sh", 0), ("exit-seven.sh", 7)] {
        let mut h = native_harness(name);
        let births = Arc::new(Mutex::new(Vec::new()));
        h.deps.spawner = Arc::new(RecordingNativeSpawner {
            custody: h.deps.pc_custody.clone(),
            births: births.clone(),
        });
        for index in 0..2 {
            let mut events = Vec::new();
            let outcome = TempCoreSession::run(
                &h.deps,
                &three_nodes(),
                &|| false,
                |_| std::future::pending::<Option<u32>>(),
                &mut |event, payload| events.push((event.to_owned(), payload)),
            )
            .await;
            assert!(
                !matches!(outcome, TempCoreOutcome::CleanupUnknown(_)),
                "{outcome:?}"
            );
            assert!(h.deps.pc_custody.current().unwrap().is_none());
            assert!(!h.dir.join(TEMP_CORE_CONFIG_NAME).exists());
            assert!(
                events
                    .iter()
                    .filter(|(event, _)| event == EVENT_SPEED_TEST_DONE)
                    .count()
                    <= 1
            );
            let birth = births.lock().unwrap()[index].clone();
            assert!(birth.has_native_terminal());
            assert!(birth.retired.load(Ordering::SeqCst));
            let fact = birth
                .child
                .lock()
                .await
                .as_ref()
                .unwrap()
                .native_exit()
                .unwrap();
            assert_eq!(fact.status_for_test().code(), Some(code));
        }
        cleanup(&h.dir);
    }
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn native_wait_caches_same_status_and_old_close_cannot_touch_successor() {
    let _registry = registry_guard();
    let h = native_harness("exit-seven.sh");
    let mut old = native_child(&h, "exit-seven.sh").await;
    assert!(old
        .birth
        .child
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native_exit()
        .is_none());
    old.wait_result().await.unwrap();
    let fact = native_fact(&old).await;
    assert_eq!(fact.status_for_test().code(), Some(7));
    old.wait_result().await.unwrap();
    assert_eq!(
        native_fact(&old).await.status_for_test(),
        fact.status_for_test()
    );
    let replay = old
        .birth
        .native
        .as_ref()
        .unwrap()
        .replayed_preparation_for_test();
    old.close_confirmed().await.unwrap();
    let mut next = native_child(&h, "sleep-two-seconds.sh").await;
    let path = h.dir.join(TEMP_CORE_CONFIG_NAME);
    let bytes = std::fs::read(&path).unwrap();
    assert!(h
        .deps
        .pc_custody
        .book_native(&replay, path.clone(), false)
        .is_err());
    old.close_confirmed().await.unwrap();
    assert!(h
        .deps
        .pc_custody
        .retire(&next.birth, PcTempRetirement::Close(Some(fact)))
        .is_err());
    assert!(Arc::ptr_eq(
        h.deps.pc_custody.current().unwrap().as_ref().unwrap(),
        &next.birth
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!next.birth.has_native_terminal());
    next.close_confirmed().await.unwrap();
    // Replaying a retired record into a vacant slot also fails its original one-shot binding.
    std::fs::write(&path, "later config").unwrap();
    assert!(h
        .deps
        .pc_custody
        .book_native(&replay, path.clone(), false)
        .is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "later config");
    cleanup(&h.dir);
}

#[cfg(all(target_os = "linux", not(target_env = "uclibc")))]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn actual_try_wait_some_preserves_native_status_and_cached_close_is_idempotent() {
    let _registry = registry_guard();
    let h = native_harness("exit-seven.sh");
    let mut child = native_child(&h, "exit-seven.sh").await;
    let pid = child.pid().unwrap();
    // WNOWAIT observes this owned fixture's actual exit without stealing Tokio's wait identity.
    let observed = tokio::task::spawn_blocking(move || {
        nix::sys::wait::waitid(
            nix::sys::wait::Id::Pid(nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap())),
            nix::sys::wait::WaitPidFlag::WEXITED | nix::sys::wait::WaitPidFlag::WNOWAIT,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(observed, nix::sys::wait::WaitStatus::Exited(_, 7)));
    assert!(child
        .birth
        .child
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native_exit()
        .is_none());
    child.close_confirmed().await.unwrap();
    let fact = native_fact(&child).await;
    assert_eq!(fact.status_for_test().code(), Some(7));
    assert!(child.birth.has_native_terminal());
    child.close_confirmed().await.unwrap();
    child.wait_result().await.unwrap();
    assert_eq!(
        native_fact(&child).await.status_for_test(),
        fact.status_for_test()
    );
    assert!(h.deps.pc_custody.current().unwrap().is_none());
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn original_consumer_rejects_missing_foreign_member_role_scope_and_status() {
    let _registry = registry_guard();
    let a = native_harness("exit-seven.sh");
    let b = native_harness("exit-success.sh");
    let mut child_a = native_child(&a, "exit-seven.sh").await;
    let mut child_b = native_child(&b, "exit-success.sh").await;
    child_a.wait_result().await.unwrap();
    child_b.wait_result().await.unwrap();
    let fact_a = native_fact(&child_a).await;
    let fact_b = native_fact(&child_b).await;
    let path = a.dir.join(TEMP_CORE_CONFIG_NAME);
    let config = std::fs::read(&path).unwrap();
    let pid = child_a.pid().unwrap();
    let mut rejected = vec![None, Some(fact_b.clone())];
    for fault in ["member", "role", "scope", "status"] {
        rejected.push(Some(fact_a.corrupted_for_test(fault, &fact_b)));
    }
    for fact in rejected {
        assert!(a
            .deps
            .pc_custody
            .retire(&child_a.birth, PcTempRetirement::Close(fact))
            .is_err());
        assert!(Arc::ptr_eq(
            a.deps.pc_custody.current().unwrap().as_ref().unwrap(),
            &child_a.birth
        ));
        assert_eq!(std::fs::read(&path).unwrap(), config);
        assert!(inflight_temp_core_pids().contains(&pid));
        assert!(!child_a.birth.has_native_terminal());
    }
    let vacant_foreign_custody = Arc::new(PcTempCoreCustody::default());
    assert!(vacant_foreign_custody
        .book_native(
            &child_a
                .birth
                .native
                .as_ref()
                .unwrap()
                .replayed_preparation_for_test(),
            b.dir.join(TEMP_CORE_CONFIG_NAME),
            false,
        )
        .is_err());
    assert!(vacant_foreign_custody.current().unwrap().is_none());
    child_a.close_confirmed().await.unwrap();
    child_b.close_confirmed().await.unwrap();
    cleanup(&a.dir);
    cleanup(&b.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn native_pid_guard_poison_fails_before_config_pid_current_or_terminal_commit() {
    let _registry = registry_guard();
    let h = native_harness("exit-success.sh");
    let mut child = native_child(&h, "exit-success.sh").await;
    child.wait_result().await.unwrap();
    let path = h.dir.join(TEMP_CORE_CONFIG_NAME);
    let config = std::fs::read(&path).unwrap();
    let pid = child.pid().unwrap();
    let birth = child.birth.clone();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lock = birth.pid_guard.lock().unwrap();
        panic!("poison only this owned test birth's PID lock");
    }))
    .is_err());
    assert!(child.close_confirmed().await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), config);
    assert!(inflight_temp_core_pids().contains(&pid));
    assert!(Arc::ptr_eq(
        h.deps.pc_custody.current().unwrap().as_ref().unwrap(),
        &birth
    ));
    assert!(!birth.retired.load(Ordering::SeqCst));
    assert!(!birth.has_native_terminal());
    // Local test teardown after all failure predicates; production has no reset operation.
    birth.pid_guard.clear_poison();
    child.close_confirmed().await.unwrap();
    cleanup(&h.dir);
}

struct UnsupportedPreparedSpawner {
    base: Arc<dyn LoginCoreSpawner>,
    fail: bool,
}

#[async_trait]
impl LoginCoreSpawner for UnsupportedPreparedSpawner {
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        TokioLoginCoreSpawner.prepare_temp_native_birth()
    }
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        if self.fail {
            return Err(SpawnError::Spawn {
                bin: req.binary,
                source: std::io::Error::other("generic dyn Err"),
            });
        }
        self.base.spawn(req).await
    }
}

#[tokio::test]
async fn some_prepared_never_downgrades_mock_close_or_generic_error_to_native_retirement() {
    for fail in [false, true] {
        let mut h = harness(true, false, vec![20001, 20002, 20003]);
        h.deps.spawner = Arc::new(UnsupportedPreparedSpawner {
            base: h.deps.spawner.clone(),
            fail,
        });
        let (outcome, _) = run_round(&h, &three_nodes()).await;
        assert!(
            matches!(outcome, TempCoreOutcome::CleanupUnknown(_)),
            "{outcome:?}"
        );
        let birth = h.deps.pc_custody.current().unwrap().unwrap();
        assert!(birth.native.is_some());
        assert!(!birth.has_native_terminal());
        assert!(birth.config_path.exists());
        assert!(!birth.retired.load(Ordering::SeqCst));
        let bytes = std::fs::read(&birth.config_path).unwrap();
        let (next, _) = run_round(&h, &three_nodes()).await;
        assert!(matches!(next, TempCoreOutcome::CleanupUnknown(_)));
        assert_eq!(std::fs::read(&birth.config_path).unwrap(), bytes);
        assert_eq!(h.spawns.load(Ordering::SeqCst), usize::from(!fail));
        h.deps.pc_custody.birth.lock().unwrap().take();
        cleanup(&h.dir);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn actual_missing_and_non_executable_binary_return_no_child_without_native_terminal() {
    for exists in [false, true] {
        let mut h = native_harness("exit-success.sh");
        let bin = h.dir.join("non-executable-owned-stand-in");
        if exists {
            std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        }
        h.deps.resolve_binary = Arc::new(move || Ok(bin.clone()));
        let births = Arc::new(Mutex::new(Vec::new()));
        h.deps.spawner = Arc::new(RecordingNativeSpawner {
            custody: h.deps.pc_custody.clone(),
            births: births.clone(),
        });
        let (outcome, _) = run_round(&h, &three_nodes()).await;
        assert!(matches!(outcome, TempCoreOutcome::Failed(_)), "{outcome:?}");
        let birth = births.lock().unwrap()[0].clone();
        assert!(birth.native.as_ref().unwrap().factory_entered_for_test());
        assert!(birth.child.lock().await.is_none());
        assert!(!birth.has_native_terminal());
        assert!(birth.retired.load(Ordering::SeqCst));
        assert!(h.deps.pc_custody.current().unwrap().is_none());
        assert!(!h.dir.join(TEMP_CORE_CONFIG_NAME).exists());
        cleanup(&h.dir);
    }
}

#[cfg(unix)]
struct DelayedNativeSpawner {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

#[cfg(unix)]
#[async_trait]
impl LoginCoreSpawner for DelayedNativeSpawner {
    async fn spawn(&self, _: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        panic!("prepared dispatch must use native method");
    }
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        TokioLoginCoreSpawner.prepare_temp_native_birth()
    }
    async fn spawn_with_temp_native_birth(
        &self,
        req: SpawnRequest,
        prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        assert!(prepared.is_some(), "native booking must stay strict");
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        TokioLoginCoreSpawner
            .spawn_with_temp_native_birth(req, prepared)
            .await
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dropped_pre_factory_native_observer_keeps_original_pending_slot_and_fixed_config() {
    let mut h = native_harness("sleep-two-seconds.sh");
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    h.deps.spawner = Arc::new(DelayedNativeSpawner {
        entered: entered.clone(),
        release,
    });
    let h = Arc::new(h);
    let running = tokio::spawn({
        let h = h.clone();
        async move { run_round(&h, &three_nodes()).await }
    });
    acquire(&entered).await;
    let birth = h.deps.pc_custody.current().unwrap().unwrap();
    assert!(!birth.native.as_ref().unwrap().factory_entered_for_test());
    let config = std::fs::read(&birth.config_path).unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(birth.child.lock().await.is_none());
    assert!(h.deps.pc_custody.retry_close().await.is_err());
    let (next, _) = run_round(&h, &three_nodes()).await;
    assert!(matches!(next, TempCoreOutcome::CleanupUnknown(_)));
    assert_eq!(std::fs::read(&birth.config_path).unwrap(), config);
    assert!(!birth.has_native_terminal());
    h.deps.pc_custody.birth.lock().unwrap().take();
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn native_shutdown_during_dispatch_notifies_same_booked_birth_before_publication() {
    let _registry = registry_guard();
    let mut h = native_harness("sleep-two-seconds.sh");
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    h.deps.spawner = Arc::new(DelayedNativeSpawner {
        entered: entered.clone(),
        release: release.clone(),
    });
    let h = Arc::new(h);
    let running = tokio::spawn({
        let h = h.clone();
        async move { run_round(&h, &three_nodes()).await }
    });
    acquire(&entered).await;
    let birth = h.deps.pc_custody.current().unwrap().unwrap();
    let shutdown = tokio::spawn({
        let custody = h.deps.pc_custody.clone();
        async move { shutdown_pc_temp_custody(&custody, Duration::from_secs(3)).await }
    });
    while !h.deps.pc_custody.closing.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    assert!(*birth.close_requested.borrow());
    assert!(!shutdown.is_finished());
    release.add_permits(1);
    let (outcome, _) = running.await.unwrap();
    assert!(matches!(outcome, TempCoreOutcome::Superseded));
    shutdown.await.unwrap().unwrap();
    assert!(h.deps.pc_custody.current().unwrap().is_none());
    assert!(birth.has_native_terminal());
    assert!(birth
        .child
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native_exit()
        .is_some());
    assert!(!birth.config_path.exists());
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn cancelled_native_readiness_and_close_borrow_keep_physical_child_and_pid_custody() {
    let _registry = registry_guard();
    let mut h = native_harness("sleep-two-seconds.sh");
    let probe_entered = Arc::new(Semaphore::new(0));
    h.deps.probe_port = {
        let entered = probe_entered.clone();
        Arc::new(move |_| {
            entered.add_permits(1);
            false
        })
    };
    let h = Arc::new(h);
    let running = tokio::spawn({
        let h = h.clone();
        async move { run_round(&h, &three_nodes()).await }
    });
    acquire(&probe_entered).await;
    let birth = h.deps.pc_custody.current().unwrap().unwrap();
    let pid = birth.pid.get().copied().flatten().unwrap();
    let config = std::fs::read(&birth.config_path).unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    let held = birth.child.lock().await;
    assert!(held.as_ref().unwrap().native_exit().is_none());
    let mut close = Box::pin(h.deps.pc_custody.retry_close());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(close);
    assert!(inflight_temp_core_pids().contains(&pid));
    assert_eq!(std::fs::read(&birth.config_path).unwrap(), config);
    assert!(!birth.has_native_terminal());
    assert!(Arc::ptr_eq(
        h.deps.pc_custody.current().unwrap().as_ref().unwrap(),
        &birth
    ));
    drop(held);
    h.deps.pc_custody.retry_close().await.unwrap();
    assert!(birth.has_native_terminal());
    assert!(h.deps.pc_custody.current().unwrap().is_none());
    assert!(!inflight_temp_core_pids().contains(&pid));
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn bound_native_echild_never_issues_fact_or_releases_original_booking() {
    let _registry = registry_guard();
    let h = native_harness("exit-success.sh");
    let mut child = native_child(&h, "exit-success.sh").await;
    let pid = child.pid().unwrap();
    tokio::task::spawn_blocking(move || {
        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(child.wait_result().await.is_err());
    assert!(child.close_confirmed().await.is_err());
    assert!(child.wait_result().await.is_err());
    assert!(child
        .birth
        .child
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native_exit()
        .is_none());
    assert!(!child.birth.has_native_terminal());
    assert!(child.birth.config_path.exists());
    assert!(inflight_temp_core_pids().contains(&pid));
    let birth = child.birth.clone();
    drop(child);
    assert!(Arc::ptr_eq(
        h.deps.pc_custody.current().unwrap().as_ref().unwrap(),
        &birth
    ));
    // This owned stand-in was actually waitpid-reaped above; only local test teardown follows.
    h.deps.pc_custody.birth.lock().unwrap().take();
    drop(birth);
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn original_binding_rejects_laundered_current_birth_with_same_numeric_pid() {
    let _registry = registry_guard();
    let h = native_harness("exit-success.sh");
    let mut old = native_child(&h, "exit-success.sh").await;
    let pid = old.pid().unwrap();
    old.wait_result().await.unwrap();
    let fact = native_fact(&old).await;
    let native = old.birth.native.clone();
    let foreign_custody = Arc::new(PcTempCoreCustody::default());
    let foreign_dir = h.dir.join("foreign-custody");
    std::fs::create_dir(&foreign_dir).unwrap();
    let path = foreign_dir.join(TEMP_CORE_CONFIG_NAME);
    std::fs::write(&path, "successor owns this config").unwrap();
    let (close_requested, _) = tokio::sync::watch::channel(false);
    // Malicious consumer input only: current B carries ref A and the same numeric PID.
    let laundered = Arc::new(PcTempCoreBirth {
        child: tokio::sync::Mutex::new(Some(Box::new(FakeChild {
            terminated: h.terminated.clone(),
            pid: Some(pid),
            die_after: None,
        }))),
        pid: OnceLock::from(Some(pid)),
        pid_guard: Mutex::new(PcTempRegistration {
            // A's actual original token still owns the identical numeric projection.
            pid_guard: None,
            native_terminal: None,
        }),
        config_path: path.clone(),
        keep_config: false,
        close_requested,
        retired: AtomicBool::new(false),
        identity: Arc::new(()),
        native,
    });
    *foreign_custody.birth.lock().unwrap() = Some(laundered.clone());
    assert!(foreign_custody
        .retire(&laundered, PcTempRetirement::Close(Some(fact)))
        .is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "successor owns this config"
    );
    assert!(inflight_temp_core_pids().contains(&pid));
    assert!(!laundered.has_native_terminal());
    assert!(!laundered.retired.load(Ordering::SeqCst));
    assert!(Arc::ptr_eq(
        foreign_custody.current().unwrap().as_ref().unwrap(),
        &laundered
    ));
    foreign_custody.birth.lock().unwrap().take();
    drop(laundered);
    assert!(!old.birth.retired.load(Ordering::SeqCst));
    old.close_confirmed().await.unwrap();
    cleanup(&h.dir);
}

#[cfg(unix)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize the existing process-wide PID projection for this owned stand-in"
)]
#[tokio::test]
async fn native_birth_lock_poison_fails_before_original_cleanup() {
    let _registry = registry_guard();
    let h = native_harness("exit-success.sh");
    let mut child = native_child(&h, "exit-success.sh").await;
    child.wait_result().await.unwrap();
    let pid = child.pid().unwrap();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lock = h.deps.pc_custody.birth.lock().unwrap();
        panic!("poison this test's original current lock");
    }))
    .is_err());
    assert!(child.close_confirmed().await.is_err());
    assert!(child.birth.config_path.exists());
    assert!(inflight_temp_core_pids().contains(&pid));
    assert!(!child.birth.has_native_terminal());
    assert!(!child.birth.retired.load(Ordering::SeqCst));
    assert!(h.deps.pc_custody.birth.is_poisoned());
    assert!(Arc::ptr_eq(
        h.deps
            .pc_custody
            .birth
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .unwrap(),
        &child.birth
    ));
    h.deps.pc_custody.birth.clear_poison();
    child.close_confirmed().await.unwrap();
    cleanup(&h.dir);
}

#[cfg(unix)]
struct PanicDrainNativeSpawner;

#[cfg(unix)]
#[async_trait]
impl LoginCoreSpawner for PanicDrainNativeSpawner {
    async fn spawn(&self, _: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        panic!("native path only");
    }
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        TokioLoginCoreSpawner.prepare_temp_native_birth()
    }
    async fn spawn_with_temp_native_birth(
        &self,
        mut req: SpawnRequest,
        prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        assert!(prepared.is_some(), "native booking must stay strict");
        req.stdio = StdioPolicy::drain(|_, _| panic!("owned Drain callback after native birth"));
        TokioLoginCoreSpawner
            .spawn_with_temp_native_birth(req, prepared)
            .await
    }
}

#[cfg(unix)]
fn isolated_native_worker(test: &str, marker: &str) -> bool {
    if std::env::var_os(marker).is_some() {
        return false;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(marker, "1")
        .env("POLARIS_NO_KERNEL_RUN", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "isolated worker failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    true
}

#[cfg(unix)]
#[tokio::test]
async fn native_drain_panic_preserves_entered_booking_in_isolated_worker() {
    if isolated_native_worker("runtime::speedtest::tests::pc_custody::native_drain_panic_preserves_entered_booking_in_isolated_worker", "POLARIS_PC_TEMP_DRAIN_WORKER") { return; }
    let mut h = native_harness("exit-success.sh");
    h.deps.spawner = Arc::new(PanicDrainNativeSpawner);
    let h = Arc::new(h);
    let running = tokio::spawn({
        let h = h.clone();
        async move { run_round(&h, &three_nodes()).await }
    });
    assert!(running.await.unwrap_err().is_panic());
    let birth = h.deps.pc_custody.current().unwrap().unwrap();
    assert!(birth.native.as_ref().unwrap().factory_entered_for_test());
    assert!(birth.child.lock().await.is_none());
    let config = std::fs::read(&birth.config_path).unwrap();
    assert!(h.deps.pc_custody.retry_close().await.is_err());
    assert!(h
        .deps
        .pc_custody
        .retire(&birth, PcTempRetirement::NoChild)
        .is_err());
    let (outcome, _) = run_round(&h, &three_nodes()).await;
    assert!(matches!(outcome, TempCoreOutcome::CleanupUnknown(_)));
    assert_eq!(std::fs::read(&birth.config_path).unwrap(), config);
    assert!(!birth.has_native_terminal());
    assert!(!birth.retired.load(Ordering::SeqCst));
    cleanup(&h.dir);
}

#[cfg(unix)]
#[tokio::test]
async fn central_admission_rejection_is_pre_factory_no_child_in_isolated_worker() {
    if isolated_native_worker("runtime::speedtest::tests::pc_custody::central_admission_rejection_is_pre_factory_no_child_in_isolated_worker", "POLARIS_PC_TEMP_ADMISSION_WORKER") { return; }
    polaris_core_supervisor::begin_check_shutdown().unwrap();
    let mut h = native_harness("exit-success.sh");
    let births = Arc::new(Mutex::new(Vec::new()));
    h.deps.spawner = Arc::new(RecordingNativeSpawner {
        custody: h.deps.pc_custody.clone(),
        births: births.clone(),
    });
    let (outcome, _) = run_round(&h, &three_nodes()).await;
    assert!(
        matches!(outcome, TempCoreOutcome::CleanupUnknown(_)),
        "{outcome:?}"
    );
    let birth = births.lock().unwrap()[0].clone();
    assert!(!birth.native.as_ref().unwrap().factory_entered_for_test());
    assert!(birth.child.lock().await.is_none());
    assert!(!birth.has_native_terminal());
    assert!(birth.retired.load(Ordering::SeqCst));
    assert!(h.deps.pc_custody.current().unwrap().is_none());
    assert!(!birth.config_path.exists());
    cleanup(&h.dir);
}

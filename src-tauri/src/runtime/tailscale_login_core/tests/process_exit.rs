//! Adversarial cleanup receipts and cancelled supervisors. No kernel or network is started.
use super::*;
use tokio::sync::Semaphore;

struct UnprovenChild(Arc<AtomicUsize>);

#[async_trait]
impl LoginCoreChild for UnprovenChild {
    fn pid(&self) -> Option<u32> {
        None
    }
    async fn wait(&mut self) {}
    async fn terminate(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn legacy_void_methods_and_missing_pid_cannot_attest_cleanup() {
    let terminations = Arc::new(AtomicUsize::new(0));
    let mut child = UnprovenChild(terminations.clone());
    assert!(child.wait_result().await.is_err());
    assert!(child.after_exit().await.is_err());
    assert!(child.close_confirmed().await.is_err());
    assert_eq!(terminations.load(Ordering::SeqCst), 1);
    assert!(child.close_confirmed().await.is_err());
    assert_eq!(terminations.load(Ordering::SeqCst), 2);
}

struct ScriptedChild {
    wait_rx: mpsc::UnboundedReceiver<Result<(), String>>,
    close_rx: mpsc::UnboundedReceiver<Result<(), String>>,
    wait_entered: Arc<Semaphore>,
    close_entered: Arc<Semaphore>,
    reaped: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl LoginCoreChild for ScriptedChild {
    fn pid(&self) -> Option<u32> {
        None
    }
    async fn wait(&mut self) {
        let _ = self.wait_result().await;
    }
    async fn wait_result(&mut self) -> Result<(), String> {
        self.wait_entered.add_permits(1);
        let result = self
            .wait_rx
            .recv()
            .await
            .expect("script retains its sender");
        assert!(result
            .as_ref()
            .err()
            .is_none_or(|error| error != "panic supervisor"));
        if result.is_ok() {
            self.reaped.store(true, Ordering::SeqCst);
        }
        result
    }
    async fn terminate(&mut self) {
        let _ = self.close_confirmed().await;
    }
    async fn after_exit(&mut self) -> Result<(), String> {
        self.reaped
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or_else(|| "script has no exit receipt".into())
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        self.close_entered.add_permits(1);
        let result = self
            .close_rx
            .recv()
            .await
            .expect("script retains its sender");
        if result.is_ok() {
            self.reaped.store(true, Ordering::SeqCst);
        }
        result
    }
}

impl Drop for ScriptedChild {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct ScriptedSpawner(Mutex<Option<Box<dyn LoginCoreChild>>>);

#[async_trait]
impl LoginCoreSpawner for ScriptedSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        if let StdioPolicy::Drain(sink) = req.stdio {
            sink(Box::new(tokio::io::empty()), Box::new(tokio::io::empty()));
        }
        Ok(self.0.lock().unwrap().take().expect("one owned child"))
    }
}

struct Script {
    registry: Arc<LoginCoreRegistry>,
    wait_tx: mpsc::UnboundedSender<Result<(), String>>,
    close_tx: mpsc::UnboundedSender<Result<(), String>>,
    close_entered: Arc<Semaphore>,
    reaped: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
    dir: PathBuf,
}

impl Script {
    async fn start() -> Self {
        let (wait_tx, wait_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        let wait_entered = Arc::new(Semaphore::new(0));
        let close_entered = Arc::new(Semaphore::new(0));
        let reaped = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let spawner = ScriptedSpawner(Mutex::new(Some(Box::new(ScriptedChild {
            wait_rx,
            close_rx,
            wait_entered: wait_entered.clone(),
            close_entered: close_entered.clone(),
            reaped: reaped.clone(),
            dropped: dropped.clone(),
        }))));
        let registry = Arc::new(LoginCoreRegistry::with_deps(
            Arc::new(spawner),
            Arc::new(FakeChecker { ok: true }),
            fake_subscriber(false),
            Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
            Duration::from_secs(60),
        ));
        let dir = temp_ud();
        started(&registry, &dir, &ts_server("ts1", "myts")).await;
        wait_entered.acquire().await.unwrap().forget();
        Self {
            registry,
            wait_tx,
            close_tx,
            close_entered,
            reaped,
            dropped,
            dir,
        }
    }

    async fn close_entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.close_entered.acquire())
            .await
            .expect("close is requested")
            .unwrap()
            .forget();
    }

    fn assert_retained(&self) {
        assert!(self.registry.shared.contains("ts1"));
        assert!(!self.reaped.load(Ordering::SeqCst));
        assert!(!self.dropped.load(Ordering::SeqCst));
        assert!(sole_login_config(&self.dir).exists());
        assert!(
            self.registry.inflight_login_pids().is_empty(),
            "missing PID is not vacancy"
        );
    }

    async fn finish(&self) {
        let closing = tokio::spawn({
            let registry = self.registry.clone();
            async move { registry.cancel_login("ts1").await }
        });
        self.close_entered().await;
        self.close_tx.send(Ok(())).unwrap();
        assert!(closing.await.unwrap().unwrap());
        wait_until(|| self.dropped.load(Ordering::SeqCst)).await;
        assert!(self.reaped.load(Ordering::SeqCst));
        assert!(!self.registry.shared.contains("ts1"));
        assert!(login_configs(&self.dir).is_empty());
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[tokio::test]
async fn wait_error_and_failed_close_retain_child_claim_and_config_until_retry() {
    let script = Script::start().await;
    script
        .wait_tx
        .send(Err("native wait failed".into()))
        .unwrap();
    script.close_entered().await;
    script.assert_retained();
    script
        .close_tx
        .send(Err("native close wait failed".into()))
        .unwrap();
    wait_until(|| {
        matches!(
            &*script.registry.shared.guard()["ts1"].closed_rx.borrow(),
            Some(Err(error)) if error == "native close wait failed"
        )
    })
    .await;
    script.assert_retained();
    let gate = script.registry.state_gate().await;
    assert_eq!(
        script.registry.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown
    );
    drop(gate);

    let deleted = Arc::new(AtomicBool::new(false));
    let logout = tokio::spawn({
        let registry = script.registry.clone();
        let deleted = deleted.clone();
        async move {
            registry
                .logout("ts1", &|| false, None, |_| {
                    deleted.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
        }
    });
    script.close_entered().await;
    script
        .close_tx
        .send(Err("still not reaped".into()))
        .unwrap();
    assert!(logout.await.unwrap().is_err());
    assert!(!deleted.load(Ordering::SeqCst));
    script.assert_retained();
    script.finish().await;
}

#[tokio::test]
async fn cancelling_close_waiter_does_not_drop_owned_child_or_release_claim() {
    let script = Script::start().await;
    let closing = tokio::spawn({
        let registry = script.registry.clone();
        async move { registry.cancel_login("ts1").await }
    });
    script.close_entered().await;
    closing.abort();
    assert!(closing.await.unwrap_err().is_cancelled());
    script.assert_retained();
    script
        .close_tx
        .send(Err("close remains uncertain".into()))
        .unwrap();
    wait_until(|| {
        matches!(
            &*script.registry.shared.guard()["ts1"].closed_rx.borrow(),
            Some(Err(_))
        )
    })
    .await;
    script.finish().await;
}

#[tokio::test]
async fn panicked_supervisor_retains_physical_child_and_reports_unknown_owner() {
    let script = Script::start().await;
    script.wait_tx.send(Err("panic supervisor".into())).unwrap();
    wait_until(|| script.registry.shared.guard()["ts1"].cancel_tx.is_closed()).await;
    script.assert_retained();
    let gate = script.registry.state_gate().await;
    assert_eq!(
        script.registry.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown
    );
    drop(gate);
    assert!(script.registry.cancel_login("ts1").await.is_err());
    assert!(script.registry.shutdown_for_exit().await.is_err());
    // Explicit fixture cleanup of the retained physical owner. Supervisor loss performed
    // neither registry nor configuration retirement and supplied no success receipt.
    let entry = script.registry.shared.take("ts1").unwrap();
    let custody = entry._child.as_ref().unwrap().clone();
    script.close_tx.send(Ok(())).unwrap();
    custody.lock().await.close_confirmed().await.unwrap();
    drop(custody);
    drop(entry);
    assert!(script.dropped.load(Ordering::SeqCst));
    std::fs::remove_dir_all(&script.dir).unwrap();
}

#[tokio::test]
async fn exit_drain_failure_and_cancel_retain_custody_and_permanently_fence_login() {
    let script = Script::start().await;
    script.registry.begin_shutdown();
    script.close_entered().await;
    script.assert_retained();
    assert!(script.registry.prepare("ts2", "after-exit").await.is_err());
    let drain = tokio::spawn({
        let registry = script.registry.clone();
        async move { registry.shutdown_for_exit().await }
    });
    script
        .close_tx
        .send(Err("native close failed".into()))
        .unwrap();
    assert!(drain.await.unwrap().is_err());
    script.assert_retained();
    let drain = tokio::spawn({
        let registry = script.registry.clone();
        async move { registry.shutdown_for_exit().await }
    });
    script.close_entered().await;
    drain.abort();
    assert!(drain.await.unwrap_err().is_cancelled());
    script.assert_retained();
    script.close_tx.send(Ok(())).unwrap();
    script.registry.shutdown_for_exit().await.unwrap();
    assert!(!script.registry.shared.contains("ts1"));
    assert!(login_configs(&script.dir).is_empty());
    assert!(script
        .registry
        .prepare("ts2", "still-closed")
        .await
        .is_err());
    std::fs::remove_dir_all(&script.dir).unwrap();
}

struct DelayedLoginSpawner {
    inner: ScriptedSpawner,
    entered: Arc<Semaphore>,
    resume: Arc<Semaphore>,
}

#[async_trait]
impl LoginCoreSpawner for DelayedLoginSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        self.entered.add_permits(1);
        self.resume.acquire().await.unwrap().forget();
        self.inner.spawn(req).await
    }
}

#[tokio::test]
async fn shutdown_in_spawn_window_cancels_exact_published_birth_before_ready() {
    let (_wait_tx, wait_rx) = mpsc::unbounded_channel();
    let (close_tx, close_rx) = mpsc::unbounded_channel();
    let entered = Arc::new(Semaphore::new(0));
    let resume = Arc::new(Semaphore::new(0));
    let close_entered = Arc::new(Semaphore::new(0));
    let registry = Arc::new(LoginCoreRegistry::with_deps(
        Arc::new(DelayedLoginSpawner {
            inner: ScriptedSpawner(Mutex::new(Some(Box::new(ScriptedChild {
                wait_rx,
                close_rx,
                wait_entered: Arc::new(Semaphore::new(0)),
                close_entered: close_entered.clone(),
                reaped: Arc::new(AtomicBool::new(false)),
                dropped: Arc::new(AtomicBool::new(false)),
            })))),
            entered: entered.clone(),
            resume: resume.clone(),
        }),
        Arc::new(FakeChecker { ok: true }),
        fake_subscriber(false),
        Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
        Duration::from_secs(60),
    ));
    let dir = temp_ud();
    registry.prepare("ts1", "exit-spawn").await.unwrap();
    let launch = tokio::spawn({
        let registry = registry.clone();
        let dir = dir.clone();
        async move {
            registry
                .start_attempt(
                    &ts_server("ts1", "myts"),
                    &dir,
                    LoginRequest {
                        attempt_id: "exit-spawn".into(),
                        mode: LoginMode::Browser,
                    },
                    &MainLoginSnapshot::default,
                    Arc::new(FakeEmitter::default()),
                )
                .await
        }
    });
    entered.acquire().await.unwrap().forget();
    registry.begin_shutdown();
    let drain = tokio::spawn({
        let registry = registry.clone();
        async move { registry.shutdown_for_exit().await }
    });
    assert!(!drain.is_finished());
    resume.add_permits(1);
    close_entered.acquire().await.unwrap().forget();
    assert!(registry.shared.contains("ts1"));
    assert!(
        !drain.is_finished(),
        "close request is not a native receipt"
    );
    close_tx.send(Ok(())).unwrap();
    assert!(matches!(
        launch.await.unwrap(),
        StartLoginOutcome::Cancelled | StartLoginOutcome::Started
    ));
    drain.await.unwrap().unwrap();
    assert!(registry.shared.guard().is_empty());
    assert!(login_configs(&dir).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn pc_child_requires_native_wait_even_when_pid_is_absent_and_repeated_close_is_safe() {
    let child = tokio::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let mut child = TokioLoginCoreChild {
        child: Some(child),
        reaped: false,
        wait_identity_lost: false,
    };
    assert!(child.after_exit().await.is_err());
    // Simulate a prior caller of native Child::wait without this wrapper's receipt.
    child.child.as_mut().unwrap().wait().await.unwrap();
    assert!(child.pid().is_none());
    assert!(!child.reaped);
    assert!(
        child.after_exit().await.is_err(),
        "PID absence cannot mint a receipt"
    );
    child.close_confirmed().await.unwrap();
    assert!(child.reaped);
    child.after_exit().await.unwrap();
    child.close_confirmed().await.unwrap();
    child.wait_result().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_native_close_future_keeps_the_child_and_never_mints_an_exit_receipt() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut process = tokio::process::Command::new("sh")
        .args(["-c", "trap '' TERM; echo ready; exec sleep 60"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .await
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let mut child = TokioLoginCoreChild {
        child: Some(process),
        reaped: false,
        wait_identity_lost: false,
    };
    let mut close = child.close_confirmed();
    assert!(
        std::future::poll_fn(|cx| { std::task::Poll::Ready(close.as_mut().poll(cx).is_pending()) })
            .await,
        "the SIGTERM-ignoring stand-in leaves close pending"
    );
    drop(close);
    assert!(!child.reaped);
    assert!(child.after_exit().await.is_err());
    assert!(child.child.as_mut().unwrap().try_wait().unwrap().is_none());
    child.child.as_mut().unwrap().start_kill().unwrap();
    child.wait_result().await.unwrap();
    child.after_exit().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn native_echild_is_an_error_and_permanently_quarantines_the_numeric_wait_identity() {
    let process = tokio::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = process.id().unwrap();
    let mut child = TokioLoginCoreChild {
        child: Some(process),
        reaped: false,
        wait_identity_lost: false,
    };
    // Existing nix safely reaps this exact stand-in outside Tokio. Its own native wait must
    // now return ECHILD; no mock can make a swallowed OS error turn this regression green.
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
    assert!(!child.reaped);
    assert!(child.wait_identity_lost);
    assert!(child.after_exit().await.is_err());
    assert!(child.close_confirmed().await.is_err());
    assert!(
        child.wait_result().await.is_err(),
        "a later PID reuse cannot repair this birth"
    );
    // Drop quarantines this invalid native wait handle; it may neither signal the old PID
    // nor hand it to Tokio's orphan queue for another waitpid after PID reuse.
    drop(child);
}

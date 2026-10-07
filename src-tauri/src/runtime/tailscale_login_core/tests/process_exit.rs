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
    assert!(
        child.native_exit().is_none(),
        "unbound login cannot issue a temp native fact"
    );
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

struct NoWarmSubscription;

#[async_trait]
impl LoginStatusSubscriber for NoWarmSubscription {
    async fn subscribe(
        &self,
        _port: u16,
        _secret: &str,
    ) -> Result<Box<dyn LoginStatusStream>, String> {
        panic!("a warm observation must not subscribe or authorize a login")
    }
}

#[tokio::test]
async fn warm_uses_original_close_supervisor_without_status_or_completing_account_action() {
    for cancelled in [false, true] {
        let registry = LoginCoreRegistry::with_deps(
            Arc::new(ScriptedSpawner(Mutex::new(None))),
            Arc::new(FakeChecker { ok: true }),
            Arc::new(NoWarmSubscription),
            Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
            Duration::from_secs(60),
        );
        let attempt = registry.attempts.prepare("ts1", "warm-action").unwrap();
        let dir = temp_ud();
        let config_path = dir.join("tailscale-login-ts1-71.json");
        std::fs::write(&config_path, "{}").unwrap();
        let (wait_tx, wait_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        let close_entered = Arc::new(Semaphore::new(0));
        let reaped = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let child: LoginChildCustody = Arc::new(tokio::sync::Mutex::new(Box::new(ScriptedChild {
            wait_rx,
            close_rx,
            wait_entered: Arc::new(Semaphore::new(0)),
            close_entered: close_entered.clone(),
            reaped: reaped.clone(),
            dropped: dropped.clone(),
        })));
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (closed_tx, mut closed_rx) = watch::channel(None);
        registry.shared.insert(
            "ts1".into(),
            LoginEntry {
                epoch: 71,
                attempt_id: "warm-action".into(),
                pid: None,
                cancel_tx: cancel_tx.clone(),
                closed_rx: closed_rx.clone(),
                _child: Some(child.clone()),
                native: None,
                drain: None,
                config_path: None,
                #[cfg(target_os = "android")]
                android_instance: None,
                #[cfg(target_os = "android")]
                android_authority: None,
            },
        );
        attempt.process_owned.store(true, Ordering::SeqCst);
        let emitter = Arc::new(FakeEmitter::default());
        let (ready_tx, ready_rx) = oneshot::channel();
        let task = tokio::spawn(subscribe_and_supervise(
            SuperviseCtx {
                shared: registry.shared.clone(),
                attempt: attempt.clone(),
                attempt_id: "warm-action".into(),
                server_id: "ts1".into(),
                node_name: "warm".into(),
                tag_to_id: BTreeMap::from([(TAILSCALE_LOGIN_ENDPOINT_TAG.into(), "ts1".into())]),
                config_path: config_path.clone(),
                epoch: 71,
                deadline: tokio::time::Instant::now() + Duration::from_secs(60),
                emitter: emitter.clone(),
                closed_tx,
                warm: true,
                #[cfg(target_os = "android")]
                android_instance: None,
            },
            child.clone(),
            Arc::new(NoWarmSubscription),
            TailscaleLoginApiService {
                port: 1,
                secret: "unused".into(),
            },
            cancel_rx,
            ready_tx,
        ));
        tokio::time::timeout(Duration::from_secs(2), close_entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        close_tx.send(Err("fixture close failed".into())).unwrap();
        closed_rx.changed().await.unwrap();
        assert_eq!(
            *closed_rx.borrow(),
            Some(Err("fixture close failed".into()))
        );
        assert!(registry.shared.contains("ts1"));
        assert!(config_path.exists());
        assert!(!reaped.load(Ordering::SeqCst));
        assert!(!attempt.is_finished());
        assert!(attempt.process_owned.load(Ordering::SeqCst));
        assert!(emitter.progress.lock().unwrap().is_empty());
        assert!(emitter.captured.lock().unwrap().is_empty());
        if cancelled {
            attempt.cancel();
        }
        // The original cancellation channel retries the original Child; no replacement birth.
        cancel_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), close_entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        close_tx.send(Ok(())).unwrap();
        assert!(matches!(
            ready_rx.await.unwrap(),
            StartLoginOutcome::Started
        ));
        task.await.unwrap();
        assert!(reaped.load(Ordering::SeqCst));
        assert!(!registry.shared.contains("ts1"));
        assert!(!config_path.exists());
        assert_eq!(
            attempt.is_finished(),
            cancelled,
            "only a cancelled request completes; warm is not authorization"
        );
        assert!(!attempt.process_owned.load(Ordering::SeqCst));
        assert!(emitter.progress.lock().unwrap().is_empty());
        assert!(emitter.captured.lock().unwrap().is_empty());
        assert_eq!(attempt.while_active(|| ()).is_some(), !cancelled);
        drop(wait_tx);
        drop(child);
        assert!(dropped.load(Ordering::SeqCst));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

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
                        replace_identity: false,
                        reuse_retained_auth_key: false,
                        expected_credential_revision: None,
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
        native_attachment: None,
        native_exit: None,
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
    assert!(child.native_exit().is_none());
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
        native_attachment: None,
        native_exit: None,
    };
    let mut close = child.close_confirmed();
    assert!(
        std::future::poll_fn(|cx| { std::task::Poll::Ready(close.as_mut().poll(cx).is_pending()) })
            .await,
        "the SIGTERM-ignoring stand-in leaves close pending"
    );
    drop(close);
    assert!(!child.reaped);
    assert!(child.native_exit().is_none());
    assert!(child.after_exit().await.is_err());
    assert!(child.child.as_mut().unwrap().try_wait().unwrap().is_none());
    child.child.as_mut().unwrap().start_kill().unwrap();
    child.wait_result().await.unwrap();
    child.after_exit().await.unwrap();
    assert!(child.native_exit().is_none());
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
        native_attachment: None,
        native_exit: None,
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
    assert!(child.native_exit().is_none());
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

#[cfg(unix)]
#[tokio::test]
async fn ordinary_tokio_factory_drains_both_streams_without_a_temp_native_fact() {
    use tokio::io::AsyncReadExt;

    let binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("crates/core-supervisor/src/config_gate/check_custody/tests/fixtures/exit-seven.sh");
    let (drained, drainage) = tokio::sync::oneshot::channel();
    let req = SpawnRequest::new(
        binary,
        PathBuf::from("owned-stand-in-unused-config.json"),
        StdioPolicy::drain(move |mut stdout, mut stderr| {
            tokio::spawn(async move {
                let mut out = Vec::new();
                let mut err = Vec::new();
                let result =
                    tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err),);
                drained.send(result).unwrap();
            });
        }),
    );
    let mut child = TokioLoginCoreSpawner.spawn(req).await.unwrap();
    assert!(child.native_exit().is_none());
    child.wait_result().await.unwrap();
    child.after_exit().await.unwrap();
    child.close_confirmed().await.unwrap();
    assert!(child.native_exit().is_none());
    let (stdout, stderr) = tokio::time::timeout(LOGIN_REAP_TIMEOUT, drainage)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stdout.unwrap(), 0);
    assert_eq!(stderr.unwrap(), 0);
}

#[test]
fn lazy_tailscale_claim_directory_matches_constructor_without_creating_state() {
    let root = crate::test_support::TestDir::new("polaris-lazy-ts-claim-");
    let directory = root.join("config/tailscale/srv-1");
    let actual = canonical_tailscale_claim_directory(&directory).unwrap();
    assert_eq!(
        actual,
        root.canonicalize().unwrap().join("config/tailscale/srv-1")
    );
    assert!(!directory.exists());
    std::fs::create_dir_all(root.join("config")).unwrap();
    assert_eq!(
        canonical_tailscale_claim_directory(&directory).unwrap(),
        actual
    );
    assert!(canonical_tailscale_claim_directory(&root.join("config/not-tailscale/srv-1")).is_err());
    assert!(
        canonical_tailscale_claim_directory(&root.join("config/tailscale/../outside")).is_err()
    );
}

#[cfg(unix)]
#[test]
fn lazy_tailscale_claim_rejects_symlink_root_escape() {
    let root = crate::test_support::TestDir::new("polaris-lazy-ts-escape-");
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    std::os::unix::fs::symlink(root.join("outside"), root.join("config/tailscale")).unwrap();
    assert!(canonical_tailscale_claim_directory(&root.join("config/tailscale/srv-1")).is_err());
}

/// Probes the real production factory, not an injected native acceptance issuer.
struct NativeBookingSpawner {
    shared: Mutex<Option<Arc<Shared>>>,
    births: Mutex<Vec<LoginNativeBirthRef>>,
    configs: Mutex<Vec<PathBuf>>,
    generic_error: bool,
    panic_drain: bool,
}
#[async_trait]
impl LoginCoreSpawner for NativeBookingSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        TokioLoginCoreSpawner.spawn(req).await
    }
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        TokioLoginCoreSpawner.prepare_temp_native_birth()
    }
    async fn spawn_with_temp_native_birth(
        &self,
        mut req: SpawnRequest,
        prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        assert!(prepared.is_some(), "production Login is Some-bound");
        {
            let shared = self.shared.lock().unwrap().as_ref().unwrap().clone();
            let entries = shared.guard();
            let entry = entries.get("ts1").expect("book before factory");
            assert!(entry._child.is_none());
            assert!(entry.pid.is_none());
            assert!(entry.config_path.as_ref().unwrap().exists());
            assert!(entry.drain.is_some());
            self.configs
                .lock()
                .unwrap()
                .push(entry.config_path.clone().unwrap());
            self.births
                .lock()
                .unwrap()
                .push(entry.native.clone().unwrap());
        }
        if self.generic_error {
            return Err(SpawnError::Spawn {
                bin: req.binary,
                source: std::io::Error::other("generic dyn error is not factory evidence"),
            });
        }
        if self.panic_drain {
            req.stdio = StdioPolicy::drain(|_, _| panic!("owned factory drain panic"));
        }
        TokioLoginCoreSpawner
            .spawn_with_temp_native_birth(req, prepared)
            .await
    }
}
fn native_booking_registry(
    binary: PathBuf,
    generic_error: bool,
    panic_drain: bool,
) -> (
    Arc<LoginCoreRegistry>,
    Arc<NativeBookingSpawner>,
    Arc<FakeStatusSubscriber>,
) {
    let spawner = Arc::new(NativeBookingSpawner {
        shared: Mutex::new(None),
        births: Mutex::new(Vec::new()),
        configs: Mutex::new(Vec::new()),
        generic_error,
        panic_drain,
    });
    let subscriber = fake_subscriber(false);
    let registry = Arc::new(LoginCoreRegistry::with_deps(
        spawner.clone(),
        Arc::new(FakeChecker { ok: true }),
        subscriber.clone(),
        Arc::new(move || Ok(binary.clone())),
        Duration::from_secs(60),
    ));
    *spawner.shared.lock().unwrap() = Some(registry.shared.clone());
    (registry, spawner, subscriber)
}

#[cfg(unix)]
#[tokio::test]
async fn real_login_registry_books_some_before_factory_and_consumes_native_task_config_terminal() {
    let dir = crate::test_support::TestDir::new("polaris-login-native-terminal-");
    let binary = crate::test_support::write_sleeping_probe(dir.path(), &dir.join("witness"));
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 60\n").unwrap();
    let (registry, spawner, subscriber) = native_booking_registry(binary, false, false);
    let emitter = started(&registry, dir.path(), &ts_server("ts1", "myts")).await;
    let birth = spawner.births.lock().unwrap()[0].clone();
    let child = registry.shared.guard()["ts1"]._child.clone().unwrap();
    assert!(registry.login_native_terminal(&birth).is_err());
    assert!(birth
        .validate_exit(&registry.identity, birth.epoch().unwrap(), None)
        .is_err());
    assert_eq!(spawner.births.lock().unwrap().len(), 1);
    subscriber.push(0, frame(TAILSCALE_LOGIN_ENDPOINT_TAG, "Running", ""));
    wait_until(|| !registry.shared.contains("ts1")).await;
    registry.login_native_terminal(&birth).unwrap();
    assert!(!spawner.configs.lock().unwrap()[0].exists());
    let fact = child
        .lock()
        .await
        .native_exit()
        .expect("same original Child waited");
    assert!(fact.matches_cell());
    assert!(birth
        .terminal(&Arc::new(RegistryIdentity), birth.epoch().unwrap())
        .is_err());
    assert!(birth
        .terminal(&registry.identity, birth.epoch().unwrap() + 1)
        .is_err());
    assert_eq!(
        emitter.progress.lock().unwrap().last().unwrap().2,
        "authorized"
    );
}

#[tokio::test]
async fn generic_dyn_spawn_error_preserves_original_pending_booking_and_config() {
    let dir = crate::test_support::TestDir::new("polaris-login-native-dyn-error-");
    let (registry, spawner, _) = native_booking_registry(dir.join("unused-binary"), true, false);
    let result = registry
        .start_login(
            &ts_server("ts1", "myts"),
            dir.path(),
            false,
            None,
            0,
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(matches!(result, StartLoginOutcome::Failed(reason) if reason == "processStartFailed"));
    let birth = spawner.births.lock().unwrap()[0].clone();
    wait_until(|| registry.shared.guard()["ts1"].closed_rx.borrow().is_some()).await;
    assert!(registry.login_native_terminal(&birth).is_err());
    assert!(registry.cancel_login("ts1").await.is_err());
    assert!(registry.shared.contains("ts1"));
    assert!(sole_login_config(dir.path()).exists());
    assert!(birth
        .validate_no_child(&registry.identity, birth.epoch().unwrap())
        .is_err());
}

#[tokio::test]
async fn sole_factory_spawn_error_proves_no_child_only_after_original_config_cleanup() {
    let dir = crate::test_support::TestDir::new("polaris-login-native-no-child-");
    let (registry, spawner, _) =
        native_booking_registry(dir.join("nonexistent-owned-binary"), false, false);
    let result = registry
        .start_login(
            &ts_server("ts1", "myts"),
            dir.path(),
            false,
            None,
            0,
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(matches!(result, StartLoginOutcome::Failed(reason) if reason == "processStartFailed"));
    let birth = spawner.births.lock().unwrap()[0].clone();
    wait_until(|| !registry.shared.contains("ts1")).await;
    registry.login_native_terminal(&birth).unwrap();
    assert!(!spawner.configs.lock().unwrap()[0].exists());
}

#[cfg(unix)]
#[tokio::test]
async fn real_factory_drain_panic_and_lost_launcher_do_not_release_pending_login() {
    // A real factory panic permanently poisons the process-wide admission gate. Keep that
    // strong assertion in an exact no-network child test, without poisoning unrelated tests.
    const CHILD: &str = "POLARIS_L1_FACTORY_PANIC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::tailscale_login_core::tests::process_exit::real_factory_drain_panic_and_lost_launcher_do_not_release_pending_login",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "owned panic child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let dir = crate::test_support::TestDir::new("polaris-login-native-panic-");
    let binary = crate::test_support::write_sleeping_probe(dir.path(), &dir.join("witness"));
    let (registry, spawner, _) = native_booking_registry(binary, false, true);
    let original = registry.clone();
    let path = dir.path().to_owned();
    let task = tokio::spawn(async move {
        original
            .start_login(
                &ts_server("ts1", "myts"),
                &path,
                false,
                None,
                0,
                Arc::new(FakeEmitter::default()),
            )
            .await
    });
    assert!(matches!(task.await, Err(error) if error.is_panic()));
    let birth = spawner.births.lock().unwrap()[0].clone();
    assert!(registry.login_native_terminal(&birth).is_err());
    assert!(registry.cancel_login("ts1").await.is_err());
    assert!(registry.shared.contains("ts1"));
    assert!(sole_login_config(dir.path()).exists());
    assert!(birth
        .validate_no_child(&registry.identity, birth.epoch().unwrap())
        .is_err());
}

#[cfg(unix)]
async fn native_login_fixture(
    fatal: Option<crate::runtime::proxy::core_log::CoreFatalSlot>,
) -> (
    crate::test_support::TestDir,
    Arc<Shared>,
    LoginNativeBirthRef,
    LoginChildCustody,
    Arc<LoginStdioDrain>,
    PathBuf,
) {
    let dir = crate::test_support::TestDir::new("polaris-login-native-tail-");
    let binary = crate::test_support::write_sleeping_probe(dir.path(), &dir.join("witness"));
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf 'FATAL[0000] missing monitor for auto DHCP' >&2\nexit 0\n",
    )
    .unwrap();
    let config = dir.join("original-config.json");
    std::fs::write(&config, b"{}").unwrap();
    let shared = Arc::new(Shared::default());
    let prepared = TokioLoginCoreSpawner.prepare_temp_native_birth().unwrap();
    let native = prepared.bind_login(&shared.identity, 23).unwrap();
    let drain = LoginStdioDrain::new();
    let original_drain = drain.clone();
    let req = SpawnRequest::new(
        binary,
        &config,
        StdioPolicy::drain(move |stdout, stderr| {
            let out = pipe_to_log_with_secrets_owned(
                LoginDrainReader::new(stdout, original_drain.clone(), 0),
                LOGIN_CORE_LOG_TARGET,
                None,
                None,
                Vec::new(),
            );
            let err = pipe_to_log_with_secrets_owned(
                LoginDrainReader::new(stderr, original_drain.clone(), 1),
                LOGIN_CORE_LOG_TARGET,
                fatal,
                None,
                Vec::new(),
            );
            original_drain.install([out, err]);
        }),
    );
    let (cancel_tx, _) = mpsc::unbounded_channel();
    let (_, closed_rx) = watch::channel(None);
    shared.insert(
        "ts1".into(),
        LoginEntry {
            epoch: 23,
            attempt_id: "actual-owned-native-tail".into(),
            pid: None,
            cancel_tx,
            closed_rx,
            _child: None,
            native: Some(native.clone()),
            drain: Some(drain.clone()),
            config_path: Some(config.clone()),
            #[cfg(target_os = "android")]
            android_instance: None,
            #[cfg(target_os = "android")]
            android_authority: None,
        },
    );
    let child = Arc::new(tokio::sync::Mutex::new(
        TokioLoginCoreSpawner
            .spawn_with_temp_native_birth(req, Some(prepared))
            .await
            .unwrap(),
    ));
    native.publish_child(child.clone());
    shared.guard().get_mut("ts1").unwrap()._child = Some(child.clone());
    child.lock().await.wait_result().await.unwrap();
    (dir, shared, native, child, drain, config)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_line_eof_cannot_retire_while_original_drainer_task_tail_is_blocked() {
    let fatal = Arc::new(Mutex::new(None));
    let blocking = fatal.clone();
    let (release, unblock) = std::sync::mpsc::channel();
    let (locked, ready) = oneshot::channel();
    let owner = std::thread::spawn(move || {
        let _lock = blocking.lock().unwrap();
        locked.send(()).unwrap();
        let _ = unblock.recv();
    });
    ready.await.unwrap();
    let (_dir, shared, native, child, drain, config) =
        native_login_fixture(Some(fatal.clone())).await;
    wait_until(|| {
        drain
            .streams
            .borrow()
            .iter()
            .all(|stream| matches!(stream, Some(Ok(()))))
    })
    .await;
    let fact = child.lock().await.native_exit();
    assert!(shared
        .retire_login_tail("ts1", 23, &native, fact.clone())
        .is_err());
    assert!(native.terminal(&shared.identity, 23).is_err());
    assert!(shared.contains("ts1"));
    assert!(config.exists());
    // EOF was observed inside read_until; the final no-newline FATAL still owns this task.
    assert!(drain
        .tasks
        .get()
        .unwrap()
        .try_lock()
        .unwrap()
        .iter()
        .any(|task| !task.handle.is_finished()));
    let original_drain = drain.clone();
    let joining = tokio::spawn(async move { original_drain.finished().await });
    wait_until(|| drain.tasks.get().unwrap().try_lock().is_err()).await;
    joining.abort();
    assert!(joining.await.unwrap_err().is_cancelled());
    assert!(native.terminal(&shared.identity, 23).is_err());
    assert!(shared
        .retire_login_tail("ts1", 23, &native, fact.clone())
        .is_err());
    release.send(()).unwrap();
    owner.join().unwrap();
    drain.finished().await.unwrap();
    assert_eq!(
        *fatal.lock().unwrap(),
        Some(crate::runtime::proxy::core_log::CoreFatalKind::DhcpMonitorMissing)
    );
    shared.retire_login_tail("ts1", 23, &native, fact).unwrap();
    native.terminal(&shared.identity, 23).unwrap();
    assert!(!config.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn native_wait_and_task_join_still_preserve_custody_on_config_cleanup_failure() {
    let (_dir, shared, native, child, drain, config) = native_login_fixture(None).await;
    drain.finished().await.unwrap();
    let fact = child.lock().await.native_exit().unwrap();
    let saved_config = config.with_extension("original");
    std::fs::rename(&config, &saved_config).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert!(shared
        .retire_login_tail("ts1", 23, &native, Some(fact.clone()))
        .is_err());
    assert!(native.terminal(&shared.identity, 23).is_err());
    assert!(shared.guard()["ts1"]._child.is_some());
    std::fs::remove_dir(&config).unwrap();
    std::fs::rename(&saved_config, &config).unwrap();
    // Retry consumes the same immutable native wait and cached original join results.
    drain.finished().await.unwrap();
    shared
        .retire_login_tail("ts1", 23, &native, Some(fact))
        .unwrap();
    native.terminal(&shared.identity, 23).unwrap();
    assert!(!config.exists());
}

#[cfg(not(target_os = "android"))]
#[cfg(unix)]
#[tokio::test]
async fn login_native_issuer_rejects_foreign_registry_birth_role_scope_status_and_temp_receipt() {
    let (_dir, shared, native, child, drain, _config) = native_login_fixture(None).await;
    drain.finished().await.unwrap();
    let fact = child.lock().await.native_exit().unwrap();
    let binary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("crates/core-supervisor/src/config_gate/check_custody/tests/fixtures/exit-seven.sh");
    let temp_owner = Arc::new(());
    let temp_birth = Arc::new(());
    let prepared = TokioLoginCoreSpawner.prepare_temp_native_birth().unwrap();
    let temp = prepared.bind(&temp_owner, &temp_birth).unwrap();
    let req = SpawnRequest::new(
        binary,
        PathBuf::from("unused-owned-config"),
        StdioPolicy::drain(|stdout, stderr| {
            drop(pipe_to_log_with_secrets_owned(
                stdout,
                LOGIN_CORE_LOG_TARGET,
                None,
                None,
                Vec::new(),
            ));
            drop(pipe_to_log_with_secrets_owned(
                stderr,
                LOGIN_CORE_LOG_TARGET,
                None,
                None,
                Vec::new(),
            ));
        }),
    );
    let mut other = TokioLoginCoreSpawner
        .spawn_with_temp_native_birth(req, Some(prepared))
        .await
        .unwrap();
    other.wait_result().await.unwrap();
    let foreign = other.native_exit().unwrap();
    assert_ne!(fact.status_for_test(), foreign.status_for_test());
    assert!(native
        .validate_exit(&Arc::new(RegistryIdentity), 23, Some(fact.clone()))
        .is_err());
    assert!(native
        .validate_exit(&shared.identity, 24, Some(fact.clone()))
        .is_err());
    assert!(native
        .validate_exit(&shared.identity, 23, Some(foreign.clone()))
        .is_err());
    assert!(temp
        .validate_exit(Some(fact.clone()), &temp_owner, &temp_birth)
        .is_err());
    for fault in ["member", "role", "scope", "status"] {
        assert!(
            native
                .validate_exit(
                    &shared.identity,
                    23,
                    Some(fact.corrupted_for_test(fault, &foreign))
                )
                .is_err(),
            "{fault}"
        );
    }
    let foreign_login = TokioLoginCoreSpawner
        .prepare_temp_native_birth()
        .unwrap()
        .bind_login(&shared.identity, 23)
        .unwrap();
    assert!(foreign_login
        .validate_exit(&shared.identity, 23, Some(fact.clone()))
        .is_err());
    shared
        .retire_login_tail("ts1", 24, &native, Some(fact.clone()))
        .unwrap_err();
    shared
        .retire_login_tail("ts1", 23, &foreign_login, Some(fact.clone()))
        .unwrap_err();
    assert!(shared.contains("ts1"));
    shared
        .retire_login_tail("ts1", 23, &native, Some(fact))
        .unwrap();
}

struct ErrorLoginReader;
impl tokio::io::AsyncRead for ErrorLoginReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("owned reader failure")))
    }
}
#[tokio::test]
async fn login_drain_io_error_early_drop_and_panicked_task_are_never_normal_tail_completion() {
    for reader in [false, true] {
        let drain = LoginStdioDrain::new();
        let out = pipe_to_log_with_secrets_owned(
            LoginDrainReader::new(tokio::io::empty(), drain.clone(), 0),
            LOGIN_CORE_LOG_TARGET,
            None,
            None,
            Vec::new(),
        );
        let err = if reader {
            pipe_to_log_with_secrets_owned(
                LoginDrainReader::new(ErrorLoginReader, drain.clone(), 1),
                LOGIN_CORE_LOG_TARGET,
                None,
                None,
                Vec::new(),
            )
        } else {
            let reader = LoginDrainReader::new(tokio::io::empty(), drain.clone(), 1);
            tokio::spawn(async move {
                drop(reader);
                Ok(())
            })
        };
        drain.install([out, err]);
        assert!(drain.finished().await.is_err());
    }
    let drain = LoginStdioDrain::new();
    drain.complete(0, Ok(()));
    drain.complete(1, Ok(()));
    let out = tokio::spawn(async {
        if std::hint::black_box(true) {
            panic!("task completion is not successful EOF");
        }
        Ok(())
    });
    let err = tokio::spawn(async { Ok(()) });
    drain.install([out, err]);
    assert!(drain.finished().await.is_err());
    assert!(
        drain.finished().await.is_err(),
        "cached JoinError never upgrades on retry"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn equal_presented_pid_and_forged_close_watch_cannot_substitute_a_new_child_birth() {
    let (_dir, shared, native, child, drain, config) = native_login_fixture(None).await;
    let (_other_dir, other_shared, _other_native, other_child, other_drain, _other_config) =
        native_login_fixture(None).await;
    drain.finished().await.unwrap();
    other_drain.finished().await.unwrap();
    let original = child.lock().await.native_exit().unwrap();
    let successor = other_child.lock().await.native_exit().unwrap();
    // Negative presentation input: two real, separately-issued native facts projected onto
    // one numeric PID, as after reuse. Neither a PID nor an ordinary watch is the issuer.
    let (_, forged) = watch::channel(Some(Ok(())));
    {
        let mut entries = shared.guard();
        let entry = entries.get_mut("ts1").unwrap();
        entry.pid = Some(4242);
        entry.closed_rx = forged;
    }
    other_shared.guard().get_mut("ts1").unwrap().pid = Some(4242);
    assert_eq!(shared.pids(), other_shared.pids());
    assert!(matches!(
        &*shared.guard()["ts1"].closed_rx.borrow(),
        Some(Ok(()))
    ));
    assert!(native.terminal(&shared.identity, 23).is_err());
    assert!(shared
        .retire_login_tail("ts1", 23, &native, Some(successor))
        .is_err());
    assert!(shared.contains("ts1"));
    assert!(config.exists());
    shared
        .retire_login_tail("ts1", 23, &native, Some(original))
        .unwrap();
}

use super::*;
use serde_json::Value;
use tokio::sync::Semaphore;

struct SlowChild {
    child: Box<dyn LoginCoreChild>,
    terminating: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl LoginCoreChild for SlowChild {
    fn pid(&self) -> Option<u32> {
        self.child.pid()
    }
    async fn wait(&mut self) {
        self.child.wait().await;
    }
    async fn terminate(&mut self) {
        self.terminating.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.child.terminate().await;
    }
}

struct SlowSpawner {
    base: Arc<FakeSpawner>,
    terminating: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl LoginCoreSpawner for SlowSpawner {
    fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        Ok(Box::new(SlowChild {
            child: self.base.spawn(req)?,
            terminating: self.terminating.clone(),
            release: self.release.clone(),
        }))
    }
}

struct BlockingChecker(Arc<Semaphore>);
#[async_trait]
impl ConfigChecker for BlockingChecker {
    async fn check(&self, _: &Path, _: &Path) -> Result<(), String> {
        self.0.add_permits(1);
        std::future::pending().await
    }
}

struct BlockingSubscriber(Arc<Semaphore>);
#[async_trait]
impl LoginStatusSubscriber for BlockingSubscriber {
    async fn subscribe(&self, _: u16, _: &str) -> Result<Box<dyn LoginStatusStream>, String> {
        self.0.add_permits(1);
        std::future::pending().await
    }
}

fn request(id: &str) -> LoginRequest {
    LoginRequest {
        attempt_id: id.into(),
        mode: LoginMode::Browser,
    }
}
fn offline() -> MainLoginSnapshot {
    MainLoginSnapshot::default()
}
async fn acquire(signal: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(2), signal.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
async fn cancellation_before_prepare_fences_delayed_start() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let server = ts_server("ts1", "myts");
    let ud = temp_ud();
    reg.cancel_attempt("ts1", "early").await.unwrap();
    reg.prepare("ts1", "early").unwrap();
    assert!(matches!(
        reg.start_attempt(
            &server,
            &ud,
            request("early"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Cancelled
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn cancellation_interrupts_pending_check() {
    let entered = Arc::new(Semaphore::new(0));
    let spawner = fake_spawner(vec![], false, false);
    let reg = Arc::new(LoginCoreRegistry::with_deps(
        spawner.clone(),
        Arc::new(BlockingChecker(entered.clone())),
        fake_subscriber(false),
        Arc::new(|| Ok("/fake/core".into())),
        Duration::from_secs(60),
    ));
    let ud = temp_ud();
    reg.prepare("ts1", "check").unwrap();
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("check"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    acquire(&entered).await;
    reg.cancel_attempt("ts1", "check").await.unwrap();
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::Cancelled));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

fn slow_registry(
    subscriber: Arc<dyn LoginStatusSubscriber>,
) -> (Arc<LoginCoreRegistry>, Arc<SlowSpawner>) {
    let spawner = Arc::new(SlowSpawner {
        base: fake_spawner(vec![], false, false),
        terminating: Arc::new(Semaphore::new(0)),
        release: Arc::new(Semaphore::new(0)),
    });
    let reg = Arc::new(LoginCoreRegistry::with_deps(
        spawner.clone(),
        Arc::new(FakeChecker { ok: true }),
        subscriber,
        Arc::new(|| Ok("/fake/core".into())),
        Duration::from_secs(60),
    ));
    (reg, spawner)
}

#[tokio::test]
async fn relogin_waits_for_old_writer_reap_before_spawn() {
    let (reg, spawner) = slow_registry(fake_subscriber(false));
    let ud = temp_ud();
    let server = ts_server("ts1", "myts");
    started(&reg, &ud, &server).await;
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let second = tokio::spawn(async move {
        reg2.start_login(
            &server,
            &ud2,
            false,
            None,
            0,
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    acquire(&spawner.terminating).await;
    assert_eq!(spawner.base.count.load(Ordering::SeqCst), 1);
    assert_eq!(reg.inflight_login_pids(), vec![4242]);
    assert!(!second.is_finished());
    spawner.release.add_permits(1);
    assert!(matches!(second.await.unwrap(), StartLoginOutcome::Started));
    assert_eq!(spawner.base.count.load(Ordering::SeqCst), 2);
    spawner.release.add_permits(1);
    reg.cancel_and_wait("ts1").await;
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn main_reservation_waits_for_reap_and_uses_generated_endpoint_set() {
    let sub = fake_subscriber(false);
    let (reg, spawner) = slow_registry(sub.clone());
    let ud = temp_ud();
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let reservation = tokio::spawn(async move {
        let _gate = reg2.state_gate().await;
        reg2.reserve_main_states(&json!({"endpoints": [{"type": "tailscale", "tag":"myts", "state_directory": ud2.join("tailscale/ts1")}, {"type": "wireguard", "tag": "ts2"}]}), &ud2).await;
    });
    acquire(&spawner.terminating).await;
    assert!(!reservation.is_finished());
    assert!(reg.shared.contains("ts1"));
    spawner.release.add_permits(1);
    reservation.await.unwrap();
    assert!(reg.main_owns("ts1", true));
    assert!(!reg.main_owns("ts2", true));
    assert!(
        !reg.main_owns("ts1", false),
        "crashed/stopped core cannot retain ownership"
    );
    reg.prepare("ts1", "main").unwrap();
    let fresh = async {
        wait_until(|| sub.senders.lock().unwrap().len() == 2).await;
        sub.push(1, frame("myts", "Running", ""));
    };
    let server = ts_server("ts1", "myts");
    let start = reg.start_attempt(
        &server,
        &ud,
        request("main"),
        &|| MainLoginSnapshot {
            alive: true,
            api_port: 9090,
            ..Default::default()
        },
        Arc::new(FakeEmitter::default()),
    );
    let (_, outcome) = tokio::join!(fresh, start);
    assert!(matches!(outcome, StartLoginOutcome::InMainCore));
    assert_eq!(spawner.base.count.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn dropping_ipc_during_subscribe_retains_pid_until_reap() {
    let entered = Arc::new(Semaphore::new(0));
    let (reg, spawner) = slow_registry(Arc::new(BlockingSubscriber(entered.clone())));
    let ud = temp_ud();
    reg.prepare("ts1", "drop").unwrap();
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("drop"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    acquire(&entered).await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    acquire(&spawner.terminating).await;
    assert_eq!(reg.inflight_login_pids(), vec![4242]);
    assert!(!login_configs(&ud).is_empty());
    spawner.release.add_permits(1);
    reg.cancel_attempt("ts1", "drop").await.unwrap();
    assert!(reg.inflight_login_pids().is_empty());
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn authorization_event_is_emitted_only_after_reap() {
    let sub = fake_subscriber(false);
    let (reg, spawner) = slow_registry(sub.clone());
    let ud = temp_ud();
    let emitter = started(&reg, &ud, &ts_server("ts1", "myts")).await;
    sub.push(0, frame("myts", "Running", ""));
    acquire(&spawner.terminating).await;
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    spawner.release.add_permits(1);
    wait_until(|| {
        emitter
            .progress
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.2 == "authorized")
    })
    .await;
    assert!(reg.inflight_login_pids().is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

struct SecretChecker(Mutex<Vec<Value>>);
#[async_trait]
impl ConfigChecker for SecretChecker {
    async fn check(&self, _: &Path, path: &Path) -> Result<(), String> {
        let cfg: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let diagnostic = cfg.to_string();
        self.0.lock().unwrap().push(cfg);
        Err(diagnostic)
    }
}

#[tokio::test]
async fn authkey_is_in_secure_config_but_not_failed_diagnostic_and_browser_omits_it() {
    let checker = Arc::new(SecretChecker(Mutex::new(Vec::new())));
    let reg = LoginCoreRegistry::with_deps(
        fake_spawner(vec![], false, false),
        checker.clone(),
        fake_subscriber(false),
        Arc::new(|| Ok("/fake/core".into())),
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let mut server = ts_server("ts1", "myts");
    server.tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            auth_key: Some("HEADSCALE_PRIVATE_SENTINEL".into()),
            control_url: Some("https://hs.example".into()),
            ..Default::default()
        },
    ));
    let emitter = Arc::new(FakeEmitter::default());
    for (id, mode) in [("key", LoginMode::Authkey), ("browser", LoginMode::Browser)] {
        reg.prepare("ts1", id).unwrap();
        let outcome = reg
            .start_attempt(
                &server,
                &ud,
                LoginRequest {
                    attempt_id: id.into(),
                    mode,
                },
                &offline,
                emitter.clone(),
            )
            .await;
        assert!(
            matches!(outcome, StartLoginOutcome::Failed(ref reason) if reason == "configurationCheckFailed")
        );
    }
    let configs = checker.0.lock().unwrap();
    assert_eq!(
        configs[0]["endpoints"][0]["auth_key"],
        "HEADSCALE_PRIVATE_SENTINEL"
    );
    assert!(configs[1]["endpoints"][0].get("auth_key").is_none());
    assert_eq!(
        configs[0]["endpoints"][0]["control_url"],
        "https://hs.example"
    );
    let output = format!("{:?}", emitter.progress.lock().unwrap());
    assert!(!output.contains("HEADSCALE_PRIVATE_SENTINEL"));
    assert!(!output.contains(configs[0]["services"][0]["secret"].as_str().unwrap()));
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn cancelling_an_old_attempt_cannot_stop_its_replacement() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let server = ts_server("ts1", "myts");
    for id in ["old", "new"] {
        reg.prepare("ts1", id).unwrap();
        assert!(matches!(
            reg.start_attempt(
                &server,
                &ud,
                request(id),
                &offline,
                Arc::new(FakeEmitter::default())
            )
            .await,
            StartLoginOutcome::Started
        ));
    }
    reg.cancel_attempt("ts1", "old").await.unwrap();
    assert!(reg.shared.contains("ts1"));
    reg.cancel_attempt("ts1", "new").await.unwrap();
    assert!(!reg.shared.contains("ts1"));
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn logout_refuses_a_live_main_owner_without_deleting_state() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let state = ud.join("tailscale/ts1");
    std::fs::create_dir_all(&state).unwrap();
    {
        let _gate = reg.state_gate().await;
        reg.reserve_main_states(
            &json!({"endpoints": [{"type":"tailscale", "state_directory":state}]}),
            &ud,
        )
        .await;
    }
    assert!(!reg
        .logout("ts1", &|| true, None, |_| panic!(
            "main-owned state must not be deleted"
        ))
        .await
        .unwrap());
    assert!(state.exists());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn authkey_replacement_preserves_prepared_request_and_waits_before_delete_then_start() {
    let (reg, spawner) = slow_registry(fake_subscriber(false));
    let ud = temp_ud();
    let mut server = ts_server("ts1", "myts");
    started(&reg, &ud, &server).await;
    let state = ud.join("tailscale/ts1");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("sentinel"), "state").unwrap();
    reg.prepare("ts1", "replacement").unwrap();
    let (reg2, state2) = (reg.clone(), state.clone());
    let logout = tokio::spawn(async move {
        reg2.logout("ts1", &|| false, Some("replacement"), |_| {
            std::fs::remove_dir_all(state2)
        })
        .await
    });
    acquire(&spawner.terminating).await;
    assert!(
        state.join("sentinel").exists(),
        "logout cannot remove a directory while the old child writes it"
    );
    assert!(!logout.is_finished());
    spawner.release.add_permits(1);
    assert!(logout.await.unwrap().unwrap());
    assert!(!state.exists());
    // The command persists this same node before starting the prepared request.
    server.tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            auth_key: Some("replacement-private-key".into()),
            ..Default::default()
        },
    ));
    let config = crate::runtime::config::ConfigManager::new(ud.clone());
    let mut stored = config.load_full().unwrap();
    stored["servers"] = json!([server]);
    config.save_full(&stored).unwrap();
    let persisted = config.current().unwrap();
    let saved: ServerConfig = serde_json::from_value(persisted["servers"][0].clone()).unwrap();
    assert!(matches!(
        reg.start_attempt(
            &saved,
            &ud,
            LoginRequest {
                attempt_id: "replacement".into(),
                mode: LoginMode::Authkey
            },
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Started
    ));
    let config: Value = serde_json::from_slice(
        &std::fs::read(
            login_configs(&ud)
                .into_iter()
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("tailscale-login-")
                })
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        config["endpoints"][0]["auth_key"],
        "replacement-private-key"
    );
    spawner.release.add_permits(1);
    reg.cancel_attempt("ts1", "replacement").await.unwrap();
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn closing_during_authkey_logout_fences_the_preserved_request() {
    let (reg, spawner) = slow_registry(fake_subscriber(false));
    let ud = temp_ud();
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    reg.prepare("ts1", "replacement").unwrap();
    let reg2 = reg.clone();
    let logout = tokio::spawn(async move {
        reg2.logout("ts1", &|| false, Some("replacement"), |_| Ok(()))
            .await
    });
    acquire(&spawner.terminating).await;
    reg.cancel_attempt("ts1", "replacement").await.unwrap();
    spawner.release.add_permits(1);
    assert!(logout.await.unwrap().unwrap());
    assert!(matches!(
        reg.start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("replacement"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Cancelled
    ));
    assert_eq!(spawner.base.count.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn logout_preservation_cannot_exempt_unknown_other_node_cancelled_or_claimed_requests() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    reg.prepare("other", "foreign").unwrap();
    reg.cancel_attempt("ts1", "cancelled").await.unwrap();
    reg.prepare("ts1", "claimed").unwrap();
    assert!(matches!(
        reg.start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("claimed"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Started
    ));
    for id in ["unknown", "foreign", "cancelled", "claimed"] {
        assert!(reg
            .logout("ts1", &|| false, Some(id), |_| panic!(
                "invalid keep ID cannot delete state"
            ))
            .await
            .is_err());
    }
    assert!(reg.shared.contains("ts1"));
    reg.cancel_attempt("ts1", "claimed").await.unwrap();
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn main_owner_does_not_authorize_new_credentials_from_old_endpoint() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    {
        let _gate = reg.state_gate().await;
        reg.reserve_main_states(&json!({"endpoints":[{"type":"tailscale", "state_directory":ud.join("tailscale/ts1"), "auth_key":"old-key"}]}), &ud).await;
    }
    let mut server = ts_server("ts1", "myts");
    server.tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            auth_key: Some("new-key".into()),
            ..Default::default()
        },
    ));
    reg.prepare("ts1", "new-key").unwrap();
    let emitter = Arc::new(FakeEmitter::default());
    assert!(matches!(
        reg.start_attempt(
            &server,
            &ud,
            LoginRequest {
                attempt_id: "new-key".into(),
                mode: LoginMode::Authkey
            },
            &|| MainLoginSnapshot {
                alive: true,
                ..Default::default()
            },
            emitter.clone()
        )
        .await,
        StartLoginOutcome::InMainCorePending
    ));
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.2 == "mainCore" && p.3.as_deref() == Some("configurationPending")));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn executable_auth_url_terminates_and_reaps_before_failure() {
    let sub = fake_subscriber(false);
    let (reg, spawner) = slow_registry(sub.clone());
    let ud = temp_ud();
    let emitter = started(&reg, &ud, &ts_server("ts1", "myts")).await;
    sub.push(0, frame("myts", "NeedsLogin", "javascript:alert(1)"));
    acquire(&spawner.terminating).await;
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "failed"));
    assert!(emitter.captured.lock().unwrap().is_empty());
    spawner.release.add_permits(1);
    wait_until(|| {
        emitter
            .progress
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.2 == "failed" && p.3.as_deref() == Some("invalidAuthUrl"))
    })
    .await;
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn prepare_tombstones_are_bounded_and_evicted_starts_are_rejected() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    for i in 0..512 {
        reg.cancel_attempt("ts1", &format!("old-{i}"))
            .await
            .unwrap();
    }
    reg.prepare("ts1", "fresh").unwrap();
    assert!(matches!(
        reg.start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("old-0"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Failed(_)
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    reg.cancel_attempt("ts1", "fresh").await.unwrap();
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn login_creates_private_parent_without_prior_main_core_start() {
    let root = temp_ud();
    let ud = root.join("not-created/app-config");
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    reg.prepare("ts1", "cold-login").unwrap();
    assert!(matches!(
        reg.start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("cold-login"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Started
    ));
    assert!(ud.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&ud).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&login_configs(&ud)[0])
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    reg.cancel_attempt("ts1", "cold-login").await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

async fn owned_main_registry(
    timeout: Duration,
) -> (Arc<LoginCoreRegistry>, Arc<FakeStatusSubscriber>, PathBuf) {
    let sub = fake_subscriber(false);
    let reg = Arc::new(reg_with(
        fake_spawner(vec![], false, false),
        sub.clone(),
        true,
        timeout,
    ));
    let ud = temp_ud();
    let _gate = reg.state_gate().await;
    reg.reserve_main_states(&json!({"endpoints":[{"type":"tailscale", "tag":"actual-generated-tag", "state_directory":ud.join("tailscale/ts1")}]}), &ud).await;
    drop(_gate);
    (reg, sub, ud)
}
fn main_snapshot(generation: u64) -> MainLoginSnapshot {
    MainLoginSnapshot {
        alive: true,
        generation,
        api_port: 12345,
        api_secret: "actual-api-secret".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn steady_main_running_is_confirmed_by_fresh_initial_frame_without_global_events() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    let emitter = Arc::new(FakeEmitter::default());
    reg.prepare("ts1", "main-fresh").unwrap();
    let (reg2, ud2, emitter2) = (reg.clone(), ud.clone(), emitter.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("main-fresh"),
            &|| main_snapshot(10),
            emitter2,
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    assert_eq!(
        sub.seen.lock().unwrap()[0],
        (12345, "actual-api-secret".into())
    );
    // Running with no self record follows the existing decoder; a residual executable URL is ignored.
    sub.push(
        0,
        frame("actual-generated-tag", "Running", "javascript:residual"),
    );
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::InMainCore));
    let events = emitter.progress.lock().unwrap();
    assert!(events.iter().any(|p| p.2 == "authorized" && p.4.is_none()));
    assert!(
        sub.senders.lock().unwrap()[0].is_closed(),
        "one-shot initial subscription must be released"
    );
    assert!(reg.inflight_login_pids().is_empty());
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn fresh_main_query_exposes_headscale_url_and_drops_its_stream() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    reg.prepare("ts1", "main-url").unwrap();
    let emitter = Arc::new(FakeEmitter::default());
    let (reg2, ud2, emitter2) = (reg.clone(), ud.clone(), emitter.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("main-url"),
            &|| main_snapshot(10),
            emitter2,
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    sub.push(
        0,
        frame(
            "actual-generated-tag",
            "NeedsLogin",
            "https://headscale.example/custom-register",
        ),
    );
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::InMainCore));
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.2 == "mainCore"
            && p.3.is_none()
            && p.4.as_deref() == Some("https://headscale.example/custom-register")));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn replaced_main_instance_cannot_confirm_the_old_request() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    let generation = Arc::new(AtomicU64::new(10));
    reg.prepare("ts1", "main-replaced").unwrap();
    let emitter = Arc::new(FakeEmitter::default());
    let (reg2, ud2, emitter2, generation2) =
        (reg.clone(), ud.clone(), emitter.clone(), generation.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("main-replaced"),
            &|| main_snapshot(generation2.load(Ordering::SeqCst)),
            emitter2,
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    generation.store(11, Ordering::SeqCst);
    sub.push(0, frame("actual-generated-tag", "Running", ""));
    assert!(
        matches!(task.await.unwrap(), StartLoginOutcome::Failed(reason) if reason == "mainCoreChanged")
    );
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn cancelling_a_fresh_main_query_drops_subscription_without_authorizing() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    reg.prepare("ts1", "main-cancel").unwrap();
    let emitter = Arc::new(FakeEmitter::default());
    let (reg2, ud2, emitter2) = (reg.clone(), ud.clone(), emitter.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("main-cancel"),
            &|| main_snapshot(10),
            emitter2,
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    reg.cancel_attempt("ts1", "main-cancel").await.unwrap();
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::Cancelled));
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    assert!(
        reg.main_owns("ts1", true),
        "cancel never stops the main connection"
    );
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn fresh_main_query_timeout_is_terminal_and_releases_subscription() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_millis(30)).await;
    reg.prepare("ts1", "main-timeout").unwrap();
    let emitter = Arc::new(FakeEmitter::default());
    let result = reg
        .start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("main-timeout"),
            &|| main_snapshot(10),
            emitter.clone(),
        )
        .await;
    assert!(
        matches!(result, StartLoginOutcome::Failed(reason) if reason == "authorizationTimedOut")
    );
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.2 == "timedOut"));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn transient_running_success_ignores_a_residual_invalid_auth_url() {
    let sub = fake_subscriber(false);
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        sub.clone(),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let emitter = started(&reg, &ud, &ts_server("ts1", "myts")).await;
    sub.push(0, frame("myts", "Running", "javascript:residual"));
    wait_until(|| {
        emitter
            .progress
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.2 == "authorized")
    })
    .await;
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "failed"));
    assert!(emitter.captured.lock().unwrap().is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

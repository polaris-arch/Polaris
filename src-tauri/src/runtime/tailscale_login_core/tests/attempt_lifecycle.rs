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
    async fn close_confirmed(&mut self) -> Result<(), String> {
        self.terminating.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.child.close_confirmed().await
    }
}

struct SlowSpawner {
    base: Arc<FakeSpawner>,
    terminating: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
#[async_trait]
impl LoginCoreSpawner for SlowSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        Ok(Box::new(SlowChild {
            child: self.base.spawn(req).await?,
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

async fn claim_main_for_test(
    reg: &LoginCoreRegistry,
    generated: &serde_json::Value,
    root: &Path,
    gate: &tokio::sync::MutexGuard<'_, ()>,
) -> MainBirthToken {
    let token = reg.mint_main_birth();
    let mut reservation = reg
        .reserve_main_states(generated, root, gate, token.clone())
        .await
        .unwrap();
    // Simulate an external start: the claim must outlive this fixture scope.
    reservation.arm_external_start();
    token
}

#[tokio::test]
async fn main_birth_token_only_retires_its_own_complete_claim() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let root = temp_ud();
    let gate = reg.state_gate().await;
    let first = json!({"endpoints":[{"type":"tailscale", "tag":"first", "state_directory":root.join("tailscale/ts1")}]});
    let second = json!({"endpoints":[{"type":"tailscale", "tag":"second", "state_directory":root.join("tailscale/ts2")}]});
    let old = claim_main_for_test(&reg, &first, &root, &gate).await;
    assert!(reg.main_owns("ts1", true));
    assert!(
        reg.reserve_main_states(&second, &root, &gate, reg.mint_main_birth())
            .await
            .is_err(),
        "overlap or changed range cannot replace a live physical attempt"
    );
    assert!(reg.release_main_states_if_token(&old, &gate).unwrap());
    let new = claim_main_for_test(&reg, &second, &root, &gate).await;
    assert!(!reg.release_main_states_if_token(&old, &gate).unwrap());
    assert!(!reg.main_owns("ts1", true));
    assert!(reg.main_owns("ts2", true));
    assert!(reg.release_main_states_if_token(&new, &gate).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn main_birth_requires_actual_registry_and_gate() {
    let make = || {
        reg_with(
            fake_spawner(vec![], false, false),
            fake_subscriber(false),
            true,
            Duration::from_secs(60),
        )
    };
    let a = make();
    let b = make();
    let root = temp_ud();
    let generated =
        json!({"endpoints":[{"type":"tailscale", "state_directory":root.join("tailscale/ts1")}]});
    let a_gate = a.state_gate().await;
    let b_gate = b.state_gate().await;
    let a_token = a.mint_main_birth();
    assert!(a
        .reserve_main_states(&generated, &root, &b_gate, a_token.clone())
        .await
        .is_err());
    assert!(b
        .reserve_main_states(&generated, &root, &b_gate, a_token.clone())
        .await
        .is_err());
    let mut reservation = a
        .reserve_main_states(&generated, &root, &a_gate, a_token.clone())
        .await
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    assert!(a.release_main_states_if_token(&a_token, &b_gate).is_err());
    assert!(b.release_main_states_if_token(&a_token, &b_gate).is_err());
    assert!(a.main_owns("ts1", true));
    assert!(a.release_main_states_if_token(&a_token, &a_gate).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn main_reservation_rejects_any_invalid_ts_directory_but_no_ts_is_noop() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let root = temp_ud();
    let gate = reg.state_gate().await;
    let invalid = json!({"endpoints":[
        {"type":"tailscale", "state_directory":root.join("tailscale/ts1")},
        {"type":"tailscale", "state_directory":root.join("other/ts2")}
    ]});
    assert!(reg
        .reserve_main_states(&invalid, &root, &gate, reg.mint_main_birth())
        .await
        .is_err());
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    let missing = json!({"endpoints":[{"type":"tailscale", "tag":"missing"}]});
    assert!(reg
        .reserve_main_states(&missing, &root, &gate, reg.mint_main_birth())
        .await
        .is_err());
    let empty = json!({"endpoints":[{"type":"wireguard", "tag":"not-ts"}]});
    let mut no_ts = reg
        .reserve_main_states(&empty, &root, &gate, reg.mint_main_birth())
        .await
        .unwrap();
    assert!(no_ts.claim_token().is_none());
    no_ts.arm_external_start();
    drop(no_ts);
    let new = claim_main_for_test(
        &reg,
        &json!({"endpoints":[{"type":"tailscale", "state_directory":root.join("tailscale/ts1")}] }),
        &root,
        &gate,
    )
    .await;
    assert!(reg.release_main_states_if_token(&new, &gate).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn pre_spawn_drop_rolls_back_only_its_token_but_armed_drop_retains_it() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let root = temp_ud();
    let generated =
        json!({"endpoints":[{"type":"tailscale", "state_directory":root.join("tailscale/ts1")}]});
    let gate = reg.state_gate().await;
    let early = reg.mint_main_birth();
    drop(
        reg.reserve_main_states(&generated, &root, &gate, early.clone())
            .await
            .unwrap(),
    );
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    let mut late = reg
        .reserve_main_states(&generated, &root, &gate, reg.mint_main_birth())
        .await
        .unwrap();
    let token = late.claim_token().unwrap();
    late.arm_external_start();
    drop(late);
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy
    );
    assert!(!reg.release_main_states_if_token(&early, &gate).unwrap());
    assert!(reg.release_main_states_if_token(&token, &gate).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn unknown_main_claim_blocks_logout_even_when_alive_probe_is_false() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let root = temp_ud();
    let state = root.join("tailscale/ts1");
    std::fs::create_dir_all(&state).unwrap();
    let token = {
        let gate = reg.state_gate().await;
        claim_main_for_test(
            &reg,
            &json!({"endpoints":[{"type":"tailscale", "state_directory":state}]}),
            &root,
            &gate,
        )
        .await
    };
    let deleted = std::sync::atomic::AtomicBool::new(false);
    assert!(!reg
        .logout("ts1", &|| false, None, |_| {
            deleted.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap());
    assert!(!deleted.load(Ordering::SeqCst));
    assert!(reg.state_in_use("ts1", false));
    assert!(
        reg.logout("ts2", &|| false, None, |_| Ok(()))
            .await
            .unwrap(),
        "unrelated ID is not blocked"
    );
    {
        let gate = reg.state_gate().await;
        assert!(reg.release_main_states_if_token(&token, &gate).unwrap());
    }
    assert!(reg
        .logout("ts1", &|| false, None, |_| {
            deleted.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap());
    assert!(deleted.load(Ordering::SeqCst));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn poisoned_main_registry_never_authorizes_state_deletion() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let shared = Arc::clone(&reg.shared);
    assert!(std::thread::spawn(move || {
        let _guard = shared.main.lock().unwrap();
        panic!("poison main registry");
    })
    .join()
    .is_err());

    let deleted = std::sync::atomic::AtomicBool::new(false);
    assert!(reg.state_in_use("ts1", false));
    assert!(!reg
        .logout("ts1", &|| false, None, |_| {
            deleted.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap());
    assert!(!deleted.load(Ordering::SeqCst));
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
    reg.prepare("ts1", "early").await.unwrap_err();
    assert!(matches!(
        reg.start_attempt(
            &server,
            &ud,
            request("early"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Failed(_)
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    assert!(login_configs(&ud).is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn retirement_fences_prepared_ids_and_a_late_prepare_until_state_commit_finishes() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = Arc::new(reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    ));
    reg.prepare("ts1", "old").await.unwrap();
    let gate = reg.state_gate().await;
    reg.retire_attempts_under_state_gate("ts1", &gate)
        .await
        .unwrap();
    assert!(reg.attempts.get("ts1", "old").is_err());

    let entered = Arc::new(Semaphore::new(0));
    let late = {
        let reg = reg.clone();
        let entered = entered.clone();
        tokio::spawn(async move {
            entered.add_permits(1);
            reg.prepare("ts1", "new").await
        })
    };
    acquire(&entered).await;
    assert!(
        !late.is_finished(),
        "prepare must wait for the retirement gate"
    );
    drop(gate);
    late.await.unwrap().unwrap();
    reg.cancel_attempt("ts1", "old").await.unwrap();
    assert!(reg.prepare("ts1", "old").await.is_err());
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_owner_fact_counts_unspawned_main_reservation_and_prepared_attempt() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    let generated = json!({"endpoints": [{
        "type": "tailscale",
        "tag": "myts",
        "state_directory": ud.join("tailscale/ts1")
    }]});
    let token = claim_main_for_test(&reg, &generated, &ud, &gate).await;
    assert!(!reg.main_owns("ts1", false));
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy,
        "reservation blocks even before the primary core has spawned"
    );
    assert_eq!(
        reg.owner_state_under_gate("ts2", &gate),
        TsRegistryOwnerState::Vacant
    );
    assert!(reg.release_main_states_if_token(&token, &gate).unwrap());
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    drop(gate);

    reg.prepare("ts1", "prepared").await.unwrap();
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy,
        "unclaimed prepare is still an admission"
    );
    reg.retire_attempts_under_state_gate("ts1", &gate)
        .await
        .unwrap();
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant,
        "finished retired attempts have no local owner and remain fenced by ID"
    );
    drop(gate);
    assert!(reg.prepare("ts1", "prepared").await.is_err());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn local_owner_fact_rejects_a_foreign_gate_and_waiting_claim() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = Arc::new(reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    ));
    let foreign = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let wrong_gate = foreign.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &wrong_gate),
        TsRegistryOwnerState::Unknown
    );
    drop(wrong_gate);

    reg.prepare("ts1", "claim").await.unwrap();
    let gate = reg.state_gate().await;
    let ud = temp_ud();
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let waiting = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("claim"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    let claimed = reg.attempts.get("ts1", "claim").unwrap();
    wait_until(|| claimed.claimed.load(Ordering::SeqCst)).await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy
    );
    reg.retire_attempts_under_state_gate("ts1", &gate)
        .await
        .unwrap();
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    drop(gate);
    assert!(matches!(
        waiting.await.unwrap(),
        StartLoginOutcome::Cancelled
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn local_owner_fact_rejects_lost_cleanup_channel_and_poisoned_reservation_table() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    reg.register_inflight_for_test("ts1", 4242);
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown,
        "a detached supervisor cannot certify native cleanup"
    );
    reg.deregister_inflight_for_test("ts1");
    drop(gate);

    let shared = reg.shared.clone();
    assert!(std::thread::spawn(move || {
        let _held = shared.main.lock().unwrap();
        panic!("poison reservation table");
    })
    .join()
    .is_err());
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown,
        "poison cannot be recovered into a no-owner fact"
    );
}

#[tokio::test]
async fn local_owner_fact_does_not_infer_vacancy_from_missing_pid_or_retained_close_receipt() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let (cancel_tx, _cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (closed_tx, closed_rx) = tokio::sync::watch::channel(None);
    reg.shared.insert(
        "ts1".into(),
        LoginEntry {
            epoch: 1,
            attempt_id: "no-pid".into(),
            pid: None,
            cancel_tx,
            closed_rx,
        },
    );
    let gate = reg.state_gate().await;
    assert!(reg.inflight_login_pids().is_empty());
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy,
        "a live entry is busy even before any PID is available"
    );
    closed_tx.send_replace(Some(Ok(())));
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown,
        "a close receipt without successful deregistration is not vacant"
    );
    reg.deregister_inflight_for_test("ts1");
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
}

#[tokio::test]
async fn retired_ids_survive_pruning_and_capacity_exhaustion_fails_closed() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    for i in 0..attempts::MAX_RETIRED_ATTEMPTS - 1 {
        let id = format!("retired-{i}");
        reg.attempts.prepare("ts1", &id).unwrap();
        reg.attempts.retire_node_except("ts1", None).unwrap();
    }
    assert_eq!(
        reg.attempts.prepare("ts1", "retired-0").err().as_deref(),
        Some("Login request was retired"),
        "a pruned tombstone must remain retired"
    );
    reg.attempts.prepare("ts1", "overflow-0").unwrap();
    reg.attempts.prepare("ts1", "overflow-1").unwrap();
    assert_eq!(
        reg.attempts
            .retire_node_except("ts1", None)
            .err()
            .as_deref(),
        Some(attempts::RETIRED_LIMIT_ERROR)
    );
    assert_eq!(
        reg.attempts.prepare("ts1", "fresh").err().as_deref(),
        Some(attempts::RETIRED_LIMIT_ERROR)
    );
    assert_eq!(
        reg.attempts.get("ts1", "overflow-0").err().as_deref(),
        Some(attempts::RETIRED_LIMIT_ERROR),
        "even a previously prepared request cannot start after exhaustion"
    );
    let ud = temp_ud();
    assert!(matches!(
        reg.start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("overflow-0"),
            &offline,
            Arc::new(FakeEmitter::default())
        )
        .await,
        StartLoginOutcome::Failed(reason) if reason == attempts::RETIRED_LIMIT_ERROR
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn late_login_uses_saved_identity_after_gate_and_rejects_old_settings() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = Arc::new(reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    ));
    reg.prepare("ts1", "new").await.unwrap();
    let old = ts_server("ts1", "myts");
    let mut current = old.clone();
    current.tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            control_url: Some("https://new.example".into()),
            ..Default::default()
        },
    ));
    let ud = temp_ud();
    let gate = reg.state_gate().await;
    let entered = Arc::new(Semaphore::new(0));
    let late = {
        let reg = reg.clone();
        let entered = entered.clone();
        let ud = ud.clone();
        tokio::spawn(async move {
            entered.add_permits(1);
            reg.start_attempt_with_saved(
                &old,
                &ud,
                request("new"),
                &|| Ok(current.clone()),
                &offline,
                Arc::new(FakeEmitter::default()),
            )
            .await
        })
    };
    acquire(&entered).await;
    assert!(!late.is_finished());
    drop(gate);
    assert!(matches!(
        late.await.unwrap(),
        StartLoginOutcome::Failed(reason) if reason == "savedTailscaleIdentityChanged"
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
    reg.prepare("ts1", "check").await.unwrap();
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
    reg.cancel_and_wait("ts1").await.unwrap();
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn local_owner_fact_keeps_transient_busy_through_reap() {
    let (reg, spawner) = slow_registry(fake_subscriber(false));
    let ud = temp_ud();
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy,
        "subscription holds the transient entry"
    );
    drop(gate);

    let reg2 = reg.clone();
    let closing = tokio::spawn(async move { reg2.cancel_login("ts1").await });
    acquire(&spawner.terminating).await;
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Busy,
        "a requested close is still a local owner until reap"
    );
    drop(gate);
    spawner.release.add_permits(1);
    assert!(closing.await.unwrap().unwrap());
    wait_until(|| !reg.attempts.owns_state("ts1")).await;
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn retirement_waits_for_transient_reap_and_keeps_failed_close_as_owner() {
    let (reg, spawner) = slow_registry(fake_subscriber(false));
    let ud = temp_ud();
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    let old = spawner.base.spawned.lock().unwrap()[0].clone();
    old.close_failures.store(1, Ordering::SeqCst);
    let (reg2, entered) = (reg.clone(), Arc::new(Semaphore::new(0)));
    let entered2 = entered.clone();
    let retiring = tokio::spawn(async move {
        let gate = reg2.state_gate().await;
        entered2.add_permits(1);
        reg2.retire_attempts_under_state_gate("ts1", &gate).await
    });
    acquire(&entered).await;
    acquire(&spawner.terminating).await;
    spawner.release.add_permits(1);
    assert!(
        retiring.await.unwrap().is_err(),
        "failed close cannot prove retirement"
    );
    assert!(reg.shared.contains("ts1"), "failed close retains the owner");
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Unknown,
        "failed native close is cleanupUnknown, not vacant"
    );
    drop(gate);

    let reg2 = reg.clone();
    let retry = tokio::spawn(async move {
        let gate = reg2.state_gate().await;
        reg2.retire_attempts_under_state_gate("ts1", &gate).await
    });
    acquire(&spawner.terminating).await;
    assert!(!retry.is_finished(), "reap must complete before retirement");
    assert!(reg.shared.contains("ts1"));
    spawner.release.add_permits(1);
    retry.await.unwrap().unwrap();
    assert!(!reg.shared.contains("ts1"));
    assert!(reg.inflight_login_pids().is_empty());
    let gate = reg.state_gate().await;
    assert_eq!(
        reg.owner_state_under_gate("ts1", &gate),
        TsRegistryOwnerState::Vacant
    );
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
        let gate = reg2.state_gate().await;
        claim_main_for_test(&reg2, &json!({"endpoints": [{"type": "tailscale", "tag":"myts", "state_directory": ud2.join("tailscale/ts1")}, {"type": "wireguard", "tag": "ts2"}]}), &ud2, &gate).await;
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
    reg.prepare("ts1", "main").await.unwrap();
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
async fn failed_transient_close_blocks_main_reservation_until_retry_confirms_reap() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    let old = spawner.spawned.lock().unwrap()[0].clone();
    old.close_failures.store(1, Ordering::SeqCst);
    let final_config = json!({"endpoints": [{
        "type": "tailscale", "tag": "myts", "state_directory": ud.join("tailscale/ts1")
    }]});
    {
        let gate = reg.state_gate().await;
        assert!(reg
            .reserve_main_states(&final_config, &ud, &gate, reg.mint_main_birth())
            .await
            .is_err());
    }
    assert!(
        reg.shared.contains("ts1"),
        "failed close retains transient claim"
    );
    assert!(!old.terminated.load(Ordering::SeqCst));
    assert!(
        !reg.main_owns("ts1", true),
        "failed reservation cannot claim main ownership"
    );
    {
        let gate = reg.state_gate().await;
        claim_main_for_test(&reg, &final_config, &ud, &gate).await;
    }
    assert!(old.terminated.load(Ordering::SeqCst));
    assert!(!reg.shared.contains("ts1"));
    assert!(reg.main_owns("ts1", true));
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn dropping_ipc_during_subscribe_retains_pid_until_reap() {
    let entered = Arc::new(Semaphore::new(0));
    let (reg, spawner) = slow_registry(Arc::new(BlockingSubscriber(entered.clone())));
    let ud = temp_ud();
    reg.prepare("ts1", "drop").await.unwrap();
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
    sub.push(0, frame(TAILSCALE_LOGIN_ENDPOINT_TAG, "Running", ""));
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
        reg.prepare("ts1", id).await.unwrap();
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
        reg.prepare("ts1", id).await.unwrap();
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
        let gate = reg.state_gate().await;
        claim_main_for_test(
            &reg,
            &json!({"endpoints": [{"type":"tailscale", "state_directory":state}]}),
            &ud,
            &gate,
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
    reg.prepare("ts1", "replacement").await.unwrap();
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
    reg.prepare("ts1", "replacement").await.unwrap();
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
        StartLoginOutcome::Failed(_)
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
    reg.prepare("other", "foreign").await.unwrap();
    reg.cancel_attempt("ts1", "cancelled").await.unwrap();
    reg.prepare("ts1", "claimed").await.unwrap();
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
        let gate = reg.state_gate().await;
        claim_main_for_test(&reg, &json!({"endpoints":[{"type":"tailscale", "state_directory":ud.join("tailscale/ts1"), "auth_key":"old-key"}]}), &ud, &gate).await;
    }
    let mut server = ts_server("ts1", "myts");
    server.tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            auth_key: Some("new-key".into()),
            ..Default::default()
        },
    ));
    reg.prepare("ts1", "new-key").await.unwrap();
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
async fn unknown_main_start_blocks_real_transient_launch_even_when_alive_is_false() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    let token = {
        let gate = reg.state_gate().await;
        claim_main_for_test(
            &reg,
            &json!({"endpoints":[{"type":"tailscale", "state_directory":ud.join("tailscale/ts1")}] }),
            &ud,
            &gate,
        )
        .await
    };
    let server = ts_server("ts1", "myts");
    reg.prepare("ts1", "unknown-main").await.unwrap();
    assert!(matches!(
        reg.start_attempt(
            &server,
            &ud,
            request("unknown-main"),
            &|| MainLoginSnapshot {
                alive: false,
                ..Default::default()
            },
            Arc::new(FakeEmitter::default()),
        )
        .await,
        StartLoginOutcome::InMainCorePending
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    let gate = reg.state_gate().await;
    assert!(reg.release_main_states_if_token(&token, &gate).unwrap());
    drop(gate);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn executable_auth_url_terminates_and_reaps_before_failure() {
    let sub = fake_subscriber(false);
    let (reg, spawner) = slow_registry(sub.clone());
    let ud = temp_ud();
    let emitter = started(&reg, &ud, &ts_server("ts1", "myts")).await;
    sub.push(
        0,
        frame(
            TAILSCALE_LOGIN_ENDPOINT_TAG,
            "NeedsLogin",
            "javascript:alert(1)",
        ),
    );
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
    reg.prepare("ts1", "fresh").await.unwrap();
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
    reg.prepare("ts1", "cold-login").await.unwrap();
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
    let gate = reg.state_gate().await;
    claim_main_for_test(&reg, &json!({"endpoints":[{"type":"tailscale", "tag":"actual-generated-tag", "state_directory":ud.join("tailscale/ts1")}]}), &ud, &gate).await;
    drop(gate);
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
    reg.prepare("ts1", "main-fresh").await.unwrap();
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
    reg.prepare("ts1", "main-url").await.unwrap();
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
    reg.prepare("ts1", "main-replaced").await.unwrap();
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
    reg.prepare("ts1", "main-cancel").await.unwrap();
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
    reg.prepare("ts1", "main-timeout").await.unwrap();
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
    sub.push(
        0,
        frame(
            TAILSCALE_LOGIN_ENDPOINT_TAG,
            "Running",
            "javascript:residual",
        ),
    );
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

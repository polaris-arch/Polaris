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
    async fn wait_result(&mut self) -> Result<(), String> {
        self.child.wait_result().await
    }
    async fn after_exit(&mut self) -> Result<(), String> {
        self.child.after_exit().await
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
        replace_identity: false,
        reuse_retained_auth_key: false,
        expected_credential_revision: None,
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
async fn confirmed_reservation_stop_commits_only_exact_registered_claim() {
    let reg = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let root = temp_ud();
    let gate = reg.state_gate().await;
    reg.assert_main_claims_drained(&gate).unwrap();
    let generated =
        json!({"endpoints":[{"type":"tailscale", "state_directory":root.join("tailscale/ts1")}]});
    let token = reg.mint_main_birth();
    let mut reservation = reg
        .reserve_main_states(&generated, &root, &gate, token.clone())
        .await
        .unwrap();
    reservation.arm_external_start();
    assert!(reg.assert_main_claims_drained(&gate).is_err());
    assert!(reservation
        .release_confirmed_stop_claim(&reg.mint_main_birth())
        .is_err());
    assert!(reservation.registered);
    assert!(reservation.external_possible);
    assert!(reg.main_claims("ts1"));
    reservation.release_confirmed_stop_claim(&token).unwrap();
    reg.assert_main_claims_drained(&gate).unwrap();
    assert!(!reservation.registered);
    assert!(!reg.main_claims("ts1"));
    assert!(reservation.release_confirmed_stop_claim(&token).is_err());
    drop(reservation);
    let successor = claim_main_for_test(&reg, &generated, &root, &gate).await;
    assert!(
        reg.main_claims("ts1"),
        "committed old guard cannot roll back a successor"
    );
    assert!(reg.release_main_states_if_token(&successor, &gate).unwrap());
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
    assert!(a.assert_main_claims_drained(&b_gate).is_err());
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
            _child: None,
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
                    replace_identity: false,
                    reuse_retained_auth_key: false,
                    expected_credential_revision: None,
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
                mode: LoginMode::Authkey,
                replace_identity: false,
                reuse_retained_auth_key: false,
                expected_credential_revision: None,
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
                mode: LoginMode::Authkey,
                replace_identity: false,
                reuse_retained_auth_key: false,
                expected_credential_revision: None,
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
    let attempt_id = emitter.progress.lock().unwrap()[0].1.clone();
    let receipt = reg.login_progress("ts1", &attempt_id).unwrap();
    assert_eq!(receipt.phase, "failed");
    assert_eq!(receipt.reason.as_deref(), Some("invalidAuthUrl"));
    assert!(reg.login_progress("ts2", &attempt_id).is_none());
    assert!(reg.login_progress("ts1", "another-attempt").is_none());
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
async fn fresh_main_observer_recovers_headscale_url_and_waits_until_authorized() {
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
    wait_until(|| {
        reg.login_progress("ts1", "main-url")
            .is_some_and(|receipt| receipt.url.is_some())
    })
    .await;
    assert!(
        !task.is_finished(),
        "fresh NeedsLogin must keep the observer alive"
    );
    assert!(!sub.senders.lock().unwrap()[0].is_closed());
    let receipt = reg.login_progress("ts1", "main-url").unwrap();
    assert_eq!(
        receipt.url.as_deref(),
        Some("https://headscale.example/custom-register")
    );
    assert_eq!(receipt.main_generation, Some(10));
    // Normal Stop can acquire this gate while the browser owns the foreground.
    let gate = tokio::time::timeout(Duration::from_millis(100), reg.state_gate())
        .await
        .unwrap();
    drop(gate);
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.2 == "mainCore"
            && p.3.is_none()
            && p.4.as_deref() == Some("https://headscale.example/custom-register")));
    sub.push(0, frame("actual-generated-tag", "Running", ""));
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::InMainCore));
    let receipt = reg.login_progress("ts1", "main-url").unwrap();
    assert_eq!(receipt.phase, "authorized");
    assert!(receipt.url.is_none());
    assert!(reg.main_owns("ts1", true));
    assert_eq!(
        emitter
            .progress
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.2 == "authorized")
            .count(),
        1
    );
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn dropping_main_login_future_retires_active_url_and_preserves_exact_context() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    reg.prepare("ts1", "dropped-main-url").await.unwrap();
    let attempt = reg.attempts.get("ts1", "dropped-main-url").unwrap();
    attempt.bind_main(10, Some("identity-drop-token".into()));
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("dropped-main-url"),
            &|| main_snapshot(10),
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    sub.push(
        0,
        frame(
            "actual-generated-tag",
            "NeedsLogin",
            "https://headscale.example/active-auth",
        ),
    );
    wait_until(|| {
        reg.login_progress("ts1", "dropped-main-url")
            .is_some_and(|p| p.url.is_some())
    })
    .await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let receipt = reg.login_progress("ts1", "dropped-main-url").unwrap();
    assert_eq!(receipt.server_id, "ts1");
    assert_eq!(receipt.attempt_id, "dropped-main-url");
    assert_eq!(receipt.phase, "cancelled");
    assert!(receipt.url.is_none());
    assert_eq!(receipt.main_generation, Some(10));
    assert_eq!(
        receipt.identity_epoch.as_deref(),
        Some("identity-drop-token")
    );
    assert!(attempt.is_finished());
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    assert!(
        reg.main_owns("ts1", true),
        "future Drop cannot stop the normal main"
    );
    assert!(reg.inflight_login_pids().is_empty());
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
async fn main_observer_rejects_saved_identity_change_without_waiting_for_another_frame() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    let requested = ts_server("ts1", "myts");
    let saved = Arc::new(Mutex::new(requested.clone()));
    let emitter = Arc::new(FakeEmitter::default());
    reg.prepare("ts1", "identity-change").await.unwrap();
    let (reg2, ud2, saved2, emitter2) = (reg.clone(), ud.clone(), saved.clone(), emitter.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt_with_saved(
            &requested,
            &ud2,
            request("identity-change"),
            &|| Ok(saved2.lock().unwrap().clone()),
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
            "https://headscale.example/auth",
        ),
    );
    wait_until(|| {
        reg.login_progress("ts1", "identity-change")
            .is_some_and(|p| p.url.is_some())
    })
    .await;
    saved.lock().unwrap().tailscale_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::TailscaleSettings {
            hostname: Some("replacement".into()),
            ..Default::default()
        },
    ));
    let outcome = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(outcome, StartLoginOutcome::Failed(reason) if reason == "savedTailscaleIdentityChanged")
    );
    let receipt = reg.login_progress("ts1", "identity-change").unwrap();
    assert_eq!(receipt.phase, "failed");
    assert!(receipt.url.is_none());
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    assert!(reg.main_owns("ts1", true));
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn main_observer_rejects_generation_change_without_a_status_frame() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    let generation = Arc::new(AtomicU64::new(10));
    reg.prepare("ts1", "quiet-supersession").await.unwrap();
    let (reg2, ud2, generation2) = (reg.clone(), ud.clone(), generation.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("quiet-supersession"),
            &|| main_snapshot(generation2.load(Ordering::SeqCst)),
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    generation.store(11, Ordering::SeqCst);
    let outcome = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, StartLoginOutcome::Failed(reason) if reason == "mainCoreChanged"));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    assert!(reg.main_owns("ts1", true));
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn duplicate_main_attempt_cannot_create_another_observer_or_authorize() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    reg.prepare("ts1", "single-observer").await.unwrap();
    let (reg2, ud2) = (reg.clone(), ud.clone());
    let task = tokio::spawn(async move {
        reg2.start_attempt(
            &ts_server("ts1", "myts"),
            &ud2,
            request("single-observer"),
            &|| main_snapshot(10),
            Arc::new(FakeEmitter::default()),
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    let duplicate = reg
        .start_attempt(
            &ts_server("ts1", "myts"),
            &ud,
            request("single-observer"),
            &|| main_snapshot(10),
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(
        matches!(duplicate, StartLoginOutcome::Failed(reason) if reason == "attemptAlreadyUsed")
    );
    assert_eq!(sub.senders.lock().unwrap().len(), 1);
    reg.cancel_attempt("ts1", "single-observer").await.unwrap();
    assert!(matches!(task.await.unwrap(), StartLoginOutcome::Cancelled));
    assert!(reg.main_owns("ts1", true));
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn expired_running_main_cannot_authorize_and_invalid_needs_login_url_is_terminal() {
    for invalid in [false, true] {
        let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
        reg.prepare("ts1", "untrusted-main-frame").await.unwrap();
        let emitter = Arc::new(FakeEmitter::default());
        let (reg2, ud2, emitter2) = (reg.clone(), ud.clone(), emitter.clone());
        let task = tokio::spawn(async move {
            reg2.start_attempt(
                &ts_server("ts1", "myts"),
                &ud2,
                request("untrusted-main-frame"),
                &|| main_snapshot(10),
                emitter2,
            )
            .await
        });
        wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
        if invalid {
            sub.push(
                0,
                frame("actual-generated-tag", "NeedsLogin", "javascript:unsafe"),
            );
            assert!(
                matches!(task.await.unwrap(), StartLoginOutcome::Failed(reason) if reason == "invalidAuthUrl")
            );
        } else {
            let mut update = frame(
                "actual-generated-tag",
                "Running",
                "https://headscale.example/auth",
            );
            update.endpoints[0].self_ = Some(daemon::TailscalePeer {
                expired: true,
                ..Default::default()
            });
            sub.push(0, update);
            wait_until(|| {
                reg.login_progress("ts1", "untrusted-main-frame")
                    .is_some_and(|p| p.url.is_some())
            })
            .await;
            assert!(!task.is_finished());
            reg.cancel_attempt("ts1", "untrusted-main-frame")
                .await
                .unwrap();
            assert!(matches!(task.await.unwrap(), StartLoginOutcome::Cancelled));
        }
        assert!(emitter
            .progress
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.2 != "authorized"));
        assert!(sub.senders.lock().unwrap()[0].is_closed());
        assert!(reg.main_owns("ts1", true));
        std::fs::remove_dir_all(ud).unwrap();
    }
}

struct MockBoundMain(AtomicBool);
#[async_trait]
impl MainLoginBinding for MockBoundMain {
    async fn validate(&self) -> Result<(), String> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("READY_MAIN_UNKNOWN".into())
        }
    }
    fn target_tag(&self, _: &str) -> Option<&str> {
        Some("actual-generated-tag")
    }
}

#[tokio::test]
async fn main_observer_checks_bound_session_again_before_accepting_running_frame() {
    let (reg, sub, ud) = owned_main_registry(Duration::from_secs(60)).await;
    reg.prepare("ts1", "native-bound").await.unwrap();
    let attempt = reg.attempts.get("ts1", "native-bound").unwrap();
    attempt.claimed.store(true, Ordering::SeqCst);
    attempt.bind_main(10, Some("epoch-token".into()));
    let bound = Arc::new(MockBoundMain(AtomicBool::new(true)));
    let emitter = Arc::new(FakeEmitter::default());
    let (reg2, bound2, emitter2, attempt2) =
        (reg.clone(), bound.clone(), emitter.clone(), attempt.clone());
    let task = tokio::spawn(async move {
        let server = ts_server("ts1", "myts");
        reg2.confirm_main_request(
            &server,
            &request("native-bound"),
            &attempt2,
            &main_snapshot(10),
            &|| main_snapshot(10),
            &|| Ok(server.clone()),
            Some(bound2.as_ref()),
            Arc::new(AttemptReceiptEmitter {
                inner: emitter2,
                attempt: attempt2.clone(),
                attempt_id: "native-bound".into(),
            }),
        )
        .await
    });
    wait_until(|| !sub.senders.lock().unwrap().is_empty()).await;
    sub.push(
        0,
        frame(
            "actual-generated-tag",
            "NeedsLogin",
            "https://headscale.example/auth",
        ),
    );
    wait_until(|| {
        reg.login_progress("ts1", "native-bound")
            .is_some_and(|p| p.url.is_some())
    })
    .await;
    assert_eq!(
        reg.login_progress("ts1", "native-bound")
            .unwrap()
            .identity_epoch
            .as_deref(),
        Some("epoch-token")
    );
    bound.0.store(false, Ordering::SeqCst);
    sub.push(0, frame("actual-generated-tag", "Running", ""));
    assert!(
        matches!(task.await.unwrap(), StartLoginOutcome::Failed(reason) if reason == "READY_MAIN_UNKNOWN")
    );
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "authorized"));
    assert!(sub.senders.lock().unwrap()[0].is_closed());
    assert!(reg.main_owns("ts1", true));
    attempt.finish();
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
    let attempt_id = emitter.progress.lock().unwrap()[0].1.clone();
    let receipt = reg.login_progress("ts1", &attempt_id).unwrap();
    assert_eq!(receipt.phase, "authorized");
    assert_eq!(receipt.reason, None);
    assert!(serde_json::to_value(&receipt).unwrap().get("url").is_none());
    assert!(emitter
        .progress
        .lock()
        .unwrap()
        .iter()
        .all(|p| p.2 != "failed"));
    assert!(emitter.captured.lock().unwrap().is_empty());
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn claimed_retirement_keeper_is_the_original_arc_and_never_waits_on_itself() {
    let registry = LoginCoreRegistry::production();
    registry.prepare("target", "own").await.unwrap();
    registry.prepare("target", "other").await.unwrap();
    let own = registry.attempts.get("target", "own").unwrap();
    own.claimed.store(true, Ordering::SeqCst);
    let other = registry.attempts.get("target", "other").unwrap();
    let gate = registry.state_gate().await;
    tokio::time::timeout(
        Duration::from_secs(1),
        registry.retire_other_attempts_under_state_gate("target", &gate, &own),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!own.cancelled());
    assert!(!own.is_finished());
    assert!(other.cancelled());
    assert!(other.is_finished());
    assert!(registry.attempts.get("target", "other").is_err());
    let foreign = Attempts::default().prepare("target", "own").unwrap();
    foreign.claimed.store(true, Ordering::SeqCst);
    assert!(registry
        .retire_other_attempts_under_state_gate("target", &gate, &foreign)
        .await
        .is_err());
    own.cancel();
    assert!(registry
        .retire_other_attempts_under_state_gate("target", &gate, &own)
        .await
        .is_err());
    assert!(
        !own.is_finished(),
        "the internal keeper is not marked done by another owner"
    );
}

#[cfg(not(target_os = "ios"))]
struct CredentialFixture {
    proxy: Arc<ProxyRuntime>,
    mesh: Arc<crate::runtime::mesh::MeshRuntime>,
    directory: crate::test_support::TestDir,
    spawner: Arc<FakeSpawner>,
    saved: Value,
    gate: Arc<polaris_core_supervisor::LifecycleGate>,
}

#[cfg(not(target_os = "ios"))]
impl CredentialFixture {
    fn new(active: bool) -> Self {
        let spawner = fake_spawner(vec![], false, false);
        Self::with_dependencies(
            active,
            spawner.clone(),
            spawner,
            Arc::new(FakeChecker { ok: true }),
        )
    }
    fn with_dependencies(
        active: bool,
        spawner: Arc<FakeSpawner>,
        actual_spawner: Arc<dyn LoginCoreSpawner>,
        checker: Arc<dyn ConfigChecker>,
    ) -> Self {
        let registry = LoginCoreRegistry::with_deps(
            actual_spawner,
            checker,
            fake_subscriber(false),
            Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
            Duration::from_secs(60),
        );
        let (proxy, mesh, directory, gate) = ProxyRuntime::credential_fixture_for_test(registry);
        let manager = proxy.config_for_commit_test();
        let mut saved = manager.current().unwrap();
        saved["servers"] = json!([{
            "id":"ts1", "name":"Credential fixture", "protocol":"tailscale",
            "tailscaleSettings": {
                "sourceTag":"credential-tag", "unknown":"keep",
                "retainedAuthKey":{"authKey":"retained-test-key", "controlAuthority":"https://controlplane.tailscale.com"}
            }
        }]);
        if active {
            saved["servers"][0]["tailscaleSettings"]["authKey"] = json!("retained-test-key");
        }
        manager.save_full(&saved).unwrap();
        let saved = manager.current().unwrap();
        Self {
            proxy,
            mesh,
            directory,
            spawner,
            saved,
            gate,
        }
    }

    fn registry(&self) -> &LoginCoreRegistry {
        self.mesh.login_registry_for_test()
    }

    fn request(&self, id: &str, mode: LoginMode, reuse: bool) -> LoginRequest {
        LoginRequest {
            attempt_id: id.into(),
            mode,
            replace_identity: true,
            reuse_retained_auth_key: reuse,
            expected_credential_revision:
                polaris_config_engine::user_config::effective_view::tailscale_credential_revision(
                    &self.saved["servers"][0],
                ),
        }
    }

    async fn start(&self, request: LoginRequest) -> StartLoginOutcome {
        let mut candidate = self.saved["servers"][0].clone();
        let settings = candidate["tailscaleSettings"].as_object_mut().unwrap();
        settings.remove("retainedAuthKey");
        settings.remove("authKey");
        let (candidate, backend_owned) =
            resolve_tailscale_credential_candidate(&self.saved, &candidate, &request).unwrap();
        assert!(backend_owned);
        let requested: ServerConfig = serde_json::from_value(candidate.clone()).unwrap();
        self.registry()
            .prepare("ts1", &request.attempt_id)
            .await
            .unwrap();
        let generation = self.proxy.core_generation();
        self.registry()
            .start_attempt_with_normal_main(
                &requested,
                &self.directory,
                request,
                &|| {
                    serde_json::from_value(
                        self.proxy.config_for_commit_test().current().unwrap()["servers"][0]
                            .clone(),
                    )
                    .map_err(|_| "fixture saved node invalid".into())
                },
                &|| MainLoginSnapshot {
                    generation,
                    ..Default::default()
                },
                Arc::new(FakeEmitter::default()),
                &self.proxy,
                &self.saved,
                None,
                &candidate,
                generation,
            )
            .await
    }
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn retained_key_reuse_uses_original_gate_saved_cas_and_single_mock_producer() {
    let fixture = CredentialFixture::new(false);
    assert!(matches!(
        fixture
            .start(fixture.request("reuse", LoginMode::Authkey, true))
            .await,
        StartLoginOutcome::Started
    ));
    assert_eq!(fixture.spawner.count.load(Ordering::SeqCst), 1);
    let saved = fixture.proxy.config_for_commit_test().current().unwrap();
    assert_eq!(
        saved["servers"][0]["tailscaleSettings"]["authKey"],
        "retained-test-key"
    );
    assert_eq!(saved["servers"][0]["tailscaleSettings"]["unknown"], "keep");
    assert_eq!(
        saved["servers"][0]["tailscaleSettings"]["sourceTag"],
        "credential-tag"
    );
    let generated: Value =
        serde_json::from_slice(&std::fs::read(sole_login_config(&fixture.directory)).unwrap())
            .unwrap();
    let emitted = generated.to_string();
    assert!(
        emitted.contains("retained-test-key"),
        "explicit key request reaches the original endpoint builder"
    );
    assert!(!emitted.contains("retainedAuthKey"));
    assert!(!emitted.contains("tailscaleCredentialRevision"));
    fixture
        .registry()
        .cancel_attempt("ts1", "reuse")
        .await
        .unwrap();
    let cancelled = fixture.proxy.config_for_commit_test().current().unwrap();
    assert!(cancelled["servers"][0]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert_eq!(
        cancelled["servers"][0]["tailscaleSettings"]["retainedAuthKey"]["authKey"],
        "retained-test-key"
    );
    assert!(fixture.spawner.spawned.lock().unwrap()[0]
        .terminated
        .load(Ordering::SeqCst));
    assert!(login_configs(&fixture.directory).is_empty());
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn explicit_browser_with_active_revision_parks_key_before_original_birth() {
    let fixture = CredentialFixture::new(true);
    assert!(matches!(
        fixture
            .start(fixture.request("browser", LoginMode::Browser, false))
            .await,
        StartLoginOutcome::Started
    ));
    let saved = fixture.proxy.config_for_commit_test().current().unwrap();
    assert!(saved["servers"][0]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert_eq!(
        saved["servers"][0]["tailscaleSettings"]["retainedAuthKey"]["authKey"],
        "retained-test-key"
    );
    let emitted = std::fs::read_to_string(sole_login_config(&fixture.directory)).unwrap();
    assert!(!emitted.contains("retained-test-key"));
    assert!(!emitted.contains("retainedAuthKey"));
    fixture
        .registry()
        .cancel_attempt("ts1", "browser")
        .await
        .unwrap();
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn done_flag_during_real_close_cannot_compensate_or_touch_successor() {
    let fixture = Arc::new(CredentialFixture::new(false));
    assert!(matches!(
        fixture
            .start(fixture.request("old", LoginMode::Authkey, true))
            .await,
        StartLoginOutcome::Started
    ));
    let before = fixture.proxy.config_for_commit_test().current().unwrap();
    let original = fixture.registry().attempts.get("ts1", "old").unwrap();
    let child = fixture.spawner.spawned.lock().unwrap()[0].clone();
    let close = Arc::new(tokio::sync::Notify::new());
    *child.close_gate.lock().unwrap() = Some(close.clone());
    let cancelling = {
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.registry().cancel_attempt("ts1", "old").await })
    };
    wait_until(|| child.close_started.load(Ordering::SeqCst) > 0).await;
    // The original guard may have finished presentation before its producer is reaped.
    // That flag supplies no terminal evidence and cannot write the saved credential.
    original.finish();
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        before
    );
    fixture
        .registry()
        .prepare("ts1", "successor")
        .await
        .unwrap();
    close.notify_one();
    cancelling.await.unwrap().unwrap();
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        before,
        "even the same key cannot compensate across a new original registry row"
    );
    assert!(!fixture
        .registry()
        .attempts
        .get("ts1", "successor")
        .unwrap()
        .cancelled());
    fixture
        .registry()
        .cancel_attempt("ts1", "successor")
        .await
        .unwrap();
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn exact_closed_child_cannot_compensate_changed_saved_target() {
    let fixture = Arc::new(CredentialFixture::new(false));
    assert!(matches!(
        fixture
            .start(fixture.request("old", LoginMode::Authkey, true))
            .await,
        StartLoginOutcome::Started
    ));
    let child = fixture.spawner.spawned.lock().unwrap()[0].clone();
    let close = Arc::new(tokio::sync::Notify::new());
    *child.close_gate.lock().unwrap() = Some(close.clone());
    let cancelling = {
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.registry().cancel_attempt("ts1", "old").await })
    };
    wait_until(|| child.close_started.load(Ordering::SeqCst) > 0).await;
    let mut edited = fixture.proxy.config_for_commit_test().current().unwrap();
    edited["servers"][0]["name"] = json!("new saved target revision");
    fixture
        .proxy
        .config_for_commit_test()
        .save_full(&edited)
        .unwrap();
    let edited = fixture.proxy.config_for_commit_test().current().unwrap();
    close.notify_one();
    assert_eq!(
        cancelling.await.unwrap().unwrap_err(),
        "credentialRevisionChanged"
    );
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        edited
    );
    assert!(child.terminated.load(Ordering::SeqCst));
}

#[test]
fn explicit_credential_resolution_never_trusts_metadata_or_changes_issuer_on_reuse() {
    let saved = json!({"servers":[{"id":"ts1", "protocol":"tailscale", "tailscaleSettings":{
        "retainedAuthKey":{"authKey":"secret-fixture", "controlAuthority":"https://controlplane.tailscale.com"}
    }}]});
    let mut request = request("reuse");
    request.mode = LoginMode::Authkey;
    request.reuse_retained_auth_key = true;
    request.expected_credential_revision =
        polaris_config_engine::user_config::effective_view::tailscale_credential_revision(
            &saved["servers"][0],
        );
    let mut candidate = json!({"id":"ts1", "protocol":"tailscale", "tailscaleSettings":{}});
    let (resolved, owned) =
        resolve_tailscale_credential_candidate(&saved, &candidate, &request).unwrap();
    assert!(owned);
    assert_eq!(resolved["tailscaleSettings"]["authKey"], "secret-fixture");
    candidate["tailscaleSettings"]["controlUrl"] = json!("https://controlplane.tailscale.com/");
    assert_eq!(
        resolve_tailscale_credential_candidate(&saved, &candidate, &request).unwrap_err(),
        "retainedAuthKeyAuthorityChanged"
    );
    candidate["tailscaleSettings"]["controlUrl"] = json!("https://controlplane.tailscale.com");
    candidate["tailscaleSettings"]["authKey"] = json!("unrelated-new-key");
    assert_eq!(
        resolve_tailscale_credential_candidate(&saved, &candidate, &request).unwrap_err(),
        "invalidCredentialIntent"
    );
    candidate["tailscaleSettings"]
        .as_object_mut()
        .unwrap()
        .remove("authKey");
    request.expected_credential_revision = Some("stale-revision".into());
    assert_eq!(
        resolve_tailscale_credential_candidate(&saved, &candidate, &request).unwrap_err(),
        "credentialRevisionChanged"
    );
    request.expected_credential_revision = None;
    candidate["tailscaleSettings"]["retainedAuthKeyAvailable"] = json!(true);
    assert_eq!(
        resolve_tailscale_credential_candidate(&saved, &candidate, &request).unwrap_err(),
        "credentialRevisionChanged"
    );
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn logout_parks_actual_key_and_retires_only_auth_preserving_taildrop_and_history() {
    let fixture = CredentialFixture::new(true);
    let state = fixture.mesh.tailscale_state_dir("ts1").unwrap();
    std::fs::create_dir_all(state.join("Taildrop")).unwrap();
    std::fs::write(state.join("Taildrop/user-file"), b"user-content").unwrap();
    let sealed = polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    );
    std::fs::write(state.join("tailscaled.state"), &sealed).unwrap();
    let retired = polaris_mesh::tailscale_state::project_tailscale_auth_state(
        &sealed,
        polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences,
    )
    .unwrap();
    assert!(fixture
        .mesh
        .logout_tailscale_with_credentials(
            "ts1",
            &fixture.proxy,
            &fixture.saved,
            fixture.proxy.core_generation(),
            None
        )
        .await
        .unwrap());
    let saved = fixture.proxy.config_for_commit_test().current().unwrap();
    assert!(saved["servers"][0]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert_eq!(
        saved["servers"][0]["tailscaleSettings"]["retainedAuthKey"]["authKey"],
        "retained-test-key"
    );
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        retired.bytes
    );
    assert_eq!(
        std::fs::read(state.join("Taildrop/user-file")).unwrap(),
        b"user-content"
    );
    assert_eq!(
        fixture.spawner.count.load(Ordering::SeqCst),
        0,
        "standalone PC logout does not birth a producer"
    );
    // Repeating Logout still passes the real gate and exact saved CAS; the strict
    // retired FileStore no-op is not presented as SDK Bound or NoOwner.
    assert!(fixture
        .mesh
        .logout_tailscale_with_credentials(
            "ts1",
            &fixture.proxy,
            &saved,
            fixture.proxy.core_generation(),
            None
        )
        .await
        .unwrap());
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        retired.bytes
    );
    assert!(
        matches!(
            fixture
                .start(fixture.request("reopen", LoginMode::Authkey, true))
                .await,
            StartLoginOutcome::Failed(_)
        ),
        "a stale pre-logout saved document cannot relogin"
    );
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn logout_rejects_main_owner_and_foreign_gate_without_credential_or_file_mutation() {
    let fixture = CredentialFixture::new(true);
    let gate = fixture.registry().state_gate().await;
    let generated = json!({"endpoints":[{"type":"tailscale", "tag":"actual-main",
        "state_directory":fixture.directory.join("tailscale/ts1")}]});
    let token =
        claim_main_for_test(fixture.registry(), &generated, &fixture.directory, &gate).await;
    drop(gate);
    assert!(!fixture
        .mesh
        .logout_tailscale_with_credentials(
            "ts1",
            &fixture.proxy,
            &fixture.saved,
            fixture.proxy.core_generation(),
            None
        )
        .await
        .unwrap());
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        fixture.saved
    );
    let foreign = LoginCoreRegistry::production();
    let foreign_gate = foreign.state_gate().await;
    assert!(fixture
        .mesh
        .tailscale_retire_pc_auth_under_gate("ts1", &foreign_gate, None)
        .is_err());
    let gate = fixture.registry().state_gate().await;
    assert!(fixture
        .registry()
        .release_main_states_if_token(&token, &gate)
        .unwrap());
}

#[test]
fn ios_credential_logout_binds_cold_saved_no_key_doc_and_hot_never_falls_back_to_start() {
    let source = crate::test_support::crate_source("runtime/tailscale_login_core.rs");
    let logout = source
        .split("pub(crate) async fn logout_with_normal_main(")
        .nth(1)
        .unwrap()
        .split("pub(crate) async fn retire_other_attempts_under_state_gate")
        .next()
        .unwrap();
    let cold = logout
        .split("Ok(None) =>")
        .nth(1)
        .unwrap()
        .split("Err(error) =>")
        .next()
        .unwrap();
    let park = cold.find("park_saved_tailscale_key").unwrap();
    let bind = cold.find("ActionBinding::new").unwrap();
    let start = cold.find("await_normal_main").unwrap();
    assert!(park < bind && bind < start);
    assert!(cold.contains("&saved"));
    assert!(cold.contains("for_attempt(generation, Arc::clone(&attempt))"));
    assert!(cold.contains("Some(&attempt)"));
    assert!(logout.contains("live_tailscale_main(binding.clone())"));
    assert!(
        logout.find("live_tailscale_main(binding.clone())").unwrap()
            < logout.find("prepare_tailscale_action_origin").unwrap()
    );
    assert!(cold.contains("prepare_tailscale_action_origin(generation, &attempt)"));
    let prerequisite = crate::test_support::crate_source("runtime/proxy/prerequisite.rs");
    let hot = prerequisite
        .split("pub(crate) async fn live_tailscale_main(")
        .nth(1)
        .unwrap()
        .split("pub(crate) async fn retire_tailscale_account")
        .next()
        .unwrap();
    assert!(!hot.contains("await_normal_main"));
    assert!(hot.contains("ready_main_for_generation"));
    assert!(hot.contains("validate_ready_main"));
    assert!(hot.contains("with_current_generation(generation"));
    assert!(hot.contains("LocalStart(_)"));
    assert!(hot.contains("owner.ready.is_none() || !committed"));
    assert!(hot.contains("check_tailscale_logout_cold(running, local_pending)"));
    let command = crate::test_support::crate_source("commands/server.rs");
    assert!(command.contains("(cfg!(target_os = \"ios\") || credential_transaction)"));
    assert!(command.contains("&& !request.replace_identity"));
    assert!(command.contains("TAILSCALE_IDENTITY_RETIREMENT_REQUIRED"));
}

#[test]
fn parked_record_is_not_transient_builder_input_without_explicit_resolution() {
    let server: ServerConfig = serde_json::from_value(json!({"id":"ts1", "name":"Parked",
        "protocol":"tailscale", "tailscaleSettings":{"retainedAuthKey":{
            "authKey":"parked-transient-sentinel", "controlAuthority":"https://controlplane.tailscale.com"}}})).unwrap();
    let config = build_tailscale_login_config(
        &server,
        Path::new("/fake/config"),
        &TailscaleLoginApiService {
            port: 9911,
            secret: "api-fixture".into(),
        },
    )
    .unwrap();
    let emitted = login_config_to_json(&config).to_string();
    assert!(!emitted.contains("parked-transient-sentinel"));
    assert!(!emitted.contains("retainedAuthKey"));
    assert!(!emitted.contains("auth_key"));
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn default_browser_parks_key_without_retiring_existing_sdk_identity() {
    let fixture = CredentialFixture::new(true);
    let state = fixture.mesh.tailscale_state_dir("ts1").unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let original = polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    );
    let mut original: Value = serde_json::from_slice(&original).unwrap();
    // The shared projection fixture omits account Config. Reuse the existing
    // cached-session fixture's real prefs shape rather than weakening Unknown.
    original["profile-a123"] = json!("eyJXYW50UnVubmluZyI6ZmFsc2UsIkxvZ2dlZE91dCI6ZmFsc2UsIkNvbmZpZyI6eyJOb2RlSUQiOiJuLWZpeHR1cmUiLCJVc2VyUHJvZmlsZSI6eyJMb2dpbk5hbWUiOiJmaXh0dXJlQGV4YW1wbGUuaW52YWxpZCJ9fX0=");
    let original = serde_json::to_vec(&original).unwrap();
    std::fs::write(state.join("tailscaled.state"), &original).unwrap();
    assert!(
        polaris_mesh::tailscale_state::cached_session_exists_for_presentation(&original).unwrap()
    );
    let mut request = fixture.request("browser-default", LoginMode::Browser, false);
    request.replace_identity = false;
    assert!(matches!(
        fixture.start(request).await,
        StartLoginOutcome::Started
    ));
    let actual = fixture.proxy.config_for_commit_test().current().unwrap();
    assert!(actual["servers"][0]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert_eq!(
        actual["servers"][0]["tailscaleSettings"]["retainedAuthKey"]["authKey"],
        "retained-test-key"
    );
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        original
    );
    assert!(
        !std::fs::read_to_string(sole_login_config(&fixture.directory))
            .unwrap()
            .contains("retained-test-key")
    );
    fixture
        .registry()
        .cancel_attempt("ts1", "browser-default")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        original
    );
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn authkey_without_cached_profile_uses_existing_machine_file_without_auth_projection() {
    let fixture = CredentialFixture::new(false);
    let state = fixture.mesh.tailscale_state_dir("ts1").unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let machine = br#"{"_machinekey":"bWFjaGluZS1maXh0dXJl"}"#;
    std::fs::write(state.join("tailscaled.state"), machine).unwrap();
    assert!(
        !polaris_mesh::tailscale_state::cached_session_exists_for_presentation(machine).unwrap()
    );
    assert!(polaris_mesh::tailscale_state::project_tailscale_auth_state(
        machine,
        polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences
    )
    .is_err());
    let mut request = fixture.request("no-profile", LoginMode::Authkey, true);
    request.replace_identity = false;
    assert!(matches!(
        fixture.start(request).await,
        StartLoginOutcome::Started
    ));
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        machine
    );
    fixture
        .registry()
        .cancel_attempt("ts1", "no-profile")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        machine
    );
}

#[cfg(not(target_os = "ios"))]
struct ReleasableCredentialChecker {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
#[async_trait]
#[cfg(not(target_os = "ios"))]
impl ConfigChecker for ReleasableCredentialChecker {
    async fn check(&self, _: &Path, _: &Path) -> Result<(), String> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(())
    }
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn stop_after_credential_cas_before_first_poll_cannot_birth_original_factory() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let spawner = fake_spawner(vec![], false, false);
    let fixture = Arc::new(CredentialFixture::with_dependencies(
        false,
        spawner.clone(),
        spawner,
        Arc::new(ReleasableCredentialChecker {
            entered: entered.clone(),
            release: release.clone(),
        }),
    ));
    let start = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .start(fixture.request("generation-before", LoginMode::Authkey, true))
                .await
        })
    };
    acquire(&entered).await;
    assert_eq!(fixture.spawner.count.load(Ordering::SeqCst), 0);
    let original_generation = fixture.proxy.core_generation();
    fixture
        .gate
        .claim_generation(
            Some(original_generation),
            polaris_core_supervisor::LifecycleKind::Stop,
        )
        .unwrap();
    release.add_permits(1);
    let outcome = start.await.unwrap();
    let spawned = fixture.spawner.count.load(Ordering::SeqCst);
    // The RED fixture always reaps an unexpected child before its assertion.
    if spawned != 0 {
        let _ = fixture
            .registry()
            .cancel_attempt("ts1", "generation-before")
            .await;
        assert!(fixture.spawner.spawned.lock().unwrap()[0]
            .terminated
            .load(Ordering::SeqCst));
    }
    assert!(
        matches!(outcome, StartLoginOutcome::Cancelled),
        "a newer Stop must invalidate the original G0 admission"
    );
    assert_eq!(spawned, 0);
    assert!(login_configs(&fixture.directory).is_empty());
}

#[cfg(not(target_os = "ios"))]
struct PendingCredentialSpawner {
    base: Arc<FakeSpawner>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
#[async_trait]
#[cfg(not(target_os = "ios"))]
impl LoginCoreSpawner for PendingCredentialSpawner {
    async fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        let child = self.base.spawn(request).await?;
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(child)
    }
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn stop_after_pending_birth_joins_exact_child_and_failed_close_retains_custody() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let base = fake_spawner(vec![], false, false);
    let fixture = Arc::new(CredentialFixture::with_dependencies(
        false,
        base.clone(),
        Arc::new(PendingCredentialSpawner {
            base,
            entered: entered.clone(),
            release: release.clone(),
        }),
        Arc::new(FakeChecker { ok: true }),
    ));
    let start = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .start(fixture.request("pending-generation", LoginMode::Authkey, true))
                .await
        })
    };
    acquire(&entered).await;
    let child = fixture.spawner.spawned.lock().unwrap()[0].clone();
    let close = Arc::new(tokio::sync::Notify::new());
    *child.close_gate.lock().unwrap() = Some(close.clone());
    child.close_failures.store(1, Ordering::SeqCst);
    let before = fixture.proxy.config_for_commit_test().current().unwrap();
    let original = fixture.proxy.core_generation();
    fixture
        .gate
        .claim_generation(Some(original), polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    assert!(
        !start.is_finished(),
        "an admitted Pending producer must be joined"
    );
    assert!(!child.terminated.load(Ordering::SeqCst));
    release.add_permits(1);
    wait_until(|| child.close_started.load(Ordering::SeqCst) > 0).await;
    assert!(fixture.registry().shared.contains("ts1"));
    assert_eq!(fixture.spawner.count.load(Ordering::SeqCst), 1);
    assert!(
        !start.is_finished(),
        "cancel waits for the exact returned child"
    );
    assert!(!login_configs(&fixture.directory).is_empty());
    close.notify_one();
    wait_until(|| {
        fixture
            .registry()
            .shared
            .guard()
            .get("ts1")
            .is_some_and(|entry| matches!(&*entry.closed_rx.borrow(), Some(Err(_))))
    })
    .await;
    // launch still holds the original start gate while awaiting ready. Observe
    // the exact entry's failed close receipt without trying to acquire that gate.
    assert!(fixture
        .registry()
        .shared
        .guard()
        .get("ts1")
        .is_some_and(|entry| {
            entry.attempt_id == "pending-generation"
                && matches!(&*entry.closed_rx.borrow(), Some(Err(_)))
        }));
    assert!(fixture.registry().shared.contains("ts1"));
    assert!(!start.is_finished());
    assert!(!child.terminated.load(Ordering::SeqCst));
    assert!(!login_configs(&fixture.directory).is_empty());
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        before
    );
    let retry = {
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.registry().cancel_login("ts1").await })
    };
    wait_until(|| child.close_started.load(Ordering::SeqCst) > 1).await;
    assert!(!retry.is_finished());
    close.notify_one();
    assert!(retry.await.unwrap().unwrap());
    assert!(matches!(start.await.unwrap(), StartLoginOutcome::Cancelled));
    assert!(child.terminated.load(Ordering::SeqCst));
    assert!(!fixture.registry().shared.contains("ts1"));
    assert!(login_configs(&fixture.directory).is_empty());
    assert_eq!(
        fixture.proxy.config_for_commit_test().current().unwrap(),
        before
    );
}

#[test]
fn unscoped_android_close_ack_cannot_admit_new_credential_transaction_or_compensation() {
    let source = crate::test_support::crate_source("runtime/tailscale_login_core.rs");
    let activation = source
        .split("async fn activate_android_credential(")
        .nth(1)
        .unwrap()
        .split("async fn compensate_android_credential(")
        .next()
        .unwrap();
    assert!(
        activation.find("close_android_tailscale_origin").unwrap()
            < activation.find("with_android_target_action").unwrap()
    );
    assert!(
        activation.find("retired.retired_export()").unwrap()
            < activation.find("commit_tailscale_credential").unwrap()
    );
    assert!(
        activation.find("selected_android_auth_node").unwrap()
            < activation.find("retire_android_tailscale_auth").unwrap()
    );
    assert!(activation.contains("if request.replace_identity"));
    let cancel = source
        .split("async fn compensate_android_credential(")
        .nth(1)
        .unwrap()
        .split("async fn logout_with_android_store(")
        .next()
        .unwrap();
    assert!(
        cancel.find("read_login_retirement").unwrap()
            < cancel.find("commit_tailscale_credential").unwrap()
    );
    assert!(cancel.contains("original_credential_row"));
    assert!(cancel.contains("assert_auth_state_available"));
    assert!(cancel.contains("with_android_target_action"));
    assert!(cancel.contains("activation.generation"));
    let reservation = source
        .split("async fn with_android_target_action<T>(")
        .nth(1)
        .unwrap()
        .split("async fn close_android_tailscale_origin(")
        .next()
        .unwrap();
    assert!(
        reservation.find("reserve_android_action").unwrap()
            < reservation.find("target_action(").unwrap()
    );
    assert!(reservation.contains("complete_scoped_action"));
    assert!(reservation.contains("finish_android_action"));
    let origin = source
        .split("async fn close_android_tailscale_origin(")
        .nth(1)
        .unwrap()
        .split("async fn activate_android_credential(")
        .next()
        .unwrap();
    assert!(origin.find("observe_login").unwrap() < origin.find("cancel_matching_login").unwrap());
    let hot = origin
        .split("let original = tailscale_store::observe_login(&metadata.0, &metadata.1)")
        .nth(1)
        .unwrap();
    assert!(
        hot.find("cancel_matching_login").unwrap() < hot.find("read_login_retirement").unwrap()
    );
    let mesh = crate::test_support::crate_source("runtime/mesh.rs");
    let logout = mesh
        .split("pub(crate) async fn logout_tailscale_with_credentials(")
        .nth(1)
        .unwrap()
        .split("pub(crate) async fn logout_tailscale_with_normal_main")
        .next()
        .unwrap();
    assert!(logout.find("logout_with_android_store").unwrap() < logout.find(".logout(").unwrap());
    assert!(logout.contains("#[cfg(target_os = \"android\")]"));
}

#[tokio::test]
async fn scoped_action_finishes_begin_decode_error_without_committing() {
    let committed = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let result = complete_scoped_action(
        async { Err::<(), _>("nativeRetirementUnknown".into()) },
        || {
            committed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        || async {
            finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert_eq!(result, Err("nativeRetirementUnknown".into()));
    assert_eq!(committed.load(Ordering::SeqCst), 0);
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn scoped_action_unknown_finish_overrides_commit_and_commit_error_still_finishes() {
    let committed = AtomicUsize::new(0);
    let result = complete_scoped_action(
        async { Ok(()) },
        || {
            committed.fetch_add(1, Ordering::SeqCst);
            Ok("written")
        },
        || async { Err("nativeRetirementUnknown".into()) },
    )
    .await;
    assert_eq!(result, Err("nativeRetirementUnknown".into()));
    assert_eq!(committed.load(Ordering::SeqCst), 1);
    let finished = AtomicUsize::new(0);
    let result = complete_scoped_action(
        async { Ok(()) },
        || Err::<(), _>("cancelled".into()),
        || async {
            finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert_eq!(result, Err("cancelled".into()));
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[cfg(not(target_os = "android"))]
#[test]
fn unresolved_native_action_survives_done_and_late_finish_cannot_release_successor() {
    use crate::runtime::tailscale_login_core::attempts::{
        AndroidActionActivity, AndroidTargetAction,
    };
    let attempts = Attempts::default();
    let original = attempts.prepare("node", "original").unwrap();
    let token = Arc::new(AndroidTargetAction {
        state_file: "/owned/tailscale/node/tailscaled.state".into(),
        action_id: "original-action".into(),
        active: AtomicBool::new(true),
    });
    original.reserve_android_action(token.clone()).unwrap();
    assert_eq!(
        original.android_action().unwrap().unwrap().state_file,
        token.state_file
    );
    assert_eq!(
        original.android_action().unwrap().unwrap().action_id,
        "original-action"
    );
    assert!(original.reserve_android_action(token.clone()).is_err());
    original.finish();
    assert!(attempts.owns_state("node"));
    assert!(attempts.local_owner_in_use("node").unwrap());
    assert!(attempts.owns_state_except("node", None).unwrap());
    assert!(!attempts.owns_state_except("node", Some(&original)).unwrap());
    drop(AndroidActionActivity(token.clone()));
    assert!(!token.active.load(Ordering::SeqCst));
    assert!(original.android_action().unwrap().is_some());
    for index in 0..512 {
        attempts.prepare("other", &format!("other-{index}")).ok();
    }
    assert!(attempts.original_credential_row("original", &original));
    original.release_android_action(&token).unwrap();
    let successor = Arc::new(AndroidTargetAction {
        state_file: token.state_file.clone(),
        action_id: "successor-action".into(),
        active: AtomicBool::new(true),
    });
    original.reserve_android_action(successor.clone()).unwrap();
    assert!(original.release_android_action(&token).is_err());
    assert!(Arc::ptr_eq(
        &original.android_action().unwrap().unwrap(),
        &successor
    ));
    original.release_android_action(&successor).unwrap();
    assert!(!attempts.owns_state("node"));
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[tokio::test]
async fn committed_credential_cas_and_park_receipts_survive_unknown_finish() {
    use crate::runtime::tailscale_login_core::attempts::AndroidTargetAction;
    let fixture = CredentialFixture::new(false);
    fixture.registry().prepare("ts1", "same-cas").await.unwrap();
    let attempt = fixture
        .registry()
        .prepared_attempt_for_test("ts1", "same-cas");
    let token = Arc::new(AndroidTargetAction {
        state_file: "/owned/tailscale/ts1/tailscaled.state".into(),
        action_id: "same-cas-action".into(),
        active: AtomicBool::new(true),
    });
    attempt.reserve_android_action(token.clone()).unwrap();
    let generation = fixture.proxy.core_generation();
    let result = complete_scoped_action(
        async { Ok(()) },
        || {
            let actual = fixture.proxy.commit_tailscale_credential(
                &fixture.saved,
                "ts1",
                generation,
                Some(&attempt),
                |current| {
                    current["servers"][0]["tailscaleSettings"]["authKey"] =
                        json!("retained-test-key");
                    Ok(())
                },
            )?;
            attempt.record_credential_activation(&fixture.proxy, actual, generation);
            Ok(())
        },
        || async { Err("nativeRetirementUnknown".into()) },
    )
    .await;
    assert_eq!(result, Err("nativeRetirementUnknown".into()));
    let activation = attempt.credential_activation().unwrap();
    assert_eq!(
        activation.saved,
        fixture.proxy.config_for_commit_test().current().unwrap()
    );
    assert_eq!(activation.generation, generation);
    assert_eq!(
        activation.saved["servers"][0]["tailscaleSettings"]["authKey"],
        "retained-test-key"
    );
    attempt.finish();
    assert!(fixture.registry().attempts.owns_state("ts1"));
    assert!(Arc::ptr_eq(
        &attempt.android_action().unwrap().unwrap(),
        &token
    ));
    let result = complete_scoped_action(
        async { Ok(()) },
        || {
            let parked = fixture.proxy.commit_tailscale_credential(
                &activation.saved,
                "ts1",
                generation,
                None,
                |current| park_saved_tailscale_key(current, "ts1"),
            )?;
            attempt.record_credential_activation(&fixture.proxy, parked, generation);
            Ok(())
        },
        || async { Err("nativeRetirementUnknown".into()) },
    )
    .await;
    assert_eq!(result, Err("nativeRetirementUnknown".into()));
    let parked = attempt.credential_activation().unwrap();
    assert_eq!(
        parked.saved,
        fixture.proxy.config_for_commit_test().current().unwrap()
    );
    assert!(parked.saved["servers"][0]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert!(fixture.registry().attempts.owns_state("ts1"));
    assert!(fixture
        .proxy
        .commit_tailscale_credential(&activation.saved, "ts1", generation, None, |current| {
            park_saved_tailscale_key(current, "ts1")
        })
        .is_err());
    let source = crate::test_support::crate_source("runtime/tailscale_login_core.rs");
    let activate = source
        .split("async fn activate_android_credential(")
        .nth(1)
        .unwrap()
        .split("async fn compensate_android_credential(")
        .next()
        .unwrap();
    assert!(
        activate.find("commit_tailscale_credential").unwrap()
            < activate.find("record_credential_activation").unwrap()
    );
    assert!(
        activate.find("record_credential_activation").unwrap() < activate.rfind(".await?").unwrap()
    );
    let compensate = source
        .split("async fn compensate_android_credential(")
        .nth(1)
        .unwrap()
        .split("async fn logout_with_android_store(")
        .next()
        .unwrap();
    assert!(compensate.contains("\"Main\" => tailscale_store::read_main_retirement"));
    assert!(compensate.contains("\"Login\" => tailscale_store::read_login_retirement"));
    assert!(
        compensate.find("record_credential_activation").unwrap()
            < compensate.rfind(".await").unwrap()
    );
    assert_eq!(fixture.spawner.count.load(Ordering::SeqCst), 0);
}

#[test]
fn android_cold_uses_one_original_validation_and_exact_config_stem_under_held_warm_action() {
    let source = crate::test_support::crate_source("runtime/tailscale_login_core.rs");
    let launch = source
        .split("async fn launch_attempt(")
        .nth(1)
        .unwrap()
        .split("async fn signal_and_wait_close(")
        .next()
        .unwrap();
    let warm = launch
        .split("if warm {")
        .last()
        .unwrap()
        .split("if !warm {")
        .next()
        .unwrap();
    assert_eq!(warm.matches("check_config_for_tailscale(").count(), 1);
    assert!(!warm.contains("self.checker"));
    assert!(warm.contains("config_path.file_stem()"));
    assert!(warm.contains("tuple.config_digest() == polaris_updater::sha256_hex(&bytes)"));
    assert!(warm.contains("tuple.logical_instance_id() == logical_id"));
    assert!(warm.find("make_warm_tuple").unwrap() < warm.find("reserve_android_action").unwrap());
    assert!(warm.find("reserve_android_action").unwrap() < warm.find("begin_warm").unwrap());
    assert!(warm.contains("with_tailscale_credential_birth(generation, attempt"));
    assert!(launch.find("begin_warm").unwrap() < launch.find("self.spawner.spawn(req)").unwrap());
    assert_eq!(launch.matches("self.checker.check_for_spawn(").count(), 1);
    assert!(!launch.contains("spawn_with_android"));
    assert!(!launch.contains("Uuid"));
    let factory = source
        .split("impl LoginCoreSpawner for AndroidLoginCoreSpawner {")
        .nth(1)
        .unwrap()
        .split("impl LoginCoreChild for AndroidLoginCoreChild {")
        .next()
        .unwrap();
    assert!(factory.contains(".file_stem()"));
    assert_eq!(factory.matches("start_transient_login(").count(), 1);
    let browser = launch
        .split("if request.mode == LoginMode::Browser {")
        .nth(1)
        .unwrap()
        .split("// (b)")
        .next()
        .unwrap();
    assert!(browser.contains("ts.auth_key = None"));
    let close = source
        .split("async fn close_android_tailscale_origin(")
        .nth(1)
        .unwrap()
        .split("async fn activate_android_credential(")
        .next()
        .unwrap();
    assert!(close.contains("mode: LoginMode::Browser"));
    assert!(close.contains("Some(&mut activity)"));
    assert!(close.contains("reservation.held_retirement()?"));
    assert!(close.contains("attempt.record_android_store(retired.clone())?"));
    assert!(close.contains("let original = attempt.android_store()?"));
    assert!(
        close.contains("original.original().logical_instance_id != tuple.logical_instance_id()")
    );
    assert!(close.contains("retired.original() != held_retired.original()"));
    assert!(!close.contains("start_transient_login("));
    let action = source
        .split("async fn with_android_target_action<T>(")
        .nth(1)
        .unwrap()
        .split("async fn close_android_tailscale_origin(")
        .next()
        .unwrap();
    assert!(action.contains("AndroidActionOrigin::Warm(tuple)"));
    assert!(
        action.find("held_retirement").unwrap()
            < action.find("return complete_scoped_action").unwrap()
    );
    assert!(action.contains("retired.original() != original.original()"));
    assert!(
        action.find("return complete_scoped_action").unwrap()
            < action.find("target_action(").unwrap()
    );
    let finish = source
        .split("async fn finish_android_action(")
        .nth(1)
        .unwrap()
        .split("async fn with_android_target_action<T>(")
        .next()
        .unwrap();
    assert!(finish.contains("finish_warm(tuple).await.is_err()"));
    assert!(finish.contains("close_warm(tuple"));
    assert!(finish.rfind("finish_warm").unwrap() < finish.find("release_android_action").unwrap());
}

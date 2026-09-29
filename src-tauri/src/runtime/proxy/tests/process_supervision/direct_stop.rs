use super::*;
use std::io;
use std::process::ExitStatus;
use std::sync::atomic::AtomicUsize;

struct ReservedDirectFixture {
    rt: Arc<ProxyRuntime>,
    dir: crate::test_support::TestDir,
    plan: polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan,
    receipt: crate::runtime::config::StopReservationReceipt,
    identity: RunIdentity,
    stop_generation: u64,
}

async fn reserved_direct_fixture() -> ReservedDirectFixture {
    reserved_direct_fixture_with_target(None).await
}

async fn reserved_direct_fixture_with_target(target_ref: Option<&str>) -> ReservedDirectFixture {
    use crate::runtime::config::ApplyCasExpected;
    use crate::runtime::proxy::mesh_apply::{ApplyClaim, ApplyStep};
    use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
    use polaris_store::mesh_guard::{POLICY_KEY, REQUIRED_MARKER_FILE, STATE_KEY};

    let (rt, dir) = test_runtime();
    let wire: serde_json::Value = serde_json::from_str(&crate::test_support::repo_file(
        "ui/src/contracts/mesh-route-state.fixture.json",
    ))
    .unwrap();
    let mut raw = polaris_store::store::default_config();
    raw[POLICY_KEY] = wire[POLICY_KEY].clone();
    raw[STATE_KEY] = wire[STATE_KEY].clone();
    raw[STATE_KEY]["revision"] = serde_json::json!("1");
    std::fs::write(dir.join("config.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    std::fs::write(
        dir.join(REQUIRED_MARKER_FILE),
        serde_json::to_vec(&serde_json::json!({
            "phase": "enabled",
            "localId": raw[STATE_KEY]["localId"],
            "legacyConfigDigest": "0".repeat(64),
        }))
        .unwrap(),
    )
    .unwrap();
    let version = crate::commands::config::config_version(&raw);
    let plan = ManagedMeshRoutePlan {
        schema_version: 1,
        plan_id: "exact-direct-stop".into(),
        config_version: version.clone(),
        input_state_revision: "1".into(),
        identity_bindings: vec![],
        protected_cidrs: vec![],
        owner_routes: vec![],
        reject_cidrs: vec![],
        unassigned_cidrs: vec![],
        released_cidrs: vec![],
        overrides: vec![],
        dns_managed: false,
    };
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let old_generation = rt
        .gate
        .claim_generation(None, LifecycleKind::Start)
        .unwrap();
    let snapshot = rt.config.read_mesh_apply_snapshot().unwrap();
    let prepared = rt
        .gate
        .with_current_generation(old_generation, |live| {
            rt.config.apply_mesh_step_if_current(
                ApplyCasExpected {
                    config_version: &version,
                    state_revision: "1",
                },
                rt.stop_domain.boot_id(),
                live,
                ApplyStep::Prepare {
                    snapshot: &snapshot,
                    plan: &plan,
                    boot_id: rt.stop_domain.boot_id(),
                    manifest_ref: "mesh-routes/plans/exact-direct-stop/manifest.json",
                },
            )
        })
        .unwrap()
        .unwrap();
    let claim = ApplyClaim::from(prepared.transaction.as_ref().unwrap());
    let stop_generation = rt
        .gate
        .claim_generation(Some(old_generation), LifecycleKind::Stop)
        .unwrap();
    let receipt = rt
        .gate
        .with_current_generation(stop_generation, |live| {
            rt.config.reserve_mesh_stop_if_current(
                ApplyCasExpected {
                    config_version: &version,
                    state_revision: &prepared.revision,
                },
                &rt.stop_domain,
                live,
                &plan,
                &claim,
                target_ref.unwrap_or_else(|| identity.persisted_ref()),
                old_generation,
                stop_generation,
            )
        })
        .unwrap()
        .unwrap();
    ReservedDirectFixture {
        rt,
        dir,
        plan,
        receipt,
        identity,
        stop_generation,
    }
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    TryWait,
    StartKill,
    NeverSignal,
}

struct ScriptedIo {
    fault: Fault,
    signals: Arc<AtomicUsize>,
}

impl ScriptedIo {
    fn new(fault: Fault) -> (Arc<Self>, Arc<AtomicUsize>) {
        let signals = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                fault,
                signals: Arc::clone(&signals),
            }),
            signals,
        )
    }
}

impl DirectStopIo for ScriptedIo {
    fn try_wait(&self, child: &mut tokio::process::Child) -> io::Result<Option<ExitStatus>> {
        if matches!(self.fault, Fault::TryWait) {
            return Err(io::Error::other("injected try_wait failure"));
        }
        child.try_wait()
    }

    fn start_kill(&self, child: &mut tokio::process::Child) -> io::Result<()> {
        match self.fault {
            Fault::StartKill => Err(io::Error::other("injected start_kill failure")),
            Fault::NeverSignal => Ok(()),
            Fault::None | Fault::TryWait => {
                child.start_kill()?;
                self.signals.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    }
}

async fn wait_view(observation: &DirectStopObservation, expected: StopView) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while observation.view() != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker reaches expected custody phase");
}

async fn reap_test_stopping_child(rt: &Arc<ProxyRuntime>) {
    {
        let mut slot = rt.child.lock().unwrap();
        let child = slot.stopping_child_for_test().expect("Stopping Child");
        child.start_kill().expect("test cleanup signal");
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let reaped = {
                let mut slot = rt.child.lock().unwrap();
                let child = slot.stopping_child_for_test().expect("Stopping Child");
                child.try_wait().expect("test cleanup wait").is_some()
            };
            if reaped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("test cleanup reaps Child");
}

async fn reap_running_child(rt: &Arc<ProxyRuntime>) {
    let mut run = rt.child.lock().unwrap().take_running_for_test().unwrap();
    run.child.kill().await.unwrap();
}

async fn attach_main_claim(
    rt: &Arc<ProxyRuntime>,
    server_id: &str,
) -> crate::runtime::tailscale_login_core::MainBirthToken {
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir(server_id).unwrap();
    let token = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]}),
            &gate,
            token.clone(),
        )
        .await
        .unwrap();
    rt.child
        .lock()
        .unwrap()
        .running_for_test()
        .expect("real Running Child")
        .main_token = Some(token.clone());
    reservation.arm_external_start();
    drop(reservation);
    token
}

async fn committed_reaped_with_main(
    server_id: &str,
) -> (
    Arc<ProxyRuntime>,
    crate::test_support::TestDir,
    DirectStopObservation,
    crate::runtime::tailscale_login_core::MainBirthToken,
) {
    let fixture = reserved_direct_fixture().await;
    let token = attach_main_claim(&fixture.rt, server_id).await;
    let prepared = prepare_direct_stop(&fixture.rt, &fixture.identity)
        .await
        .unwrap();
    let observation = fixture
        .rt
        .commit_reserved_direct_stop(fixture.receipt, &fixture.plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit same-birth Child"));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    (fixture.rt, fixture.dir, observation, token)
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cas_bridge_reaps_legacy_child_and_keeps_stop_requested() {
    use polaris_config_engine::user_config::mesh_route_state::MeshTransactionPhase;

    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let prepared = prepare_direct_stop(&rt, &identity).await.unwrap();
    let observation = rt
        .commit_reserved_direct_stop(receipt, &plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit the exact Legacy Child"));
    assert_eq!(observation.provenance(), DirectStopProvenance::LegacyExact);
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    assert!(rt.child.lock().unwrap().is_reaped_for_test());
    assert_eq!(
        rt.config
            .read_mesh_apply_snapshot()
            .unwrap()
            .state()
            .transaction
            .as_ref()
            .unwrap()
            .phase,
        MeshTransactionPhase::StopRequested
    );
    assert!(rt.register_helper_backend().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn reaped_direct_birth_closes_only_its_main_claim_and_keeps_terminal_child() {
    let (rt, _dir, observation, token) = committed_reaped_with_main("ts-direct-close").await;
    assert!(rt.mesh.main_owns_tailscale("ts-direct-close", true));
    let old_generation = rt.gate.generation();
    rt.gate
        .claim_generation(None, LifecycleKind::Start)
        .expect("later generation");
    assert_ne!(rt.gate.generation(), old_generation);

    let closed = rt.close_reaped_direct_birth(&observation).await.unwrap();
    assert!(observation.matches_closed_for_test(&closed));
    assert_eq!(observation.view(), StopView::BirthClosed);
    assert!(!rt.mesh.main_owns_tailscale("ts-direct-close", true));
    let gate = rt.mesh.tailscale_state_gate().await;
    assert!(!rt
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
    drop(gate);
    drop(closed);

    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    assert!(!rt.child.lock().unwrap().is_empty());
    assert!(rt.register_helper_backend().is_err());
    assert!(matches!(
        prepare_direct_stop(&rt, &observation.identity_for_test()).await,
        Err(PrepareError::Slot(SlotAdmissionError::Busy))
    ));
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::AlreadyClosed)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_birth_close_waits_for_real_ts_gate_even_without_a_main_claim() {
    let fixture = reserved_direct_fixture().await;
    let prepared = prepare_direct_stop(&fixture.rt, &fixture.identity)
        .await
        .unwrap();
    let observation = fixture
        .rt
        .commit_reserved_direct_stop(fixture.receipt, &fixture.plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit no-TS Child"));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    let gate = fixture.rt.mesh.tailscale_state_gate().await;
    let observation = Arc::new(observation);
    let rt = Arc::clone(&fixture.rt);
    let observed = Arc::clone(&observation);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let close = tokio::spawn(async move {
        let _ = entered_tx.send(());
        rt.close_reaped_direct_birth(&observed).await
    });
    entered_rx.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!close.is_finished(), "even no-TS close needs the real gate");
    assert_eq!(observation.view(), StopView::Reaped);
    drop(gate);
    assert!(close.await.unwrap().is_ok());
    assert_eq!(observation.view(), StopView::BirthClosed);
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_birth_close_rejects_missing_or_successor_registry_claim() {
    let (rt, _dir, observation, token) = committed_reaped_with_main("ts-old").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
    drop(gate);
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::ClaimMismatch)
    ));
    assert_eq!(observation.view(), StopView::Reaped);

    let gate = rt.mesh.tailscale_state_gate().await;
    let successor = rt.mesh.mint_tailscale_main_birth();
    let state = rt.mesh.tailscale_state_dir("ts-successor").unwrap();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]}),
            &gate,
            successor.clone(),
        )
        .await
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    drop(gate);
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::ClaimMismatch)
    ));
    assert_eq!(observation.view(), StopView::Reaped);
    assert!(rt.mesh.main_owns_tailscale("ts-successor", true));
    let gate = rt.mesh.tailscale_state_gate().await;
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&successor, &gate)
        .unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn poisoned_main_registry_leaves_reaped_direct_birth_unclosed() {
    let (rt, _dir, observation, _) = committed_reaped_with_main("ts-poison").await;
    rt.mesh.poison_tailscale_main_claim_lock_for_test();
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::Registry(_))
    ));
    assert_eq!(observation.view(), StopView::Reaped);
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_birth_close_rejects_a_token_from_another_registry() {
    let fixture = reserved_direct_fixture().await;
    let (foreign, _foreign_dir) = test_runtime();
    let gate = foreign.mesh.tailscale_state_gate().await;
    let token = foreign.mesh.mint_tailscale_main_birth();
    let state = foreign.mesh.tailscale_state_dir("ts-foreign").unwrap();
    let mut reservation = foreign
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]}),
            &gate,
            token.clone(),
        )
        .await
        .unwrap();
    fixture
        .rt
        .child
        .lock()
        .unwrap()
        .running_for_test()
        .unwrap()
        .main_token = Some(token.clone());
    reservation.arm_external_start();
    drop(reservation);
    drop(gate);

    let prepared = prepare_direct_stop(&fixture.rt, &fixture.identity)
        .await
        .unwrap();
    let observation = fixture
        .rt
        .commit_reserved_direct_stop(fixture.receipt, &fixture.plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit local Child"));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    assert!(matches!(
        fixture.rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::Registry(_))
    ));
    assert_eq!(observation.view(), StopView::Reaped);
    assert!(foreign.mesh.main_owns_tailscale("ts-foreign", true));
    let gate = foreign.mesh.tailscale_state_gate().await;
    assert!(foreign
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_birth_close_rejects_foreign_runtime_run_nonce_and_slot() {
    let (rt, _dir, observation, token) = committed_reaped_with_main("ts-identity").await;
    let (foreign, _foreign_dir) = test_runtime();
    assert!(matches!(
        foreign.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::WrongRuntime)
    ));
    let mut wrong_domain = observation.clone_for_test();
    wrong_domain.rebind_domain_for_test(Arc::clone(&foreign.stop_domain));
    assert!(matches!(
        rt.close_reaped_direct_birth(&wrong_domain).await,
        Err(DirectBirthCloseError::WrongRuntime)
    ));
    let mut wrong_run = observation.clone_for_test();
    wrong_run.rebind_identity_for_test(RunIdentity::new());
    assert!(matches!(
        rt.close_reaped_direct_birth(&wrong_run).await,
        Err(DirectBirthCloseError::WrongRun)
    ));
    let mut wrong_nonce = observation.clone_for_test();
    wrong_nonce.corrupt_nonce_for_test();
    assert!(matches!(
        rt.close_reaped_direct_birth(&wrong_nonce).await,
        Err(DirectBirthCloseError::WrongRun)
    ));
    let mut wrong_slot = observation.clone_for_test();
    wrong_slot.rebind_slot_for_test(Arc::clone(&foreign.child));
    assert!(matches!(
        rt.close_reaped_direct_birth(&wrong_slot).await,
        Err(DirectBirthCloseError::WrongRuntime)
    ));
    rt.core_via_helper.store(true, Ordering::SeqCst);
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::HelperTouched)
    ));
    rt.core_via_helper.store(false, Ordering::SeqCst);
    assert_eq!(observation.view(), StopView::Reaped);
    assert!(rt.mesh.main_owns_tailscale("ts-identity", true));
    let closed = rt.close_reaped_direct_birth(&observation).await.unwrap();
    assert!(observation.matches_closed_for_test(&closed));
    let gate = rt.mesh.tailscale_state_gate().await;
    assert!(!rt
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_slot_instance_rejects_direct_birth_close_without_releasing_claim() {
    let (rt, _dir, observation, _) = committed_reaped_with_main("ts-instance").await;
    rt.child.lock().unwrap().replace_instance_for_test();
    assert!(matches!(
        rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::WrongSlot)
    ));
    assert!(rt.mesh.main_owns_tailscale("ts-instance", true));
    assert!(rt.child.lock().unwrap().is_reaped_for_test());
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_gone_without_reap_cannot_close_or_release_direct_birth() {
    let fixture = reserved_direct_fixture().await;
    let _token = attach_main_claim(&fixture.rt, "ts-not-reaped").await;
    let (io, _) = ScriptedIo::new(Fault::TryWait);
    let prepared = prepare_with_io_for_test(
        &fixture.rt,
        &fixture.identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let observation = fixture
        .rt
        .commit_reserved_direct_stop(fixture.receipt, &fixture.plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit local Child"));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::RetainedFailure
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while observation.worker_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        fixture.rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::NotReaped)
    ));
    assert!(fixture.rt.mesh.main_owns_tailscale("ts-not-reaped", true));
    assert_eq!(observation.view(), StopView::RetainedFailure);
    reap_test_stopping_child(&fixture.rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_observer_can_close_later_reaped_direct_birth() {
    let fixture = reserved_direct_fixture().await;
    let _token = attach_main_claim(&fixture.rt, "ts-late-reap").await;
    let pause = Arc::new(tokio::sync::Notify::new());
    let (io, _) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &fixture.rt,
        &fixture.identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        Some(Arc::clone(&pause)),
    )
    .await
    .unwrap();
    let observation = fixture
        .rt
        .commit_reserved_direct_stop(fixture.receipt, &fixture.plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("real CAS must commit local Child"));
    wait_view(&observation, StopView::KillRequested).await;
    assert!(tokio::time::timeout(
        Duration::from_millis(10),
        observation.wait_for(Duration::from_secs(5))
    )
    .await
    .is_err());
    assert!(matches!(
        fixture.rt.close_reaped_direct_birth(&observation).await,
        Err(DirectBirthCloseError::NotReaped)
    ));
    assert!(fixture.rt.mesh.main_owns_tailscale("ts-late-reap", true));
    pause.notify_one();
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while observation.worker_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture
        .rt
        .close_reaped_direct_birth(&observation)
        .await
        .unwrap();
    assert_eq!(observation.view(), StopView::BirthClosed);
    assert!(!fixture.rt.mesh.main_owns_tailscale("ts-late-reap", true));
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_direct_birth_close_consumes_one_reaped_proof() {
    let (rt, _dir, observation, _) = committed_reaped_with_main("ts-concurrent").await;
    let observation = Arc::new(observation);
    let first_rt = Arc::clone(&rt);
    let first_observation = Arc::clone(&observation);
    let first =
        tokio::spawn(async move { first_rt.close_reaped_direct_birth(&first_observation).await });
    let second_rt = Arc::clone(&rt);
    let second_observation = Arc::clone(&observation);
    let second = tokio::spawn(async move {
        second_rt
            .close_reaped_direct_birth(&second_observation)
            .await
    });
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(DirectBirthCloseError::AlreadyClosed))
            || matches!(b, Err(DirectBirthCloseError::AlreadyClosed))
    );
    assert_eq!(observation.view(), StopView::BirthClosed);
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_rejects_changed_plan_without_signaling_running_child() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        mut plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    plan.plan_id = "different-plan".into();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_bridge_preserves_same_receipt_and_worker_for_retry() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let prepared = prepare_direct_stop(&rt, &identity).await.unwrap();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let config = Arc::clone(&rt.config);
    let writer = std::thread::spawn(move || {
        let _lock = config.hold_write_lock_for_test();
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    locked_rx.recv().unwrap();
    let (receipt, prepared) = match rt
        .commit_reserved_direct_stop(receipt, &plan, prepared)
        .await
    {
        Err(CommitDirectStopError::Busy { receipt, prepared }) => (*receipt, prepared),
        _ => panic!("contended config writer must return the same typed inputs"),
    };
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    let observation = rt
        .commit_reserved_direct_stop(receipt, &plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("retry must commit the original receipt and worker"));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_gone_before_bridge_leaves_running_child() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    prepared.abort_handle_for_test().abort();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !prepared.gone_before_for_test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn newer_stop_generation_rejects_bridge_without_signaling() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        stop_generation,
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    rt.gate
        .claim_generation(Some(stop_generation), LifecycleKind::Stop)
        .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_disk_revision_or_config_rejects_bridge_before_custody() {
    use polaris_store::mesh_guard::STATE_KEY;

    for case in ["revision", "config"] {
        let ReservedDirectFixture {
            rt,
            dir,
            plan,
            receipt,
            identity,
            ..
        } = reserved_direct_fixture().await;
        let (io, signals) = ScriptedIo::new(Fault::None);
        let prepared = prepare_with_io_for_test(
            &rt,
            &identity,
            io,
            Duration::from_millis(10),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
        if case == "revision" {
            let path = dir.join("config.json");
            let mut raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            raw[STATE_KEY]["revision"] = serde_json::json!("999");
            std::fs::write(path, serde_json::to_vec(&raw).unwrap()).unwrap();
        } else {
            rt.config
                .set_value("logLevel", serde_json::json!("debug"))
                .unwrap();
        }
        assert!(matches!(
            rt.commit_reserved_direct_stop(receipt, &plan, prepared)
                .await,
            Err(CommitDirectStopError::Rejected)
        ));
        assert!(rt.child.lock().unwrap().running_matches(&identity));
        assert_eq!(signals.load(Ordering::SeqCst), 0);
        reap_running_child(&rt).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receipt_target_ref_must_match_prepared_exact_run() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture_with_target(Some("different-direct-run")).await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_child_with_new_run_identity_cannot_inherit_prepared_stop() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    reap_running_child(&rt).await;
    let replacement = DirectCoreRun::new(spawn_custody_stand_in());
    let replacement_identity = replacement.identity.clone();
    assert!(rt
        .child
        .lock()
        .unwrap()
        .install_running(replacement)
        .is_ok());
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt
        .child
        .lock()
        .unwrap()
        .running_matches(&replacement_identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn replaced_slot_instance_rejects_prepared_worker() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    rt.child.lock().unwrap().replace_instance_for_test();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_outer_slot_must_be_runtime_owned_arc() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (other, _other_dir) = test_runtime();
    let (io, signals) = ScriptedIo::new(Fault::None);
    let mut prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    prepared.rebind_slot_for_test(Arc::clone(&other.child));
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn receipt_from_other_runtime_domain_cannot_commit_same_manager() {
    let ReservedDirectFixture {
        mut rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let gate = Arc::clone(&rt.gate);
    Arc::get_mut(&mut rt).unwrap().stop_domain =
        crate::runtime::config::StopRuntimeDomain::new(gate);
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_runtime_gate_must_have_minted_live_claim() {
    let ReservedDirectFixture {
        mut rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let other_gate = Arc::new(LifecycleGate::default());
    other_gate
        .claim_generation(None, LifecycleKind::Start)
        .unwrap();
    other_gate
        .claim_generation(None, LifecycleKind::Stop)
        .unwrap();
    Arc::get_mut(&mut rt).unwrap().gate = other_gate;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_gone_after_commit_retains_stopping_custody() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, _signals) = ScriptedIo::new(Fault::NeverSignal);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let abort = prepared.abort_handle_for_test();
    let observation = rt
        .commit_reserved_direct_stop(receipt, &plan, prepared)
        .await
        .unwrap_or_else(|_| panic!("commit must succeed before worker cancellation"));
    abort.abort();
    assert_eq!(
        observation.wait_for(Duration::from_secs(3)).await,
        StopWaitOutcome::WorkerGone
    );
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    reap_test_stopping_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_bridge_while_waiting_for_ts_gate_leaves_running_child() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let ts_guard = rt.mesh.tailscale_state_gate().await;
    let caller_rt = Arc::clone(&rt);
    let pending = tokio::spawn(async move {
        caller_rt
            .commit_reserved_direct_stop(receipt, &plan, prepared)
            .await
    });
    tokio::task::yield_now().await;
    pending.abort();
    assert!(pending.await.is_err());
    drop(ts_guard);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn helper_history_rejects_bridge_but_allows_later_legacy_direct_install() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let prepared = prepare_direct_stop(&rt, &identity).await.unwrap();
    rt.register_helper_backend().unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    reap_running_child(&rt).await;
    let replacement = DirectCoreRun::new(spawn_custody_stand_in());
    let replacement_identity = replacement.identity.clone();
    assert!(rt
        .child
        .lock()
        .unwrap()
        .install_running(replacement)
        .is_ok());
    assert!(rt
        .child
        .lock()
        .unwrap()
        .running_matches(&replacement_identity));
    assert!(matches!(
        prepare_direct_stop(&rt, &replacement_identity).await,
        Err(PrepareError::Slot(SlotAdmissionError::HelperTouched))
    ));
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_background_helper_service_work_keeps_prequeued_fence() {
    use std::sync::atomic::AtomicBool;

    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    // The command wrapper registers before spawn_blocking. A blocking task
    // keeps running after its async JoinHandle is aborted (as an installer
    // waiting for a privilege dialog does).
    rt.register_helper_backend().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let late = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&release);
    let completed = Arc::clone(&late);
    let work = tokio::task::spawn_blocking(move || {
        entered_tx.send(()).unwrap();
        let (lock, wake) = &*held;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
        completed.store(true, Ordering::Release);
    });
    entered_rx.await.unwrap();
    work.abort();
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !late.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_helper_marks_all_registered_runtime_slots() {
    let ReservedDirectFixture {
        rt: first,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let second = Arc::new(ProxyRuntime::new(
        Arc::clone(&first.config),
        Arc::clone(&first.helper),
        Arc::clone(&first.mesh),
        Box::new(RecordingClearer {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(NoNetworkDoh),
    ));
    let prepared = prepare_direct_stop(&first, &identity).await.unwrap();
    second.register_helper_backend().unwrap();
    assert!(matches!(
        first
            .commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    reap_running_child(&first).await;

    let third = Arc::new(ProxyRuntime::new(
        Arc::clone(&first.config),
        Arc::clone(&first.helper),
        Arc::clone(&first.mesh),
        Box::new(RecordingClearer {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(NoNetworkDoh),
    ));
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    assert!(third.child.lock().unwrap().install_running(run).is_ok());
    assert!(matches!(
        prepare_direct_stop(&third, &identity).await,
        Err(PrepareError::Slot(SlotAdmissionError::HelperTouched))
    ));
    reap_running_child(&third).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_run_facts_remain_unsupported_at_bridge() {
    let ReservedDirectFixture {
        rt,
        dir: _dir,
        plan,
        receipt,
        identity,
        ..
    } = reserved_direct_fixture().await;
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    rt.child.lock().unwrap().mark_running_managed_for_test();
    assert!(matches!(
        rt.commit_reserved_direct_stop(receipt, &plan, prepared)
            .await,
        Err(CommitDirectStopError::Rejected)
    ));
    assert!(matches!(
        prepare_direct_stop(&rt, &identity).await,
        Err(PrepareError::Slot(SlotAdmissionError::ManagedOrigin))
    ));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    reap_running_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_cancel_and_rejected_identity_leave_running_child_untouched() {
    let (rt, _dir) = test_runtime();
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    let wrong = RunIdentity::new();
    let pid = run.child.id().unwrap();
    rt.child.lock().unwrap().install_running_for_test(run);
    let (io, signals) = ScriptedIo::new(Fault::None);
    rt.core_via_helper.store(true, Ordering::SeqCst);
    assert!(matches!(
        prepare_direct_stop(&rt, &identity).await,
        Err(PrepareError::Unsupported)
    ));
    rt.core_via_helper.store(false, Ordering::SeqCst);
    assert!(matches!(
        prepare_with_io_for_test(
            &rt,
            &wrong,
            io.clone(),
            Duration::from_millis(10),
            Duration::from_secs(2),
            None,
        )
        .await,
        Err(PrepareError::Slot(SlotAdmissionError::WrongRun))
    ));
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io.clone(),
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .expect("ready worker before commit");
    drop(prepared);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert!(pid_alive(pid));
    assert_eq!(signals.load(Ordering::SeqCst), 0);

    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let rejected = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &wrong, prepared)
    };
    assert!(matches!(
        rejected,
        Err(CommitRejected::Slot(SlotAdmissionError::WrongRun))
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert!(pid_alive(pid));
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    let mut run = rt.child.lock().unwrap().take_running_for_test().unwrap();
    run.child.kill().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_worker_reaps_after_observer_drop_but_never_empties_slot() {
    let (rt, _dir) = test_runtime();
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let prepared = prepare_direct_stop(&rt, &identity).await.unwrap();
    let observation = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &identity, prepared).unwrap()
    };
    drop(observation);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rt.child.lock().unwrap().is_reaped_for_test() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("detached worker reaps exact Child");
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    assert!(!rt.child.lock().unwrap().is_empty());
    assert!(matches!(
        rt.child.lock().unwrap().take_running_legacy(),
        Err(TakeRunningError::Stopping)
    ));
    let Err(rejected) = rt
        .child
        .lock()
        .unwrap()
        .install_running(DirectCoreRun::new(spawn_custody_stand_in()))
    else {
        panic!("Reaped Stopping must reject replacement");
    };
    let mut rejected_child = rejected.child;
    rejected_child.kill().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn helper_registry_retains_detached_stopping_slot_after_runtime_drop() {
    let (rt, _dir) = test_runtime();
    let helper = Arc::clone(&rt.helper);
    let weak_child = Arc::downgrade(&rt.child);
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let pause = Arc::new(tokio::sync::Notify::new());
    let (io, _) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        Some(Arc::clone(&pause)),
    )
    .await
    .unwrap();
    let observation = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &identity, prepared).unwrap()
    };
    wait_view(&observation, StopView::KillRequested).await;
    drop(observation);
    drop(rt);
    assert!(
        weak_child.upgrade().is_some(),
        "detached worker still owns Child"
    );
    assert!(
        helper.register_core_mutation().is_err(),
        "Stopping denies helper IPC"
    );
    assert_eq!(helper.core_fence_count_for_test(), 1);

    pause.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak_child.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    helper.register_core_mutation().unwrap();
    assert_eq!(helper.core_fence_count_for_test(), 0);
}

#[test]
fn helper_registry_prunes_dead_child_slots_during_runtime_churn() {
    let (rt, _dir) = test_runtime();
    let helper = Arc::clone(&rt.helper);
    for _ in 0..64 {
        let temporary = ProxyRuntime::new(
            Arc::clone(&rt.config),
            Arc::clone(&helper),
            Arc::clone(&rt.mesh),
            Box::new(RecordingClearer {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoNetworkDoh),
        );
        assert!(helper.core_fence_count_for_test() <= 2);
        drop(temporary);
    }
    helper.register_core_mutation().unwrap();
    assert_eq!(helper.core_fence_count_for_test(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn already_exited_child_reaps_without_a_kill_request() {
    let (rt, _dir) = test_runtime();
    let mut child = spawn_custody_stand_in();
    child.kill().await.expect("exit before exact stop");
    let run = DirectCoreRun::new(child);
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let observation = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &identity, prepared).unwrap()
    };
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    assert_eq!(signals.load(Ordering::SeqCst), 0);
    assert!(rt.child.lock().unwrap().is_reaped_for_test());
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_request_waits_for_exact_reap_and_observer_timeout_does_not_abort() {
    let (rt, _dir) = test_runtime();
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let pause = Arc::new(tokio::sync::Notify::new());
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        Some(Arc::clone(&pause)),
    )
    .await
    .unwrap();
    let observation = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &identity, prepared).unwrap()
    };
    wait_view(&observation, StopView::KillRequested).await;
    assert!(!rt.child.lock().unwrap().is_reaped_for_test());
    assert_eq!(signals.load(Ordering::SeqCst), 1);
    assert_eq!(
        observation.wait_for(Duration::from_millis(30)).await,
        StopWaitOutcome::Pending
    );
    assert!(observation.worker_alive());
    pause.notify_one();
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_io_errors_and_timeout_retain_stopping_child() {
    for fault in [Fault::TryWait, Fault::StartKill, Fault::NeverSignal] {
        let (rt, _dir) = test_runtime();
        let run = DirectCoreRun::new(spawn_custody_stand_in());
        let identity = run.identity.clone();
        let pid = run.child.id().unwrap();
        rt.child.lock().unwrap().install_running_for_test(run);
        let (io, signals) = ScriptedIo::new(fault);
        let prepared = prepare_with_io_for_test(
            &rt,
            &identity,
            io,
            Duration::from_millis(10),
            Duration::from_millis(80),
            None,
        )
        .await
        .unwrap();
        let observation = {
            let mut slot = rt.child.lock().unwrap();
            commit_for_test(&mut slot, &identity, prepared).unwrap()
        };
        assert_eq!(
            observation.wait_for(Duration::from_secs(3)).await,
            StopWaitOutcome::RetainedFailure
        );
        assert_eq!(observation.view(), StopView::RetainedFailure);
        assert_eq!(signals.load(Ordering::SeqCst), 0);
        assert!(rt.child.lock().unwrap().is_stopping_for_test());
        assert!(pid_alive(pid));
        reap_test_stopping_child(&rt).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn aborted_worker_keeps_child_and_nonreaped_custody() {
    let (rt, _dir) = test_runtime();
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    let pause = Arc::new(tokio::sync::Notify::new());
    let (io, signals) = ScriptedIo::new(Fault::None);
    let prepared = prepare_with_io_for_test(
        &rt,
        &identity,
        io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        Some(pause),
    )
    .await
    .unwrap();
    let abort = prepared.abort_handle_for_test();
    let observation = {
        let mut slot = rt.child.lock().unwrap();
        commit_for_test(&mut slot, &identity, prepared).unwrap()
    };
    wait_view(&observation, StopView::KillRequested).await;
    abort.abort();
    assert_eq!(
        observation.wait_for(Duration::from_secs(3)).await,
        StopWaitOutcome::WorkerGone
    );
    assert_eq!(signals.load(Ordering::SeqCst), 1);
    assert!(!rt.child.lock().unwrap().is_reaped_for_test());
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    reap_test_stopping_child(&rt).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_slot_and_nonce_cannot_act_on_another_real_child() {
    let (first, _first_dir) = test_runtime();
    let (second, _second_dir) = test_runtime();
    let first_run = DirectCoreRun::new(spawn_custody_stand_in());
    let first_identity = first_run.identity.clone();
    let first_pid = first_run.child.id().unwrap();
    first
        .child
        .lock()
        .unwrap()
        .install_running_for_test(first_run);
    let second_run = DirectCoreRun::new(spawn_custody_stand_in());
    let second_identity = second_run.identity.clone();
    second
        .child
        .lock()
        .unwrap()
        .install_running_for_test(second_run);
    let (first_io, first_signals) = ScriptedIo::new(Fault::None);
    let first_prepared = prepare_with_io_for_test(
        &first,
        &first_identity,
        first_io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let rejected = {
        let mut wrong_slot = second.child.lock().unwrap();
        commit_for_test(&mut wrong_slot, &first_identity, first_prepared)
    };
    assert!(matches!(
        rejected,
        Err(CommitRejected::Slot(SlotAdmissionError::WrongSlot))
    ));
    assert!(first.child.lock().unwrap().running_matches(&first_identity));
    assert!(second
        .child
        .lock()
        .unwrap()
        .running_matches(&second_identity));
    assert!(pid_alive(first_pid));
    assert_eq!(first_signals.load(Ordering::SeqCst), 0);

    let (winner_io, winner_signals) = ScriptedIo::new(Fault::None);
    let (loser_io, loser_signals) = ScriptedIo::new(Fault::None);
    let winner = prepare_with_io_for_test(
        &second,
        &second_identity,
        winner_io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let loser = prepare_with_io_for_test(
        &second,
        &second_identity,
        loser_io,
        Duration::from_millis(10),
        Duration::from_secs(2),
        None,
    )
    .await
    .unwrap();
    let observation = {
        let mut slot = second.child.lock().unwrap();
        commit_for_test(&mut slot, &second_identity, winner).unwrap()
    };
    let rejected = {
        let mut slot = second.child.lock().unwrap();
        commit_for_test(&mut slot, &second_identity, loser)
    };
    assert!(matches!(
        rejected,
        Err(CommitRejected::Slot(SlotAdmissionError::Busy))
    ));
    assert_eq!(
        observation.wait_for(Duration::from_secs(5)).await,
        StopWaitOutcome::Reaped
    );
    assert!(matches!(
        prepare_direct_stop(&second, &second_identity).await,
        Err(PrepareError::Slot(SlotAdmissionError::Busy))
    ));
    assert_eq!(winner_signals.load(Ordering::SeqCst), 1);
    assert_eq!(loser_signals.load(Ordering::SeqCst), 0);
    let mut first_run = first.child.lock().unwrap().take_running_for_test().unwrap();
    first_run.child.kill().await.unwrap();
}

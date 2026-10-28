use super::*;

mod direct_stop;
mod native_direct_stop;

#[cfg(target_os = "linux")]
struct BirthDaemonStream {
    inner: polaris_helper_client::MockStream,
    frames: Arc<Mutex<Vec<String>>>,
}

#[cfg(target_os = "linux")]
impl polaris_helper_client::ConnectionStream for BirthDaemonStream {
    fn read_until_timeout(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
        self.inner.read_until_timeout(buf)
    }

    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(data)?;
        self.frames
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(data).into_owned());
        Ok(())
    }

    fn shutdown(&mut self) -> std::io::Result<()> {
        self.inner.shutdown()
    }
}

#[cfg(target_os = "linux")]
struct BirthDaemonConnector {
    replies: Mutex<std::collections::VecDeque<polaris_helper_client::MockStream>>,
    frames: Arc<Mutex<Vec<String>>>,
}

#[cfg(target_os = "linux")]
impl polaris_helper_client::Connector for BirthDaemonConnector {
    fn connect(
        &self,
    ) -> Result<Box<dyn polaris_helper_client::ConnectionStream>, polaris_helper_client::ClientError>
    {
        let reply = self.replies.lock().unwrap().pop_front().ok_or_else(|| {
            polaris_helper_client::ClientError::Connect("mock daemon exhausted".into())
        })?;
        Ok(Box::new(BirthDaemonStream {
            inner: reply,
            frames: Arc::clone(&self.frames),
        }))
    }
}

#[cfg(target_os = "linux")]
fn birth_daemon_runtime(
    replies: impl IntoIterator<Item = String>,
) -> (Arc<ProxyRuntime>, TestDir, Arc<Mutex<Vec<String>>>) {
    let dir = fresh_test_dir();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let connector = BirthDaemonConnector {
        replies: Mutex::new(
            replies
                .into_iter()
                .map(|line| polaris_helper_client::MockStream::with_response(line.into_bytes()))
                .collect(),
        ),
        frames: Arc::clone(&frames),
    };
    let helper = crate::runtime::helper::HelperRuntime::with_test_connector_for_tests(
        dir.clone(),
        installed_helper_status(false),
        Arc::new(connector),
    );
    (test_runtime_in_on(dir.clone(), helper), dir, frames)
}

fn android_target(run_id: &str) -> super::super::android_bridge::AndroidExactTarget {
    super::super::android_bridge::AndroidExactTarget {
        run_id: run_id.to_owned(),
        birth_nonce: format!("nonce-{run_id}"),
    }
}

fn spawn_custody_stand_in() -> tokio::process::Child {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("powershell");
        command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        command
    } else {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("30");
        command
    };
    command
        .kill_on_drop(true)
        .spawn()
        .expect("spawn local custody stand-in")
}

#[tokio::test]
async fn main_birth_follows_real_direct_child_through_confirmed_stop() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-main").unwrap();
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
    reservation.arm_external_start();
    drop(reservation);
    let run =
        DirectCoreRun::with_main_token(spawn_custody_stand_in(), RunIdentity::new(), token.clone());
    rt.child.lock().unwrap().install_running_for_test(run);
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&token)));
    assert!(rt.mesh.main_owns_tailscale("ts-main", true));
    rt.kill_core_and_release_main(&gate).await.unwrap();
    assert!(!rt.mesh.main_owns_tailscale("ts-main", true));
    assert!(!rt
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
}

#[tokio::test]
async fn legacy_helper_ack_keeps_main_birth_and_backend_custody() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-helper").unwrap();
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
    let attempt = rt
        .register_helper_start_backend_with_main(Some(token.clone()))
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .finish_helper_start(&attempt, Some(7123)));
    *rt.pid.lock().unwrap() = Some(7123);
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&token)));
    let (failed, _, _) = RecordingStop::new(Err("no helper ACK".into()));
    assert!(rt
        .kill_core_via_helper(failed as Arc<dyn HelperStopOps>)
        .await
        .is_err());
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&token)));
    assert!(rt.mesh.main_owns_tailscale("ts-helper", true));
    let (retry, calls, wants) = RecordingStop::new(Ok(()));
    let error = rt
        .kill_core_via_helper_with_main(retry as Arc<dyn HelperStopOps>, Some(&gate))
        .await
        .unwrap_err();
    assert!(error.contains("legacy Stop ACK"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*wants.lock().unwrap(), vec![Some(7123)]);
    assert!(rt.main_token_for_stop().unwrap().is_some());
    assert!(
        rt.mesh.main_owns_tailscale("ts-helper", true),
        "local registry is not itself a helper ACK"
    );
    assert!(rt.child.lock().unwrap().has_helper_start());
    assert_eq!(*rt.pid.lock().unwrap(), Some(7123));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn exact_helper_main_mismatch_retains_custody_then_retries_production_birth_transport() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::HelperStartResult;
    let pid = 7123;
    let hex = "00112233445566778899aabbccddeeff";
    let target = exact_helper_target(pid, hex);
    let (rt, _dir, frames) = birth_daemon_runtime([
        format!("OK birth-stopped {pid} {hex}\n"),
        format!("OK birth-stopped {pid} {hex}\n"),
    ]);
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("helper-main-compare").unwrap();
    let generated =
        serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]});
    let original = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, original.clone())
        .await
        .unwrap();
    let attempt = rt
        .register_helper_start_backend_with_main(Some(original.clone()))
        .unwrap();
    HelperStartCompletion::for_test(&rt, attempt.clone())
        .publish(&Ok(HelperStartResult::BirthStarted(target)))
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&original, &gate)
        .unwrap());
    let successor = rt.mesh.mint_tailscale_main_birth();
    let mut replacement = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, successor.clone())
        .await
        .unwrap();
    replacement.arm_external_start();
    drop(replacement);
    assert!(rt
        .kill_core_and_release_main(&gate)
        .await
        .unwrap_err()
        .contains("main claim birth changed"));
    assert!(rt.child.lock().unwrap().has_helper_start());
    assert!(!rt.child.lock().unwrap().helper_stop_inflight_for_test());
    assert_eq!(*rt.pid.lock().unwrap(), Some(pid));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&original)));
    assert!(
        rt.mesh
            .release_tailscale_main_states_if_token(&successor, &gate)
            .unwrap(),
        "the successor main claim was untouched"
    );
    let mut restored = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, original)
        .await
        .unwrap();
    restored.arm_external_start();
    drop(restored);
    rt.kill_core_and_release_main(&gate).await.unwrap();
    assert!(!rt.mesh.main_owns_tailscale("helper-main-compare", true));
    assert!(!rt.child.lock().unwrap().has_helper_start());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(
        rt.core_via_helper.load(Ordering::SeqCst),
        "native birth exit does not attest platform NoOwner"
    );
    assert_eq!(
        *frames.lock().unwrap(),
        vec![format!("stop-birth-safe\n{pid}\n{hex}\n"); 2]
    );
}

#[tokio::test]
async fn cancelled_detached_android_start_retains_unknown_birth_after_late_reply() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-android").unwrap();
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
    rt.book_android_global_start(Some(token.clone())).unwrap();
    reservation.arm_external_start();
    drop(reservation);
    // Same topology as call_with_budget: dropping the waiter does not cancel
    // the queued bridge task. A late Start success cannot confirm that waiter.
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let reached = Arc::new(tokio::sync::Semaphore::new(0));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let late = tokio::spawn({
        let release = release.clone();
        let reached = reached.clone();
        async move {
            reached.add_permits(1);
            release.acquire().await.unwrap().forget();
            let _ = done_tx.send(());
        }
    });
    reached.acquire().await.unwrap().forget();
    drop(late);
    release.add_permits(1);
    done_rx.await.unwrap();
    assert!(
        !rt.android_main_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .start_confirmed
    );
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .exact_target
        .is_none());
    assert!(rt.begin_android_stop_booking(true).is_err());
    assert!(rt.book_android_global_start(None).is_err());
    assert!(rt.mesh.main_owns_tailscale("ts-android", true));
}

#[tokio::test]
async fn android_exact_receipt_stays_with_its_request_birth() {
    let (rt, _dir) = test_runtime();
    let a = rt.book_android_global_start(None).unwrap();
    let valid_a = android_target("android-a");
    let invalid = super::super::android_bridge::AndroidExactTarget {
        run_id: "android-a".into(),
        birth_nonce: " ".into(),
    };
    assert!(rt.confirm_android_global_start(&a, invalid).is_err());
    assert!(
        !rt.android_main_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .start_confirmed
    );
    assert!(rt.begin_android_stop_booking(true).is_err());

    rt.confirm_android_global_start(&a, valid_a.clone())
        .unwrap();
    rt.confirm_android_global_start(&a, valid_a.clone())
        .unwrap();
    assert!(rt
        .confirm_android_global_start(&a, android_target("android-a-different"))
        .is_err());
    assert_eq!(
        rt.android_main_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .exact_target,
        Some(valid_a.clone())
    );

    let stop_a = rt.begin_android_stop_booking(true).unwrap();
    stop_a.finish_without_main(Ok(())).unwrap();
    let b = rt.book_android_global_start(None).unwrap();
    assert!(rt.confirm_android_global_start(&a, valid_a).is_err());
    {
        let custody = rt.android_main_token.lock().unwrap();
        let current = custody.as_ref().unwrap();
        assert!(current.birth.same(&b));
        assert!(!current.start_confirmed);
        assert!(current.exact_target.is_none());
    }
    assert!(rt.begin_android_stop_booking(true).is_err());
    let valid_b = android_target("android-b");
    rt.confirm_android_global_start(&b, valid_b.clone())
        .unwrap();
    assert_eq!(
        rt.android_main_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .exact_target,
        Some(valid_b)
    );
}

#[tokio::test]
async fn typed_pre_dispatch_retirement_releases_only_exact_android_custody_and_ts_claim() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-endpoint-retired").unwrap();
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
    let first = rt.book_android_global_start(Some(token)).unwrap();
    reservation.arm_external_start();
    assert!(rt.mesh.main_owns_tailscale("ts-endpoint-retired", true));
    rt.abandon_android_global_start_without_birth(&first)
        .unwrap();
    reservation.confirmed_no_external_writer();
    drop(reservation);
    assert!(!rt.mesh.main_owns_tailscale("ts-endpoint-retired", true));

    let second = rt.book_android_global_start(None).unwrap();
    assert!(rt
        .abandon_android_global_start_without_birth(&first)
        .is_err());
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .birth
        .same(&second));
    rt.confirm_android_global_start(&second, android_target("new-core"))
        .unwrap();
    assert!(rt
        .abandon_android_global_start_without_birth(&second)
        .is_err());
}

#[test]
fn no_birth_retirement_never_erases_unknown_android_custody() {
    let (rt, _dir) = test_runtime();
    let birth = rt.book_android_global_start(None).unwrap();
    rt.android_main_token
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .historic_unknown = true;
    assert!(rt
        .abandon_android_global_start_without_birth(&birth)
        .is_err());
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .birth
        .same(&birth));
}

#[test]
fn retired_system_start_port_retries_with_fresh_port_and_stop_supersedes_old_birth() {
    use polaris_core_supervisor::port_bookkeeping::{
        FreePortProvider, PortAllocator, PortExclusions, PrimaryApiPortLedger,
    };
    use std::collections::VecDeque;

    struct Ports(Mutex<VecDeque<u16>>);
    impl FreePortProvider for Ports {
        fn try_allocate(&self) -> Option<u16> {
            self.0.lock().unwrap().pop_front()
        }
    }

    let (rt, _dir) = test_runtime();
    let ledger = PrimaryApiPortLedger::default();
    let allocator = PortAllocator::new(Ports(Mutex::new(VecDeque::from([20_001, 20_001, 20_002]))))
        .with_max_attempts(2);
    let exclusions = PortExclusions::for_primary_api(Some(9090), None, None, None);
    let generation = rt.gate.generation();

    // A previous SystemStart used P before Rust's ledger existed. Rust first
    // proposes P; Kotlin rejects it before VpnBridge.beginStart/native birth.
    let first_port = ledger.allocate(&allocator, &exclusions).unwrap().port;
    assert_eq!(first_port, 20_001);
    let first = rt
        .book_android_global_start_for_generation(generation, None)
        .unwrap()
        .unwrap();
    rt.abandon_android_global_start_without_birth(&first)
        .unwrap();

    // A Stop/successor generation at this point must prevent the old request
    // from booking or dispatching another native birth.
    rt.gate.bump_generation();
    assert!(rt
        .book_android_global_start_for_generation(generation, None)
        .unwrap()
        .is_none());
    assert!(rt.android_main_token.lock().unwrap().is_none());

    // A fresh bridge Start remains possible. Even if the provider offers P
    // again, the process ledger keeps it retired and picks Q.
    let fresh_port = ledger.allocate(&allocator, &exclusions).unwrap().port;
    assert_eq!(fresh_port, 20_002);
    let second = rt
        .book_android_global_start_for_generation(rt.gate.generation(), None)
        .unwrap()
        .unwrap();
    assert!(!first.same(&second));
}

#[tokio::test]
async fn cancelled_detached_android_stop_keeps_s2_ack_from_releasing_birth() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-android-stop").unwrap();
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
    let birth = rt.book_android_global_start(Some(token.clone())).unwrap();
    reservation.arm_external_start();
    drop(reservation);
    rt.confirm_android_global_start(&birth, android_target("android-stop"))
        .unwrap();
    let queued = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let s1_waiter = tokio::spawn({
        let rt = rt.clone();
        let queued = queued.clone();
        let release = release.clone();
        async move {
            let booking = rt.begin_android_stop_booking(true).unwrap();
            let detached = tokio::spawn(async move {
                queued.add_permits(1);
                release.acquire().await.unwrap().forget();
                let _ = done_tx.send(());
            });
            detached.await.unwrap();
            drop(booking);
        }
    });
    queued.acquire().await.unwrap().forget();
    s1_waiter.abort();
    assert!(s1_waiter.await.is_err());
    // A second global Stop may ACK; S1 may still be delivered afterwards.
    let s2 = rt.begin_android_stop_booking(true).unwrap();
    assert!(s2.finish_with_gate(Ok(()), &rt.mesh, &gate).is_err());
    release.add_permits(1);
    done_rx.await.unwrap();
    assert!(
        rt.android_main_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .historic_unknown
    );
    assert!(rt.mesh.main_owns_tailscale("ts-android-stop", true));
}

#[tokio::test]
async fn certain_android_stop_ack_removes_matching_ts_and_global_birth_together() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-android-certain").unwrap();
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
    reservation.arm_external_start();
    drop(reservation);
    let birth = rt.book_android_global_start(Some(token.clone())).unwrap();
    rt.confirm_android_global_start(&birth, android_target("android-certain"))
        .unwrap();
    let booking = rt.begin_android_stop_booking(true).unwrap();
    assert!(rt.admit_android_global_custody().is_err());
    booking.finish_with_gate(Ok(()), &rt.mesh, &gate).unwrap();
    assert!(!rt.mesh.main_owns_tailscale("ts-android-certain", true));
    assert!(rt.android_main_token.lock().unwrap().is_none());
    assert!(!rt
        .mesh
        .release_tailscale_main_states_if_token(&token, &gate)
        .unwrap());
}

#[tokio::test]
async fn android_stop_ack_cannot_clear_a_successor_registry_birth() {
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-android-successor").unwrap();
    let generated =
        serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]});
    let old = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, old.clone())
        .await
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    let birth = rt.book_android_global_start(Some(old.clone())).unwrap();
    rt.confirm_android_global_start(&birth, android_target("android-successor"))
        .unwrap();
    let old_stop = rt.begin_android_stop_booking(true).unwrap();

    // Deliberately bypass admission in this fixture to model a successor
    // registry claim that an old asynchronous ACK must never erase.
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&old, &gate)
        .unwrap());
    let new = rt.mesh.mint_tailscale_main_birth();
    let mut successor = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, new.clone())
        .await
        .unwrap();
    successor.arm_external_start();
    drop(successor);
    assert!(old_stop.finish_with_gate(Ok(()), &rt.mesh, &gate).is_err());
    assert!(rt.mesh.main_owns_tailscale("ts-android-successor", true));
    assert!(rt.admit_android_global_custody().is_err());
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&new, &gate)
        .unwrap());
}

#[tokio::test]
async fn no_ts_android_start_and_stop_still_hold_global_birth() {
    let (rt, _dir) = test_runtime();
    let birth = rt.book_android_global_start(None).unwrap();
    assert!(rt.admit_android_global_custody().is_err());
    assert!(rt.begin_android_stop_booking(true).is_err());
    rt.confirm_android_global_start(&birth, android_target("android-no-ts"))
        .unwrap();
    let stop = rt.begin_android_stop_booking(true).unwrap();
    assert!(rt.admit_android_global_custody().is_err());
    assert!(stop.finish_without_main(Ok(())).is_ok());
    assert!(rt.admit_android_global_custody().is_ok());
    assert!(rt.book_android_global_start(None).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn newer_android_claim_preempts_an_older_preflight_before_booking() {
    let (rt, _dir) = test_runtime();
    let a_preflight = Arc::new(tokio::sync::Semaphore::new(0));
    let b_claimed = Arc::new(tokio::sync::Semaphore::new(0));
    let a = tokio::spawn({
        let rt = Arc::clone(&rt);
        let a_preflight = Arc::clone(&a_preflight);
        let b_claimed = Arc::clone(&b_claimed);
        async move {
            let _ts_gate = rt.mesh.tailscale_state_gate().await;
            let generation = rt.claim_android_global_start_generation().unwrap();
            a_preflight.add_permits(1);
            tokio::time::timeout(Duration::from_secs(3), b_claimed.acquire())
                .await
                .expect("B must claim while A is in preflight")
                .unwrap()
                .forget();
            rt.book_android_global_start_for_generation(generation, None)
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(3), a_preflight.acquire())
        .await
        .expect("A must reach preflight")
        .unwrap()
        .forget();
    let b_generation = rt.claim_android_global_start_generation().unwrap();
    b_claimed.add_permits(1);
    assert!(
        a.await.unwrap().is_none(),
        "obsolete A must dispatch no IPC"
    );
    assert!(rt.android_main_token.lock().unwrap().is_none());
    let _ts_gate = rt.mesh.tailscale_state_gate().await;
    assert!(rt
        .book_android_global_start_for_generation(b_generation, None)
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn booked_android_birth_blocks_a_new_claim_before_generation_changes() {
    let (rt, _dir) = test_runtime();
    let a_booked = Arc::new(tokio::sync::Semaphore::new(0));
    let release_a = Arc::new(tokio::sync::Semaphore::new(0));
    let a = tokio::spawn({
        let rt = Arc::clone(&rt);
        let a_booked = Arc::clone(&a_booked);
        let release_a = Arc::clone(&release_a);
        async move {
            let _ts_gate = rt.mesh.tailscale_state_gate().await;
            let generation = rt.claim_android_global_start_generation().unwrap();
            let birth = rt
                .book_android_global_start_for_generation(generation, None)
                .unwrap()
                .unwrap();
            a_booked.add_permits(1);
            tokio::time::timeout(Duration::from_secs(3), release_a.acquire())
                .await
                .expect("B must inspect A's booked birth")
                .unwrap()
                .forget();
            birth
        }
    });
    tokio::time::timeout(Duration::from_secs(3), a_booked.acquire())
        .await
        .expect("A must book before B claims")
        .unwrap()
        .forget();
    let owned_generation = rt.gate.generation();
    assert!(rt.claim_android_global_start_generation().is_err());
    assert_eq!(rt.gate.generation(), owned_generation);
    release_a.add_permits(1);
    let booked = a.await.unwrap();
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .birth
        .same(&booked));
}

#[tokio::test]
async fn no_ts_android_detached_stop_and_cold_sweep_remain_sticky() {
    let (rt, _dir) = test_runtime();
    let birth = rt.book_android_global_start(None).unwrap();
    rt.confirm_android_global_start(&birth, android_target("android-sticky"))
        .unwrap();
    let s1 = rt.begin_android_stop_booking(true).unwrap();
    drop(s1);
    let s2 = rt.begin_android_stop_booking(true).unwrap();
    assert!(s2.finish_without_main(Ok(())).is_err());
    assert!(rt.admit_android_global_custody().is_err());
    assert!(rt.book_android_global_start(None).is_err());

    let (fresh, _dir) = test_runtime();
    let cold = fresh.begin_android_stop_booking(false).unwrap();
    assert!(fresh.admit_android_global_custody().is_err());
    drop(cold);
    let retry = fresh.begin_android_stop_booking(false).unwrap();
    assert!(retry.finish_without_main(Ok(())).is_err());
    assert!(fresh.admit_android_global_custody().is_err());
}

#[tokio::test]
async fn stale_android_stop_nonce_cannot_clear_a_later_booking() {
    let (rt, _dir) = test_runtime();
    let birth = rt.book_android_global_start(None).unwrap();
    rt.confirm_android_global_start(&birth, android_target("android-stale-stop"))
        .unwrap();
    let stale = rt.begin_android_stop_booking(true).unwrap();
    {
        // Force the state a cancelled S1 would leave while retaining a stale
        // test handle, so a late old ACK exercises the nonce comparison.
        let mut guard = rt.android_main_token.lock().unwrap();
        let attempt = guard.as_mut().unwrap();
        attempt.stop_inflight = None;
        attempt.historic_unknown = true;
    }
    let current = rt.begin_android_stop_booking(true).unwrap();
    let current_nonce = rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .stop_inflight
        .as_ref()
        .unwrap()
        .clone();
    assert!(stale.finish_without_main(Ok(())).is_err());
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .stop_inflight
        .as_ref()
        .is_some_and(|nonce| Arc::ptr_eq(nonce, &current_nonce)));
    assert!(current.finish_without_main(Ok(())).is_err());
    assert!(rt.admit_android_global_custody().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_stopping_custody_keeps_real_child_and_retires_legacy_observers() {
    let (rt, _dir) = test_runtime();
    let wrong = RunIdentity::new();
    assert!(matches!(
        rt.child
            .lock()
            .unwrap()
            .reserve_stopping_without_worker_for_test(&wrong),
        Err(ReserveStoppingError::Empty)
    ));
    assert!(rt.child.lock().unwrap().is_empty());

    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    let pid = run.child_id_for_test().expect("stand-in PID");
    assert!(rt.child.lock().unwrap().install_running(run).is_ok());
    assert!(matches!(
        rt.child
            .lock()
            .unwrap()
            .reserve_stopping_without_worker_for_test(&wrong),
        Err(ReserveStoppingError::WrongRun)
    ));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert!(rt.child.lock().unwrap().empty_for_install().is_none());

    let Err(rejected) = rt
        .child
        .lock()
        .unwrap()
        .install_running(DirectCoreRun::new(spawn_custody_stand_in()))
    else {
        panic!("Running rejects replacement and returns its Child");
    };
    let mut returned_child = rejected.into_child_for_test();
    assert!(returned_child.id().is_some());
    returned_child.kill().await.expect("reap rejected Child");

    rt.spawn_crash_monitor(rt.gate.generation(), Some(identity.clone()));
    let observation = rt
        .child
        .lock()
        .unwrap()
        .reserve_stopping_without_worker_for_test(&identity)
        .expect("the exact Running identity reserves Stopping");
    assert!(observation.same_run(&identity));
    drop(observation);
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    assert!(rt.child.lock().unwrap().empty_for_install().is_none());
    assert!(pid_alive(pid), "dropping observation cannot release Child");
    assert!(matches!(
        rt.child
            .lock()
            .unwrap()
            .reserve_stopping_without_worker_for_test(&identity),
        Err(ReserveStoppingError::Busy)
    ));
    assert!(matches!(
        rt.child.lock().unwrap().take_running_legacy(),
        Err(TakeRunningError::Stopping)
    ));
    assert!(!rt.child.lock().unwrap().is_running_alive());
    assert!(!rt.child.lock().unwrap().running_matches(&identity));
    assert!(!rt.child.lock().unwrap().running_exit_proven(&identity));

    let Err(rejected) = rt
        .child
        .lock()
        .unwrap()
        .install_running(DirectCoreRun::new(spawn_custody_stand_in()))
    else {
        panic!("Stopping rejects replacement and returns its Child");
    };
    let mut returned_child = rejected.into_child_for_test();
    assert!(returned_child.id().is_some());
    returned_child.kill().await.expect("reap rejected Child");
    assert!(
        rt.kill_core().await.is_err(),
        "legacy Stop cannot take Stopping"
    );
    assert!(
        pid_alive(pid),
        "legacy Stop cannot signal the reserved Child"
    );

    tokio::time::timeout(Duration::from_secs(3), async {
        while Arc::strong_count(&identity.0) != 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the old crash monitor retires when its run is Stopping");
    assert_eq!(rt.crash_lock().restart_count(), 0);
    assert!(rt.child.lock().unwrap().is_stopping_for_test());
    assert!(pid_alive(pid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn occupied_direct_slot_rejects_start_before_retiring_old_session() {
    let (rt, _dir, clearer_calls) = test_runtime_recording();
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    let pid = run.child_id_for_test().expect("old core PID");
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        ..Default::default()
    };
    rt.set_race_server(5353, vec!["1.1.1.1".into()], vec![443]);
    rt.core_via_helper.store(true, Ordering::SeqCst);
    let old_generation = rt.gate.generation();
    let starter = {
        let mut slot = rt.child.lock().unwrap();
        let starter = {
            let rt = Arc::clone(&rt);
            tokio::spawn(async move { rt.start(local_only_config(free_port())).await })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while rt.crash_recovery.try_lock().is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "start must reach crash→Child admission"
            );
            std::thread::yield_now();
        }
        slot.install_running_for_test(run);
        starter
    };

    let error = starter
        .await
        .expect("start task completes")
        .expect_err("occupied slot must reject a second start");
    assert!(error.admission_denied);
    assert_eq!(rt.gate.generation(), old_generation);
    assert_eq!(clearer_calls.load(Ordering::SeqCst), 0);
    assert_eq!(rt.race_server_port(), 5353);
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.status().running);
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert!(pid_alive(pid));

    rt.core_via_helper.store(false, Ordering::SeqCst);
    let mut old_run = rt.child.lock().unwrap().take_running_for_test().unwrap();
    old_run
        .child_for_test()
        .kill()
        .await
        .expect("reap old stand-in");
}

#[cfg(not(target_os = "android"))]
#[tokio::test]
async fn helper_attempt_rejects_explicit_start_before_claim_or_crash_reset() {
    for known in [false, true] {
        let (rt, _dir) = test_runtime();
        let attempt = rt.register_helper_start_backend().unwrap();
        assert!(rt
            .child
            .lock()
            .unwrap()
            .finish_helper_start(&attempt, known.then_some(4242)));
        if known {
            *rt.pid.lock().unwrap() = Some(4242);
        }
        rt.crash_lock().mark_user_aborted();
        let generation = rt.gate.generation();
        let sweeps = rt.stale_sweep_runs.load(Ordering::SeqCst);
        let restart_count = rt.crash_lock().restart_count();

        let error = rt
            .start(local_only_config(free_port()))
            .await
            .expect_err("an unresolved helper attempt must deny explicit Start");
        assert!(error.admission_denied);
        assert_eq!(rt.gate.generation(), generation);
        assert_eq!(rt.stale_sweep_runs.load(Ordering::SeqCst), sweeps);
        assert!(rt.crash_lock().auto_restart_aborted());
        assert_eq!(rt.crash_lock().restart_count(), restart_count);
        assert!(rt.core_via_helper.load(Ordering::SeqCst));
        assert_eq!(*rt.pid.lock().unwrap(), known.then_some(4242));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_direct_slot_rechecks_after_waiting_for_ts_gate() {
    let (rt, _dir, clearer_calls) = test_runtime_recording();
    let held_gate = rt.mesh.tailscale_state_gate().await;
    let old_generation = rt.gate.generation();
    let starter = {
        let rt = Arc::clone(&rt);
        tokio::spawn(async move { rt.start(local_only_config(free_port())).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while rt.gate.generation() == old_generation {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("start claims a generation before waiting for the TS gate");

    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    let pid = run.child_id_for_test().expect("old core PID");
    rt.child.lock().unwrap().install_running_for_test(run);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        ..Default::default()
    };
    rt.set_race_server(5353, vec!["1.1.1.1".into()], vec![443]);
    drop(held_gate);

    let error = starter
        .await
        .expect("start task completes")
        .expect_err("late occupied slot must reject before preflight");
    assert!(error.admission_denied);
    assert_eq!(clearer_calls.load(Ordering::SeqCst), 0);
    assert_eq!(rt.race_server_port(), 5353);
    assert!(rt.status().running);
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert!(pid_alive(pid));

    let mut old_run = rt.child.lock().unwrap().take_running_for_test().unwrap();
    old_run
        .child_for_test()
        .kill()
        .await
        .expect("reap old stand-in");
}

#[tokio::test(flavor = "multi_thread")]
async fn newly_claimed_start_supersedes_an_old_direct_spawn() {
    let (rt, _dir) = test_runtime();
    rt.stale_sweep_disabled.store(true, Ordering::SeqCst);
    let held_gate = rt.mesh.tailscale_state_gate().await;
    let old_generation = rt.gate.generation();
    let starter = {
        let rt = Arc::clone(&rt);
        tokio::spawn(async move { rt.start(local_only_config(free_port())).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while rt.gate.generation() == old_generation {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("new start claims while the direct slot is Empty");

    // The previous start reaches its Child critical section only after this
    // claim. It must see the newer generation and leave the slot Empty.
    {
        let slot = rt.child.lock().unwrap();
        assert!(slot.is_empty());
        assert_ne!(rt.gate.generation(), old_generation);
    }
    let old_result = rt
        .start_inner(local_only_config(free_port()), old_generation, &held_gate)
        .await
        .expect("superseded start yields without a Child");
    assert!(!old_result.running);
    assert!(rt.child.lock().unwrap().is_empty());

    drop(held_gate);
    let _ = starter.await.expect("new start task completes");
}

/// ⑩ stale-core 清扫：**本 app** 孤儿被清 + **非本 app** 的 sing-box **不被误杀**（最关键的安全点）。
///
/// - 「本 app 孤儿」= 用 `POLARIS_SINGBOX_PATH` 指向的核二进制直接 spawn（不经 ProxyRuntime → 无句柄管理）。
/// - 「非本 app」= 把同一核**复制到另一路径**再起 → argv[0] 路径不同 → `is_our_core` 判 false → 存活。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "真机验证：需 POLARIS_SINGBOX_PATH 指向真实 sing-box；非 CI 门"]
async fn real_core_stale_cleanup_kills_own_orphan_spares_foreign() {
    let _real_core_guard = lock_real_core_tests().await;
    use std::process::Stdio;
    let (rt, dir, core) = real_core_runtime();
    crate::logging::init(&dir);

    // ── 孤儿①（本 app）：用本 app 核路径直接 spawn，不经 ProxyRuntime → 成孤儿 ──
    let ours_cfg = dir.join("orphan-ours.json");
    write_bare_singbox_config(&ours_cfg, free_port());
    let mut ours_orphan = tokio::process::Command::from(crate::runtime::kernel_run::with_run(
        std::process::Command::new(&core),
    ))
    .args(["-c", ours_cfg.to_str().unwrap(), "--disable-color"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("spawn 本 app 孤儿核");
    let ours_pid = ours_orphan.id().expect("本 app 孤儿 pid");
    // 清扫器在 SIGKILL 后会再次探活。若测试自己一直持有未 wait 的 Child，Linux 会把已死进程
    // 留成 zombie，`kill(pid, 0)` 仍会报存在，清扫器便会误判为 EPERM/root survivor。
    // 独立 reaper 从一开始就等待：既不参与杀进程，也能在清扫杀掉它后立即收割。
    let ours_reaper = tokio::spawn(async move { ours_orphan.wait().await });

    // ── 「非本 app」sing-box：复制核到异路径再起 → 路径不同 → 绝不该被误杀 ──
    let foreign_bin = dir.join("foreign-sing-box");
    std::fs::copy(&core, &foreign_bin).expect("复制核到异路径（std::fs::copy 保留可执行位）");
    let foreign_cfg = dir.join("foreign.json");
    write_bare_singbox_config(&foreign_cfg, free_port());
    let mut foreign = tokio::process::Command::from(crate::runtime::kernel_run::with_run(
        std::process::Command::new(&foreign_bin),
    ))
    .args(["-c", foreign_cfg.to_str().unwrap(), "--disable-color"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("spawn 非本 app sing-box（异路径）");
    let foreign_pid = foreign.id().expect("非本 app sing-box pid");

    // 等两个核都真正起来。
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(ps_alive(ours_pid), "[⑩] 前提：本 app 孤儿在跑");
    assert!(ps_alive(foreign_pid), "[⑩] 前提：非本 app sing-box 在跑");
    println!("[⑩] 本 app 孤儿 pid={ours_pid}（{}）", core.display());
    println!(
        "[⑩] 非本 app sing-box pid={foreign_pid}（{}）",
        foreign_bin.display()
    );

    // ── stale 清扫：按本 app 二进制路径精确判定 ──
    // 同用户起的孤儿用户态就杀得动 → 不该走到 T3 提权腿，必须干净返回 Ok。
    assert!(
        rt.cleanup_stale_cores().await.is_ok(),
        "[⑩] 同用户孤儿用户态可杀 → 不得落 ROOT_ORPHAN_BLOCKED"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    // reaper 应在清扫后立即拿到退出状态；超时表示进程其实仍在跑。
    tokio::time::timeout(Duration::from_secs(3), ours_reaper)
        .await
        .expect("[⑩] 本 app 孤儿必须在清扫后被 reaper 收割（超时=仍在跑）")
        .expect("[⑩] reaper 任务不应 panic")
        .expect("[⑩] wait 本 app 孤儿不应失败");
    // 非本 app sing-box（异路径）genuinely 存活（未被杀、非 zombie）→ ps_alive 判据可靠。
    assert!(
        ps_alive(foreign_pid),
        "[⑩] **核心安全点**：非本 app 的 sing-box pid={foreign_pid}（异路径）绝不能被误杀"
    );
    println!(
        "[⑩] 本 app 孤儿已清（wait 收割确认退出）+ 非本 app sing-box 存活 → 只杀自己、不误杀他人 ✓"
    );

    // 收尾：清掉 foreign（本 app 孤儿已收割）。
    send_signal(foreign_pid, Signal::Sigkill);
    let _ = foreign.wait().await;
}

// ─── P1-b：起核收口腿必须让 daemon 停掉它自己的受管 child ──────────────────────────

use std::sync::atomic::AtomicUsize;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn failed_stop_reservation_keeps_its_real_child_monitor_and_retires_on_replacement() {
    use crate::runtime::config::{ApplyCasExpected, ApplyPersistError};
    use crate::runtime::proxy::mesh_apply::{ApplyClaim, ApplyStep};
    use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
    use polaris_store::mesh_guard::{POLICY_KEY, REQUIRED_MARKER_FILE, STATE_KEY};

    let (rt, dir) = test_runtime();
    let wire: Value = serde_json::from_str(&crate::test_support::repo_file(
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
        plan_id: "run-identity-cas".into(),
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
                    manifest_ref: "mesh-routes/plans/run-identity-cas/manifest.json",
                },
            )
        })
        .unwrap()
        .unwrap();
    let claim = ApplyClaim::from(prepared.transaction.as_ref().unwrap());

    let child = tokio::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn real local child");
    let run = DirectCoreRun::new(child);
    let identity = run.identity.clone();
    let old_run_ref = identity.persisted_ref().to_string();
    rt.child.lock().unwrap().install_running_for_test(run);
    rt.spawn_crash_monitor(old_generation, Some(identity.clone()));
    tokio::time::sleep(Duration::from_millis(CRASH_MONITOR_POLL_MS + 100)).await;
    assert_eq!(
        Arc::strong_count(&identity.0),
        3,
        "child, test and monitor own the run"
    );

    // A real persistent CAS rejection occurs after Stop claims a request generation.
    // No teardown ran; the old Child remains the physical owner.
    rt.config
        .set_value("logLevel", serde_json::json!("debug"))
        .unwrap();
    let stop_generation = rt
        .gate
        .claim_generation(Some(old_generation), LifecycleKind::Stop)
        .unwrap();
    let rejected = rt
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
                &old_run_ref,
                old_generation,
                stop_generation,
            )
        })
        .unwrap();
    assert!(matches!(
        rejected,
        Err(ApplyPersistError::StopReservationUncertain(cause))
            if matches!(*cause, ApplyPersistError::ConfigChanged)
    ));
    assert!(
        rt.admit_legacy_start().is_err(),
        "a failed managed Stop claim cannot reopen legacy crash restart"
    );
    tokio::time::sleep(Duration::from_millis(CRASH_MONITOR_POLL_MS + 100)).await;
    assert_eq!(
        Arc::strong_count(&identity.0),
        3,
        "failed Stop CAS must not retire the old child's live monitor"
    );
    assert!(matches!(
        rt.child
            .lock()
            .unwrap()
            .running_for_test()
            .unwrap()
            .try_wait_with(tokio::process::Child::try_wait),
        Ok(None)
    ));

    // Replace the slot while the old monitor is alive. It must release only
    // its own token and never inspect or classify the newer live child.
    let next = DirectCoreRun::new(
        tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn replacement child"),
    );
    let next_identity = next.identity.clone();
    let mut old = {
        let mut slot = rt.child.lock().unwrap();
        let old = slot.take_running_for_test().unwrap();
        slot.install_running_for_test(next);
        old
    };
    old.child_for_test().kill().await.unwrap();
    drop(old);
    tokio::time::sleep(Duration::from_millis(CRASH_MONITOR_POLL_MS + 100)).await;
    assert_eq!(Arc::strong_count(&identity.0), 1, "old monitor retired");
    assert_eq!(
        rt.crash_lock().restart_count(),
        0,
        "old monitor must not classify the replacement as its own crash"
    );
    assert!(rt
        .child
        .lock()
        .unwrap()
        .running_for_test()
        .unwrap()
        .identity
        .same_run(&next_identity));
    assert!(matches!(
        rt.child
            .lock()
            .unwrap()
            .running_for_test()
            .unwrap()
            .try_wait_with(tokio::process::Child::try_wait),
        Ok(None)
    ));
    rt.kill_core().await.unwrap();
}

/// 可观测的 [`HelperStopOps`] 替身：记调用次数 + 每次带的身份 pid，并可被指定成失败腿。
///
/// `during_call` 在「IPC 往返中」执行 —— 用来**确定性**地复现「停核请求在飞、期间新会话起了新核」
/// 那条时序（真机上它是 helper 无响应 + 用户重装 helper 的窗口，靠 sleep 撞不出来）。
struct RecordingStop {
    calls: Arc<AtomicUsize>,
    wants: Arc<Mutex<Vec<Option<u32>>>>,
    targets: Arc<Mutex<Vec<crate::runtime::helper::HelperStopTarget>>>,
    result: Result<(), String>,
    during_call: Option<Box<dyn Fn() + Send + Sync>>,
}
type StopProbe = (
    Arc<RecordingStop>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Option<u32>>>>,
);
impl RecordingStop {
    fn new(result: Result<(), String>) -> StopProbe {
        Self::with_hook(result, None)
    }
    fn with_hook(
        result: Result<(), String>,
        during_call: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> StopProbe {
        let calls = Arc::new(AtomicUsize::new(0));
        let wants = Arc::new(Mutex::new(Vec::new()));
        let targets = Arc::new(Mutex::new(Vec::new()));
        let ops = Arc::new(Self {
            calls: Arc::clone(&calls),
            wants: Arc::clone(&wants),
            targets,
            result,
            during_call,
        });
        (ops, calls, wants)
    }
}
impl HelperStopOps for RecordingStop {
    fn stop_managed_core(
        &self,
        target: crate::runtime::helper::HelperStopTarget,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.wants.lock().unwrap().push(Some(target.pid()));
        self.targets.lock().unwrap().push(target);
        if let Some(f) = self.during_call.as_ref() {
            f();
        }
        self.result.clone()
    }
}

fn known_helper_attempt(rt: &Arc<ProxyRuntime>, pid: u32) -> HelperStartToken {
    let token = rt.child.lock().unwrap().known_helper_start_for_test(pid);
    *rt.pid.lock().unwrap() = Some(pid);
    rt.core_via_helper.store(true, Ordering::SeqCst);
    token
}

fn exact_helper_target(pid: u32, birth: &str) -> polaris_helper_proto::HelperBirthTarget {
    polaris_helper_proto::HelperBirthTarget::parse_wire(&pid.to_string(), birth).unwrap()
}

fn known_exact_helper_attempt(
    rt: &Arc<ProxyRuntime>,
    target: polaris_helper_proto::HelperBirthTarget,
) -> HelperStartToken {
    use super::super::startup::HelperStartCompletion;
    let attempt = rt.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(rt, attempt.clone())
        .publish(&Ok(
            crate::runtime::helper::HelperStartResult::BirthStarted(target),
        ))
        .unwrap();
    attempt
}

#[tokio::test]
async fn exact_stop_permit_binds_birth_and_keeps_managed_gate() {
    use crate::runtime::helper::HelperStopTarget;
    let (rt, _dir) = test_runtime();
    let a = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let b = exact_helper_target(4242, "11112222333344445555666677778888");
    let attempt = known_exact_helper_attempt(&rt, a);
    let (_, reserved, nonce) = rt.child.lock().unwrap().begin_helper_stop().unwrap();
    assert_eq!(reserved, HelperStopTarget::Birth(a));
    let forged = HelperStopPermit::new(
        Arc::clone(&rt.child),
        attempt.clone(),
        HelperStopTarget::Birth(b),
        nonce,
    );
    assert!(!rt
        .clear_helper_core_bookkeeping_with_main(&forged, |_| Err("no test claim release".into()))
        .unwrap());
    assert_eq!(*rt.pid.lock().unwrap(), Some(4242));
    assert!(rt.child.lock().unwrap().has_helper_start());
    drop(forged);

    let (ops, _, _) = RecordingStop::new(Ok(()));
    let stopped_attempt = rt
        .kill_core_via_helper(Arc::clone(&ops) as Arc<dyn HelperStopOps>)
        .await
        .unwrap();
    assert!(stopped_attempt.same(&attempt));
    assert_eq!(*ops.targets.lock().unwrap(), [HelperStopTarget::Birth(a)]);
    assert!(rt.pid.lock().unwrap().is_none());
    assert!(!rt.child.lock().unwrap().has_helper_start());
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.start(local_only_config(free_port())).await.is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn reaped_exact_birth_allows_same_helper_start_leg_but_not_direct_start() {
    use super::super::lifecycle::StartLeg;

    let (rt, _dir, _prompts) = test_runtime_installed_helper(false, false);
    let old = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    known_exact_helper_attempt(&rt, old);
    let (ops, _, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .unwrap();
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.child.lock().unwrap().helper_touched_for_test());

    let before = rt.gate.generation();
    assert!(rt.start(local_only_config(free_port())).await.is_err());
    assert_eq!(
        rt.gate.generation(),
        before,
        "direct admission stays closed"
    );

    // Both explicit Stop→Start and the restart/Apply start leg pass this same
    // admission. The test helper has no daemon socket, so the subsequent
    // read-only capability probe fails before a new helper attempt is booked.
    let error = rt.start(tun_config()).await.unwrap_err();
    assert!(
        error.to_string().contains("exact birth 能力探测"),
        "helper Start reached a different failure: {error}"
    );
    assert!(rt.gate.generation() > before);
    assert!(!rt.child.lock().unwrap().has_helper_start());
    let guarded = rt
        .start_guarded(tun_config(), Some(rt.gate.generation()))
        .await;
    assert!(matches!(guarded, StartLeg::Finished(Err(_), _)));
    assert!(!rt.child.lock().unwrap().has_helper_start());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
}

#[tokio::test]
async fn reaped_exact_start_releases_only_its_reserved_main_claim() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::{HelperStartResult, HelperStopTarget};

    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-reaped-exact").unwrap();
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
    let attempt = rt
        .register_helper_start_backend_with_main(Some(token))
        .unwrap();
    reservation.arm_external_start();
    let target = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    HelperStartCompletion::for_test(&rt, attempt.clone())
        .publish(&Ok(HelperStartResult::BirthStarted(target)))
        .unwrap();
    assert!(rt.mesh.main_owns_tailscale("ts-reaped-exact", true));

    let (ops, calls, _) = RecordingStop::new(Ok(()));
    let (message, confirmed) = rt
        .reject_helper_start_with_result(
            ops.clone(),
            &attempt,
            HelperStopTarget::Birth(target),
            Some(&mut reservation),
        )
        .await;
    assert!(confirmed, "same-attempt native Stop ACK: {message}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(reservation);
    assert!(!rt.mesh.main_owns_tailscale("ts-reaped-exact", true));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
}

#[tokio::test]
async fn exact_start_cleanup_keeps_reservation_custody_when_registry_compare_remove_fails() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::{HelperStartResult, HelperStopTarget};
    for poison in [false, true] {
        let (rt, _dir) = test_runtime();
        let gate = rt.mesh.tailscale_state_gate().await;
        let state = rt.mesh.tailscale_state_dir("start-cleanup-claim").unwrap();
        let generated =
            serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]});
        let token = rt.mesh.mint_tailscale_main_birth();
        let mut reservation = rt
            .mesh
            .reserve_tailscale_main_states(&generated, &gate, token.clone())
            .await
            .unwrap();
        let attempt = rt
            .register_helper_start_backend_with_main(Some(token.clone()))
            .unwrap();
        reservation.arm_external_start();
        let target = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
        HelperStartCompletion::for_test(&rt, attempt.clone())
            .publish(&Ok(HelperStartResult::BirthStarted(target)))
            .unwrap();
        let successor = rt.mesh.mint_tailscale_main_birth();
        if poison {
            rt.mesh.poison_tailscale_main_claim_lock_for_test();
        } else {
            assert!(rt
                .mesh
                .release_tailscale_main_states_if_token(&token, &gate)
                .unwrap());
            let mut replacement = rt
                .mesh
                .reserve_tailscale_main_states(&generated, &gate, successor.clone())
                .await
                .unwrap();
            replacement.arm_external_start();
            drop(replacement);
        }
        let (ops, calls, _) = RecordingStop::new(Ok(()));
        let (message, confirmed) = rt
            .reject_helper_start_with_result(
                ops.clone(),
                &attempt,
                HelperStopTarget::Birth(target),
                Some(&mut reservation),
            )
            .await;
        assert!(!confirmed, "{message}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(*rt.pid.lock().unwrap(), Some(4242));
        assert!(rt.child.lock().unwrap().has_helper_start());
        assert!(rt.core_via_helper.load(Ordering::SeqCst));
        assert!(rt.mesh.main_owns_tailscale("start-cleanup-claim", true));
        if !poison {
            assert!(
                rt.mesh
                    .release_tailscale_main_states_if_token(&successor, &gate)
                    .unwrap(),
                "successor survived"
            );
            let mut restored = rt
                .mesh
                .reserve_tailscale_main_states(&generated, &gate, token)
                .await
                .unwrap();
            restored.arm_external_start();
            drop(restored);
            let (_, confirmed) = rt
                .reject_helper_start_with_result(
                    ops,
                    &attempt,
                    HelperStopTarget::Birth(target),
                    Some(&mut reservation),
                )
                .await;
            assert!(confirmed);
            assert!(!rt.child.lock().unwrap().has_helper_start());
            assert_eq!(*rt.pid.lock().unwrap(), None);
            assert!(!rt.mesh.main_owns_tailscale("start-cleanup-claim", true));
        }
        drop(reservation);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn mock_daemon_exact_start_stop_start_uses_production_transport_and_custody() {
    use crate::runtime::helper::HelperStopTarget;

    let mut a_process = spawn_custody_stand_in();
    let mut b_process = spawn_custody_stand_in();
    let a_pid = a_process.id().unwrap();
    let b_pid = b_process.id().unwrap();
    let a_hex = "00112233445566778899aabbccddeeff";
    let b_hex = "11112222333344445555666677778888";
    let (rt, dir, frames) = birth_daemon_runtime([
        format!("OK birth-started {a_pid} {a_hex}\n"),
        format!("OK birth-stopped {a_pid} {a_hex}\n"),
        format!("OK birth-started {b_pid} {b_hex}\n"),
    ]);
    let gate = rt.mesh.tailscale_state_gate().await;
    let user_config: UserConfig = serde_json::from_value(tun_config()).unwrap();
    let binary = dir.join("missing-test-core"); // Reconcile is best effort; no host helper.
    let config_path = dir.join("test-core.json");
    let mut a_reservation = rt
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints": []}),
            &gate,
            rt.mesh.mint_tailscale_main_birth(),
        )
        .await
        .unwrap();
    assert_eq!(
        rt.spawn_core_via_helper(
            &binary,
            &config_path,
            &user_config,
            rt.gate.generation(),
            &mut a_reservation,
        )
        .await
        .unwrap(),
        Some(a_pid)
    );
    drop(a_reservation);
    let a_target = exact_helper_target(a_pid, a_hex);
    assert_eq!(
        rt.child.lock().unwrap().helper_stop_target().unwrap().1,
        HelperStopTarget::Birth(a_target)
    );
    let a_attempt = rt.child.lock().unwrap().helper_stop_target().unwrap().0;
    drop(gate);
    rt.stop().await.unwrap();
    assert!(!rt.child.lock().unwrap().has_helper_start());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    a_process.start_kill().unwrap();
    a_process.wait().await.unwrap();

    let gate = rt.mesh.tailscale_state_gate().await;
    let mut b_reservation = rt
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints": []}),
            &gate,
            rt.mesh.mint_tailscale_main_birth(),
        )
        .await
        .unwrap();
    assert_eq!(
        rt.spawn_core_via_helper(
            &binary,
            &config_path,
            &user_config,
            rt.gate.generation(),
            &mut b_reservation,
        )
        .await
        .unwrap(),
        Some(b_pid)
    );
    drop(b_reservation);
    let (current, target) = rt.child.lock().unwrap().helper_stop_target().unwrap();
    assert!(!current.same(&a_attempt));
    assert_eq!(
        target,
        HelperStopTarget::Birth(exact_helper_target(b_pid, b_hex))
    );
    {
        let sent = frames.lock().unwrap();
        assert_eq!(
            sent.len(),
            3,
            "only Start A, Stop A, Start B may reach the daemon"
        );
        assert!(sent[0].starts_with("start-birth-safe\n"));
        assert_eq!(sent[1], format!("stop-birth-safe\n{a_pid}\n{a_hex}\n"));
        assert!(sent[2].starts_with("start-birth-safe\n"));
    }
    b_process.start_kill().unwrap();
    b_process.wait().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dead_exact_start_spawn_branch_releases_reserved_main_claim_after_stop_ack() {
    let pid = u32::MAX; // checked_pid rejects it without probing any host process.
    let hex = "00112233445566778899aabbccddeeff";
    let (rt, dir, frames) = birth_daemon_runtime([
        format!("OK birth-started {pid} {hex}\n"),
        format!("OK birth-stopped {pid} {hex}\n"),
    ]);
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-dead-exact-spawn").unwrap();
    let token = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(
            &serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]}),
            &gate,
            token,
        )
        .await
        .unwrap();
    assert!(rt.mesh.main_owns_tailscale("ts-dead-exact-spawn", true));
    let user_config: UserConfig = serde_json::from_value(tun_config()).unwrap();
    let err = rt
        .spawn_core_via_helper(
            &dir.join("missing-test-core"),
            &dir.join("test-core.json"),
            &user_config,
            rt.gate.generation(),
            &mut reservation,
        )
        .await
        .unwrap_err();
    assert!(err.contains("进程不存在"), "{err}");
    drop(reservation);
    assert!(!rt.mesh.main_owns_tailscale("ts-dead-exact-spawn", true));
    assert!(!rt.child.lock().unwrap().has_helper_start());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    let sent = frames.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].starts_with("start-birth-safe\n"));
    assert_eq!(sent[1], format!("stop-birth-safe\n{pid}\n{hex}\n"));
}

#[tokio::test]
async fn exact_not_admitted_target_can_stop_but_unknown_without_target_cannot() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::{HelperStartResult, HelperStopTarget};
    let target = exact_helper_target(7331, "00112233445566778899aabbccddeeff");
    let (rt, _dir) = test_runtime();
    let attempt = rt.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(&rt, attempt)
        .publish(&Ok(HelperStartResult::BirthNotAdmitted {
            target: Some(target),
            pending: true,
        }))
        .unwrap();
    assert!(rt.pid.lock().unwrap().is_none());
    let (ops, _, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper(Arc::clone(&ops) as Arc<dyn HelperStopOps>)
        .await
        .unwrap();
    assert_eq!(
        *ops.targets.lock().unwrap(),
        [HelperStopTarget::Birth(target)]
    );
    assert!(rt.core_via_helper.load(Ordering::SeqCst));

    let (unknown, _dir) = test_runtime();
    let attempt = unknown.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(&unknown, attempt)
        .publish(&Ok(HelperStartResult::BirthNotAdmitted {
            target: None,
            pending: false,
        }))
        .unwrap();
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    assert!(unknown
        .kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .unwrap_err()
        .contains("cleanup-unknown"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(unknown.child.lock().unwrap().has_helper_start());
    assert!(unknown.core_via_helper.load(Ordering::SeqCst));

    let (already, _dir) = test_runtime();
    let attempt = already.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(&already, attempt)
        .publish(&Ok(HelperStartResult::BirthAlready(target)))
        .unwrap();
    assert!(
        already.pid.lock().unwrap().is_none(),
        "Already did not start this config"
    );
    assert_eq!(
        already
            .child
            .lock()
            .unwrap()
            .helper_stop_target()
            .unwrap()
            .1,
        HelperStopTarget::Birth(target)
    );
}

#[tokio::test]
async fn unknown_helper_start_refuses_stop_and_stale_sweep_without_ipc() {
    let (rt, _dir) = test_runtime();
    let attempt = rt.register_helper_start_backend().unwrap();
    assert!(rt.child.lock().unwrap().finish_helper_start(&attempt, None));
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    let error = rt
        .kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .unwrap_err();
    assert!(error.contains("cleanup-unknown"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "unknown pid forbids Stop(None)"
    );
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.pid.lock().unwrap().is_none());

    let sweeps = rt.stale_sweep_runs.load(Ordering::SeqCst);
    assert!(rt.start(local_only_config(free_port())).await.is_err());
    assert_eq!(rt.stale_sweep_runs.load(Ordering::SeqCst), sweeps);

    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        started_via_helper: true,
        ..Default::default()
    };
    let error = rt.stop().await.unwrap_err();
    assert!(error.contains("cleanup-unknown"));
    assert!(
        rt.status().running,
        "unknown helper Start cannot publish stopped"
    );
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_helper_start_worker_publishes_late_exact_birth() {
    use super::super::startup::HelperStartCompletion;

    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-late-helper").unwrap();
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
    let attempt = rt
        .register_helper_start_backend_with_main(Some(token.clone()))
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    drop(gate);
    let mut completion = HelperStartCompletion::for_test(&rt, attempt.clone());
    let target = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = Arc::clone(&release);
    let late = tokio::task::spawn_blocking(move || {
        entered_tx.send(()).unwrap();
        let (lock, wake) = &*held;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
        completion
            .publish(&Ok(
                crate::runtime::helper::HelperStartResult::BirthStarted(target),
            ))
            .unwrap();
    });
    entered_rx.await.unwrap();
    late.abort();
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while *rt.pid.lock().unwrap() != Some(4242) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (current, known) = rt.child.lock().unwrap().helper_stop_target().unwrap();
    assert!(current.same(&attempt));
    assert_eq!(
        known,
        crate::runtime::helper::HelperStopTarget::Birth(target)
    );
    let (claim_attempt, claim_main) = rt
        .child
        .lock()
        .unwrap()
        .helper_main_claim_for_stop()
        .unwrap();
    assert!(claim_attempt.same(&attempt));
    assert!(claim_main.is_some_and(|found| found.same(&token)));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&token)));
    assert!(rt.mesh.main_owns_tailscale("ts-late-helper", true));
}

#[test]
fn late_old_attempt_ack_cannot_replace_same_pid_new_birth() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::{HelperStartResult, HelperStopTarget};
    let (rt, _dir) = test_runtime();
    let a = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let b = exact_helper_target(4242, "11112222333344445555666677778888");
    let old = rt.register_helper_start_backend().unwrap();
    let mut late = HelperStartCompletion::for_test(&rt, old.clone());
    {
        let mut child = rt.child.lock().unwrap();
        assert!(child.finish_helper_birth_start(&old, a));
        let (_, target, nonce) = child.begin_helper_stop().unwrap();
        assert!(child.confirm_helper_stop(&old, target, &nonce));
    }
    // A same-backend successor may start after exact Stop. Its receipt must
    // not be overwritten by the prior attempt's delayed Start completion.
    let new = rt.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(&rt, new.clone())
        .publish(&Ok(HelperStartResult::BirthStarted(b)))
        .unwrap();
    assert!(late
        .publish(&Ok(HelperStartResult::BirthStarted(a)))
        .is_err());
    let (current, target) = rt.child.lock().unwrap().helper_stop_target().unwrap();
    assert!(current.same(&new));
    assert_eq!(target, HelperStopTarget::Birth(b));
    assert_eq!(*rt.pid.lock().unwrap(), Some(4242));
}

#[tokio::test]
async fn start_not_admitted_preserves_prior_helper_and_main_custody() {
    use super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::HelperStartResult;
    use polaris_helper_proto::StartNotAdmitted;

    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("ts-helper-blocker").unwrap();
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
    let attempt = rt
        .register_helper_start_backend_with_main(Some(token.clone()))
        .unwrap();
    reservation.arm_external_start();
    let mut completion = HelperStartCompletion::for_test(&rt, attempt.clone());
    completion
        .publish(&Ok(HelperStartResult::NotAdmitted(
            StartNotAdmitted::Pending { pid: 7331 },
        )))
        .unwrap();
    drop(reservation);

    {
        let child = rt.child.lock().unwrap();
        assert!(child.helper_start_not_admitted_for_test());
        let (blocker_attempt, blocker_pid) = child.helper_stop_target().unwrap();
        assert!(blocker_attempt.same(&attempt));
        assert_eq!(blocker_pid.pid(), 7331);
    }
    assert!(rt.pid.lock().unwrap().is_none(), "no new core was spawned");
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.mesh.main_owns_tailscale("ts-helper-blocker", true));

    let (failed, failed_calls, wants) = RecordingStop::new(Err("still pending".into()));
    assert!(rt
        .kill_core_via_helper(failed as Arc<dyn HelperStopOps>)
        .await
        .is_err());
    assert_eq!(failed_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*wants.lock().unwrap(), [Some(7331)]);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .helper_start_not_admitted_for_test());
    assert!(rt.mesh.main_owns_tailscale("ts-helper-blocker", true));

    let (acknowledged, _, _) = RecordingStop::new(Ok(()));
    assert!(rt
        .kill_core_via_helper_with_main(acknowledged as Arc<dyn HelperStopOps>, Some(&gate))
        .await
        .unwrap_err()
        .contains("legacy Stop ACK"));
    assert!(rt.child.lock().unwrap().has_helper_start());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|found| found.same(&token)));
    assert!(rt.mesh.main_owns_tailscale("ts-helper-blocker", true));
}

#[tokio::test]
async fn acknowledged_stop_keeps_exclusive_permit_through_final_clear() {
    let (rt, _dir) = test_runtime();
    known_exact_helper_attempt(
        &rt,
        exact_helper_target(4242, "00112233445566778899aabbccddeeff"),
    );
    let (token, pid, nonce) = rt.child.lock().unwrap().begin_helper_stop().unwrap();
    let permit = HelperStopPermit::new(Arc::clone(&rt.child), token.clone(), pid, nonce);
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    ops.stop_managed_core(pid).unwrap(); // ACK arrived; async caller has not cleared yet.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rt.child.lock().unwrap().helper_stop_inflight_for_test());

    let (second, second_calls, _) = RecordingStop::new(Ok(()));
    assert!(rt
        .kill_core_via_helper(second as Arc<dyn HelperStopOps>)
        .await
        .unwrap_err()
        .contains("cleanup-unknown"));
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    let sweeps = rt.stale_sweep_runs.load(Ordering::SeqCst);
    assert!(rt.start(local_only_config(free_port())).await.is_err());
    assert_eq!(rt.stale_sweep_runs.load(Ordering::SeqCst), sweeps);

    assert!(rt
        .clear_helper_core_bookkeeping_with_main(&permit, |_| Err("no test claim release".into()))
        .unwrap());
    drop(permit);
    assert!(rt.pid.lock().unwrap().is_none());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[cfg(not(target_os = "android"))]
#[tokio::test]
async fn cancelled_helper_stop_keeps_existing_legacy_lease_until_ipc_returns() {
    let (rt, dir) = test_runtime();
    known_helper_attempt(&rt, 4242);
    let outer_lease = rt.config.lease_legacy_start().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = Arc::clone(&release);
    let entered = Mutex::new(Some(entered_tx));
    let (ops, _, _) = RecordingStop::with_hook(
        Ok(()),
        Some(Box::new(move || {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            let (lock, wake) = &*held;
            let mut released = lock.lock().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !*released {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "test did not release helper IPC");
                released = wake.wait_timeout(released, remaining).unwrap().0;
            }
        })),
    );
    let task = tokio::spawn({
        let rt = Arc::clone(&rt);
        async move { rt.kill_core_via_helper(ops as Arc<dyn HelperStopOps>).await }
    });
    entered_rx.await.unwrap();
    task.abort();
    let _ = task.await;
    let direct = DirectCoreRun::new(spawn_custody_stand_in());
    let direct_identity = direct.identity.clone();
    assert!(rt.child.lock().unwrap().install_running(direct).is_ok());
    assert!(matches!(
        prepare_direct_stop(&rt, &direct_identity).await,
        Err(PrepareError::Unsupported)
    ));
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
    drop(outer_lease);
    assert!(rt.config.prepare_mesh_route_enable("local-test-1").is_err());
    assert!(!dir
        .join(polaris_store::mesh_guard::REQUIRED_MARKER_FILE)
        .exists());

    {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_one();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if rt.config.prepare_mesh_route_enable("local-test-1").is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "blocking IPC did not release its lease"
        );
        tokio::task::yield_now().await;
    }
    assert!(dir
        .join(polaris_store::mesh_guard::REQUIRED_MARKER_FILE)
        .exists());
    assert!(matches!(
        prepare_direct_stop(&rt, &direct_identity).await,
        Err(PrepareError::Unsupported)
    ));
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
    let mut direct = rt.child.lock().unwrap().take_running_for_test().unwrap();
    direct.child_for_test().kill().await.unwrap();
}

// ─── 停核的受管 pid 身份：app 侧下发 + 记账收口 ────────────────────────────────

/// **变异门（下发侧）**：helper 停核腿必须把「本腿意图停的那个 pid」**随请求带下去**。
///
/// 判据只能在 helper 进程里执行（真正杀进程的是它），app 不下发 = 判据永远拿不到 want =
/// daemon 退回「反正要停就杀当前的」。
///
/// 变异（逃逸面穷举）：
/// - `stop_managed_core(intended)` 改回 `stop_managed_core(None)` → 首条断言转红。
/// - 把 `let intended = ...` 挪到 await **之后**再读 → 读到的是新会话的 pid → 首条转红
///   （那等于把「我要停谁」交给接管方决定）。
#[tokio::test]
async fn helper_stop_leg_sends_the_pid_it_intends_to_stop() {
    let (rt, _dir) = test_runtime();
    known_helper_attempt(&rt, 4242);
    let (ops, calls, wants) = RecordingStop::new(Ok(()));

    assert!(rt
        .kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .unwrap_err()
        .contains("legacy Stop ACK"));

    assert_eq!(
        *wants.lock().unwrap(),
        vec![Some(4242)],
        "停核请求必须携带受管 pid 身份 —— 这是 helper 侧唯一能据以拒杀的依据"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "恰调一次");
    // Request acceptance cannot attest a native exit, even without takeover.
    assert_eq!(*rt.pid.lock().unwrap(), Some(4242));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

/// **结果未知门**：通信失败时不能清 helper 记账。请求可能根本没到，也可能已停但回包丢失；
/// 两种情况都只能保留身份，让上层停止失败而不是伪造 stopped。
#[tokio::test]
async fn helper_stop_failure_preserves_managed_identity() {
    let (rt, _dir) = test_runtime();
    known_helper_attempt(&rt, 4242);
    let (ops, calls, wants) = RecordingStop::new(Err("mock transport timeout".to_owned()));

    let error = rt
        .kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .expect_err("没有确定停核回执时必须向上报错");

    assert!(error.contains("timeout"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*wants.lock().unwrap(), vec![Some(4242)]);
    assert_eq!(
        *rt.pid.lock().unwrap(),
        Some(4242),
        "结果未知时清 pid 会让仍在跑的 SYSTEM/root 核失联"
    );
    assert!(
        rt.core_via_helper.load(Ordering::SeqCst),
        "结果未知时必须保留 helper 受管路径"
    );
}

/// 组合路径：`stop` 收到 helper 通信失败后，不清运行态、不发成功终态所依赖的 `Ok(())`。
/// `test_runtime` 的 helper 被结构性禁止连接真实 daemon，因此此测零宿主副作用。
#[tokio::test]
async fn active_stop_keeps_running_state_when_helper_stop_is_unconfirmed() {
    let (rt, _dir) = test_runtime();
    known_helper_attempt(&rt, 4242);
    {
        let mut status = rt.status.write().unwrap();
        status.running = true;
        status.started_via_helper = true;
    }

    let error = rt
        .stop()
        .await
        .expect_err("helper 未确认停核时 stop 必须失败");

    assert!(error.contains("helper"));
    assert!(rt.status().running, "不得伪造 stopped 运行态");
    assert_eq!(*rt.pid.lock().unwrap(), Some(4242));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

/// **变异门（记账侧）**：IPC 往返期间受管 pid 记账被新会话换人 → 收口腿**不得**清它。
///
/// 清了不是「多清一次」而是让新核**失联**：`status()` 的 helper 腿据 `pid` 探活、诊断据它报 pid、
/// `cleanup_stale_cores` 的「受管 pid 排除表」也据它 —— 排除表里少了新核，下一次起核的孤儿清扫
/// 就把它当孤儿杀掉（换个地方杀错进程）；`core_via_helper` 被清则让此后的停核走本地 child 腿
/// （child 恒 None）= 停核变 no-op = root 孤儿。
///
/// 变异：`clear_helper_core_bookkeeping` 退回无条件 `*g = None; store(false)` → 两条断言全红。
#[tokio::test]
async fn helper_stop_leg_does_not_wipe_bookkeeping_taken_over_mid_flight() {
    let (rt, _dir) = test_runtime();
    known_exact_helper_attempt(
        &rt,
        exact_helper_target(4242, "00112233445566778899aabbccddeeff"),
    );
    // 「IPC 在飞时新会话起了新核并提交 pid」——真机上这正是老 stop 腿醒来后会杀错人的那一刻。
    let pid_slot = Arc::clone(&rt.pid);
    let (ops, _calls, wants) = RecordingStop::with_hook(
        Ok(()),
        Some(Box::new(move || {
            *pid_slot.lock().unwrap() = Some(9001);
        })),
    );

    let error = rt
        .kill_core_via_helper(ops as Arc<dyn HelperStopOps>)
        .await
        .expect_err("attempt/pid 换人后不能给上层已停成功回执");
    assert!(error.contains("cleanup-unconfirmed"));

    assert_eq!(
        *wants.lock().unwrap(),
        vec![Some(4242)],
        "下发的身份仍是老腿意图停的那个（不是接管方的）"
    );
    assert_eq!(
        *rt.pid.lock().unwrap(),
        Some(9001),
        "新会话的受管 pid 记账必须原样保留 —— 清它 = 新核在 status/诊断/孤儿清扫排除表里集体失联"
    );
    assert!(
        rt.core_via_helper.load(Ordering::SeqCst),
        "helper 受管标记同样属新会话：清它会让此后的停核走本地 child 腿（child 恒 None）= 停核变 no-op"
    );
}

/// **变异门（逃逸面穷举）**：探活判死的收口腿**必须**调 daemon stop，且**恰调一次**。
///
/// - 删掉 `spawn_blocking(stop_managed_core)` → calls==0 → 转红（这就是孤儿的成因）。
/// - 改成循环/重复调用 → calls!=1 → 转红（重复停核会误伤后续世代的核）。
/// - 把返回消息改掉丢了 pid → 末条断言转红（用户拿不到可 `sudo kill` 的 pid）。
/// - 把 `stop_managed_core(Some(pid))` 退回不带身份的 `None` → 身份断言转红（那等于让 daemon
///   「停它此刻手里的随便哪个」，本方法整段可与新会话并发 ⇒ 杀错进程）。
#[tokio::test]
async fn rejected_helper_start_asks_daemon_to_stop_its_child() {
    let (rt, _dir) = test_runtime();
    let attempt = known_helper_attempt(&rt, 6439);
    let (ops, calls, wants) = RecordingStop::new(Ok(()));
    let msg = rt.reject_helper_start(ops, &attempt, 6439).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "探活判死时必须请 daemon 收口它自己的受管 child，恰一次——否则活着的 root 核就此失联成孤儿"
    );
    assert_eq!(
        *wants.lock().unwrap(),
        vec![Some(6439)],
        "收口请求必须**指名道姓**停那个 pid：不带身份 = 授权 daemon 杀它当前受管的任何核，\
             而这条腿完全可能与新会话的起核并发"
    );
    assert!(msg.contains("6439"), "失败消息须带 pid，用户才可能手动收拾");
}

/// **反向失效门**：stop 失败**不得**改判成功、也不得吞掉错误消息。
///
/// 核确实可能真死了（那时 daemon stop 返 notrunning/错误是正常的），故这条腿是 best-effort：
/// 打断（stop 返 Err 时改成 `return Ok`/返回空串/panic）→ 本测转红。
#[tokio::test]
async fn reject_leg_still_reports_failure_when_daemon_stop_errors() {
    let (rt, _dir) = test_runtime();
    let attempt = known_helper_attempt(&rt, 777);
    let (ops, calls, _wants) = RecordingStop::new(Err("daemon 说 notrunning".to_owned()));
    let msg = rt.reject_helper_start(ops, &attempt, 777).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "失败腿也必须真尝试过 stop");
    assert!(
        msg.contains("777") && msg.contains("进程不存在"),
        "stop 失败不改判：起核失败的结论与消息原样返回，不得被 stop 的结果污染"
    );
    assert!(msg.contains("cleanup-unconfirmed"));
    assert_eq!(*rt.pid.lock().unwrap(), Some(777));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_reject_stop_keeps_legacy_attempt_after_repeated_acks() {
    let (rt, _dir) = test_runtime();
    let attempt = known_helper_attempt(&rt, 777);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = Arc::clone(&release);
    let late_done = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&late_done);
    let entered = Mutex::new(Some(entered_tx));
    let (ops, calls, wants) = RecordingStop::with_hook(
        Ok(()),
        Some(Box::new(move || {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            let (lock, wake) = &*held;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            done.store(true, Ordering::Release);
        })),
    );
    let task = tokio::spawn({
        let rt = Arc::clone(&rt);
        let attempt = attempt.clone();
        async move { rt.reject_helper_start(ops, &attempt, 777).await }
    });
    entered_rx.await.unwrap();
    task.abort();
    let _ = task.await;
    assert!(rt.child.lock().unwrap().helper_stop_inflight_for_test());
    let (second, second_calls, _) = RecordingStop::new(Ok(()));
    let error = rt
        .kill_core_via_helper(second as Arc<dyn HelperStopOps>)
        .await
        .unwrap_err();
    assert!(error.contains("cleanup-unknown"));
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !late_done.load(Ordering::Acquire)
            || rt.child.lock().unwrap().helper_stop_inflight_for_test()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*wants.lock().unwrap(), vec![Some(777)]);
    assert_eq!(*rt.pid.lock().unwrap(), Some(777));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));

    let (confirm, _, _) = RecordingStop::new(Ok(()));
    assert!(rt
        .kill_core_via_helper(confirm)
        .await
        .unwrap_err()
        .contains("legacy Stop ACK"));
    assert_eq!(*rt.pid.lock().unwrap(), Some(777));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

/// **P1-a 不变式门（有牙版）**：**每一次** `start` 都必须走 stale 清扫腿，不是只走首次。
///
/// 直接驱动**两次真 start** 并数清扫实跑次数——不是读那个开关（读开关的写法对
/// `swap(true)` 一次性门闩免疫 = 没门）。
///
/// **变异门（逃逸面穷举）**：
/// - 调用点退回 `swap(true, ...)` 一次性门闩 → 第二次 start 不清扫 → runs==1 → 转红。
/// - 删掉整个清扫调用 → runs==0 → 转红。
/// - 把计数挪到 `resolve_core_binary` 成功之后 → 本测（核不可解析）恒 0 → 转红。
///
/// **本机零副作用**：`POLARIS_SINGBOX_PATH` 指向目录 → `resolve_core_binary` 必 Err → 清扫在
/// 计数后立刻早退，**不扫 /proc、不发任何信号**。
// 跨 await 持 `ENV_LOCK`：同 `start_emits_invalid_nodes_on_real_start_path`，见该测说明。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn stale_sweep_runs_on_every_start_not_only_the_first() {
    let (rt, dir) = test_runtime();
    assert!(
        !rt.stale_sweep_disabled.load(Ordering::SeqCst),
        "生产默认必须开启清扫（该开关仅单测置位）"
    );
    // 端口解析都到不了就失败的最小配置：本测只关心清扫腿被走到几次。
    let config = serde_json::json!({ "servers": [], "proxyModeType": "systemProxy" });

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("POLARIS_SINGBOX_PATH", &*dir);
    let first = rt.start(config.clone()).await;
    assert_eq!(
        rt.stale_sweep_runs.load(Ordering::SeqCst),
        1,
        "首次 start 必清扫一次"
    );
    let second = rt.start(config).await;
    std::env::remove_var("POLARIS_SINGBOX_PATH");
    drop(_g);

    assert!(
        first.is_err() && second.is_err(),
        "核二进制解析失败 → 两次均失败"
    );
    assert_eq!(
        rt.stale_sweep_runs.load(Ordering::SeqCst),
        2,
        "第二次 start 也必须清扫——一次性门闩会让本会话中途产生的孤儿永远落在射程外，\
             那正是真机把用户卡死的放大器"
    );
}

/// 🔴 **在飞测速临时核不得被孤儿清扫误杀**（用户报的「测速到一半启动 TUN，启动明显变慢」那条腿）。
///
/// 临时核 argv = `<同一个核二进制> run -c <临时配置> --disable-color` ⇒ 与主核同路径 + `run` token
/// ⇒ `is_our_core` 必然命中 ⇒ 在候选集里它与「上次遗留的孤儿」不可区分；而清扫跑在**每一次**
/// `start()` 上。不排除的后果是三层：整批测速被 SIGTERM 掐断、起核白等两段 `STALE_KILL_GRACE`
/// （+3.0s）、杀不动时升级到 `ROOT_ORPHAN_BLOCKED` 直接判死这次起核。
///
/// 四个向量一次钉死，**全部用构造的 [`CoreProcess`] 走纯判定：不起进程、不发任何信号**：
/// 1. 在飞临时核在候选集里 ⇒ 不得进 victims；
/// 2. 受管主核 pid 仍被排除（不许改坏原有行为）；
/// 3. **真孤儿仍被杀** —— 排除表变大最容易变成「什么都不杀了」，那是把一个缺陷换成另一个：
///    孤儿核占着 `cache.db`，下次起核会 `initialize cache-file: timeout`；
/// 4. 临时核退出、pid 出表之后，它若真成了孤儿，下一轮清扫照杀（排除是**此刻在飞**，不是永久豁免）。
///
/// **变异门**：`sweep_exclusions` 里 `exclude.extend(temp)` 那一行删掉 → 断言 1 转红，
/// 且红在「在飞 pid 被选成 victim」这件事本身上（断言 2/3/4 仍绿 ⇒ 红因唯一）。
///
/// # 登记走生产的 [`TempCorePidGuard`]，不手写 `insert`/`remove` 一对
///
/// `INFLIGHT_TEMP_CORES` 是**进程级**表。手写的那对里，`remove` 只能落在最后一条断言之后 ⇒
/// 任一断言先失败，pid 就永久留在表里污染同进程后续用例（今天全仓没有「表必须为空」的断言，
/// 所以只是隐患；下一个人加一条就会莫名假红，且归因方向指向别处）。RAII 守卫的 `Drop` 跑在
/// panic 展开上，无论断言在哪一条失败都清得干净；④ 那一格改用显式 `drop()` 表达「此刻出表」。
#[test]
fn stale_sweep_spares_inflight_temp_cores_but_still_kills_real_orphans() {
    use polaris_core_supervisor::{stale_pids, CoreProcess};
    // 与在飞 pid 表的其它用例串行：那张表是进程级共享状态，退出清理的用例会**整表排空**。
    let _registry = crate::runtime::speedtest::registry_guard();
    let (rt, _dir) = test_runtime();

    const OURS: &str = "/opt/polaris/resources/linux/sing-box";
    const MANAGED: u32 = 940_001;
    const INFLIGHT: u32 = 940_002;
    const ORPHAN: u32 = 940_003;
    const FOREIGN: u32 = 940_004;
    let binary = std::path::PathBuf::from(OURS);
    let proc = |pid: u32, args: &[&str]| CoreProcess {
        pid,
        cmdline: args.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    let candidates = vec![
        proc(MANAGED, &[OURS, "run", "-c", "singbox-runtime.json"]),
        // 临时核那一行逐字对齐 `SpawnRequest::argv()` + `speedtest.rs` 的 extra_args。
        proc(
            INFLIGHT,
            &[OURS, "run", "-c", "speedtest-core.json", "--disable-color"],
        ),
        proc(ORPHAN, &[OURS, "run", "-c", "singbox-runtime.json"]),
        // 用户系统装的 sing-box（异路径）：任何时候都不该动它（本清扫的安全底线）。
        proc(
            FOREIGN,
            &[
                "/usr/bin/sing-box",
                "run",
                "-c",
                "/etc/sing-box/config.json",
            ],
        ),
    ];

    *rt.pid.lock().unwrap() = Some(MANAGED);
    let inflight_guard = crate::runtime::speedtest::TempCorePidGuard::register(INFLIGHT)
        .expect("非 0 pid 必须登记成功（`register` 只对 pid==0 返 None）");
    let victims = stale_pids(&candidates, &binary, &rt.sweep_exclusions());
    assert!(
        !victims.contains(&INFLIGHT),
        "[①] 在飞测速临时核 pid={INFLIGHT} 被选成孤儿 victim —— 起核会掐断正在跑的测速并白等两段宽限"
    );
    assert!(
        !victims.contains(&MANAGED),
        "[②] 受管主核 pid={MANAGED} 必须仍被排除"
    );
    assert!(
        victims.contains(&ORPHAN),
        "[③] 真孤儿 pid={ORPHAN} 必须仍被杀 —— 排除表变大不得退化成「什么都不杀」（孤儿占着 cache.db）"
    );
    assert!(
        !victims.contains(&FOREIGN),
        "[安全底线] 非本 app 的 sing-box（异路径）绝不能被选中"
    );

    // ④ 会话收尾/被丢弃 → pid 出表（`TempCorePidGuard` 的 Drop 跑在 terminate 收割之后）。
    // 它若真的留成了孤儿，下一轮清扫必须能杀掉它。
    drop(inflight_guard);
    let after = stale_pids(&candidates, &binary, &rt.sweep_exclusions());
    assert!(
        after.contains(&INFLIGHT),
        "[④] 出表之后同一个 pid 必须重新落进 victims —— 排除的是「此刻在飞」，不是永久豁免"
    );
}

/// 🔴 **在飞 Tailscale 瞬态登录核不得被孤儿清扫误杀**（上一条的姊妹腿）。
///
/// 缺陷同源：登录核走同一个 `resolve_core_binary` + `SpawnRequest`，argv 逐字同形
/// （`<同一核二进制> run -c <cfg> --disable-color`）⇒ `is_our_core` 必然命中 ⇒ 在候选集里它与
/// 「上次会话遗留的孤儿」不可区分。用户序列是「点了 Tailscale 登录、正等着扫码，顺手去开 TUN」——
/// 登录核被 SIGTERM 掐死，登录 URL 作废，前端只看到「登录没反应」。
/// `tailscale_login_core.rs` 的模块文档**早就把这条缺陷登记在案**（当时的不修理由是「需要
/// mesh↔proxy 反向耦合」），本批把它接上：耦合方向本来就是现成的。
///
/// # 本条是端到端的，不是源码文本的
///
/// 走的是真 `ProxyRuntime` → 真 `Arc<MeshRuntime>` → 真 `LoginCoreRegistry`：
/// `rt.sweep_exclusions()` 内部经生产的 `self.mesh.inflight_login_core_pids()` →
/// `LoginCoreRegistry::inflight_login_pids()` 读注册表。测试只往注册表里 `insert` 一条假条目
/// （**不起进程、不发信号**），读侧一寸生产代码都没绕。
///
/// 三个向量：① 在飞登录核不进 victims；② 真孤儿仍被杀（排除表变大最容易退化成「什么都不杀」）；
/// ③ 出表之后同一 pid 重新落进 victims（排除的是「此刻在飞」，不是永久豁免）。
///
/// **变异门**：`sweep_exclusions` 里 `exclude.extend(login)` 删掉 → ① 转红（②③ 仍绿 ⇒ 红因唯一）；
/// `start_login` 里 `pid: child.pid()` 改成 `pid: None` → 本条**不红**，那一半由
/// `tailscale_login_core` 的 `inflight_login_pid_comes_from_the_child_handle` 钉。
#[test]
fn stale_sweep_spares_inflight_tailscale_login_cores() {
    use polaris_core_supervisor::{stale_pids, CoreProcess};
    // `rt.sweep_exclusions()` 顺带读**进程级**的在飞测速临时核表（`INFLIGHT_TEMP_CORES`），
    // 而那张表另有用例会整表排空 ⇒ 与上一条一样串行到同一把闸上。
    // 登录注册表本身是 `test_runtime()` 造的**每实例**状态，不需要串行，但读侧同一次调用两张表都碰。
    let _registry = crate::runtime::speedtest::registry_guard();
    let (rt, _dir) = test_runtime();

    const OURS: &str = "/opt/polaris/resources/linux/sing-box";
    const LOGIN: u32 = 950_001;
    const ORPHAN: u32 = 950_002;
    let binary = std::path::PathBuf::from(OURS);
    let proc = |pid: u32, args: &[&str]| CoreProcess {
        pid,
        cmdline: args.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    let candidates = vec![
        // 登录核那一行逐字对齐 `start_login` 的 `SpawnRequest` + `extra_args`。
        proc(
            LOGIN,
            &[
                OURS,
                "run",
                "-c",
                "tailscale-login-s1-7.json",
                "--disable-color",
            ],
        ),
        proc(ORPHAN, &[OURS, "run", "-c", "singbox-runtime.json"]),
    ];

    rt.mesh
        .login_registry_for_test()
        .register_inflight_for_test("s1", LOGIN);
    let victims = stale_pids(&candidates, &binary, &rt.sweep_exclusions());
    assert!(
        !victims.contains(&LOGIN),
        "[①] 在飞 Tailscale 登录核 pid={LOGIN} 被选成孤儿 victim —— 起核会掐断正在进行的登录"
    );
    assert!(
        victims.contains(&ORPHAN),
        "[②] 真孤儿 pid={ORPHAN} 必须仍被杀 —— 排除表变大不得退化成「什么都不杀」"
    );

    rt.mesh
        .login_registry_for_test()
        .deregister_inflight_for_test("s1");
    let after = stale_pids(&candidates, &binary, &rt.sweep_exclusions());
    assert!(
        after.contains(&LOGIN),
        "[③] 出表之后同一个 pid 必须重新落进 victims —— 排除的是「此刻在飞」，不是永久豁免"
    );
}

/// **接线门**：`sweep_exclusions` 算得对，不代表清扫真的去问了它，也不代表问的**顺序**对。
///
/// 判据取 `cleanup_stale_cores` 的方法体（`module_source` 本就剔 `tests/`，故判据区域不含本文件）。
/// 两条：
/// - 排除表必须真的交给 `stale_pids`（删掉 = 上一条行为门整条失去生产写侧，全绿也无意义）；
/// - 读表必须在 `scan_running_cores()` **之后**：反过来则「读表 → 临时核 spawn → 扫描」这个窗口里
///   起的临时核既在候选集、又不在表快照里，排除照样漏。
///
/// # 取材面必须是 [`module_code`]（剥注释面），不是 `module_source`
///
/// 本条两个判据都是**正面** `find()`，而 [`method_body`] → `strip_line_comments` **只把整行注释
/// 换成空行，行尾注释与块注释原样留在切片里**。喂 `module_source` 的版本实测可被一句行尾注释
/// 完整喂饱：把调用点整段退回本批之前的形态、行尾补一句
/// `// 排除表见 self.sweep_exclusions()`，`cargo build` rc=0、全仓 4875 项全绿，本条**不红** ——
/// 整批修复被撤销而无人察觉。换成 [`module_code`]（= `literal_face(module_source(..))`，注释按字节
/// 抹成空格、偏移与行号守恒 ⇒ `find` 比大小的顺序语义不变）后同一变异必红。
#[test]
fn stale_sweep_reads_exclusions_after_scanning_candidates() {
    const HEAD: &str =
        "    pub(super) async fn cleanup_stale_cores(&self) -> Result<(), StartError> {";
    let body = method_body(&module_code("runtime/proxy"), HEAD);
    let scan_at = body
        .find("scan_running_cores()")
        .expect("清扫必须先扫描候选集，锚点消失即守卫失去判据");
    let exclude_at = body
        .find("self.sweep_exclusions()")
        .expect("清扫必须把排除表交给 stale_pids —— 缺了它，在飞测速临时核就是候选集里的孤儿");
    assert!(
        exclude_at > scan_at,
        "排除表必须读在扫描之后：先读表再扫描会漏掉「读表后才 spawn」的那一格临时核"
    );
}

// ─── T1：pid 探活的 errno 语义（真机 TUN 卡死链的判定侧根因）────────────────────────

/// **变异门①（复现缺陷）**：`EPERM` 必须判**存活**。
///
/// 把 [`alive_from_probe`] 退回成 `r.is_ok()` → EPERM 落进 false → 本测转红。那正是真机
/// 「helper 报告已启动但进程不存在」的判定侧根因：helper 以 root 起核，app 以普通用户
/// `kill(pid,0)` 探活收 EPERM（进程活得好好的，只是没权限发信号）。
///
/// **变异门②（反向失效）**：`ESRCH` 必须判**不存活**。
/// 把 `Err(_) => true` 写成无条件 true（改过头，连 ESRCH 也算活）→ 本测转红。
/// 没有这一半，崩溃监测就永远发现不了核真的死了，孤儿也永远清不掉。
#[cfg(unix)] // 用 nix::errno / alive_from_probe（均 unix-only），windows 排除
#[test]
fn alive_probe_treats_eperm_as_alive_and_only_esrch_as_dead() {
    use nix::errno::Errno;
    assert!(alive_from_probe(Ok(())), "有权发信号且进程在 → 存活");
    assert!(
        alive_from_probe(Err(Errno::EPERM)),
        "[变异门①] EPERM = 进程存在但不属本用户（root 核）→ 必须判存活"
    );
    assert!(
        !alive_from_probe(Err(Errno::ESRCH)),
        "[变异门②] ESRCH = 内核确认无此进程 → 唯一的不存活判据"
    );
    // 其余 errno 不是死亡证据 → 保守判活（绝不据此宣告核已崩）。
    assert!(alive_from_probe(Err(Errno::EINVAL)), "非死亡证据 → 判存活");
}

/// 端到端接线：真跑 `kill(pid,0)` 三种现实情形，锁死 [`pid_alive`] 确实用了新判据。
///
/// **pid 1**（launchd/systemd）是现成的 **root 且非本用户**进程 —— 正是 helper 起的 root 核那一类。
/// 打断（`pid_alive` 绕开 `alive_from_probe` 直接 `.is_ok()`）→ 非 root 运行时本测转红。
#[cfg(unix)] // 用 nix::sys::signal::kill / nix::unistd::Pid（unix-only），windows 排除
#[test]
fn pid_alive_reports_root_owned_process_as_alive() {
    use nix::errno::Errno;
    // 自身必存活（任何实现都该过——防呆基线）。
    assert!(pid_alive(std::process::id()), "自身进程必判存活");
    // 不存在的 pid 必判死（取一个合法但不可能被占用的值）。
    assert!(!pid_alive(i32::MAX as u32), "不存在的 pid 必判不存活");

    // **广播语义门**：0 与越 i32 回绕的 pid 必须判不活，且不得走到 kill 的广播语义上。
    // 打断（去掉 `checked_pid` 直接 `pid as i32`）→ `kill(-1,0)`/`kill(0,0)` 恒 Ok → 本测转红。
    // 同一个 cast 也喂 `send_signal`，在那边等价于 `SIGKILL` 全场，故这是安全门不是洁癖。
    assert!(
        !pid_alive(0),
        "pid 0 = 当前进程组广播，绝不可判为某个进程存活"
    );
    assert!(
        !pid_alive(u32::MAX),
        "u32::MAX 回绕成 -1 = 全体广播，绝不可判存活"
    );

    // pid 1 的属主判定：非 root 用户探它必得 EPERM。若本次恰以 root 运行（CI 容器），
    // 这一腿没有 EPERM 可验 —— 照实跳过，不伪装成验过。
    let probe = nix::sys::signal::kill(nix::unistd::Pid::from_raw(1), None);
    if probe == Err(Errno::EPERM) {
        assert!(
            pid_alive(1),
            "[变异门①端到端] root 所有的 pid 1 探活收 EPERM，必须判存活"
        );
    } else {
        // 以 root 运行 → kill(1,0) 返 Ok，EPERM 腿在本环境无从构造。
        assert_eq!(probe, Ok(()), "非 EPERM 时只可能是 root 运行下的 Ok");
    }
}

/// Windows 探活必须走原生 API，禁止退回每轮启动 `tasklist` 的高延迟实现。
/// 本机 Linux 不编译 Windows 模块，故以源码契约锁住 FFI 与安全判据；Windows Package gate
/// 另会编译并运行模块内的 liveness 真值表测试。
#[test]
fn windows_pid_probe_uses_native_process_handle_not_tasklist() {
    let source = crate::test_support::crate_code("runtime/windows_process.rs");
    for required in ["OpenProcess(", "GetExitCodeProcess(", "GetProcessTimes("] {
        assert!(source.contains(required), "缺原生探活锚点：{required}");
    }
    assert!(source.contains("ERROR_INVALID_PARAMETER"));
    assert!(!source.contains("Command::new(\"tasklist\")"));
}

/// `/proc/<pid>/stat` 的 starttime 取材腿（helper 腿 pid 身份令牌的 linux 侧）。
///
/// 打断（改成对整行 `split_whitespace().nth(21)`，即不从最后一个 `)` 之后切）→ 第二个断言转红：
/// comm 含空格/右括号的进程会整体错位。这不是理论角落 —— 进程名由启动方控制，
/// 而本令牌一旦取到**错字段**，要么恒变（假崩溃 + 无谓重启）要么恒不变（门形同虚设），
/// 两种都比没有这道复核更坏。
#[test]
fn proc_stat_starttime_survives_comm_with_spaces_and_parens() {
    // 真实形状：pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt
    // cmajflt utime stime cutime cstime priority nice num_threads itrealvalue starttime …
    let fields: Vec<String> = (3..=22).map(|i| i.to_string()).collect();
    let tail = fields.join(" ");
    let plain = format!("6439 (sing-box) {tail} 上略");
    assert_eq!(
        parse_proc_stat_starttime(&plain).as_deref(),
        Some("22"),
        "starttime 是第 22 字段"
    );

    let nasty = format!("6439 (we ird) (name) {tail} 上略");
    assert_eq!(
        parse_proc_stat_starttime(&nasty).as_deref(),
        Some("22"),
        "comm 含空格与右括号时仍须取到第 22 字段（必须从最后一个 `)` 之后切）"
    );

    // 字段不够（读到半截 / 不是 stat）→ None，不返回一个错位的值。
    assert_eq!(
        parse_proc_stat_starttime("6439 (sing-box) S 1").as_deref(),
        None
    );
    // 连 `)` 都没有 → None。
    assert_eq!(parse_proc_stat_starttime("garbage").as_deref(), None);
}

/// **本次修复要防住的那件事的回放**：pid 还在（`pid_alive` 恒真）、但号码上换了进程。
///
/// 三条断言各锁一个方向：
/// - 令牌变 ⇒ `Mismatch`（崩溃监测据此判退出 → 自愈；此前这一格恒 `Alive`，自愈永不触发）；
/// - 取不到材料 ⇒ `Unobservable` 而**非** `Mismatch` —— 折成不匹配等于把一次读失败变成一次
///   假崩溃，下游是自动重启；
/// - 令牌未变 ⇒ `Match`。
///
/// 打断（把 `pid_identity_verdict` 的 `_ => Unobservable` 改成 `_ => Mismatch`）→ 第二组转红。
#[test]
fn pid_identity_flags_reuse_but_never_invents_a_crash() {
    assert_eq!(
        pid_identity_verdict(Some("998877"), Some("112233")),
        PidIdentity::Mismatch,
        "同一 pid 上令牌变了 = 换了进程"
    );
    assert_eq!(
        pid_identity_verdict(Some("998877"), Some("998877")),
        PidIdentity::Match
    );
    for (base, cur) in [(None, Some("x")), (Some("x"), None), (None, None)] {
        assert_eq!(
            pid_identity_verdict(base, cur),
            PidIdentity::Unobservable,
            "缺任一侧材料一律 Unobservable（没观测到 ≠ 观测到没问题）"
        );
    }
}

/// Windows TUN 真机回放：helper 核身份观察开始后，用户 stop 在另一 worker 上先 bump 世代并停核；
/// 观察最终拿到 Exited。分类必须读取**观察完成后的**世代，因此 Retire，绝不能触发崩溃自愈。
///
/// 变异：生产调用点改回「观察前 `let gen_now = ...`，观察后直接喂旧值」时，本 seam 不再被使用；
/// 相邻 `crash_monitor_classification_is_wired_after_observation` 会转红。这里则锁住 seam 自身的语义。
#[test]
fn active_stop_during_helper_observation_retires_instead_of_recovering() {
    let gate = LifecycleGate::default();
    let my_gen = gate.bump_generation();

    // 模拟同步 process_identity/pid_alive 观察期间，另一 runtime worker 执行 stop 入口。
    gate.bump_generation();
    let verdict = classify_observed_child_exit(&gate, my_gen, ChildObservation::Exited);

    assert_eq!(
        verdict,
        ExitClassification::Retire,
        "观察期间发生的主动 stop 必须按最新世代让旧监测退场，不能把 TUN 自动拉回"
    );
}

/// 接线顺序门：分类调用必须位于 `let observation = ...` 之后，且生产方法不得再在观察前缓存
/// `gen_now`。纯函数测试只能证明判据会算，守不住调用点重新喂陈旧快照的回归，故这里对方法体锁序。
#[test]
fn crash_monitor_classification_is_wired_after_observation() {
    const HEAD: &str = "    pub(super) fn spawn_crash_monitor(";
    let body = method_body(&module_code("runtime/proxy"), HEAD);
    let observation_at = body
        .find("let observation =")
        .expect("崩溃监测必须形成一次完整 observation");
    let classify_at = body
        .find("classify_observed_child_exit(&me.gate, my_gen, observation)")
        .expect("崩溃监测必须走观察后读世代的分类 seam");
    assert!(
        classify_at > observation_at,
        "分类必须发生在观察完成后，否则主动停核仍可能与旧世代拼成假崩溃"
    );
    assert!(
        !body[..observation_at].contains("let gen_now ="),
        "观察前不得缓存世代；Windows 同步身份查询期间 stop 可在另一 worker 上推进世代"
    );
}

/// `CrashRecoveryMachine` 的主动停/新起方法此前只有状态机单测，没有生产写侧。这里从公开入口回放：
/// stop 置 abort；下一次 start（即便配置随后校验失败）先复位，避免“一次停过、永不再自愈”。
#[tokio::test]
async fn public_stop_marks_recovery_aborted_and_next_start_resets_it() {
    let (rt, _dir) = test_runtime();
    rt.stop().await.expect("空闲态 stop 应幂等成功");
    assert!(
        rt.crash_lock().auto_restart_aborted(),
        "主动 stop 必须中止退避中的崩溃自愈"
    );

    rt.stale_sweep_disabled.store(true, Ordering::SeqCst);
    let _ = rt.start(bad_config()).await;
    assert!(
        !rt.crash_lock().auto_restart_aborted(),
        "下一次显式 start 必须复位旧 stop 的 abort 标记"
    );
}

/// **接线门**：纯逻辑对了不代表崩溃监测真的去问了它（**本地**令牌腿这一半）。
///
/// 本仓两天内被同一形状骗过两次（判据落在「这个词出现过吗」，而词的来源包含判据自身）⇒
/// 判据取的是 [`method_body`] 截出的 `spawn_crash_monitor` **方法体**（剥掉整行注释、
/// 到方法末尾封顶），既排除本测试模块自身，也排除方法内注释里的同名文本。
///
/// # 为什么还要再切一刀到本地腿
///
/// D3 的 helper 令牌腿加进来之后，`pid_identity_verdict(` / `process_identity(p)` /
/// `PidIdentity::Mismatch` 在这个方法体里各有**两份**（本地一份、helper 一份）。在整个方法体上
/// 断言，两份互相作证：把**本地**腿的消费分支删光（保留调用、`verdict` 仍参与后面的
/// `Unobservable` 条件，编得过），这条门照样绿 —— 实测过。
///
/// 这份污染不是遗留问题，是加 helper 腿的**同一轮**自己造出来的：加之前方法体里只有一份，
/// 那时这条门是有牙的。故切点与 [`crash_monitor_consults_the_helper_reported_created_token`]
/// **互为镜像** —— 那条从 helper 令牌取材处切到臂尾（自检「切片里不得有本地腿」），
/// 这条从本地复核处切到 helper 腿起手处（自检「切片里不得有 helper 腿」）。
///
/// 打断（把复核那段删掉、只留 `pid_alive`）→ 本地腿那几条全红。
#[test]
fn crash_monitor_actually_consults_the_pid_identity() {
    const HEAD: &str = "    pub(super) fn spawn_crash_monitor(";
    let src = module_code("runtime/proxy");
    // 切在「锚点之后的第一个顶层 `#[cfg(test)]`」：本文件里生产码与测试模块**交替**出现
    // （实测顶层 cfg(test) 有 5 处，最后一处还在本测试之后）⇒ 切第一处会把待验方法切掉、
    // 切最后一处会把本测试留在判据区域里。两种都实测过。
    let at = src
        .find(HEAD)
        .unwrap_or_else(|| panic!("锚点 `{HEAD}` 消失，源码型守卫已失去判据"));
    let cut = src[at..]
        .find("\n#[cfg(test)]\n")
        .map_or(src.len(), |i| at + i);
    let prod = &src[..cut];
    // 切点自检：判据区域里若还留着本测试自身，下面三条就会被自己写的字面量喂饱（生产调用点
    // 删光也照样绿）。本仓两天内被这个形状骗过两次，故显式锁住。
    assert!(
        !prod.contains("fn crash_monitor_actually_consults_the_pid_identity"),
        "判据区域包含本测试自身 —— 切点选错，断言会被自己的字面量污染"
    );
    let body = method_body(prod, HEAD);
    // 切到**本地令牌腿**：从本地复核起手，到 helper 腿起手处封顶。
    // 封顶锚点刻意取**不含 `} else `** 的形态 —— 删掉本地腿消费分支的变异会把 `} else if` 变成
    // `if`，锚点若带上 `} else ` 就会随变异一起消失，本条于是红在「锚点没了」而不是红在判据上。
    const LOCAL_LEG_START: &str = "let due = ticks.is_multiple_of(PID_IDENTITY_RECHECK_TICKS);";
    const LOCAL_LEG_END: &str = "identity.is_none() || verdict == PidIdentity::Unobservable";
    let leg_at = body
        .find(LOCAL_LEG_START)
        .unwrap_or_else(|| panic!("锚点 `{LOCAL_LEG_START}` 消失，本地令牌腿的判据已失去切点"));
    let leg_end = body[leg_at..]
        .find(LOCAL_LEG_END)
        .unwrap_or_else(|| panic!("封顶锚点 `{LOCAL_LEG_END}` 消失，切片会漫进 helper 腿"))
        + leg_at;
    let leg = &body[leg_at..leg_end];
    // 切点自检（与 helper 腿那条镜像）：切片里不得混进 helper 令牌腿，否则它替本地腿作证。
    assert!(
        !leg.contains("helper_identity_token(") && !leg.contains("managed_core_status()"),
        "切片里混进了 helper 令牌腿 —— 封顶选晚了，下面的断言会被它喂饱"
    );
    assert!(
        leg.contains("pid_identity_verdict("),
        "崩溃监测没有调用 pid_identity_verdict —— 身份复核没接线，pid 复用仍不可发现"
    );
    assert!(
        leg.contains("process_identity(p)"),
        "崩溃监测没有取当前令牌 —— 复核会拿基线跟自己比，恒 Match"
    );
    assert!(
        leg.contains("verdict == PidIdentity::Mismatch"),
        "本地令牌算了不用 —— 复核结果没进控制流，这条腿恒判存活"
    );
    assert!(
        leg.contains("ChildObservation::Exited"),
        "本地令牌不匹配时没有改判退出 —— pid 复用仍然不可发现"
    );
    assert!(
        leg.contains("崩溃监测：pid={p} 的进程身份令牌已变"),
        "改判退出时没有留下诊断 —— 真机上这条 warn 是「本地复用检出生效了」的唯一现场证据"
    );
    // helper 权威查询腿住在本地腿**之后**，故这两条仍按整个方法体断言。
    assert!(
        body.contains("helper.managed_core_status()"),
        "本地无法观察特权核时必须查询 helper 权威状态"
    );
    assert!(
        body.contains("ManagedCoreStatus::Stopped"),
        "helper 明确报告 stopped 时必须改判核退出"
    );
}

/// D2：内核自证在本地观测取不到时改问 helper —— 但**只在 pid 对得上**时才采信它的 `image=`。
///
/// 三条断言各锁一格：
/// - pid 相同 + 有 image → 采信（Windows 上这是自证从「未能进行」转为可判的唯一材料）；
/// - pid 不同 → `None`：helper 管着另一个会话的核，拿它的映像对账是在回答另一个问题；
/// - 无 image（旧 helper / FFI 读失败）→ `None`：读不到就说读不到，不冒充自证通过。
#[test]
fn attestation_only_trusts_the_helper_image_for_its_own_pid() {
    use crate::runtime::helper::ManagedCoreStatus;

    let running = |pid: u32, image: Option<&str>| ManagedCoreStatus::Running {
        pid,
        created: Some(133_600_000_000_000_000),
        image: image.map(ToOwned::to_owned),
    };
    let core = r"C:\ProgramData\Polaris\core\sing-box.exe";

    assert_eq!(
        image_from_managed_status(&running(4242, Some(core)), 4242),
        Some(std::path::PathBuf::from(core))
    );
    assert_eq!(
        image_from_managed_status(&running(9001, Some(core)), 4242),
        None,
        "helper 手里是另一个会话的核 —— 它的映像不能拿来给本代对账"
    );
    assert_eq!(image_from_managed_status(&running(4242, None), 4242), None);
    assert_eq!(
        image_from_managed_status(&ManagedCoreStatus::Stopped, 4242),
        None
    );
}

/// **接线门**：自证腿必须真的去问 helper，**并且真的把答案用掉**。
///
/// # 为什么不能只断言「函数名出现过」
///
/// 只断言 `helper_reported_core_image(...)` 出现在方法体里，挡不住这一类变异：
///
/// ```ignore
/// let running = running_exe_path(pid);
/// let _ = helper_reported_core_image(helper.as_deref(), pid);   // 调用还在，结果丢了
/// ```
///
/// 调用点一个字没少、门照绿，而 Windows 自证退回恒 `Unobservable` —— D2 整条失效。故断言必须
/// 咬住**数据流**：helper 的答案要经 `.or_else` 接进 `running`，`running` 再喂给
/// `attest_core_binary`。判据按**去空白**形态比对（`split_whitespace().collect()`），
/// 这样 rustfmt 怎么折行都不影响，改的是接线才会红。
///
/// 变异锁：删掉 `.or_else(...)` → 第三条红；把 `.or_else` 的结果丢弃（上面那段）→ 第三条红；
/// 把 `attest_core_binary` 的实参换成 `None` → 第四条红。
#[test]
fn attestation_consults_the_helper_reported_image() {
    const HEAD: &str =
        "    async fn attest_running_core_binary(&self, pid: u32, expected: &Path, my_gen: u64) {";
    let src = module_code("runtime/proxy");
    let at = src
        .find(HEAD)
        .unwrap_or_else(|| panic!("锚点 `{HEAD}` 消失，源码型守卫已失去判据"));
    let cut = src[at..]
        .find("\n#[cfg(test)]\n")
        .map_or(src.len(), |i| at + i);
    let prod = &src[..cut];
    // 切点自检：判据区域若含本测试自身，断言会被自己的字面量喂饱（生产调用点删光也绿）。
    assert!(
        !prod.contains("fn attestation_consults_the_helper_reported_image"),
        "判据区域包含本测试自身 —— 切点选错"
    );
    let body = method_body(prod, HEAD);
    assert!(
        body.contains("helper_reported_core_image(helper.as_deref(), pid)"),
        "自证没有第二观测腿 —— Windows 上 running_exe_path 恒 None，自证恒「未能进行」"
    );
    assert!(
        body.contains("running_exe_path(pid)"),
        "自证不能只靠 helper：本地读得到时必须优先用本地事实（少一次 IPC，且不受 helper 影响）"
    );
    // 消费侧：两条观测腿必须汇进同一个 `running`，`running` 必须真的喂给判定函数。
    let compact: String = body.split_whitespace().collect();
    assert!(
        compact.contains(
            "letrunning=running_exe_path(pid).or_else(||helper_reported_core_image(helper.as_deref(),pid));"
        ),
        "helper 那条腿的结果没有接进 `running` —— 调用还在但答案被丢掉，Windows 自证仍恒「未能进行」"
    );
    assert!(
        compact.contains("attest_core_binary(&expected,running.as_deref(),"),
        "`running` 没喂给 attest_core_binary —— 观测到了却不参与判定，等于没观测"
    );
}

/// **接线门**：崩溃监测必须把 helper 回传的 `created=` 拿去复核，**并且据结果改判**。
///
/// # 判据为什么要切到 helper 那一条 match 臂里
///
/// 只在整个方法体上断言「`helper_identity_token(created)` 出现过」，挡不住这一类变异：
///
/// ```ignore
/// let verdict = pid_identity_verdict(/* …取材一字不改… */);
/// let _ = verdict;                 // 判定还在算，分支删了
/// ChildObservation::Alive          // 恒活
/// ```
///
/// 取材腿一个字没少、门照绿，而 D3 的「helper 令牌复核」整条失效。更麻烦的是
/// `verdict == PidIdentity::Mismatch` 在**本地令牌腿**里也有一份 —— 在整个方法体上断言它，
/// 本地那条会替 helper 这条作证。故判据必须先切到 helper 臂内（切点自检见下），再断言消费侧。
///
/// 变异锁：把 helper 臂的 `if verdict == PidIdentity::Mismatch {…} else {…}` 换成恒
/// `ChildObservation::Alive` → 后三条转红；把那段整体换回只比 `pid == p` → 前两条也转红。
#[test]
fn crash_monitor_consults_the_helper_reported_created_token() {
    const HEAD: &str = "    pub(super) fn spawn_crash_monitor(";
    let src = module_code("runtime/proxy");
    let at = src
        .find(HEAD)
        .unwrap_or_else(|| panic!("锚点 `{HEAD}` 消失，源码型守卫已失去判据"));
    let cut = src[at..]
        .find("\n#[cfg(test)]\n")
        .map_or(src.len(), |i| at + i);
    let prod = &src[..cut];
    assert!(
        !prod.contains("fn crash_monitor_consults_the_helper_reported_created_token"),
        "判据区域包含本测试自身 —— 切点选错"
    );
    let body = method_body(prod, HEAD);
    assert!(
        body.contains("helper_identity_token(created)"),
        "helper 回传的 created 没被取成令牌 —— 身份复核拿不到材料"
    );
    assert!(
        body.contains("helper_identity"),
        "没有单独的 helper 基线 —— 与本地令牌混用会在同一进程上判出假不匹配"
    );
    // 切到 helper 那一条 match 臂：从令牌取材处起，到下一条臂（app 记账 pid ≠ helper 受管 pid）为止。
    const LEG_START: &str = "let token = helper_identity_token(created);";
    const LEG_END: &str = "Ok(Ok(ManagedCoreStatus::Running { pid, .. })) => {";
    let leg_at = body
        .find(LEG_START)
        .unwrap_or_else(|| panic!("锚点 `{LEG_START}` 消失，helper 腿的判据已失去切点"));
    let leg_end = body[leg_at..]
        .find(LEG_END)
        .unwrap_or_else(|| panic!("封顶锚点 `{LEG_END}` 消失，切片会漫到臂外"))
        + leg_at;
    let leg = &body[leg_at..leg_end];
    // 切点自检：本地令牌腿必须落在切片**之外**，否则它的 `PidIdentity::Mismatch` 会替 helper 腿作证。
    assert!(
        !leg.contains("process_identity(p)"),
        "切片里混进了本地令牌腿 —— 切点选早了，下面的断言会被它喂饱"
    );
    assert!(
        leg.contains("verdict == PidIdentity::Mismatch"),
        "helper 令牌算了不用 —— 复核结果没进控制流，这条腿恒判存活"
    );
    assert!(
        leg.contains("ChildObservation::Exited"),
        "令牌不匹配时没有改判退出 —— pid 复用仍然不可发现"
    );
    assert!(
        leg.contains("崩溃监测：helper 报告 pid={p} 的进程创建时间已变"),
        "改判退出时没有留下诊断 —— 真机上这条 warn 是「复用检出生效了」的唯一现场证据"
    );
}

/// **接线门**（B1）：崩溃监测的 helper 身份基线必须**起手就从 start 响应取初值**。
///
/// 不取的话，初值只能来自第一次 status —— 而那一格里有一个真窗口：核在首次 status 之前自然
/// 死亡，期间另一方发 start 让 helper 换掉受管 child、关掉旧句柄 ⇒ 旧 PID 从那一刻起可被复用；
/// 第一次 status 读到的已经是另一个进程的创建时间，登记成基线后每次复核都自己跟自己比，恒 `Match`。
///
/// 变异（把初值改回 `None`）→ 本条转红；`managed_start_identity` 的存/清由
/// `runtime::helper::tests::start_identity_baseline_is_remembered_and_never_goes_stale` 覆盖。
#[test]
fn crash_monitor_seeds_the_helper_baseline_from_the_start_response() {
    const HEAD: &str = "    pub(super) fn spawn_crash_monitor(";
    let src = module_code("runtime/proxy");
    let at = src
        .find(HEAD)
        .unwrap_or_else(|| panic!("锚点 `{HEAD}` 消失，源码型守卫已失去判据"));
    let cut = src[at..]
        .find("\n#[cfg(test)]\n")
        .map_or(src.len(), |i| at + i);
    let prod = &src[..cut];
    assert!(
        !prod.contains("fn crash_monitor_seeds_the_helper_baseline_from_the_start_response"),
        "判据区域包含本测试自身 —— 切点选错"
    );
    let compact: String = method_body(prod, HEAD).split_whitespace().collect();
    assert!(
        compact.contains("me.helper.managed_start_identity()"),
        "helper 基线没取 start 的初值 —— D2/D3(4) 的「start 拿初值」按字面不成立"
    );
    assert!(
        compact.contains("helper_identity_token(Some(created))"),
        "start 的 created 没被折成与 status 同一种令牌 —— 两边格式不同则首次复核必判假不匹配"
    );
}

/// D3：helper 回传的 created 变了 = 这个号码上换了进程；读不到则一律「不可观测」。
///
/// 判定复用 [`pid_identity_verdict`] 的三态口径，本条锁的是「created → 令牌」这一步不吃掉信息：
/// 两个不同的 u64 必须给出两个不同的令牌，否则复用永远判不出来。
#[test]
fn helper_created_token_detects_reuse_and_degrades_honestly() {
    let base = helper_identity_token(Some(133_600_000_000_000_000));
    let same = helper_identity_token(Some(133_600_000_000_000_000));
    let reused = helper_identity_token(Some(133_600_000_000_000_001));
    assert_eq!(base, same);
    assert_ne!(base, reused, "不同创建时间必须给出不同令牌");

    assert_eq!(
        pid_identity_verdict(base.as_deref(), reused.as_deref()),
        PidIdentity::Mismatch,
        "创建时间变了 ⇒ 判不匹配 ⇒ 崩溃监测据此判核已退出"
    );
    assert_eq!(
        pid_identity_verdict(base.as_deref(), same.as_deref()),
        PidIdentity::Match
    );
    // 旧 helper 不回传 → 令牌缺失 → Unobservable（**不是** Mismatch）：否则每 tick 都是一次假崩溃。
    assert_eq!(helper_identity_token(None), None);
    assert_eq!(
        pid_identity_verdict(base.as_deref(), helper_identity_token(None).as_deref()),
        PidIdentity::Unobservable
    );
}

#[test]
fn local_native_birth_and_terminal_facts_follow_actual_production_edges() {
    let startup = module_code("runtime/proxy/startup");
    let start = method_body(&startup, "    pub(super) async fn start_inner(");
    assert!(
        start.find("self.admitted_native_producer(my_gen)").unwrap()
            < start.find("self.spawn_direct_native(").unwrap()
    );
    assert_eq!(start.matches("TokioSpawner::new()").count(), 1);
    assert!(start.contains("StdioPolicy::drain("));
    let admission = method_body(&startup, "    pub(super) fn spawn_direct_native(");
    assert!(admission.contains("producer.belongs_to(&self.stop_domain, birth_generation)"));
    assert!(admission.contains("self.gate.generation() != birth_generation"));
    assert!(
        !admission.contains("normal_start"),
        "no reverse catalog lock in protected admission"
    );
    let source = module_code("runtime/proxy/process_supervision/direct_custody");
    let spawn = method_body(
        &source,
        "    pub(in crate::runtime::proxy) fn spawn_native(",
    );
    assert!(spawn.find("producer.enter_factory(").unwrap() < spawn.find("factory_call()").unwrap());
    assert!(
        spawn.find("factory_call()").unwrap()
            < spawn.find("SealedNativeMembers::attached(").unwrap()
    );
    assert!(
        spawn.find("SealedNativeMembers::attached(").unwrap()
            < spawn
                .find("self.slot.state = SlotState::Running(run)")
                .unwrap()
    );
    let wait = method_body(
        &source,
        "    pub(in crate::runtime::proxy) fn poll_native_wait(",
    );
    assert_eq!(wait.matches("members.observe_exit(").count(), 1);
    assert!(
        wait.find("Poll::Ready(Ok(status))").unwrap() < wait.find("members.observe_exit(").unwrap()
    );
    let retire = method_body(
        &source,
        "    pub(in crate::runtime::proxy) fn retire_native_stop(",
    );
    let validate = retire.find("members.consume_native_exit(").unwrap();
    let release = retire.find("release_main(token)?").unwrap();
    let terminal = retire.find("validation.retire()").unwrap();
    let empty = retire.find("self.state = SlotState::Empty").unwrap();
    assert!(validate < release && release < terminal && terminal < empty);
    assert!(!retire[release + "release_main(token)?".len()..].contains(".lock("));
    assert!(!retire[release + "release_main(token)?".len()..].contains("?"));
    let stop = method_body(
        &module_code("runtime/proxy/process_supervision"),
        "    pub(super) async fn kill_direct_core_with_io(",
    );
    assert!(
        stop.find("*pid = None").unwrap() < stop.find("self.prune_normal_producers()").unwrap()
    );
}

#[test]
fn writer_retirement_requires_every_original_run_and_node() {
    let node = |tag: &str| WriterNodeScope {
        tag: tag.into(),
        directory: format!("/private/state/{tag}"),
        file: format!("/private/state/{tag}/tailscaled.state"),
        terminal: false,
    };
    let original = vec![
        WriterRunScope {
            nonce: "a".repeat(64),
            digest: "1".repeat(64),
            terminal: false,
            complete: false,
            nodes: vec![node("target"), node("other")],
        },
        WriterRunScope {
            nonce: "b".repeat(64),
            digest: "2".repeat(64),
            terminal: true,
            complete: true,
            nodes: vec![node("retained")],
        },
    ];
    let expected: Vec<_> = original[0]
        .nodes
        .iter()
        .map(|node| (node.tag.clone(), node.directory.clone(), node.file.clone()))
        .collect();
    let mut retired = original.clone();
    for run in &mut retired {
        run.terminal = true;
        run.complete = true;
        for node in &mut run.nodes {
            node.terminal = true;
        }
    }
    assert!(verify_writer_census(&original, &retired, &"1".repeat(64), &expected).is_ok());
    for mutation in 0..9 {
        let mut bad = retired.clone();
        match mutation {
            0 => {
                bad.pop();
            }
            1 => {
                bad[0].nodes.pop();
            }
            2 => {
                bad[1].complete = false;
            }
            3 => {
                bad[1].terminal = false;
            }
            4 => {
                bad[0].nodes[1].terminal = false;
            }
            5 => {
                bad[0].nonce = "c".repeat(64);
            }
            6 => {
                bad[0].digest = "3".repeat(64);
            }
            7 => {
                bad[0].nodes[0].directory = "/private/other".into();
            }
            _ => {
                bad.push(bad[0].clone());
            }
        }
        assert!(
            verify_writer_census(&original, &bad, &"1".repeat(64), &expected).is_err(),
            "mutation {mutation}"
        );
    }
    assert!(verify_writer_census(&[], &[], &"1".repeat(64), &expected).is_err());
    assert!(verify_writer_census(&original, &retired, &"9".repeat(64), &expected).is_err());
    assert!(verify_writer_census(&original, &retired, &"1".repeat(64), &expected[..1]).is_err());
}

#[test]
fn pre_ready_retirement_uses_original_full_claim_without_a_ready_baseline() {
    let node = |tag: &str| WriterNodeScope {
        tag: tag.into(),
        directory: format!("/private/state/{tag}"),
        file: format!("/private/state/{tag}/tailscaled.state"),
        terminal: true,
    };
    let retired = vec![WriterRunScope {
        nonce: "a".repeat(64),
        digest: "1".repeat(64),
        terminal: true,
        complete: true,
        nodes: vec![node("target"), node("other")],
    }];
    let expected: Vec<_> = retired[0]
        .nodes
        .iter()
        .map(|node| (node.tag.clone(), node.directory.clone(), node.file.clone()))
        .collect();
    assert!(verify_pre_ready_writer_census(&retired, &"1".repeat(64), &expected).is_ok());
    for mutation in 0..9 {
        let mut bad = retired.clone();
        match mutation {
            0 => bad.clear(),
            1 => {
                bad[0].nodes.pop();
            }
            2 => bad[0].complete = false,
            3 => bad[0].terminal = false,
            4 => bad[0].nodes[1].terminal = false,
            5 => {
                let mut historic = bad[0].clone();
                historic.nonce = "b".repeat(64);
                historic.digest = "2".repeat(64);
                bad.push(historic);
            }
            6 => bad.push(bad[0].clone()),
            7 => bad[0].nodes[0].file = "/private/other/tailscaled.state".into(),
            _ => bad[0].nodes[0].directory = "relative/path".into(),
        }
        assert!(
            verify_pre_ready_writer_census(&bad, &"1".repeat(64), &expected).is_err(),
            "mutation {mutation}"
        );
    }
    assert!(verify_pre_ready_writer_census(&retired, &"1".repeat(64), &[]).is_err());
    assert!(verify_pre_ready_writer_census(&retired, &"1".repeat(64), &expected[..1]).is_err());
}

#[test]
fn ios_cold_and_pre_ready_consumers_keep_original_custody_and_strict_stop() {
    let proxy = module_code("runtime/proxy");
    let custody = method_body(&proxy, "    fn allows_successor(&self) -> bool {");
    assert!(custody.contains("self.main_token.is_none()"));
    assert!(custody.contains("self.stopped.is_some()"));
    assert!(custody.contains("IosMainOrigin::LocalStart(start)"));
    assert!(custody.contains("receipt.belongs_to(start)"));
    assert!(
        !custody.contains("IosMainOrigin::Observed"),
        "observed token=None is not closure"
    );
    let source = module_code("runtime/proxy/prerequisite");
    let attach = method_body(
        &source,
        "    pub(crate) async fn prepare_tailscale_action_origin(",
    );
    assert!(attach.contains("self.core_generation() != generation"));
    let attach_compact: String = attach.split_whitespace().collect();
    assert!(attach_compact.contains("observe_current_session().await"));
    assert!(attach_compact.contains("attempt.while_active(||"));
    assert!(attach.contains("with_current_generation(generation"));
    assert!(attach_compact.contains("ifslot.is_some()"));
    assert!(attach_compact.contains("main_token:None"));
    assert!(!attach.contains("mint_tailscale_main_birth"));
    assert!(
        attach.find("drop(gate)").unwrap()
            < attach
                .rfind("stop_for_tailscale_action(generation, attempt)")
                .unwrap()
    );
    let stop = method_body(
        &module_code("runtime/proxy/process_supervision"),
        "    pub(super) async fn kill_core_and_release_main(",
    );
    assert!(stop.contains("stop_observed(observed).await"));
    let stop_compact: String = stop.split_whitespace().collect();
    assert!(stop_compact.contains("source_extension_generation()!=observed.extension_generation()"));
    assert!(stop_compact
        .contains("verify_pre_ready_writer_census(&retired,&owner.config_digest,&expected,)"));
    assert!(stop.contains("current.same_origin(&owner)"));
    assert!(
        !stop.contains("ReadySessionReceipt::"),
        "no fabricated Ready for failed Start"
    );
    let startup = method_body(
        &module_code("runtime/proxy/startup"),
        "    pub(super) async fn start_inner(",
    );
    assert!(startup.contains("!owner.allows_successor()"));
    assert!(
        startup.find("!owner.allows_successor()").unwrap()
            < startup
                .find("tauri_plugin_polaris_ios::prepare_start(")
                .unwrap()
    );
}

#[test]
fn post_logout_sign_in_requires_original_normal_ready_and_fresh_status() {
    let login = module_code("runtime/tailscale_login_core");
    let ordinary = method_body(&login, "    async fn launch_normal_main_attempt(");
    let compact: String = ordinary.split_whitespace().collect();
    assert!(compact.contains("prepare_tailscale_action_origin(normal.action_generation,attempt)"));
    assert!(compact.contains("binding.for_attempt(action_generation,Arc::clone(attempt))"));
    assert!(compact.contains("normal.proxy.await_normal_main(binding)"));
    assert!(compact.contains("self.confirm_main_request("));
    assert!(
        !compact.contains("cached_session_exists"),
        "readonly absence must not bypass normal Ready"
    );
    let logout = method_body(&login, "    pub(crate) async fn logout_with_normal_main(");
    let compact: String = logout.split_whitespace().collect();
    assert!(compact.contains("prepare_tailscale_action_origin(generation,&attempt)"));
    assert!(compact.contains("proxy.await_normal_main(binding)"));
    assert!(compact.contains("retire_tailscale_account(&ready,server_id,&attempt,&saved,None)"));
    assert_eq!(
        compact.matches("proxy.await_normal_main(binding)").count(),
        1,
        "standalone logout has only the old saved prerequisite, no successor Start"
    );
    assert!(!compact.contains("confirm_main_request"));
    assert!(
        compact.contains("error.code().to_owned()"),
        "normal permission failures keep their structured code"
    );
}

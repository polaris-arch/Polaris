//! Production custody/Stop/exit seams; no daemon, GUI or host route is changed.
use super::*;

#[tokio::test]
async fn confirmed_terminal_replays_stop_without_ipc_or_releasing_a_new_main_claim() {
    use super::super::super::startup::HelperStartCompletion;
    use crate::runtime::helper::HelperStartResult;
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let state = rt.mesh.tailscale_state_dir("terminal-main").unwrap();
    let generated = serde_json::json!({"endpoints":[{"type":"tailscale","state_directory":state}]});
    let original = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, original.clone())
        .await
        .unwrap();
    let attempt = rt
        .register_helper_start_backend_with_main(Some(original.clone()))
        .unwrap();
    let target = exact_helper_target(7123, "00112233445566778899aabbccddeeff");
    HelperStartCompletion::for_test(&rt, attempt.clone())
        .publish(&Ok(HelperStartResult::BirthStarted(target)))
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper_with_main(ops.clone(), Some(&gate))
        .await
        .unwrap();
    assert!(!rt.mesh.main_owns_tailscale("terminal-main", true));
    let successor = rt.mesh.mint_tailscale_main_birth();
    let mut replacement = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, successor.clone())
        .await
        .unwrap();
    replacement.arm_external_start();
    drop(replacement);
    let repeated = rt
        .kill_core_via_helper_with_main(ops.clone(), Some(&gate))
        .await
        .unwrap();
    assert!(repeated.same(&attempt));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rt.mesh.main_owns_tailscale("terminal-main", true));
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&successor, &gate)
        .unwrap());
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_terminal_is_bound_to_original_runtime_pid_custody_slot_and_birth_generation() {
    let (rt, _dir) = test_runtime();
    let (foreign, _other_dir) = test_runtime();
    let generation = rt
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    let target = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let attempt = known_exact_helper_attempt(&rt, target);
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper(ops.clone()).await.unwrap();
    {
        let slot = rt.child.lock().unwrap();
        assert!(slot
            .locally_closed_helper_attempt(&rt.child, &rt.pid, None, generation)
            .is_some_and(|found| found.same(&attempt)));
        assert!(slot
            .locally_closed_helper_attempt(&rt.child, &rt.pid, None, generation - 1)
            .is_none());
        assert!(slot
            .locally_closed_helper_attempt(&foreign.child, &rt.pid, None, generation)
            .is_none());
        assert!(slot
            .locally_closed_helper_attempt(&rt.child, &foreign.pid, None, generation)
            .is_none());
        assert!(slot
            .locally_closed_helper_attempt(&rt.child, &rt.pid, Some(target.pid.get()), generation)
            .is_none());
    }
    rt.child.lock().unwrap().replace_instance_for_test();
    assert!(rt
        .kill_core_via_helper(ops)
        .await
        .unwrap_err()
        .contains("cleanup-unknown"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_superseded_stop_nonce_cannot_publish_a_local_terminal() {
    let (rt, _dir) = test_runtime();
    let birth = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let attempt = known_exact_helper_attempt(&rt, birth);
    let (_, target, old_nonce) = rt.child.lock().unwrap().begin_helper_stop().unwrap();
    rt.child
        .lock()
        .unwrap()
        .finish_helper_stop_ipc(&attempt, &old_nonce);
    let (_, _, current_nonce) = rt.child.lock().unwrap().begin_helper_stop().unwrap();
    let old = HelperStopPermit::new(Arc::clone(&rt.child), attempt.clone(), target, old_nonce);
    assert!(!rt
        .clear_helper_core_bookkeeping_with_main(&old, |_| Ok(()))
        .unwrap());
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    drop(old);
    let current = HelperStopPermit::new(Arc::clone(&rt.child), attempt, target, current_nonce);
    assert!(rt
        .clear_helper_core_bookkeeping_with_main(&current, |_| Ok(()))
        .unwrap());
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
}

#[tokio::test]
async fn a_new_helper_attempt_replaces_only_the_old_terminal_and_must_stop_its_own_birth() {
    let (rt, _dir) = test_runtime();
    let old = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let new = exact_helper_target(4242, "11112222333344445555666677778888");
    let old_attempt = known_exact_helper_attempt(&rt, old);
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper(ops.clone()).await.unwrap();
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    let new_attempt = known_exact_helper_attempt(&rt, new);
    assert!(!old_attempt.same(&new_attempt));
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert_eq!(*rt.pid.lock().unwrap(), Some(new.pid.get()));
    rt.kill_core_via_helper(ops.clone()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *ops.targets.lock().unwrap(),
        [
            crate::runtime::helper::HelperStopTarget::Birth(old),
            crate::runtime::helper::HelperStopTarget::Birth(new),
        ]
    );
    let repeated = rt.kill_core_via_helper(ops).await.unwrap();
    assert!(repeated.same(&new_attempt));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread")]
async fn publishing_a_birth_is_not_original_start_worker_completion() {
    use super::super::super::startup::HelperStartCompletion;
    let (rt, _dir) = test_runtime();
    let target = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    let attempt = rt.register_helper_start_backend().unwrap();
    let mut completion = HelperStartCompletion::for_test(&rt, attempt.clone());
    let (published, ready) = tokio::sync::oneshot::channel();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = Arc::clone(&release);
    let worker = tokio::task::spawn_blocking(move || {
        completion
            .publish(&Ok(
                crate::runtime::helper::HelperStartResult::BirthStarted(target),
            ))
            .unwrap();
        published.send(()).unwrap();
        let (lock, wake) = &*held;
        let mut done = lock.lock().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !*done {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "test did not release original Start worker"
            );
            done = wake.wait_timeout(done, remaining).unwrap().0;
        }
    });
    assert!(rt
        .child
        .lock()
        .unwrap()
        .bind_helper_start_worker(&attempt, worker.abort_handle()));
    ready.await.unwrap();
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    assert!(rt.kill_core_via_helper(ops.clone()).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    // Cancel the observer, not the blocking producer. Custody retains the original handle.
    worker.abort();
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    worker.await.unwrap();
    rt.kill_core_via_helper(ops).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
}

/// Inputs at the actual post-Ready rollback seam. Readiness/route observations
/// are fixtures; Stop transport, registry retirement and strict Main drain are real.
#[cfg(target_os = "linux")]
pub(crate) async fn helper_terminal_exit_fixture(
    postflight_rollback: bool,
) -> (Arc<ProxyRuntime>, TestDir, Arc<Mutex<Vec<String>>>) {
    use super::super::super::startup::HelperStartCompletion;
    let pid = 7124;
    let hex = "ffeeddccbbaa99887766554433221100";
    let (rt, dir, frames) = birth_daemon_runtime([format!("OK birth-stopped {pid} {hex}\n")]);
    {
        let gate = rt.mesh.tailscale_state_gate().await;
        let state = rt.mesh.tailscale_state_dir("terminal-exit").unwrap();
        let main = rt.mesh.mint_tailscale_main_birth();
        let mut reservation = rt
            .mesh
            .reserve_tailscale_main_states(
                &serde_json::json!({"endpoints":[{"type":"tailscale","state_directory":state}]}),
                &gate,
                main.clone(),
            )
            .await
            .unwrap();
        let attempt = rt
            .register_helper_start_backend_with_main(Some(main))
            .unwrap();
        HelperStartCompletion::for_test(&rt, attempt)
            .publish(&Ok(
                crate::runtime::helper::HelperStartResult::BirthStarted(exact_helper_target(
                    pid, hex,
                )),
            ))
            .unwrap();
        reservation.arm_external_start();
        drop(reservation);
        if postflight_rollback {
            let error = rt
                .reject_tun_route_start(
                    rt.gate.generation(),
                    &gate,
                    "fixture: route postflight rejected after Ready".into(),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("postflight rejected"));
        }
    }
    if !postflight_rollback {
        rt.stop().await.unwrap();
    }
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert!(!rt.mesh.main_owns_tailscale("terminal-exit", true));
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert_eq!(frames.lock().unwrap().len(), 1);
    (rt, dir, frames)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn exact_closed_helper_to_direct_start_requires_restart_without_a_new_birth() {
    let (rt, _dir, frames) = helper_terminal_exit_fixture(true).await;
    let generation = rt.gate.generation();
    for _ in 0..2 {
        let error = rt.start(local_only_config(free_port())).await.unwrap_err();
        assert_eq!(error.code, Some(code::HELPER_RUNTIME_RESTART_REQUIRED));
        assert!(error.admission_denied);
        assert_eq!(rt.gate.generation(), generation);
        assert!(!rt.status().running);
        assert!(rt.core_via_helper.load(Ordering::SeqCst));
        assert!(rt.child.lock().unwrap().helper_touched_for_test());
        assert!(rt
            .child
            .lock()
            .unwrap()
            .has_local_helper_terminal_for_test());
        assert_eq!(frames.lock().unwrap().len(), 1);
    }
    // A repeated local denial does not consume the terminal or weaken normal exit.
    rt.stop().await.unwrap();
    rt.shutdown_for_exit().await.unwrap();
    assert_eq!(frames.lock().unwrap().len(), 1);
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tokio::test]
async fn active_or_unconfirmed_helper_does_not_claim_the_restart_required_state() {
    for confirmed_birth in [false, true] {
        let (rt, _dir) = test_runtime();
        if confirmed_birth {
            known_exact_helper_attempt(
                &rt,
                exact_helper_target(4242, "00112233445566778899aabbccddeeff"),
            );
        } else {
            rt.register_helper_start_backend().unwrap();
        }
        let generation = rt.gate.generation();
        let error = rt.start(local_only_config(free_port())).await.unwrap_err();
        assert_eq!(error.code, Some(code::STARTUP_FAILED));
        assert!(error.admission_denied);
        assert_eq!(rt.gate.generation(), generation);
        assert!(rt.child.lock().unwrap().has_helper_start());
        assert!(!rt
            .child
            .lock()
            .unwrap()
            .has_local_helper_terminal_for_test());
        assert!(rt.core_via_helper.load(Ordering::SeqCst));
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn postflight_rollback_then_second_stop_and_strict_main_exit_are_idempotent() {
    let (rt, _dir, frames) = helper_terminal_exit_fixture(true).await;
    rt.stop().await.unwrap();
    rt.shutdown_for_exit().await.unwrap();
    assert_eq!(frames.lock().unwrap().len(), 1);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert!(rt.child.lock().unwrap().helper_touched_for_test());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
    assert!(rt.start(local_only_config(free_port())).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_stop_observer_does_not_publish_terminal_from_a_late_ack() {
    let (rt, _dir) = test_runtime();
    let birth = exact_helper_target(4242, "00112233445566778899aabbccddeeff");
    known_exact_helper_attempt(&rt, birth);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let entered = Mutex::new(Some(entered));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let held = Arc::clone(&release);
    let (blocked, _, _) = RecordingStop::with_hook(
        Ok(()),
        Some(Box::new(move || {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            let (lock, wake) = &*held;
            let mut released = lock.lock().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !*released {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "test did not release original Stop worker"
                );
                released = wake.wait_timeout(released, remaining).unwrap().0;
            }
        })),
    );
    let observer = tokio::spawn({
        let rt = Arc::clone(&rt);
        async move { rt.kill_core_via_helper(blocked).await }
    });
    ready.await.unwrap();
    observer.abort();
    let _ = observer.await;
    let (retry, calls, _) = RecordingStop::new(Ok(()));
    assert!(rt.kill_core_via_helper(retry.clone()).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while rt.child.lock().unwrap().helper_stop_inflight_for_test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert_eq!(*rt.pid.lock().unwrap(), Some(birth.pid.get()));
    rt.kill_core_via_helper(retry.clone()).await.unwrap();
    rt.kill_core_via_helper(retry).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_successor_cannot_reuse_a_predecessor_terminal_or_authorize_exit() {
    use super::super::super::startup::HelperStartCompletion;
    let (rt, _dir) = test_runtime();
    known_exact_helper_attempt(
        &rt,
        exact_helper_target(4242, "00112233445566778899aabbccddeeff"),
    );
    let (ops, calls, _) = RecordingStop::new(Ok(()));
    rt.kill_core_via_helper(ops.clone()).await.unwrap();
    let successor = rt.register_helper_start_backend().unwrap();
    HelperStartCompletion::for_test(&rt, successor)
        .publish(&Err("fixture: Start result unknown".into()))
        .unwrap();
    assert!(!rt
        .child
        .lock()
        .unwrap()
        .has_local_helper_terminal_for_test());
    assert!(rt
        .kill_core_via_helper(ops)
        .await
        .unwrap_err()
        .contains("cleanup-unknown"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(rt
        .shutdown_for_exit()
        .await
        .unwrap_err()
        .contains("cleanup-unknown"));
    assert!(rt.child.lock().unwrap().has_helper_start());
    assert!(rt.core_via_helper.load(Ordering::SeqCst));
}

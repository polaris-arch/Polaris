use super::*;
use std::future::{poll_fn, Future};
use std::io;
use std::process::ExitStatus;
use std::task::{Context, Poll};

#[derive(Clone, Copy)]
enum WaitFault {
    Native,
    Error,
    PendingThenError,
    Pending,
}

struct NativeWaitIo {
    fault: WaitFault,
    waits: AtomicUsize,
    terms: AtomicUsize,
    kills: AtomicUsize,
    kill_error: bool,
}

impl NativeWaitIo {
    fn new(fault: WaitFault) -> Self {
        Self {
            fault,
            waits: AtomicUsize::new(0),
            terms: AtomicUsize::new(0),
            kills: AtomicUsize::new(0),
            kill_error: false,
        }
    }
}

impl DirectStopIo for NativeWaitIo {
    fn try_wait(&self, child: &mut tokio::process::Child) -> io::Result<Option<ExitStatus>> {
        child.try_wait()
    }

    fn poll_wait(
        &self,
        child: &mut tokio::process::Child,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<ExitStatus>> {
        let previous = self.waits.fetch_add(1, Ordering::SeqCst);
        match self.fault {
            WaitFault::Pending => Poll::Pending,
            WaitFault::PendingThenError if previous == 0 => Poll::Pending,
            WaitFault::Error | WaitFault::PendingThenError => {
                Poll::Ready(Err(io::Error::other("injected native wait failure")))
            }
            WaitFault::Native => {
                let wait = child.wait();
                tokio::pin!(wait);
                wait.poll(cx)
            }
        }
    }

    fn terminate(&self, child: &mut tokio::process::Child) -> io::Result<()> {
        self.terms.fetch_add(1, Ordering::SeqCst);
        if matches!(self.fault, WaitFault::Native) {
            // A local sleep stand-in has no graceful shutdown work to exercise.
            child.start_kill()?;
        }
        Ok(())
    }

    fn start_kill(&self, child: &mut tokio::process::Child) -> io::Result<()> {
        self.kills.fetch_add(1, Ordering::SeqCst);
        if self.kill_error {
            return Err(io::Error::other("injected owned Child kill failure"));
        }
        if !matches!(self.fault, WaitFault::Pending) {
            child.start_kill()?;
        }
        Ok(())
    }
}

async fn install_main_run(
    rt: &Arc<ProxyRuntime>,
    child: tokio::process::Child,
    recorded_pid: u32,
    server_id: &str,
) -> (
    RunIdentity,
    crate::runtime::tailscale_login_core::MainBirthToken,
) {
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
    let identity = RunIdentity::new();
    rt.child
        .lock()
        .unwrap()
        .install_running_for_test(DirectCoreRun::with_main_token(
            child,
            identity.clone(),
            token.clone(),
        ));
    *rt.pid.lock().unwrap() = Some(recorded_pid);
    reservation.arm_external_start();
    drop(reservation);
    (identity, token)
}

async fn live_main_run(
    server_id: &str,
) -> (
    Arc<ProxyRuntime>,
    TestDir,
    RunIdentity,
    crate::runtime::tailscale_login_core::MainBirthToken,
    u32,
) {
    let (rt, dir) = test_runtime();
    let child = spawn_custody_stand_in();
    let pid = child.id().unwrap();
    let (identity, token) = install_main_run(&rt, child, pid, server_id).await;
    (rt, dir, identity, token, pid)
}

fn assert_retained_main(rt: &ProxyRuntime, server_id: &str, pid: u32) {
    let mut slot = rt.child.lock().unwrap();
    assert!(slot.is_stopping_for_test());
    assert!(slot.empty_for_install().is_none());
    assert_eq!(*rt.pid.lock().unwrap(), Some(pid));
    assert!(rt.mesh.main_owns_tailscale(server_id, true));
}

#[cfg(unix)]
#[tokio::test]
async fn real_echild_from_running_observation_blocks_all_future_native_wait_and_signal() {
    let (rt, _dir) = test_runtime();
    let child = tokio::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let (identity, _token) = install_main_run(&rt, child, pid, "main-echild").await;
    tokio::task::spawn_blocking(move || {
        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();
    // The first native failure occurs while Running, before a Stop booking exists.
    assert!(matches!(
        rt.child.lock().unwrap().observe_running(&identity),
        polaris_core_supervisor::ChildObservation::Alive
    ));
    assert!(!rt.child.lock().unwrap().running_exit_proven(&identity));
    rt.runtime_binding_state
        .lock()
        .unwrap()
        .managed_tun_interface = ExitInterfaceId::from_alias("unknown-tun");
    *rt.network_watcher.lock().unwrap() = Some(tokio::spawn(std::future::pending()));
    assert!(rt
        .reset_crashed_run_state(rt.gate.generation(), Some(&identity))
        .await
        .is_none());
    assert_eq!(
        rt.runtime_binding_state
            .lock()
            .unwrap()
            .managed_tun_interface,
        ExitInterfaceId::from_alias("unknown-tun")
    );
    assert!(rt.network_watcher.lock().unwrap().is_some());
    rt.stop_network_watcher();
    let gate = rt.mesh.tailscale_state_gate().await;
    let io = NativeWaitIo::new(WaitFault::Native);
    for _ in 0..2 {
        assert!(rt.kill_direct_core_with_io(Some(&gate), &io).await.is_err());
        assert_retained_main(&rt, "main-echild", pid);
    }
    assert_eq!(
        io.waits.load(Ordering::SeqCst),
        0,
        "no new wait can adopt a reused PID"
    );
    assert_eq!(io.terms.load(Ordering::SeqCst), 0);
    assert_eq!(io.kills.load(Ordering::SeqCst), 0);
    // Drop isolates just this ECHILD wrapper from Tokio's own Reaper/orphan queue.
    drop(gate);
    drop(rt);
}

#[cfg(unix)]
#[test]
fn injected_echild_cannot_be_repaired_by_a_later_success_or_signal() {
    // Externally kill/reap this stand-in after the injected custody failure,
    // before its quarantined wrapper is dropped.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let process = spawn_custody_stand_in();
        let pid = process.id().unwrap();
        let mut run = DirectCoreRun::new(process);
        assert!(run
            .try_wait_with(|_| Err(io::Error::from_raw_os_error(
                nix::errno::Errno::ECHILD as i32
            )))
            .is_err());
        assert!(run
            .try_wait_with(|_| panic!("lost wait cannot be retried"))
            .is_err());
        assert!(run
            .signal_with(|_| panic!("lost identity cannot signal"))
            .is_err());
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
        tokio::task::spawn_blocking(move || {
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                None,
            )
        })
        .await
        .unwrap()
        .unwrap();
        drop(run);
    });
}

#[tokio::test]
async fn native_wait_error_after_term_retains_live_child_pid_and_main_for_retry() {
    let (rt, _dir, _identity, token, pid) = live_main_run("native-wait-error").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    let fault = NativeWaitIo::new(WaitFault::PendingThenError);
    let error = rt
        .kill_direct_core_with_io(Some(&gate), &fault)
        .await
        .unwrap_err();
    assert!(error.contains("injected native wait failure"));
    assert_eq!(fault.terms.load(Ordering::SeqCst), 1);
    assert_retained_main(&rt, "native-wait-error", pid);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .stopping_child_for_test()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|retained| retained.same(&token)));

    rt.kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap();
    assert!(rt.child.lock().unwrap().is_empty());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(!rt.mesh.main_owns_tailscale("native-wait-error", true));
}

#[tokio::test]
async fn native_wait_error_on_pid_none_retains_claim_until_real_nonzero_exit_receipt() {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("powershell");
        command.args(["-NoProfile", "-Command", "exit 17"]);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exit 17"]);
        command
    };
    let mut child = command.kill_on_drop(true).spawn().unwrap();
    let pid = child.id().unwrap();
    assert_eq!(child.wait().await.unwrap().code(), Some(17));
    assert_eq!(child.id(), None);
    let (rt, _dir) = test_runtime();
    install_main_run(&rt, child, pid, "native-reaped-error").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    let fault = NativeWaitIo::new(WaitFault::Error);
    assert!(rt
        .kill_direct_core_with_io(Some(&gate), &fault)
        .await
        .is_err());
    assert_eq!(fault.terms.load(Ordering::SeqCst), 0);
    assert_retained_main(&rt, "native-reaped-error", pid);
    // An ordinary nonzero exit still proves that this owned Child exited.
    rt.kill_core_and_release_main(&gate).await.unwrap();
    assert!(rt.child.lock().unwrap().is_empty());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(!rt.mesh.main_owns_tailscale("native-reaped-error", true));
}

#[tokio::test]
async fn cancelled_native_wait_blocks_overlapping_stop_and_spawn_then_retries_same_child() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("native-cancel").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    let pending = NativeWaitIo::new(WaitFault::Pending);
    let mut stop = Box::pin(rt.kill_direct_core_with_io(Some(&gate), &pending));
    assert!(futures::poll!(stop.as_mut()).is_pending());
    assert_eq!(pending.terms.load(Ordering::SeqCst), 1);
    assert!(rt
        .kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .is_err());
    let Err(rejected) = rt
        .child
        .lock()
        .unwrap()
        .install_running(DirectCoreRun::new(spawn_custody_stand_in()))
    else {
        panic!("native wait custody must reject replacement before spawn publication");
    };
    let mut returned = rejected.into_child_for_test();
    returned.kill().await.unwrap();
    assert_retained_main(&rt, "native-cancel", pid);
    drop(stop);
    assert_retained_main(&rt, "native-cancel", pid);
    assert!(rt
        .child
        .lock()
        .unwrap()
        .stopping_child_for_test()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    rt.kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap();
    assert!(!rt.mesh.main_owns_tailscale("native-cancel", true));
    let next = DirectCoreRun::new(spawn_custody_stand_in());
    rt.child.lock().unwrap().install_running_for_test(next);
    rt.kill_core().await.unwrap();
}

#[tokio::test]
async fn reaped_native_child_survives_commit_error_and_retry_uses_cached_exit() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("native-cached-exit").await;
    let error = rt
        .kill_direct_core_with_io(None, &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap_err();
    assert!(error.contains("main claim release requires TS gate"));
    assert_retained_main(&rt, "native-cached-exit", pid);
    assert_eq!(
        rt.child
            .lock()
            .unwrap()
            .stopping_child_for_test()
            .unwrap()
            .id(),
        None
    );
    let gate = rt.mesh.tailscale_state_gate().await;
    let must_not_wait_again = NativeWaitIo::new(WaitFault::Error);
    rt.kill_direct_core_with_io(Some(&gate), &must_not_wait_again)
        .await
        .unwrap();
    assert_eq!(must_not_wait_again.waits.load(Ordering::SeqCst), 0);
    assert_eq!(must_not_wait_again.terms.load(Ordering::SeqCst), 0);
    assert!(rt.child.lock().unwrap().is_empty());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(!rt.mesh.main_owns_tailscale("native-cached-exit", true));
}

#[tokio::test]
async fn native_exit_does_not_release_successor_main_claim_and_can_retry_original() {
    let (rt, _dir, _identity, original, pid) = live_main_run("native-main-successor").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    assert!(rt
        .mesh
        .release_tailscale_main_states_if_token(&original, &gate)
        .unwrap());
    let state = rt
        .mesh
        .tailscale_state_dir("native-main-successor")
        .unwrap();
    let generated =
        serde_json::json!({"endpoints":[{"type":"tailscale", "state_directory":state}]});
    let successor = rt.mesh.mint_tailscale_main_birth();
    let mut reservation = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, successor.clone())
        .await
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    assert!(rt
        .kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap_err()
        .contains("no longer matches"));
    assert_retained_main(&rt, "native-main-successor", pid);
    assert!(rt
        .main_token_for_stop()
        .unwrap()
        .is_some_and(|retained| retained.same(&original)));
    assert!(
        rt.mesh
            .release_tailscale_main_states_if_token(&successor, &gate)
            .unwrap(),
        "the successor claim must remain untouched"
    );
    let mut restored = rt
        .mesh
        .reserve_tailscale_main_states(&generated, &gate, original)
        .await
        .unwrap();
    restored.arm_external_start();
    drop(restored);
    let no_more_wait = NativeWaitIo::new(WaitFault::Error);
    rt.kill_direct_core_with_io(Some(&gate), &no_more_wait)
        .await
        .unwrap();
    assert_eq!(no_more_wait.waits.load(Ordering::SeqCst), 0);
    assert!(rt.child.lock().unwrap().is_empty());
}

#[tokio::test]
async fn poisoned_registry_retains_native_exit_child_pid_and_claim_without_resignalling() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("native-main-poison").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    rt.mesh.poison_tailscale_main_claim_lock_for_test();
    assert!(rt
        .kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap_err()
        .contains("registry lock poisoned"));
    assert_retained_main(&rt, "native-main-poison", pid);
    assert_eq!(
        rt.child
            .lock()
            .unwrap()
            .stopping_child_for_test()
            .unwrap()
            .id(),
        None
    );
    let no_more_wait = NativeWaitIo::new(WaitFault::Error);
    assert!(rt
        .kill_direct_core_with_io(Some(&gate), &no_more_wait)
        .await
        .is_err());
    assert_eq!(no_more_wait.waits.load(Ordering::SeqCst), 0);
    assert_eq!(no_more_wait.terms.load(Ordering::SeqCst), 0);
    assert_retained_main(&rt, "native-main-poison", pid);
}

#[test]
fn windows_native_terminate_uses_owned_handle_without_blocking_pid_command() {
    // Linux cannot exercise Windows APIs. This source gate covers the actual
    // default I/O branch; the cross-target check verifies its platform types.
    let source = module_code("runtime/proxy/process_supervision/direct_stop");
    let terminate = source
        .split("fn terminate(")
        .nth(1)
        .unwrap()
        .split("#[cfg(not(windows))]")
        .next()
        .unwrap();
    assert!(terminate.contains("#[cfg(windows)]"));
    assert!(terminate.contains("child.start_kill()"));
    for forbidden in ["send_signal(", "taskkill", ".output()", "child.id()"] {
        assert!(
            !terminate.contains(forbidden),
            "Windows native stop uses {forbidden}"
        );
    }
}

#[tokio::test]
async fn old_native_booking_cannot_unbook_retry_or_retire_replacement_run() {
    let (rt, _dir) = test_runtime();
    let mut child = spawn_custody_stand_in();
    child.kill().await.unwrap();
    rt.child
        .lock()
        .unwrap()
        .install_running_for_test(DirectCoreRun::new(child));
    let (identity, old_nonce) = rt
        .child
        .lock()
        .unwrap()
        .begin_native_stop()
        .unwrap()
        .unwrap();
    let native = NativeWaitIo::new(WaitFault::Native);
    poll_fn(|cx| {
        rt.child
            .lock()
            .unwrap()
            .poll_native_wait(&identity, &old_nonce, cx, |child, cx| {
                native.poll_wait(child, cx)
            })
    })
    .await
    .unwrap();
    let (_, retry_nonce) = {
        let mut slot = rt.child.lock().unwrap();
        slot.finish_native_stop_booking(&identity, &old_nonce);
        slot.begin_native_stop().unwrap().unwrap()
    };
    {
        let mut slot = rt.child.lock().unwrap();
        slot.finish_native_stop_booking(&identity, &old_nonce);
        assert!(
            slot.begin_native_stop().is_err(),
            "old Drop cannot unbook retry"
        );
        assert!(slot
            .retire_native_stop(&identity, &old_nonce, |_| panic!("stale release"))
            .is_err());
        slot.retire_native_stop(&identity, &retry_nonce, |_| Ok(()))
            .unwrap();
    }
    let replacement = DirectCoreRun::new(spawn_custody_stand_in());
    let replacement_identity = replacement.identity.clone();
    let replacement_pid = replacement.child_id_for_test().unwrap();
    {
        let mut slot = rt.child.lock().unwrap();
        slot.install_running_for_test(replacement);
        *rt.pid.lock().unwrap() = Some(replacement_pid);
        slot.finish_native_stop_booking(&identity, &old_nonce);
        assert!(slot
            .retire_native_stop(&identity, &old_nonce, |_| panic!("stale release"))
            .is_err());
        assert!(slot.running_matches(&replacement_identity));
        assert_eq!(*rt.pid.lock().unwrap(), Some(replacement_pid));
    }
    rt.kill_core().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn owned_kill_failure_retains_native_child_and_main_custody() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("native-kill-error").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    let mut fault = NativeWaitIo::new(WaitFault::Pending);
    fault.kill_error = true;
    let mut stop = Box::pin(rt.kill_direct_core_with_io(Some(&gate), &fault));
    assert!(futures::poll!(stop.as_mut()).is_pending());
    tokio::time::advance(STOP_GRACE).await;
    assert!(stop
        .await
        .unwrap_err()
        .contains("injected owned Child kill failure"));
    assert_eq!(fault.kills.load(Ordering::SeqCst), 1);
    assert_retained_main(&rt, "native-kill-error", pid);
    tokio::time::resume();
    rt.kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn kill_ack_without_native_wait_exit_times_out_and_keeps_main_claim() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("native-kill-ack").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    let fault = NativeWaitIo::new(WaitFault::Pending);
    let mut stop = Box::pin(rt.kill_direct_core_with_io(Some(&gate), &fault));
    assert!(futures::poll!(stop.as_mut()).is_pending());
    tokio::time::advance(STOP_GRACE).await;
    assert!(futures::poll!(stop.as_mut()).is_pending());
    assert_eq!(fault.kills.load(Ordering::SeqCst), 1);
    tokio::time::advance(STOP_GRACE).await;
    assert!(stop.await.unwrap_err().contains("native wait timed out"));
    assert_retained_main(&rt, "native-kill-ack", pid);
    tokio::time::resume();
    rt.kill_direct_core_with_io(Some(&gate), &NativeWaitIo::new(WaitFault::Native))
        .await
        .unwrap();
}

#[tokio::test]
async fn shutdown_fence_rejects_public_and_guarded_starts_and_runtime_restart() {
    let (rt, _dir) = test_runtime();
    rt.begin_shutdown().unwrap();
    let generation = rt.gate.generation();
    assert!(rt
        .start(serde_json::Value::Null)
        .await
        .unwrap_err()
        .message
        .contains("shutting down"));
    assert!(rt
        .restart(serde_json::Value::Null)
        .await
        .unwrap_err()
        .message
        .contains("shutting down"));
    assert!(matches!(
        rt.start_guarded(serde_json::Value::Null, Some(generation))
            .await,
        super::super::super::lifecycle::StartLeg::Finished(Err(_), None)
    ));
    assert_eq!(
        rt.gate.generation(),
        generation,
        "rejected starts cannot supersede an exit drain"
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    rt.debounced
        .schedule_with_ticket(true, move |outcome, ticket| {
            let _ = tx.send((outcome, ticket));
        });
    let (_, ticket) = rx.await.unwrap();
    assert!(rt
        .claim_debounced_restart(None, generation, ticket)
        .is_none());
    assert_eq!(rt.gate.generation(), generation);
    rt.shutdown_for_exit().await.unwrap();
    rt.shutdown_for_exit().await.unwrap();
}

#[tokio::test]
async fn superseded_exit_drain_is_an_error_and_retry_reaps_the_same_owned_birth() {
    let (rt, _dir, identity, _token, pid) = live_main_run("exit-superseded").await;
    let held_gate = rt.mesh.tailscale_state_gate().await;
    let mut drain = Box::pin(rt.shutdown_for_exit());
    assert!(futures::poll!(drain.as_mut()).is_pending());
    let mut ordinary_stop = Box::pin(rt.stop());
    assert!(futures::poll!(ordinary_stop.as_mut()).is_pending());
    drop(ordinary_stop);
    drop(held_gate);
    assert!(drain.await.unwrap_err().contains("superseded"));
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    assert_eq!(*rt.pid.lock().unwrap(), Some(pid));
    assert!(rt.mesh.main_owns_tailscale("exit-superseded", true));
    rt.shutdown_for_exit().await.unwrap();
    assert!(rt.child.lock().unwrap().is_empty());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(!rt.mesh.main_owns_tailscale("exit-superseded", true));
}

#[tokio::test]
async fn strict_exit_registry_error_retains_native_custody_and_stays_closed() {
    let (rt, _dir, _identity, _token, pid) = live_main_run("exit-registry-error").await;
    rt.mesh.poison_tailscale_main_claim_lock_for_test();
    assert!(rt
        .shutdown_for_exit()
        .await
        .unwrap_err()
        .contains("poisoned"));
    assert_retained_main(&rt, "exit-registry-error", pid);
    assert!(rt
        .start(serde_json::Value::Null)
        .await
        .unwrap_err()
        .message
        .contains("shutting down"));
}

#[tokio::test]
async fn empty_child_slot_cannot_hide_a_residual_main_claim_from_exit() {
    let (rt, _dir) = test_runtime();
    let token = rt.mesh.mint_tailscale_main_birth();
    {
        let gate = rt.mesh.tailscale_state_gate().await;
        let state = rt.mesh.tailscale_state_dir("exit-residual-claim").unwrap();
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
    }
    assert!(rt.child.lock().unwrap().is_empty());
    assert!(rt.shutdown_for_exit().await.is_err());
    assert!(rt.mesh.main_owns_tailscale("exit-residual-claim", true));
    assert!(*rt.desktop_shutdown.lock().unwrap());
    {
        let gate = rt.mesh.tailscale_state_gate().await;
        assert!(rt
            .mesh
            .release_tailscale_main_states_if_token(&token, &gate)
            .unwrap());
    }
    rt.shutdown_for_exit().await.unwrap();
}

#[tokio::test]
async fn empty_child_slot_and_poisoned_main_registry_never_authorize_exit() {
    let (rt, _dir) = test_runtime();
    rt.mesh.poison_tailscale_main_claim_lock_for_test();
    assert!(rt.child.lock().unwrap().is_empty());
    assert!(rt
        .shutdown_for_exit()
        .await
        .unwrap_err()
        .contains("unknown"));
    assert!(*rt.desktop_shutdown.lock().unwrap());
}

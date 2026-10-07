use super::*;
#[cfg(unix)]
use crate::runtime::proxy::mesh_apply::owner_proof::{ATTACHED, FACTORY_ENTERED, RETIRED};
#[cfg(unix)]
use polaris_core_supervisor::{SingBoxSpawner, TokioSpawner};
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
    #[cfg(unix)]
    let (identity, token, pid) = protected_main_run(&rt, &dir, server_id).await;
    #[cfg(not(unix))]
    let (identity, token, pid) = {
        let child = spawn_custody_stand_in();
        let pid = child.id().unwrap();
        let (identity, token) = install_main_run(&rt, child, pid, server_id).await;
        (identity, token, pid)
    };
    (rt, dir, identity, token, pid)
}

/// Harmless local sleep fixture, routed through the same production factory.
#[cfg(unix)]
fn native_fixture_request(
    dir: &TestDir,
    stdio: polaris_core_supervisor::StdioPolicy,
) -> polaris_core_supervisor::SpawnRequest {
    use std::os::unix::fs::PermissionsExt;
    let binary = dir.join("native-child-fixture.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    polaris_core_supervisor::SpawnRequest::new(
        binary,
        dir.join("ignored-native-fixture-config"),
        stdio,
    )
}

#[cfg(unix)]
async fn protected_main_run(
    rt: &Arc<ProxyRuntime>,
    dir: &TestDir,
    server_id: &str,
) -> (
    RunIdentity,
    crate::runtime::tailscale_login_core::MainBirthToken,
    u32,
) {
    use super::super::super::prerequisite::NormalStartCompletion;
    let gate = rt.mesh.tailscale_state_gate().await;
    let mut completion = rt.normal_start_completion(serde_json::Value::Null).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while matches!(
            &*completion.borrow_and_update(),
            NormalStartCompletion::Pending(_)
        ) {
            completion.changed().await.unwrap();
        }
    })
    .await
    .expect("actual normal producer admission before TS wait");
    let generation = rt.core_generation();
    assert!(
        matches!(&*completion.borrow(), NormalStartCompletion::Starting(owner) if *owner == generation)
    );
    let producer = rt.admitted_native_producer(generation).unwrap().unwrap();
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
    let (pid, identity) = rt
        .spawn_direct_native(
            Some(&producer),
            generation,
            || {
                TokioSpawner::new()
                    .spawn(native_fixture_request(
                        dir,
                        polaris_core_supervisor::StdioPolicy::Discard,
                    ))
                    .map(|spawned| spawned.child)
            },
            Some(token.clone()),
            || reservation.arm_external_start(),
        )
        .unwrap()
        .expect("actual protected factory installs fixture Child");
    *rt.pid.lock().unwrap() = Some(pid);
    drop(reservation);
    drop(gate);
    // The ordinary continuation sees its already-installed Child and returns;
    // only that actual return closes dispatch. No manual mark-admitted/finished.
    tokio::time::timeout(Duration::from_secs(2), async {
        while !matches!(
            &*completion.borrow_and_update(),
            NormalStartCompletion::Finished(Err(_), _)
        ) {
            completion.changed().await.unwrap();
        }
    })
    .await
    .expect("ordinary producer returns without starting another core");
    assert!(
        !producer.reclaimable(),
        "attached Child remains a responsibility after observer failure"
    );
    (identity, token, pid)
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
    #[cfg(unix)]
    assert!(
        rt.child.lock().unwrap().native_exit_for_test(),
        "actual native fact survives fallible registry commit"
    );
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
    #[cfg(unix)]
    assert!(
        rt.child.lock().unwrap().native_exit_for_test(),
        "actual native fact survives fallible registry commit"
    );
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
    #[cfg(unix)]
    assert!(
        rt.child.lock().unwrap().native_exit_for_test(),
        "actual native fact survives fallible registry commit"
    );
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
    #[cfg(unix)]
    let _protected = protected_main_run(&rt, &_dir, "native-booking").await;
    #[cfg(not(unix))]
    {
        let mut child = spawn_custody_stand_in();
        child.kill().await.unwrap();
        rt.child
            .lock()
            .unwrap()
            .install_running_for_test(DirectCoreRun::new(child));
    }
    #[cfg(unix)]
    rt.child
        .lock()
        .unwrap()
        .running_for_test()
        .unwrap()
        .child_for_test()
        .start_kill()
        .unwrap();
    let gate = rt.mesh.tailscale_state_gate().await;
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
        #[cfg(unix)]
        assert!(
            slot.native_exit_for_test(),
            "retry keeps original fact but validates current booking"
        );
        let terminal = slot
            .retire_native_stop(&identity, &retry_nonce, |token| {
                assert!(rt
                    .mesh
                    .release_tailscale_main_states_if_token(token, &gate)?);
                Ok(())
            })
            .unwrap();
        #[cfg(unix)]
        assert!(terminal.as_ref().unwrap().same_run_for_test(&identity));
        #[cfg(not(unix))]
        assert!(
            terminal.is_none(),
            "synthetic unbound fixture issues no terminal"
        );
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

#[cfg(unix)]
#[tokio::test]
async fn protected_factory_native_terminal_reclaims_original_cell_and_admits_successor() {
    let (rt, dir, _identity, _token, _pid) = live_main_run("native-proof-positive").await;
    let birth_generation = rt.core_generation();
    let original = rt
        .admitted_native_producer(birth_generation)
        .unwrap()
        .unwrap();
    assert_eq!(
        original.observation().3,
        vec![ATTACHED],
        "one actual returned Child seals exactly one member"
    );
    assert!(
        original.observation().2,
        "ordinary producer actually returned"
    );
    let stop_generation = rt
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    assert_ne!(birth_generation, stop_generation);
    let gate = rt.mesh.tailscale_state_gate().await;
    rt.kill_core_and_release_main(&gate).await.unwrap();
    assert!(rt.child.lock().unwrap().is_empty());
    assert_eq!(*rt.pid.lock().unwrap(), None);
    assert!(!rt.mesh.main_owns_tailscale("native-proof-positive", true));
    assert_eq!(
        original.observation().3,
        vec![RETIRED],
        "real native wait and real retire consume original birth"
    );
    assert!(original.reclaimable());
    assert!(rt
        .admitted_native_producer(birth_generation)
        .unwrap()
        .is_none());
    drop(gate);
    // Successor uses actual ordinary enrollment/admission and same protected factory.
    let (_identity, _token, _pid) = protected_main_run(&rt, &dir, "native-proof-positive").await;
    let successor = rt
        .admitted_native_producer(rt.core_generation())
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(&original, &successor));
    assert!(rt.core_generation() > stop_generation);
    let gate = rt.mesh.tailscale_state_gate().await;
    rt.kill_core_and_release_main(&gate).await.unwrap();
    assert!(successor.reclaimable());
}

#[cfg(unix)]
#[tokio::test]
async fn synchronous_drain_panic_retains_factory_entered_unknown_after_dispatch_return() {
    const CHILD_ENV: &str = "POLARIS_NATIVE_DRAIN_PANIC_FIXTURE";
    if std::env::var_os(CHILD_ENV).is_none() {
        // Panic intentionally poisons the real process-singleton check admission.
        // Isolate that production fault without resetting or weakening the singleton.
        let dir = fresh_test_dir();
        let output_path = dir.join("native-drain-panic-worker.log");
        let output = std::fs::File::create(&output_path).unwrap();
        let test_name = concat!(
            module_path!(),
            "::synchronous_drain_panic_retains_factory_entered_unknown_after_dispatch_return"
        )
        .split_once("::")
        .unwrap()
        .1;
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_ENV, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(output.try_clone().unwrap()))
            .stderr(std::process::Stdio::from(output))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(status) => status.unwrap(),
            Err(_) => {
                child.start_kill().unwrap();
                tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .unwrap()
                    .unwrap();
                panic!("isolated native Drain panic fixture timed out");
            }
        };
        let output = std::fs::read_to_string(output_path).unwrap();
        assert!(status.success(), "isolated fixture failed: {output}");
        assert!(
            output.contains(test_name)
                && output.contains("running 1 test")
                && output.contains("test result: ok. 1 passed; 0 failed;")
                && output.contains("POLARIS_U301A_DRAIN_PANIC_UNKNOWN_CHECKED"),
            "exact one-test worker must execute every Unknown assertion: {output}"
        );
        return;
    }
    use super::super::super::prerequisite::NormalStartCompletion;
    let (rt, _dir) = test_runtime();
    let gate = rt.mesh.tailscale_state_gate().await;
    let mut completion = rt.normal_start_completion(serde_json::Value::Null).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while matches!(
            &*completion.borrow_and_update(),
            NormalStartCompletion::Pending(_)
        ) {
            completion.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let generation = rt.core_generation();
    let producer = rt.admitted_native_producer(generation).unwrap().unwrap();
    // /bin/true exits immediately; the permitted stand-in never launches a core.
    let request = polaris_core_supervisor::SpawnRequest::new(
        "/bin/true",
        "/unused-fixture-config",
        polaris_core_supervisor::StdioPolicy::drain(|_stdout, _stderr| {
            panic!("fixture synchronous Drain panic")
        }),
    );
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.spawn_direct_native(
            Some(&producer),
            generation,
            || {
                TokioSpawner::new()
                    .spawn(request)
                    .map(|spawned| spawned.child)
            },
            None,
            || {},
        )
    }))
    .is_err());
    assert_eq!(
        producer.observation().3,
        vec![FACTORY_ENTERED],
        "factory entered before callback could unwind"
    );
    drop(gate);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !matches!(
            &*completion.borrow_and_update(),
            NormalStartCompletion::Finished(Err(_), _)
        ) {
            completion.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(producer.observation().2);
    assert!(
        !producer.reclaimable(),
        "returned observer error cannot prove no Child after factory panic"
    );
    assert!(rt.admitted_native_producer(generation).unwrap().is_some());
    eprintln!("POLARIS_U301A_DRAIN_PANIC_UNKNOWN_CHECKED");
}

#[tokio::test]
async fn unbound_fixture_native_wait_issues_no_typed_birth_fact() {
    let (rt, _dir) = test_runtime();
    let child = spawn_custody_stand_in();
    rt.child
        .lock()
        .unwrap()
        .install_running_for_test(DirectCoreRun::new(child));
    let (identity, nonce) = rt
        .child
        .lock()
        .unwrap()
        .begin_native_stop()
        .unwrap()
        .unwrap();
    rt.child
        .lock()
        .unwrap()
        .signal_native_stop(&identity, &nonce, |child| child.start_kill())
        .unwrap();
    let native = NativeWaitIo::new(WaitFault::Native);
    poll_fn(|cx| {
        rt.child
            .lock()
            .unwrap()
            .poll_native_wait(&identity, &nonce, cx, |child, cx| {
                native.poll_wait(child, cx)
            })
    })
    .await
    .unwrap();
    let mut slot = rt.child.lock().unwrap();
    assert!(!slot.native_exit_for_test());
    slot.retire_native_stop(&identity, &nonce, |_| panic!("fixture has no main token"))
        .unwrap();
    assert!(slot.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn f1_original_main_membership_survives_stop_generation_and_explicit_surrender() {
    const MARKER: &str = "POLARIS_F1_MAIN_PAUSE_CHILD";
    if std::env::var_os(MARKER).is_none() {
        let test = format!(
            "{}::f1_original_main_membership_survives_stop_generation_and_explicit_surrender",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--nocapture"])
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    use crate::runtime::proxy::mesh_apply::pc_owner_census::TailObligation;
    let (rt, dir, identity, _token, _pid) = live_main_run("f1-original-main-positive").await;
    let generation = rt.core_generation();
    let pause = rt.pause_pc_producers(generation).await.unwrap();
    let observer = pause.clone();
    let membership = pause.seal(&rt).unwrap();
    assert!(membership.member_counts().0 > 0);
    assert!(membership
        .main_native_phases()
        .iter()
        .any(|(_, phases)| phases == &[ATTACHED]));
    assert!(membership
        .tail_obligations()
        .contains(&TailObligation::MainDetachedLogTasks));
    assert!(membership
        .tail_obligations()
        .contains(&TailObligation::TempConfigBestEffort));
    let (foreign, _foreign_dir) = test_runtime();
    assert!(pause.surrender(&foreign).is_err());
    drop(observer);
    assert!(rt.normal_start_completion(serde_json::Value::Null).is_err());
    assert!(matches!(
        rt.restart_guarded_outcome(serde_json::Value::Null, Some(generation))
            .await,
        super::super::super::lifecycle::RestartLeg::Finished(Err(_), _)
    ));
    assert_eq!(
        rt.core_generation(),
        generation,
        "late restart cannot take the Stop leg"
    );
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    let displaced = rt.child.lock().unwrap().take_running_for_test().unwrap();
    assert!(
        pause.surrender(&rt).is_err(),
        "attached metadata cannot replace physical original custody"
    );
    rt.child.lock().unwrap().install_running_for_test(displaced);
    // Exercise the original ordinary Stop generation authority without OS teardown.
    let stopped = rt
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    assert_ne!(stopped, generation);
    assert!(membership.assert_fresh(&rt).is_err());
    assert!(pause.seal(&rt).is_err());
    let gate = rt.mesh.tailscale_state_gate().await;
    rt.kill_core_and_release_main(&gate).await.unwrap();
    drop(gate);
    assert!(membership
        .main_native_phases()
        .iter()
        .any(|(_, phases)| phases == &[RETIRED]));
    assert!(membership
        .tail_obligations()
        .contains(&TailObligation::MainDetachedLogTasks));
    pause.surrender(&rt).unwrap();
    assert!(
        pause.surrender(&rt).is_err(),
        "same lease cannot release twice"
    );
    assert!(
        membership.assert_fresh(&rt).is_err(),
        "surrender grants no fresh census"
    );
    let (_new_run, _new_token, _new_pid) =
        protected_main_run(&rt, &dir, "f1-original-main-positive").await;
    let gate = rt.mesh.tailscale_state_gate().await;
    rt.kill_core_and_release_main(&gate).await.unwrap();
    drop(gate);
    let exit_pause = rt.pause_pc_producers(rt.core_generation()).await.unwrap();
    rt.begin_shutdown().unwrap();
    assert!(exit_pause.surrender(&rt).is_err());
    assert!(*rt.desktop_shutdown.lock().unwrap());
}

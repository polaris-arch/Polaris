#![cfg(not(any(target_os = "android", target_os = "ios")))]

use super::*;
use polaris_core_supervisor::{
    classify_child_exit, ChildObservation, CoreReadyOutcome, ExitClassification,
};

fn install_observed_run(rt: &Arc<ProxyRuntime>) -> RunIdentity {
    let run = DirectCoreRun::new(spawn_custody_stand_in());
    let identity = run.identity.clone();
    rt.child.lock().unwrap().install_running_for_test(run);
    identity
}

#[tokio::test]
async fn unknown_native_observation_reaches_real_readiness_without_releasing_child() {
    let (rt, _dir) = test_runtime();
    let identity = install_observed_run(&rt);
    // Reserve a loopback port without listening. The first failed TCP probe
    // must traverse startup's actual Child observation before API readiness.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = socket.local_addr().unwrap().port();
    let observed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let ready = observed.clone();
    let done = release.clone();
    let listener = tokio::spawn(async move {
        ready.notified().await;
        let listener = socket.listen(8).unwrap();
        done.notified().await;
        drop(listener);
    });
    rt.child
        .lock()
        .unwrap()
        .inject_observation_wait_for_test(Box::new(move |_| {
            observed.notify_one();
            Err(std::io::Error::other("injected transient try_wait error"))
        }));
    assert_eq!(
        rt.wait_ready(port, rt.gate.generation(), 1_000).await,
        CoreReadyOutcome::Ready
    );
    let observation = rt.child.lock().unwrap().observe_running(&identity);
    assert_eq!(observation, ChildObservation::Unknown);
    assert_eq!(
        classify_child_exit(1, 1, observation),
        ExitClassification::KeepWatching
    );
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    // Real cleanup remains possible; Unknown never extracted or killed custody.
    release.notify_one();
    listener.await.unwrap();
    rt.kill_core().await.unwrap();
}

#[tokio::test]
async fn persistent_unknown_is_bounded_timeout_and_preserves_custody_until_stop() {
    let (rt, _dir) = test_runtime();
    let identity = install_observed_run(&rt);
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = socket.local_addr().unwrap().port();
    rt.child
        .lock()
        .unwrap()
        .inject_observation_wait_for_test(Box::new(|_| {
            Err(std::io::Error::other("persistent unknown"))
        }));
    assert_eq!(
        rt.wait_ready(port, rt.gate.generation(), 100).await,
        CoreReadyOutcome::Timeout
    );
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    rt.kill_core().await.unwrap();
}

#[tokio::test]
async fn generation_takeover_during_unknown_is_superseded_and_keeps_the_run() {
    let (rt, _dir) = test_runtime();
    let identity = install_observed_run(&rt);
    let generation = rt.gate.generation();
    let gate = rt.gate.clone();
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = socket.local_addr().unwrap().port();
    rt.child
        .lock()
        .unwrap()
        .inject_observation_wait_for_test(Box::new(move |_| {
            gate.bump_generation();
            Err(std::io::Error::other("observation raced Stop/new Start"))
        }));
    assert_eq!(
        rt.wait_ready(port, generation, 100).await,
        CoreReadyOutcome::Superseded
    );
    assert!(rt.child.lock().unwrap().running_matches(&identity));
    rt.kill_core().await.unwrap();
}

#[tokio::test]
async fn confirmed_exit_remains_dead_and_absent_run_yields_to_owner() {
    let (rt, _dir) = test_runtime();
    install_observed_run(&rt);
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = socket.local_addr().unwrap().port();
    rt.child
        .lock()
        .unwrap()
        .inject_observation_wait_for_test(Box::new(|_| {
            #[cfg(unix)]
            use std::os::unix::process::ExitStatusExt;
            #[cfg(windows)]
            use std::os::windows::process::ExitStatusExt;
            Ok(Some(std::process::ExitStatus::from_raw(0)))
        }));
    assert_eq!(
        rt.wait_ready(port, rt.gate.generation(), 100).await,
        CoreReadyOutcome::Dead
    );
    rt.kill_core().await.unwrap();
    assert_eq!(
        rt.wait_ready(port, rt.gate.generation(), 100).await,
        CoreReadyOutcome::Superseded
    );
}

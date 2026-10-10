//! Production observer + original admitted collector, injected command bytes only.
use super::*;
use crate::commands::coexistence_snapshot::{CommandCustody, ObservationSource, StartedCommand};
use polaris_system_integration::exec::{Command, CommandCleanup, CommandOutput};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Condvar};
struct Returned;
impl CommandCustody for Returned {
    fn poll_cleanup(&mut self, _: Duration) -> CommandCleanup {
        CommandCleanup::NativeAndReadersReturned
    }
    fn operation(&self) -> Option<Result<CommandOutput, String>> {
        Some(Ok(CommandOutput {
            stdout: "[]".into(),
            stderr: String::new(),
        }))
    }
}
struct EmptySource;
impl ObservationSource for EmptySource {
    fn start(&self, _: &Command, _: Duration) -> StartedCommand {
        StartedCommand::Owned(Box::new(Returned))
    }
}
fn fixture() -> (Arc<Observer>, Arc<AtomicU64>, Arc<AtomicUsize>) {
    let clock = Arc::new(AtomicU64::new(0));
    let now = clock.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    (
        Arc::new(Observer {
            state: Mutex::default(),
            sink: Mutex::default(),
            service: Arc::default(),
            collector: Arc::new(move |a| {
                count.fetch_add(1, Ordering::SeqCst);
                snapshot::collect_admitted(a, polaris_helper_proto::Platform::Linux, EmptySource)
            }),
            clock: Arc::new(move || Duration::from_millis(now.load(Ordering::SeqCst))),
            interval: Duration::ZERO,
        }),
        clock,
        calls,
    )
}
fn bind(o: &Arc<Observer>, gen: u64) {
    // Invoke the actual production hooks. Starting phase cannot auto-acquire.
    o.claim(gen, true);
    o.commit_config(&mut None, json!({"fixtureApplied":gen}));
    o.begin_watcher(gen).unwrap();
    o.watcher(gen, 1, true);
    {
        o.lock().binding.phase = "ready";
    }
}
fn run(o: &Observer) {
    o.lock().running = true;
    o.run();
}
fn report(o: &Observer) -> Value {
    o.get()["report"]["value"].clone()
}
#[test]
fn unknown_required_binding_never_becomes_latest() {
    for missing in ["session", "lifecycle", "config", "network"] {
        let (o, _, _) = fixture();
        bind(&o, 7);
        {
            let mut s = o.lock();
            match missing {
                "session" => s.binding.session = None,
                "lifecycle" => s.binding.lifecycle = None,
                "config" => s.binding.config = None,
                _ => s.binding.network = None,
            }
        }
        run(&o);
        assert_ne!(o.get()["freshness"], "latest", "{missing}");
    }
}
#[test]
fn actual_collector_serializer_keeps_inner_unknown_trust() {
    let (o, clock, calls) = fixture();
    bind(&o, 7);
    clock.store(9, Ordering::SeqCst);
    run(&o);
    let state = o.get();
    assert_eq!(state["freshness"], "latest");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let inner = report(&o)["snapshot"].clone();
    assert_eq!(inner["schemaVersion"], 1);
    assert_eq!(inner["classification"]["status"], "unknown");
    for key in [
        "observationPhase",
        "ownInterfaces",
        "criteria",
        "repairHistory",
    ] {
        assert_eq!(inner["context"][key]["status"], "unknown");
    }
}
#[test]
fn failure_is_generation_bound_and_guard_catches_early_return() {
    let (o, _, _) = fixture();
    o.claim(1, true);
    {
        let _guard = StartAttempt::new(o.clone(), 1);
    }
    assert_eq!(o.get()["freshness"], "unavailable");
    assert_eq!(o.get()["binding"]["sessionId"]["status"], "unknown");
    o.claim(2, true);
    o.fail_start(1);
    assert_eq!(o.get()["binding"]["sessionId"]["value"], "2");
    o.fail_start(2);
    assert!(o.lock().pending.is_none());
}
fn blocked() -> (
    Arc<Observer>,
    Arc<(Mutex<bool>, Condvar)>,
    mpsc::Receiver<()>,
) {
    let (o, _, _) = fixture();
    let mut o = Arc::try_unwrap(o).ok().unwrap();
    let collector = o.collector.clone();
    let latch = Arc::new((Mutex::new(false), Condvar::new()));
    let wait = latch.clone();
    let (tx, rx) = mpsc::channel();
    o.collector = Arc::new(move |a| {
        tx.send(()).unwrap();
        let (lock, cv) = &*wait;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = cv.wait(done).unwrap();
        }
        collector(a)
    });
    (Arc::new(o), latch, rx)
}
fn release(latch: &Arc<(Mutex<bool>, Condvar)>) {
    *latch.0.lock().unwrap() = true;
    latch.1.notify_all();
}
#[test]
fn watcher_failure_recovery_and_unknown_aba_reject_old_result() {
    let (o, latch, rx) = blocked();
    bind(&o, 4);
    o.lock().running = true;
    let worker = o.clone();
    let h = std::thread::spawn(move || worker.run());
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // Actual watcher hooks: unavailable ABA cannot accept the original capture.
    o.lock().driver = true;
    o.watcher(4, 1, false);
    o.watcher(4, 1, false);
    o.watcher(4, 1, true);
    release(&latch);
    h.join().unwrap();
    assert_eq!(o.get()["freshness"], "stale");
}
#[test]
fn config_commit_and_capture_compete_at_same_boundary() {
    let (o, latch, rx) = blocked();
    bind(&o, 4);
    o.lock().running = true;
    let worker = o.clone();
    let h = std::thread::spawn(move || worker.run());
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // No async runtime here: phase starting avoids queue; commit still uses actual production lock.
    o.lock().binding.phase = "starting";
    let mut config = Some(json!({"old":true}));
    o.commit_config(&mut config, json!({"new":true}));
    assert_eq!(config, Some(json!({"new":true})));
    release(&latch);
    h.join().unwrap();
    assert_ne!(o.get()["freshness"], "latest");
    assert_ne!(
        report(&o)["acquisition"]["startBinding"]["configGeneration"],
        report(&o)["acquisition"]["endBinding"]["configGeneration"]
    );
}
#[test]
fn stop_crash_and_late_worker_never_publish_old_session() {
    for crash in [false, true] {
        let (o, latch, rx) = blocked();
        bind(&o, 8);
        o.lock().running = true;
        let worker = o.clone();
        let h = std::thread::spawn(move || worker.run());
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        if crash {
            o.terminal(8, true)
        } else {
            o.claim(9, false)
        };
        o.claim(10, true);
        release(&latch);
        h.join().unwrap();
        assert_eq!(o.get()["report"]["status"], "unknown");
        assert_eq!(o.get()["binding"]["sessionId"]["value"], "10");
    }
}
#[test]
fn unknown_phase_is_not_rebranded_before_tun() {
    let (o, _, _) = fixture();
    bind(&o, 9);
    o.lock().binding.phase = "starting";
    run(&o);
    assert_eq!(o.get()["freshness"], "stale");
    assert_eq!(
        report(&o)["snapshot"]["context"]["observationPhase"]["status"],
        "unknown"
    );
}
#[test]
fn actual_service_admission_blocks_manual_competition() {
    let (o, latch, rx) = blocked();
    bind(&o, 1);
    o.lock().running = true;
    let worker = o.clone();
    let h = std::thread::spawn(move || worker.run());
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let denied_while_running = o.service.admit().is_err();
    o.claim(2, false);
    let denied_after_stop = o.service.admit().is_err();
    release(&latch);
    h.join().unwrap();
    assert!(denied_while_running);
    assert!(denied_after_stop);
    assert!(o.service.admit().is_ok());
}
async fn settled(o: &Observer) {
    for _ in 0..2000 {
        if !o.lock().driver {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("fixture driver stuck");
}
#[tokio::test]
async fn burst_coalesces_manual_and_preserves_pending_deadline() {
    let (o, clock, calls) = fixture();
    o.claim(1, true);
    o.commit_config(&mut None, json!({"fixture":1}));
    o.begin_watcher(1).unwrap();
    o.watcher(1, 1, true);
    {
        let mut s = o.lock();
        s.binding.phase = "ready";
        s.driver = true;
    }
    for _ in 0..100 {
        o.receipt(1, 1);
    }
    let first = o.lock().pending.as_ref().unwrap().first;
    clock.store(9000, Ordering::SeqCst);
    let revision = o.lock().revision;
    o.manual(revision);
    assert_eq!(o.lock().pending.as_ref().unwrap().first, first);
    assert!(o.lock().pending.as_ref().unwrap().manual);
    clock.store(10000, Ordering::SeqCst);
    o.clone().drive().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(o.get()["reason"]["value"]["code"], "pendingExpired");
}
#[tokio::test]
async fn original_hook_ready_drives_worker_event_and_state() {
    let (o, _, calls) = fixture();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = events.clone();
    o.set_sink(Arc::new(move |v| sink.lock().unwrap().push(v)));
    o.claim(3, true);
    o.commit_config(&mut None, json!({"actualApplied":true}));
    o.begin_watcher(3).unwrap();
    o.watcher(3, 1, true);
    o.ready(3);
    settled(&o).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(o.get()["freshness"], "latest");
    assert_eq!(events.lock().unwrap().last().unwrap(), &o.get());
    // Actual runtime-generated fixture is emitted only under an explicitly supplied test export path.
    if let Ok(path) = std::env::var("POLARIS_COEX_RUNTIME_FIXTURE") {
        let mut states = serde_json::Map::new();
        states.insert("latest".into(), o.get());
        o.lock().driver = true;
        o.receipt(3, 1);
        states.insert("staleBusy".into(), o.get());
        o.terminal(3, true);
        states.insert("crashed".into(), o.get());
        o.claim(4, false);
        states.insert("stopped".into(), o.get());
        for partial in [true, false] {
            let (windows, _, _) = fixture();
            let mut windows = Arc::try_unwrap(windows).ok().unwrap();
            windows.collector = Arc::new(move |a| {
                snapshot::collect_admitted(
                    a,
                    polaris_helper_proto::Platform::Win,
                    WindowsFixture(partial),
                )
            });
            let windows = Arc::new(windows);
            bind(&windows, 5);
            run(&windows);
            states.insert(
                if partial {
                    "windowsPartial"
                } else {
                    "windowsUnavailable"
                }
                .into(),
                windows.get(),
            );
        }
        std::fs::write(path,serde_json::to_string_pretty(&json!({"provenance":"Actual Observer hooks, original admitted Linux fixture collector, actual runtime serializer; no native API/core/IPC device","states":states})).unwrap()).unwrap();
    }
}

struct NoSystemProxy;
impl super::super::SystemProxyClearer for NoSystemProxy {
    fn ensure_cleared(&mut self) -> bool {
        true
    }
    fn detect_foreign_proxy(&self) -> Option<String> {
        None
    }
    fn enable_system_proxy(
        &mut self,
        _: &polaris_system_integration::proxy_ops::ProxyEnableRequest,
    ) -> Result<(), String> {
        panic!("network writer forbidden in readonly fixture")
    }
    fn recover_from_marker(&mut self) -> Result<bool, String> {
        panic!("network writer forbidden in readonly fixture")
    }
}
#[tokio::test]
async fn facade_claim_commit_receipt_worker_and_get_state_are_same_production_path() {
    use crate::runtime::{
        config::ConfigManager,
        helper::HelperRuntime,
        mesh::MeshRuntime,
        proxy::{NoNetworkDoh, ProxyRuntime},
    };
    use polaris_core_supervisor::LifecycleKind;
    let dir = std::env::temp_dir().join(format!("polaris-coex-runtime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (o, _, calls) = fixture();
    let mut rt = ProxyRuntime::new(
        Arc::new(ConfigManager::new(dir.clone())),
        Arc::new(HelperRuntime::never_installed_for_tests(dir.clone())),
        Arc::new(MeshRuntime::new(dir.clone())),
        Box::new(NoSystemProxy),
        Arc::new(NoNetworkDoh),
    );
    rt.coex = o.clone();
    let rt = Arc::new(rt);
    let gen = rt.claim_generation(None, LifecycleKind::Start).unwrap();
    rt.commit_coex_current_config(json!({"realCommitFixture":true}));
    rt.coex.begin_watcher(gen).unwrap();
    rt.coex.watcher(gen, 1, true);
    rt.coex_committed_ready(gen);
    settled(&o).await;
    assert_eq!(rt.coex_state()["freshness"], "latest");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    rt.coex_network_receipt(gen, 1);
    assert_eq!(rt.coex_state()["freshness"], "stale");
    settled(&o).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    rt.claim_generation(Some(gen), LifecycleKind::Stop).unwrap();
    assert_eq!(rt.coex_state()["report"]["status"], "unknown");
    assert_eq!(rt.coex_state()["freshness"], "unavailable");
    std::fs::remove_dir_all(dir).unwrap();
}

struct WindowsFixture(bool);
impl ObservationSource for WindowsFixture {
    fn start(&self, _: &Command, _: Duration) -> StartedCommand {
        panic!("Windows fixture never spawns a command")
    }
    fn windows_facts(&self) -> polaris_system_integration::coexistence::windows::WindowsFactInput {
        use polaris_config_engine::builder::coexistence::Fact;
        use polaris_system_integration::coexistence::windows::{ReadRows, WindowsFactInput};
        fn unavailable<T>() -> Fact<T> {
            Fact::Unknown("fixture permission unavailable".into())
        }
        WindowsFactInput {
            adapters: if self.0 {
                Fact::Known(ReadRows {
                    rows: vec![],
                    complete: Fact::Unknown("fixture completeness unavailable".into()),
                    compartment: Fact::Unknown("fixture compartment unavailable".into()),
                    error: Fact::Known(None),
                })
            } else {
                Fact::Unknown("fixture permission unavailable".into())
            },
            addresses: unavailable(),
            routes4: unavailable(),
            routes6: unavailable(),
            ras: unavailable(),
        }
    }
}

#[tokio::test]
async fn stuck_worker_pending_expiry_never_releases_original_admission() {
    let (o, clock, calls) = fixture();
    let mut o = Arc::try_unwrap(o).ok().unwrap();
    o.interval = MIN_INTERVAL;
    let collector = o.collector.clone();
    let latch = Arc::new((Mutex::new(false), Condvar::new()));
    let wait = latch.clone();
    let (tx, rx) = mpsc::channel();
    o.collector = Arc::new(move |a| {
        tx.send(()).unwrap();
        let (lock, cv) = &*wait;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cv.wait(ready).unwrap();
        }
        collector(a)
    });
    let o = Arc::new(o);
    bind(&o, 1);
    o.manual(o.get()["reportRevision"].as_str().unwrap().parse().unwrap());
    tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(2)).unwrap())
        .await
        .unwrap();
    clock.store(1000, Ordering::SeqCst);
    for _ in 0..100 {
        o.receipt(1, 1);
    }
    let rev = o.get()["reportRevision"].as_str().unwrap().parse().unwrap();
    o.manual(rev);
    assert_eq!(o.get()["activity"], "runningWithPending");
    assert_eq!(
        o.get()["pending"]["value"]["requestedAtMonotonicMillis"],
        "1000"
    );
    clock.store(11000, Ordering::SeqCst);
    for _ in 0..500 {
        if o.lock().pending.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(o.lock().pending.is_none());
    assert!(o.lock().running);
    assert_eq!(o.get()["reason"]["value"]["code"], "workerStillRunning");
    assert!(o.service.admit().is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    release(&latch);
    settled(&o).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_ne!(o.get()["freshness"], "latest");
}
#[tokio::test]
async fn spacing_starts_at_actual_admission_not_queued_receipt() {
    let (o, clock, calls) = fixture();
    let mut o = Arc::try_unwrap(o).ok().unwrap();
    o.interval = MIN_INTERVAL;
    let o = Arc::new(o);
    bind(&o, 1);
    let held = o.service.admit().unwrap();
    o.manual(o.get()["reportRevision"].as_str().unwrap().parse().unwrap());
    settled(&o).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(o.lock().last_started.is_none());
    held.finish();
    clock.store(3000, Ordering::SeqCst);
    o.manual(o.get()["reportRevision"].as_str().unwrap().parse().unwrap());
    settled(&o).await;
    assert_eq!(o.lock().last_started, Some(Duration::from_millis(3000)));
    clock.store(4999, Ordering::SeqCst);
    o.manual(o.get()["reportRevision"].as_str().unwrap().parse().unwrap());
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    clock.store(5000, Ordering::SeqCst);
    settled(&o).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(o.lock().last_started, Some(Duration::from_millis(5000)));
}

#[test]
fn replaced_watcher_same_generation_cannot_recover_or_advance_new_owner() {
    let (o, _, _) = fixture();
    bind(&o, 1);
    let before = o.get();
    let replacement = o.begin_watcher(1).unwrap();
    assert_eq!(replacement, 2);
    o.watcher(1, 1, true);
    o.receipt(1, 1);
    assert_eq!(o.get()["binding"]["networkEpoch"]["status"], "unknown");
    assert_ne!(o.get(), before);
    o.lock().binding.phase = "starting";
    o.watcher(1, replacement, true);
    let epoch = o.get()["binding"]["networkEpoch"].clone();
    o.watcher(1, 1, false);
    assert_eq!(o.get()["binding"]["networkEpoch"], epoch);
}

#[test]
fn stale_manual_revision_schedules_nothing_and_no_session_never_acquires() {
    let (o, _, calls) = fixture();
    let revision = o.lock().revision;
    o.claim(1, true);
    let before = o.get();
    assert_eq!(o.manual(revision), before);
    assert!(o.lock().pending.is_none());
    o.claim(2, false);
    let revision = o.lock().revision;
    let state = o.manual(revision);
    assert_eq!(state["reason"]["value"]["code"], "noActiveSession");
    assert!(o.lock().pending.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn synchronous_refresh_without_tokio_context_runs_and_releases_driver() {
    use crate::runtime::{
        config::ConfigManager,
        helper::HelperRuntime,
        mesh::MeshRuntime,
        proxy::{NoNetworkDoh, ProxyRuntime},
    };
    use polaris_core_supervisor::LifecycleKind;
    assert!(tokio::runtime::Handle::try_current().is_err());
    let dir = std::env::temp_dir().join(format!("polaris-coex-sync-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (o, _, calls) = fixture();
    let mut rt = ProxyRuntime::new(
        Arc::new(ConfigManager::new(dir.clone())),
        Arc::new(HelperRuntime::never_installed_for_tests(dir.clone())),
        Arc::new(MeshRuntime::new(dir.clone())),
        Box::new(NoSystemProxy),
        Arc::new(NoNetworkDoh),
    );
    rt.coex = o.clone();
    let gen = rt.claim_generation(None, LifecycleKind::Start).unwrap();
    rt.commit_coex_current_config(json!({"synchronousFixture":true}));
    let token = o.begin_watcher(gen).unwrap();
    o.watcher(gen, token, true);
    // The synchronous Tauri command delegates directly to this actual facade method.
    // Set only the injected fixture phase so no automatic request precedes refresh.
    o.lock().binding.phase = "ready";
    for expected_calls in [1, 2] {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let revision = rt.coex_state()["reportRevision"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        rt.coex_refresh(revision);
        let deadline = Instant::now() + Duration::from_secs(3);
        while o.lock().driver && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!o.lock().driver, "synchronous refresh left driver latched");
        assert!(o.lock().pending.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(rt.coex_state()["freshness"], "latest");
    }
    assert!(tokio::runtime::Handle::try_current().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn report_projection_and_outer_revision_are_one_read_transaction() {
    let (o, _, _) = fixture();
    let mut o = Arc::try_unwrap(o).ok().unwrap();
    let collector = o.collector.clone();
    let (captured_tx, captured_rx) = mpsc::channel();
    let latch = Arc::new((Mutex::new(false), Condvar::new()));
    let waiting = latch.clone();
    o.collector = Arc::new(move |a| {
        let wire = collector(a);
        captured_tx.send(()).unwrap();
        let mut released = waiting.0.lock().unwrap();
        while !*released {
            released = waiting.1.wait(released).unwrap();
        }
        wire
    });
    let o = Arc::new(o);
    bind(&o, 1);
    o.lock().running = true;
    let worker = o.clone();
    let task = std::thread::spawn(move || worker.run());
    captured_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let service = o.service.clone();
    let holder = std::thread::spawn(move || service.hold_slot_for_tests(held_tx, release_rx));
    held_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    release(&latch);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !o.service.finish_waiting_for_tests() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let finish_waiting = o.service.finish_waiting_for_tests();
    let (read_tx, read_rx) = mpsc::channel();
    let reader = o.clone();
    let read = std::thread::spawn(move || {
        read_tx.send(reader.get()).unwrap();
    });
    let early = read_rx.recv_timeout(Duration::from_millis(50)).ok();
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    task.join().unwrap();
    read.join().unwrap();
    let state = early
        .clone()
        .unwrap_or_else(|| read_rx.recv_timeout(Duration::from_secs(3)).unwrap());
    assert!(finish_waiting);
    assert!(
        early.is_none(),
        "get-state observed an uncommitted report before physical finish"
    );
    let report_revision: u64 = state["report"]["value"]["reportRevision"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let outer_revision: u64 = state["reportRevision"].as_str().unwrap().parse().unwrap();
    assert!(report_revision <= outer_revision);
    assert_eq!(state, o.get());
}
#[test]
fn unchanged_config_is_rebound_to_a_new_session_with_new_revision() {
    let (o, _, _) = fixture();
    let value = json!({"unchangedAppliedConfig":true});
    let mut current = Some(value.clone());
    o.claim(1, true);
    o.commit_config(&mut current, value.clone());
    let first = o.get();
    o.claim(2, true);
    o.commit_config(&mut current, value.clone());
    let second = o.get();
    assert_eq!(current, Some(value));
    assert_ne!(
        first["binding"]["configGeneration"],
        second["binding"]["configGeneration"]
    );
    assert_ne!(first["reportRevision"], second["reportRevision"]);
}

#[tokio::test]
async fn actual_debounced_restart_invalidates_before_blocked_failed_cleanup_and_old_worker() {
    use crate::runtime::{
        config::ConfigManager,
        helper::HelperRuntime,
        mesh::MeshRuntime,
        proxy::{NoNetworkDoh, ProxyRuntime},
    };
    use polaris_core_supervisor::LifecycleKind;
    let dir = std::env::temp_dir().join(format!("polaris-coex-restart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (o, _, _) = fixture();
    let mut o = Arc::try_unwrap(o).ok().unwrap();
    let collector = o.collector.clone();
    let block = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let should_block = block.clone();
    let latch = Arc::new((Mutex::new(false), Condvar::new()));
    let wait = latch.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    o.collector = Arc::new(move |a| {
        let wire = collector(a);
        if should_block.load(Ordering::SeqCst) {
            entered_tx.send(()).unwrap();
            let mut released = wait.0.lock().unwrap();
            while !*released {
                released = wait.1.wait(released).unwrap();
            }
        }
        wire
    });
    let o = Arc::new(o);
    let mut rt = ProxyRuntime::new(
        Arc::new(ConfigManager::new(dir.clone())),
        Arc::new(HelperRuntime::never_installed_for_tests(dir.clone())),
        Arc::new(MeshRuntime::new(dir.clone())),
        Box::new(NoSystemProxy),
        Arc::new(NoNetworkDoh),
    );
    rt.coex = o.clone();
    let rt = Arc::new(rt);
    let generation = rt.claim_generation(None, LifecycleKind::Start).unwrap();
    rt.commit_coex_current_config(json!({"actualRestartFixture":true}));
    let token = o.begin_watcher(generation).unwrap();
    o.watcher(generation, token, true);
    o.lock().binding.phase = "ready";
    run(&o);
    assert_eq!(rt.coex_state()["freshness"], "latest");
    block.store(true, Ordering::SeqCst);
    {
        let mut state = o.lock();
        state.running = true;
        state.driver = true;
    }
    let worker = o.clone();
    let old = std::thread::spawn(move || worker.run());
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap();
    rt.coex_refresh(
        rt.coex_state()["reportRevision"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    );
    assert_eq!(rt.coex_state()["pending"]["status"], "known");
    let gate = rt.mesh.tailscale_state_gate().await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    rt.debounced
        .schedule_with_ticket(true, move |outcome, ticket| {
            tx.send((outcome, ticket)).unwrap();
        });
    let (outcome, ticket) = rx.await.unwrap();
    assert!(matches!(
        outcome,
        polaris_switch_engine::DebouncedOutcome::Proceed(None)
    ));
    let (_, claimed, _lease) = rt
        .claim_debounced_restart(None, generation, ticket)
        .unwrap();
    let claim_state = rt.coex_state();
    let stopping = rt.clone();
    let cleanup = tokio::spawn(async move {
        stopping
            .stop_inner(super::super::lifecycle::StopClaim::AlreadyClaimed(claimed))
            .await
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while rt.gate.depth() < 2 && Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    let cleanup_entered = rt.gate.depth() >= 2;
    release(&latch);
    old.join().unwrap();
    let while_blocked = rt.coex_state();
    // Poison empty Child custody to force an actual cleanup error before any native StopIo,
    // route/DNS teardown or physical core operation can run after the held gate is released.
    let poisoned = rt.clone();
    assert!(std::thread::spawn(move || {
        let _slot = poisoned.child.lock().unwrap();
        panic!("fixture custody poison before native StopIo");
    })
    .join()
    .is_err());
    drop(gate);
    let result = cleanup.await.unwrap();
    rt.finish_lifecycle(LifecycleKind::Restart);
    assert!(cleanup_entered);
    assert!(result.unwrap_err().contains("custody poisoned before Stop"));
    for state in [claim_state, while_blocked, rt.coex_state()] {
        assert_eq!(state["freshness"], "unavailable");
        assert_eq!(state["report"]["status"], "unknown");
        assert_eq!(state["pending"]["status"], "unknown");
        assert_eq!(state["binding"]["sessionId"]["status"], "unknown");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

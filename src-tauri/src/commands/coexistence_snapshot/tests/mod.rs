//! Injected command ownership and synthetic ip JSON only. No native start call.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Condvar;

struct ReturnedOwner(Result<CommandOutput, String>);

impl CommandCustody for ReturnedOwner {
    fn poll_cleanup(&mut self, budget: Duration) -> CommandCleanup {
        assert_eq!(budget, CLEANUP_OBSERVATION);
        CommandCleanup::NativeAndReadersReturned
    }

    fn operation(&self) -> Option<Result<CommandOutput, String>> {
        Some(self.0.clone())
    }
}

#[derive(Clone)]
struct FixtureSource {
    outputs: Arc<Vec<Result<String, String>>>,
    calls: Arc<Mutex<Vec<(Command, Duration)>>>,
}

impl FixtureSource {
    fn new(outputs: Vec<Result<String, String>>) -> Self {
        assert_eq!(outputs.len(), 6);
        Self {
            outputs: Arc::new(outputs),
            calls: Arc::default(),
        }
    }

    fn empty() -> Self {
        Self::new(vec![Ok("[]".into()); 6])
    }

    fn with_links(routes4: Value, routes6: Value, rules4: Value, rules6: Value) -> Self {
        Self::new(vec![
            Ok(json!([{"ifname":"vpn0","linkinfo":{"info_kind":"tun"}}]).to_string()),
            Ok(json!([{"ifname":"vpn0","addr_info":[{"family":"inet","local":"10.8.0.2","prefixlen":24}]}]).to_string()),
            Ok(routes4.to_string()), Ok(routes6.to_string()),
            Ok(rules4.to_string()), Ok(rules6.to_string()),
        ])
    }
}

impl ObservationSource for FixtureSource {
    fn start(&self, command: &Command, timeout: Duration) -> StartedCommand {
        let mut calls = self.calls.lock().unwrap();
        let index = calls.len();
        calls.push((command.clone(), timeout));
        let output = self.outputs[index].clone().map(|stdout| CommandOutput {
            stdout,
            stderr: String::new(),
        });
        StartedCommand::Owned(Box::new(ReturnedOwner(output)))
    }
}

async fn snapshot(source: FixtureSource) -> Value {
    collect_request(Arc::default(), Platform::Linux, source)
        .await
        .data
        .unwrap()
}

fn assert_context_unknown(value: &Value) {
    for field in [
        "observationPhase",
        "ownInterfaces",
        "criteria",
        "repairHistory",
    ] {
        assert_eq!(value["context"][field]["status"], "unknown");
        assert!(value["context"][field].get("value").is_none());
    }
    assert_eq!(value["classification"]["status"], "unknown");
    assert!(value.get("conflicts").is_none());
}

#[tokio::test]
async fn actual_collector_entry_uses_six_fixed_readonly_queries_and_known_empty_is_not_classified()
{
    let source = FixtureSource::empty();
    let calls = Arc::clone(&source.calls);
    let value = snapshot(source).await;
    assert_eq!(value["objects"], json!({"status":"known","value":[]}));
    assert_eq!(value["observation"]["value"]["atomic"], false);
    assert_context_unknown(&value);
    let expected = [
        vec!["-j", "-d", "link", "show"],
        vec!["-j", "address", "show"],
        vec!["-j", "-d", "-N", "-4", "route", "show", "table", "all"],
        vec!["-j", "-d", "-N", "-6", "route", "show", "table", "all"],
        vec!["-j", "-N", "-4", "rule", "show"],
        vec!["-j", "-N", "-6", "rule", "show"],
    ];
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), expected.len());
    for ((actual, timeout), argv) in calls.iter().zip(expected) {
        assert_eq!(actual, &Command::new("ip", argv));
        assert_eq!(*timeout, Duration::from_secs(2));
    }
}

#[tokio::test]
async fn failures_invalid_json_unsupported_shape_and_limits_never_become_empty_success() {
    for bad in [
        Err("permission denied".into()),
        Err("command operation budget elapsed".into()),
        Err("ip not found".into()),
        Ok("{".into()),
        Ok("{}".into()),
        Ok("x".repeat(polaris_system_integration::coexistence::MAX_SOURCE_BYTES + 1)),
        Ok(json!(vec![
            json!({"ifname":"vpn0"});
            polaris_system_integration::coexistence::MAX_INTERFACE_ROWS
                + 1
        ])
        .to_string()),
    ] {
        let mut outputs = vec![Ok("[]".into()); 6];
        outputs[0] = bad;
        let value = snapshot(FixtureSource::new(outputs)).await;
        assert_eq!(value["objects"]["status"], "unknown");
        assert!(value["objects"].get("value").is_none());
        assert!(value["objects"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("ip link:"));
        assert_context_unknown(&value);
    }
}

#[tokio::test]
async fn fact_wire_retains_host_address_table_family_role_and_selector_unknowns() {
    let source = FixtureSource::with_links(
        json!([
            {"dst":"0.0.0.0/1","dev":"vpn0","table":254,"type":"1","flags":[],"scope":"0"},
            {"dst":"128.0.0.0/1","dev":"vpn0","table":100,"type":"1","flags":[],"scope":"0"}
        ]),
        json!([{"dst":"::/0","dev":"vpn0","table":100,"type":"1","flags":[],"scope":"0"}]),
        json!([{"priority":100,"src":"all","table":100,"fwmark":1}]),
        json!([{"priority":100,"src":"all","table":254}]),
    );
    let value = snapshot(source).await;
    assert_eq!(value["objects"]["status"], "known");
    let object = &value["objects"]["value"][0];
    assert_eq!(object["addresses"]["value"][0]["address"], "10.8.0.2");
    let routes = object["routes"]["value"].as_array().unwrap();
    assert_eq!(routes.len(), 3);
    assert_eq!(routes[0]["table"]["value"], 254);
    assert_eq!(routes[1]["table"]["value"], 100);
    for route in routes {
        assert_eq!(route["role"]["status"], "unknown");
        assert!(route["role"].get("value").is_none());
    }
    let rules = object["policyRules"]["value"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["addressFamily"]["value"], "ipv4");
    assert_eq!(rules[1]["addressFamily"]["value"], "ipv6");
    assert_eq!(rules[0]["selectorScope"]["value"]["kind"], "limited");
    for rule in rules {
        assert_ne!(
            rule["appliesToObject"],
            json!({"status":"known","value":true})
        );
    }
    assert_eq!(
        object["stableIdentity"],
        json!({"status":"known","value":null})
    );
    assert_context_unknown(&value);
}

#[tokio::test]
async fn unknown_route_source_keeps_independent_address_evidence() {
    let mut outputs =
        (*FixtureSource::with_links(json!([]), json!([]), json!([]), json!([])).outputs).clone();
    outputs[2] = Err("permission denied for IPv4 routes".into());
    let value = snapshot(FixtureSource::new(outputs)).await;
    let object = &value["objects"]["value"][0];
    assert_eq!(object["addresses"]["status"], "known");
    assert_eq!(object["routes"]["status"], "unknown");
    assert!(object["routes"]["reason"]
        .as_str()
        .unwrap()
        .contains("permission denied"));
    assert_context_unknown(&value);
}

#[tokio::test]
async fn unsupported_platforms_and_denied_windows_have_zero_source_calls() {
    for platform in [
        Platform::Mac,
        Platform::Win,
        Platform::Other,
        Platform::Android,
        Platform::Ios,
    ] {
        let source = FixtureSource::empty();
        let calls = Arc::clone(&source.calls);
        let response = request_for_window(Arc::default(), platform, source, "main").await;
        assert!(response.success);
        let value = response.data.unwrap();
        assert_eq!(value["objects"]["status"], "unknown");
        assert_context_unknown(&value);
        assert!(calls.lock().unwrap().is_empty());
    }
    for label in [
        "",
        "Main",
        "main ",
        "singbox-dashboard",
        "tray",
        "update-popup",
    ] {
        let source = FixtureSource::empty();
        let calls = Arc::clone(&source.calls);
        let response = request_for_window(Arc::default(), Platform::Linux, source, label).await;
        assert!(!response.success);
        assert_eq!(response.code.as_deref(), Some("coex_window_denied"));
        assert!(response.data.is_none());
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn allowed_main_window_returns_existing_api_envelope_with_explicit_unknown_context() {
    let source = FixtureSource::empty();
    let calls = Arc::clone(&source.calls);
    let response = request_for_window(Arc::default(), Platform::Linux, source, "main").await;
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire["success"], true);
    assert!(wire.get("error").is_none());
    assert_eq!(
        wire["data"]["objects"],
        json!({"status":"known","value":[]})
    );
    assert_context_unknown(&wire["data"]);
    assert_eq!(calls.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn contended_worker_lock_returns_unknown_without_waiting_or_starting_queries() {
    let service = Arc::new(SnapshotService::default());
    let worker_service = Arc::clone(&service);
    let (started, receiver) = tokio::sync::oneshot::channel();
    let (release, release_receiver) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _slot = worker_service.slot.lock().unwrap();
        started.send(()).unwrap();
        let _ = release_receiver.recv_timeout(Duration::from_secs(2));
    });
    receiver.await.unwrap();
    let source = FixtureSource::empty();
    let calls = Arc::clone(&source.calls);
    let response = collect_request(service, Platform::Linux, source).await;
    // Release even before assertions so a regression does not strand the fixture holder.
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(response.data.unwrap()["objects"]["reason"]
        .as_str()
        .unwrap()
        .contains("busy"));
    assert!(calls.lock().unwrap().is_empty());
}

struct RetainedOwner {
    closed: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    panic_on_poll: bool,
}

impl CommandCustody for RetainedOwner {
    fn poll_cleanup(&mut self, budget: Duration) -> CommandCleanup {
        assert_eq!(budget, CLEANUP_OBSERVATION);
        self.polls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_on_poll, "injected cleanup observation panic");
        if self.closed.load(Ordering::SeqCst) {
            CommandCleanup::NativeAndReadersReturned
        } else {
            CommandCleanup::Unknown
        }
    }

    fn operation(&self) -> Option<Result<CommandOutput, String>> {
        Some(Err(
            "injected operation timed out; no native cleanup proof".into()
        ))
    }
}

impl Drop for RetainedOwner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

struct RetainedSource {
    calls: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    panic_on_poll: bool,
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Option<Arc<(Mutex<bool>, Condvar)>>,
}

impl RetainedSource {
    fn new() -> Self {
        Self {
            calls: Arc::default(),
            closed: Arc::default(),
            polls: Arc::default(),
            drops: Arc::default(),
            panic_on_poll: false,
            started: Mutex::new(None),
            release: None,
        }
    }
}

impl ObservationSource for RetainedSource {
    fn start(&self, _command: &Command, timeout: Duration) -> StartedCommand {
        assert_eq!(timeout, Duration::from_secs(2));
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        if let Some(sender) = self.started.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        if let Some(release) = &self.release {
            let (lock, ready) = &**release;
            let mut value = lock.lock().unwrap();
            while !*value {
                let (next, timeout) = ready.wait_timeout(value, Duration::from_secs(2)).unwrap();
                value = next;
                if timeout.timed_out() {
                    break;
                }
            }
        }
        StartedCommand::Owned(Box::new(RetainedOwner {
            closed: Arc::clone(&self.closed),
            polls: Arc::clone(&self.polls),
            drops: Arc::clone(&self.drops),
            panic_on_poll: self.panic_on_poll,
        }))
    }
}

#[tokio::test]
async fn retained_original_owner_blocks_remaining_queries_and_reentry_until_observed_closed() {
    let service = Arc::new(SnapshotService::default());
    let source = RetainedSource::new();
    let calls = Arc::clone(&source.calls);
    let closed = Arc::clone(&source.closed);
    let polls = Arc::clone(&source.polls);
    let drops = Arc::clone(&source.drops);
    let first = collect_request(Arc::clone(&service), Platform::Linux, source)
        .await
        .data
        .unwrap();
    assert_eq!(first["objects"]["status"], "unknown");
    assert_eq!(first["commandCleanup"]["status"], "unknown");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let fresh = FixtureSource::empty();
    let fresh_calls = Arc::clone(&fresh.calls);
    let second = collect_request(Arc::clone(&service), Platform::Linux, fresh.clone())
        .await
        .data
        .unwrap();
    assert_eq!(second["objects"]["status"], "unknown");
    assert!(fresh_calls.lock().unwrap().is_empty());
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    closed.store(true, Ordering::SeqCst);
    let third = collect_request(service, Platform::Linux, fresh)
        .await
        .data
        .unwrap();
    assert_eq!(third["objects"]["status"], "known");
    assert_eq!(fresh_calls.lock().unwrap().len(), 6);
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancel_ipc_waiter_keeps_worker_admission_then_retains_original_owner() {
    let service = Arc::new(SnapshotService::default());
    let mut source = RetainedSource::new();
    let (started, receiver) = tokio::sync::oneshot::channel();
    *source.started.lock().unwrap() = Some(started);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    source.release = Some(Arc::clone(&release));
    let drops = Arc::clone(&source.drops);
    let source_calls = Arc::clone(&source.calls);
    let worker_service = Arc::clone(&service);
    let waiter =
        tokio::spawn(async move { collect_request(worker_service, Platform::Linux, source).await });
    tokio::time::timeout(Duration::from_secs(2), receiver)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let fresh = FixtureSource::empty();
    let calls = Arc::clone(&fresh.calls);
    let busy = collect_request(Arc::clone(&service), Platform::Linux, fresh.clone())
        .await
        .data
        .unwrap();
    assert!(busy["objects"]["reason"].as_str().unwrap().contains("busy"));
    assert!(calls.lock().unwrap().is_empty());
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !service.lock_worker().busy {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(service.lock_worker().pending.is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let retained = collect_request(Arc::clone(&service), Platform::Linux, fresh)
        .await
        .data
        .unwrap();
    assert_eq!(retained["commandCleanup"]["status"], "unknown");
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(source_calls.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn worker_panic_preserves_stored_owner_and_quarantines_instead_of_clearing_busy_only() {
    let service = Arc::new(SnapshotService::default());
    let mut source = RetainedSource::new();
    source.panic_on_poll = true;
    let drops = Arc::clone(&source.drops);
    let response = collect_request(Arc::clone(&service), Platform::Linux, source)
        .await
        .data
        .unwrap();
    assert!(response["objects"]["reason"]
        .as_str()
        .unwrap()
        .contains("join failed"));
    assert!(service.lock_worker().pending.is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let fresh = FixtureSource::empty();
    let calls = Arc::clone(&fresh.calls);
    let blocked = collect_request(Arc::clone(&service), Platform::Linux, fresh)
        .await
        .data
        .unwrap();
    assert_eq!(blocked["objects"]["status"], "unknown");
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}

struct SpawnFailure;
impl ObservationSource for SpawnFailure {
    fn start(&self, _command: &Command, _timeout: Duration) -> StartedCommand {
        StartedCommand::NotSpawned("injected executable unavailable".into())
    }
}

#[tokio::test]
async fn spawn_failure_is_unknown_without_fabricating_objects_or_holding_unspawned_owner() {
    let service = Arc::new(SnapshotService::default());
    let value = collect_request(Arc::clone(&service), Platform::Linux, SpawnFailure)
        .await
        .data
        .unwrap();
    assert_eq!(value["objects"]["status"], "unknown");
    assert!(service.lock_worker().pending.is_none());
    let fresh = collect_request(service, Platform::Linux, FixtureSource::empty())
        .await
        .data
        .unwrap();
    assert_eq!(fresh["objects"]["status"], "known");
    assert_context_unknown(&fresh);
}

struct InvalidOwnership;
impl ObservationSource for InvalidOwnership {
    fn start(&self, _command: &Command, _timeout: Duration) -> StartedCommand {
        StartedCommand::OwnershipUnavailable("injected inconsistent ownership result".into())
    }
}

#[tokio::test]
async fn unavailable_ownership_and_poisoned_admission_fail_closed() {
    let service = Arc::new(SnapshotService::default());
    let value = collect_request(Arc::clone(&service), Platform::Linux, InvalidOwnership)
        .await
        .data
        .unwrap();
    assert_eq!(value["objects"]["status"], "unknown");
    let fresh = FixtureSource::empty();
    let calls = Arc::clone(&fresh.calls);
    collect_request(service, Platform::Linux, fresh).await;
    assert!(calls.lock().unwrap().is_empty());
    let poisoned = Arc::new(SnapshotService::default());
    let poison_ref = Arc::clone(&poisoned);
    assert!(std::panic::catch_unwind(move || {
        let _lock = poison_ref.slot.lock().unwrap();
        panic!("injected state poison");
    })
    .is_err());
    let fresh = FixtureSource::empty();
    let calls = Arc::clone(&fresh.calls);
    let value = collect_request(poisoned, Platform::Linux, fresh)
        .await
        .data
        .unwrap();
    assert!(value["objects"]["reason"]
        .as_str()
        .unwrap()
        .contains("poisoned"));
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn registration_and_native_observed_custody_path_are_anchored_to_actual_entry() {
    use crate::commands::guard_scan::top_level_fn_body;
    let source = crate::test_support::crate_source("commands/coexistence_snapshot.rs");
    let command = top_level_fn_body(&source, "pub async fn coex_readonly_snapshot(");
    assert!(command.contains("request_for_window("));
    assert!(command.contains("window.label()"));
    assert!(command.contains("NativeSource"));
    assert!(command.contains("static SERVICE: OnceLock<Arc<SnapshotService>>"));
    assert!(command.contains("SERVICE.get_or_init"));
    let request = top_level_fn_body(&source, "async fn request_for_window<S:");
    assert!(
        request.find("!allowed_window(label)").unwrap() < request.find("collect_request(").unwrap()
    );
    let collect = top_level_fn_body(&source, "async fn collect_request<S:");
    assert!(collect
        .contains("tokio::task::spawn_blocking(move || collect_blocking(admission, source))"));
    let worker = top_level_fn_body(&source, "fn collect_blocking<S:");
    assert!(worker.contains("collect_linux(&SnapshotRunner"));
    assert!(worker.find("snapshot_wire(").unwrap() < worker.find("admission.finish()").unwrap());
    assert!(source.contains("StdCommandRunner.run_observed(command, timeout)"));
    assert!(source.contains("observed.into_pending()"));
    let modules = crate::test_support::crate_source("commands.rs");
    assert_eq!(modules.matches("pub mod coexistence_snapshot;").count(), 1);
    assert_eq!(
        modules
            .matches("pub use coexistence_snapshot::coex_readonly_snapshot;")
            .count(),
        1
    );
    let lib = crate::test_support::crate_source("lib.rs");
    let handler = lib
        .split_once(".invoke_handler(tauri::generate_handler![")
        .unwrap()
        .1
        .split_once("])")
        .unwrap()
        .0;
    assert_eq!(
        handler
            .lines()
            .filter(|line| line.trim() == "coex_readonly_snapshot,")
            .count(),
        1
    );
}

//! Pure loopback diagnostics fixtures: no sing-box, TUN or host settings.
use super::*;

fn stage(
    name: &str,
    origin: Instant,
    started: Instant,
    error: Option<std::io::ErrorKind>,
) -> Value {
    json!({"stage":name,"startedMicros":started.duration_since(origin).as_micros(),
        "completedMicros":origin.elapsed().as_micros(),
        "errorKind":error.map(|kind|format!("{kind:?}"))})
}

pub(super) fn probe_stages(port: u16, origin: Instant) -> (Result<(), String>, Value) {
    let started = Instant::now();
    let mut stages = Vec::with_capacity(4);
    let mut socket = None;
    let mut timeout_error = None;
    let result = (|| {
        let connected = connect_tcp_timeout(&local(port), Duration::from_millis(600));
        stages.push(stage(
            "connect",
            origin,
            started,
            connected.as_ref().err().map(|e| e.kind()),
        ));
        let mut stream = connected.map_err(|error| format!("connect: {error}"))?;
        socket = Some(json!({"local":stream.local_addr().ok(),"peer":stream.peer_addr().ok()}));
        timeout_error = stream
            .set_read_timeout(Some(Duration::from_millis(600)))
            .err();
        let write_started = Instant::now();
        let written = stream.write_all(&[0x59]);
        stages.push(stage(
            "write",
            origin,
            write_started,
            written.as_ref().err().map(|e| e.kind()),
        ));
        written.map_err(|error| format!("write: {error}"))?;
        let read_started = Instant::now();
        let mut echoed = [0u8; 1];
        let read = stream.read_exact(&mut echoed);
        stages.push(stage(
            "read",
            origin,
            read_started,
            read.as_ref().err().map(|e| e.kind()),
        ));
        read.map_err(|error| format!("read: {error}; set_read_timeout_error={timeout_error:?}"))?;
        let validation_started = Instant::now();
        let matches = echoed == [0x59];
        stages.push(json!({"stage":"echo-check", "startedMicros":validation_started.duration_since(origin).as_micros(),
            "completedMicros":origin.elapsed().as_micros(),"matches":matches}));
        if matches {
            Ok(())
        } else {
            Err("echo mismatch: response did not match".to_owned())
        }
    })();
    let result = result.map_err(|error| {
        format!(
            "relay={} elapsed={:?} {error}",
            local(port),
            started.elapsed()
        )
    });
    let observed = json!({"startedMicros":started.duration_since(origin).as_micros(),
        "completedMicros":origin.elapsed().as_micros(),"socket":socket,"stages":stages,
        "readTimeoutSetupErrorKind":timeout_error.map(|e|format!("{:?}",e.kind())),
        "scope":"monotonic fixture socket stages only; no payload/key bytes or causal verdict"});
    (result, observed)
}

fn log_windows(logs: &[(&str, &Path)], origin: Instant) -> Value {
    Value::Object(
        logs.iter()
            .map(|(role, path)| {
                let observed = std::fs::metadata(path);
                (
                    (*role).to_owned(),
                    json!({"observedMicros":origin.elapsed().as_micros(),
            "fileBytes":observed.as_ref().ok().map(std::fs::Metadata::len),
            "errorKind":observed.err().map(|e|format!("{:?}",e.kind()))}),
                )
            })
            .collect(),
    )
}

pub(super) fn listener_snapshots(listeners: &[(&str, &Listener)]) -> Value {
    Value::Object(
        listeners
            .iter()
            .map(|(name, listener)| ((*name).into(), listener.snapshot()))
            .collect(),
    )
}

fn deltas(before: &Value, after: &Value) -> Value {
    Value::Object(
        after
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, row)| {
                let mut counters = serde_json::Map::new();
                for key in ["accepted", "replies", "errorSequence"] {
                    let delta = row[key]
                        .as_u64()
                        .zip(before[name][key].as_u64())
                        .and_then(|(a, b)| a.checked_sub(b));
                    counters.insert(key.into(), json!(delta));
                }
                (name.clone(), Value::Object(counters))
            })
            .collect(),
    )
}

pub(super) fn observe_forward_tcp(
    port: u16,
    listeners: &[(&str, &Listener)],
    policy_baseline: &Value,
) -> (Result<(), String>, Value) {
    observe_forward_tcp_with_logs(port, listeners, policy_baseline, &[])
}

pub(super) fn observe_forward_tcp_with_logs(
    port: u16,
    listeners: &[(&str, &Listener)],
    policy_baseline: &Value,
    logs: &[(&str, &Path)],
) -> (Result<(), String>, Value) {
    let origin = Instant::now();
    let mut attempts = Vec::new();
    let mut count = 0usize;
    let mut last_error = "no probe attempt completed".to_string();
    let passed = eventually(|| {
        count += 1;
        let before = listener_snapshots(listeners);
        let logs_before = log_windows(logs, origin);
        let (result, probe_window) = probe_stages(port, origin); // Original 600ms probe /10s window.
        let after = listener_snapshots(listeners);
        let logs_after = log_windows(logs, origin);
        let record = json!({"attempt":count,"before":before,"after":after,
            "delta":deltas(&before,&after),"probeResult":result.as_ref().err(),
            "probeStages":probe_window,"coreLogBefore":logs_before,"coreLogAfter":logs_after});
        // Bound observation storage, never the actual probe/retry count.
        if attempts.len() < 100 {
            attempts.push(record);
        } else {
            *attempts.last_mut().unwrap() = record;
        }
        match result {
            Ok(()) => true,
            Err(error) => {
                last_error = error;
                false
            }
        }
    });
    let pre_signal = listener_snapshots(listeners);
    let observation = json!({"policyBaseline":policy_baseline,"preSIGQUIT":pre_signal,
        "policyDelta":deltas(policy_baseline,&pre_signal),"attemptCount":count,
        "omittedAttempts":count.saturating_sub(attempts.len()),"attempts":attempts,
        "scope":"policy/probe monotonic sampling windows and core log byte offsets before SIGQUIT, not exact engine event times; no payload/key capture; EOF may follow client timeout and is not attributed as original root cause"});
    (if passed { Ok(()) } else { Err(last_error) }, observation)
}

#[test]
fn probe_stages_and_log_offsets_keep_metadata_without_content() {
    let listener = Listener::tcp_on(false);
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("owned.stderr");
    let private_content = b"controlled-log-content-not-for-export";
    std::fs::write(&log, private_content).unwrap();
    let listeners = [("b_tcp", &listener)];
    let baseline = listener_snapshots(&listeners);
    let (result, observation) =
        observe_forward_tcp_with_logs(listener.port, &listeners, &baseline, &[("a", &log)]);
    assert!(result.is_ok());
    let attempt = &observation["attempts"][0];
    let probe = &attempt["probeStages"];
    let stages = probe["stages"].as_array().unwrap();
    assert_eq!(
        stages
            .iter()
            .map(|row| row["stage"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["connect", "write", "read", "echo-check"]
    );
    let mut previous = probe["startedMicros"].as_u64().unwrap();
    for row in stages {
        let start = row["startedMicros"].as_u64().unwrap();
        let end = row["completedMicros"].as_u64().unwrap();
        assert!(previous <= start && start <= end);
        previous = end;
    }
    assert!(previous <= probe["completedMicros"].as_u64().unwrap());
    assert_eq!(
        attempt["coreLogBefore"]["a"]["fileBytes"],
        private_content.len()
    );
    assert_eq!(
        attempt["coreLogAfter"]["a"]["fileBytes"],
        private_content.len()
    );
    assert!(!observation
        .to_string()
        .contains(std::str::from_utf8(private_content).unwrap()));
    assert!(
        log_windows(&[("missing", &temp.path().join("missing"))], Instant::now())["missing"]
            ["fileBytes"]
            .is_null()
    );
}

#[test]
fn policy_and_attempt_receipts_do_not_attribute_old_or_later_errors_to_current_probe() {
    let listener = Listener::tcp_on(false);
    listener
        .diagnostics
        .record("prior policy", std::io::Error::other("old error"));
    let listeners = [("b_tcp", &listener)];
    let baseline = listener_snapshots(&listeners);
    let (result, frozen) = observe_forward_tcp(listener.port, &listeners, &baseline);
    assert!(result.is_ok());
    assert_eq!(frozen["attemptCount"], 1);
    assert_eq!(frozen["policyDelta"]["b_tcp"]["accepted"], 1);
    assert_eq!(frozen["policyDelta"]["b_tcp"]["errorSequence"], 0);
    assert!(frozen["preSIGQUIT"]["b_tcp"]["last_error"]
        .as_str()
        .unwrap()
        .contains("old error"));
    let original = frozen.clone();
    listener
        .diagnostics
        .record("after signal", std::io::Error::other("later EOF"));
    assert_eq!(frozen, original);
    assert!(!frozen.to_string().contains("later EOF"));
    assert_eq!(
        deltas(&baseline, &listener_snapshots(&listeners))["b_tcp"]["errorSequence"],
        1
    );
}

#[test]
fn healthy_listener_echo_and_worker_receipts_are_observable() {
    let listener = Listener::tcp_on(false);
    tcp_probe(listener.port).unwrap_or_else(|error| {
        panic!(
            "healthy TCP probe failed: {error}; {}",
            listener.diagnostic()
        )
    });
    assert!(eventually(|| listener
        .diagnostics
        .replies
        .load(Ordering::Relaxed)
        == 1));
    assert_eq!(listener.count(), 1);
    assert!(listener.diagnostics.last_error.lock().unwrap().is_none());
    assert!(listener.diagnostic().contains("worker_finished=false"));
}

#[test]
fn accepted_connection_waits_for_payload_sent_after_accept_receipt() {
    let listener = Listener::tcp_on(false);
    let mut stream = connect_tcp(local(listener.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    // No payload until the worker has accepted this connection: no sleep or probe retry.
    assert!(
        eventually(|| listener.count() == 1),
        "accept receipt missing: {}",
        listener.diagnostic()
    );
    let before_send = listener.diagnostics.last_error.lock().unwrap().clone();
    assert!(
        before_send.is_none(),
        "accepted connection failed before payload: {}",
        listener.diagnostic()
    );
    assert_eq!(listener.diagnostics.replies.load(Ordering::Relaxed), 0);
    stream.write_all(&[0x59]).unwrap_or_else(|error| {
        panic!(
            "delayed payload write failed: {error}; {}",
            listener.diagnostic()
        )
    });
    let mut echoed = [0];
    stream.read_exact(&mut echoed).unwrap_or_else(|error| {
        panic!(
            "delayed payload read failed: {error}; {}",
            listener.diagnostic()
        )
    });
    assert_eq!(echoed, [0x59]);
    assert!(
        eventually(|| listener.diagnostics.replies.load(Ordering::Relaxed) == 1),
        "echo receipt missing: {}",
        listener.diagnostic()
    );
    assert_eq!(listener.count(), 1);
    let after_echo = listener.diagnostics.last_error.lock().unwrap().clone();
    assert!(after_echo.is_none(), "{}", listener.diagnostic());
}

fn response_fixture(response: Option<u8>) -> (u16, Arc<AtomicBool>, JoinHandle<()>) {
    let socket = bind_tcp("127.0.0.1:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    let port = socket.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let worker = thread::spawn(move || {
        while !stopping.load(Ordering::Relaxed) {
            let (mut stream, _) = match socket.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            };
            // Winsock inherits the listener's nonblocking mode; only accept polls.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [0x59]);
            if let Some(response) = response {
                stream.write_all(&[response]).unwrap();
            }
            break;
        }
    });
    (port, stop, worker)
}

#[test]
fn idle_response_fixture_can_be_cancelled_without_a_connection() {
    let (_port, stop, worker) = response_fixture(None);
    stop.store(true, Ordering::Relaxed);
    assert!(eventually(|| worker.is_finished()));
    worker.join().unwrap();
}

#[test]
fn early_eof_and_wrong_echo_keep_the_probe_failure_stage() {
    for response in [None, Some(0x58)] {
        let (port, stop, worker) = response_fixture(response);
        let (result, observation) = probe_stages(port, Instant::now());
        // Even a connect failure or unexpected success must release the idle worker.
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        let failure = result.unwrap_err();
        assert!(failure.contains("relay=127.0.0.1:") && failure.contains("elapsed="));
        assert!(failure.contains(if response.is_none() {
            "read:"
        } else {
            "echo mismatch:"
        }));
        let stages = observation["stages"].as_array().unwrap();
        if response.is_none() {
            assert_eq!(stages.last().unwrap()["stage"], "read");
            assert!(!stages.last().unwrap()["errorKind"].is_null());
        } else {
            assert_eq!(stages.last().unwrap()["stage"], "echo-check");
            assert_eq!(stages.last().unwrap()["matches"], false);
            assert!(!failure.contains("0x58") && !failure.contains("[88]"));
        }
    }
}

#[test]
fn listener_read_failure_and_finished_worker_are_visible() {
    let listener = Listener::tcp_on(false);
    let stream = connect_tcp(local(listener.port)).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(eventually(|| listener
        .diagnostics
        .last_error
        .lock()
        .unwrap()
        .is_some()));
    assert!(listener.diagnostic().contains("tcp read:"));
    assert_eq!(listener.diagnostics.replies.load(Ordering::Relaxed), 0);
    listener.stop.store(true, Ordering::Relaxed);
    assert!(eventually(|| listener
        .worker
        .as_ref()
        .unwrap()
        .is_finished()));
    assert!(listener.diagnostic().contains("worker_finished=true"));
}

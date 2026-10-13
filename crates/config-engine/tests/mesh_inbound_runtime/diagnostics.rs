//! Pure loopback diagnostics fixtures: no sing-box, TUN or host settings.
use super::*;

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
    let mut attempts = Vec::new();
    let mut count = 0usize;
    let mut last_error = "no probe attempt completed".to_string();
    let passed = eventually(|| {
        count += 1;
        let before = listener_snapshots(listeners);
        let result = tcp_probe(port); // Same original 600ms probe and 10s retry window.
        let after = listener_snapshots(listeners);
        let record = json!({"attempt":count,"before":before,"after":after,
            "delta":deltas(&before,&after),"probeResult":result.as_ref().err()});
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
        "scope":"policy and probe attempt observation windows frozen before SIGQUIT; EOF may follow client timeout and is not attributed as original root cause"});
    (if passed { Ok(()) } else { Err(last_error) }, observation)
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
    let mut stream = TcpStream::connect(local(listener.port)).unwrap();
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
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
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
        let result = tcp_probe(port);
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
    }
}

#[test]
fn listener_read_failure_and_finished_worker_are_visible() {
    let listener = Listener::tcp_on(false);
    let stream = TcpStream::connect(local(listener.port)).unwrap();
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

//! Pure loopback diagnostics fixtures: no sing-box, TUN or host settings.
use super::*;

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

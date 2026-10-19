use super::*;

struct BudgetConnector {
    observed: Arc<Mutex<Option<Instant>>>,
    writes: Arc<Mutex<usize>>,
    failure: Option<io::ErrorKind>,
}
impl Connector for BudgetConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        panic!("client bypassed connect_until")
    }
    fn connect_until(&self, deadline: Instant) -> Result<Box<dyn ConnectionStream>, ClientError> {
        *self.observed.lock().unwrap() = Some(deadline);
        Ok(Box::new(BudgetStream {
            deadline,
            writes: self.writes.clone(),
            failure: self.failure,
        }))
    }
}
struct BudgetStream {
    deadline: Instant,
    writes: Arc<Mutex<usize>>,
    failure: Option<io::ErrorKind>,
}
impl ConnectionStream for BudgetStream {
    fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
        panic!("unbudgeted write")
    }
    fn write_all_until(&mut self, _: &[u8], deadline: Instant) -> io::Result<()> {
        assert_eq!(deadline, self.deadline);
        *self.writes.lock().unwrap() += 1;
        match self.failure {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
    fn shutdown(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn set_read_timeout(&mut self, remaining: Duration) -> io::Result<()> {
        assert!(
            remaining
                <= self.deadline.saturating_duration_since(Instant::now())
                    + Duration::from_millis(10)
        );
        Ok(())
    }
    fn read_until_timeout(&mut self, buffer: &mut Vec<u8>) -> io::Result<usize> {
        buffer.extend_from_slice(b"OK pong uid=0 v9\n");
        Ok(buffer.len())
    }
}

#[test]
fn one_absolute_budget_reaches_connect_and_write() {
    let observed = Arc::new(Mutex::new(None));
    let writes = Arc::new(Mutex::new(0));
    let client = HelperClient::new(
        Box::new(BudgetConnector {
            observed: observed.clone(),
            writes: writes.clone(),
            failure: None,
        }),
        Platform::Mac,
        "TOK",
    );
    let before = Instant::now();
    client
        .send_with_timeout(&Request::Ping, Duration::from_secs(1))
        .unwrap();
    assert!((before..=Instant::now()).contains(
        &(*observed.lock().unwrap())
            .unwrap()
            .checked_sub(Duration::from_secs(1))
            .unwrap()
    ));
    assert_eq!(*writes.lock().unwrap(), 1);
}

#[test]
fn backpressured_or_partial_write_is_unknown_and_never_retried() {
    for failure in [
        io::ErrorKind::TimedOut,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::WriteZero,
    ] {
        let writes = Arc::new(Mutex::new(0));
        let client = HelperClient::new(
            Box::new(BudgetConnector {
                observed: Arc::new(Mutex::new(None)),
                writes: writes.clone(),
                failure: Some(failure),
            }),
            Platform::Mac,
            "TOK",
        );
        assert!(client
            .send_with_retry(
                &Request::Stop { pid: None },
                Duration::from_secs(1),
                3,
                Duration::ZERO
            )
            .is_err());
        assert_eq!(*writes.lock().unwrap(), 1);
    }
}

#[test]
fn incomplete_and_oversized_responses_are_rejected_without_replay() {
    for response in [
        b"OK stopped 123".to_vec(),
        vec![b'x'; codec::MAX_FRAME_BYTES + 1],
        vec![],
    ] {
        let conn = SequenceConnector {
            attempts: Arc::new(Mutex::new(vec![
                ConnAttempt::Ok(MockStream::with_response(response)),
                ConnAttempt::Ok(MockStream::with_response(b"OK stopped 123\n".to_vec())),
            ])),
        };
        let attempts = conn.attempts.clone();
        let client = HelperClient::new(Box::new(conn), Platform::Mac, "TOK");
        assert!(client
            .send_with_retry(
                &Request::Stop { pid: None },
                Duration::from_secs(5),
                1,
                Duration::ZERO
            )
            .is_err());
        assert_eq!(attempts.lock().unwrap().len(), 1);
    }
}

#[test]
fn expired_budget_does_not_connect() {
    struct NeverConnect;
    impl Connector for NeverConnect {
        fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
            panic!("expired request connected")
        }
    }
    let client = HelperClient::new(Box::new(NeverConnect), Platform::Mac, "TOK");
    assert!(matches!(
        client.send_with_timeout(&Request::Ping, Duration::ZERO),
        Err(ClientError::Timeout)
    ));
}

#[test]
fn busy_connect_timeout_cannot_write_or_retry() {
    struct Busy;
    impl Connector for Busy {
        fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
            panic!("unbudgeted connect")
        }
        fn connect_until(&self, _: Instant) -> Result<Box<dyn ConnectionStream>, ClientError> {
            Err(ClientError::Timeout)
        }
    }
    let client = HelperClient::new(Box::new(Busy), Platform::Mac, "TOK");
    assert!(matches!(
        client.send(&Request::Ping),
        Err(ClientError::Timeout)
    ));
}

struct BytePipeConnector(Arc<Mutex<Vec<Vec<u8>>>>);
impl Connector for BytePipeConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        Ok(Box::new(BytePipe(self.0.clone())))
    }
}
struct BytePipe(Arc<Mutex<Vec<Vec<u8>>>>);
impl ConnectionStream for BytePipe {
    fn supports_request_boundaries(&self) -> bool {
        false
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
    fn shutdown(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn read_until_timeout(&mut self, buffer: &mut Vec<u8>) -> io::Result<usize> {
        buffer.extend_from_slice(b"OK pong uid=0 v2\n");
        Ok(buffer.len())
    }
}

#[test]
fn old_byte_pipe_rejects_runtime_before_writing_but_keeps_upgrade_admission_reachable() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let client = HelperClient::new(
        Box::new(BytePipeConnector(writes.clone())),
        Platform::Win,
        "TOK",
    );
    for request in [
        Request::Stop { pid: None },
        Request::NativeStatusBirth,
        Request::Cleanup,
    ] {
        let error = client.send(&request).unwrap_err();
        assert!(
            matches!(error, ClientError::Io(ref error) if error.kind() == io::ErrorKind::Unsupported)
        );
    }
    assert!(client
        .probe_framed_transport(Duration::from_secs(1))
        .is_err());
    assert!(writes.lock().unwrap().is_empty());
    for request in [
        Request::Ping,
        Request::Version,
        Request::PrepareHelperUpgrade,
        Request::CancelHelperUpgrade,
    ] {
        client.send(&request).unwrap();
    }
    let observed = writes.lock().unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(observed[2], b"TOK\nprepare-helper-upgrade\n");
    assert_eq!(observed[3], b"TOK\ncancel-helper-upgrade\n");
}

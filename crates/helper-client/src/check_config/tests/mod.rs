//! Controlled wire responses only; these fixtures do not attest a Root child.
use super::*;
use crate::{ClientError, ConnectionStream, Connector, MockStream};
use polaris_helper_proto::HelperBirthToken;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct Queue(Arc<Mutex<VecDeque<MockStream>>>);
impl Connector for Queue {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .map(|s| Box::new(s) as Box<dyn ConnectionStream>)
            .ok_or_else(|| ClientError::Connect("controlled queue exhausted".into()))
    }
}
fn key() -> CheckKey {
    CheckKey {
        core: CoreId::parse(&"a".repeat(64)).unwrap(),
        correlation: HelperBirthToken::from_bytes([3; 16]),
    }
}
fn client(replies: Vec<CheckReply>) -> (HelperClient, Arc<Mutex<VecDeque<MockStream>>>) {
    let q = Arc::new(Mutex::new(
        replies
            .into_iter()
            .map(|r| MockStream::with_response((r.to_wire() + "\n").into_bytes()))
            .collect(),
    ));
    (
        HelperClient::new(Box::new(Queue(Arc::clone(&q))), Platform::Mac, "token"),
        q,
    )
}
#[test]
fn unknown_is_never_released_and_same_key_cancel_has_no_exec_fallback() {
    let (c, q) = client(vec![
        CheckReply::Begun { key: key() },
        CheckReply::Appended {
            key: key(),
            received: 2,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Running,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Unknown,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Unknown,
            stdout: 0,
            stderr: 0,
        },
    ]);
    assert!(c
        .run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false))
        .unwrap_err()
        .to_string()
        .contains("Unknown"));
    assert!(q.lock().unwrap().is_empty());
}
#[test]
fn terminal_output_is_read_before_release_and_keeps_both_streams() {
    let (c, q) = client(vec![
        CheckReply::Begun { key: key() },
        CheckReply::Appended {
            key: key(),
            received: 2,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Running,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Rejected,
            stdout: 3,
            stderr: 4,
        },
        CheckReply::Output {
            key: key(),
            stream: OutputStream::Stdout,
            offset: 0,
            bytes: b"out".to_vec(),
        },
        CheckReply::Output {
            key: key(),
            stream: OutputStream::Stderr,
            offset: 0,
            bytes: b"err!".to_vec(),
        },
        CheckReply::Released { key: key() },
    ]);
    let out = c
        .run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false))
        .unwrap();
    assert_eq!(out.status, CheckStatus::Rejected);
    assert_eq!(out.stdout, b"out");
    assert_eq!(out.stderr, b"err!");
    assert!(q.lock().unwrap().is_empty());
}
#[test]
fn different_core_or_correlation_response_is_not_custody() {
    let mut wrong = key();
    wrong.correlation = HelperBirthToken::from_bytes([9; 16]);
    let (c, _) = client(vec![CheckReply::Released { key: wrong }]);
    assert!(c
        .send_macos_check(CheckRequest::Release { key: key() }, Duration::from_secs(1))
        .is_err());
    let (c, q) = client(Vec::new());
    assert!(c
        .run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(true))
        .is_err());
    assert!(q.lock().unwrap().is_empty());
}
#[test]
fn unsupported_platforms_do_not_send_a_check_frame() {
    for platform in [
        Platform::Linux,
        Platform::Android,
        Platform::Ios,
        Platform::Other,
    ] {
        let q = Arc::new(Mutex::new(VecDeque::from([MockStream::with_response(
            b"OK\n".to_vec(),
        )])));
        let c = HelperClient::new(Box::new(Queue(Arc::clone(&q))), platform, "token");
        assert!(c
            .send_macos_check(CheckRequest::Cancel { key: key() }, Duration::from_secs(1))
            .unwrap_err()
            .contains("Unsupported"));
        assert_eq!(
            q.lock().unwrap().len(),
            1,
            "unsupported platform must not connect: {platform:?}"
        );
    }
}

#[test]
fn old_helper_refusal_before_begin_is_unsupported_without_inventing_cleanup() {
    let q = Arc::new(Mutex::new(VecDeque::from([MockStream::with_response(
        b"ERR unknown\n".to_vec(),
    )])));
    let c = HelperClient::new(Box::new(Queue(Arc::clone(&q))), Platform::Mac, "token");
    assert!(matches!(
        c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)),
        Err(HelperCheckFailure::NotAdmitted(_))
    ));
    assert!(
        q.lock().unwrap().is_empty(),
        "unknown Begin was not admitted; no invented Release or ordinary exec"
    );
}

#[test]
fn begin_authentication_denial_does_not_invent_an_admitted_request() {
    let q = Arc::new(Mutex::new(VecDeque::from([MockStream::with_response(
        b"ERR auth\n".to_vec(),
    )])));
    let c = HelperClient::new(Box::new(Queue(Arc::clone(&q))), Platform::Mac, "token");
    assert!(matches!(
        c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)),
        Err(HelperCheckFailure::NotAdmitted(_))
    ));
    assert!(
        q.lock().unwrap().is_empty(),
        "auth precedes Begin; no cleanup authority is fabricated"
    );
}

#[test]
fn pre_write_endpoint_refusal_is_not_admission_but_lost_reply_stays_unknown() {
    struct Refused;
    impl Connector for Refused {
        fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
            Err(ClientError::Connect(
                "pre-write path/OS peer refused".into(),
            ))
        }
    }
    let c = HelperClient::new(Box::new(Refused), Platform::Mac, "token");
    assert!(matches!(
        c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)),
        Err(HelperCheckFailure::NotAdmitted(_))
    ));
    let q = Arc::new(Mutex::new(VecDeque::from([MockStream::with_response(
        Vec::new(),
    )])));
    let c = HelperClient::new(Box::new(Queue(Arc::clone(&q))), Platform::Mac, "token");
    assert!(matches!(
        c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)),
        Err(HelperCheckFailure::Unconfirmed(_))
    ));
    assert!(q.lock().unwrap().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn system_client_rejects_frame_injection_without_connecting() {
    for bad in ["", "token\ncheck-config-custody-v1", "token\rstatus"] {
        assert!(SystemMacCheckClient::system(bad.into()).is_err());
    }
    // Construction is lazy and gives no Root completion. There is deliberately
    // no constructor accepting our Queue, MockStream or a generic HelperClient.
    assert!(SystemMacCheckClient::system("bounded-auth-data".into()).is_ok());
}

#[test]
fn first_failure_is_preserved_after_same_key_cancel_and_confirmed_release() {
    let (c, q) = client(vec![
        CheckReply::Begun { key: key() },
        CheckReply::Appended {
            key: key(),
            received: 1,
        }, // wrong offset acknowledgement
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Cancelled,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Released { key: key() },
    ]);
    match c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)) {
        Err(HelperCheckFailure::Retired(first)) => {
            assert!(first.contains("offset response differs"))
        }
        other => panic!("first failure and confirmed Release were not kept distinct: {other:?}"),
    }
    assert!(q.lock().unwrap().is_empty());
}

#[test]
fn missing_release_reply_is_not_retirement_even_after_terminal_status() {
    let q = Arc::new(Mutex::new(VecDeque::from([
        MockStream::with_response((CheckReply::Begun { key: key() }.to_wire() + "\n").into_bytes()),
        MockStream::with_response(
            (CheckReply::Appended {
                key: key(),
                received: 2,
            }
            .to_wire()
                + "\n")
                .into_bytes(),
        ),
        MockStream::with_response(
            (CheckReply::Status {
                key: key(),
                status: CheckStatus::Running,
                stdout: 0,
                stderr: 0,
            }
            .to_wire()
                + "\n")
                .into_bytes(),
        ),
        MockStream::with_response(
            (CheckReply::Status {
                key: key(),
                status: CheckStatus::Done,
                stdout: 0,
                stderr: 0,
            }
            .to_wire()
                + "\n")
                .into_bytes(),
        ),
        MockStream::with_response(Vec::new()), // the Release reply was lost
        MockStream::with_response(
            (CheckReply::Status {
                key: key(),
                status: CheckStatus::Unknown,
                stdout: 0,
                stderr: 0,
            }
            .to_wire()
                + "\n")
                .into_bytes(),
        ),
    ])));
    let c = HelperClient::new(Box::new(Queue(Arc::clone(&q))), Platform::Mac, "token");
    assert!(matches!(
        c.run_macos_check(key().core, b"{}", [0; 32], &AtomicBool::new(false)),
        Err(HelperCheckFailure::Unconfirmed(_))
    ));
    assert!(q.lock().unwrap().is_empty());
}

#[test]
fn caller_cancel_waits_for_terminal_before_explicit_output_abandon_and_release() {
    use std::sync::atomic::AtomicUsize;
    struct CancelsAfterAppend {
        queue: Queue,
        cancel: Arc<AtomicBool>,
        calls: AtomicUsize,
    }
    impl Connector for CancelsAfterAppend {
        fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                self.cancel.store(true, Ordering::SeqCst);
            }
            self.queue.connect()
        }
    }
    let replies = vec![
        CheckReply::Begun { key: key() },
        CheckReply::Appended {
            key: key(),
            received: 2,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Receiving,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Status {
            key: key(),
            status: CheckStatus::Cancelled,
            stdout: 0,
            stderr: 0,
        },
        CheckReply::Released { key: key() },
    ];
    let q = Arc::new(Mutex::new(
        replies
            .into_iter()
            .map(|r| MockStream::with_response((r.to_wire() + "\n").into_bytes()))
            .collect(),
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let c = HelperClient::new(
        Box::new(CancelsAfterAppend {
            queue: Queue(Arc::clone(&q)),
            cancel: Arc::clone(&cancel),
            calls: AtomicUsize::new(0),
        }),
        Platform::Mac,
        "token",
    );
    let out = c
        .run_macos_check(key().core, b"{}", [0; 32], &cancel)
        .unwrap();
    assert_eq!(out.status, CheckStatus::Cancelled);
    assert!(
        q.lock().unwrap().is_empty(),
        "Release follows the pending and terminal statuses; Commit was never sent"
    );
}

#[test]
fn platform_response_families_cannot_settle_each_other_even_with_identical_keys() {
    let request = CheckRequest::Status { key: key() };
    let reply = CheckReply::Released { key: key() };
    for (platform, response, accepted) in [
        (
            Platform::Mac,
            ResponseKind::WindowsCheck(reply.clone()),
            false,
        ),
        (Platform::Win, ResponseKind::MacCheck(reply.clone()), false),
        (
            Platform::Win,
            ResponseKind::WindowsCheck(reply.clone()),
            true,
        ),
    ] {
        let stream =
            MockStream::with_response((Response::Ok(response).to_wire_line() + "\n").into_bytes());
        let queue = Arc::new(Mutex::new(VecDeque::from([stream])));
        let client = HelperClient::new(Box::new(Queue(queue)), platform, "token");
        assert_eq!(
            client
                .send_macos_check(request.clone(), Duration::from_secs(1))
                .is_ok(),
            accepted
        );
    }
}
#[cfg(windows)]
#[test]
fn closed_system_windows_client_only_accepts_bounded_authentication_data() {
    for token in ["", "line\nsecond", "line\rsecond"] {
        assert!(SystemWindowsCheckClient::system(token.into()).is_err());
    }
    // Constructing the connector does not connect, issue authority or retire custody.
    assert!(SystemWindowsCheckClient::system("bounded-auth-data".into()).is_ok());
}

use super::*;
use polaris_helper_proto::{codec, Platform, Request, RouteParams, StartParams};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

fn request_samples() -> Vec<Request> {
    let id = polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap();
    let target = polaris_helper_proto::HelperBirthTarget::parse_wire(
        "4242",
        "00112233445566778899aabbccddeeff",
    )
    .unwrap();
    let mut samples = vec![
        Request::Ping,
        Request::Version,
        Request::Status,
        Request::NativeStatusBirth,
        Request::PrepareHelperUpgrade,
        Request::CancelHelperUpgrade,
        Request::CoreStatus { id: id.clone() },
        Request::NativeStopBirth { target },
        Request::Cleanup,
        Request::Uninstall,
        Request::FlushDns,
        Request::WindowsFlushDnsNative,
        Request::FreePort { port: 65535 },
        Request::Stop { pid: None },
        Request::Stop { pid: Some(4242) },
        Request::IfaceMetric {
            iface: "polaris-ts".into(),
            metric: 12345,
        },
        Request::RouteAdd(RouteParams {
            iface: "polaris-ts".into(),
            cidrs: vec!["10.0.0.0/8".into(), "::/0".into()],
        }),
        Request::RouteDel(RouteParams {
            iface: "polaris-ts".into(),
            cidrs: Vec::new(),
        }),
    ];
    for parent_pid in [None, Some(12345)] {
        samples.push(Request::NativeStartPinned(
            polaris_helper_proto::core_protocol::PinnedStartParams {
                id: id.clone(),
                common: StartParams {
                    cfg: "C:\\Users\\测试\\配置.json".into(),
                    log: String::new(),
                    fwd: true,
                    parent_pid,
                },
            },
        ));
    }
    samples
}

#[test]
fn every_native_read_boundary_preserves_complete_request_bytes() {
    for request in request_samples() {
        let wire = codec::encode(Platform::Win, "token", &request);
        let expected = String::from_utf8(wire.clone()).unwrap();
        for boundary in 1..wire.len() {
            let mut decoder = MessageFrame::default();
            assert_eq!(decoder.push(&wire[..boundary], false).unwrap(), None);
            assert_eq!(
                decoder.push(&wire[boundary..], true).unwrap(),
                Some(expected.clone())
            );
        }
        let mut decoder = MessageFrame::default();
        for (index, byte) in wire.iter().enumerate() {
            let result = decoder.push(&[*byte], index + 1 == wire.len()).unwrap();
            if index + 1 == wire.len() {
                assert_eq!(result, Some(expected.clone()));
            } else {
                assert_eq!(result, None);
            }
        }
    }
}

#[test]
fn complete_looking_prefix_cannot_dispatch_before_native_message_end() {
    // Parent-less Start is valid, so newline/arity alone cannot choose this end.
    let request = request_samples().pop().unwrap();
    let wire = codec::encode(Platform::Win, "token", &request);
    let boundary = wire.len() - "12345\n".len();
    let mut decoder = MessageFrame::default();
    assert_eq!(decoder.push(&wire[..boundary], false).unwrap(), None);
    assert_eq!(decoder.push(b"123", false).unwrap(), None);
    assert_eq!(
        decoder.push(b"45\n", true).unwrap(),
        String::from_utf8(wire).ok()
    );
}

#[test]
fn distinct_native_messages_are_not_concatenated() {
    let mut decoder = MessageFrame::default();
    assert!(decoder
        .push(b"token\niface-metric\npolaris-ts\n12", true)
        .is_err());
    assert!(decoder.push(b"345\n", true).is_err());
    let mut decoder = MessageFrame::default();
    assert_eq!(
        decoder.push(b"token\nping\n", true).unwrap(),
        Some("token\nping\n".into())
    );
    assert!(decoder.push(b"token\ncleanup\n", true).is_err());
}

#[test]
fn malformed_or_unterminated_messages_never_expose_text() {
    for bytes in [b"".as_slice(), b"token\n", b"token\nping", b"token\n\xff\n"] {
        assert!(MessageFrame::default().push(bytes, true).is_err());
    }
    assert!(MessageFrame::default().push(b"", false).is_err());
}

#[test]
fn cumulative_maximum_includes_every_chunk_and_exact_limit_is_allowed() {
    let mut wire = b"token\nunknown\n".to_vec();
    wire.resize(MAX_FRAME_BYTES - 1, b'x');
    wire.push(b'\n');
    let mut decoder = MessageFrame::default();
    for chunk in wire[..wire.len() - 1].chunks(4096) {
        assert_eq!(decoder.push(chunk, false).unwrap(), None);
    }
    assert_eq!(
        decoder.push(b"\n", true).unwrap().unwrap().len(),
        MAX_FRAME_BYTES
    );
    let mut oversized = MessageFrame::default();
    assert_eq!(
        oversized.push(&wire[..MAX_FRAME_BYTES - 1], false).unwrap(),
        None
    );
    assert!(oversized.push(b"xx", true).is_err());
    assert!(MessageFrame::default().push(&wire, false).is_err());
}

#[test]
fn one_deadline_covers_empty_polls_and_dribbling_fragments() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let reads = Cell::new(0);
    let result = read_frame_until(
        start + Duration::from_millis(12),
        |buffer| {
            reads.set(reads.get() + 1);
            if reads.get() % 2 == 0 {
                buffer[0] = b'x';
                Ok(MessageRead {
                    bytes: 1,
                    complete: false,
                })
            } else {
                Err(ErrorKind::WouldBlock.into())
            }
        },
        || clock.get(),
        |duration| clock.set(clock.get() + duration),
    );
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
    assert_eq!(clock.get(), start + Duration::from_millis(12));
    assert_eq!(reads.get(), 5);
}

#[test]
fn expired_deadline_never_starts_read_and_late_complete_message_is_not_returned() {
    let start = Instant::now();
    let result = read_frame_until(
        start,
        |_| panic!("expired read"),
        || start,
        |_| panic!("wait"),
    );
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
    let clock = Cell::new(start);
    let deadline = start + Duration::from_millis(1);
    let result = read_frame_until(
        deadline,
        |buffer| {
            buffer[..11].copy_from_slice(b"token\nping\n");
            clock.set(deadline);
            Ok(MessageRead {
                bytes: 11,
                complete: true,
            })
        },
        || clock.get(),
        |_| panic!("wait"),
    );
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
}

#[test]
fn interrupted_poll_and_more_data_share_deadline_then_return_exact_message() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let steps = RefCell::new(VecDeque::from([
        Err(ErrorKind::Interrupted),
        Err(ErrorKind::WouldBlock),
        Ok((b"token\niface-metric\npolaris-ts\n12".as_slice(), false)),
        Err(ErrorKind::WouldBlock),
        Ok((b"345\n".as_slice(), true)),
    ]));
    let result = read_frame_until(
        start + Duration::from_secs(1),
        |buffer| match steps
            .borrow_mut()
            .pop_front()
            .expect("unexpected extra read")
        {
            Err(kind) => Err(kind.into()),
            Ok((bytes, complete)) => {
                buffer[..bytes.len()].copy_from_slice(bytes);
                Ok(MessageRead {
                    bytes: bytes.len(),
                    complete,
                })
            }
        },
        || clock.get(),
        |duration| clock.set(clock.get() + duration),
    )
    .unwrap();
    assert_eq!(result, "token\niface-metric\npolaris-ts\n12345\n");
    assert!(steps.borrow().is_empty());
}

#[test]
fn peer_close_after_partial_message_is_failure_not_end_of_frame() {
    let start = Instant::now();
    let mut first = true;
    let error = read_frame_until(
        start + Duration::from_secs(1),
        |buffer| {
            if first {
                first = false;
                buffer[..11].copy_from_slice(b"token\nping\n");
                Ok(MessageRead {
                    bytes: 11,
                    complete: false,
                })
            } else {
                Err(ErrorKind::UnexpectedEof.into())
            }
        },
        || start,
        |_| panic!("wait"),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn cancellation_repeats_when_first_attempt_precedes_operation_start() {
    let start = Instant::now();
    let current = Cell::new(start);
    let pending = Cell::new(false);
    let finished = Cell::new(false);
    let cancellations = Cell::new(0);
    cancel_after_deadline(
        start,
        || finished.get(),
        || current.get(),
        |duration| {
            current.set(current.get() + duration);
            if !finished.get() {
                pending.set(true);
            }
        },
        || {
            cancellations.set(cancellations.get() + 1);
            if pending.get() {
                finished.set(true);
            }
        },
    );
    assert_eq!(cancellations.get(), 2);
}

#[test]
fn cancellation_repeats_across_write_to_flush_gap_and_stops_before_other_work() {
    let start = Instant::now();
    let phase = Cell::new(0); // 0: write complete, 1: gap, 2: flush, 3: scope done
    let cancelled_phases = RefCell::new(Vec::new());
    cancel_after_deadline(
        start,
        || phase.get() == 3,
        || start,
        |_| {
            if phase.get() < 2 {
                phase.set(phase.get() + 1);
            }
        },
        || {
            cancelled_phases.borrow_mut().push(phase.get());
            if phase.get() == 2 {
                phase.set(3);
            }
        },
    );
    assert_eq!(*cancelled_phases.borrow(), [0, 1, 2]);
    cancel_after_deadline(
        start,
        || true,
        || panic!("clock"),
        |_| panic!("park"),
        || panic!("cancel unrelated work"),
    );
}

#[test]
fn cancellation_uses_callers_absolute_deadline_even_if_watchdog_starts_late() {
    let start = Instant::now();
    let finished = Cell::new(false);
    let cancellations = Cell::new(0);
    cancel_after_deadline(
        start,
        || finished.get(),
        || start + Duration::from_secs(10),
        |_| finished.set(true),
        || cancellations.set(cancellations.get() + 1),
    );
    assert_eq!(cancellations.get(), 1);
}

#[test]
fn native_source_preserves_message_modes_and_exact_joined_thread_custody() {
    let source = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let code = polaris_source_probe::mask_comments_and_strings(&source);
    let compact = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(compact.contains(
        "PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS"
    ));
    assert!(compact.contains("PIPE_NOWAIT | PIPE_READMODE_MESSAGE"));
    assert!(compact.contains("PIPE_WAIT | PIPE_READMODE_MESSAGE"));
    assert!(!compact.contains("PIPE_TYPE_BYTE"));
    assert!(!compact.contains("PIPE_READMODE_BYTE"));
    assert!(compact.contains("Some(ERROR_MORE_DATA) => Ok(MessageRead"));
    assert!(compact.contains("complete: false"));
    assert!(compact.contains("CancelSynchronousIo(thread.as_raw_handle())"));
    assert!(!compact.contains("CancelIoEx"));
    let duplicate = compact.find("DuplicateHandle(").unwrap();
    let spawn = compact.find("Builder::new().spawn").unwrap();
    assert!(duplicate < spawn);
    assert!(compact[duplicate..spawn].contains("GetCurrentThread()"));
    let drop = compact.find("impl Drop for IoTimeoutGuard").unwrap();
    assert!(compact[drop..].contains("j.join()"));
}

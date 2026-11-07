use super::*;
use crate::{codec, Platform, Request, Response, ResponseKind};
fn key() -> CheckKey {
    CheckKey {
        core: CoreId::parse(&"a".repeat(64)).unwrap(),
        correlation: HelperBirthToken::from_bytes([7; 16]),
    }
}

#[test]
fn full_size_input_is_chunked_below_both_actual_wire_caps() {
    let token = "t".repeat(crate::command::mac::MAX_WIRE_LINE_BYTES);
    let begin = CheckRequest::Begin {
        core: key().core,
        bytes: MAX_CONFIG_BYTES,
        sha256: [1; 32],
    };
    let mut total = 0;
    for request in std::iter::once(begin).chain((0..MAX_CONFIG_BYTES / CHUNK_BYTES).map(|n| {
        CheckRequest::Append {
            key: key(),
            offset: n * CHUNK_BYTES,
            bytes: vec![0xff; CHUNK_BYTES],
        }
    })) {
        let mut args = Vec::new();
        request.write_args(&mut args);
        assert_eq!(CheckRequest::parse(&args), Some(request.clone()));
        let lines = codec::encode_frame(Platform::Mac, &token, &Request::MacCheck(request));
        assert!(lines
            .iter()
            .all(|s| s.len() <= crate::command::mac::MAX_WIRE_LINE_BYTES));
        let actual = codec::frame_to_bytes(&lines);
        assert!(
            actual.len() <= codec::MAX_FRAME_BYTES,
            "whole frame, not just its payload"
        );
        if let Some(data) = args.last().and_then(|s| unhex(s, CHUNK_BYTES)) {
            if args[0] == "append" {
                total += data.len();
            }
        }
    }
    assert_eq!(total, MAX_CONFIG_BYTES);
}
#[test]
fn malformed_or_unbounded_input_has_no_shorter_operation_projection() {
    let mut args = vec![
        "append".into(),
        key().core.as_str().into(),
        key().correlation.to_wire(),
        "0".into(),
        "ab".into(),
    ];
    assert!(CheckRequest::parse(&args).is_some());
    for bad in ["", "A0", "a", "gg"] {
        args[4] = bad.into();
        assert!(CheckRequest::parse(&args).is_none());
    }
    args[4] = hex(&vec![0; CHUNK_BYTES + 1]);
    assert!(CheckRequest::parse(&args).is_none());
    args[4] = "ab".into();
    args[3] = "00".into();
    assert!(CheckRequest::parse(&args).is_none());
    args[3] = "0".into();
    args.push("replayed-tail".into());
    assert!(CheckRequest::parse(&args).is_none());
    for bytes in [0, MAX_CONFIG_BYTES + 1] {
        let r = CheckRequest::Begin {
            core: key().core,
            bytes,
            sha256: [0; 32],
        };
        let mut a = Vec::new();
        r.write_args(&mut a);
        assert!(CheckRequest::parse(&a).is_none());
    }
}
#[test]
fn each_pipe_is_separate_and_each_reply_is_canonical_bounded() {
    for stream in [OutputStream::Stdout, OutputStream::Stderr] {
        for offset in [0, OUTPUT_BYTES - CHUNK_BYTES] {
            let reply = CheckReply::Output {
                key: key(),
                stream,
                offset,
                bytes: vec![0xff; CHUNK_BYTES],
            };
            let wire = reply.to_wire();
            assert!(wire.len() + 1 < crate::command::mac::MAX_WIRE_LINE_BYTES);
            assert_eq!(
                Response::parse(&wire),
                Response::Ok(ResponseKind::MacCheck(reply))
            );
            assert!(CheckReply::parse(&(wire + " ")).is_none());
        }
    }
    let over = CheckReply::Output {
        key: key(),
        stream: OutputStream::Stdout,
        offset: OUTPUT_BYTES,
        bytes: vec![1],
    };
    assert!(CheckReply::parse(&over.to_wire()).is_none());
    let unknown = CheckReply::Status {
        key: key(),
        status: CheckStatus::Unknown,
        stdout: 0,
        stderr: 0,
    };
    assert_eq!(CheckReply::parse(&unknown.to_wire()), Some(unknown));
    assert!(!CheckStatus::Unknown.terminal());
    for request in [
        CheckRequest::Commit { key: key() },
        CheckRequest::Status { key: key() },
        CheckRequest::Cancel { key: key() },
        CheckRequest::Release { key: key() },
    ] {
        let mut args = Vec::new();
        request.write_args(&mut args);
        assert_eq!(CheckRequest::parse(&args), Some(request));
        args.push("extra".into());
        assert!(CheckRequest::parse(&args).is_none());
    }
}

#[test]
fn windows_check_has_a_distinct_command_and_response_family() {
    let request = CheckRequest::Status { key: key() };
    let mac = Request::MacCheck(request.clone());
    let win = Request::WindowsCheck(request);
    assert_ne!(mac.command_name(), win.command_name());
    let reply = CheckReply::Released { key: key() };
    let win = Response::Ok(ResponseKind::WindowsCheck(reply.clone()));
    let mac = Response::Ok(ResponseKind::MacCheck(reply));
    assert_ne!(win.to_wire_line(), mac.to_wire_line());
    assert_eq!(Response::parse(&win.to_wire_line()), win);
    assert_eq!(Response::parse(&mac.to_wire_line()), mac);
}

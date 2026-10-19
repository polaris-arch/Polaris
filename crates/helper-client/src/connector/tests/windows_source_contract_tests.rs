//! Inert source contracts only. Native overlapped cancellation and backpressure
//! require Windows acceptance; these assertions do not claim kernel execution.
#[test]
fn windows_client_owns_overlapped_io_until_completion() {
    let source = polaris_source_probe::crate_source!("windows_pipe.rs");
    let code = polaris_source_probe::mask_comments(&source);
    for anchor in [
        "FILE_FLAG_OVERLAPPED",
        "CreateEventW(",
        "ReadFile(",
        "WriteFile(",
        "WaitForSingleObject(",
        "CancelIoEx(",
        "GetOverlappedResult(",
        "OwnedHandle",
        "fn write_all_until",
    ] {
        assert!(code.contains(anchor), "missing {anchor}");
    }
    assert!(!code.contains("thread::spawn"));
    assert!(code.contains("GetNamedPipeInfo("));
    assert!(code.contains("flags & PIPE_TYPE_MESSAGE != 0"));
    let cancellation = code.split("if wait != WAIT_OBJECT_0").nth(1).unwrap();
    assert!(
        cancellation.find("CancelIoEx(").unwrap()
            < cancellation.find("GetOverlappedResult(").unwrap()
    );
    assert!(
        cancellation.find("GetOverlappedResult(").unwrap()
            < cancellation.find("return Err(error)").unwrap()
    );
}

#[test]
fn windows_read_and_write_use_one_bounded_transfer_owner() {
    let raw = polaris_source_probe::crate_source!("windows_pipe.rs");
    let code = polaris_source_probe::mask_comments(&raw);
    assert!(code.contains("Instant::now() >= deadline"));
    assert!(code.contains("MAX_FRAME_BYTES") && code.contains(".saturating_sub(total)"));
    assert!(code.contains("written != data.len()"));
    assert!(code.contains("ErrorKind::WriteZero"));
    let read = code
        .split("fn read_until_timeout")
        .nth(1)
        .unwrap()
        .split("fn set_read_timeout")
        .next()
        .unwrap();
    let write = code
        .split("fn write_all_until")
        .nth(1)
        .unwrap()
        .split("fn shutdown")
        .next()
        .unwrap();
    assert!(read.contains("self.transfer("));
    assert!(write.contains("self.transfer("));
    assert!(read.contains("is_peer_gone("));
    assert!(!write.contains("is_peer_gone("));
}

#[test]
fn production_connectors_override_absolute_deadline() {
    let raw = polaris_source_probe::crate_source!("connector.rs");
    let code = polaris_source_probe::mask_comments(&raw);
    assert_eq!(code.matches("fn connect_until").count(), 2);
    assert!(code.contains("socket.connect_timeout(&address, remaining)"));
    assert!(!code.contains("UnixStream::connect("));
    assert!(!code.contains("thread::spawn"));
}

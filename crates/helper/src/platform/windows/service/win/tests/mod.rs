/// win 侧 wire 契约（序列化本体已上提 helper-proto，见 `Response::to_wire_line`；
/// 本测试保留为 win 视角的回归断言 —— win 的 uid 恒 0，`helper-win/helper.go:179`）。
#[test]
fn wire_ok_matches_go_format() {
    use polaris_helper_proto::{Pong, Response, ResponseKind};
    let wire = |k| Response::Ok(k).to_wire_line();
    assert_eq!(
        wire(ResponseKind::Pong(Pong {
            uid: 0,
            proto_version: 5,
            build_identity: None,
        })),
        "OK pong uid=0 v5"
    );
    assert_eq!(wire(ResponseKind::Version { proto_version: 5 }), "OK 5");
    assert_eq!(wire(ResponseKind::Cleaned), "OK cleaned");
    assert_eq!(wire(ResponseKind::Uninstalling), "OK uninstalling");
}

use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

#[allow(unsafe_code, reason = "owns both ends of a uniquely named test pipe")]
fn connected_test_pipe() -> (OwnedHandle, OwnedHandle) {
    static NEXT_PIPE: AtomicUsize = AtomicUsize::new(0);
    let name = wide_null(format!(
        r"\\.\pipe\polaris-helper-connect-test-{}-{}",
        std::process::id(),
        NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
    ));
    // SAFETY: name is NUL-terminated and all optional pointers are null. The
    // default process DACL is sufficient for this same-process test connection.
    let raw_server = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(
        raw_server,
        INVALID_HANDLE_VALUE,
        "{}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful API result transfers exactly one owned handle.
    let server = unsafe { OwnedHandle::from_raw_handle(raw_server) };
    // Connect the client *before* calling the production ConnectNamedPipe path.
    // SAFETY: name is live, optional parameters are null and the returned handle
    // is owned below. No real helper pipe or SCM state is accessed.
    let raw_client = unsafe {
        CreateFileW(
            name.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | windows_sys::Win32::Foundation::GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        raw_client,
        INVALID_HANDLE_VALUE,
        "{}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful API result transfers exactly one owned handle.
    let client = unsafe { OwnedHandle::from_raw_handle(raw_client) };
    (server, client)
}

#[test]
fn connect_accepts_client_that_connected_before_accept() {
    let (server, _client) = connected_test_pipe();
    assert!(
        connect_pipe(server.as_raw_handle()),
        "ERROR_PIPE_CONNECTED means the early client is already connected"
    );
}

#[test]
fn connect_rejects_real_disconnected_client_error() {
    let (server, client) = connected_test_pipe();
    drop(client);
    assert!(
        !connect_pipe(server.as_raw_handle()),
        "a client that already closed must not be treated as connected"
    );
}

#[test]
fn accepted_stop_and_shutdown_return_scm_success_after_requesting_stop() {
    use std::cell::RefCell;
    for control in [
        SERVICE_CONTROL_STOP,
        windows_sys::Win32::System::Services::SERVICE_CONTROL_SHUTDOWN,
    ] {
        let actions = RefCell::new(Vec::new());
        let result = handle_control(
            control,
            || {
                actions.borrow_mut().push("fence");
                true
            },
            || actions.borrow_mut().push("request-stop"),
        );
        assert_eq!(result, NO_ERROR);
        assert_eq!(*actions.borrow(), ["fence", "request-stop"]);
    }
}

#[test]
fn missing_custody_rejects_stop_without_requesting_it() {
    for control in [
        SERVICE_CONTROL_STOP,
        windows_sys::Win32::System::Services::SERVICE_CONTROL_SHUTDOWN,
    ] {
        let result = handle_control(control, || false, || panic!("stop must not be accepted"));
        assert_eq!(result, ERROR_SERVICE_CANNOT_ACCEPT_CTRL);
        assert_ne!(result, NO_ERROR);
    }
}

#[allow(
    unsafe_code,
    reason = "creates an isolated owned accept regression pipe"
)]
fn waiting_test_pipe() -> OwnedHandle {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let name = wide_null(format!(
        r"\\.\pipe\polaris-stop-poll-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    unsafe { OwnedHandle::from_raw_handle(handle) }
}
#[test]
fn stop_requested_before_pipe_creation_needs_no_wakeup_client() {
    let stop = true;
    let pipe = waiting_test_pipe();
    assert!(!connect_pipe_until_stopped(pipe.as_raw_handle(), || stop));
}
#[test]
fn stop_after_first_empty_accept_poll_is_observed() {
    let pipe = waiting_test_pipe();
    let mut polls = 0;
    assert!(!connect_pipe_until_stopped(pipe.as_raw_handle(), || {
        polls += 1;
        polls > 1
    }));
    assert_eq!(polls, 2);
}
#[test]
fn early_connected_client_is_not_confused_with_available_pipe() {
    let (server, _client) = connected_test_pipe();
    assert!(connect_pipe_until_stopped(server.as_raw_handle(), || false));
}

#[allow(
    unsafe_code,
    reason = "writes one explicit message to an isolated test pipe"
)]
fn write_test_message(client: &OwnedHandle, bytes: &[u8]) {
    let mut written = 0;
    // SAFETY: caller owns this test pipe; input remains live until synchronous
    // completion. A short write is a failed fixture, never a suffix message.
    let ok = unsafe {
        WriteFile(
            client.as_raw_handle(),
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    assert_eq!(written as usize, bytes.len());
}

#[test]
fn native_message_larger_than_read_buffer_is_accumulated_without_byte_changes() {
    let (server, client) = connected_test_pipe();
    assert!(connect_pipe(server.as_raw_handle()));
    let expected = format!("token\nunknown\n{}\n", "配置".repeat(5000));
    let request = expected.clone();
    let writer = std::thread::spawn(move || {
        write_test_message(&client, request.as_bytes());
        client // retain peer until the reader has consumed the complete message
    });
    let actual = read_frame(
        server.as_raw_handle(),
        std::time::Instant::now() + std::time::Duration::from_secs(2),
    )
    .unwrap();
    let _client = writer.join().unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn native_incomplete_first_message_is_rejected_not_joined_to_the_next_message() {
    let (server, client) = connected_test_pipe();
    assert!(connect_pipe(server.as_raw_handle()));
    write_test_message(&client, b"token\niface-metric\npolaris-ts\n12");
    write_test_message(&client, b"345\n");
    let error = read_frame(
        server.as_raw_handle(),
        std::time::Instant::now() + std::time::Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn native_empty_pipe_read_obeys_absolute_deadline() {
    let (server, _client) = connected_test_pipe();
    assert!(connect_pipe(server.as_raw_handle()));
    let error = read_frame(
        server.as_raw_handle(),
        std::time::Instant::now() + std::time::Duration::from_millis(30),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn native_guard_cancels_flush_started_after_an_earlier_cancel_found_no_pending_io() {
    let (server, client) = connected_test_pipe();
    assert!(connect_pipe(server.as_raw_handle()));
    // This test deliberately creates the one-shot cancellation gap. The peer
    // remains open and never reads; only repeated synchronous cancellation can
    // release this test's isolated flush. It is a native acceptance fixture.
    write_test_message(&server, b"OK pong\n");
    // A failed cancellation implementation must fail this fixture, not hang the
    // test runner forever. Peer closure after two seconds releases the flush.
    let (finish, finished) = std::sync::mpsc::channel();
    let peer = std::thread::spawn(move || {
        let _ = finished.recv_timeout(std::time::Duration::from_secs(2));
        drop(client);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
    let guard = IoTimeoutGuard::arm(deadline).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(40));
    #[allow(unsafe_code, reason = "flushes this isolated owned test pipe")]
    let result = unsafe { FlushFileBuffers(server.as_raw_handle()) };
    let error = std::io::Error::last_os_error();
    drop(guard);
    let _ = finish.send(());
    peer.join().unwrap();
    assert_eq!(result, 0);
    assert_eq!(
        error.raw_os_error(),
        Some(windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED as i32)
    );
}

#[test]
#[allow(
    unsafe_code,
    reason = "reads an isolated default-byte-mode legacy client handle"
)]
fn unchanged_synchronous_client_round_trips_with_message_mode_server() {
    let (server, client) = connected_test_pipe();
    assert!(connect_pipe(server.as_raw_handle()));
    // The fixture client uses CreateFileW flags=0, default byte-read mode and
    // the same one synchronous WriteFile used by the previous production client.
    let request = polaris_helper_proto::codec::encode(
        polaris_helper_proto::Platform::Win,
        "token",
        &polaris_helper_proto::Request::Ping,
    );
    write_test_message(&client, &request);
    let raw = read_frame(
        server.as_raw_handle(),
        std::time::Instant::now() + std::time::Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(raw.as_bytes(), request);
    let responder = std::thread::spawn(move || {
        write_response(
            server.as_raw_handle(),
            b"OK pong uid=0 v2\n",
            FlushMode::WaitPeer,
        );
        server
    });
    let mut response = [0u8; 128];
    let mut got = 0;
    // SAFETY: client and bounded output buffer remain owned through this call.
    let ok = unsafe {
        ReadFile(
            client.as_raw_handle(),
            response.as_mut_ptr(),
            response.len() as u32,
            &mut got,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    assert_eq!(&response[..got as usize], b"OK pong uid=0 v2\n");
    let _server = responder.join().unwrap();
}

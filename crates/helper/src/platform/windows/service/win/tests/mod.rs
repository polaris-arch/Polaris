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
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(
        raw_server, INVALID_HANDLE_VALUE,
        "{}", std::io::Error::last_os_error()
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
        raw_client, INVALID_HANDLE_VALUE,
        "{}", std::io::Error::last_os_error()
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

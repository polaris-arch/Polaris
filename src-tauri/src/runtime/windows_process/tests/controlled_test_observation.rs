//! Strict read-only OS observations for the controlled real-core tests.
//!
//! This test-only adapter belongs to the existing Windows FFI boundary. Production
//! liveness deliberately treats unknown as alive; tests instead require explicit
//! evidence and fail on probe errors. Neither observer grants termination rights.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, FALSE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

// Keep the queried process alive by handle for the entire observation. A wait
// result distinguishes exit from running, including exit code STILL_ACTIVE.
#[allow(unsafe_code, reason = "read-only Win32 process observation")]
pub(crate) fn alive(pid: u32) -> bool {
    assert_ne!(pid, 0, "cannot observe PID zero");
    // SAFETY: no pointers; requests only the right to wait, never termination.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, FALSE, pid) };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return false; // Windows explicitly reports that this PID does not exist.
        }
        panic!("OpenProcess({pid}) observation failed: {error}");
    }
    // SAFETY: successful OpenProcess returned a new owned handle.
    let process = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    // SAFETY: process owns a valid SYNCHRONIZE handle throughout this call.
    match unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 0) } {
        WAIT_OBJECT_0 => false,
        WAIT_TIMEOUT => true,
        other => panic!(
            "process wait observation failed ({other}): {}",
            std::io::Error::last_os_error()
        ),
    }
}

#[allow(unsafe_code, reason = "read-only Win32 ToolHelp census")]
pub(crate) fn count() -> usize {
    // SAFETY: no pointers; failure is INVALID_HANDLE_VALUE.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert!(
        raw != INVALID_HANDLE_VALUE,
        "process snapshot failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful snapshot call returned a new owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snapshot remains owned; entry is writable and declares its size.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle().cast(), &mut entry) };
    let mut count = 0;
    loop {
        if more == 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_NO_MORE_FILES as i32),
                "process census failed: {error}"
            );
            return count;
        }
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .expect("terminated process name");
        // Other process names may contain arbitrary UTF-16. Compare only the
        // exact ASCII basename, without lossy conversion or command-line matches.
        if is_singbox_name(&entry.szExeFile[..end]) {
            count += 1;
        }
        // SAFETY: same owned snapshot and writable entry as Process32FirstW.
        more = unsafe { Process32NextW(snapshot.as_raw_handle().cast(), &mut entry) };
    }
}

fn is_singbox_name(name: &[u16]) -> bool {
    let expected = b"sing-box.exe";
    name.len() == expected.len()
        && name.iter().zip(expected).all(|(actual, expected)| {
            *actual <= 127 && (*actual as u8).eq_ignore_ascii_case(expected)
        })
}

#[test]
fn windows_name_is_exact_case_insensitive_basename() {
    for name in ["sing-box.exe", "SING-BOX.EXE"] {
        assert!(is_singbox_name(&name.encode_utf16().collect::<Vec<_>>()));
    }
    for name in [
        "sing-box",
        "sing-box.exe.old",
        "not-sing-box.exe",
        "C:\\sing-box.exe",
        "test sing-box.exe",
    ] {
        assert!(!is_singbox_name(&name.encode_utf16().collect::<Vec<_>>()));
    }
}

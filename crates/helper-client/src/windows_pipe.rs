//! Deadline-aware Windows byte-pipe client. Each pending operation owns its event,
//! OVERLAPPED and buffer until completion, including cancellation completion.
//! No detached blocking worker can outlive a request or use a recycled handle.
use crate::transport::ConnectionStream;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_NOT_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeInfo, PIPE_TYPE_MESSAGE};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const _: () = assert!(
    polaris_helper_proto::windows_helper::PIPE_CLIENT_ACCESS
        == windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA
            | windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA
            | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
            | windows_sys::Win32::Storage::FileSystem::READ_CONTROL
            | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE
);

fn is_peer_gone(error: u32) -> bool {
    error == ERROR_BROKEN_PIPE || error == ERROR_PIPE_NOT_CONNECTED
}

pub(crate) struct WinPipeStream {
    handle: OwnedHandle,
    read_timeout: Duration,
    message_pipe: bool,
}

#[allow(
    unsafe_code,
    reason = "owns pipe/event handles and drains each OVERLAPPED before releasing borrowed buffers"
)]
impl WinPipeStream {
    pub(crate) fn connect(path: &Path) -> io::Result<Self> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: NUL-terminated path lives through the call; valid handle is
        // immediately adopted, never shared with another stream reader/writer.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                polaris_helper_proto::windows_helper::PIPE_CLIENT_ACCESS,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut stream = Self {
            // SAFETY: new valid exclusively owned CreateFileW handle.
            handle: unsafe { OwnedHandle::from_raw_handle(handle.cast()) },
            read_timeout: crate::transport::READ_TIMEOUT,
            message_pipe: false,
        };
        let mut flags = 0;
        // SAFETY: query only this owned connected pipe; unused outputs are null.
        if unsafe {
            GetNamedPipeInfo(
                stream.raw(),
                &mut flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        stream.message_pipe = flags & PIPE_TYPE_MESSAGE != 0;
        Ok(stream)
    }

    pub(crate) fn connect_system() -> io::Result<Self> {
        let stream = Self::connect(Path::new(polaris_helper_proto::windows_helper::PIPE_NAME))?;
        stream.require_system_security()?;
        Ok(stream)
    }

    fn require_system_security(&self) -> io::Result<()> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION};
        let mut owner = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        // Authenticate the connected object, not its name or a recyclable PID.
        // Ordinary desktop users cannot open a SYSTEM service process/token.
        let error = unsafe {
            GetSecurityInfo(
                self.raw(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        // owner and dacl borrow the descriptor until LocalFree below.
        let matches = unsafe { system_pipe_security_matches(owner, dacl) };
        unsafe { LocalFree(descriptor) };
        if !matches? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Helper pipe lacks SYSTEM-only server authority; update the installed Helper",
            ));
        }
        // The retained pipe handle pins this object. Only SYSTEM can create its
        // server instances or change its descriptor; disconnect cannot rebind a
        // client handle to another server. No process-query permission is needed.
        Ok(())
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle().cast()
    }

    /// The pointer borrows the caller's read/write buffer for this entire call.
    /// Cancellation is a request, not completion: always drain before returning.
    /// Native cancellation latency is a platform acceptance gate, not a promise
    /// that kernel completion can be abandoned at the wall-clock deadline.
    fn transfer(
        &self,
        buffer: *mut u8,
        len: u32,
        write: bool,
        deadline: Instant,
    ) -> io::Result<usize> {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // SAFETY: unnamed manual-reset event, no inherited handle or security data.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW returned a new valid exclusively owned event.
        let event = unsafe { OwnedHandle::from_raw_handle(event.cast()) };
        // SAFETY: zero initialization is valid for a fresh OVERLAPPED; hEvent is
        // live until the operation completes. The stack object never moves while pending.
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = event.as_raw_handle().cast();
        let mut transferred = 0;
        // SAFETY: caller's buffer has len bytes and remains borrowed until drain;
        // write uses immutable input, read uses exclusive writable memory.
        let ok = unsafe {
            if write {
                WriteFile(
                    self.raw(),
                    buffer.cast_const(),
                    len,
                    std::ptr::null_mut(),
                    &mut operation,
                )
            } else {
                ReadFile(
                    self.raw(),
                    buffer,
                    len,
                    std::ptr::null_mut(),
                    &mut operation,
                )
            }
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = remaining
                .as_millis()
                .saturating_add(u128::from(
                    !remaining.subsec_nanos().is_multiple_of(1_000_000),
                ))
                .min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: event is live, belongs to this outstanding operation only.
            let wait = unsafe { WaitForSingleObject(operation.hEvent, millis) };
            if wait != WAIT_OBJECT_0 {
                let error = if wait == WAIT_TIMEOUT {
                    io::ErrorKind::TimedOut.into()
                } else {
                    io::Error::last_os_error()
                };
                // SAFETY: cancel this exact submitted operation, then retain all
                // resources until the kernel signals completion (success or abort).
                unsafe {
                    CancelIoEx(self.raw(), &operation);
                    GetOverlappedResult(self.raw(), &operation, &mut transferred, 1);
                }
                return Err(error);
            }
        }
        // SAFETY: operation completed immediately or its event signalled. Always
        // obtain the byte count from OVERLAPPED, not the asynchronous call's out argument.
        if unsafe { GetOverlappedResult(self.raw(), &operation, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(transferred as usize)
    }
}

impl ConnectionStream for WinPipeStream {
    fn supports_request_boundaries(&self) -> bool {
        self.message_pipe
    }

    fn read_until_timeout(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.read_until_deadline(buf, Instant::now() + self.read_timeout)
    }

    fn read_until_deadline(&mut self, buf: &mut Vec<u8>, deadline: Instant) -> io::Result<usize> {
        let mut total = 0;
        loop {
            let capacity = polaris_helper_proto::codec::MAX_FRAME_BYTES
                .saturating_sub(total)
                .min(4096);
            if capacity == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut chunk = [0u8; 4096];
            let got = match self.transfer(chunk.as_mut_ptr(), capacity as u32, false, deadline) {
                Ok(n) => n,
                Err(error)
                    if error
                        .raw_os_error()
                        .is_some_and(|code| is_peer_gone(code as u32)) =>
                {
                    return Ok(total);
                }
                Err(error) => return Err(error),
            };
            if got == 0 {
                return Ok(total);
            }
            buf.extend_from_slice(&chunk[..got]);
            total += got;
            if let Some(end) = chunk[..got].iter().position(|byte| *byte == b'\n') {
                if end + 1 != got {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                return Ok(total);
            }
        }
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        if timeout.is_zero() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.read_timeout = timeout;
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all_until(data, Instant::now() + self.read_timeout)
    }

    fn write_all_until(&mut self, data: &[u8], deadline: Instant) -> io::Result<()> {
        // One WriteFile is one request message on the server's message-type pipe.
        // Preserve the historical single-write contract. A short completion is an
        // unknown-outcome error, never a second message containing the suffix.
        let length = u32::try_from(data.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let written = self.transfer(data.as_ptr().cast_mut(), length, true, deadline)?;
        if written != data.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }

    fn shutdown(&mut self) -> io::Result<()> {
        // Duplex pipes have no write half-close. The server's message mode
        // supplies the request boundary; clients retain byte-mode response reads.
        // Never FlushFileBuffers here (would introduce another peer-controlled wait).
        Ok(())
    }
}

/// Authenticate the immutable server authority of the connected pipe object.
///
/// # Safety
/// owner/dacl must belong to a live, OS-validated security descriptor.
#[allow(
    unsafe_code,
    reason = "compares borrowed OS ACLs and frees the expected SDDL descriptor"
)]
unsafe fn system_pipe_security_matches(
    owner: windows_sys::Win32::Security::PSID,
    dacl: *const windows_sys::Win32::Security::ACL,
) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, IsWellKnownSid, WinLocalSystemSid,
    };
    if owner.is_null() || dacl.is_null() || unsafe { IsWellKnownSid(owner, WinLocalSystemSid) } == 0
    {
        return Ok(false);
    }
    let expected: Vec<u16> = polaris_helper_proto::windows_helper::PIPE_SDDL
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            expected.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut expected_dacl = std::ptr::null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    let valid = unsafe {
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut expected_dacl, &mut defaulted)
    } != 0
        && present != 0
        && !expected_dacl.is_null();
    // Exact ACL equality also rejects NULL, extra trustees, and the old generic
    // write grant (which lets Interactive Users create a spoof server instance).
    let matches = valid
        && unsafe {
            (*dacl).AclSize == (*expected_dacl).AclSize
                && std::slice::from_raw_parts(dacl.cast::<u8>(), (*dacl).AclSize as usize)
                    == std::slice::from_raw_parts(
                        expected_dacl.cast::<u8>(),
                        (*expected_dacl).AclSize as usize,
                    )
        };
    unsafe { LocalFree(descriptor) };
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, GetSecurityDescriptorOwner};

    #[test]
    #[allow(
        unsafe_code,
        reason = "owns and inspects converted native test descriptors"
    )]
    fn system_pipe_descriptor_rejects_spoofable_server_authority() {
        for (sddl, accepted) in [
            (polaris_helper_proto::windows_helper::PIPE_SDDL, true),
            ("O:SYD:(A;;FA;;;SY)(A;;GRGW;;;IU)", false),
            ("O:BAD:(A;;FA;;;SY)(A;;0x120083;;;IU)", false),
            ("O:SYD:(A;;FA;;;SY)(A;;0x120083;;;IU)(A;;FA;;;BU)", false),
            ("O:SYD:NO_ACCESS_CONTROL", false),
            ("O:SYD:", false),
        ] {
            let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        wide.as_ptr(),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let mut owner = std::ptr::null_mut();
            let mut dacl = std::ptr::null_mut();
            let mut defaulted = 0;
            let mut present = 0;
            assert_ne!(
                unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) },
                0
            );
            assert_ne!(
                unsafe {
                    GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
                },
                0
            );
            let result = unsafe { system_pipe_security_matches(owner, dacl) };
            unsafe { LocalFree(descriptor) };
            assert_eq!(result.unwrap(), accepted, "{sddl}");
        }
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "owns an isolated native pipe with the current user's default ACL"
    )]
    fn connected_user_pipe_cannot_supply_system_evidence() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE,
        };
        let name = format!(r"\\.\pipe\polaris-auth-negative-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let server = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(
            server,
            INVALID_HANDLE_VALUE,
            "{}",
            io::Error::last_os_error()
        );
        let _server = unsafe { OwnedHandle::from_raw_handle(server) };
        let client = WinPipeStream::connect(Path::new(&name)).unwrap();
        assert_eq!(
            client.require_system_security().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

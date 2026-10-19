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
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeInfo, PIPE_TYPE_MESSAGE};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

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
                GENERIC_READ | GENERIC_WRITE,
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

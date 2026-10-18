//! Bounded fixed-service cleanup policy, separate from Windows IO for real
//! transition/error tests. No service identifier is accepted by this interface.
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Absent,
    DeletePending,
    Stopped,
    Starting,
    Stopping,
    Running,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Requested,
    NotActive,
    CannotAccept,
}
#[derive(Debug)]
pub(crate) enum Failure {
    Operation(io::Error),
    Timeout { deleting: bool },
}
pub(crate) trait Ops {
    fn state(&mut self) -> io::Result<State>;
    fn stop(&mut self) -> io::Result<Stop>;
    fn delete(&mut self) -> io::Result<()>;
    fn pause(&mut self) -> bool;
}
pub(crate) fn remove(ops: &mut impl Ops) -> Result<(), Failure> {
    let mut deleting = false;
    loop {
        let state = ops.state().map_err(Failure::Operation)?;
        if state == State::Absent {
            return Ok(());
        }
        if state == State::DeletePending {
            deleting = true;
        }
        if !deleting {
            match state {
                State::Stopped => {
                    ops.delete().map_err(Failure::Operation)?;
                    deleting = true;
                }
                State::Running => {
                    let _ = ops.stop().map_err(Failure::Operation)?;
                }
                State::Starting | State::Stopping => {}
                State::Absent | State::DeletePending => unreachable!(),
            }
        }
        if !ops.pause() {
            return Err(Failure::Timeout { deleting });
        }
    }
}

// Only the deletion call attaches this provenance. Win32 error 5 alone
// cannot distinguish an image mapping from a real access/ACL refusal.
const STATUS_CANNOT_DELETE: i32 = 0xc000_0121u32 as i32;
#[derive(Debug)]
pub(crate) struct DeleteStatus {
    pub(crate) status: i32,
    win32: io::Error,
}
impl std::fmt::Display for DeleteStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NtSetInformationFile deletion status {:#010x}: {}",
            self.status as u32, self.win32
        )
    }
}
impl std::error::Error for DeleteStatus {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.win32)
    }
}
pub(crate) fn deletion_failure(status: i32, win32: u32) -> io::Error {
    let win32 = io::Error::from_raw_os_error(win32 as i32);
    io::Error::new(win32.kind(), DeleteStatus { status, win32 })
}
fn temporary_win32(code: Option<i32>) -> bool {
    matches!(code, Some(32 | 33 | 1224))
}
pub(crate) fn temporary_file_error(error: &io::Error) -> bool {
    temporary_win32(error.raw_os_error())
        || error
            .get_ref()
            .and_then(|e| e.downcast_ref::<DeleteStatus>())
            .is_some_and(|e| {
                e.status == STATUS_CANNOT_DELETE || temporary_win32(e.win32.raw_os_error())
            })
}
pub(crate) fn retry_cleanup(
    mut attempt: impl FnMut() -> io::Result<()>,
    mut pause: impl FnMut() -> bool,
) -> io::Result<()> {
    loop {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(error) if temporary_file_error(&error) => {
                if !pause() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, error));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests;

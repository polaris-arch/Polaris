//! macOS stale-process identity. `ps` rows carry no handle, so a row keeps the kernel start time
//! of its PID, read right after the scan; ending it re-reads that start time for the same PID
//! first and signals only when the two agree (the Windows `same_birth` rule).
use super::ProcessHold;
use nix::libc;
use std::sync::Arc;

pub(super) struct HeldProcess {
    pid: libc::pid_t,
    birth: u64,
}

impl HeldProcess {
    #[allow(unsafe_code, reason = "kill is libc FFI")]
    pub(super) fn signal(&self, signal: crate::process_killer::Signal) -> bool {
        let signal = match signal {
            crate::process_killer::Signal::Sigterm => libc::SIGTERM,
            crate::process_killer::Signal::Sigkill => libc::SIGKILL,
        };
        // SAFETY: no pointer arguments; sent only while the PID still carries the start time
        // seen at scan.
        self.is_running() && unsafe { libc::kill(self.pid, signal) } == 0
    }

    pub(super) fn is_running(&self) -> bool {
        super::same_birth(Some(self.birth), birth(self.pid))
    }
}

/// Start time in microseconds since the epoch; `None` once the PID is gone or unreadable.
#[allow(unsafe_code, reason = "proc_pidinfo is libproc FFI")]
fn birth(pid: libc::pid_t) -> Option<u64> {
    // SAFETY: `proc_bsdinfo` is plain data; all-zero is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a writable local of exactly `size` bytes.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size).then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// The identity of a row `ps` listed. `scanned` is the wall clock (µs) taken before `ps` ran:
/// a start at or after it belongs to a process that reused the PID since, so no identity.
pub(super) fn hold(pid: u32, scanned: u64) -> ProcessHold {
    let held = libc::pid_t::try_from(pid).ok().and_then(|pid| {
        birth(pid)
            .filter(|birth| *birth < scanned)
            .map(|birth| Arc::new(HeldProcess { pid, birth }))
    });
    ProcessHold { held }
}

#[cfg(test)]
mod tests;

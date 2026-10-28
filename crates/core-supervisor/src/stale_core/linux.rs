//! Linux stale-process identity: bind observations through an open proc directory,
//! then retain pidfd authority through TERM, grace and KILL. Never signal a PID number.
use super::{owner_from_uid, parse_proc_status_uid, CoreProcess, ProcessHold};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fd::OwnedFd;
use rustix::fs::{openat, Mode, OFlags};
use rustix::process::{pidfd_open, pidfd_send_signal, Pid, PidfdFlags, Signal};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub(super) struct HeldProcess {
    pidfd: OwnedFd,
}

impl HeldProcess {
    pub(super) fn signal(&self, signal: crate::process_killer::Signal) -> bool {
        let signal = match signal {
            crate::process_killer::Signal::Sigterm => Signal::TERM,
            crate::process_killer::Signal::Sigkill => Signal::KILL,
        };
        pidfd_send_signal(&self.pidfd, signal).is_ok()
    }

    pub(super) fn is_running(&self) -> Option<bool> {
        let mut fds = [PollFd::new(&self.pidfd, PollFlags::IN)];
        poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        )
        .ok()?;
        let events = fds[0].revents();
        if events.intersects(PollFlags::ERR | PollFlags::NVAL) {
            return None;
        }
        if events.intersects(PollFlags::IN | PollFlags::HUP) {
            return Some(false);
        }
        Some(true)
    }
}

fn read_member(directory: &File, name: &str, limit: u64) -> Option<Vec<u8>> {
    let fd = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .ok()?;
    let mut bytes = Vec::new();
    File::from(fd)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// stat comm may contain spaces and ')'; fields after its final ')' start at state (3).
fn live_birth(stat: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(stat).ok()?;
    let fields: Vec<_> = text.rsplit_once(')')?.1.split_whitespace().collect();
    if matches!(*fields.first()?, "Z" | "X" | "x") {
        return None;
    }
    fields.get(19)?.parse().ok()
}

pub(super) fn capture(pid: u32, path: &Path, me: Option<u32>) -> Option<CoreProcess> {
    // An open /proc/PID directory keeps referring to that process, even after number reuse.
    // A final read through it cannot accidentally observe a replacement /proc/PID directory.
    let directory = File::open(path).ok()?;
    let before = read_member(&directory, "stat", 64 * 1024).and_then(|b| live_birth(&b));
    let bytes = read_member(&directory, "cmdline", 1024 * 1024)?;
    let cmdline: Vec<String> = bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    if cmdline.is_empty() {
        return None;
    }
    let owner = read_member(&directory, "status", 64 * 1024)
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| parse_proc_status_uid(&s));
    // Keep descriptors only for run -c candidates; unrelated system processes remain
    // inventory rows and cannot exhaust one pidfd per process during every startup.
    let potential_core = cmdline.get(1).is_some_and(|arg| arg == "run")
        && cmdline.get(2).is_some_and(|arg| arg == "-c")
        && owner_from_uid(owner, me).sweepable();
    let held = potential_core
        .then_some(pid)
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(Pid::from_raw)
        .and_then(|pid| pidfd_open(pid, PidfdFlags::empty()).ok())
        .and_then(|pidfd| {
            let after = read_member(&directory, "stat", 64 * 1024).and_then(|b| live_birth(&b));
            // Not merely two numeric-PID reads: the second read is relative to the original
            // proc directory *after* pidfd acquisition. If that process died, fail closed.
            if !matches!((before, after), (Some(a), Some(b)) if a == b) {
                return None;
            }
            let held = HeldProcess { pidfd };
            (held.is_running() == Some(true)).then(|| Arc::new(held))
        });
    Some(CoreProcess {
        pid,
        cmdline,
        owner: owner_from_uid(owner, me),
        hold: ProcessHold { held },
        ..CoreProcess::default()
    })
}

#[cfg(test)]
mod tests;

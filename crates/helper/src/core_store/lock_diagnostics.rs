//! Test-only observations of the original mutation File and matching descriptors.
//! No unlock, duplicate, flag change, subprocess, retry or lock-policy decision.
use super::{File, LockObservation};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::sync::OnceLock;
use std::thread::ThreadId;
use std::time::Instant;

const MAX_EVENTS: usize = 512;
const MAX_PROCESSES: usize = 64;
const MAX_FDS: usize = 512;
const MAX_PEERS: usize = 32;
const MAX_RACE_OBSERVATIONS: usize = 32;

// One epoch for all threads in this test process. Initialization does not hold
// a lock across any acquisition, spawn, observation or File close. PID remains
// part of every record: this epoch does not establish cross-process ordering.
static CLOCK: OnceLock<Instant> = OnceLock::new();
fn monotonic_ns() -> u128 {
    CLOCK.get_or_init(Instant::now).elapsed().as_nanos()
}

#[cfg(target_os = "macos")]
pub(super) fn spawn_origin(origin: &'static str, phase: &'static str, child_pid: Option<u32>) {
    use std::io::Write;
    let unix_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_nanos());
    let monotonic_ns = monotonic_ns();
    let correlation = crate::platform::macos::exec::test_spawn_correlation();
    // Source-owned category/PID/time only; never arguments, paths or env.
    // This is observation in the unit-test process, not production logging,
    // process authority or serialization of spawn and lock acquisition.
    let _ = writeln!(
        std::io::stderr().lock(),
        "TEST_SPAWN_ORIGIN origin={origin} phase={phase} parent_pid={} child_pid={child_pid:?} unix_ns={unix_ns:?} monotonic_ns={monotonic_ns} correlation={correlation:?} scope=test-process-only",
        std::process::id()
    );
}

#[derive(Default)]
struct History {
    events: VecDeque<ScopeEvent>,
    lost: u64,
}
thread_local! {
    static HISTORY: RefCell<History> = RefCell::new(History::default());
}

#[derive(Clone, Copy)]
struct ScopeEvent {
    phase: &'static str,
    pid: u32,
    thread: ThreadId,
    elapsed_ns: u128,
    fd: Option<i32>,
    dev: Option<u64>,
    inode: Option<u64>,
}
impl fmt::Debug for ScopeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopeEvent")
            .field("phase", &self.phase)
            .field("pid", &self.pid)
            .field("thread", &self.thread)
            .field("ns", &self.elapsed_ns)
            .field("fd", &self.fd)
            .field("dev", &self.dev)
            .field("inode", &self.inode)
            .finish()
    }
}
pub(super) fn record(phase: &'static str, observation: &LockObservation) {
    let event = ScopeEvent {
        phase,
        pid: std::process::id(),
        thread: std::thread::current().id(),
        elapsed_ns: monotonic_ns(),
        fd: observation.fd,
        dev: observation.dev,
        inode: observation.inode,
    };
    // Calling-thread events need no global lock and cannot serialize parallel
    // tests. Other threads' descriptors are covered by the native snapshot.
    HISTORY.with(|history| {
        let mut history = history.borrow_mut();
        if history.events.len() == MAX_EVENTS {
            history.events.pop_front();
            history.lost += 1;
        }
        history.events.push_back(event);
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    pid: u32,
    ppid: u32,
    start: [u64; 2],
}
struct Peer {
    identity: Identity,
    fd: i32,
    cloexec: Option<bool>,
    same_executable: Option<bool>,
    shared_description: Option<bool>,
    flock_write: Option<bool>,
    flock_acquirer_pid: Option<i32>,
}
impl fmt::Debug for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Peer")
            .field("identity", &self.identity)
            .field("fd", &self.fd)
            .field("cloexec", &self.cloexec)
            .field("same-executable", &self.same_executable)
            .field("shared-description", &self.shared_description)
            .field("flock-write", &self.flock_write)
            .field("flock-acquirer-pid", &self.flock_acquirer_pid)
            .finish()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RaceStage {
    ParentChanged,
    #[cfg(target_os = "linux")]
    DescriptorMetadata,
    #[cfg(target_os = "linux")]
    DescriptorIdentity,
    #[cfg(target_os = "macos")]
    DescriptorInfo,
    IdentityOrImageChanged,
}

#[derive(Default)]
struct Native {
    peers: Vec<Peer>,
    unavailable: usize,
    raced: usize,
    race_observations: Vec<(RaceStage, u32, Option<i32>)>,
    race_observations_lost: usize,
    truncated: bool,
}
impl Native {
    fn record_race(&mut self, stage: RaceStage, pid: u32, fd: Option<i32>) {
        self.raced += 1;
        if self.race_observations.len() < MAX_RACE_OBSERVATIONS {
            self.race_observations.push((stage, pid, fd));
        } else {
            self.race_observations_lost += 1;
        }
    }
}
pub(super) struct Snapshot {
    pid: u32,
    sample_start_ns: u128,
    sample_end_ns: u128,
    events: Vec<ScopeEvent>,
    events_lost: u64,
    native: Native,
}
impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{scope=self-and-direct-children,clock=process-monotonic,pid={},sample-start-ns={},sample-end-ns={},calling-thread-events={:?},events-lost={},peers={:?},native-unavailable={},native-raced={},native-race-observations={:?},native-race-observations-lost={},native-truncated={},claim=descriptor-observation-only}}",
            self.pid,
            self.sample_start_ns,
            self.sample_end_ns,
            self.events,
            self.events_lost,
            self.native.peers,
            self.native.unavailable,
            self.native.raced,
            self.native.race_observations,
            self.native.race_observations_lost,
            self.native.truncated
        )
    }
}
pub(super) fn snapshot(wanted: &LockObservation) -> Snapshot {
    let pid = std::process::id();
    let sample_start_ns = monotonic_ns();
    let (events, events_lost) = HISTORY.with(|history| {
        let history = history.borrow();
        let events = history
            .events
            .iter()
            .filter(|e| e.pid == pid && e.dev == wanted.dev && e.inode == wanted.inode)
            .copied()
            .collect();
        (events, history.lost)
    });
    let native = match (wanted.dev, wanted.inode) {
        (Some(dev), Some(inode)) => platform::collect(pid, dev, inode),
        _ => Native {
            unavailable: 1,
            ..Native::default()
        },
    };
    Snapshot {
        pid,
        sample_start_ns,
        sample_end_ns: monotonic_ns(),
        events,
        events_lost,
        native,
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::fs;
    use std::io::{self, Read};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn read(path: impl AsRef<Path>, limit: u64) -> io::Result<String> {
        let mut raw = Vec::new();
        File::open(path)?.take(limit + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 > limit {
            return Err(io::Error::other("diagnostic byte budget"));
        }
        String::from_utf8(raw).map_err(|_| io::Error::other("diagnostic encoding"))
    }
    fn identity(pid: u32) -> Option<Identity> {
        let raw = read(format!("/proc/{pid}/stat"), 8192).ok()?;
        let (prefix, rest) = raw.rsplit_once(") ")?;
        if prefix.split_once(" (")?.0.parse::<u32>().ok()? != pid {
            return None;
        }
        let fields: Vec<_> = rest.split_whitespace().collect();
        Some(Identity {
            pid,
            ppid: fields.get(1)?.parse().ok()?,
            start: [fields.get(19)?.parse().ok()?, 0],
        })
    }
    pub(super) fn flock(raw: &str, dev: u64, inode: u64) -> Option<(bool, i32)> {
        for line in raw.lines().filter(|line| line.starts_with("lock:")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 9 || fields[2] != "FLOCK" || fields[3] != "ADVISORY" {
                continue;
            }
            let key: Vec<_> = fields[6].split(':').collect();
            if key.len() != 3 {
                continue;
            }
            if u64::from_str_radix(key[0], 16).ok() != Some(nix::sys::stat::major(dev))
                || u64::from_str_radix(key[1], 16).ok() != Some(nix::sys::stat::minor(dev))
                || key[2].parse::<u64>().ok() != Some(inode)
            {
                continue;
            }
            let write = match fields[4] {
                "WRITE" => true,
                "READ" => false,
                _ => continue,
            };
            if let Ok(pid) = fields[5].parse() {
                return Some((write, pid));
            }
        }
        None
    }
    pub(super) fn collect(pid: u32, dev: u64, inode: u64) -> Native {
        let mut result = Native::default();
        let mut pids = vec![pid];
        match fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(tasks) => {
                for (count, task) in tasks.enumerate() {
                    if count == MAX_PROCESSES {
                        result.truncated = true;
                        break;
                    }
                    let children = task
                        .ok()
                        .and_then(|t| read(t.path().join("children"), 8192).ok());
                    let Some(children) = children else {
                        result.unavailable += 1;
                        continue;
                    };
                    for child in children
                        .split_whitespace()
                        .filter_map(|p| p.parse::<u32>().ok())
                    {
                        if pids.contains(&child) {
                            continue;
                        }
                        if pids.len() == MAX_PROCESSES {
                            result.truncated = true;
                            break;
                        }
                        pids.push(child);
                    }
                }
            }
            Err(_) => result.unavailable += 1,
        }
        let own_image = fs::read_link(format!("/proc/{pid}/exe")).ok();
        for child in pids {
            let Some(before) = identity(child) else {
                result.unavailable += 1;
                continue;
            };
            if child != pid && before.ppid != pid {
                result.record_race(RaceStage::ParentChanged, child, None);
                continue;
            }
            let Ok(fds) = fs::read_dir(format!("/proc/{child}/fd")) else {
                result.unavailable += 1;
                continue;
            };
            let image = fs::read_link(format!("/proc/{child}/exe")).ok();
            let first = result.peers.len();
            for (count, entry) in fds.enumerate() {
                if count == MAX_FDS {
                    result.truncated = true;
                    break;
                }
                let Some(entry) = entry.ok() else {
                    result.unavailable += 1;
                    continue;
                };
                let Some(fd) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<i32>().ok())
                else {
                    continue;
                };
                let Ok(meta) = fs::metadata(entry.path()) else {
                    result.record_race(RaceStage::DescriptorMetadata, child, Some(fd));
                    continue;
                };
                if meta.dev() != dev || meta.ino() != inode {
                    continue;
                }
                if result.peers.len() == MAX_PEERS {
                    result.truncated = true;
                    break;
                }
                let info = read(format!("/proc/{child}/fdinfo/{fd}"), 16384).ok();
                let Some(info) = info else {
                    result.unavailable += 1;
                    continue;
                };
                let actual_inode = info.lines().find_map(|l| {
                    l.strip_prefix("ino:")
                        .and_then(|v| v.trim().parse::<u64>().ok())
                });
                if actual_inode != Some(inode) {
                    result.record_race(RaceStage::DescriptorIdentity, child, Some(fd));
                    continue;
                }
                let flags = info.lines().find_map(|l| {
                    l.strip_prefix("flags:")
                        .and_then(|v| u64::from_str_radix(v.trim(), 8).ok())
                });
                let lock = flock(&info, dev, inode);
                let has_flock = info.lines().any(|line| {
                    line.starts_with("lock:")
                        && line.split_whitespace().any(|field| field == "FLOCK")
                });
                if has_flock && lock.is_none() {
                    result.unavailable += 1;
                }
                result.peers.push(Peer {
                    identity: before,
                    fd,
                    cloexec: flags.map(|v| v & nix::libc::O_CLOEXEC as u64 != 0),
                    same_executable: own_image.as_ref().zip(image.as_ref()).map(|(a, b)| a == b),
                    shared_description: None,
                    flock_write: match lock {
                        Some(value) => Some(value.0),
                        None if has_flock => None,
                        None => Some(false),
                    },
                    flock_acquirer_pid: lock.map(|v| v.1),
                });
            }
            if identity(child) != Some(before)
                || fs::read_link(format!("/proc/{child}/exe")).ok() != image
            {
                result.peers.truncate(first);
                result.record_race(RaceStage::IdentityOrImageChanged, child, None);
            }
        }
        result
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use nix::libc;
    use std::mem::{size_of, size_of_val, zeroed};
    // SDK proc_info.h: proc_fileinfo + vnode_info. This flavor returns no paths.
    #[repr(C)]
    struct FileInfo {
        _openflags: u32,
        status: u32,
        _offset: i64,
        _kind: i32,
        _guardflags: u32,
    }
    #[repr(C)]
    struct VnodeFd {
        file: FileInfo,
        vnode: libc::vnode_info,
    }
    #[allow(unsafe_code, reason = "test-only read-only libproc linkage")]
    #[link(name = "proc")]
    unsafe extern "C" {}

    #[allow(unsafe_code, reason = "test-only bounded native PID/FD observation")]
    pub(super) fn collect(pid: u32, dev: u64, inode: u64) -> Native {
        // SAFETY: libproc writes only into correctly aligned repr(C) or libc
        // numeric buffers with their exact capacities. Every returned length is
        // checked before use. No descriptor is opened, duplicated or changed.
        unsafe {
            fn identity(pid: u32) -> Option<Identity> {
                // SAFETY: proc_bsdinfo is the SDK layout, all fields admit zero.
                unsafe {
                    let mut info: libc::proc_bsdinfo = zeroed();
                    if libc::proc_pidinfo(
                        pid as i32,
                        libc::PROC_PIDTBSDINFO,
                        0,
                        (&mut info as *mut libc::proc_bsdinfo).cast(),
                        size_of::<libc::proc_bsdinfo>() as i32,
                    ) != size_of::<libc::proc_bsdinfo>() as i32
                        || info.pbi_pid != pid
                    {
                        return None;
                    }
                    Some(Identity {
                        pid,
                        ppid: info.pbi_ppid,
                        start: [info.pbi_start_tvsec, info.pbi_start_tvusec],
                    })
                }
            }
            fn image(pid: u32) -> Option<Vec<u8>> {
                // SAFETY: the bounded output buffer meets libproc path size.
                unsafe {
                    let mut raw = [0u8; 4096];
                    let n =
                        libc::proc_pidpath(pid as i32, raw.as_mut_ptr().cast(), raw.len() as u32);
                    if n <= 0 || n as usize >= raw.len() {
                        return None;
                    }
                    Some(raw[..n as usize].to_vec())
                }
            }
            let mut result = Native::default();
            let mut children = [0i32; MAX_PROCESSES];
            let n = libc::proc_listchildpids(
                pid as i32,
                children.as_mut_ptr().cast(),
                size_of_val(&children) as i32,
            );
            let mut pids = vec![pid];
            if n <= 0 || n as usize > children.len() {
                result.unavailable += 1;
            } else {
                result.truncated |= n as usize == children.len();
                pids.extend(
                    children[..n as usize]
                        .iter()
                        .filter(|v| **v > 0)
                        .take(MAX_PROCESSES - 1)
                        .map(|v| *v as u32),
                );
            }
            let own_image = image(pid);
            for child in pids {
                let Some(before) = identity(child) else {
                    result.unavailable += 1;
                    continue;
                };
                if child != pid && before.ppid != pid {
                    result.record_race(RaceStage::ParentChanged, child, None);
                    continue;
                }
                let child_image = image(child);
                let mut fds: [libc::proc_fdinfo; MAX_FDS] =
                    std::array::from_fn(|_| libc::proc_fdinfo {
                        proc_fd: 0,
                        proc_fdtype: 0,
                    });
                let n = libc::proc_pidinfo(
                    child as i32,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    size_of_val(&fds) as i32,
                );
                if n <= 0
                    || n as usize > size_of_val(&fds)
                    || !(n as usize).is_multiple_of(size_of::<libc::proc_fdinfo>())
                {
                    result.unavailable += 1;
                    continue;
                }
                result.truncated |= n as usize == size_of_val(&fds);
                let first = result.peers.len();
                for fd in &fds[..n as usize / size_of::<libc::proc_fdinfo>()] {
                    if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
                        continue;
                    }
                    let mut info: VnodeFd = zeroed();
                    if libc::proc_pidfdinfo(
                        child as i32,
                        fd.proc_fd,
                        1,
                        (&mut info as *mut VnodeFd).cast(),
                        size_of::<VnodeFd>() as i32,
                    ) != size_of::<VnodeFd>() as i32
                    {
                        result.record_race(RaceStage::DescriptorInfo, child, Some(fd.proc_fd));
                        continue;
                    }
                    if u64::from(info.vnode.vi_stat.vst_dev) != dev
                        || info.vnode.vi_stat.vst_ino != inode
                    {
                        continue;
                    }
                    if result.peers.len() == MAX_PEERS {
                        result.truncated = true;
                        break;
                    }
                    result.peers.push(Peer {
                        identity: before,
                        fd: fd.proc_fd,
                        cloexec: Some(info.file.status & 2 != 0), // PROC_FP_CLEXEC
                        same_executable: own_image
                            .as_ref()
                            .zip(child_image.as_ref())
                            .map(|(a, b)| a == b),
                        shared_description: Some(info.file.status & 1 != 0), // PROC_FP_SHARED
                        flock_write: None,
                        flock_acquirer_pid: None,
                    }); // libproc does not report flock ownership.
                }
                if identity(child) != Some(before) || image(child) != child_image {
                    result.peers.truncate(first);
                    result.record_race(RaceStage::IdentityOrImageChanged, child, None);
                }
            }
            result
        }
    }
}

#[cfg(test)]
mod tests;

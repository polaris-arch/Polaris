use super::*;
use crate::core_store::{CoreStore, LockFailure, OwnerPolicy};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn store() -> (tempfile::TempDir, CoreStore) {
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let store = CoreStore::initialize(root.path(), OwnerPolicy { uid: Some(uid) }).unwrap();
    (root, store)
}
fn failure(store: &CoreStore) -> io::Error {
    match store.mutation_lock() {
        Ok(_) => panic!("expected one nonblocking mutation-lock contention"),
        Err(error) => error,
    }
}
fn observed(error: &io::Error) -> &Snapshot {
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<LockFailure>()
        .unwrap()
        .holder_snapshot
        .as_ref()
        .unwrap()
}

#[test]
fn same_process_holder_and_parent_file_close_have_distinct_original_observations() {
    let (_root, store) = store();
    let owner = store.mutation_lock().unwrap();
    let fd = owner.file.as_ref().unwrap().as_raw_fd();
    let flags =
        nix::fcntl::fcntl(owner.file.as_ref().unwrap(), nix::fcntl::FcntlArg::F_GETFD).unwrap();
    let error = failure(&store);
    let snapshot = observed(&error);
    assert!(
        snapshot
            .events
            .iter()
            .any(|e| e.phase == "acquired" && e.fd == Some(fd))
    );
    assert!(
        snapshot
            .native
            .peers
            .iter()
            .any(|p| p.identity.pid == std::process::id() && p.fd == fd)
    );
    assert!(
        snapshot
            .native
            .peers
            .iter()
            .filter(|p| p.fd == fd)
            .all(|p| p.identity.start[0] > 0)
    );
    #[cfg(target_os = "linux")]
    assert!(
        snapshot
            .native
            .peers
            .iter()
            .any(|p| p.fd == fd && p.flock_write == Some(true))
    );
    assert_eq!(
        nix::fcntl::fcntl(owner.file.as_ref().unwrap(), nix::fcntl::FcntlArg::F_GETFD).unwrap(),
        flags
    );
    assert!(!error.to_string().contains(_root.path().to_str().unwrap()));
    drop(owner);
    // This is a separately opened description and a new scope, not retrying
    // a failed install or converting the holder's shared/exclusive mode.
    let next = store.mutation_lock().unwrap();
    let snapshot = super::snapshot(&next.observation);
    assert!(
        snapshot
            .events
            .iter()
            .any(|e| e.phase == "parent-file-closed" && e.fd == Some(fd))
    );
}

struct ChildCleanup(Option<i32>);
impl Drop for ChildCleanup {
    fn drop(&mut self) {
        // This guard owns the unreaped child. Even assertion failure releases
        // the controlled barrier; no detached thread or external PID is killed.
        let Some(raw) = self.0 else {
            return;
        };
        let pid = nix::unistd::Pid::from_raw(raw);
        if matches!(
            nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
            Ok(nix::sys::wait::WaitStatus::StillAlive)
        ) {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = nix::sys::wait::waitpid(pid, None);
        }
    }
}

#[test]
#[allow(
    unsafe_code,
    reason = "controlled fork-only diagnostic; child uses async-signal-safe syscalls"
)]
fn inherited_cloexec_description_remains_observable_after_parent_file_closes() {
    const PROBE: &str = "POLARIS_LOCK_DIAGNOSTIC_PROBE";
    if std::env::var(PROBE).as_deref() != Ok("isolated-v1") {
        // The deliberate fork barrier must not inherit and prolong unrelated
        // parallel unit tests' descriptors. Exec the exact control first; its
        // later fork sees only this isolated fixture, not the full test cohort.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "core_store::lock_diagnostics::tests::inherited_cloexec_description_remains_observable_after_parent_file_closes", "--nocapture"])
            .env(PROBE, "isolated-v1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap(); // output() concurrently drains both captured streams.
        eprintln!(
            "isolated-lock-control stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "isolated native inheritance control failed"
        );
        return;
    }
    // SAFETY: only the isolated control process receives this deadline. The
    // real parent test process and the rest of the unit cohort are untouched.
    unsafe {
        nix::libc::alarm(20);
    }
    let (_root, store) = store();
    let owner = store.mutation_lock().unwrap();
    let fd = owner.file.as_ref().unwrap().as_raw_fd();
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    parent
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let child_fd = child.as_raw_fd();
    // SAFETY: in the child only libc fcntl/write/read/_exit run; no allocator,
    // formatting, mutex, Rust Drop, privilege/device/network operation or exec.
    // The one-byte barrier holds the descriptor between fork and exec/exit.
    let pid = unsafe { nix::libc::fork() };
    assert!(pid >= 0, "controlled fork failed");
    if pid == 0 {
        unsafe {
            nix::libc::alarm(10); // Bound the controlled child even if parent cleanup cannot observe it.
            let flags = nix::libc::fcntl(fd, nix::libc::F_GETFD);
            let ready = [u8::from(flags >= 0 && flags & nix::libc::FD_CLOEXEC != 0)];
            if nix::libc::write(child_fd, ready.as_ptr().cast(), 1) != 1 {
                nix::libc::_exit(91);
            }
            let mut release = [0u8];
            if nix::libc::read(child_fd, release.as_mut_ptr().cast(), 1) != 1 {
                nix::libc::_exit(92);
            }
            nix::libc::_exit(0);
        }
    }
    let mut cleanup = ChildCleanup(Some(pid));
    drop(child);
    let mut ready = [0u8];
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(
        ready,
        [1],
        "actual inherited descriptor must retain CLOEXEC before exec"
    );
    drop(owner);
    let error = failure(&store);
    let snapshot = observed(&error);
    eprintln!("controlled-fork-only-child-pid={pid} {error}");
    assert!(
        snapshot
            .events
            .iter()
            .any(|e| e.phase == "parent-file-closed" && e.fd == Some(fd))
    );
    assert!(
        snapshot
            .native
            .peers
            .iter()
            .any(|p| p.identity.pid == pid as u32
                && p.fd == fd
                && p.cloexec == Some(true)
                && p.same_executable == Some(true))
    );
    #[cfg(target_os = "linux")]
    {
        assert!(
            snapshot
                .native
                .peers
                .iter()
                .any(|p| p.identity.pid == pid as u32 && p.flock_write == Some(true))
        );
        // The contender may reuse the closed parent's descriptor number. Its
        // open description must not be mislabelled as the original lock holder.
        assert!(
            !snapshot
                .native
                .peers
                .iter()
                .any(|p| p.identity.pid == std::process::id() && p.flock_write == Some(true))
        );
    }
    parent.write_all(&[1]).unwrap();
    // Wait/reap belongs to normal control cleanup, not an acquisition retry.
    let status = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).unwrap();
    cleanup.0 = None; // Reaped regardless of status; never signal a potentially reused PID.
    assert!(matches!(status, nix::sys::wait::WaitStatus::Exited(_, 0)));
    let _new_scope = store.mutation_lock().unwrap();
}

#[test]
fn diagnostic_missing_identity_remains_explicitly_unknown() {
    let observation = LockObservation::default();
    let snapshot = snapshot(&observation);
    assert_eq!(snapshot.native.unavailable, 1);
    assert!(snapshot.native.peers.is_empty());
    let formatted = snapshot.to_string();
    assert!(formatted.contains("claim=descriptor-observation-only"));
    assert!(formatted.contains("native-unavailable=1"));
}

#[cfg(target_os = "linux")]
#[test]
fn flock_fields_require_exact_native_kind_device_inode_and_closed_mode() {
    let dev = nix::sys::stat::makedev(8, 1);
    let raw =
        "pos:\t0\nflags:\t02100002\nino:\t99\nlock:\t1: FLOCK ADVISORY WRITE 123 08:01:99 0 EOF\n";
    assert_eq!(platform::flock(raw, dev, 99), Some((true, 123)));
    for wrong in [
        raw.replace("FLOCK", "POSIX"),
        raw.replace("08:01", "08:02"),
        raw.replace(":99", ":100"),
        raw.replace("WRITE", "arbitrary-secret-name"),
        raw.replace("123", "not-a-pid"),
    ] {
        assert_eq!(platform::flock(&wrong, dev, 99), None);
    }
    assert_eq!(
        platform::flock(&raw.replace("WRITE", "READ"), dev, 99),
        Some((false, 123))
    );
}

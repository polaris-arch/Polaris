#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_tests {
    use super::super::{linux, CandidateCheckError};
    use crate::runtime::proxy::mesh_apply::file_snapshot::FileSnapshot;
    use crate::test_support::TestDir;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    struct ReapedChild(std::process::Child);

    impl Drop for ReapedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_for_ready(marker: &Path, timeout: Duration) -> std::io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match fs::read(marker) {
                Ok(bytes) if bytes == b"ready" => return Ok(()),
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "shell did not acknowledge completed exec",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn spawn_ready_child(stdin: Stdio) -> (TestDir, ReapedChild) {
        let dir = TestDir::new("polaris-check-fd-probe-");
        let marker = dir.join("ready");
        let child = ReapedChild(
            Command::new("/bin/sh")
                .args(["-c", "printf ready > \"$1\" || exit 1; exec sleep 5", "sh"])
                .arg(&marker)
                .stdin(stdin)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // A vfork-based spawn may return while exec is still closing CLOEXEC
        // descriptors. Only code running in the new shell can write this marker.
        wait_for_ready(&marker, Duration::from_secs(2)).unwrap();
        (dir, child)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct InheritedConfig {
        fd: u32,
        identity: (u64, u64),
        bytes_sha256: String,
    }

    fn inherited_configs(child: &ReapedChild) -> Vec<InheritedConfig> {
        let fd_dir = format!("/proc/{}/fd", child.0.id());
        let mut inherited = Vec::new();
        for entry in fs::read_dir(fd_dir).unwrap() {
            let path = entry.unwrap().path();
            let target = match fs::read_link(&path) {
                Ok(target) => target,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!(
                    "cannot inspect child descriptor {}: {error}",
                    path.display()
                ),
            };
            // Keep the original all-config gate, including another test's file.
            if target
                .to_string_lossy()
                .contains("polaris-strict-check-config")
            {
                let file = fs::File::open(&path).unwrap();
                let metadata = file.metadata().unwrap();
                let bytes_sha256 =
                    polaris_updater::verify::sha256_reader_hex(&mut file.take(8 * 1024 * 1024 + 1))
                        .unwrap();
                inherited.push(InheritedConfig {
                    fd: path.file_name().unwrap().to_str().unwrap().parse().unwrap(),
                    identity: (metadata.dev(), metadata.ino()),
                    bytes_sha256,
                });
            }
        }
        inherited
    }

    fn assert_no_inherited_configs(child: &ReapedChild) {
        let inherited = inherited_configs(child);
        assert!(
            inherited.is_empty(),
            "inherited sealed configs: {inherited:?}"
        );
    }

    fn pinned_binary() -> Option<PathBuf> {
        match std::env::var_os("POLARIS_TEST_CORE") {
            Some(path) => Some(PathBuf::from(path)),
            None if std::env::var_os("POLARIS_REQUIRE_KERNEL_GATE").is_some() => {
                panic!("POLARIS_TEST_CORE required for pinned strict-check tests")
            }
            None => None,
        }
    }

    #[test]
    fn sealed_config_disallows_writes_after_hashing() {
        let mut config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        config.seek(SeekFrom::Start(0)).unwrap();
        assert!(config.write_all(b"x").is_err());
        assert!(config.set_len(0).is_err());
    }

    #[test]
    fn unrelated_spawn_does_not_inherit_sealed_config() {
        use nix::fcntl::{fcntl, FcntlArg};

        let config = linux::sealed_config(b"{\"fixture\":\"owned\"}").unwrap();
        assert_ne!(
            fcntl(&config, FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
            0
        );
        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            scope.spawn(move || {
                let other = linux::sealed_config(b"{\"fixture\":\"parallel\"}").unwrap();
                let copies: Vec<_> = (0..64)
                    .map(|_| rustix::io::fcntl_dupfd_cloexec(&other, 3).unwrap())
                    .collect();
                for fd in &copies {
                    assert_ne!(
                        fcntl(fd, FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
                        0
                    );
                }
                ready_tx.send(()).unwrap();
                // Dropping release_tx during a panic also releases this worker.
                let _ = release_rx.recv();
            });
            ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let (_dir, child) = spawn_ready_child(Stdio::null());
            assert_no_inherited_configs(&child);
            release_tx.send(()).unwrap();
        });
    }

    #[test]
    fn fd_probe_rejects_owned_and_other_configs_after_exec() {
        let owned = linux::sealed_config(b"{\"fixture\":\"owned\"}").unwrap();
        let other = linux::sealed_config(b"{\"fixture\":\"other\"}").unwrap();
        assert_ne!(
            owned.metadata().unwrap().ino(),
            other.metadata().unwrap().ino()
        );
        for (config, bytes) in [
            (&owned, b"{\"fixture\":\"owned\"}".as_slice()),
            (&other, b"{\"fixture\":\"other\"}".as_slice()),
        ] {
            let stdin = fs::File::from(rustix::io::fcntl_dupfd_cloexec(config, 3).unwrap());
            let (_dir, child) = spawn_ready_child(Stdio::from(stdin));
            let metadata = config.metadata().unwrap();
            assert_eq!(
                inherited_configs(&child),
                vec![InheritedConfig {
                    fd: 0,
                    identity: (metadata.dev(), metadata.ino()),
                    bytes_sha256: polaris_updater::verify::sha256_hex(bytes),
                }]
            );
            let rejection = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_no_inherited_configs(&child);
            }));
            assert!(
                rejection.is_err(),
                "the all-config gate must reject either inode"
            );
        }
    }

    #[test]
    fn fd_probe_readiness_timeout_is_bounded_and_reaps_child() {
        let dir = TestDir::new("polaris-check-fd-timeout-");
        let child = ReapedChild(
            Command::new("sleep")
                .arg("5")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id();
        let started = Instant::now();
        assert_eq!(
            wait_for_ready(&dir.join("missing"), Duration::from_millis(20))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(child);
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }

    #[test]
    fn fd_probe_reaps_child_during_assertion_unwind() {
        let mut pid = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_dir, child) = spawn_ready_child(Stdio::null());
            pid = Some(child.0.id());
            panic!("exercise FD probe cleanup");
        }));
        assert!(result.is_err());
        assert!(!PathBuf::from(format!("/proc/{}", pid.unwrap())).exists());
    }

    #[test]
    fn sealed_binary_stat_fifo_replacement_is_bounded_without_writer() {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = TestDir::new("polaris-check-stat-fifo-");
        let source = dir.join("core");
        fs::write(&source, b"original regular bytes").unwrap();
        let worker_path = source.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = linux::sealed_binary_with_stat_hook(&worker_path, || {
                fs::remove_file(&worker_path).unwrap();
                nix::unistd::mkfifo(
                    &worker_path,
                    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
                )
                .unwrap();
                ready_tx.send(()).unwrap();
            })
            .map(|_| ());
            result_tx.send(result).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(1));
        // A writer exists only to settle a regressed blocking open after the
        // rejection deadline has already failed, never on the passing path.
        let _unblock_on_failure = result.is_err().then(|| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
                .open(&source)
                .unwrap()
        });
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Err(CandidateCheckError::Unsupported));
    }

    #[tokio::test]
    async fn timeout_reaps_without_publishing_any_artifact() {
        let Some(binary_path) = pinned_binary() else {
            return;
        };
        let dir = TestDir::new("polaris-check-timeout-");
        let (binary, _) = linux::sealed_binary(&binary_path).unwrap();
        let config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        assert_eq!(
            linux::run_check(binary, config, Duration::ZERO).await,
            Err(CandidateCheckError::TimedOut)
        );
        assert!(!dir.join("mesh-routes").exists());
    }

    #[tokio::test]
    async fn sealed_binary_execution_survives_source_path_replacement_but_identity_detects_it() {
        let Some(binary_path) = pinned_binary() else {
            return;
        };
        let dir = TestDir::new("polaris-check-source-swap-");
        let source = dir.join("core");
        fs::hard_link(&binary_path, &source).unwrap_or_else(|_| {
            fs::copy(&binary_path, &source).unwrap();
        });
        let (binary, before) = linux::sealed_binary(&source).unwrap();
        let replacement = dir.join("replacement");
        fs::write(&replacement, b"wrong binary").unwrap();
        fs::rename(&replacement, &source).unwrap();
        let config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        assert_eq!(
            linux::run_check(binary, config, Duration::from_secs(8)).await,
            Ok(())
        );
        let after = FileSnapshot::path(&source).unwrap();
        assert!(!before.same_snapshot(&after));
        assert!(!dir.join("mesh-routes").exists());
    }

    #[tokio::test]
    async fn closed_standard_descriptors_still_execute_the_sealed_binary() {
        let Some(binary_path) = pinned_binary() else {
            return;
        };
        if let Some(marker) = std::env::var_os("POLARIS_CLOSED_STD_CHILD_MARKER") {
            let (binary, _) = linux::sealed_binary(&binary_path).unwrap();
            let config =
                linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                    .unwrap();
            linux::run_check(binary, config, Duration::from_secs(8))
                .await
                .unwrap();
            fs::write(marker, b"checked with 0/1/2 closed").unwrap();
            return;
        }
        let dir = TestDir::new("polaris-check-closed-std-");
        let marker = dir.join("checked");
        let test_name = "runtime::proxy::mesh_apply::candidate::check::tests::linux_tests::closed_standard_descriptors_still_execute_the_sealed_binary";
        let status = Command::new("sh")
            .arg("-c")
            .arg("exec 0<&- 1>&- 2>&-; exec \"$1\" --exact \"$2\" --nocapture")
            .arg("sh")
            .arg(std::env::current_exe().unwrap())
            .arg(test_name)
            .env("POLARIS_CLOSED_STD_CHILD_MARKER", &marker)
            .status()
            .unwrap();
        assert_eq!(fs::read(marker).unwrap(), b"checked with 0/1/2 closed");
        assert!(status.success());
    }

    fn sealed_fixture_binary(path: &str) -> fs::File {
        use nix::fcntl::{fcntl, FcntlArg, SealFlag};
        use nix::sys::memfd::{memfd_create, MFdFlags};
        let actual = fs::canonicalize(path).unwrap();
        let bytes =
            polaris_core_supervisor::exact_spawn::read_linux_binary_source(&actual).unwrap();
        let mut file = fs::File::from(
            memfd_create(
                "polaris-check-no-network-fixture",
                MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
            )
            .unwrap(),
        );
        file.write_all(&bytes).unwrap();
        nix::sys::stat::fchmod(
            &file,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IXUSR,
        )
        .unwrap();
        fcntl(
            &file,
            FcntlArg::F_ADD_SEALS(
                SealFlag::F_SEAL_WRITE
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_SEAL,
            ),
        )
        .unwrap();
        fs::File::from(rustix::io::fcntl_dupfd_cloexec(&file, 3).unwrap())
    }

    #[tokio::test]
    async fn caller_cancellation_before_and_after_spawn_leaves_no_child() {
        let dir = TestDir::new("polaris-check-cancel-");
        let marker = dir.join("should-not-exist");
        let binary = sealed_fixture_binary("/bin/sh");
        let mut before =
            tokio::process::Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        before.arg("-c").arg("touch \"$1\"").arg("sh").arg(&marker);
        let config = linux::sealed_config(b"{}").unwrap();
        let (notifier, notified) = tokio::sync::oneshot::channel();
        let never_polled = linux::supervise(
            before,
            binary,
            config,
            Duration::from_secs(8),
            Some(notifier),
        );
        drop(never_polled);
        assert!(notified.await.is_err());
        assert!(!marker.exists());

        let binary = sealed_fixture_binary("/bin/sleep");
        let mut after =
            tokio::process::Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        // Rust coreutils is a multicall ELF on this host; preserve the original
        // sleep fixture's argv0. This never alters the strict production profile.
        after.as_std_mut().arg0("sleep");
        after
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let config = linux::sealed_config(b"{}").unwrap();
        let (notifier, notified) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(linux::supervise(
            after,
            binary,
            config,
            Duration::from_secs(8),
            Some(notifier),
        ));
        let pid = notified.await.unwrap();
        assert!(PathBuf::from(format!("/proc/{pid}")).exists());
        caller.abort();
        tokio::time::timeout(Duration::from_secs(3), async {
            while PathBuf::from(format!("/proc/{pid}")).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled child must be killed and reaped");
        assert!(caller.await.unwrap_err().is_cancelled());
    }
}

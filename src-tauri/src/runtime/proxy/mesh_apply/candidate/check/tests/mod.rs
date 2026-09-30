#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_tests {
    use super::super::{linux, CandidateCheckError};
    use crate::runtime::proxy::mesh_apply::file_snapshot::FileSnapshot;
    use crate::test_support::TestDir;
    use std::fs;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::Duration;

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
        let _config = linux::sealed_config(b"{}").unwrap();
        let mut child = Command::new("sleep")
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let fd_dir = format!("/proc/{}/fd", child.id());
        for entry in fs::read_dir(fd_dir).unwrap() {
            let target = fs::read_link(entry.unwrap().path()).unwrap();
            assert!(!target
                .to_string_lossy()
                .contains("polaris-strict-check-config"));
        }
        child.kill().unwrap();
        child.wait().unwrap();
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

    #[tokio::test]
    async fn caller_cancellation_before_and_after_spawn_leaves_no_child() {
        let dir = TestDir::new("polaris-check-cancel-");
        let marker = dir.join("should-not-exist");
        let mut before = tokio::process::Command::new("sh");
        before.arg("-c").arg("touch \"$1\"").arg("sh").arg(&marker);
        let binary = linux::sealed_config(b"{}").unwrap();
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

        let mut after = tokio::process::Command::new("sleep");
        after
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let binary = linux::sealed_config(b"{}").unwrap();
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

#[cfg(unix)]
use super::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicU8;

    struct Fixture(PathBuf, PathBuf);
    impl Fixture {
        fn new(script: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "polaris-check-custody-{}-{}",
                std::process::id(),
                NEXT_REQUEST.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("config.json"), b"{\"secret\":\"custody-test\"}").unwrap();
            // These closed helpers are never opened for writing while tests spawn.
            let helper = match script {
                "exec /bin/sleep 2" => "sleep-two-seconds.sh",
                "exit 0" => "exit-success.sh",
                "exit 7" => "exit-seven.sh",
                _ => panic!("unknown custody fixture script"),
            };
            let binary = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/config_gate/check_custody/tests/fixtures")
                .join(helper);
            Self(path, binary)
        }
        fn binary(&self) -> PathBuf {
            self.1.clone()
        }
        fn config(&self) -> PathBuf {
            self.0.join("config.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct FaultIo {
        mode: AtomicU8,
        waits: AtomicUsize,
        signals: AtomicUsize,
    }
    impl FaultIo {
        fn new(mode: u8) -> Self {
            Self {
                mode: AtomicU8::new(mode),
                waits: AtomicUsize::new(0),
                signals: AtomicUsize::new(0),
            }
        }
    }
    impl CheckIo for FaultIo {
        fn poll_wait(
            &self,
            child: &mut Child,
            cx: &mut Context<'_>,
        ) -> Poll<io::Result<ExitStatus>> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            match self.mode.load(Ordering::SeqCst) {
                1 => Poll::Pending,
                2 => Poll::Ready(Err(io::Error::other("injected native wait error"))),
                3 => Poll::Ready(Err(io::Error::from_raw_os_error(10))),
                _ => NativeIo.poll_wait(child, cx),
            }
        }
        fn start_kill(&self, child: &mut Child) -> io::Result<()> {
            self.signals.fetch_add(1, Ordering::SeqCst);
            if self.mode.load(Ordering::SeqCst) == 1 {
                Ok(())
            } else {
                child.start_kill()
            }
        }
    }

    #[tokio::test]
    async fn cancellation_keeps_child_private_secret_snapshot_and_blocks_new_check_until_retry() {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(1));
        let binary = fixture.binary();
        let config = fixture.config();
        let mut check = Box::pin(custody.run(&binary, &config, Duration::from_secs(3), io.clone()));
        let first_poll = poll_fn(|cx| Poll::Ready(check.as_mut().poll(cx))).await;
        assert!(
            first_poll.is_pending(),
            "expected pending custody check, got {first_poll:?}"
        );
        let (request, snapshot) = {
            let state = custody.state.lock().unwrap();
            let (id, run) = state.runs.iter().next().unwrap();
            assert!(run.child.is_some());
            assert_eq!(
                std::fs::metadata(&run.snapshot)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            (*id, run.snapshot.clone())
        };
        drop(check);
        std::fs::remove_file(&config).unwrap();
        assert_eq!(
            std::fs::read(&snapshot).unwrap(),
            b"{\"secret\":\"custody-test\"}"
        );
        assert!(custody
            .run(&binary, &config, Duration::from_secs(1), Arc::new(NativeIo))
            .await
            .is_err());
        assert_eq!(custody.state.lock().unwrap().runs.len(), 1);
        io.mode.store(0, Ordering::SeqCst);
        custody.close_confirmed(request).await.unwrap();
        assert!(custody.state.lock().unwrap().runs.is_empty());
        assert!(!snapshot.exists());
    }

    #[tokio::test]
    async fn native_wait_error_retains_birth_and_generic_error_can_retry_the_owned_child() {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(2));
        assert!(custody
            .run(
                &fixture.binary(),
                &fixture.config(),
                Duration::from_secs(1),
                io.clone()
            )
            .await
            .is_err());
        let request = *custody.state.lock().unwrap().runs.keys().next().unwrap();
        assert!(custody.assert_admission().is_err());
        assert!(custody.state.lock().unwrap().runs[&request]
            .snapshot
            .exists());
        io.mode.store(0, Ordering::SeqCst);
        custody.close_confirmed(request).await.unwrap();
        custody.assert_admission().unwrap();
    }

    #[tokio::test]
    async fn echild_is_sticky_and_never_calls_native_wait_or_signal_again() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(0));
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), io.clone())
            .unwrap()
            .unwrap();
        let pid = custody.state.lock().unwrap().runs[&request]
            .child
            .as_ref()
            .unwrap()
            .id()
            .unwrap();
        assert!(matches!(
            nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None).unwrap(),
            nix::sys::wait::WaitStatus::Exited(_, 0)
        ));
        assert!(
            custody.wait_exit(request).await.is_err(),
            "Tokio must receive real ECHILD after an external native reap"
        );
        let waits = io.waits.load(Ordering::SeqCst);
        assert_eq!(io.signals.load(Ordering::SeqCst), 0);
        io.mode.store(0, Ordering::SeqCst);
        assert!(custody.close_confirmed(request).await.is_err());
        assert!(custody.shutdown().await.is_err());
        assert_eq!(io.waits.load(Ordering::SeqCst), waits);
        assert_eq!(io.signals.load(Ordering::SeqCst), 0);
        assert!(custody.state.lock().unwrap().runs[&request]
            .snapshot
            .exists());
    }

    #[tokio::test]
    async fn cleanup_failure_after_cached_exit_blocks_replacement_and_retries_only_the_tail() {
        let fixture = Fixture::new("exit 7");
        let custody = Arc::new(CheckCustody::default());
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        assert!(!custody.wait_exit(request).await.unwrap().success());
        let snapshot = custody.state.lock().unwrap().runs[&request]
            .snapshot
            .clone();
        std::fs::remove_file(&snapshot).unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        assert!(custody.retire(request).is_err());
        assert!(custody.assert_admission().is_err());
        assert!(custody.state.lock().unwrap().runs[&request].child.is_some());
        std::fs::remove_dir(&snapshot).unwrap();
        custody.close_confirmed(request).await.unwrap();
        custody.assert_admission().unwrap();
    }

    #[tokio::test]
    async fn closing_fences_late_spawn_and_shutdown_drains_every_registered_birth() {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        for _ in 0..2 {
            custody
                .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
                .unwrap()
                .unwrap();
        }
        custody.begin_shutdown().unwrap();
        assert!(matches!(
            custody.spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo)),
            Err(ValidationLifecycleError::Closing)
        ));
        custody.shutdown().await.unwrap();
        assert!(custody.state.lock().unwrap().runs.is_empty());
        assert!(matches!(
            custody.assert_admission(),
            Err(ValidationLifecycleError::Closing)
        ));
    }

    #[tokio::test]
    async fn old_request_cleanup_cannot_clear_a_new_birth_and_no_child_spawn_failure_retires_copy()
    {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let missing = fixture.0.join("missing");
        assert!(matches!(
            custody
                .run(
                    &missing,
                    &fixture.config(),
                    Duration::from_secs(1),
                    Arc::new(NativeIo)
                )
                .await
                .unwrap(),
            RawCheck::SpawnFailed(_)
        ));
        assert!(custody.state.lock().unwrap().runs.is_empty());
        let (old, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.close_confirmed(old).await.unwrap();
        let (new, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.request_close(old).unwrap();
        custody.retire(old).unwrap();
        assert!(custody.state.lock().unwrap().runs.contains_key(&new));
        custody.close_confirmed(new).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn close_wait_timeout_keeps_original_child_snapshot_and_debt_until_native_retry() {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(1));
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), io.clone())
            .unwrap()
            .unwrap();
        assert!(custody.close_confirmed(request).await.is_err());
        {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request];
            assert!(run.child.is_some() && run.exit.is_none() && run.debt && run.snapshot.exists());
        }
        assert!(custody.assert_admission().is_err());
        tokio::time::resume();
        io.mode.store(0, Ordering::SeqCst);
        custody.settle_debts().await.unwrap();
        assert!(custody.state.lock().unwrap().runs.is_empty());
        custody.assert_admission().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn queued_cleanup_has_one_total_budget_and_cancelled_settle_preserves_debt() {
        let fixture = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(1));
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), io.clone())
            .unwrap()
            .unwrap();
        custody.request_close(request).unwrap();
        let held = custody.cleanup_gate.lock().await;
        assert!(custody
            .settle_debts()
            .await
            .unwrap_err()
            .to_string()
            .contains("budget expired"));
        assert!(!custody.state.lock().unwrap().settling);
        assert!(custody.state.lock().unwrap().runs[&request].debt);
        drop(held);
        let mut settle = Box::pin(custody.settle_debts());
        assert!(poll_fn(|cx| Poll::Ready(settle.as_mut().poll(cx).is_pending())).await);
        assert!(custody.state.lock().unwrap().settling);
        drop(settle);
        assert!(!custody.state.lock().unwrap().settling);
        assert!(custody.state.lock().unwrap().runs[&request].child.is_some());
        assert!(custody.assert_admission().is_err());
        tokio::time::resume();
        io.mode.store(0, Ordering::SeqCst);
        custody.settle_debts().await.unwrap();
    }

    #[tokio::test]
    async fn poisoned_registry_never_mints_empty_admission_retirement_or_exit() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.wait_exit(request).await.unwrap();
        let poisoned = Arc::clone(&custody);
        assert!(std::thread::spawn(move || {
            let _state = poisoned.state.lock().unwrap();
            panic!("poison registry");
        })
        .join()
        .is_err());
        assert!(custody.assert_admission().is_err());
        assert!(custody.retire(request).is_err());
        assert!(custody.shutdown().await.is_err());
        let state = custody.state.lock().err().unwrap().into_inner();
        assert!(state.runs[&request].child.is_some());
        assert!(state.runs[&request].snapshot.exists());
    }

    #[tokio::test]
    async fn one_lost_wait_cannot_prevent_other_registered_birth_from_receiving_close_and_reap() {
        let dead = Fixture::new("exit 0");
        let live = Fixture::new("exec /bin/sleep 2");
        let custody = Arc::new(CheckCustody::default());
        let lost_io = Arc::new(FaultIo::new(0));
        let live_io = Arc::new(FaultIo::new(0));
        let (lost, _) = custody
            .spawn(&dead.binary(), &dead.config(), lost_io.clone())
            .unwrap()
            .unwrap();
        let (other, _) = custody
            .spawn(&live.binary(), &live.config(), live_io.clone())
            .unwrap()
            .unwrap();
        let pid = custody.state.lock().unwrap().runs[&lost]
            .child
            .as_ref()
            .unwrap()
            .id()
            .unwrap();
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None).unwrap();
        assert!(custody.wait_exit(lost).await.is_err());
        assert!(custody.shutdown().await.is_err());
        let state = custody.state.lock().unwrap();
        assert!(state.runs.contains_key(&lost));
        assert!(!state.runs.contains_key(&other));
        assert_eq!(lost_io.signals.load(Ordering::SeqCst), 0);
        assert!(live_io.signals.load(Ordering::SeqCst) >= 1);
    }
}

#[cfg(unix)]
use super::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU8, AtomicUsize};

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
        tries: AtomicUsize,
        signals: AtomicUsize,
    }
    impl FaultIo {
        fn new(mode: u8) -> Self {
            Self {
                mode: AtomicU8::new(mode),
                waits: AtomicUsize::new(0),
                tries: AtomicUsize::new(0),
                signals: AtomicUsize::new(0),
            }
        }
    }
    impl CheckIo for FaultIo {
        fn try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
            self.tries.fetch_add(1, Ordering::SeqCst);
            child.try_wait()
        }
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

    fn assert_pending_native(run: &CheckRun) {
        let NativeFactoryObservation::Attached(members) = &run.factory else {
            panic!("actual factory must retain its attached binding");
        };
        assert!(members.request.same(&run.request));
        assert!(run.child.is_some());
        assert!(run.native_exited.is_none());
        assert!(run.native_terminal.is_none());
    }

    fn cached_native(run: &CheckRun, source: NativeExitSource) -> ValidationNativeExited {
        let NativeFactoryObservation::Attached(members) = &run.factory else {
            panic!("actual factory must retain its attached binding");
        };
        let fact = run.native_exited.as_ref().expect("actual native return");
        assert!(members.request.same(&run.request));
        assert!(members.same(&fact.members));
        assert_eq!(Some(fact.status), run.exit);
        assert!(fact.source == source);
        assert!(run.native_terminal.is_none());
        fact.clone()
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
            let (_, run) = state.runs.iter().next().unwrap();
            assert!(run.child.is_some());
            assert_pending_native(run);
            assert_eq!(
                std::fs::metadata(run.tail.path_snapshot())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            (run.request.clone(), run.tail.path_snapshot().clone())
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
        custody.close_confirmed(&request).await.unwrap();
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
        let request = custody
            .state
            .lock()
            .unwrap()
            .runs
            .values()
            .next()
            .unwrap()
            .request
            .clone();
        assert!(custody.assert_admission().is_err());
        assert!(custody.state.lock().unwrap().runs[&request.id]
            .tail
            .path_snapshot()
            .exists());
        assert_pending_native(&custody.state.lock().unwrap().runs[&request.id]);
        io.mode.store(0, Ordering::SeqCst);
        custody.close_confirmed(&request).await.unwrap();
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
        let pid = custody.state.lock().unwrap().runs[&request.id]
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
            custody.wait_exit(&request).await.is_err(),
            "Tokio must receive real ECHILD after an external native reap"
        );
        let waits = io.waits.load(Ordering::SeqCst);
        assert_eq!(io.signals.load(Ordering::SeqCst), 0);
        io.mode.store(0, Ordering::SeqCst);
        assert!(custody.close_confirmed(&request).await.is_err());
        assert!(custody.shutdown().await.is_err());
        assert_eq!(io.waits.load(Ordering::SeqCst), waits);
        assert_eq!(io.signals.load(Ordering::SeqCst), 0);
        assert!(custody.state.lock().unwrap().runs[&request.id]
            .tail
            .path_snapshot()
            .exists());
        assert_pending_native(&custody.state.lock().unwrap().runs[&request.id]);
    }

    #[tokio::test]
    async fn cleanup_failure_after_cached_exit_blocks_replacement_and_retries_only_the_tail() {
        let fixture = Fixture::new("exit 7");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(0));
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), io.clone())
            .unwrap()
            .unwrap();
        assert!(!custody.wait_exit(&request).await.unwrap().success());
        let fact = cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::PollWait,
        );
        let calls = (
            io.waits.load(Ordering::SeqCst),
            io.tries.load(Ordering::SeqCst),
            io.signals.load(Ordering::SeqCst),
        );
        assert_eq!(custody.wait_exit(&request).await.unwrap(), fact.status);
        let snapshot = custody.state.lock().unwrap().runs[&request.id]
            .tail
            .path_snapshot()
            .clone();
        std::fs::remove_file(&snapshot).unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        assert!(custody.retire(&request).is_err());
        assert!(custody.assert_admission().is_err());
        assert!(custody.state.lock().unwrap().runs[&request.id]
            .child
            .is_some());
        let retained = cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::PollWait,
        );
        assert!(retained.members.same(&fact.members));
        std::fs::remove_dir(&snapshot).unwrap();
        custody.close_confirmed(&request).await.unwrap();
        custody.assert_admission().unwrap();
        assert_eq!(
            calls,
            (
                io.waits.load(Ordering::SeqCst),
                io.tries.load(Ordering::SeqCst),
                io.signals.load(Ordering::SeqCst)
            )
        );
        assert!(matches!(
            custody
                .run(
                    &fixture.binary(),
                    &fixture.config(),
                    Duration::from_secs(1),
                    Arc::new(NativeIo)
                )
                .await
                .unwrap(),
            RawCheck::Done { success: false, .. }
        ));
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
        {
            let state = custody.state.lock().unwrap();
            assert_eq!(state.runs.len(), 2);
            let mut runs = state.runs.values();
            let first = runs.next().unwrap();
            let second = runs.next().unwrap();
            assert_pending_native(first);
            assert_pending_native(second);
            assert!(!first.request.same(&second.request));
        }
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
        custody.close_confirmed(&old).await.unwrap();
        let (new, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.request_close(&old).unwrap();
        custody.retire(&old).unwrap();
        assert!(custody.state.lock().unwrap().runs.contains_key(&new.id));
        assert_pending_native(&custody.state.lock().unwrap().runs[&new.id]);
        custody.close_confirmed(&new).await.unwrap();
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
        assert!(custody.close_confirmed(&request).await.is_err());
        {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            assert!(
                run.child.is_some()
                    && run.exit.is_none()
                    && run.debt
                    && run.tail.path_snapshot().exists()
            );
            assert_pending_native(run);
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
        custody.request_close(&request).unwrap();
        let held = custody.cleanup_gate.lock().await;
        assert!(custody
            .settle_debts()
            .await
            .unwrap_err()
            .to_string()
            .contains("budget expired"));
        assert!(!custody.state.lock().unwrap().settling);
        assert!(custody.state.lock().unwrap().runs[&request.id].debt);
        drop(held);
        let mut settle = Box::pin(custody.settle_debts());
        assert!(poll_fn(|cx| Poll::Ready(settle.as_mut().poll(cx).is_pending())).await);
        assert!(custody.state.lock().unwrap().settling);
        drop(settle);
        assert!(!custody.state.lock().unwrap().settling);
        assert!(custody.state.lock().unwrap().runs[&request.id]
            .child
            .is_some());
        assert_pending_native(&custody.state.lock().unwrap().runs[&request.id]);
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
        custody.wait_exit(&request).await.unwrap();
        custody.wait_output(&request).await.unwrap();
        let poisoned = Arc::clone(&custody);
        assert!(std::thread::spawn(move || {
            let _state = poisoned.state.lock().unwrap();
            panic!("poison registry");
        })
        .join()
        .is_err());
        assert!(custody.assert_admission().is_err());
        assert!(custody.retire(&request).is_err());
        assert!(custody.shutdown().await.is_err());
        let state = custody.state.lock().err().unwrap().into_inner();
        assert!(state.runs[&request.id].child.is_some());
        assert!(state.runs[&request.id].tail.path_snapshot().exists());
        cached_native(&state.runs[&request.id], NativeExitSource::PollWait);
        assert!(state.runs[&request.id].native_terminal.is_none());
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
        let pid = custody.state.lock().unwrap().runs[&lost.id]
            .child
            .as_ref()
            .unwrap()
            .id()
            .unwrap();
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None).unwrap();
        assert!(custody.wait_exit(&lost).await.is_err());
        assert!(custody.shutdown().await.is_err());
        let state = custody.state.lock().unwrap();
        assert!(state.runs.contains_key(&lost.id));
        assert_pending_native(&state.runs[&lost.id]);
        assert!(!state.runs.contains_key(&other.id));
        assert_eq!(lost_io.signals.load(Ordering::SeqCst), 0);
        assert!(live_io.signals.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn actual_factory_poll_wait_consumes_native_fact_and_allows_ordinary_successor() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let (request, output) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        assert_pending_native(&custody.state.lock().unwrap().runs[&request.id]);
        assert!(custody.wait_exit(&request).await.unwrap().success());
        output.finished().await.unwrap();
        let snapshot = {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            cached_native(run, NativeExitSource::PollWait);
            assert!(run.validate_retirement().unwrap().is_some());
            run.tail.path_snapshot().clone()
        };
        custody.retire(&request).unwrap();
        assert!(!snapshot.exists());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        assert!(matches!(
            custody
                .run(
                    &fixture.binary(),
                    &fixture.config(),
                    Duration::from_secs(1),
                    Arc::new(NativeIo)
                )
                .await
                .unwrap(),
            RawCheck::Done { success: true, .. }
        ));
    }

    #[tokio::test]
    async fn actual_try_wait_some_caches_nonzero_fact_and_tail_retry_never_waits_or_signals() {
        let fixture = Fixture::new("exit 7");
        let custody = Arc::new(CheckCustody::default());
        // Only suppress signals while the stand-in is finishing; try_wait always
        // calls the real Child. No synthetic Ok or poll_wait issues this fact.
        let io = Arc::new(FaultIo::new(1));
        let (request, output) = custody
            .spawn(&fixture.binary(), &fixture.config(), io.clone())
            .unwrap()
            .unwrap();
        output.finished().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                custody.request_close(&request).unwrap();
                if custody.state.lock().unwrap().runs[&request.id]
                    .native_exited
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let (fact, snapshot) = {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            (
                cached_native(run, NativeExitSource::TryWait),
                run.tail.path_snapshot().clone(),
            )
        };
        assert_eq!(fact.status.code(), Some(7));
        assert_eq!(io.waits.load(Ordering::SeqCst), 0);
        let tries = io.tries.load(Ordering::SeqCst);
        let signals = io.signals.load(Ordering::SeqCst);
        assert!(tries >= 1);
        assert_eq!(custody.wait_exit(&request).await.unwrap(), fact.status);
        std::fs::remove_file(&snapshot).unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        assert!(custody.retire(&request).is_err());
        let retained = cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::TryWait,
        );
        assert!(fact.members.same(&retained.members));
        assert!(custody.assert_admission().is_err());
        std::fs::remove_dir(&snapshot).unwrap();
        custody.close_confirmed(&request).await.unwrap();
        assert_eq!(io.waits.load(Ordering::SeqCst), 0);
        assert_eq!(io.tries.load(Ordering::SeqCst), tries);
        assert_eq!(io.signals.load(Ordering::SeqCst), signals);
        custody.assert_admission().unwrap();
    }

    #[tokio::test]
    async fn foreign_request_and_native_binding_mismatches_preserve_original_custody() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.wait_exit(&request).await.unwrap();
        custody.wait_output(&request).await.unwrap();
        let original = cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::PollWait,
        );
        let foreign = CheckCustody::default();
        for wrong_request in [
            CheckRequestRef {
                issuer: Arc::clone(&foreign.issuer),
                ..request.clone()
            },
            CheckRequestRef {
                identity: Arc::new(CheckRequestIdentity),
                ..request.clone()
            },
        ] {
            assert!(custody.wait_exit(&wrong_request).await.is_err());
            assert!(custody.request_close(&wrong_request).is_err());
            assert!(custody.retire(&wrong_request).is_err());
            let state = custody.state.lock().unwrap();
            assert!(state.runs[&request.id].tail.path_snapshot().exists());
            assert!(state.runs[&request.id].child.is_some());
        }
        assert!(foreign.retire(&request).is_err());
        // These deliberately corrupted shapes are negative consumer inputs only.
        for case in 0..12 {
            let mut fact = original.clone();
            match case {
                0 => fact.members.request.issuer = Arc::clone(&foreign.issuer),
                1 => fact.members.request.identity = Arc::new(CheckRequestIdentity),
                2 => fact.members.request.id += 1,
                3 => fact.members.birth = Arc::new(ValidationBirthIdentity),
                4 => fact.members.member = Arc::new(ValidationMemberIdentity),
                5 => fact.members.role = ValidationRole::Foreign,
                6 => fact.members.scope = ValidationScope::Foreign,
                7 => fact.status = std::os::unix::process::ExitStatusExt::from_raw(7 << 8),
                10 => fact.members.role = ValidationRole::Foreign,
                11 => fact.members.scope = ValidationScope::Foreign,
                _ => {}
            }
            {
                let mut state = custody.state.lock().unwrap();
                let run = state.runs.get_mut(&request.id).unwrap();
                run.native_exited = if case == 8 { None } else { Some(fact) };
                if case == 9 {
                    run.factory = NativeFactoryObservation::Entered;
                } else if case >= 10 {
                    // Equal counterfeit labels on both sides cannot widen the
                    // fixed production Validation/SingleChild scope either.
                    run.factory = NativeFactoryObservation::Attached(
                        run.native_exited.as_ref().unwrap().members.clone(),
                    );
                }
            }
            assert!(
                custody.retire(&request).is_err(),
                "negative binding case {case}"
            );
            let mut state = custody.state.lock().unwrap();
            let run = state.runs.get_mut(&request.id).unwrap();
            assert!(run.tail.path_snapshot().exists() && run.child.is_some());
            assert!(run.native_terminal.is_none());
            run.factory = NativeFactoryObservation::Attached(original.members.clone());
            run.native_exited = Some(original.clone());
        }
        custody.retire(&request).unwrap();
    }

    #[tokio::test]
    async fn entered_or_partial_attachment_without_child_never_becomes_prebirth_cleanup() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let (request, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        let (partial, _) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        {
            let mut state = custody.state.lock().unwrap();
            state.runs.get_mut(&partial.id).unwrap().factory = NativeFactoryObservation::Entered;
        }
        // A real native return after an incomplete attachment can cache its
        // status, but cannot issue a fact or turn the cached shortcut into one.
        assert!(custody.wait_exit(&partial).await.is_err());
        assert!(custody.wait_exit(&partial).await.is_err());
        assert!(custody.retire(&partial).is_err());
        {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&partial.id];
            assert!(run.child.is_some() && run.exit.is_some() && run.tail.path_snapshot().exists());
            assert!(run.debt && run.native_exited.is_none() && run.native_terminal.is_none());
        }
        custody.wait_exit(&request).await.unwrap();
        custody.wait_output(&request).await.unwrap();
        let (child, fact) = {
            let mut state = custody.state.lock().unwrap();
            let run = state.runs.get_mut(&request.id).unwrap();
            let fact = cached_native(run, NativeExitSource::PollWait);
            run.factory = NativeFactoryObservation::Entered;
            run.exit = None;
            run.native_exited = None;
            (run.child.take().unwrap(), fact)
        };
        assert!(custody.close_confirmed(&request).await.is_err());
        {
            let mut state = custody.state.lock().unwrap();
            let run = state.runs.get_mut(&request.id).unwrap();
            assert!(run.tail.path_snapshot().exists() && run.native_exited.is_none());
            run.factory = NativeFactoryObservation::Attached(fact.members.clone());
            run.native_exited = Some(fact.clone());
            run.exit = Some(fact.status);
        }
        assert!(custody.retire(&request).is_err());
        let state = custody.state.lock().unwrap();
        assert!(state.runs[&request.id].tail.path_snapshot().exists());
        assert!(state.runs[&request.id].native_terminal.is_none());
        drop(child); // Already reaped by the actual native poll, never a fake birth.
    }

    #[tokio::test]
    async fn actual_copy_failure_cleans_only_own_snapshot_without_native_return() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let io = Arc::new(FaultIo::new(0));
        let missing_config = fixture.0.join("missing-config.json");
        assert!(matches!(
            custody
                .run(
                    &fixture.binary(),
                    &missing_config,
                    Duration::from_secs(1),
                    io.clone()
                )
                .await
                .unwrap(),
            RawCheck::SpawnFailed(_)
        ));
        assert!(custody.state.lock().unwrap().runs.is_empty());
        assert_eq!(io.waits.load(Ordering::SeqCst), 0);
        assert_eq!(io.tries.load(Ordering::SeqCst), 0);
        assert_eq!(io.signals.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        custody.assert_admission().unwrap();
    }

    fn replace_test_capture(run: &mut CheckRun, replacement: Arc<OutputCapture>) {
        match &mut run.tail {
            CheckTail::PathSnapshot { output, .. } => *output = replacement,
            #[cfg(target_os = "linux")]
            CheckTail::OwnedSealedInputs { .. } => {
                panic!("ordinary negative fixture needs its path tail")
            }
        }
    }

    struct EofReader(Option<tokio::sync::oneshot::Sender<()>>);
    impl AsyncRead for EofReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if let Some(eof) = self.0.take() {
                let _ = eof.send(());
            }
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eof_before_task_return_and_cancelled_join_keep_original_tail_and_request() {
        let fixture = Fixture::new("exit 0");
        let custody = Arc::new(CheckCustody::default());
        let (request, original) = custody
            .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.wait_exit(&request).await.unwrap();
        original.finished().await.unwrap();
        let output = Arc::new(OutputCapture::default());
        // Exercise the very same read_to_end/buffer-assignment task body. This
        // private negative tail fixture does not claim real process I/O failure.
        let blocked = Arc::clone(&output);
        let (locked, acquired) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || {
            let _buffer = blocked.stdout.lock().unwrap();
            let _ = locked.send(());
            released.recv().unwrap();
        });
        acquired.await.unwrap();
        let (eof, observed) = tokio::sync::oneshot::channel();
        output.read(Some(EofReader(Some(eof))), false).unwrap();
        output.read(Some(tokio::io::empty()), true).unwrap();
        observed.await.unwrap();
        {
            let mut state = custody.state.lock().unwrap();
            replace_test_capture(
                state.runs.get_mut(&request.id).unwrap(),
                Arc::clone(&output),
            );
        }
        let mut joining = Box::pin(output.finished());
        assert!(poll_fn(|cx| Poll::Ready(joining.as_mut().poll(cx).is_pending())).await);
        let mut successor = Box::pin(output.finished());
        assert!(poll_fn(|cx| Poll::Ready(successor.as_mut().poll(cx).is_pending())).await);
        drop(joining);
        assert!(output.tasks.lock().unwrap()[0].handle.is_some());
        assert!(custody.retire(&request).is_err());
        {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            assert!(run.tail.path_snapshot().exists() && run.child.is_some() && run.debt);
            assert!(run.native_terminal.is_none());
            cached_native(run, NativeExitSource::PollWait);
        }
        assert!(custody.assert_admission().is_err());
        release.send(()).unwrap();
        owner.join().unwrap();
        successor.await.unwrap();
        output.finished().await.unwrap();
        custody.retire(&request).unwrap();
        custody.assert_admission().unwrap();
    }

    struct FailingReader(bool);
    impl AsyncRead for FailingReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            assert!(!self.0, "reader task panic fixture");
            Poll::Ready(Err(io::Error::other("reader I/O failure fixture")))
        }
    }

    #[tokio::test]
    async fn read_error_keeps_diagnostic_mapping_but_panic_abort_and_missing_reader_keep_custody() {
        let output = Arc::new(OutputCapture::default());
        output.read(Some(FailingReader(false)), false).unwrap();
        output.read(Some(tokio::io::empty()), true).unwrap();
        output.finished().await.unwrap();
        assert!(matches!(output.result(true), RawCheck::OutputFailed(_)));
        output.validate_finished().unwrap();
        for case in 0..3 {
            let fixture = Fixture::new("exit 0");
            let custody = Arc::new(CheckCustody::default());
            let (request, original) = custody
                .spawn(&fixture.binary(), &fixture.config(), Arc::new(NativeIo))
                .unwrap()
                .unwrap();
            custody.wait_exit(&request).await.unwrap();
            original.finished().await.unwrap();
            let output = Arc::new(OutputCapture::default());
            if case == 0 {
                output.read(Some(FailingReader(true)), false).unwrap();
            } else if case == 1 {
                let (_writer, reader) = tokio::io::duplex(8);
                output.read(Some(reader), false).unwrap();
                output.tasks.lock().unwrap()[0]
                    .handle
                    .as_ref()
                    .unwrap()
                    .abort();
            } else {
                output.read(None::<tokio::io::Empty>, false).unwrap();
            }
            output.read(Some(tokio::io::empty()), true).unwrap();
            assert!(output.finished().await.is_err());
            {
                let mut state = custody.state.lock().unwrap();
                replace_test_capture(state.runs.get_mut(&request.id).unwrap(), output);
            }
            assert!(custody.retire(&request).is_err());
            assert!(custody.close_confirmed(&request).await.is_err());
            assert!(custody.assert_admission().is_err());
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            assert!(run.child.is_some() && run.tail.path_snapshot().exists());
            assert!(run.native_terminal.is_none());
        }
    }

    #[cfg(target_os = "linux")]
    fn owned_fixture(program: &str, args: &[&str]) -> (tokio::process::Command, File, File) {
        use nix::fcntl::{fcntl, FcntlArg, SealFlag};
        use nix::sys::memfd::{memfd_create, MFdFlags};
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let image = |bytes: &[u8], executable: bool| {
            let mut file = File::from(
                memfd_create(
                    "polaris-check-owned-fixture",
                    MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
                )
                .unwrap(),
            );
            file.write_all(bytes).unwrap();
            if executable {
                nix::sys::stat::fchmod(
                    &file,
                    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IXUSR,
                )
                .unwrap();
            }
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
            file
        };
        let binary = image(&std::fs::read(program).unwrap(), true);
        let config = image(b"{\"owned-check-fixture\":true}", false);
        let mut command =
            tokio::process::Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        use std::os::unix::process::CommandExt;
        command
            .as_std_mut()
            .arg0(Path::new(program).file_name().unwrap());
        command.args(args);
        (command, binary, config)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn owned_sealed_factory_publishes_original_child_files_and_native_terminal() {
        use std::os::fd::AsRawFd;
        let custody = Arc::new(CheckCustody::default());
        let (command, binary, config) = owned_fixture("/bin/true", &[]);
        let binary_fd = binary.as_raw_fd();
        let config_fd = config.as_raw_fd();
        let request = custody
            .spawn_owned_sealed(command, binary, config, Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        {
            let state = custody.state.lock().unwrap();
            let run = &state.runs[&request.id];
            assert_pending_native(run);
            let CheckTail::OwnedSealedInputs {
                _binary, _config, ..
            } = &run.tail
            else {
                panic!("owned inputs cannot become a path snapshot")
            };
            assert_eq!(_binary.as_raw_fd(), binary_fd);
            assert_eq!(_config.as_raw_fd(), config_fd);
            validate_sealed_file(_binary).unwrap();
            validate_sealed_file(_config).unwrap();
            assert!(run.child.as_ref().unwrap().stdout.is_none());
            assert!(run.child.as_ref().unwrap().stderr.is_none());
            assert!(run.tail.output().is_none());
            assert!(run.validate_retirement().is_err());
        }
        assert!(custody.wait_exit(&request).await.unwrap().success());
        cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::PollWait,
        );
        custody.retire(&request).unwrap();
        assert!(custody.state.lock().unwrap().runs.is_empty());
        custody.assert_admission().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn owned_sealed_timeout_and_shutdown_use_the_same_central_admission_and_reap() {
        let custody = Arc::new(CheckCustody::default());
        let (command, binary, config) = owned_fixture("/bin/sleep", &["30"]);
        assert!(matches!(
            custody
                .supervise_owned_sealed(command, binary, config, Duration::ZERO, None)
                .await
                .unwrap(),
            RawCheck::TimedOut { .. }
        ));
        assert!(custody.state.lock().unwrap().runs.is_empty());
        let (command, binary, config) = owned_fixture("/bin/sleep", &["30"]);
        let request = custody
            .spawn_owned_sealed(command, binary, config, Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        let pid = custody.state.lock().unwrap().runs[&request.id]
            .child
            .as_ref()
            .unwrap()
            .id()
            .unwrap();
        custody.shutdown().await.unwrap();
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        let (command, binary, config) = owned_fixture("/bin/true", &[]);
        assert!(matches!(
            custody.spawn_owned_sealed(command, binary, config, Arc::new(NativeIo)),
            Err(ValidationLifecycleError::Closing)
        ));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn owned_sealed_incomplete_protection_foreign_command_and_binding_never_retire_as_null_proof(
    ) {
        let custody = Arc::new(CheckCustody::default());
        let (command, binary, _config) = owned_fixture("/bin/true", &[]);
        let unsealed = File::from(
            nix::sys::memfd::memfd_create(
                "polaris-check-unsealed",
                nix::sys::memfd::MFdFlags::MFD_CLOEXEC
                    | nix::sys::memfd::MFdFlags::MFD_ALLOW_SEALING,
            )
            .unwrap(),
        );
        assert!(custody
            .spawn_owned_sealed(command, binary, unsealed, Arc::new(NativeIo))
            .unwrap()
            .is_err());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        let (_, binary, config) = owned_fixture("/bin/true", &[]);
        assert!(custody
            .spawn_owned_sealed(
                tokio::process::Command::new("/bin/true"),
                binary,
                config,
                Arc::new(NativeIo)
            )
            .unwrap()
            .is_err());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        let (command, binary, config) = owned_fixture("/bin/true", &[]);
        let request = custody
            .spawn_owned_sealed(command, binary, config, Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        custody.wait_exit(&request).await.unwrap();
        let original = cached_native(
            &custody.state.lock().unwrap().runs[&request.id],
            NativeExitSource::PollWait,
        );
        let foreign = CheckRequestRef {
            identity: Arc::new(CheckRequestIdentity),
            ..request.clone()
        };
        assert!(custody.retire(&foreign).is_err());
        {
            let mut state = custody.state.lock().unwrap();
            let run = state.runs.get_mut(&request.id).unwrap();
            run.native_exited.as_mut().unwrap().members.role = ValidationRole::Foreign;
        }
        assert!(custody.retire(&request).is_err());
        assert!(custody.assert_admission().is_err());
        {
            let mut state = custody.state.lock().unwrap();
            let run = state.runs.get_mut(&request.id).unwrap();
            assert!(run.child.is_some() && matches!(run.tail, CheckTail::OwnedSealedInputs { .. }));
            assert!(run.native_terminal.is_none());
            run.native_exited = Some(original);
        }
        custody.close_confirmed(&request).await.unwrap();
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn f1_queued_owned_images_are_in_original_runs_before_dispatch_and_pause() {
        use std::os::fd::AsRawFd;
        let custody = Arc::new(CheckCustody::default());
        let (command, binary, config) = owned_fixture("/bin/true", &[]);
        let binary_fd = binary.as_raw_fd();
        let config_fd = config.as_raw_fd();
        let request = custody
            .queue_owned_sealed(command, binary, config, Arc::new(NativeIo))
            .unwrap()
            .unwrap();
        let completion = {
            let state = custody.state.lock().unwrap();
            let original = &state.runs[&request.id];
            assert!(matches!(
                original.factory,
                NativeFactoryObservation::PreFactory
            ));
            assert!(original.child.is_none());
            let CheckTail::OwnedSealedInputs {
                _binary, _config, ..
            } = &original.tail
            else {
                panic!("missing original owned images")
            };
            assert_eq!(_binary.as_raw_fd(), binary_fd);
            assert_eq!(_config.as_raw_fd(), config_fd);
            Arc::clone(&original.completion)
        };
        let view = CheckProducerView {
            custody: Arc::clone(&custody),
            members: vec![(request.clone(), Arc::clone(&completion))],
        };
        custody.state.lock().unwrap().pause = Some(Arc::new(()));
        assert!(custody.assert_admission().is_err());
        assert!(custody.dispatch_owned_sealed(&request).is_err());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        assert!(completion.terminal.load(Ordering::SeqCst));
        assert!(
            view.verify_original_custody().is_err(),
            "coordinator return cannot be inferred from empty runs"
        );
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn f1_actual_queued_coordinator_keeps_images_visible_and_retires_on_cutoff() {
        let custody = Arc::new(CheckCustody::default());
        let (command, binary, config) = owned_fixture("/bin/true", &[]);
        let future =
            custody.supervise_owned_sealed(command, binary, config, Duration::from_secs(1), None);
        tokio::pin!(future);
        poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let view = {
            let mut state = custody.state.lock().unwrap();
            assert_eq!(state.runs.len(), 1);
            let run = state.runs.values().next().unwrap();
            assert!(run.child.is_none());
            assert!(matches!(
                &run.tail,
                CheckTail::OwnedSealedInputs {
                    command: Some(_),
                    ..
                }
            ));
            let members = vec![(run.request.clone(), Arc::clone(&run.completion))];
            state.pause = Some(Arc::new(()));
            CheckProducerView {
                custody: Arc::clone(&custody),
                members,
            }
        };
        assert!(view.verify_original_custody().is_err());
        assert!(future.await.is_err());
        assert!(custody.state.lock().unwrap().runs.is_empty());
        view.verify_original_custody().unwrap();
    }
}

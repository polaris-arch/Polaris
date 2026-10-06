use super::*;
use std::sync::atomic::AtomicUsize;

fn saved() -> Value {
    let mut config = polaris_store::default_config();
    config["servers"] = serde_json::json!([
        {"id":"first", "name":"First", "protocol":"tailscale", "tailscaleSettings":{}},
        {"id":"second", "name":"Second", "protocol":"tailscale", "tailscaleSettings":{}},
    ]);
    config["selectedServerId"] = serde_json::json!("first");
    config
}

fn ticket() -> ReadyMainTicket {
    let saved = saved();
    let binding = ActionBinding::new(
        NormalMainAction::TailscaleLogin,
        "attempt".into(),
        &saved,
        vec!["second".into()],
        Some("epoch-authority".into()),
    )
    .unwrap();
    let core = Arc::new(ReadyMainCore {
        generation: 7,
        saved_digest: digest(&saved).unwrap(),
        emission_digest: "final-emission".into(),
        targets: BTreeMap::from([("second".into(), "Second".into())]),
        status: ProxyStatus {
            running: true,
            clash_api_port: 9876,
            ..ProxyStatus::default()
        },
        api_secret: "bound-secret".into(),
        probe_ports: vec![9877],
        local_http_proxy: Some(LocalHttpProxy {
            port: 9878,
            auth: None,
        }),
        committed: AtomicBool::new(true),
    });
    ReadyMainTicket { core, binding }
}

#[test]
fn final_emission_includes_unselected_ts_and_excludes_peeled_targets() {
    let parsed = serde_json::from_value::<UserConfig>(saved()).unwrap();
    let final_config =
        serde_json::json!({"endpoints":[{"type":"tailscale","tag":"Second"}], "outbounds":[]});
    assert_eq!(
        final_targets(&parsed, &final_config),
        BTreeMap::from([("second".into(), "Second".into())])
    );
    let mut ticket = ticket();
    ticket.binding.target_ids = vec!["first".into()];
    assert_eq!(
        check_binding(
            &ticket,
            7,
            true,
            false,
            &ticket.binding.saved_digest,
            Some(&ticket.core)
        ),
        Err(MainPrerequisiteError::TargetMissing)
    );
}

#[test]
fn ready_capture_uses_final_effective_ids_after_same_name_target_is_peeled() {
    let (runtime, _directory) = runtime();
    let mut saved = saved();
    saved["servers"][0]["name"] = serde_json::json!("X");
    saved["servers"][1]["name"] = serde_json::json!("X");
    let mut effective: UserConfig = serde_json::from_value(saved.clone()).unwrap();
    effective.servers.retain(|server| server.id == "second");
    let emitted = serde_json::json!({"endpoints":[{"type":"tailscale","tag":"X"}],"outbounds":[]});
    runtime
        .capture_ready_main(
            runtime.core_generation(),
            &saved,
            &effective,
            &emitted,
            &ProxyStatus {
                running: true,
                clash_api_port: 9876,
                ..Default::default()
            },
        )
        .unwrap();
    let core = runtime.ready_main.read().unwrap().clone().unwrap();
    assert_eq!(
        core.targets,
        BTreeMap::from([("second".into(), "X".into())])
    );
    assert!(
        !core.targets.contains_key("first"),
        "the peeled identity cannot inherit a reused tag"
    );
    assert_eq!(core.saved_digest, digest(&saved).unwrap());
    assert_ne!(
        core.saved_digest,
        digest(&serde_json::to_value(&effective).unwrap()).unwrap()
    );
    assert_eq!(core.emission_digest, digest(&emitted).unwrap());
}

type ProbeWrites = Arc<std::sync::Mutex<Vec<(u16, String, String, String)>>>;
struct RecordedProbeApi {
    port: u16,
    secret: String,
    writes: ProbeWrites,
    current: Arc<AtomicBool>,
    supersede_during_write: bool,
}
#[async_trait]
impl polaris_switch_engine::ManagementApi for RecordedProbeApi {
    async fn select_outbound(
        &self,
        selector: &str,
        member: &str,
    ) -> Result<(), polaris_switch_engine::ManagementError> {
        self.writes.lock().unwrap().push((
            self.port,
            self.secret.clone(),
            selector.into(),
            member.into(),
        ));
        if self.supersede_during_write {
            self.current.store(false, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn close_connection(
        &self,
        _: &str,
    ) -> Result<(), polaris_switch_engine::ManagementError> {
        panic!("a bound probe must not close another connection")
    }
    async fn first_connection_snapshot(
        &self,
    ) -> Result<
        Vec<polaris_switch_engine::ConnectionSnapshot>,
        polaris_switch_engine::ManagementError,
    > {
        panic!("a bound probe must not query current connection state")
    }
}

#[tokio::test]
async fn bound_probe_transport_stays_on_ticket_a_when_current_runtime_becomes_b() {
    let (runtime, _directory) = runtime();
    runtime.status.write().unwrap().clash_api_port = 9876;
    let ticket = ticket();
    let current = Arc::new(AtomicBool::new(true));
    let writes = ProbeWrites::default();
    let connected = std::sync::Mutex::new(Vec::new());
    let selected = select_ticket_probe(
        &ticket,
        0,
        "Second",
        || current.load(Ordering::SeqCst) && runtime.status().clash_api_port == ticket.api_port(),
        |port, secret| {
            std::future::ready({
                connected.lock().unwrap().push((port, secret.clone()));
                // This is the real transport-construction seam, after validation A. Generation B
                // owns a different current endpoint before construction completes.
                runtime.status.write().unwrap().clash_api_port = 34567;
                current.store(false, Ordering::SeqCst);
                RecordedProbeApi {
                    port,
                    secret,
                    writes: writes.clone(),
                    current: current.clone(),
                    supersede_during_write: false,
                }
            })
        },
    )
    .await;
    assert!(!selected);
    assert_eq!(runtime.status().clash_api_port, 34567);
    assert_eq!(
        *connected.lock().unwrap(),
        vec![(9876, "bound-secret".into())]
    );
    assert!(
        writes.lock().unwrap().is_empty(),
        "neither A nor successor B may receive a stale PUT"
    );
}

#[tokio::test]
async fn bound_probe_actual_write_is_pinned_and_supersession_discards_completion() {
    for supersede_during_write in [false, true] {
        let ticket = ticket();
        let current = Arc::new(AtomicBool::new(true));
        let writes = ProbeWrites::default();
        let selected = select_ticket_probe(
            &ticket,
            0,
            "Second",
            || current.load(Ordering::SeqCst),
            |port, secret| {
                std::future::ready({
                    RecordedProbeApi {
                        port,
                        secret,
                        writes: writes.clone(),
                        current: current.clone(),
                        supersede_during_write,
                    }
                })
            },
        )
        .await;
        assert_eq!(selected, !supersede_during_write);
        assert_eq!(
            *writes.lock().unwrap(),
            vec![(
                9876,
                "bound-secret".into(),
                "probe-selector-0".into(),
                "Second".into()
            )]
        );
    }
}

#[tokio::test]
async fn bound_probe_rejects_foreign_target_slot_or_invalid_ticket_before_transport() {
    for (slot, member, current) in [
        (0, "First", true),
        (1, "Second", true),
        (0, "Second", false),
    ] {
        let connected = AtomicUsize::new(0);
        let ticket = ticket();
        assert!(
            !select_ticket_probe(
                &ticket,
                slot,
                member,
                || current,
                |port, secret| std::future::ready({
                    connected.fetch_add(1, Ordering::SeqCst);
                    RecordedProbeApi {
                        port,
                        secret,
                        writes: ProbeWrites::default(),
                        current: Arc::new(AtomicBool::new(current)),
                        supersede_during_write: false,
                    }
                })
            )
            .await
        );
        assert_eq!(connected.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn inherited_running_tcp_and_uncommitted_start_cannot_authorize() {
    let ticket = ticket();
    assert_eq!(
        check_binding(&ticket, 7, true, false, &ticket.binding.saved_digest, None),
        Err(MainPrerequisiteError::ReadyUnknown)
    );
    ticket.core.committed.store(false, Ordering::SeqCst);
    assert_eq!(
        check_binding(
            &ticket,
            7,
            true,
            false,
            &ticket.binding.saved_digest,
            Some(&ticket.core)
        ),
        Err(MainPrerequisiteError::ReadyUnknown)
    );
    ticket.core.committed.store(true, Ordering::SeqCst);
    for (generation, running, busy) in [(8, true, false), (7, false, false), (7, true, true)] {
        assert_eq!(
            check_binding(
                &ticket,
                generation,
                running,
                busy,
                &ticket.binding.saved_digest,
                Some(&ticket.core)
            ),
            Err(MainPrerequisiteError::Superseded)
        );
    }
    assert_eq!(
        check_binding(&ticket, 7, true, false, "changed", Some(&ticket.core)),
        Err(MainPrerequisiteError::ConfigurationChanged)
    );
}

#[test]
fn epoch_and_config_content_are_bound_without_compatibility_version_shortcuts() {
    let ticket = ticket();
    assert_eq!(ticket.identity_epoch(), Some("epoch-authority"));
    assert_eq!(ticket.target_tag("second"), Some("Second"));
    assert_eq!(ticket.target_tag("first"), None);
    let mut newer = saved();
    newer["servers"][1]["tailscaleSettings"]["hostname"] = serde_json::json!("changed");
    assert_ne!(digest(&saved()).unwrap(), digest(&newer).unwrap());
    assert_eq!(
        NormalMainAction::TailscaleLogin.requirement(Platform::Ios),
        ActionRequirement::NormalMainRequired
    );
    for platform in [
        Platform::Mac,
        Platform::Linux,
        Platform::Win,
        Platform::Android,
    ] {
        assert_eq!(
            NormalMainAction::TailscaleLogin.requirement(platform),
            ActionRequirement::IndependentExistingPath
        );
        assert_eq!(
            NormalMainAction::ManualSpeedTest.requirement(platform),
            ActionRequirement::IndependentExistingPath
        );
        assert_eq!(
            NormalMainAction::ManualNetworkCheck.requirement(platform),
            ActionRequirement::NormalMainRequired
        );
    }
}

struct MockProbe {
    calls: AtomicUsize,
    current: AtomicBool,
    result: Result<(), MainPrerequisiteError>,
    supersede_during_io: bool,
}
#[async_trait]
impl MainReadinessProbe for MockProbe {
    async fn observe(&self, _core: &ReadyMainCore) -> Result<(), MainPrerequisiteError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.supersede_during_io {
            self.current.store(false, Ordering::SeqCst);
        }
        self.result.clone()
    }
}

#[tokio::test]
async fn fresh_observation_is_required_and_supersession_during_io_rejects_commit() {
    for (result, supersede, expected) in [
        (Ok(()), false, Ok(())),
        (
            Err(MainPrerequisiteError::ReadyUnknown),
            false,
            Err(MainPrerequisiteError::ReadyUnknown),
        ),
        (Ok(()), true, Err(MainPrerequisiteError::Superseded)),
    ] {
        let probe = MockProbe {
            calls: AtomicUsize::new(0),
            current: AtomicBool::new(true),
            result,
            supersede_during_io: supersede,
        };
        let ticket = ticket();
        let result = validate_with_probe(&probe, &ticket, || {
            if !probe.current.load(Ordering::SeqCst) {
                return Err(MainPrerequisiteError::Superseded);
            }
            check_binding(
                &ticket,
                7,
                true,
                false,
                &ticket.binding.saved_digest,
                Some(&ticket.core),
            )
        })
        .await;
        assert_eq!(result, expected);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn invalid_binding_never_dispatches_target_observation() {
    let probe = MockProbe {
        calls: AtomicUsize::new(0),
        current: AtomicBool::new(true),
        result: Ok(()),
        supersede_during_io: false,
    };
    let ticket = ticket();
    assert_eq!(
        validate_with_probe(&probe, &ticket, || Err(
            MainPrerequisiteError::ConfigurationChanged
        ))
        .await,
        Err(MainPrerequisiteError::ConfigurationChanged)
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}

struct NoSystemProxy;
impl super::super::system_takeover::SystemProxyClearer for NoSystemProxy {
    fn ensure_cleared(&mut self) -> bool {
        false
    }
    fn detect_foreign_proxy(&self) -> Option<String> {
        None
    }
    fn enable_system_proxy(
        &mut self,
        _: &polaris_system_integration::proxy_ops::ProxyEnableRequest,
    ) -> Result<(), String> {
        Ok(())
    }
    fn recover_from_marker(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}
struct NoNetwork;
#[async_trait]
impl polaris_dns_race::DohPost for NoNetwork {
    async fn post_dns_message(&self, _: &str, _: Vec<u8>) -> Result<Vec<u8>, String> {
        panic!("prerequisite pure tests cannot use host network")
    }
}
pub(super) fn runtime() -> (Arc<ProxyRuntime>, crate::test_support::TestDir) {
    let dir = crate::test_support::TestDir::new("polaris-prerequisite-");
    let runtime = Arc::new(ProxyRuntime::new(
        Arc::new(crate::runtime::config::ConfigManager::new(
            dir.to_path_buf(),
        )),
        Arc::new(
            crate::runtime::helper::HelperRuntime::never_installed_for_tests(dir.to_path_buf()),
        ),
        Arc::new(crate::runtime::mesh::MeshRuntime::new(dir.to_path_buf())),
        Box::new(NoSystemProxy),
        Arc::new(NoNetwork),
    ));
    (runtime, dir)
}

pub(super) fn credential_runtime(
    registry: crate::runtime::tailscale_login_core::LoginCoreRegistry,
) -> (
    Arc<ProxyRuntime>,
    Arc<crate::runtime::mesh::MeshRuntime>,
    crate::test_support::TestDir,
    Arc<polaris_core_supervisor::LifecycleGate>,
) {
    let dir = crate::test_support::TestDir::new("polaris-credential-");
    let mesh = Arc::new(
        crate::runtime::mesh::MeshRuntime::with_login_registry_for_test(
            dir.to_path_buf(),
            registry,
        ),
    );
    let runtime = Arc::new(ProxyRuntime::new(
        Arc::new(crate::runtime::config::ConfigManager::new(
            dir.to_path_buf(),
        )),
        Arc::new(
            crate::runtime::helper::HelperRuntime::never_installed_for_tests(dir.to_path_buf()),
        ),
        Arc::clone(&mesh),
        Box::new(NoSystemProxy),
        Arc::new(NoNetwork),
    ));
    let gate = Arc::clone(&runtime.gate);
    (runtime, mesh, dir, gate)
}

pub(super) fn ready_for(runtime: &ProxyRuntime) -> (Arc<ReadyMainCore>, ActionBinding) {
    let saved = runtime.config.current().unwrap();
    let binding = ActionBinding::new(
        NormalMainAction::ManualNetworkCheck,
        "network-check".into(),
        &saved,
        vec![],
        None,
    )
    .unwrap();
    let status = ProxyStatus {
        running: true,
        clash_api_port: 9876,
        main_generation: runtime.core_generation(),
        ..ProxyStatus::default()
    };
    let core = Arc::new(ReadyMainCore {
        generation: runtime.core_generation(),
        saved_digest: digest(&saved).unwrap(),
        emission_digest: "observed-final-emission".into(),
        targets: BTreeMap::new(),
        status: status.clone(),
        api_secret: String::new(),
        probe_ports: vec![],
        local_http_proxy: None,
        committed: AtomicBool::new(true),
    });
    *runtime.status.write().unwrap() = status;
    *runtime.ready_main.write().unwrap() = Some(Arc::clone(&core));
    (core, binding)
}

#[test]
fn saved_writer_completed_before_ready_commit_rejects_same_selection() {
    let (runtime, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    runtime.check_ready_main(&ticket).unwrap();
    let before = runtime.config.current().unwrap();
    let selected = before.get("selectedServerId").cloned();
    let mut changed = before.clone();
    changed["logLevel"] = serde_json::json!("debug");
    let writer = Arc::clone(&runtime.config);
    std::thread::spawn(move || {
        writer
            .save_full_deferred_cleanup(&before, &changed)
            .unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(
        runtime
            .config
            .current()
            .unwrap()
            .get("selectedServerId")
            .cloned(),
        selected
    );
    let calls = AtomicUsize::new(0);
    assert_eq!(
        runtime.with_ready_main_commit(&ticket, || calls.fetch_add(1, Ordering::SeqCst)),
        Err(MainPrerequisiteError::ConfigurationChanged)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn ready_commit_busy_writer_is_unknown_and_completed_stop_is_superseded() {
    let (runtime, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let writer = runtime.config.hold_write_lock_for_test();
    assert_eq!(
        runtime.with_ready_main_commit(&ticket, || ()),
        Err(MainPrerequisiteError::ReadyUnknown)
    );
    drop(writer);
    let stopping = Arc::clone(&runtime);
    std::thread::spawn(move || {
        stopping
            .gate
            .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
    })
    .join()
    .unwrap();
    let called = AtomicUsize::new(0);
    assert_eq!(
        runtime.with_ready_main_commit(&ticket, || called.fetch_add(1, Ordering::SeqCst)),
        Err(MainPrerequisiteError::Superseded)
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn ready_commit_serializes_a_later_stop_generation() {
    use std::sync::mpsc;
    let (runtime, ticket, _directory) = ProxyRuntime::ready_commit_fixture_for_test();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let stopping = Arc::clone(&runtime);
    let stop = runtime
        .with_ready_main_commit(&ticket, || {
            let stop = std::thread::spawn(move || {
                attempt_tx.send(()).unwrap();
                stopping
                    .gate
                    .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop);
                done_tx.send(()).unwrap();
            });
            attempt_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
            stop
        })
        .unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    stop.join().unwrap();
    assert_eq!(
        runtime.with_ready_main_commit(&ticket, || ()),
        Err(MainPrerequisiteError::Superseded)
    );
}

#[tokio::test(start_paused = true)]
async fn committed_main_waits_for_outer_lifecycle_and_tun_flush_without_restart() {
    let (runtime, _dir) = runtime();
    let (core, binding) = ready_for(&runtime);
    let generation = runtime.core_generation();
    runtime.gate.begin();
    let flush = runtime.network_settle.begin("mock-post-start-flush");
    let waiter_runtime = Arc::clone(&runtime);
    let waiter = tokio::spawn(async move {
        waiter_runtime
            .wait_ready_main_stable(&core, &binding, Duration::from_secs(1))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    runtime
        .gate
        .end(polaris_core_supervisor::LifecycleKind::Start);
    tokio::time::advance(Duration::from_millis(30)).await;
    assert!(
        !waiter.is_finished(),
        "the actual TUN flush must return its guard"
    );
    drop(flush);
    assert_eq!(waiter.await.unwrap(), Ok(()));
    assert_eq!(runtime.core_generation(), generation);
    assert!(runtime.core_running());
}

#[tokio::test(start_paused = true)]
async fn unresolved_busy_is_unknown_and_successor_cannot_complete_old_ready_wait() {
    let (runtime, _dir) = runtime();
    let (core, binding) = ready_for(&runtime);
    runtime.gate.begin();
    assert_eq!(
        runtime
            .wait_ready_main_stable(&core, &binding, Duration::from_millis(30))
            .await,
        Err(MainPrerequisiteError::ReadyUnknown),
    );
    assert!(
        runtime.core_running(),
        "action timeout does not Stop the normal main"
    );
    runtime.gate.bump_generation();
    assert_eq!(
        runtime
            .wait_ready_main_stable(&core, &binding, Duration::from_secs(1))
            .await,
        Err(MainPrerequisiteError::Superseded),
    );
}

#[test]
fn completed_generation_cannot_capture_same_config_successor_evidence() {
    let (runtime, _dir) = runtime();
    let (old, _) = ready_for(&runtime);
    runtime
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    let (successor, _) = ready_for(&runtime);
    assert_eq!(old.saved_digest, successor.saved_digest);
    assert!(matches!(
        runtime.ready_main_for_generation(old.generation),
        Err(MainPrerequisiteError::Superseded)
    ));
    assert!(Arc::ptr_eq(
        &runtime
            .ready_main_for_generation(successor.generation)
            .unwrap(),
        &successor
    ));
}

#[test]
fn stop_at_ready_commit_boundary_prevents_commit_and_started_publication() {
    let (runtime, _dir) = runtime();
    let (old, _) = ready_for(&runtime);
    old.committed.store(false, Ordering::SeqCst);
    let stop_generation = runtime
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    let published = AtomicUsize::new(0);
    assert!(!runtime.publish_committed_ready_main(old.generation, || {
        published.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert!(!old.committed.load(Ordering::SeqCst));
    assert_eq!(runtime.core_generation(), stop_generation);

    runtime
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Start)
        .unwrap();
    let (successor, _) = ready_for(&runtime);
    successor.committed.store(false, Ordering::SeqCst);
    assert!(
        runtime.publish_committed_ready_main(successor.generation, || {
            // This is the production event callback boundary: ordinary readers may
            // read the gate/status/evidence because commit holds none of those locks.
            assert_eq!(runtime.status().main_generation, successor.generation);
            assert!(runtime.ready_main.try_write().is_ok());
            published.fetch_add(1, Ordering::SeqCst);
        })
    );
    assert_eq!(published.load(Ordering::SeqCst), 1);
    assert!(successor.committed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn running_before_transaction_commit_only_joins_its_current_saved_producer() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    let mut producer = runtime.normal_start_completion(Value::Null).unwrap();
    while matches!(
        &*producer.borrow_and_update(),
        NormalStartCompletion::Pending(_)
    ) {
        producer.changed().await.unwrap();
    }
    runtime.status.write().unwrap().running = true;
    let joined = runtime
        .join_current_normal_start(&Value::Null)
        .unwrap()
        .unwrap();
    assert!(producer.same_channel(&joined));
    assert!(runtime
        .join_current_normal_start(&serde_json::json!(false))
        .unwrap()
        .is_none());
    assert!(runtime.ready_main.read().unwrap().is_none());
    drop(state_gate);
    while !matches!(
        &*producer.borrow_and_update(),
        NormalStartCompletion::Finished(Err(_), _)
    ) {
        producer.changed().await.unwrap();
    }
    assert!(
        runtime.ready_main.read().unwrap().is_none(),
        "running alone never produced ready authority"
    );
}

#[test]
fn finished_same_generation_producer_remains_joinable_while_outer_operation_settles() {
    let (runtime, _dir) = runtime();
    let (core, _) = ready_for(&runtime);
    let saved = runtime.config.current().unwrap();
    let (completion, _) = watch::channel(NormalStartCompletion::Finished(
        Ok(core.status.clone()),
        Some(core.generation),
    ));
    runtime.normal_start.lock().unwrap().replace(NormalStart {
        digest: digest(&saved).unwrap(),
        completion,
        identity: ProducerCell::queued(Arc::clone(&runtime.stop_domain), core.generation),
        attempt: None,
    });
    runtime.gate.begin();
    assert!(runtime.join_current_normal_start(&saved).unwrap().is_some());
    runtime.gate.bump_generation();
    assert!(runtime.join_current_normal_start(&saved).unwrap().is_none());
}

#[tokio::test]
async fn current_same_config_producer_is_shared_and_waiter_drop_never_stops_it() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    // Invalid input terminates before any spawn/probe after the gate is released.
    let mut first = runtime.normal_start_completion(Value::Null).unwrap();
    while matches!(
        &*first.borrow_and_update(),
        NormalStartCompletion::Pending(_)
    ) {
        first.changed().await.unwrap();
    }
    let generation = runtime.core_generation();
    let original_cell = runtime
        .admitted_native_producer(generation)
        .unwrap()
        .unwrap();
    let second = runtime.normal_start_completion(Value::Null).unwrap();
    assert!(Arc::ptr_eq(
        &original_cell,
        &runtime
            .admitted_native_producer(generation)
            .unwrap()
            .unwrap()
    ));
    assert_eq!(runtime.normal_start.lock().unwrap().retained.len(), 1);
    assert!(
        !original_cell.reclaimable(),
        "Starting is only observer state"
    );
    assert!(first.same_channel(&second));
    drop(first);
    drop(second);
    let mut completion = runtime
        .normal_start
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .completion
        .subscribe();
    drop(state_gate);
    while !matches!(
        &*completion.borrow_and_update(),
        NormalStartCompletion::Finished(Err(_), _)
    ) {
        completion.changed().await.unwrap();
    }
    assert_eq!(
        runtime.core_generation(),
        generation,
        "dropping all action waiters never invokes Stop"
    );
    assert!(!runtime.status().starting);
    assert!(
        original_cell.reclaimable(),
        "actual returned prebirth producer cannot enter factory again"
    );
    assert!(runtime.normal_start.lock().unwrap().retained.is_empty());
}

#[tokio::test]
async fn different_config_cannot_join_and_old_completion_cannot_follow_successor() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    let mut first = runtime.normal_start_completion(Value::Null).unwrap();
    while matches!(
        &*first.borrow_and_update(),
        NormalStartCompletion::Pending(_)
    ) {
        first.changed().await.unwrap();
    }
    let old_generation = runtime.core_generation();
    let old_cell = runtime
        .admitted_native_producer(old_generation)
        .unwrap()
        .unwrap();
    let mut second = runtime
        .normal_start_completion(serde_json::json!(false))
        .unwrap();
    assert!(!first.same_channel(&second));
    assert!(!old_cell.reclaimable());
    assert!(
        runtime
            .normal_start
            .lock()
            .unwrap()
            .retained
            .iter()
            .any(|cell| Arc::ptr_eq(&cell.identity, &old_cell)),
        "current B replacement cannot erase admitted A"
    );
    while matches!(
        &*second.borrow_and_update(),
        NormalStartCompletion::Pending(_)
    ) {
        second.changed().await.unwrap();
    }
    drop(state_gate);
    while !matches!(
        &*first.borrow_and_update(),
        NormalStartCompletion::Superseded
    ) {
        first.changed().await.unwrap();
    }
    while !matches!(
        &*second.borrow_and_update(),
        NormalStartCompletion::Finished(Err(_), _)
    ) {
        second.changed().await.unwrap();
    }
    assert!(runtime.ready_main.read().unwrap().is_none());
    assert!(old_cell.reclaimable());
    assert!(runtime.normal_start.lock().unwrap().retained.is_empty());
}

#[tokio::test]
async fn explicit_same_config_replaces_pending_action_producer_before_dispatch() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    let base = runtime.core_generation();
    // Neither producer can run yet on the current-thread executor. Explicit Start must
    // replace even an identical pending action producer; subsequent actions join the new one.
    let mut action = runtime.normal_start_completion(Value::Null).unwrap();
    let action_cell = Arc::clone(
        &runtime
            .normal_start
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .identity,
    );
    let mut explicit = runtime.explicit_start_completion(Value::Null).unwrap();
    let explicit_cell = Arc::clone(
        &runtime
            .normal_start
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .identity,
    );
    assert!(!Arc::ptr_eq(&action_cell, &explicit_cell));
    assert_eq!(runtime.normal_start.lock().unwrap().retained.len(), 2);
    let joined_action = runtime.normal_start_completion(Value::Null).unwrap();
    assert!(!action.same_channel(&explicit));
    assert!(explicit.same_channel(&joined_action));
    tokio::time::timeout(Duration::from_secs(2), async {
        while !matches!(
            &*action.borrow_and_update(),
            NormalStartCompletion::Superseded
        ) {
            action.changed().await.unwrap();
        }
        while matches!(
            &*explicit.borrow_and_update(),
            NormalStartCompletion::Pending(_)
        ) {
            explicit.changed().await.unwrap();
        }
    })
    .await
    .expect("the pending producer must yield to explicit Start");
    assert_eq!(runtime.core_generation(), base + 1);
    assert!(action_cell.reclaimable());
    assert!(!explicit_cell.reclaimable());
    assert_eq!(runtime.normal_start.lock().unwrap().retained.len(), 1);
    drop(state_gate);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !matches!(
            &*explicit.borrow_and_update(),
            NormalStartCompletion::Finished(Err(_), _)
        ) {
            explicit.changed().await.unwrap();
        }
    })
    .await
    .expect("invalid mock input must complete without starting a core");
    assert_eq!(runtime.core_generation(), base + 1);
    assert!(!runtime.status().starting);
    assert!(explicit_cell.reclaimable());
    assert!(runtime.normal_start.lock().unwrap().retained.is_empty());
}

#[tokio::test]
async fn latest_different_config_registered_before_dispatch_wins_once() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    let base = runtime.core_generation();
    // Current-thread runtime cannot poll either producer until both requests are registered.
    let mut first = runtime.normal_start_completion(Value::Null).unwrap();
    let mut second = runtime
        .normal_start_completion(serde_json::json!(false))
        .unwrap();
    assert!(!first.same_channel(&second));
    while !matches!(
        &*first.borrow_and_update(),
        NormalStartCompletion::Superseded
    ) {
        first.changed().await.unwrap();
    }
    while matches!(
        &*second.borrow_and_update(),
        NormalStartCompletion::Pending(_)
    ) {
        second.changed().await.unwrap();
    }
    assert_eq!(
        runtime.core_generation(),
        base + 1,
        "superseded queued Start cannot claim a generation"
    );
    drop(state_gate);
    while !matches!(
        &*second.borrow_and_update(),
        NormalStartCompletion::Finished(Err(_), _)
    ) {
        second.changed().await.unwrap();
    }
}

#[tokio::test]
async fn stop_between_pending_and_dispatch_then_new_start_uses_new_producer() {
    let (runtime, _dir) = runtime();
    let state_gate = runtime.mesh.tailscale_state_gate().await;
    let mut old = runtime.normal_start_completion(Value::Null).unwrap();
    let old_cell = Arc::clone(
        &runtime
            .normal_start
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .identity,
    );
    let stop_generation = runtime
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    let mut new = runtime.normal_start_completion(Value::Null).unwrap();
    assert!(
        !old.same_channel(&new),
        "the new explicit action cannot join Stop's obsolete pending producer"
    );
    while !matches!(&*old.borrow_and_update(), NormalStartCompletion::Superseded) {
        old.changed().await.unwrap();
    }
    while matches!(&*new.borrow_and_update(), NormalStartCompletion::Pending(_)) {
        new.changed().await.unwrap();
    }
    assert_eq!(runtime.core_generation(), stop_generation + 1);
    assert!(old_cell.reclaimable());
    assert_eq!(runtime.normal_start.lock().unwrap().retained.len(), 1);
    drop(state_gate);
    while !matches!(
        &*new.borrow_and_update(),
        NormalStartCompletion::Finished(Err(_), _)
    ) {
        new.changed().await.unwrap();
    }
}

#[tokio::test]
async fn guarded_requested_generation_is_not_final_native_birth_generation() {
    let (runtime, _dir) = runtime();
    let base = runtime
        .gate
        .claim_generation(None, polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    let gate = runtime.mesh.tailscale_state_gate().await;
    let producer_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move {
        producer_runtime
            .start_guarded(Value::Null, Some(base))
            .await
    });
    let (cell, mut completion) = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(start) = runtime.normal_start.lock().unwrap().as_ref() {
                if matches!(&*start.completion.borrow(), NormalStartCompletion::Starting(owner) if *owner == base) {
                    break (Arc::clone(&start.identity), start.completion.subscribe());
                }
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(cell.observation().0, base);
    assert_eq!(cell.observation().1, base);
    assert!(!cell.reclaimable());
    drop(gate);
    assert!(
        matches!(task.await.unwrap(), StartLeg::Finished(Err(_), Some(generation)) if generation == base + 1)
    );
    assert_eq!(
        cell.observation().1,
        base + 1,
        "record final atomic admission rather than first Starting watch"
    );
    assert!(
        matches!(&*completion.borrow_and_update(), NormalStartCompletion::Finished(Err(_), Some(generation)) if *generation == base + 1)
    );
    assert!(cell.reclaimable());
    assert!(runtime.normal_start.lock().unwrap().retained.is_empty());
}

#[tokio::test]
async fn completed_prebirth_normal_requests_do_not_accumulate_native_catalog_entries() {
    let (runtime, _dir) = runtime();
    for _ in 0..16 {
        let mut completion = runtime.explicit_start_completion(Value::Null).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !matches!(
                &*completion.borrow_and_update(),
                NormalStartCompletion::Finished(Err(_), _)
            ) {
                completion.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(
            runtime.normal_start.lock().unwrap().retained.is_empty(),
            "completed nonnative request creates no permanent single-Child obligations"
        );
    }
}

#[test]
fn candidate_cas_preserves_other_nodes_and_saved_provenance() {
    let mut original = saved();
    original["servers"][1]["sourceTag"] = serde_json::json!("original-import");
    original["servers"][1]["unknownField"] = serde_json::json!({"keep": true});
    let candidate = serde_json::json!({"id":"second", "name":"Candidate", "protocol":"tailscale", "sourceTag":"renderer", "tailscaleSettings":{"authKey":"synthetic-new"}});
    let mut actual = original.clone();
    merge_tailscale_candidate(&mut actual, &original, &candidate).unwrap();
    assert_eq!(actual["servers"][0], original["servers"][0]);
    assert_eq!(actual["selectedServerId"], original["selectedServerId"]);
    assert_eq!(
        actual["servers"][1]["sourceTag"],
        original["servers"][1]["sourceTag"]
    );
    assert_eq!(
        actual["servers"][1]["unknownField"],
        original["servers"][1]["unknownField"]
    );
    assert_eq!(
        actual["servers"][1]["tailscaleSettings"],
        candidate["tailscaleSettings"]
    );
    let mut changed = original.clone();
    changed["unrelatedConcurrentWriter"] = serde_json::json!(true);
    let before = changed.clone();
    assert!(merge_tailscale_candidate(&mut changed, &original, &candidate).is_err());
    assert_eq!(changed, before);
    let mut duplicated = original.clone();
    duplicated["servers"]
        .as_array_mut()
        .unwrap()
        .push(original["servers"][1].clone());
    assert!(merge_tailscale_candidate(&mut duplicated.clone(), &duplicated, &candidate).is_err());
}

#[tokio::test]
async fn action_producer_cannot_rebase_after_stop_or_cancellation() {
    let (runtime, _directory) = runtime();
    let registry = runtime.mesh.login_registry_for_test();
    registry.prepare("first", "scoped-action").await.unwrap();
    let attempt = registry.prepared_attempt_for_test("first", "scoped-action");
    let captured = runtime.core_generation();
    let stopped = runtime
        .gate
        .claim_generation(Some(captured), polaris_core_supervisor::LifecycleKind::Stop)
        .unwrap();
    assert!(runtime
        .register_normal_start(
            serde_json::json!(false),
            true,
            Some((captured, Arc::clone(&attempt)))
        )
        .is_err());
    assert_eq!(runtime.core_generation(), stopped);
    assert!(runtime.normal_start.lock().unwrap().as_ref().is_none());
    attempt.cancel();
    assert!(runtime
        .register_normal_start(serde_json::json!(false), true, Some((stopped, attempt)))
        .is_err());
    assert_eq!(runtime.core_generation(), stopped);
    assert!(runtime.normal_start.lock().unwrap().as_ref().is_none());
}

#[tokio::test]
async fn cancellation_after_queue_before_dispatch_cannot_birth_a_producer() {
    let (runtime, _directory) = runtime();
    let registry = runtime.mesh.login_registry_for_test();
    registry.prepare("first", "queued-action").await.unwrap();
    let attempt = registry.prepared_attempt_for_test("first", "queued-action");
    let captured = runtime.core_generation();
    let mut completion = runtime
        .register_normal_start(
            serde_json::json!(false),
            true,
            Some((captured, Arc::clone(&attempt))),
        )
        .unwrap();
    attempt.cancel();
    while matches!(
        &*completion.borrow_and_update(),
        NormalStartCompletion::Pending(_) | NormalStartCompletion::Starting(_)
    ) {
        completion.changed().await.unwrap();
    }
    assert!(matches!(
        &*completion.borrow(),
        NormalStartCompletion::Superseded
    ));
    assert_eq!(runtime.core_generation(), captured);
}

#[test]
fn browser_candidate_removes_old_key_and_preserves_unmodeled_ts_fields() {
    let mut original = saved();
    original["servers"][1]["tailscaleSettings"] = serde_json::json!({
        "authKey":"synthetic-old", "sourceTag":"original-ts-source",
        "unknownSetting":{"keep":true}, "exitNode":"synthetic-old-exit"
    });
    let candidate = serde_json::json!({"id":"second","protocol":"tailscale",
        "tailscaleSettings":{"hostname":"candidate", "sourceTag":"renderer"}});
    let mut actual = original.clone();
    merge_tailscale_candidate(&mut actual, &original, &candidate).unwrap();
    assert!(actual["servers"][1]["tailscaleSettings"]
        .get("authKey")
        .is_none());
    assert!(actual["servers"][1]["tailscaleSettings"]
        .get("exitNode")
        .is_none());
    assert_eq!(
        actual["servers"][1]["tailscaleSettings"]["unknownSetting"],
        original["servers"][1]["tailscaleSettings"]["unknownSetting"]
    );
    assert_eq!(
        actual["servers"][1]["tailscaleSettings"]["sourceTag"],
        original["servers"][1]["tailscaleSettings"]["sourceTag"]
    );
}

#[test]
fn credential_logout_rejects_partial_local_start_before_cold_reclassification() {
    assert_eq!(check_tailscale_logout_cold(false, false), Ok(true));
    assert_eq!(check_tailscale_logout_cold(true, false), Ok(false));
    for running in [false, true] {
        assert_eq!(
            check_tailscale_logout_cold(running, true),
            Err(MainPrerequisiteError::ReadyUnknown)
        );
    }
}

fn android_retirement_fixture() -> (
    Vec<polaris_mesh::tailscale_state::retirement::ObservedTailscaleStoreRun>,
    Value,
) {
    use polaris_mesh::tailscale_state::retirement::{
        ObservedTailscaleStoreRun, ObservedTailscaleStoreScope,
    };
    let scope = ObservedTailscaleStoreScope {
        tag: "selected".into(),
        state_directory: "/owned/tailscale/node".into(),
        state_file: "/owned/tailscale/node/tailscaled.state".into(),
    };
    let observed: Vec<_> = ['a', 'b']
        .into_iter()
        .map(|nonce| ObservedTailscaleStoreRun {
            run_nonce: nonce.to_string().repeat(64),
            config_digest: "c".repeat(64),
            scopes: vec![scope.clone()],
        })
        .collect();
    let instances: Vec<_> = observed.iter().map(|run| serde_json::json!({
        "runNonce":run.run_nonce, "configDigest":run.config_digest, "terminal":"SealedDrained", "censusComplete":true,
        "nodes":[{"tag":scope.tag, "stateDirectory":scope.state_directory, "stateFile":scope.state_file,
            "writerState":"SealedDrained", "stateFileState":"Regular", "stateFileRevision":"d".repeat(64),
            "profileState":"Bound", "profileFingerprint":"e".repeat(64)}]
    })).collect();
    (
        observed,
        serde_json::json!({"contractVersion":"polaris-ts-auth-writer-retirement-v1",
        "globalCleanupEvidence":"CleanupUnknown", "instances":instances}),
    )
}

#[test]
fn android_auth_selects_only_original_current_run_never_historical_bound() {
    use polaris_mesh::tailscale_state::retirement::{TailscaleProfileState, TailscaleStoreExport};
    let (observed, mut raw) = android_retirement_fixture();
    raw["instances"][1]["nodes"][0]["profileState"] = serde_json::json!("Unknown");
    raw["instances"][1]["nodes"][0]["profileFingerprint"] = serde_json::json!("");
    let export: TailscaleStoreExport = serde_json::from_value(raw.clone()).unwrap();
    let node = ProxyRuntime::selected_android_auth_node(
        &observed,
        &"c".repeat(64),
        "selected",
        "/owned/tailscale/node",
        &export,
    )
    .unwrap();
    assert_eq!(node.profile_state, TailscaleProfileState::Unknown);
    assert!(node.profile_fingerprint.is_empty());
    assert!(ProxyRuntime::selected_android_auth_node(
        &observed,
        &"f".repeat(64),
        "selected",
        "/owned/tailscale/node",
        &export
    )
    .is_err());
    assert!(ProxyRuntime::selected_android_auth_node(
        &observed,
        &"c".repeat(64),
        "selected",
        "/foreign/tailscale/node",
        &export
    )
    .is_err());
    raw["instances"].as_array_mut().unwrap().pop();
    let export = serde_json::from_value(raw).unwrap();
    assert!(ProxyRuntime::selected_android_auth_node(
        &observed,
        &"c".repeat(64),
        "selected",
        "/owned/tailscale/node",
        &export
    )
    .is_err());
}

#[test]
fn android_auth_rejects_partial_foreign_or_live_family_before_selecting_bound() {
    use polaris_mesh::tailscale_state::retirement::TailscaleStoreExport;
    let (observed, original) = android_retirement_fixture();
    let export: TailscaleStoreExport = serde_json::from_value(original.clone()).unwrap();
    assert!(ProxyRuntime::selected_android_auth_node(
        &observed,
        &"c".repeat(64),
        "selected",
        "/owned/tailscale/node",
        &export
    )
    .is_ok());
    for (key, value) in [
        ("censusComplete", serde_json::json!(false)),
        ("runNonce", serde_json::json!("f".repeat(64))),
        ("terminal", serde_json::json!("Unknown")),
        ("configDigest", serde_json::json!("f".repeat(64))),
    ] {
        let mut raw = original.clone();
        raw["instances"][0][key] = value;
        let export = serde_json::from_value(raw).unwrap();
        assert!(
            ProxyRuntime::selected_android_auth_node(
                &observed,
                &"c".repeat(64),
                "selected",
                "/owned/tailscale/node",
                &export
            )
            .is_err(),
            "{key}"
        );
    }
    let mut missing = observed.clone();
    missing[0].scopes.clear();
    assert!(ProxyRuntime::selected_android_auth_node(
        &missing,
        &"c".repeat(64),
        "selected",
        "/owned/tailscale/node",
        &export
    )
    .is_err());
}

#[test]
fn android_main_retirement_binds_raw_config_whole_claim_and_one_original_stop() {
    let source = crate::test_support::crate_source("runtime/proxy/prerequisite.rs");
    let main = source
        .split("pub(crate) async fn stop_android_tailscale_main_origin(")
        .nth(1)
        .unwrap()
        .split("pub(crate) fn ")
        .next()
        .unwrap();
    assert!(main.contains("core.committed.load"));
    assert!(main.contains("tailscale_main_scope_if_token"));
    assert!(main.contains("ticket.core.raw_config_digest"));
    assert!(main.contains("owner.birth.same(&birth)"));
    assert!(main.find("observe_main").unwrap() < main.find("stop_for_tailscale_action").unwrap());
    assert!(
        main.find("stop_for_tailscale_action").unwrap()
            < main.find("read_main_retirement").unwrap()
    );
    assert_eq!(main.matches("stop_for_tailscale_action(").count(), 1);
    let raw = source
        .split("let raw_config_digest = {")
        .nth(1)
        .unwrap()
        .split("let ios_receipt")
        .next()
        .unwrap();
    assert!(raw.contains("File::open(self.runtime_config_path())"));
    assert!(raw.contains("actual == emitted"));
    assert!(raw.contains("sha256_hex(&bytes)"));
    assert!(raw.contains("take(4 * 1024 * 1024 + 1)"));
}

use super::*;

mod readback_carrier;

#[test]
fn hmac_sha256_rfc4231_including_long_keys() {
    for (key,data,want) in [
  (vec![0x0b;20],"Hi There","b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
  (b"Jefe".to_vec(),"what do ya want for nothing?","5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"),
  (vec![0xaa;131],"Test Using Larger Than Block-Size Key - Hash Key First","60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"),
  (vec![0xaa;131],"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.","9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"),
 ]{assert_eq!(hex(&hmac256(&key,data.as_bytes())),want)}
}
fn fixture() -> Vec<u8> {
    let source =
        crate::test_support::repo_file("crates/singbox-grpc/src/runtime_status_handoff.hex");
    let text = source.trim();
    text.as_bytes()
        .chunks_exact(2)
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect()
}
fn verifier() -> Verifier {
    Verifier {
        key: Mutex::new(std::array::from_fn(|i| i as u8)),
        deadline: tokio::time::Instant::now() + HANDOFF_AUTH_LIFETIME,
        revoked: AtomicBool::new(false),
        generation: "11".repeat(16),
        endpoint: "127.0.0.1:39001".into(),
    }
}
fn envelope() -> polaris_singbox_grpc::RuntimeStatusEnvelope {
    polaris_singbox_grpc::RuntimeStatusEnvelope {
        status: daemon::RuntimeConfigStatus {
            active: Some(daemon::RuntimeIdentity {
                incarnation: "incarnation-A".into(),
                applied_revision: u64::MAX,
                effective_digest: "3".repeat(64),
            }),
            ..Default::default()
        },
        status_wire: fixture(),
        handoff: Some(polaris_singbox_grpc::ApiHandoffMetadata {
            protocol: PROTOCOL.into(),
            generation: "11".repeat(16),
            endpoint: "127.0.0.1:39001".into(),
            key: None,
            proof: Some("5629cc88b2e018dea1c4c419fc2c78f3a8510cac8bd8e65cc3cbf51472921287".into()),
            acknowledged: false,
        }),
    }
}
#[test]
fn actual_go_wire_proof_and_ack_are_domain_separated_and_bind_entire_body() {
    let v = verifier();
    let mut response = envelope();
    let txn = "22".repeat(16);
    let nonce = "44".repeat(32);
    assert_eq!(
        digest(&response.status_wire),
        "ab8bec568c0d7aaeab5843ce03908d48fbc8f76ef4f74b54d0f9ebb185e7523f"
    );
    assert!(verify_wire_proof(&v, &txn, &response, &nonce, false));
    assert!(!verify_wire_proof(&v, &txn, &response, &nonce, true));
    let active = response.status.active.as_ref().unwrap();
    assert_eq!(
        hex(&hmac256(
            &*v.key.lock().unwrap(),
            transcript("ack", &v.generation, &txn, active, &v.endpoint, &nonce, "").as_bytes()
        )),
        "7ca7710b3d40664b6f6af3cb9e73ece6880c4682998d839d5a77cfb354d2d1e4"
    );
    // Claiming ack on an ordinary proof must fail even though identity/body stay identical.
    response.handoff.as_mut().unwrap().acknowledged = true;
    assert!(!verify_wire_proof(&v, &txn, &response, &nonce, true));
    response.handoff.as_mut().unwrap().proof =
        Some("25d35530e20c0afe79a2c1d38809102c210946b774d96284e10d09e3035860be".into());
    assert!(verify_wire_proof(&v, &txn, &response, &nonce, true));
    response.handoff.as_mut().unwrap().acknowledged = false;
    assert!(!verify_wire_proof(&v, &txn, &response, &nonce, false));
    for change in 0..6 {
        let mut response = envelope();
        match change {
            0 => response.status_wire.extend_from_slice(&[0xa0, 0x06, 0x01]),
            1 => {
                let at = response
                    .status_wire
                    .windows(3)
                    .position(|b| b == b"D:9")
                    .unwrap();
                response.status_wire[at + 2] = b'8';
            }
            2 => response.handoff.as_mut().unwrap().generation = "12".repeat(16),
            3 => response.handoff.as_mut().unwrap().key = Some(hex(&*v.key.lock().unwrap())),
            4 => response.handoff.as_mut().unwrap().endpoint = "127.0.0.1:39002".into(),
            _ => response.handoff.as_mut().unwrap().proof = Some("0".repeat(64)),
        }
        assert!(!verify_wire_proof(&v, &txn, &response, &nonce, false));
    }
    assert!(!verify_wire_proof(
        &v,
        &txn,
        &envelope(),
        &"45".repeat(32),
        false
    ));
}
struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release)
    }
}
#[tokio::test]
async fn relay_epoch_joins_actual_task_and_waits_for_stream_borrower() {
    let epoch = Arc::new(ApiRelayEpoch::default());
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    epoch.spawn(async move {
        let _tail = Dropped(flag);
        let _ = entered.send(());
        std::future::pending::<()>().await
    });
    ready.await.unwrap();
    let borrow = epoch.borrow();
    let owner = epoch.clone();
    let join = tokio::spawn(async move { owner.seal_and_join().await });
    borrow.cancelled().await;
    assert!(!join.is_finished());
    drop(borrow);
    assert!(join.await.unwrap().is_ok());
    assert!(dropped.load(Ordering::Acquire));
    assert!(epoch.tasks.lock().unwrap().is_empty());
    let called = Arc::new(AtomicBool::new(false));
    let mark = called.clone();
    epoch.spawn(async move { mark.store(true, Ordering::Release) });
    assert!(!called.load(Ordering::Acquire));
}
#[tokio::test]
async fn cancelled_join_waiter_keeps_owned_task_for_retry() {
    let epoch = Arc::new(ApiRelayEpoch::default());
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    epoch.spawn(async move {
        let _tail = Dropped(flag);
        let _ = entered.send(());
        std::future::pending::<()>().await
    });
    ready.await.unwrap();
    {
        let seal = epoch.seal_and_join();
        tokio::pin!(seal);
        assert!(futures::poll!(&mut seal).is_pending());
    }
    assert!(!epoch.tasks.lock().unwrap().is_empty());
    assert!(epoch.seal_and_join().await.is_ok());
    assert!(dropped.load(Ordering::Acquire));
}

#[derive(Clone)]
struct ReadyApiFixture {
    caps: daemon::RuntimeCapabilities,
}
#[polaris_singbox_grpc::tonic::async_trait]
impl daemon::runtime_config_service_server::RuntimeConfigService for ReadyApiFixture {
    async fn get_runtime_capabilities(
        &self,
        _: polaris_singbox_grpc::tonic::Request<()>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeCapabilities>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Ok(polaris_singbox_grpc::tonic::Response::new(
            self.caps.clone(),
        ))
    }
    async fn plan_runtime_config(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::PlanRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigPlan>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented(
            "not an Apply fixture",
        ))
    }
    async fn apply_runtime_config(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::ApplyRuntimeConfigRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        panic!("API view renewal must not reapply")
    }
    async fn get_runtime_config_status(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::RuntimeConfigStatusRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigStatus>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented(
            "not a handoff proof fixture",
        ))
    }
    async fn retry_runtime_persistence(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::RuntimePersistenceRetryRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented("unused"))
    }
    async fn acknowledge_runtime_persistence(
        &self,
        _: polaris_singbox_grpc::tonic::Request<daemon::RuntimePersistenceAcknowledgementRequest>,
    ) -> Result<
        polaris_singbox_grpc::tonic::Response<daemon::RuntimeConfigOperation>,
        polaris_singbox_grpc::tonic::Status,
    > {
        Err(polaris_singbox_grpc::tonic::Status::unimplemented("unused"))
    }
}

async fn ready_api_peer(
    listener: tokio::net::TcpListener,
    caps: daemon::RuntimeCapabilities,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), polaris_singbox_grpc::tonic::transport::Error>>,
) {
    let incoming = futures::stream::unfold(listener, |listener| async move {
        let next = listener.accept().await.map(|(stream, _)| stream);
        Some((next, listener))
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        polaris_singbox_grpc::tonic::transport::Server::builder()
            .serve_with_incoming_shutdown(
                daemon::runtime_config_service_server::RuntimeConfigServiceServer::new(
                    ReadyApiFixture { caps },
                ),
                incoming,
                async {
                    let _ = stopped.await;
                },
            )
            .await
    });
    (stop, server)
}

// The proof verifier is covered above against actual-Go-wire vectors. Here its
// private confirmed projection is the unit boundary; the real normal producer
// mints a new view and makes a fresh RPC, while the native ReadyMainCore Arc is
// retained. This does not claim an end-to-end Core binary handshake.
#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn normal_producer_renews_only_confirmed_same_core_api_view() {
    use crate::runtime::proxy::prerequisite::{
        ActionBinding, MainPrerequisiteError, NormalMainAction,
    };
    for change in ["secret", "port"] {
        let old_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let old_port = old_listener.local_addr().unwrap().port();
        let listener = if change == "port" {
            Some(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap())
        } else {
            None
        };
        let port = listener
            .as_ref()
            .map_or(old_port, |l| l.local_addr().unwrap().port());
        let mut peers = Vec::new();
        let (rt, mut old_ticket, _directory) =
            ProxyRuntime::ready_api_commit_fixture_for_test(old_port, "old".into());
        let old = rt.config.current().unwrap();
        *rt.current_config.write().unwrap() = Some(old.clone());
        let typed: UserConfig = serde_json::from_value(old.clone()).unwrap();
        let deps = rt.generate_deps(old_port, 0, 0, None, &[], &old, false);
        let generated = polaris_config_engine::builder::generate_sing_box_config(
            &typed,
            &BTreeMap::new(),
            &deps,
        )
        .unwrap();
        let caps = super::super::tests::paired_capabilities();
        let active = caps.active.clone().unwrap();
        let pairing = observe_runtime_pairing(&caps).unwrap();
        let generation = rt.core_generation();
        rt.capture_runtime_config_baseline(generation, deps, generated, typed, pairing);
        let core = rt.ready_main.read().unwrap().clone().unwrap();
        {
            let mut b = rt.runtime_config_baseline.write().unwrap();
            let b = b.as_mut().unwrap();
            b.management = Some(ApiEndpoint {
                host: "127.0.0.1".into(),
                port: old_port,
                secret: "old".into(),
            });
        }
        peers.push(ready_api_peer(old_listener, caps.clone()).await);
        let old_control =
            SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", old_port), "old")
                .await
                .unwrap();
        old_control.get_runtime_capabilities().await.unwrap();
        rt.bind_runtime_api_control(generation, Arc::new(old_control));
        old_ticket.api_view = rt.api_view_for_ready_ticket(&core, &core.saved_digest);
        assert!(old_ticket.api_view.is_some());
        assert!(rt.check_ready_main(&old_ticket).is_ok());
        let original_readback = rt.readback_binding(Some(&old_ticket)).unwrap();
        assert_eq!(original_readback.port, old_port);
        assert!(Arc::ptr_eq(
            original_readback.epoch.as_ref().unwrap(),
            &old_ticket.api_view.as_ref().unwrap().epoch,
        ));
        let mut next = old.clone();
        let new_secret = if change == "secret" { "new" } else { "old" };
        if change == "secret" {
            next["clashApiSecret"] = serde_json::json!(new_secret)
        } else {
            next["controlApiPort"] = serde_json::json!(port)
        }
        let plan = {
            let b = rt.runtime_config_baseline.read().unwrap();
            rt.api_saved_projection_for(&next, b.as_ref().unwrap())
                .unwrap()
        };
        let mut mixed = next.clone();
        mixed["logLevel"] = serde_json::json!("debug");
        assert!(rt
            .api_saved_projection_for(
                &mixed,
                rt.runtime_config_baseline.read().unwrap().as_ref().unwrap()
            )
            .is_none());
        rt.config.save_full_deferred_cleanup(&old, &next).unwrap();
        {
            let mut b = rt.runtime_config_baseline.write().unwrap();
            let b = b.as_mut().unwrap();
            let epoch = Arc::new(ApiRelayEpoch::default());
            let endpoint = ApiEndpoint {
                host: "127.0.0.1".into(),
                port,
                secret: new_secret.into(),
            };
            b.api_epoch = epoch.clone();
            b.management = Some(endpoint.clone());
            b.api_saved_projection = Some(ApiSavedProjection {
                owner: plan.owner,
                original: plan.original,
                current: plan.current,
                active: active.clone(),
                epoch,
                endpoint,
            });
        }
        *rt.status.write().unwrap() = crate::runtime::proxy::ProxyStatus {
            running: true,
            clash_api_port: port,
            main_generation: generation,
            ..Default::default()
        };
        assert!(matches!(
            rt.check_ready_main(&old_ticket),
            Err(MainPrerequisiteError::ApiViewStale)
        ));
        assert!(rt.readback_endpoint(Some(&old_ticket)).is_none());
        if let Some(listener) = listener {
            peers.push(ready_api_peer(listener, caps).await);
        }
        let control =
            SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", port), new_secret)
                .await
                .unwrap();
        control.get_runtime_capabilities().await.unwrap();
        rt.bind_runtime_api_control(generation, Arc::new(control));
        let binding = ActionBinding::new(
            NormalMainAction::ManualNetworkCheck,
            "renew-api".into(),
            &next,
            vec![],
            None,
        )
        .unwrap();
        let renewed = rt.await_normal_main(binding).await.unwrap();
        assert_eq!(renewed.generation(), generation);
        assert_eq!(renewed.api_port(), port);
        assert_eq!(renewed.api_secret(), new_secret);
        let readback = rt.readback_binding(Some(&renewed)).unwrap();
        assert_eq!(readback.port, port);
        assert_eq!(readback.secret, new_secret);
        assert!(Arc::ptr_eq(
            readback.epoch.as_ref().unwrap(),
            &renewed.api_view.as_ref().unwrap().epoch,
        ));
        assert!(!Arc::ptr_eq(
            readback.epoch.as_ref().unwrap(),
            original_readback.epoch.as_ref().unwrap(),
        ));
        assert!(Arc::ptr_eq(
            &core,
            rt.ready_main.read().unwrap().as_ref().unwrap()
        ));
        assert_eq!(rt.core_generation(), generation);
        assert!(rt.check_ready_main(&old_ticket).is_err());
        *rt.current_config.write().unwrap() = Some(next.clone());
        let mut twice = next.clone();
        twice["clashApiSecret"] = serde_json::json!("third");
        let second = {
            let b = rt.runtime_config_baseline.read().unwrap();
            rt.api_saved_projection_for(&twice, b.as_ref().unwrap())
                .unwrap()
        };
        assert_eq!(second.original, core.saved_digest);
        assert!(second
            .owner
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, &core)));
        // Even a later cycle to the original tuple cannot revive an old API epoch.
        {
            let mut b = rt.runtime_config_baseline.write().unwrap();
            let b = b.as_mut().unwrap();
            b.management = Some(ApiEndpoint {
                host: "127.0.0.1".into(),
                port: old_port,
                secret: "old".into(),
            });
            b.api_epoch = Arc::default();
        }
        assert!(matches!(
            rt.check_ready_main(&old_ticket),
            Err(MainPrerequisiteError::ApiViewStale)
        ));
        rt.gate.bump_generation();
        assert!(rt
            .api_view_for_ready_ticket(&core, &core.saved_digest)
            .is_none());
        assert!(matches!(
            rt.check_ready_main(&old_ticket),
            Err(MainPrerequisiteError::Superseded)
        ));
        rt.clear_runtime_config_baseline();
        assert!(rt
            .api_view_for_ready_ticket(&core, &core.saved_digest)
            .is_none());
        drop(original_readback);
        drop(readback);
        for (stop, server) in peers {
            let _ = stop.send(());
            tokio::time::timeout(std::time::Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn local_auth_deadline_clears_idle_cached_proof_and_ack_loss_material() {
    for phase in ["key", "proof", "ack-loss"] {
        let origin = Arc::new(
            SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", 1), "old")
                .await
                .unwrap(),
        );
        let h = ApiChannelHandoff::new(
            origin,
            ApiEndpoint {
                host: "127.0.0.1".into(),
                port: 39001,
                secret: "new".into(),
            },
            Arc::default(),
            None,
        );
        let v = Arc::new(verifier());
        *h.verifier.lock().unwrap() = Some(v.clone());
        let mut response = envelope();
        if phase == "ack-loss" {
            response.handoff.as_mut().unwrap().acknowledged = true;
            response.handoff.as_mut().unwrap().proof =
                Some("25d35530e20c0afe79a2c1d38809102c210946b774d96284e10d09e3035860be".into());
        }
        assert!(verify_wire_proof(
            &v,
            &"22".repeat(16),
            &response,
            &"44".repeat(32),
            phase == "ack-loss"
        ));
        tokio::task::yield_now().await;
        tokio::time::advance(HANDOFF_AUTH_LIFETIME).await;
        tokio::task::yield_now().await;
        // Observe timer effects before any has_verifier()/confirmation call.
        assert!(h.verifier.lock().unwrap().is_none(), "{phase}");
        assert_eq!(*v.key.lock().unwrap(), [0; 32]);
        assert!(!verify_wire_proof(
            &v,
            &"22".repeat(16),
            &response,
            &"44".repeat(32),
            phase == "ack-loss"
        ));
        assert!(!h.has_verifier().await);
        assert!(!h.confirmed());
        assert!(!h.origin_epoch.is_closed());
    }
}

#[tokio::test]
async fn local_stop_and_drop_revoke_even_a_verifier_borrowed_by_inflight_confirmation() {
    for stop in [true, false] {
        let origin = Arc::new(
            SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", 1), "old")
                .await
                .unwrap(),
        );
        let h = ApiChannelHandoff::new(
            origin,
            ApiEndpoint {
                host: "127.0.0.1".into(),
                port: 39001,
                secret: "new".into(),
            },
            Arc::default(),
            None,
        );
        let v = Arc::new(verifier());
        *h.verifier.lock().unwrap() = Some(v.clone());
        if stop {
            h.revoke();
        }
        drop(h);
        assert_eq!(*v.key.lock().unwrap(), [0; 32]);
        assert!(!verify_wire_proof(
            &v,
            &"22".repeat(16),
            &envelope(),
            &"44".repeat(32),
            false
        ));
    }
}

#[cfg(not(target_os = "ios"))]
#[tokio::test]
async fn deferred_native_save_callback_does_not_borrow_and_resumes_with_fresh_confirmed_view() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (rt, _, _directory) = ProxyRuntime::ready_api_commit_fixture_for_test(port, "old".into());
    let saved = rt.config.current().unwrap();
    let typed: UserConfig = serde_json::from_value(saved.clone()).unwrap();
    let deps = rt.generate_deps(port, 0, 0, None, &[], &saved, false);
    let generated =
        polaris_config_engine::builder::generate_sing_box_config(&typed, &BTreeMap::new(), &deps)
            .unwrap();
    let caps = super::super::tests::paired_capabilities();
    let generation = rt.core_generation();
    rt.capture_runtime_config_baseline(
        generation,
        deps,
        generated,
        typed,
        observe_runtime_pairing(&caps).unwrap(),
    );
    *rt.switch_snapshot.write().unwrap() = Some(super::super::super::hot_switch::SwitchSnapshot {
        id_to_tag: BTreeMap::from([("taildrop-node".into(), "emitted-node".into())]),
        ..Default::default()
    });
    let origin_epoch = rt
        .runtime_config_baseline
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .api_epoch
        .clone();
    let (reply, callback) = tokio::sync::oneshot::channel::<Option<String>>();
    let dialog = crate::commands::taildrop::connect_after_taildrop_dialog(
        &rt,
        "taildrop-node",
        generation,
        "emitted-node",
        async { callback.await.ok().flatten() },
    );
    tokio::pin!(dialog);
    assert!(futures::poll!(&mut dialog).is_pending());
    // No native UI response, no socket work: actual epoch join must complete.
    assert!(origin_epoch.seal_and_join().await.is_ok());
    assert!(futures::poll!(&mut dialog).is_pending());
    assert!(crate::commands::taildrop::connect_after_taildrop_dialog(
        &rt,
        "taildrop-node",
        generation,
        "emitted-node",
        std::future::ready(Some("unknown-view"))
    )
    .await
    .is_err());
    assert!(crate::commands::taildrop::connect_after_taildrop_dialog(
        &rt,
        "taildrop-node",
        generation,
        "emitted-node",
        std::future::ready(None::<String>)
    )
    .await
    .unwrap()
    .is_none());
    let incoming = futures::stream::unfold(listener, |l| async move {
        let next = l.accept().await.map(|(s, _)| s);
        Some((next, l))
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        polaris_singbox_grpc::tonic::transport::Server::builder()
            .serve_with_incoming_shutdown(
                daemon::runtime_config_service_server::RuntimeConfigServiceServer::new(
                    ReadyApiFixture { caps },
                ),
                incoming,
                async {
                    let _ = stopped.await;
                },
            )
            .await
    });
    let control = SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", port), "new")
        .await
        .unwrap();
    control.get_runtime_capabilities().await.unwrap();
    let next_epoch = Arc::new(ApiRelayEpoch::default());
    {
        let mut b = rt.runtime_config_baseline.write().unwrap();
        let b = b.as_mut().unwrap();
        b.management = Some(ApiEndpoint {
            host: "127.0.0.1".into(),
            port,
            secret: "new".into(),
        });
        b.api_epoch = next_epoch.clone();
        b.control = Some(Arc::new(control));
    }
    reply.send(Some("chosen-destination".into())).unwrap();
    let (picked, client, tag) = dialog.await.unwrap().unwrap();
    assert_eq!(picked, "chosen-destination");
    assert_eq!(tag, "emitted-node");
    assert!(client.captured_transport_live());
    client.get_runtime_capabilities().await.unwrap();
    assert_eq!(rt.core_generation(), generation);
    assert_eq!(origin_epoch.state.lock().unwrap().1, 0);
    assert_eq!(next_epoch.state.lock().unwrap().1, 1);
    drop(client);
    assert!(next_epoch.seal_and_join().await.is_ok());
    rt.gate.bump_generation();
    assert!(crate::commands::taildrop::connect_after_taildrop_dialog(
        &rt,
        "taildrop-node",
        generation,
        "emitted-node",
        std::future::ready(Some("stale-core"))
    )
    .await
    .is_err());
    rt.clear_runtime_config_baseline();
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

// The five production entry wiring guards live with commands::speedtest. The
// actual epoch owner stays private here; this regression uses the same witness
// task implementation without broadening production disposal authority.
#[tokio::test]
async fn scoped_readback_joins_the_actual_pending_open_and_stream_tasks() {
    use crate::commands::speedtest::StreamWitness;
    use futures::StreamExt;
    struct Unwound(Arc<AtomicBool>);
    impl Drop for Unwound {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    for opening in [true, false] {
        let epoch = Arc::new(ApiRelayEpoch::default());
        let unwound = Arc::new(AtomicBool::new(false));
        let guard = Unwound(unwound.clone());
        let (entered, polled) = tokio::sync::oneshot::channel();
        let witness = StreamWitness::spawn_in_api_epoch(
            async move {
                if opening {
                    let _guard = guard;
                    let _ = entered.send(());
                    return std::future::pending().await;
                }
                Ok::<_, String>(
                    futures::stream::once(async move {
                        let _guard = guard;
                        let _ = entered.send(());
                        std::future::pending().await
                    })
                    .boxed(),
                )
            },
            Some(epoch.clone()),
        );
        polled.await.unwrap();
        assert!(!unwound.load(Ordering::Acquire));
        assert!(epoch.seal_and_join().await.is_ok());
        assert!(
            unwound.load(Ordering::Acquire),
            "join returned before real readback task unwound"
        );
        drop(witness);
        let polled_after_seal = Arc::new(AtomicBool::new(false));
        let flag = polled_after_seal.clone();
        let late = StreamWitness::spawn_in_api_epoch(
            async move {
                flag.store(true, Ordering::Release);
                Ok::<_, String>(futures::stream::empty())
            },
            Some(epoch),
        );
        tokio::task::yield_now().await;
        assert!(
            !polled_after_seal.load(Ordering::Acquire),
            "sealed readback constructor performed transport work"
        );
        drop(late);
    }
}

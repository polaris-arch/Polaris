use super::super::android_bridge::{AndroidExactTarget, AndroidStartReceipt};
use super::super::android_probe_loan::DebugCoreProbeSessionScope;
use super::*;

// Pure fixture inputs. No Kotlin/JNI/device/core data are manufactured by production.
fn scope() -> DebugCoreProbeSessionScope {
    DebugCoreProbeSessionScope {
        boot_nonce: "1".repeat(32),
        session_id: "2".repeat(32),
        nonce: "3".repeat(48),
        plan_sha256: "4".repeat(64),
        apk_sha256: "5".repeat(64),
        expected_source_pin: "6".repeat(64),
        run_id: "probe-fixture".into(),
        birth_nonce: "fixture-birth".into(),
        revision: 1,
        config_digest: "a".repeat(64),
        deadline_elapsed: 9000,
        sampled_elapsed: 1000,
    }
}

fn install(
    rt: &ProxyRuntime,
    scope: &DebugCoreProbeSessionScope,
) -> (u64, super::super::AndroidRequestBirth) {
    let generation = rt.bump_generation();
    let birth = rt.book_android_global_start(None).unwrap();
    let receipt = AndroidStartReceipt {
        run_id: scope.run_id.clone(),
        birth_nonce: scope.birth_nonce.clone(),
        config_digest: scope.config_digest.clone(),
        claim: None,
        tun: None,
    };
    rt.confirm_android_global_start(&birth, receipt.exact_target())
        .unwrap();
    rt.record_android_probe_start(generation, &birth, &receipt);
    rt.publish_android_probe_snapshot(
        generation,
        SwitchSnapshot {
            probe_proxy_port: Some(19385),
            loopback_auth: Some(InboundUser {
                username: "polaris".into(),
                password: "b".repeat(32),
            }),
            ..SwitchSnapshot::default()
        },
    );
    rt.status.write().unwrap().running = true;
    (generation, birth)
}

#[tokio::test]
async fn android_probe_loan_actual_custody_publication_and_collector_bind_start_bytes() {
    let (rt, _dir) = test_runtime();
    let s = scope();
    let (generation, _) = install(&rt, &s);
    let payload = rt.collect_android_probe_loan(&s).await.unwrap();
    let wire = serde_json::to_value(payload).unwrap();
    assert_eq!(wire["generation"], generation.to_string());
    assert_eq!(wire["configDigest"], s.config_digest);
    assert_eq!(wire["probePort"], 19385);
    assert_eq!(wire["password"].as_array().unwrap().len(), 32);
    // Current/saved user data do not stand in for the actual sent Start receipt.
    *rt.current_config.write().unwrap() = Some(serde_json::json!({"digest": "unrelated"}));
    assert!(rt.collect_android_probe_loan(&s).await.is_ok());
    let mut reload = s.clone();
    reload.config_digest = "c".repeat(64);
    assert!(rt.collect_android_probe_loan(&reload).await.is_err());
}

#[tokio::test]
async fn android_probe_loan_changed_generation_selector_unknown_and_missing_publication_reject() {
    for mutation in 0..6 {
        let (rt, _dir) = test_runtime();
        let s = scope();
        install(&rt, &s);
        match mutation {
            0 => {
                rt.gate.claim_generation(None, LifecycleKind::Stop);
            }
            1 => {
                rt.register_selector_intent();
            }
            2 => {
                rt.android_main_token
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .historic_unknown = true;
            }
            3 => {
                rt.switch_snapshot
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .android_probe_input = None;
            }
            4 => {
                rt.switch_snapshot
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .loopback_auth = None;
            }
            5 => {
                rt.status.write().unwrap().running = false;
            }
            _ => unreachable!(),
        }
        assert!(
            rt.collect_android_probe_loan(&s).await.is_err(),
            "mutation {mutation}"
        );
    }
    let (rt, _dir) = test_runtime();
    let mut s = scope();
    install(&rt, &s);
    s.sampled_elapsed = s.deadline_elapsed;
    assert!(rt.collect_android_probe_loan(&s).await.is_err());
}

#[tokio::test]
async fn android_probe_loan_stop_claim_while_waiting_existing_state_gate_cannot_borrow_old_auth() {
    let (rt, _dir) = test_runtime();
    let s = scope();
    install(&rt, &s);
    let state = rt.mesh.tailscale_state_gate().await;
    let collector = {
        let rt = Arc::clone(&rt);
        tokio::spawn(async move { rt.collect_android_probe_loan(&s).await })
    };
    tokio::task::yield_now().await;
    rt.gate.claim_generation(None, LifecycleKind::Stop);
    drop(state);
    assert!(tokio::time::timeout(Duration::from_secs(3), collector)
        .await
        .unwrap()
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn android_probe_loan_late_start_receipt_and_old_snapshot_cannot_publish_after_stop() {
    let (rt, _dir) = test_runtime();
    let s = scope();
    let (old, birth) = install(&rt, &s);
    rt.gate.claim_generation(None, LifecycleKind::Stop);
    rt.android_main_token
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .debug_probe_input = None;
    rt.record_android_probe_start(
        old,
        &birth,
        &AndroidStartReceipt {
            run_id: s.run_id.clone(),
            birth_nonce: s.birth_nonce.clone(),
            config_digest: s.config_digest.clone(),
            claim: None,
            tun: None,
        },
    );
    assert!(rt
        .android_main_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .debug_probe_input
        .is_none());
    rt.switch_snapshot.write().unwrap().take();
    rt.publish_android_probe_snapshot(old, SwitchSnapshot::default());
    assert!(rt.switch_snapshot.read().unwrap().is_none());
    let wrong = AndroidExactTarget {
        run_id: s.run_id.clone(),
        birth_nonce: "another-birth".into(),
    };
    assert!(rt.confirm_android_global_start(&birth, wrong).is_err());
}

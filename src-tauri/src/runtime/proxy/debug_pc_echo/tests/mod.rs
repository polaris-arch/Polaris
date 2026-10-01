use crate::runtime::proxy::android_probe_loan::DebugCoreProbeSessionScope;
use crate::runtime::proxy::debug_pc_echo::PrivatePcReady;
fn scope() -> DebugCoreProbeSessionScope {
    DebugCoreProbeSessionScope {
        boot_nonce: "1".repeat(32),
        session_id: "2".repeat(32),
        nonce: "3".repeat(48),
        plan_sha256: "4".repeat(64),
        apk_sha256: "5".repeat(64),
        expected_source_pin: "6".repeat(64),
        run_id: "logic-only".into(),
        birth_nonce: "logic-birth".into(),
        revision: 1,
        config_digest: "7".repeat(64),
        deadline_elapsed: 9000,
        sampled_elapsed: 1000,
    }
}
fn fixture() -> serde_json::Value {
    serde_json::json!({"requestId":"8".repeat(32),"deadlineElapsed":2800,"target":{
        "schema":"polaris-pc-echo-target-v1","pcRunId":"1".repeat(32),"pcPlanSha256":"2".repeat(64),
        "readyReceiptSha256":"3".repeat(64),"pcCandidateSha":"4".repeat(40),"androidCandidateSha":"5".repeat(40),
        "receiverSourceSha256":"6".repeat(64),"androidPackageSha256":"5".repeat(64),"receiverInstanceId":"7".repeat(32),
        "tcpSocketInstanceId":"8".repeat(32),"udpSocketInstanceId":"9".repeat(32),"destinationIPv4":"192.168.1.1",
        "expectedSenderIPv4":"192.168.1.2","tcpPort":50001,"udpPort":50002,"echoNonce":"a".repeat(32),
        "echoNonceSha256":polaris_updater::verify::sha256_hex("a".repeat(32).as_bytes()),"lifetimeSeconds":120,"maxRequests":32}})
}
#[test]
fn private_bridge_shape_is_closed_and_native_scope_bounds_the_original_deadline() {
    let valid: PrivatePcReady = serde_json::from_value(fixture()).unwrap();
    let attempt = valid.admit(&scope()).unwrap();
    assert_eq!(attempt.request_id(), "8".repeat(32));
    assert_eq!(attempt.deadline_elapsed(), 2800);
    for (key, value) in [
        ("approved", serde_json::json!(true)),
        ("deadlineElapsed", serde_json::json!(true)),
    ] {
        let mut v = fixture();
        v[key] = value;
        assert!(serde_json::from_value::<PrivatePcReady>(v).is_err());
    }
    for (key, value) in [
        ("tcpPort", serde_json::json!(true)),
        ("extra", serde_json::json!("authority")),
    ] {
        let mut v = fixture();
        v["target"][key] = value;
        assert!(serde_json::from_value::<PrivatePcReady>(v).is_err());
    }
    for (key, value) in [
        ("androidPackageSha256", serde_json::json!("0".repeat(64))),
        ("destinationIPv4", serde_json::json!("8.8.8.8")),
        ("udpPort", serde_json::json!(50001)),
        ("maxRequests", serde_json::json!(31)),
    ] {
        let mut v = fixture();
        v["target"][key] = value;
        assert!(serde_json::from_value::<PrivatePcReady>(v)
            .unwrap()
            .admit(&scope())
            .is_err());
    }
    for deadline in [1000, 9001, 122000] {
        let mut v = fixture();
        v["deadlineElapsed"] = serde_json::json!(deadline);
        assert!(serde_json::from_value::<PrivatePcReady>(v)
            .unwrap()
            .admit(&scope())
            .is_err());
    }
    let raw = serde_json::to_string(&fixture()).unwrap();
    let duplicate = raw.replacen("{", "{\"requestId\":\"copied\",", 1);
    assert!(serde_json::from_str::<PrivatePcReady>(&duplicate).is_err());
}

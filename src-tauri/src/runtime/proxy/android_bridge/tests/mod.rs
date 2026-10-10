use super::*;

#[test]
fn capacity_rejection_survives_only_the_explicit_bridge_code() {
    assert_eq!(
        map_rejected_code(Some(code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED)),
        code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED
    );
    for code in [
        None,
        Some("ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED "),
        Some("ANDROID_NATIVE_ADMISSION_CLOSED"),
        Some("restart app"),
    ] {
        assert_eq!(map_rejected_code(code), code::STARTUP_FAILED);
    }
}

#[test]
fn check_config_optional_code_has_a_typed_cause_and_never_guesses_raw_text() {
    let accepted: CheckResponse = serde_json::from_str("{}").unwrap();
    assert!(matches!(
        check_response_verdict(accepted),
        Ok(ConfigCheckVerdict::Accepted)
    ));
    let capacity: CheckResponse = serde_json::from_value(serde_json::json!({"error":"private text", "errorCode":code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED})).unwrap();
    assert!(matches!(
        check_response_verdict(capacity),
        Err(CapacityClosed)
    ));
    for code in [
        None,
        Some("unknown"),
        Some("ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED "),
    ] {
        let raw: CheckResponse = serde_json::from_value(
            serde_json::json!({"error":CapacityClosed.to_string(), "errorCode":code}),
        )
        .unwrap();
        assert!(matches!(
            check_response_verdict(raw),
            Ok(ConfigCheckVerdict::Unattributable(_))
        ));
    }
}

#[test]
fn actual_close_failure_has_priority_over_capacity_and_never_becomes_an_exact_receipt() {
    assert!(matches!(
        speedtest_start_failure(Some(CapacityClosed), Ok(())),
        SpeedtestStartError::CapacityClosed(_)
    ));
    assert!(matches!(
        speedtest_start_failure(
            Some(CapacityClosed),
            Err(TransientCloseError::CapacityClosed(CapacityClosed))
        ),
        SpeedtestStartError::CapacityClosed(_)
    ));
    match speedtest_start_failure(
        Some(CapacityClosed),
        Err(TransientCloseError::Failed("close/network failed".into())),
    ) {
        SpeedtestStartError::CleanupUnknown(message) => {
            assert_eq!(message, "Android 测速临时核关闭结果未知；本轮已停止")
        }
        other => panic!("real cleanup must win: {other:?}"),
    }
    assert!(matches!(
        speedtest_start_failure(
            None,
            Err(TransientCloseError::CapacityClosed(CapacityClosed))
        ),
        SpeedtestStartError::CleanupUnknown(_)
    ));
    match speedtest_start_failure(None, Ok(())) {
        SpeedtestStartError::Failed(message) => {
            assert_eq!(message, "Android 测速临时核启动失败或超时")
        }
        other => panic!("old fallback changed: {other:?}"),
    }
}
#[test]
fn native_system_endpoint_guard_code_survives_the_bridge_whitelist() {
    assert_eq!(
        map_rejected_code(Some(code::SYSTEM_INTERFACE_UNSUPPORTED)),
        code::SYSTEM_INTERFACE_UNSUPPORTED
    );
    assert_eq!(
        map_rejected_code(Some("unrecognized")),
        code::STARTUP_FAILED
    );
    assert_eq!(
        map_rejected_code(Some(ENDPOINT_RETIRED_NO_BIRTH)),
        ENDPOINT_RETIRED_NO_BIRTH
    );
}

/// 🔴 **变异锁：非 Android 上三条腿都必须是「诚实失败 / fail-open」，绝不静默成功。**
///
/// 变异：把 `start_core` 的非 Android 腿改成 `Ok(())` ⇒ 第一条断。这条腿看着不可达，
/// 但它正是「调用点的 `cfg!` 守卫哪天被人删掉」时唯一还站着的东西。
#[tokio::test]
async fn non_android_legs_fail_honestly() {
    if cfg!(target_os = "android") {
        return;
    }
    let (msg, error_code) = start_core("{}").await.expect_err("非 Android 必须失败");
    assert_eq!(error_code, code::STARTUP_FAILED);
    assert!(!msg.trim().is_empty(), "失败必须带原因");
    assert!(stop_core().await.is_err(), "非 Android 停核必须失败");
    assert!(
        matches!(check_config("{}").await, ConfigCheckVerdict::Unavailable(_)),
        "闸门腿必须 fail-open 成 Unavailable，而不是 Accepted（那会假装闸门跑过）"
    );
    assert!(!core_started(), "非 Android 上没有进程内核");
}

/// 🔴 **变异锁：非 Android 上「系统拉起的核」恒不存在。**
///
/// 这一位在桌面上也被读（启动期自动连接腿不分平台地问它）。变异：非 Android 腿改成 `true` ⇒
/// 本条断；而生产后果是桌面每次冷启动都无视 `autoConnect:false` 去起核（`AdoptSystemCore` 优先于开关）。
#[tokio::test]
async fn non_android_never_reports_a_system_started_core() {
    if cfg!(target_os = "android") {
        return;
    }
    assert!(!system_started_core_running().await);
}

#[test]
fn android_receipt_requires_exact_run_config_and_claim() {
    let receipt: AndroidStartReceipt = serde_json::from_value(serde_json::json!({
        "runId": "candidate-7",
        "birthNonce": "birth-7",
        "configDigest": "a".repeat(64),
        "claim": "claim-7",
        "tun": {
            "autoRoute": true,
            "routes": ["0.0.0.0/0"],
            "excludedRoutes": [],
            "skippedExcludes": [],
            "allowedPackages": [],
            "excludedPackages": [],
            "skippedPackages": []
        }
    }))
    .unwrap();
    assert!(receipt.matches_request("candidate-7", &"a".repeat(64), Some("claim-7")));
    assert_eq!(receipt.exact_target().birth_nonce, "birth-7");
    assert!(!receipt.matches_request("candidate-8", &"a".repeat(64), Some("claim-7")));
    assert!(!receipt.matches_request("candidate-7", &"b".repeat(64), Some("claim-7")));
    assert!(!receipt.matches_request("candidate-7", &"a".repeat(64), None));
    assert!(receipt.managed_tun_evidence().is_some());

    let mut unknown = receipt.clone();
    unknown.claim = None; // legacy UUID is never a managed claim
    assert!(unknown.managed_tun_evidence().is_none());
    unknown.claim = Some("claim-7".into());
    unknown.tun = None; // no fd was observed
    assert!(unknown.managed_tun_evidence().is_none());
    unknown.tun = receipt.tun;
    unknown.tun.as_mut().unwrap().auto_route = false;
    assert!(unknown.managed_tun_evidence().is_none());
    unknown.tun.as_mut().unwrap().auto_route = true;
    unknown
        .tun
        .as_mut()
        .unwrap()
        .skipped_excludes
        .push("127.0.0.0/8".into());
    assert!(unknown.managed_tun_evidence().is_none());
    unknown.tun.as_mut().unwrap().skipped_excludes.clear();
    unknown
        .tun
        .as_mut()
        .unwrap()
        .skipped_packages
        .push("include:missing.app".into());
    assert!(unknown.managed_tun_evidence().is_none());
    assert!(
        serde_json::from_value::<AndroidStartReceipt>(serde_json::json!({
            "configDigest": "a".repeat(64)
        }))
        .is_err()
    );
}

#[test]
fn exact_stop_requires_the_birth_nonce_and_confirmed_release_state() {
    let target = AndroidExactTarget {
        run_id: "same-run".into(),
        birth_nonce: "first-birth".into(),
    };
    let closed: AndroidExactStopReceipt = serde_json::from_value(serde_json::json!({
        "runId": "same-run", "birthNonce": "first-birth", "state": "Closed"
    }))
    .unwrap();
    assert!(closed.matches_target(&target));
    assert!(closed.confirms_closed());
    let gone: AndroidExactStopReceipt = serde_json::from_value(serde_json::json!({
        "runId": "same-run", "birthNonce": "first-birth", "state": "AlreadyGone"
    }))
    .unwrap();
    assert!(gone.matches_target(&target));
    assert!(gone.confirms_closed());
    let busy: AndroidExactStopReceipt = serde_json::from_value(serde_json::json!({
        "runId": "same-run", "birthNonce": "first-birth", "state": "Busy"
    }))
    .unwrap();
    assert!(!busy.confirms_closed());
    let unknown: AndroidExactStopReceipt = serde_json::from_value(serde_json::json!({
        "runId": "same-run", "birthNonce": "first-birth", "state": "Unknown",
        "reason": "different-owner"
    }))
    .unwrap();
    assert!(!unknown.confirms_closed());
    assert!(!AndroidExactStopReceipt {
        birth_nonce: "second-birth".into(),
        ..closed.clone()
    }
    .matches_target(&target));
    assert!(!AndroidExactStopReceipt {
        reason: Some("timeout".into()),
        ..closed
    }
    .matches_target(&target));
    assert!(!AndroidExactStopReceipt {
        reason: None,
        ..unknown
    }
    .matches_target(&target));
    assert!(
        serde_json::from_value::<AndroidStartReceipt>(serde_json::json!({
            "runId": "same-run", "configDigest": "a".repeat(64), "claim": null
        }))
        .is_err()
    );
}

#[tokio::test]
async fn non_android_exact_stop_and_query_cannot_claim_a_release() {
    if cfg!(target_os = "android") {
        return;
    }
    let target = AndroidExactTarget {
        run_id: "old".into(),
        birth_nonce: "birth".into(),
    };
    assert!(main_core_exact_status(&target).await.is_err());
    assert!(stop_core_exact(&target).await.is_err());
}

#[tokio::test]
async fn non_android_main_owner_query_is_unknown() {
    if cfg!(target_os = "android") {
        return;
    }
    assert!(main_core_ownership().await.is_err());
}

#[test]
fn legacy_fence_status_requires_exact_id_and_three_known_shapes() {
    let vacant: AndroidLegacyDrainStatus = serde_json::from_value(serde_json::json!({
        "fenceId": "fence-7", "processNonce": "process-3", "state": "vacant",
        "closedRunId": "old-run"
    }))
    .unwrap();
    assert!(vacant.matches_request("fence-7"));
    assert!(!vacant.matches_request("fence-8"));
    assert!(!AndroidLegacyDrainStatus {
        closed_run_id: Some(String::new()),
        ..vacant.clone()
    }
    .matches_request("fence-7"));

    let owned: AndroidLegacyDrainStatus = serde_json::from_value(serde_json::json!({
        "fenceId": "fence-7", "processNonce": "process-3", "state": "owned",
        "runId": "old-run"
    }))
    .unwrap();
    assert!(owned.matches_request("fence-7"));

    let unknown: AndroidLegacyDrainStatus = serde_json::from_value(serde_json::json!({
        "fenceId": "fence-7", "processNonce": "process-3", "state": "unknown",
        "runId": "old-run", "reason": "cleanup-unknown"
    }))
    .unwrap();
    assert!(unknown.matches_request("fence-7"));
    assert!(!AndroidLegacyDrainStatus {
        state: "vacant".into(),
        reason: Some("timeout".into()),
        ..vacant
    }
    .matches_request("fence-7"));
    assert!(!AndroidLegacyDrainStatus {
        state: "owned".into(),
        run_id: None,
        ..owned
    }
    .matches_request("fence-7"));
}

#[tokio::test]
async fn non_android_legacy_fence_status_is_unknown() {
    if cfg!(target_os = "android") {
        return;
    }
    assert!(legacy_drain_status("fence-7").await.is_err());
}

/// 🔴 **变异锁：Kotlin 侧的 code 必须过白名单，不得原样透传。**
///
/// 变异：把 `map_rejected_code` 改成 `code.unwrap_or(STARTUP_FAILED)` 之类的透传 ⇒ 第三条断。
/// 透传的后果不是「多一个码」，而是 Kotlin 能凭空造一个 Rust `mod code` 从未声明的串，
/// 直接绕过前端覆盖门（`proxy-error-key-coverage.test.ts` 的 G1 只对账**声明集**）——
/// 用户拿到的是一个没有 i18n 文案的裸码。
#[test]
fn rejected_code_goes_through_a_whitelist() {
    assert_eq!(
        map_rejected_code(Some(code::VPN_PERMISSION_DENIED)),
        code::VPN_PERMISSION_DENIED
    );
    assert_eq!(map_rejected_code(None), code::STARTUP_FAILED);
    assert_eq!(
        map_rejected_code(Some("MADE_UP_CODE")),
        code::STARTUP_FAILED
    );
    assert_eq!(
        map_rejected_code(Some("")),
        code::STARTUP_FAILED,
        "空串也不得穿过"
    );
}

/// 🔴 **变异锁：非 Android 上两条新腿也必须诚实失败 —— 尤其不许「读不到 ⇒ 空表」。**
///
/// 变异：把 `installed_apps` 的非 Android 腿改成 `Ok(Vec::new())` ⇒ 第一条断。
///
/// 那个变异为什么危险：空表与「这台机器上真的一个第三方应用都没有」在渲染端**不可区分**，
/// 而后者几乎不可能、前者（包可见性没声明对 / 桥没接线 / 本平台没这条腿）很可能。
/// 于是「自定义应用挑不出任何东西」会被当成事实呈现，而不是当成故障 ——
/// 这与 `VpnAuthState::Unknown` 不许折成 `Denied` 是同一条口径：**没有登记来源的数据位不许编**。
///
/// 第二条同理：`hand_apk_to_system_installer` 在没有系统安装器的平台上返
/// `Ok(ApkHandoff { handed_off: true, .. })` 会让 command 层回一句「已交系统安装器」，
/// 而实际上什么都没发生。
#[tokio::test]
async fn non_android_installed_apps_and_apk_handoff_fail_honestly() {
    if cfg!(target_os = "android") {
        return;
    }
    let apps = installed_apps().await;
    let err = apps.expect_err("非 Android 必须失败，不许返一个空表冒充事实");
    assert!(!err.trim().is_empty(), "失败必须带原因");

    let handoff = hand_apk_to_system_installer("/tmp/whatever.apk").await;
    let err = handoff.expect_err("非 Android 必须失败，不许假装交出去了");
    assert!(!err.trim().is_empty(), "失败必须带原因");
}

/// `ApkHandoff` 的三态语义（`Ok(false)` 是**事实**不是错误）在类型上就得站得住。
///
/// 这条钉的是「交不出去但知道为什么」这一态确实可表达 —— 变异：把 `reason` 删掉、
/// 只留一个 `bool`，本条编不过。没有这一态时，command 层只剩「成功 / 失败」两格可填，
/// 而「没授予安装未知应用（按一下开关就能继续）」与「本机根本装不了」会被压进同一句话。
#[test]
fn apk_handoff_can_express_a_known_reason_without_being_an_error() {
    let denied = ApkHandoff {
        handed_off: false,
        reason: Some("unknown-sources-denied".to_string()),
    };
    assert!(!denied.handed_off);
    assert_eq!(denied.reason.as_deref(), Some("unknown-sources-denied"));

    let ok = ApkHandoff {
        handed_off: true,
        reason: None,
    };
    assert!(ok.handed_off && ok.reason.is_none());
}

/// 🔴 **变异锁：非 Android 上系统备份开关的读写都必须诚实失败。**
///
/// 变异：把 `system_backup` 的非 Android 腿改成 `Ok(false)` ⇒ 本条断。那种「假装读到了关」会让
/// 一个误把这一行画出来的平台显示成「已关」，而那台设备上根本没有这个闸门。
#[tokio::test]
async fn non_android_system_backup_toggle_fails_honestly() {
    if cfg!(target_os = "android") {
        return;
    }
    let err = set_system_backup(true)
        .await
        .expect_err("非 Android 写必须失败");
    assert!(!err.trim().is_empty(), "失败必须带原因");
    let err = system_backup()
        .await
        .expect_err("非 Android 读必须失败，不许折成 false");
    assert!(!err.trim().is_empty(), "失败必须带原因");
}

#[cfg(test)]
mod scoped_store_tests;

fn s5_wire(query: u64, source: &str, seq: serde_json::Value) -> DeviceConditionsWire {
    serde_json::from_value(serde_json::json!({
        "metered": true, "powerSave": true, "queryId": query.to_string(),
        "networkObservation": {"version":1,"sourceEpoch":source,"seq":seq,"currentKnown":false,"coverageGap":true}
    })).unwrap()
}
const S5_SOURCE_A: &str = "ffffffff-ffff-4fff-9fff-ffffffffffff";
const S5_SOURCE_B: &str = "00000000-0000-4000-9000-000000000001";

#[test]
fn s5_android_bridge_exact_integers_and_optional_invalid_fields_preserve_policy() {
    let mut gate = DeviceQueryGate::default();
    let query = gate.issue().unwrap();
    for integer in [0, 9_007_199_254_740_993u64, i64::MAX as u64] {
        let result = gate
            .finish(
                query,
                s5_wire(query, S5_SOURCE_A, serde_json::json!(integer.to_string())),
            )
            .unwrap();
        assert!(result.metered && result.power_save);
        assert_eq!(result.network.unwrap().seq, integer);
    }
    for invalid in [
        serde_json::json!(0),
        serde_json::json!(1.5),
        serde_json::json!("-1"),
        serde_json::json!("00"),
        serde_json::json!("01"),
        serde_json::json!("+1"),
        serde_json::json!(" 1"),
        serde_json::json!("1.0"),
        serde_json::json!("9223372036854775808"),
        serde_json::Value::Null,
    ] {
        let result = gate
            .finish(query, s5_wire(query, S5_SOURCE_A, invalid.clone()))
            .unwrap();
        assert!(result.metered && result.power_save, "{invalid}");
        assert!(result.network.is_none(), "{invalid}");
    }
    for field in [
        "version",
        "sourceEpoch",
        "seq",
        "currentKnown",
        "coverageGap",
    ] {
        let mut wire = s5_wire(query, S5_SOURCE_A, serde_json::json!("2"));
        wire.network_observation
            .as_object_mut()
            .unwrap()
            .remove(field);
        let result = gate.finish(query, wire).unwrap();
        assert!(result.network.is_none(), "missing {field}");
        assert!(result.metered && result.power_save);
    }
    for (field, invalid) in [
        ("version", serde_json::json!(2)),
        ("version", serde_json::json!(1.0)),
        ("sourceEpoch", serde_json::json!("not-a-source")),
        ("sourceEpoch", serde_json::json!(S5_SOURCE_A.to_uppercase())),
        ("currentKnown", serde_json::json!(1)),
        ("coverageGap", serde_json::json!("false")),
    ] {
        let mut wire = s5_wire(query, S5_SOURCE_A, serde_json::json!("2"));
        wire.network_observation[field] = invalid;
        assert!(
            gate.finish(query, wire).unwrap().network.is_none(),
            "{field}"
        );
    }
    let legacy =
        serde_json::from_str::<DeviceConditionsWire>(r#"{"metered":true,"powerSave":false}"#)
            .unwrap();
    let result = gate.finish(query, legacy).unwrap();
    assert!(result.network.is_none());
    assert!(result.metered);
    assert!(!result.power_save);
    for invalid_query in [
        serde_json::json!(query),
        serde_json::json!("01"),
        serde_json::json!("2"),
        serde_json::Value::Null,
    ] {
        let mut wire = s5_wire(query, S5_SOURCE_A, serde_json::json!("2"));
        wire.query_id = invalid_query;
        assert!(gate.finish(query, wire).unwrap().network.is_none());
    }
}

#[test]
fn s5_android_bridge_latest_query_fences_completion_and_delayed_publication_before_legal_source_swap(
) {
    use crate::runtime::auto_select::NativeNetworkHistory;
    let mut gate = DeviceQueryGate::default();
    let mut history = NativeNetworkHistory::default();
    let q1 = gate.issue().unwrap();
    let old = gate
        .finish(q1, s5_wire(q1, S5_SOURCE_A, serde_json::json!("9")))
        .unwrap()
        .network
        .unwrap();
    assert_eq!(
        gate.publish(&old, |network| history.observe(network)),
        Some(false)
    );
    let q2 = gate.issue().unwrap();
    assert!(gate
        .finish(q1, s5_wire(q1, S5_SOURCE_A, serde_json::json!("100")))
        .is_none());
    assert_eq!(gate.publish(&old, |network| history.observe(network)), None);
    let new = gate
        .finish(q2, s5_wire(q2, S5_SOURCE_B, serde_json::json!("1")))
        .unwrap()
        .network
        .unwrap();
    assert_eq!(
        gate.publish(&new, |network| history.observe(network)),
        Some(false),
        "new source baseline, UUID order is irrelevant"
    );
    assert_eq!(history.current().unwrap().source, S5_SOURCE_B);
    let q3 = gate.issue().unwrap();
    let retired = gate
        .finish(q3, s5_wire(q3, S5_SOURCE_A, serde_json::json!("101")))
        .unwrap()
        .network
        .unwrap();
    assert_eq!(
        gate.publish(&retired, |network| history.observe(network)),
        Some(false)
    );
    assert_eq!(
        history.current().unwrap().source,
        S5_SOURCE_B,
        "retired source cannot switch back even via newer query"
    );
}

async fn s5_fake_native_budget(
    query: u64,
    started: tokio::sync::oneshot::Sender<u64>,
    response: tokio::sync::oneshot::Receiver<DeviceConditionsWire>,
    returned: tokio::sync::oneshot::Sender<()>,
    budget: std::time::Duration,
) -> Result<DeviceConditionsWire, String> {
    started.send(query).unwrap();
    // Mirror the existing native adapter's detached receive lifetime. This is fake native I/O,
    // not a claim that the host executes Tauri/Android call_with_budget.
    let native = tokio::spawn(async move {
        let wire = response.await.unwrap();
        returned.send(()).unwrap();
        wire
    });
    match tokio::time::timeout(budget, native).await {
        Ok(Ok(wire)) => Ok(wire),
        Ok(Err(_)) => Err("fake native task failed".to_owned()),
        Err(_) => Err("fake native budget expired".to_owned()),
    }
}

#[tokio::test]
async fn s5_android_bridge_timeout_and_cancel_late_responses_have_no_publication_permit() {
    use crate::runtime::auto_select::NativeNetworkHistory;
    let gate = std::sync::Arc::new(std::sync::Mutex::new(DeviceQueryGate::default()));
    let mut history = NativeNetworkHistory::default();
    for cancel in [false, true] {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let (returned_tx, returned_rx) = tokio::sync::oneshot::channel();
        let queries = std::sync::Arc::clone(&gate);
        let query = tokio::spawn(async move {
            read_device_conditions_from(&queries, |id| {
                s5_fake_native_budget(
                    id,
                    started_tx,
                    response_rx,
                    returned_tx,
                    std::time::Duration::from_millis(if cancel { 5000 } else { 1 }),
                )
            })
            .await
        });
        let old_id = started_rx.await.unwrap();
        if cancel {
            query.abort();
            assert!(query.await.unwrap_err().is_cancelled());
        } else {
            assert!(query.await.unwrap().unwrap_err().contains("budget"));
        }
        let fresh = read_device_conditions_from(&gate, |id| {
            std::future::ready(Ok(s5_wire(id, S5_SOURCE_B, serde_json::json!("3"))))
        })
        .await
        .unwrap()
        .network
        .unwrap();
        gate.lock()
            .unwrap()
            .publish(&fresh, |next| history.observe(next));
        response_tx
            .send(s5_wire(old_id, S5_SOURCE_A, serde_json::json!("999")))
            .unwrap();
        returned_rx.await.unwrap(); // Late native receive succeeds; timed-out/cancelled waiter cannot consume it.
        assert_eq!(history.current().unwrap().source, S5_SOURCE_B);
        assert_eq!(history.current().unwrap().seq, 3);
    }
}

#[tokio::test]
async fn s5_android_bridge_actual_query_flow_supersedes_old_source_and_allows_new_baseline_and_growth(
) {
    use crate::runtime::auto_select::NativeNetworkHistory;
    let gate = std::sync::Arc::new(std::sync::Mutex::new(DeviceQueryGate::default()));
    let mut history = NativeNetworkHistory::default();
    let initial = read_device_conditions_from(&gate, |id| {
        std::future::ready(Ok(s5_wire(id, S5_SOURCE_A, serde_json::json!("9"))))
    })
    .await
    .unwrap()
    .network
    .unwrap();
    assert_eq!(
        gate.lock()
            .unwrap()
            .publish(&initial, |next| history.observe(next)),
        Some(false)
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    let queries = std::sync::Arc::clone(&gate);
    let old = tokio::spawn(async move {
        read_device_conditions_from(&queries, |id| {
            started_tx.send(id).unwrap();
            async move { Ok(response_rx.await.unwrap()) }
        })
        .await
    });
    let old_id = started_rx.await.unwrap();
    let fresh = read_device_conditions_from(&gate, |id| {
        std::future::ready(Ok(s5_wire(id, S5_SOURCE_B, serde_json::json!("0"))))
    })
    .await
    .unwrap()
    .network
    .unwrap();
    assert_eq!(
        gate.lock()
            .unwrap()
            .publish(&fresh, |next| history.observe(next)),
        Some(false)
    );
    response_tx
        .send(s5_wire(old_id, S5_SOURCE_A, serde_json::json!("100")))
        .unwrap();
    assert!(old.await.unwrap().unwrap_err().contains("superseded"));
    assert_eq!(
        gate.lock()
            .unwrap()
            .publish(&initial, |next| history.observe(next)),
        None
    );
    let next = read_device_conditions_from(&gate, |id| {
        std::future::ready(Ok(s5_wire(id, S5_SOURCE_B, serde_json::json!("1"))))
    })
    .await
    .unwrap()
    .network
    .unwrap();
    assert_eq!(
        gate.lock()
            .unwrap()
            .publish(&next, |next| history.observe(next)),
        Some(true)
    );
    let retired = read_device_conditions_from(&gate, |id| {
        std::future::ready(Ok(s5_wire(id, S5_SOURCE_A, serde_json::json!("101"))))
    })
    .await
    .unwrap()
    .network
    .unwrap();
    assert_eq!(
        gate.lock()
            .unwrap()
            .publish(&retired, |next| history.observe(next)),
        Some(false)
    );
    assert_eq!(history.current().unwrap().source, S5_SOURCE_B);
    assert_eq!(history.current().unwrap().seq, 1);
}

#[test]
fn s5_android_bridge_query_exhaustion_never_wraps() {
    let mut gate = DeviceQueryGate {
        issued: u64::MAX - 1,
    };
    assert_eq!(gate.issue(), Some(u64::MAX));
    assert_eq!(gate.issue(), None);
    assert_eq!(gate.issued, u64::MAX);
    assert!(DeviceQueryGate::default()
        .finish(0, s5_wire(0, S5_SOURCE_A, serde_json::json!("0")))
        .is_none());
}

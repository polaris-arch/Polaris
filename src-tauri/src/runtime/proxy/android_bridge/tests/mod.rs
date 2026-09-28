use super::*;

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

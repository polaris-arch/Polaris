mod probe_tests;
mod rule_pattern_admission_tests;

/// [`parse_probe_diagnostic`] 结构化提取门。样本来源标注在每条测试里：**真机**＝真跑随包
/// `resources/linux/sing-box`（`version` 自报 `1.14.0-beta.7`）对构造出的坏 config 执行
/// `check -c` 拿到的原始 stderr（未做任何清理，含真实 ANSI 色码），**构造**＝Windows 场景本机无法
/// 跑 `.exe`，按已用 Linux 等价场景（含冒号目录名）验证过的分隔符规律手工拼的字符串。
mod probe_diagnostic_tests;

/// Strict App admission and verdict mapping; genuine native lifecycle positives
/// use supervisor's private admitted fixture and the same central custody.
#[cfg(unix)]
mod probe_check_process;

mod system_proxy_live_tests;

mod start_payload_guard;

mod system_proxy_live_guard;

#[test]
fn expected_stop_rejects_explicit_invalid_identity_instead_of_unconditional_fallback() {
    use super::stop_expectation;
    use serde_json::json;
    assert_eq!(stop_expectation(&json!({})), Ok(None));
    assert_eq!(
        stop_expectation(
            &json!({"expectedMainGeneration":7,"expectedSelectedServerId":null,"expectedSelectionIntentGeneration":0})
        ),
        Ok(Some(crate::runtime::proxy::ExpectedCoreStopRequest {
            expected_main_generation: 7,
            expected_selected_server_id: None,
            expected_selection_intent_generation: 0
        }))
    );
    for value in [
        json!(null),
        json!(0),
        json!(-1),
        json!(1.5),
        json!("7"),
        json!(9_007_199_254_740_992_u64),
    ] {
        assert!(stop_expectation(&json!({"expectedMainGeneration": value})).is_err());
    }
    assert!(stop_expectation(&json!([])).is_err());
}

#[test]
fn owned_stop_requires_complete_and_exact_selection_context() {
    let base = serde_json::json!({"expectedMainGeneration":7,"expectedSelectedServerId":"node-a","expectedSelectionIntentGeneration":2});
    assert!(super::stop_expectation(&base).unwrap().is_some());
    for key in [
        "expectedMainGeneration",
        "expectedSelectedServerId",
        "expectedSelectionIntentGeneration",
    ] {
        let mut partial = base.clone();
        partial.as_object_mut().unwrap().remove(key);
        assert!(super::stop_expectation(&partial).is_err(), "{key}");
    }
    for value in [
        serde_json::json!(null),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("2"),
        serde_json::json!(9_007_199_254_740_992_u64),
    ] {
        let mut bad = base.clone();
        bad["expectedSelectionIntentGeneration"] = value;
        assert!(super::stop_expectation(&bad).is_err());
    }
    for value in [
        serde_json::json!(""),
        serde_json::json!(4),
        serde_json::json!({}),
    ] {
        let mut bad = base.clone();
        bad["expectedSelectedServerId"] = value;
        assert!(super::stop_expectation(&bad).is_err());
    }
}

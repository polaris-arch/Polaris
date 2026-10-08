use super::*;

/// Windows：漫游与超限各自压过成本类型；其余按成本类型，未知即不可得。
#[test]
fn windows_cost_folds_to_three_states() {
    for (cost_type, roaming, over_data_limit, expected) in [
        (1, false, false, Some(false)),
        (2, false, false, Some(true)),
        (3, false, false, Some(true)),
        (0, false, false, None),
        (4, false, false, None),
        (-1, false, false, None),
        (1, true, false, Some(true)),
        (1, false, true, Some(true)),
        (0, true, false, Some(true)),
        (0, false, true, Some(true)),
    ] {
        assert_eq!(
            fold_windows_cost(cost_type, roaming, over_data_limit),
            expected,
            "cost_type={cost_type} roaming={roaming} over_data_limit={over_data_limit}"
        );
    }
}

/// macOS：只有可用的路径才答「昂贵」位；无效、不可用、待连接的路径一律不可得。
#[test]
fn macos_path_folds_to_three_states() {
    for (status, expensive, expected) in [
        (1, true, Some(true)),
        (1, false, Some(false)),
        (0, true, None),
        (2, true, None),
        (2, false, None),
        (3, true, None),
    ] {
        assert_eq!(
            fold_macos_path(status, expensive),
            expected,
            "status={status} expensive={expensive}"
        );
    }
}

/// 本模块在这两个平台之外不答。
#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn other_platforms_report_unavailable() {
    assert_eq!(metered(), None);
}

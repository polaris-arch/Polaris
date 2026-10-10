//! `extract_version_token` 的金样。

use polaris_updater::core_build::extract_version_token;

#[test]
fn extract_version_token_from_full_version_line() {
    // it('从完整 version 行提取 token')
    assert_eq!(extract_version_token("sing-box version 1.13.13"), "1.13.13");
    assert_eq!(
        extract_version_token("sing-box version 1.13.13-reF1nd"),
        "1.13.13-reF1nd"
    );
}

#[test]
fn extract_version_token_bare_token_and_v_prefix() {
    // it('裸 token / 前缀 v')
    assert_eq!(extract_version_token("1.13.13"), "1.13.13");
    assert_eq!(extract_version_token("v1.13.13"), "1.13.13");
}

#[test]
fn extract_version_token_empty_and_dirty_input() {
    // it('空/脏输入')。上游第 3 条 `undefined as any` 在 Rust 由 `&str` 类型排除，
    // 等价形态（空串）已由前两条覆盖。
    assert_eq!(extract_version_token(""), "");
    assert_eq!(extract_version_token("   "), "");
}

use super::*;

#[test]
fn compare_semver_basic() {
    // 移植自 version.ts:compareSemver 注释链：alpha.32 < alpha.33 < beta.1 < rc.1 < 1.14.0 < 1.14.1
    assert_eq!(compare_semver("1.14.0-alpha.32", "1.14.0-alpha.33"), Ok(-1));
    assert_eq!(compare_semver("1.14.0-alpha.33", "1.14.0-beta.1"), Ok(-1));
    assert_eq!(compare_semver("1.14.0-beta.1", "1.14.0-rc.1"), Ok(-1));
    assert_eq!(compare_semver("1.14.0-rc.1", "1.14.0"), Ok(-1));
    assert_eq!(compare_semver("1.14.0", "1.14.1"), Ok(-1));
    // 反向 → 1。
    assert_eq!(compare_semver("1.14.1", "1.14.0"), Ok(1));
    // 相等 → 0。
    assert_eq!(compare_semver("1.14.0", "1.14.0"), Ok(0));
}

/// **随包基线升级用例（跨 alpha→beta 标识符）**：`alpha.45` 必须 < `beta.3`。
///
/// 为什么单列一条而不靠上面的 `alpha.33 < beta.1`：那一对的数字段恰好也是升序（33→1 除外，
/// 但首段 `alpha`/`beta` 就已定序），**掩盖不了**「只比数字段」这类退化——本对的数字段是
/// **降序**（45 → 3），只有真按 semver「先比首个标识符（字母段 ASCII）」才能得 `-1`。
///
/// 变异锁：把 [`cmp_pre`] 改成先比末段数字（或整体丢掉 prerelease 比较只比主版本段）→
/// 本用例得 `1`（或 `0`）→ 转红。预发布通道的应用更新判定同样以它为算术前提。
#[test]
fn compare_semver_alpha_to_beta_ignores_numeric_suffix_order() {
    assert_eq!(compare_semver("1.14.0-alpha.45", "1.14.0-beta.3"), Ok(-1));
    assert_eq!(compare_semver("1.14.0-beta.3", "1.14.0-alpha.45"), Ok(1));
    // 同一标识符内仍按数值（不是字典序：`beta.3` < `beta.10`，字典序会判反）。
    assert_eq!(compare_semver("1.14.0-beta.3", "1.14.0-beta.10"), Ok(-1));
}

#[test]
fn compare_semver_prefix_and_build() {
    // 容忍前导 v。
    assert_eq!(compare_semver("v1.13.13", "1.13.13"), Ok(0));
    // build (+...) 后缀不参与比较（semver 规范）。
    assert_eq!(compare_semver("1.14.0+naive", "1.14.0+other"), Ok(0));
    // 不同主版本。
    assert_eq!(compare_semver("2.0.0", "1.99.99"), Ok(1));
}

#[test]
fn compare_semver_missing_segments() {
    // 缺失段按 0 计（上游 `parseInt(p, 10) || 0`）。
    assert_eq!(compare_semver("1.14", "1.14.0"), Ok(0));
    assert_eq!(compare_semver("1.14.0.1", "1.14.0"), Ok(1));
}

#[test]
fn compare_semver_prerelease_segment_count() {
    // 段数多者更大（alpha < alpha.1）。
    assert_eq!(compare_semver("1.0.0-alpha", "1.0.0-alpha.1"), Ok(-1));
    // 数字段 < 字母段。
    assert_eq!(compare_semver("1.0.0-1", "1.0.0-alpha"), Ok(-1));
    // 纯数字段按数值。
    assert_eq!(compare_semver("1.0.0-2", "1.0.0-10"), Ok(-1));
}

#[test]
fn compare_semver_empty_errors() {
    // 空串显式报错（Polaris 容忍但产出无意义结果；显式报错防漏更新）。
    assert_eq!(compare_semver("", "1.0.0"), Err(ParseVersionError::Empty));
    assert_eq!(compare_semver("1.0.0", ""), Err(ParseVersionError::Empty));
}

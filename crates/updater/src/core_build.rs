//! 内核版本行解析：从 `sing-box version` 的第一行取出版本 token。
//!
//! 手写解析而非引入 `regex`：形状只有「`version` 关键词 + 空白 + 非空白」一条，stdlib 的
//! `split_whitespace` 足够表达，且本 crate 是纯逻辑 crate（依赖越少编译面越小）。

/// 剥掉**一个**前导 `v` / `V`（= 上游 `.replace(/^v/i, '')`，只剥一个）。
///
/// 注意与 [`crate::version::compare_semver`] 内部 `trim_start_matches(['v','V'])`（剥多个）的区别。
fn strip_one_leading_v(s: &str) -> &str {
    s.strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s)
}

/// 在 `s` 中查找 `/version\s+(\S+)/i` 的首个匹配，返回捕获组。
///
/// 逐字对齐 JS 正则语义（**非**锚定、**非**单词边界）：从左到右逐个起点尝试，命中
/// 大小写不敏感的 `version` 后要求「≥1 个空白 + ≥1 个非空白」，首个成功的起点获胜。
///
/// 刻意保留 JS 的「子串匹配」怪癖：`"conversion 1.2.3"` 里的 `version` 也算命中（`conVERSION`）。
/// 这一行为由 `tests/core_build_golden.rs` 的金样钉着，不「顺手修正」。
fn find_token_after_version_keyword(s: &str) -> Option<String> {
    const KEY: &str = "version";
    let lower = s.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(KEY) {
        let after = from + rel + KEY.len();
        let rest = &s[after..];
        // `\s+`：至少一个空白。
        let trimmed = rest.trim_start();
        if trimmed.len() < rest.len() {
            // `(\S+)`：至少一个非空白。
            if let Some(tok) = trimmed.split_whitespace().next() {
                return Some(tok.to_string());
            }
        }
        // 该起点不满足 → 下一个起点继续（对齐正则引擎的回溯尝试）。
        from = from + rel + 1;
    }
    None
}

/// 从 `sing-box version` 第一行（或裸 token）提取版本 token（剥前缀 `v`，到空白为止）。
#[must_use]
pub fn extract_version_token(version_line: &str) -> String {
    let s = version_line.trim();
    if s.is_empty() {
        return String::new();
    }
    let tok = find_token_after_version_keyword(s)
        .unwrap_or_else(|| s.split_whitespace().next().unwrap_or("").to_string());
    strip_one_leading_v(&tok).trim().to_string()
}

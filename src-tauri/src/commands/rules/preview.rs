//! Bounded, read-only regex preview. Reuses the workspace regex engine; this is
//! deliberately a small ASCII RE2-compatible subset, not kernel validation.
//! Save admission uses the actual core checker separately.
use crate::response::ApiResponse;

fn preview(patterns: &[String], input: &str) -> Result<Vec<Option<bool>>, &'static str> {
    if patterns.len() > 32 || input.len() > 4096 || patterns.iter().any(|p| p.len() > 1024) {
        return Err("Rule preview input exceeds its limit");
    }
    Ok(patterns
        .iter()
        .map(|pattern| {
            // Rust regex and Go RE2 differ for Unicode classes, escapes, flags and
            // character-class set operators. Do not claim parity for those forms.
            if pattern.is_empty()
                || !input.is_ascii()
                || !pattern.is_ascii()
                || pattern.contains("(?")
                || pattern.contains("&&")
                || pattern.contains("--")
                || pattern.contains("~~")
                || pattern.contains("[[")
                || pattern.contains("[^")
                || pattern.contains("||")
            {
                return None;
            }
            let mut escaped = false;
            let mut in_class = false;
            for ch in pattern.chars() {
                if escaped {
                    if ch.is_ascii_alphanumeric() {
                        return None;
                    }
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '[' {
                    if in_class {
                        return None;
                    }
                    in_class = true;
                } else if ch == ']' {
                    in_class = false;
                }
            }
            if escaped {
                return None;
            }
            // RE2 rejects repetition counts above 1000; Rust accepts larger counts.
            // Conservative lexical bound: extra rejection is unknown, never false parity.
            for fragment in pattern.split('{').skip(1) {
                let Some(body) = fragment.split('}').next() else {
                    return None;
                };
                for bound in body.split(',') {
                    if !bound.is_empty()
                        && bound.bytes().all(|c| c.is_ascii_digit())
                        && bound.parse::<u32>().map_or(true, |n| n > 1000)
                    {
                        return None;
                    }
                }
            }
            let regex = regex::RegexBuilder::new(pattern)
                .unicode(false)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024)
                .build()
                .ok()?;
            Some(regex.is_match(input))
        })
        .collect())
}

#[tauri::command]
pub async fn rules_preview_regex(
    patterns: Vec<String>,
    input: String,
) -> ApiResponse<Vec<Option<bool>>> {
    // Keep compilation off the event loop; a try-acquired slot prevents typing
    // or concurrent renderers from creating an unbounded blocking work queue.
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ApiResponse::err("Rule preview is busy");
    }
    match tauri::async_runtime::spawn_blocking(move || {
        struct Release;
        impl Drop for Release {
            fn drop(&mut self) {
                BUSY.store(false, Ordering::Release);
            }
        }
        let _release = Release;
        preview(&patterns, &input)
    })
    .await
    {
        Ok(Ok(result)) => ApiResponse::ok(result),
        Ok(Err(error)) => ApiResponse::err(error),
        Err(_) => ApiResponse::err("Rule preview could not complete"),
    }
}

#[cfg(test)]
mod tests {
    use super::preview;
    #[test]
    fn ascii_subset_is_case_sensitive_and_unsupported_is_unknown() {
        let p = [
            "^Example[.]com$",
            "^example[.]com$",
            "(?i)example",
            r"\p{L}+",
            "(?=a)a",
            "[a&&b]",
        ];
        assert_eq!(
            preview(&p.map(str::to_owned), "Example.com").unwrap(),
            vec![Some(true), Some(false), None, None, None, None]
        );
    }
    #[test]
    fn bounded_without_truncation_or_backtracking() {
        assert!(preview(&vec!["a".into(); 33], "a").is_err());
        assert!(preview(&["a".repeat(1025)], "a").is_err());
        assert!(preview(&[], &"a".repeat(4097)).is_err());
        assert_eq!(
            preview(&["(a+)+$".into()], "aaaaaaaa!").unwrap(),
            vec![Some(false)]
        );
        assert_eq!(preview(&["[".into()], "a").unwrap(), vec![None]);
    }
}

/// Explicit-submit admission uses the actual selected core, never the preview subset.
#[tauri::command]
pub async fn rules_validate_regex(
    state: tauri::State<'_, crate::runtime::AppRuntime>,
    patterns: Vec<String>,
) -> ApiResponse<Vec<Option<bool>>> {
    if patterns.len() > 32 || patterns.iter().any(|p| p.is_empty() || p.len() > 1024) {
        return ApiResponse::err("Rule syntax input exceeds its limit");
    }
    let verdict =
        crate::commands::proxy::check_rule_patterns(state.config().dir(), &patterns).await;
    ApiResponse::ok(vec![verdict; patterns.len()])
}

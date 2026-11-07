//! Source conservation: native carrier cannot omit any of the original business assertions.
use crate::test_support::{crate_source, method_body};
fn assertions(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        let macro_len = ["assert_eq!", "assert_ne!", "assert!"]
            .iter()
            .find(|s| body[i..].starts_with(**s))
            .map(|s| s.len());
        if let Some(n) = macro_len {
            let begin = i;
            i += n;
            while bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            assert_eq!(bytes[i], b'(');
            let mut depth = 0;
            let mut string = false;
            loop {
                let c = bytes[i];
                if string {
                    if c == b'\\' {
                        i += 1;
                    } else if c == b'"' {
                        string = false;
                    }
                } else if c == b'"' {
                    string = true;
                } else if c == b'(' {
                    depth += 1;
                } else if c == b')' {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
            out.push(
                body[begin..i]
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect(),
            );
        } else {
            i += body[i..].chars().next().unwrap().len_utf8();
        }
    }
    out
}
#[test]
fn normal_native_carrier_preserves_all_six_original_business_assertion_sets() {
    let carrier = crate_source("native_verification/proxy_carrier.rs");
    for (file, name) in [
        (
            "runtime/proxy/tests/lifecycle.rs",
            "real_core_full_lifecycle",
        ),
        (
            "runtime/proxy/tests/lifecycle.rs",
            "real_core_lifecycle_race_start_then_immediate_stop",
        ),
        (
            "runtime/proxy/tests/hot_switch.rs",
            "real_core_hot_switch_keeps_pid",
        ),
        (
            "runtime/proxy/tests/hot_switch.rs",
            "real_core_auto_failover_attests_without_applying_saved_debt",
        ),
        (
            "runtime/proxy/tests/hot_switch.rs",
            "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",
        ),
        (
            "runtime/stats/tests/real_core_tests.rs",
            "real_core_aggregate_relay_emits_real_frames",
        ),
    ] {
        let signature = format!("async fn {name}(");
        let original = assertions(&method_body(&crate_source(file), &signature));
        let native = assertions(
            &method_body(&carrier, &signature)
                .replace(
                    "apply_confirmed_for_native_verification",
                    "apply_confirmed_for_test",
                )
                .replace(
                    "restart_choice_for_native_verification",
                    "restart_choice_for_test",
                )
                .replace("super::core_binary", "super::super::core_binary"),
        );
        assert!(
            !original.is_empty(),
            "original scenario assertions absent: {name}"
        );
        assert_eq!(
            native, original,
            "native carrier changed business assertions: {name}"
        );
    }
}

#[path = "../../../native_verification/cleanup.rs"]
mod cleanup;
#[tokio::test]
async fn check_fence_error_keeps_both_original_owner_drains_unconditional() {
    use std::sync::{Arc, Mutex};
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let main = Arc::clone(&attempts);
    let checks = Arc::clone(&attempts);
    let result = cleanup::drain_after_fence(
        Err("original registry lock poisoned".into()),
        async move {
            main.lock().unwrap().push("main");
            Err("main retained".into())
        },
        async move {
            checks.lock().unwrap().push("checks");
            Err("checks retained".into())
        },
    )
    .await;
    assert_eq!(*attempts.lock().unwrap(), vec!["main", "checks"]);
    let error = result.unwrap_err();
    for required in [
        "original registry lock poisoned",
        "main retained",
        "checks retained",
    ] {
        assert!(error.contains(required));
    }
    let code = crate::test_support::crate_code("native_verification/proxy_carrier.rs");
    let body: String = method_body(&code, "pub(crate) async fn run_case(")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(body.contains("polaris_core_supervisor::begin_check_shutdown()"));
    assert!(body.contains("cleanup::drain_after_fence("));
    assert!(body.contains("runtime.shutdown_for_exit()"));
    assert!(body.contains("polaris_core_supervisor::shutdown_checks_for_exit().await"));
}
#[test]
fn normal_library_carrier_has_explicit_non_test_dependencies_and_bounded_pid_wait() {
    let source = crate_source("native_verification/proxy_carrier.rs");
    for item in [
        "use std::path::PathBuf;",
        "use std::time::Duration;",
        "use std::sync::atomic::Ordering;",
        "use crate::runtime::auto_switch::RuntimeCandidate;",
        "use polaris_core_supervisor::Signal;",
    ] {
        assert!(source.contains(item), "missing {item}");
    }
    let wait = method_body(&source, "async fn wait_pid_change(");
    assert!(wait.contains("tokio::time::Instant::now()"));
    assert!(wait.contains("deadline"));
    assert!(!source.contains("use crate::runtime::proxy::tests"));
}

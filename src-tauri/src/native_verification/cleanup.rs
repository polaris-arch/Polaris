//! Cleanup orchestration only. Neither futures nor success data create a native receipt.
use std::future::Future;
pub(super) async fn drain_after_fence(
    fence: Result<(), String>,
    main: impl Future<Output = Result<(), String>>,
    checks: impl Future<Output = Result<(), String>>,
) -> Result<(), String> {
    // Keep both original owners' attempts unconditional, including poison/fence failures.
    let main = main.await;
    let checks = checks.await;
    let mut failures = Vec::new();
    for (owner, result) in [
        ("Check fence", fence),
        ("main shutdown", main),
        ("Check drain", checks),
    ] {
        if let Err(error) = result {
            failures.push(format!("{owner}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

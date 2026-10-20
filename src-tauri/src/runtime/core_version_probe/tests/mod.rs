use super::*;
#[test]
fn attestation_uses_shared_probe_and_keeps_generation_commit_gate() {
    let source = crate::test_support::crate_code("runtime/proxy/process_supervision.rs");
    assert!(source.contains("let version_probe = Arc::clone(&self.core_version_probe)"));
    assert!(source.contains("version_probe.first_line(&expected)"));
    assert!(source.contains("version_probe.first_line(r)"));
    assert!(
        source.contains("attestation_commit_allowed(self.gate.generation(), my_gen, &status, pid)")
    );
    assert!(!source.contains("fn core_version_first_line("));
}

#[test]
fn bounded_original_custody_and_first_line_contract() {
    let source = crate::test_support::crate_code("runtime/core_version_probe.rs");
    let body =
        crate::commands::guard_scan::impl_method_body(&source, "    pub(crate) fn first_line(");
    for anchor in [
        "self.pending.try_lock()",
        "StdCommandRunner.run_observed(",
        "previous.poll_cleanup(CORE_VERSION_CLEANUP_BUDGET)",
        "observed.into_pending()",
        "child.request_stop()",
    ] {
        assert!(body.contains(anchor), "{anchor}");
    }
    for unbounded in [".output()", ".wait()", ".join()", "into_legacy_result"] {
        assert!(!body.contains(unbounded));
    }
    assert_eq!(CORE_VERSION_PROBE_BUDGET, Duration::from_secs(2));
    assert_eq!(CORE_VERSION_CLEANUP_BUDGET, Duration::from_millis(100));
    assert_eq!(
        core_version_first_line("  sing-box version 1.2.3\r\nextra"),
        "sing-box version 1.2.3"
    );
    assert_eq!(core_version_first_line(""), "");
}

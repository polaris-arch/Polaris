use super::*;

#[test]
fn attestation_never_executes_pid_paths_and_keeps_generation_commit_gate() {
    let source = crate::test_support::crate_code("runtime/proxy/process_supervision.rs");
    let body = crate::commands::guard_scan::impl_method_body(
        &source,
        "    async fn attest_running_core_binary(",
    );
    assert!(!body.contains("first_line("));
    assert!(!body.contains("Command::"));
    assert!(body.contains("attest_core_binary(expected, None, \"\", \"\")"));
    assert!(
        body.contains("attestation_commit_allowed(self.gate.generation(), my_gen, &status, pid)")
    );
}

#[test]
fn cached_or_foreign_diagnostic_paths_are_unknown_without_execution() {
    let probe = CoreVersionProbe::default();
    for path in ["/tmp/foreign/sing-box", "/proc/42/exe", "/nonexistent"] {
        assert!(probe.first_line(Path::new(path)).is_empty());
    }
    let source = crate::test_support::crate_code("runtime/core_version_probe.rs");
    for executable in ["Command::", "run_observed(", ".spawn()", ".output()"] {
        assert!(!source.contains(executable));
    }
}

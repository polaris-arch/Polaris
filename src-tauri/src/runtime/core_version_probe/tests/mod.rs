use super::*;

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

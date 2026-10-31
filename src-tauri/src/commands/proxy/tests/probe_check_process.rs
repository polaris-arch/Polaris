//! App boundary: unadmitted stand-ins never reach native check execution.
//! Real success/timeout/config-lifetime/native-wait coverage lives in the
//! supervisor's existing private admitted fixture and shared custody tests.
use super::super::{probe_check_from_raw, run_probe_check, ProbeCheck};
use crate::test_support::{write_executable_stand_in, TestDir};
use polaris_core_supervisor::{RawCheck, ValidationLifecycleError};

#[tokio::test]
async fn unadmitted_probe_stand_in_is_refused_before_native_birth() {
    let dir = TestDir::new("polaris-probe-no-admission-");
    let witness = dir.path().join("must-not-run");
    let binary = dir.path().join("unadmitted-probe.sh");
    write_executable_stand_in(
        &binary,
        format!("#!/bin/sh\nprintf forbidden > '{}'\n", witness.display()),
    );
    let config = dir.path().join("probe.json");
    std::fs::write(&config, b"{}").unwrap();
    assert!(matches!(
        run_probe_check(&binary, &config).await,
        Err(ValidationLifecycleError::Admission(_))
    ));
    assert!(!witness.exists(), "a rejected input must not execute");
    assert_eq!(std::fs::read(config).unwrap(), b"{}");
}

#[test]
fn native_results_keep_supported_timeout_and_output_failure_distinct() {
    assert!(matches!(
        probe_check_from_raw(RawCheck::Done {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }),
        ProbeCheck::Supported
    ));
    for raw in [
        RawCheck::TimedOut { after_secs: 8.0 },
        RawCheck::SpawnFailed("native spawn failed after admission".into()),
        RawCheck::OutputFailed("same-child exit, output failed".into()),
    ] {
        assert!(matches!(
            probe_check_from_raw(raw),
            ProbeCheck::Indeterminate
        ));
    }
    assert!(matches!(
        probe_check_from_raw(RawCheck::Done {
            success: false,
            stdout: "fallback diagnostic".into(),
            stderr: "kernel rejection".into(),
        }),
        ProbeCheck::Unsupported(_)
    ));
}

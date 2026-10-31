//! App mapping and strict admission boundary. Native accepted/timeout waits
//! remain covered by the supervisor's private admitted fixture, not a shell
//! path promoted into PreparedExecution by an App test.
use super::super::{
    config_check_from_raw, ConfigCheckFailure, ConfigChecker, SingBoxConfigChecker,
};
use crate::test_support::{write_executable_stand_in, TestDir};
use polaris_core_supervisor::{RawCheck, ValidationLifecycleError};

#[tokio::test]
async fn unadmitted_login_checker_input_has_no_native_birth_or_data_loss() {
    let dir = TestDir::new("polaris-login-check-no-admission-");
    let witness = dir.path().join("must-not-run");
    let binary = dir.path().join("unadmitted-checker.sh");
    write_executable_stand_in(
        &binary,
        format!("#!/bin/sh\nprintf forbidden > '{}'\n", witness.display()),
    );
    let config = dir.path().join("private-config.json");
    std::fs::write(&config, b"{\"keep\":true}").unwrap();
    assert!(matches!(
        SingBoxConfigChecker.check_for_spawn(&binary, &config).await,
        Err(ConfigCheckFailure::Lifecycle(
            ValidationLifecycleError::Admission(_)
        ))
    ));
    assert!(!witness.exists());
    assert_eq!(std::fs::read(config).unwrap(), b"{\"keep\":true}");
}

#[test]
fn reaped_success_and_timeout_remain_different_login_check_results() {
    assert!(config_check_from_raw(RawCheck::Done {
        success: true,
        stdout: String::new(),
        stderr: String::new(),
    })
    .is_ok());
    let timeout = config_check_from_raw(RawCheck::TimedOut { after_secs: 5.0 });
    assert!(
        matches!(timeout, Err(ConfigCheckFailure::Rejected(ref detail)) if detail.contains("5s"))
    );
    for raw in [
        RawCheck::SpawnFailed("native spawn failed".into()),
        RawCheck::OutputFailed("output unavailable after native exit".into()),
        RawCheck::Done {
            success: false,
            stdout: "fallback".into(),
            stderr: "kernel rejection".into(),
        },
    ] {
        assert!(matches!(
            config_check_from_raw(raw),
            Err(ConfigCheckFailure::Rejected(_))
        ));
    }
}

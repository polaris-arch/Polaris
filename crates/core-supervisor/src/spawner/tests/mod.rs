#![allow(clippy::too_many_lines)]

use super::*;

#[test]
fn spawn_request_argv_is_run_c_config_plus_extras() {
    // 对齐 Polaris 直接 spawn：`['run', '-c', configPath]`（:4790）。
    let req = SpawnRequest::unadmitted_intent(
        "/usr/local/bin/sing-box",
        "/tmp/cfg.json",
        StdioPolicy::Discard,
    );
    assert_eq!(
        req.argv(),
        vec![
            "run".to_string(),
            "-c".to_string(),
            "/tmp/cfg.json".to_string()
        ]
    );
}

#[test]
fn spawn_request_argv_appends_extra_args() {
    let mut req =
        SpawnRequest::unadmitted_intent("/bin/sing-box", "/tmp/c.json", StdioPolicy::Discard);
    req.extra_args = vec!["--debug".to_string(), "--legacy".to_string()];
    assert_eq!(
        req.argv(),
        vec![
            "run".to_string(),
            "-c".to_string(),
            "/tmp/c.json".to_string(),
            "--debug".to_string(),
            "--legacy".to_string()
        ]
    );
}

#[tokio::test]
async fn tokio_spawner_refuses_unadmitted_intent_even_for_a_named_binary() {
    let request = SpawnRequest::unadmitted_intent(
        "/nonexistent/sing-box-xyz",
        "/tmp/c.json",
        StdioPolicy::Discard,
    );
    assert!(matches!(
        TokioSpawner::new().spawn(request),
        Err(SpawnError::Admission(
            crate::admitted_payload::AdmissionError {
                kind: crate::admitted_payload::AdmissionErrorKind::Unknown,
                ..
            }
        ))
    ));
}

#[test]
fn spawned_child_pid_api_is_callable() {
    // SpawnedChild::pid 转发 child.id()。仅静态验证 API 存在；
    // 真实 pid 读取由 tokio_spawner_real_child_spawns_and_pid_present 覆盖。
    fn takes_spawned(s: &SpawnedChild) -> Option<u32> {
        s.pid()
    }
    // 引用以避免未用警告；不构造真实 Child（pid 语义由集成测试覆盖）。
    let _ = takes_spawned as fn(&SpawnedChild) -> Option<u32>;
}

#[cfg(target_os = "linux")]
mod process;

#[cfg(target_os = "linux")]
#[test]
fn only_admitted_main_with_explicit_absolute_utf8_directory_gets_recovery_root() {
    use crate::admitted_payload::{fixture, ExecPurpose};
    let directory = PathBuf::from("/explicit state/测试");
    let payload = fixture(b"#!/bin/sh\nexit 0\n");
    for purpose in [
        ExecPurpose::RunMain,
        ExecPurpose::RunLogin,
        ExecPurpose::RunSpeedtest,
    ] {
        let mut request = SpawnRequest::new(
            payload.prepare(purpose).unwrap(),
            "/other/profiles/config.json",
            StdioPolicy::Discard,
        );
        request.working_dir = Some(directory.clone());
        request.extra_args = vec!["--disable-color".into()];
        let expected = if purpose == ExecPurpose::RunMain {
            vec![
                "run",
                "-c",
                "/other/profiles/config.json",
                "-D",
                "/explicit state/测试",
                "--disable-color",
            ]
        } else {
            vec![
                "run",
                "-c",
                "/other/profiles/config.json",
                "--disable-color",
            ]
        };
        assert_eq!(request.argv(), expected);
    }
    let mut unadmitted = SpawnRequest::unadmitted_intent("/bin/core", "/cfg", StdioPolicy::Discard);
    unadmitted.working_dir = Some(directory);
    assert!(!unadmitted.argv().contains(&"-D".to_string()));
    use std::os::unix::ffi::OsStringExt;
    for directory in [
        None,
        Some(PathBuf::from("relative")),
        Some(PathBuf::from(std::ffi::OsString::from_vec(
            b"/invalid/\xff".to_vec(),
        ))),
    ] {
        let mut request = SpawnRequest::new(
            payload.prepare(ExecPurpose::RunMain).unwrap(),
            "/cfg",
            StdioPolicy::Discard,
        );
        request.working_dir = directory;
        assert_eq!(request.argv(), ["run", "-c", "/cfg"]);
    }
}

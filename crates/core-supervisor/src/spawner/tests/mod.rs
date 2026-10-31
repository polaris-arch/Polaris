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

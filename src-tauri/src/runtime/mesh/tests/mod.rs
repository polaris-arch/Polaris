mod exit_route_pure_tests;
mod exit_route_wiring_tests;
mod ts_status_cache_tests;
mod warp_drain_tests;
mod warp_tests;

/// A4 登录期出口让位纯谓词门：`mesh_login_fallback_should_engage` 六条件穷举。
///
/// 变异有牙：从「全命中」基线出发，逐一翻转每个入参 → 结果必翻假（覆盖 6 条逃逸路径，防碰巧真数据对）。
mod login_fallback_predicate_tests;

/// The registry only sweeps leftover login cores when the runtime installs the sweeper.
#[test]
fn production_runtime_installs_the_leftover_login_core_sweeper() {
    let root = crate::test_support::TestDir::new("polaris-stale-login-sweeper-");
    let mesh = super::MeshRuntime::new(root.join("config"));
    assert!(mesh.login_registry_for_test().has_stale_login_sweeper());
}

#[tokio::test]
async fn scoped_android_fs_body_rejects_unbound_changed_revision_and_missing_reappearance() {
    use super::MeshRuntime;
    use polaris_mesh::tailscale_state::retirement::{
        TailscaleProfileState, TailscaleStateFileState, TailscaleStoreRetirementNode,
        TailscaleWriterState,
    };
    let root = crate::test_support::TestDir::new("polaris-android-scoped-auth-fs-");
    let mesh = MeshRuntime::new(root.join("config"));
    let directory = mesh.tailscale_state_dir("node").unwrap();
    std::fs::create_dir_all(directory.join("taildrop")).unwrap();
    std::fs::write(
        directory.join("taildrop/kept"),
        b"user-owned synthetic file",
    )
    .unwrap();
    let file = directory.join("tailscaled.state");
    let sealed = polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    );
    let expected: serde_json::Value = serde_json::from_slice(&polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"), "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.expected.json")).unwrap();
    std::fs::write(&file, &sealed).unwrap();
    let registry = mesh.login_registry_for_test();
    registry.prepare("node", "same-action").await.unwrap();
    let attempt = registry.prepared_attempt_for_test("node", "same-action");
    let gate = mesh.tailscale_state_gate().await;
    let mut node = TailscaleStoreRetirementNode {
        tag: "selected".into(),
        state_directory: directory.canonicalize().unwrap().to_str().unwrap().into(),
        state_file: file.canonicalize().unwrap().to_str().unwrap().into(),
        writer_state: TailscaleWriterState::SealedDrained,
        state_file_state: TailscaleStateFileState::Regular,
        state_file_revision: polaris_updater::sha256_hex(&sealed),
        profile_state: TailscaleProfileState::Unknown,
        profile_fingerprint: String::new(),
    };
    assert!(mesh
        .tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), sealed);
    node.profile_state = TailscaleProfileState::Bound;
    node.profile_fingerprint = expected["profileFingerprint"].as_str().unwrap().into();
    node.state_file_revision = "0".repeat(64);
    assert!(mesh
        .tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), sealed);
    node.state_file_revision = polaris_updater::sha256_hex(&sealed);
    node.writer_state = TailscaleWriterState::NoStoreConstruction;
    assert!(mesh
        .tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .is_err());
    node.writer_state = TailscaleWriterState::SealedDrained;
    mesh.tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .unwrap();
    let retired = std::fs::read(&file).unwrap();
    assert_ne!(retired, sealed);
    let before: serde_json::Value = serde_json::from_slice(&sealed).unwrap();
    let after: serde_json::Value = serde_json::from_slice(&retired).unwrap();
    let active = expected["profileKey"].as_str().unwrap();
    assert!(after.get(active).is_none());
    for (key, value) in before.as_object().unwrap() {
        if key != active && key != "_profiles" {
            assert_eq!(after.get(key), Some(value), "{key}");
        }
    }
    assert_eq!(
        std::fs::read(directory.join("taildrop/kept")).unwrap(),
        b"user-owned synthetic file"
    );
    node.profile_state = TailscaleProfileState::Unknown;
    node.profile_fingerprint.clear();
    node.state_file_revision = polaris_updater::sha256_hex(&retired);
    mesh.tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), retired);
    node.state_file_state = TailscaleStateFileState::Missing;
    assert!(mesh
        .tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), retired);
    std::fs::remove_file(&file).unwrap();
    mesh.tailscale_retire_android_auth_under_gate("node", &gate, &attempt, &node)
        .unwrap();
    assert!(!file.exists());
    let foreign = MeshRuntime::new(root.join("foreign"));
    let foreign_gate = foreign.tailscale_state_gate().await;
    assert!(mesh
        .tailscale_retire_android_auth_under_gate("node", &foreign_gate, &attempt, &node)
        .is_err());
}

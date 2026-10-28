//! A3 缓存面门：末帧缓存读写 + 快照合成（relay ⇄ tailscale_get_status 的中转）。
use super::super::*;
use crate::test_support::TestDir;

fn temp_dir(tag: &str) -> TestDir {
    TestDir::new(&format!("polaris-tsstatus-{tag}-"))
}

fn event(id: &str, logged_in: bool) -> TailscaleStatusEvent {
    TailscaleStatusEvent {
        server_id: id.to_string(),
        backend_state: if logged_in { "Running" } else { "NeedsLogin" }.to_string(),
        logged_in,
        auth_url: None,
        tailscale_ips: vec!["100.64.0.1".to_string()],
        expired: false,
        peers: Vec::new(),
        details: Default::default(),
        // Taildrop 四位在本用例无关，取「无能力、无文件」的中性值；不给 Default 是刻意的：
        // 日后再加字段时，这些构造点必须重新被人看一眼，而不是被 `..Default::default()` 静默补齐。
        can_share_files: false,
        waiting_file_count: 0,
        receiving_file_count: 0,
        unread_file_count: 0,
    }
}

#[tokio::test]
async fn logout_rejects_path_escape_without_touching_sibling_directory() {
    let root = temp_dir("logout-escape");
    let config = root.join("config");
    let victim = root.join("victim");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("sentinel"), b"keep").unwrap();

    let mesh = MeshRuntime::new(config);
    let error = mesh
        .logout_tailscale_safely("../victim", &|| false, None)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(victim.join("sentinel").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn explicit_node_deletion_removes_only_the_valid_managed_state_directory() {
    let root = temp_dir("logout-valid");
    let config = root.join("config");
    let mesh = MeshRuntime::new(config.clone());
    let state = mesh.tailscale_state_dir("srv-1").unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("tailscaled.state"), b"state").unwrap();

    let gate = mesh.tailscale_state_gate().await;
    mesh.tailscale_logout_under_gate("srv-1", &gate, false)
        .unwrap();
    assert!(!state.exists());
    assert!(config.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn logout_rejects_a_state_root_symlink_escape() {
    let root = temp_dir("logout-symlink");
    let config = root.join("config");
    let victim_state = root.join("victim/srv-1");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&victim_state).unwrap();
    std::fs::write(victim_state.join("sentinel"), b"keep").unwrap();
    std::os::unix::fs::symlink(root.join("victim"), config.join("tailscale")).unwrap();

    let mesh = MeshRuntime::new(config);
    let error = mesh
        .logout_tailscale_safely("srv-1", &|| false, None)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(victim_state.join("sentinel").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// 空缓存（无帧）→ 快照 statuses 空；connected 透传调用方入参（核 running 态）。
#[test]
fn empty_cache_snapshot_is_empty_but_connected_passes_through() {
    let dir = temp_dir("empty");
    let mesh = MeshRuntime::new(dir.clone());
    let snap = mesh.tailscale_status_snapshot(true);
    assert!(snap.connected, "connected 由入参透传（核在跑）");
    assert!(snap.statuses.is_empty(), "无帧 → statuses 空");
}

/// update → 快照读回真数据（非恒空）。打断 `update_ts_status` 落库 / `tailscale_status_snapshot` 读缓存 → 转红。
#[test]
fn update_then_snapshot_returns_cached_frame() {
    let dir = temp_dir("update");
    let mesh = MeshRuntime::new(dir.clone());
    mesh.update_ts_status(vec![event("srv-a", true), event("srv-b", false)]);
    let snap = mesh.tailscale_status_snapshot(true);
    assert_eq!(snap.statuses.len(), 2, "快照读回缓存末帧（非恒空）");
    assert_eq!(snap.statuses[0].server_id, "srv-a");
    assert!(snap.statuses[0].logged_in);
    assert!(!snap.statuses[1].logged_in);
}

/// 每帧整体替换（非累加）：第二帧覆盖第一帧。打断「替换」为「追加」→ len 转红。
#[test]
fn frame_replaces_wholesale() {
    let dir = temp_dir("replace");
    let mesh = MeshRuntime::new(dir.clone());
    mesh.update_ts_status(vec![event("srv-a", true), event("srv-b", true)]);
    mesh.update_ts_status(vec![event("srv-c", false)]); // 新的全量帧
    let snap = mesh.tailscale_status_snapshot(true);
    assert_eq!(snap.statuses.len(), 1, "全量帧整体替换，非累加");
    assert_eq!(snap.statuses[0].server_id, "srv-c");
}

/// 停核 clear → 缓存清空。打断 `clear_ts_status` → 快照仍带陈旧帧 → 转红。
#[test]
fn clear_drops_cached_frame() {
    let dir = temp_dir("clear");
    let mesh = MeshRuntime::new(dir.clone());
    mesh.update_ts_status(vec![event("srv-a", true)]);
    mesh.clear_ts_status();
    let snap = mesh.tailscale_status_snapshot(false);
    assert!(!snap.connected);
    assert!(snap.statuses.is_empty(), "清缓存后无陈旧帧");
}

/// A4：`selected_exit_backend_state` 读选中出口末帧 backendState。
#[test]
fn selected_exit_backend_state_reads_frame() {
    let dir = temp_dir("bstate");
    let mesh = MeshRuntime::new(dir.clone());
    // 无帧 → None。
    assert_eq!(mesh.selected_exit_backend_state("srv-a"), None);
    // 有帧 → 读回 backendState。
    mesh.update_ts_status(vec![event("srv-a", false), event("srv-b", true)]);
    assert_eq!(
        mesh.selected_exit_backend_state("srv-a").as_deref(),
        Some("NeedsLogin")
    );
    assert_eq!(
        mesh.selected_exit_backend_state("srv-b").as_deref(),
        Some("Running")
    );
    // 未在册端点 → None。
    assert_eq!(mesh.selected_exit_backend_state("srv-x"), None);
}

/// A4：`expired` 帧即便 backendState=Running 也投影为 `"NeedsLogin"`（key 过期须重登，防死出口黑洞）。
/// 打断 `selected_exit_backend_state` 的 expired 分支 → 返回 "Running" → 转红。
#[test]
fn selected_exit_backend_state_expired_maps_to_needs_login() {
    let dir = temp_dir("expired");
    let mesh = MeshRuntime::new(dir.clone());
    let mut ev = event("srv-a", true); // backend_state=Running, logged_in=true
    ev.expired = true;
    mesh.update_ts_status(vec![ev]);
    assert_eq!(
        mesh.selected_exit_backend_state("srv-a").as_deref(),
        Some("NeedsLogin"),
        "过期 key 须投影为 NeedsLogin，即便帧仍报 Running"
    );
}

fn auth_sealed() -> Vec<u8> {
    polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    )
}

fn auth_expected() -> Vec<u8> {
    polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.expected.json",
    )
}

#[tokio::test]
async fn logout_projects_only_active_auth_and_repeat_preserves_exact_bytes() {
    let root = temp_dir("logout-auth-only");
    let mesh = MeshRuntime::new(root.join("config"));
    let state = mesh.tailscale_state_dir("srv-1").unwrap();
    std::fs::create_dir_all(state.join("taildrop")).unwrap();
    let file = state.join("tailscaled.state");
    std::fs::write(&file, auth_sealed()).unwrap();
    std::fs::write(
        state.join("taildrop/user-file"),
        b"user-owned synthetic file",
    )
    .unwrap();
    let original: serde_json::Value = serde_json::from_slice(&auth_sealed()).unwrap();
    mesh.logout_tailscale_safely("srv-1", &|| false, None)
        .await
        .unwrap();
    assert!(state.exists());
    let retired = std::fs::read(&file).unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&retired).unwrap();
    assert_eq!(
        std::fs::read(state.join("taildrop/user-file")).unwrap(),
        b"user-owned synthetic file"
    );
    for key in [
        "_machinekey",
        "_taildrop-received",
        "profile-b456",
        "_serve/a123",
        "ipn-go-bridge",
        "unknown-user-value",
        "_current-profile",
    ] {
        assert_eq!(actual[key], original[key], "preserve {key}");
    }
    assert!(actual.get("profile-a123").is_none());
    assert_eq!(
        polaris_updater::sha256_hex(&retired),
        "28851585b59661040db39387c91e6a5a9b633ec341c6c4a6dc99f526eb370799"
    );
    assert_eq!(
        polaris_mesh::tailscale_state::cached_session_exists_for_presentation(&retired),
        Ok(false)
    );
    assert!(polaris_mesh::tailscale_state::cached_session_exists(&retired).is_err());
    mesh.logout_tailscale_safely("srv-1", &|| false, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), retired);
}

#[tokio::test]
async fn bound_auth_projection_rejects_changed_revision_without_mutation() {
    let root = temp_dir("logout-auth-revision");
    let mesh = MeshRuntime::new(root.join("config"));
    let state = mesh.tailscale_state_dir("srv-1").unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let file = state.join("tailscaled.state");
    std::fs::write(&file, auth_sealed()).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(&auth_expected()).unwrap();
    let gate = mesh.tailscale_state_gate().await;
    assert!(mesh
        .tailscale_retire_auth_under_gate(
            "srv-1",
            &gate,
            None,
            polaris_mesh::tailscale_state::AuthProjectionProvenance::BoundFingerprint(
                expected["profileFingerprint"].as_str().unwrap()
            ),
            Some(&"0".repeat(64))
        )
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), auth_sealed());
    mesh.tailscale_retire_auth_under_gate(
        "srv-1",
        &gate,
        None,
        polaris_mesh::tailscale_state::AuthProjectionProvenance::BoundFingerprint(
            expected["profileFingerprint"].as_str().unwrap(),
        ),
        Some(expected["stateFileRevision"].as_str().unwrap()),
    )
    .unwrap();
    let retired = std::fs::read(&file).unwrap();
    assert!(mesh
        .tailscale_retire_auth_under_gate(
            "srv-1",
            &gate,
            None,
            polaris_mesh::tailscale_state::AuthProjectionProvenance::BoundFingerprint(
                expected["profileFingerprint"].as_str().unwrap()
            ),
            Some(&polaris_updater::sha256_hex(&retired))
        )
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), retired);
}

#[tokio::test]
async fn sealed_unbound_auth_allows_only_retired_exact_bytes_and_revision() {
    use polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences;
    let root = temp_dir("logout-auth-unbound");
    let mesh = MeshRuntime::new(root.join("config"));
    let state = mesh.tailscale_state_dir("srv-1").unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let file = state.join("tailscaled.state");
    std::fs::write(&file, auth_sealed()).unwrap();
    let gate = mesh.tailscale_state_gate().await;
    // A complete sealed file is insufficient to delete an active unbound identity.
    assert!(mesh
        .tailscale_retire_auth_under_gate(
            "srv-1",
            &gate,
            None,
            CurrentFileReferences,
            Some(&polaris_updater::sha256_hex(&auth_sealed()))
        )
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), auth_sealed());
    mesh.tailscale_retire_auth_under_gate("srv-1", &gate, None, CurrentFileReferences, None)
        .unwrap();
    let retired = std::fs::read(&file).unwrap();
    let revision = polaris_updater::sha256_hex(&retired);
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    mesh.tailscale_retire_auth_under_gate(
        "srv-1",
        &gate,
        None,
        CurrentFileReferences,
        Some(&revision),
    )
    .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), retired);
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        before
    );
    let mut changed = retired.clone();
    changed.push(b'\n');
    std::fs::write(&file, &changed).unwrap();
    assert!(mesh
        .tailscale_retire_auth_under_gate(
            "srv-1",
            &gate,
            None,
            CurrentFileReferences,
            Some(&revision)
        )
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), changed);
    // Read-only absence does not confer admission when an original Main claim remains.
    assert_eq!(
        polaris_mesh::tailscale_state::cached_session_exists_for_presentation(&changed),
        Ok(false)
    );
    let token = mesh.mint_tailscale_main_birth();
    let generated = serde_json::json!({"endpoints":[{"type":"tailscale","tag":"ts-srv-1", "state_directory":state}]});
    let mut reservation = mesh
        .reserve_tailscale_main_states(&generated, &gate, token)
        .await
        .unwrap();
    reservation.arm_external_start();
    drop(reservation);
    assert!(mesh
        .tailscale_retire_auth_under_gate(
            "srv-1",
            &gate,
            None,
            CurrentFileReferences,
            Some(&polaris_updater::sha256_hex(&changed))
        )
        .is_err());
    assert_eq!(std::fs::read(&file).unwrap(), changed);
}

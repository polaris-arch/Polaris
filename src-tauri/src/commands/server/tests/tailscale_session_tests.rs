use super::*;

#[test]
fn cached_presentation_is_separate_from_physical_directory_cleanup() {
    let root = TestDir::new("polaris-ts-cached-empty-");
    let state = root.join("node");
    assert!(!tailscale_state_exists_at(&state, false).unwrap());
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    std::fs::create_dir(&state).unwrap();
    assert!(tailscale_state_exists_at(&state, false).unwrap());
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    std::fs::create_dir(state.join("taildrop")).unwrap();
    std::fs::write(state.join("tailscaled.log.conf"), b"cache-fixture").unwrap();
    assert!(tailscale_state_exists_at(&state, false).unwrap());
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    for bytes in [
        b"{}".as_slice(),
        br#"{"_machinekey":"bWFjaGluZS1maXh0dXJl"}"#,
    ] {
        std::fs::write(state.join("tailscaled.state"), bytes).unwrap();
        assert!(!tailscale_state_exists_at(&state, true).unwrap());
        assert_eq!(
            std::fs::read(state.join("tailscaled.state")).unwrap(),
            bytes
        );
        assert!(tailscale_state_exists_at(&state, false).unwrap());
    }
    assert!(state.join("taildrop").is_dir());
    assert_eq!(
        std::fs::read(state.join("tailscaled.log.conf")).unwrap(),
        b"cache-fixture"
    );
}

#[tokio::test]
async fn cached_session_disappears_after_guarded_logout_and_empty_reopen() {
    let root = TestDir::new("polaris-ts-cached-logout-");
    let mesh = crate::runtime::mesh::MeshRuntime::new(root.to_path_buf());
    let state = mesh.tailscale_state_dir("node").unwrap();
    std::fs::create_dir_all(state.join("taildrop")).unwrap();
    let bytes = polaris_source_probe::repo_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/mesh/src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    );
    let mut original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // The projection fixture's prefs omit persisted account Config. Retain the
    // original query test's synthetic NodeID/LoginName before checking presence.
    original["profile-a123"] = serde_json::json!("eyJXYW50UnVubmluZyI6ZmFsc2UsIkxvZ2dlZE91dCI6ZmFsc2UsIkNvbmZpZyI6eyJOb2RlSUQiOiJuLWZpeHR1cmUiLCJVc2VyUHJvZmlsZSI6eyJMb2dpbk5hbWUiOiJmaXh0dXJlQGV4YW1wbGUuaW52YWxpZCJ9fX0=");
    let bytes = serde_json::to_vec(&original).unwrap();
    std::fs::write(state.join("tailscaled.state"), &bytes).unwrap();
    std::fs::write(state.join("taildrop/user-file"), b"preserved user file").unwrap();
    assert!(tailscale_state_exists_at(&state, true).unwrap());
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        bytes
    );
    mesh.logout_tailscale_safely("node", &|| false, None)
        .await
        .unwrap();
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    assert!(state.is_dir());
    let retired = std::fs::read(state.join("tailscaled.state")).unwrap();
    let projected: serde_json::Value = serde_json::from_slice(&retired).unwrap();
    assert!(projected.get("profile-a123").is_none());
    for key in [
        "_current-profile",
        "_machinekey",
        "profile-b456",
        "_taildrop-received",
        "ipn-go-bridge",
        "unknown-user-value",
    ] {
        assert_eq!(projected[key], original[key], "preserve {key}");
    }
    assert!(polaris_mesh::tailscale_state::cached_session_exists(&retired).is_err());
    // A fresh runtime reopens the retained retired store; it does not recreate
    // an empty directory or choose the preserved historical profile.
    drop(mesh);
    let reopened = crate::runtime::mesh::MeshRuntime::new(root.to_path_buf());
    assert_eq!(reopened.tailscale_state_dir("node").unwrap(), state);
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    assert!(tailscale_state_exists_at(&state, false).unwrap());
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        retired
    );
    assert_eq!(
        std::fs::read(state.join("taildrop/user-file")).unwrap(),
        b"preserved user file"
    );
}

#[test]
fn malformed_store_and_real_read_failure_remain_query_errors() {
    let state = TestDir::new("polaris-ts-cached-unknown-");
    let file = state.join("tailscaled.state");
    std::fs::write(&file, b"{malformed").unwrap();
    assert_eq!(
        tailscale_state_exists_at(&state, true).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"{malformed");
    assert!(tailscale_state_exists_at(&state, false).unwrap());
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(tailscale_state_exists_at(&state, true).is_err());
    assert!(file.is_dir());
}

#[test]
fn oversized_native_store_is_unknown_without_changing_it() {
    let root = TestDir::new("polaris-ts-cached-size-");
    let file = root.join("tailscaled.state");
    let handle = std::fs::File::create(&file).unwrap();
    handle.set_len(4 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        tailscale_state_exists_at(&root, true).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 4 * 1024 * 1024 + 1);
}

#[cfg(unix)]
#[test]
fn cached_query_rejects_node_file_and_state_root_symlink_escapes() {
    let root = TestDir::new("polaris-ts-cached-links-");
    let config = root.join("config");
    let state_root = config.join("tailscale");
    let state = state_root.join("node");
    let outside = root.join("outside");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("tailscaled.state"), b"outside-fixture").unwrap();
    std::os::unix::fs::symlink(
        outside.join("tailscaled.state"),
        state.join("tailscaled.state"),
    )
    .unwrap();
    assert!(tailscale_state_exists_at(&state, true).is_err());
    std::fs::remove_file(state.join("tailscaled.state")).unwrap();
    std::fs::remove_dir(&state).unwrap();
    std::os::unix::fs::symlink(&outside, &state).unwrap();
    assert!(tailscale_state_exists_at(&state, true).is_err());
    std::fs::remove_file(&state).unwrap();
    std::fs::remove_dir(&state_root).unwrap();
    std::os::unix::fs::symlink(&outside, &state_root).unwrap();
    std::fs::create_dir(outside.join("node")).unwrap();
    assert!(tailscale_state_exists_at(&state, true).is_err());
    assert_eq!(
        std::fs::read(outside.join("tailscaled.state")).unwrap(),
        b"outside-fixture"
    );
}

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
    std::fs::create_dir_all(&state).unwrap();
    let bytes = br#"{"_current-profile":"cHJvZmlsZS1hYjEy","_profiles":"eyJhYjEyIjp7IklEIjoiYWIxMiIsIktleSI6InByb2ZpbGUtYWIxMiJ9fQ==","profile-ab12":"eyJXYW50UnVubmluZyI6ZmFsc2UsIkxvZ2dlZE91dCI6ZmFsc2UsIkNvbmZpZyI6eyJOb2RlSUQiOiJuLWZpeHR1cmUiLCJVc2VyUHJvZmlsZSI6eyJMb2dpbk5hbWUiOiJmaXh0dXJlQGV4YW1wbGUuaW52YWxpZCJ9fX0="}"#;
    std::fs::write(state.join("tailscaled.state"), bytes).unwrap();
    assert!(tailscale_state_exists_at(&state, true).unwrap());
    assert_eq!(
        std::fs::read(state.join("tailscaled.state")).unwrap(),
        bytes
    );
    mesh.logout_tailscale_safely("node", &|| false, None)
        .await
        .unwrap();
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    std::fs::create_dir(&state).unwrap();
    assert!(!tailscale_state_exists_at(&state, true).unwrap());
    assert!(tailscale_state_exists_at(&state, false).unwrap());
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

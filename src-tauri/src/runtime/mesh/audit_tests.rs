use super::*;
#[test]
#[cfg(unix)]
fn warp_queue_uses_private_durable_publication() {
    use std::os::unix::fs::PermissionsExt;
    let temp = crate::test_support::TestDir::new("polaris-warp-private-");
    let mesh = MeshRuntime::new(temp.path().to_path_buf());
    let _lock = mesh.warp_queue_lock.lock().unwrap();
    mesh.save_warp_queue(&[]).unwrap();
    assert_eq!(
        std::fs::metadata(&mesh.warp_queue_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&mesh.warp_queue_path).unwrap(),
        "[]"
    );
    assert!(!mesh.warp_queue_path.with_extension("json.tmp").exists());
}

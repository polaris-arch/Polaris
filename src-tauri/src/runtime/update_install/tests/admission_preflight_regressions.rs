use super::super::*;
#[test]
fn invalid_lease_rejects_before_exit_and_does_not_follow_symlink() {
    let dir = std::env::temp_dir().join(format!(
        "polaris-admission-preflight-test-{}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let target = dir.join("other");
    std::fs::write(&target, b"unchanged").unwrap();
    std::os::unix::fs::symlink(&target, dir.join(".polaris-update.lease")).unwrap();
    assert!(preflight_mac_admission(&dir.join("Polaris.app")).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
    std::fs::remove_dir_all(dir).unwrap();
}

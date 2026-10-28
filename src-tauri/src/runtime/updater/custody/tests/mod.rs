use super::*;
use crate::test_support::TestDir;
use serde_json::json;

fn info() -> Value {
    json!({"version":"v1.0.1", "downloadUrl":"https://github.com/polaris-arch/Polaris/releases/download/v1.0.1/Polaris_1.0.1_x64-win-setup.exe", "fileName":"Polaris_1.0.1_x64-win-setup.exe", "fileSize":4, "sha256":polaris_updater::sha256_hex(b"test"), "isPrerelease":false, "title":"Release", "releaseNotes":"notes"})
}

#[test]
fn renderer_cannot_substitute_any_executable_identity_field() {
    let custody = UpdateCustody::default();
    assert!(custody.admit(&info()).is_err());
    custody.select(info()).unwrap();
    let mut projected = info();
    projected.as_object_mut().unwrap().remove("title");
    projected.as_object_mut().unwrap().remove("releaseNotes");
    assert_eq!(custody.admit(&projected).unwrap(), info());
    for key in [
        "version",
        "downloadUrl",
        "fileName",
        "fileSize",
        "sha256",
        "isPrerelease",
    ] {
        let mut forged = info();
        forged[key] = Value::Null;
        assert!(custody.admit(&forged).is_err(), "{key}");
    }
    let mut replacement = info();
    replacement["version"] = json!("v1.0.2");
    custody.select(replacement).unwrap();
    assert!(custody.admit(&info()).is_err());
}

#[test]
fn even_backend_selection_requires_https_github_asset_and_digest() {
    for (key, bad) in [
        ("sha256", Value::Null),
        ("sha256", json!("bad")),
        (
            "downloadUrl",
            json!("http://github.com/polaris-arch/Polaris/releases/download/v1/a.exe"),
        ),
        ("downloadUrl", json!("https://attacker.invalid/a.exe")),
        (
            "downloadUrl",
            json!("https://u:p@github.com/polaris-arch/Polaris/releases/download/v1/a.exe"),
        ),
        ("fileName", json!("../payload.exe")),
    ] {
        let custody = UpdateCustody::default();
        let mut invalid = info();
        invalid[key] = bad;
        custody.select(invalid.clone()).unwrap();
        assert!(custody.admit(&invalid).is_err(), "{key}");
    }
}

#[test]
fn only_received_bytes_can_be_frozen_and_changes_fail_closed() {
    let dir = TestDir::new("polaris-update-custody");
    let source = dir.join("Polaris_1.0.1_x64-win-setup.exe");
    std::fs::write(&source, b"test").unwrap(); // Inert bytes, never executed.
    let custody = UpdateCustody::default();
    assert!(custody.stage(&source).is_err());
    custody
        .record(&source, &polaris_updater::sha256_hex(b"test"), 4)
        .unwrap();
    let stage = custody.stage(&source).unwrap();
    let staged = stage.path().to_path_buf();
    assert_eq!(std::fs::read(&staged).unwrap(), b"test");
    assert_ne!(staged, source);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(staged.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(stage);
    assert!(!staged.exists());
    assert!(source.exists());
    std::fs::write(&source, b"evil").unwrap(); // Same length, changed digest.
    assert!(custody.stage(&source).is_err());
    std::fs::write(&source, b"longer").unwrap();
    assert!(custody.stage(&source).is_err());
    assert!(UpdateCustody::default().stage(&source).is_err()); // Restart grants no authority.
    let other = dir.join("other.exe");
    std::fs::write(&other, b"test").unwrap();
    assert!(custody.stage(&other).is_err());
    assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 2); // Failed stages cleaned up.
}

#[cfg(unix)]
#[test]
fn receipt_does_not_authorize_a_symlink_replacement() {
    let dir = TestDir::new("polaris-update-custody-symlink");
    let source = dir.join("candidate.exe");
    let other = dir.join("other");
    std::fs::write(&source, b"test").unwrap();
    std::fs::write(&other, b"test").unwrap();
    let custody = UpdateCustody::default();
    custody
        .record(&source, &polaris_updater::sha256_hex(b"test"), 4)
        .unwrap();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(&other, &source).unwrap();
    assert!(custody.stage(&source).is_err());
    assert_eq!(std::fs::read(&other).unwrap(), b"test");
}

#[test]
fn zero_and_oversized_receipts_are_never_admitted() {
    let custody = UpdateCustody::default();
    let hash = polaris_updater::sha256_hex(b"test");
    assert!(custody.record(Path::new("test.exe"), &hash, 0).is_err());
    assert!(custody
        .record(Path::new("test.exe"), &hash, MAX_BYTES + 1)
        .is_err());
}

#[test]
fn successful_handoff_keeps_only_its_private_copy_for_the_os_consumer() {
    let dir = TestDir::new("polaris-update-custody-handoff");
    let source = dir.join("inert.exe");
    std::fs::write(&source, b"test").unwrap();
    let custody = UpdateCustody::default();
    custody
        .record(&source, &polaris_updater::sha256_hex(b"test"), 4)
        .unwrap();
    let stage = custody.stage(&source).unwrap();
    let path = stage.path().to_path_buf();
    stage.hand_off();
    assert_eq!(std::fs::read(&path).unwrap(), b"test");
    assert_eq!(std::fs::read(&source).unwrap(), b"test");
}

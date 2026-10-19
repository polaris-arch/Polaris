use super::*;
#[test]
fn registration_requires_one_complete_native_config_token_record() {
    let id = CallerIdentity::Unix(1000);
    let config = std::env::temp_dir().join("polaris-inert-config");
    let token = "a".repeat(64);
    let raw = format!("u1000\n{}\n{token}\n", config.display());
    assert_eq!(parse_registration(&raw, &id).unwrap(), (config, token));
    for bad in [
        raw.trim_end().to_owned(),
        raw.replace("u1000", "u1001"),
        raw.replace(&"a".repeat(64), "not-hex"),
        format!("{raw}extra\n"),
        raw.replace("polaris-inert-config", "polaris\rconfig"),
    ] {
        assert!(parse_registration(&bad, &id).is_err());
    }
    assert!(parse_identity("u01000").is_none());
    assert!(parse_identity("sS-1-5-21-1-2-3-01000").is_none());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn registration_transaction_restores_exact_bytes_and_absence() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let caller = CallerIdentity::Unix(uid);
    let policy = || OwnerPolicy { uid: Some(uid) };
    let directory = root.path().join("users").join(caller.key());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let record = directory.join("registration");
    let absent_backup = root.path().join("absent.backup");
    snapshot_registration_with_owner(root.path(), &caller, &absent_backup, policy()).unwrap();
    std::fs::write(&record, b"new registration").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    restore_registration_with_owner(root.path(), &caller, &absent_backup, policy()).unwrap();
    assert!(!record.exists());
    std::fs::write(&record, b"exact old registration\n").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o400)).unwrap();
    let backup = root.path().join("record.backup");
    snapshot_registration_with_owner(root.path(), &caller, &backup, policy()).unwrap();
    assert!(snapshot_registration_with_owner(root.path(), &caller, &backup, policy()).is_err());
    std::fs::remove_file(&record).unwrap();
    std::fs::write(&record, b"replacement").unwrap();
    restore_registration_with_owner(root.path(), &caller, &backup, policy()).unwrap();
    assert_eq!(std::fs::read(&record).unwrap(), b"exact old registration\n");
    assert_eq!(
        std::fs::metadata(&record).unwrap().permissions().mode() & 0o777,
        0o400
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn registration_snapshot_rejects_linked_or_nonprivate_records() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let caller = CallerIdentity::Unix(uid);
    let directory = root.path().join("users").join(caller.key());
    std::fs::create_dir_all(&directory).unwrap();
    let record = directory.join("registration");
    let backup = root.path().join("backup");
    symlink(root.path().join("missing"), &record).unwrap();
    assert!(snapshot_registration_with_owner(
        root.path(),
        &caller,
        &backup,
        OwnerPolicy { uid: Some(uid) }
    )
    .is_err());
    assert!(!backup.exists());
    std::fs::remove_file(&record).unwrap();
    std::fs::write(&record, b"private").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(snapshot_registration_with_owner(
        root.path(),
        &caller,
        &backup,
        OwnerPolicy { uid: Some(uid) }
    )
    .is_err());
    assert!(!backup.exists());
}

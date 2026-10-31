use super::*;
use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest, InstalledCoreManifest};
use std::os::unix::fs::PermissionsExt;
#[test]
fn status_distinguishes_uninstalled_id_from_missing_installed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    for dir in [root.path(), source.path()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let uid = nix::unistd::Uid::effective().as_raw();
    let owner = OwnerPolicy { uid: Some(uid) };
    let store = CoreStore::initialize(root.path(), owner).unwrap();
    let files = [
        ("sing-box", b"inert-core".as_slice()),
        ("libcronet.so", b"inert-sidecar".as_slice()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        let path = source.path().join(name);
        std::fs::write(&path, raw).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(raw),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "linux".into(),
        version: "1.15.0-alpha.12".into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    let id = core.id(crate::core_hash::sha256_hex).unwrap();
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.as_str().into(),
        app_version: "1.0.0".into(),
        core,
    };
    assert_eq!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Absent { id: id.clone() }
    );
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    #[cfg(target_os = "macos")]
    {
        use polaris_core_supervisor::admitted_payload::{
            AdmissionError, AdmissionErrorKind, RecoveryAction,
        };
        let lock_path = root.path().join(".core-store.lock");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.try_lock().unwrap(); // A distinct actual OFD owns the mutation lock.
        let error = store.verify_for_start(&id).unwrap_err();
        let admission = error
            .get_ref()
            .unwrap()
            .downcast_ref::<AdmissionError>()
            .unwrap();
        assert_eq!(admission.kind, AdmissionErrorKind::Busy);
        assert_eq!(admission.recovery, RecoveryAction::RetryAfterStop);
        assert!(!admission.kind.is_payload_damage());
        assert_eq!(
            status_with_owner(Some(root.path()), &id, owner),
            CoreStatus::Invalid {
                id: id.clone(),
                reason: AdmissionErrorKind::Busy.status_token().into(),
            }
        );
        drop(held);
        let saved = root.path().join("saved-mutation-lock");
        std::fs::rename(&lock_path, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &lock_path).unwrap();
        let error = store.verify_for_start(&id).unwrap_err();
        let admission = error
            .get_ref()
            .unwrap()
            .downcast_ref::<AdmissionError>()
            .unwrap();
        assert_eq!(admission.kind, AdmissionErrorKind::Mismatch);
        assert_eq!(admission.recovery, RecoveryAction::RepairHelperInstallation);
        assert!(admission.kind.is_payload_damage());
        assert_eq!(
            status_with_owner(Some(root.path()), &id, owner),
            CoreStatus::Invalid {
                id: id.clone(),
                reason: AdmissionErrorKind::Mismatch.status_token().into(),
            }
        );
        std::fs::remove_file(&lock_path).unwrap();
        std::fs::create_dir(&lock_path).unwrap(); // Real open I/O error, not fabricated admission.
        let error = store.verify_for_start(&id).unwrap_err();
        let admission = error
            .get_ref()
            .unwrap()
            .downcast_ref::<AdmissionError>()
            .unwrap();
        assert_eq!(admission.kind, AdmissionErrorKind::Io);
        assert_eq!(admission.recovery, RecoveryAction::None);
        assert!(!admission.kind.is_payload_damage());
        assert_eq!(
            status_with_owner(Some(root.path()), &id, owner),
            CoreStatus::Invalid {
                id: id.clone(),
                reason: AdmissionErrorKind::Io.status_token().into(),
            }
        );
        std::fs::remove_dir(&lock_path).unwrap();
        std::fs::rename(saved, lock_path).unwrap();
    }
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Invalid { .. }
    ));
    // A rejected status admission must not retain a shared execution lease
    // and permanently prevent the real existing repair transaction.
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    std::fs::remove_file(
        root.path()
            .join("versions")
            .join(id.as_str())
            .join("libcronet.so"),
    )
    .unwrap();
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Invalid { .. }
    ));
    std::fs::remove_file(
        root.path()
            .join("manifests")
            .join(format!("{}.json", id.as_str())),
    )
    .unwrap();
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Invalid { .. }
    ));
}

#[test]
fn typed_rejection_round_trips_existing_safe_reason_slot_without_paths_or_details() {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionError, AdmissionErrorKind as K, RecoveryAction,
    };
    let id = CoreId::parse(&"a".repeat(64)).unwrap();
    for kind in [
        K::Missing,
        K::Mismatch,
        K::Changed,
        K::UnsupportedExactObject,
        K::UnsupportedCapability,
        K::LoaderPolicy,
        K::Busy,
        K::Unknown,
        K::Io,
    ] {
        let error: std::io::Error = AdmissionError {
            kind,
            purpose: None,
            member: Some("private-member".into()),
            recovery: RecoveryAction::None,
            detail: "/private/path raw detail".into(),
        }
        .into();
        let status = CoreStatus::Invalid {
            id: id.clone(),
            reason: status_reason(&error),
        };
        let wire = status.to_wire();
        assert!(!wire.contains("private"));
        assert!(!wire.contains("raw detail"));
        assert_eq!(
            CoreStatus::parse(
                "core-invalid",
                wire.strip_prefix("OK core-invalid ").unwrap()
            ),
            Some(status)
        );
        assert_eq!(K::from_status_token(kind.status_token()), Some(kind));
    }
}

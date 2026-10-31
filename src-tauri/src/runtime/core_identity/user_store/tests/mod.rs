use super::*;
use crate::runtime::core_identity::recovery::tests::Fixture;

#[cfg(unix)]
#[test]
fn consumer_observes_real_selected_slot_damage_only_under_original_leases() {
    use crate::runtime::helper::recovery_consumer::{
        selected_store_requirement, CoreRecoveryState as S,
    };
    let f = Fixture::new("linux");
    let stage = f.stage().unwrap();
    assert_eq!(selected_store_requirement(&f.config, &f.selected), Ok(None));
    install(&f.config, &stage).unwrap();
    assert_eq!(
        selected_store_requirement(&f.config, &f.selected),
        Err(S::NotNeeded)
    );
    for damage in ["missing", "size", "hash"] {
        let held = selected_at(&f.config, &f.selected).unwrap().unwrap();
        let binary = held.directory.join(&f.selected.files[0].name);
        let original = std::fs::read(&binary).unwrap();
        match damage {
            "missing" => std::fs::remove_file(&binary).unwrap(),
            "size" => std::fs::write(&binary, b"wrong size").unwrap(),
            _ => std::fs::write(&binary, vec![b'x'; original.len()]).unwrap(),
        }
        assert!(selected_at(&f.config, &f.selected).is_err() || damage == "hash");
        assert_eq!(
            selected_store_requirement(&f.config, &f.selected),
            Err(S::StopRequired)
        );
        assert!(install(&f.config, &stage).is_err());
        drop(held);
        assert_eq!(
            selected_store_requirement(&f.config, &f.selected),
            Ok(Some(())),
            "{damage}"
        );
        install(&f.config, &stage).unwrap();
        assert_eq!(
            selected_store_requirement(&f.config, &f.selected),
            Err(S::NotNeeded)
        );
        let repaired = selected_at(&f.config, &f.selected).unwrap().unwrap();
        assert_eq!(
            std::fs::read(repaired.directory.join(&f.selected.files[0].name)).unwrap(),
            original
        );
    }
    let data = selected_at(&f.config, &f.selected).unwrap().unwrap();
    let directory = data.directory.clone();
    drop(data);
    let unknown = directory.join("unknown-recovery-object");
    std::fs::write(&unknown, b"retain").unwrap();
    assert_eq!(
        selected_store_requirement(&f.config, &f.selected),
        Err(S::ReinstallRequired)
    );
    assert_eq!(std::fs::read(unknown).unwrap(), b"retain");
}

#[cfg(unix)]
#[test]
fn root_and_each_initialize_failure_preserve_partial_namespace_unknown_members_and_sources() {
    for failure in [
        "root",
        "versions",
        "manifests",
        ".staging",
        ".trash",
        "leases",
    ] {
        let f = Fixture::new("linux");
        let archive = std::fs::read(&f.archive).unwrap();
        let helper = std::fs::read(&f.helper).unwrap();
        std::fs::write(f.config.join("user-configuration"), b"keep user data").unwrap();
        let stage = f.stage().unwrap();
        let config = if failure == "root" {
            // The missing parent makes the first private mkdir fail; no store
            // or later install call has been reached.
            f.config.join("missing-parent")
        } else {
            let root = f.config.join(USER_STORE);
            super::super::recovery::create_private_directory(&root).unwrap();
            std::fs::write(root.join("unknown-owner-member"), b"keep unknown").unwrap();
            std::fs::write(root.join(failure), b"not a directory; retain").unwrap();
            f.config.clone()
        };
        assert!(install(&config, &stage).is_err(), "{failure}");
        if failure == "root" {
            assert!(!config.exists());
        } else {
            let root = config.join(USER_STORE);
            assert_eq!(
                std::fs::read(root.join(failure)).unwrap(),
                b"not a directory; retain"
            );
            assert_eq!(
                std::fs::read(root.join("unknown-owner-member")).unwrap(),
                b"keep unknown"
            );
            // Initialization may have made other directories. They remain
            // partial data, never a success receipt or permission to fall back.
            assert!(selected_at(&config, &f.selected).is_err());
        }
        assert_eq!(std::fs::read(&f.archive).unwrap(), archive);
        assert_eq!(std::fs::read(&f.helper).unwrap(), helper);
        assert_eq!(
            std::fs::read(f.config.join("user-configuration")).unwrap(),
            b"keep user data"
        );
        assert!(stage.directory.join("sing-box").exists());
    }
}

#[cfg(unix)]
#[test]
fn explicit_restore_is_same_id_persistent_and_idempotent_without_touching_bundle_or_user_data() {
    let f = Fixture::new("linux");
    std::fs::write(f.config.join("tailscale-state"), b"keep authorization").unwrap();
    assert!(selected_at(&f.config, &f.selected).unwrap().is_none());
    let stage = f.stage().unwrap();
    assert_eq!(
        install(&f.config, &stage).unwrap(),
        UserRecoveryState::Installed
    );
    let first = selected_at(&f.config, &f.selected).unwrap().unwrap();
    let path = first.directory.clone();
    assert!(first.data_is_current());
    drop(first);
    // Reopen the real persisted store, not a process-local success flag.
    let held_archive = f.archive.with_extension("held-fixture");
    std::fs::rename(&f.archive, &held_archive).unwrap();
    let restarted = selected_at(&f.config, &f.selected).unwrap().unwrap();
    assert_eq!(restarted.directory, path);
    drop(restarted);
    std::fs::rename(held_archive, &f.archive).unwrap();
    assert_eq!(
        install(&f.config, &stage).unwrap(),
        UserRecoveryState::AlreadyPresent
    );
    assert_eq!(
        std::fs::read(f.config.join("tailscale-state")).unwrap(),
        b"keep authorization"
    );
    assert!(f.archive.exists());
    assert!(f.helper.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_shared_data_lease_blocks_same_id_repair_and_keeps_original_inodes_until_retry() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new("linux");
    let stage = f.stage().unwrap();
    install(&f.config, &stage).unwrap();
    let held = selected_at(&f.config, &f.selected).unwrap().unwrap();
    let path = held.directory.join("sing-box");
    let before = std::fs::metadata(&path).unwrap().ino();
    // Damage can make repair necessary, but never supplies no-owner authority.
    std::fs::write(&path, b"damaged").unwrap();
    assert!(!held.data_is_current());
    assert!(install(&f.config, &stage).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), before);
    assert_eq!(std::fs::read(&path).unwrap(), b"damaged");
    drop(held);
    assert_eq!(
        install(&f.config, &stage).unwrap(),
        UserRecoveryState::Installed
    );
    let repaired = selected_at(&f.config, &f.selected).unwrap().unwrap();
    assert_eq!(
        std::fs::read(repaired.directory.join("sing-box")).unwrap(),
        b"inert core fixture"
    );
}

#[cfg(unix)]
#[test]
fn selected_partial_tampered_and_pending_slot_never_becomes_absent_or_bundle_fallback() {
    let f = Fixture::new("linux");
    let stage = f.stage().unwrap();
    install(&f.config, &stage).unwrap();
    let root = f.config.join(USER_STORE);
    let manifest = root
        .join("manifests")
        .join(format!("{}.json", stage.manifest.id));
    let original = std::fs::read(&manifest).unwrap();
    std::fs::remove_file(&manifest).unwrap();
    assert!(selected_at(&f.config, &f.selected).is_err());
    std::fs::write(&manifest, &original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::write(
        root.join("versions")
            .join(&stage.manifest.id)
            .join("unknown"),
        b"retain unknown member",
    )
    .unwrap();
    assert!(selected_at(&f.config, &f.selected).is_err());
    assert!(root
        .join("versions")
        .join(&stage.manifest.id)
        .join("unknown")
        .exists());
    assert!(f.archive.exists());
}

#[cfg(windows)]
#[test]
fn user_acl_support_is_not_faked_by_the_privileged_store_judge() {
    let f = Fixture::new("win");
    let stage = f.stage().unwrap();
    match install(&f.config, &stage) {
        Ok(UserRecoveryState::Installed | UserRecoveryState::AlreadyPresent) => {
            let data = selected_at(&f.config, &f.selected).unwrap().unwrap();
            assert!(data.data_is_current());
            let store = CoreStore::open_current_user_data(&f.config.join(USER_STORE)).unwrap();
            let id = f.selected.id(polaris_updater::sha256_hex).unwrap();
            assert_eq!(
                store.verify_for_start(&id).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        Ok(UserRecoveryState::Unsupported) => {
            assert!(!matches!(selected_at(&f.config, &f.selected), Ok(Some(_))));
        }
        Ok(UserRecoveryState::Pending) => panic!("pending is not a committed data installation"),
        Err(raw) => {
            let typed = super::super::selection_error(
                raw,
                polaris_core_supervisor::admitted_payload::ExecPurpose::CheckConfig,
            );
            assert!(matches!(
                typed.kind,
                polaris_core_supervisor::admitted_payload::AdmissionErrorKind::Io
                    | polaris_core_supervisor::admitted_payload::AdmissionErrorKind::Unknown
                    | polaris_core_supervisor::admitted_payload::AdmissionErrorKind::Changed
                    | polaris_core_supervisor::admitted_payload::AdmissionErrorKind::Busy
            ));
            assert!(!matches!(selected_at(&f.config, &f.selected), Ok(Some(_))));
        }
    }
    assert!(f.archive.is_file());
    assert!(f.helper.is_file());
    assert!(stage.directory.is_dir());
}

#[cfg(windows)]
#[test]
fn windows_structural_data_namespace_never_falls_back_or_deletes_unknown_objects() {
    use polaris_core_supervisor::admitted_payload::{AdmissionErrorKind as K, ExecPurpose};
    for ancestor in [false, true] {
        let f = Fixture::new("win");
        let path = if ancestor {
            f.config.clone()
        } else {
            f.config.join(USER_STORE)
        };
        if ancestor {
            std::fs::rename(&path, f.root.join("retained-config")).unwrap();
        }
        std::fs::write(&path, b"retain unknown data").unwrap();
        let raw = selected_at(&f.config, &f.selected).unwrap_err();
        assert!(
            crate::runtime::helper::recovery_consumer::selected_store_requirement(
                &f.config,
                &f.selected
            )
            .is_err()
        );
        assert_eq!(
            super::super::selection_error(raw, ExecPurpose::RunMain).kind,
            K::Changed
        );
        assert_eq!(std::fs::read(path).unwrap(), b"retain unknown data");
        assert!(f.archive.is_file());
    }
}

#[cfg(unix)]
#[test]
fn real_structural_damage_has_typed_reinstall_action_and_retains_objects() {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionErrorKind as K, ExecPurpose, RecoveryAction,
    };
    use std::os::unix::fs::{symlink, PermissionsExt};
    for damage in [
        "fifo",
        "hardlink",
        "four-members",
        "ancestor-link",
        "config-ancestor-link",
        "ancestor-file",
    ] {
        let f = Fixture::new("linux");
        let stage = f.stage().unwrap();
        install(&f.config, &stage).unwrap();
        let root = f.config.join(USER_STORE);
        let version = root.join("versions").join(&stage.manifest.id);
        let binary = version.join("sing-box");
        match damage {
            "fifo" => {
                std::fs::remove_file(&binary).unwrap();
                nix::unistd::mkfifo(
                    &binary,
                    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
                )
                .unwrap();
            }
            "hardlink" => {
                std::fs::hard_link(&binary, f.root.join("retained-hardlink")).unwrap();
            }
            "four-members" => {
                for name in ["unknown-one", "unknown-two"] {
                    let path = version.join(name);
                    std::fs::write(&path, b"retain unknown bytes").unwrap();
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
                }
            }
            "ancestor-link" => {
                std::fs::rename(&root, f.config.join("retained-store")).unwrap();
                symlink(f.config.join("retained-store"), &root).unwrap();
            }
            "config-ancestor-link" => {
                let retained = f.root.join("retained-config");
                std::fs::rename(&f.config, &retained).unwrap();
                symlink(retained, &f.config).unwrap();
            }
            "ancestor-file" => {
                std::fs::rename(root.join("versions"), root.join("retained-versions")).unwrap();
                std::fs::write(root.join("versions"), b"retain non-directory").unwrap();
            }
            _ => unreachable!(),
        }
        let raw = match selected_at(&f.config, &f.selected) {
            Err(error) => error,
            Ok(_) => panic!("{damage} unexpectedly became a selectable source"),
        };
        assert!(
            crate::runtime::helper::recovery_consumer::selected_store_requirement(
                &f.config,
                &f.selected
            )
            .is_err(),
            "{damage} must not become recoverable member damage"
        );
        let typed = super::super::selection_error(raw, ExecPurpose::RunMain);
        assert_eq!(typed.kind, K::Changed, "{damage}: {typed:?}");
        assert_eq!(
            typed.recovery,
            RecoveryAction::ReinstallCurrentApp,
            "{damage}"
        );
        assert!(f.archive.is_file());
        if damage == "four-members" {
            assert!(version.join("unknown-one").exists());
            assert!(version.join("unknown-two").exists());
        }
        if damage == "ancestor-file" {
            assert_eq!(
                std::fs::read(root.join("versions")).unwrap(),
                b"retain non-directory"
            );
        }
    }
}

#[test]
fn namespace_classification_keeps_real_io_busy_and_unknown_separate_from_damage() {
    use polaris_core_supervisor::admitted_payload::{
        AdmissionErrorKind as K, ExecPurpose, RecoveryAction as R,
    };
    for (kind, expected, action) in [
        (std::io::ErrorKind::PermissionDenied, K::Io, R::None),
        (std::io::ErrorKind::WouldBlock, K::Busy, R::RetryAfterStop),
        (std::io::ErrorKind::Other, K::Unknown, R::RetryAfterStop),
    ] {
        let typed = super::super::selection_error(
            namespace_error(std::io::Error::new(kind, "fixture")),
            ExecPurpose::CheckConfig,
        );
        assert_eq!(typed.kind, expected);
        assert_eq!(typed.recovery, action);
    }
    #[cfg(unix)]
    for errno in [nix::libc::ELOOP, nix::libc::ENOTDIR] {
        let typed = super::super::selection_error(
            namespace_error(std::io::Error::from_raw_os_error(errno)),
            ExecPurpose::RunMain,
        );
        assert_eq!(typed.kind, K::Changed);
        assert_eq!(typed.recovery, R::ReinstallCurrentApp);
    }
}

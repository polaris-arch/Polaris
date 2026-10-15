use super::*;
use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};

fn fixture(version: &str) -> (tempfile::TempDir, InstalledCoreManifest) {
    let source = tempfile::tempdir().unwrap();
    private_mode(source.path(), 0o700).unwrap();
    let files: Vec<_> = [
        ("sing-box", format!("core-{version}").into_bytes()),
        ("libcronet.so", b"sidecar".to_vec()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        fs::write(source.path().join(name), &raw).unwrap();
        CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(&raw),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "linux".into(),
        version: version.into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: Some("b".repeat(64)),
        cleaner_sha256: None,
    };
    let id = core
        .id(crate::core_hash::sha256_hex)
        .unwrap()
        .as_str()
        .to_owned();
    (
        source,
        InstalledCoreManifest {
            schema: "polaris-core-manifest-v1".into(),
            id,
            app_version: "1.0.0".into(),
            core,
        },
    )
}

fn store() -> (tempfile::TempDir, CoreStore, u32) {
    let root = tempfile::tempdir().unwrap();
    private_mode(root.path(), 0o700).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let store = CoreStore::initialize(root.path(), OwnerPolicy { uid: Some(uid) }).unwrap();
    (root, store, uid)
}

#[test]
fn runtime_open_is_read_only_and_does_not_invent_an_installation() {
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn mismatched_source_never_becomes_an_eligible_version() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::write(source.path().join("libcronet.so"), b"changed").unwrap();
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    assert!(!root
        .path()
        .join("manifests")
        .join(format!("{}.json", manifest.id))
        .exists());
    assert!(!root.path().join("versions").join(&manifest.id).exists());
}

#[test]
fn same_id_is_immutable_and_every_start_rechecks_contents() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::AlreadyPresent
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    let verified = store.verify_for_start(&id).unwrap();
    assert_eq!(verified.manifest.id, manifest.id);
    fs::write(
        root.path()
            .join("versions")
            .join(&manifest.id)
            .join("libcronet.so"),
        b"changed",
    )
    .unwrap();
    assert!(store.verify_for_start(&id).is_err());
    // A damaged active version cannot be replaced beneath a held lazy-sidecar lease.
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    drop(verified);
    assert_eq!(
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        InstallOutcome::Installed
    );
    assert!(store.verify_for_start(&id).is_ok());
}

#[test]
fn busy_leases_allow_new_install_but_prevent_retirement() {
    let (root, store, uid) = store();
    let mut leases = Vec::new();
    let started = std::time::Instant::now();
    for (iteration, version) in ["1.0.0", "1.0.1", "1.0.2", "1.0.3", "1.0.4"]
        .into_iter()
        .enumerate()
    {
        let (source, manifest) = fixture(version);
        store
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap_or_else(|error| panic!("busy-lease stage=install iteration={iteration} version={version} held_leases={} elapsed_ns={} error={error:?}",
                leases.len(), started.elapsed().as_nanos()));
        leases.push(
            store
                .verify_for_start(&CoreId::parse(&manifest.id).unwrap())
                .unwrap_or_else(|error| panic!("busy-lease stage=outer-start-verify iteration={iteration} version={version} held_leases={} elapsed_ns={} error={error:?}",
                    leases.len(), started.elapsed().as_nanos())),
        );
    }
    eprintln!("busy-lease stage=retire-held held_leases={}", leases.len());
    assert!(store.retain_latest_three(&[]).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("manifests")).unwrap().count(),
        5
    );
    // A shared open-description reference survives the original VerifiedCore.
    // Mutation-scope unlock must never release these distinct runtime leases.
    let inherited_leases: Vec<File> = leases
        .iter()
        .map(|core| core._lease.try_clone().unwrap())
        .collect();
    drop(leases);
    assert!(store.retain_latest_three(&[]).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("manifests")).unwrap().count(),
        5
    );
    drop(inherited_leases);
    eprintln!("busy-lease stage=retire-after-drop held_leases=0");
    let retired = store.retain_latest_three(&[]).unwrap_or_else(|error| {
        panic!(
            "busy-lease stage=retire-after-drop elapsed_ns={} error={error:?}",
            started.elapsed().as_nanos()
        )
    });
    assert_eq!(
        retired.len(),
        2,
        "busy-lease stage=retire-after-drop elapsed_ns={}",
        started.elapsed().as_nanos()
    );
    assert_eq!(
        fs::read_dir(root.path().join("versions")).unwrap().count(),
        3
    );
}

#[test]
fn lock_failures_keep_closed_roles_and_native_error_classification_without_paths() {
    for role in [
        LockRole::StoreMutationExclusive,
        LockRole::RepairExclusive,
        LockRole::OrphanExclusive,
        LockRole::StartShared,
        LockRole::RetainExclusive,
        LockRole::GcExclusive,
    ] {
        let error = lock_error(
            role,
            LockObservation {
                fd: Some(41),
                dev: Some(7),
                inode: Some(11),
                cloexec: Some(true),
            },
            TryLockError::WouldBlock,
        );
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let text = format!("{error:?}");
        assert!(text.contains(&format!("role={}", role.name())));
        for fact in [
            "class=would-block",
            "fd=Some(41)",
            "dev=Some(7)",
            "inode=Some(11)",
            "cloexec=Some(true)",
        ] {
            assert!(text.contains(fact), "missing closed observation: {fact}");
        }
    }
    let error = lock_error(
        LockRole::StoreMutationExclusive,
        LockObservation::default(),
        TryLockError::Error(io::Error::from_raw_os_error(nix::libc::EACCES)),
    );
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("class=io-error"));
    let failure = error
        .get_ref()
        .unwrap()
        .downcast_ref::<LockFailure>()
        .unwrap();
    let source = std::error::Error::source(failure)
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap();
    assert_eq!(source.raw_os_error(), Some(nix::libc::EACCES));
    // Display and Debug stay safe even when an injected original OS error has
    // a path/message. The original cause remains available through source().
    let secret = "/private/fixture/source payload=never-export";
    let error = lock_error(
        LockRole::StartShared,
        LockObservation::default(),
        TryLockError::Error(io::Error::other(secret)),
    );
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
    assert!(error.to_string().contains("fd=None"));
}

#[test]
fn optional_retirement_skips_only_native_contention_and_preserves_other_errors() {
    let root = tempfile::tempdir().unwrap();
    let file = File::create_new(root.path().join("controlled-lease")).unwrap();
    for role in [LockRole::RetainExclusive, LockRole::GcExclusive] {
        assert!(optional_lock_result(&file, role, Ok(())).unwrap());
        assert!(!optional_lock_result(&file, role, Err(TryLockError::WouldBlock)).unwrap());
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::WouldBlock] {
            let error = optional_lock_result(
                &file,
                role,
                Err(TryLockError::Error(io::Error::new(
                    kind,
                    "/private/controlled-error must-not-appear",
                ))),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(error.to_string().contains("class=io-error"));
            assert!(error.to_string().contains(role.name()));
            assert!(!error.to_string().contains("/private/controlled-error"));
        }
    }
}

#[test]
fn role_diagnostics_preserve_shared_and_exclusive_modes_and_descriptor_flags() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("controlled-lease");
    let owner = File::create_new(&path).unwrap();
    let peer_shared = File::open(&path).unwrap();
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    acquire_lock(&owner, LockRole::StartShared).unwrap();
    acquire_lock(&peer_shared, LockRole::StartShared).unwrap();
    let flags = nix::fcntl::fcntl(&contender, nix::fcntl::FcntlArg::F_GETFD).unwrap();
    for role in [
        LockRole::StoreMutationExclusive,
        LockRole::RepairExclusive,
        LockRole::OrphanExclusive,
    ] {
        let error = acquire_lock(&contender, role).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<LockFailure>()
            .unwrap();
        assert_eq!(failure.observation.fd, Some(contender.as_raw_fd()));
        assert_eq!(
            failure.observation.dev,
            Some(contender.metadata().unwrap().dev())
        );
        assert_eq!(
            failure.observation.inode,
            Some(contender.metadata().unwrap().ino())
        );
        assert_eq!(
            failure.observation.cloexec,
            Some(flags & nix::libc::FD_CLOEXEC != 0)
        );
        assert_eq!(
            nix::fcntl::fcntl(&contender, nix::fcntl::FcntlArg::F_GETFD).unwrap(),
            flags
        );
    }
    // The contender is a separately opened, initially unlocked description;
    // never attempt unspecified shared-to-exclusive conversion on a holder.
    let exclusive_path = root.path().join("exclusive-lease");
    let exclusive = File::create_new(&exclusive_path).unwrap();
    let shared_contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&exclusive_path)
        .unwrap();
    acquire_lock(&exclusive, LockRole::StoreMutationExclusive).unwrap();
    let error = acquire_lock(&shared_contender, LockRole::StartShared).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("role=start-shared"));
}

#[test]
fn linked_payload_and_weakened_store_permissions_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::remove_file(source.path().join("sing-box")).unwrap();
    symlink(
        source.path().join("libcronet.so"),
        source.path().join("sing-box"),
    )
    .unwrap();
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
}

#[test]
fn flat_retirement_preserves_unknown_or_busy_cohorts_and_all_versioned_inputs() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    for name in ["sing-box", "libcronet.so"] {
        fs::write(root.path().join(name), b"old-flat").unwrap();
    }
    for state in [
        crate::legacy_use::LegacyUse::Unknown,
        crate::legacy_use::LegacyUse::InUse,
    ] {
        assert!(store.retire_flat_layout_with(|_| state).unwrap().is_empty());
        for name in ["sing-box", "libcronet.so"] {
            assert_eq!(fs::read(root.path().join(name)).unwrap(), b"old-flat");
        }
    }
    assert_eq!(
        store
            .retire_flat_layout_with(|paths| {
                let mut observed = paths
                    .iter()
                    .map(|p| p.file_name().unwrap().to_str().unwrap())
                    .collect::<Vec<_>>();
                observed.sort_unstable();
                assert_eq!(observed, ["libcronet.so", "sing-box"]);
                assert!(paths.iter().all(|p| p.parent() == Some(root.path())));
                crate::legacy_use::LegacyUse::Idle
            })
            .unwrap()
            .len(),
        2
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    assert!(store.verify_for_start(&id).is_ok());
}

#[test]
fn mutation_scope_releases_on_error_and_unwind() {
    let (_root, store, _uid) = store();
    let error = (|| -> io::Result<()> {
        let _scope = store.mutation_lock()?;
        Err(io::Error::other("controlled mutation error"))
    })();
    assert!(error.is_err());
    drop(store.mutation_lock().unwrap());
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = store.mutation_lock().unwrap();
        panic!("controlled mutation unwind");
    }));
    assert!(unwound.is_err());
    let _next_scope = store.mutation_lock().unwrap();
}

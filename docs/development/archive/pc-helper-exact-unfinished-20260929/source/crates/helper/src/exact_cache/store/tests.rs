use super::*;
use crate::exact_cache::{
    CacheCandidate, CacheClaim, CacheContent, CacheSource, FreshCacheOwner, StoppedCacheFence,
};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::Command;
use tempfile::TempDir;

fn claim() -> CacheClaim {
    CacheClaim {
        local_id: "device-local-1".into(),
        intent_revision: "7".into(),
        boot_id: "boot-1".into(),
        owner_ref: "install-cache-1".into(),
        run_ref: "run-1".into(),
        stop_target_run_ref: "old-run-1".into(),
        lifecycle_generation: "generation-1".into(),
        source_path_sha256: "a".repeat(64),
        source_config_sha256: "b".repeat(64),
        plan_id: "plan-1".into(),
        plan_digest: "c".repeat(64),
        artifact_manifest_ref: "mesh-routes/plans/plan-1/manifest.json".into(),
        artifact_manifest_sha256: "d".repeat(64),
        artifact_digest: "d".repeat(64),
    }
}

fn pending() -> CacheJournal {
    CacheJournal::begin_migration(
        claim(),
        CacheSource::Present {
            sha256: "e".repeat(64),
            bytes: 4,
            object_key: "source-object-1".into(),
        },
        &FreshCacheOwner {
            owner_ref: "install-cache-1".into(),
            owner_uid: nix::unistd::getuid().as_raw(),
            source_path_sha256: "a".repeat(64),
        },
        &StoppedCacheFence {
            owner_ref: "install-cache-1".into(),
            lifecycle_generation: "generation-1".into(),
        },
    )
    .unwrap()
}

fn idle(pending: &CacheJournal, owner_key: &str) -> CacheJournal {
    pending
        .finish_migration(
            &CacheSource::Present {
                sha256: "e".repeat(64),
                bytes: 4,
                object_key: "source-object-1".into(),
            },
            &CacheCandidate {
                owner_key: owner_key.into(),
                content: CacheContent::Present {
                    sha256: "e".repeat(64),
                    bytes: 4,
                },
            },
            &StoppedCacheFence {
                owner_ref: "install-cache-1".into(),
                lifecycle_generation: "generation-1".into(),
            },
        )
        .unwrap()
}

fn try_store(dir: &TempDir) -> Result<CacheJournalStore, StoreError> {
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw())
}

fn store(dir: &TempDir) -> CacheJournalStore {
    try_store(dir).unwrap()
}

fn auth() -> AuthenticatedOwnerUid {
    AuthenticatedOwnerUid {
        local_id: "device-local-1".into(),
        owner_ref: "install-cache-1".into(),
        uid: nix::unistd::getuid().as_raw(),
    }
}

fn private_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[test]
fn transition_shape_keeps_install_and_candidate_join_fields_exact() {
    let first = pending();
    let idle = idle(&first, "owner-key-1");
    let mut different_install = idle.clone();
    different_install.local_id = "another-device".into();
    assert!(!transition_shape(&idle, &different_install));

    let mut reserved = idle.clone();
    reserved.seen_run_refs.insert("run-1".into());
    reserved.phase = CachePhase::Reserved {
        origin: CacheSource::Present {
            sha256: "e".repeat(64),
            bytes: 4,
            object_key: "source-object-1".into(),
        },
        owner_key: "owner-key-1".into(),
        claim: claim(),
        launch: super::super::LaunchIdentity {
            core_sha256: "f".repeat(64),
            launched_config_sha256: "1".repeat(64),
            launch_closure_digest: "2".repeat(64),
        },
    };
    assert!(transition_shape(&idle, &reserved));
    let mut checked = reserved.clone();
    if let CachePhase::Reserved {
        origin,
        owner_key,
        claim,
        launch,
    } = &reserved.phase
    {
        checked.phase = CachePhase::Checked {
            origin: origin.clone(),
            owner_key: owner_key.clone(),
            claim: claim.clone(),
            launch: launch.clone(),
        };
    }
    assert!(transition_shape(&reserved, &checked));
    let altered: [fn(&mut CacheClaim); 14] = [
        |value| value.local_id = "other-device".into(),
        |value| value.intent_revision = "8".into(),
        |value| value.boot_id = "other-boot".into(),
        |value| value.run_ref = "other-candidate".into(),
        |value| value.lifecycle_generation = "other-generation".into(),
        |value| value.plan_id = "other-plan".into(),
        |value| value.plan_digest = "9".repeat(64),
        |value| value.source_config_sha256 = "9".repeat(64),
        |value| value.source_path_sha256 = "9".repeat(64),
        |value| value.owner_ref = "other-owner".into(),
        |value| value.stop_target_run_ref = "other-old-run".into(),
        |value| value.artifact_manifest_ref = "mesh-routes/plans/other-plan/manifest.json".into(),
        |value| value.artifact_manifest_sha256 = "9".repeat(64),
        |value| value.artifact_digest = "9".repeat(64),
    ];
    for damage in altered {
        let mut drifted = checked.clone();
        if let CachePhase::Checked { claim, .. } = &mut drifted.phase {
            damage(claim);
        }
        assert!(!transition_shape(&reserved, &drifted));
    }
}

#[test]
fn canonical_roundtrip_cas_and_cross_process_lock_admission() {
    let dir = private_dir();
    let mut store = store(&dir);
    let competing_fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(competing_fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::LockBusy)
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "exact_cache::store::tests::subprocess_lock_probe",
            "--nocapture",
        ])
        .env("POLARIS_H2B_PROBE_DIR", dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = pending();
    let mut wrong_uid = first.clone();
    wrong_uid.owner_uid = auth().uid.checked_add(1).unwrap();
    assert_eq!(
        store.compare_and_replace(&auth(), None, &wrong_uid),
        Err(StoreError::UnsafeOwner)
    );
    store.compare_and_replace(&auth(), None, &first).unwrap();
    assert_eq!(
        store
            .load_for_phase(&AuthenticatedOwnerUid {
                local_id: "different-device".into(),
                owner_ref: "install-cache-1".into(),
                uid: auth().uid,
            })
            .err(),
        Some(StoreError::UnsafeOwner)
    );
    assert_eq!(
        store
            .load_for_phase(&AuthenticatedOwnerUid {
                local_id: "device-local-1".into(),
                owner_ref: "different-install".into(),
                uid: auth().uid,
            })
            .err(),
        Some(StoreError::UnsafeOwner)
    );
    assert_eq!(
        store
            .load_for_phase(&AuthenticatedOwnerUid {
                local_id: "device-local-1".into(),
                owner_ref: "install-cache-1".into(),
                uid: auth().uid.checked_add(1).unwrap(),
            })
            .err(),
        Some(StoreError::UnsafeOwner)
    );
    assert_eq!(store.load_raw().unwrap(), Some(first.clone()));
    assert_eq!(
        store.load_diagnostic().unwrap(),
        Some(JournalDiagnostic {
            owner_ref: "install-cache-1".into(),
            owner_uid: auth().uid,
            phase: "migration_pending",
        })
    );
    assert_eq!(
        store.compare_and_replace(&auth(), None, &first),
        Err(StoreError::StaleJournal)
    );
    assert_eq!(
        store.load_idle_for_recovery(&auth()).err(),
        Some(StoreError::UnsafeRecovery)
    );
    let second = idle(&first, "missing-owner-key");
    assert_eq!(
        store.compare_and_replace(&auth(), Some(&first), &second),
        Err(StoreError::UnsafeOwner)
    );
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        store.compare_and_replace(&auth(), Some(&first), &idle(&first, "forged-owner-key")),
        Err(StoreError::UnsafeOwner)
    );
    let observed = store.observe_owner(nix::unistd::getuid().as_raw()).unwrap();
    let observed_key = observed.observation().owner_key.clone();
    let second = idle(&first, &observed_key);
    drop(observed);
    store
        .compare_and_replace(&auth(), Some(&first), &second)
        .unwrap();
    let (recovered, recovered_owner) = store.load_idle_for_recovery(&auth()).unwrap().unwrap();
    assert_eq!(recovered, second);
    assert_eq!(recovered_owner.observation().owner_key, observed_key);
    drop(recovered_owner);
    assert_eq!(
        store.compare_and_replace(&auth(), Some(&second), &first),
        Err(StoreError::UnsafeRecovery)
    );
    fs::remove_dir(dir.path().join(OWNER)).unwrap();
    assert_eq!(
        store.load_idle_for_recovery(&auth()).err(),
        Some(StoreError::UnsafeOwner)
    );
    symlink("elsewhere", dir.path().join(OWNER)).unwrap();
    assert_eq!(
        store.load_idle_for_recovery(&auth()).err(),
        Some(StoreError::UnsafeOwner)
    );
    assert_eq!(
        store.compare_and_replace(&auth(), Some(&first), &first),
        Err(StoreError::UnsafeOwner)
    );
}

#[test]
fn owner_observation_requires_a_real_private_directory_and_rechecks_its_inode() {
    let dir = private_dir();
    let store = store(&dir);
    let uid = nix::unistd::getuid().as_raw();
    fs::write(dir.path().join(OWNER), b"marker-is-not-an-owner").unwrap();
    assert_eq!(
        store.observe_owner(uid).err(),
        Some(StoreError::UnsafeOwner)
    );
    fs::remove_file(dir.path().join(OWNER)).unwrap();
    symlink("elsewhere", dir.path().join(OWNER)).unwrap();
    assert_eq!(
        store.observe_owner(uid).err(),
        Some(StoreError::UnsafeOwner)
    );
    fs::remove_file(dir.path().join(OWNER)).unwrap();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(
        store.observe_owner(uid).err(),
        Some(StoreError::UnsafeOwner)
    );
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        store.observe_owner(uid.saturating_add(1)).err(),
        Some(StoreError::UnsafeOwner)
    );
    let observed = store.observe_owner(uid).unwrap();
    assert!(observed.observation().owner_key.starts_with("unix-uid-"));
    fs::rename(dir.path().join(OWNER), dir.path().join("retired-owner")).unwrap();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        observed.recheck(&store.directory),
        Err(StoreError::UnsafeOwner)
    );
}

#[test]
fn recovery_rejects_a_valid_journal_bound_to_a_missing_or_replaced_owner() {
    let dir = private_dir();
    let uid = nix::unistd::getuid().as_raw();
    let mut journal_store = store(&dir);
    let first = pending();
    journal_store
        .compare_and_replace(&auth(), None, &first)
        .unwrap();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    let observed = journal_store.observe_owner(uid).unwrap();
    let owner_key = observed.observation().owner_key.clone();
    let real_idle = idle(&first, &owner_key);
    drop(observed);
    journal_store
        .compare_and_replace(&auth(), Some(&first), &real_idle)
        .unwrap();
    drop(journal_store);

    // Simulate a valid but wrong journal recovered after out-of-band root damage.
    let forged_idle = idle(&first, "forged-owner-key");
    fs::write(
        dir.path().join(JOURNAL),
        forged_idle.to_canonical_bytes().unwrap(),
    )
    .unwrap();
    let diagnostic_store = store(&dir);
    assert_eq!(
        diagnostic_store.load_diagnostic().unwrap().unwrap().phase,
        "idle"
    );
    assert_eq!(
        diagnostic_store.load_for_phase(&auth()).err(),
        Some(StoreError::UnsafeOwner)
    );
    drop(diagnostic_store);

    fs::write(
        dir.path().join(JOURNAL),
        real_idle.to_canonical_bytes().unwrap(),
    )
    .unwrap();
    fs::rename(dir.path().join(OWNER), dir.path().join("retired-owner")).unwrap();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    let diagnostic_store = store(&dir);
    assert_eq!(
        diagnostic_store.load_for_phase(&auth()).err(),
        Some(StoreError::UnsafeOwner)
    );
}

#[test]
fn same_inode_changed_uid_cannot_reuse_a_persisted_owner_identity() {
    let dir = private_dir();
    let uid = auth().uid;
    let mut journal_store = store(&dir);
    let first = pending();
    journal_store
        .compare_and_replace(&auth(), None, &first)
        .unwrap();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    fs::set_permissions(dir.path().join(OWNER), fs::Permissions::from_mode(0o700)).unwrap();
    let opened = journal_store.observe_owner(uid).unwrap();
    let old_key = opened.observation().owner_key.clone();
    let real_idle = idle(&first, &old_key);
    drop(opened);
    journal_store
        .compare_and_replace(&auth(), Some(&first), &real_idle)
        .unwrap();

    // Deterministic metadata fault: chown changes st_uid without changing dev/ino.
    let mut changed_stat = fstatat(
        &journal_store.directory,
        OWNER,
        AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .unwrap();
    let dev = changed_stat.st_dev;
    let ino = changed_stat.st_ino;
    let changed_uid = uid.checked_add(1).unwrap();
    changed_stat.st_uid = changed_uid;
    assert_eq!((changed_stat.st_dev, changed_stat.st_ino), (dev, ino));
    assert_eq!(
        checked_owner(&changed_stat, uid),
        Err(StoreError::UnsafeOwner)
    );
    assert_ne!(owner_object_key(changed_uid, &changed_stat), old_key);
    assert_eq!(
        journal_store
            .load_for_phase(&AuthenticatedOwnerUid {
                local_id: "device-local-1".into(),
                owner_ref: "install-cache-1".into(),
                uid: changed_uid,
            })
            .err(),
        Some(StoreError::UnsafeOwner)
    );

    // When the test process is root, exercise the actual same-inode chown as well.
    if nix::unistd::geteuid().as_raw() == 0 {
        std::os::unix::fs::chown(dir.path().join(OWNER), Some(changed_uid), None).unwrap();
        let after = fstatat(
            &journal_store.directory,
            OWNER,
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert_eq!((after.st_dev, after.st_ino), (dev, ino));
        assert_eq!(after.st_uid, changed_uid);
        assert_eq!(
            journal_store.load_for_phase(&auth()).err(),
            Some(StoreError::UnsafeOwner)
        );
        assert_eq!(
            journal_store.compare_and_replace(&auth(), Some(&real_idle), &real_idle),
            Err(StoreError::UnsafeOwner)
        );
    }
}

#[test]
fn search_only_parent_can_host_uid_writable_owner_without_exposing_journal() {
    let dir = private_dir();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o711)).unwrap();
    let mut store = store(&dir);
    store
        .compare_and_replace(&auth(), None, &pending())
        .unwrap();
    let journal_mode = fs::metadata(dir.path().join(JOURNAL))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(journal_mode & 0o077, 0);
    let lock_mode = fs::metadata(dir.path().join(LOCK))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(lock_mode & 0o077, 0);
}

#[test]
fn interrupted_write_blocks_recovery_and_never_publishes_partial_bytes() {
    let dir = private_dir();
    let mut store = store(&dir);
    let first = pending();
    assert_eq!(
        store.compare_and_replace_inner(&auth(), None, &first, WriteFault::BeforeRename),
        Err(StoreError::InterruptedWrite)
    );
    assert!(!dir.path().join(JOURNAL).exists());
    assert!(dir.path().join(NEXT).exists());
    assert_eq!(store.load_raw().err(), Some(StoreError::InterruptedWrite));
    drop(store);
    assert_eq!(try_store(&dir).err(), Some(StoreError::InterruptedWrite));
}

#[test]
fn post_rename_fsync_gap_reports_uncertain_durability() {
    let dir = private_dir();
    let mut store = store(&dir);
    let first = pending();
    assert_eq!(
        store.compare_and_replace_inner(&auth(), None, &first, WriteFault::AfterRename),
        Err(StoreError::DurabilityUnknown)
    );
    assert_eq!(store.load_raw().unwrap(), Some(first));
    assert_eq!(
        store.load_idle_for_recovery(&auth()).err(),
        Some(StoreError::UnsafeRecovery)
    );
}

#[test]
fn subprocess_lock_probe() {
    let Some(path) = std::env::var_os("POLARIS_H2B_PROBE_DIR") else {
        return;
    };
    let fd = open(Path::new(&path), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::LockBusy)
    );
}

#[test]
fn orphaned_owner_and_corrupt_or_noncanonical_journal_fail_closed() {
    let dir = private_dir();
    fs::create_dir(dir.path().join(OWNER)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::MissingJournalWithOwner)
    );
    fs::remove_dir(dir.path().join(OWNER)).unwrap();
    fs::write(dir.path().join(JOURNAL), b"{broken").unwrap();
    fs::set_permissions(dir.path().join(JOURNAL), fs::Permissions::from_mode(0o600)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::CorruptJournal)
    );
    let valid = pending().to_canonical_bytes().unwrap();
    let mut whitespace = valid;
    whitespace.push(b'\n');
    fs::write(dir.path().join(JOURNAL), whitespace).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::CorruptJournal)
    );
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&pending().to_canonical_bytes().unwrap()).unwrap();
    legacy["schemaVersion"] = 1.into();
    legacy.as_object_mut().unwrap().remove("ownerUid");
    fs::write(
        dir.path().join(JOURNAL),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert_eq!(try_store(&dir).err(), Some(StoreError::CorruptJournal));
}

#[test]
fn symlink_hardlink_and_open_permissions_are_rejected() {
    let dir = private_dir();
    let outside = TempDir::new().unwrap();
    fs::write(
        outside.path().join("target"),
        pending().to_canonical_bytes().unwrap(),
    )
    .unwrap();
    symlink(outside.path().join("target"), dir.path().join(JOURNAL)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::UnsafePath)
    );
    fs::remove_file(dir.path().join(JOURNAL)).unwrap();
    fs::hard_link(outside.path().join("target"), dir.path().join(JOURNAL)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::UnsafePath)
    );
    fs::remove_file(dir.path().join(JOURNAL)).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o770)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::UnsafePath)
    );
}

#[test]
fn symlinked_lock_and_owner_are_not_treated_as_absence() {
    let dir = private_dir();
    symlink("elsewhere", dir.path().join(LOCK)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::UnsafePath)
    );
    fs::remove_file(dir.path().join(LOCK)).unwrap();
    symlink("elsewhere", dir.path().join(OWNER)).unwrap();
    let fd = open(dir.path(), DIR_FLAGS, Mode::empty()).unwrap();
    assert_eq!(
        CacheJournalStore::from_directory(fd, nix::unistd::getuid().as_raw()).err(),
        Some(StoreError::MissingJournalWithOwner)
    );
}

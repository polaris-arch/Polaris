use super::*;
use std::cell::RefCell;

type Fault = Box<dyn FnMut(Point) -> io::Result<()>>;
thread_local! { static FAULT: RefCell<Option<Fault>> = RefCell::new(None); }

pub(super) fn checkpoint(point: Point) -> io::Result<()> {
    FAULT.with(|fault| match fault.borrow_mut().as_mut() {
        Some(fault) => fault(point),
        None => Ok(()),
    })
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        FAULT.with(|fault| *fault.borrow_mut() = None);
    }
}
fn fault(callback: impl FnMut(Point) -> io::Result<()> + 'static) -> Reset {
    FAULT.with(|fault| {
        assert!(fault.borrow().is_none());
        *fault.borrow_mut() = Some(Box::new(callback));
    });
    Reset
}
fn fail_once(at: Point) -> Reset {
    let mut fired = false;
    fault(move |point| {
        if point == at && !fired {
            fired = true;
            Err(io::Error::other("injected boundary failure"))
        } else {
            Ok(())
        }
    })
}

#[test]
fn journal_rejects_truncation_tampering_path_injection_and_extra_fields() {
    let regular = Regular {
        identity: Identity("u1-2".into()),
        bytes: 4,
        sha256: "b".repeat(64),
    };
    let plan = Plan {
        selected: CoreId::parse(&"a".repeat(64)).unwrap(),
        root: Identity("u1-3".into()),
        transaction: Identity("u1-4".into()),
        lease: Identity("u1-6".into()),
        stage: "stage-one".into(),
        candidate: Directory {
            identity: Identity("u1-5".into()),
            files: [("sing-box".into(), regular.clone())].into_iter().collect(),
        },
        candidate_manifest: regular,
        original: None,
        original_manifest: None,
    };
    let raw = plan.encode().unwrap();
    assert_eq!(Plan::parse(&raw).unwrap(), plan);
    for length in 0..raw.len() {
        assert!(Plan::parse(&raw[..length]).is_err());
    }
    let mut changed = raw.clone();
    changed[40] ^= 1;
    assert!(Plan::parse(&changed).is_err());
    for name in [
        "../outside",
        "/outside",
        "a/b",
        "a\\b",
        ".",
        "..",
        "C:alias",
        "x\0y",
        "CON",
        "con.txt",
        "evil.",
        "evil ",
    ] {
        let mut changed = plan.clone();
        changed.stage = name.into();
        assert!(changed.encode().is_err());
    }
    let mut changed = raw;
    changed.extend_from_slice(b"extra\n");
    assert!(Plan::parse(&changed).is_err());
}

#[test]
fn journal_accepts_native_windows_identity_without_path_based_identity() {
    let regular = Regular {
        identity: Identity("w1234-ab-cd".into()),
        bytes: 1,
        sha256: "b".repeat(64),
    };
    let plan = Plan {
        selected: CoreId::parse(&"a".repeat(64)).unwrap(),
        root: Identity("w1234-ab-ce".into()),
        transaction: Identity("w1234-ab-cf".into()),
        lease: Identity("w1234-ab-d1".into()),
        stage: "stage-1".into(),
        candidate: Directory {
            identity: Identity("w1234-ab-d0".into()),
            files: [("sing-box.exe".into(), regular.clone())]
                .into_iter()
                .collect(),
        },
        candidate_manifest: regular,
        original: None,
        original_manifest: None,
    };
    assert_eq!(Plan::parse(&plan.encode().unwrap()).unwrap(), plan);
}

#[test]
fn io_projection_keeps_retirement_debt_and_unknown_explicit() {
    assert!(RepairOutcome::NoPendingJournal
        .into_install_result()
        .is_err());
    let error = RepairOutcome::InstalledRetirementPending {
        journal: PathBuf::from("owned-journal"),
        cause: io::Error::other("retirement failed"),
    }
    .into_install_result()
    .unwrap_err();
    assert!(error.to_string().contains("committed; retirement pending"));
    let error: io::Error = RepairError {
        cause: io::Error::new(io::ErrorKind::Interrupted, "cancel"),
        recovery: RecoveryState::Unknown,
        journal: Some(PathBuf::from("owned-journal")),
    }
    .into();
    assert_eq!(error.kind(), io::ErrorKind::Other);
}

// Native object/lock tests use only owned temporary stores. Execution policy,
// privileged Windows ACL setup and crash/power-loss durability require the
// platform integration gate; a parser test is not a Windows acceptance result.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod filesystem {
    use super::super::super::{acquire_lock, private_dir, seal_dir, LockRole, OwnerPolicy};
    use super::*;
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::{symlink, OpenOptionsExt};
    use std::sync::Arc;

    struct Fixture {
        _root: tempfile::TempDir,
        store: CoreStore,
        manifest: InstalledCoreManifest,
        selected: CoreId,
        stage: PathBuf,
        old: Directory,
        old_manifest: Regular,
    }

    impl Fixture {
        fn new() -> Self {
            let uid = nix::unistd::Uid::effective().as_raw();
            // Root tests must not pretend /tmp is protected ancestry.
            let root = if uid == 0 {
                #[cfg(target_os = "linux")]
                let base = Path::new("/root");
                #[cfg(target_os = "macos")]
                let base = Path::new("/private/var/root");
                tempfile::Builder::new()
                    .prefix("core-repair-")
                    .tempdir_in(base)
                    .unwrap()
            } else {
                tempfile::tempdir().unwrap()
            };
            private_mode(root.path(), 0o700).unwrap();
            let root_path = root.path().canonicalize().unwrap();
            let store = CoreStore::initialize(&root_path, OwnerPolicy { uid: Some(uid) }).unwrap();
            let contents = [
                ("sing-box", b"new-core".as_slice()),
                ("libcronet.so", b"new-sidecar".as_slice()),
            ];
            let core = CoreManifest {
                platform: "linux".into(),
                version: "1.0.0".into(),
                provider_fingerprint: "a".repeat(64),
                helper_sha256: Some("b".repeat(64)),
                cleaner_sha256: None,
                files: contents
                    .iter()
                    .map(|(name, raw)| CoreFile {
                        name: (*name).into(),
                        bytes: raw.len() as u64,
                        sha256: crate::core_hash::sha256_hex(raw),
                    })
                    .collect(),
            };
            let selected = core.id(crate::core_hash::sha256_hex).unwrap();
            let manifest = InstalledCoreManifest {
                schema: "polaris-core-manifest-v1".into(),
                id: selected.as_str().into(),
                app_version: "1.0.0".into(),
                core,
            };
            let stage = root_path.join(".staging").join("candidate-input");
            private_dir(&stage).unwrap();
            for (name, raw) in contents {
                write_private(&stage.join(name), raw);
            }
            seal_dir(&stage).unwrap();
            let old_path = version_path(&store, &selected);
            private_dir(&old_path).unwrap();
            write_private(&old_path.join("sing-box"), b"bad-core");
            write_private(&old_path.join("libcronet.so"), b"old-sidecar");
            write_private(&manifest_path(&store, &selected), b"{broken old receipt");
            let lease = root_path
                .join("leases")
                .join(format!("{}.lock", selected.as_str()));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(lease)
                .unwrap()
                .sync_all()
                .unwrap();
            let ns = Namespace::open(&store).unwrap();
            let old = ns.directory(&old_path).unwrap();
            let old_manifest = ns
                .regular(&manifest_path(&store, &selected), 64 * 1024)
                .unwrap();
            Self {
                _root: root,
                store,
                manifest,
                selected,
                stage,
                old,
                old_manifest,
            }
        }
        fn prepared(&self) -> PreparedCandidate {
            PreparedCandidate::open(&self.store, &self.manifest, &self.stage).unwrap()
        }
        fn guards(&self) -> (MutationLock, File) {
            let mutation = self.store.mutation_lock().unwrap();
            let lease = self.store.lease_file(&self.selected).unwrap();
            acquire_lock(&lease, LockRole::RepairExclusive).unwrap();
            (mutation, lease)
        }
        fn assert_originals(&self) {
            let ns = Namespace::open(&self.store).unwrap();
            assert_eq!(
                ns.directory(&version_path(&self.store, &self.selected))
                    .unwrap(),
                self.old
            );
            assert_eq!(
                ns.regular(&manifest_path(&self.store, &self.selected), 64 * 1024)
                    .unwrap(),
                self.old_manifest
            );
        }
        fn assert_active(&self) {
            let ns = Namespace::open(&self.store).unwrap();
            let actual = ns
                .directory(&version_path(&self.store, &self.selected))
                .unwrap();
            verify_selected(&actual, &self.manifest).unwrap();
            let parsed = InstalledCoreManifest::parse(
                &ns.read(&manifest_path(&self.store, &self.selected), 64 * 1024)
                    .unwrap(),
                crate::core_hash::sha256_hex,
            )
            .unwrap();
            assert_eq!(parsed, self.manifest);
        }
        fn retry(&self, mutation: &MutationLock, lease: &File) {
            let result = resume(
                &self.store,
                &self.selected,
                mutation,
                lease,
                &AtomicBool::new(false),
            )
            .unwrap();
            let RepairOutcome::Installed { receipt } = result else {
                panic!("expected completed retirement: {result:?}");
            };
            assert_eq!(fs::read_dir(&receipt).unwrap().count(), 2);
            assert!(receipt.join("plan").is_file());
            assert!(receipt.join("commit").is_file());
            assert!(!journal_path(&self.store, &self.selected).exists());
            self.assert_active();
        }
    }

    fn write_private(path: &Path, raw: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(raw).unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn old_pair_survives_until_durable_commit_and_is_retired_only_afterward() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let root = f.store.root.clone();
        let selected = f.selected.clone();
        let old = f.old.clone();
        let old_manifest = f.old_manifest.clone();
        let mut committed = false;
        let observation = fault(move |point| {
            if point == Point::Committed {
                committed = true;
            }
            if !committed || point == Point::Committed {
                let active = root.join("versions").join(selected.as_str());
                let backup = root
                    .join(".staging")
                    .join(format!("repair-{}", selected.as_str()))
                    .join("original-version");
                let found = [active, backup]
                    .into_iter()
                    .filter(|p| p.exists())
                    .any(|p| {
                        objects::identity(&objects::open_nofollow(&p, true).unwrap()).unwrap()
                            == old.identity
                    });
                assert!(
                    found,
                    "original directory disappeared before commit: {point:?}"
                );
                let active = root
                    .join("manifests")
                    .join(format!("{}.json", selected.as_str()));
                let backup = root
                    .join(".staging")
                    .join(format!("repair-{}", selected.as_str()))
                    .join("original-manifest");
                assert!([active, backup]
                    .into_iter()
                    .filter(|p| p.exists())
                    .any(
                        |p| objects::identity(&objects::open_nofollow(&p, false).unwrap()).unwrap()
                            == old_manifest.identity
                    ));
            }
            Ok(())
        });
        assert!(matches!(
            replace(
                &f.store,
                &f.selected,
                f.prepared(),
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap(),
            RepairOutcome::Installed { .. }
        ));
        drop(observation);
        f.assert_active();
    }

    #[test]
    fn prepare_failures_and_early_cancel_never_touch_originals() {
        for point in [
            Point::StageSync,
            Point::JournalCreate,
            Point::PlanWrite,
            Point::PlanDurable,
        ] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            if point == Point::StageSync {
                let injection = fail_once(point);
                assert!(PreparedCandidate::open(&f.store, &f.manifest, &f.stage).is_err());
                drop(injection);
            } else {
                let candidate = f.prepared();
                let injection = fail_once(point);
                let error = replace(
                    &f.store,
                    &f.selected,
                    candidate,
                    &mutation,
                    &lease,
                    &AtomicBool::new(false),
                )
                .unwrap_err();
                assert_eq!(error.recovery, RecoveryState::Untouched, "{point:?}");
                drop(injection);
            }
            f.assert_originals();
            assert!(f.stage.is_dir());
            assert!(!journal_path(&f.store, &f.selected).exists());
            let error = replace(
                &f.store,
                &f.selected,
                f.prepared(),
                &mutation,
                &lease,
                &AtomicBool::new(true),
            )
            .unwrap_err();
            assert_eq!(error.recovery, RecoveryState::Untouched);
            f.assert_originals();
            assert!(matches!(
                replace(
                    &f.store,
                    &f.selected,
                    f.prepared(),
                    &mutation,
                    &lease,
                    &AtomicBool::new(false)
                )
                .unwrap(),
                RepairOutcome::Installed { .. }
            ));
        }
    }

    #[test]
    fn publication_failures_restore_exact_original_pair_then_resume_same_candidate() {
        for point in [
            Point::CandidateMove,
            Point::PreserveVersion,
            Point::PreserveManifest,
            Point::PublishVersion,
            Point::PublishManifest,
            Point::PublishedSync,
        ] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let injection = fail_once(point);
            let error = replace(
                &f.store,
                &f.selected,
                candidate,
                &mutation,
                &lease,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert_eq!(
                error.recovery,
                RecoveryState::OriginalsRestored,
                "{point:?}: {error}"
            );
            drop(injection);
            f.assert_originals();
            f.retry(&mutation, &lease);
        }
    }

    #[test]
    fn commit_failures_are_unknown_and_preserve_originals_for_resume() {
        for point in [Point::CommitWrite, Point::CommitSync, Point::Committed] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let injection = fail_once(point);
            let error = replace(
                &f.store,
                &f.selected,
                candidate,
                &mutation,
                &lease,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert_eq!(error.recovery, RecoveryState::Unknown);
            let journal = error.journal.unwrap();
            assert!(journal.join("original-version").is_dir());
            assert!(journal.join("original-manifest").is_file());
            drop(injection);
            f.retry(&mutation, &lease);
        }
    }

    #[test]
    fn retirement_debt_and_partial_member_retirement_are_resumable() {
        for point in [
            Point::RetireVersion,
            Point::RetireMember,
            Point::RetireManifest,
            Point::Receipt,
        ] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let mut members = 0;
            let injection = fault(move |at| {
                if at == Point::RetireMember {
                    members += 1;
                }
                if at == point && (point != Point::RetireMember || members == 2) {
                    Err(io::Error::other("retirement boundary failure"))
                } else {
                    Ok(())
                }
            });
            let result = replace(
                &f.store,
                &f.selected,
                candidate,
                &mutation,
                &lease,
                &AtomicBool::new(false),
            )
            .unwrap();
            let RepairOutcome::InstalledRetirementPending { journal, cause } = result else {
                panic!("debt lost: {result:?}");
            };
            assert!(journal.join("commit").exists());
            assert!(!cause.to_string().is_empty());
            f.assert_active();
            if point == Point::RetireMember {
                assert_eq!(
                    fs::read_dir(journal.join("original-version"))
                        .unwrap()
                        .count(),
                    1
                );
            }
            drop(injection);
            f.retry(&mutation, &lease);
        }
    }

    #[test]
    fn failed_rollback_never_claims_restored_and_resume_reconciles_partial_pair() {
        for rollback in [
            Point::Rollback,
            Point::RollbackManifest,
            Point::RollbackVersion,
            Point::RestoreVersion,
            Point::RestoreManifest,
            Point::RollbackSync,
        ] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let injection = fault(move |at| {
                if at == Point::PublishedSync || at == rollback {
                    Err(io::Error::other("publication and rollback failure"))
                } else {
                    Ok(())
                }
            });
            let error = replace(
                &f.store,
                &f.selected,
                candidate,
                &mutation,
                &lease,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert_eq!(error.recovery, RecoveryState::Unknown);
            let journal = error.journal.unwrap();
            assert!(
                journal.join("original-version").is_dir()
                    || version_path(&f.store, &f.selected).is_dir()
            );
            drop(injection);
            f.retry(&mutation, &lease);
        }
    }

    #[test]
    fn changed_candidate_and_foreign_retirement_objects_are_never_deleted() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let candidate = f.prepared();
        fs::write(f.stage.join("sing-box"), b"tampered").unwrap();
        assert_eq!(
            replace(
                &f.store,
                &f.selected,
                candidate,
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .recovery,
            RecoveryState::Untouched
        );
        f.assert_originals();

        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let candidate = f.prepared();
        let injection = fail_once(Point::RetireVersion);
        let result = replace(
            &f.store,
            &f.selected,
            candidate,
            &mutation,
            &lease,
            &AtomicBool::new(false),
        )
        .unwrap();
        let RepairOutcome::InstalledRetirementPending { journal, .. } = result else {
            panic!("expected debt");
        };
        drop(injection);
        fs::rename(
            journal.join("original-version"),
            f.store.root.join("retained-original"),
        )
        .unwrap();
        private_dir(&journal.join("original-version")).unwrap();
        write_private(&journal.join("original-version/foreign-marker"), b"keep");
        assert_eq!(
            resume(
                &f.store,
                &f.selected,
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .recovery,
            RecoveryState::Unknown
        );
        assert_eq!(
            fs::read(journal.join("original-version/foreign-marker")).unwrap(),
            b"keep"
        );
        assert_eq!(
            fs::read(f.store.root.join("retained-original/sing-box")).unwrap(),
            b"bad-core"
        );
    }

    #[test]
    fn unexpected_original_members_are_preserved_instead_of_becoming_cleanup_targets() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let extra = version_path(&f.store, &f.selected).join("config.json");
        write_private(&extra, b"keep");
        assert_eq!(
            replace(
                &f.store,
                &f.selected,
                f.prepared(),
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .recovery,
            RecoveryState::Untouched
        );
        assert_eq!(fs::read(&extra).unwrap(), b"keep");
        assert_eq!(
            fs::read(manifest_path(&f.store, &f.selected)).unwrap(),
            b"{broken old receipt"
        );
        assert!(!journal_path(&f.store, &f.selected).exists());
    }

    #[test]
    fn cancel_before_commit_restores_but_after_commit_only_defers_retirement() {
        for after_commit in [false, true] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let cancel = Arc::new(AtomicBool::new(false));
            let signal = Arc::clone(&cancel);
            let injection = fault(move |point| {
                if point
                    == if after_commit {
                        Point::Committed
                    } else {
                        Point::PreserveManifest
                    }
                {
                    signal.store(true, Ordering::Release);
                }
                Ok(())
            });
            let result = replace(&f.store, &f.selected, candidate, &mutation, &lease, &cancel);
            if after_commit {
                assert!(matches!(
                    result.unwrap(),
                    RepairOutcome::InstalledRetirementPending { .. }
                ));
                f.assert_active();
            } else {
                assert_eq!(
                    result.unwrap_err().recovery,
                    RecoveryState::OriginalsRestored
                );
                f.assert_originals();
            }
            drop(injection);
            cancel.store(false, Ordering::Release);
            f.retry(&mutation, &lease);
        }
    }

    #[test]
    fn actual_mutation_and_exclusive_lease_remain_held_through_all_boundaries() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let root = f.store.root.clone();
        let id = f.selected.clone();
        let observation = fault(move |_| {
            let mutation = OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join(".core-store.lock"))
                .unwrap();
            let lease =
                File::open(root.join("leases").join(format!("{}.lock", id.as_str()))).unwrap();
            assert!(matches!(
                mutation.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            assert!(matches!(
                lease.try_lock_shared(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            Ok(())
        });
        assert!(matches!(
            replace(
                &f.store,
                &f.selected,
                f.prepared(),
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap(),
            RepairOutcome::Installed { .. }
        ));
        drop(observation);
    }

    #[test]
    fn busy_parent_and_wrong_guard_object_cannot_start_a_repair() {
        let f = Fixture::new();
        let reader = f.store.lease_file(&f.selected).unwrap();
        acquire_lock(&reader, LockRole::StartShared).unwrap();
        let mutation = f.store.mutation_lock().unwrap();
        let second = f.store.lease_file(&f.selected).unwrap();
        assert_eq!(
            acquire_lock(&second, LockRole::RepairExclusive)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        f.assert_originals();
        assert!(!journal_path(&f.store, &f.selected).exists());
        let wrong = File::open(f.store.root.join(".core-store.lock")).unwrap();
        let error = replace(
            &f.store,
            &f.selected,
            f.prepared(),
            &mutation,
            &wrong,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.recovery, RecoveryState::Untouched);
        f.assert_originals();
    }

    #[test]
    fn symlink_hardlink_and_replaced_stage_never_enter_the_active_namespace() {
        for attack in 0..4 {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            if attack == 0 {
                fs::hard_link(f.stage.join("sing-box"), f.store.root.join("alias")).unwrap();
                assert!(PreparedCandidate::open(&f.store, &f.manifest, &f.stage).is_err());
            } else if attack == 1 {
                fs::remove_file(f.stage.join("sing-box")).unwrap();
                symlink(
                    version_path(&f.store, &f.selected).join("sing-box"),
                    f.stage.join("sing-box"),
                )
                .unwrap();
                assert!(PreparedCandidate::open(&f.store, &f.manifest, &f.stage).is_err());
            } else if attack == 2 {
                let candidate = f.prepared();
                fs::rename(&f.stage, f.store.root.join("held-stage")).unwrap();
                symlink(f.store.root.join("held-stage"), &f.stage).unwrap();
                assert!(replace(
                    &f.store,
                    &f.selected,
                    candidate,
                    &mutation,
                    &lease,
                    &AtomicBool::new(false)
                )
                .is_err());
            } else {
                let candidate = f.prepared();
                fs::hard_link(
                    version_path(&f.store, &f.selected).join("sing-box"),
                    f.store.root.join("old-alias"),
                )
                .unwrap();
                assert!(replace(
                    &f.store,
                    &f.selected,
                    candidate,
                    &mutation,
                    &lease,
                    &AtomicBool::new(false)
                )
                .is_err());
                fs::remove_file(f.store.root.join("old-alias")).unwrap();
            }
            f.assert_originals();
        }
    }

    #[test]
    fn ancestor_replacement_is_unknown_and_does_not_delete_the_substitute() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let candidate = f.prepared();
        let root = f.store.root.clone();
        let injection = fault(move |at| {
            if at == Point::PreserveVersion {
                fs::rename(root.join("versions"), root.join("versions-held"))?;
                private_dir(&root.join("versions"))?;
                write_private(&root.join("versions").join("foreign-marker"), b"keep");
            }
            Ok(())
        });
        let error = replace(
            &f.store,
            &f.selected,
            candidate,
            &mutation,
            &lease,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.recovery, RecoveryState::Unknown);
        assert_eq!(
            fs::read(f.store.root.join("versions/foreign-marker")).unwrap(),
            b"keep"
        );
        assert!(f
            .store
            .root
            .join("versions-held")
            .join(f.selected.as_str())
            .join("sing-box")
            .exists());
        drop(injection);
    }

    #[test]
    fn a_preexisting_write_descriptor_is_not_frozen_by_a_read_handle() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let candidate = f.prepared();
        let mut writer = OpenOptions::new()
            .write(true)
            .open(version_path(&f.store, &f.selected).join("sing-box"))
            .unwrap();
        let injection = fault(move |at| {
            if at == Point::PreserveManifest {
                writer.seek(SeekFrom::Start(0))?;
                writer.write_all(b"tampered")?;
                writer.sync_all()?;
            }
            Ok(())
        });
        let error = replace(
            &f.store,
            &f.selected,
            candidate,
            &mutation,
            &lease,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.recovery, RecoveryState::Unknown);
        let original = error.journal.unwrap().join("original-version/sing-box");
        assert_eq!(fs::read(original).unwrap(), b"tampered");
        drop(injection);
    }

    #[test]
    fn foreign_journal_members_and_torn_plans_are_preserved_without_guessing() {
        for torn in [false, true] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            let candidate = f.prepared();
            let injection = fail_once(Point::PublishManifest);
            assert_eq!(
                replace(
                    &f.store,
                    &f.selected,
                    candidate,
                    &mutation,
                    &lease,
                    &AtomicBool::new(false)
                )
                .unwrap_err()
                .recovery,
                RecoveryState::OriginalsRestored
            );
            drop(injection);
            let journal = journal_path(&f.store, &f.selected);
            if torn {
                fs::write(journal.join("plan"), b"torn").unwrap();
            } else {
                write_private(&journal.join("foreign"), b"keep");
            }
            let error = resume(
                &f.store,
                &f.selected,
                &mutation,
                &lease,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert_eq!(error.recovery, RecoveryState::Unknown);
            f.assert_originals();
            assert!(if torn {
                journal.join("plan").exists()
            } else {
                journal.join("foreign").exists()
            });
        }
    }

    #[test]
    fn resumed_transaction_rejects_a_replaced_native_lease_object() {
        let f = Fixture::new();
        let (mutation, lease) = f.guards();
        let injection = fail_once(Point::PublishManifest);
        assert_eq!(
            replace(
                &f.store,
                &f.selected,
                f.prepared(),
                &mutation,
                &lease,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .recovery,
            RecoveryState::OriginalsRestored
        );
        drop(injection);
        let path = f
            .store
            .root
            .join("leases")
            .join(format!("{}.lock", f.selected.as_str()));
        fs::rename(&path, f.store.root.join("retained-lease")).unwrap();
        write_private(&path, b"");
        let replacement = f.store.lease_file(&f.selected).unwrap();
        acquire_lock(&replacement, LockRole::RepairExclusive).unwrap();
        assert_eq!(
            resume(
                &f.store,
                &f.selected,
                &mutation,
                &replacement,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .recovery,
            RecoveryState::Unknown
        );
        f.assert_originals();
        assert!(journal_path(&f.store, &f.selected)
            .join("candidate")
            .exists());
    }

    #[test]
    fn orphan_directory_or_manifest_uses_the_same_pair_transaction() {
        for missing in ["version", "manifest", "both"] {
            let f = Fixture::new();
            let (mutation, lease) = f.guards();
            // Only fixture setup removes its own synthetic initial objects.
            if missing != "manifest" {
                fs::remove_dir_all(version_path(&f.store, &f.selected)).unwrap();
            }
            if missing != "version" {
                fs::remove_file(manifest_path(&f.store, &f.selected)).unwrap();
            }
            assert!(matches!(
                replace(
                    &f.store,
                    &f.selected,
                    f.prepared(),
                    &mutation,
                    &lease,
                    &AtomicBool::new(false)
                )
                .unwrap(),
                RepairOutcome::Installed { .. }
            ));
            f.assert_active();
        }
    }
    fn parent_install(f: &Fixture) -> io::Result<super::super::super::InstallOutcome> {
        f.store
            .install(&f.manifest, &f.stage, f.store.owner.uid, None)
    }
    fn reopened(f: &Fixture) -> CoreStore {
        CoreStore::open(&f.store.root, f.store.owner).unwrap()
    }
    fn audit_path(f: &Fixture) -> PathBuf {
        let rows: Vec<_> = archived_names(&f.store)
            .unwrap()
            .into_iter()
            .filter(|(id, _)| id == &f.selected)
            .collect();
        assert_eq!(rows.len(), 1);
        f.store.root.join(".trash").join(&rows[0].1)
    }
    fn lease_identity(f: &Fixture) -> Identity {
        objects::identity(&f.store.lease_file(&f.selected).unwrap()).unwrap()
    }

    #[test]
    fn parent_copy_failure_preserves_originals_before_any_publication() {
        let f = Fixture::new();
        let lease = lease_identity(&f);
        fs::write(f.stage.join("libcronet.so"), b"bad input").unwrap();
        assert!(parent_install(&f).is_err());
        f.assert_originals();
        assert_eq!(lease_identity(&f), lease);
        assert!(!journal_path(&f.store, &f.selected).exists());
    }

    #[test]
    fn parent_unknown_cleanup_keeps_original_version_and_saved_manifest_then_resumes_without_input()
    {
        let f = Fixture::new();
        let lease = lease_identity(&f);
        let injection = fault(|at| {
            if matches!(at, Point::PublishedSync | Point::RestoreManifest) {
                Err(io::Error::other("publication and restore failure"))
            } else {
                Ok(())
            }
        });
        let error = parent_install(&f).unwrap_err();
        let detail = error
            .get_ref()
            .unwrap()
            .downcast_ref::<RepairError>()
            .unwrap();
        assert_eq!(detail.recovery, RecoveryState::Unknown);
        let journal = journal_path(&f.store, &f.selected);
        assert!(journal.join("candidate").is_dir());
        assert!(journal.join("original-manifest").is_file());
        assert!(!manifest_path(&f.store, &f.selected).exists());
        let ns = Namespace::open(&f.store).unwrap();
        assert_eq!(
            ns.directory(&version_path(&f.store, &f.selected)).unwrap(),
            f.old
        );
        assert_eq!(lease_identity(&f), lease);
        drop(injection);
        let restarted = reopened(&f);
        assert_eq!(
            restarted
                .install(
                    &f.manifest,
                    &f.store.root.join("absent-new-input"),
                    f.store.owner.uid,
                    None
                )
                .unwrap(),
            super::super::super::InstallOutcome::Installed
        );
        f.assert_active();
        assert_eq!(lease_identity(&f), lease);
    }

    #[test]
    fn parent_pending_fences_gc_retention_migration_and_busy_retry_before_copy() {
        let f = Fixture::new();
        let injection = fail_once(Point::PublishManifest);
        assert!(parent_install(&f).is_err());
        drop(injection);
        let journal = journal_path(&f.store, &f.selected);
        let plan = fs::read(journal.join("plan")).unwrap();
        let lease = lease_identity(&f);
        for result in [f.store.gc(&[]), f.store.retain_latest_three(&[])] {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        }
        assert_eq!(
            f.store
                .install_preserving(&f.manifest, &f.stage, f.store.owner.uid, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        let reader = f.store.lease_file(&f.selected).unwrap();
        acquire_lock(&reader, LockRole::StartShared).unwrap();
        assert_eq!(
            parent_install(&f).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(fs::read(journal.join("plan")).unwrap(), plan);
        assert_eq!(lease_identity(&f), lease);
        f.assert_originals();
        drop(reader);
        assert_eq!(
            parent_install(&f).unwrap(),
            super::super::super::InstallOutcome::Installed
        );
        f.assert_active();
    }

    #[test]
    fn parent_resume_precedes_already_present_and_preserves_torn_or_replaced_lease_evidence() {
        for replace_lease in [false, true] {
            let f = Fixture::new();
            let injection = fail_once(Point::Committed);
            assert!(parent_install(&f).is_err());
            drop(injection);
            f.assert_active();
            let journal = journal_path(&f.store, &f.selected);
            if replace_lease {
                let lease = f
                    .store
                    .root
                    .join("leases")
                    .join(format!("{}.lock", f.selected.as_str()));
                fs::rename(&lease, f.store.root.join("retained-native-lease")).unwrap();
                write_private(&lease, b"");
            } else {
                fs::write(journal.join("plan"), b"torn").unwrap();
            }
            let error = reopened(&f)
                .install(
                    &f.manifest,
                    &f.store.root.join("missing-source"),
                    f.store.owner.uid,
                    None,
                )
                .unwrap_err();
            assert_eq!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<RepairError>()
                    .unwrap()
                    .recovery,
                RecoveryState::Unknown
            );
            assert!(journal.join("original-version").is_dir());
            assert!(journal.join("original-manifest").is_file());
            f.assert_active();
        }
    }

    #[test]
    fn parent_orphan_shapes_preserve_the_same_lease_and_publish_the_full_pair() {
        for missing in ["version", "manifest"] {
            let f = Fixture::new();
            let lease = lease_identity(&f);
            if missing == "version" {
                fs::remove_dir_all(version_path(&f.store, &f.selected)).unwrap();
            } else {
                fs::remove_file(manifest_path(&f.store, &f.selected)).unwrap();
            }
            assert_eq!(
                parent_install(&f).unwrap(),
                super::super::super::InstallOutcome::Installed
            );
            assert_eq!(lease_identity(&f), lease);
            f.assert_active();
        }
    }

    #[test]
    fn parent_missing_old_lease_never_creates_a_substitute_or_deletes_originals() {
        let f = Fixture::new();
        let path = f
            .store
            .root
            .join("leases")
            .join(format!("{}.lock", f.selected.as_str()));
        fs::remove_file(&path).unwrap();
        assert_eq!(
            parent_install(&f).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        f.assert_originals();
        assert!(!path.exists());
        assert!(!journal_path(&f.store, &f.selected).exists());
    }

    #[test]
    fn parent_guards_cover_candidate_copy_sync_publication_and_retirement() {
        let f = Fixture::new();
        let root = f.store.root.clone();
        let id = f.selected.clone();
        let observation = fault(move |_| {
            let mutation = File::open(root.join(".core-store.lock")).unwrap();
            let lease =
                File::open(root.join("leases").join(format!("{}.lock", id.as_str()))).unwrap();
            assert!(matches!(
                mutation.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            assert!(matches!(
                lease.try_lock_shared(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            Ok(())
        });
        parent_install(&f).unwrap();
        drop(observation);
        f.assert_active();
    }

    #[test]
    fn parent_reopened_retry_replays_every_post_archive_rename_barrier_before_already_present() {
        for point in [
            Point::ArchiveDirectorySync,
            Point::ArchiveStagingSync,
            Point::ArchiveTrashSync,
        ] {
            let f = Fixture::new();
            let lease = lease_identity(&f);
            let injection = fail_once(point);
            let error = parent_install(&f).unwrap_err();
            let debt = error
                .get_ref()
                .unwrap()
                .downcast_ref::<RetirementPending>()
                .unwrap();
            assert_eq!(
                debt.journal.parent(),
                Some(f.store.root.join(".trash").as_path())
            );
            let archive = debt.journal.clone();
            assert!(!journal_path(&f.store, &f.selected).exists());
            assert_eq!(fs::read_dir(&archive).unwrap().count(), 2);
            f.assert_active();
            drop(injection);
            let retry_failure = fail_once(point);
            let restarted = reopened(&f);
            let error = restarted
                .install(
                    &f.manifest,
                    &f.store.root.join("missing-source"),
                    f.store.owner.uid,
                    None,
                )
                .unwrap_err();
            assert_eq!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<RetirementPending>()
                    .unwrap()
                    .journal,
                archive
            );
            drop(retry_failure);
            assert_eq!(
                reopened(&f)
                    .install(
                        &f.manifest,
                        &f.store.root.join("still-missing-source"),
                        f.store.owner.uid,
                        None
                    )
                    .unwrap(),
                super::super::super::InstallOutcome::AlreadyPresent
            );
            assert_eq!(lease_identity(&f), lease);
        }
    }

    #[test]
    fn parent_gc_cannot_swallow_archived_barrier_debt_and_completed_audits_do_not_pin_forever() {
        for point in [
            Point::ArchiveDirectorySync,
            Point::ArchiveStagingSync,
            Point::ArchiveTrashSync,
        ] {
            let f = Fixture::new();
            let injection = fail_once(point);
            assert!(parent_install(&f).is_err());
            drop(injection);
            let audit = audit_path(&f);
            let plan = fs::read(audit.join("plan")).unwrap();
            let lease = lease_identity(&f);
            fs::remove_file(manifest_path(&f.store, &f.selected)).unwrap();
            let failure = fail_once(point);
            assert!(reopened(&f)
                .gc(&[])
                .unwrap_err()
                .get_ref()
                .unwrap()
                .downcast_ref::<RetirementPending>()
                .is_some());
            assert!(version_path(&f.store, &f.selected).is_dir());
            assert_eq!(lease_identity(&f), lease);
            assert_eq!(fs::read(audit.join("plan")).unwrap(), plan);
            drop(failure);
            assert_eq!(reopened(&f).gc(&[]).unwrap(), vec![f.selected.clone()]);
            assert!(audit.is_dir());
            assert!(!version_path(&f.store, &f.selected).exists());
            assert_eq!(lease_identity(&f), lease);
        }
    }

    #[test]
    fn parent_retention_replays_archive_barriers_before_removing_the_receipt() {
        for point in [
            Point::ArchiveDirectorySync,
            Point::ArchiveStagingSync,
            Point::ArchiveTrashSync,
        ] {
            let f = Fixture::new();
            parent_install(&f).unwrap();
            File::open(manifest_path(&f.store, &f.selected))
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(10))
                .unwrap();
            for index in 0..3 {
                let mut manifest = f.manifest.clone();
                manifest.core.version = format!("2.0.{index}");
                manifest.id = manifest
                    .core
                    .id(crate::core_hash::sha256_hex)
                    .unwrap()
                    .as_str()
                    .into();
                f.store
                    .install(&manifest, &f.stage, f.store.owner.uid, None)
                    .unwrap();
            }
            let audit = audit_path(&f);
            let lease = lease_identity(&f);
            let failure = fail_once(point);
            assert!(reopened(&f)
                .retain_latest_three(&[])
                .unwrap_err()
                .get_ref()
                .unwrap()
                .downcast_ref::<RetirementPending>()
                .is_some());
            assert!(manifest_path(&f.store, &f.selected).is_file());
            assert!(version_path(&f.store, &f.selected).is_dir());
            assert_eq!(lease_identity(&f), lease);
            assert_eq!(fs::read_dir(&audit).unwrap().count(), 2);
            drop(failure);
            assert_eq!(
                reopened(&f).retain_latest_three(&[]).unwrap(),
                vec![f.selected.clone()]
            );
            assert!(audit.is_dir());
            assert!(!manifest_path(&f.store, &f.selected).exists());
        }
    }

    #[test]
    fn archived_tampering_or_retained_data_is_unknown_and_never_a_completed_audit() {
        for attack in 0..3 {
            let f = Fixture::new();
            parent_install(&f).unwrap();
            let audit = audit_path(&f);
            if attack == 0 {
                write_private(&audit.join("unexpected-data"), b"keep");
            } else if attack == 1 {
                fs::write(audit.join("commit"), b"wrong").unwrap();
            } else {
                let mut name = audit.file_name().unwrap().to_str().unwrap().to_owned();
                let last = name.pop().unwrap();
                name.push(if last == 'a' { 'b' } else { 'a' });
                fs::rename(&audit, f.store.root.join(".trash").join(name)).unwrap();
            }
            let error = parent_install(&f).unwrap_err();
            assert_eq!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<RepairError>()
                    .unwrap()
                    .recovery,
                RecoveryState::Unknown
            );
            f.assert_active();
            if attack == 0 {
                assert_eq!(fs::read(audit.join("unexpected-data")).unwrap(), b"keep");
            }
        }
    }

    #[test]
    fn parent_prepared_sync_failure_retains_completed_candidate_and_exact_originals() {
        let f = Fixture::new();
        let before = lease_identity(&f);
        let injection = fail_once(Point::StageSync);
        assert!(parent_install(&f).is_err());
        drop(injection);
        f.assert_originals();
        assert_eq!(lease_identity(&f), before);
        assert!(!journal_path(&f.store, &f.selected).exists());
        let staged: Vec<_> = fs::read_dir(f.store.root.join(".staging"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path != &f.stage)
            .collect();
        assert_eq!(staged.len(), 1);
        for expected in &f.manifest.core.files {
            assert_eq!(
                crate::core_hash::sha256_hex(&fs::read(staged[0].join(&expected.name)).unwrap()),
                expected.sha256
            );
        }
    }

    #[test]
    fn historical_completed_audit_does_not_require_retired_candidate_inodes() {
        let f = Fixture::new();
        parent_install(&f).unwrap();
        let first = audit_path(&f);
        let first_plan = Plan::parse(&fs::read(first.join("plan")).unwrap()).unwrap();
        fs::write(
            version_path(&f.store, &f.selected).join("sing-box"),
            b"damage",
        )
        .unwrap();
        parent_install(&f).unwrap();
        let current = Namespace::open(&f.store)
            .unwrap()
            .identity_at(&version_path(&f.store, &f.selected), true)
            .unwrap();
        assert_ne!(current, first_plan.candidate.identity);
        assert_eq!(archived_names(&f.store).unwrap().len(), 2);
        assert_eq!(
            reopened(&f)
                .install(
                    &f.manifest,
                    &f.store.root.join("no-input"),
                    f.store.owner.uid,
                    None
                )
                .unwrap(),
            super::super::super::InstallOutcome::AlreadyPresent
        );
        assert!(first.join("plan").exists());
        f.assert_active();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn fresh_publication_faults_never_report_success_and_reopen_replays_real_barriers() {
    let points = [
        Point::FreshLeasePublished,
        Point::FreshVersionPublished,
        Point::FreshReceiptPublished,
        Point::PublicationPayloadSync,
        Point::PublicationStagingSync,
        Point::PublicationVersionsSync,
        Point::PublicationLeaseSync,
        Point::PublicationLeasesSync,
        Point::PublicationReceiptSync,
        Point::PublicationManifestsSync,
        Point::PublicationRootSync,
        Point::PublicationParentSync,
    ];
    for point in points {
        let (root, store, uid) = super::super::tests::store();
        let (source, manifest) = super::super::tests::fixture("fresh-durable");
        let id = CoreId::parse(&manifest.id).unwrap();
        let original = std::fs::read(source.path().join("sing-box")).unwrap();
        {
            let _fault = fail_once(point);
            assert!(
                store
                    .install(&manifest, source.path(), Some(uid), None)
                    .is_err(),
                "{point:?}"
            );
        }
        assert_eq!(
            std::fs::read(source.path().join("sing-box")).unwrap(),
            original
        );
        let lease = store
            .root
            .join("leases")
            .join(format!("{}.lock", id.as_str()));
        assert!(
            lease.is_file(),
            "partial publication must retain the actual lease: {point:?}"
        );
        let version = store.root.join("versions").join(id.as_str());
        if version.exists() {
            assert_eq!(std::fs::read(version.join("sing-box")).unwrap(), original);
        }
        drop(store);
        // A new CoreStore replays actual fsync calls. This is fault/retry
        // sequencing in one kernel, explicitly not a power-loss simulation.
        let reopened = CoreStore::open(
            &root.path().canonicalize().unwrap(),
            super::super::OwnerPolicy { uid: Some(uid) },
        )
        .unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = seen.clone();
        let _record = fault(move |point| {
            observed.lock().unwrap().push(point);
            Ok(())
        });
        let outcome = reopened
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap();
        assert!(matches!(
            outcome,
            super::super::InstallOutcome::Installed | super::super::InstallOutcome::AlreadyPresent
        ));
        for barrier in [
            Point::PublicationVersionsSync,
            Point::PublicationLeasesSync,
            Point::PublicationReceiptSync,
            Point::PublicationManifestsSync,
            Point::PublicationRootSync,
            Point::PublicationParentSync,
        ] {
            assert!(
                seen.lock().unwrap().contains(&barrier),
                "retry skipped {barrier:?} after {point:?}"
            );
        }
        let data = reopened.verify_objects(&id, true).unwrap();
        assert!(data.data_is_current());
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn already_present_must_repeat_failed_parent_barriers_before_reporting_success() {
    let (root, store, uid) = super::super::tests::store();
    let (source, manifest) = super::super::tests::fixture("already-durable");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    for point in [
        Point::PublicationVersionsSync,
        Point::PublicationLeasesSync,
        Point::PublicationManifestsSync,
        Point::PublicationRootSync,
        Point::PublicationParentSync,
    ] {
        let _fault = fail_once(point);
        assert!(
            store
                .install(&manifest, source.path(), Some(uid), None)
                .is_err(),
            "{point:?}"
        );
    }
    drop(store);
    let reopened = CoreStore::open(
        &root.path().canonicalize().unwrap(),
        super::super::OwnerPolicy { uid: Some(uid) },
    )
    .unwrap();
    assert_eq!(
        reopened
            .install(&manifest, source.path(), Some(uid), None)
            .unwrap(),
        super::super::InstallOutcome::AlreadyPresent
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn every_new_parent_barrier_is_real_and_failure_requires_a_complete_retry() {
    let (source, manifest) = super::super::tests::fixture("parent-durable");
    let (sample_root, sample_store, _) = super::super::tests::store();
    let parent_count = sample_store.root.ancestors().skip(1).count();
    drop(sample_store);
    drop(sample_root);
    for fail_at in 1..=parent_count {
        let (root, store, uid) = super::super::tests::store();
        let mut seen = 0;
        let injection = fault(move |point| {
            if point == Point::PublicationParentSync {
                seen += 1;
                if seen == fail_at {
                    return Err(io::Error::other("parent directory barrier unavailable"));
                }
            }
            Ok(())
        });
        assert!(store
            .install(&manifest, source.path(), Some(uid), None)
            .is_err());
        drop(injection);
        drop(store);
        let reopened = CoreStore::open(
            &root.path().canonicalize().unwrap(),
            super::super::OwnerPolicy { uid: Some(uid) },
        )
        .unwrap();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let _trace = fault(move |point| {
            if point == Point::PublicationParentSync {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        });
        assert_eq!(
            reopened
                .install(&manifest, source.path(), Some(uid), None)
                .unwrap(),
            super::super::InstallOutcome::AlreadyPresent
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            reopened.root.ancestors().skip(1).count()
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_fresh_publication_and_retry_never_delete_a_new_unknown_member() {
    let (root, store, uid) = super::super::tests::store();
    let (source, manifest) = super::super::tests::fixture("unknown-after-publish");
    let alien = store
        .root
        .join("versions")
        .join(&manifest.id)
        .join("retain-unknown");
    let injected = alien.clone();
    let _fault = fault(move |point| {
        if point == Point::PublicationRootSync {
            std::fs::write(&injected, b"unknown original data")?;
            private_mode(&injected, 0o600)?;
            return Err(io::Error::other(
                "directory barrier failed after foreign mutation",
            ));
        }
        Ok(())
    });
    assert!(store
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    drop(_fault);
    drop(store);
    assert_eq!(std::fs::read(&alien).unwrap(), b"unknown original data");
    let reopened = CoreStore::open(
        &root.path().canonicalize().unwrap(),
        super::super::OwnerPolicy { uid: Some(uid) },
    )
    .unwrap();
    assert!(reopened
        .install(&manifest, source.path(), Some(uid), None)
        .is_err());
    assert_eq!(std::fs::read(&alien).unwrap(), b"unknown original data");
}

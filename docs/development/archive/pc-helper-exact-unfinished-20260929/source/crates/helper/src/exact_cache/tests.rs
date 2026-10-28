use super::*;
use polaris_helper_proto::ExactBinding;

fn claim(run: &str, generation: &str, config_digit: char) -> CacheClaim {
    CacheClaim {
        local_id: "device-local-1".into(),
        intent_revision: "7".into(),
        boot_id: "boot-1".into(),
        owner_ref: "install-cache-1".into(),
        run_ref: run.into(),
        stop_target_run_ref: "old-run-1".into(),
        lifecycle_generation: generation.into(),
        source_path_sha256: "a".repeat(64),
        source_config_sha256: config_digit.to_string().repeat(64),
        plan_id: "plan-1".into(),
        plan_digest: "d".repeat(64),
        artifact_manifest_ref: "mesh-routes/plans/plan-1/manifest.json".into(),
        artifact_manifest_sha256: "e".repeat(64),
        artifact_digest: "e".repeat(64),
    }
}

fn fence(generation: &str) -> StoppedCacheFence {
    StoppedCacheFence {
        owner_ref: "install-cache-1".into(),
        lifecycle_generation: generation.into(),
    }
}

fn fresh() -> FreshCacheOwner {
    FreshCacheOwner {
        owner_ref: "install-cache-1".into(),
        owner_uid: 1000,
        source_path_sha256: "a".repeat(64),
    }
}

fn source() -> CacheSource {
    CacheSource::Present {
        sha256: "b".repeat(64),
        bytes: 4096,
        object_key: "dev1-ino7-mtime1".into(),
    }
}

fn candidate() -> CacheCandidate {
    CacheCandidate {
        owner_key: "protected-owner-1".into(),
        content: CacheContent::Present {
            sha256: "b".repeat(64),
            bytes: 4096,
        },
    }
}

fn owner() -> CacheOwnerObservation {
    CacheOwnerObservation {
        owner_key: "protected-owner-1".into(),
    }
}

fn launch() -> LaunchIdentity {
    LaunchIdentity {
        core_sha256: "f".repeat(64),
        launched_config_sha256: "1".repeat(64),
        launch_closure_digest: "2".repeat(64),
    }
}

fn checked(launch: LaunchIdentity) -> CheckedLaunchIdentity {
    CheckedLaunchIdentity { launch }
}

fn receipt(claim: &CacheClaim, launch: &LaunchIdentity) -> ExactReceipt {
    ExactReceipt {
        binding: ExactBinding {
            run_ref: claim.run_ref.clone(),
            plan_id: claim.plan_id.clone(),
            plan_digest: claim.plan_digest.clone(),
            artifact_digest: claim.artifact_digest.clone(),
            config_sha256: claim.source_config_sha256.clone(),
            core_sha256: launch.core_sha256.clone(),
        },
        launched_config_sha256: launch.launched_config_sha256.clone(),
        launch_closure_digest: launch.launch_closure_digest.clone(),
        daemon_birth: crate::exact::daemon_birth().into(),
        pid: 42,
        process_birth: "birth-42".into(),
    }
}

fn observed(claim: &CacheClaim, receipt: ExactReceipt) -> ObservedStarted {
    ObservedStarted {
        lifecycle_generation: claim.lifecycle_generation.clone(),
        observed_process: CacheProcess {
            daemon_birth: receipt.daemon_birth.clone(),
            pid: receipt.pid,
            process_birth: receipt.process_birth.clone(),
        },
        receipt,
    }
}

fn idle_present() -> CacheJournal {
    let first = claim("run-1", "generation-1", 'c');
    CacheJournal::begin_migration(first, source(), &fresh(), &fence("generation-1"))
        .unwrap()
        .finish_migration(&source(), &candidate(), &fence("generation-1"))
        .unwrap()
}

fn finish_run(idle: &CacheJournal, claim: CacheClaim) -> CacheJournal {
    let launch = launch();
    let reserved = idle
        .reserve(
            claim.clone(),
            launch.clone(),
            &owner(),
            &fence(&claim.lifecycle_generation),
        )
        .unwrap();
    let checked = reserved
        .record_checked(
            &accept_strict_check(
                &launch,
                StrictCheckOutcome::ExitedSuccess {
                    checked: checked(launch.clone()),
                },
            )
            .unwrap(),
        )
        .unwrap();
    let started = receipt(&claim, &launch);
    let active = checked
        .record_started(&observed(&claim, started.clone()))
        .unwrap();
    active
        .release_exact(&ExactCacheExit {
            run_ref: claim.run_ref,
            lifecycle_generation: claim.lifecycle_generation,
            process: CacheProcess {
                daemon_birth: started.daemon_birth,
                pid: started.pid,
                process_birth: started.process_birth,
            },
        })
        .unwrap()
}

#[test]
fn migration_preserves_initial_bytes_and_later_reuses_mutable_owner() {
    let first = claim("run-1", "generation-1", 'c');
    let pending =
        CacheJournal::begin_migration(first.clone(), source(), &fresh(), &fence("generation-1"))
            .unwrap();
    assert_eq!(
        pending.recover_idle(&first, &owner(), &fence("generation-1")),
        Err(CacheError::UnsafeTransition)
    );
    let encoded = pending.to_canonical_bytes().unwrap();
    assert_eq!(CacheJournal::parse_canonical(&encoded).unwrap(), pending);
    let idle = pending
        .finish_migration(&source(), &candidate(), &fence("generation-1"))
        .unwrap();
    idle.recover_idle(&first, &owner(), &fence("generation-1"))
        .unwrap();

    let launch = launch();
    let reserved = idle
        .reserve(
            first.clone(),
            launch.clone(),
            &owner(),
            &fence("generation-1"),
        )
        .unwrap();
    let check = accept_strict_check(
        &launch,
        StrictCheckOutcome::ExitedSuccess {
            checked: checked(launch.clone()),
        },
    )
    .unwrap();
    let checked = reserved.record_checked(&check).unwrap();
    let started = receipt(&first, &launch);
    let active = checked
        .record_started(&observed(&first, started.clone()))
        .unwrap();
    let exited = ExactCacheExit {
        run_ref: first.run_ref.clone(),
        lifecycle_generation: first.lifecycle_generation.clone(),
        process: CacheProcess {
            daemon_birth: started.daemon_birth,
            pid: started.pid,
            process_birth: started.process_birth,
        },
    };
    let idle = active.release_exact(&exited).unwrap();
    assert_eq!(
        idle.reserve(first, launch.clone(), &owner(), &fence("generation-1")),
        Err(CacheError::Mismatch),
        "a released run/generation cannot be replayed"
    );
    let second = claim("run-2", "generation-2", '3');
    let reused = idle
        .reserve(second, launch, &owner(), &fence("generation-2"))
        .unwrap();
    assert!(matches!(reused.phase, CachePhase::Reserved { .. }));
    assert!(matches!(idle.phase, CachePhase::Idle { .. }));
}

#[test]
fn absent_cache_requires_explicit_absence_on_both_sides() {
    let absent = CacheSource::Absent {
        parent_key: "source-parent-1".into(),
    };
    let pending = CacheJournal::begin_migration(
        claim("run-1", "generation-1", 'c'),
        absent.clone(),
        &fresh(),
        &fence("generation-1"),
    )
    .unwrap();
    assert_eq!(
        pending.finish_migration(&absent, &candidate(), &fence("generation-1")),
        Err(CacheError::Mismatch)
    );
    assert_eq!(
        pending.finish_migration(
            &CacheSource::Absent {
                parent_key: "replaced-parent".into(),
            },
            &CacheCandidate {
                owner_key: "protected-owner-1".into(),
                content: CacheContent::Absent,
            },
            &fence("generation-1"),
        ),
        Err(CacheError::Mismatch)
    );
    let idle = pending
        .finish_migration(
            &absent,
            &CacheCandidate {
                owner_key: "protected-owner-1".into(),
                content: CacheContent::Absent,
            },
            &fence("generation-1"),
        )
        .unwrap();
    assert!(matches!(idle.phase, CachePhase::Idle { .. }));
}

#[test]
fn source_replacement_or_incomplete_copy_stays_pending() {
    let mut wrong_owner = fresh();
    wrong_owner.owner_ref = "other-install".into();
    assert_eq!(
        CacheJournal::begin_migration(
            claim("run-1", "generation-1", 'c'),
            source(),
            &wrong_owner,
            &fence("generation-1"),
        ),
        Err(CacheError::Mismatch),
        "fresh-owner evidence must bind the requested owner"
    );
    let pending = CacheJournal::begin_migration(
        claim("run-1", "generation-1", 'c'),
        source(),
        &fresh(),
        &fence("generation-1"),
    )
    .unwrap();
    let changed = CacheSource::Present {
        sha256: "b".repeat(64),
        bytes: 4096,
        object_key: "replaced-object".into(),
    };
    assert_eq!(
        pending.finish_migration(&changed, &candidate(), &fence("generation-1")),
        Err(CacheError::Mismatch)
    );
    assert_eq!(
        pending.finish_migration(&source(), &candidate(), &fence("stale-generation")),
        Err(CacheError::Mismatch),
        "a recovered pending migration needs a fresh matching stop fence"
    );
    let mut short = candidate();
    short.content = CacheContent::Present {
        sha256: "b".repeat(64),
        bytes: 1024,
    };
    assert_eq!(
        pending.finish_migration(&source(), &short, &fence("generation-1")),
        Err(CacheError::Mismatch)
    );
    assert!(matches!(pending.phase, CachePhase::MigrationPending { .. }));
}

#[test]
fn strict_check_only_accepts_exact_rewritten_identity() {
    let expected = launch();
    for failed in [
        StrictCheckOutcome::ExitedFailure,
        StrictCheckOutcome::SpawnFailed,
        StrictCheckOutcome::TimedOut,
    ] {
        assert!(matches!(
            accept_strict_check(&expected, failed),
            Err(CacheError::UnsafeTransition)
        ));
    }
    let mut source_check = expected.clone();
    source_check.launched_config_sha256 = "9".repeat(64);
    assert!(matches!(
        accept_strict_check(
            &expected,
            StrictCheckOutcome::ExitedSuccess {
                checked: checked(source_check)
            }
        ),
        Err(CacheError::Mismatch)
    ));
}

#[test]
fn started_receipt_must_bind_plan_source_rewrite_and_closure() {
    let claim = claim("run-1", "generation-1", 'c');
    let launch = launch();
    let reserved = idle_present()
        .reserve(
            claim.clone(),
            launch.clone(),
            &owner(),
            &fence("generation-1"),
        )
        .unwrap();
    let check = accept_strict_check(
        &launch,
        StrictCheckOutcome::ExitedSuccess {
            checked: checked(launch.clone()),
        },
    )
    .unwrap();
    let checked = reserved.record_checked(&check).unwrap();
    let mut mismatched = [
        receipt(&claim, &launch),
        receipt(&claim, &launch),
        receipt(&claim, &launch),
        receipt(&claim, &launch),
    ];
    mismatched[0].binding.plan_id = "other-plan".into();
    mismatched[1].binding.config_sha256 = "8".repeat(64);
    mismatched[2].launched_config_sha256 = "8".repeat(64);
    mismatched[3].launch_closure_digest = "8".repeat(64);
    for bad in mismatched {
        assert_eq!(
            checked.record_started(&observed(&claim, bad)),
            Err(CacheError::Mismatch)
        );
    }
    assert!(matches!(checked.phase, CachePhase::Checked { .. }));
}

#[test]
fn crash_phases_never_auto_resume_or_release_by_pid() {
    let claim = claim("run-1", "generation-1", 'c');
    let launch = launch();
    let idle = idle_present();
    let reserved = idle
        .reserve(
            claim.clone(),
            launch.clone(),
            &owner(),
            &fence("generation-1"),
        )
        .unwrap();
    let token = accept_strict_check(
        &launch,
        StrictCheckOutcome::ExitedSuccess {
            checked: checked(launch.clone()),
        },
    )
    .unwrap();
    let checked = reserved.record_checked(&token).unwrap();
    let active = checked
        .record_started(&observed(&claim, receipt(&claim, &launch)))
        .unwrap();
    for blocked in [&reserved, &checked, &active] {
        assert_eq!(
            &CacheJournal::parse_canonical(&blocked.to_canonical_bytes().unwrap()).unwrap(),
            blocked
        );
        assert_eq!(
            blocked.recover_idle(&claim, &owner(), &fence("generation-1")),
            Err(CacheError::UnsafeTransition)
        );
    }
    let wrong_birth = ExactCacheExit {
        run_ref: claim.run_ref,
        lifecycle_generation: claim.lifecycle_generation,
        process: CacheProcess {
            daemon_birth: crate::exact::daemon_birth().into(),
            pid: 42,
            process_birth: "reused-pid-birth".into(),
        },
    };
    assert_eq!(
        active.release_exact(&wrong_birth),
        Err(CacheError::Mismatch)
    );
}

#[test]
fn journal_rejects_unknown_version_fields_and_noncanonical_bytes() {
    let idle = idle_present();
    let canonical = idle.to_canonical_bytes().unwrap();
    let mut spaced = canonical.clone();
    spaced.push(b'\n');
    assert_eq!(
        CacheJournal::parse_canonical(&spaced),
        Err(CacheError::NonCanonical)
    );
    let mut value: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
    value["schemaVersion"] = 1.into();
    assert_eq!(
        CacheJournal::parse_canonical(&serde_json::to_vec(&value).unwrap()),
        Err(CacheError::Invalid)
    );
    value["schemaVersion"] = 3.into();
    value["unrecognizedFutureLease"] = true.into();
    assert_eq!(
        CacheJournal::parse_canonical(&serde_json::to_vec(&value).unwrap()),
        Err(CacheError::NonCanonical)
    );
    value
        .as_object_mut()
        .unwrap()
        .remove("unrecognizedFutureLease");
    value.as_object_mut().unwrap().remove("ownerUid");
    value["schemaVersion"] = 1.into();
    assert_eq!(
        CacheJournal::parse_canonical(&serde_json::to_vec(&value).unwrap()),
        Err(CacheError::NonCanonical),
        "a pre-UID v1 journal cannot be migrated by parsing it as v3"
    );

    let mut old_version: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
    old_version["schemaVersion"] = 2.into();
    assert_eq!(
        CacheJournal::parse_canonical(&serde_json::to_vec(&old_version).unwrap()),
        Err(CacheError::Invalid),
        "the unpublished v2 journal must not be reinterpreted as v3"
    );
    old_version["schemaVersion"] = 3.into();
    old_version.as_object_mut().unwrap().remove("localId");
    assert_eq!(
        CacheJournal::parse_canonical(&serde_json::to_vec(&old_version).unwrap()),
        Err(CacheError::NonCanonical)
    );
}

#[test]
fn typed_join_requires_every_field_without_confusing_old_and_candidate_runs() {
    let base = claim("run-1", "generation-1", 'c');
    let invalid: [fn(&mut CacheClaim); 10] = [
        |value| value.local_id.clear(),
        |value| value.intent_revision = "07".into(),
        |value| value.boot_id.clear(),
        |value| value.stop_target_run_ref.clear(),
        |value| value.stop_target_run_ref = value.run_ref.clone(),
        |value| value.plan_id = "../other".into(),
        |value| value.artifact_manifest_ref = "mesh-routes/plans/other/manifest.json".into(),
        |value| value.artifact_manifest_ref.clear(),
        |value| value.artifact_manifest_sha256 = "f".repeat(64),
        |value| value.artifact_digest = "f".repeat(64),
    ];
    for damage in invalid {
        let mut bad = base.clone();
        damage(&mut bad);
        assert_eq!(
            CacheJournal::begin_migration(bad, source(), &fresh(), &fence("generation-1")),
            Err(CacheError::Invalid)
        );
    }
    let pending =
        CacheJournal::begin_migration(base.clone(), source(), &fresh(), &fence("generation-1"))
            .unwrap();
    let canonical = pending.to_canonical_bytes().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
    assert_eq!(value["localId"], base.local_id);
    for field in [
        "localId",
        "intentRevision",
        "bootId",
        "stopTargetRunRef",
        "artifactManifestRef",
        "artifactManifestSha256",
    ] {
        let mut missing = value.clone();
        missing["phase"]["claim"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_eq!(
            CacheJournal::parse_canonical(&serde_json::to_vec(&missing).unwrap()),
            Err(CacheError::NonCanonical),
            "missing {field} must not be treated as an older optional field"
        );
    }

    let idle = pending
        .finish_migration(&source(), &candidate(), &fence("generation-1"))
        .unwrap();
    let mut different_local = claim("run-2", "generation-2", '3');
    different_local.local_id = "different-device".into();
    assert_eq!(
        idle.reserve(different_local, launch(), &owner(), &fence("generation-2")),
        Err(CacheError::Mismatch)
    );
}

#[test]
fn owner_and_generation_mismatch_block_reuse() {
    let idle = idle_present();
    assert_eq!(
        idle.reserve(
            claim("run-2", "generation-2", '3'),
            launch(),
            &owner(),
            &fence("generation-1"),
        ),
        Err(CacheError::Mismatch)
    );
    assert_eq!(
        idle.reserve(
            claim("run-2", "generation-2", '3'),
            launch(),
            &CacheOwnerObservation {
                owner_key: "different-owner-directory".into(),
            },
            &fence("generation-2"),
        ),
        Err(CacheError::Mismatch)
    );
}

#[test]
fn a_b_a_replay_is_rejected_even_with_a_new_generation() {
    let idle = idle_present();
    let idle = finish_run(&idle, claim("run-a", "generation-1", 'c'));
    let idle = finish_run(&idle, claim("run-b", "generation-2", '3'));
    let recovered = CacheJournal::parse_canonical(&idle.to_canonical_bytes().unwrap()).unwrap();
    let replay = claim("run-a", "generation-3", '4');
    assert_eq!(
        recovered.reserve(replay, launch(), &owner(), &fence("generation-3")),
        Err(CacheError::Mismatch),
        "a public exact receipt has no generation; runRef may never be recycled"
    );
}

#[test]
fn observed_started_requires_generation_and_independent_os_birth_match() {
    let claim = claim("run-1", "generation-1", 'c');
    let launch = launch();
    let reserved = idle_present()
        .reserve(
            claim.clone(),
            launch.clone(),
            &owner(),
            &fence("generation-1"),
        )
        .unwrap();
    let checked = reserved
        .record_checked(
            &accept_strict_check(
                &launch,
                StrictCheckOutcome::ExitedSuccess {
                    checked: checked(launch.clone()),
                },
            )
            .unwrap(),
        )
        .unwrap();
    let dto = receipt(&claim, &launch);
    let mut stale_generation = observed(&claim, dto.clone());
    stale_generation.lifecycle_generation = "generation-2".into();
    assert_eq!(
        checked.record_started(&stale_generation),
        Err(CacheError::Mismatch)
    );
    let mut forged_birth = observed(&claim, dto.clone());
    forged_birth.observed_process.process_birth = "other-birth".into();
    assert_eq!(
        checked.record_started(&forged_birth),
        Err(CacheError::Mismatch)
    );
    let mut forged_pid = observed(&claim, dto);
    forged_pid.observed_process.pid += 1;
    assert_eq!(
        checked.record_started(&forged_pid),
        Err(CacheError::Mismatch)
    );
    let mut forged_daemon = observed(&claim, receipt(&claim, &launch));
    forged_daemon.observed_process.daemon_birth = "other-daemon".into();
    assert_eq!(
        checked.record_started(&forged_daemon),
        Err(CacheError::Mismatch)
    );
    assert!(matches!(checked.phase, CachePhase::Checked { .. }));
}

#[test]
fn run_ref_history_capacity_fails_closed_without_pruning() {
    let mut idle = idle_present();
    idle.seen_run_refs = (0..MAX_SEEN_RUN_REFS)
        .map(|index| format!("seen-{index}"))
        .collect();
    assert_eq!(
        idle.reserve(
            claim("new-run", "generation-2", '3'),
            launch(),
            &owner(),
            &fence("generation-2"),
        ),
        Err(CacheError::RunRefCapacity)
    );
    assert_eq!(idle.seen_run_refs.len(), MAX_SEEN_RUN_REFS);
}

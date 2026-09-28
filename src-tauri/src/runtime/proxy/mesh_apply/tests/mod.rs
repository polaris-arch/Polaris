use super::*;
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshCandidate, ManagedMeshPlanInput,
};
use polaris_config_engine::user_config::mesh_route_state::{
    revise_semantic, MeshRoutePolicy, MeshRouteState,
};
use serde_json::Value;
use std::collections::BTreeMap;

fn fixture() -> (MeshRouteState, ManagedMeshRoutePlan) {
    let raw: Value = serde_json::from_str(include_str!(
        "../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    let mut state: MeshRouteState = serde_json::from_value(raw["meshRouteState"].clone()).unwrap();
    state.revision = "1".into();
    state.active_plan = Some(MeshActivePlan {
        plan_id: "old-plan".into(),
        digest: "old-digest".into(),
        config_version: "old-config".into(),
        input_state_revision: "0".into(),
    });
    let plan = ManagedMeshRoutePlan {
        schema_version: 1,
        plan_id: "plan-2".into(),
        config_version: "config-2".into(),
        input_state_revision: "1".into(),
        identity_bindings: vec![MeshOwnerRef {
            server_id: "ts-a".into(),
            identity_epoch: "epoch-a".into(),
        }],
        protected_cidrs: vec!["100.80.0.0/16".into()],
        owner_routes: vec![],
        reject_cidrs: vec!["100.80.0.0/16".into()],
        unassigned_cidrs: vec!["100.80.0.0/16".into()],
        released_cidrs: vec![],
        overrides: vec![],
        dns_managed: false,
    };
    (state, plan)
}

fn compiled_plan(state: MeshRouteState, with_endpoint: bool) -> ManagedMeshRoutePlan {
    let raw: Value = serde_json::from_str(include_str!(
        "../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    let policy: MeshRoutePolicy = serde_json::from_value(raw["meshRoutePolicy"].clone()).unwrap();
    compile_managed_mesh_plan(ManagedMeshPlanInput {
        plan_id: "compiled-plan".into(),
        config_version: "config-2".into(),
        policy,
        state,
        candidates: if with_endpoint {
            vec![ManagedMeshCandidate {
                owner_ref: MeshOwnerRef {
                    server_id: "ts-a".into(),
                    identity_epoch: "epoch-a".into(),
                },
                configured_cidrs: vec!["100.80.0.0/16".into()],
                endpoint_tag: Some("ep-ts-a".into()),
                evidence_complete: true,
            }]
        } else {
            vec![]
        },
        scopeable_rule_matchers: BTreeMap::new(),
    })
    .unwrap()
}

fn persist(previous: &MeshRouteState, candidate: MeshRouteState) -> MeshRouteState {
    revise_semantic(previous, &previous.revision, candidate)
        .unwrap()
        .expect("journal step must change state")
}

fn advance_at(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    claim: &ApplyClaim,
    event: PhaseEvent<'_>,
) -> Result<MeshRouteState, ApplyError> {
    let plan = fixture().1;
    super::advance(
        state,
        expected_state_revision,
        current_config_version,
        &claim.boot_id,
        &claim.lifecycle_generation,
        claim,
        &plan,
        event,
    )
}

fn prepared() -> (MeshRouteState, ManagedMeshRoutePlan, ApplyClaim) {
    let (state, plan) = fixture();
    let candidate = record_prepared(
        &state,
        "1",
        "config-2",
        &plan,
        "boot-a",
        "generation-1",
        "mesh-routes/plans/plan-2/manifest.json",
    )
    .unwrap();
    let state = persist(&state, candidate);
    let claim = ApplyClaim::from(state.transaction.as_ref().unwrap());
    (state, plan, claim)
}

#[test]
fn stop_reservation_records_generation_but_not_old_exit() {
    let (state, plan) = fixture();
    let prepared = persist(
        &state,
        record_prepared(
            &state,
            "1",
            "config-2",
            &plan,
            "boot-a",
            "7",
            "mesh-routes/plans/plan-2/manifest.json",
        )
        .unwrap(),
    );
    let claim = ApplyClaim::from(prepared.transaction.as_ref().unwrap());
    assert_eq!(
        request_stop_reserved(
            &prepared,
            &prepared.revision,
            "config-2",
            "boot-a",
            9,
            &claim,
            &plan,
            7,
            9,
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    assert_eq!(
        request_stop_reserved(
            &prepared,
            &prepared.revision,
            "config-2",
            "boot-b",
            8,
            &claim,
            &plan,
            7,
            8,
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    let reserved = persist(
        &prepared,
        request_stop_reserved(
            &prepared,
            &prepared.revision,
            "config-2",
            "boot-a",
            8,
            &claim,
            &plan,
            7,
            8,
        )
        .unwrap(),
    );
    let tx = reserved.transaction.as_ref().unwrap();
    assert_eq!(tx.phase, MeshTransactionPhase::StopRequested);
    assert_eq!(tx.lifecycle_generation, "8");
    assert_eq!(tx.previous_plan_id.as_deref(), Some("old-plan"));
    assert_eq!(reserved.active_plan, prepared.active_plan);
    let stop_claim = ApplyClaim::from(tx);
    assert_eq!(
        advance(
            &reserved,
            &reserved.revision,
            "config-2",
            "boot-a",
            "8",
            &stop_claim,
            &plan,
            PhaseEvent::OldStopped {
                exited: true,
                owners_released: false,
            },
        )
        .unwrap_err(),
        ApplyError::Invalid("illegal Apply phase transition")
    );
    assert_eq!(
        reserved.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::StopRequested
    );
    let stopped = advance(
        &reserved,
        &reserved.revision,
        "config-2",
        "boot-a",
        "8",
        &stop_claim,
        &plan,
        PhaseEvent::OldStopped {
            exited: true,
            owners_released: true,
        },
    )
    .unwrap();
    assert_eq!(
        stopped.transaction.unwrap().phase,
        MeshTransactionPhase::OldStopped
    );
}

fn step(state: &MeshRouteState, claim: &ApplyClaim, event: PhaseEvent<'_>) -> MeshRouteState {
    let plan = fixture().1;
    persist(
        state,
        super::advance(
            state,
            &state.revision,
            "config-2",
            &claim.boot_id,
            &claim.lifecycle_generation,
            claim,
            &plan,
            event,
        )
        .unwrap(),
    )
}

fn core_receipt(state: &MeshRouteState, claim: &ApplyClaim) -> CoreReceipt {
    CoreReceipt {
        claim: claim.clone(),
        run_id: "run-2".into(),
        identity_bindings: state
            .transaction
            .as_ref()
            .unwrap()
            .identity_bindings
            .clone(),
    }
}

fn platform_receipt(plan: &ManagedMeshRoutePlan, core: &CoreReceipt) -> PlatformReceipt {
    PlatformReceipt {
        core: core.clone(),
        scope: "tun:all-apps".into(),
        protected_cidrs: plan.protected_cidrs.clone(),
        reject_cidrs: plan.reject_cidrs.clone(),
        overrides_digest: polaris_updater::verify::sha256_hex(
            &serde_json::to_vec(&plan.overrides).unwrap(),
        ),
        result: ProtectionResult::Complete,
        evidence: "platform-instance-2".into(),
    }
}

#[test]
fn prepared_requires_current_plan_and_unique_safe_artifact_generation() {
    let (state, mut plan) = fixture();
    assert_eq!(
        record_prepared(&state, "0", "config-2", &plan, "boot", "gen", "manifest").unwrap_err(),
        ApplyError::Conflict
    );
    assert_eq!(
        record_prepared(&state, "1", "config-3", &plan, "boot", "gen", "manifest").unwrap_err(),
        ApplyError::Conflict
    );
    plan.plan_id = "../other".into();
    assert_eq!(
        record_prepared(&state, "1", "config-2", &plan, "boot", "gen", "manifest").unwrap_err(),
        ApplyError::Invalid("incomplete Apply claim")
    );
    plan.plan_id = "old-plan".into();
    assert_eq!(
        record_prepared(
            &state,
            "1",
            "config-2",
            &plan,
            "boot",
            "gen",
            "mesh-routes/plans/old-plan/manifest.json"
        )
        .unwrap_err(),
        ApplyError::Invalid("reused planId")
    );
    let (state, plan, claim) = prepared();
    assert_eq!(claim.plan_digest, plan_digest(&plan).unwrap());
    assert_eq!(state.intent.desired_run, MeshDesiredRun::Running);
    assert_eq!(
        state
            .transaction
            .as_ref()
            .unwrap()
            .previous_plan_id
            .as_deref(),
        Some("old-plan")
    );
    assert_eq!(
        state.transaction.as_ref().unwrap().input_state_revision,
        "1"
    );
    assert_eq!(state.revision, "2"); // journal CAS revision is separate
}

#[test]
fn missing_historical_owner_stays_in_reject_plan_without_blocking_apply_journal() {
    let (mut state, _) = fixture();
    state.identities.clear();
    let plan = compiled_plan(state.clone(), false);
    assert!(plan.owner_routes.is_empty());
    assert!(plan.identity_bindings.contains(&MeshOwnerRef {
        server_id: "ts-a".into(),
        identity_epoch: "epoch-a".into(),
    }));
    assert!(plan.reject_cidrs.contains(&"100.80.0.0/16".to_string()));
    let candidate = record_prepared(
        &state,
        "1",
        "config-2",
        &plan,
        "boot-a",
        "generation-1",
        "mesh-routes/plans/compiled-plan/manifest.json",
    )
    .unwrap();
    let state = persist(&state, candidate);
    let claim = ApplyClaim::from(state.transaction.as_ref().unwrap());
    let next = advance(
        &state,
        &state.revision,
        "config-2",
        "boot-a",
        "generation-1",
        &claim,
        &plan,
        PhaseEvent::NoOldCore,
    )
    .unwrap();
    assert_eq!(
        next.transaction.unwrap().phase,
        MeshTransactionPhase::OldStopped
    );
}

#[test]
fn compiled_owner_route_requires_current_bound_epoch() {
    let (mut state, _) = fixture();
    let plan = compiled_plan(state.clone(), true);
    assert!(!plan.owner_routes.is_empty());
    state.identities[0].binding_state = MeshBindingState::Retired;
    assert_eq!(
        record_prepared(
            &state,
            "1",
            "config-2",
            &plan,
            "boot-a",
            "generation-1",
            "mesh-routes/plans/compiled-plan/manifest.json",
        )
        .unwrap_err(),
        ApplyError::Invalid("active owner binding is not bound")
    );
}

#[test]
fn stop_and_core_receipts_gate_each_phase_and_platform_ack_gates_commit() {
    let (mut state, plan, mut claim) = prepared();
    assert_eq!(
        advance_at(
            &state,
            &state.revision,
            "config-2",
            &claim,
            PhaseEvent::OldStopped {
                exited: true,
                owners_released: true
            }
        )
        .unwrap_err(),
        ApplyError::Invalid("illegal Apply phase transition")
    );
    state = step(&state, &claim, PhaseEvent::RequestStop);
    assert!(advance_at(
        &state,
        &state.revision,
        "config-2",
        &claim,
        PhaseEvent::OldStopped {
            exited: true,
            owners_released: false
        }
    )
    .is_err());
    state = step(
        &state,
        &claim,
        PhaseEvent::OldStopped {
            exited: true,
            owners_released: true,
        },
    );
    state = step(
        &state,
        &claim,
        PhaseEvent::RequestStart {
            run_id: "run-2",
            start_generation: "generation-2",
        },
    );
    claim = ApplyClaim::from(state.transaction.as_ref().unwrap());
    let mut core = core_receipt(&state, &claim);
    core.run_id = "wrong-run".into();
    assert_eq!(
        advance_at(
            &state,
            &state.revision,
            "config-2",
            &claim,
            PhaseEvent::CoreReady(&core)
        )
        .unwrap_err(),
        ApplyError::Invalid("core receipt binding mismatch")
    );
    core.run_id = "run-2".into();
    state = step(&state, &claim, PhaseEvent::CoreReady(&core));
    let mut platform = platform_receipt(&plan, &core);
    platform.result = ProtectionResult::Partial;
    assert!(advance_at(
        &state,
        &state.revision,
        "config-2",
        &claim,
        PhaseEvent::Commit {
            core: &core,
            platform: &platform,
            plan: &plan
        }
    )
    .is_err());
    platform.result = ProtectionResult::Unknown;
    assert!(advance_at(
        &state,
        &state.revision,
        "config-2",
        &claim,
        PhaseEvent::Commit {
            core: &core,
            platform: &platform,
            plan: &plan
        }
    )
    .is_err());
    platform.result = ProtectionResult::Complete;
    platform.reject_cidrs.clear();
    assert!(advance_at(
        &state,
        &state.revision,
        "config-2",
        &claim,
        PhaseEvent::Commit {
            core: &core,
            platform: &platform,
            plan: &plan
        }
    )
    .is_err());
    platform.reject_cidrs = plan.reject_cidrs.clone();
    state = step(
        &state,
        &claim,
        PhaseEvent::Commit {
            core: &core,
            platform: &platform,
            plan: &plan,
        },
    );
    assert_eq!(
        state.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::Committed
    );
    assert_eq!(state.active_plan.as_ref().unwrap().plan_id, "plan-2");
    assert_eq!(
        state.active_plan.as_ref().unwrap().input_state_revision,
        "1"
    );
}

#[test]
fn later_stop_and_config_change_take_action_rights_away() {
    let (state, _, claim) = prepared();
    let stopped = persist(&state, record_stop_intent(&state, &state.revision).unwrap());
    assert_eq!(stopped.intent.desired_run, MeshDesiredRun::Stopped);
    assert_eq!(
        stopped.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::Interrupted
    );
    assert_eq!(
        advance_at(
            &stopped,
            &stopped.revision,
            "config-2",
            &claim,
            PhaseEvent::RequestStop
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    assert_eq!(
        advance_at(
            &state,
            &state.revision,
            "config-3",
            &claim,
            PhaseEvent::RequestStop
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    assert_eq!(
        advance_at(&state, "1", "config-2", &claim, PhaseEvent::RequestStop).unwrap_err(),
        ApplyError::Conflict
    );
    assert_eq!(
        super::advance(
            &state,
            &state.revision,
            "config-2",
            "boot-a",
            "generation-2",
            &claim,
            &fixture().1,
            PhaseEvent::RequestStop,
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    let mut stale_claim = claim;
    stale_claim.lifecycle_generation = "generation-2".into();
    assert_eq!(
        advance_at(
            &state,
            &state.revision,
            "config-2",
            &stale_claim,
            PhaseEvent::RequestStop
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
}

#[test]
fn start_generation_handoff_is_persisted_before_any_new_core_receipt() {
    let (state, _, stop_claim) = prepared();
    let stopped = step(&state, &stop_claim, PhaseEvent::NoOldCore);
    assert_eq!(
        advance_at(
            &stopped,
            &stopped.revision,
            "config-2",
            &stop_claim,
            PhaseEvent::RequestStart {
                run_id: "run-2",
                start_generation: "generation-1"
            }
        )
        .unwrap_err(),
        ApplyError::Invalid("illegal Apply phase transition")
    );
    let start_requested = step(
        &stopped,
        &stop_claim,
        PhaseEvent::RequestStart {
            run_id: "run-2",
            start_generation: "generation-2",
        },
    );
    let start_claim = ApplyClaim::from(start_requested.transaction.as_ref().unwrap());
    assert_eq!(start_claim.lifecycle_generation, "generation-2");
    let core = core_receipt(&start_requested, &start_claim);
    assert_eq!(
        advance_at(
            &start_requested,
            &start_requested.revision,
            "config-2",
            &stop_claim,
            PhaseEvent::CoreReady(&core)
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    assert_eq!(
        super::advance(
            &start_requested,
            &start_requested.revision,
            "config-2",
            "boot-a",
            "generation-1",
            &start_claim,
            &fixture().1,
            PhaseEvent::CoreReady(&core),
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    let ready = step(&start_requested, &start_claim, PhaseEvent::CoreReady(&core));
    assert_eq!(
        ready.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::CoreReady
    );
}

#[test]
fn stop_wins_if_a_phase_was_computed_but_not_persisted() {
    let (state, _, claim) = prepared();
    let unpersisted = advance_at(
        &state,
        &state.revision,
        "config-2",
        &claim,
        PhaseEvent::RequestStop,
    )
    .unwrap();
    assert_eq!(
        state.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::Prepared
    );
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        RecoveryDecision::KeepOldPending
    );
    let stopped = persist(&state, record_stop_intent(&state, &state.revision).unwrap());
    assert!(revise_semantic(&stopped, &state.revision, unpersisted).is_err());
    assert_eq!(stopped.intent.desired_run, MeshDesiredRun::Stopped);
    assert_eq!(
        stopped.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::Interrupted
    );
}

#[test]
fn failure_after_old_stopped_keeps_old_plan_history_without_rollback() {
    let (state, _, claim) = prepared();
    let state = step(&state, &claim, PhaseEvent::NoOldCore);
    let failed = step(
        &state,
        &claim,
        PhaseEvent::Fail(JournalErrorCode::StartFailed),
    );
    assert_eq!(
        failed.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::Failed
    );
    assert_eq!(failed.active_plan.as_ref().unwrap().plan_id, "old-plan");
    assert_eq!(
        recovery_decision(&failed, "boot-b", RecoveredRun::None),
        RecoveryDecision::NoReplay
    );
}

#[test]
fn failure_classification_uses_supervisor_facts_not_phase_or_active_plan() {
    let (state, _, claim) = prepared();
    let tx = state.transaction.as_ref().unwrap();
    assert_eq!(
        failure_disposition(
            tx,
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        FailureDisposition::KeepOldRunning
    );
    assert_eq!(
        failure_disposition(tx, RecoveredRun::Unknown),
        FailureDisposition::CleanupUnknown
    );
    let failed = step(
        &state,
        &claim,
        PhaseEvent::Fail(JournalErrorCode::StopTimeout),
    );
    assert_eq!(
        recovery_decision(
            &failed,
            "boot-b",
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        RecoveryDecision::PreserveOldAfterFailure
    );
    assert_eq!(
        failure_disposition(
            failed.transaction.as_ref().unwrap(),
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        FailureDisposition::KeepOldRunning
    );
    let stopped = step(&state, &claim, PhaseEvent::NoOldCore);
    assert_eq!(
        failure_disposition(stopped.transaction.as_ref().unwrap(), RecoveredRun::None),
        FailureDisposition::StoppedUnprotected
    );
    assert_eq!(
        failure_disposition(
            stopped.transaction.as_ref().unwrap(),
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        FailureDisposition::CleanupUnknown
    );
    let started = step(
        &stopped,
        &claim,
        PhaseEvent::RequestStart {
            run_id: "run-2",
            start_generation: "generation-2",
        },
    );
    assert_eq!(
        failure_disposition(
            started.transaction.as_ref().unwrap(),
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "run-2",
                ready: false
            }
        ),
        FailureDisposition::StopCandidate
    );
    assert_eq!(
        failure_disposition(
            started.transaction.as_ref().unwrap(),
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "wrong",
                ready: false
            }
        ),
        FailureDisposition::CleanupUnknown
    );
}

#[test]
fn crash_recovery_requires_fresh_supervisor_proof_and_never_replays_old_ack() {
    let (state, _, mut claim) = prepared();
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        RecoveryDecision::KeepOldPending
    );
    assert_eq!(
        recovery_decision(&state, "boot-b", RecoveredRun::Unknown),
        RecoveryDecision::BlockUnknown
    );
    let state = step(&state, &claim, PhaseEvent::NoOldCore);
    let state = step(
        &state,
        &claim,
        PhaseEvent::RequestStart {
            run_id: "run-2",
            start_generation: "generation-2",
        },
    );
    claim = ApplyClaim::from(state.transaction.as_ref().unwrap());
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "run-2",
                ready: true
            }
        ),
        RecoveryDecision::StopCandidate
    );
    let core = core_receipt(&state, &claim);
    let state = step(&state, &claim, PhaseEvent::CoreReady(&core));
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "run-2",
                ready: true
            }
        ),
        RecoveryDecision::NeedsFreshClaimAndAck
    );
    let reclaimed = persist(
        &state,
        reclaim_ready_candidate(
            &state,
            &state.revision,
            "config-2",
            &claim,
            &fixture().1,
            "boot-b",
            "generation-b",
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "run-2",
                ready: true,
            },
        )
        .unwrap(),
    );
    assert_eq!(
        reclaimed.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::StartRequested
    );
    assert_eq!(
        advance_at(
            &reclaimed,
            &reclaimed.revision,
            "config-2",
            &claim,
            PhaseEvent::CoreReady(&core)
        )
        .unwrap_err(),
        ApplyError::Superseded
    );
    let new_claim = ApplyClaim::from(reclaimed.transaction.as_ref().unwrap());
    let fresh_core = core_receipt(&reclaimed, &new_claim);
    let reverified = persist(
        &reclaimed,
        super::advance(
            &reclaimed,
            &reclaimed.revision,
            "config-2",
            "boot-b",
            "generation-b",
            &new_claim,
            &fixture().1,
            PhaseEvent::CoreReady(&fresh_core),
        )
        .unwrap(),
    );
    assert_eq!(
        reverified.transaction.as_ref().unwrap().phase,
        MeshTransactionPhase::CoreReady
    );
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "wrong",
                ready: true
            }
        ),
        RecoveryDecision::BlockUnknown
    );
    let stopped = persist(&state, record_stop_intent(&state, &state.revision).unwrap());
    assert_eq!(
        recovery_decision(&stopped, "boot-b", RecoveredRun::Unknown),
        RecoveryDecision::StopIntentCleanupUnknown
    );
    assert_eq!(
        recovery_decision(&stopped, "boot-b", RecoveredRun::None),
        RecoveryDecision::StoppedIntentNoRun
    );
    assert_eq!(
        recovery_decision(
            &stopped,
            "boot-b",
            RecoveredRun::Candidate {
                plan_id: "plan-2",
                run_id: "run-2",
                ready: true
            }
        ),
        RecoveryDecision::StopOwnedRun
    );
}

#[test]
fn unknown_owner_blocks_autostart_even_without_pending_journal() {
    let (mut state, _) = fixture();
    state.intent.desired_run = MeshDesiredRun::Running;
    assert_eq!(
        recovery_decision(&state, "boot-b", RecoveredRun::Unknown),
        RecoveryDecision::BlockUnknown
    );
    assert_eq!(
        recovery_decision(
            &state,
            "boot-b",
            RecoveredRun::Old {
                plan_id: "old-plan"
            }
        ),
        RecoveryDecision::BlockUnknown
    );
    assert_eq!(
        recovery_decision(&state, "boot-b", RecoveredRun::None),
        RecoveryDecision::NoReplay
    );
}

//! S4's value-level Apply journal. Production callers must use
//! `ConfigManager::apply_mesh_step_if_current` under a live generation guard
//! before the next external action. A phase or receipt alone never proves OS
//! protection.

pub(crate) mod artifact;
pub(crate) mod closure;
pub(crate) mod materialize;
pub(crate) mod owner_scope;
pub(crate) mod preflight;

use crate::runtime::config::ApplyInputSnapshot;
use polaris_config_engine::builder::managed_mesh_plan::{ManagedMeshRoutePlan, ManagedPlanTarget};
use polaris_config_engine::user_config::mesh_route_state::{
    MeshActivePlan, MeshBindingState, MeshDesiredRun, MeshOwnerRef, MeshRouteState,
    MeshTransaction, MeshTransactionPhase,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApplyError {
    Conflict,
    Superseded,
    Invalid(&'static str),
}

/// The journal's immutable claim. `expected_state_revision` is intentionally
/// absent: journal writes themselves advance that CAS revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyClaim {
    pub plan_id: String,
    pub plan_digest: String,
    pub input_config_version: String,
    pub intent_revision: String,
    pub boot_id: String,
    pub lifecycle_generation: String,
}

impl From<&MeshTransaction> for ApplyClaim {
    fn from(tx: &MeshTransaction) -> Self {
        Self {
            plan_id: tx.plan_id.clone(),
            plan_digest: tx.plan_digest.clone(),
            input_config_version: tx.input_config_version.clone(),
            intent_revision: tx.intent_revision.clone(),
            boot_id: tx.boot_id.clone(),
            lifecycle_generation: tx.lifecycle_generation.clone(),
        }
    }
}

/// Receipt producer must independently prove the core instance; this type
/// only prevents a valid receipt for another plan/run/boot being reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoreReceipt {
    pub claim: ApplyClaim,
    pub run_id: String,
    pub identity_bindings: Vec<MeshOwnerRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtectionResult {
    Complete,
    Partial,
    Unknown,
}

/// Produced by the platform adapter after checking its actual TUN/proxy scope.
/// `scope` and `evidence` are mandatory, but their truth is adapter-owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformReceipt {
    pub core: CoreReceipt,
    pub scope: String,
    pub protected_cidrs: Vec<String>,
    pub reject_cidrs: Vec<String>,
    pub overrides_digest: String,
    pub result: ProtectionResult,
    pub evidence: String,
}

/// Stable, non-sensitive journal errors. Raw core/OS error text can contain
/// credentials or paths and must stay outside the persisted mesh ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalErrorCode {
    Preflight,
    StopTimeout,
    StopUnknown,
    IdentityEffect,
    ArtifactChanged,
    StartFailed,
    CoreReadyTimeout,
    PlatformAckFailed,
    CommitUncertain,
    Superseded,
}

impl JournalErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::StopTimeout => "stopTimeout",
            Self::StopUnknown => "stopUnknown",
            Self::IdentityEffect => "identityEffect",
            Self::ArtifactChanged => "artifactChanged",
            Self::StartFailed => "startFailed",
            Self::CoreReadyTimeout => "coreReadyTimeout",
            Self::PlatformAckFailed => "platformAckFailed",
            Self::CommitUncertain => "commitUncertain",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PhaseEvent<'a> {
    RequestStop,
    /// The supervisor proved that no old primary or temporary owner existed.
    NoOldCore,
    /// Both the process exit and every owned TS state release were confirmed.
    OldStopped {
        exited: bool,
        owners_released: bool,
    },
    /// The host has checked artifact hashes and authorized epoch effects, then
    /// records this ID before calling the actual start operation.
    RequestStart {
        run_id: &'a str,
        /// The lifecycle gate must reserve this next generation before spawn.
        start_generation: &'a str,
    },
    CoreReady(&'a CoreReceipt),
    Commit {
        core: &'a CoreReceipt,
        platform: &'a PlatformReceipt,
        plan: &'a ManagedMeshRoutePlan,
    },
    Fail(JournalErrorCode),
    Interrupt(JournalErrorCode),
}

/// The only state mutations admitted through ConfigManager's guarded Apply
/// CAS. No caller-supplied closure may rewrite the managed ledger directly.
pub(crate) enum ApplyStep<'a> {
    Prepare {
        /// An opaque, strict raw document snapshot. The ConfigManager CAS
        /// rechecks its strong digest under the same write lock; callers
        /// cannot substitute a configVersion-only claim.
        snapshot: &'a ApplyInputSnapshot,
        plan: &'a ManagedMeshRoutePlan,
        boot_id: &'a str,
        manifest_ref: &'a str,
    },
    Advance {
        plan: &'a ManagedMeshRoutePlan,
        claim: &'a ApplyClaim,
        event: PhaseEvent<'a>,
    },
    /// The gate must already have claimed `new_generation` as Start, and the
    /// journal must still carry `old_generation`. Persist before spawn.
    RequestStartReserved {
        plan: &'a ManagedMeshRoutePlan,
        claim: &'a ApplyClaim,
        run_id: &'a str,
        old_generation: u64,
        new_generation: u64,
    },
    /// The gate has already claimed the next Stop generation. Persist that
    /// handoff before touching the old process. A failed CAS leaves its
    /// process and supervision status unknown to this journal. A `None` from
    /// `with_current_generation` has the same conservative outcome.
    RequestStopReserved {
        plan: &'a ManagedMeshRoutePlan,
        claim: &'a ApplyClaim,
        old_generation: u64,
        stop_generation: u64,
    },
    StopIntent,
}

pub(super) fn plan_digest(plan: &ManagedMeshRoutePlan) -> Result<String, ApplyError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| ApplyError::Invalid("plan serialization"))?;
    Ok(polaris_updater::verify::sha256_hex(&bytes))
}

fn next_intent_revision(current: &str) -> Result<String, ApplyError> {
    current
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_add(1))
        .map(|n| n.to_string())
        .ok_or(ApplyError::Invalid("intent revision exhausted"))
}

fn same_bindings(a: &[MeshOwnerRef], b: &[MeshOwnerRef]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b && a.windows(2).all(|w| w[0] != w[1])
}

fn valid_id(value: &str) -> bool {
    !value.trim().is_empty() && value.trim() == value
}

fn safe_plan_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// `identityBindings` also contains historical/rejected references. Only a
/// route actually emitted to an endpoint may require a current Bound epoch.
fn active_owners_bound(
    state: &MeshRouteState,
    plan: &ManagedMeshRoutePlan,
) -> Result<(), ApplyError> {
    let is_bound = |owner: &MeshOwnerRef, endpoint_tag: &str| {
        valid_id(endpoint_tag)
            && plan.identity_bindings.contains(owner)
            && state.identities.iter().any(|identity| {
                identity.server_id == owner.server_id
                    && identity.identity_epoch == owner.identity_epoch
                    && identity.binding_state == MeshBindingState::Bound
            })
    };
    if !plan
        .owner_routes
        .iter()
        .all(|route| is_bound(&route.owner_ref, &route.endpoint_tag))
        || !plan.overrides.iter().all(|rule| match &rule.target {
            ManagedPlanTarget::Owner {
                owner_ref,
                endpoint_tag,
            } => is_bound(owner_ref, endpoint_tag),
            ManagedPlanTarget::Reject | ManagedPlanTarget::Unmanaged => true,
        })
    {
        return Err(ApplyError::Invalid("active owner binding is not bound"));
    }
    Ok(())
}

fn check_plan(
    state: &MeshRouteState,
    tx: &MeshTransaction,
    plan: &ManagedMeshRoutePlan,
) -> Result<(), ApplyError> {
    if plan.plan_id != tx.plan_id
        || plan.config_version != tx.input_config_version
        || plan.input_state_revision != tx.input_state_revision
        || !same_bindings(&plan.identity_bindings, &tx.identity_bindings)
        || plan_digest(plan)? != tx.plan_digest
    {
        return Err(ApplyError::Superseded);
    }
    active_owners_bound(state, plan).map_err(|_| ApplyError::Superseded)
}

/// Pure candidate for the first durable CAS. The caller must have completed
/// compile, staging, manifest verification and platform preflight before
/// persisting this `Prepared` transaction. It cannot act on the old core yet.
pub(crate) fn record_prepared(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    plan: &ManagedMeshRoutePlan,
    boot_id: &str,
    lifecycle_generation: &str,
    manifest_ref: &str,
) -> Result<MeshRouteState, ApplyError> {
    if state.revision != expected_state_revision
        || plan.input_state_revision != expected_state_revision
        || plan.config_version != current_config_version
    {
        return Err(ApplyError::Conflict);
    }
    if plan.schema_version != 1
        || !safe_plan_id(&plan.plan_id)
        || !valid_id(boot_id)
        || !valid_id(lifecycle_generation)
        || manifest_ref != format!("mesh-routes/plans/{}/manifest.json", plan.plan_id)
    {
        return Err(ApplyError::Invalid("incomplete Apply claim"));
    }
    if state
        .transaction
        .as_ref()
        .is_some_and(|tx| !is_terminal(tx.phase))
    {
        return Err(ApplyError::Conflict);
    }
    if state
        .active_plan
        .as_ref()
        .is_some_and(|active| active.plan_id == plan.plan_id)
        || state
            .transaction
            .as_ref()
            .is_some_and(|tx| tx.plan_id == plan.plan_id)
    {
        return Err(ApplyError::Invalid("reused planId"));
    }
    if !same_bindings(&plan.identity_bindings, &plan.identity_bindings) {
        return Err(ApplyError::Invalid("duplicate identity bindings"));
    }
    active_owners_bound(state, plan)?;
    let digest = plan_digest(plan)?;
    let mut next = state.clone();
    let intent_revision = next_intent_revision(&state.intent.revision)?;
    next.intent.revision = intent_revision.clone();
    next.intent.desired_run = MeshDesiredRun::Running;
    next.transaction = Some(MeshTransaction {
        plan_id: plan.plan_id.clone(),
        plan_digest: digest,
        input_config_version: current_config_version.into(),
        input_state_revision: expected_state_revision.into(),
        identity_bindings: plan.identity_bindings.clone(),
        intent_revision,
        boot_id: boot_id.into(),
        lifecycle_generation: lifecycle_generation.into(),
        phase: MeshTransactionPhase::Prepared,
        previous_plan_id: state
            .active_plan
            .as_ref()
            .map(|active| active.plan_id.clone()),
        candidate_run_id: None,
        artifact_manifest_ref: Some(manifest_ref.into()),
        last_error: None,
    });
    Ok(next)
}

fn is_terminal(phase: MeshTransactionPhase) -> bool {
    matches!(
        phase,
        MeshTransactionPhase::Committed
            | MeshTransactionPhase::Failed
            | MeshTransactionPhase::Interrupted
    )
}

fn check_stored_claim(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
) -> Result<(), ApplyError> {
    if state.revision != expected_state_revision {
        return Err(ApplyError::Conflict);
    }
    let tx = state.transaction.as_ref().ok_or(ApplyError::Superseded)?;
    if ApplyClaim::from(tx) != *claim
        || state.intent.revision != tx.intent_revision
        || state.intent.desired_run != MeshDesiredRun::Running
        || current_config_version != tx.input_config_version
    {
        return Err(ApplyError::Superseded);
    }
    check_plan(state, tx, plan)
}

fn check_live_claim(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    current_boot_id: &str,
    current_lifecycle_generation: &str,
    claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
) -> Result<(), ApplyError> {
    check_stored_claim(
        state,
        expected_state_revision,
        current_config_version,
        claim,
        plan,
    )?;
    if claim.boot_id != current_boot_id
        || claim.lifecycle_generation != current_lifecycle_generation
    {
        return Err(ApplyError::Superseded);
    }
    Ok(())
}

fn check_core(tx: &MeshTransaction, receipt: &CoreReceipt) -> Result<(), ApplyError> {
    if receipt.claim != ApplyClaim::from(tx)
        || tx.candidate_run_id.as_deref() != Some(receipt.run_id.as_str())
        || !same_bindings(&receipt.identity_bindings, &tx.identity_bindings)
    {
        return Err(ApplyError::Invalid("core receipt binding mismatch"));
    }
    Ok(())
}

/// One journal step. Its returned state must be written by a short CAS before
/// any next await/action; a CAS miss means the caller has lost ownership.
pub(crate) fn advance(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    current_boot_id: &str,
    current_lifecycle_generation: &str,
    claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
    event: PhaseEvent<'_>,
) -> Result<MeshRouteState, ApplyError> {
    check_live_claim(
        state,
        expected_state_revision,
        current_config_version,
        current_boot_id,
        current_lifecycle_generation,
        claim,
        plan,
    )?;
    let mut next = state.clone();
    let tx = next.transaction.as_mut().ok_or(ApplyError::Superseded)?;
    match (tx.phase, event) {
        (MeshTransactionPhase::Prepared, PhaseEvent::RequestStop) => {
            tx.phase = MeshTransactionPhase::StopRequested;
        }
        (MeshTransactionPhase::Prepared, PhaseEvent::NoOldCore)
        | (
            MeshTransactionPhase::StopRequested,
            PhaseEvent::OldStopped {
                exited: true,
                owners_released: true,
            },
        ) => {
            tx.phase = MeshTransactionPhase::OldStopped;
        }
        (
            MeshTransactionPhase::OldStopped,
            PhaseEvent::RequestStart {
                run_id,
                start_generation,
            },
        ) if valid_id(run_id)
            && valid_id(start_generation)
            && start_generation != tx.lifecycle_generation =>
        {
            tx.candidate_run_id = Some(run_id.into());
            tx.lifecycle_generation = start_generation.into();
            tx.phase = MeshTransactionPhase::StartRequested;
        }
        (MeshTransactionPhase::StartRequested, PhaseEvent::CoreReady(receipt)) => {
            check_core(tx, receipt)?;
            tx.phase = MeshTransactionPhase::CoreReady;
        }
        (
            MeshTransactionPhase::CoreReady,
            PhaseEvent::Commit {
                core,
                platform,
                plan,
            },
        ) => {
            check_core(tx, core)?;
            if platform.core != *core
                || platform.result != ProtectionResult::Complete
                || !valid_id(&platform.scope)
                || !valid_id(&platform.evidence)
                || plan.plan_id != tx.plan_id
                || plan.config_version != tx.input_config_version
                || plan.input_state_revision != tx.input_state_revision
                || !same_bindings(&plan.identity_bindings, &tx.identity_bindings)
                || plan_digest(plan)? != tx.plan_digest
                || platform.protected_cidrs != plan.protected_cidrs
                || platform.reject_cidrs != plan.reject_cidrs
                || platform.overrides_digest
                    != polaris_updater::verify::sha256_hex(
                        &serde_json::to_vec(&plan.overrides)
                            .map_err(|_| ApplyError::Invalid("override serialization"))?,
                    )
            {
                return Err(ApplyError::Invalid("platform receipt binding mismatch"));
            }
            next.active_plan = Some(MeshActivePlan {
                plan_id: tx.plan_id.clone(),
                digest: tx.plan_digest.clone(),
                config_version: tx.input_config_version.clone(),
                input_state_revision: tx.input_state_revision.clone(),
            });
            tx.phase = MeshTransactionPhase::Committed;
        }
        (phase, PhaseEvent::Fail(reason)) if !is_terminal(phase) => {
            tx.phase = MeshTransactionPhase::Failed;
            tx.last_error = Some(reason.as_str().into());
        }
        (phase, PhaseEvent::Interrupt(reason)) if !is_terminal(phase) => {
            tx.phase = MeshTransactionPhase::Interrupted;
            tx.last_error = Some(reason.as_str().into());
        }
        _ => return Err(ApplyError::Invalid("illegal Apply phase transition")),
    }
    Ok(next)
}

/// Reserve the old→Stop lifecycle handoff after the gate claim and before any
/// actual stop. This checks journal ownership, not the old process identity or
/// its monitor attachment. `StopRequested` is only intent; process exit and
/// both owner releases still require independent evidence for `OldStopped`.
pub(crate) fn request_stop_reserved(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    current_boot_id: &str,
    live_stop_generation: u64,
    claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
    old_generation: u64,
    stop_generation: u64,
) -> Result<MeshRouteState, ApplyError> {
    if claim.lifecycle_generation != old_generation.to_string()
        || live_stop_generation != stop_generation
        || old_generation.checked_add(1) != Some(stop_generation)
    {
        return Err(ApplyError::Superseded);
    }
    check_stored_claim(
        state,
        expected_state_revision,
        current_config_version,
        claim,
        plan,
    )?;
    if claim.boot_id != current_boot_id {
        return Err(ApplyError::Superseded);
    }
    let tx = state.transaction.as_ref().ok_or(ApplyError::Superseded)?;
    if tx.phase != MeshTransactionPhase::Prepared {
        return Err(ApplyError::Invalid("stop reservation requires prepared"));
    }
    let mut next = state.clone();
    let tx = next.transaction.as_mut().ok_or(ApplyError::Superseded)?;
    tx.lifecycle_generation = stop_generation.to_string();
    tx.phase = MeshTransactionPhase::StopRequested;
    Ok(next)
}

/// The old Stop→new Start generation transition. `advance` normally requires
/// the old claim to be live and must not be called with a fabricated old live
/// generation after the gate has already reserved a new Start generation.
pub(crate) fn request_start_reserved(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    current_boot_id: &str,
    live_new_generation: u64,
    claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
    run_id: &str,
    old_generation: u64,
    new_generation: u64,
) -> Result<MeshRouteState, ApplyError> {
    if claim.lifecycle_generation != old_generation.to_string()
        || live_new_generation != new_generation
        || old_generation.checked_add(1) != Some(new_generation)
        || !valid_id(run_id)
    {
        return Err(ApplyError::Superseded);
    }
    check_stored_claim(
        state,
        expected_state_revision,
        current_config_version,
        claim,
        plan,
    )?;
    if claim.boot_id != current_boot_id {
        return Err(ApplyError::Superseded);
    }
    let tx = state.transaction.as_ref().ok_or(ApplyError::Superseded)?;
    if tx.phase != MeshTransactionPhase::OldStopped {
        return Err(ApplyError::Invalid("start reservation requires oldStopped"));
    }
    let mut next = state.clone();
    let tx = next.transaction.as_mut().ok_or(ApplyError::Superseded)?;
    tx.candidate_run_id = Some(run_id.into());
    tx.lifecycle_generation = new_generation.to_string();
    tx.phase = MeshTransactionPhase::StartRequested;
    Ok(next)
}

/// Stop claims a newer durable intent first. The caller may perform emergency
/// stop after a write error, but must report that the stopped intent was not
/// saved. The phase never claims that the process has actually exited.
pub(crate) fn record_stop_intent(
    state: &MeshRouteState,
    expected_state_revision: &str,
) -> Result<MeshRouteState, ApplyError> {
    if state.revision != expected_state_revision {
        return Err(ApplyError::Conflict);
    }
    let mut next = state.clone();
    next.intent.revision = next_intent_revision(&state.intent.revision)?;
    next.intent.desired_run = MeshDesiredRun::Stopped;
    if let Some(tx) = next.transaction.as_mut() {
        if !is_terminal(tx.phase) {
            tx.phase = MeshTransactionPhase::Interrupted;
            tx.last_error = Some("superseded by Stop".into());
        }
    }
    Ok(next)
}

/// Facts from a supervisor, not inferred from the journal or activePlan.
/// `Old` means the supervisor proved the same previous process instance and
/// its plan artifact, not merely a process with a matching plan name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveredRun<'a> {
    None,
    Old {
        plan_id: &'a str,
    },
    Candidate {
        plan_id: &'a str,
        run_id: &'a str,
        ready: bool,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryDecision {
    /// No old Apply is replayed; ordinary autostart still needs its own proof.
    NoReplay,
    /// Desired Stop was saved and the supervisor proves nothing is running.
    StoppedIntentNoRun,
    /// Desired Stop was saved; stop only this supervisor-proven owned run.
    StopOwnedRun,
    /// Desired Stop was saved, but process ownership is unknown: block starts
    /// and surface cleanupUnknown instead of claiming a completed Stop.
    StopIntentCleanupUnknown,
    KeepOldPending,
    PreserveOldAfterFailure,
    StopCandidate,
    /// Only after a new current-boot claim and new core/platform evidence.
    NeedsFreshClaimAndAck,
    InterruptedStopped,
    BlockUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureDisposition {
    /// Supervisor proves the exact previous plan is still running.
    KeepOldRunning,
    /// Nothing owned is running; the old plan is historical, not protection.
    StoppedUnprotected,
    /// Stop this exact candidate before reporting the failure as settled.
    StopCandidate,
    /// Do not start or kill an unproven process; surface cleanupUnknown.
    CleanupUnknown,
}

/// A failed journal phase is not a process status. Classify only from current
/// supervisor evidence, including after a stop timeout or a crash.
pub(super) fn failure_disposition(
    tx: &MeshTransaction,
    run: RecoveredRun<'_>,
) -> FailureDisposition {
    match run {
        RecoveredRun::None => FailureDisposition::StoppedUnprotected,
        RecoveredRun::Old { plan_id }
            if tx.previous_plan_id.as_deref() == Some(plan_id)
                && tx.candidate_run_id.is_none()
                && matches!(
                    tx.phase,
                    MeshTransactionPhase::Prepared
                        | MeshTransactionPhase::StopRequested
                        | MeshTransactionPhase::Failed
                        | MeshTransactionPhase::Interrupted
                ) =>
        {
            FailureDisposition::KeepOldRunning
        }
        RecoveredRun::Candidate {
            plan_id, run_id, ..
        } if tx.plan_id == plan_id && tx.candidate_run_id.as_deref() == Some(run_id) => {
            FailureDisposition::StopCandidate
        }
        RecoveredRun::Old { .. } | RecoveredRun::Candidate { .. } | RecoveredRun::Unknown => {
            FailureDisposition::CleanupUnknown
        }
    }
}

/// Called only after strict ledger load. Old-boot receipts are never reused;
/// a ready candidate may only seek a new platform ACK after fresh supervisor
/// proof and a new ownership claim in the current boot.
pub(super) fn recovery_decision(
    state: &MeshRouteState,
    current_boot_id: &str,
    run: RecoveredRun<'_>,
) -> RecoveryDecision {
    if state.intent.desired_run == MeshDesiredRun::Stopped {
        return match run {
            RecoveredRun::None => RecoveryDecision::StoppedIntentNoRun,
            RecoveredRun::Unknown => RecoveryDecision::StopIntentCleanupUnknown,
            RecoveredRun::Old { .. } | RecoveredRun::Candidate { .. } => {
                RecoveryDecision::StopOwnedRun
            }
        };
    }
    if run == RecoveredRun::Unknown {
        return RecoveryDecision::BlockUnknown;
    }
    let Some(tx) = &state.transaction else {
        return if run == RecoveredRun::None {
            RecoveryDecision::NoReplay
        } else {
            RecoveryDecision::BlockUnknown
        };
    };
    if matches!(
        tx.phase,
        MeshTransactionPhase::Failed | MeshTransactionPhase::Interrupted
    ) && state.intent.revision == tx.intent_revision
        && tx.candidate_run_id.is_none()
        && matches!(run, RecoveredRun::Old { plan_id } if tx.previous_plan_id.as_deref() == Some(plan_id))
    {
        return RecoveryDecision::PreserveOldAfterFailure;
    }
    if is_terminal(tx.phase) || state.intent.revision != tx.intent_revision {
        return if run == RecoveredRun::None {
            RecoveryDecision::NoReplay
        } else {
            RecoveryDecision::BlockUnknown
        };
    }
    match run {
        RecoveredRun::Unknown => RecoveryDecision::BlockUnknown,
        RecoveredRun::Old { plan_id }
            if tx.previous_plan_id.as_deref() == Some(plan_id)
                && matches!(
                    tx.phase,
                    MeshTransactionPhase::Prepared | MeshTransactionPhase::StopRequested
                ) =>
        {
            RecoveryDecision::KeepOldPending
        }
        RecoveredRun::Candidate {
            plan_id,
            run_id,
            ready: true,
        } if tx.boot_id != current_boot_id
            && tx.plan_id == plan_id
            && tx.candidate_run_id.as_deref() == Some(run_id)
            && tx.phase == MeshTransactionPhase::CoreReady =>
        {
            RecoveryDecision::NeedsFreshClaimAndAck
        }
        RecoveredRun::Candidate {
            plan_id, run_id, ..
        } if tx.plan_id == plan_id && tx.candidate_run_id.as_deref() == Some(run_id) => {
            RecoveryDecision::StopCandidate
        }
        RecoveredRun::None => RecoveryDecision::InterruptedStopped,
        _ => RecoveryDecision::BlockUnknown,
    }
}

/// A desktop supervisor may prove that the old candidate survived an app
/// crash. Reclaim it in the new boot through CAS, demoting `coreReady` so both
/// core evidence and the platform ACK must be collected again. No start or
/// commit is authorized by this function itself.
pub(super) fn reclaim_ready_candidate(
    state: &MeshRouteState,
    expected_state_revision: &str,
    current_config_version: &str,
    old_claim: &ApplyClaim,
    plan: &ManagedMeshRoutePlan,
    new_boot_id: &str,
    new_lifecycle_generation: &str,
    supervisor_run: RecoveredRun<'_>,
) -> Result<MeshRouteState, ApplyError> {
    check_stored_claim(
        state,
        expected_state_revision,
        current_config_version,
        old_claim,
        plan,
    )?;
    if !valid_id(new_boot_id)
        || !valid_id(new_lifecycle_generation)
        || new_boot_id == old_claim.boot_id
        || recovery_decision(state, new_boot_id, supervisor_run)
            != RecoveryDecision::NeedsFreshClaimAndAck
    {
        return Err(ApplyError::Invalid("candidate recovery proof mismatch"));
    }
    let mut next = state.clone();
    let tx = next.transaction.as_mut().ok_or(ApplyError::Superseded)?;
    tx.boot_id = new_boot_id.into();
    tx.lifecycle_generation = new_lifecycle_generation.into();
    tx.phase = MeshTransactionPhase::StartRequested;
    Ok(next)
}

#[cfg(test)]
mod tests;

//! Immutable candidate facts for one managed direct Child. The current exact
//! start proof lacks frozen runtime bindings, so no production managed birth
//! permit can be issued yet. A fact snapshot is never a spawn or StopLease.

use super::preflight::{ExactStartReadiness, VerifiedExactStart};
use crate::runtime::proxy::process_supervision::{DirectCoreRun, DirectRunOrigin, RunIdentity};
use polaris_config_engine::user_config::mesh_route_state::{
    MeshDesiredRun, MeshRouteState, MeshTransactionPhase,
};
use tokio::process::Child;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedBirthError {
    StartRequestMismatch,
    UnsupportedRuntimeFacts,
}

/// Minted before StartRequested and consumed before spawn. There is no
/// conversion from an existing DirectCoreRun or a caller-supplied run ref.
/// A future coordinator must guard the spawn and slot assignment together;
/// this type alone cannot prevent trusted proxy modules from relabeling a run.
#[must_use]
pub(crate) struct ManagedDirectIntent {
    identity: RunIdentity,
}

impl ManagedDirectIntent {
    pub(in crate::runtime::proxy) fn new() -> Self {
        Self {
            identity: RunIdentity::new(),
        }
    }

    pub(in crate::runtime::proxy) fn run_ref(&self) -> &str {
        self.identity.persisted_ref()
    }
}

/// Plan and artifact identity frozen before a physical direct spawn. All
/// fields are private and must match the durable StartRequested transaction.
/// They do not prove that the pathname remains unchanged or that a core/OS
/// owner has acknowledged readiness or release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedRunFacts {
    plan_id: String,
    plan_digest: String,
    artifact_manifest_ref: String,
    artifact_manifest_sha256: String,
    config_sha256: String,
    binary_sha256: String,
    run_ref: String,
}

impl ManagedRunFacts {
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn fixture_for_test(run_ref: &str) -> Self {
        Self {
            plan_id: "fixture-plan".into(),
            plan_digest: "fixture-digest".into(),
            artifact_manifest_ref: "fixture-manifest".into(),
            artifact_manifest_sha256: "fixture-hash".into(),
            config_sha256: "fixture-config".into(),
            binary_sha256: "fixture-binary".into(),
            run_ref: run_ref.into(),
        }
    }

    /// Build a candidate snapshot from the checked artifact and the exact
    /// StartRequested run. The future coordinator must obtain `state` from a
    /// strict, current disk read under the TS/lifecycle gate at spawn time.
    pub(in crate::runtime::proxy) fn from_start_request(
        proof: &VerifiedExactStart,
        state: &MeshRouteState,
        intent: &ManagedDirectIntent,
    ) -> Result<Self, ManagedBirthError> {
        state
            .validate()
            .map_err(|_| ManagedBirthError::StartRequestMismatch)?;
        let tx = state
            .transaction
            .as_ref()
            .ok_or(ManagedBirthError::StartRequestMismatch)?;
        if state.intent.desired_run != MeshDesiredRun::Running
            || tx.phase != MeshTransactionPhase::StartRequested
            || tx.candidate_run_id.as_deref() != Some(intent.run_ref())
            || tx.intent_revision != state.intent.revision
            || super::check_plan(state, tx, proof.plan()).is_err()
            || tx.artifact_manifest_ref.as_deref() != Some(proof.manifest_ref())
            || proof.manifest_ref()
                != format!("mesh-routes/plans/{}/manifest.json", proof.plan().plan_id)
            || !proof.config_hash_matches_owned_bytes()
        {
            return Err(ManagedBirthError::StartRequestMismatch);
        }
        Ok(Self {
            plan_id: proof.plan().plan_id.clone(),
            plan_digest: proof.plan_digest().into(),
            artifact_manifest_ref: proof.manifest_ref().into(),
            artifact_manifest_sha256: proof.manifest_sha256().into(),
            config_sha256: proof.config_sha256().into(),
            binary_sha256: proof.binary_sha256().into(),
            run_ref: intent.run_ref().into(),
        })
    }
}

/// Intended to be consumed once when a spawned Child enters its run slot.
/// The future coordinator must attach the Child returned by its own guarded
/// spawn; this API does not yet enforce the Child's provenance.
pub(crate) struct ManagedDirectBirth {
    identity: RunIdentity,
    facts: ManagedRunFacts,
}

impl ManagedDirectBirth {
    /// Must run before spawn. At present every VerifiedExactStart reports
    /// unsupported runtime facts, so this returns an error before a Child can
    /// be launched or relabeled as managed.
    pub(in crate::runtime::proxy) fn prepare(
        proof: &VerifiedExactStart,
        state: &MeshRouteState,
        intent: ManagedDirectIntent,
    ) -> Result<Self, ManagedBirthError> {
        let facts = ManagedRunFacts::from_start_request(proof, state, &intent)?;
        match proof.readiness() {
            // An additional readiness state must add an explicit arm here.
            // Otherwise a future enum extension could silently enable spawn.
            ExactStartReadiness::UnsupportedRuntimeFacts => {
                let _ = (facts, intent);
                Err(ManagedBirthError::UnsupportedRuntimeFacts)
            }
        }
    }

    /// Attach to a direct Child after a future guarded spawn. Proxy descendants
    /// can still mutate the slot; they are not excluded by this type boundary.
    pub(in crate::runtime::proxy) fn attach(self, child: Child) -> DirectCoreRun {
        DirectCoreRun {
            child,
            identity: self.identity,
            origin: DirectRunOrigin::Managed(self.facts),
        }
    }
}

#[cfg(all(test, unix))]
mod tests;

//! Offline H2 cache handoff ledger. No filesystem write, process observation, or helper wire
//! calls this module yet. The future privileged adapter must durably publish each returned
//! journal before performing the next effect and may construct a stop fence only while holding
//! the lifecycle/TS gate after proving every former cache user exited.
//!
//! A cache is runtime-writable: its migration hash describes only the initial copy. Once a run
//! owns it, neither the journal nor an exact-start receipt asserts immutable cache bytes.

use polaris_helper_proto::{exact::token, ExactReceipt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub mod store;

const SCHEMA_VERSION: u32 = 3;
/// A bounded replay ledger, not an assertion that runRef is globally unique. Reaching the
/// limit is Unsupported until a durable uniqueness/rotation contract is approved.
const MAX_SEEN_RUN_REFS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError {
    Invalid,
    Mismatch,
    UnsafeTransition,
    NonCanonical,
    RunRefCapacity,
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn plan_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Unauthenticated B3/H2 join data. `run_ref` is B3 `candidateRunId`; the old core is named by
/// the distinct, mandatory `stop_target_run_ref`. A future adapter must compare every field to
/// the same gated app transaction and protected artifacts before any effect. The source path
/// digest is over a separately verified canonical absolute path, not over cache bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheClaim {
    pub local_id: String,
    pub intent_revision: String,
    pub boot_id: String,
    pub owner_ref: String,
    pub run_ref: String,
    pub stop_target_run_ref: String,
    pub lifecycle_generation: String,
    pub source_path_sha256: String,
    pub source_config_sha256: String,
    pub plan_id: String,
    pub plan_digest: String,
    /// B3's plan-relative `mesh-routes/plans/<planId>/manifest.json`.
    pub artifact_manifest_ref: String,
    /// SHA-256 of the raw manifest bytes. H1 `ExactBinding.artifact_digest` has this meaning.
    pub artifact_manifest_sha256: String,
    pub artifact_digest: String,
}

impl CacheClaim {
    fn valid(&self) -> bool {
        token(&self.local_id)
            && self
                .intent_revision
                .parse::<u64>()
                .is_ok_and(|revision| revision.to_string() == self.intent_revision)
            && token(&self.boot_id)
            && token(&self.owner_ref)
            && token(&self.run_ref)
            && token(&self.stop_target_run_ref)
            && self.stop_target_run_ref != self.run_ref
            && token(&self.lifecycle_generation)
            && sha256(&self.source_path_sha256)
            && sha256(&self.source_config_sha256)
            && plan_id(&self.plan_id)
            && sha256(&self.plan_digest)
            && self.artifact_manifest_ref
                == format!("mesh-routes/plans/{}/manifest.json", self.plan_id)
            && sha256(&self.artifact_manifest_sha256)
            && sha256(&self.artifact_digest)
            && self.artifact_manifest_sha256 == self.artifact_digest
    }

    fn marker(&self) -> LeaseMarker {
        LeaseMarker {
            run_ref: self.run_ref.clone(),
            lifecycle_generation: self.lifecycle_generation.clone(),
        }
    }
}

/// These are facts supplied by a future no-follow source adapter while the old-core gate is
/// held. `Absent` means a verified ENOENT and a stable parent object, never a failed read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheSource {
    Present {
        sha256: String,
        bytes: u64,
        object_key: String,
    },
    Absent {
        parent_key: String,
    },
}

impl CacheSource {
    fn valid(&self) -> bool {
        match self {
            Self::Present {
                sha256: digest,
                object_key,
                ..
            } => sha256(digest) && token(object_key),
            Self::Absent { parent_key } => token(parent_key),
        }
    }
}

/// The owner-directory object must be protected by the helper. A core may replace cache.db
/// during corruption recovery, so later leases pin the owner directory rather than file bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheCandidate {
    pub owner_key: String,
    pub content: CacheContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheContent {
    Present { sha256: String, bytes: u64 },
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LeaseMarker {
    run_ref: String,
    lifecycle_generation: String,
}

impl LeaseMarker {
    fn valid(&self) -> bool {
        token(&self.run_ref) && token(&self.lifecycle_generation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchIdentity {
    pub core_sha256: String,
    pub launched_config_sha256: String,
    pub launch_closure_digest: String,
}

impl LaunchIdentity {
    fn valid(&self) -> bool {
        sha256(&self.core_sha256)
            && sha256(&self.launched_config_sha256)
            && sha256(&self.launch_closure_digest)
    }
}

/// The process adapter supplies this after a real spawn and OS birth observation. Persisting a
/// Checked journal before spawn means a crash between spawn and Active cannot be auto-released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheProcess {
    daemon_birth: String,
    pid: u32,
    process_birth: String,
}

impl CacheProcess {
    fn valid(&self) -> bool {
        token(&self.daemon_birth) && self.pid > 0 && token(&self.process_birth)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum CachePhase {
    MigrationPending {
        claim: CacheClaim,
        source: CacheSource,
    },
    Idle {
        origin: CacheSource,
        owner_key: String,
        last_released: Option<LeaseMarker>,
    },
    Reserved {
        origin: CacheSource,
        owner_key: String,
        claim: CacheClaim,
        launch: LaunchIdentity,
    },
    Checked {
        origin: CacheSource,
        owner_key: String,
        claim: CacheClaim,
        launch: LaunchIdentity,
    },
    Active {
        origin: CacheSource,
        owner_key: String,
        claim: CacheClaim,
        launch: LaunchIdentity,
        process: CacheProcess,
    },
}

/// A versioned journal value. Persistence is intentionally outside this pure module: the
/// adapter must use protected no-follow objects, write-new+fsync+atomic rename+parent fsync.
/// External crates cannot synthesize a phase by deserializing JSON or calling the disk parser.
///
/// ```compile_fail
/// use polaris_helper::exact_cache::CacheJournal;
/// let _: CacheJournal = serde_json::from_str("{}").unwrap();
/// ```
///
/// ```compile_fail
/// use polaris_helper::exact_cache::CacheJournal;
/// let _ = CacheJournal::parse_canonical(b"{}");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheJournal {
    schema_version: u32,
    local_id: String,
    owner_ref: String,
    owner_uid: u32,
    source_path_sha256: String,
    seen_run_refs: BTreeSet<String>,
    phase: CachePhase,
}

/// Only the protected store may decode persisted bytes. Keeping Deserialize on this private
/// mirror prevents public serde APIs from bypassing the evidence-gated journal transitions.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheJournalDisk {
    schema_version: u32,
    local_id: String,
    owner_ref: String,
    owner_uid: u32,
    source_path_sha256: String,
    seen_run_refs: BTreeSet<String>,
    phase: CachePhase,
}

/// Only a platform/lifecycle adapter may eventually construct this after proving all former
/// users of the cache are gone under the same gate. The pure module exposes no constructor.
pub struct StoppedCacheFence {
    owner_ref: String,
    lifecycle_generation: String,
}

/// Initial migration additionally needs proof that neither a durable journal nor a private
/// owner already exists. A lost journal must never cause the old app cache to overwrite a
/// private cache that may have been changed by a previous core.
pub struct FreshCacheOwner {
    owner_ref: String,
    owner_uid: u32,
    source_path_sha256: String,
}

/// Likewise, a recovery/release proof must refer to this exact process instance, not a PID.
pub struct ExactCacheExit {
    run_ref: String,
    lifecycle_generation: String,
    process: CacheProcess,
}

/// Observation of the protected owner-directory object, made by the future OS adapter.
pub struct CacheOwnerObservation {
    owner_key: String,
}

/// Raw check outcome supplied by the future bounded pinned-core checker. No fail-open verdict
/// is accepted, and a check of an earlier source config cannot authorize the rewritten bytes.
pub enum StrictCheckOutcome {
    ExitedSuccess { checked: CheckedLaunchIdentity },
    ExitedFailure,
    SpawnFailed,
    TimedOut,
}

/// Only the future bounded subprocess adapter may construct this after observing a zero exit
/// from the pinned core and rechecking the same protected config/executable objects.
pub struct CheckedLaunchIdentity {
    launch: LaunchIdentity,
}

/// An internally observed, OS-born child. No production constructor exists. The future
/// platform adapter must hold the lifecycle gate, observe this exact daemon/PID/process birth
/// independently of the public receipt DTO, and bind the reserved lifecycle generation.
#[allow(dead_code)] // H2a is offline; no production adapter can construct this proof yet.
pub(crate) struct ObservedStarted {
    receipt: ExactReceipt,
    lifecycle_generation: String,
    observed_process: CacheProcess,
}

pub struct StrictCheckToken {
    launch: LaunchIdentity,
}

pub fn accept_strict_check(
    expected: &LaunchIdentity,
    outcome: StrictCheckOutcome,
) -> Result<StrictCheckToken, CacheError> {
    if !expected.valid() {
        return Err(CacheError::Invalid);
    }
    match outcome {
        StrictCheckOutcome::ExitedSuccess { checked } if checked.launch == *expected => {
            Ok(StrictCheckToken {
                launch: checked.launch,
            })
        }
        StrictCheckOutcome::ExitedSuccess { .. } => Err(CacheError::Mismatch),
        StrictCheckOutcome::ExitedFailure
        | StrictCheckOutcome::SpawnFailed
        | StrictCheckOutcome::TimedOut => Err(CacheError::UnsafeTransition),
    }
}

impl CacheJournal {
    pub fn begin_migration(
        claim: CacheClaim,
        source: CacheSource,
        fresh: &FreshCacheOwner,
        fence: &StoppedCacheFence,
    ) -> Result<Self, CacheError> {
        if !claim.valid() || !source.valid() {
            return Err(CacheError::Invalid);
        }
        if fence.owner_ref != claim.owner_ref
            || fence.lifecycle_generation != claim.lifecycle_generation
            || fresh.owner_ref != claim.owner_ref
            || fresh.owner_uid == u32::MAX
            || fresh.source_path_sha256 != claim.source_path_sha256
        {
            return Err(CacheError::Mismatch);
        }
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            local_id: claim.local_id.clone(),
            owner_ref: claim.owner_ref.clone(),
            owner_uid: fresh.owner_uid,
            source_path_sha256: claim.source_path_sha256.clone(),
            seen_run_refs: BTreeSet::new(),
            phase: CachePhase::MigrationPending { claim, source },
        })
    }

    /// Completes only an exact initial copy, or an explicitly verified absent source/candidate.
    /// A failed/interrupted copy leaves MigrationPending; recovery never copies over an owner.
    pub fn finish_migration(
        &self,
        source_after: &CacheSource,
        candidate: &CacheCandidate,
        fence: &StoppedCacheFence,
    ) -> Result<Self, CacheError> {
        self.validate()?;
        let CachePhase::MigrationPending { claim, source } = &self.phase else {
            return Err(CacheError::UnsafeTransition);
        };
        if source_after != source
            || !token(&candidate.owner_key)
            || fence.owner_ref != claim.owner_ref
            || fence.lifecycle_generation != claim.lifecycle_generation
        {
            return Err(CacheError::Mismatch);
        }
        let copied = matches!(
            (source, &candidate.content),
            (
                CacheSource::Present { sha256, bytes, .. },
                CacheContent::Present {
                    sha256: copied_sha,
                    bytes: copied_bytes
                }
            ) if sha256 == copied_sha && bytes == copied_bytes
        ) || matches!(
            (source, &candidate.content),
            (CacheSource::Absent { .. }, CacheContent::Absent)
        );
        if !copied {
            return Err(CacheError::Mismatch);
        }
        Ok(Self {
            phase: CachePhase::Idle {
                origin: source.clone(),
                owner_key: candidate.owner_key.clone(),
                last_released: None,
            },
            ..self.clone()
        })
    }

    /// The returned Reserved journal must be durable before check or spawn can touch the cache.
    pub fn reserve(
        &self,
        claim: CacheClaim,
        launch: LaunchIdentity,
        owner: &CacheOwnerObservation,
        fence: &StoppedCacheFence,
    ) -> Result<Self, CacheError> {
        self.validate()?;
        if !claim.valid() || !launch.valid() {
            return Err(CacheError::Invalid);
        }
        let CachePhase::Idle {
            origin,
            owner_key,
            last_released,
        } = &self.phase
        else {
            return Err(CacheError::UnsafeTransition);
        };
        if self.local_id != claim.local_id
            || self.owner_ref != claim.owner_ref
            || self.source_path_sha256 != claim.source_path_sha256
            || fence.owner_ref != claim.owner_ref
            || fence.lifecycle_generation != claim.lifecycle_generation
            || owner.owner_key != *owner_key
            || last_released.as_ref() == Some(&claim.marker())
            || self.seen_run_refs.contains(&claim.run_ref)
        {
            return Err(CacheError::Mismatch);
        }
        if self.seen_run_refs.len() >= MAX_SEEN_RUN_REFS {
            return Err(CacheError::RunRefCapacity);
        }
        let mut seen_run_refs = self.seen_run_refs.clone();
        seen_run_refs.insert(claim.run_ref.clone());
        Ok(Self {
            seen_run_refs,
            phase: CachePhase::Reserved {
                origin: origin.clone(),
                owner_key: owner_key.clone(),
                claim,
                launch,
            },
            ..self.clone()
        })
    }

    pub fn record_checked(&self, token: &StrictCheckToken) -> Result<Self, CacheError> {
        self.validate()?;
        let CachePhase::Reserved {
            origin,
            owner_key,
            claim,
            launch,
        } = &self.phase
        else {
            return Err(CacheError::UnsafeTransition);
        };
        if token.launch != *launch {
            return Err(CacheError::Mismatch);
        }
        Ok(Self {
            phase: CachePhase::Checked {
                origin: origin.clone(),
                owner_key: owner_key.clone(),
                claim: claim.clone(),
                launch: launch.clone(),
            },
            ..self.clone()
        })
    }

    /// A Checked journal cannot itself authorize a receipt: the future adapter must first
    /// prove the OS-born child. A public ExactReceipt alone cannot cross this boundary.
    #[allow(dead_code)] // H2a is offline; the proof-producing spawn adapter is not connected.
    pub(crate) fn record_started(&self, started: &ObservedStarted) -> Result<Self, CacheError> {
        self.validate()?;
        let CachePhase::Checked {
            origin,
            owner_key,
            claim,
            launch,
        } = &self.phase
        else {
            return Err(CacheError::UnsafeTransition);
        };
        let receipt = &started.receipt;
        if !receipt.valid()
            || started.lifecycle_generation != claim.lifecycle_generation
            || !started.observed_process.valid()
            || started.observed_process.daemon_birth != crate::exact::daemon_birth()
            || started.observed_process.daemon_birth != receipt.daemon_birth
            || started.observed_process.pid != receipt.pid
            || started.observed_process.process_birth != receipt.process_birth
            || receipt.binding.run_ref != claim.run_ref
            || receipt.binding.plan_id != claim.plan_id
            || receipt.binding.plan_digest != claim.plan_digest
            || receipt.binding.artifact_digest != claim.artifact_digest
            || receipt.binding.config_sha256 != claim.source_config_sha256
            || receipt.binding.core_sha256 != launch.core_sha256
            || receipt.launched_config_sha256 != launch.launched_config_sha256
            || receipt.launch_closure_digest != launch.launch_closure_digest
        {
            return Err(CacheError::Mismatch);
        }
        Ok(Self {
            phase: CachePhase::Active {
                origin: origin.clone(),
                owner_key: owner_key.clone(),
                claim: claim.clone(),
                launch: launch.clone(),
                process: started.observed_process.clone(),
            },
            ..self.clone()
        })
    }

    pub fn release_exact(&self, exit: &ExactCacheExit) -> Result<Self, CacheError> {
        self.validate()?;
        let CachePhase::Active {
            origin,
            owner_key,
            claim,
            process,
            ..
        } = &self.phase
        else {
            return Err(CacheError::UnsafeTransition);
        };
        if exit.run_ref != claim.run_ref
            || exit.lifecycle_generation != claim.lifecycle_generation
            || exit.process != *process
        {
            return Err(CacheError::Mismatch);
        }
        Ok(Self {
            phase: CachePhase::Idle {
                origin: origin.clone(),
                owner_key: owner_key.clone(),
                last_released: Some(claim.marker()),
            },
            ..self.clone()
        })
    }

    /// Crash recovery is intentionally conservative. Pending, Reserved, Checked, and Active
    /// never auto-resume or overwrite anything; an external exact exit proof must first settle
    /// Active. A missing journal with an existing owner has no entrypoint in this API.
    pub fn recover_idle(
        &self,
        requested: &CacheClaim,
        owner: &CacheOwnerObservation,
        fence: &StoppedCacheFence,
    ) -> Result<(), CacheError> {
        self.validate()?;
        if !requested.valid() {
            return Err(CacheError::Invalid);
        }
        let CachePhase::Idle { owner_key, .. } = &self.phase else {
            return Err(CacheError::UnsafeTransition);
        };
        if owner.owner_key != *owner_key
            || requested.local_id != self.local_id
            || requested.owner_ref != self.owner_ref
            || requested.source_path_sha256 != self.source_path_sha256
            || fence.owner_ref != self.owner_ref
            || fence.lifecycle_generation != requested.lifecycle_generation
        {
            return Err(CacheError::Mismatch);
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, CacheError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CacheError::Invalid)
    }

    fn parse_canonical(bytes: &[u8]) -> Result<Self, CacheError> {
        let disk: CacheJournalDisk =
            serde_json::from_slice(bytes).map_err(|_| CacheError::NonCanonical)?;
        let journal = Self {
            schema_version: disk.schema_version,
            local_id: disk.local_id,
            owner_ref: disk.owner_ref,
            owner_uid: disk.owner_uid,
            source_path_sha256: disk.source_path_sha256,
            seen_run_refs: disk.seen_run_refs,
            phase: disk.phase,
        };
        let encoded = journal.to_canonical_bytes()?;
        if encoded != bytes {
            return Err(CacheError::NonCanonical);
        }
        Ok(journal)
    }

    fn validate(&self) -> Result<(), CacheError> {
        if self.schema_version != SCHEMA_VERSION
            || !token(&self.local_id)
            || !token(&self.owner_ref)
            || self.owner_uid == u32::MAX
            || !sha256(&self.source_path_sha256)
            || self.seen_run_refs.len() > MAX_SEEN_RUN_REFS
            || self.seen_run_refs.iter().any(|run_ref| !token(run_ref))
        {
            return Err(CacheError::Invalid);
        }
        let (origin, owner_key, claim, launch, process, last_released) = match &self.phase {
            CachePhase::MigrationPending { claim, source } => {
                if !self.seen_run_refs.is_empty()
                    || !claim.valid()
                    || claim.local_id != self.local_id
                    || claim.owner_ref != self.owner_ref
                    || claim.source_path_sha256 != self.source_path_sha256
                    || !source.valid()
                {
                    return Err(CacheError::Invalid);
                }
                return Ok(());
            }
            CachePhase::Idle {
                origin,
                owner_key,
                last_released,
            } => (origin, owner_key, None, None, None, last_released.as_ref()),
            CachePhase::Reserved {
                origin,
                owner_key,
                claim,
                launch,
            }
            | CachePhase::Checked {
                origin,
                owner_key,
                claim,
                launch,
            } => (origin, owner_key, Some(claim), Some(launch), None, None),
            CachePhase::Active {
                origin,
                owner_key,
                claim,
                launch,
                process,
            } => (
                origin,
                owner_key,
                Some(claim),
                Some(launch),
                Some(process),
                None,
            ),
        };
        if !origin.valid()
            || !token(owner_key)
            || last_released.is_some_and(|marker| {
                !marker.valid() || !self.seen_run_refs.contains(&marker.run_ref)
            })
            || claim.is_some_and(|claim| {
                !claim.valid()
                    || claim.local_id != self.local_id
                    || claim.owner_ref != self.owner_ref
                    || claim.source_path_sha256 != self.source_path_sha256
                    || !self.seen_run_refs.contains(&claim.run_ref)
            })
            || launch.is_some_and(|launch| !launch.valid())
            || process.is_some_and(|process| !process.valid())
        {
            return Err(CacheError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

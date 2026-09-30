//! Prepare physical images from a checked candidate. No production spawn,
//! managed permit, mutable writer handoff, or generic check-to-run conversion.

use super::check::CheckedCandidate;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use super::check::CHECK_PROFILE;
use crate::runtime::config::ApplyInputSnapshot;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use crate::runtime::proxy::mesh_apply::candidate::{CandidateProfile, CheckSupport};
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtectedCandidateError {
    Unsupported,
    CandidateMismatch,
    Source,
    Protection,
}

/// Owns the checked facts and actual sealed resources together. Neither is
/// Clone, and there is no public constructor, Child attach, or run method.
pub(crate) struct ProtectedCandidate {
    checked: CheckedCandidate,
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    inputs: polaris_core_supervisor::exact_spawn::LinuxProtectedInputs,
}

impl ProtectedCandidate {
    pub(crate) fn checked(&self) -> &CheckedCandidate {
        &self.checked
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub(crate) fn inputs(&self) -> &polaris_core_supervisor::exact_spawn::LinuxProtectedInputs {
        &self.inputs
    }
}

pub(crate) fn prepare_checked_linux_inputs(
    checked: CheckedCandidate,
    snapshot: &ApplyInputSnapshot,
    plan: &ManagedMeshRoutePlan,
    binary_path: &Path,
) -> Result<ProtectedCandidate, ProtectedCandidateError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use polaris_core_supervisor::exact_spawn::{
            prepare_linux_protected_inputs, read_linux_binary_source, LinuxInputProfile,
            MutableObligation,
        };
        let candidate = checked.candidate();
        candidate
            .validate_same_candidate(snapshot, plan, CandidateProfile::DesktopNonTunDirectVlessV1)
            .map_err(|_| ProtectedCandidateError::CandidateMismatch)?;
        if checked.execution_profile() != CHECK_PROFILE
            || candidate.metadata().check_support != CheckSupport::LinuxX8664PinnedPlainTcpOnly
        {
            return Err(ProtectedCandidateError::Unsupported);
        }
        let inputs = prepare_linux_protected_inputs(
            read_linux_binary_source(binary_path).map_err(|_| ProtectedCandidateError::Source)?,
            candidate.materialized.closure.config_bytes.clone(),
            &candidate.materialized.closure.rule_files,
            LinuxInputProfile::B609PlainTcpCheckV1,
            vec![MutableObligation::CacheWriterLeaseAndSelectorReadback],
        )
        .map_err(|_| ProtectedCandidateError::Protection)?;
        let hex = |value: &[u8; 32]| {
            value
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        if hex(inputs.binary_digest()) != checked.binary_sha256()
            || hex(inputs.config_digest()) != checked.config_sha256()
        {
            return Err(ProtectedCandidateError::CandidateMismatch);
        }
        Ok(ProtectedCandidate { checked, inputs })
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (checked, snapshot, plan, binary_path);
        Err(ProtectedCandidateError::Unsupported)
    }
}

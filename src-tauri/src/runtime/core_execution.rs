//! Request issuance for production and explicit, unadmitted business intents
//! for unit-test mocks. The real factory rejects those intents in every build.
use super::core_identity::{AdmissionError, ExecPurpose};
use polaris_core_supervisor::{SpawnRequest, StdioPolicy};
use std::path::Path;
pub(crate) fn run_request(
    binary: &Path,
    purpose: ExecPurpose,
    config: &Path,
    stdio: StdioPolicy,
) -> Result<SpawnRequest, AdmissionError> {
    #[cfg(not(test))]
    {
        Ok(SpawnRequest::new(
            super::core_identity::prepare(binary, purpose)?,
            config,
            stdio,
        ))
    }
    #[cfg(test)]
    {
        let _ = purpose;
        Ok(SpawnRequest::unadmitted_intent(binary, config, stdio))
    }
}
#[cfg(not(test))]
pub(crate) use polaris_core_supervisor::TokioSpawner;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::TokioSpawner;

use super::*;
#[derive(Debug)]
pub(super) struct ProtectedMembers;
pub(super) fn protect_members(
    _: &UntrustedPayload,
    _: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    Err(AdmissionError::new(
        AdmissionErrorKind::UnsupportedExactObject,
        "pre-execution exact-object protection is not proved on this platform",
    ))
}
impl ProtectedMembers {
    pub(super) fn is_current(&self) -> bool {
        false
    }
    pub(super) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        Err(AdmissionError::new(
            AdmissionErrorKind::UnsupportedExactObject,
            "exact-object executor unavailable",
        ))
    }
}

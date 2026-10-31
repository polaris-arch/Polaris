//! External candidates cannot create a check/query execution plan.
use polaris_core_supervisor::admitted_payload::*;
#[test]
fn foreign_probe_cannot_self_authorize_with_its_version_or_path() {
    let error = admit_selected(
        UntrustedPayload::bundled(env!("CARGO_BIN_EXE_check_probe")),
        ExecPurpose::CheckConfig,
    )
    .unwrap_err();
    assert!(matches!(
        error.kind,
        AdmissionErrorKind::Mismatch
            | AdmissionErrorKind::UnsupportedCapability
            | AdmissionErrorKind::UnsupportedExactObject
    ));
}

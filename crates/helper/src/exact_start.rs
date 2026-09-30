//! Preparation-only subject validation. No wire handler or launch permission.

use polaris_helper_proto::exact_start::{CapsuleError, ExactStartCapsule, ExactStartMetadata};

/// Issued only from an actual Linux connection and the existing authorization
/// check. Public PeerCred data, request subjects and ping UID cannot create it.
pub struct AuthenticatedPeer {
    uid: u32,
}

impl AuthenticatedPeer {
    #[must_use]
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    #[cfg(all(test, target_os = "linux"))]
    fn synthetic_for_test(uid: u32) -> Self {
        Self { uid }
    }
}

#[cfg(target_os = "linux")]
pub fn authenticate_linux_peer(
    connection: &tokio::net::UnixStream,
    auth_file: &std::path::Path,
) -> Result<AuthenticatedPeer, crate::platform::linux::auth::AuthError> {
    use crate::platform::linux::auth::{is_authorized, AuthError, PeerCredProvider, TokioPeerCred};
    let credential = TokioPeerCred::new(connection)
        .peer_cred()
        .ok_or(AuthError::Peercred)?;
    if !is_authorized(credential.uid, auth_file) {
        return Err(AuthError::Unauthorized);
    }
    Ok(AuthenticatedPeer {
        uid: credential.uid,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectValidationError {
    Capsule(CapsuleError),
    AuthenticatedUidMismatch,
}

/// A valid return is metadata consistency, never a consumed-resource witness.
pub fn validate_authenticated_subject(
    peer: &AuthenticatedPeer,
    capsule_bytes: &[u8],
    expected: &ExactStartMetadata,
) -> Result<ExactStartCapsule, SubjectValidationError> {
    let capsule =
        ExactStartCapsule::parse(capsule_bytes).map_err(SubjectValidationError::Capsule)?;
    if capsule.metadata().subject_uid != peer.uid() || expected.subject_uid != peer.uid() {
        return Err(SubjectValidationError::AuthenticatedUidMismatch);
    }
    capsule
        .validate_against(expected)
        .map_err(SubjectValidationError::Capsule)?;
    Ok(capsule)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

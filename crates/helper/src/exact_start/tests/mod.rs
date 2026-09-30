use super::*;
use polaris_helper_proto::exact_start::ExecutionProfile;

fn synthetic_published_subject(uid: u32) -> ExactStartMetadata {
    ExactStartMetadata {
        nonce: [1; 32],
        subject_uid: uid,
        plan_id: "plan-a".into(),
        plan_digest: [2; 32],
        candidate_run_id: "direct-a".into(),
        config_digest: [3; 32],
        binary_digest: [4; 32],
        manifest_ref: "mesh-routes/plans/plan-a/manifest.json".into(),
        manifest_digest: [5; 32],
        closure_digest: [6; 32],
        profile: ExecutionProfile::LinuxNoNetworkFixtureV1,
        config_bytes: 1,
        binary_bytes: 1,
        rule_count: 0,
        immutable_bytes: 2,
    }
}

#[test]
fn synthetic_peer_only_tests_subject_is_not_authentication() {
    let peer = AuthenticatedPeer::synthetic_for_test(1000);
    let expected = synthetic_published_subject(1000);
    let bytes = ExactStartCapsule::new(expected.clone()).unwrap().encode();
    assert!(validate_authenticated_subject(&peer, &bytes, &expected).is_ok());
    let other = AuthenticatedPeer::synthetic_for_test(1001);
    assert_eq!(
        validate_authenticated_subject(&other, &bytes, &expected),
        Err(SubjectValidationError::AuthenticatedUidMismatch)
    );
    let mut changed = expected.clone();
    changed.nonce[0] ^= 1;
    assert_eq!(
        validate_authenticated_subject(&peer, &bytes, &changed),
        Err(SubjectValidationError::Capsule(
            CapsuleError::SubjectMismatch
        ))
    );
}

#[tokio::test]
async fn actual_socket_peer_must_pass_existing_authorization_before_issuance() {
    let (connection, _other) = tokio::net::UnixStream::pair().unwrap();
    let actual = connection.peer_cred().unwrap();
    let uid = nix::unistd::getuid().as_raw();
    assert_eq!(actual.uid(), uid);
    let result = authenticate_linux_peer(
        &connection,
        std::path::Path::new("/nonexistent/polaris-exact-auth"),
    );
    if uid == 0 {
        let peer = result.unwrap();
        assert_eq!(peer.uid(), actual.uid());
        let subject = synthetic_published_subject(uid);
        let capsule = ExactStartCapsule::new(subject.clone()).unwrap().encode();
        assert!(validate_authenticated_subject(&peer, &capsule, &subject).is_ok());
        let wrong_subject = synthetic_published_subject(uid + 1);
        let wrong_capsule = ExactStartCapsule::new(wrong_subject.clone())
            .unwrap()
            .encode();
        assert_eq!(
            validate_authenticated_subject(&peer, &wrong_capsule, &wrong_subject),
            Err(SubjectValidationError::AuthenticatedUidMismatch)
        );
    } else {
        assert!(matches!(
            result,
            Err(crate::platform::linux::auth::AuthError::Unauthorized)
        ));
    }
}

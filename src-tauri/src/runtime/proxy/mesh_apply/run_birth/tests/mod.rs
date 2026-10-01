use super::*;

#[cfg(unix)]
#[tokio::test]
async fn legacy_constructor_marks_legacy_and_synthetic_birth_attaches_facts() {
    let mut legacy = DirectCoreRun::with_identity(
        tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap(),
        RunIdentity::new(),
    );
    assert!(matches!(legacy.origin, DirectRunOrigin::Legacy));

    // Construct a synthetic permit only inside this module's unit test. In
    // production prepare() rejects the currently unsupported runtime facts.
    let identity = RunIdentity::new();
    let run_ref = identity.persisted_ref().to_owned();
    let birth = ManagedDirectBirth {
        identity: identity.clone(),
        facts: ManagedRunFacts {
            plan_id: "plan-a".into(),
            plan_digest: "a".repeat(64),
            artifact_manifest_ref: "mesh-routes/plans/plan-a/manifest.json".into(),
            artifact_manifest_sha256: "d".repeat(64),
            config_sha256: "b".repeat(64),
            binary_sha256: "c".repeat(64),
            run_ref: run_ref.clone(),
        },
    };
    let mut managed = birth.attach_synthetic_for_test(
        tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    assert!(managed.identity.same_run(&identity));
    assert!(matches!(
        &managed.origin,
        DirectRunOrigin::Managed(facts) if facts.run_ref == run_ref
    ));
    assert!(matches!(legacy.origin, DirectRunOrigin::Legacy));
    assert!(!legacy.identity.same_run(&managed.identity));

    legacy.child_for_test().kill().await.unwrap();
    managed.child_for_test().kill().await.unwrap();
}

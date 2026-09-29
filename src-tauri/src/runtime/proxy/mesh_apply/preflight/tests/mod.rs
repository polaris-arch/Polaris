use super::*;
use crate::test_support::TestDir;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use std::fs;

fn plan() -> ManagedMeshRoutePlan {
    ManagedMeshRoutePlan {
        schema_version: 1,
        plan_id: "exact-start-plan".into(),
        config_version: "config-7".into(),
        input_state_revision: "12".into(),
        identity_bindings: vec![],
        protected_cidrs: vec![],
        owner_routes: vec![],
        reject_cidrs: vec![],
        unassigned_cidrs: vec![],
        released_cidrs: vec![],
        overrides: vec![],
        dns_managed: false,
    }
}

fn fixture(config_bytes: &[u8]) -> (TestDir, ManagedMeshRoutePlan, CheckedArtifacts, PathBuf) {
    // The checker is injected only in this test fixture. Production
    // CheckedArtifacts is constructed solely by stage_checked_with_core.
    let dir = TestDir::new("polaris-exact-start-test-");
    let plan = plan();
    let staged = super::super::artifact::stage_artifacts(
        dir.path(),
        &plan,
        config_bytes,
        &[("rules/a.json".into(), b"{\"version\":1}".to_vec())],
        "generator-1",
    )
    .unwrap();
    let binary = dir.path().join("test-core");
    fs::write(&binary, b"fixed-core-a").unwrap();
    let checked = CheckedArtifacts {
        core_check: CoreCheckEvidence {
            binary_sha256: binary_digest(&binary).unwrap(),
            config_sha256: staged.manifest.config.sha256.clone(),
            plan_digest: staged.manifest.plan_digest.clone(),
            manifest_ref: staged.manifest_ref.clone(),
        },
        staged,
    };
    (dir, plan, checked, binary)
}

const CONFIG: &[u8] = br#"{"log":{"level":"info","timestamp":true},"inbounds":[],"outbounds":[]}"#;

#[test]
fn exact_start_proof_owns_the_checked_candidate_but_cannot_launch_without_frozen_facts() {
    let (dir, plan, checked, binary) = fixture(CONFIG);
    let proof = verify_exact_start(dir.path(), &plan, &checked, &binary).unwrap();
    assert_eq!(proof.plan_id, plan.plan_id);
    assert_eq!(proof.config_version, plan.config_version);
    assert_eq!(proof.input_state_revision, plan.input_state_revision);
    assert_eq!(proof.plan_digest, checked.core_check.plan_digest);
    assert_eq!(proof.manifest_ref, checked.core_check.manifest_ref);
    assert_eq!(proof.config_sha256, checked.core_check.config_sha256);
    assert_eq!(proof.binary_sha256, checked.core_check.binary_sha256);
    assert_eq!(proof.config_bytes, CONFIG);
    assert_eq!(proof.rule_files[0].1, b"{\"version\":1}");
    assert_eq!(
        proof.config_path,
        dir.path()
            .join("mesh-routes/plans/exact-start-plan/config.json")
    );
    assert!(proof.config.route.is_none());
    assert_eq!(
        proof.readiness,
        ExactStartReadiness::UnsupportedRuntimeFacts
    );
}

#[test]
fn candidate_plan_version_revision_and_receipts_are_exact() {
    let (dir, mut plan, mut checked, binary) = fixture(CONFIG);
    plan.plan_id = "other-plan".into();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
    plan.plan_id = "exact-start-plan".into();
    plan.config_version = "config-8".into();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
    plan.config_version = "config-7".into();
    plan.input_state_revision = "13".into();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
    plan.input_state_revision = "12".into();
    checked.core_check.manifest_ref = "mesh-routes/plans/other/manifest.json".into();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
    checked.core_check.manifest_ref = checked.staged.manifest_ref.clone();
    checked.core_check.config_sha256 = "0".repeat(64);
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
    checked.core_check.config_sha256 = checked.staged.manifest.config.sha256.clone();
    checked.core_check.plan_digest = "0".repeat(64);
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::CandidateMismatch)
    ));
}

#[test]
fn changed_or_missing_binary_and_changed_config_or_rule_fail_closed() {
    let (dir, plan, checked, binary) = fixture(CONFIG);
    fs::write(&binary, b"fixed-core-b").unwrap();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::BinaryChanged)
    ));
    fs::remove_file(&binary).unwrap();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::BinaryUnavailable)
    ));

    let (dir, plan, checked, binary) = fixture(CONFIG);
    fs::write(
        dir.path()
            .join("mesh-routes/plans/exact-start-plan/config.json"),
        b"changed",
    )
    .unwrap();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::Artifact(_))
    ));

    let (dir, plan, checked, binary) = fixture(CONFIG);
    fs::write(
        dir.path()
            .join("mesh-routes/plans/exact-start-plan/rules/a.json"),
        b"changed",
    )
    .unwrap();
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::Artifact(_))
    ));
}

#[test]
fn accepted_but_unrepresentable_config_cannot_form_an_exact_start_proof() {
    let (dir, plan, checked, binary) = fixture(b"{}");
    assert!(matches!(
        verify_exact_start(dir.path(), &plan, &checked, &binary),
        Err(PreflightError::InvalidConfig)
    ));
}

#[test]
fn frozen_full_generator_configs_survive_the_exact_start_typed_roundtrip() {
    // These are the repository's exported full builder outputs, not a hand
    // assembled empty config. The config-engine golden gate separately checks
    // current Rust generation against this corpus and its documented deltas.
    let cases: serde_json::Value = serde_json::from_str(&crate::test_support::repo_file(
        "crates/config-engine/fixtures/config-snapshot.json",
    ))
    .unwrap();
    let cases = cases["cases"].as_array().unwrap();
    assert!(cases.len() >= 30);
    for case in cases {
        let raw = &case["expected"]["config"];
        let typed: SingBoxConfig = serde_json::from_value(raw.clone()).unwrap_or_else(|error| {
            panic!("full config {:?} cannot be read: {error}", case["name"])
        });
        assert_eq!(
            serde_json::to_value(typed).unwrap(),
            *raw,
            "full config {:?} loses a field in the typed read",
            case["name"]
        );
    }
}

use super::*;
use crate::test_support::TestDir;
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshPlanInput,
};
use polaris_config_engine::user_config::mesh_route_state::{MeshRoutePolicy, MeshRouteState};
use std::collections::BTreeMap;

fn tempdir() -> TestDir {
    TestDir::new("polaris-mesh-artifact-test-")
}

fn plan(id: &str) -> ManagedMeshRoutePlan {
    let raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    let policy: MeshRoutePolicy = serde_json::from_value(raw["meshRoutePolicy"].clone()).unwrap();
    let mut state: MeshRouteState = serde_json::from_value(raw["meshRouteState"].clone()).unwrap();
    state.revision = "1".into();
    compile_managed_mesh_plan(ManagedMeshPlanInput {
        plan_id: id.into(),
        config_version: "config-2".into(),
        policy,
        state,
        candidates: vec![],
        scopeable_rule_matchers: BTreeMap::new(),
    })
    .unwrap()
}

fn files() -> Vec<(String, Vec<u8>)> {
    vec![
        ("rules/b.json".into(), b"{\"version\":2}".to_vec()),
        ("rules/a.json".into(), b"{\"version\":1}".to_vec()),
    ]
}

#[test]
fn stages_one_immutable_generation_and_verifies_every_byte_without_secret_in_manifest() {
    let dir = tempdir();
    let plan = plan("stage-plan-a");
    let config = b"{\"authKey\":\"do-not-copy-to-manifest\"}";
    let staged = stage_artifacts(dir.path(), &plan, config, &files(), "generator-1").unwrap();
    assert_eq!(
        staged.manifest_ref,
        "mesh-routes/plans/stage-plan-a/manifest.json"
    );
    assert_eq!(staged.manifest.plan_digest, plan_digest(&plan).unwrap());
    assert_eq!(staged.manifest.config.bytes, config.len() as u64);
    assert_eq!(staged.manifest.rule_files[0].relative_path, "rules/a.json");
    let root = dir.path().join("mesh-routes/plans/stage-plan-a");
    let manifest_bytes = fs::read(root.join(MANIFEST_NAME)).unwrap();
    assert!(!String::from_utf8_lossy(&manifest_bytes).contains("do-not-copy-to-manifest"));
    assert_eq!(
        verify_artifacts(dir.path(), &plan).unwrap(),
        staged.manifest
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o077, 0);
        for relative in [CONFIG_NAME, PLAN_NAME, MANIFEST_NAME, "rules/a.json"] {
            assert_eq!(
                fs::metadata(root.join(relative))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        assert_eq!(staged.durability, DurableWriteGuarantee::FileAndDirectory);
    }
    assert!(stage_artifacts(dir.path(), &plan, b"different", &[], "generator-1").is_err());
    assert_eq!(fs::read(root.join(CONFIG_NAME)).unwrap(), config);
}

#[test]
fn failed_stage_never_publishes_manifest_or_reuses_incomplete_plan_id() {
    let dir = tempdir();
    let plan = plan("stage-plan-b");
    let result =
        stage_artifacts_with_hook(dir.path(), &plan, b"{}", &files(), "generator-1", |point| {
            if point == StagePoint::AfterPayload {
                Err(ArtifactError::Invalid("injected failure"))
            } else {
                Ok(())
            }
        });
    assert_eq!(
        result.unwrap_err(),
        ArtifactError::Invalid("injected failure")
    );
    let root = dir.path().join("mesh-routes/plans/stage-plan-b");
    assert!(!root.join(MANIFEST_NAME).exists());
    assert!(verify_artifacts(dir.path(), &plan).is_err());
    assert!(stage_artifacts(dir.path(), &plan, b"{}", &[], "generator-1").is_err());
}

#[test]
fn unpublished_payload_is_not_referenceable_and_tamper_blocks_publish() {
    let dir = tempdir();
    let plan = plan("stage-pending-tamper");
    let pending = stage_payload(dir.path(), &plan, b"{}", &files(), "generator-1").unwrap();
    let root = dir.path().join("mesh-routes/plans/stage-pending-tamper");
    assert!(!root.join(MANIFEST_NAME).exists());
    assert!(verify_artifacts(dir.path(), &plan).is_err());
    fs::write(root.join(CONFIG_NAME), b"[]").unwrap();
    assert_eq!(
        publish_manifest(pending, &plan).unwrap_err(),
        ArtifactError::Invalid("artifact hash or size mismatch")
    );
    assert!(!root.join(MANIFEST_NAME).exists());
    assert!(stage_payload(dir.path(), &plan, b"{}", &[], "generator-1").is_err());
}

#[test]
fn after_manifest_rename_failure_is_commit_uncertain_and_re_readable() {
    let dir = tempdir();
    let plan = plan("stage-plan-c");
    let result = stage_artifacts_with_hook(dir.path(), &plan, b"{}", &[], "generator-1", |point| {
        if point == StagePoint::AfterManifestRename {
            Err(ArtifactError::Io("injected directory sync failure".into()))
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(ArtifactError::CommitUncertain(_))));
    assert!(verify_artifacts(dir.path(), &plan).is_ok());
}

#[test]
fn post_publish_integrity_failure_is_commit_uncertain() {
    let dir = tempdir();
    let plan = plan("stage-plan-post-publish");
    let root = dir.path().join("mesh-routes/plans/stage-plan-post-publish");
    let result = stage_artifacts_with_hook(dir.path(), &plan, b"{}", &[], "generator-1", |point| {
        if point == StagePoint::AfterManifestRename {
            fs::write(root.join(CONFIG_NAME), b"tampered").unwrap();
        }
        Ok(())
    });
    assert!(matches!(result, Err(ArtifactError::CommitUncertain(_))));
    assert!(verify_artifacts(dir.path(), &plan).is_err());
}

#[test]
fn failure_before_manifest_rename_leaves_only_an_unusable_pending_file() {
    let dir = tempdir();
    let plan = plan("stage-plan-before-rename");
    let result = stage_artifacts_with_hook(dir.path(), &plan, b"{}", &[], "generator-1", |point| {
        if point == StagePoint::BeforeManifestRename {
            Err(ArtifactError::Invalid("injected rename failure"))
        } else {
            Ok(())
        }
    });
    assert_eq!(
        result.unwrap_err(),
        ArtifactError::Invalid("injected rename failure")
    );
    let root = dir
        .path()
        .join("mesh-routes/plans/stage-plan-before-rename");
    assert!(root.join("manifest.pending").exists());
    assert!(!root.join(MANIFEST_NAME).exists());
    assert!(verify_artifacts(dir.path(), &plan).is_err());
}

#[test]
fn tampered_or_missing_file_and_wrong_manifest_binding_are_rejected() {
    let dir = tempdir();
    let plan = plan("stage-plan-d");
    stage_artifacts(dir.path(), &plan, b"{}", &files(), "generator-1").unwrap();
    let root = dir.path().join("mesh-routes/plans/stage-plan-d");
    fs::write(root.join("rules/a.json"), b"tampered").unwrap();
    assert!(matches!(
        verify_artifacts(dir.path(), &plan),
        Err(ArtifactError::Invalid("artifact hash or size mismatch"))
    ));
    fs::write(root.join("rules/a.json"), b"{\"version\":1}").unwrap();
    fs::remove_file(root.join("rules/b.json")).unwrap();
    assert!(verify_artifacts(dir.path(), &plan).is_err());
    let manifest_path = root.join(MANIFEST_NAME);
    let mut manifest: ArtifactManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.plan_digest = "wrong".into();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        verify_artifacts(dir.path(), &plan).unwrap_err(),
        ArtifactError::Invalid("artifact manifest binding mismatch")
    );
}

#[test]
fn path_traversal_duplicate_and_symlinked_parent_are_rejected_before_publish() {
    for bad in [
        "../escape.json",
        "/absolute.json",
        "rules\\escape.json",
        "rules//x.json",
        "C:/x.json",
        "CON.json",
        "rules/trailing.",
        "rules/é.json",
    ] {
        let dir = tempdir();
        let plan = plan("stage-plan-e");
        assert!(stage_artifacts(
            dir.path(),
            &plan,
            b"{}",
            &[(bad.into(), b"{}".to_vec())],
            "generator-1"
        )
        .is_err());
        assert!(!dir.path().join("mesh-routes").exists());
    }
    let dir = tempdir();
    let ordinary_plan = plan("stage-plan-f");
    assert!(stage_artifacts(
        dir.path(),
        &ordinary_plan,
        b"{}",
        &[(CONFIG_NAME.into(), b"{}".to_vec())],
        "generator-1"
    )
    .is_err());
    let reserved_plan = plan("CON");
    assert!(stage_artifacts(dir.path(), &reserved_plan, b"{}", &[], "generator-1").is_err());
    assert!(!dir.path().join("mesh-routes").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tempdir();
        symlink(outside.path(), dir.path().join("mesh-routes")).unwrap();
        assert_eq!(
            stage_artifacts(dir.path(), &ordinary_plan, b"{}", &[], "generator-1").unwrap_err(),
            ArtifactError::Invalid("artifact directory is not a real directory")
        );
    }
}

#[cfg(unix)]
#[test]
fn verifier_rejects_symlinked_rule_file_even_when_hash_matches() {
    use std::os::unix::fs::symlink;
    let dir = tempdir();
    let plan = plan("stage-plan-symlink");
    stage_artifacts(dir.path(), &plan, b"{}", &files(), "generator-1").unwrap();
    let root = dir.path().join("mesh-routes/plans/stage-plan-symlink");
    let outside = tempdir();
    fs::write(outside.path().join("copy.json"), b"{\"version\":1}").unwrap();
    fs::remove_file(root.join("rules/a.json")).unwrap();
    symlink(outside.path().join("copy.json"), root.join("rules/a.json")).unwrap();
    assert_eq!(
        verify_artifacts(dir.path(), &plan).unwrap_err(),
        ArtifactError::Invalid("artifact is not a regular file")
    );
}

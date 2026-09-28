use super::*;
use crate::test_support::TestDir;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshCandidate;
use polaris_config_engine::user_config::mesh_route_state::{
    MeshOwnerRef, MeshRoutePolicy, MeshRouteState,
};
use polaris_core_supervisor::{ConfigCheckVerdict, KernelRejection, RejectedArray};
use serde_json::json;
use std::fs;

use super::super::materialize::{materialize_local_rule_sets, MaterializeError};
use super::super::preflight::{checked_stage_with, stage_checked_with_core, PreflightError};

fn fixture() -> (
    TestDir,
    ManagedMeshPlanInput,
    ManagedMeshRoutePlan,
    SingBoxConfig,
    SingBoxConfig,
    Vec<RulePayload>,
) {
    let dir = TestDir::new("polaris-mesh-closure-test-");
    let raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    let policy: MeshRoutePolicy = serde_json::from_value(raw["meshRoutePolicy"].clone()).unwrap();
    let mut state: MeshRouteState = serde_json::from_value(raw["meshRouteState"].clone()).unwrap();
    state.revision = "1".into();
    let input = ManagedMeshPlanInput {
        plan_id: "closure-plan".into(),
        config_version: "config-2".into(),
        policy,
        state,
        candidates: vec![ManagedMeshCandidate {
            owner_ref: MeshOwnerRef {
                server_id: "ts-a".into(),
                identity_epoch: "epoch-a".into(),
            },
            configured_cidrs: vec!["100.80.0.0/16".into()],
            endpoint_tag: Some("ep-ts-a".into()),
            evidence_complete: true,
        }],
        scopeable_rule_matchers: BTreeMap::new(),
    };
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let path = dir
        .path()
        .join("mesh-routes/plans/closure-plan/rules/a.json")
        .to_string_lossy()
        .into_owned();
    let legacy: SingBoxConfig = serde_json::from_value(json!({
        "log":{"level":"error","timestamp":false,"disabled":true},
        "dns":{
            "servers":[{"tag":"dns-remote","type":"local"}],
            "rules":[],"final":"dns-remote"
        },
        "inbounds":[],
        "outbounds":[{"type":"direct","tag":"direct"}],
        "endpoints":[{
            "type":"tailscale","tag":"ep-ts-a","auth_key":"tskey-auth-example",
            "state_directory":"/tmp/closure-ts-a","control_url":"https://127.0.0.1",
            "hostname":"closure-ts-a"
        }],
        "route":{
            "rule_set":[{"tag":"managed-a","type":"local","format":"source","path":path}],
            "rules":[
                {"action":"sniff"},
                {"process_name":["sing-box","sing-box.exe"],"action":"route","outbound":"direct"},
                {"ip_cidr":["223.5.5.5/32"],"port":[53,443],"action":"route","outbound":"direct"},
                {"port":[53],"action":"hijack-dns"},
                {"ip_cidr":["100.64.0.0/10"],"action":"route","outbound":"direct"}
            ],
            "default_domain_resolver":"dns-remote","final":"direct"
        }
    }))
    .unwrap();
    let config = emit_managed_mesh_config(&legacy, &input, &plan)
        .unwrap()
        .config;
    let rules = vec![RulePayload {
        relative_path: "rules/a.json".into(),
        bytes: b"{\"version\":1,\"rules\":[{\"ip_cidr\":[\"100.80.0.0/16\"]}]}".to_vec(),
    }];
    (dir, input, plan, legacy, config, rules)
}

#[test]
fn emitted_managed_config_preserves_every_field_through_exact_start_read() {
    let (_, _, _, _, emitted, _) = fixture();
    let original = serde_json::to_value(&emitted).unwrap();
    let read: SingBoxConfig = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), original);
}

fn receipt(
    input: &ManagedMeshPlanInput,
    plan: &ManagedMeshRoutePlan,
    config: &SingBoxConfig,
    rules: &[RulePayload],
) -> ExpectedEmission {
    let endpoints = config.endpoints.as_deref().unwrap_or_default();
    ExpectedEmission {
        plan_id: plan.plan_id.clone(),
        plan_digest: plan_digest(plan).unwrap(),
        config_version: input.config_version.clone(),
        input_state_revision: input.state.revision.clone(),
        config_sha256: sha256(&serde_json::to_vec_pretty(config).unwrap()),
        endpoint_sha256: endpoints
            .iter()
            .map(|endpoint| {
                (
                    endpoint.tag.clone(),
                    sha256(&serde_json::to_vec(endpoint).unwrap()),
                )
            })
            .collect(),
        rule_file_sha256: rules
            .iter()
            .map(|rule| (rule.relative_path.clone(), sha256(&rule.bytes)))
            .collect(),
    }
}

fn source_fixture() -> (
    TestDir,
    ManagedMeshPlanInput,
    ManagedMeshRoutePlan,
    SingBoxConfig,
    std::path::PathBuf,
    std::path::PathBuf,
    Vec<u8>,
) {
    let (dir, input, plan, mut legacy, _, rules) = fixture();
    let source_root = dir.path().join("sources");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("builtin-geo.json");
    fs::write(&source, &rules[0].bytes).unwrap();
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].path =
        Some(source.to_string_lossy().into_owned());
    (
        dir,
        input,
        plan,
        legacy,
        source_root,
        source,
        rules[0].bytes.clone(),
    )
}

#[tokio::test]
async fn materializer_snapshots_local_rule_bytes_and_emits_private_paths() {
    let (dir, input, plan, mut legacy, source_root, source, original) = source_fixture();
    let binary = source_root.join("builtin-geo.srs");
    let binary_bytes = b"binary-rule-set-snapshot".to_vec();
    fs::write(&binary, &binary_bytes).unwrap();
    let rule_sets = legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap();
    let mut second = rule_sets[0].clone();
    second.tag = "managed-b".into();
    second.format = "binary".into();
    second.path = Some(binary.to_string_lossy().into_owned());
    rule_sets.push(second);
    let materialized =
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[source_root]).unwrap();
    let root = dir.path().join("mesh-routes/plans/closure-plan");
    assert_eq!(
        materialized
            .emission
            .config
            .route
            .as_ref()
            .unwrap()
            .rule_set
            .as_ref()
            .unwrap()[0]
            .path
            .as_deref(),
        Some(root.join("rules/rs-0000.json").to_str().unwrap())
    );
    assert_eq!(materialized.closure.rule_files[0].1, original);
    assert_eq!(materialized.closure.rule_files[1].1, binary_bytes);
    assert_eq!(
        materialized
            .emission
            .config
            .route
            .as_ref()
            .unwrap()
            .rule_set
            .as_ref()
            .unwrap()[1]
            .path
            .as_deref(),
        Some(root.join("rules/rs-0001.srs").to_str().unwrap())
    );
    assert!(!root.exists());

    // The source may change after snapshot; staging consumes the owned bytes.
    fs::write(source, b"different").unwrap();
    fs::write(binary, b"also different").unwrap();
    let checked = checked_stage_with(
        &plan,
        materialized.closure,
        "generator-1",
        |_| std::future::ready(ConfigCheckVerdict::Accepted),
        || Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(root.join("rules/rs-0000.json")).unwrap(), original);
    assert_eq!(
        fs::read(root.join("rules/rs-0001.srs")).unwrap(),
        binary_bytes
    );
    assert_eq!(checked.manifest.rule_files.len(), 2);
}

#[test]
fn materializer_rejects_stale_plan_remote_missing_and_untrusted_sources_without_writes() {
    let (dir, input, plan, mut legacy, source_root, source, _) = source_fixture();
    let mut stale = input.clone();
    stale.config_version = "config-3".into();
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &stale, &legacy, &[source_root.clone()])
            .unwrap_err(),
        MaterializeError::SnapshotMismatch
    );

    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].type_field = "remote".into();
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[source_root.clone()])
            .unwrap_err(),
        MaterializeError::UnsupportedRuleSet
    );
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].type_field = "local".into();
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].path = Some(
        source_root
            .join("absent.json")
            .to_string_lossy()
            .into_owned(),
    );
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[source_root.clone()])
            .unwrap_err(),
        MaterializeError::MissingRuleSource
    );
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].path = Some(
        dir.path()
            .join("outside.json")
            .to_string_lossy()
            .into_owned(),
    );
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[source_root.clone()])
            .unwrap_err(),
        MaterializeError::UntrustedRuleSource
    );
    assert!(source.exists());
    assert!(!dir.path().join("mesh-routes").exists());
}

#[cfg(unix)]
#[test]
fn materializer_rejects_symlinked_source_or_ancestor() {
    use std::os::unix::fs::symlink;

    let (dir, input, plan, mut legacy, source_root, source, _) = source_fixture();
    let link = source_root.join("link.json");
    symlink(&source, &link).unwrap();
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].path =
        Some(link.to_string_lossy().into_owned());
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[source_root.clone()])
            .unwrap_err(),
        MaterializeError::UntrustedRuleSource
    );
    let linked_root = dir.path().join("linked-sources");
    symlink(&source_root, &linked_root).unwrap();
    assert_eq!(
        materialize_local_rule_sets(dir.path(), &plan, &input, &legacy, &[linked_root])
            .unwrap_err(),
        MaterializeError::InvalidSourceRoot
    );
    assert!(!dir.path().join("mesh-routes").exists());
}

#[test]
fn exact_same_snapshot_endpoint_and_rule_bytes_close_without_writing() {
    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let closed = validate_closure(
        dir.path(),
        &plan,
        &input,
        &legacy,
        &config,
        &expected,
        rules,
    )
    .unwrap();
    assert_eq!(closed.plan_digest, expected.plan_digest);
    assert_eq!(closed.config_sha256, expected.config_sha256);
    assert_eq!(closed.rule_file_sha256, expected.rule_file_sha256);
    assert!(!dir.path().join("mesh-routes").exists());
}

#[test]
fn stale_snapshot_or_changed_plan_cannot_borrow_a_valid_emission_receipt() {
    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let mut changed = input.clone();
    changed.config_version = "config-3".into();
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &changed,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::SnapshotMismatch
    );
    changed = input.clone();
    changed.state.revision = "2".into();
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &changed,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::SnapshotMismatch
    );
    changed = input.clone();
    changed.candidates[0].configured_cidrs = vec!["100.81.0.0/16".into()];
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &changed,
            &legacy,
            &config,
            &expected,
            rules
        )
        .unwrap_err(),
        ClosureError::PlanMismatch
    );
    assert!(!dir.path().join("mesh-routes").exists());
}

#[test]
fn trusted_emitter_recalculation_catches_route_or_endpoint_change_despite_new_hash() {
    let (dir, input, plan, legacy, mut config, rules) = fixture();
    config.route.as_mut().unwrap().rules.pop();
    let expected = receipt(&input, &plan, &config, &rules);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::EmissionMismatch
    );
    let (_, _, _, _, mut config, _) = fixture();
    config.endpoints.as_mut().unwrap()[0].hostname = Some("changed".into());
    let expected = receipt(&input, &plan, &config, &rules);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules
        )
        .unwrap_err(),
        ClosureError::EmissionMismatch
    );
}

#[test]
fn receipt_hashes_must_match_the_independently_regenerated_emission() {
    let (dir, input, plan, legacy, config, rules) = fixture();
    let mut expected = receipt(&input, &plan, &config, &rules);
    expected.config_sha256 = "wrong".into();
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::ConfigHashMismatch
    );
    let mut expected = receipt(&input, &plan, &config, &rules);
    expected
        .endpoint_sha256
        .insert("ep-ts-a".into(), "wrong".into());
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules
        )
        .unwrap_err(),
        ClosureError::EndpointMismatch
    );
}

#[test]
fn local_rule_set_closure_rejects_missing_extra_tampered_or_outside_files() {
    let (dir, input, plan, mut legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            vec![]
        )
        .unwrap_err(),
        ClosureError::RuleFileMismatch
    );
    let mut tampered = rules.clone();
    tampered[0].bytes = b"tampered".to_vec();
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            tampered
        )
        .unwrap_err(),
        ClosureError::RuleFileMismatch
    );
    let mut extra = rules.clone();
    extra.push(RulePayload {
        relative_path: "rules/b.json".into(),
        bytes: b"{}".to_vec(),
    });
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            extra
        )
        .unwrap_err(),
        ClosureError::RuleFileMismatch
    );
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].path =
        Some(dir.path().join("global.srs").to_string_lossy().into_owned());
    let config = emit_managed_mesh_config(&legacy, &input, &plan)
        .unwrap()
        .config;
    let expected = receipt(&input, &plan, &config, &rules);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::RuleSetOutsidePlan
    );
    legacy.route.as_mut().unwrap().rule_set.as_mut().unwrap()[0].type_field = "remote".into();
    let config = emit_managed_mesh_config(&legacy, &input, &plan)
        .unwrap()
        .config;
    let expected = receipt(&input, &plan, &config, &rules);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules
        )
        .unwrap_err(),
        ClosureError::RuleSetUnsupported
    );
    assert!(!dir.path().join("mesh-routes").exists());
}

#[test]
fn missing_or_duplicate_endpoint_is_rejected_by_the_same_emitter() {
    let (dir, input, plan, mut legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    legacy.endpoints = None;
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules.clone()
        )
        .unwrap_err(),
        ClosureError::EmissionMismatch
    );
    let (_, _, _, mut legacy, _, _) = fixture();
    let duplicate = legacy.endpoints.as_ref().unwrap()[0].clone();
    legacy.endpoints.as_mut().unwrap().push(duplicate);
    assert_eq!(
        validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules
        )
        .unwrap_err(),
        ClosureError::EmissionMismatch
    );
    assert!(!dir.path().join("mesh-routes").exists());
}

#[tokio::test]
async fn accepted_check_runs_before_manifest_publish() {
    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let closure = validate_closure(
        dir.path(),
        &plan,
        &input,
        &legacy,
        &config,
        &expected,
        rules,
    )
    .unwrap();
    let root = dir.path().join("mesh-routes/plans/closure-plan");
    let during_check = root.clone();
    let staged = checked_stage_with(
        &plan,
        closure,
        "generator-1",
        move |config_path| async move {
            assert_eq!(
                fs::read(config_path).unwrap(),
                serde_json::to_vec_pretty(&config).unwrap()
            );
            assert!(!during_check.join("manifest.json").exists());
            ConfigCheckVerdict::Accepted
        },
        || Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(staged.manifest.plan_digest, expected.plan_digest);
    assert!(root.join("manifest.json").exists());
}

#[tokio::test]
async fn rejected_unattributable_and_unavailable_checks_leave_no_manifest() {
    let cases = [
        (
            ConfigCheckVerdict::Rejected(KernelRejection {
                array: RejectedArray::Outbounds,
                index: 0,
                detail: "rejected".into(),
            }),
            PreflightError::CoreRejected,
        ),
        (
            ConfigCheckVerdict::Unattributable("bad route".into()),
            PreflightError::CoreUnattributable,
        ),
        (
            ConfigCheckVerdict::Unavailable("timeout".into()),
            PreflightError::CoreUnavailable,
        ),
    ];
    for (verdict, expected_error) in cases {
        let (dir, input, plan, legacy, config, rules) = fixture();
        let expected = receipt(&input, &plan, &config, &rules);
        let closure = validate_closure(
            dir.path(),
            &plan,
            &input,
            &legacy,
            &config,
            &expected,
            rules,
        )
        .unwrap();
        let root = dir.path().join("mesh-routes/plans/closure-plan");
        let during_check = root.clone();
        let result = checked_stage_with(
            &plan,
            closure,
            "generator-1",
            move |path| async move {
                assert!(path.exists());
                assert!(!during_check.join("manifest.json").exists());
                verdict
            },
            || Ok(()),
        )
        .await;
        assert_eq!(result.unwrap_err(), expected_error);
        assert!(!root.join("manifest.json").exists());
        assert!(super::super::artifact::verify_artifacts(dir.path(), &plan).is_err());
    }
}

#[tokio::test]
async fn changed_binary_or_payload_after_check_cannot_publish_manifest() {
    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let closure = validate_closure(
        dir.path(),
        &plan,
        &input,
        &legacy,
        &config,
        &expected,
        rules,
    )
    .unwrap();
    let root = dir.path().join("mesh-routes/plans/closure-plan");
    let result = checked_stage_with(
        &plan,
        closure,
        "generator-1",
        |_| std::future::ready(ConfigCheckVerdict::Accepted),
        || Err(PreflightError::BinaryChanged),
    )
    .await;
    assert_eq!(result.unwrap_err(), PreflightError::BinaryChanged);
    assert!(!root.join("manifest.json").exists());

    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let closure = validate_closure(
        dir.path(),
        &plan,
        &input,
        &legacy,
        &config,
        &expected,
        rules,
    )
    .unwrap();
    let root = dir.path().join("mesh-routes/plans/closure-plan");
    let config_path = root.join("config.json");
    let result = checked_stage_with(
        &plan,
        closure,
        "generator-1",
        |_| std::future::ready(ConfigCheckVerdict::Accepted),
        move || {
            fs::write(config_path, b"tampered").unwrap();
            Ok(())
        },
    )
    .await;
    assert!(matches!(result, Err(PreflightError::Artifact(_))));
    assert!(!root.join("manifest.json").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn fixed_b609_core_check_accepts_only_the_staged_config_bytes() {
    let bundled =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../resources/linux/sing-box");
    let binary = std::env::var_os("POLARIS_TEST_CORE")
        .map(std::path::PathBuf::from)
        .unwrap_or(bundled);
    if !binary.is_file() {
        assert_ne!(
            std::env::var("POLARIS_REQUIRE_KERNEL_GATE").as_deref(),
            Ok("1"),
            "fixed b609 core is required for this gate"
        );
        eprintln!("fixed b609 core absent; set POLARIS_TEST_CORE or fetch package core");
        return;
    }
    let version = std::process::Command::new(&binary)
        .arg("version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout)
        .contains("b609f959f57ce34416c51c7b87ce4a76f2e1df56"));
    let (dir, input, plan, legacy, config, rules) = fixture();
    let expected = receipt(&input, &plan, &config, &rules);
    let closure = validate_closure(
        dir.path(),
        &plan,
        &input,
        &legacy,
        &config,
        &expected,
        rules,
    )
    .unwrap();
    let checked = stage_checked_with_core(&plan, closure, "generator-1", &binary)
        .await
        .unwrap();
    assert_eq!(checked.core_check().plan_digest, expected.plan_digest);
    assert_eq!(checked.core_check().config_sha256, expected.config_sha256);
    assert_eq!(
        checked.core_check().manifest_ref,
        checked.staged().manifest_ref
    );
    assert_eq!(checked.core_check().binary_sha256.len(), 64);
    assert!(dir
        .path()
        .join("mesh-routes/plans/closure-plan/manifest.json")
        .exists());
}

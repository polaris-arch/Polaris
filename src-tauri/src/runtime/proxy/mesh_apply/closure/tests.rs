use super::*;
use crate::test_support::TestDir;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshCandidate;
use polaris_config_engine::user_config::mesh_route_state::{
    MeshOwnerRef, MeshRoutePolicy, MeshRouteState,
};
use serde_json::json;

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
        "../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
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
        bytes: b"{\"version\":1}".to_vec(),
    }];
    (dir, input, plan, legacy, config, rules)
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

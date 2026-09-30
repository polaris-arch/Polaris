use super::*;
use crate::user_config::mesh_route_state::{MeshAssignment, MeshOverride};
use serde_json::json;

fn fixture() -> (MeshRoutePolicy, MeshRouteState) {
    let wire: serde_json::Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    (
        serde_json::from_value(wire["meshRoutePolicy"].clone()).unwrap(),
        serde_json::from_value(wire["meshRouteState"].clone()).unwrap(),
    )
}

fn owner(server_id: &str, identity_epoch: &str) -> MeshOwnerRef {
    MeshOwnerRef {
        server_id: server_id.into(),
        identity_epoch: identity_epoch.into(),
    }
}

fn input() -> ManagedMeshPlanInput {
    let (policy, state) = fixture();
    ManagedMeshPlanInput {
        plan_id: "plan-unique-1".into(),
        config_version: "frontend-version-1".into(),
        policy,
        state,
        candidates: vec![ManagedMeshCandidate {
            owner_ref: owner("ts-a", "epoch-a"),
            configured_cidrs: vec!["100.80.0.0/16".into()],
            endpoint_tag: Some("ep-ts-a".into()),
            evidence_complete: true,
        }],
        scopeable_rule_matchers: BTreeMap::new(),
    }
}

fn add_second_bound(input: &mut ManagedMeshPlanInput) {
    input.state.identities.push(
        serde_json::from_value(json!({
            "serverId":"ts-b", "identityEpoch":"epoch-b",
            "controlAuthority":"https://other.example.invalid/",
            "bindingState":"bound", "evidenceSource":"status"
        }))
        .unwrap(),
    );
    input.candidates.push(ManagedMeshCandidate {
        owner_ref: owner("ts-b", "epoch-b"),
        configured_cidrs: vec!["100.80.0.0/24".into()],
        endpoint_tag: Some("ep-ts-b".into()),
        evidence_complete: true,
    });
}

fn covers(cidrs: &[String], target: &str) -> bool {
    cidrs.iter().any(|cidr| cidr_contains(cidr, target))
}

fn first_match<'a>(plan: &'a ManagedMeshRoutePlan, address: &str) -> &'a str {
    for route in &plan.owner_routes {
        if cidr_contains(&route.cidr, address) {
            return &route.endpoint_tag;
        }
    }
    if covers(&plan.reject_cidrs, address) {
        "reject"
    } else {
        "public-final"
    }
}

#[test]
fn nested_explicit_owner_wins_and_whole_q_reject_follows_all_owner_routes() {
    let mut input = input();
    add_second_bound(&mut input);
    input.policy.assignments.push(MeshAssignment {
        cidr: "100.80.0.0/24".into(),
        target: MeshTarget::Owner {
            server_id: "ts-b".into(),
            identity_epoch: "epoch-b".into(),
        },
    });
    let plan = compile_managed_mesh_plan(input).unwrap();
    assert!(plan
        .owner_routes
        .iter()
        .any(|route| { route.cidr == "100.80.0.0/24" && route.endpoint_tag == "ep-ts-b" }));
    assert!(!plan.owner_routes.iter().any(|route| {
        route.endpoint_tag == "ep-ts-a" && cidr_contains(&route.cidr, "100.80.0.0/24")
    }));
    assert!(covers(&plan.reject_cidrs, "100.80.0.0/24"));
    assert!(!covers(&plan.protected_cidrs, TAILNET_MAGICDNS_V4));
    assert!(!covers(&plan.protected_cidrs, TAILNET_MAGICDNS_V6));
    assert_eq!(plan.identity_bindings.len(), 2);
    assert_eq!(first_match(&plan, "100.80.0.8/32"), "ep-ts-b");
    assert_eq!(first_match(&plan, "100.80.1.8/32"), "ep-ts-a");
    assert_eq!(first_match(&plan, "100.81.1.8/32"), "reject");
    assert_eq!(first_match(&plan, "8.8.8.8/32"), "public-final");
    for (index, route) in plan.owner_routes.iter().enumerate() {
        assert!(covers(&plan.protected_cidrs, &route.cidr));
        for other in plan.owner_routes.iter().skip(index + 1) {
            assert!(!cidrs_overlap(&route.cidr, &other.cidr));
        }
    }
}

#[test]
fn missing_endpoint_or_retired_epoch_keeps_scope_rejected_without_transfer() {
    let mut input = input();
    add_second_bound(&mut input);
    input.policy.assignments.push(MeshAssignment {
        cidr: "100.80.0.0/24".into(),
        target: MeshTarget::Owner {
            server_id: "ts-b".into(),
            identity_epoch: "epoch-b".into(),
        },
    });
    input.candidates[1].endpoint_tag = None;
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    assert!(!plan
        .owner_routes
        .iter()
        .any(|route| cidr_contains(&route.cidr, "100.80.0.0/24")));
    assert!(covers(&plan.unassigned_cidrs, "100.80.0.0/24"));
    assert_eq!(first_match(&plan, "100.80.0.8/32"), "reject");

    input.candidates[1].endpoint_tag = Some("ep-ts-b".into());
    input.state.identities[1].binding_state = MeshBindingState::Retired;
    let retired = compile_managed_mesh_plan(input).unwrap();
    assert!(covers(&retired.unassigned_cidrs, "100.80.0.0/24"));
}

#[test]
fn explicit_unmanaged_release_is_not_inferred_from_absence() {
    let mut input = input();
    input.policy.assignments.push(MeshAssignment {
        cidr: "100.80.0.0/24".into(),
        target: MeshTarget::Unmanaged,
    });
    let plan = compile_managed_mesh_plan(input).unwrap();
    assert!(covers(&plan.released_cidrs, "100.80.0.0/24"));
    assert!(!covers(&plan.protected_cidrs, "100.80.0.0/24"));
    assert_eq!(first_match(&plan, "100.80.0.8/32"), "public-final");
    assert!(!plan
        .owner_routes
        .iter()
        .any(|route| cidr_contains(&route.cidr, "100.80.0.0/24")));
}

#[test]
fn legacy_bootstrap_candidate_never_claims_unknown_pool() {
    let mut input = input();
    input.candidates[0].configured_cidrs = vec![
        TAILNET_CGNAT.into(),
        TAILNET_ULA_V6.into(),
        "100.80.1.9/32".into(),
    ];
    let plan = compile_managed_mesh_plan(input).unwrap();
    assert_eq!(first_match(&plan, "100.80.1.9/32"), "ep-ts-a");
    assert_eq!(first_match(&plan, "100.82.1.9/32"), "reject");
    assert_eq!(first_match(&plan, "fd7a:115c:a1e0:1::9/128"), "reject");
}

#[test]
fn opaque_candidates_catch_all_conflicts_and_unscoped_overrides_block_plan() {
    let mut opaque = input();
    opaque.candidates[0].evidence_complete = false;
    assert!(compile_managed_mesh_plan(opaque).is_err());

    let mut full = input();
    full.candidates[0].configured_cidrs = vec!["0.0.0.0/0".into()];
    assert!(compile_managed_mesh_plan(full).is_err());

    let mut conflict = input();
    conflict.policy.assignments.push(MeshAssignment {
        cidr: "100.80.0.0/16".into(),
        target: MeshTarget::Reject,
    });
    assert!(compile_managed_mesh_plan(conflict).is_err());

    let mut override_input = input();
    override_input.policy.overrides.push(MeshOverride {
        rule_id: "rule-1".into(),
        scope_cidrs: vec!["100.80.1.0/24".into()],
        target: MeshTarget::Reject,
    });
    assert!(compile_managed_mesh_plan(override_input.clone()).is_err());
    override_input.scopeable_rule_matchers.insert(
        "rule-1".into(),
        RouteRule {
            domain_suffix: Some(vec!["corp.example.invalid".into()]),
            ..RouteRule::default()
        },
    );
    let plan = compile_managed_mesh_plan(override_input).unwrap();
    assert_eq!(plan.overrides[0].scope_cidrs, vec!["100.80.1.0/24"]);
    assert_eq!(
        plan.overrides[0].matcher.domain_suffix,
        Some(vec!["corp.example.invalid".to_string()])
    );

    let mut injected = input();
    injected.policy.overrides.push(MeshOverride {
        rule_id: "rule-1".into(),
        scope_cidrs: vec!["100.80.1.0/24".into()],
        target: MeshTarget::Reject,
    });
    injected.scopeable_rule_matchers.insert(
        "rule-1".into(),
        RouteRule {
            domain_suffix: Some(vec!["corp.example.invalid".into()]),
            outbound: Some("direct".into()),
            ..RouteRule::default()
        },
    );
    assert!(compile_managed_mesh_plan(injected).is_err());
}

#[test]
fn dns_policy_epochs_are_bound_even_when_the_ip_route_plan_rejects_them() {
    let mut input = input();
    input.policy.dns_policy = Some(
        serde_json::from_value(json!({
            "schemaVersion": 1,
            "suffixAssignments": [{
                "suffix": "one.example.invalid",
                "target": {"kind": "owner", "serverId": "ts-b", "identityEpoch": "epoch-b"}
            }],
            "shortNamePolicy": {"kind": "owner", "serverId": "ts-c", "identityEpoch": "epoch-c"},
            "serviceOwner": {"kind": "owner", "serverId": "ts-d", "identityEpoch": "epoch-d"}
        }))
        .unwrap(),
    );
    let plan = compile_managed_mesh_plan(input).unwrap();
    assert!(plan.dns_managed);
    assert_eq!(
        plan.identity_bindings,
        vec![
            owner("ts-a", "epoch-a"),
            owner("ts-b", "epoch-b"),
            owner("ts-c", "epoch-c"),
            owner("ts-d", "epoch-d"),
        ]
    );
}

#[test]
fn ip_assignment_cannot_silently_disappear_into_magic_dns_service_scope() {
    let mut input = input();
    input.policy.assignments.push(MeshAssignment {
        cidr: TAILNET_MAGICDNS_V4.into(),
        target: MeshTarget::Reject,
    });
    assert!(compile_managed_mesh_plan(input).is_err());
}

#[test]
fn nested_override_matcher_is_bounded_before_serializing_the_root() {
    let leaf = RouteRule {
        domain_suffix: Some(vec!["corp.example.invalid".into()]),
        ..RouteRule::default()
    };
    let mut deep = leaf.clone();
    for _ in 0..9 {
        deep = RouteRule {
            type_field: Some("logical".into()),
            mode: Some("and".into()),
            rules: Some(vec![deep]),
            ..RouteRule::default()
        };
    }
    assert!(validate_pure_matcher(&deep).is_err());

    let four_leaves = RouteRule {
        type_field: Some("logical".into()),
        mode: Some("or".into()),
        rules: Some(vec![leaf; 4]),
        ..RouteRule::default()
    };
    let wide = RouteRule {
        type_field: Some("logical".into()),
        mode: Some("and".into()),
        rules: Some(vec![four_leaves; 32]),
        ..RouteRule::default()
    };
    assert!(validate_pure_matcher(&wide).is_err());
}

use super::*;
use serde_json::Value;

fn fixture() -> (MeshRoutePolicy, MeshRouteState) {
    let wire: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    let policy = serde_json::from_value(wire["meshRoutePolicy"].clone()).unwrap();
    let mut state: MeshRouteState = serde_json::from_value(wire["meshRouteState"].clone()).unwrap();
    state.revision = "4".into();
    (policy, state)
}

fn old_owner() -> MeshOwnerRef {
    MeshOwnerRef {
        server_id: "ts-a".into(),
        identity_epoch: "epoch-a".into(),
    }
}

fn new_identity<'a>(
    epoch: &'a str,
    authority: &'a str,
    stable: Option<&'a str>,
) -> ReplacementIdentity<'a> {
    ReplacementIdentity {
        epoch,
        control_authority: authority,
        self_stable_id: stable,
    }
}

fn complete_scope<'a>(
    state: &'a MeshRouteState,
    configured: &'a [String],
    active: &'a [String],
) -> RetirementScopeSnapshot<'a> {
    RetirementScopeSnapshot {
        owner_ref: old_owner(),
        state_revision: &state.revision,
        active_plan_id: state.active_plan.as_ref().map(|plan| plan.plan_id.as_str()),
        configured_cidrs: Some(configured),
        active_plan_owner_cidrs: Some(active),
    }
}

#[test]
fn empty_status_and_rename_do_not_rotate_epoch() {
    let (policy, previous) = fixture();
    // Display name is not identity evidence. A missing STATUS self ID is unknown.
    let same = reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity("unused-epoch", "https://control.example.test/path", None),
        None,
    )
    .unwrap();
    assert!(same.is_none());
    let same = reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity(
            "unused-epoch",
            "https://control.example.test/path",
            Some("self-a"),
        ),
        None,
    )
    .unwrap();
    assert!(same.is_none());
    let changed = reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity(
            "epoch-b",
            "https://control.example.test/path",
            Some("self-b"),
        ),
        Some(complete_scope(&previous, &[], &[])),
    )
    .unwrap()
    .unwrap();
    assert_eq!(changed.identities.len(), 2);
    assert_eq!(changed.identity_effects.len(), 0);
    assert!(changed
        .reservations
        .iter()
        .any(|reservation| reservation.cidr == "100.80.1.2/32"));
}

#[test]
fn authority_case_normalizes_but_path_and_stable_identity_do_not() {
    let (policy, mut previous) = fixture();
    assert!(reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity(
            "unused",
            "HTTPS://CONTROL.EXAMPLE.TEST/path",
            Some("self-a")
        ),
        None,
    )
    .unwrap()
    .is_none());
    previous.identities[0].self_stable_id = None;
    let enriched = reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity(
            "unused",
            "https://control.example.test/path",
            Some("self-a"),
        ),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(enriched.identities.len(), 1);
    assert_eq!(enriched.identities[0].identity_epoch, "epoch-a");
    let changed = reconcile_identity_evidence(
        &enriched,
        &policy,
        "ts-a",
        new_identity(
            "epoch-b",
            "https://control.example.test/path/",
            Some("self-a"),
        ),
        Some(complete_scope(&enriched, &[], &[])),
    )
    .unwrap()
    .unwrap();
    assert_eq!(changed.identities.len(), 2);
}

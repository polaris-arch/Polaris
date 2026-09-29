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
fn retirement_retains_every_old_scope_and_records_only_old_epoch() {
    let (policy, mut previous) = fixture();
    // Public CIDR exists only in the old node config; the second one exists
    // only in the last applied owner plan. Neither is in STATUS or policy.
    let configured = vec!["203.0.113.0/24".into()];
    let active = vec!["198.51.100.0/24".into()];
    previous.active_plan = Some(super::super::mesh_route_state::MeshActivePlan {
        plan_id: "old-plan".into(),
        digest: "digest".into(),
        config_version: "config-version".into(),
        input_state_revision: "4".into(),
    });
    previous.observations[0]
        .raw_hosts
        .push("100.80.1.9/32".into());
    previous.observations[0]
        .advertised_routes
        .push("fd00:1234::/48".into());
    let next = reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        Some(new_identity(
            "epoch-b",
            "https://other.example.test/path",
            None,
        )),
        ControlledIdentityChange::ExplicitReplacement,
        Some("effect-old-a"),
        complete_scope(&previous, &configured, &active),
    )
    .unwrap();
    assert_eq!(next.revision, "5");
    assert_eq!(next.intent.revision, "2");
    for cidr in [
        "100.80.0.0/16",
        "100.80.1.2/32",
        "100.80.1.9/32",
        "fd00:1234::/48",
        "203.0.113.0/24",
        "198.51.100.0/24",
    ] {
        assert!(
            next.reservations.iter().any(|reservation| {
                reservation.cidr == cidr
                    && reservation.owner_ref
                        == MeshReservationOwner::Owner {
                            server_id: "ts-a".into(),
                            identity_epoch: "epoch-a".into(),
                        }
            }),
            "lost reservation {cidr}"
        );
    }
    assert_eq!(
        next.identities
            .iter()
            .find(|identity| identity.identity_epoch == "epoch-a")
            .unwrap()
            .binding_state,
        MeshBindingState::Retired
    );
    assert_eq!(
        next.identities
            .iter()
            .find(|identity| identity.identity_epoch == "epoch-b")
            .unwrap()
            .binding_state,
        MeshBindingState::Unbound
    );
    assert_eq!(next.identity_effects[0].retired_epoch, "epoch-a");
    assert_eq!(
        next.identity_effects[0].status,
        MeshIdentityEffectStatus::Pending
    );
    assert_eq!(next.observations, previous.observations);
}

#[test]
fn new_epoch_or_unknown_directory_blocks_old_effect() {
    let (policy, previous) = fixture();
    let mut next = reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        Some(new_identity(
            "epoch-b",
            "https://control.example.test/path",
            None,
        )),
        ControlledIdentityChange::ExplicitReplacement,
        Some("effect-old-a"),
        complete_scope(&previous, &[], &[]),
    )
    .unwrap();
    assert!(
        check_retired_effect_target(&next, "effect-old-a", StateDirectoryOccupant::Unknown)
            .is_err()
    );
    assert!(check_retired_effect_target(
        &next,
        "effect-old-a",
        StateDirectoryOccupant::Epoch("epoch-b")
    )
    .is_err());
    assert!(check_retired_effect_target(
        &next,
        "effect-old-a",
        StateDirectoryOccupant::Epoch("epoch-a")
    )
    .is_ok());
    next.identities
        .iter_mut()
        .find(|identity| identity.identity_epoch == "epoch-b")
        .unwrap()
        .binding_state = MeshBindingState::Bound;
    assert!(check_retired_effect_target(
        &next,
        "effect-old-a",
        StateDirectoryOccupant::Epoch("epoch-a")
    )
    .is_err());
    next.identity_effects[0].retired_epoch = "epoch-b".into();
    assert!(check_retired_effect_target(
        &next,
        "effect-old-a",
        StateDirectoryOccupant::Epoch("epoch-b")
    )
    .is_err());
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
fn config_replacement_never_grants_delete_effect() {
    let (policy, previous) = fixture();
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        Some(new_identity(
            "epoch-b",
            "https://control.example.test/other",
            None
        )),
        ControlledIdentityChange::ConfigReplacement,
        Some("wrong-effect"),
        complete_scope(&previous, &[], &[]),
    )
    .is_err());
    let next = reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        Some(new_identity(
            "epoch-b",
            "https://control.example.test/other",
            None,
        )),
        ControlledIdentityChange::ConfigReplacement,
        None,
        complete_scope(&previous, &[], &[]),
    )
    .unwrap();
    assert!(next.identity_effects.is_empty());
    assert_eq!(next.reservations.len(), 2);
}

#[test]
fn retirement_blocks_unknown_scope_and_over_budget_input() {
    let (policy, previous) = fixture();
    let replacement = Some(new_identity("epoch-b", "https://new.example.test", None));
    let missing_plan = RetirementScopeSnapshot {
        owner_ref: old_owner(),
        state_revision: &previous.revision,
        active_plan_id: None,
        configured_cidrs: Some(&[]),
        active_plan_owner_cidrs: None,
    };
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        replacement,
        ControlledIdentityChange::ConfigReplacement,
        None,
        missing_plan,
    )
    .is_err());
    let mut stale = complete_scope(&previous, &[], &[]);
    stale.state_revision = "3";
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        replacement,
        ControlledIdentityChange::ConfigReplacement,
        None,
        stale,
    )
    .is_err());
    let mut wrong_owner = complete_scope(&previous, &[], &[]);
    wrong_owner.owner_ref.identity_epoch = "different-epoch".into();
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        replacement,
        ControlledIdentityChange::ConfigReplacement,
        None,
        wrong_owner,
    )
    .is_err());
    assert!(reconcile_identity_evidence(
        &previous,
        &policy,
        "ts-a",
        new_identity("epoch-b", "https://new.example.test", None),
        None,
    )
    .is_err());
    let huge = vec!["203.0.113.0/24".into(); MAX_RETIREMENT_SCOPE_CIDRS + 1];
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        replacement,
        ControlledIdentityChange::ConfigReplacement,
        None,
        complete_scope(&previous, &huge, &[]),
    )
    .is_err());
    let default = vec!["0.0.0.0/0".into()];
    assert!(reconcile_controlled_identity(
        &previous,
        &policy,
        &old_owner(),
        replacement,
        ControlledIdentityChange::ConfigReplacement,
        None,
        complete_scope(&previous, &default, &[]),
    )
    .is_err());
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

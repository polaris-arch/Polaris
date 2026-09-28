//! Pure S3c identity reconciliation. No filesystem or Tailscale state access.
//! A caller must commit the returned ledger with its config change in one CAS write.

use super::cidr::normalize_cidr;
use super::mesh_route_state::{
    revise_semantic, MeshBindingState, MeshIdentity, MeshIdentityEffect, MeshIdentityEffectStatus,
    MeshOwnerRef, MeshReservation, MeshReservationOwner, MeshRoutePolicy, MeshRouteState,
    MeshTarget,
};
use std::collections::BTreeSet;

/// Complete pre-mutation config CIDRs and last applied owner slices, supplied
/// by the trusted caller under the same config write lock/CAS snapshot. The
/// active slices must be verified against the old OwnerRef, plan and state
/// revision; an activePlan digest alone is not the slices. `Some(&[])` is valid
/// only after proving that source empty. `None` means proof is unavailable and
/// retirement must stop; UI or an old preview cannot assert completeness.
#[derive(Debug, Clone)]
pub struct RetirementScopeSnapshot<'a> {
    pub owner_ref: MeshOwnerRef,
    pub state_revision: &'a str,
    pub active_plan_id: Option<&'a str>,
    pub configured_cidrs: Option<&'a [String]>,
    pub active_plan_owner_cidrs: Option<&'a [String]>,
}

const MAX_RETIREMENT_SCOPE_CIDRS: usize = 4096;

/// A new local epoch is supplied by the trusted caller. An imported epoch is never used here.
#[derive(Debug, Clone, Copy)]
pub struct ReplacementIdentity<'a> {
    pub epoch: &'a str,
    pub control_authority: &'a str,
    pub self_stable_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledIdentityChange {
    /// A config edit or selective restore changes identity, without authorizing deletion.
    ConfigReplacement,
    /// The user explicitly logs out this identity.
    Logout,
    /// The user explicitly replaces this identity and requests old state cleanup.
    ExplicitReplacement,
}

/// An effect is a ledger instruction only. S4 must first prove the old main and
/// temporary owners released the state directory under its lifecycle/state gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateDirectoryOccupant<'a> {
    Empty,
    Epoch(&'a str),
    Unknown,
}

/// URL authority comparison only folds scheme and host case. Path, port,
/// query and trailing slash remain distinct so different control services
/// cannot accidentally share an epoch.
pub fn canonical_control_authority(raw: &str) -> Result<String, String> {
    if raw.trim() != raw || raw.chars().any(char::is_whitespace) {
        return Err("invalid mesh control authority".into());
    }
    let (scheme, remainder) = raw
        .split_once("://")
        .ok_or("mesh control authority needs URL scheme")?;
    if scheme.is_empty()
        || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return Err("invalid mesh control authority scheme".into());
    }
    let split = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let (authority, suffix) = remainder.split_at(split);
    if authority.is_empty() || authority.contains('@') || authority.starts_with(':') {
        return Err("invalid mesh control authority host".into());
    }
    Ok(format!(
        "{}://{}{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase(),
        suffix
    ))
}

fn current_identity<'a>(
    state: &'a MeshRouteState,
    server_id: &str,
) -> Result<Option<&'a MeshIdentity>, String> {
    let mut current = state.identities.iter().filter(|identity| {
        identity.server_id == server_id
            && matches!(
                identity.binding_state,
                MeshBindingState::Bound | MeshBindingState::Unbound
            )
    });
    let found = current.next();
    if current.next().is_some() {
        return Err("multiple current mesh identity epochs".into());
    }
    Ok(found)
}

fn checked_next(value: &str) -> Result<String, String> {
    value
        .parse::<u64>()
        .map_err(|_| "invalid mesh intent revision".to_owned())?
        .checked_add(1)
        .map(|next| next.to_string())
        .ok_or_else(|| "mesh intent revision exhausted".to_owned())
}

fn validate_replacement(
    state: &MeshRouteState,
    server_id: &str,
    replacement: ReplacementIdentity<'_>,
) -> Result<(), String> {
    if replacement.epoch.trim().is_empty()
        || replacement
            .self_stable_id
            .is_some_and(|value| value.trim().is_empty())
        || state.identities.iter().any(|identity| {
            identity.server_id == server_id && identity.identity_epoch == replacement.epoch
        })
    {
        return Err("invalid or reused mesh identity epoch/evidence".into());
    }
    canonical_control_authority(replacement.control_authority)?;
    Ok(())
}

fn add_replacement(
    state: &mut MeshRouteState,
    server_id: &str,
    replacement: ReplacementIdentity<'_>,
    source: &str,
) -> Result<(), String> {
    state.identities.push(MeshIdentity {
        server_id: server_id.into(),
        identity_epoch: replacement.epoch.into(),
        control_authority: canonical_control_authority(replacement.control_authority)?,
        self_stable_id: replacement.self_stable_id.map(str::to_owned),
        binding_state: MeshBindingState::Unbound,
        evidence_source: source.into(),
    });
    Ok(())
}

/// Reserve every known old-owner scope before retiring it. An unknown source
/// blocks the whole change; an empty slice explicitly proves no scope there.
fn reserve_old_scope(
    state: &mut MeshRouteState,
    policy: &MeshRoutePolicy,
    owner: &MeshOwnerRef,
    scope: RetirementScopeSnapshot<'_>,
) -> Result<(), String> {
    if scope.owner_ref != *owner
        || scope.state_revision != state.revision
        || scope.active_plan_id != state.active_plan.as_ref().map(|plan| plan.plan_id.as_str())
    {
        return Err("old mesh scope snapshot does not match owner/plan/revision".into());
    }
    let configured = scope
        .configured_cidrs
        .ok_or("old configured mesh scope is unknown")?;
    let active = scope
        .active_plan_owner_cidrs
        .ok_or("old active-plan mesh scope is unknown")?;
    let observed: Vec<&str> = state
        .observations
        .iter()
        .filter(|observation| observation.owner_ref == *owner)
        .flat_map(|observation| {
            observation
                .raw_hosts
                .iter()
                .chain(&observation.advertised_routes)
        })
        .map(String::as_str)
        .collect();
    let assigned: Vec<&str> = policy
        .assignments
        .iter()
        .filter_map(|assignment| {
            matches!(&assignment.target, MeshTarget::Owner { server_id, identity_epoch }
            if server_id == &owner.server_id && identity_epoch == &owner.identity_epoch)
            .then_some(assignment.cidr.as_str())
        })
        .collect();
    let overridden: Vec<&str> = policy
        .overrides
        .iter()
        .filter(|entry| {
            matches!(&entry.target, MeshTarget::Owner { server_id, identity_epoch }
            if server_id == &owner.server_id && identity_epoch == &owner.identity_epoch)
        })
        .flat_map(|entry| entry.scope_cidrs.iter().map(String::as_str))
        .collect();
    let reserved: Vec<String> = state.reservations.iter().filter(|reservation| {
        matches!(&reservation.owner_ref, MeshReservationOwner::Owner { server_id, identity_epoch }
            if server_id == &owner.server_id && identity_epoch == &owner.identity_epoch)
    }).map(|reservation| reservation.cidr.clone()).collect();
    let count = configured
        .len()
        .saturating_add(active.len())
        .saturating_add(observed.len())
        .saturating_add(assigned.len())
        .saturating_add(overridden.len())
        .saturating_add(reserved.len());
    if count > MAX_RETIREMENT_SCOPE_CIDRS {
        return Err("old mesh scope exceeds retirement CIDR budget".into());
    }
    let mut seen = BTreeSet::new();
    let mut add = |cidr: &str, origin: &str| -> Result<(), String> {
        let canonical = normalize_cidr(cidr).ok_or("invalid old mesh scope CIDR")?;
        if canonical.ends_with("/0") {
            return Err("old mesh scope contains an unsupported /0".into());
        }
        if !seen.insert(canonical.clone()) {
            return Ok(());
        }
        let owner_ref = MeshReservationOwner::Owner {
            server_id: owner.server_id.clone(),
            identity_epoch: owner.identity_epoch.clone(),
        };
        if !state.reservations.iter().any(|reservation| {
            normalize_cidr(&reservation.cidr).as_deref() == Some(canonical.as_str())
                && reservation.owner_ref == owner_ref
        }) {
            state.reservations.push(MeshReservation {
                cidr: canonical,
                owner_ref,
                origin: origin.into(),
            });
        }
        Ok(())
    };
    for cidr in reserved
        .iter()
        .map(String::as_str)
        .chain(observed)
        .chain(assigned)
        .chain(overridden)
    {
        add(cidr, "identityRetirement")?;
    }
    for cidr in configured {
        add(cidr, "configuredIdentityRetirement")?;
    }
    for cidr in active {
        add(cidr, "activePlanIdentityRetirement")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> (MeshRoutePolicy, MeshRouteState) {
        let wire: Value = serde_json::from_str(include_str!(
            "../../../../ui/src/contracts/mesh-route-state.fixture.json"
        ))
        .unwrap();
        let policy = serde_json::from_value(wire["meshRoutePolicy"].clone()).unwrap();
        let mut state: MeshRouteState =
            serde_json::from_value(wire["meshRouteState"].clone()).unwrap();
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
        assert!(check_retired_effect_target(
            &next,
            "effect-old-a",
            StateDirectoryOccupant::Unknown
        )
        .is_err());
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
}

fn retire(
    state: &mut MeshRouteState,
    policy: &MeshRoutePolicy,
    owner: &MeshOwnerRef,
    scope: RetirementScopeSnapshot<'_>,
) -> Result<(), String> {
    reserve_old_scope(state, policy, owner, scope)?;
    let identity = state
        .identities
        .iter_mut()
        .find(|identity| {
            identity.server_id == owner.server_id && identity.identity_epoch == owner.identity_epoch
        })
        .ok_or("mesh identity epoch changed")?;
    if !matches!(
        identity.binding_state,
        MeshBindingState::Bound | MeshBindingState::Unbound
    ) {
        return Err("mesh identity already retired".into());
    }
    identity.binding_state = MeshBindingState::Retired;
    state.intent.revision = checked_next(&state.intent.revision)?;
    Ok(())
}

/// Process a valid STATUS identity hint. Missing self stable ID is unknown, not
/// logout. A first stable ID enriches the same epoch. Only a changed known ID
/// within the same authority or an authority change retires the old epoch.
pub fn reconcile_identity_evidence(
    previous: &MeshRouteState,
    policy: &MeshRoutePolicy,
    server_id: &str,
    evidence: ReplacementIdentity<'_>,
    retirement_scope: Option<RetirementScopeSnapshot<'_>>,
) -> Result<Option<MeshRouteState>, String> {
    previous.validate()?;
    policy.validate()?;
    if server_id.trim().is_empty() {
        return Err("mesh serverId is empty".into());
    }
    let current = current_identity(previous, server_id)?;
    let authority = canonical_control_authority(evidence.control_authority)?;
    let changed = current.is_some_and(|identity| {
        canonical_control_authority(&identity.control_authority).as_deref()
            != Ok(authority.as_str())
            || matches!(
                (identity.self_stable_id.as_deref(), evidence.self_stable_id),
                (Some(old), Some(new)) if old != new
            )
    });
    if changed || current.is_none() {
        validate_replacement(previous, server_id, evidence)?;
    }
    let mut next = previous.clone();
    if let Some(current) = current {
        if changed {
            let owner = MeshOwnerRef {
                server_id: server_id.into(),
                identity_epoch: current.identity_epoch.clone(),
            };
            let scope = retirement_scope.ok_or("old mesh scope evidence is absent")?;
            retire(&mut next, policy, &owner, scope)?;
            add_replacement(&mut next, server_id, evidence, "status")?;
        } else if let Some(stable_id) = evidence.self_stable_id {
            if current.self_stable_id.is_none() {
                next.identities
                    .iter_mut()
                    .find(|identity| {
                        identity.server_id == server_id
                            && identity.identity_epoch == current.identity_epoch
                    })
                    .ok_or("mesh identity epoch changed")?
                    .self_stable_id = Some(stable_id.into());
            }
        }
    } else {
        add_replacement(&mut next, server_id, evidence, "status")?;
    }
    revise_semantic(previous, &previous.revision, next)
}

/// Trusted edit/restore/logout mutation. Only explicit logout or replacement
/// may create an old-epoch cleanup effect; config changes keep state on disk.
pub fn reconcile_controlled_identity(
    previous: &MeshRouteState,
    policy: &MeshRoutePolicy,
    owner: &MeshOwnerRef,
    replacement: Option<ReplacementIdentity<'_>>,
    cause: ControlledIdentityChange,
    effect_id: Option<&str>,
    retirement_scope: RetirementScopeSnapshot<'_>,
) -> Result<MeshRouteState, String> {
    previous.validate()?;
    policy.validate()?;
    let current = current_identity(previous, &owner.server_id)?.ok_or("mesh identity is absent")?;
    if current.identity_epoch != owner.identity_epoch {
        return Err("mesh identity epoch changed".into());
    }
    if cause == ControlledIdentityChange::Logout && replacement.is_some() {
        return Err("logout cannot bind replacement identity".into());
    }
    if cause == ControlledIdentityChange::ExplicitReplacement && replacement.is_none() {
        return Err("explicit replacement requires a new epoch".into());
    }
    if cause == ControlledIdentityChange::ConfigReplacement && effect_id.is_some() {
        return Err("config change cannot authorize state deletion".into());
    }
    if cause != ControlledIdentityChange::ConfigReplacement && effect_id.is_none() {
        return Err("explicit identity cleanup needs an effectId".into());
    }
    if let Some(replacement) = replacement {
        validate_replacement(previous, &owner.server_id, replacement)?;
    }
    if effect_id.is_some_and(|id| {
        id.trim().is_empty()
            || previous
                .identity_effects
                .iter()
                .any(|effect| effect.effect_id == id)
    }) {
        return Err("invalid or reused identity effectId".into());
    }
    let mut next = previous.clone();
    retire(&mut next, policy, owner, retirement_scope)?;
    if let Some(replacement) = replacement {
        add_replacement(&mut next, &owner.server_id, replacement, "controlledChange")?;
    }
    if let Some(effect_id) = effect_id {
        next.identity_effects.push(MeshIdentityEffect {
            effect_id: effect_id.into(),
            server_id: owner.server_id.clone(),
            retired_epoch: owner.identity_epoch.clone(),
            kind: "deleteState".into(),
            status: MeshIdentityEffectStatus::Pending,
        });
    }
    revise_semantic(previous, &previous.revision, next)?
        .ok_or_else(|| "identity change did not advance mesh revision".into())
}

/// Pure exact-target check. `Unknown` always blocks. The S4 caller must hold
/// the state gate and separately prove that no main or temporary owner runs.
pub fn check_retired_effect_target<'a>(
    state: &'a MeshRouteState,
    effect_id: &str,
    directory: StateDirectoryOccupant<'_>,
) -> Result<&'a MeshIdentityEffect, String> {
    state.validate()?;
    let effect = state
        .identity_effects
        .iter()
        .find(|effect| effect.effect_id == effect_id)
        .ok_or("mesh identity effect is absent")?;
    if effect.status != MeshIdentityEffectStatus::Pending || effect.kind != "deleteState" {
        return Err("mesh identity effect is not pending deletion".into());
    }
    let old = state
        .identities
        .iter()
        .find(|identity| {
            identity.server_id == effect.server_id
                && identity.identity_epoch == effect.retired_epoch
        })
        .ok_or("retired mesh identity epoch is absent")?;
    if !matches!(
        old.binding_state,
        MeshBindingState::Retiring | MeshBindingState::Retired
    ) {
        return Err("mesh identity effect does not name a retired epoch".into());
    }
    match directory {
        StateDirectoryOccupant::Empty => {}
        StateDirectoryOccupant::Epoch(epoch) if epoch == effect.retired_epoch => {}
        StateDirectoryOccupant::Epoch(_) | StateDirectoryOccupant::Unknown => {
            return Err("state directory is not proven to belong to retired epoch".into());
        }
    }
    if state.identities.iter().any(|identity| {
        identity.server_id == effect.server_id
            && identity.identity_epoch != effect.retired_epoch
            && identity.binding_state == MeshBindingState::Bound
    }) {
        return Err("new mesh identity already bound to state directory".into());
    }
    Ok(effect)
}

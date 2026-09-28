//! Strict managed-state boundary. The legacy sanitizer is intentionally permissive;
//! policy and local ledger must be checked on the *raw* JSON before it can run.

use polaris_config_engine::user_config::mesh_route_state::{
    MeshBindingState, MeshRoutePolicy, MeshRouteState,
};
use serde_json::Value;

use crate::StoreError;

pub const POLICY_KEY: &str = "meshRoutePolicy";
pub const STATE_KEY: &str = "meshRouteState";
pub const REQUIRED_MARKER_FILE: &str = "mesh-route-state.required";

fn server_by_id<'a>(config: &'a Value, id: &str) -> Result<Option<&'a Value>, StoreError> {
    let mut matches = config
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|server| server.get("id").and_then(Value::as_str) == Some(id));
    let found = matches.next();
    if matches.next().is_some() {
        return Err(StoreError::validation("duplicate managed mesh serverId"));
    }
    Ok(found)
}

/// Until S4 can prove the complete old owner plan under the config write lock,
/// ordinary writes must not change an active TS identity ahead of reservations.
/// No credential value is included in the diagnostic.
fn reject_unreconciled_identity_change(
    previous: &Value,
    incoming: &Value,
    state: &MeshRouteState,
) -> Result<(), StoreError> {
    const IDENTITY_SETTINGS: [&str; 4] = ["controlUrl", "sourceTag", "authKey", "ephemeral"];
    for identity in state.identities.iter().filter(|identity| {
        matches!(
            identity.binding_state,
            MeshBindingState::Bound | MeshBindingState::Unbound
        )
    }) {
        let old = server_by_id(previous, &identity.server_id)?;
        let new = server_by_id(incoming, &identity.server_id)?;
        if old.is_some() != new.is_some()
            || old.zip(new).is_some_and(|(old, new)| {
                old.get("protocol") != new.get("protocol")
                    || IDENTITY_SETTINGS.iter().any(|key| {
                        old.get("tailscaleSettings")
                            .and_then(|settings| settings.get(key))
                            != new
                                .get("tailscaleSettings")
                                .and_then(|settings| settings.get(key))
                    })
            })
        {
            return Err(StoreError::validation(
                "managed mesh identity edit needs a trusted retirement transaction with complete old scope",
            ));
        }
    }
    Ok(())
}

pub fn has_managed_fields(raw: &Value) -> bool {
    raw.get(POLICY_KEY).is_some() || raw.get(STATE_KEY).is_some()
}

/// Absence of *both* fields is legacy. One missing field, null, malformed or future
/// schema is a protected error; it must never sanitize to an empty managed plan.
pub fn validate_raw(raw: &Value) -> Result<bool, StoreError> {
    if !has_managed_fields(raw) {
        return Ok(false);
    }
    let policy: MeshRoutePolicy =
        serde_json::from_value(raw.get(POLICY_KEY).cloned().ok_or_else(|| {
            StoreError::validation("meshRoutePolicy missing from managed document")
        })?)
        .map_err(|error| StoreError::validation(format!("invalid meshRoutePolicy: {error}")))?;
    let state: MeshRouteState =
        serde_json::from_value(raw.get(STATE_KEY).cloned().ok_or_else(|| {
            StoreError::validation("meshRouteState missing from managed document")
        })?)
        .map_err(|error| StoreError::validation(format!("invalid meshRouteState: {error}")))?;
    policy.validate().map_err(StoreError::validation)?;
    state.validate().map_err(StoreError::validation)?;
    Ok(true)
}

/// All ordinary writers retain the exact disk ledger/policy. Missing fields from an
/// old frontend snapshot are restored; an explicit edit is rejected. A future trusted
/// backend mutation uses a separate CAS entry point rather than this route.
pub fn reconcile_untrusted(previous: &Value, incoming: &mut Value) -> Result<(), StoreError> {
    let previous_managed = validate_raw(previous)?;
    let incoming_managed = has_managed_fields(incoming);
    if !previous_managed {
        if incoming_managed {
            return Err(StoreError::validation(
                "mesh route state requires a trusted backend mutation",
            ));
        }
        return Ok(());
    }
    let Some(object) = incoming.as_object_mut() else {
        return Err(StoreError::validation("config root must be an object"));
    };
    for key in [POLICY_KEY, STATE_KEY] {
        let old = &previous[key];
        if object.get(key).is_some_and(|value| value != old) {
            return Err(StoreError::validation(format!(
                "{key} is backend protected; use a revision-checked mutation"
            )));
        }
        object.insert(key.into(), old.clone());
    }
    validate_raw(incoming)?;
    let state: MeshRouteState =
        serde_json::from_value(previous[STATE_KEY].clone()).map_err(StoreError::from_parse)?;
    reject_unreconciled_identity_change(previous, incoming, &state)?;
    Ok(())
}

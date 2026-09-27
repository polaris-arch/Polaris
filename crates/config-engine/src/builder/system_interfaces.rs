//! Runtime ownership checks on generated endpoints. Config-only `check` remains unrestricted.

use crate::singbox::Endpoint;
use serde_json::Value;

pub const INVALID_REASON_SYSTEM_INTERFACE_REQUIRES_HELPER: &str =
    "system-interface-requires-helper";

/// Inspect both representations: flattened extras can conflict with typed fields when serialized.
#[must_use]
pub fn endpoint_requests_system_interface(endpoint: &Endpoint) -> bool {
    endpoint.system == Some(true)
        || endpoint.system_interface == Some(true)
        || endpoint.extra.get("system").and_then(Value::as_bool) == Some(true)
        || endpoint
            .extra
            .get("system_interface")
            .and_then(Value::as_bool)
            == Some(true)
}

#[must_use]
pub fn raw_endpoint_requests_system_interface(endpoint: &Value) -> bool {
    endpoint.get("system").and_then(Value::as_bool) == Some(true)
        || endpoint.get("system_interface").and_then(Value::as_bool) == Some(true)
}

#[must_use]
pub fn system_interface_ownership_error(tags: &[String]) -> String {
    format!(
        "System interface requested by endpoint(s) [{}]. Use desktop TUN with the managed helper, or disable these nodes' system interfaces. Temporary speed tests cannot manage system interfaces; test through an already running managed desktop TUN core or disable the system interface.",
        tags.join(", ")
    )
}

/// A system endpoint must run in the existing managed helper path, never an ordinary child process.
pub fn ensure_managed_system_interfaces(
    endpoints: &[Endpoint],
    managed_helper: bool,
) -> Result<(), String> {
    if managed_helper {
        return Ok(());
    }
    let tags: Vec<String> = endpoints
        .iter()
        .filter(|endpoint| endpoint_requests_system_interface(endpoint))
        .map(|endpoint| endpoint.tag.clone())
        .collect();
    if tags.is_empty() {
        Ok(())
    } else {
        Err(system_interface_ownership_error(&tags))
    }
}

#[cfg(test)]
mod tests;

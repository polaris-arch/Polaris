//! Presentation-only cached account evidence, separate from directory ownership/cleanup.
//!
//! sagernet/tailscale a8fbeb4b0838: FileStore is a JSON map of base64-encoded byte values;
//! ipn.Prefs stores Persist as `Config`. Profiles are persisted only after NodeID and
//! UserProfile.LoginName are known. This cannot establish live or valid authentication.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownTailscaleSession;

impl std::fmt::Display for UnknownTailscaleSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("cannot verify cached Tailscale session")
    }
}

impl std::error::Error for UnknownTailscaleSession {}

/// Read-only FileStore parser. Errors never mean that state can safely be discarded.
/// No state contents, profile identity or credentials are included in the error.
pub fn cached_session_exists(bytes: &[u8]) -> Result<bool, UnknownTailscaleSession> {
    let document: Value = serde_json::from_slice(bytes).map_err(|_| UnknownTailscaleSession)?;
    let store = document.as_object().ok_or(UnknownTailscaleSession)?;
    let decoded = store
        .iter()
        .map(|(key, value)| Ok((key.as_str(), decode_value(value)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>, UnknownTailscaleSession>>()?;
    let profiles: Value = match decoded.get("_profiles") {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| UnknownTailscaleSession)?,
        None => Value::Object(Default::default()),
    };
    let profiles = profiles.as_object().ok_or(UnknownTailscaleSession)?;
    for (id, profile) in profiles {
        if id.is_empty()
            || profile.get("ID").and_then(Value::as_str) != Some(id.as_str())
            || profile.get("Key").and_then(Value::as_str).is_none()
        {
            return Err(UnknownTailscaleSession);
        }
    }
    // tsnet sets LocalBackendStartKeyOSNeutral: every platform selects _current-profile,
    // not Windows server-mode-start-key or per-user defaults. Historical profiles are not active.
    if let Some(bytes) = decoded.get("_current-profile") {
        let current = std::str::from_utf8(bytes).map_err(|_| UnknownTailscaleSession)?;
        if current.is_empty() {
            return Ok(false);
        }
        if !profiles
            .values()
            .any(|profile| profile.get("Key").and_then(Value::as_str) == Some(current))
        {
            return Err(UnknownTailscaleSession);
        }
        return profile_has_account(decoded.get(current).ok_or(UnknownTailscaleSession)?);
    }
    if !profiles.is_empty() {
        return Ok(false);
    }
    // With no current/known profiles, only recognized pre-login metadata proves absence.
    // Legacy migration differs by native platform; leave it Unknown rather than guessing an account.
    if decoded
        .keys()
        .any(|key| !matches!(*key, "_profiles" | "_machinekey" | "_taildrop-received"))
    {
        return Err(UnknownTailscaleSession);
    }
    Ok(false)
}

fn profile_has_account(bytes: &[u8]) -> Result<bool, UnknownTailscaleSession> {
    let prefs: Value = serde_json::from_slice(bytes).map_err(|_| UnknownTailscaleSession)?;
    let prefs = prefs.as_object().ok_or(UnknownTailscaleSession)?;
    let logged_out = match prefs.get("LoggedOut") {
        Some(Value::Bool(value)) => *value,
        None => false,
        Some(_) => return Err(UnknownTailscaleSession),
    };
    let config = prefs.get("Config").ok_or(UnknownTailscaleSession)?;
    if config.is_null() {
        return Ok(false);
    }
    let node = config
        .get("NodeID")
        .and_then(Value::as_str)
        .ok_or(UnknownTailscaleSession)?;
    let login = config
        .get("UserProfile")
        .and_then(|user| user.get("LoginName"))
        .and_then(Value::as_str)
        .ok_or(UnknownTailscaleSession)?;
    Ok(!logged_out && !node.is_empty() && !login.is_empty())
}

fn decode_value(value: &Value) -> Result<Vec<u8>, UnknownTailscaleSession> {
    let encoded = value.as_str().ok_or(UnknownTailscaleSession)?;
    let content = encoded.trim_end_matches('=');
    let padding = encoded.len() - content.len();
    if !encoded.len().is_multiple_of(4)
        || padding > 2
        || !content
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        return Err(UnknownTailscaleSession);
    }
    // The existing WARP decoder is deliberately tolerant; this boundary accepts only canonical
    // standard base64 written by Go encoding/json, including zero unused bits before padding.
    if padding > 0 {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let last = content.as_bytes().last().ok_or(UnknownTailscaleSession)?;
        let sextet = ALPHABET
            .iter()
            .position(|byte| byte == last)
            .ok_or(UnknownTailscaleSession)?;
        if sextet & ((1 << (padding * 2)) - 1) != 0 {
            return Err(UnknownTailscaleSession);
        }
    }
    crate::warp::base64_decode(encoded).ok_or(UnknownTailscaleSession)
}

#[cfg(test)]
mod tests;

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
    let document = parse_value(bytes)?;
    let store = document.as_object().ok_or(UnknownTailscaleSession)?;
    let decoded = store
        .iter()
        .map(|(key, value)| Ok((key.as_str(), decode_value(value)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>, UnknownTailscaleSession>>()?;
    let profiles: Value = match decoded.get("_profiles") {
        Some(bytes) => parse_value(bytes)?,
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

/// Read-only account presence after auth retirement. Absence grants no writer or cleanup authority.
/// Only a complete modern retired reference pair adds an absent result to the strict getter.
pub fn cached_session_exists_for_presentation(
    bytes: &[u8],
) -> Result<bool, UnknownTailscaleSession> {
    match cached_session_exists(bytes) {
        Err(_) if matches!(is_modern_retired_pair(bytes), Ok(true)) => Ok(false),
        result => result,
    }
}

/// Shared structural classifier only; the caller must separately admit any retirement operation.
pub(super) fn is_modern_retired_pair(bytes: &[u8]) -> Result<bool, UnknownTailscaleSession> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(UnknownTailscaleSession);
    }
    let document = parse_value(bytes)?;
    let store = document.as_object().ok_or(UnknownTailscaleSession)?;
    let decoded = store
        .iter()
        .map(|(key, value)| Ok((key.as_str(), decode_value(value)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>, UnknownTailscaleSession>>()?;
    let Some(current) = decoded.get("_current-profile") else {
        return Ok(false);
    };
    let current = std::str::from_utf8(current).map_err(|_| UnknownTailscaleSession)?;
    let modern_id = |id: &str| {
        id.len() == 4
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    };
    let Some(id) = current.strip_prefix("profile-").filter(|id| modern_id(id)) else {
        return Ok(false);
    };
    let profiles = parse_value(decoded.get("_profiles").ok_or(UnknownTailscaleSession)?)?;
    let profiles = profiles.as_object().ok_or(UnknownTailscaleSession)?;
    for (id, profile) in profiles {
        if !modern_id(id)
            || !profile.is_object()
            || profile.get("ID").and_then(Value::as_str) != Some(id.as_str())
            || profile.get("Key").and_then(Value::as_str) != Some(format!("profile-{id}").as_str())
        {
            return Err(UnknownTailscaleSession);
        }
    }
    Ok(!profiles.contains_key(id) && !decoded.contains_key(current))
}

fn profile_has_account(bytes: &[u8]) -> Result<bool, UnknownTailscaleSession> {
    let prefs = parse_value(bytes)?;
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

pub(super) fn decode_value(value: &Value) -> Result<Vec<u8>, UnknownTailscaleSession> {
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

/// Serde JSON grammar with duplicate object members rejected instead of silently overwritten.
/// The same byte boundary is used for presentation and the stricter auth projection.
pub(super) fn parse_value(bytes: &[u8]) -> Result<Value, UnknownTailscaleSession> {
    use serde::Deserialize;
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut parser).map_err(|_| UnknownTailscaleSession)?;
    parser.end().map_err(|_| UnknownTailscaleSession)?;
    Ok(value.0)
}

struct UniqueValue(Value);

impl<'de> serde::Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(parser: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object members")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<UniqueValue, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut members: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = members.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON member"));
                    }
                    values.insert(key, members.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        parser.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests;

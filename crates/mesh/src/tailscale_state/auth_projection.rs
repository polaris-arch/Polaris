//! Pure auth retirement projection for the pinned SDK's modern FileStore references.
//! This module grants no filesystem, writer-retirement or global owner authority.

use super::cached_session::{decode_value, is_modern_retired_pair, parse_value};
use super::UnknownTailscaleSession;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The caller's explicit provenance is separate from presentation-only cached account state.
#[derive(Clone, Copy)]
pub enum AuthProjectionProvenance<'a> {
    /// Original sealed SDK profile fingerprint. Recomputed from the actual file references.
    BoundFingerprint(&'a str),
    /// PC-only file selection after the caller's TS gate, in-use and transient-close admission.
    /// This does not assert that G or the SDK produced a Bound profile receipt.
    CurrentFileReferences,
}

/// Candidate file bytes only. The caller must revalidate the original byte revision before commit.
pub struct AuthStateProjection {
    pub bytes: Vec<u8>,
    pub changed: bool,
}

impl std::fmt::Debug for AuthStateProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthStateProjection")
            .field("bytes", &self.bytes.len())
            .field("changed", &self.changed)
            .finish()
    }
}

/// Remove only the referenced modern prefs key and its known-profile item.
/// Preserve the nonempty retired current pointer to prevent pinned SDK legacy migration.
/// A complete modern retired pair is an unchanged candidate only for CurrentFileReferences;
/// BoundFingerprint still requires an existing profile. Other missing or invalid references remain
/// Unknown, never broad cleanup. Neither candidate supplies the caller's runtime admission.
pub fn project_tailscale_auth_state(
    bytes: &[u8],
    provenance: AuthProjectionProvenance<'_>,
) -> Result<AuthStateProjection, UnknownTailscaleSession> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(UnknownTailscaleSession);
    }
    if matches!(provenance, AuthProjectionProvenance::CurrentFileReferences)
        && is_modern_retired_pair(bytes)?
    {
        return Ok(AuthStateProjection {
            bytes: bytes.to_vec(),
            changed: false,
        });
    }
    let mut document = parse_value(bytes)?;
    let store = document.as_object_mut().ok_or(UnknownTailscaleSession)?;
    // Validate every outer value, including unknown binary values, without interpreting them.
    let decoded = store
        .iter()
        .map(|(key, value)| Ok((key.as_str(), decode_value(value)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>, UnknownTailscaleSession>>()?;
    let current = std::str::from_utf8(
        decoded
            .get("_current-profile")
            .ok_or(UnknownTailscaleSession)?,
    )
    .map_err(|_| UnknownTailscaleSession)?;
    let mut profiles = parse_value(decoded.get("_profiles").ok_or(UnknownTailscaleSession)?)?;
    let profiles = profiles.as_object_mut().ok_or(UnknownTailscaleSession)?;
    let mut keys = BTreeSet::new();
    let mut active_id = None;
    for (id, profile) in profiles.iter() {
        let profile = profile.as_object().ok_or(UnknownTailscaleSession)?;
        let key = profile
            .get("Key")
            .and_then(Value::as_str)
            .ok_or(UnknownTailscaleSession)?;
        // a8fbeb4b0838 profiles.go:newUnusedID creates 2-byte lower-hex IDs and profile-ID keys.
        if id.len() != 4
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            || profile.get("ID").and_then(Value::as_str) != Some(id.as_str())
            || key != format!("profile-{id}")
            || !keys.insert(key)
        {
            return Err(UnknownTailscaleSession);
        }
        if key == current {
            active_id = Some(id.clone());
        }
    }
    let active_id = active_id.ok_or(UnknownTailscaleSession)?;
    // A modern reference must name an actual prefs object, not unrelated or legacy binary state.
    if !parse_value(decoded.get(current).ok_or(UnknownTailscaleSession)?)?.is_object() {
        return Err(UnknownTailscaleSession);
    }
    if let AuthProjectionProvenance::BoundFingerprint(expected) = provenance {
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"polaris-ts-profile-v1\0");
        fingerprint.update(active_id.as_bytes());
        fingerprint.update(b"\0");
        fingerprint.update(current.as_bytes());
        let actual: String = fingerprint
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != expected {
            return Err(UnknownTailscaleSession);
        }
    }
    let current = current.to_owned();
    profiles.remove(&active_id);
    let profile_bytes = serde_json::to_vec(profiles).map_err(|_| UnknownTailscaleSession)?;
    store.remove(&current);
    store.insert(
        "_profiles".into(),
        Value::String(base64_encode(&profile_bytes)),
    );
    let projected = serde_json::to_vec(&document).map_err(|_| UnknownTailscaleSession)?;
    Ok(AuthStateProjection {
        changed: projected != bytes,
        bytes: projected,
    })
}

// Reused verbatim from runtime/mesh.rs's standard encoder, without a second dependency.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 0x3f) as usize] as char);
        out.push(T[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests;

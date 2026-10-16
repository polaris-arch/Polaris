//! uTLS intent and effective overrides. Stored subscription nodes stay unchanged.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::normalize::normalize_token;
use super::protocol_settings::TlsSettings;
use super::server_config::{Protocol, SecurityMode, ServerConfig};
use crate::singbox::outbound::Utls;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UtlsMode {
    Default,
    Disabled,
    Profile,
}

/// Recognized JSON IDs. Runtime capability reports/checks decide actual support.
pub const SUPPORTED_PROFILES: &[&str] = &[
    "chrome",
    "chrome_155",
    "chrome_133",
    "firefox_148",
    "firefox_120",
    "safari_26_3",
    "safari_16_0",
    "firefox",
    "edge",
    "safari",
    "360",
    "qq",
    "ios",
    "android",
    "random",
    "randomized",
    "chrome_psk",
    "chrome_psk_shuffle",
    "chrome_padding_psk_shuffle",
    "chrome_pq",
    "chrome_pq_psk",
];

/// `None` mode preserves protocol-specific legacy defaults. Empty fingerprint
/// only means core default when explicitly enabled; it never means disabled.
pub fn node_utls(mode: Option<UtlsMode>, fingerprint: Option<&str>, legacy: &str) -> Option<Utls> {
    let fingerprint = match mode {
        Some(UtlsMode::Disabled) => return None,
        Some(UtlsMode::Default) => String::new(),
        Some(UtlsMode::Profile) => {
            normalize_token(fingerprint.unwrap_or_default()).unwrap_or_default()
        }
        None => normalize_token(fingerprint.unwrap_or(legacy)).unwrap_or_else(|| legacy.to_owned()),
    };
    (fingerprint != "none").then_some(Utls {
        enabled: true,
        fingerprint,
    })
}

/// Share links use `none` for disabled and `chrome` for the current core Auto
/// default. Never export a dormant fingerprint that would re-enable uTLS.
pub fn share_fingerprint(mode: Option<UtlsMode>, fp: Option<&str>) -> Option<String> {
    match mode {
        Some(UtlsMode::Disabled) => Some("none".into()),
        Some(UtlsMode::Default) => Some("chrome".into()),
        Some(UtlsMode::Profile) | None => fp.and_then(normalize_token),
    }
}

fn validate(
    mode: Option<UtlsMode>,
    fp: Option<&str>,
    legacy: &str,
) -> Result<Option<Utls>, &'static str> {
    if mode == Some(UtlsMode::Profile) && fp.and_then(normalize_token).is_none() {
        return Err("Choose a uTLS fingerprint or the core default.");
    }
    if mode == Some(UtlsMode::Profile) && fp.and_then(normalize_token).as_deref() == Some("none") {
        return Err("This uTLS fingerprint is not supported by the bundled core.");
    }
    let value = node_utls(mode, fp, legacy);
    if value.as_ref().is_some_and(|u| {
        !u.fingerprint.is_empty() && !SUPPORTED_PROFILES.contains(&u.fingerprint.as_str())
    }) {
        return Err("This uTLS fingerprint is not supported by the bundled core.");
    }
    Ok(value)
}

/// The only policy application used by main generation, temporary measurement
/// and connection digests. It does not recursively edit custom/QUIC/Cronet TLS.
pub fn effective_server<'a>(
    server: &'a ServerConfig,
    policy: Option<&str>,
) -> Result<Cow<'a, ServerConfig>, &'static str> {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    effective_server_for_platform(server, policy, platform)
}

/// Validate against the same platform gate used by actual outbound generation.
/// A dormant foreign-platform engine must not change legacy follow semantics.
pub fn effective_server_for_platform<'a>(
    server: &'a ServerConfig,
    policy: Option<&str>,
    platform: &str,
) -> Result<Cow<'a, ServerConfig>, &'static str> {
    let policy = policy
        .and_then(normalize_token)
        .unwrap_or_else(|| "follow".into());
    if policy != "follow" && policy != "default" && !SUPPORTED_PROFILES.contains(&policy.as_str()) {
        return Err("The global uTLS policy is not supported by the bundled core.");
    }
    let ordinary = matches!(
        server.protocol,
        Protocol::Vless | Protocol::Vmess | Protocol::Trojan | Protocol::Anytls | Protocol::Http
    ) && (matches!(server.protocol, Protocol::Trojan | Protocol::Anytls)
        || server.tls_settings.is_some()
        || server
            .security
            .as_ref()
            .is_some_and(|s| s.is_tls() || s.is_reality()));
    let shadow = server.shadow_tls_settings.is_some();
    if !ordinary && !shadow {
        return Ok(Cow::Borrowed(server));
    }

    let mut effective = Cow::Borrowed(server);
    if policy != "follow" {
        let node = effective.to_mut();
        let mode = if policy == "default" {
            UtlsMode::Default
        } else {
            UtlsMode::Profile
        };
        if ordinary {
            let tls = node.tls_settings.get_or_insert_with(TlsSettings::default);
            tls.utls_mode = Some(mode);
            tls.fingerprint = (policy != "default").then(|| policy.clone());
        }
        if let Some(tls) = node.shadow_tls_settings.as_mut() {
            tls.utls_mode = Some(mode);
            tls.fingerprint = (policy != "default").then(|| policy.clone());
        }
    }
    if ordinary {
        let tls = effective.tls_settings.as_ref();
        let legacy = if matches!(server.protocol, Protocol::Vless | Protocol::Anytls)
            || server
                .security
                .as_ref()
                .is_some_and(SecurityMode::is_reality)
        {
            "chrome"
        } else {
            "none"
        };
        let utls = validate(
            tls.and_then(|t| t.utls_mode),
            tls.and_then(|t| t.fingerprint.as_deref()),
            legacy,
        )?;
        let reality = server
            .security
            .as_ref()
            .is_some_and(SecurityMode::is_reality);
        if reality && utls.is_none() {
            return Err(
                "REALITY requires uTLS; choose the core default or a supported fingerprint.",
            );
        }
        let engine = tls.and_then(|t| t.engine.as_deref());
        // Reality always uses the Go implementation in the existing builder.
        // Preserve its dormant engine field on legacy round trips; only an
        // engine actually emitted for ordinary TLS can conflict with uTLS.
        if !reality
            && crate::builder::outbound_helpers::should_emit_tls_engine(engine, platform)
            && utls.is_some()
        {
            return Err("uTLS requires the Go TLS engine; disable uTLS or select Go TLS.");
        }
    }
    if let Some(tls) = &effective.shadow_tls_settings {
        if validate(tls.utls_mode, tls.fingerprint.as_deref(), "chrome")?.is_none() {
            return Err("Disabling uTLS for ShadowTLS is not supported by the bundled core.");
        }
    }
    Ok(effective)
}

#[cfg(test)]
mod tests;

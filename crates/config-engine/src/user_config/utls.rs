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
pub const REALITY_UTLS_REQUIRED: &str =
    "REALITY requires uTLS; choose Default or a supported fingerprint.";

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

/// Missing intent follows the source fingerprint or the capability-aware
/// fallback supplied by the caller. Explicit disabled intent always wins.
/// `UtlsMode::Default` retains the historical explicitly enabled,
/// empty-fingerprint (core default) meaning.
pub fn node_utls(
    mode: Option<UtlsMode>,
    fingerprint: Option<&str>,
    fallback: &str,
) -> Option<Utls> {
    let fingerprint = match mode {
        Some(UtlsMode::Disabled) => return None,
        Some(UtlsMode::Default) => String::new(),
        Some(UtlsMode::Profile) => {
            normalize_token(fingerprint.unwrap_or_default()).unwrap_or_default()
        }
        None => {
            normalize_token(fingerprint.unwrap_or(fallback)).unwrap_or_else(|| fallback.to_owned())
        }
    };
    (fingerprint != "none").then_some(Utls {
        enabled: true,
        fingerprint,
    })
}

/// Explicit off wins over global profiles. A dormant fingerprint on the
/// historical core-default mode is ignored, just as in `node_utls`.
fn explicitly_disabled(mode: Option<UtlsMode>, fingerprint: Option<&str>) -> bool {
    mode == Some(UtlsMode::Disabled)
        || (mode != Some(UtlsMode::Default)
            && fingerprint.and_then(normalize_token).as_deref() == Some("none"))
}

/// Default uTLS is Chrome only for the ordinary TLS protocols supported by
/// this app. An actually emitted native TLS engine cannot use uTLS; foreign
/// or dormant engine values follow the same platform gate as the builder.
pub fn ordinary_fallback(server: &ServerConfig, platform: &str) -> &'static str {
    if matches!(
        server.protocol,
        Protocol::Vless | Protocol::Vmess | Protocol::Trojan | Protocol::Anytls | Protocol::Http
    ) && !crate::builder::outbound_helpers::should_emit_tls_engine(
        server
            .tls_settings
            .as_ref()
            .and_then(|tls| tls.engine.as_deref()),
        platform,
    ) {
        "chrome"
    } else {
        "none"
    }
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
        return Err("Choose Default or a uTLS fingerprint.");
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

    let reality = server
        .security
        .as_ref()
        .is_some_and(SecurityMode::is_reality);
    let node_tls = server.tls_settings.as_ref();
    let override_ordinary = ordinary
        && (reality || ordinary_fallback(server, platform) == "chrome")
        && !explicitly_disabled(
            node_tls.and_then(|tls| tls.utls_mode),
            node_tls.and_then(|tls| tls.fingerprint.as_deref()),
        );
    let override_shadow = server
        .shadow_tls_settings
        .as_ref()
        .is_some_and(|tls| !explicitly_disabled(tls.utls_mode, tls.fingerprint.as_deref()));
    let mut effective = Cow::Borrowed(server);
    if policy != "follow" && (override_ordinary || override_shadow) {
        let node = effective.to_mut();
        // Global Default is explicitly Chrome. Historical node-level
        // UtlsMode::Default remains the separate saved core-default intent.
        let fingerprint = if policy == "default" {
            "chrome"
        } else {
            &policy
        };
        if override_ordinary {
            let tls = node.tls_settings.get_or_insert_with(TlsSettings::default);
            tls.utls_mode = Some(UtlsMode::Profile);
            tls.fingerprint = Some(fingerprint.to_owned());
        }
        if override_shadow {
            if let Some(tls) = node.shadow_tls_settings.as_mut() {
                tls.utls_mode = Some(UtlsMode::Profile);
                tls.fingerprint = Some(fingerprint.to_owned());
            }
        }
    }
    if ordinary {
        let tls = effective.tls_settings.as_ref();
        let fallback = if reality {
            "chrome"
        } else {
            ordinary_fallback(&effective, platform)
        };
        // Historical profile=none has the same REALITY conflict as explicit disabled.
        if reality
            && node_utls(
                tls.and_then(|t| t.utls_mode),
                tls.and_then(|t| t.fingerprint.as_deref()),
                fallback,
            )
            .is_none()
        {
            return Err(REALITY_UTLS_REQUIRED);
        }
        let utls = validate(
            tls.and_then(|t| t.utls_mode),
            tls.and_then(|t| t.fingerprint.as_deref()),
            fallback,
        )?;
        if reality && utls.is_none() {
            return Err(REALITY_UTLS_REQUIRED);
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
        // ShadowTLS keeps TLS enabled; typed disabled / link fp=none only
        // omits uTLS and uses the kernel's standard-TLS fallback.
        validate(tls.utls_mode, tls.fingerprint.as_deref(), "chrome")?;
    }
    Ok(effective)
}

#[cfg(test)]
mod tests;

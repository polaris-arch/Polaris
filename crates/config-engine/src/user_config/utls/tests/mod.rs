use super::*;
use crate::builder::orchestration::config_generation_norm;
use crate::builder::outbound::build_proxy_outbound;
use crate::singbox::dns::DomainResolver;
use crate::user_config::UserConfig;
use serde_json::{json, Value};

fn server(protocol: Protocol) -> ServerConfig {
    ServerConfig {
        id: "node".into(),
        name: "node".into(),
        protocol,
        address: "example.com".into(),
        port: 443,
        security: Some(SecurityMode::Tls),
        ..Default::default()
    }
}

fn generated(node: &ServerConfig, policy: Option<&str>) -> Value {
    let effective = effective_server(node, policy).unwrap();
    serde_json::to_value(build_proxy_outbound(
        effective.as_ref(),
        "node",
        &DomainResolver::Tag("dns-bootstrap".into()),
        "x64",
        "linux",
    ))
    .unwrap()
}

#[test]
fn node_intent_global_policy_protocol_matrix_keeps_stored_nodes_unchanged() {
    for protocol in [
        Protocol::Vless,
        Protocol::Vmess,
        Protocol::Trojan,
        Protocol::Anytls,
        Protocol::Http,
    ] {
        for mode in [
            None,
            Some(UtlsMode::Default),
            Some(UtlsMode::Disabled),
            Some(UtlsMode::Profile),
        ] {
            let mut node = server(protocol);
            node.tls_settings = Some(TlsSettings {
                utls_mode: mode,
                fingerprint: (mode == Some(UtlsMode::Profile)).then(|| "firefox".into()),
                ..Default::default()
            });
            let original = serde_json::to_value(&node).unwrap();
            for policy in [
                None,
                Some("follow"),
                Some("default"),
                Some("random"),
                Some("safari"),
            ] {
                let value = generated(&node, policy);
                assert_eq!(value["tls"]["enabled"], true, "disabling uTLS keeps TLS");
                let expected = match policy {
                    Some("default") => Some(""),
                    Some("random") => Some("random"),
                    Some("safari") => Some("safari"),
                    _ => match mode {
                        Some(UtlsMode::Default) => Some(""),
                        Some(UtlsMode::Disabled) => None,
                        Some(UtlsMode::Profile) => Some("firefox"),
                        None if matches!(protocol, Protocol::Vless | Protocol::Anytls) => {
                            Some("chrome")
                        }
                        None => None,
                    },
                };
                match expected {
                    Some(profile) => {
                        assert_eq!(value["tls"]["utls"]["enabled"], true);
                        assert_eq!(value["tls"]["utls"]["fingerprint"], profile);
                    }
                    None => assert!(value["tls"].get("utls").is_none()),
                }
                assert_eq!(serde_json::to_value(&node).unwrap(), original);
            }
        }
    }
}

#[test]
fn core_default_and_disabled_do_not_confuse_dormant_fingerprints() {
    let mut node = server(Protocol::Vmess);
    node.tls_settings = Some(TlsSettings {
        utls_mode: Some(UtlsMode::Disabled),
        fingerprint: Some("chrome".into()),
        ..Default::default()
    });
    assert!(generated(&node, None)["tls"].get("utls").is_none());
    assert_eq!(
        share_fingerprint(Some(UtlsMode::Disabled), Some("chrome")).as_deref(),
        Some("none")
    );
    node.tls_settings.as_mut().unwrap().utls_mode = Some(UtlsMode::Default);
    assert_eq!(
        generated(&node, None)["tls"]["utls"],
        json!({"enabled":true,"fingerprint":""})
    );
}

#[test]
fn reality_and_native_engine_conflicts_are_explicit() {
    let mut node = server(Protocol::Vless);
    node.security = Some(SecurityMode::Reality);
    node.tls_settings = Some(TlsSettings {
        utls_mode: Some(UtlsMode::Disabled),
        ..Default::default()
    });
    assert_eq!(
        effective_server(&node, None).unwrap_err(),
        "REALITY requires uTLS; choose the core default or a supported fingerprint."
    );
    node.security = Some(SecurityMode::Tls);
    node.tls_settings.as_mut().unwrap().engine = Some("apple".into());
    assert!(
        effective_server(&node, None).is_ok(),
        "native TLS without uTLS is supported"
    );
    assert_eq!(
        effective_server_for_platform(&node, Some("chrome"), "darwin").unwrap_err(),
        "uTLS requires the Go TLS engine; disable uTLS or select Go TLS."
    );
}

#[test]
fn unsupported_profiles_are_retained_but_cannot_be_enabled() {
    let mut node = server(Protocol::Vless);
    node.tls_settings = Some(TlsSettings {
        fingerprint: Some("polaris_unknown_fingerprint".into()),
        ..Default::default()
    });
    let before = serde_json::to_value(&node).unwrap();
    assert_eq!(
        effective_server(&node, None).unwrap_err(),
        "This uTLS fingerprint is not supported by the bundled core."
    );
    assert_eq!(serde_json::to_value(&node).unwrap(), before);
    for profile in SUPPORTED_PROFILES {
        assert!(effective_server(&server(Protocol::Vless), Some(profile)).is_ok());
    }
    node.tls_settings.as_mut().unwrap().fingerprint = Some("safari_26_3".into());
    assert!(
        effective_server(&node, None).is_ok(),
        "Safari 26.3 is a supported positive case"
    );
}

#[test]
fn global_policy_does_not_rewrite_quic_cronet_or_custom_nodes() {
    for protocol in [
        Protocol::Hysteria,
        Protocol::Hysteria2,
        Protocol::Tuic,
        Protocol::Naive,
        Protocol::MasqueClient,
        Protocol::Custom,
    ] {
        let node = server(protocol);
        assert!(matches!(
            effective_server(&node, Some("random")).unwrap(),
            Cow::Borrowed(_)
        ));
    }
}

#[test]
fn policy_projection_enters_refresh_and_missing_is_follow() {
    let base = UserConfig {
        servers: vec![server(Protocol::Vless)],
        ..Default::default()
    };
    let mut changed = base.clone();
    changed.utls_policy = Some("follow".into());
    assert_eq!(
        config_generation_norm(&base, None),
        config_generation_norm(&changed, None)
    );
    changed.utls_policy = Some("default".into());
    assert_ne!(
        config_generation_norm(&base, None),
        config_generation_norm(&changed, None)
    );
    changed.utls_policy = Some("random".into());
    assert_ne!(
        config_generation_norm(&base, None),
        config_generation_norm(&changed, None)
    );
}

#[test]
fn legacy_reality_dormant_engine_is_preserved_without_emitting_a_native_engine() {
    let mut node = server(Protocol::Vless);
    node.security = Some(SecurityMode::Reality);
    node.reality_settings = Some(crate::user_config::protocol_settings::RealitySettings {
        public_key: "key".into(),
        ..Default::default()
    });
    node.tls_settings = Some(TlsSettings {
        engine: Some("apple".into()),
        ..Default::default()
    });
    let original = serde_json::to_value(&node).unwrap();
    let effective = effective_server_for_platform(&node, None, "darwin").unwrap();
    let output = serde_json::to_value(build_proxy_outbound(
        effective.as_ref(),
        "node",
        &DomainResolver::Tag("dns-bootstrap".into()),
        "x64",
        "darwin",
    ))
    .unwrap();
    assert!(output["tls"].get("engine").is_none());
    assert_eq!(output["tls"]["utls"]["fingerprint"], "chrome");
    assert_eq!(serde_json::to_value(&node).unwrap(), original);
}

#[test]
fn legacy_dormant_engine_case_uses_the_exact_existing_emission_gate() {
    let mut node = server(Protocol::Vless);
    node.tls_settings = Some(TlsSettings {
        engine: Some("APPLE".into()),
        ..Default::default()
    });
    let effective = effective_server_for_platform(&node, None, "darwin").unwrap();
    let output = serde_json::to_value(build_proxy_outbound(
        effective.as_ref(),
        "node",
        &DomainResolver::Tag("dns-bootstrap".into()),
        "x64",
        "darwin",
    ))
    .unwrap();
    assert!(output["tls"].get("engine").is_none());
    assert_eq!(output["tls"]["utls"]["fingerprint"], "chrome");
}

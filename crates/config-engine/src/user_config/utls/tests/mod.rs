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
    serde_json::to_value(
        build_proxy_outbound(
            effective.as_ref(),
            "node",
            &DomainResolver::Tag("dns-bootstrap".into()),
            "x64",
            "linux",
        )
        .expect("valid test outbound"),
    )
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
                let expected = match (mode, policy) {
                    (Some(UtlsMode::Disabled), _) => None,
                    (_, None | Some("default")) => Some("chrome"),
                    (_, Some("random")) => Some("random"),
                    (_, Some("safari")) => Some("safari"),
                    _ => match mode {
                        Some(UtlsMode::Default) => Some(""),
                        Some(UtlsMode::Disabled) => None,
                        Some(UtlsMode::Profile) => Some("firefox"),
                        None => Some("chrome"),
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
        generated(&node, Some("follow"))["tls"]["utls"],
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
        "REALITY requires uTLS; choose Default or a supported fingerprint."
    );
    node.security = Some(SecurityMode::Tls);
    node.tls_settings.as_mut().unwrap().engine = Some("apple".into());
    assert!(
        effective_server(&node, None).is_ok(),
        "native TLS without uTLS is supported"
    );
    // Global profiles cannot override explicit off, even on native TLS.
    assert!(effective_server_for_platform(&node, Some("chrome"), "darwin").is_ok());
    node.tls_settings.as_mut().unwrap().utls_mode = Some(UtlsMode::Profile);
    node.tls_settings.as_mut().unwrap().fingerprint = Some("chrome".into());
    assert_eq!(
        effective_server_for_platform(&node, Some("follow"), "darwin").unwrap_err(),
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
        effective_server(&node, Some("follow")).unwrap_err(),
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
fn policy_projection_enters_refresh_and_missing_is_default() {
    let base = UserConfig {
        servers: vec![server(Protocol::Vless)],
        ..Default::default()
    };
    let mut changed = base.clone();
    changed.utls_policy = Some("follow".into());
    assert_ne!(
        config_generation_norm(&base, None),
        config_generation_norm(&changed, None)
    );
    changed.utls_policy = None;
    assert_eq!(
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
    let output = serde_json::to_value(
        build_proxy_outbound(
            effective.as_ref(),
            "node",
            &DomainResolver::Tag("dns-bootstrap".into()),
            "x64",
            "darwin",
        )
        .expect("valid test outbound"),
    )
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
    let output = serde_json::to_value(
        build_proxy_outbound(
            effective.as_ref(),
            "node",
            &DomainResolver::Tag("dns-bootstrap".into()),
            "x64",
            "darwin",
        )
        .expect("valid test outbound"),
    )
    .unwrap();
    assert!(output["tls"].get("engine").is_none());
    assert_eq!(output["tls"]["utls"]["fingerprint"], "chrome");
}

#[test]
fn ordinary_source_defaults_and_historical_profiles_survive_save_and_follow() {
    for protocol in [
        Protocol::Vless,
        Protocol::Vmess,
        Protocol::Trojan,
        Protocol::Anytls,
        Protocol::Http,
    ] {
        for fingerprint in [
            None,
            Some(""),
            Some("none"),
            Some("chrome"),
            Some("randomized"),
        ] {
            let mut node = server(protocol);
            node.tls_settings = Some(TlsSettings {
                fingerprint: fingerprint.map(str::to_owned),
                ..Default::default()
            });
            let saved: ServerConfig =
                serde_json::from_value(serde_json::to_value(&node).unwrap()).unwrap();
            let original = serde_json::to_value(&saved).unwrap();
            for policy in [Some("follow"), Some(" FOLLOW ")] {
                assert!(matches!(
                    effective_server(&saved, policy).unwrap(),
                    Cow::Borrowed(_)
                ));
                let output = generated(&saved, policy);
                assert_eq!(output["tls"]["enabled"], true);
                if fingerprint == Some("none") {
                    assert!(output["tls"].get("utls").is_none());
                } else {
                    let expected = fingerprint.filter(|fp| !fp.is_empty()).unwrap_or("chrome");
                    assert_eq!(
                        output["tls"]["utls"]["fingerprint"], expected,
                        "{protocol:?}"
                    );
                }
                assert_eq!(serde_json::to_value(&saved).unwrap(), original);
            }
        }
        // A missing settings object also defaults at generation time only.
        let node = server(protocol);
        assert_eq!(
            generated(&node, Some("follow"))["tls"]["utls"]["fingerprint"],
            "chrome"
        );
    }
}

#[test]
fn shadow_source_intent_and_global_overrides_preserve_stored_values() {
    use crate::builder::outbounds::build_shadow_tls_outbound;
    use crate::user_config::protocol_settings::ShadowTlsSettings;
    for (mode, fingerprint, expected) in [
        (None, None, Some("chrome")),
        (None, Some(""), Some("chrome")),
        (None, Some("none"), None),
        (None, Some("safari"), Some("safari")),
        (None, Some("randomized"), Some("randomized")),
        (Some(UtlsMode::Disabled), Some("chrome"), None),
        (Some(UtlsMode::Default), Some("safari"), Some("")),
        (Some(UtlsMode::Profile), Some("firefox"), Some("firefox")),
    ] {
        let mut node = server(Protocol::Shadowsocks);
        node.security = None;
        node.shadow_tls_settings = Some(ShadowTlsSettings {
            password: "secret".into(),
            sni: "example.com".into(),
            utls_mode: mode,
            fingerprint: fingerprint.map(str::to_owned),
            ..Default::default()
        });
        let saved: ServerConfig =
            serde_json::from_value(serde_json::to_value(&node).unwrap()).unwrap();
        let original = serde_json::to_value(&saved).unwrap();
        for policy in [
            None,
            Some("follow"),
            Some("default"),
            Some("randomized"),
            Some("firefox"),
        ] {
            let effective = effective_server(&saved, policy).unwrap();
            let output =
                serde_json::to_value(build_shadow_tls_outbound(&effective, None).unwrap()).unwrap();
            assert_eq!(output["tls"]["enabled"], true);
            let expected = if mode == Some(UtlsMode::Disabled) || fingerprint == Some("none") {
                None
            } else {
                match policy {
                    None | Some("default") => Some("chrome"),
                    Some("randomized") => Some("randomized"),
                    Some("firefox") => Some("firefox"),
                    _ => expected,
                }
            };
            match expected {
                Some(profile) => assert_eq!(
                    output["tls"]["utls"],
                    json!({"enabled":true,"fingerprint":profile})
                ),
                None => assert!(output["tls"].get("utls").is_none()),
            }
            assert_eq!(serde_json::to_value(&saved).unwrap(), original);
        }
    }
}

#[test]
fn reality_defaults_to_chrome_without_overriding_source_intent() {
    use crate::user_config::protocol_settings::RealitySettings;
    for protocol in [
        Protocol::Vless,
        Protocol::Vmess,
        Protocol::Trojan,
        Protocol::Anytls,
        Protocol::Http,
    ] {
        for (mode, fingerprint, expected) in [
            (None, None, Some("chrome")),
            (None, Some(""), Some("chrome")),
            (None, Some("firefox"), Some("firefox")),
            (None, Some("randomized"), Some("randomized")),
            (Some(UtlsMode::Default), None, Some("")),
            (Some(UtlsMode::Disabled), Some("chrome"), None),
            (None, Some("none"), None),
        ] {
            let mut node = server(protocol);
            node.security = Some(SecurityMode::Reality);
            node.reality_settings = Some(RealitySettings {
                public_key: "key".into(),
                ..Default::default()
            });
            node.tls_settings = Some(TlsSettings {
                utls_mode: mode,
                fingerprint: fingerprint.map(str::to_owned),
                ..Default::default()
            });
            let original = serde_json::to_value(&node).unwrap();
            if let Some(profile) = expected {
                let output = generated(&node, Some("follow"));
                assert_eq!(output["tls"]["reality"]["enabled"], true);
                assert_eq!(
                    output["tls"]["utls"],
                    json!({"enabled":true,"fingerprint":profile})
                );
            } else {
                assert_eq!(
                    effective_server(&node, Some("follow")).unwrap_err(),
                    REALITY_UTLS_REQUIRED
                );
            }
            assert_eq!(serde_json::to_value(&node).unwrap(), original);
        }
        let mut node = server(protocol);
        node.security = Some(SecurityMode::Reality);
        node.reality_settings = Some(RealitySettings {
            public_key: "key".into(),
            ..Default::default()
        });
        assert_eq!(
            generated(&node, Some("follow"))["tls"]["utls"]["fingerprint"],
            "chrome"
        );
        assert!(node.tls_settings.is_none());
    }
}

#[test]
fn explicit_off_survives_every_global_policy_and_reality_stays_invalid() {
    for protocol in [
        Protocol::Vless,
        Protocol::Vmess,
        Protocol::Trojan,
        Protocol::Anytls,
        Protocol::Http,
    ] {
        for (mode, fingerprint) in [
            (Some(UtlsMode::Disabled), Some("safari")),
            (None, Some(" NoNe ")),
        ] {
            let mut node = server(protocol);
            node.tls_settings = Some(TlsSettings {
                utls_mode: mode,
                fingerprint: fingerprint.map(str::to_owned),
                ..Default::default()
            });
            for policy in [
                None,
                Some("follow"),
                Some("default"),
                Some("random"),
                Some("randomized"),
                Some("chrome"),
                Some("firefox"),
            ] {
                let before = serde_json::to_value(&node).unwrap();
                let output = generated(&node, policy);
                assert_eq!(output["tls"]["enabled"], true);
                assert!(output["tls"].get("utls").is_none());
                assert_eq!(serde_json::to_value(&node).unwrap(), before);
                node.security = Some(SecurityMode::Reality);
                assert_eq!(
                    effective_server(&node, policy).unwrap_err(),
                    REALITY_UTLS_REQUIRED
                );
                node.security = Some(SecurityMode::Tls);
            }
        }
    }
}

#[test]
fn default_and_global_profiles_skip_native_engines_but_explicit_node_conflicts_fail() {
    for protocol in [
        Protocol::Vless,
        Protocol::Vmess,
        Protocol::Trojan,
        Protocol::Anytls,
        Protocol::Http,
    ] {
        for (engine, platform, active) in [
            ("apple", "darwin", true),
            ("windows", "win32", true),
            ("apple", "linux", false),
            ("windows", "darwin", false),
            ("apple", "ios", false),
            ("APPLE", "darwin", false),
        ] {
            let mut node = server(protocol);
            node.tls_settings = Some(TlsSettings {
                engine: Some(engine.into()),
                ..Default::default()
            });
            let original = serde_json::to_value(&node).unwrap();
            for (policy, profile) in [
                (None, "chrome"),
                (Some("follow"), "chrome"),
                (Some("default"), "chrome"),
                (Some("random"), "random"),
                (Some("firefox"), "firefox"),
            ] {
                let effective = effective_server_for_platform(&node, policy, platform).unwrap();
                let output = serde_json::to_value(
                    build_proxy_outbound(
                        &effective,
                        "node",
                        &DomainResolver::Tag("dns-bootstrap".into()),
                        "x64",
                        platform,
                    )
                    .expect("valid test outbound"),
                )
                .unwrap();
                if active {
                    assert!(matches!(effective, Cow::Borrowed(_)));
                    assert_eq!(output["tls"]["engine"], engine);
                    assert!(output["tls"].get("utls").is_none());
                } else {
                    assert!(output["tls"].get("engine").is_none());
                    assert_eq!(output["tls"]["utls"]["fingerprint"], profile);
                }
                assert_eq!(serde_json::to_value(&node).unwrap(), original);
            }
            if active {
                for profile in ["chrome", "random"] {
                    node.tls_settings.as_mut().unwrap().fingerprint = Some(profile.into());
                    for policy in [
                        None,
                        Some("follow"),
                        Some("default"),
                        Some("random"),
                        Some("safari"),
                    ] {
                        assert_eq!(
                            effective_server_for_platform(&node, policy, platform).unwrap_err(),
                            "uTLS requires the Go TLS engine; disable uTLS or select Go TLS."
                        );
                    }
                }
            }
        }
    }
}

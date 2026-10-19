use super::*;

#[test]
fn https_and_naive_export_preserve_distinct_tls_identity() {
    for protocol in [Protocol::Http, Protocol::Naive] {
        let node = ServerConfig {
            name: "fixture".into(),
            protocol,
            address: "192.0.2.1".into(),
            port: 443,
            username: Some("u".into()),
            password: Some("pw".into()),
            security: Some(SecurityMode::Tls),
            tls_settings: Some(TlsSettings {
                server_name: Some("certificate.example".into()),
                allow_insecure: Some(true),
                alpn: Some(vec!["h2".into()]),
                fingerprint: Some("chrome".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let roundtripped = roundtrip(&node);
        let tls = roundtripped.tls_settings.as_ref().unwrap();
        assert_eq!(tls.server_name.as_deref(), Some("certificate.example"));
        assert_eq!(tls.allow_insecure, Some(true));
        assert_eq!(tls.alpn, Some(vec!["h2".into()]));
        assert_eq!(tls.fingerprint.as_deref(), Some("chrome"));
        let output = polaris_config_engine::builder::outbound::build_proxy_outbound(
            &roundtripped,
            "fixture",
            &polaris_config_engine::singbox::DomainResolver::Tag("dns".into()),
            "x64",
            "linux",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap()["tls"]["server_name"],
            "certificate.example"
        );
    }
}

#[test]
fn shadowsocks_preserves_utf8_and_whitespace_passwords() {
    for password in ["päss", " secret ", " ", "密碼:two"] {
        let node = ServerConfig {
            name: "fixture".into(),
            protocol: Protocol::Shadowsocks,
            address: "ss.example".into(),
            port: 8388,
            shadowsocks_settings: Some(Box::new(ShadowsocksSettings {
                method: "aes-128-gcm".into(),
                password: password.into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert_eq!(
            roundtrip(&node).shadowsocks_settings.unwrap().password,
            password
        );
        let plain = format!(
            "ss://aes-128-gcm:{}@ss.example:8388#fixture",
            percent_encode_component(password)
        );
        assert_eq!(
            parse_ok(&plain).shadowsocks_settings.unwrap().password,
            password
        );
    }
}

#[test]
fn every_shadowtls_port_form_checks_integer_bounds() {
    for port in [
        "0",
        "-1",
        "65536",
        "65537",
        "1.5",
        "18446744073709551616",
        "bad",
        "",
    ] {
        for plugin in ["", "plugin=shadow-tls&"] {
            let url = format!("ss://aes-128-gcm:pw@ss.example:8388?{plugin}shadow-tls-password=outer&shadow-tls-sni=s.example&shadow-tls-port={port}#fixture");
            assert!(!parse_err(&url).is_empty());
        }
    }
    for port in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(65536),
        serde_json::json!(65537),
        serde_json::json!(1.5),
        serde_json::json!("65535"),
        serde_json::json!(1e30),
    ] {
        let payload = serde_json::json!({"password":"outer","host":"s.example","port":port});
        let encoded = base64_encode(payload.to_string().as_bytes(), true, false);
        assert!(!parse_err(&format!(
            "ss://aes-128-gcm:pw@ss.example:8388?shadow-tls={encoded}#fixture"
        ))
        .is_empty());
    }
    for plugin in ["", "plugin=shadow-tls&"] {
        let node = parse_ok(&format!("ss://aes-128-gcm:pw@ss.example:8388?{plugin}shadow-tls-password=outer&shadow-tls-sni=s.example&shadow-tls-port=65535#fixture"));
        assert_eq!(node.shadow_tls_settings.unwrap().port, Some(65535));
    }
    let payload = serde_json::json!({"password":"outer","host":"s.example","port":65535});
    let encoded = base64_encode(payload.to_string().as_bytes(), true, false);
    assert_eq!(
        parse_ok(&format!(
            "ss://aes-128-gcm:pw@ss.example:8388?shadow-tls={encoded}#fixture"
        ))
        .shadow_tls_settings
        .unwrap()
        .port,
        Some(65535)
    );
}

#[test]
fn malformed_pin_members_reject_entire_share_node() {
    for pin in ["bad".to_string(), format!("{PIN_HEX},bad")] {
        assert!(!parse_err(&format!(
            "vless://{U}@a.com:443?security=tls&pcs={}#fixture",
            percent_encode_component(&pin)
        ))
        .is_empty());
    }
}

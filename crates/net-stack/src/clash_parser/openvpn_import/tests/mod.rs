use super::*;

fn parse(extra: &str) -> Result<ServerConfig, String> {
    let yaml = format!(
        "name: vpn\ntype: openvpn\nserver: vpn.example.com\nport: 1194\nusername: user\nca: |\n  -----BEGIN CERTIFICATE-----\n  TEST\n  -----END CERTIFICATE-----\n{extra}"
    );
    let proxy: Value = serde_yaml::from_str(&yaml).unwrap();
    parse_openvpn_proxy(&proxy, ServerConfig::default())
}

#[test]
fn maps_transport_tls_and_timing_to_endpoint_wire_keys() {
    let server = parse(
        "proto: tcp-client\nkey-direction: '1'\ntls-auth: |\n  -----BEGIN OpenVPN Static key V1-----\n  TEST\n  -----END OpenVPN Static key V1-----\ncipher: AES-128-CBC\ndata-ciphers: [AES-256-GCM, AES-128-GCM]\nauth: SHA256\ncomp-lzo: adaptive\nping: 10\nping-restart: 60\nmtu: 1400\nudp: true\ntfo: true\nmptcp: true\n",
    )
    .unwrap();
    let endpoint =
        polaris_config_engine::builder::endpoints::build_vpn_client_endpoint(&server, "vpn", None)
            .unwrap();
    let endpoint = serde_json::to_value(endpoint).unwrap();
    assert_eq!(endpoint["type"], "openvpn-client");
    assert_eq!(endpoint["network"], "tcp");
    assert_eq!(endpoint["tls"]["control_wrap"]["type"], "tls_auth");
    assert_eq!(endpoint["tcp_fast_open"], true);
    assert_eq!(endpoint["tcp_multi_path"], true);
    let settings = server.openvpn_client_settings.unwrap();
    assert_eq!(settings.network.as_deref(), Some("tcp"));
    assert_eq!(settings.mtu, Some(1400));
    assert_eq!(settings.auth.as_deref(), Some("SHA256"));
    assert_eq!(settings.extra["data_ciphers_fallback"], "AES-128-CBC");
    assert_eq!(
        settings.extra["data_ciphers"],
        json!(["AES-256-GCM", "AES-128-GCM"])
    );
    assert_eq!(settings.extra["compression_lzo"], "yes");
    assert_eq!(settings.extra["ping_interval"], "10s");
    assert_eq!(settings.extra["ping_restart"], "60s");
    assert_eq!(
        settings.tls.as_ref().unwrap().extra["control_wrap"]["direction"],
        "client"
    );
    assert!(endpoint.get("cipher").is_none());
}

#[test]
fn fails_on_unportable_material_or_unmapped_behavior() {
    assert!(parse("cert: /tmp/client.crt\nkey: /tmp/client.key\n")
        .unwrap_err()
        .contains("cert"));
    assert!(parse("peer-info: {UV_DEVICE_ID: a}\n")
        .unwrap_err()
        .contains("peer-info"));
    assert!(parse("dialer-proxy: upstream\n")
        .unwrap_err()
        .contains("dialer-proxy"));
    assert!(parse("interface-name: utun4\n")
        .unwrap_err()
        .contains("interface-name"));
    assert!(parse("routing-mark: 12\n")
        .unwrap_err()
        .contains("routing-mark"));
    assert!(parse("tls-auth: /tmp/ta.key\n")
        .unwrap_err()
        .contains("tls-auth"));
    assert!(parse("tls-auth: |\n  -----BEGIN OpenVPN Static key V1-----\n  TEST\n  -----END OpenVPN Static key V1-----\ntls-crypt: |\n  -----BEGIN OpenVPN Static key V1-----\n  TEST\n  -----END OpenVPN Static key V1-----\n")
        .unwrap_err()
        .contains("互斥"));
}

#[test]
fn does_not_confuse_udp_capability_with_transport() {
    let server = parse("proto: udp\nudp: false\n").unwrap();
    let settings = server.openvpn_client_settings.unwrap();
    assert_eq!(settings.network.as_deref(), Some("udp"));
    assert_eq!(settings.auth.as_deref(), Some("SHA256"));
    assert_eq!(settings.extra["data_ciphers"], json!(["AES-128-GCM"]));
    assert_eq!(settings.extra["data_ciphers_fallback"], "AES-128-GCM");
}

#[test]
fn different_legacy_and_fallback_ciphers_are_rejected() {
    assert!(parse("data-ciphers-fallback: AES-256-GCM\n")
        .unwrap_err()
        .contains("无法等价转换"));
    let server = parse("cipher: AES-256-GCM\ndata-ciphers-fallback: AES-256-GCM\n").unwrap();
    assert_eq!(
        server.openvpn_client_settings.unwrap().extra["data_ciphers_fallback"],
        "AES-256-GCM"
    );
}

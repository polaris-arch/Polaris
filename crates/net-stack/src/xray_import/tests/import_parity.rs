use super::*;
#[test]
fn xray_preserves_supported_http_and_ws_headers() {
    for stream in [
        json!({"network":"ws","wsSettings":{"path":"/ws","headers":{"Host":"front.example","X-Auth":"token"}}}),
        json!({"network":"http","httpSettings":{"path":"/rpc","method":"POST","headers":{"X-Auth":["one","two"]}}}),
    ] {
        let result = parse(
            json!({"outbounds":[{"protocol":"vless","settings":{"vnext":[{"address":"e.example","port":443,"users":[{"id":"uuid"}]}]},"streamSettings":stream}]}),
        );
        let output = serde_json::to_value(
            polaris_config_engine::builder::outbound::build_proxy_outbound(
                &result.servers[0],
                "fixture",
                &polaris_config_engine::singbox::DomainResolver::Tag("dns".into()),
                "x64",
                "linux",
            )
            .unwrap(),
        )
        .unwrap();
        if stream["network"] == "ws" {
            assert_eq!(output["transport"]["headers"]["X-Auth"], "token");
        } else {
            assert_eq!(output["transport"]["method"], "POST");
            assert_eq!(
                output["transport"]["headers"]["X-Auth"],
                json!(["one", "two"])
            );
        }
    }
}
#[test]
fn xray_mixed_and_all_invalid_pins_fail_closed() {
    for pins in [
        "bad",
        "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881,bad",
    ] {
        let result = parse(
            json!({"outbounds":[{"protocol":"trojan","settings":{"servers":[{"address":"a.com","port":443,"password":"pw"}]},
            "streamSettings":{"security":"tls","tlsSettings":{"pinnedPeerCertSha256":pins}}}]}),
        );
        assert!(result.servers.is_empty());
        assert_eq!(result.failed, 1);
    }
}

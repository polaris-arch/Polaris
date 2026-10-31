use super::super::*;
#[test]
fn dedupe_keeps_transport_and_outer_tls_differences() {
    let node = crate::share_link::parse_share_url(
        "ss://YWVzLTI1Ni1nY206cHc@host.example:443",
        &mut || "id".into(),
        &mut |_| {},
    )
    .unwrap();
    let mut same = node.clone();
    same.id = "other".into();
    same.name = "renamed".into();
    same.subscription_id = Some("other-sub".into());
    assert_eq!(dedupe_by_fingerprint(vec![node.clone(), same]).len(), 1);
    let mut changed = node.clone();
    changed.port = 8443;
    assert_eq!(dedupe_by_fingerprint(vec![node.clone(), changed]).len(), 2);
    let mut changed = serde_json::to_value(&node).unwrap();
    changed["shadowTlsSettings"] =
        serde_json::json!({"password":"outer", "sni":"cover.example", "port":8443});
    let changed = serde_json::from_value(changed).unwrap();
    assert_eq!(dedupe_by_fingerprint(vec![node, changed]).len(), 2);
}

#[test]
fn tailscale_provider_identity_is_not_collapsed() {
    use polaris_config_engine::user_config::server_config::Protocol;
    let node = ServerConfig {
        id: "one".into(),
        name: "tailnet".into(),
        protocol: Protocol::Tailscale,
        provider_name: Some("provider-a".into()),
        ..Default::default()
    };
    let mut other = node.clone();
    other.id = "two".into();
    other.provider_name = Some("provider-b".into());
    assert_eq!(dedupe_by_fingerprint(vec![node, other]).len(), 2);
}

#[test]
fn snell_v4_modes_reach_every_bounded_subscription_dispatch_and_origin() {
    for mode in ["http", "tls"] {
        let uri = format!("snell://part1%3Apart2%2Bsecret@s.example:443?version=4&obfs={mode}&obfs-host=Cover.Example#SN");
        let native = serde_json::json!({"outbounds":[{"type":"snell","tag":"SN","server":"s.example",
            "server_port":443,"psk":"part1:part2+secret","version":4,"obfs_mode":mode,"obfs_host":"Cover.Example"}]}).to_string();
        let clash = serde_json::json!({"proxies":[{"name":"SN","type":"snell","server":"s.example",
            "port":443,"psk":"part1:part2+secret","version":4,"obfs-opts":{"mode":mode,"host":"Cover.Example"}}]});
        let yaml = serde_yaml::to_string(&clash).unwrap();
        let list = format!("# subscription\n{uri}\n");
        for (label, text) in [
            ("native", native),
            ("URI", uri),
            ("list", list.clone()),
            ("base64", super::b64(&list)),
            ("Clash JSON", clash.to_string()),
            ("Clash YAML", yaml),
        ] {
            for origin in [ImportOrigin::LocalFile, ImportOrigin::RemoteSubscription] {
                let mut serial = 0;
                let mut ids = || {
                    serial += 1;
                    format!("sn-{serial}")
                };
                let bundle = parse_subscription_bundle_limited(
                    &text,
                    "sub",
                    "now",
                    &mut ids,
                    origin,
                    SubscriptionParseLimits::default(),
                )
                .unwrap();
                let r = bundle.parsed;
                assert_eq!(
                    (r.servers.len(), r.failed, r.skipped),
                    (1, 0, 0),
                    "{label}: {:?}",
                    r.warnings
                );
                assert_eq!(r.servers[0].password.as_deref(), Some("part1:part2+secret"));
                assert_eq!(r.servers[0].subscription_id.as_deref(), Some("sub"));
                let sn = r.servers[0].snell_settings.as_deref().unwrap();
                assert_eq!(sn.obfs_mode.as_deref(), Some(mode));
                assert_eq!(sn.obfs_host.as_deref(), Some("Cover.Example"));
                let error = parse_subscription_bundle_limited(
                    &text,
                    "sub",
                    "now",
                    &mut ids,
                    origin,
                    SubscriptionParseLimits {
                        max_nodes: 0,
                        ..Default::default()
                    },
                )
                .unwrap_err();
                assert!(error.contains("节点数超过上限"), "{label}: {error}");
                let error = parse_subscription_bundle_limited(
                    &text,
                    "sub",
                    "now",
                    &mut ids,
                    origin,
                    SubscriptionParseLimits {
                        max_body_bytes: text.len() - 1,
                        ..Default::default()
                    },
                )
                .unwrap_err();
                assert!(error.contains("字节超过解析上限"));
            }
        }
    }
}

#[test]
fn snell_mixed_lines_count_failures_and_dedupe_only_equivalent_modes_hosts() {
    let body = "snell://pw@s.example:443?obfs=http&obfs-host=a.example#http\nsnell://pw@s.example:443?obfs=tls&obfs-host=a.example#tls\nsnell://pw@s.example:443?obfs=tls&obfs-host=b.example#otherhost\nsnell://pw@s.example:443?obfs=tls&obfs-host=a.example#duplicate\nsnell://pw@s.example:443?obfs=unknown#bad\nsnell://pw@s.example:443?version=6&obfs=tls#badv6\nsnell://%20@s.example:443?obfs=tls#badpsk\n";
    for text in [body.to_owned(), super::b64(body)] {
        let mut serial = 0;
        let r = parse_subscription(
            &text,
            "sub",
            "now",
            &mut || {
                serial += 1;
                format!("sn-{serial}")
            },
            ImportOrigin::RemoteSubscription,
        );
        assert_eq!((r.servers.len(), r.failed, r.skipped), (4, 3, 0));
        assert_eq!(dedupe_by_fingerprint(r.servers).len(), 3);
        assert!(r.warnings.iter().any(|w| w.contains("obfs 不受支持")));
    }
    let native=serde_json::json!({"outbounds":[
        {"type":"snell","server":"s.example","server_port":443,"psk":"pw","version":4,"obfs_mode":"tls"},
        {"type":"snell","server":"s.example","server_port":443,"psk":"pw","version":4,"obfs_mode":true}
    ]}).to_string();
    let r = parse_subscription(
        &native,
        "sub",
        "now",
        &mut || "id".into(),
        ImportOrigin::RemoteSubscription,
    );
    assert_eq!((r.servers.len(), r.failed, r.skipped), (1, 1, 0));
}

use super::*;
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

use super::*;
#[test]
fn shadowtls_share_roundtrip_preserves_outer_credentials() {
    let mut warnings = Vec::new();
    let source = "ss://YWVzLTI1Ni1nY206cHc@host.example:443?shadow-tls-password=outer%26secret&shadow-tls-sni=cover.example&shadow-tls-fp=firefox&shadow-tls-port=8443#node";
    let first = parse_share_url(source, &mut || "id".into(), &mut |w| warnings.push(w)).unwrap();
    let encoded = encode_share_url(&first).unwrap();
    let second =
        parse_share_url(&encoded, &mut || "next".into(), &mut |w| warnings.push(w)).unwrap();
    assert_eq!(
        serde_json::to_value(first.shadow_tls_settings).unwrap(),
        serde_json::to_value(second.shadow_tls_settings).unwrap()
    );
    assert!(warnings.is_empty());
}
#[test]
fn malformed_shadowtls_payload_is_not_diagnostic_content() {
    let secret = "private-secret-not-base64";
    let mut warnings = Vec::new();
    let url = format!("ss://YWVzLTI1Ni1nY206cHc@host.example:443?shadow-tls={secret}");
    let _ = parse_share_url(&url, &mut || "id".into(), &mut |w| warnings.push(w));
    assert!(!warnings.is_empty());
    assert!(warnings.iter().all(|w| !w.contains(secret)));
}

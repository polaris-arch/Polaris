use super::*;
#[test]
fn provider_filter_warnings_do_not_disclose_url_credentials() {
    let text = "proxies:\n  - {name: node, type: ss, server: host.example, port: 443, cipher: aes-256-gcm, password: pw}\n";
    let source = "https://private-user:private-pass@feed.example/list?token=private-token";
    let request = ProviderParseRequest {
        text: text.into(), provider_name: "safe-provider".into(), filter: Some("[".into()),
        exclude_filter: Some("[".into()), override_value: None, subscription_id: "sub".into(),
        now: "now".into(), source: source.into(), limits: SubscriptionParseLimits::default(),
    };
    let result = parse_provider_request(request, &mut || "id".into()).unwrap().parsed;
    assert_eq!(result.servers.len(), 1);
    assert!(!result.warnings.is_empty());
    let legacy = parse_provider_text(text, Some("["), Some("["), None, "sub", "now", source, &mut || "id".into()).unwrap();
    for warning in result.warnings.iter().chain(legacy.warnings.iter()) {
        for secret in ["private-user", "private-pass", "private-token"] { assert!(!warning.contains(secret)); }
    }
}

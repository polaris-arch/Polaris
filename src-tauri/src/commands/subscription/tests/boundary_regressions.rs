use super::super::*;
use polaris_net_stack::safe_redirect::{FetchInit, HttpClient, MinimalResponse};
use polaris_net_stack::subscription::{fetch_subscription_full, parse_subscription_bundle, resolve_proxy_providers};
use std::{future::Future, pin::Pin};

struct ResolverFailure;
impl DnsLookup for ResolverFailure {
    async fn lookup_all(&self, host: &str) -> Result<Vec<String>, String> {
        match host {
            "unavailable.example" => Err("temporary resolver failure".into()),
            "empty.example" => Ok(Vec::new()),
            "unsafe.example" => Ok(vec!["127.0.0.1".into()]),
            _ => panic!("unexpected fixture host"),
        }
    }
}

struct NeverDial;
impl HttpClient for NeverDial {
    async fn fetch(&self, _: &str, _: &FetchInit) -> Result<MinimalResponse, String> {
        panic!("DNS unavailability and unsafe addresses must both deny HTTP dispatch")
    }
}

#[tokio::test]
async fn dns_failures_preserve_provider_ids_but_unsafe_provider_is_not_preserved() {
    let body = "proxies:\n  - {name: Inline, type: http, server: inline.example, port: 8080}\nproxy-providers:\n  unavailable: {type: http, url: 'https://unavailable.example/sub'}\n  empty: {type: http, url: 'https://empty.example/sub'}\n  unsafe: {type: http, url: 'https://unsafe.example/sub'}\n";
    let mut n = 0;
    let mut ids = || { n += 1; format!("generated-{n}") };
    let bundle = parse_subscription_bundle(body, "sub", "now", &mut ids, ImportOrigin::RemoteSubscription);
    assert_eq!(bundle.parsed.servers.len(), 1);
    let fetch = |url: &str| -> Pin<Box<dyn Future<Output = Result<String, ProviderFetchError>> + Send>> {
        let url = url.to_owned();
        Box::pin(async move {
            fetch_subscription_full(&NeverDial, &ResolverFailure, &url, "fixture", None, false, 1000)
                .await.map_err(|error| {
                    let expected = if url.contains("unsafe.example") { SubscriptionErrorKind::Ssrf } else { SubscriptionErrorKind::Dns };
                    assert_eq!(error.kind, expected);
                    assert_eq!(primary_fetch_retry_delay(error.kind, 0).is_some(), expected == SubscriptionErrorKind::Dns);
                    classify_provider_fetch_error(&error)
                })
        })
    };
    let providers = resolve_proxy_providers(bundle.proxy_providers.as_ref().unwrap(), "sub", "now", 8, &fetch, &mut ids).await;
    assert!(providers.any_failed);
    assert_eq!(providers.failed_providers, vec!["unavailable", "empty"]);
    let mut cfg = json!({"selectedServerId":"stable-unavailable", "servers":[
        {"id":"stable-unavailable","name":"Old","protocol":"http","address":"old.example","port":8080,"subscriptionId":"sub","providerName":"unavailable"},
        {"id":"stable-empty","name":"Old empty","protocol":"http","address":"old-empty.example","port":8080,"subscriptionId":"sub","providerName":"empty"},
        {"id":"unsafe-old","name":"Unsafe old","protocol":"http","address":"unsafe-old.example","port":8080,"subscriptionId":"sub","providerName":"unsafe"}
    ]});
    let result = reconcile_subscription_servers(&mut cfg, "sub", bundle.parsed.servers, providers.any_failed, &providers.failed_providers);
    assert_eq!(result.deleted, 1);
    let current: Vec<_> = cfg["servers"].as_array().unwrap().iter().filter_map(|s| s["id"].as_str()).collect();
    assert!(current.contains(&"stable-unavailable"));
    assert!(current.contains(&"stable-empty"));
    assert!(!current.contains(&"unsafe-old"));
    assert_eq!(cfg["selectedServerId"], "stable-unavailable");
}

#[test]
fn malformed_metadata_edit_preserves_persisted_record_and_owned_nodes() {
    let dir = crate::test_support::TestDir::new("subscription-invalid-edit-");
    let config = crate::runtime::config::ConfigManager::new(dir.clone());
    let mut initial = config.load_full().unwrap();
    initial["subscriptions"] = json!([{"id":"sub","name":"Original","url":"https://example.com/sub"}]);
    initial["servers"] = json!([{"id":"owned","name":"Owned","protocol":"http","address":"example.com","port":8080,"subscriptionId":"sub"}]);
    config.save_full(&initial).unwrap();
    let before = config.load_full().unwrap();
    let bytes = std::fs::read(dir.join("config.json")).unwrap();
    for field in ["id", "name", "url"] {
        for invalid in [Value::Null, json!(""), json!(17), json!([])] {
            let mut edit = before["subscriptions"][0].clone();
            edit[field] = invalid;
            assert!(subscription_update_core(&config, edit).is_err());
            assert_eq!(std::fs::read(dir.join("config.json")).unwrap(), bytes);
            assert_eq!(config.load_full().unwrap(), before);
        }
    }
    let mut valid = before["subscriptions"][0].clone();
    valid["name"] = json!("Renamed");
    let saved = subscription_update_core(&config, valid).unwrap();
    assert_eq!(saved["subscriptions"][0]["name"], "Renamed");
    assert_eq!(saved["servers"], before["servers"]);
}

#[test]
fn all_refresh_origins_share_admission_before_snapshot_and_both_commit_paths_keep_identity() {
    let code = crate::test_support::crate_code("commands/subscription.rs");
    let inner = code.split("async fn perform_subscription_update_inner(").nth(1).unwrap();
    let outer = code.split("pub(crate) async fn perform_subscription_update(").nth(1).unwrap();
    assert!(outer.find("subscription_refresh.try_admit(subscription_id)").unwrap() < outer.find("let sink:").unwrap());
    assert!(outer.find("subscription_refresh.try_admit(subscription_id)").unwrap() < outer.find("perform_subscription_update_inner(app,").unwrap());
    assert_eq!(inner.matches("fetch_identity.check_current(cfg, subscription_id)").count(), 2);
    let scheduler = crate::test_support::crate_code("runtime/subscription_scheduler.rs");
    assert!(scheduler.contains("perform_subscription_update(app, state.inner(), &id).await"));
}

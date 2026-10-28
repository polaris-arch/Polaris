use super::super::*;
use polaris_net_stack::safe_redirect::{FetchInit, HttpClient, MinimalResponse};
use polaris_net_stack::subscription::{
    fetch_subscription_full, parse_subscription_bundle, resolve_proxy_providers,
};
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
    let mut ids = || {
        n += 1;
        format!("generated-{n}")
    };
    let bundle = parse_subscription_bundle(
        body,
        "sub",
        "now",
        &mut ids,
        ImportOrigin::RemoteSubscription,
    );
    assert_eq!(bundle.parsed.servers.len(), 1);
    let fetch =
        |url: &str| -> Pin<Box<dyn Future<Output = Result<String, ProviderFetchError>> + Send>> {
            let url = url.to_owned();
            Box::pin(async move {
                fetch_subscription_full(
                    &NeverDial,
                    &ResolverFailure,
                    &url,
                    "fixture",
                    None,
                    false,
                    1000,
                )
                .await
                .map_err(|error| {
                    let expected = if url.contains("unsafe.example") {
                        SubscriptionErrorKind::Ssrf
                    } else {
                        SubscriptionErrorKind::Dns
                    };
                    assert_eq!(error.kind, expected);
                    assert_eq!(
                        primary_fetch_retry_delay(error.kind, 0).is_some(),
                        expected == SubscriptionErrorKind::Dns
                    );
                    classify_provider_fetch_error(&error)
                })
            })
        };
    let providers = resolve_proxy_providers(
        bundle.proxy_providers.as_ref().unwrap(),
        "sub",
        "now",
        8,
        &fetch,
        &mut ids,
    )
    .await;
    assert!(providers.any_failed);
    assert_eq!(providers.failed_providers, vec!["unavailable", "empty"]);
    let mut cfg = json!({"selectedServerId":"stable-unavailable", "servers":[
        {"id":"stable-unavailable","name":"Old","protocol":"http","address":"old.example","port":8080,"subscriptionId":"sub","providerName":"unavailable"},
        {"id":"stable-empty","name":"Old empty","protocol":"http","address":"old-empty.example","port":8080,"subscriptionId":"sub","providerName":"empty"},
        {"id":"unsafe-old","name":"Unsafe old","protocol":"http","address":"unsafe-old.example","port":8080,"subscriptionId":"sub","providerName":"unsafe"}
    ]});
    let result = reconcile_subscription_servers(
        &mut cfg,
        "sub",
        bundle.parsed.servers,
        providers.any_failed,
        &providers.failed_providers,
    );
    assert_eq!(result.deleted, 1);
    let current: Vec<_> = cfg["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
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
    initial["subscriptions"] =
        json!([{"id":"sub","name":"Original","url":"https://example.com/sub"}]);
    initial["servers"] = json!([{"id":"owned","name":"Owned","protocol":"http","address":"example.com","port":8080,"subscriptionId":"sub"}]);
    config.save_full(&initial).unwrap();
    let before = config.load_full().unwrap();
    let bytes = std::fs::read(dir.join("config.json")).unwrap();
    for field in ["id", "name", "url"] {
        for invalid in [Value::Null, json!(""), json!(17), json!([])] {
            let mut edit = before["subscriptions"][0].clone();
            edit[field] = invalid;
            assert!(subscription_update_core(
                &config,
                edit,
                &crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default()
            )
            .is_err());
            assert_eq!(std::fs::read(dir.join("config.json")).unwrap(), bytes);
            assert_eq!(config.load_full().unwrap(), before);
        }
    }
    let mut valid = before["subscriptions"][0].clone();
    valid["name"] = json!("Renamed");
    let saved = subscription_update_core(
        &config,
        valid,
        &crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default(),
    )
    .unwrap()
    .0;
    assert_eq!(saved["subscriptions"][0]["name"], "Renamed");
    assert_eq!(saved["servers"], before["servers"]);
}

#[test]
fn all_refresh_origins_share_admission_before_snapshot_and_both_commit_paths_keep_identity() {
    let code = crate::test_support::crate_code("commands/subscription.rs");
    let inner = code
        .split("async fn perform_subscription_update_inner(")
        .nth(1)
        .unwrap();
    let outer = code
        .split("pub(crate) async fn perform_subscription_update(")
        .nth(1)
        .unwrap();
    assert!(
        outer
            .find("subscription_refresh.try_admit(subscription_id)")
            .unwrap()
            < outer.find("let sink:").unwrap()
    );
    assert!(
        outer
            .find("subscription_refresh.try_admit(subscription_id)")
            .unwrap()
            < outer
                .find("perform_subscription_update_inner(app,")
                .unwrap()
    );
    assert_eq!(
        inner
            .matches("fetch_identity.check_current(cfg, subscription_id)")
            .count(),
        2
    );
    let scheduler = crate::test_support::crate_code("runtime/subscription_scheduler.rs");
    assert!(scheduler.contains("perform_subscription_update(app, state.inner(), &id).await"));
}

#[tokio::test]
async fn edit_supersession_drops_the_actual_awaited_pipeline_and_cancels_providers() {
    use crate::runtime::subscription_refresh::SubscriptionRefreshAdmission;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct DropProbe(Arc<AtomicBool>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let admission = SubscriptionRefreshAdmission::default();
    let owner = admission.try_admit("sub").unwrap();
    let control = ProviderResolveControl::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let probe = DropProbe(Arc::clone(&dropped));
    let pending = async move {
        let _probe = probe;
        std::future::pending::<()>().await
    };
    let mut waiting = Box::pin(await_refresh_pipeline(
        &owner,
        &control,
        std::time::Instant::now() + Duration::from_secs(60),
        pending,
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let replacement = admission.supersede("sub");
    assert!(waiting.await.is_none());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(control.is_cancelled());
    assert!(replacement.token().is_current());
}

#[tokio::test]
async fn refresh_pipeline_obeys_existing_deadline_without_retrying_or_disabling_owner() {
    use crate::runtime::subscription_refresh::SubscriptionRefreshAdmission;
    let admission = SubscriptionRefreshAdmission::default();
    let owner = admission.try_admit("sub").unwrap();
    let control = ProviderResolveControl::default();
    let result = await_refresh_pipeline(
        &owner,
        &control,
        std::time::Instant::now(),
        std::future::pending::<()>(),
    )
    .await;
    assert!(matches!(result, Some(Err(_))));
    assert!(owner.token().is_current());
}

#[tokio::test]
async fn late_refresh_publication_cannot_invalidate_new_saved_edit_intent() {
    struct NoSystemMutation;
    impl crate::runtime::proxy::system_takeover::SystemProxyClearer for NoSystemMutation {
        fn ensure_cleared(&mut self) -> bool {
            false
        }
        fn detect_foreign_proxy(&self) -> Option<String> {
            None
        }
        fn enable_system_proxy(
            &mut self,
            _: &polaris_system_integration::proxy_ops::ProxyEnableRequest,
        ) -> Result<(), String> {
            panic!("no native proxy mutation in publication test")
        }
        fn recover_from_marker(&mut self) -> Result<bool, String> {
            Ok(false)
        }
    }
    let directory = crate::test_support::TestDir::new("subscription-publication-");
    let config = Arc::new(crate::runtime::config::ConfigManager::new(
        directory.clone(),
    ));
    let proxy = Arc::new(ProxyRuntime::new(
        Arc::clone(&config),
        Arc::new(
            crate::runtime::helper::HelperRuntime::never_installed_for_tests(directory.clone()),
        ),
        Arc::new(crate::runtime::mesh::MeshRuntime::new(directory.clone())),
        Box::new(NoSystemMutation),
    ));
    let mut old_cfg = config.load_full().unwrap();
    old_cfg["subscriptions"] = json!([{"id":"s","name":"Old","url":"https://example.invalid/a"}]);
    config.save_full(&old_cfg).unwrap();
    let old_cfg = config.load_full().unwrap();
    let admission = crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
    let old_owner = admission.try_admit("s").unwrap();
    let old_intent =
        prepare_subscription_publication(&config, &proxy, &old_cfg, Some(&old_owner.token()))
            .unwrap()
            .unwrap();
    // Barrier: old refresh has committed, but has not dispatched its broadcast yet.
    let mut edit = old_cfg["subscriptions"][0].clone();
    edit["url"] = json!("https://example.invalid/b");
    let (latest, new_owner) = subscription_update_core(&config, edit, &admission).unwrap();
    let new_owner = new_owner.unwrap();
    let new_intent =
        prepare_subscription_publication(&config, &proxy, &latest, Some(&new_owner.token()))
            .unwrap()
            .unwrap();
    assert!(proxy.selector_intent_is_current(new_intent));
    assert_eq!(
        prepare_subscription_publication(&config, &proxy, &old_cfg, Some(&old_owner.token()))
            .unwrap(),
        None
    );
    assert!(
        proxy.selector_intent_is_current(new_intent),
        "late old publication must not register any intent"
    );
    let old_projection = std::sync::atomic::AtomicBool::new(false);
    crate::commands::config::apply_config_broadcast_core(
        &proxy,
        old_cfg,
        false,
        old_intent,
        |_| old_projection.store(true, std::sync::atomic::Ordering::SeqCst),
        |_, accepted| assert!(accepted.is_none()),
    )
    .await;
    assert!(!old_projection.load(std::sync::atomic::Ordering::SeqCst));
    assert!(proxy.selector_intent_is_current(new_intent));
    let new_projection = std::sync::atomic::AtomicBool::new(false);
    crate::commands::config::apply_config_broadcast_core(
        &proxy,
        latest,
        false,
        new_intent,
        |_| new_projection.store(true, std::sync::atomic::Ordering::SeqCst),
        |_, _| {},
    )
    .await;
    assert!(new_projection.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn edit_refresh_uses_effective_request_identity_not_metadata_or_requested_ua_text() {
    // The actual command macro compiles in this crate. Its Result transports the
    // existing envelope unchanged; it must not encode Ok/Err tags in frontend data.
    let command_source = crate::test_support::crate_source("commands/subscription.rs");
    let command = command_source
        .split("pub async fn subscription_update(")
        .nth(1)
        .unwrap()
        .split("fn valid_edit_request_id")
        .next()
        .unwrap();
    assert!(command.contains("-> Result<ApiResponse<Value>, ()>"));
    let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("Ok(matchsubscription_update_core("));
    assert!(command.contains("return Ok(ApiResponse::err("));
    let data = json!({"saved":true,"editRequestId":"00000000-0000-4000-8000-000000000001","publication":"scheduled","refresh":{"success":false,"errorKind":"timeout"}});
    fn assert_command_result<'a, F>(
        _: impl FnOnce(AppHandle, State<'a, AppRuntime>, Value, Option<String>) -> F,
    ) where
        F: Future<Output = Result<ApiResponse<Value>, ()>>,
    {
    }
    // Compile the actual command function's future output, without invoking IPC,
    // creating a fake literal Result, or starting an application/runtime.
    assert_command_result(subscription_update);
    let envelope = serde_json::to_value(ApiResponse::ok(data.clone())).unwrap();
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["data"], data);
    let envelope = serde_json::to_value(ApiResponse::<Value>::err("storage failed")).unwrap();
    assert_eq!(envelope["success"], false);
    assert_eq!(envelope["error"], "storage failed");

    assert!(super::super::valid_edit_request_id(
        "00000000-0000-4000-8000-000000000001"
    ));
    for invalid in [
        "",
        "secret",
        "00000000000040008000000000000001",
        "AAAAAAAA-0000-4000-8000-000000000001",
    ] {
        assert!(!super::super::valid_edit_request_id(invalid));
    }
    let source = crate::test_support::crate_source("commands/subscription.rs");
    let command = source
        .split("pub async fn subscription_update(")
        .nth(1)
        .unwrap()
        .split("fn valid_edit_request_id")
        .next()
        .unwrap();
    assert!(
        command.find("subscription_update_core(").unwrap()
            < command.find("perform_owned_subscription_update(").unwrap()
    );
    assert!(source.contains("perform_owned_subscription_update(app, state, &owner, None)"));

    let dir = crate::test_support::TestDir::new("subscription-edit-identities-");
    let config = crate::runtime::config::ConfigManager::new(dir.clone());
    let mut initial = config.load_full().unwrap();
    initial["subscriptionUserAgent"] = json!("custom/1");
    initial["subscriptions"] =
        json!([{"id":"s","name":"Old","url":"https://example.invalid/a","etag":"old"}]);
    config.save_full(&initial).unwrap();
    let admission = crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
    for patch in [json!({"name":"Renamed"}), json!({"userAgent":"custom/1"})] {
        let mut edit = config.load_full().unwrap()["subscriptions"][0].clone();
        for (key, value) in patch.as_object().unwrap() {
            edit[key] = value.clone();
        }
        let (_, owner) = subscription_update_core(&config, edit, &admission).unwrap();
        assert!(owner.is_none());
    }
    for patch in [
        json!({"url":"https://example.invalid/b"}),
        json!({"userAgent":"custom/2"}),
    ] {
        let mut edit = config.load_full().unwrap()["subscriptions"][0].clone();
        for (key, value) in patch.as_object().unwrap() {
            edit[key] = value.clone();
        }
        let (saved, owner) = subscription_update_core(&config, edit, &admission).unwrap();
        assert!(owner.unwrap().token().is_current());
        assert!(saved["subscriptions"][0].get("etag").is_none());
    }
}

#[test]
fn not_modified_metadata_persists_before_one_signal_without_node_or_selection_changes() {
    use std::cell::Cell;
    let dir = crate::test_support::TestDir::new("subscription-metadata-signal-");
    let config = crate::runtime::config::ConfigManager::new(dir.clone());
    let mut initial = config.load_full().unwrap();
    initial["subscriptions"] =
        json!([{"id":"sub","name":"S","url":"https://example.com/sub","etag":"old"}]);
    initial["servers"] = json!([{"id":"b","name":"B","protocol":"http","address":"b.example","port":80,"subscriptionId":"sub"},{"id":"a","name":"A","protocol":"http","address":"a.example","port":80,"subscriptionId":"sub"}]);
    initial["selectedServerId"] = json!("b");
    config.save_full(&initial).unwrap();
    let before = config.load_full().unwrap();
    let identity = SubscriptionFetchIdentity::new(&before, &before["subscriptions"][0]);
    let admission = crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
    let owner = admission.try_admit("sub").unwrap();
    let signals = Cell::new(0);
    refresh_not_modified_metadata(
        &config,
        "sub",
        &identity,
        &owner.token(),
        Some("new"),
        Some("date"),
        || {
            // Native listeners may synchronously load the already-persisted latest disk state.
            assert_eq!(
                config.load_full().unwrap()["subscriptions"][0]["etag"],
                "new"
            );
            signals.set(signals.get() + 1);
        },
    )
    .unwrap();
    let after = config.load_full().unwrap();
    assert_eq!(signals.get(), 1);
    assert_eq!(after["subscriptions"][0]["etag"], "new");
    assert_eq!(after["subscriptions"][0]["lastModified"], "date");
    assert!(after["subscriptions"][0]["lastUpdated"].is_string());
    assert_eq!(after["servers"], before["servers"]);
    assert_eq!(after["selectedServerId"], before["selectedServerId"]);
}

#[test]
fn not_modified_metadata_failure_missing_edit_and_supersession_publish_nothing() {
    use std::cell::Cell;
    for case in [
        "persist",
        "missing",
        "url_changed",
        "superseded",
        "cancelled",
    ] {
        let dir = crate::test_support::TestDir::new("subscription-metadata-reject-");
        let config = crate::runtime::config::ConfigManager::new(dir.clone());
        let mut initial = config.load_full().unwrap();
        initial["subscriptions"] =
            json!([{"id":"sub","name":"S","url":"https://example.com/sub","etag":"old"}]);
        config.save_full(&initial).unwrap();
        let snapshot = config.load_full().unwrap();
        let identity = SubscriptionFetchIdentity::new(&snapshot, &snapshot["subscriptions"][0]);
        let admission =
            crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
        let owner = admission.try_admit("sub").unwrap();
        let token = owner.token();
        let mut replacement = None;
        match case {
            "persist" => config.fail_next_config_persist_for_test(),
            "missing" => {
                let mut value = snapshot.clone();
                value["subscriptions"] = json!([]);
                config.save_full(&value).unwrap();
            }
            "url_changed" => {
                let mut value = snapshot.clone();
                value["subscriptions"][0]["url"] = json!("https://example.com/new");
                config.save_full(&value).unwrap();
            }
            "superseded" => replacement = Some(admission.supersede("sub")),
            "cancelled" => drop(owner),
            _ => unreachable!(),
        }
        let before = std::fs::read(dir.join("config.json")).unwrap();
        let signals = Cell::new(0);
        assert!(
            refresh_not_modified_metadata(
                &config,
                "sub",
                &identity,
                &token,
                Some("new"),
                None,
                || signals.set(signals.get() + 1)
            )
            .is_err(),
            "{case}"
        );
        assert_eq!(signals.get(), 0, "{case}");
        assert_eq!(
            std::fs::read(dir.join("config.json")).unwrap(),
            before,
            "{case}"
        );
        drop(replacement);
    }
}

#[test]
fn metadata_signal_after_persistence_rejects_superseded_and_dropped_owners() {
    use std::cell::Cell;
    let admission = crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
    let old = admission.try_admit("sub").unwrap();
    let old_token = old.token();
    let current = admission.supersede("sub");
    let signals = Cell::new(0);
    publish_subscription_metadata(&old_token, || signals.set(signals.get() + 1));
    assert_eq!(signals.get(), 0);
    publish_subscription_metadata(&current.token(), || signals.set(signals.get() + 1));
    assert_eq!(signals.get(), 1);
    let token = current.token();
    drop(current);
    publish_subscription_metadata(&token, || signals.set(signals.get() + 1));
    assert_eq!(signals.get(), 1);
}

#[test]
fn metadata_signal_releases_lock_and_late_signal_reads_newer_edit() {
    let dir = crate::test_support::TestDir::new("subscription-metadata-late-signal-");
    let config = crate::runtime::config::ConfigManager::new(dir.clone());
    let mut initial = config.load_full().unwrap();
    initial["subscriptions"] =
        json!([{"id":"sub","name":"S","url":"https://example.com/old","etag":"old"}]);
    config.save_full(&initial).unwrap();
    let admission = crate::runtime::subscription_refresh::SubscriptionRefreshAdmission::default();
    let owner = admission.try_admit("sub").unwrap();
    let mut next = None;
    // publish invokes the callback only after its current-generation check. A new
    // editor can win at that point; the empty signal must not carry old state.
    publish_subscription_metadata(&owner.token(), || {
        let mut edited = config.load_full().unwrap()["subscriptions"][0].clone();
        edited["url"] = json!("https://example.com/new");
        let (saved, replacement) = subscription_update_core(&config, edited, &admission).unwrap();
        next = replacement;
        assert_eq!(saved["subscriptions"][0]["url"], "https://example.com/new");
        let latest = config.load_full().unwrap();
        assert_eq!(latest["subscriptions"][0]["url"], "https://example.com/new");
        assert!(latest["subscriptions"][0].get("etag").is_none());
    });
    assert!(next.unwrap().token().is_current());
    assert!(!owner.token().is_current());
}

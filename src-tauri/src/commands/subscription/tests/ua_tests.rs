use super::super::{
    invalidate_validators_on_global_ua_change, resolve_subscription_ua, ua_changed,
};
use serde_json::json;

/// 三级链 + **falsy 语义**（与 TS `??` 的差异已登记在函数文档与 `contracts/types.ts`）。
#[test]
fn ua_falls_back_through_three_levels_with_falsy_empty() {
    let global = json!({ "subscriptionUserAgent": "clash-verge/1.0" });
    // per-sub 优先。
    assert_eq!(
        resolve_subscription_ua(&global, &json!({ "userAgent": "sing-box/1.9" })).as_deref(),
        Some("sing-box/1.9")
    );
    // per-sub 缺 → 全局。
    assert_eq!(
        resolve_subscription_ua(&global, &json!({})).as_deref(),
        Some("clash-verge/1.0")
    );
    // **差异格**：per-sub 显式空串 → falsy 回落全局（TS `??` 会让 `""` 胜出并发空 UA）。
    assert_eq!(
        resolve_subscription_ua(&global, &json!({ "userAgent": "" })).as_deref(),
        Some("clash-verge/1.0"),
        "空串按未设处理（发 `User-Agent: ` 空值会让机场下发错格式/0 节点）"
    );
    assert_eq!(
        resolve_subscription_ua(&global, &json!({ "userAgent": "   " })).as_deref(),
        Some("clash-verge/1.0"),
        "纯空白同理"
    );
    // 全局也空 → None（交默认 UA）。
    assert_eq!(
        resolve_subscription_ua(&json!({ "subscriptionUserAgent": "  " }), &json!({})),
        None
    );
    assert_eq!(resolve_subscription_ua(&json!({}), &json!({})), None);
}

/// UA 变更 → 条件 GET 验证器作废（否则机场按 UA 下发变体时永远 304 拿不到新格式）。
///
/// 变异锁：把 `subscription_update` 里那段 `if ua_changed(...) { set_or_remove(..., None) }`
/// 删掉 → `changing_ua_drops_validators` 转红。
#[test]
fn ua_changed_uses_the_same_falsy_normalisation_as_resolution() {
    let with = |ua: serde_json::Value| json!({ "id": "s1", "userAgent": ua });
    assert!(ua_changed(&with(json!("a")), &with(json!("b"))));
    assert!(ua_changed(&json!({ "id": "s1" }), &with(json!("a"))));
    assert!(ua_changed(&with(json!("a")), &json!({ "id": "s1" })));
    // UA 同值不算 UA 变化；URL 变化由独立响应身份判据处理。
    assert!(!ua_changed(&with(json!("a")), &with(json!("a"))));
    // 归一口径与 `resolve_subscription_ua` 一致：缺省/空串/纯空白三者互相之间**不算变更**
    //（它们求值出的实际 UA 完全相同，清验证器纯属白扔）。
    assert!(!ua_changed(&json!({}), &with(json!(""))));
    assert!(!ua_changed(&with(json!("")), &with(json!("   "))));
    // 名字变、UA 没变 → 不动验证器。
    assert!(!ua_changed(
        &json!({ "name": "old", "userAgent": "a" }),
        &json!({ "name": "new", "userAgent": "a" })
    ));
}

// ── 全局 `subscriptionUserAgent` 变更 → 验证器作废（LOW-1）────────────────────

/// 两条订阅：`s-global` 靠全局 UA、`s-own` 自带 per-sub 覆盖。改全局 UA 后**只有前者**该被作废。
///
/// 牙：
/// - 删掉 `invalidate_validators_on_global_ua_change` 里的清除腿 → 第 1、2 条断言转红
///   （= 改了全局 UA 仍带旧 ETag 请求 → 机场恒 304 → 新格式永远拿不到）；
/// - 把射程放宽成「全局变了就全清」（去掉 `per_sub.is_some()` 早退）→ 第 3、4 条转红
///   （白扔一次条件 GET，把下次更新变成全量下载）。
#[test]
fn global_ua_change_invalidates_only_subs_without_per_sub_override() {
    let old = json!({ "subscriptionUserAgent": "clash-verge/1.0" });
    let mut new = json!({
        "subscriptionUserAgent": "sing-box/1.9",
        "subscriptions": [
            { "id": "s-global", "etag": "W/\"v1\"", "lastModified": "Mon, 01 Jan 2024 00:00:00 GMT" },
            { "id": "s-own", "userAgent": "mihomo/1.18", "etag": "W/\"v2\"", "lastModified": "x" },
        ]
    });
    assert_eq!(
        invalidate_validators_on_global_ua_change(&old, &mut new),
        1,
        "只该有 1 条被作废（另一条有 per-sub 覆盖，生效 UA 没变）"
    );
    let subs = new["subscriptions"].as_array().unwrap();
    assert!(
        subs[0].get("etag").is_none(),
        "①靠全局 UA 的订阅：etag 必须清"
    );
    assert!(
        subs[0].get("lastModified").is_none(),
        "②靠全局 UA 的订阅：lastModified 必须清"
    );
    assert_eq!(
        subs[1]["etag"], "W/\"v2\"",
        "③per-sub 覆盖的订阅：生效 UA 与全局无关 → 验证器不得白扔"
    );
    assert_eq!(subs[1]["lastModified"], "x", "④同上");
}

/// 归一口径必须与 [`resolve_subscription_ua`] 一致：缺省 / `""` / 纯空白三者互换**不算变更**；
/// 全局键没动时更不该动任何东西（改别的设置每次都清验证器 = 每次更新都全量下载）。
///
/// 牙：把早退判据从 `pick_ua(...) == pick_ua(...)` 换成裸 `old.get(K) == new.get(K)` → 第 2、3 条转红。
#[test]
fn global_ua_normalisation_and_no_op_paths() {
    let subs = || json!([{ "id": "s1", "etag": "W/\"v1\"", "lastModified": "L" }]);
    // ① 全局键完全没动（改的是别的键）→ 零作废。
    let mut new =
        json!({ "subscriptionUserAgent": "ua/1", "logLevel": "debug", "subscriptions": subs() });
    assert_eq!(
        invalidate_validators_on_global_ua_change(
            &json!({ "subscriptionUserAgent": "ua/1" }),
            &mut new
        ),
        0
    );
    assert_eq!(new["subscriptions"][0]["etag"], "W/\"v1\"");
    // ② 空串 → 纯空白：求值出的 UA 完全相同，不算变更。
    let mut new = json!({ "subscriptionUserAgent": "   ", "subscriptions": subs() });
    assert_eq!(
        invalidate_validators_on_global_ua_change(
            &json!({ "subscriptionUserAgent": "" }),
            &mut new
        ),
        0
    );
    assert_eq!(new["subscriptions"][0]["etag"], "W/\"v1\"");
    // ③ 缺键 → 空串：同理不算变更。
    let mut new = json!({ "subscriptionUserAgent": "", "subscriptions": subs() });
    assert_eq!(
        invalidate_validators_on_global_ua_change(&json!({}), &mut new),
        0
    );
    assert_eq!(new["subscriptions"][0]["etag"], "W/\"v1\"");
    // ④ 缺键 → 真值：这是**真变更**（此前走 net-stack 默认 UA，现在走用户设的）。
    let mut new = json!({ "subscriptionUserAgent": "ua/2", "subscriptions": subs() });
    assert_eq!(
        invalidate_validators_on_global_ua_change(&json!({}), &mut new),
        1
    );
    assert!(new["subscriptions"][0].get("etag").is_none());
    // ⑤ 真值 → 清空：同样是真变更（回落默认 UA，变体可能又换一套）。
    let mut new = json!({ "subscriptionUserAgent": "", "subscriptions": subs() });
    assert_eq!(
        invalidate_validators_on_global_ua_change(
            &json!({ "subscriptionUserAgent": "ua/2" }),
            &mut new
        ),
        1
    );
    assert!(new["subscriptions"][0].get("lastModified").is_none());
}

/// 计数只算「真有验证器被扔掉」的订阅；无 `subscriptions` 键 / 空数组不得 panic。
#[test]
fn count_reflects_actually_dropped_validators_and_tolerates_missing_array() {
    let old = json!({ "subscriptionUserAgent": "a" });
    // 从没拉取过（无验证器）→ 不计数（否则日志谎报条数）。
    let mut new = json!({ "subscriptionUserAgent": "b", "subscriptions": [{ "id": "s1" }] });
    assert_eq!(invalidate_validators_on_global_ua_change(&old, &mut new), 0);
    // subscriptions 缺失 / 非数组 / 空数组 → 0，不 panic。
    let mut new = json!({ "subscriptionUserAgent": "b" });
    assert_eq!(invalidate_validators_on_global_ua_change(&old, &mut new), 0);
    let mut new = json!({ "subscriptionUserAgent": "b", "subscriptions": {} });
    assert_eq!(invalidate_validators_on_global_ua_change(&old, &mut new), 0);
    let mut new = json!({ "subscriptionUserAgent": "b", "subscriptions": [] });
    assert_eq!(invalidate_validators_on_global_ua_change(&old, &mut new), 0);
}

#[test]
fn refresh_identity_rejects_late_url_and_effective_ua_changes_but_accepts_renames() {
    use super::super::SubscriptionFetchIdentity;
    let original = json!({"subscriptionUserAgent":"global/1", "subscriptions":[{
        "id":"s", "name":"before", "url":"https://example.com/a", "etag":"old"
    }]});
    let identity = SubscriptionFetchIdentity::new(&original, &original["subscriptions"][0]);
    let mut current = original.clone();
    current["subscriptions"][0]["name"] = json!("after");
    assert!(identity.check_current(&current, "s").is_ok());
    for (key, value) in [("url", "https://example.com/b"), ("userAgent", "own/2")] {
        let mut changed = current.clone();
        changed["subscriptions"][0][key] = json!(value);
        assert!(identity.check_current(&changed, "s").is_err(), "{key}");
    }
    current["subscriptionUserAgent"] = json!("global/2");
    assert!(identity.check_current(&current, "s").is_err());
    current["subscriptions"] = json!([]);
    assert!(identity.check_current(&current, "s").is_err());
}

#[test]
fn refresh_identity_uses_resolved_ua_not_raw_preference_fields() {
    use super::super::SubscriptionFetchIdentity;
    let mut config = json!({"subscriptionUserAgent":"global/1", "subscriptions":[{
        "id":"s", "url":"https://example.com/a", "userAgent":"own/1"
    }]});
    let own = SubscriptionFetchIdentity::new(&config, &config["subscriptions"][0]);
    config["subscriptionUserAgent"] = json!("global/2");
    assert!(own.check_current(&config, "s").is_ok());
    config["subscriptionUserAgent"] = json!("");
    config["subscriptions"][0]["userAgent"] = json!("");
    let default = SubscriptionFetchIdentity::new(&config, &config["subscriptions"][0]);
    config["subscriptionUserAgent"] = json!("   ");
    config["subscriptions"][0]["userAgent"] = json!("   ");
    assert!(default.check_current(&config, "s").is_ok());
    config["subscriptions"][0]["userAgent"] = json!(crate::runtime::http::app_user_agent());
    assert!(default.check_current(&config, "s").is_ok());
}

#[test]
fn late_response_guard_skips_both_candidate_mutation_and_persistence() {
    use super::super::SubscriptionFetchIdentity;
    use crate::runtime::config::{ConfigManager, Decision};
    use crate::test_support::TestDir;
    let dir = TestDir::new("polaris-refresh-identity-");
    let manager = ConfigManager::new(dir.clone());
    manager
        .update(|cfg| {
            cfg["subscriptions"] = json!([{"id":"s", "name":"A", "url":"https://example.com/a"}]);
            Decision::Write(())
        })
        .unwrap();
    let original = manager.load_full().unwrap();
    let identity = SubscriptionFetchIdentity::new(&original, &original["subscriptions"][0]);
    manager
        .update(|cfg| {
            cfg["subscriptions"][0]["url"] = json!("https://example.com/b");
            Decision::Write(())
        })
        .unwrap();
    let current = manager.load_full().unwrap();
    // The production 200 and 304 branches use this same pre-mutation guard.
    for _response_kind in [200, 304] {
        let (result, saved) = manager
            .update(|cfg| {
                if let Err(error) = identity.check_current(cfg, "s") {
                    return Decision::Skip(Err(error));
                }
                cfg["subscriptions"][0]["etag"] = json!("stale-response");
                cfg["servers"] = json!([]);
                Decision::Write(Ok(()))
            })
            .unwrap();
        assert!(result.is_err());
        assert!(saved.is_none());
        assert_eq!(manager.load_full().unwrap(), current);
    }
}

#[test]
fn both_refresh_commit_branches_guard_identity_and_url_edits_drop_validators() {
    use super::super::subscription_url_changed;
    assert!(subscription_url_changed(
        &json!({"url":"a"}),
        &json!({"url":"b"})
    ));
    assert!(!subscription_url_changed(
        &json!({"url":"a", "name":"old"}),
        &json!({"url":"a", "name":"new"})
    ));
    let source = crate::test_support::crate_code("commands/subscription.rs");
    assert_eq!(
        source
            .matches("fetch_identity.check_current(cfg, subscription_id)")
            .count(),
        2
    );
    assert!(source.contains("subscription_url_changed(&arr[idx], &subscription)"));
    let update =
        crate::commands::guard_scan::top_level_fn_body(&source, "fn subscription_update_core(");
    assert!(
        update.find("subscription_url_changed(").unwrap() < update.find("set_or_remove(").unwrap()
    );
}

#[test]
fn fetch_identity_preserves_custom_ua_and_shares_default() {
    use super::super::SubscriptionFetchIdentity;
    let global = json!({"subscriptionUserAgent": "custom-global/1"});
    let sub = json!({"userAgent": "custom-sub/2"});
    assert_eq!(
        SubscriptionFetchIdentity::new(&global, &sub).user_agent,
        "custom-sub/2"
    );
    assert_eq!(
        SubscriptionFetchIdentity::new(&global, &json!({})).user_agent,
        "custom-global/1"
    );
    assert_eq!(
        SubscriptionFetchIdentity::new(&json!({}), &json!({})).user_agent,
        crate::runtime::http::app_user_agent()
    );
}

#[test]
fn effective_ua_default_and_persisted_override_matrix() {
    use super::super::SubscriptionFetchIdentity;
    let default = crate::runtime::http::app_user_agent();
    assert_eq!(
        default,
        polaris_net_stack::subscription::default_subscription_user_agent(
            env!("CARGO_PKG_VERSION"),
            crate::runtime::core_version::selected_bundled_version()
        )
    );
    assert!(default.starts_with("Polaris/"));
    assert!(default.contains(" sing-box/"));
    for (global, sub, expected) in [
        (json!({}), json!({}), default.clone()),
        (
            json!({"subscriptionUserAgent":"  "}),
            json!({"userAgent":""}),
            default.clone(),
        ),
        (
            json!({"subscriptionUserAgent":"global/2"}),
            json!({}),
            "global/2".into(),
        ),
        (
            json!({"subscriptionUserAgent":"global/2"}),
            json!({"userAgent":"per-sub/3"}),
            "per-sub/3".into(),
        ),
        (
            json!({"subscriptionUserAgent":"global/2"}),
            json!({"userAgent":"  "}),
            "global/2".into(),
        ),
        (
            json!({}),
            json!({"userAgent":"Polaris/1.0"}),
            "Polaris/1.0".into(),
        ),
    ] {
        let mut cfg = polaris_store::default_config();
        if let Some(value) = global.get("subscriptionUserAgent") {
            cfg["subscriptionUserAgent"] = value.clone();
        }
        let mut subscription =
            json!({"id":"sub","name":"fixture","url":"https://subscription.example/list"});
        if let Some(value) = sub.get("userAgent") {
            subscription["userAgent"] = value.clone();
        }
        cfg["subscriptions"] = json!([subscription]);
        let sanitized = polaris_store::sanitize_config(&cfg.to_string()).unwrap();
        assert_eq!(
            SubscriptionFetchIdentity::new(&sanitized, &sanitized["subscriptions"][0]).user_agent,
            expected
        );
    }
}

#[test]
fn ua_display_reports_actual_identity_and_shared_precedence_without_mutation() {
    use super::super::describe_subscription_user_agent;
    let actual_default = crate::runtime::http::app_user_agent();
    let cfg = json!({ "subscriptionUserAgent": " global/2 " });
    let unchanged = cfg.clone();
    for draft in [
        json!({}),
        json!({"userAgent": ""}),
        json!({"userAgent": "  "}),
    ] {
        let info = describe_subscription_user_agent(
            Some(&cfg),
            &draft,
            "app-fixture",
            "core-fixture",
            actual_default.clone(),
        )
        .unwrap();
        assert_eq!(info.default_user_agent, actual_default);
        assert_eq!(info.fallback_user_agent.as_deref(), Some("global/2"));
        assert_eq!(info.effective_user_agent.as_deref(), Some("global/2"));
        assert_eq!(info.source, Some("global"));
    }
    for cfg in [json!({}), json!({"subscriptionUserAgent": "  "})] {
        let info = describe_subscription_user_agent(
            Some(&cfg),
            &json!({"userAgent": ""}),
            "app-fixture",
            "core-fixture",
            actual_default.clone(),
        )
        .unwrap();
        assert_eq!(
            info.effective_user_agent.as_deref(),
            Some(actual_default.as_str())
        );
        assert_eq!(
            info.fallback_user_agent.as_deref(),
            Some(actual_default.as_str())
        );
        assert_eq!(info.source, Some("default"));
    }
    let info = describe_subscription_user_agent(
        Some(&cfg),
        &json!({"userAgent": " custom/3 "}),
        "app-fixture",
        "core-fixture",
        actual_default.clone(),
    )
    .unwrap();
    assert_eq!(info.default_user_agent, actual_default);
    assert_eq!(info.effective_user_agent.as_deref(), Some("custom/3"));
    assert_eq!(info.fallback_user_agent.as_deref(), Some("global/2"));
    assert_eq!(info.source, Some("subscription"));
    let wire = serde_json::to_value(&info).unwrap();
    assert_eq!(wire["appVersion"], "app-fixture");
    assert_eq!(wire["coreVersion"], "core-fixture");
    assert_eq!(wire["defaultUserAgent"], actual_default);
    assert_eq!(wire["fallbackUserAgent"], "global/2");
    assert_eq!(wire["effectiveUserAgent"], "custom/3");
    assert_eq!(wire["source"], "subscription");
    assert_eq!(wire["effectiveStatus"], "ready");
    assert_eq!(
        cfg, unchanged,
        "read-only preview must never persist a default or override"
    );
}

#[test]
fn ua_display_rejects_unavailable_versions_even_when_override_exists() {
    use super::super::describe_subscription_user_agent;
    for (app, core, default) in [
        ("", "core", "ua"),
        ("app", "", "ua"),
        ("app", "unknown", "ua"),
        ("unknown", "core", "ua"),
        ("app", " ", "ua"),
        ("app", "core", ""),
    ] {
        let result = describe_subscription_user_agent(
            Some(&json!({"subscriptionUserAgent":"global"})),
            &json!({"userAgent":"custom"}),
            app,
            core,
            default.to_owned(),
        );
        assert!(
            result.is_err(),
            "a custom UA cannot disguise unavailable build identity"
        );
    }
}

#[test]
fn ua_display_command_reads_real_sources_without_fetch_or_write() {
    let source = crate::test_support::crate_source("commands/subscription.rs");
    let body = crate::commands::guard_scan::top_level_fn_body(
        &source,
        "pub async fn subscription_user_agent_info(",
    );
    for required in [
        "state.config().try_with_saved_commit(|cfg|",
        "env!(\"CARGO_PKG_VERSION\")",
        "crate::runtime::core_version::selected_bundled_version()",
        "crate::runtime::http::app_user_agent()",
        "describe_subscription_user_agent(",
        "ApiResponse::err_with_code",
        "subscriptionUserAgentUnavailable",
        "Err(_) => ApiResponse::ok(build_info)",
    ] {
        assert!(
            body.contains(required),
            "missing authoritative read: {required}"
        );
    }
    let fence = body.find("try_with_saved_commit").unwrap();
    assert!(body.find("selected_bundled_version()").unwrap() < fence);
    assert!(body.find("app_user_agent()").unwrap() < fence);
    for forbidden in [
        "load_full",
        ".current()",
        "save_full",
        "fetch_parse_resolve",
        "preview_core",
        "proxy.start",
        "unwrap_or_default",
    ] {
        assert!(
            !body.contains(forbidden),
            "read-only identity must not fetch, write or hide errors: {forbidden}"
        );
    }
}

#[test]
fn ua_display_saved_projection_is_cold_busy_safe_and_never_loads_or_writes() {
    use super::super::describe_subscription_user_agent;
    use crate::runtime::config::ConfigManager;
    use crate::test_support::TestDir;
    let directory = TestDir::new("polaris-ua-display-read-only-");
    let manager = ConfigManager::new(directory.clone());
    let called = std::cell::Cell::new(false);
    let project = |cfg: &serde_json::Value| {
        called.set(true);
        describe_subscription_user_agent(
            Some(cfg),
            &json!({"userAgent":"custom/fixture"}),
            "app-fixture",
            "core-fixture",
            "opaque/default".to_owned(),
        )
    };
    let unavailable = || {
        describe_subscription_user_agent(
            None,
            &json!({"userAgent":"custom/fixture"}),
            "app-fixture",
            "core-fixture",
            "opaque/default".to_owned(),
        )
    };
    let assert_build_only = |info: super::super::SubscriptionUserAgentInfo| {
        assert_eq!(info.app_version, "app-fixture");
        assert_eq!(info.core_version, "core-fixture");
        assert_eq!(info.default_user_agent, "opaque/default");
        assert_eq!(info.effective_status, "unavailable");
        assert!(info.fallback_user_agent.is_none());
        assert!(info.effective_user_agent.is_none());
        assert!(info.source.is_none());
        let wire = serde_json::to_value(info).unwrap();
        assert!(wire["effectiveUserAgent"].is_null());
        assert!(wire["fallbackUserAgent"].is_null());
    };
    let cold = manager
        .try_with_saved_commit(project)
        .unwrap_or_else(|_| unavailable())
        .unwrap();
    assert_build_only(cold);
    assert!(
        !called.get(),
        "cold config must not evaluate the UA projection"
    );
    assert!(
        !directory.join("config.json").exists(),
        "UA read must not initialize or migrate config"
    );
    // Only fixture setup warms the real manager; the production read never does this.
    manager.current().unwrap();
    let before = std::fs::read(directory.join("config.json")).unwrap();
    let writer = manager.hold_write_lock_for_test();
    let busy = manager
        .try_with_saved_commit(project)
        .unwrap_or_else(|_| unavailable())
        .unwrap();
    assert_build_only(busy);
    assert!(
        !called.get(),
        "busy writer must not evaluate the UA projection"
    );
    drop(writer);
    let info = manager.try_with_saved_commit(project).unwrap().unwrap();
    assert!(called.get());
    assert_eq!(info.default_user_agent, "opaque/default");
    assert_eq!(info.effective_status, "ready");
    assert_eq!(info.effective_user_agent.as_deref(), Some("custom/fixture"));
    assert_eq!(
        std::fs::read(directory.join("config.json")).unwrap(),
        before
    );
}

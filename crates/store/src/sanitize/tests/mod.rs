use super::*;
use serde_json::json;

#[test]
fn bad_json_returns_parse_err() {
    let res = sanitize_config("{ not valid json");
    assert!(matches!(res, Err(crate::StoreError::Parse(_))));
}

#[test]
fn bad_field_skipped_good_field_kept() {
    // servers 是 string（坏字段）→ 删除；但 customRules 合法 → 保留。
    // 绝不因 servers 坏而丢弃整份配置（维度7 #7 核心）。
    let json = r#"{
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "servers": "not-an-array",
        "customRules": [
            {"id":"r1","type":"domain","values":["a.com"],"action":"proxy","enabled":true}
        ],
        "mixedPort": "bad-port"
    }"#;
    let v = sanitize_config(json).unwrap();
    let obj = v.as_object().unwrap();
    // 坏字段已删
    assert!(!obj.contains_key("servers"));
    assert!(!obj.contains_key("mixedPort"));
    // 好字段保留
    assert_eq!(obj["proxyMode"], "global");
    assert_eq!(obj["customRules"].as_array().unwrap().len(), 1);
}

#[test]
fn bad_server_dropped_good_server_kept() {
    let json = r#"{
        "proxyMode": "global",
        "proxyModeType": "tun",
        "servers": [
            {"id":"good","name":"Good","protocol":"trojan","address":"1.2.3.4","port":443,"password":"pw"},
            {"id":"","name":"Bad","protocol":"trojan"},
            {"id":"unknown-proto","name":"X","protocol":"nonexistent","address":"5.6.7.8","port":80}
        ]
    }"#;
    let v = sanitize_config(json).unwrap();
    let servers = v["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["id"], "good");
}

#[test]
fn multiple_userspace_tailscale_nodes_survive_storage_roundtrip() {
    let original = r#"{"servers":[
        {"id":"ts-a","name":"Tailnet A","protocol":"tailscale",
         "tailscaleSettings":{"reverseMesh":false,"controlUrl":"https://a.example"}},
        {"id":"ts-b","name":"Tailnet B","protocol":"tailscale",
         "tailscaleSettings":{"reverseMesh":false,"controlUrl":"https://b.example"}}
    ]}"#;
    let first = sanitize_config(original).unwrap();
    let saved = serde_json::to_string(&first).unwrap();
    let loaded = sanitize_config(&saved).unwrap();
    let ids: Vec<_> = loaded["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|server| server["id"].as_str())
        .collect();
    assert_eq!(ids, ["ts-a", "ts-b"]);
    assert_eq!(
        loaded["servers"][1]["tailscaleSettings"]["controlUrl"],
        "https://b.example"
    );
}

/// tailcat 是无地址协议（同 tor）：没有 address/port 也保留；设置块不合格则按必填门丢弃。
#[test]
fn addressless_tailcat_kept_invalid_tailcat_dropped() {
    let pk = "lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=";
    let json = format!(
        r#"{{"servers":[
            {{"id":"tc","name":"TC","protocol":"tailcat",
              "tailcatSettings":{{"serverPublicKey":"{pk}","serverDiscoKey":"{pk}","derpRegion":1}}}},
            {{"id":"bad","name":"Bad","protocol":"tailcat",
              "tailcatSettings":{{"serverPublicKey":"{pk}","derpRegion":1}}}}
        ]}}"#
    );
    let v = sanitize_config(&json).unwrap();
    let ids: Vec<&str> = v["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["tc"]);
}

#[test]
fn bool_field_bad_removed() {
    let json = r#"{
        "proxyMode": "direct",
        "proxyModeType": "manual",
        "appRoutingEnabled": "yes",
        "singboxDashboard": 1,
        "keepTrayMenuWarm": "yes",
        "hardwareAcceleration": true
    }"#;
    let v = sanitize_config(json).unwrap();
    let obj = v.as_object().unwrap();
    assert!(!obj.contains_key("appRoutingEnabled"));
    assert!(!obj.contains_key("singboxDashboard"));
    assert!(!obj.contains_key("keepTrayMenuWarm"));
    assert_eq!(obj["hardwareAcceleration"], Value::Bool(true));
}

#[test]
fn update_channels_share_the_stable_prerelease_value_domain() {
    let valid =
        sanitize_config(r#"{"appUpdateChannel":"prerelease","coreUpdateChannel":"stable"}"#)
            .unwrap();
    assert_eq!(valid["appUpdateChannel"], "prerelease");
    assert_eq!(valid["coreUpdateChannel"], "stable");

    let invalid =
        sanitize_config(r#"{"appUpdateChannel":"nightly","coreUpdateChannel":true}"#).unwrap();
    assert!(invalid.get("appUpdateChannel").is_none());
    assert!(invalid.get("coreUpdateChannel").is_none());
}

#[test]
fn legacy_domain_rule_preserved_garbage_dropped() {
    // 项1：无 `type` + `domains` 数组的旧 DomainRule 不被 sanitize 丢弃（原样留待 migrate 转 Rule）；
    // 无 type 且**无 domains** 的纯垃圾条目仍照丢。判据复用 migrate::is_legacy_domain_rule（单一真值）。
    let json = r#"{
        "proxyMode":"global","proxyModeType":"systemProxy",
        "customRules":[
            {"id":"legacy","domains":["a.com"],"action":"proxy","enabled":true},
            {"id":"garbage","action":"proxy","enabled":true}
        ]
    }"#;
    let v = sanitize_config(json).unwrap();
    let rules = v["customRules"].as_array().unwrap();
    assert_eq!(rules.len(), 1, "旧 DomainRule 保留、纯垃圾丢弃");
    assert_eq!(rules[0]["id"], "legacy");
    assert!(
        rules[0].get("type").is_none(),
        "sanitize 不给旧规则补 type（交 migrate 处理）"
    );
}

#[test]
fn rule_effects_are_preserved_and_malformed_branches_degrade_independently() {
    let json = r#"{
        "proxyMode":"smart","proxyModeType":"tun",
        "customRules":[
            {"id":"dns-only","type":"domain","values":["a.com"],"action":"direct","enabled":true,
             "effects":{"dns":{"resolver":"proxy","answerMode":"real"}}},
            {"id":"mixed","type":"domainSuffix","values":["example.com"],"action":"proxy","enabled":true,
             "effects":{"route":{"action":"proxy","targetServerId":7},
                        "dns":{"resolver":"direct","answerMode":"fakeIp"}}},
            {"id":"fallback","type":"domain","values":["b.com"],"action":"direct","enabled":true,
             "effects":"bad"}
        ]
    }"#;
    let value = sanitize_config(json).unwrap();
    let rules = value["customRules"].as_array().unwrap();
    assert_eq!(rules[0]["effects"]["dns"]["resolver"], "proxy");
    assert_eq!(rules[1]["effects"]["route"]["action"], "proxy");
    assert!(rules[1]["effects"]["route"].get("targetServerId").is_none());
    assert_eq!(rules[1]["effects"]["dns"]["answerMode"], "fakeIp");
    assert!(rules[2].get("effects").is_none());
}

#[test]
fn dns_policy_v2_drops_bad_items_without_losing_good_resources_or_rules() {
    let json = r#"{
        "proxyMode":"smart","proxyModeType":"tun",
        "policyRules":[
            {"id":"good","type":"domain","values":["a.example"],"action":"direct","enabled":true,
             "effects":{"dns":{"resolver":"direct","answerMode":"real",
             "action":{"type":"server","serverId":"dns-good"}}}},
            {"id":"degrade","type":"domain","values":["b.example"],"action":"direct","enabled":true,
             "effects":{"dns":{"resolver":"direct","answerMode":"real",
             "action":{"type":"unknown"}}}}
        ],
        "routeRuleOrder":["good",7,"good","degrade"],
        "dnsServers":[
            {"id":"dns-good","name":"Good","enabled":true,"type":"https",
             "endpoint":{"host":"1.1.1.1","port":443,"path":"/dns-query"},
             "outbound":{"type":"direct"}},
            {"id":"dns-bad","enabled":true,"type":"https",
             "endpoint":{"host":"1.1.1.1","port":"bad"},"outbound":{"type":"direct"}},
            {"id":"dns-good","enabled":true,"type":"local","outbound":{"type":"direct"}}
        ],
        "dnsServerGroups":[
            {"id":"race","name":"Race","enabled":true,"mode":"race","members":["dns-good"]},
            {"id":"bad","name":"Bad","enabled":true,"mode":"unknown","members":[]}
        ]
    }"#;
    let value = sanitize_config(json).unwrap();
    assert_eq!(value["policyRules"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["policyRules"][0]["effects"]["dns"]["action"]["serverId"],
        "dns-good"
    );
    assert!(value["policyRules"][1]["effects"]["dns"]
        .get("action")
        .is_none());
    assert_eq!(
        value["routeRuleOrder"],
        serde_json::json!(["good", "degrade"])
    );
    assert_eq!(value["dnsServers"].as_array().unwrap().len(), 1);
    assert_eq!(value["dnsServerGroups"].as_array().unwrap().len(), 1);
}

#[test]
fn root_non_object_is_error() {
    let res = sanitize_config("[1,2,3]");
    assert!(matches!(res, Err(crate::StoreError::Validation(_))));
}

#[test]
fn empty_input_is_error() {
    let res = sanitize_config("");
    assert!(matches!(res, Err(crate::StoreError::Parse(_))));
}

/// 网络场景 sanitize（spec §7）：缺 id / 重复 id / 结构坏的条目丢弃，其余保留；搜索域去首尾点、小写、
/// 去空、去重。牙：删掉 `sanitize_network_profiles` 调用 → 条目数与规范化断言转红。
#[test]
fn network_profiles_sanitize_drops_idless_entries_and_normalizes_domains() {
    let json = r#"{
        "proxyMode": "smart",
        "proxyModeType": "systemProxy",
        "networkProfiles": [
            {"name": "no id", "match": {"dnsServerCidrs": ["10.0.0.0/8"]}},
            {"id": "np", "name": "office", "match": {"searchDomains": [".Corp.Example.", "corp.example", "  ", "LAN"]}},
            {"id": "np", "name": "dup"},
            {"id": "bad", "probe": "sometimes"}
        ]
    }"#;
    let v = sanitize_config(json).unwrap();
    let profiles = v["networkProfiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 1, "{profiles:?}");
    assert_eq!(profiles[0]["name"], "office", "重复 id 保留首个");
    assert_eq!(
        profiles[0]["match"]["searchDomains"],
        serde_json::json!(["corp.example", "lan"])
    );
    let not_array = sanitize_config(r#"{"networkProfiles": {"id": "x"}}"#).unwrap();
    assert!(not_array.get("networkProfiles").is_none(), "非数组整键删除");
}

/// 槽位上限按平台取值：桌面 64、Android 32、iOS 16。
#[test]
fn the_speed_test_slot_cap_depends_on_the_platform() {
    for platform in Platform::ALL {
        let expected = match platform {
            Platform::Mac | Platform::Win | Platform::Linux => 64,
            Platform::Android => 32,
            Platform::Ios | Platform::Other => 16,
        };
        assert_eq!(speed_test_slot_cap(*platform), expected, "{platform:?}");
    }
}

/// 测速并发：`"auto"` 与 4 到本平台上限的整数保留，其余删除（回落自动）。上限随平台不同，
/// 桌面设的 48 拿到手机上被清成自动。
#[test]
fn speed_test_concurrency_is_kept_only_within_the_platform_range() {
    let sanitized = |value: Value, platform| {
        let mut obj = Map::new();
        obj.insert("speedTestConcurrency".into(), value);
        speed_test_concurrency_or_remove(&mut obj, platform);
        obj.get("speedTestConcurrency").cloned()
    };
    for (value, platform, kept) in [
        (json!("auto"), Platform::Linux, true),
        (json!(4), Platform::Linux, true),
        (json!(64), Platform::Linux, true),
        (json!(48), Platform::Linux, true),
        (json!(48), Platform::Android, false),
        (json!(32), Platform::Android, true),
        (json!(17), Platform::Ios, false),
        (json!(3), Platform::Linux, false),
        (json!(65), Platform::Linux, false),
        (json!(8.5), Platform::Linux, false),
        (json!("16"), Platform::Linux, false),
        (json!(true), Platform::Linux, false),
    ] {
        assert_eq!(
            sanitized(value.clone(), platform),
            kept.then_some(value.clone()),
            "{value} on {platform:?}"
        );
    }
}

/// 逐订阅的两个键与顶层的两个键：类型或取值不对即删除，回到缺省。越界的周期删除而不是钳到边界。
#[test]
fn periodic_speed_test_settings_are_removed_not_clamped_when_invalid() {
    let subscription = |extra: Value| {
        let mut sub = json!({ "id": "s", "name": "n", "url": "https://a.example/x" });
        sub.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let config = json!({ "subscriptions": [sub] }).to_string();
        sanitize_config(&config).unwrap()["subscriptions"][0].clone()
    };
    let kept = subscription(json!({ "periodicSpeedTest": true, "speedTestIntervalMinutes": 5 }));
    assert_eq!(kept["periodicSpeedTest"], true);
    assert_eq!(kept["speedTestIntervalMinutes"], 5);
    assert_eq!(
        subscription(json!({ "speedTestIntervalMinutes": 360 }))["speedTestIntervalMinutes"],
        360
    );
    for bad in [
        json!(4),
        json!(361),
        json!(30.5),
        json!("30"),
        json!(-5),
        json!(null),
    ] {
        let sub = subscription(json!({ "speedTestIntervalMinutes": bad }));
        assert!(
            sub.get("speedTestIntervalMinutes").is_none(),
            "{bad} 应被删除而不是钳到边界"
        );
        assert_eq!(sub["id"], "s", "坏值只丢这一个键，订阅本身保留");
    }
    assert!(subscription(json!({ "periodicSpeedTest": "yes" }))
        .get("periodicSpeedTest")
        .is_none());

    let top = |config: Value| sanitize_config(&config.to_string()).unwrap();
    for policy in ["pause", "reduced", "normal"] {
        assert_eq!(
            top(json!({ "speedTestMeteredPolicy": policy }))["speedTestMeteredPolicy"],
            policy
        );
    }
    for bad in [json!("slow"), json!(4), json!(null)] {
        assert!(top(json!({ "speedTestMeteredPolicy": bad }))
            .get("speedTestMeteredPolicy")
            .is_none());
    }
    assert_eq!(
        top(json!({ "periodicSpeedTestEnabled": false }))["periodicSpeedTestEnabled"],
        false
    );
    assert!(top(json!({ "periodicSpeedTestEnabled": "off" }))
        .get("periodicSpeedTestEnabled")
        .is_none());
    // 缺省值是这几处的单一来源。
    const { assert!(PERIODIC_SPEED_TEST_ENABLED_DEFAULT) };
    const { assert!(!PERIODIC_SPEED_TEST_SUBSCRIPTION_DEFAULT) };
    assert_eq!(SPEED_TEST_METERED_POLICY_DEFAULT, "reduced");
    assert_eq!(SPEED_TEST_INTERVAL_MINUTES_DEFAULT, 30);
    assert!(SPEED_TEST_INTERVAL_MINUTES.contains(&SPEED_TEST_INTERVAL_MINUTES_DEFAULT));
}

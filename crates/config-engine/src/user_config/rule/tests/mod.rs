use super::*;

#[test]
fn rule_deserializes_from_polaris_json() {
    // 验证能反序列化 Polaris config.json 里的 Rule 结构（camelCase 键）。
    let json = r#"{
            "id": "r1",
            "type": "domain",
            "values": ["example.com"],
            "action": "proxy",
            "enabled": true,
            "targetServerId": "s2"
        }"#;
    let rule: Rule = serde_json::from_str(json).unwrap();
    assert_eq!(rule.type_field, RuleType::Domain);
    assert_eq!(rule.action, RuleAction::Proxy);
    assert_eq!(rule.target_server_id.as_deref(), Some("s2"));
    assert_eq!(rule.route_action(), Some(RuleAction::Proxy));
    assert!(rule.effects.is_none());
    assert!(rule.conditions.is_none()); // 单条件无 conditions
}

#[test]
fn multi_condition_rule_with_and() {
    let json = r#"{
            "id": "r2",
            "type": "domain",
            "values": ["a.com"],
            "conditions": [
                {"type": "domainSuffix", "values": [".com"]},
                {"type": "ipCidr", "values": ["1.2.3.0/24"]}
            ],
            "combineMode": "and",
            "action": "direct",
            "enabled": true
        }"#;
    let rule: Rule = serde_json::from_str(json).unwrap();
    assert_eq!(rule.combine_mode, Some(CombineMode::And));
    assert_eq!(rule.conditions.as_ref().unwrap().len(), 2);
}

#[test]
fn dns_only_effect_is_authoritative_over_legacy_route_mirror() {
    let json = r#"{
            "id":"dns-only",
            "type":"domainSuffix",
            "values":["example.com"],
            "action":"direct",
            "effects":{"dns":{"resolver":"proxy","answerMode":"fakeIp"}},
            "enabled":true
        }"#;
    let rule: Rule = serde_json::from_str(json).unwrap();
    assert_eq!(rule.route_action(), None);
    assert_eq!(
        rule.dns_effect(),
        Some(RuleDnsEffect {
            enabled: true,
            action: None,
            migrated_implicit_resolve: false,
            resolver: RuleDnsResolver::Proxy,
            answer_mode: RuleDnsAnswerMode::FakeIp,
        })
    );
    let encoded = serde_json::to_value(&rule).unwrap();
    assert_eq!(encoded["effects"]["dns"]["answerMode"], "fakeIp");
    assert!(encoded["effects"].get("route").is_none());
}

/* ══════════════ `CustomAppPreset.packageNames`（2026-09-13 批 16 新增字段）══════════════ */

/// **迁移口径就是「没有迁移」——这条测的就是那句话。**
///
/// 新增一个配置字段最容易漏的不是新形态，而是**旧盘上那些不带这个键的对象**：它们要么照读、
/// 要么整份配置反序列化失败回落默认（`store::sanitize` 对 `customAppPresets` 只做
/// `ensure_array_or_remove`，不逐字段清洗 ⇒ 一条坏就是全盘）。
/// `#[serde(default)]` 让这一格恒是「照读 + 空表」，故不需要 `migrate` pass。
#[test]
fn custom_app_preset_without_package_names_still_reads_as_empty() {
    // 逐字是**本字段出现之前**那份 schema 写出来的对象（`user_config_key_contract.rs` 里那条同形）。
    let json = r#"{
            "id": "custom-foo",
            "name": "Foo",
            "emoji": "🚀",
            "geositeTags": ["foo"],
            "processNames": ["FooApp"],
            "category": "tools"
        }"#;
    let preset: CustomAppPreset = serde_json::from_str(json).expect("旧配置必须照读");
    assert!(
        preset.package_names.is_empty(),
        "不带 packageNames 的旧对象必须读成空表（= 接通之前的行为），实得 {:?}",
        preset.package_names
    );
    // 正向对照：旁边的字段真的读进来了 —— 否则上面那条会被「整个对象都空」骗过。
    assert_eq!(preset.geosite_tags, vec!["foo".to_string()]);
    assert_eq!(
        preset.process_names.as_deref(),
        Some(["FooApp".to_string()].as_slice())
    );
}

/// 带包名的新对象逐字读回，且 **`packageNames` 与 `processNames` 各读各的**
/// （两者是同一件事的两个平台形态，串台就是把 Android 那条腿重新变哑）。
#[test]
fn custom_app_preset_reads_package_names_separately_from_process_names() {
    let json = r#"{
            "id": "custom-foo",
            "name": "Foo",
            "emoji": "🚀",
            "geositeTags": ["foo"],
            "processNames": ["FooApp"],
            "packageNames": ["com.example.foo", "com.example.bar"],
            "category": "tools"
        }"#;
    let preset: CustomAppPreset = serde_json::from_str(json).unwrap();
    assert_eq!(
        preset.package_names,
        vec!["com.example.foo".to_string(), "com.example.bar".to_string()]
    );
    assert_eq!(
        preset.process_names.as_deref(),
        Some(["FooApp".to_string()].as_slice())
    );
}

/// **空表不写回盘**（`skip_serializing_if`）：没挑包名的预设在磁盘上一个字节都不变，
/// 降级回旧版本也读得回去。非空则必须写出来 —— 两半一条测，只写前半会被「这个键永远不发」骗过。
#[test]
fn empty_package_names_are_not_serialized_but_non_empty_are() {
    let base = CustomAppPreset {
        id: "custom-foo".into(),
        name: "Foo".into(),
        emoji: "🚀".into(),
        icon_url: None,
        geosite_tags: vec!["foo".into()],
        geoip_tags: vec![],
        process_names: None,
        package_names: vec![],
        category: None,
    };
    let empty = serde_json::to_value(&base).unwrap();
    assert!(
        empty.get("packageNames").is_none(),
        "空包名表不该写进磁盘，实得 {empty}"
    );
    let filled = serde_json::to_value(CustomAppPreset {
        package_names: vec!["com.example.foo".into()],
        ..base
    })
    .unwrap();
    assert_eq!(
        filled.get("packageNames"),
        Some(&serde_json::json!(["com.example.foo"])),
        "非空包名表必须原样写出去，实得 {filled}"
    );
}

/// **往返**：写出去再读回来恒等（键名 `packageNames` 两侧一致；写成 `package_names` 会在这里红）。
#[test]
fn package_names_round_trip_through_json() {
    let preset = CustomAppPreset {
        id: "custom-foo".into(),
        name: "Foo".into(),
        emoji: "🚀".into(),
        icon_url: None,
        geosite_tags: vec!["foo".into()],
        geoip_tags: vec![],
        process_names: Some(vec!["FooApp".into()]),
        package_names: vec!["com.example.foo".into()],
        category: Some("tools".into()),
    };
    let back: CustomAppPreset =
        serde_json::from_str(&serde_json::to_string(&preset).unwrap()).unwrap();
    assert_eq!(back, preset);
}

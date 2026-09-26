use super::*;
use crate::user_config::rule::{Rule, RuleAction, RuleEffects, RuleRouteEffect, RuleType};

fn deps_default() -> OutboundsDeps {
    OutboundsDeps {
        platform: "linux".into(),
        arch: "x64".into(),
        gate_invalid_nodes: std::collections::BTreeMap::new(),
        system_interface_available: false,
        probe_pool_ports: vec![],
        tailscale_state_dir_prefix: "/fake/ts".into(),
        has_cronet_lib: true,
        log: |_, _| {},
    }
}

#[test]
fn required_interfaces_reuse_generation_priority() {
    use crate::user_config::app_config::{NetworkInterfaceDefaults, SubscriptionInterfacePolicy};

    let mut manual = ServerConfig {
        id: "manual".into(),
        ..Default::default()
    };
    manual.bind_interface = Some("Ethernet".into());
    let mut inherited = ServerConfig {
        id: "inherited".into(),
        ..Default::default()
    };
    inherited.bind_interface = Some(" ".into());
    let mut subscribed = ServerConfig {
        id: "sub-node".into(),
        subscription_id: Some("sub-1".into()),
        ..Default::default()
    };
    // 订阅节点自己的字段必须被忽略，不能让供应方配置夺取本机网卡策略。
    subscribed.bind_interface = Some("provider-owned".into());

    let mut config = UserConfig::default();
    config.network_interfaces = Some(NetworkInterfaceDefaults {
        direct: Some("en0".into()),
        proxy: Some("Wi-Fi".into()),
    });
    config.subscriptions = vec![SubscriptionInterfacePolicy {
        id: "sub-1".into(),
        proxy_bind_interface: Some("utun4".into()),
    }];
    config.servers = vec![manual, inherited, subscribed];

    assert_eq!(
        required_bind_interfaces(&config),
        BTreeSet::from([
            "Ethernet".to_string(),
            "Wi-Fi".to_string(),
            "en0".to_string(),
            "utun4".to_string(),
        ])
    );
}

#[test]
fn two_subscriptions_emit_their_own_bind_interface() {
    use crate::user_config::app_config::SubscriptionInterfacePolicy;

    let make = |id: &str, name: &str, address: &str, subscription: &str| ServerConfig {
        id: id.into(),
        name: name.into(),
        protocol: Protocol::Vless,
        address: address.into(),
        port: 443,
        subscription_id: Some(subscription.into()),
        ..Default::default()
    };
    let mut config = UserConfig::default();
    config.servers = vec![
        make("node-a", "A", "1.1.1.1", "sub-a"),
        make("node-b", "B", "2.2.2.2", "sub-b"),
    ];
    config.selected_server_id = Some("node-a".into());
    config.subscriptions = vec![
        SubscriptionInterfacePolicy {
            id: "sub-a".into(),
            proxy_bind_interface: Some("en0".into()),
        },
        SubscriptionInterfacePolicy {
            id: "sub-b".into(),
            proxy_bind_interface: Some("en7".into()),
        },
    ];

    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let interface_for = |address: &str| {
        result
            .outbounds
            .iter()
            .find(|outbound| outbound.server.as_deref() == Some(address))
            .and_then(|outbound| outbound.extra.get("bind_interface"))
            .and_then(serde_json::Value::as_str)
    };
    assert_eq!(interface_for("1.1.1.1"), Some("en0"));
    assert_eq!(interface_for("2.2.2.2"), Some("en7"));
}

#[test]
fn idle_explicit_interface_is_not_required_until_its_root_becomes_active() {
    let make = |id: &str, address: &str, interface: &str| ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Vless,
        address: address.into(),
        port: 443,
        bind_interface: Some(interface.into()),
        ..Default::default()
    };
    let mut config = UserConfig::default();
    config.servers = vec![
        make("node-a", "1.1.1.1", "en0"),
        make("node-b", "2.2.2.2", "en7"),
    ];
    config.selected_server_id = Some("node-a".into());

    assert_eq!(
        required_bind_interfaces(&config),
        BTreeSet::from(["en0".to_string()]),
        "A/en0 活跃时，闲置 B/en7 缺失不得阻断起核"
    );

    config.selected_server_id = Some("node-b".into());
    assert_eq!(
        required_bind_interfaces(&config),
        BTreeSet::from(["en7".to_string()])
    );
}

#[test]
fn detour_child_binding_neither_emits_nor_becomes_required() {
    let mut child = ServerConfig {
        id: "child".into(),
        name: "child".into(),
        protocol: Protocol::Vless,
        address: "child.example.com".into(),
        port: 443,
        detour: Some("root".into()),
        bind_interface: Some("missing-child-iface".into()),
        ..Default::default()
    };
    let root = ServerConfig {
        id: "root".into(),
        name: "root".into(),
        protocol: Protocol::Vless,
        address: "root.example.com".into(),
        port: 443,
        bind_interface: Some("en0".into()),
        ..Default::default()
    };
    let config = UserConfig {
        servers: vec![child.clone(), root],
        selected_server_id: Some("child".into()),
        ..Default::default()
    };
    assert_eq!(
        required_bind_interfaces(&config),
        BTreeSet::from(["en0".to_string()])
    );

    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let child_outbound = result
        .outbounds
        .iter()
        .find(|outbound| outbound.server.as_deref() == Some("child.example.com"))
        .unwrap();
    assert!(child_outbound.extra.get("bind_interface").is_none());

    // 取消 detour 后该节点自己的策略立即恢复。
    child.detour = None;
    assert_eq!(
        effective_proxy_bind_interface(&child, &UserConfig::default()).as_deref(),
        Some("missing-child-iface")
    );
}

#[test]
fn runtime_binding_fills_only_unconfigured_nodes() {
    let make = |id: &str, address: &str| ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Vless,
        address: address.into(),
        port: 443,
        ..Default::default()
    };
    let mut explicit = make("explicit", "1.1.1.1");
    explicit.bind_interface = Some("en-user".into());
    let mut config = UserConfig::default();
    config.servers = vec![explicit, make("automatic", "2.2.2.2")];
    config.selected_server_id = Some("automatic".into());
    let runtime = BTreeMap::from([
        ("explicit".to_owned(), "en-auto-wrong".to_owned()),
        ("automatic".to_owned(), "en-auto".to_owned()),
    ]);

    let result =
        build_outbounds_with_runtime_bindings(&config, &mut deps_default(), &runtime).unwrap();
    let interface_for = |address: &str| {
        result
            .outbounds
            .iter()
            .find(|outbound| outbound.server.as_deref() == Some(address))
            .and_then(|outbound| outbound.extra.get("bind_interface"))
            .and_then(serde_json::Value::as_str)
    };
    assert_eq!(interface_for("1.1.1.1"), Some("en-user"));
    assert_eq!(interface_for("2.2.2.2"), Some("en-auto"));
}

#[test]
fn single_node_selector_and_direct_without_block() {
    let mut config = UserConfig::default();
    config.servers = vec![ServerConfig {
        id: "s1".into(),
        name: "HK".into(),
        protocol: Protocol::Vless,
        address: "a.com".into(),
        port: 443,
        uuid: Some("u".into()),
        security: Some(SecurityMode::Tls),
        ..Default::default()
    }];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    // 节点 outbound + proxy-selector + direct = 3（legacy block 出站已删）。
    assert!(result.outbounds.len() >= 3);
    assert!(result.outbounds.iter().any(|o| o.tag == "proxy-selector"));
    assert!(result
        .outbounds
        .iter()
        .any(|o| o.tag == "direct" && o.type_field == "direct"));
    assert!(
        !result.outbounds.iter().any(|o| o.tag == "block"),
        "legacy block 出站被复活了 —— 阻断应由规则级 reject 表达"
    );
}

#[test]
fn direct_selection_selector_default() {
    let mut config = UserConfig::default();
    config.servers = vec![ServerConfig {
        id: "s1".into(),
        name: "HK".into(),
        protocol: Protocol::Vless,
        address: "a.com".into(),
        port: 443,
        ..Default::default()
    }];
    config.selected_server_id = Some("__direct__".into()); // 直连哨兵
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let selector = result
        .outbounds
        .iter()
        .find(|o| o.tag == "proxy-selector")
        .unwrap();
    assert_eq!(selector.default.as_deref(), Some("direct"));
}

#[test]
fn direct_policy_lands_on_real_direct_outbound_not_selector() {
    use crate::user_config::app_config::NetworkInterfaceDefaults;

    let config = UserConfig {
        network_interfaces: Some(NetworkInterfaceDefaults {
            direct: Some("en-direct".into()),
            proxy: None,
        }),
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let selector = result
        .outbounds
        .iter()
        .find(|outbound| outbound.tag == PROXY_SELECTOR_TAG)
        .unwrap();
    let direct = result
        .outbounds
        .iter()
        .find(|outbound| outbound.tag == DIRECT_TAG)
        .unwrap();

    assert!(selector.extra.get("bind_interface").is_none());
    assert_eq!(
        direct
            .extra
            .get("bind_interface")
            .and_then(serde_json::Value::as_str),
        Some("en-direct")
    );
}

/// 只有一个节点的配置，用于阻断哨兵三连测。
fn one_node_config(selected: &str) -> UserConfig {
    let mut config = UserConfig::default();
    config.servers = vec![ServerConfig {
        id: "s1".into(),
        name: "HK".into(),
        protocol: Protocol::Vless,
        address: "a.com".into(),
        port: 443,
        ..Default::default()
    }];
    config.selected_server_id = Some(selected.into());
    config
}

/// 阻断哨兵 ⇒ selector default = block 出站 tag。
///
/// 变异锁：把 `outbounds.rs` 的 `else if is_block { BLOCK_TAG }` 腿删掉 → default 落到
/// `node_tags.first()`（"HK"）→ 转红。
#[test]
fn block_selection_selector_default_is_direct_not_block() {
    let config = one_node_config("__block__");
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let selector = result
        .outbounds
        .iter()
        .find(|o| o.tag == "proxy-selector")
        .unwrap();
    // 阻断态下没有任何规则会路由到 proxy-selector（全被 route 改写成 reject），
    // 这里的 default 只是让 selector 结构合法，取 direct。
    assert_eq!(selector.default.as_deref(), Some("direct"));
}

/// 阻断哨兵 ⇒ block 必须**同时**是 selector 成员，否则 sing-box 起核即报 default 不在成员表。
///
/// 变异锁：删掉 `if is_block { selector_members.push(BLOCK_TAG) }` → 转红。
#[test]
fn block_selection_keeps_block_out_of_selector() {
    // 阻断已改由**规则级** `action:"reject"` 表达（见 `builder::route` 末尾），selector 不再承载它。
    // 反向锁：若 `block` 又出现在成员表或 default 上，说明 legacy 出站被复活了。
    let config = one_node_config("__block__");
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let selector = result
        .outbounds
        .iter()
        .find(|o| o.tag == "proxy-selector")
        .unwrap();
    let members = selector.outbounds.as_ref().expect("selector 须有成员表");
    assert!(
        !members.iter().any(|m| m == "block"),
        "block 又进了 selector 成员表：{members:?}"
    );
    let default = selector.default.as_deref().unwrap();
    assert_ne!(default, "block", "selector default 又指回 block 了");
    assert!(
        members.iter().any(|m| m == default),
        "selector default 必须是自己的成员之一：default={default} members={members:?}"
    );
}

/// **未选阻断时 block 不得进成员表** —— 这是金样 37 例逐字节不变的前提（见生成处注释①）。
///
/// 变异锁：把成员 push 改成无条件（去掉 `if is_block`）→ 转红，且 golden_config_snapshot 同时红。
#[test]
fn non_block_selection_keeps_block_out_of_selector_members() {
    for selected in ["s1", "__direct__"] {
        let config = one_node_config(selected);
        let result = build_outbounds(&config, &mut deps_default()).unwrap();
        let selector = result
            .outbounds
            .iter()
            .find(|o| o.tag == "proxy-selector")
            .unwrap();
        let members = selector.outbounds.as_ref().unwrap();
        assert!(
            !members.iter().any(|m| m == "block"),
            "selected={selected} 时 block 不该进 selector 成员表：{members:?}"
        );
    }
}

#[test]
fn rule_sel_generated_for_smart_proxy_rules() {
    let mut config = UserConfig::default();
    config.proxy_mode = crate::user_config::ProxyMode::Smart;
    config.servers = vec![ServerConfig {
        id: "s1".into(),
        name: "HK".into(),
        protocol: Protocol::Vless,
        address: "a.com".into(),
        port: 443,
        ..Default::default()
    }];
    config.selected_server_id = Some("s1".into());
    config.custom_rules = vec![Rule {
        id: "r1".into(),
        type_field: RuleType::Domain,
        values: vec!["example.com".into()],
        action: RuleAction::Proxy,
        enabled: true,
        ..Default::default()
    }];
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    assert!(result.outbounds.iter().any(|o| o.tag == "rule-sel-r1"));
    assert!(result
        .pending_rule_selectors
        .iter()
        .any(|p| p.rule_key == "custom:r1"));
}

#[test]
fn rule_selectors_use_traffic_rules_as_sot() {
    let mut config = UserConfig::default();
    config.proxy_mode = crate::user_config::ProxyMode::Smart;
    config.servers = vec![ServerConfig {
        id: "s1".into(),
        name: "HK".into(),
        protocol: Protocol::Vless,
        address: "a.com".into(),
        port: 443,
        ..Default::default()
    }];
    config.selected_server_id = Some("s1".into());
    config.custom_rules = vec![Rule {
        id: "legacy".into(),
        type_field: RuleType::Domain,
        values: vec!["legacy.example".into()],
        action: RuleAction::Proxy,
        enabled: true,
        ..Default::default()
    }];
    config.traffic_rules = Some(vec![Rule {
        id: "traffic".into(),
        type_field: RuleType::Domain,
        values: vec!["traffic.example".into()],
        action: RuleAction::Proxy,
        enabled: true,
        effects: Some(RuleEffects {
            route: Some(RuleRouteEffect {
                enabled: true,
                action: RuleAction::Proxy,
                target_server_id: Some("s1".into()),
                destination_resolution: None,
                resolution_only: false,
            }),
            dns: None,
        }),
        ..Default::default()
    }]);

    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    assert!(result
        .pending_rule_selectors
        .iter()
        .any(|selector| selector.rule_key == "custom:traffic"));
    assert!(!result
        .pending_rule_selectors
        .iter()
        .any(|selector| selector.rule_key == "custom:legacy"));
}

use crate::user_config::protocol_settings::{ShadowTlsSettings, ShadowsocksSettings};
use crate::user_config::server_config::{Protocol, SecurityMode};

fn ss_server(id: &str, name: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: name.into(),
        address: format!("{id}.example.com"),
        port: 8388,
        protocol: Protocol::Shadowsocks,
        shadowsocks_settings: Some(Box::new(ShadowsocksSettings {
            method: "aes-256-gcm".into(),
            password: "pass".into(),
            plugin: None,
            plugin_opts: None,
        })),
        ..Default::default()
    }
}

#[test]
fn shadow_tls_postprocess_creates_outer_outbound() {
    let mut srv = ss_server("s1", "节点1");
    srv.bind_interface = Some("en7".into());
    srv.shadow_tls_settings = Some(ShadowTlsSettings {
        password: "stls-pass".into(),
        sni: "sni.example.com".into(),
        fingerprint: Some("firefox".into()),
        port: Some(443),
    });
    let mut config = UserConfig::default();
    config.servers = vec![srv];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    // 外层 shadowtls outbound 存在
    let stls = result
        .outbounds
        .iter()
        .find(|o| o.tag == "stls-out-s1")
        .expect("stls-out-s1 应存在");
    assert_eq!(stls.type_field, "shadowtls");
    assert_eq!(stls.server_port, Some(443));
    assert_eq!(stls.version, Some(crate::singbox::OutboundVersion::Num(3)));
    assert_eq!(
        stls.extra
            .get("bind_interface")
            .and_then(serde_json::Value::as_str),
        Some("en7"),
        "真正拨号的 ShadowTLS 外层必须承接网卡绑定"
    );
    // TLS utls fingerprint = firefox
    let tls = stls.tls.as_ref().unwrap();
    assert_eq!(tls.utls.as_ref().unwrap().fingerprint, "firefox");
    assert_eq!(tls.server_name.as_deref(), Some("sni.example.com"));
    // 主 outbound 的 detour 指向 stls-out-s1
    let main = result
        .outbounds
        .iter()
        .find(|o| o.tag.contains("节点1") && o.type_field == "shadowsocks")
        .expect("主 ss outbound 应存在");
    assert_eq!(main.detour.as_deref(), Some("stls-out-s1"));
    assert!(main.extra.get("bind_interface").is_none());
}

#[test]
fn shadow_tls_empty_sni_omits_server_name() {
    let mut srv = ss_server("s1", "节点1");
    srv.shadow_tls_settings = Some(ShadowTlsSettings {
        password: "p".into(),
        sni: String::new(), // 空 → server_name 不输出
        fingerprint: None,  // → 默认 chrome
        port: None,         // → 降级用主端口 8388
    });
    let mut config = UserConfig::default();
    config.servers = vec![srv];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let stls = result
        .outbounds
        .iter()
        .find(|o| o.tag == "stls-out-s1")
        .unwrap();
    assert_eq!(stls.server_port, Some(8388)); // 降级主端口
    assert!(stls.tls.as_ref().unwrap().server_name.is_none()); // 空串 → None
    assert_eq!(
        stls.tls
            .as_ref()
            .unwrap()
            .utls
            .as_ref()
            .unwrap()
            .fingerprint,
        "chrome"
    );
}

/// UI「齐备才写」那道门的**后端侧证据**：`shadowTlsSettings` 一旦存在，后处理只看 `is_some()`，
/// 从不校验内容 —— password 空串 / sni 空串照样造出外层 shadowtls 出站并把 SS 的 detour 指过去。
/// 于是「表单开关一开就写 `{password:'', sni:''}`」= 生成一个**必然连不上**的节点。
///
/// 断言落在**序列化后的 JSON**：`Outbound::password` 带 `skip_serializing_if = "Option::is_none"`，
/// 只断结构体字段的话，哪天这个键被漏出配置也照样绿。
#[test]
fn shadow_tls_empty_credentials_still_emit_unusable_outbound_json() {
    let mut srv = ss_server("s1", "节点1");
    srv.shadow_tls_settings = Some(ShadowTlsSettings {
        password: String::new(), // 旧前端「开关一开就写空壳」的原样形状
        sni: String::new(),
        fingerprint: None,
        port: None,
    });
    let mut config = UserConfig::default();
    config.servers = vec![srv];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let stls = result
        .outbounds
        .iter()
        .find(|o| o.tag == "stls-out-s1")
        .expect("空壳设置照样会造出 shadowtls 出站 —— 这正是前端必须拦在提交前的原因");
    let v = serde_json::to_value(stls).unwrap();
    assert_eq!(v["type"], serde_json::json!("shadowtls"));
    // 空口令原样下发（sing-box 侧握手必失败），且 server_name 整键缺席（无伪装目标）。
    assert_eq!(v["password"], serde_json::json!(""));
    assert!(
        v["tls"].get("server_name").is_none(),
        "sni 空串 → server_name 键缺席"
    );
    // 且 SS 主出站的 detour 已经指过去 ⇒ 该节点的流量全走这条坏链路，用户侧只看到「连不上」。
    let main = result
        .outbounds
        .iter()
        .find(|o| o.type_field == "shadowsocks")
        .expect("主 ss outbound 应存在");
    assert_eq!(main.detour.as_deref(), Some("stls-out-s1"));
}

/// 齐备设置 → 生成的 JSON 逐键就位（前端补齐四颗控件后能产出的形状）。
#[test]
fn shadow_tls_full_settings_emit_expected_outbound_json() {
    let mut srv = ss_server("s1", "节点1");
    srv.shadow_tls_settings = Some(ShadowTlsSettings {
        password: "stls-pass".into(),
        sni: "www.microsoft.com".into(),
        fingerprint: Some("firefox".into()),
        port: Some(8443),
    });
    let mut config = UserConfig::default();
    config.servers = vec![srv];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let v = serde_json::to_value(
        result
            .outbounds
            .iter()
            .find(|o| o.tag == "stls-out-s1")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["password"], serde_json::json!("stls-pass"));
    assert_eq!(v["server"], serde_json::json!("s1.example.com")); // 外层拨的是节点地址
    assert_eq!(v["server_port"], serde_json::json!(8443)); // 真实端口覆盖主端口 8388
    assert_eq!(v["version"], serde_json::json!(3));
    assert_eq!(v["tls"]["enabled"], serde_json::json!(true));
    assert_eq!(
        v["tls"]["server_name"],
        serde_json::json!("www.microsoft.com")
    );
    assert_eq!(
        v["tls"]["utls"]["fingerprint"],
        serde_json::json!("firefox")
    );
}

/// `port: Some(0)` 的降级腿（既有测试只覆盖了 `Some(443)` 与 `None`）。
/// 前端 number 字段清空回 `undefined`（不是 0），但订阅/导入的存量 JSON 可能带 `"port": 0`。
#[test]
fn shadow_tls_zero_port_falls_back_to_node_port() {
    let mut srv = ss_server("s1", "节点1");
    srv.shadow_tls_settings = Some(ShadowTlsSettings {
        password: "p".into(),
        sni: "s.example".into(),
        fingerprint: None,
        port: Some(0),
    });
    let mut config = UserConfig::default();
    config.servers = vec![srv];
    config.selected_server_id = Some("s1".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let v = serde_json::to_value(
        result
            .outbounds
            .iter()
            .find(|o| o.tag == "stls-out-s1")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["server_port"], serde_json::json!(8388)); // 0 → 降级用节点主端口
}

// ── custom 逃生舱在装配层：endpoint 腿真透传 + 形状非法必须留痕（P0 回归锁）────────────
//
// 修复前的 endpoint 腿是 `if let Ok(ep) = from_value::<Endpoint>(val) { push }` ——
// Err 分支**无 push、无 log、无上报**。而 `Endpoint` 只有 WG/TS 的字段集（没有
// `server`/`server_port`/`username`/`password`），于是同一条腿上并存两档坏法，实测都复现：
//   a) 未建模字段 → 解析成功但字段全丢（`openconnect` 的 server/username/password）；
//   b) 与已建模字段类型冲突（`address` 给字符串而非数组）→ **整节点静默消失**。
// 这条腿是 `openvpn-client` / `openconnect` 一族的**唯一**通路（实测塞进 `outbounds[]` 得
// `unknown outbound type`），坏在这里等于那些协议根本没法用。

fn custom_node(id: &str, raw: serde_json::Value, is_endpoint: bool) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Custom,
        address: "unused.example".into(),
        port: 1,
        custom_settings: Some(crate::user_config::protocol_settings::CustomSettings {
            outbound: raw,
            is_endpoint: if is_endpoint { Some(true) } else { None },
            secret_keys: None,
        }),
        ..Default::default()
    }
}

fn build_with_custom(server: ServerConfig) -> (OutboundsResult, OutboundsDeps) {
    let mut config = UserConfig::default();
    config.servers = vec![server];
    config.selected_server_id = Some("__direct__".into()); // 不选中，避免死引用抛错干扰
    let mut deps = deps_default();
    let result = build_outbounds(&config, &mut deps).unwrap();
    (result, deps)
}

/// 🔴 **变异锁：custom endpoint 逐键真透传**（含 `Endpoint` 完全没建模的字段）。
///
/// 取的是随包 sing-box 1.14.0-beta.7 实测 `check` rc=0 的最小合法 `openconnect` 端点
/// —— 它的三个键（server/username/password）在 `Endpoint` 里**一个都没有**，修复前全丢。
#[test]
fn custom_endpoint_passes_through_unmodeled_fields() {
    let raw = serde_json::json!({"type":"openconnect","server":"v.example.com",
        "username":"u","password":"p"});
    let (result, _deps) = build_with_custom(custom_node("e1", raw.clone(), true));
    let ep = result
        .pending_endpoints
        .first()
        .expect("自定义 endpoint 必须发射");
    let mut expected = raw;
    expected["tag"] = serde_json::json!("e1");
    assert_eq!(
        serde_json::to_value(ep).unwrap(),
        expected,
        "custom endpoint 必须逐键原样进 endpoints[]"
    );
}

/// 🔴 **变异锁：与已建模字段类型冲突不得再让节点静默消失**。
///
/// `address` 在 `Endpoint` 里是 `Option<Vec<String>>`，这里给字符串 —— 修复前
/// `from_value::<Endpoint>` 直接 Err，Err 分支空实现 ⇒ `endpoints` 为空、`invalid_nodes` 为空、
/// 日志一个字没有。断言同时钉住「节点还在」与「内容逐键还在」。
#[test]
fn custom_endpoint_type_collision_no_longer_disappears() {
    let raw = serde_json::json!({"type":"wireguard","address":"10.0.0.2/32",
        "private_key":"k","peers":[]});
    let (result, deps) = build_with_custom(custom_node("e1", raw.clone(), true));
    assert_eq!(
        result.pending_endpoints.len(),
        1,
        "节点不得静默消失（修复前此处是 0，且没有任何日志/上报）"
    );
    let mut expected = raw;
    expected["tag"] = serde_json::json!("e1");
    assert_eq!(
        serde_json::to_value(&result.pending_endpoints[0]).unwrap(),
        expected
    );
    assert!(
        deps.gate_invalid_nodes.is_empty(),
        "形状合法 ⇒ 不该上报无效"
    );
}

/// 🔴 **变异锁：形状非法 → 剔除 + 上报，两条腿同判、同 token**。
///
/// 判据（带 string `type` 的对象）与 C10 probe 按钮共用同一个谓词。上报走的是 detour 级联那条
/// **既有**通道（`gate_invalid_nodes` → `InvalidNode` → `EVENT_PROXY_INVALID_NODES`），不是新造的。
///
/// 变异：把 `None =>` 那一支删掉（回到静默）⇒ endpoint 腿断在 `invalid_nodes` 空、
/// outbound 腿断在「下发了 `{"type":"custom"}` 毒丸」。
#[test]
fn custom_malformed_shape_is_reported_on_both_legs() {
    for is_endpoint in [true, false] {
        for raw in [
            serde_json::json!([1, 2, 3]),
            serde_json::json!("hysteria"),
            serde_json::json!({"server":"no-type.example"}),
            serde_json::json!({"type":4}),
        ] {
            let mut config = UserConfig::default();
            config.servers = vec![custom_node("c1", raw.clone(), is_endpoint)];
            config.selected_server_id = Some("__direct__".into());
            let mut deps = deps_default();
            let result = build_outbounds(&config, &mut deps).unwrap();

            assert_eq!(
                deps.gate_invalid_nodes.get("c1").copied(),
                Some(INVALID_REASON_CUSTOM_MALFORMED),
                "isEndpoint={is_endpoint} raw={raw}：必须记进 invalid_nodes（节点消失而不告知比报错更坏）"
            );
            assert!(
                result.pending_endpoints.is_empty(),
                "isEndpoint={is_endpoint} raw={raw}：形状非法的节点不得进 endpoints[]"
            );
            assert!(
                !result
                    .outbounds
                    .iter()
                    .any(|o| o.type_field == "custom" || o.tag == "c1"),
                "isEndpoint={is_endpoint} raw={raw}：形状非法的节点不得进 outbounds[]（尤其不得\
                     下发 `type:\"custom\"` 那颗会让整份配置 FATAL 的毒丸）"
            );
        }
    }
}

/// 全局「TLS 分片」开关对 custom 节点**仍然生效**（载体换了，行为不能跟着丢）。
///
/// 上游 那段 `if (ob.tls && …) ob.tls.fragment = true` 对 custom 是生效的（那边 `ob.tls` 就是
/// 用户 raw 里的 tls 块）。本仓把 raw 挪进 `extra` 之后必须显式走这一条，否则开关静默失效。
/// 同时断言用户 tls 块里的其它键**一个不少** —— 修复前这条腿会把 tls 窄化成本仓建模的字段集。
#[test]
fn global_tls_fragment_still_reaches_custom_outbound_tls_block() {
    let mut config = UserConfig::default();
    config.tls_fragment = Some(true);
    config.servers = vec![custom_node(
        "c1",
        serde_json::json!({"type":"hysteria","server":"h.example.com",
            "tls":{"enabled":true,"server_name":"h.example.com","ca_str":"-----BEGIN..."}}),
        false,
    )];
    config.selected_server_id = Some("__direct__".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let v = serde_json::to_value(result.outbounds.iter().find(|o| o.tag == "c1").unwrap()).unwrap();
    assert_eq!(v["tls"]["fragment"], serde_json::json!(true));
    assert_eq!(
        v["tls"]["ca_str"],
        serde_json::json!("-----BEGIN..."),
        "注入分片不得顺手把用户 tls 块窄化掉"
    );
}

/// QUIC 三协议（hysteria2/tuic/naive）即使走 custom 也不得被注入分片 —— 与建模腿同一条排除。
#[test]
fn global_tls_fragment_skips_quic_managed_custom_outbounds() {
    for ty in ["hysteria2", "tuic", "naive"] {
        let mut config = UserConfig::default();
        config.tls_fragment = Some(true);
        config.servers = vec![custom_node(
            "c1",
            serde_json::json!({"type": ty, "server":"x.example.com","tls":{"enabled":true}}),
            false,
        )];
        config.selected_server_id = Some("__direct__".into());
        let result = build_outbounds(&config, &mut deps_default()).unwrap();
        let v =
            serde_json::to_value(result.outbounds.iter().find(|o| o.tag == "c1").unwrap()).unwrap();
        assert!(
            v["tls"].get("fragment").is_none(),
            "{ty}：QUIC 自管 TLS，分片下发即 FATAL 风险"
        );
    }
}

/// 非 endpoint 的 custom 走通用代理腿 ⇒ 仍然吃到 detour 解析与死引用剪枝
/// （这正是它不另走并行通道的理由：并行通道会让 custom 节点整个逃出剪枝机制）。
#[test]
fn custom_outbound_still_participates_in_detour_resolution() {
    let mut config = UserConfig::default();
    let mut custom = custom_node(
        "c1",
        serde_json::json!({"type":"hysteria","server":"h.example.com","auth_str":"a"}),
        false,
    );
    custom.detour = Some("s1".into());
    config.servers = vec![ss_server("s1", "前置"), custom];
    config.selected_server_id = Some("__direct__".into());
    let result = build_outbounds(&config, &mut deps_default()).unwrap();
    let v = serde_json::to_value(
        result
            .outbounds
            .iter()
            .find(|o| o.tag == "c1")
            .expect("custom outbound 应存在"),
    )
    .unwrap();
    assert_eq!(v["type"], serde_json::json!("hysteria"));
    assert_eq!(v["auth_str"], serde_json::json!("a")); // 透传仍然成立
    assert_eq!(v["detour"], serde_json::json!("前置")); // 外层 detour 由装配层接
}

#[test]
fn detour_dead_reference_on_gate_invalid_pruned() {
    // s1 被 gate 剔除（naive 无 cronet 场景模拟），s2 detour 指向 s1 → s2 detour 死引用被剔。
    // 用 gate_invalid_nodes 预置 s1 无效。
    let s1 = ss_server("s1", "节点1");
    let mut s2 = ss_server("s2", "节点2");
    s2.detour = Some("s1".into()); // s2 经 s1 代理
    let mut config = UserConfig::default();
    config.servers = vec![s1, s2];
    config.selected_server_id = Some("__direct__".into()); // 不选中避免 throw
    let mut deps = deps_default();
    deps.gate_invalid_nodes
        .insert("s1".into(), INVALID_REASON_DETOUR_CASCADE); // s1 被 gate 剔除
    let result = build_outbounds(&config, &mut deps).unwrap();
    // s1 outbound 不存在（gate 剔除）
    assert!(!result.outbounds.iter().any(|o| o.tag.contains("节点1")));
    // s2 的 detour 指向 s1（被剔）→ s2 也被剔（detour 死引用修剪）
    assert!(!result.outbounds.iter().any(|o| o.tag.contains("节点2")));
    // gateInvalidNodes 记录 s2
    assert!(deps.gate_invalid_nodes.contains_key("s2"));
}

#[test]
fn detour_dead_reference_on_selected_gate_invalid_throws() {
    // 选中节点 s2 的 detour 依赖被 gate 剔除的 s1 → throw。
    let s1 = ss_server("s1", "节点1");
    let mut s2 = ss_server("s2", "节点2");
    s2.detour = Some("s1".into());
    let mut config = UserConfig::default();
    config.servers = vec![s1, s2];
    config.selected_server_id = Some("s2".into()); // 选中 s2
    let mut deps = deps_default();
    deps.gate_invalid_nodes
        .insert("s1".into(), INVALID_REASON_DETOUR_CASCADE); // s1 被 gate 剔除 → s2 detour 死引用
    let result = build_outbounds(&config, &mut deps);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("代理链依赖的前置节点不存在"));
}

// ── #335：dial 侧 domain_resolver 形态 ────────────────────────────────────────────
//
// 断言用**精确形状**（整个 `DomainResolver` 值相等），不是「含有 strategy 就算过」：
// 后者对「server 填错 tag」「strategy 填成 ipv4_only」都不转红，而这两种恰恰是本缺陷的
// 复发形态（顶层已经是 ipv4_only，覆盖成同一个值 = 白覆盖）。

/// 一个 vless 代理节点 + 一个域名 server 的 WireGuard 节点。
/// WG 用**域名**而非 IP：`build_wireguard_endpoint` 只对非 IP 字面量下发 domain_resolver，
/// 用 IP 会让 endpoint 那条腿静默出射程。
fn config_with_proxy_and_wg() -> UserConfig {
    let mut config = UserConfig::default();
    let mut wg = ServerConfig {
        id: "w1".into(),
        name: "WG".into(),
        protocol: Protocol::Wireguard,
        address: "wg.example.com".into(),
        port: 51820,
        ..Default::default()
    };
    wg.wireguard_settings = Some(Box::new(
        crate::user_config::server_config::WireGuardSettings {
            private_key: Some("priv".into()),
            peer_public_key: Some("pub".into()),
            local_address: vec!["10.0.0.2/32".into()],
            allow_internet: Some(true),
            ..Default::default()
        },
    ));
    config.servers = vec![
        ServerConfig {
            id: "s1".into(),
            name: "HK".into(),
            protocol: Protocol::Vless,
            address: "a.example.com".into(),
            port: 443,
            uuid: Some("u".into()),
            ..Default::default()
        },
        wg,
    ];
    config.selected_server_id = Some("s1".into());
    config
}

#[test]
fn dial_domain_resolver_is_structured_when_ipv6_off() {
    let mut config = config_with_proxy_and_wg();
    config.enable_ipv6 = Some(false);
    let result = build_outbounds(&config, &mut deps_default()).unwrap();

    // 期望 tag：`UserConfig::default()` 的 `resolveNodeDomainsAhead` 未设 ⇒ race **on**
    // （`is_node_race_on`：只有显式 `Some(false)` 才关）⇒ dial 解析器是 `dns-node-race`。
    let expected = DomainResolver::Detailed {
        server: "dns-node-race".into(),
        strategy: crate::singbox::DomainStrategy::PreferIpv4,
    };

    let node = result
        .outbounds
        .iter()
        .find(|o| o.type_field == "vless")
        .expect("vless outbound 应存在");
    assert_eq!(node.domain_resolver.as_ref(), Some(&expected));

    let ep = result
        .pending_endpoints
        .iter()
        .find(|e| e.type_field == "wireguard")
        .expect("wireguard endpoint 应存在");
    assert_eq!(ep.domain_resolver.as_ref(), Some(&expected));

    // direct 拨的是目标站点，**刻意**保持纯 tag（#57 的 AAAA 抑制在那条腿上是收益不是 bug）。
    let direct = result
        .outbounds
        .iter()
        .find(|o| o.tag == DIRECT_TAG)
        .expect("direct outbound 应存在");
    assert!(matches!(
        direct.domain_resolver.as_ref(),
        Some(DomainResolver::Tag(_))
    ));
}

#[test]
fn dial_domain_resolver_stays_plain_tag_when_ipv6_on() {
    let mut config = config_with_proxy_and_wg();
    config.enable_ipv6 = Some(true);
    let result = build_outbounds(&config, &mut deps_default()).unwrap();

    // 顶层 dns.strategy 此时已是 prefer_ipv4，无需覆盖 ⇒ 形态必须与修复前逐字节一致。
    let expected = DomainResolver::Tag("dns-node-race".into());
    let node = result
        .outbounds
        .iter()
        .find(|o| o.type_field == "vless")
        .expect("vless outbound 应存在");
    assert_eq!(node.domain_resolver.as_ref(), Some(&expected));
    let ep = result
        .pending_endpoints
        .iter()
        .find(|e| e.type_field == "wireguard")
        .expect("wireguard endpoint 应存在");
    assert_eq!(ep.domain_resolver.as_ref(), Some(&expected));

    // 序列化到 JSON 也必须是**裸字符串**（金样 delta 不得落到 enableIPv6=true 这一支上）。
    assert_eq!(
        serde_json::to_value(&node.domain_resolver).unwrap(),
        serde_json::json!("dns-node-race")
    );
}

// ── on_demand（sing-box 1.15）：必须抵达**每一条** endpoint 腿 ─────────────────────
//
// 遗漏一条腿的形态是**静默的**：那个协议的节点照常生成、`sing-box check` 照常 rc=0，只是永远
// 不会挂起。用户侧表现为「开关拨了没反应」，而任何"看生成结果非空"的门都是绿的。
// 故取材面从 `lands_in_endpoints` 反推（新增 endpoint 协议自动入列），而不是手抄一份协议清单。

/// 造一个该协议下**能真正生成出 endpoint** 的最小节点。
fn endpoint_node_for(protocol: Protocol, on_demand: Option<bool>) -> ServerConfig {
    let mut server = ServerConfig {
        id: "e1".into(),
        name: "E1".into(),
        protocol,
        address: "vpn.example.com".into(),
        port: 443,
        ..Default::default()
    };
    server.on_demand = on_demand;
    if protocol == Protocol::Wireguard {
        server.port = 51820;
        server.wireguard_settings = Some(Box::new(
            crate::user_config::server_config::WireGuardSettings {
                private_key: Some("priv".into()),
                peer_public_key: Some("pub".into()),
                local_address: vec!["10.0.0.2/32".into()],
                // 不设 allow_internet=false，否则 `is_mesh_node_unroutable` 会把它整条跳过，
                // 断言就退化成"节点不存在也算过"。
                allow_internet: Some(true),
                ..Default::default()
            },
        ));
    }
    server
}

fn build_single(server: ServerConfig) -> OutboundsResult {
    let mut config = UserConfig::default();
    config.servers = vec![server];
    config.selected_server_id = Some("__direct__".into());
    let mut deps = deps_default();
    build_outbounds(&config, &mut deps).expect("最小节点应能生成")
}

fn vpn_client_node(protocol: Protocol) -> ServerConfig {
    let mut vpn = endpoint_node_for(protocol, None);
    vpn.openconnect_settings = (protocol == Protocol::Openconnect).then(|| {
        Box::new(crate::user_config::protocol_settings::OpenconnectSettings {
            server: Some("vpn.example.com:443".into()),
            ..Default::default()
        })
    });
    vpn.openvpn_client_settings = (protocol == Protocol::OpenvpnClient).then(|| {
        Box::new(
            crate::user_config::protocol_settings::OpenvpnClientSettings {
                server: Some("vpn.example.com".into()),
                server_port: Some(1194),
                tls: Some(Default::default()),
                ..Default::default()
            },
        )
    });
    vpn
}

fn vpn_forward_node() -> ServerConfig {
    ServerConfig {
        id: "forward-id".into(),
        name: "Forward SOCKS".into(),
        protocol: Protocol::Socks,
        address: "127.0.0.1".into(),
        port: 1080,
        ..Default::default()
    }
}

#[test]
fn vpn_client_endpoints_resolve_local_detour_ids_to_outbound_tags() {
    let forward = vpn_forward_node();
    for protocol in [Protocol::Openconnect, Protocol::OpenvpnClient] {
        let mut vpn = vpn_client_node(protocol);
        vpn.detour = Some(forward.id.clone());
        let config = UserConfig {
            selected_server_id: Some(vpn.id.clone()),
            servers: vec![vpn, forward.clone()],
            ..Default::default()
        };
        let result = build_outbounds(&config, &mut deps_default()).unwrap();
        let forward_tag = result
            .outbounds
            .iter()
            .find(|outbound| outbound.server.as_deref() == Some("127.0.0.1"))
            .map(|outbound| outbound.tag.as_str())
            .expect("前置代理应生成 outbound");
        let endpoint = result
            .pending_endpoints
            .iter()
            .find(|endpoint| {
                endpoint.type_field == crate::builder::outbound::protocol_str(protocol)
            })
            .expect("VPN 客户端应生成 endpoint");
        assert_eq!(endpoint.detour.as_deref(), Some(forward_tag));
        assert_ne!(endpoint.detour.as_deref(), Some("forward-id"));
        assert_eq!(
            serde_json::to_value(endpoint).unwrap()["detour"],
            serde_json::json!(forward_tag)
        );
    }
}

#[test]
fn vpn_client_endpoints_reject_missing_self_cyclic_and_endpoint_detours() {
    for protocol in [Protocol::Openconnect, Protocol::OpenvpnClient] {
        for case in ["missing", "self", "cycle", "endpoint"] {
            let mut vpn = vpn_client_node(protocol);
            let mut other = Vec::new();
            match case {
                "missing" => vpn.detour = Some("absent".into()),
                "self" => vpn.detour = Some(vpn.id.clone()),
                "cycle" => {
                    vpn.detour = Some("forward-id".into());
                    let mut forward = vpn_forward_node();
                    forward.detour = Some(vpn.id.clone());
                    other.push(forward);
                }
                "endpoint" => {
                    vpn.detour = Some("other-endpoint".into());
                    other.push(mesh_node("other-endpoint", Protocol::Tailscale, false));
                }
                _ => unreachable!(),
            }
            let config = UserConfig {
                selected_server_id: Some(vpn.id.clone()),
                servers: std::iter::once(vpn).chain(other).collect(),
                ..Default::default()
            };
            let result = build_outbounds(&config, &mut deps_default()).unwrap();
            let endpoint = result
                .pending_endpoints
                .iter()
                .find(|ep| ep.type_field == crate::builder::outbound::protocol_str(protocol))
                .expect("VPN 客户端仍应生成");
            assert!(endpoint.detour.is_none(), "{protocol:?}: {case}");
        }
    }
}

fn mesh_node(id: &str, protocol: Protocol, system: bool) -> ServerConfig {
    let mut node = endpoint_node_for(protocol, None);
    node.id = id.into();
    node.name = id.into();
    match protocol {
        Protocol::Tailscale => {
            node.tailscale_settings = Some(Box::new(
                crate::user_config::server_config::TailscaleSettings {
                    reverse_mesh: Some(system),
                    ..Default::default()
                },
            ));
        }
        Protocol::Wireguard => {
            node.wireguard_settings.as_mut().unwrap().reverse_mesh = Some(system);
        }
        _ => unreachable!(),
    }
    node
}

#[test]
fn system_mesh_interface_conflict_rejects_only_duplicate_active_system_type() {
    for (protocol, interface) in [
        (Protocol::Tailscale, "polaris-ts"),
        (Protocol::Wireguard, "polaris-wg"),
    ] {
        let config = UserConfig {
            selected_server_id: Some("a".into()),
            servers: vec![
                mesh_node("a", protocol, true),
                mesh_node("b", protocol, true),
            ],
            ..Default::default()
        };
        let mut deps = deps_default();
        deps.system_interface_available = true;
        let error = build_outbounds(&config, &mut deps).unwrap_err();
        assert!(error.contains(interface), "{error}");
        assert!(error.contains('a') && error.contains('b'), "{error}");

        let mut userspace = config.clone();
        userspace.servers[1] = mesh_node("b", protocol, false);
        let result = build_outbounds(&userspace, &mut deps).unwrap();
        assert_eq!(result.pending_endpoints.len(), 2);

        userspace.servers[0] = mesh_node("a", protocol, false);
        let result = build_outbounds(&userspace, &mut deps).unwrap();
        assert_eq!(result.pending_endpoints.len(), 2);

        let mut unavailable = deps_default();
        let result = build_outbounds(&config, &mut unavailable).unwrap();
        assert_eq!(result.pending_endpoints.len(), 2);
        assert!(result
            .pending_endpoints
            .iter()
            .all(|ep| { ep.system != Some(true) && ep.system_interface != Some(true) }));
    }

    let config = UserConfig {
        selected_server_id: Some("ts".into()),
        servers: vec![
            mesh_node("ts", Protocol::Tailscale, true),
            mesh_node("wg", Protocol::Wireguard, true),
        ],
        ..Default::default()
    };
    let mut deps = deps_default();
    deps.system_interface_available = true;
    let result = build_outbounds(&config, &mut deps).unwrap();
    assert_eq!(result.pending_endpoints.len(), 2);

    // custom endpoint 透传袋也会成为真正的 System 接口，不能绕过同名冲突守卫。
    let config = UserConfig {
        selected_server_id: Some("ts".into()),
        servers: vec![
            mesh_node("ts", Protocol::Tailscale, true),
            custom_node(
                "custom-ts",
                serde_json::json!({ "type": "tailscale", "system_interface": true }),
                true,
            ),
        ],
        ..Default::default()
    };
    let error = build_outbounds(&config, &mut deps).unwrap_err();
    assert!(error.contains("polaris-ts"), "{error}");
}

#[test]
fn on_demand_reaches_every_endpoint_leg() {
    use crate::user_config::server_config::{lands_in_endpoints, ALL_PROTOCOLS};

    let endpoint_protocols: Vec<Protocol> = ALL_PROTOCOLS
        .into_iter()
        .filter(|p| lands_in_endpoints(*p))
        .collect();
    // 取材面自检：判据塌成空集时，下面的循环会平凡通过。
    assert!(
        endpoint_protocols.len() >= 4,
        "lands_in_endpoints 只认出 {} 个协议 —— 取材面塌了，本门此刻是假绿",
        endpoint_protocols.len()
    );

    for protocol in endpoint_protocols {
        let name = crate::builder::outbound::protocol_str(protocol);

        // ① 设了 true ⇒ 必须抵达生成结果。
        let result = build_single(endpoint_node_for(protocol, Some(true)));
        let ep = result
            .pending_endpoints
            .iter()
            .find(|e| e.tag == "E1")
            .unwrap_or_else(|| panic!("{name}：最小节点没生成 endpoint，断言无从谈起"));
        assert_eq!(
            ep.on_demand,
            Some(true),
            "{name}：on_demand 没抵达这条腿 —— 症状是「开关拨了没反应」，且所有看非空的门都绿"
        );
        // 序列化面也要有（`skip_serializing_if` 写错会让类型化字段在 JSON 里凭空消失）。
        let json = serde_json::to_value(ep).unwrap();
        assert_eq!(
            json.get("on_demand"),
            Some(&serde_json::Value::Bool(true)),
            "{name}：on_demand 在序列化后丢了"
        );

        // ② 未设置 ⇒ **一个键都不发**（与本字段出现之前逐字节等价）。
        let result = build_single(endpoint_node_for(protocol, None));
        let ep = result
            .pending_endpoints
            .iter()
            .find(|e| e.tag == "E1")
            .unwrap();
        assert_eq!(ep.on_demand, None, "{name}：未设置却发了 on_demand");
        let json = serde_json::to_value(ep).unwrap();
        assert!(
            json.get("on_demand").is_none(),
            "{name}：未设置却在 JSON 里出现 on_demand —— 会改动金样字节"
        );
    }
}

/// custom（`isEndpoint`）逃生舱这条腿：`lands_in_endpoints` 看不到它（它看节点设置不看协议），
/// 故必须单独钉一次，否则唯一能跑 openvpn/openconnect 手写配置的那条通路无人覆盖。
#[test]
fn on_demand_reaches_the_custom_endpoint_leg() {
    let raw = serde_json::json!({ "type": "openconnect", "server": "vpn.example.com" });

    let mut node = custom_node("c1", raw.clone(), true);
    node.on_demand = Some(true);
    let (result, _) = build_with_custom(node);
    let ep = result
        .pending_endpoints
        .iter()
        .find(|e| e.tag == "c1")
        .expect("custom endpoint 应存在");
    assert_eq!(ep.on_demand, Some(true), "custom 逃生舱漏了 on_demand");

    // 未设置 ⇒ 不碰 `extra`：raw JSON 里手写的 on_demand 照常原样透传（逃生舱既有语义）。
    let raw_with_key =
        serde_json::json!({ "type": "openconnect", "server": "v.example.com", "on_demand": true });
    let (result, _) = build_with_custom(custom_node("c2", raw_with_key, true));
    let ep = result
        .pending_endpoints
        .iter()
        .find(|e| e.tag == "c2")
        .unwrap();
    let json = serde_json::to_value(ep).unwrap();
    assert_eq!(
        json.get("on_demand"),
        Some(&serde_json::Value::Bool(true)),
        "raw 里手写的 on_demand 被吃掉了"
    );

    // 两者同时存在 ⇒ 类型化字段赢，且 JSON 里**只能有一个** on_demand（flatten 重键会出两次）。
    let raw_conflict = serde_json::json!({
        "type": "openconnect", "server": "v.example.com", "on_demand": true
    });
    let mut node = custom_node("c3", raw_conflict, true);
    node.on_demand = Some(false);
    let (result, _) = build_with_custom(node);
    let ep = result
        .pending_endpoints
        .iter()
        .find(|e| e.tag == "c3")
        .unwrap();
    let text = serde_json::to_string(ep).unwrap();
    assert_eq!(
        text.matches("\"on_demand\"").count(),
        1,
        "on_demand 在 JSON 里出现了不止一次（flatten 重键）：{text}"
    );
    assert_eq!(
        serde_json::to_value(ep).unwrap().get("on_demand"),
        Some(&serde_json::Value::Bool(false)),
        "ServerConfig.onDemand 应当压过 raw 里的同名键"
    );
}

// ── Tailcat（2026-09-24）──

const TC_PUB: &str = "lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=";
const TC_DISCO: &str = "qQ+kiWwZ8BrTYDZpj+6bnx2JxWxx0SAh1krqPGndCmQ=";

fn tailcat_node(
    id: &str,
    settings: crate::user_config::protocol_settings::TailcatSettings,
) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailcat,
        tailcat_settings: Some(Box::new(settings)),
        ..Default::default()
    }
}

fn tailcat_region() -> crate::user_config::protocol_settings::TailcatSettings {
    crate::user_config::protocol_settings::TailcatSettings {
        server_public_key: Some(TC_PUB.into()),
        server_disco_key: Some(TC_DISCO.into()),
        derp_region: Some(1),
        ..Default::default()
    }
}

/// 🔴 **剔节点判据逐条照内核**：下面每个坏形态都是随包 1.15.0-alpha.7 `check` 实测整核失败的写法
/// （无填充 / url-safe / hex / 首尾空白的 key、缺服务端 key、DERP 两者都有或都没有、region ≤ 0、
/// servers 项缺 host 或非串非对象）⇒ 必须剔除并以对应 token 上报；好形态（含内核放行的空 PSK）必须发射。
#[test]
fn tailcat_emit_check_drops_every_core_killing_shape() {
    use crate::user_config::protocol_settings::{
        TailcatSettings, INVALID_REASON_TAILCAT_DERP, INVALID_REASON_TAILCAT_KEY,
    };
    let key = |f: fn(&mut TailcatSettings)| {
        let mut t = tailcat_region();
        f(&mut t);
        t
    };
    let bad: Vec<(&str, TailcatSettings, &str)> = vec![
        (
            "no-pub",
            key(|t| t.server_public_key = None),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "no-disco",
            key(|t| t.server_disco_key = None),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "nopad",
            key(|t| t.server_public_key = Some(TC_PUB.trim_end_matches('=').into())),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "urlsafe",
            key(|t| t.server_disco_key = Some(TC_DISCO.replace('+', "-"))),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "hex",
            key(|t| {
                t.server_public_key =
                    Some("94f2c31cfd18a2b10d428baa812531d461eefb739c0dcfc5ef5677ae83134b2e".into())
            }),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "space",
            key(|t| t.server_public_key = Some(format!(" {}", &TC_PUB[1..]))),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "bad-psk",
            key(|t| t.pre_shared_key = Some("x".into())),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "bad-priv",
            key(|t| t.private_key = Some(TC_PUB.replace('=', "A"))),
            INVALID_REASON_TAILCAT_KEY,
        ),
        (
            "no-derp",
            key(|t| t.derp_region = None),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "region-0",
            key(|t| t.derp_region = Some(0)),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "region-neg",
            key(|t| t.derp_region = Some(-1)),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "both",
            key(|t| t.derp_servers = vec![serde_json::json!("d.example")]),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "srv-empty",
            key(|t| {
                t.derp_region = None;
                t.derp_servers = vec![serde_json::json!("")]
            }),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "srv-nohost",
            key(|t| {
                t.derp_region = None;
                t.derp_servers = vec![serde_json::json!({"ipv4":"192.0.2.1"})]
            }),
            INVALID_REASON_TAILCAT_DERP,
        ),
        (
            "srv-num",
            key(|t| {
                t.derp_region = None;
                t.derp_servers = vec![serde_json::json!(1)]
            }),
            INVALID_REASON_TAILCAT_DERP,
        ),
    ];
    let good: Vec<(&str, TailcatSettings)> = vec![
        ("region", tailcat_region()),
        ("empty-psk", key(|t| t.pre_shared_key = Some(String::new()))),
        (
            "servers-mixed",
            key(|t| {
                t.derp_region = None;
                t.derp_servers = vec![
                    serde_json::json!("d1.example"),
                    serde_json::json!({"host":"d2.example"}),
                ];
            }),
        ),
    ];
    let mut config = UserConfig::default();
    config.servers = bad
        .iter()
        .map(|(id, t, _)| tailcat_node(id, t.clone()))
        .chain(good.iter().map(|(id, t)| tailcat_node(id, t.clone())))
        .collect();
    // 缺设置块本身也不合格（没有服务端 key）。
    config.servers.push(ServerConfig {
        id: "no-settings".into(),
        name: "no-settings".into(),
        protocol: Protocol::Tailcat,
        ..Default::default()
    });
    config.selected_server_id = Some("__direct__".into());
    let mut deps = deps_default();
    let result = build_outbounds(&config, &mut deps).unwrap();
    let emitted: Vec<&str> = result
        .outbounds
        .iter()
        .filter(|o| o.type_field == "tailcat")
        .map(|o| o.tag.as_str())
        .collect();
    for (id, _, token) in &bad {
        assert_eq!(
            deps.gate_invalid_nodes.get(*id).copied(),
            Some(*token),
            "{id}"
        );
        assert!(!emitted.contains(id), "{id} 坏形态被下发了");
    }
    assert_eq!(
        deps.gate_invalid_nodes.get("no-settings").copied(),
        Some(INVALID_REASON_TAILCAT_KEY)
    );
    for (id, _) in &good {
        assert!(
            emitted.contains(id),
            "{id} 好形态被误剔：{:?}",
            deps.gate_invalid_nodes
        );
    }
}

/// 🔴 **`http_client` 只在 region 模式出现、缺省 `direct`、有前置代理时跟随它（D10 选项 C）**；
/// servers 模式三者全无（与 derp_servers 互斥）；透传袋里的生成侧键（`http_client` / `detour` /
/// `server` / 另一模式的 DERP 键）一律剥掉，袋里的无害键原样下发（证明合并确实发生过）。
#[test]
fn tailcat_http_client_is_region_only_and_follows_detour() {
    let mut chained = tailcat_node("chained", tailcat_region());
    chained.detour = Some("s".into());
    let mut bag = serde_json::Map::new();
    bag.insert(
        "http_client".into(),
        serde_json::json!({"detour": "proxy-selector"}),
    );
    bag.insert("detour".into(), serde_json::json!("x"));
    bag.insert("server".into(), serde_json::json!("x.example"));
    bag.insert("derp_region".into(), serde_json::json!(9));
    bag.insert("udp_timeout".into(), serde_json::json!("5m"));
    let mut servers = tailcat_node(
        "servers",
        crate::user_config::protocol_settings::TailcatSettings {
            derp_region: None,
            derp_map_url: Some("https://left-over.example/m.json".into()),
            derp_servers: vec![serde_json::json!("d.example")],
            extra: bag,
            ..tailcat_region()
        },
    );
    servers.detour = Some("s".into());
    let mut config = UserConfig::default();
    config.servers = vec![
        ServerConfig {
            id: "s".into(),
            name: "SOCKS".into(),
            protocol: Protocol::Socks,
            address: "1.2.3.4".into(),
            port: 1080,
            ..Default::default()
        },
        tailcat_node("plain", tailcat_region()),
        chained,
        servers,
    ];
    config.selected_server_id = Some("plain".into());
    let mut deps = deps_default();
    let result = build_outbounds(&config, &mut deps).unwrap();
    let get = |tag: &str| {
        serde_json::to_value(result.outbounds.iter().find(|o| o.tag == tag).unwrap()).unwrap()
    };
    assert_eq!(
        get("plain")["http_client"],
        serde_json::json!({"detour": "direct"})
    );
    assert_eq!(
        get("chained")["http_client"],
        serde_json::json!({"detour": "SOCKS"})
    );
    assert_eq!(get("chained")["detour"], serde_json::json!("SOCKS"));
    let s = get("servers");
    for k in [
        "http_client",
        "derp_region",
        "derp_map_url",
        "server",
        "server_port",
    ] {
        assert!(s.get(k).is_none(), "servers 模式不得带 `{k}`：{s:#}");
    }
    assert_eq!(
        s["detour"],
        serde_json::json!("SOCKS"),
        "袋里的 detour 不得盖过装配层的真值"
    );
    assert_eq!(
        s["udp_timeout"],
        serde_json::json!("5m"),
        "袋里的无害键应原样下发"
    );
    assert_eq!(s["derp_servers"], serde_json::json!(["d.example"]));
}

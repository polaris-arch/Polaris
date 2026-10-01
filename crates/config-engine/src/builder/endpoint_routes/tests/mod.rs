use super::*;
use crate::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings, WireGuardSettings,
};

fn wg_server(id: &str, allowed: &[&str], allow_internet: Option<bool>) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Wireguard,
        address: "1.2.3.4".into(),
        port: 443,
        wireguard_settings: Some(Box::new(WireGuardSettings {
            allowed_ips: allowed.iter().map(|s| s.to_string()).collect(),
            allow_internet,
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn ts_server(id: &str, exit_node: Option<&str>, routes: &[&str]) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailscale,
        tailscale_settings: Some(Box::new(TailscaleSettings {
            exit_node: exit_node.map(String::from),
            routes: routes.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[test]
fn wg_forced_route_strips_catch_all() {
    let s = wg_server("w1", &["10.0.0.0/24", "0.0.0.0/0"], Some(true));
    assert_eq!(
        endpoint_forced_route_cidrs(&s, &ObservedTailnetAddresses::new()),
        vec!["10.0.0.0/24".to_string()]
    );
}

#[test]
fn ts_forced_route_includes_tailnet() {
    let s = ts_server("t1", None, &["192.168.10.0/24"]);
    let cidrs = endpoint_forced_route_cidrs(&s, &ObservedTailnetAddresses::new());
    assert!(cidrs.contains(&TAILNET_CGNAT.to_string()));
    assert!(cidrs.contains(&TAILNET_ULA_V6.to_string()));
    assert!(cidrs.contains(&"192.168.10.0/24".to_string()));
}

#[test]
fn mesh_allows_internet_works() {
    assert!(mesh_allows_internet(&wg_server(
        "w",
        &["10.0.0.0/24"],
        Some(true)
    )));
    assert!(!mesh_allows_internet(&wg_server(
        "w",
        &["10.0.0.0/24"],
        Some(false)
    )));
    assert!(mesh_allows_internet(&wg_server(
        "w",
        &["10.0.0.0/24"],
        None
    ))); // 缺省 true
    assert!(mesh_allows_internet(&ts_server("t", Some("exit"), &[])));
    assert!(!mesh_allows_internet(&ts_server("t", None, &[])));
    assert!(!mesh_allows_internet(&ts_server("t", Some("  "), &[])));
}

#[test]
fn warp_ignores_legacy_custom_route_fields() {
    let mut s = wg_server("warp", &["10.0.0.0/24"], Some(false));
    s.address = "engage.cloudflareclient.com".into();
    assert!(mesh_allows_internet(&s), "WARP 恒为云出口");
    assert!(endpoint_forced_route_cidrs(&s, &ObservedTailnetAddresses::new()).is_empty());
    assert_eq!(
        wireguard_peer_allowed_ips(&s),
        Some(
            FULL_TUNNEL_CIDRS
                .iter()
                .map(|cidr| cidr.to_string())
                .collect()
        )
    );
}

#[test]
fn force_route_always_on() {
    let s = wg_server("w", &["10.0.0.0/24"], None); // alwaysRoute 缺省 true
    let targeted = BTreeSet::new();
    assert!(should_force_route_subnets(&s, None, &targeted));
}

#[test]
fn force_route_off_engaged_by_selection() {
    let mut s = wg_server("w", &["10.0.0.0/24"], None);
    s.wireguard_settings.as_mut().unwrap().always_route_subnets = Some(false);
    let targeted = BTreeSet::new();
    assert!(!should_force_route_subnets(&s, Some("other"), &targeted));
    assert!(should_force_route_subnets(&s, Some("w"), &targeted)); // 选中
}

#[test]
fn force_route_off_engaged_by_rule() {
    let mut s = wg_server("w", &["10.0.0.0/24"], None);
    s.wireguard_settings.as_mut().unwrap().always_route_subnets = Some(false);
    let mut targeted = BTreeSet::new();
    targeted.insert("w".into());
    assert!(should_force_route_subnets(&s, None, &targeted));
}

#[test]
fn collect_targeted_from_rules() {
    use crate::user_config::rule::{CombineMode, Rule, RuleType};
    let rules = vec![
        Rule {
            id: "r1".into(),
            type_field: RuleType::Domain,
            values: vec!["a.com".into()],
            conditions: None,
            combine_mode: None,
            effects: None,
            action: RuleAction::Proxy,
            enabled: true,
            bypass_fakeip: None,
            target_server_id: Some("s2".into()),
            remarks: None,
            tls_spoof: None,
            tls_spoof_method: None,
            network_profile_id: None,
        },
        Rule {
            id: "r2".into(),
            type_field: RuleType::Domain,
            values: vec!["b.com".into()],
            conditions: None,
            combine_mode: Some(CombineMode::And),
            effects: None,
            action: RuleAction::Direct, // 非 proxy，不含
            enabled: true,
            bypass_fakeip: None,
            target_server_id: Some("s3".into()),
            remarks: None,
            tls_spoof: None,
            tls_spoof_method: None,
            network_profile_id: None,
        },
    ];
    let ids = collect_rule_targeted_server_ids(&rules);
    assert!(ids.contains("s2"));
    assert!(!ids.contains("s3")); // direct 不算
}

#[test]
fn mesh_forced_route_union() {
    let servers = [
        wg_server("w1", &["10.0.0.0/24"], None),
        wg_server("w2", &["10.0.0.0/24", "172.16.0.0/24"], None), // 10.0 重复
    ];
    let cidrs = mesh_forced_route_cidrs(&servers, &ObservedTailnetAddresses::new());
    assert_eq!(cidrs.len(), 2); // 去重
    assert!(cidrs.contains(&"10.0.0.0/24".to_string()));
    assert!(cidrs.contains(&"172.16.0.0/24".to_string()));
}

#[test]
fn catch_all_detection() {
    assert!(has_catch_all(&["0.0.0.0/0".into()]));
    assert!(has_catch_all(&["::/0".into(), "10.0.0.0/8".into()]));
    assert!(!has_catch_all(&["10.0.0.0/8".into()]));
    assert_eq!(
        strip_catch_all(&["0.0.0.0/0".into(), "10.0.0.0/8".into()]),
        vec!["10.0.0.0/8".to_string()]
    );
}

#[test]
fn referenced_ids_includes_selected_and_endpoint() {
    use crate::user_config::app_config::UserConfig;
    let mut config = UserConfig::default();
    config.servers = vec![
        ServerConfig {
            id: "s1".into(),
            name: "普通节点".into(),
            protocol: Protocol::Shadowsocks,
            address: "1.1.1.1".into(),
            port: 443,
            ..Default::default()
        },
        wg_server("wg1", &["10.0.0.0/24"], None),
    ];
    config.selected_server_id = Some("s1".into());
    let refs = referenced_server_ids(&config);
    // s1 选中 + wg1 是 endpoint（保守纳入）
    assert!(refs.contains("s1"));
    assert!(refs.contains("wg1"));
}

#[test]
fn referenced_ids_detour_transitive_closure() {
    use crate::user_config::app_config::UserConfig;
    // s1 经 s2 代理链（detour），s2 经 s3 → 全闭包 {s1,s2,s3}
    let mut s1 = ServerConfig {
        id: "s1".into(),
        name: "s1".into(),
        protocol: Protocol::Shadowsocks,
        address: "1.1.1.1".into(),
        port: 443,
        detour: Some("s2".into()),
        ..Default::default()
    };
    let mut s2 = ServerConfig {
        id: "s2".into(),
        name: "s2".into(),
        protocol: Protocol::Shadowsocks,
        address: "2.2.2.2".into(),
        port: 443,
        detour: Some("s3".into()),
        ..Default::default()
    };
    let s3 = ServerConfig {
        id: "s3".into(),
        name: "s3".into(),
        protocol: Protocol::Shadowsocks,
        address: "3.3.3.3".into(),
        port: 443,
        ..Default::default()
    };
    let config = UserConfig {
        servers: vec![s1.clone(), s2.clone(), s3],
        selected_server_id: Some("s1".into()),
        ..Default::default()
    };
    let refs = referenced_server_ids(&config);
    assert!(refs.contains("s1"));
    assert!(refs.contains("s2"));
    assert!(refs.contains("s3"));
    // 恢复（避免 borrow 问题，此处不再用）
    s1.detour = None;
    s2.detour = None;
}

#[test]
fn referenced_ids_direct_sentinel_excluded() {
    use crate::user_config::app_config::UserConfig;
    let config = UserConfig {
        servers: vec![ServerConfig {
            id: "s1".into(),
            name: "s1".into(),
            protocol: Protocol::Shadowsocks,
            address: "1.1.1.1".into(),
            port: 443,
            ..Default::default()
        }],
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let refs = referenced_server_ids(&config);
    // direct 哨兵剔除，s1 未被选中/规则引用 → 仅 endpoint 保守纳入（s1 非 endpoint）
    assert!(!refs.contains("__direct__"));
}

#[test]
fn referenced_ids_rule_target_included() {
    use crate::user_config::app_config::UserConfig;
    use crate::user_config::rule::{Rule, RuleAction, RuleType};
    let config = UserConfig {
        servers: vec![
            ServerConfig {
                id: "s1".into(),
                name: "s1".into(),
                protocol: Protocol::Shadowsocks,
                address: "1.1.1.1".into(),
                port: 443,
                ..Default::default()
            },
            ServerConfig {
                id: "s2".into(),
                name: "s2".into(),
                protocol: Protocol::Shadowsocks,
                address: "2.2.2.2".into(),
                port: 443,
                ..Default::default()
            },
        ],
        selected_server_id: Some("s1".into()),
        custom_rules: vec![Rule {
            id: "r1".into(),
            type_field: RuleType::Domain,
            values: vec!["example.com".into()],
            conditions: None,
            combine_mode: None,
            effects: None,
            action: RuleAction::Proxy,
            enabled: true,
            bypass_fakeip: None,
            target_server_id: Some("s2".into()),
            remarks: None,
            tls_spoof: None,
            tls_spoof_method: None,
            network_profile_id: None,
        }],
        ..Default::default()
    };
    let refs = referenced_server_ids(&config);
    assert!(refs.contains("s1")); // 选中
    assert!(refs.contains("s2")); // 规则目标
}

#[test]
fn active_roots_follow_traffic_rules_sot_and_detour_to_physical_root() {
    use crate::user_config::proxy_mode::ProxyMode;
    use crate::user_config::rule::{
        AppRule, Rule, RuleAction, RuleEffects, RuleRouteEffect, RuleType,
    };

    let rule = |id: &str, target: &str| Rule {
        id: id.into(),
        type_field: RuleType::Domain,
        values: vec!["example.com".into()],
        action: RuleAction::Proxy,
        enabled: true,
        target_server_id: Some(target.into()),
        ..Default::default()
    };
    let mut child = ss("child", "2.2.2.2");
    child.detour = Some("root".into());
    let config = UserConfig {
        servers: vec![ss("selected", "1.1.1.1"), child, ss("root", "3.3.3.3")],
        selected_server_id: Some("selected".into()),
        proxy_mode: ProxyMode::Smart,
        // legacy 镜像故意指向 selected；一等 trafficRules 必须压过它。
        custom_rules: vec![rule("legacy", "selected")],
        traffic_rules: Some(vec![Rule {
            effects: Some(RuleEffects {
                route: Some(RuleRouteEffect {
                    enabled: true,
                    action: RuleAction::Proxy,
                    target_server_id: Some("child".into()),
                    destination_resolution: None,
                    resolution_only: false,
                }),
                dns: None,
            }),
            ..rule("traffic", "legacy-ignored")
        }]),
        ..Default::default()
    };

    assert_eq!(
        active_physical_root_ids(&config),
        BTreeSet::from(["root".to_string(), "selected".to_string()])
    );

    let app_targeted = UserConfig {
        servers: config.servers.clone(),
        selected_server_id: Some("selected".into()),
        proxy_mode: ProxyMode::Smart,
        app_routing_enabled: Some(true),
        app_rules: vec![AppRule {
            app_id: "browser".into(),
            action: RuleAction::Proxy,
            enabled: true,
            target_server_id: Some("child".into()),
        }],
        ..Default::default()
    };
    assert_eq!(
        active_physical_root_ids(&app_targeted),
        BTreeSet::from(["root".to_string(), "selected".to_string()]),
        "启用的 app rule 目标也必须沿 detour 收敛到物理根"
    );

    let app_disabled = UserConfig {
        app_routing_enabled: Some(false),
        ..app_targeted
    };
    assert_eq!(
        active_physical_root_ids(&app_disabled),
        BTreeSet::from(["selected".to_string()]),
        "关闭 app routing 后闲置目标不得继续阻断或触发重规划"
    );
}

#[test]
fn idle_plain_node_is_not_an_active_root_but_fallback_is_conservative() {
    let selected = UserConfig {
        servers: three_plain_nodes(),
        selected_server_id: Some("s1".into()),
        ..Default::default()
    };
    assert_eq!(
        active_physical_root_ids(&selected),
        BTreeSet::from(["s1".to_string()])
    );

    let fallback = UserConfig {
        selected_server_id: None,
        ..selected
    };
    assert_eq!(
        active_physical_root_ids(&fallback),
        BTreeSet::from(["s1".to_string(), "s2".to_string(), "s3".to_string()])
    );
}

// === selector default 兜底（proxy-selector 的 default 落到「非选中节点」）===

fn ss(id: &str, addr: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Shadowsocks,
        address: addr.into(),
        port: 8388,
        ..Default::default()
    }
}

fn three_plain_nodes() -> Vec<ServerConfig> {
    vec![
        ss("s1", "1.1.1.1"),
        ss("s2", "2.2.2.2"),
        ss("s3", "3.3.3.3"),
    ]
}

/// 【常态不得被拖成恒重启】正常选中了一个必定被发射的普通代理节点 ⇒ 兜底不可能触发 ⇒
/// 引用集只含选中节点，未引用节点仍可 defer。
/// 本用例红 = 修复过度保守，把「正常选了节点」也拖成了「任何节点编辑都重启」。
#[test]
fn referenced_ids_normal_selection_stays_minimal() {
    use crate::user_config::app_config::UserConfig;
    let config = UserConfig {
        servers: three_plain_nodes(),
        selected_server_id: Some("s2".into()),
        ..Default::default()
    };
    let refs = referenced_server_ids(&config);
    assert!(!selector_default_may_fall_back(&config));
    assert_eq!(refs, ["s2".to_string()].into_iter().collect());
}

/// 【缺陷复现①：未选节点】`selectedServerId=None` ⇒ `build_outbounds`（outbounds.rs:262-271）
/// 的 `selected_tag` 是字面量 `"proxy"`，匹配不到任何节点 tag → default 落 `node_tags.first()`。
/// 那个节点承载**全部**代理流量，却不在任何一条播种里。
/// 本用例红 = 兜底节点又漏出引用集 → 编辑它会被判「未引用」走 defer 腿静默不重启。
#[test]
fn referenced_ids_without_selection_includes_all_nodes() {
    use crate::user_config::app_config::UserConfig;
    let config = UserConfig {
        servers: three_plain_nodes(),
        selected_server_id: None,
        ..Default::default()
    };
    assert!(selector_default_may_fall_back(&config));
    let refs = referenced_server_ids(&config);
    // 「哪个节点会被首先发射」取决于生成期跳过了谁（运行期能力），静态算不出 ⇒ 全部纳入。
    for id in ["s1", "s2", "s3"] {
        assert!(refs.contains(id), "{id} 未纳入引用集");
    }
}

/// 【缺陷复现②：悬空选中】选中 id 不在 servers 里（节点被删/订阅换了 id）⇒ id→tag 解析不到
/// → 同样落 `node_tags.first()` 兜底。
#[test]
fn referenced_ids_dangling_selection_includes_all_nodes() {
    use crate::user_config::app_config::UserConfig;
    let config = UserConfig {
        servers: three_plain_nodes(),
        selected_server_id: Some("ghost".into()),
        ..Default::default()
    };
    assert!(selector_default_may_fall_back(&config));
    let refs = referenced_server_ids(&config);
    for id in ["s1", "s2", "s3"] {
        assert!(refs.contains(id), "{id} 未纳入引用集");
    }
}

/// 选中 naive 时缺 libcronet 会被 generate 的 selected-server 前置门终止，库可用时则必定发射；
/// 两条腿都不允许 selector 静默落到其它节点。若把它重新当成 fallback，会把一次 H3 选择扩成
/// “全部订阅节点都可能承流”，进而让 TUN 逐目的路由规划无谓扫描全订阅。
#[test]
fn referenced_ids_selected_naive_stays_minimal() {
    use crate::user_config::app_config::UserConfig;
    let mut servers = three_plain_nodes();
    servers[1].protocol = Protocol::Naive;
    let config = UserConfig {
        servers,
        selected_server_id: Some("s2".into()),
        ..Default::default()
    };
    assert!(!selector_default_may_fall_back(&config));
    let refs = referenced_server_ids(&config);
    assert_eq!(refs, BTreeSet::from(["s2".to_string()]));
}

#[test]
fn selector_fallback_tracks_tailscale_control_url_gate() {
    use crate::user_config::app_config::UserConfig;

    let mut tailscale = ts_server("ts", None, &[]);
    tailscale.tailscale_settings.as_mut().unwrap().control_url = Some("https://100.64.0.1".into());
    let mut config = UserConfig {
        servers: vec![tailscale, ss("fallback", "2.2.2.2")],
        selected_server_id: Some("ts".into()),
        ..Default::default()
    };
    assert!(
        selector_default_may_fall_back(&config),
        "非法 control_url 会让选中 Tailscale 在发射期被剔除"
    );
    assert!(referenced_server_ids(&config).contains("fallback"));

    config.servers[0]
        .tailscale_settings
        .as_mut()
        .unwrap()
        .control_url = Some("https://control.example.com".into());
    assert!(!selector_default_may_fall_back(&config));
    assert_eq!(
        referenced_server_ids(&config),
        BTreeSet::from(["ts".to_string()])
    );
}

/// 【直连哨兵不触发全纳入】`__direct__` ⇒ default 恒 = `direct` 出站，没有节点承载
/// （outbounds.rs:262-263 的 `is_direct` 腿）⇒ 引用集不得被撑成全体。
#[test]
fn referenced_ids_direct_sentinel_no_blanket_inclusion() {
    use crate::user_config::app_config::UserConfig;
    let config = UserConfig {
        servers: three_plain_nodes(),
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    assert!(!selector_default_may_fall_back(&config));
    assert!(referenced_server_ids(&config).is_empty());
}

/// 【WG 复用真判据】选中 WG 节点时，「会不会被发射」直接问 `build_wireguard_endpoint`：
/// 配置完整 ⇒ 必定发射（不触发兜底）；缺 privateKey ⇒ Err ⇒ 不发射 ⇒ 兜底可能触发。
/// 本用例红 = 判据与真正的发射腿漂移了。
#[test]
fn selector_fallback_tracks_wireguard_buildability() {
    use crate::user_config::app_config::UserConfig;
    let mut wg = wg_server("wg1", &["10.0.0.0/24"], Some(true));
    let s = wg.wireguard_settings.as_mut().unwrap();
    s.private_key = Some("k".into());
    s.peer_public_key = Some("p".into());
    s.local_address = vec!["10.0.0.2/32".into()];
    let mut config = UserConfig {
        servers: vec![wg.clone(), ss("s2", "2.2.2.2")],
        selected_server_id: Some("wg1".into()),
        ..Default::default()
    };
    assert!(
        !selector_default_may_fall_back(&config),
        "配置完整的 WG 必定发射"
    );
    assert!(!referenced_server_ids(&config).contains("s2"));

    config.servers[0]
        .wireguard_settings
        .as_mut()
        .unwrap()
        .private_key = None;
    assert!(
        selector_default_may_fall_back(&config),
        "缺 privateKey 的 WG 构建失败 → 不发射 → 兜底可能触发"
    );
    assert!(referenced_server_ids(&config).contains("s2"));
}

#[test]
fn custom_endpoint_carries_traffic_detects_keys() {
    use serde_json::json;
    // system 键命中
    assert!(custom_endpoint_carries_traffic(&json!({"system": true})));
    // allowed_ips 嵌套命中
    assert!(custom_endpoint_carries_traffic(&json!({
        "peers": [{"allowed_ips": ["0.0.0.0/0"]}]
    })));
    // 无语义键
    assert!(!custom_endpoint_carries_traffic(
        &json!({"tag": "x", "type": "wireguard"})
    ));
    // 数组递归
    assert!(custom_endpoint_carries_traffic(
        &json!([{"exit_node": true}])
    ));
}

/// OpenVPN 全隧道必须被判为承流 —— 语料取**真实可用**的 `openvpn-client` 端点
/// （对随包核 1.14.0-beta.12 跑 `sing-box check` rc=0 的形状：`tls` 必填，缺了报
/// `missing 'tls' options`），不是手捏一个只有目标键的空壳。
///
/// 变异靶：把 `redirect_gateway` 从 `CARRY_TRAFFIC_KEYS` 里删掉 → 第一条 assert 转红。
/// 这条**不能**只写「含 routes 的那份命中」——OpenVPN 表达全隧道的常见写法就是只给
/// `redirect_gateway: true` 而不写 `routes`，那正是原表漏掉的那一半。
#[test]
fn openvpn_full_tunnel_counts_as_carrying_traffic() {
    use serde_json::json;
    let ovpn = |extra: serde_json::Value| {
        let mut base = json!({
            "type": "openvpn-client",
            "server": "1.2.3.4",
            "server_port": 1194,
            "username": "u",
            "password": "p",
            "tls": { "certificate": ["-----BEGIN CERTIFICATE-----"] }
        });
        let map = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            map.insert(k.clone(), v.clone());
        }
        base
    };
    // 只给 redirect_gateway（不写 routes）—— 补键之前这条是 false
    assert!(custom_endpoint_carries_traffic(&ovpn(
        json!({"redirect_gateway": true})
    )));
    assert!(custom_endpoint_carries_traffic(&ovpn(
        json!({"redirect_private": true})
    )));
    assert!(custom_endpoint_carries_traffic(&ovpn(
        json!({"route_no_pull": true})
    )));
    // 不过度纳入：纯拨号型 openvpn-client（无任何路由语义键）仍判 false，
    // 否则「过度纳入只多一次重启」会退化成「每个 OpenVPN 节点必重启」。
    assert!(!custom_endpoint_carries_traffic(&ovpn(json!({}))));
    // openconnect 的路由语义键只有 system —— 原表已覆盖，这条钉住别在补键时把它漏掉。
    assert!(custom_endpoint_carries_traffic(&json!({
        "type": "openconnect", "server": "vpn.example.com:443",
        "username": "u", "password": "p", "flavor": "anyconnect", "system": true
    })));
    assert!(!custom_endpoint_carries_traffic(&json!({
        "type": "openconnect", "server": "vpn.example.com:443",
        "username": "u", "password": "p", "flavor": "anyconnect"
    })));
}

/// WG `reverseMesh:true` 的三态：普通 WG 放行 / WARP（带凭据）否决 / WARP（仅域名）否决。
///
/// 一个测试里放三条是**故意**的：把「否决」和「不过度否决」钉在同一处，
/// 免得后人只看到 WARP 那两条 assert 就把整条 WG 腿改成恒 false —— 那会让正常 WG 的
/// System 接入模式（子网路由 / 反向可达）整体失效，是个只有真机才暴露的静默回归。
#[test]
fn wg_reverse_mesh_system_vetoed_only_for_warp() {
    fn wg_reverse_mesh(address: &str, warp_device: bool) -> ServerConfig {
        ServerConfig {
            id: "w".into(),
            name: "w".into(),
            protocol: Protocol::Wireguard,
            address: address.into(),
            port: 2408,
            wireguard_settings: Some(Box::new(WireGuardSettings {
                reverse_mesh: Some(true),
                warp_device: warp_device.then(|| {
                    crate::user_config::protocol_settings::WarpDevice {
                        device_id: "d".into(),
                        token: "t".into(),
                    }
                }),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    // 反向对照（**先写**）：普通 WG 的 System 接入模式必须照旧生效，否决不得收得过宽。
    assert!(
        mesh_uses_system_interface(&wg_reverse_mesh("vpn.example.com", false)),
        "非 WARP 的 WG reverseMesh:true 必须仍返 true —— 收宽了就是把 System 接入模式整体废掉"
    );

    // 新注册的 WARP：带自删凭据，address 是注册响应给的裸 IP。
    assert!(
        !mesh_uses_system_interface(&wg_reverse_mesh("162.159.192.1", true)),
        "WARP（warpDevice 标记）reverseMesh:true 必须被否决 —— 抢 utun ⇒ resource busy FATAL"
    );

    // 旧 / 导入 / 上游 迁移来的 WARP：无 warpDevice，只能靠端点域名兜底。
    // 这三条腿都不经渲染端，前端的否决在此无效 —— 本用例守的就是那道口子。
    assert!(
        !mesh_uses_system_interface(&wg_reverse_mesh("engage.cloudflareclient.com", false)),
        "无 warpDevice 标记的旧 WARP 必须按域名兜底否决（导入/手改/迁移绕过前端）"
    );
}

/// 线格式必须是 camelCase —— 前端 `contracts/endpoint-force-route-report.ts` 按这几个键读。
///
/// 漏掉 `rename_all` 或改错一个键名，**两侧的 tsc 与单测都不会红**：TS 那边读到的是
/// `undefined`，而 `undefined` 在角标判据里是假值 ⇒ 角标恒不亮，表现与「本来就没冲突」一模一样
/// （`tun_exclusion_preview` 的头注记的是同一个坑）。故键名必须逐条钉死。
#[test]
fn server_force_route_wire_shape_is_camel_case() {
    let entry = ServerForceRoute {
        server_id: "s".into(),
        leg: ForceRouteLeg::ExternalRuleSet,
        has_observation: true,
        emitted: vec![],
        external_rule_set_cidrs: vec!["32.0.0.28/32".into()],
        absorbed: vec![],
        coverage: ForceRouteCoverage::Covered,
    };
    let json = serde_json::to_value(&entry).expect("序列化");
    let obj = json.as_object().expect("应是对象");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "absorbed",
            "coverage",
            "emitted",
            "externalRuleSetCidrs",
            "hasObservation",
            "leg",
            "serverId",
        ],
        "线格式键名漂了 —— 前端按这几个键读，漏一个只会让对应角标恒不亮而不会红"
    );
    assert_eq!(
        obj["externalRuleSetCidrs"],
        serde_json::json!(["32.0.0.28/32"]),
        "新字段的值没落到 camelCase 键上"
    );
}

/// [`settled_force_route_cidrs`] 的取材面：两条**产段**的腿都在内，`PreferredBy` 不在。
///
/// 正向：ExternalRuleSet 腿的段（自建 tailnet 的观测地址只走这条腿）与 Inline 腿结算后的
/// `emitted` 都进并集，且并集去重。
/// 负向：`PreferredBy` 腿的段**不进**（它的段由内核运行期归位，配置期这边只有「它声明了什么」，
/// 没有「内核接管了什么」—— 理由写在 `settled_force_route_cidrs` 的文档里）。
#[test]
fn settled_force_route_cidrs_covers_both_producing_legs_and_excludes_preferred_by() {
    let ts = ts_server("ts-file", None, &[]);
    let mut observed = ObservedTailnetAddresses::new();
    observed.insert("ts-file".into(), vec!["32.0.0.28".into()]);
    let wg_a = wg_server("wg-a", &["10.9.0.0/24"], Some(true));
    let wg_dup = wg_server("wg-dup", &["10.9.0.0/24"], Some(true));
    let wg_lan = wg_server("wg-lan", &["192.168.77.0/24"], Some(false));

    let report = settle_force_route_claims(
        &[
            (&ts, ForceRouteLeg::ExternalRuleSet),
            (&wg_a, ForceRouteLeg::Inline),
            (&wg_dup, ForceRouteLeg::Inline),
            (&wg_lan, ForceRouteLeg::PreferredBy),
        ],
        &observed,
    );
    let union = settled_force_route_cidrs(&report);

    // 前提：夹具真的建立起了三种腿各自的形态，否则下面三条断言各自都可能空转。
    assert_eq!(
        report.servers[0].external_rule_set_cidrs,
        endpoint_forced_route_cidrs(&ts, &observed),
        "前提没建立：ExternalRuleSet 腿没记下它会落盘的那份段"
    );
    assert_eq!(
        report.servers[1].emitted,
        vec!["10.9.0.0/24".to_string()],
        "前提没建立：第一个 Inline 节点没发出它的段"
    );
    assert!(
        report.servers[2].emitted.is_empty() && report.servers[2].absorbed.len() == 1,
        "前提没建立：第二个 Inline 节点的段没有被首声明者吸收，去重那一格失去讨论对象"
    );
    assert!(
        report.servers[3].external_rule_set_cidrs.is_empty()
            && report.servers[3].emitted.is_empty(),
        "前提没建立：PreferredBy 腿不该有任何段形态"
    );

    // 正向：观测段（只走 rule-set 腿）+ Inline 的 emitted 都在。
    assert!(
        union.contains(&"32.0.0.28/32".to_string()),
        "自建 tailnet 的观测段没进并集 —— 它只走 ExternalRuleSet 腿，`emitted` 永远看不见它。实得 {union:?}"
    );
    assert!(
        union.contains(&"10.9.0.0/24".to_string()),
        "Inline 腿结算后发出的段没进并集。实得 {union:?}"
    );
    assert_eq!(
        union.iter().filter(|c| *c == "10.9.0.0/24").count(),
        1,
        "并集没去重：两个 Inline 节点声索同一段时它出现了多次。实得 {union:?}"
    );

    // 负向：PreferredBy 腿的段不在（登记在案的边界，不是遗漏）。
    assert!(
        !union.contains(&"192.168.77.0/24".to_string()),
        "PreferredBy 腿的段进了并集 —— 那是「它声明了什么」而不是「内核接管了什么」。实得 {union:?}"
    );
}

/// 【MASQUE 复用真判据】path 非法的 MASQUE 会在发射期被剔 ⇒ 选中它时 selector 可能落兜底；
/// 合法（含缺设置）⇒ 必定发射。本用例红 = 判据与发射腿漂移了。
#[test]
fn selector_fallback_tracks_masque_buildability() {
    use crate::user_config::app_config::UserConfig;
    use crate::user_config::protocol_settings::MasqueClientSettings;
    let mq = ServerConfig {
        id: "mq".into(),
        name: "MQ".into(),
        protocol: Protocol::MasqueClient,
        address: "mq.example.com".into(),
        port: 443,
        ..Default::default()
    };
    let mut config = UserConfig {
        servers: vec![mq, ss("s2", "2.2.2.2")],
        selected_server_id: Some("mq".into()),
        ..Default::default()
    };
    assert!(
        !selector_default_may_fall_back(&config),
        "缺设置的 MASQUE 也是完整节点，必定发射"
    );
    config.servers[0].masque_client_settings = Some(Box::new(MasqueClientSettings {
        path: Some("no-slash".into()),
        ..Default::default()
    }));
    assert!(
        selector_default_may_fall_back(&config),
        "path 非法的 MASQUE 会被剔除 → 不发射 → 兜底可能触发"
    );
    assert!(referenced_server_ids(&config).contains("s2"));
}

/// MASQUE 凭 `meshRoutes` 获得组网资格，force-route 段只认用户声明的那份（去 catch-all）。
#[test]
fn masque_forced_route_comes_from_mesh_routes() {
    let mq = ServerConfig {
        id: "mq".into(),
        protocol: Protocol::MasqueClient,
        mesh_routes: vec!["10.77.0.0/24".into(), "0.0.0.0/0".into()],
        ..Default::default()
    };
    assert_eq!(
        endpoint_forced_route_cidrs(&mq, &ObservedTailnetAddresses::new()),
        vec!["10.77.0.0/24".to_string()]
    );
}

/// 【Tailcat 复用真判据】key / DERP 非法的 Tailcat 会在发射期被剔 ⇒ 选中它时 selector 可能落兜底；
/// 合法 ⇒ 必定发射。本用例红 = 判据与发射腿漂移了（误判「必定发射」= 错跳重启）。
#[test]
fn selector_fallback_tracks_tailcat_emit_check() {
    use crate::user_config::app_config::UserConfig;
    use crate::user_config::protocol_settings::TailcatSettings;
    let tc = ServerConfig {
        id: "tc".into(),
        name: "TC".into(),
        protocol: Protocol::Tailcat,
        tailcat_settings: Some(Box::new(TailcatSettings {
            server_public_key: Some("lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=".into()),
            server_disco_key: Some("qQ+kiWwZ8BrTYDZpj+6bnx2JxWxx0SAh1krqPGndCmQ=".into()),
            derp_region: Some(1),
            ..Default::default()
        })),
        ..Default::default()
    };
    let mut config = UserConfig {
        servers: vec![tc, ss("s2", "2.2.2.2")],
        selected_server_id: Some("tc".into()),
        ..Default::default()
    };
    assert!(
        !selector_default_may_fall_back(&config),
        "合法 Tailcat 必定发射"
    );
    config.servers[0]
        .tailcat_settings
        .as_mut()
        .unwrap()
        .derp_servers = vec![serde_json::json!("d.example")];
    assert!(
        selector_default_may_fall_back(&config),
        "region 与 servers 同时设的 Tailcat 会被剔除 → 兜底可能触发"
    );
}

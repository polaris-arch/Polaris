use super::*;
use polaris_config_engine::user_config::proxy_mode::ProxyModeType;

fn server(id: &str, address: &str) -> ServerConfig {
    ServerConfig {
        id: id.to_owned(),
        name: id.to_owned(),
        protocol: Protocol::Vless,
        address: address.to_owned(),
        port: 443,
        ..Default::default()
    }
}

#[test]
fn host_normalization_covers_urls_ports_and_ipv6() {
    assert_eq!(
        normalize_host("vpn.example.com:443").as_deref(),
        Some("vpn.example.com")
    );
    assert_eq!(
        normalize_host("https://u@vpn.example.com:8443/path").as_deref(),
        Some("vpn.example.com")
    );
    assert_eq!(
        normalize_host("[2001:db8::1]:443").as_deref(),
        Some("2001:db8::1")
    );
    assert_eq!(
        normalize_host("2001:db8::2").as_deref(),
        Some("2001:db8::2")
    );
}

#[test]
fn candidates_collapse_detour_children_to_the_physical_root() {
    let mut child = server("child", "child.example.com");
    child.detour = Some("root".into());
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![child, server("root", "root.example.com")],
        ..Default::default()
    };
    let candidates = hot_switch_runtime_binding_candidates(Platform::Linux, &config);
    assert_eq!(
        candidates,
        vec![Candidate {
            server_id: "root".into(),
            host: "root.example.com".into(),
        }]
    );
}

#[test]
fn explicit_binding_and_dynamic_tailscale_targets_are_not_inferred() {
    let mut explicit = server("explicit", "1.1.1.1");
    explicit.bind_interface = Some("en0".into());
    let mut tailscale = server("ts", "");
    tailscale.protocol = Protocol::Tailscale;
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![explicit, tailscale],
        ..Default::default()
    };
    assert!(!needs_runtime_binding_plan(&config));
}

/// Tailcat 没有服务器主机（DERP 主机由内核按地图/servers 自拨）：顶层 address 即便残留了值也不得
/// 被当成出口主机去绑网卡。
#[test]
fn addressless_tailcat_has_no_route_host() {
    let mut tc = server("tc", "left-over.example.com");
    tc.protocol = Protocol::Tailcat;
    assert_eq!(server_route_host(&tc), None);
}

#[test]
fn active_roots_stay_minimal_while_hot_switch_plan_covers_all_nodes() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![server("node-a", "1.1.1.1"), server("node-b", "2.2.2.2")],
        selected_server_id: Some("node-a".into()),
        ..Default::default()
    };
    assert_eq!(
        automatic_runtime_binding_root_ids(&config),
        BTreeSet::from(["node-a".to_string()]),
        "配置变更判定只看当前承流根，闲置 B 不得让订阅更新误重启"
    );
    assert_eq!(
        hot_switch_runtime_binding_candidates(Platform::Linux, &config)
            .into_iter()
            .map(|candidate| candidate.server_id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["node-a".to_string(), "node-b".to_string()]),
        "起核前必须覆盖 selector 全集，A→B 才是真热切"
    );
}

#[test]
fn selected_naive_keeps_active_roots_minimal_but_does_not_shrink_hot_switch_coverage() {
    let mut selected = server("h3", "h3.example.com");
    selected.protocol = Protocol::Naive;
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![
            server("idle-a", "1.1.1.1"),
            selected,
            server("idle-b", "2.2.2.2"),
        ],
        selected_server_id: Some("h3".into()),
        ..Default::default()
    };
    assert_eq!(
        automatic_runtime_binding_root_ids(&config),
        BTreeSet::from(["h3".to_string()]),
        "当前承流判定仍只含 H3"
    );
    assert_eq!(
        hot_switch_runtime_binding_candidates(Platform::Linux, &config)
            .into_iter()
            .map(|candidate| candidate.server_id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["h3".to_string(), "idle-a".to_string(), "idle-b".to_string(),]),
        "H3 不能再把热切覆盖误收窄为当前节点"
    );
}

#[test]
fn duplicate_hosts_share_one_route_query_group_without_losing_roots() {
    let grouped = group_candidates_by_host(vec![
        Candidate {
            server_id: "node-a".into(),
            host: "shared.example.com".into(),
        },
        Candidate {
            server_id: "node-b".into(),
            host: "shared.example.com".into(),
        },
        Candidate {
            server_id: "node-c".into(),
            host: "other.example.com".into(),
        },
    ]);
    assert_eq!(grouped.len(), 2);
    assert_eq!(
        grouped.get("shared.example.com"),
        Some(&vec!["node-a".to_string(), "node-b".to_string()])
    );
    assert_eq!(
        grouped.get("other.example.com"),
        Some(&vec!["node-c".to_string()])
    );
}

#[test]
fn traffic_rule_target_adds_its_detour_physical_root() {
    use polaris_config_engine::user_config::proxy_mode::ProxyMode;
    use polaris_config_engine::user_config::rule::{
        Rule, RuleAction, RuleEffects, RuleRouteEffect, RuleType,
    };

    let mut child = server("child", "child.example.com");
    child.detour = Some("root".into());
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        proxy_mode: ProxyMode::Smart,
        servers: vec![
            server("node-a", "1.1.1.1"),
            child,
            server("root", "3.3.3.3"),
        ],
        selected_server_id: Some("node-a".into()),
        traffic_rules: Some(vec![Rule {
            id: "rule-b".into(),
            type_field: RuleType::Domain,
            values: vec!["example.com".into()],
            action: RuleAction::Proxy,
            enabled: true,
            effects: Some(RuleEffects {
                route: Some(RuleRouteEffect {
                    enabled: true,
                    action: RuleAction::Proxy,
                    target_server_id: Some("child".into()),
                    target_auto_select: None,
                    destination_resolution: None,
                    resolution_only: false,
                }),
                dns: None,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert_eq!(
        automatic_runtime_binding_root_ids(&config),
        BTreeSet::from(["node-a".to_string(), "root".to_string()])
    );
}

#[test]
fn custom_outbound_uses_its_passthrough_server_not_the_placeholder_address() {
    use polaris_config_engine::user_config::protocol_settings::CustomSettings;

    let mut custom = server("custom", "unused.example");
    custom.protocol = Protocol::Custom;
    custom.custom_settings = Some(CustomSettings {
        outbound: serde_json::json!({
            "type": "socks",
            "server": "custom.example.com",
            "server_port": 1080
        }),
        is_endpoint: None,
        secret_keys: None,
    });
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![custom],
        ..Default::default()
    };
    assert_eq!(
        hot_switch_runtime_binding_candidates(Platform::Linux, &config),
        vec![Candidate {
            server_id: "custom".into(),
            host: "custom.example.com".into(),
        }]
    );
}

/// 🔴 **K6b 起停循环门（上半）**：没有路由查询腿的平台上，逐目的绑定规划必须**一个候选都不产**。
///
/// 缺陷原文（真机实测，2026-09-04 模拟器）：Android 上 `Platform::current() == Other`，
/// `SystemRouteOps::exit_interface_for` 那条腿是 `Ok(None)`，于是这份规划恒产出
/// 「候选 1 / 绑定 0 / 未决 1」。而**未决非空**正是 `inferred_binding_replan_needed` 里最激进的
/// 那条腿（退回整份网卡指纹比对），起核就绪后那次比对必然多出我们自己刚建的 `tun0`
/// ⇒ 判据恒真 ⇒ 每次起核收尾排一次去抖重启 ⇒ 300 秒 69 次 spawn。
///
/// 下半（判据侧的复合断言与它的正对照）在
/// `runtime/proxy/tests/route_replan.rs::runtime_binding_replan_is_inert_without_a_route_leg`。
/// 完整根因与收据：`~/docs/polaris/design/polaris-android-core-respawn-2026-09-04.md`。
///
/// **两个方向都断言**：只写「Other 上为空」会被「规划整个坏掉、哪个平台都空」骗过；
/// 故同一份配置在 Linux 上必须仍然产出那一个候选。
#[test]
fn runtime_binding_planning_is_off_where_the_platform_has_no_route_leg() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![server("k4-probe-node", "203.0.113.7")],
        ..Default::default()
    };

    // 正对照：有路由查询腿的平台上，这份配置**确实**是一个候选（否则下面那条空断言无信息量）。
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(
            runtime_binding_planning_supported(platform),
            "{platform:?} 有 SystemRouteOps::exit_interface_for 的实现腿"
        );
        assert_eq!(
            hot_switch_runtime_binding_candidates(platform, &config)
                .into_iter()
                .map(|candidate| candidate.server_id)
                .collect::<Vec<_>>(),
            vec!["k4-probe-node".to_string()],
            "{platform:?} 上这个物理根必须仍然进入逐目的规划"
        );
    }

    // 修复本身：无路由查询腿的平台一律关掉规划。
    //
    // 🔴 2026-09-04 K10：Android 此前是靠**落进 `Other`** 才被这条闸盖住的（`Platform::current()`
    // 只认 macos/windows/linux）。给 Android 具名之后那条继承关系断了 —— 若只留 `Other` 这一格，
    // K6b 修掉的 300s/69 次起停循环会原样回来。故两个变体各自断言，不靠任何一个盖住另一个。
    //
    // 2026-09-06 K13 同理补 `Platform::Ios`：它在生产侧已有独立臂（`route_binding.rs` 的
    // `Platform::Ios => false`），但那个答案此前没有任何运行期证据 —— 手写的变体子集
    // 加变体时不会自曝，这正是「牙长在隔壁那条腿上」。
    for platform in [Platform::Other, Platform::Android, Platform::Ios] {
        assert!(
            !runtime_binding_planning_supported(platform),
            "{platform:?} 上 `SystemRouteOps::exit_interface_for` 恒 Ok(None) ⇒ 规划恒无结论"
        );
        assert!(
            hot_switch_runtime_binding_candidates(platform, &config).is_empty(),
            "{platform:?}: 无路由查询腿 ⇒ 一个候选都不许产（产了就会喂出恒真的重启判据）"
        );
        assert!(
            runtime_binding_candidates_for_roots(
                platform,
                &config,
                active_physical_root_ids(&config)
            )
            .is_empty(),
            "{platform:?}: 热切保护门消费的 automatic 口径同样必须为空（两条口径共用同一个闸）"
        );
    }
}

#[test]
fn non_tun_modes_never_inject_runtime_bindings() {
    for mode in [ProxyModeType::SystemProxy, ProxyModeType::Manual] {
        let config = UserConfig {
            proxy_mode_type: mode,
            servers: vec![server("node", "1.1.1.1")],
            ..Default::default()
        };
        assert!(!needs_runtime_binding_plan(&config));
    }
}

#[test]
fn default_route_uses_native_auto_detect_and_only_exceptions_are_bound() {
    assert_eq!(
        classify_runtime_binding("en0", Some(" en0 ")),
        RuntimeBindingDecision::Native
    );
    assert_eq!(
        classify_runtime_binding("utun7", Some("en0")),
        RuntimeBindingDecision::Bind("utun7".into())
    );
    assert_eq!(
        classify_runtime_binding("eth0", None),
        RuntimeBindingDecision::Bind("eth0".into()),
        "默认出口不可读时不得猜成另一张接口"
    );
}

#[tokio::test]
async fn explicit_ipv6_literal_is_routable_even_when_dns_ipv6_is_disabled() {
    assert_eq!(
        resolve_route_probe_ip("2606:4700:4700::1111", false).await,
        Some("2606:4700:4700::1111".parse().unwrap())
    );
}

/// 只读真机门：走与生产相同的 DNS/IP → OS 路由 → 接口名链路，不改路由、不起 TUN。
/// 默认测试集不能假定 runner 有公网路由；三平台发布验证须显式执行本条。
#[tokio::test]
#[ignore = "requires a live OS route table"]
async fn live_route_planner_returns_a_real_interface() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![server("live-route-probe", "1.1.1.1")],
        ..Default::default()
    };
    let plan = plan_runtime_bindings(&config).await;
    assert_eq!(plan.candidate_count, 1);
    assert!(plan.unresolved_roots.is_empty());
    if let Some(interface) = plan.bindings.get("live-route-probe") {
        assert!(!interface.trim().is_empty());
        eprintln!("live route planner selected exceptional interface: {interface}");
    } else {
        assert!(plan.native_roots.contains("live-route-probe"));
        eprintln!("live route planner retained native auto_detect_interface");
    }
}

// This guard uses Platform::current; unsupported mobile platforms deliberately
// have no inferred route candidates, so exercise substantive roots on desktops.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn runtime_reuse_allows_same_host_credentials_without_mutating_accepted_facts() {
    let old = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("node-a".into()),
        servers: vec![server("node-a", "1.1.1.1")],
        ..Default::default()
    };
    let plan = RuntimeBindingPlan {
        covered_roots: BTreeSet::from(["node-a".to_string()]),
        bindings: BTreeMap::from([("node-a".to_string(), "accepted-interface".to_string())]),
        probe_ips: BTreeMap::from([("node-a".to_string(), "1.1.1.1".parse().unwrap())]),
        ..Default::default()
    };
    let accepted = plan.clone();
    let mut next = old.clone();
    next.servers[0].uuid = Some("new-credential".into());
    next.servers[0].port = 8443;
    assert!(!automatic_runtime_binding_root_ids(&next).is_empty());
    assert!(runtime_binding_targets_reusable(&old, &next, &plan));
    assert_eq!(plan, accepted, "reuse must retain the accepted route facts");
    next.servers[0].address = "2.2.2.2".into();
    assert!(
        !runtime_binding_targets_reusable(&old, &next, &plan),
        "the same server ID does not attest a changed route target"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn runtime_reuse_rejects_new_or_preexisting_uncovered_active_roots() {
    let old = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("node-a".into()),
        servers: vec![server("node-a", "1.1.1.1"), server("node-b", "2.2.2.2")],
        ..Default::default()
    };
    let mut plan = RuntimeBindingPlan {
        covered_roots: BTreeSet::from(["node-a".to_string()]),
        ..Default::default()
    };
    let mut next = old.clone();
    next.selected_server_id = Some("node-b".into());
    assert!(!runtime_binding_targets_reusable(&old, &next, &plan));
    plan.covered_roots.insert("node-b".into());
    assert!(
        runtime_binding_targets_reusable(&old, &next, &plan),
        "positive control: an unchanged already covered target is reusable"
    );
    let mut inactive_edit = old.clone();
    inactive_edit.servers[1].address = "4.4.4.4".into();
    assert!(
        runtime_binding_targets_reusable(&old, &inactive_edit, &plan),
        "an inactive host edit need not consume an old route fact yet"
    );
    let mut activated = inactive_edit.clone();
    activated.selected_server_id = Some("node-b".into());
    assert!(
        !runtime_binding_targets_reusable(&old, &activated, &plan),
        "activation must compare against trusted startup host, not later HOT runtime content"
    );
    assert!(
        runtime_binding_targets_reusable(&inactive_edit, &activated, &plan),
        "negative baseline control: using edited R would incorrectly appear reusable"
    );
    next.servers.push(server("new-root", "3.3.3.3"));
    next.selected_server_id = Some("new-root".into());
    assert!(!runtime_binding_targets_reusable(&old, &next, &plan));
    plan.covered_roots.insert("new-root".into());
    assert!(
        !runtime_binding_targets_reusable(&old, &next, &plan),
        "coverage alone cannot invent a previous accepted route target"
    );
}

#[test]
fn protected_plan_extends_only_new_active_targets_and_keeps_accepted_owner_immutable() {
    let old = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("a".into()),
        servers: vec![server("a", "old.example")],
        ..Default::default()
    };
    let accepted = RuntimeBindingPlan {
        native_roots: BTreeSet::from(["a".into()]),
        covered_roots: BTreeSet::from(["a".into()]),
        candidate_count: 1,
        ..Default::default()
    };
    let mut new = old.clone();
    new.servers[0].address = "new.example".into();
    let mut resolved = Vec::new();
    let mut verified = Vec::new();
    let (plan, ttl, targets) = extend_protected_binding_plan(
        &old,
        &new,
        &accepted,
        "eth0",
        |host| {
            resolved.push(host.to_owned());
            Ok(("203.0.113.8".parse().unwrap(), Duration::from_secs(23)))
        },
        |ip, port| {
            verified.push((ip, port));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(resolved, ["new.example"]);
    assert_eq!(verified, targets);
    assert_eq!(ttl, Duration::from_secs(23));
    assert_eq!(plan.bindings.get("a").map(String::as_str), Some("eth0"));
    assert!(!plan.native_roots.contains("a"));
    assert!(accepted.bindings.is_empty() && accepted.native_roots.contains("a"));
    let reused = extend_protected_binding_plan(
        &old,
        &old,
        &accepted,
        "eth0",
        |_| panic!("covered host must not resolve again"),
        |_, _| panic!("covered host must not probe again"),
    )
    .unwrap();
    assert_eq!(reused.0, accepted);
}
#[test]
fn protected_plan_resolution_and_route_failures_never_partially_publish() {
    let old = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let new = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("a".into()),
        servers: vec![server("a", "new.example")],
        ..Default::default()
    };
    let accepted = RuntimeBindingPlan::default();
    assert!(extend_protected_binding_plan(
        &old,
        &new,
        &accepted,
        "eth0",
        |_| Err("DNS IO still owned or failed".into()),
        |_, _| panic!("no route query after DNS error")
    )
    .is_err());
    assert!(extend_protected_binding_plan(
        &old,
        &new,
        &accepted,
        "eth0",
        |_| Ok(("203.0.113.8".parse().unwrap(), Duration::from_secs(30))),
        |_, _| Err("policy drift".into())
    )
    .is_err());
    assert_eq!(accepted, RuntimeBindingPlan::default());
    let mut ipv6 = new;
    ipv6.servers[0].address = "2001:db8::8".into();
    assert!(extend_protected_binding_plan(
        &old,
        &ipv6,
        &accepted,
        "eth0",
        |_| panic!("no unprotected fallback lookup"),
        |_, _| Ok(())
    )
    .is_err());
}
fn physical_answer_fixture(host: &str, ip: [u8; 4], ttl: u32) -> (u16, Vec<u8>) {
    let (id, mut packet) = physical_dns_query(host).unwrap();
    packet[2] = 0x81;
    packet[3] = 0x80;
    packet[7] = 1;
    packet.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
    packet.extend_from_slice(&ttl.to_be_bytes());
    packet.extend_from_slice(&[0, 4]);
    packet.extend_from_slice(&ip);
    (id, packet)
}
#[test]
fn physical_dns_wire_validates_original_question_id_truncation_and_compression() {
    let (id, packet) = physical_answer_fixture("node.example", [203, 0, 113, 8], 21);
    assert_eq!(
        physical_dns_answer(&packet, id, "node.example").unwrap(),
        ("203.0.113.8".parse().unwrap(), Duration::from_secs(21))
    );
    assert!(physical_dns_answer(&packet, id.wrapping_add(1), "node.example").is_err());
    assert!(physical_dns_answer(&packet, id, "foreign.example").is_err());
    let mut truncated = packet.clone();
    truncated[2] |= 2;
    assert!(physical_dns_answer(&truncated, id, "node.example").is_err());
    for end in 0..packet.len() {
        assert!(physical_dns_answer(&packet[..end], id, "node.example").is_err());
    }
    let mut cursor = 0;
    assert!(physical_dns_name(&[0xc0, 0], &mut cursor).is_err());
    for (ip, ttl) in [
        ([127, 0, 0, 1], 30),
        ([198, 18, 0, 2], 30),
        ([203, 0, 113, 8], 0),
    ] {
        let (id, answer) = physical_answer_fixture("node.example", ip, ttl);
        assert!(physical_dns_answer(&answer, id, "node.example").is_err());
    }
    assert!(physical_dns_query("singlelabel").is_err());
    assert!(physical_dns_query("bad\n.example").is_err());
}
#[test]
fn physical_dns_alias_requires_exact_chain_and_keeps_shortest_ttl() {
    let (id, mut packet) = physical_dns_query("node.example").unwrap();
    packet[2] = 0x81;
    packet[3] = 0x80;
    packet[7] = 2;
    let alias = b"\x04edge\x07example\0";
    packet.extend_from_slice(&[0xc0, 0x0c, 0, 5, 0, 1]);
    packet.extend_from_slice(&7u32.to_be_bytes());
    packet.extend_from_slice(&(alias.len() as u16).to_be_bytes());
    packet.extend_from_slice(alias);
    packet.extend_from_slice(alias);
    packet.extend_from_slice(&[0, 1, 0, 1]);
    packet.extend_from_slice(&40u32.to_be_bytes());
    packet.extend_from_slice(&[0, 4, 203, 0, 113, 9]);
    assert_eq!(
        physical_dns_answer(&packet, id, "node.example").unwrap().1,
        Duration::from_secs(7)
    );
    let (foreign_id, foreign) = physical_answer_fixture("other.example", [203, 0, 113, 9], 30);
    assert!(physical_dns_answer(&foreign, foreign_id, "node.example").is_err());
}

fn protected_generated_fixture() -> polaris_config_engine::singbox::SingBoxConfig {
    serde_json::from_value(serde_json::json!({
        "log":{"level":"info","timestamp":true},
        "inbounds":[{"type":"tun","tag":"tun-in","interface_name":"polaris-tun0","address":["172.19.0.1/30"],"auto_route":true,"auto_redirect":false}],
        "outbounds":[{"type":"vless","tag":"np-a","server":"old.example","server_port":443,"uuid":"fixture"}],
        "route":{"rules":[],"auto_detect_interface":true,"final":"np-a"}
    })).unwrap()
}
#[test]
fn protected_generated_projection_allows_new_proven_node_and_logical_route_changes() {
    let before = protected_generated_fixture();
    let mut after = before.clone();
    let mut node = after.outbounds[0].clone();
    node.tag = "np-b".into();
    node.server = Some("new.example".into());
    node.extra
        .insert("bind_interface".into(), serde_json::json!("eth0"));
    after.outbounds.push(node);
    after.outbounds[0].uuid = Some("changed-credential".into());
    after.route.as_mut().unwrap().final_outbound = Some("np-b".into());
    after.route.as_mut().unwrap().rules.push(
        serde_json::from_value(
            serde_json::json!({"domain":["example.test"],"action":"route","outbound":"np-b"}),
        )
        .unwrap(),
    );
    assert!(protected_generated_policy_compatible(&before, &after));
}
#[test]
fn protected_generated_projection_rejects_tun_replacement_implicit_marks_and_special_sockets() {
    let before = protected_generated_fixture();
    let mut next = before.clone();
    next.inbounds[0].mtu = Some(1400);
    assert!(!protected_generated_policy_compatible(&before, &next));
    next = before.clone();
    next.inbounds[0].auto_redirect = Some(true);
    assert!(!protected_generated_policy_compatible(&next, &next));
    next = before.clone();
    next.route.as_mut().unwrap().auto_detect_interface = Some(false);
    assert!(!protected_generated_policy_compatible(&before, &next));
    for (key, value) in [
        ("routing_mark", serde_json::json!(123)),
        ("network_namespace", serde_json::json!("foreign")),
        ("inet4_bind_address", serde_json::json!("192.0.2.9")),
        ("network_strategy", serde_json::json!("hybrid")),
        ("protect_path", serde_json::json!("/run/socket")),
    ] {
        next = before.clone();
        next.outbounds[0].extra.insert(key.into(), value);
        assert!(
            !protected_generated_policy_compatible(&next, &next),
            "{key}"
        );
    }
    next = before.clone();
    next.outbounds[0]
        .extra
        .insert("routing_mark".into(), serde_json::json!(0));
    assert!(protected_generated_policy_compatible(&before, &next));
    next = before.clone();
    next.inbounds.push(next.inbounds[0].clone());
    assert!(!protected_generated_policy_compatible(&next, &next));
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn protected_plan_does_not_attest_an_inactive_edit_when_advancing_trusted_config() {
    let old = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("a".into()),
        servers: vec![server("a", "old-a.example"), server("b", "old-b.example")],
        ..Default::default()
    };
    let accepted = RuntimeBindingPlan {
        covered_roots: BTreeSet::from(["a".into(), "b".into()]),
        native_roots: BTreeSet::from(["a".into(), "b".into()]),
        candidate_count: 2,
        ..Default::default()
    };
    let mut next = old.clone();
    next.servers[0].address = "new-a.example".into();
    next.servers[1].address = "unprobed-b.example".into();
    let (plan, _, _) = extend_protected_binding_plan(
        &old,
        &next,
        &accepted,
        "eth0",
        |host| {
            assert_eq!(host, "new-a.example");
            Ok(("203.0.113.9".parse().unwrap(), Duration::from_secs(30)))
        },
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(plan.covered_roots.contains("a") && !plan.covered_roots.contains("b"));
    assert!(
        accepted.covered_roots.contains("b"),
        "old owner facts remain unchanged"
    );
    let mut activated = next.clone();
    activated.selected_server_id = Some("b".into());
    assert!(
        !runtime_binding_targets_reusable(&next, &activated, &plan),
        "new trusted host cannot borrow removed old coverage"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn negative_memory_proofs_fail_before_any_native_or_network_verification() {
    for expired in [true, false] {
        let (owner, proof) = blocked_physical_binding_proof_for_test(7, 11, expired);
        assert!(owner.seal.lock().unwrap().is_none());
        assert!(!proof.publication_current(7, 11));
        assert_eq!(
            proof.verify(7, 11).await.unwrap_err(),
            "protected route proof expired, revoked or changed owner"
        );
        // No real proof may be synthesized by changing the caller's identity.
        assert!(!proof.publication_current(8, 11));
        assert!(!proof.publication_current(7, 12));
        owner.revoke();
        assert!(!proof.publication_current(7, 11));
    }
}

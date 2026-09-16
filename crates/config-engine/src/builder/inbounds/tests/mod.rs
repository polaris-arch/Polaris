use super::*;

fn deps_linux() -> InboundsDeps {
    InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        update_in_port: None,
        subscription_update_in_port: None,
        probe_pool_ports: vec![],
        platform: "linux".into(),
        own_lan_cidrs: vec![],
        // 无运行期观测 ⇒ engaged_mesh 仅含 tailnet 默认常量段，与本字段出现之前同。
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    }
}

#[test]
fn linux_tun_uses_stable_resolved_interface_name() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let inbounds = build_inbounds(&config, None, &deps_linux());
    let tun = inbounds.iter().find(|i| i.tag == "tun-in").expect("有 tun");
    assert_eq!(
        tun.interface_name.as_deref(),
        Some(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME)
    );
}

// warn 收集器：`log` 是裸 fn 指针（闭包捕获不了）⇒ thread_local sink（与 route.rs/custom_rules.rs 同手法）。
thread_local! {
    static WARN_SINK: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}
fn capture_warn(lvl: LogLevel, msg: &str) {
    assert_eq!(
        lvl,
        LogLevel::Warn,
        "「连入来源排除」静默剔除告警必须是 warn 档（会被级别过滤吞掉的 info 等于没打）"
    );
    WARN_SINK.with(|s| s.borrow_mut().push(msg.to_string()));
}
fn take_warns() -> Vec<String> {
    WARN_SINK.with(|s| s.borrow_mut().drain(..).collect())
}

/// 【不变式：静默剔除必告警 —— Linux 忽略腿】
/// 用户在 Linux 填了「连入来源排除」→ 整块跳过，但必须 warn 出「已忽略 N 条」，
/// 且**确实不发射** route_exclude_address（Linux 加法态下它是毒丸）。
/// 变异验证：删掉 linux 分支的 `(deps.log)(...)` → 首个断言转红。
#[test]
fn inbound_exclude_on_linux_warns_and_emits_nothing() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            inbound_exclude_cidrs: Some(vec!["10.0.0.0/24".into(), "192.168.9.0/24".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.log = capture_warn;
    let inbounds = build_inbounds(&config, None, &deps);
    let warns = take_warns();
    assert!(
        warns
            .iter()
            .any(|m| m.contains("已忽略 2 条声明段") && m.contains("Linux")),
        "Linux 忽略腿必须逐条自曝（含条数），实际: {warns:?}"
    );
    let tun = inbounds.iter().find(|i| i.tag == "tun-in").expect("有 tun");
    assert!(
        tun.route_exclude_address.is_none(),
        "Linux 恒不发射 route_exclude_address（非空即触发策略路由表分解 → 连入全断）"
    );
}

/// Linux 上用户**没填**时不得凭空 warn（告警要与用户动作一一对应，否则日志噪音掩盖真问题）。
#[test]
fn inbound_exclude_on_linux_is_silent_when_user_declared_nothing() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.log = capture_warn;
    let _ = build_inbounds(&config, None, &deps);
    assert!(take_warns().is_empty(), "未声明任何段 → 不得告警");
}

#[test]
fn inbound_exclude_warns_invalid_cidr() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            inbound_exclude_cidrs: Some(vec!["not-a-cidr".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    deps.log = capture_warn;
    let _ = build_inbounds(&config, None, &deps);
    let warns = take_warns();
    assert!(
        warns.iter().any(|m| m.contains("非法/过宽网段")),
        "实际: {warns:?}"
    );
}

#[test]
fn inbound_exclude_warns_mesh_overlap() {
    let mut config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("w1".into()),
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            inbound_exclude_cidrs: Some(vec!["10.0.0.0/24".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    config
        .servers
        .push(crate::user_config::server_config::ServerConfig {
            id: "w1".into(),
            name: "WG".into(),
            protocol: crate::user_config::server_config::Protocol::Wireguard,
            address: "1.2.3.4".into(),
            port: 443,
            wireguard_settings: Some(Box::new(
                crate::user_config::server_config::WireGuardSettings {
                    allowed_ips: vec!["10.0.0.0/24".into()],
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    deps.log = capture_warn;
    let _ = build_inbounds(&config, None, &deps);
    let warns = take_warns();
    assert!(
        warns
            .iter()
            .any(|m| m.contains("组网(WG/Tailscale)路由段重叠")),
        "实际: {warns:?}"
    );
}

#[test]
fn inbound_exclude_warns_mac_own_lan_overlap() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            inbound_exclude_cidrs: Some(vec!["10.0.0.0/24".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    deps.own_lan_cidrs = vec!["10.0.0.0/24".into()];
    deps.log = capture_warn;
    let _ = build_inbounds(&config, None, &deps);
    let warns = take_warns();
    assert!(
        warns.iter().any(|m| m.contains("本机物理 LAN")),
        "实际: {warns:?}"
    );
}

#[test]
fn inbound_exclude_warns_custom_rule_overlap() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        proxy_mode: crate::user_config::ProxyMode::Smart,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            inbound_exclude_cidrs: Some(vec!["10.0.0.0/24".into()]),
            ..Default::default()
        }),
        custom_rules: vec![crate::user_config::rule::Rule {
            id: "r1".into(),
            type_field: crate::user_config::rule::RuleType::IpCidr,
            values: vec!["10.0.0.0/24".into()],
            conditions: None,
            combine_mode: None,
            effects: None,
            action: RuleAction::Proxy,
            enabled: true,
            bypass_fakeip: None,
            target_server_id: None,
            remarks: None,
            tls_spoof: None,
            tls_spoof_method: None,
        }],
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    deps.log = capture_warn;
    let _ = build_inbounds(&config, None, &deps);
    let warns = take_warns();
    assert!(
        warns
            .iter()
            .any(|m| m.contains("非直连（走代理/拦截）自定义规则段重叠")),
        "实际: {warns:?}"
    );
}

#[test]
fn system_proxy_single_mixed_inbound() {
    let config = UserConfig::default();
    let inbounds = build_inbounds(&config, None, &deps_linux());
    assert_eq!(inbounds.len(), 1);
    assert_eq!(inbounds[0].type_field, "mixed");
    assert_eq!(inbounds[0].tag, "mixed-in");
    assert_eq!(inbounds[0].listen.as_deref(), Some("127.0.0.1"));
    assert_eq!(inbounds[0].listen_port, Some(7890)); // 默认 mixed port
}

#[test]
fn system_proxy_allow_lan_listens_all() {
    let config = UserConfig {
        allow_lan: Some(true),
        ..Default::default()
    };
    let inbounds = build_inbounds(&config, None, &deps_linux());
    assert_eq!(inbounds[0].listen.as_deref(), Some("::"));
}

#[test]
fn probe_inbounds_when_ports_set() {
    let mut deps = deps_linux();
    deps.probe_direct_port = Some(12345);
    deps.probe_proxy_port = Some(12346);
    let inbounds = build_inbounds(&UserConfig::default(), None, &deps);
    assert_eq!(inbounds.len(), 3); // mixed + probe-direct + probe-proxy
    assert_eq!(inbounds[1].tag, "probe-direct-in");
    assert_eq!(inbounds[2].tag, "probe-proxy-in");
}

#[test]
fn proxy_health_inbound_does_not_require_direct_probe() {
    let mut deps = deps_linux();
    deps.probe_direct_port = None;
    deps.probe_proxy_port = Some(12346);
    let inbounds = build_inbounds(&UserConfig::default(), None, &deps);

    assert_eq!(inbounds.len(), 2); // mixed + dedicated proxy health probe
    assert!(inbounds
        .iter()
        .all(|inbound| inbound.tag != "probe-direct-in"));
    let probe = inbounds
        .iter()
        .find(|inbound| inbound.tag == "probe-proxy-in")
        .expect("proxy probe inbound must be generated independently");
    assert_eq!(probe.type_field, "http");
    assert_eq!(probe.listen.as_deref(), Some("127.0.0.1"));
    assert_eq!(probe.listen_port, Some(12346));
}

#[test]
fn update_inbound_when_port_set() {
    let mut deps = deps_linux();
    deps.update_in_port = Some(12347);
    let inbounds = build_inbounds(&UserConfig::default(), None, &deps);
    assert_eq!(inbounds.len(), 2); // mixed + update-in
    assert_eq!(inbounds[1].type_field, "socks");
    assert_eq!(inbounds[1].tag, "update-in");
}

#[test]
fn subscription_update_inbound_is_independent_loopback_socks() {
    let mut deps = deps_linux();
    deps.update_in_port = Some(12347);
    deps.subscription_update_in_port = Some(12348);
    let inbounds = build_inbounds(&UserConfig::default(), None, &deps);
    let subscription = inbounds
        .iter()
        .find(|inbound| inbound.tag == "subscription-update-in")
        .expect("subscription-only inbound");
    assert_eq!(subscription.type_field, "socks");
    assert_eq!(subscription.listen.as_deref(), Some("127.0.0.1"));
    assert_eq!(subscription.listen_port, Some(12348));
    assert_eq!(
        inbounds
            .iter()
            .find(|inbound| inbound.tag == "update-in")
            .and_then(|inbound| inbound.listen_port),
        Some(12347),
        "共享 update-in 必须保留原口供图标等消费"
    );
}

#[test]
fn tun_linux_no_exclude_addr() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let inbounds = build_inbounds(&config, None, &deps_linux());
    assert_eq!(inbounds.len(), 2); // mixed + tun
    let tun = &inbounds[1];
    assert_eq!(tun.type_field, "tun");
    assert_eq!(tun.tag, "tun-in");
    assert!(
        tun.route_exclude_address.is_none()
            || tun.route_exclude_address.as_ref().unwrap().is_empty()
    );
    assert_eq!(tun.mtu, Some(DEFAULT_TUN_MTU)); // 缺席 → 与栈、平台无关的单一默认
}

#[test]
fn tun_mac_has_loopback_exclude() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    let inbounds = build_inbounds(&config, None, &deps);
    let tun = &inbounds[1];
    assert_eq!(tun.mtu, Some(DEFAULT_TUN_MTU)); // mac 与 linux 同值（不再按栈派生）
    let exclude = tun.route_exclude_address.as_ref().unwrap();
    assert!(exclude.contains(&"127.0.0.0/8".to_string()));
    assert!(exclude.contains(&"::1/128".to_string()));
}

#[test]
fn tun_mac_has_http_proxy_platform() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.platform = "darwin".into();
    let inbounds = build_inbounds(&config, None, &deps);
    let tun = &inbounds[1];
    assert!(tun.platform.is_some());
    assert!(tun.platform.as_ref().unwrap().http_proxy.is_some());
}

#[test]
fn tun_ipv6_address_when_enabled() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        enable_ipv6: Some(true),
        ..Default::default()
    };
    let inbounds = build_inbounds(&config, None, &deps_linux());
    let tun = &inbounds[1];
    let addr = tun.address.as_ref().unwrap();
    assert_eq!(addr.len(), 2); // v4 + v6
    assert!(addr[1].contains("::"));
}

#[test]
fn tun_probe_pool_inbounds() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let mut deps = deps_linux();
    deps.probe_pool_ports = vec![20000, 20001];
    let inbounds = build_inbounds(&config, None, &deps);
    // mixed + probe-in-0 + probe-in-1 + tun
    assert_eq!(inbounds.len(), 4);
    assert_eq!(inbounds[1].tag, "probe-in-0");
    assert_eq!(inbounds[2].tag, "probe-in-1");
}

// ── TUN stack 弃用：生成期不发 stack、MTU 默认与栈无关 ──────────────────────

/// Polaris 的全部发行平台（`deps.platform` 的取值域，与 `scripts/fetch-core.mjs` 的三个 OS 对应）。
const RELEASE_PLATFORMS: [&str; 3] = ["linux", "darwin", "win32"];

/// 在 `platform` 上生成 TUN inbound，取**序列化后的 JSON**（内核读的是它，不是结构体）。
fn tun_json_on(config: &UserConfig, platform: &str) -> serde_json::Value {
    let mut deps = deps_linux();
    deps.platform = platform.into();
    let tun = build_inbounds(config, None, &deps)
        .into_iter()
        .find(|i| i.type_field == "tun")
        .expect("TUN 模式必产出 tun inbound");
    serde_json::to_value(tun).expect("tun inbound 序列化失败")
}

/// 判据本体：TUN inbound 序列化形上的违规清单（空 = 合规）。正向用例与反向对照共用这一份，
/// 保证「喂进违规输入会红」证明的是**同一段**判据，而不是另写一份更严的。
fn tun_inbound_violations(tun: &serde_json::Value, want_mtu: u32) -> Vec<String> {
    let Some(obj) = tun.as_object() else {
        return vec![format!("tun inbound 不是 JSON 对象：{tun}")];
    };
    let mut out = Vec::new();
    if let Some(stack) = obj.get("stack") {
        out.push(format!(
            "发出了 `stack`={stack}（sing-box 1.15.0-alpha.3 起即报弃用，1.17 删除）"
        ));
    }
    if obj.get("mtu") != Some(&serde_json::json!(want_mtu)) {
        out.push(format!("`mtu` 应为 {want_mtu}，实得 {:?}", obj.get("mtu")));
    }
    out
}

/// 🔴 【生成期断言】三平台生成的 TUN inbound **一律不含 `stack` 键**，且用户未填 MTU 时 `mtu` 恒为
/// [`DEFAULT_TUN_MTU`]（键在场、值与平台无关）。
///
/// 输入刻意带上遗留的 `tunConfig.stack`（旧 UI 的四个取值 + 一个非法值 + 缺席）：磁盘上的旧配置
/// 在 store 迁移之前就可能被读进来，这里证明它**读得进来且漏不到生成侧**。
///
/// 反向对照见 [`tun_inbound_violations_has_teeth`]：同一判据喂「真实生成结果 + 手工塞回 stack」必红。
#[test]
fn tun_inbound_never_emits_stack_on_any_platform() {
    assert_eq!(
        DEFAULT_TUN_MTU, 65535,
        "默认值变了须同步 ui/src/domain/tun-mtu.ts"
    );
    let legacy_values = [
        None,
        Some("auto"),
        Some("system"),
        Some("gvisor"),
        Some("mixed"),
        Some("bogus"),
    ];
    let mut checked = 0usize;
    for legacy in legacy_values {
        let mut tun_cfg = serde_json::json!({ "autoRoute": true, "strictRoute": true });
        if let Some(stack) = legacy {
            tun_cfg["stack"] = serde_json::json!(stack);
        }
        let config: UserConfig = serde_json::from_value(serde_json::json!({
            "servers": [],
            "proxyModeType": "tun",
            "tunConfig": tun_cfg,
        }))
        .unwrap_or_else(|e| panic!("遗留 stack={legacy:?} 的用户配置反序列化失败：{e}"));
        for platform in RELEASE_PLATFORMS {
            let tun = tun_json_on(&config, platform);
            let violations = tun_inbound_violations(&tun, DEFAULT_TUN_MTU);
            assert!(
                violations.is_empty(),
                "[{platform} / 输入 stack={legacy:?}] {violations:?}\n实际: {tun}"
            );
            checked += 1;
        }
    }
    // 射程对账：6 种输入 × 3 平台，少一格说明有腿被静默跳过。
    assert_eq!(checked, legacy_values.len() * RELEASE_PLATFORMS.len());
}

/// 用户显式 MTU 仍**逐字**下发（三平台），不被新默认值顶掉。
#[test]
fn explicit_tun_mtu_is_emitted_verbatim_on_every_platform() {
    for mtu in [1280u32, 1400, 9000, 65535] {
        let config = UserConfig {
            proxy_mode_type: ProxyModeType::Tun,
            tun_config: Some(crate::user_config::tun_config::TunModeConfig {
                mtu: Some(mtu),
                ..Default::default()
            }),
            ..Default::default()
        };
        for platform in RELEASE_PLATFORMS {
            let tun = tun_json_on(&config, platform);
            let violations = tun_inbound_violations(&tun, mtu);
            assert!(
                violations.is_empty(),
                "[{platform} / mtu={mtu}] {violations:?}"
            );
        }
    }
}

/// 反向对照：判据必须对「stack 仍被发出」「mtu 不是默认值」两种形态各自转红。
///
/// 喂的是**真实生成结果**再手工改一处，而不是另造一份 JSON：这样证明的是「生成形状上的这一个键」
/// 被抓住，排除「判据其实只是在对象形状不对时才红」的假牙。
#[test]
fn tun_inbound_violations_has_teeth() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    for platform in RELEASE_PLATFORMS {
        let clean = tun_json_on(&config, platform);
        assert!(tun_inbound_violations(&clean, DEFAULT_TUN_MTU).is_empty());

        for stack in ["go", "system", "gvisor", "mixed", ""] {
            let mut leaked = clean.clone();
            leaked["stack"] = serde_json::json!(stack);
            let v = tun_inbound_violations(&leaked, DEFAULT_TUN_MTU);
            assert!(
                v.len() == 1 && v[0].contains("stack"),
                "[{platform}] 塞回 stack={stack:?} 判据没红：{v:?}"
            );
        }

        let mut wrong_mtu = clean.clone();
        wrong_mtu["mtu"] = serde_json::json!(9000);
        let v = tun_inbound_violations(&wrong_mtu, DEFAULT_TUN_MTU);
        assert!(
            v.len() == 1 && v[0].contains("mtu"),
            "[{platform}] 改 mtu 判据没红：{v:?}"
        );

        let mut no_mtu = clean.clone();
        no_mtu.as_object_mut().unwrap().remove("mtu");
        assert_eq!(
            tun_inbound_violations(&no_mtu, DEFAULT_TUN_MTU).len(),
            1,
            "[{platform}] mtu 缺席判据没红"
        );
    }
}

// ── NAT 类型（udp_mapping × udp_filtering）─────────────────────────────────

fn tun_with_nat(nat: Option<UdpNatType>) -> Inbound {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            udp_nat_type: nat,
            ..Default::default()
        }),
        ..Default::default()
    };
    let inbounds = build_inbounds(&config, None, &deps_linux());
    inbounds
        .into_iter()
        .find(|i| i.type_field == "tun")
        .expect("TUN 模式必产出 tun inbound")
}

/// 【不变式：默认零下发】未选档 ⇒ 序列化出的 TUN inbound **一个 udp_* 键都没有**。
///
/// 断言落在**序列化后的 JSON** 而非 `Option` 字段上：`skip_serializing_if` 漏写时字段仍是 `None`、
/// 结构体断言照绿，而 JSON 里会多出 `"udp_mapping": null` —— 那正是金样 `config-snapshot.json`
/// 转红的形态。变异锁：把 `unwrap_or((None, None))` 改成回落全锥、或删掉任一 `skip_serializing_if`，
/// 本条即红。
#[test]
fn udp_nat_absent_by_default_emits_no_key() {
    let json = serde_json::to_value(tun_with_nat(None)).unwrap();
    let obj = json.as_object().unwrap();
    for key in ["udp_mapping", "udp_filtering", "udp_nat_max"] {
        assert!(
            !obj.contains_key(key),
            "未选 NAT 类型档时不得下发 `{key}`（金样 config-snapshot.json 依赖这条零 delta），实际: {json}"
        );
    }
    // 同一条不变量的另一半：非 TUN inbound 永远不带这组键（内核 schema 里 mixed/http/socks 没有它们，
    // 发了即 `sing-box check` FATAL）。
    let config = UserConfig::default(); // 默认 systemProxy ⇒ 只有 mixed
    let mixed = serde_json::to_value(&build_inbounds(&config, None, &deps_linux())[0]).unwrap();
    assert!(!mixed.as_object().unwrap().contains_key("udp_mapping"));
}

/// 【映射表逐档钉死】三档 → `(udp_mapping, udp_filtering)`，值取**序列化后的字面量**
/// （枚举 `rename_all` 写错时结构体断言看不出来，内核只认字面量）。
///
/// 变异锁：把任一档的 filtering 挪一格（受限锥 ↔ 端口受限锥）即红；把哪一档的 mapping 收紧成
/// address_* 即红 —— 那会把锥形变成对称 NAT，档位名对用户的承诺当场作废。
#[test]
fn udp_nat_type_maps_each_tier() {
    let cases = [
        (
            UdpNatType::FullCone,
            "endpoint_independent",
            "endpoint_independent",
        ),
        (
            UdpNatType::RestrictedCone,
            "endpoint_independent",
            "address_dependent",
        ),
        (
            UdpNatType::PortRestrictedCone,
            "endpoint_independent",
            "address_and_port_dependent",
        ),
    ];
    for (nat, mapping, filtering) in cases {
        let json = serde_json::to_value(tun_with_nat(Some(nat))).unwrap();
        assert_eq!(
            json.get("udp_mapping").and_then(|v| v.as_str()),
            Some(mapping),
            "{nat:?} 的 udp_mapping 不符"
        );
        assert_eq!(
            json.get("udp_filtering").and_then(|v| v.as_str()),
            Some(filtering),
            "{nat:?} 的 udp_filtering 不符"
        );
    }
}

/// `udpNatType` 走用户配置 JSON（camelCase）反序列化 —— 前端 `TunModeConfig.udpNatType` 与
/// Rust 的键名/值名是同一套字面量，改名只在这里被抓住（前端那侧没有编译期依赖）。
#[test]
fn udp_nat_type_deserializes_from_user_config_json() {
    let config: UserConfig = serde_json::from_str(
        r#"{"servers":[],"proxyModeType":"tun","tunConfig":{"udpNatType":"portRestrictedCone"}}"#,
    )
    .expect("用户配置反序列化失败");
    assert_eq!(
        config.tun_config.as_ref().unwrap().udp_nat_type,
        Some(UdpNatType::PortRestrictedCone)
    );
    let tun = build_inbounds(&config, None, &deps_linux())
        .into_iter()
        .find(|i| i.type_field == "tun")
        .unwrap();
    assert_eq!(
        tun.udp_filtering,
        Some(UdpNatBehavior::AddressAndPortDependent)
    );
}

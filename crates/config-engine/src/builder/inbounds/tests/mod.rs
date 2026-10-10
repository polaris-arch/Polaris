use super::*;
// 生产侧已改读「本平台生效值」（`ProxyModeType::effective_on`），不再直接引这个类型；
// 测试仍要按档构造输入，故在此单独引入。
use crate::user_config::ProxyModeType;

fn deps_linux() -> InboundsDeps {
    InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: vec![],
        platform: "linux".into(),
        own_lan_cidrs: vec![],
        // 无运行期观测 ⇒ engaged_mesh 仅含 tailnet 默认常量段，与本字段出现之前同。
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    }
}

#[test]
fn debug_probe_mixed_udp_is_same_authenticated_probe_only_on_android() {
    let mut deps = deps_linux();
    deps.probe_proxy_port = Some(19385);
    deps.debug_probe_mixed_udp = true;
    deps.loopback_auth = Some(InboundUser {
        username: "polaris".into(),
        password: "b".repeat(32),
    });
    for platform in ["android", "linux", "darwin", "win32"] {
        deps.platform = platform.into();
        let list = build_inbounds(&UserConfig::default(), None, &deps);
        let probe = list.iter().find(|i| i.tag == "probe-proxy-in").unwrap();
        assert_eq!(
            probe.type_field,
            if cfg!(debug_assertions) && platform == "android" {
                "mixed"
            } else {
                "http"
            }
        );
        assert_eq!(probe.listen.as_deref(), Some("127.0.0.1"));
        assert_eq!(probe.listen_port, Some(19385));
        if platform == "android" {
            assert_eq!(probe.users.as_ref().unwrap()[0].password, "b".repeat(32));
        }
    }
    deps.platform = "android".into();
    deps.debug_probe_mixed_udp = false;
    assert_eq!(
        build_inbounds(&UserConfig::default(), None, &deps)
            .iter()
            .find(|i| i.tag == "probe-proxy-in")
            .unwrap()
            .type_field,
        "http"
    );
    deps.debug_probe_mixed_udp = true;
    deps.loopback_auth = None;
    assert!(!build_inbounds(&UserConfig::default(), None, &deps)
        .iter()
        .any(|i| i.tag == "probe-proxy-in"));
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
            network_profile_id: None,
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
    assert_eq!(tun.mtu, None); // 未设 → 不发键，交内核取默认（全平台逐格见 `tun_platform_axis`）
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
    assert_eq!(tun.mtu, None); // mac 同样不发键
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

// ── TUN stack 弃用：生成期不发 stack；MTU 缺席即不发键（交内核取默认）──────────

/// 平台轴：**从 `Platform::ALL` 派生**，每个变体 → 生成期用的 `deps.platform` 串。
///
/// - 轴不手抄：遍历的是 `Platform::ALL`，下面是穷举 `match` —— 新增平台变体时这里编译不过，
///   逼着为它答「生成期拿什么串」。
/// - 串与变体的对应由 `Platform::parse` 当场反证：写错一个串即红，否则那一格测的其实是 `Other` 的腿。
fn tun_platform_axis() -> Vec<(Platform, &'static str)> {
    let axis: Vec<(Platform, &'static str)> = Platform::ALL
        .iter()
        .copied()
        .map(|p| {
            let name = match p {
                Platform::Mac => "darwin",
                Platform::Win => "win32",
                Platform::Linux => "linux",
                Platform::Android => "android",
                Platform::Ios => "ios",
                // 未知平台：`Platform::parse` 不认得的任意串都落这里。
                Platform::Other => "freebsd",
            };
            assert_eq!(
                Platform::parse(name),
                p,
                "平台串 {name:?} 被 Platform::parse 解成了别的变体 —— 这一格测的不是 {p:?}"
            );
            (p, name)
        })
        .collect();
    // 取材面自检：轴塌了（ALL 被清空 / 过滤写歪）时下面的循环一次都不跑也全绿。
    assert_eq!(axis.len(), Platform::ALL.len());
    assert!(
        axis.iter().any(|(p, _)| *p == Platform::Android)
            && axis.iter().any(|(p, _)| *p == Platform::Ios),
        "平台轴上没有 android / ios —— 移动端没被测到"
    );
    axis
}

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
///
/// `want_mtu`：`None` = 用户未设 ⇒ `mtu` 键**必须缺席**（交内核取默认）；`Some(n)` ⇒ 键在场且逐字为 `n`。
fn tun_inbound_violations(tun: &serde_json::Value, want_mtu: Option<u32>) -> Vec<String> {
    let Some(obj) = tun.as_object() else {
        return vec![format!("tun inbound 不是 JSON 对象：{tun}")];
    };
    let mut out = Vec::new();
    if let Some(stack) = obj.get("stack") {
        out.push(format!(
            "发出了 `stack`={stack}（sing-box 1.15.0-alpha.3 起即报弃用，1.17 删除）"
        ));
    }
    let got = obj.get("mtu");
    match want_mtu {
        None if got.is_some() => out.push(format!(
            "用户未设 MTU 却下发了 `mtu`={got:?}（应缺席，交内核按运行环境取默认）"
        )),
        Some(n) if got != Some(&serde_json::json!(n)) => {
            out.push(format!("`mtu` 应为用户显式值 {n}，实得 {got:?}"));
        }
        _ => {}
    }
    out
}

/// 🔴 【生成期断言】全平台（[`tun_platform_axis`]，含 android / ios）生成的 TUN inbound **一律不含
/// `stack` 键**，且用户未设 MTU 时**不含 `mtu` 键**。
///
/// 输入刻意带上遗留的 `tunConfig.stack`（旧 UI 的四个取值 + 一个非法值 + 缺席）：磁盘上的旧配置
/// 在 store 迁移之前就可能被读进来，这里证明它**读得进来且漏不到生成侧**。
///
/// 反向对照见 [`tun_inbound_violations_has_teeth`]：同一判据喂「真实生成结果 + 手工塞回 stack」必红。
#[test]
fn tun_inbound_never_emits_stack_on_any_platform() {
    let axis = tun_platform_axis();
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
        for &(_, platform) in &axis {
            let tun = tun_json_on(&config, platform);
            let violations = tun_inbound_violations(&tun, None);
            assert!(
                violations.is_empty(),
                "[{platform} / 输入 stack={legacy:?}] {violations:?}\n实际: {tun}"
            );
            checked += 1;
        }
    }
    // 射程对账：6 种输入 × 全平台，少一格说明有腿被静默跳过。
    assert_eq!(checked, legacy_values.len() * axis.len());
}

/// 🔴 用户**未设** MTU 时，任何平台（`tunConfig` 缺席 / 在场但无 `mtu` / `mtu: null` 三种形态）
/// 生成的 TUN inbound 都**不含 `mtu` 键** —— 默认值由内核按运行环境取（上游 `inbound.go` 的
/// `options.MTU == 0` 分支），Polaris 不持有平台 → MTU 表。
///
/// 正面断言：同一份 inbound 上 `type == "tun"`、`tag == "tun-in"` 在场 —— 排除「整个 inbound
/// 没生成 / 生成了别的东西」时「没有 mtu 键」恒真的假绿。
#[test]
fn tun_inbound_omits_mtu_when_unset_on_every_platform() {
    let inputs = [
        serde_json::json!({ "servers": [], "proxyModeType": "tun" }),
        serde_json::json!({ "servers": [], "proxyModeType": "tun",
            "tunConfig": { "autoRoute": true, "strictRoute": true } }),
        serde_json::json!({ "servers": [], "proxyModeType": "tun",
            "tunConfig": { "autoRoute": true, "strictRoute": true, "mtu": null } }),
    ];
    let axis = tun_platform_axis();
    let mut checked = 0usize;
    for input in &inputs {
        let config: UserConfig = serde_json::from_value(input.clone())
            .unwrap_or_else(|e| panic!("输入 {input} 反序列化失败：{e}"));
        for &(_, platform) in &axis {
            let tun = tun_json_on(&config, platform);
            assert_eq!(tun["type"], "tun", "[{platform}] 不是 tun inbound：{tun}");
            assert_eq!(tun["tag"], "tun-in", "[{platform}] tag 不对：{tun}");
            assert!(
                tun.as_object().is_some_and(|o| !o.contains_key("mtu")),
                "[{platform} / 输入 {input}] 用户未设 MTU 却下发了 mtu：{tun}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, inputs.len() * axis.len());
}

/// 用户显式 MTU **逐字**下发（全平台）。
#[test]
fn explicit_tun_mtu_is_emitted_verbatim_on_every_platform() {
    let axis = tun_platform_axis();
    for mtu in [1280u32, 1400, 4064, 9000, 65535] {
        let config = UserConfig {
            proxy_mode_type: ProxyModeType::Tun,
            tun_config: Some(crate::user_config::tun_config::TunModeConfig {
                mtu: Some(mtu),
                ..Default::default()
            }),
            ..Default::default()
        };
        for &(_, platform) in &axis {
            let tun = tun_json_on(&config, platform);
            let violations = tun_inbound_violations(&tun, Some(mtu));
            assert!(
                violations.is_empty(),
                "[{platform} / mtu={mtu}] {violations:?}"
            );
        }
    }
}

/// 反向对照：判据必须对「stack 仍被发出」「未设却发了 mtu」「显式值被改 / 被丢」各自转红。
///
/// 喂的是**真实生成结果**再手工改一处，而不是另造一份 JSON：这样证明的是「生成形状上的这一个键」
/// 被抓住，排除「判据其实只是在对象形状不对时才红」的假牙。
#[test]
fn tun_inbound_violations_has_teeth() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    for (_, platform) in tun_platform_axis() {
        let clean = tun_json_on(&config, platform);
        assert!(tun_inbound_violations(&clean, None).is_empty());

        for stack in ["go", "system", "gvisor", "mixed", ""] {
            let mut leaked = clean.clone();
            leaked["stack"] = serde_json::json!(stack);
            let v = tun_inbound_violations(&leaked, None);
            assert!(
                v.len() == 1 && v[0].contains("stack"),
                "[{platform}] 塞回 stack={stack:?} 判据没红：{v:?}"
            );
        }

        // 未设却发了 mtu（任何值，含与上游默认同值的 65535 / 9000 / 4064）必红。
        for n in [65535, 9000, 4064] {
            let mut leaked = clean.clone();
            leaked["mtu"] = serde_json::json!(n);
            let v = tun_inbound_violations(&leaked, None);
            assert!(
                v.len() == 1 && v[0].contains("mtu"),
                "[{platform}] 未设却塞了 mtu={n} 判据没红：{v:?}"
            );
            // 显式值被改写 / 被丢：
            let v = tun_inbound_violations(&leaked, Some(1400));
            assert!(
                v.len() == 1,
                "[{platform}] 显式 1400 被改成 {n} 判据没红：{v:?}"
            );
        }
        assert_eq!(
            tun_inbound_violations(&clean, Some(1400)).len(),
            1,
            "[{platform}] 显式 mtu 被丢（缺席）判据没红"
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

/// Tailcat 节点 `address` 恒空：非 Linux 的「节点 IP 排除」改从 servers 模式 DERP 项的
/// host（含裸字符串项）/ `ipv4` / `ipv6` 里的 IP 字面值取（主机名 / `"none"` / 空跳过）；region 模式 IP 未知不加；Linux 整段不发射。
#[test]
fn tailcat_derp_server_ips_join_route_exclude_on_non_linux() {
    use crate::user_config::protocol_settings::TailcatSettings;
    use crate::user_config::server_config::{Protocol, ServerConfig};
    let tailcat = |derp_region: Option<i64>, derp_servers: Vec<serde_json::Value>| UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        selected_server_id: Some("tc".into()),
        servers: vec![ServerConfig {
            id: "tc".into(),
            name: "TC".into(),
            protocol: Protocol::Tailcat,
            tailcat_settings: Some(Box::new(TailcatSettings {
                derp_region,
                derp_servers,
                ..Default::default()
            })),
            ..Default::default()
        }],
        ..Default::default()
    };
    let exclude = |config: &UserConfig, platform: &str| -> Vec<String> {
        let mut deps = deps_linux();
        deps.platform = platform.into();
        let inbounds = build_inbounds(config, None, &deps);
        let tun = inbounds.iter().find(|i| i.tag == "tun-in").expect("有 tun");
        tun.route_exclude_address.clone().unwrap_or_default()
    };
    let servers = tailcat(
        None,
        vec![
            serde_json::json!({ "host": "derp1.example.com", "ipv4": "203.0.113.7", "ipv6": "2001:db8::7" }),
            serde_json::json!({ "host": "derp2.example.com", "ipv4": "none", "ipv6": "" }),
            serde_json::json!({ "host": "derp3.example.com", "ipv4": "derp3.example.com" }),
            serde_json::json!("derp4.example.com"),
            // host 为 IP 字面值同样排除；与 ipv4 同值只加一次。
            serde_json::json!({ "host": "198.51.100.5", "ipv4": "198.51.100.5" }),
            serde_json::json!({ "host": "[2001:db8::5]" }),
            // 裸字符串项即 host。
            serde_json::json!("198.51.100.6"),
        ],
    );
    let got = exclude(&servers, "darwin");
    assert!(got.contains(&"203.0.113.7/32".to_string()), "{got:?}");
    assert!(got.contains(&"2001:db8::7/128".to_string()), "{got:?}");
    for cidr in ["198.51.100.5/32", "2001:db8::5/128", "198.51.100.6/32"] {
        assert!(
            got.contains(&cidr.to_string()),
            "host 为 IP 字面值须排除 {cidr}：{got:?}"
        );
    }
    assert_eq!(
        got.iter().filter(|c| *c == "198.51.100.5/32").count(),
        1,
        "host 与 ipv4 同值只加一次：{got:?}"
    );
    assert!(
        !got.iter().any(|c| c.contains("none") || c.contains("derp")),
        "\"none\" / 非 IP 字面值必须跳过：{got:?}"
    );
    let baseline = exclude(
        &UserConfig {
            proxy_mode_type: ProxyModeType::Tun,
            ..Default::default()
        },
        "darwin",
    );
    assert_eq!(
        got.len(),
        baseline.len() + 5,
        "servers 模式恰多出五条 DERP 字面 IP：{got:?} vs {baseline:?}"
    );

    let region = tailcat(Some(1), vec![]);
    assert_eq!(
        exclude(&region, "darwin"),
        baseline,
        "region 模式 DERP IP 未知，不得多加任何段"
    );

    assert!(
        exclude(&servers, "linux").is_empty(),
        "Linux 恒不发射 route_exclude_address（与节点 IP 排除的非 Linux 条件一致）"
    );
}

// One JSON predicate is shared by positive cases and adversarial mutations.
fn dns_contract_holds(value: &serde_json::Value, desktop_tun: bool, auto_route: bool) -> bool {
    value.get("auto_route") == Some(&serde_json::json!(auto_route))
        && if desktop_tun && !auto_route {
            value.get("dns_mode") == Some(&serde_json::json!("hijack"))
        } else {
            value.get("dns_mode").is_none()
        }
}

#[test]
fn tun_dns_contract_all_platforms_and_missing_config() {
    for (platform, name) in tun_platform_axis() {
        let desktop = matches!(platform, Platform::Linux | Platform::Mac | Platform::Win);
        for configured in [None, Some(true), Some(false)] {
            let mut config = UserConfig {
                proxy_mode_type: ProxyModeType::Tun,
                ..Default::default()
            };
            config.tun_config =
                configured.map(|auto_route| crate::user_config::tun_config::TunModeConfig {
                    auto_route,
                    ..Default::default()
                });
            let tun = tun_json_on(&config, name);
            assert!(
                dns_contract_holds(&tun, desktop, configured.unwrap_or(true)),
                "{name}/{configured:?}: {tun}"
            );
            // Every mixed, direct, authenticated probe and update inbound stays unchanged.
            let mut deps = deps_linux();
            deps.platform = name.into();
            deps.probe_direct_port = Some(21001);
            deps.probe_proxy_port = Some(21002);
            deps.update_in_port = Some(21003);
            deps.loopback_auth = Some(InboundUser {
                username: "test".into(),
                password: "b".repeat(32),
            });
            for inbound in build_inbounds(&config, None, &deps) {
                if inbound.type_field != "tun" {
                    assert!(serde_json::to_value(inbound)
                        .unwrap()
                        .get("dns_mode")
                        .is_none());
                }
            }
            config.proxy_mode_type = ProxyModeType::SystemProxy;
            for inbound in build_inbounds(&config, None, &deps) {
                assert!(serde_json::to_value(inbound)
                    .unwrap()
                    .get("dns_mode")
                    .is_none());
            }
        }
    }
}

#[test]
fn tun_dns_contract_rejects_old_output_and_wrong_modes() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(crate::user_config::tun_config::TunModeConfig {
            auto_route: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    for name in ["linux", "darwin", "win32"] {
        let tun = tun_json_on(&config, name);
        assert!(dns_contract_holds(&tun, true, false));
        let mut old_output = tun.clone();
        old_output.as_object_mut().unwrap().remove("dns_mode");
        assert!(
            !dns_contract_holds(&old_output, true, false),
            "old77 manual-route output must fail"
        );
        for wrong in ["disabled", "native", "typo"] {
            let mut bad = tun.clone();
            bad["dns_mode"] = serde_json::json!(wrong);
            assert!(!dns_contract_holds(&bad, true, false));
        }
        let mut forced_route = tun;
        forced_route["auto_route"] = serde_json::json!(true);
        assert!(!dns_contract_holds(&forced_route, true, false));
    }
    for name in ["android", "ios", "freebsd"] {
        let mut bad = tun_json_on(&config, name);
        bad["dns_mode"] = serde_json::json!("hijack");
        assert!(!dns_contract_holds(&bad, false, false));
    }
}

#[test]
fn tun_dns_mode_is_typed_and_omission_roundtrips() {
    for (mode, spelling) in [
        (TunDnsMode::Disabled, "disabled"),
        (TunDnsMode::Native, "native"),
        (TunDnsMode::Hijack, "hijack"),
    ] {
        assert_eq!(
            serde_json::to_value(mode).unwrap(),
            serde_json::json!(spelling)
        );
        assert_eq!(
            serde_json::from_value::<TunDnsMode>(serde_json::json!(spelling)).unwrap(),
            mode
        );
    }
    assert!(serde_json::from_value::<TunDnsMode>(serde_json::json!("unexpected")).is_err());
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let original = tun_json_on(&config, "linux");
    let roundtrip: Inbound = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(roundtrip.dns_mode, None);
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), original);
}

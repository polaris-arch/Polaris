use super::*;
use crate::runtime::management_api::tests::FakeConnectionApi;
use crate::runtime::proxy::connection_flush::FlushOutcome;
use polaris_switch_engine::ManagementError;

/// 被测对象是 `managed_tun_interface_for_network_watcher` / `managed_tun_interface_for_session` /
/// `ExitInterfaceId` —— 三者都留在 src-tauri，故这半段不随 E2② 搬走。
///
/// 它原本是 `dns_monitor_lines_follow_platform_event_shapes` 的后半段；前半段（平台 monitor 行的
/// 解析）已随被测符号迁进 `polaris-platform-events`。**测试跟着被测符号走，不跟着断言里出现的类型走。**
#[test]
fn managed_tun_interface_resolution_follows_platform_and_mode() {
    let mut config = UserConfig {
        proxy_mode_type: polaris_config_engine::user_config::proxy_mode::ProxyModeType::Tun,
        tun_config: Some(Default::default()),
        ..Default::default()
    };
    assert_eq!(
        managed_tun_interface_for_network_watcher(&config, Platform::Win).as_deref(),
        Some("polaris-tun0")
    );
    assert_eq!(
        managed_tun_interface_for_network_watcher(&config, Platform::Linux).as_deref(),
        Some(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME)
    );
    assert_eq!(
        managed_tun_interface_for_network_watcher(&config, Platform::Mac),
        None
    );
    assert_eq!(
        managed_tun_interface_for_session(
            &config,
            Platform::Mac,
            ExitInterfaceId::from_alias("utun8")
        ),
        ExitInterfaceId::from_alias("utun8"),
        "macOS 动态 utun 必须由 post-flight 捕获值补齐"
    );
    let win_session = managed_tun_interface_for_session(
        &config,
        Platform::Win,
        Some(ExitInterfaceId {
            alias: None,
            ifindex: Some(42),
        }),
    )
    .expect("Windows TUN 会话身份必须可得");
    assert_eq!(
        win_session.alias(),
        Some("polaris-tun0"),
        "Windows 路由闸返回 ifindex，接口快照/订阅必须继续用稳定别名"
    );
    assert_eq!(
        win_session.ifindex,
        Some(42),
        "同一张网卡的 ifindex 必须并进同一份身份，否则退场等待没有可比表示"
    );
    assert_eq!(
        managed_tun_interface_for_session(&config, Platform::Linux, None)
            .as_ref()
            .and_then(ExitInterfaceId::alias),
        Some(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME),
        "Linux 会话必须沿用 helper 与 sing-box 共同约定的稳定接口名"
    );
    config.proxy_mode_type =
        polaris_config_engine::user_config::proxy_mode::ProxyModeType::SystemProxy;
    assert_eq!(
        managed_tun_interface_for_network_watcher(&config, Platform::Win),
        None
    );
    assert_eq!(
        managed_tun_interface_for_session(
            &config,
            Platform::Mac,
            ExitInterfaceId::from_alias("utun8")
        ),
        None,
        "非 TUN 模式不得消费捕获接口"
    );
}

/// F5 回归：Windows 口径下「配置别名 + 路由闸 ifindex」与探测出的 ifindex 是同一张网卡。
/// 旧实现拿别名字符串 `!=` `"ifindex:42"`，必然判成两张网卡。
#[test]
fn exit_interface_identity_compares_windows_alias_and_ifindex_as_one_interface() {
    let managed = ExitInterfaceId::from_alias("polaris-tun0")
        .expect("非空别名是合法身份")
        .merged_with(Some(ExitInterfaceId {
            alias: None,
            ifindex: Some(42),
        }));
    let same = ExitInterfaceId {
        alias: None,
        ifindex: Some(42),
    };
    let other = ExitInterfaceId {
        alias: None,
        ifindex: Some(43),
    };
    assert_eq!(managed.same_interface(&same), Some(true));
    assert_eq!(managed.same_interface(&other), Some(false));
    assert_eq!(
        managed.alias(),
        Some("polaris-tun0"),
        "合并只补缺失表示，配置别名不得被覆盖"
    );

    // 只有别名的两侧按别名比。
    let mac = ExitInterfaceId::from_alias("utun8").unwrap();
    assert_eq!(
        mac.same_interface(&ExitInterfaceId::from_alias(" utun8 ").unwrap()),
        Some(true)
    );
    assert_eq!(
        mac.same_interface(&ExitInterfaceId::from_alias("utun9").unwrap()),
        Some(false)
    );

    // 没有共同表示 = 不可比，**不是**「不同」。
    assert_eq!(mac.same_interface(&same), None);
    assert_eq!(ExitInterfaceId::from_alias("   "), None);
}

/// F5 回归：退场等待的三态结局必须各自可达且可辨。
#[tokio::test]
async fn retiring_tun_route_wait_reports_each_outcome() {
    let managed = ExitInterfaceId::from_alias("polaris-tun0")
        .unwrap()
        .merged_with(Some(ExitInterfaceId {
            alias: None,
            ifindex: Some(42),
        }));
    let old_tun = ExitInterfaceId {
        alias: None,
        ifindex: Some(42),
    };
    let physical = ExitInterfaceId {
        alias: None,
        ifindex: Some(7),
    };

    // skipped：没有可等的对象。
    assert_eq!(
        wait_for_retiring_tun_route_outcome(None, 4, Duration::ZERO, || async {
            unreachable!("没有退场对象时不得发起任何探测")
        })
        .await,
        RetiringTunRouteOutcome::Skipped("no_managed_tun_interface")
    );

    // skipped：身份不可比（macOS 别名身份撞 Windows ifindex 探测）——旧实现在这里静默返回。
    let alias_only = ExitInterfaceId::from_alias("polaris-tun0").unwrap();
    assert_eq!(
        wait_for_retiring_tun_route_outcome(Some(&alias_only), 4, Duration::ZERO, || {
            let observed = old_tun.clone();
            async move { Some(observed) }
        })
        .await,
        RetiringTunRouteOutcome::Skipped("incomparable_interface_identity")
    );

    // matched：第 3 次探测出口终于切走。
    let observations = std::sync::Mutex::new(vec![
        Some(old_tun.clone()),
        Some(old_tun.clone()),
        Some(physical.clone()),
    ]);
    assert_eq!(
        wait_for_retiring_tun_route_outcome(Some(&managed), 8, Duration::ZERO, || {
            let next = observations.lock().unwrap().remove(0);
            async move { next }
        })
        .await,
        RetiringTunRouteOutcome::Retired { polls: 3 }
    );

    // timeout：界内每一次都仍是旧 TUN。
    assert_eq!(
        wait_for_retiring_tun_route_outcome(Some(&managed), 4, Duration::ZERO, || {
            let observed = old_tun.clone();
            async move { Some(observed) }
        })
        .await,
        RetiringTunRouteOutcome::TimedOut { polls: 4 }
    );

    // 探测不可读沿用旧行为：不空等满界。
    assert_eq!(
        wait_for_retiring_tun_route_outcome(Some(&managed), 4, Duration::ZERO, || async { None })
            .await,
        RetiringTunRouteOutcome::Retired { polls: 1 }
    );
}

#[test]
fn runtime_binding_replan_matrix_filters_noise_and_keeps_failover_safe() {
    let plan = RuntimeBindingPlan {
        bindings: BTreeMap::from([("node-a".into(), "en0".into())]),
        native_roots: BTreeSet::new(),
        covered_roots: BTreeSet::from(["node-a".into()]),
        probe_ips: BTreeMap::from([("node-a".into(), "198.51.100.77".parse().unwrap())]),
        candidate_count: 1,
        unresolved_roots: BTreeMap::new(),
    };
    let baseline = BTreeMap::from([
        ("en0".into(), (true, vec!["192.0.2.2".into()])),
        ("en9".into(), (true, vec!["198.51.100.2".into()])),
    ]);
    let unrelated_change = BTreeMap::from([
        ("en0".into(), (true, vec!["192.0.2.2".into()])),
        ("en9".into(), (false, vec!["198.51.100.2".into()])),
    ]);
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_unknown: true,
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_prefixes: BTreeSet::from([RoutePrefix::parse("198.51.100.0/24").unwrap(),]),
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_prefixes: BTreeSet::from([RoutePrefix::parse("192.0.2.0/24").unwrap(),]),
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_prefixes: BTreeSet::from([RoutePrefix::parse("0.0.0.0/0").unwrap()]),
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            interface: true,
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&unrelated_change),
        None,
    ));
    let bound_down = BTreeMap::from([("en0".into(), (false, Vec::new()))]);
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            interface: true,
            ..Default::default()
        },
        &plan,
        Some(&baseline),
        Some(&bound_down),
        None,
    ));

    let unresolved = RuntimeBindingPlan {
        candidate_count: 1,
        unresolved_roots: BTreeMap::from([("node-a".into(), "node-a.example.com".into())]),
        ..Default::default()
    };
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            interface: true,
            ..Default::default()
        },
        &unresolved,
        Some(&baseline),
        Some(&unrelated_change),
        None,
    ));

    let tun_added = BTreeMap::from([
        ("en0".into(), (true, vec!["192.0.2.2".into()])),
        ("en9".into(), (true, vec!["198.51.100.2".into()])),
        ("utun8".into(), (true, vec!["172.19.0.1".into()])),
    ]);
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            interface: true,
            ..Default::default()
        },
        &unresolved,
        Some(&baseline),
        Some(&tun_added),
        Some("utun8"),
    ));
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            interface: true,
            ..Default::default()
        },
        &unresolved,
        Some(&baseline),
        Some(&tun_added),
        None,
    ));

    let tun_added_and_physical_changed = BTreeMap::from([
        ("en0".into(), (false, vec!["192.0.2.2".into()])),
        ("en9".into(), (true, vec!["198.51.100.2".into()])),
        ("utun8".into(), (true, vec!["172.19.0.1".into()])),
    ]);
    assert!(
        inferred_binding_replan_needed(
            &NetworkChangeImpact {
                interface: true,
                ..Default::default()
            },
            &unresolved,
            Some(&baseline),
            Some(&tun_added_and_physical_changed),
            Some("utun8"),
        ),
        "忽略受管 TUN 只能消除自身噪音，不得吞掉同窗发生的物理接口变化"
    );

    let native = RuntimeBindingPlan {
        native_roots: BTreeSet::from(["node-a".into()]),
        covered_roots: BTreeSet::from(["node-a".into()]),
        probe_ips: BTreeMap::from([("node-a".into(), "198.51.100.77".parse().unwrap())]),
        candidate_count: 1,
        ..Default::default()
    };
    // F13：native 计划下的未知路由事件也要重算 —— 它可能就是下面那条 `198.51.100.77/32`
    // （只是事件源没能把前缀交出来）。此处此前断言 false，与下一格自相矛盾。
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            interface: true,
            route_unknown: true,
            ..Default::default()
        },
        &native,
        Some(&baseline),
        Some(&unrelated_change),
        None,
    ));
    assert!(inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_prefixes: BTreeSet::from([RoutePrefix::parse("198.51.100.77/32").unwrap(),]),
            ..Default::default()
        },
        &native,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
    assert!(!inferred_binding_replan_needed(
        &NetworkChangeImpact {
            route: true,
            route_prefixes: BTreeSet::from([RoutePrefix::parse("203.0.113.0/24").unwrap(),]),
            ..Default::default()
        },
        &native,
        Some(&baseline),
        Some(&baseline),
        None,
    ));
}

#[test]
fn existing_selector_roots_are_hot_switchable_but_runtime_additions_require_restart() {
    use polaris_config_engine::user_config::proxy_mode::ProxyModeType;
    use polaris_config_engine::user_config::server_config::Protocol;

    let server = |id: &str, address: &str| ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Vless,
        address: address.into(),
        port: 443,
        ..Default::default()
    };
    let plan = RuntimeBindingPlan {
        native_roots: BTreeSet::new(),
        covered_roots: BTreeSet::from(["node-a".to_string(), "node-b".to_string()]),
        candidate_count: 2,
        unresolved_roots: BTreeMap::from([
            ("node-a".into(), "1.1.1.1".into()),
            ("node-b".into(), "2.2.2.2".into()),
        ]),
        ..Default::default()
    };
    let mut config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        servers: vec![server("node-a", "1.1.1.1"), server("node-b", "2.2.2.2")],
        selected_server_id: Some("node-a".into()),
        ..Default::default()
    };
    assert!(runtime_binding_roots_covered(&config, &plan));

    config.selected_server_id = Some("node-b".into());
    assert!(
        runtime_binding_roots_covered(&config, &plan),
        "同一运行核已生成的 B 必须可从 A 真热切，不能因启动时闲置而重启"
    );

    config.servers.push(server("node-c", "3.3.3.3"));
    assert!(
        runtime_binding_roots_covered(&config, &plan),
        "订阅只新增闲置 C 时仍由当前 B 承流，不得提前重启"
    );
    config.selected_server_id = Some("node-c".into());
    assert!(
        !runtime_binding_roots_covered(&config, &plan),
        "运行期新增 C 未进入当前核的路由规划，必须重启后再承流"
    );

    use polaris_config_engine::user_config::proxy_mode::ProxyMode;
    use polaris_config_engine::user_config::rule::{
        Rule, RuleAction, RuleEffects, RuleRouteEffect, RuleType,
    };
    config.selected_server_id = Some("node-a".into());
    config.proxy_mode = ProxyMode::Smart;
    config.traffic_rules = Some(vec![Rule {
        id: "to-c".into(),
        type_field: RuleType::Domain,
        values: vec!["example.com".into()],
        action: RuleAction::Proxy,
        enabled: true,
        effects: Some(RuleEffects {
            route: Some(RuleRouteEffect {
                enabled: true,
                action: RuleAction::Proxy,
                target_server_id: Some("node-c".into()),
                destination_resolution: None,
                resolution_only: false,
            }),
            dns: None,
        }),
        ..Default::default()
    }]);
    assert!(!runtime_binding_roots_covered(&config, &plan));
}

#[test]
fn explicit_interface_unavailability_distinguishes_missing_down_and_recovery() {
    let required = BTreeSet::from(["en0".to_owned(), "utun7".to_owned()]);
    let observed = BTreeMap::from([("en0".into(), (false, Vec::new()))]);
    let unavailable = required_interfaces_unavailable(&required, &observed);
    assert_eq!(unavailable.down, BTreeSet::from(["en0".to_owned()]));
    assert_eq!(unavailable.missing, BTreeSet::from(["utun7".to_owned()]));

    let recovered = BTreeMap::from([
        ("en0".into(), (true, Vec::new())),
        ("utun7".into(), (true, Vec::new())),
    ]);
    assert!(required_interfaces_unavailable(&required, &recovered).is_empty());
}

// ══════════════════════════════════════════════════════════════════════════════
// A1 系统代理启用侧（最大缺口：systemProxy 模式 start 成功却从不设 OS 代理 → 流量不经核）
//
// 注：**start_inner 内的调用点**（wait_ready 成功后）无法在本机验证——它须真核就绪、而本机硬禁
// 起核（同 residual 发射的约束，见其上方注释）。故此处覆盖 `maybe_enable_system_proxy` 的**全部
// 决策 + 装配逻辑**（模式门控 / enable 真被调 / req 参数），start_inner 的单行调用点靠代码审查背书
// （诚实披露，见报告）。enable 内部状态机（marker/防自指/fail-closed 回滚）另在
// `system-integration::proxy_ops` 单测覆盖。
// ══════════════════════════════════════════════════════════════════════════════

/// C-tun-conflict 模式守卫：桌面三平台上 TUN 出口夺取硬闸**仅**适用 TUN 模式。
/// systemProxy/manual 不接管 tun、出口恒在物理网卡 → baseline 差分永不成立，设闸必误判 → 不闸（caveat）。
/// 变异锁：改成恒 true → systemProxy/manual 起核会被本不该有的闸拦（且 baseline/verify 空跑）。
#[test]
fn tun_route_gate_only_applies_to_tun_mode_on_desktop() {
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(tun_route_gate_applies(ProxyModeType::Tun, platform));
        assert!(!tun_route_gate_applies(
            ProxyModeType::SystemProxy,
            platform
        ));
        assert!(!tun_route_gate_applies(ProxyModeType::Manual, platform));
    }
}

/// **Android / 未知平台：三个存盘档位得到同一个答案 —— 不挂闸。**
///
/// 判据本体是 baseline 差分，而这两个平台上
/// [`SystemRouteOps::exit_interface_for`](polaris_system_integration::route_ops::SystemRouteOps::exit_interface_for)
/// 恒 `Ok(None)`（Android 臂由 `system-integration` 的
/// `impl_android_returns_none_and_never_spawns_ip` 钉住）⇒ 前后两次探测都读不出出口 ⇒ 判定
/// 恒 `Indeterminate`（放行）。挂着闸唯一的效果是每次起核在主链上白付一整个 grace 窗口
/// （`TUN_ROUTE_GRACE_POLLS` × `TUN_ROUTE_POLL_INTERVAL` ≈ 3.5s）。
///
/// **这不是假设**：本条修复前，存盘值恰为 `tun` 的 Android 客户端已经在付这笔钱；而把接管方式
/// 改读生效值之后（Android 恒 TUN），全部 Android 客户端都会付。
///
/// 变异锁：删掉 `tun_route_gate_applies` 里的 `Platform::Android => false` 臂（改成走 true 那一支）
/// → 本测三档全红。
#[test]
fn tun_route_gate_never_applies_where_exit_probing_is_blind() {
    // 2026-09-06 K13 补 `Platform::Ios`：生产侧已有独立臂（`route_replan.rs` 的
    // `Platform::Ios => false`），本圈是手写子集、加变体不会自曝。
    for platform in [Platform::Android, Platform::Other, Platform::Ios] {
        for mode in [
            ProxyModeType::SystemProxy,
            ProxyModeType::Tun,
            ProxyModeType::Manual,
        ] {
            assert!(
                !tun_route_gate_applies(mode, platform),
                "{platform:?} + 存盘 {mode:?}：该平台读不出逐目的出口，闸没有取材面，\
                 挂上去只会在每次起核上白付一个 grace 窗口"
            );
        }
    }
    // 反向对照：同一个 `Tun` 档在桌面上必须仍然挂闸，否则上面那条「全不挂」可能只是谓词整个坏了。
    assert!(tun_route_gate_applies(ProxyModeType::Tun, Platform::Mac));
}

/// 接线门：起核前后那两条腿真的经 [`tun_route_gate_applies`] 分流，而不是各写一份 `is_tun()`。
///
/// 上一条只证明**谓词**答对了；谓词答对而生产不调它，Android 上照样白等 3.5s ——
/// 「机制有门、接线没门」在这一处的形态。故两条成对交。
#[test]
fn tun_route_capture_and_verify_both_go_through_the_gate() {
    let source = module_code("runtime/proxy");
    for signature in [
        "    pub(super) async fn capture_tun_route_baseline(",
        "    pub(super) async fn verify_tun_route_captured(",
    ] {
        let body = method_body(&source, signature);
        assert!(
            body.contains("tun_route_gate_applies(mode, self.helper.platform())"),
            "`{signature}` 必须经 `tun_route_gate_applies(mode, self.helper.platform())` 分流。\
             实得方法体：\n{body}"
        );
    }
}

/// 🔴 **守卫①**：非 TUN 模式一律不 flush。
///
/// systemProxy / manual 的旧连接多在 sing-box 连接表之外，无差别 RST 够不着它们、只会误伤
/// 已经过代理的连接。**其余前置全部满足**（核在跑、世代未变）—— 唯一变量就是模式，
/// 否则测的是「别的守卫恰好也拦了」。
///
/// **变异锁**：删掉 `if !mode.is_tun()` 早退 → 两个模式都走到建连腿 → 本测转红。
#[tokio::test]
async fn flush_skips_every_non_tun_mode() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    for mode in [ProxyModeType::SystemProxy, ProxyModeType::Manual] {
        assert_eq!(
            rt.flush_connections_once(mode, my_gen, 1).await,
            FlushOutcome::SkippedNotTun,
            "{mode:?} 模式绝不允许 flush：够不着表外的旧连接，只会误伤已代理的连接"
        );
    }
}

/// 🔴 **守卫②·世代**：延迟窗口内被 stop / 重启接管 → 放弃，不得打到已换的核。
///
/// **变异锁**：删掉世代比对 → 落到建连腿（非 Skipped*）→ 本测转红。
#[tokio::test]
async fn flush_skips_when_generation_superseded() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    rt.bump_generation(); // 等价于窗口内来了一次 stop / restart
    assert_eq!(
        rt.flush_connections_once(ProxyModeType::Tun, my_gen, 1)
            .await,
        FlushOutcome::SkippedSuperseded,
        "世代已被接管仍开枪 = 把新核刚建立的连接全 RST 掉"
    );
}

/// 🔴 **守卫②·核在跑**：核已停 → 无连接表可 flush。
///
/// **变异锁**：删掉 `status().running` 判定 → 落到建连腿（非 Skipped*）→ 本测转红。
#[tokio::test]
async fn flush_skips_when_core_stopped() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    *rt.status.write().unwrap() = ProxyStatus::default(); // running:false
    assert_eq!(
        rt.flush_connections_once(ProxyModeType::Tun, my_gen, 1)
            .await,
        FlushOutcome::SkippedCoreStopped,
        "核已停不该再去连管理 API"
    );
}

/// 🔴 **守卫全过 ⇒ 真的走到管理 API**（不是「三条跳过腿都绿」的假闭环）。
///
/// 没有活核，所以断言只到「**不是**任何一条跳过腿」：说明两条守卫都放行、代码真的去开枪了。
/// 端口取一个刚释放的空闲口（纯回环、无监听，不碰宿主网络），故必然落 `ConnectFailed`
/// 或 `CallFailed` —— 具体哪个取决于 tonic 建连是否惰性，不该由本测钉死。
/// 真的把连接 RST 掉需要活核 + 抓包，属真机门。
#[tokio::test]
async fn flush_reaches_management_api_when_both_guards_pass() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    let dead_port = free_port(); // 监听已 drop ⇒ 无人接
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        rt.flush_connections_once(ProxyModeType::Tun, my_gen, dead_port),
    )
    .await
    .expect("flush 腿必须自行了结，不得挂死在建连上");
    assert!(
        matches!(
            outcome,
            FlushOutcome::ConnectFailed(_) | FlushOutcome::CallFailed(_)
        ),
        "两条守卫都放行时必须真的走到管理 API，实得 {outcome:?}"
    );
}

/// 🔴 **接线守卫**：flush 必须在 `running:true` 落定之后才可能开枪。
///
/// 顺序不是洁癖：守卫②查的就是 `status().running`，排在状态提交之前会让每次起核都落
/// `SkippedCoreStopped` —— 腿在、恒不开枪，而上面四条单测照样全绿（它们直调决策点，不经起核腿）。
///
/// **判据现在是间接的**：flush 已挪进 selector 校正的续延（上游「时序修 E」，理由见
/// [`after_selector_reasserted`](ProxyRuntime::after_selector_reasserted)），起核腿里只剩
/// **spawn** 那一行。于是这条不变式改由「spawn 点晚于状态提交」承担 —— 续延只会更晚，
/// 传递性给出同样的保证，且比原来更强（原来 flush 与提交之间还隔着一整段可被重排的主链）。
///
/// 「恰调一次 flush」那条计数不在这里：它已经被
/// [`selector_reassert_continuation_holds_all_three_deferred_actions`] 与
/// [`start_inner_spawns_reassert_and_defers_unlock_invalidation`] 两侧夹住
/// （续延里恰一次 + 主链上零次）。此处再抄一遍只会在下次搬家时留下第三处要改的地方。
///
/// **变异锁**：把 `spawn_reassert_selector_selection(...)` 挪到 `*g = new_status.clone();` 之前 → 转红。
#[test]
fn connection_flush_is_reachable_only_after_status_commit() {
    let body = method_body(
        &module_code("runtime/proxy"),
        "    pub(super) async fn start_inner(",
    );
    let commit = body
        .find("*g = new_status.clone();")
        .expect("锚点 `*g = new_status.clone();` 消失，顺序守卫已失去判据");
    let spawn = body
        .find("self.spawn_reassert_selector_selection(")
        .expect("校正腿的 spawn 点消失 —— flush 已随它挪进续延，没有 spawn 就没有 flush");
    assert!(
        commit < spawn,
        "校正腿（flush 挂在它的续延上）必须 spawn 在 running:true 提交之后，\
             否则守卫②恒判『核已停』→ 腿在但永不开枪"
    );
}

/// 🔴 **建连之后必须再查一次世代**：建连（await 点）期间被接管 → 放弃，一条都不关。
///
/// 原先是源码守卫（数 `self.gate.generation() != my_gen` 出现两次），因为单测里没有活的管理 API、
/// 建连必失败。建连现经 `connect` 注入，替身可以在「建连成功」的同时 bump 世代，故改为行为断言。
///
/// **变异锁**：删掉建连后的那次世代比对 → 落到 `Flushed` 且替身记录到关闭 → 本测转红。
#[tokio::test]
async fn flush_rechecks_generation_after_connect() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    let api = FakeConnectionApi::with_snapshot(&[("a", 0)]);
    let closed = Arc::clone(&api.closed);
    let outcome = rt
        .flush_connections_with(ProxyModeType::Tun, my_gen, || async {
            rt.bump_generation(); // 建连期间来了一次 stop / restart
            Ok(api)
        })
        .await;
    assert_eq!(
        outcome,
        FlushOutcome::SkippedSuperseded,
        "建连后世代已变仍开枪 = 把新核刚建立的连接关掉"
    );
    assert!(closed.lock().unwrap().is_empty(), "被接管后不得关任何连接");
}

/// 🔴 **逐条关闭活连接，跳过幽灵**（不是 `CloseAllConnections`）。
///
/// `CloseAllConnections` 会连带关掉默认拨号器登记的节点传输 socket（MASQUE QUIC / DoH），见
/// `connection_flush.rs` 模块文档。trait 面上没有 close-all，故走替身「从不调 close-all」是结构性的；
/// 这里钉的是：快照里每条活连接恰好单条关闭一次、`closed_at > 0` 的历史环幽灵被跳过、条数可观测。
///
/// **变异锁**：去掉 `close_live_connections` 的 `closed_at <= 0` 过滤 → 幽灵被关 → 转红。
#[tokio::test]
async fn flush_closes_each_live_connection_individually() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    let api = FakeConnectionApi::with_snapshot(&[("a", 0), ("ghost", 1_000_000_000), ("b", 0)]);
    let closed = Arc::clone(&api.closed);
    let outcome = rt
        .flush_connections_with(ProxyModeType::Tun, my_gen, || async { Ok(api) })
        .await;
    assert_eq!(
        outcome,
        FlushOutcome::Flushed {
            closed: 2,
            failed: 0
        }
    );
    let mut ids = closed.lock().unwrap().clone();
    ids.sort(); // 并发关闭：断言集合不断言次序
    assert_eq!(
        ids,
        vec!["a".to_string(), "b".to_string()],
        "每条活连接恰好关一次，幽灵跳过"
    );
}

/// 🔴 **K6b 起停循环门（下半）**：起核收尾那次「推断绑定事实变了吗」的判据，
/// **不许在某个平台上恒真** —— 恒真 = 每次起核都排一次去抖重启 = 起停循环。
///
/// # 缺陷原文（真机实测，2026-09-04 模拟器 Pixel 6 / x86_64 / Android 16）
///
/// 300 秒窗口内 `sing-box 已 spawn` **69 次**（均 4.35s 一轮），每一轮起核成功后紧跟一句
/// `起核就绪前推断绑定接口事实发生变化 → 调度一次受控重启重新规划`（105 次 spawn 对 104 次告警），
/// 500ms 后去抖 timer 落到 depth=0 → `Proceed` → `restart` → 下一轮。
///
/// 恒真的成因是**两条平台腿在 `Platform::Other` 上同时缺席**（Android 落在 `Other`：
/// `Platform::current()` 只认 macos/windows/linux）：
///  1. `SystemRouteOps::exit_interface_for` 恒 `Ok(None)` ⇒ 规划恒「0 绑定 + 全部候选未决」，
///     而未决非空会让本判据退回**整份**网卡指纹比对；
///  2. `managed_tun_interface_for_network_watcher(_, Other)` 恒 `None`、post-flight 捕获也
///     `不可断言` ⇒ `ignored_interface` 恒 `None`，**我们自己刚建出来的 TUN 剔不掉**。
///
/// 于是那份「整份指纹比对」比的正是「起核前没有 tun0 / 就绪后有 tun0」——一个由本次起核自己造成的
/// 差异，被读成「外部网络事实变了，需要撤 TUN 重新规划」。
///
/// # 这条门为什么在没有真机的 CI 上也说得出话
///
/// 被测的三样全是纯函数：平台能力谓词、指纹比对、以及喂给它的 plan 形态。真机指纹是**抄下来的常量**
/// （`adb shell ip -o addr`，见文档 §取证），不需要设备在场。它守不住的是「Android 上真的没别的腿再
/// 排重启」——那条只有真机能答，收据在文档里。
///
/// 上半（「无路由腿的平台上候选恒空」及其正对照）在
/// `runtime/route_binding/tests/mod.rs::runtime_binding_planning_is_off_where_the_platform_has_no_route_leg`。
#[test]
fn runtime_binding_replan_is_inert_without_a_route_leg() {
    use crate::runtime::route_binding::runtime_binding_planning_supported;

    // 模拟器实测的两份指纹：起核前 / 就绪后**只差一张我们自己建的 tun0**，
    // 且 tun0 的 link-local 每一代都换（故按名字剔除是唯一可行的剔法）。
    let before: InterfaceFingerprint = BTreeMap::from([
        (
            "dummy0".into(),
            (true, vec!["fe80::40df:abff:fe16:515".into()]),
        ),
        (
            "eth0".into(),
            (
                true,
                vec!["10.0.2.15".into(), "fe80::5054:ff:fe12:3456".into()],
            ),
        ),
        ("lo".into(), (true, vec!["127.0.0.1".into(), "::1".into()])),
        (
            "wlan0".into(),
            (
                true,
                vec!["10.0.2.16".into(), "fe80::f1be:d325:e4e1:8915".into()],
            ),
        ),
    ]);
    let mut after = before.clone();
    after.insert(
        "tun0".into(),
        (
            true,
            vec!["172.19.0.1".into(), "fe80::469b:9602:fc09:232e".into()],
        ),
    );

    // 起核收尾那次调用的形参形态（`startup.rs` 里写死 `interface: true`）。
    let starting_impact = NetworkChangeImpact {
        interface: true,
        ..Default::default()
    };

    // 🔴 正对照：判据**确实**会对这份差异喊「变了」。缺陷当天 Android 产出的 plan 形态就是这个
    // （候选 1 / 绑定 0 / 未决 1，日志原文「TUN 逐目的网卡规划：候选 1，特殊路由绑定 0，
    // 原生自动探测 0，降级 1」）。这条若转绿，下面那条 `false` 就只是「判据坏了」，不是「修好了」。
    let plan_before_fix = RuntimeBindingPlan {
        bindings: BTreeMap::new(),
        native_roots: BTreeSet::new(),
        covered_roots: BTreeSet::from(["k4-probe-node".to_string()]),
        probe_ips: BTreeMap::from([("k4-probe-node".into(), "203.0.113.7".parse().unwrap())]),
        unresolved_roots: BTreeMap::from([("k4-probe-node".into(), "203.0.113.7".into())]),
        candidate_count: 1,
    };
    assert!(
        inferred_binding_replan_needed(
            &starting_impact,
            &plan_before_fix,
            Some(&before),
            Some(&after),
            None,
        ),
        "剔除名单为空时，我方 tun0 的出现本身就会被判成『绑定事实变了』——这正是恒真的那一步"
    );

    // 修复后：`Other` 上不再产候选 ⇒ `plan_runtime_bindings` 走 `candidate_count == 0` 早退，
    // Android 能拿到的**只可能**是这份 default plan；同一份指纹差异下判据为假。
    assert!(!runtime_binding_planning_supported(Platform::Other));
    assert!(
        !inferred_binding_replan_needed(
            &starting_impact,
            &RuntimeBindingPlan::default(),
            Some(&before),
            Some(&after),
            None,
        ),
        "无候选 ⇒ 无未决根、无绑定 ⇒ 起核自己建出来的 tun0 不得再被读成外部网络变化"
    );

    // 桌面零回归的可执行断言：同一份 default plan 在**有**路由腿的平台上也从不因 tun0 排重启，
    // 而真正需要重启的形态（绑定的网卡不见了）照旧转真 —— 判据没有被整体削弱。
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(runtime_binding_planning_supported(platform));
    }
    let plan_bound_to_missing_iface = RuntimeBindingPlan {
        bindings: BTreeMap::from([("node-a".to_string(), "en9".to_string())]),
        native_roots: BTreeSet::new(),
        covered_roots: BTreeSet::from(["node-a".to_string()]),
        probe_ips: BTreeMap::new(),
        unresolved_roots: BTreeMap::new(),
        candidate_count: 1,
    };
    assert!(
        inferred_binding_replan_needed(
            &starting_impact,
            &plan_bound_to_missing_iface,
            Some(&before),
            Some(&after),
            None,
        ),
        "被绑定的网卡不在指纹里 ⇒ 仍必须重规划（本批只关掉『没有路由腿的平台』，不放宽这条）"
    );
}

/// 🔴 **单条关闭失败必须在 `Flushed.failed` 上可观测**（日志据此升 warn）。
///
/// **变异锁**：flush 把 `failed` 丢成 0 → 转红。
#[tokio::test]
async fn flush_reports_failed_close_count() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    let api = FakeConnectionApi::with_snapshot(&[("a", 0), ("b", 0)]).rejecting("a");
    let outcome = rt
        .flush_connections_with(ProxyModeType::Tun, my_gen, || async { Ok(api) })
        .await;
    assert_eq!(
        outcome,
        FlushOutcome::Flushed {
            closed: 1,
            failed: 1
        }
    );
}

/// 🔴 **快照超时必须作为独立出口可观测**，不得静默成 `Flushed { closed: 0 }`。
#[tokio::test]
async fn flush_reports_snapshot_timeout() {
    let (rt, _dir, my_gen) = flush_ready_runtime();
    let api = FakeConnectionApi::snapshot_err(ManagementError::SnapshotTimeout);
    let closed = Arc::clone(&api.closed);
    let outcome = rt
        .flush_connections_with(ProxyModeType::Tun, my_gen, || async { Ok(api) })
        .await;
    assert_eq!(outcome, FlushOutcome::SnapshotTimeout);
    assert!(closed.lock().unwrap().is_empty());
}

/// 🔴 **flush 与「关闭全部」的生产代码不得再调 `close_all_connections`**（剥注释后取材）。
///
/// 行为测试走的是 trait 替身，而 trait 面上本就没有 close-all；真正的回退形态是在 gRPC 客户端上
/// 直接调它（本批修掉的正是这种写法），替身看不见 —— 只能由源码守卫兜。
///
/// **变异锁**：在 `flush_connections_once` 或 `connections_close_all` 里改回
/// `client.close_all_connections()` → 转红。
#[test]
fn flush_and_close_all_never_call_close_all_connections() {
    let flush = module_code("runtime/proxy");
    let cmd = crate::test_support::module_code("commands/proxy");
    for (name, code) in [("runtime/proxy", &flush), ("commands/proxy", &cmd)] {
        assert!(
            !code.contains("close_all_connections("),
            "{name} 生产代码调用了 close_all_connections：它会连带关掉节点传输 socket（见 connection_flush.rs）"
        );
        // 正面对照：取材面确实覆盖到两处调用点（取材为空时上面的否定断言恒绿）。
        assert!(
            code.contains("close_live_connections("),
            "{name} 取材面上找不到 close_live_connections 调用点，判据已失去取材"
        );
    }
}

#[test]
fn android_successful_empty_interface_snapshot_is_not_a_bridge_failure() {
    let required = BTreeSet::from(["wlan0".to_owned()]);
    let empty = observed_interface_fingerprint(Ok(Vec::new()), Platform::Android)
        .expect("successful native empty snapshot is authoritative");
    assert_eq!(
        required_interfaces_unavailable(&required, &empty).missing,
        required
    );
    assert!(
        observed_interface_fingerprint(Err("bridge_failure".into()), Platform::Android).is_none()
    );
    assert!(observed_interface_fingerprint(Ok(Vec::new()), Platform::Linux).is_none());
}

#[test]
fn android_interface_snapshot_keeps_real_names_and_availability() {
    let rows = vec![
        crate::commands::system::NetworkInterfaceInfo {
            name: "wlan0".into(),
            display_name: "wlan0".into(),
            is_up: true,
            addresses: vec!["192.0.2.2".into(), "192.0.2.2".into()],
        },
        crate::commands::system::NetworkInterfaceInfo {
            name: "rmnet0".into(),
            display_name: "rmnet0".into(),
            is_up: false,
            addresses: vec![],
        },
    ];
    let observed = observed_interface_fingerprint(Ok(rows), Platform::Android).unwrap();
    assert_eq!(observed["wlan0"], (true, vec!["192.0.2.2".to_owned()]));
    let unavailable = required_interfaces_unavailable(
        &BTreeSet::from([
            "wlan0".to_owned(),
            "rmnet0".to_owned(),
            "missing0".to_owned(),
        ]),
        &observed,
    );
    assert_eq!(unavailable.down, BTreeSet::from(["rmnet0".to_owned()]));
    assert_eq!(unavailable.missing, BTreeSet::from(["missing0".to_owned()]));
}

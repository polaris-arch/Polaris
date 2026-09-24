//! 网络场景 N2 运行时接线（spec §11 N2 行）：非致命信号正反两向、R4 起核兜底、接管事实注入、
//! 「本机解析后的探测源」查询。纯逻辑 + 生成闸门（不带核，不起进程、不碰网络）。

use super::*;
use polaris_config_engine::builder::network_env::{
    PrunedEnvRule, PRUNE_DHCP_MONITOR_MISSING, PRUNE_PROBE_UNAVAILABLE, WARN_DHCP_IPV6_ONLY,
};

fn pruned(id: &str, reason: &'static str) -> PrunedEnvRule {
    PrunedEnvRule {
        rule_id: Some(id.into()),
        reason,
    }
}

/// 验收①正向：报告非空 ⇒ emit 一条 `NETWORK_PROFILE_RULES_PRUNED` 并落码，核仍标 running；
/// 反向：修好（下一次报告为空）⇒ 本码被清掉，且不发新事件。
/// 牙：删掉非空腿的 `set_nonfatal_error` → 正向转红；删掉空腿的 `clear_nonfatal_error_if` → 反向转红。
#[tokio::test]
async fn network_profile_signal_appears_on_prune_and_clears_when_fixed() {
    let (rt, _dir, events) =
        test_runtime_errors_with_clearer(Box::new(EnableRecordingClearer::default()));
    mark_running(&rt);

    rt.warn_network_profile_rules(&[
        pruned("r-dns", PRUNE_PROBE_UNAVAILABLE),
        pruned("r-v6", WARN_DHCP_IPV6_ONLY),
    ]);
    let got = events.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "剪枝必须发一条 proxyError：{got:?}");
    assert_eq!(got[0].1, code::NETWORK_PROFILE_RULES_PRUNED);
    assert!(
        got[0].0.contains("1 条规则本次未生成") && got[0].0.contains("1 条规则可能永不命中"),
        "剔除与告警分开计数：{}",
        got[0].0
    );
    assert!(got[0].0.contains(PRUNE_PROBE_UNAVAILABLE) && got[0].0.contains(WARN_DHCP_IPV6_ONLY));
    assert!(rt.status().running, "核确在跑 → 不得标成未运行");
    assert_eq!(
        rt.status().error_code.as_deref(),
        Some(code::NETWORK_PROFILE_RULES_PRUNED)
    );

    rt.warn_network_profile_rules(&[]);
    assert_eq!(events.lock().unwrap().len(), 1, "修好后不得再发事件");
    assert!(
        rt.status().error_code.is_none(),
        "修好后本码必须清除：{:?}",
        rt.status().error_code
    );
    assert!(rt.status().running);
}

/// 清码只清**本码**：并发产生的其它非终态告警（此处 `RULE_RESOURCES_MISSING`）不被顺手抹掉。
#[tokio::test]
async fn network_profile_clear_leaves_other_nonfatal_codes_alone() {
    let (rt, _dir, _events) =
        test_runtime_errors_with_clearer(Box::new(EnableRecordingClearer::default()));
    mark_running(&rt);
    rt.warn_pruned_rule_resources(&["geosite-cn".to_string()]);
    rt.warn_network_profile_rules(&[]);
    assert_eq!(
        rt.status().error_code.as_deref(),
        Some(code::RULE_RESOURCES_MISSING)
    );
}

/// R4 判据：内核源码字面量 `missing monitor for auto DHCP` 出现在 FATAL/ERROR 行 ⇒ 结构化真因；
/// 同一串出现在 INFO 行不算（别人的日志噪音）。管道腿（逐行）与 helper 日志腿（文本块）同一判据。
/// 牙：删掉 `classify_core_fatal_line` 里的 token 判定 → 前两条转红。
#[test]
fn r4_dhcp_monitor_missing_is_classified_from_injected_fatal_line() {
    let line = "+0800 2026-09-25 12:00:00 FATAL[0000] start service: start dns/dhcp[dns-netenv]: \
                missing monitor for auto DHCP, set route.auto_detect_interface";
    assert_eq!(
        classify_core_fatal_line(line, singbox_line_level(line)),
        Some(CoreFatalKind::DhcpMonitorMissing)
    );
    let helper_log = format!("+0800 INFO network: updated default interface eno1\n{line}\n");
    assert_eq!(
        scan_core_fatal(&helper_log),
        Some(CoreFatalKind::DhcpMonitorMissing)
    );
    let info = "+0800 INFO note: missing monitor for auto DHCP (quoted by someone else)";
    assert_eq!(
        classify_core_fatal_line(info, singbox_line_level(info)),
        None
    );
    // 终态收口不给它专属码（R4 已就地重试过；再失败就是常规起核失败）。
    let (msg, code_out) = settle_start_failure(
        "sing-box 启动期退出".into(),
        Some(CoreFatalKind::DhcpMonitorMissing),
    );
    assert_eq!(
        (msg.as_str(), code_out),
        ("sing-box 启动期退出", code::STARTUP_FAILED)
    );
}

/// R4 兜底只触发一次：首个 `DhcpMonitorMissing` ⇒ 置会话态并要求重试；再来一次 ⇒ 不再重试；
/// 其它真因 / 无真因 ⇒ 不触发。会话态经 `generate_deps` 进生成依赖。
/// 牙：去掉 `swap` 的「已置位就不再重试」→ 第二次断言转红（无限重试）。
#[test]
fn r4_suppression_retries_once_and_flows_into_generate_deps() {
    let (rt, _dir) = test_runtime();
    assert!(!rt.suppress_netenv_after_fatal(None));
    assert!(!rt.suppress_netenv_after_fatal(Some(CoreFatalKind::TunAddressUnavailable)));
    assert!(
        !rt.generate_deps(0, 0, 0, None, &[], &serde_json::json!({}), false)
            .netenv_dhcp_suppressed
    );
    assert!(rt.suppress_netenv_after_fatal(Some(CoreFatalKind::DhcpMonitorMissing)));
    assert!(
        !rt.suppress_netenv_after_fatal(Some(CoreFatalKind::DhcpMonitorMissing)),
        "只重试一次"
    );
    assert!(
        rt.generate_deps(0, 0, 0, None, &[], &serde_json::json!({}), false)
            .netenv_dhcp_suppressed
    );
}

fn tun_netenv_config() -> Value {
    serde_json::json!({
        "servers": [],
        "selectedServerId": "__direct__",
        "proxyMode": "smart",
        "proxyModeType": "tun",
        "mixedPort": 17890,
        "configSchemaVersion": 4,
        "networkProfiles": [{"id": "np", "name": "office", "enabled": true, "probe": "dhcp",
            "match": {"searchDomains": ["corp.example"]}}],
        "dnsRules": [
            {"id": "r-np", "type": "domainSuffix", "values": ["corp.example"], "action": "direct",
             "enabled": true, "networkProfileId": "np",
             "effects": {"dns": {"enabled": true, "resolver": "inherit", "answerMode": "real",
                "action": {"type": "server", "serverId": "builtin-domestic"}}}},
            {"id": "r-d5", "type": "domainSuffix", "values": ["d5.example"], "action": "direct",
             "enabled": true,
             "effects": {"dns": {"enabled": true, "resolver": "inherit", "answerMode": "real",
                "action": {"type": "server", "serverId": "builtin-netenv-dhcp"}}}}
        ],
    })
}

/// R4 端到端（生成闸门，不带核）：会话态置位后重生成 ⇒ `dns-netenv` 与引用它的两条规则都不在
/// 落盘配置里，报告里两条都是 `DHCP_MONITOR_MISSING`（运行时据此发非致命信号）。反向对照：未置位时
/// 同一配置生成 `dns-netenv`、报告为空。只在 Linux/TUN 形态下有意义（dhcp 需特权），故 unix-only。
#[cfg(unix)]
#[tokio::test]
async fn r4_regeneration_drops_netenv_and_reports_it() {
    let dir = fresh_test_dir();
    let rt = test_runtime_in(dir.clone());
    let cfg = tun_netenv_config();
    let user_config: UserConfig = serde_json::from_value(cfg.clone()).unwrap();
    let path = dir.join("r4.json");
    let has_netenv = |gate: &GateOutcome| {
        gate.config
            .dns
            .as_ref()
            .is_some_and(|d| d.servers.iter().any(|s| s.tag == "dns-netenv"))
    };

    let deps = rt.generate_deps(0, 0, 0, None, &[], &cfg, false);
    let before = rt
        .generate_and_gate(&user_config, &deps, &path, None, &mut BTreeMap::new())
        .await
        .unwrap();
    if Platform::parse(platform_tag()) != Platform::Linux {
        return; // 非 Linux 主机：系统代理/TUN 的 dhcp 可用性不同，本用例只钉 Linux TUN 形态。
    }
    assert!(has_netenv(&before), "对照：未剔除时应生成 dns-netenv");
    assert!(
        before.pruned_env_rules.is_empty(),
        "{:?}",
        before.pruned_env_rules
    );

    assert!(rt.suppress_netenv_after_fatal(Some(CoreFatalKind::DhcpMonitorMissing)));
    let deps = rt.generate_deps(0, 0, 0, None, &[], &cfg, false);
    let after = rt
        .generate_and_gate(&user_config, &deps, &path, None, &mut BTreeMap::new())
        .await
        .unwrap();
    assert!(!has_netenv(&after));
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("dns-netenv") && !on_disk.contains("d5.example"));
    assert_eq!(
        after.pruned_env_rules,
        vec![
            pruned("r-np", PRUNE_DHCP_MONITOR_MISSING),
            pruned("r-d5", PRUNE_DHCP_MONITOR_MISSING)
        ]
    );
}

/// 接线守卫：R4 判定在 Dead / Timeout 两条失败腿各接一次、入口复位会话态、就绪后按本次报告发信号、
/// 接管事实由起核腿算好传进 `generate_deps`（不再是 N1 的写死 false）。行为测试够不着
/// （真起核 + 真 FATAL = 真机门）。
#[test]
fn network_profile_runtime_is_wired_into_start_inner() {
    let body = method_body(
        &module_code("runtime/proxy"),
        "    pub(super) async fn start_inner(",
    );
    let compact: String = body.split_whitespace().collect();
    assert_eq!(
        compact
            .matches("self.suppress_netenv_after_fatal(fatal)")
            .count(),
        2,
        "R4 兜底必须在 Dead / Timeout 两条腿各判一次"
    );
    assert!(compact.contains("self.netenv_dhcp_suppressed.store(false,Ordering::SeqCst)"));
    assert!(compact.contains("self.warn_network_profile_rules(&pruned_env_rules)"));
    assert!(
        compact.contains("&config,takeover_active,)"),
        "generate_deps 必须拿到起核腿算的接管事实"
    );
    let deps = method_body(
        &module_code("runtime/proxy"),
        "    ) -> GenerateConfigDeps {",
    );
    assert!(
        !deps.contains("system_dns_takeover_active: false"),
        "接管事实不得再写死 false"
    );
}

/// 接管事实真值表：只有 mac + TUN + 开关未显式关 为真（与 C7 接管门同一口径）。
#[test]
fn system_dns_takeover_active_truth_table() {
    use super::super::dns_takeover::system_dns_takeover_active as f;
    assert!(f(Platform::Mac, true, None));
    assert!(f(Platform::Mac, true, Some(true)));
    assert!(!f(Platform::Mac, true, Some(false)));
    assert!(!f(Platform::Mac, false, None));
    assert!(!f(Platform::Linux, true, None));
    assert!(!f(Platform::Win, true, None));
    let (rt, _dir) = test_runtime();
    assert!(
        rt.generate_deps(0, 0, 0, None, &[], &serde_json::json!({}), true)
            .system_dns_takeover_active,
        "generate_deps 必须原样注入起核腿给的接管事实"
    );
}

/// 查询接口：与生成侧同一判据（Linux 主机上：系统代理 + 显式 dhcp ⇒ 不可用；TUN ⇒ 可用；R4 会话态
/// 置位后 ⇒ `DHCP_MONITOR_MISSING`）。JSON 形状即 IPC 契约。
#[test]
fn resolved_sources_query_uses_generation_facts() {
    if Platform::parse(platform_tag()) != Platform::Linux {
        return;
    }
    let (rt, _dir) = test_runtime();
    let mut cfg = tun_netenv_config();
    cfg["proxyModeType"] = serde_json::json!("systemProxy");
    let got = serde_json::to_value(rt.network_profile_resolved_sources(&cfg).unwrap()).unwrap();
    assert_eq!(
        got,
        serde_json::json!([{"profileId": "np", "probeSource": "dhcp", "available": false,
            "reason": "dhcpNeedsPrivilege"}])
    );
    cfg["proxyModeType"] = serde_json::json!("tun");
    let got = serde_json::to_value(rt.network_profile_resolved_sources(&cfg).unwrap()).unwrap();
    assert_eq!(
        got,
        serde_json::json!([{"profileId": "np", "probeSource": "dhcp", "available": true,
            "reason": null}])
    );
    assert!(rt.suppress_netenv_after_fatal(Some(CoreFatalKind::DhcpMonitorMissing)));
    let got = serde_json::to_value(rt.network_profile_resolved_sources(&cfg).unwrap()).unwrap();
    assert_eq!(got[0]["reason"], "dhcpMonitorMissing");
    assert_eq!(got[0]["available"], false);
}

/// 内置解析器可用性查询：与生成侧同一组本机输入（Linux 系统代理 ⇒ `dhcpNeedsPrivilege`，TUN ⇒ 可用，
/// R4 会话态置位 ⇒ `dhcpMonitorMissing`）。
#[test]
fn builtin_dhcp_status_query_uses_generation_facts() {
    if Platform::parse(platform_tag()) != Platform::Linux {
        return;
    }
    let (rt, _dir) = test_runtime();
    let mut cfg = tun_netenv_config();
    let json = |rt: &ProxyRuntime, cfg: &Value| {
        serde_json::to_value(rt.network_profile_builtin_dhcp_status(cfg).unwrap()).unwrap()
    };
    cfg["proxyModeType"] = serde_json::json!("systemProxy");
    assert_eq!(
        json(&rt, &cfg),
        serde_json::json!({"available": false, "reason": "dhcpNeedsPrivilege"})
    );
    cfg["proxyModeType"] = serde_json::json!("tun");
    assert_eq!(
        json(&rt, &cfg),
        serde_json::json!({"available": true, "reason": null})
    );
    assert!(rt.suppress_netenv_after_fatal(Some(CoreFatalKind::DhcpMonitorMissing)));
    assert_eq!(
        json(&rt, &cfg),
        serde_json::json!({"available": false, "reason": "dhcpMonitorMissing"})
    );
}

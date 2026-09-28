//! `appRoutingEnabled` 三态裁定的回归面：**缺省（None）= 开，只有显式 false 才关**。
//!
//! # 裁定与来由
//!
//! 引擎读这个字段的站点共五处，全部经 `effective_app_rules` 或同形判断门控应用分流：
//!
//! | 站点 | 本文件里的可观测量 |
//! |---|---|
//! | `builder/route.rs`（`build_route_config` 的 `app_rules_eff`） | `route.rules` 里按应用身份命中的应用规则（桌面 `process_name` 两条；Android `package_name` 只 proxy 一条） |
//! | `builder/outbounds.rs`（规则选择器生成） | 出站 `rule-sel-app-<appId>` |
//! | `builder/inbounds.rs`（`effective_app_rules_proxy`） | Android `exclude_package`；非 Linux TUN 的节点 IP 排除 |
//! | `builder/endpoint_routes.rs`（`engaged_rule_targeted_server_ids`） | 返回集含应用规则的目标节点 |
//! | `builder/endpoint_routes.rs`（`referenced_server_ids` 的播种） | 返回集含应用规则的目标节点 |
//!
//! 旧口径是 `== Some(true)`：存量配置没写过这个键时，引擎整块关掉应用分流，而两端界面
//! （`AppPolicyScreen.tsx` / `mobile/screens/rules/RulesScreen.tsx`，都是 `!== false`）显示它开着，
//! 存储层 `store::sanitize` 的头注也写着「读取侧 `!== false` 视为开」。现口径 `!= Some(false)`
//! 与这两侧同侧。全新安装不受影响：`store::default_config()` 显式写 `false`。
//!
//! # 判据形态
//!
//! - 三态逐一量，**缺省与显式 false 分开量**（否则「这条规则本来就进不去」会同时满足两者）。
//! - 每个平台都跑，平台轴取自 [`Platform::ALL`]；平台 → builder 平台串的映射是穷举 `match`，
//!   新增变体编译不过，逼着回答这个平台上 inbounds 那一站能观测到什么。
//! - 正面断言（开时逐字出现的具体值）与反面断言（关时不出现）成对；另有一条与门无关的正对照
//!   （选中节点 IP 恒在排除表里），证明读的那张表不是空的。

use polaris_config_engine::builder::endpoint_routes::{
    engaged_rule_targeted_server_ids, referenced_server_ids,
};
use polaris_config_engine::builder::{generate_sing_box_config, GenerateConfigDeps, Platform};
use polaris_config_engine::user_config::app_config::UserConfig;
use std::collections::BTreeMap;

/// 三态。`None` 表示**键缺席**（存量配置的形态），不是写成 `null`。
const GATE_STATES: [Option<bool>; 3] = [None, Some(true), Some(false)];

const SELECTED_IP: &str = "198.51.100.1";
const TARGET_ID: &str = "s2";
const TARGET_IP: &str = "203.0.113.7";
const PROXY_APP_SELECTOR: &str = "rule-sel-app-custom-proxy";
const DIRECT_PACKAGE: &str = "com.example.directapp";

/// 平台 → `deps.platform` 串（对齐 `process.platform` 口径，`Platform::parse` 的逆）。
fn platform_str(p: Platform) -> &'static str {
    let s = match p {
        Platform::Mac => "darwin",
        Platform::Win => "win32",
        Platform::Linux => "linux",
        Platform::Android => "android",
        Platform::Ios => "ios",
        // 任取一个本仓没有答过题的串；下面的回环断言保证它确实落 `Other`。
        Platform::Other => "freebsd",
    };
    assert_eq!(Platform::parse(s), p, "{s:?} 解析不回 {p:?} —— 映射写歪了");
    s
}

/// inbounds 那一站在各平台上能观测到的东西。
struct InboundsSignal {
    /// Android：`direct` 档应用的包名进 `exclude_package`。
    exclude_package: bool,
    /// TUN 下节点 IP 进 `route_exclude_address`（`deps.platform != "linux"` 那条腿）。
    node_ip_excluded: bool,
}

fn inbounds_signal(p: Platform) -> InboundsSignal {
    match p {
        Platform::Android => InboundsSignal {
            exclude_package: true,
            node_ip_excluded: true,
        },
        Platform::Mac | Platform::Win | Platform::Ios | Platform::Other => InboundsSignal {
            exclude_package: false,
            node_ip_excluded: true,
        },
        // `inbounds.rs` 的节点 IP 排除这条腿门在 `deps.platform != "linux"` ⇒ inbounds 这一站在
        // Linux 上没有可观测量（route / outbounds / endpoint_routes 三站仍然量）。
        Platform::Linux => InboundsSignal {
            exclude_package: false,
            node_ip_excluded: false,
        },
    }
}

/// route 那一站按平台用哪一个应用身份键（与 `builder::route::app_owner_leg` 同答，**手写**不引用它 ——
/// 引用实现就成了自证）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteOwnerKey {
    /// 桌面三端 / iOS / Other：`process_name`，三档都发。
    ProcessName,
    /// Android：`package_name`，只发 proxy / block；direct 由 `exclude_package` 在系统边界兑现。
    PackageName,
}

fn route_owner_key(p: Platform) -> RouteOwnerKey {
    match p {
        Platform::Android => RouteOwnerKey::PackageName,
        Platform::Mac | Platform::Win | Platform::Linux | Platform::Ios | Platform::Other => {
            RouteOwnerKey::ProcessName
        }
    }
}

fn config_with_gate(gate: Option<bool>) -> UserConfig {
    let mut v = serde_json::json!({
        "servers": [
            {"id": "s1", "name": "HK", "protocol": "vless", "address": SELECTED_IP, "port": 443, "uuid": "uuid-1"},
            {"id": TARGET_ID, "name": "JP", "protocol": "vless", "address": TARGET_IP, "port": 443, "uuid": "uuid-2"}
        ],
        "selectedServerId": "s1",
        "proxyMode": "smart",
        "proxyModeType": "tun",
        "appRules": [
            {"appId": "custom-proxy", "action": "proxy", "enabled": true, "targetServerId": TARGET_ID},
            {"appId": "custom-direct", "action": "direct", "enabled": true}
        ],
        "customAppPresets": [
            {"id": "custom-proxy", "name": "P", "emoji": "", "processNames": ["proxyapp"], "packageNames": ["com.example.proxyapp"]},
            {"id": "custom-direct", "name": "D", "emoji": "", "processNames": ["directapp"], "packageNames": [DIRECT_PACKAGE]}
        ]
    });
    if let Some(g) = gate {
        v["appRoutingEnabled"] = serde_json::json!(g);
    }
    // 键缺席必须真的是缺席（不是 null），否则量的就不是存量配置的形态。
    assert_eq!(v.get("appRoutingEnabled").is_some(), gate.is_some());
    let cfg: UserConfig = serde_json::from_value(v).expect("config 反序列化失败");
    assert_eq!(cfg.app_routing_enabled, gate);
    cfg
}

fn deps(platform: &str) -> GenerateConfigDeps {
    GenerateConfigDeps {
        platform: platform.into(),
        arch: "x86_64".into(),
        race_server_port: 0,
        probe_direct_port: None,
        probe_proxy_port: None,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        network_canary_port: None,
        probe_pool_ports: vec![],
        lan_resolver_for_dns: None,
        race_upstream_ips: vec![],
        race_upstream_ports: vec![],
        has_cronet: true,
        cronet_copy_failed: false,
        has_management_api: false,
        privacy_mode: false,
        log_level: polaris_config_engine::user_config::LogLevel::Info,
        disable_log_file: false,
        dashboard_serve_dir: None,
        tailscale_api_port: 0,
        cache_path: "/fake/userData/cache.db".into(),
        log_file_path: Some("/fake/userData/singbox.log".into()),
        runtime_rules_dir: "/fake/userData/rules".into(),
        rule_resources_path: "/fake/userData/rule-resource".into(),
        custom_rules_dir: "/fake/userData/custom-rules".into(),
        tailscale_state_dir_prefix: "/fake/userData/tailscale".into(),
        tailnet_rules_dir: "/fake/userData/tailnet-rules".into(),
        observed_tailnet_addresses: Default::default(),
        is_valid_srs_fn: |p| p.ends_with(".srs"),
        own_lan_cidrs: vec![],
        system_dns_takeover_active: false,
        netenv_dhcp_suppressed: false,
        log: |_, _| {},
        on_degraded: || {},
    }
}

fn str_list(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `route.rules` 里 `<key> == [name]` 的那条规则的 outbound（`key` = `process_name` / `package_name`）。
fn app_route_outbound(out: &serde_json::Value, key: &str, name: &str) -> Option<String> {
    let hits: Vec<&serde_json::Value> = out["route"]["rules"]
        .as_array()
        .expect("route.rules 不是数组")
        .iter()
        .filter(|r| str_list(&r[key]) == [name])
        .collect();
    assert!(hits.len() <= 1, "{key}={name} 命中 {} 条规则", hits.len());
    hits.first()
        .map(|r| r["outbound"].as_str().unwrap_or("<无 outbound>").to_owned())
}

#[test]
fn generated_config_follows_the_tristate_gate_on_every_platform() {
    let mut runs = 0usize;
    for gate in GATE_STATES {
        let on = gate != Some(false);
        let cfg = config_with_gate(gate);
        for p in Platform::ALL.iter().copied() {
            let plat = platform_str(p);
            let out = serde_json::to_value(
                generate_sing_box_config(&cfg, &BTreeMap::new(), &deps(plat))
                    .unwrap_or_else(|e| panic!("[{p:?} gate={gate:?}] 生成失败: {e}")),
            )
            .unwrap();
            let ctx = format!("[{p:?}/{plat} appRoutingEnabled={gate:?}]");

            // ── outbounds.rs 站：规则选择器出站。先于 route 站量：选择器缺席时 route 那条应用规则
            // 也会跟着没了（引用不到出站），先量 route 会把 outbounds 站的回归报成 route 站。
            let tags: Vec<&str> = out["outbounds"]
                .as_array()
                .expect("outbounds 不是数组")
                .iter()
                .filter_map(|o| o["tag"].as_str())
                .collect();
            assert!(
                tags.contains(&"proxy-selector"),
                "{ctx} 正对照：proxy-selector 不在 —— 读错了表"
            );
            assert_eq!(
                tags.contains(&PROXY_APP_SELECTOR),
                on,
                "{ctx} outbounds.rs 站：{PROXY_APP_SELECTOR} 出站，实得 {tags:?}"
            );

            // ── route.rs 站：按应用身份命中的应用规则（键随平台，见 `route_owner_key`）。
            match route_owner_key(p) {
                RouteOwnerKey::ProcessName => {
                    let (want_proxy, want_direct) = if on {
                        (
                            Some(PROXY_APP_SELECTOR.to_owned()),
                            Some("direct".to_owned()),
                        )
                    } else {
                        (None, None)
                    };
                    assert_eq!(
                        app_route_outbound(&out, "process_name", "proxyapp"),
                        want_proxy,
                        "{ctx} route.rs 站：proxy 档应用规则"
                    );
                    assert_eq!(
                        app_route_outbound(&out, "process_name", "directapp"),
                        want_direct,
                        "{ctx} route.rs 站：direct 档应用规则"
                    );
                }
                RouteOwnerKey::PackageName => {
                    assert_eq!(
                        app_route_outbound(&out, "package_name", "com.example.proxyapp"),
                        on.then(|| PROXY_APP_SELECTOR.to_owned()),
                        "{ctx} route.rs 站：proxy 档应用规则（package_name）"
                    );
                    // direct 档恒不进 route（由下面 inbounds 站的 exclude_package 兑现）。
                    assert_eq!(
                        app_route_outbound(&out, "package_name", DIRECT_PACKAGE),
                        None,
                        "{ctx} route.rs 站：direct 档不该发 package_name 规则"
                    );
                    // Android 上 process_name 应用规则无意义，一条都不许有。
                    assert_eq!(
                        app_route_outbound(&out, "process_name", "proxyapp"),
                        None,
                        "{ctx}"
                    );
                    assert_eq!(
                        app_route_outbound(&out, "process_name", "directapp"),
                        None,
                        "{ctx}"
                    );
                }
            }

            // ── inbounds.rs 站。
            let tun = out["inbounds"]
                .as_array()
                .expect("inbounds 不是数组")
                .iter()
                .find(|i| i["tag"] == "tun-in")
                .unwrap_or_else(|| {
                    panic!("{ctx} 没有 tun-in —— 夹具前提（proxyModeType=tun）塌了")
                });
            let sig = inbounds_signal(p);
            let pkgs = str_list(&tun["exclude_package"]);
            if sig.exclude_package {
                let want: Vec<String> = if on {
                    vec![DIRECT_PACKAGE.into()]
                } else {
                    vec![]
                };
                assert_eq!(pkgs, want, "{ctx} inbounds.rs 站：exclude_package");
            } else {
                assert!(
                    pkgs.is_empty(),
                    "{ctx} 本平台不该发 exclude_package，实得 {pkgs:?}"
                );
            }
            let rea = str_list(&tun["route_exclude_address"]);
            if sig.node_ip_excluded {
                // 正对照：选中节点的 IP 与门无关、恒在 ⇒ 下面那条「目标节点 IP 不在」有信息量。
                assert!(
                    rea.contains(&format!("{SELECTED_IP}/32")),
                    "{ctx} 正对照：选中节点 IP 不在排除表 —— 本平台节点 IP 排除这条腿已不存在，inbounds_signal 要重答"
                );
                assert_eq!(
                    rea.contains(&format!("{TARGET_IP}/32")),
                    on,
                    "{ctx} inbounds.rs 站：应用规则目标节点 IP 排除，实得 {rea:?}"
                );
            } else {
                assert!(
                    !rea.iter()
                        .any(|c| c.starts_with(SELECTED_IP) || c.starts_with(TARGET_IP)),
                    "{ctx} 本平台不该发派生的节点 IP 排除，实得 {rea:?} —— inbounds_signal 要重答"
                );
            }
            runs += 1;
        }
    }
    // 取材面自检：平台轴或三态轴被清空时上面一条断言都不跑。
    assert!(
        Platform::ALL.len() >= 6,
        "Platform::ALL 只剩 {} 个",
        Platform::ALL.len()
    );
    assert!(Platform::ALL.contains(&Platform::Android) && Platform::ALL.contains(&Platform::Linux));
    assert_eq!(runs, GATE_STATES.len() * Platform::ALL.len());
}

/// `endpoint_routes.rs` 两站：与平台无关（入参只有 `UserConfig`）。
#[test]
fn engaged_and_referenced_sets_follow_the_tristate_gate() {
    for gate in GATE_STATES {
        let on = gate != Some(false);
        let cfg = config_with_gate(gate);

        let engaged = engaged_rule_targeted_server_ids(&cfg);
        assert_eq!(
            engaged.contains(TARGET_ID),
            on,
            "[appRoutingEnabled={gate:?}] endpoint_routes.rs 站（engaged_rule_targeted_server_ids）：实得 {engaged:?}"
        );

        let referenced = referenced_server_ids(&cfg);
        // 正对照：选中节点恒被引用。
        assert!(
            referenced.contains("s1"),
            "[{gate:?}] 选中节点不在引用集: {referenced:?}"
        );
        assert_eq!(
            referenced.contains(TARGET_ID),
            on,
            "[appRoutingEnabled={gate:?}] endpoint_routes.rs 站（referenced_server_ids 播种）：实得 {referenced:?}"
        );
    }
}

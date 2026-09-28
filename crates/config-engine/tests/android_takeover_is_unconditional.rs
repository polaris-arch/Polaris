//! Android 上 `proxy_mode_type` 的三个档位必须生成**逐字节相同**的整份 sing-box 配置。
//!
//! # 为什么要在整份配置这一层再守一次（`golden_inbounds_android.rs` 已经守了入站）
//!
//! Android 上「接管方式」这个选择不存在（判据全在 `ProxyModeType::effective_on` 的文档注释里），
//! 而 `proxy_mode_type` 的**缺省值是 `systemProxy`** —— 全新安装、备份恢复、手工改过 config 的
//! 客户端拿到的都是它。于是**每一处**按 `proxy_mode_type` 分流的生成侧判据都是一个潜在的
//! 「Android 走错分支」，而它们分布在四个 builder 里：
//!
//! | 判据 | Android 照存量值分流的后果 |
//! |---|---|
//! | `builder::inbounds` 的 tun 入站 | 一个用户流量入站都没有 ⇒ 隧道建起来但流量没出口，且不报错 |
//! | `builder::route` 的 `auto_detect_interface` | 核出站拿不到 `VpnService.protect()` ⇒ 回灌 TUN 死循环 |
//! | `builder::log` 的 `output` | 一条核日志都不落盘 ⇒ 导出诊断里核日志是空的 |
//! | `builder::dns` 的 INV-1（节点解析器） | `system` 档退回 `dns-local` ⇒ 防递归那条腿关掉 |
//!
//! 入站那道门只看得见第一行。**本门的判据不枚举上表**：它要求整份配置逐字节相同，于是
//! 「日后有人新加一处读 `proxy_mode_type` 的判据、又忘了 Android」这件事会自动变红 ——
//! 那正是上表四行各自的成因，而逐条列举的门只挡得住已经想到的那几条。
//!
//! # 判据里的正面断言（只比"三份相同"会被"三份同样地空"骗过）
//!
//! 相等只说明三条路一致，不说明它们对。故另有三条正面断言钉住那份配置**确实是能用的那份**：
//! tun 入站在场、`route.auto_detect_interface` 为 `true`、`log.output` 非空。
//!
//! # 反向对照（否则"相等"可能只是因为比较器坏了）
//!
//! 同一组输入在 linux 上，`systemProxy` 与 `tun` 必须**不相等** —— 桌面的三档是真实存在的接管
//! 形态，存量档位在那里仍然算数。这条同时是本批「桌面零回归」的一半。

use std::collections::BTreeMap;

use polaris_config_engine::builder::{generate_sing_box_config, GenerateConfigDeps};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::proxy_mode::ProxyModeType;

const MODES: [ProxyModeType; 3] = [
    ProxyModeType::SystemProxy,
    ProxyModeType::Manual,
    ProxyModeType::Tun,
];

fn deps(platform: &str) -> GenerateConfigDeps {
    GenerateConfigDeps {
        platform: platform.to_owned(),
        arch: "aarch64".into(),
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
        // 本门问的是「Android 上 DNS 接管是不是无条件的」，与组网段结算无关：两条新入参都显式给
        // **空值**（空 map ⟺ 本轮零运行期观测，行为与观测面存在之前逐字节相同）。目录指一个不存在
        // 的路径，免得本门的结果取决于跑它那台机器上恰好有什么文件。
        tailnet_rules_dir: "/nonexistent/polaris-tailnet-rules".into(),
        observed_tailnet_addresses: Default::default(),
        is_valid_srs_fn: |path| path.ends_with(".srs"),
        own_lan_cidrs: vec![],
        system_dns_takeover_active: false,
        netenv_dhcp_suppressed: false,
        log: |_, _| {},
        on_degraded: || {},
    }
}

/// 本门的两份输入。
///
/// # 为什么不是一份
///
/// 「最小配置」把自变量收敛到只剩 `proxy_mode_type`，读起来最干净 —— 但**它到不了某些分支**：
/// `builder::dns` 的 INV-1（节点域名解析器 `system` 档在 TUN 下强制走 `dns-node` 防递归）只在
/// 「有节点、且节点地址是域名、且用户把节点解析器选成 `system`」时才出现在配置里。用最小配置跑，
/// 那一格的变异是**不红**的（实测：把 `builder::dns` 那处的生效值改回裸值，最小配置下本门全绿）。
///
/// 「富配置」就是为那一格准备的取材面。两份都跑，本门才真的覆盖了头注表格里的第四行。
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// 出口选「直连」哨兵、零节点：避开节点校验与协议分支，自变量只剩 `proxy_mode_type`。
    Minimal,
    /// 一个域名地址的节点 + 节点解析器选 `system`：把 INV-1 那条 DNS 规则拉进取材面。
    NodeResolverSystem,
}

fn user_config(shape: Shape, mode: ProxyModeType) -> UserConfig {
    let raw = match shape {
        Shape::Minimal => serde_json::json!({
            "selectedServerId": "__direct__",
            "servers": [],
        }),
        Shape::NodeResolverSystem => serde_json::json!({
            "selectedServerId": "s1",
            "servers": [{
                "id": "s1",
                "name": "HK",
                "protocol": "vless",
                "address": "a.example.com",
                "port": 443,
                "uuid": "uuid-1",
            }],
            "dnsConfig": { "nodeResolverSingle": "system" },
        }),
    };
    let mut config: UserConfig = serde_json::from_value(raw).expect("测试输入反序列化失败");
    config.proxy_mode_type = mode;
    config
}

fn generate(platform: &str, shape: Shape, mode: ProxyModeType) -> serde_json::Value {
    let config = user_config(shape, mode);
    let resolved = BTreeMap::new();
    let cfg = generate_sing_box_config(&config, &resolved, &deps(platform))
        .unwrap_or_else(|e| panic!("[{platform} / {shape:?} / {mode:?}] 生成失败: {e}"));
    serde_json::to_value(&cfg).expect("序列化失败")
}

fn tags(config: &serde_json::Value) -> Vec<&str> {
    config["inbounds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.get("tag").and_then(serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn android_config_is_identical_for_every_stored_proxy_mode_type() {
    for shape in [Shape::Minimal, Shape::NodeResolverSystem] {
        let generated: Vec<(ProxyModeType, serde_json::Value)> = MODES
            .iter()
            .map(|m| (*m, generate("android", shape, *m)))
            .collect();

        // ── 正面断言：那份配置确实是能用的那份 ──────────────────────────
        for (mode, cfg) in &generated {
            assert!(
                tags(cfg).contains(&"tun-in"),
                "Android / {shape:?} + {mode:?}：整份配置里没有 tun 入站 —— 一个用户流量入站都没有，\
                 核跑得起来、界面显示已连接、流量进去没出口，且不报错。实得入站 {:?}",
                tags(cfg)
            );
            assert_eq!(
                cfg["route"]["auto_detect_interface"],
                serde_json::json!(true),
                "Android / {shape:?} + {mode:?}：缺 route.auto_detect_interface —— 它是核出站 socket \
                 拿到 VpnService.protect() 的唯一开关（链路见 builder::route 该字段处），缺席则出站\
                 命中我们刚装上的默认路由、回灌 TUN 死循环"
            );
            assert!(
                cfg["log"]["output"].is_string(),
                "Android / {shape:?} + {mode:?}：核日志不落盘 —— 进程内 libbox 没有子进程管道可接，\
                 落盘是导出诊断拿到核原文的唯一途径"
            );
        }

        // ── 三份逐字节相同 ──────────────────────────────────────────────
        for (mode, cfg) in &generated[1..] {
            assert!(
                cfg == &generated[0].1,
                "Android / {shape:?}：{mode:?} 与 {:?} 生成的配置不一致 —— 那个平台上只有一种接管\
                 形态（`VpnService` 的 tun fd），两份不同的配置里必有一份没人为它答过题。\
                 第一处差异：{}",
                generated[0].0,
                first_diff(&generated[0].1, cfg).unwrap_or_else(|| "(未定位)".into())
            );
        }
    }
}

/// **取材面自检**：富配置确实把 INV-1 那条 DNS 规则拉进了取材面。
///
/// 没有这条，`Shape::NodeResolverSystem` 可能因为输入写错（节点地址不是域名、解析器档位拼错、
/// 上游改了 schema）而根本生成不出那条规则，于是上一条测里那一格**恒真而无信息量** ——
/// 「什么都没发生」型的绿。
#[test]
fn the_rich_shape_actually_reaches_the_node_resolver_rule() {
    let cfg = generate("android", Shape::NodeResolverSystem, ProxyModeType::Tun);
    let rules = cfg["dns"]["rules"].as_array().expect("dns.rules 不是数组");
    let hit = rules.iter().any(|r| {
        r["domain"]
            .as_array()
            .is_some_and(|d| d.iter().any(|x| x == "a.example.com"))
    });
    assert!(
        hit,
        "富配置没有产出节点域名解析规则 —— INV-1 那一格在本门里没有取材面。实得 dns.rules={rules:?}"
    );
}

/// **反向对照**：桌面上 `systemProxy` 与 `tun` 必须生成**不同**的配置。
///
/// 上一条全绿也可能是因为比较器坏了 / 三次生成拿的是同一份缓存。这条喂一对**已知应当不同**的
/// 输入，要求它真的报出不同 —— 同时它也是桌面零回归的一半：存量档位在桌面仍然算数。
#[test]
fn desktop_config_still_differs_between_stored_proxy_mode_types() {
    for platform in ["linux", "darwin", "win32"] {
        let sys = generate(platform, Shape::Minimal, ProxyModeType::SystemProxy);
        let tun = generate(platform, Shape::Minimal, ProxyModeType::Tun);
        assert_ne!(
            sys, tun,
            "{platform}：systemProxy 与 tun 生成了同一份配置 —— 要么桌面被接管成恒 TUN，\
             要么本文件的比较根本报不出差异（那样上一条的『相等』也就没有信息量）"
        );
        assert!(
            !tags(&sys).contains(&"tun-in") && tags(&tun).contains(&"tun-in"),
            "{platform}：systemProxy 不该有 tun 入站、tun 档必须有。实得 {:?} / {:?}",
            tags(&sys),
            tags(&tun)
        );
    }
}

/// 定位第一处差异（仅用于失败信息，判据本身是整体相等）。
fn first_diff(a: &serde_json::Value, b: &serde_json::Value) -> Option<String> {
    fn walk(a: &serde_json::Value, b: &serde_json::Value, path: &str) -> Option<String> {
        match (a, b) {
            (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
                let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
                keys.sort();
                keys.dedup();
                for k in keys {
                    let (va, vb) = (x.get(k), y.get(k));
                    match (va, vb) {
                        (Some(va), Some(vb)) => {
                            if let Some(d) = walk(va, vb, &format!("{path}.{k}")) {
                                return Some(d);
                            }
                        }
                        _ => return Some(format!("{path}.{k}: {va:?} vs {vb:?}")),
                    }
                }
                None
            }
            (serde_json::Value::Array(x), serde_json::Value::Array(y)) => {
                if x.len() != y.len() {
                    return Some(format!("{path}: 长度 {} vs {}", x.len(), y.len()));
                }
                for (i, (va, vb)) in x.iter().zip(y.iter()).enumerate() {
                    if let Some(d) = walk(va, vb, &format!("{path}[{i}]")) {
                        return Some(d);
                    }
                }
                None
            }
            _ if a == b => None,
            _ => Some(format!("{path}: {a} vs {b}")),
        }
    }
    walk(a, b, "$")
}

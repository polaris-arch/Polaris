//! Android 按应用「指定节点 / 阻断」—— route 层 `package_name` 腿的回归面（2026-09-25 A5）。
//!
//! # 守的是什么
//!
//! 此前 Android 上应用策略只有「直连」一档按应用身份生效（tun `exclude_package`），「指定节点 /
//! 阻断」只剩 geosite/geoip 那条域名腿，且 route 里还发着 Android 上无意义的 `process_name` 应用规则。
//! 本批让应用规则的身份键随平台走（`builder::route::app_owner_leg`）：
//!
//! | 平台 | proxy / follow | block | direct |
//! |---|---|---|---|
//! | Android | `package_name` → `rule-sel-app-<id>` | `package_name` → `reject`(`no_drop`) | **不进 route**，由 tun `exclude_package` 兑现 |
//! | 桌面 / iOS / Other | `process_name`（与本批之前逐字节相同，`golden_config_snapshot` 兜） | 同左 | 同左 |
//!
//! # 判据形态
//!
//! - 输入逐字取自**跨语言契约夹具** `fixtures/mobile-app-policy-writes.json`（UI 端
//!   `app-policy-write.test.ts` 读同一份）：缝的两侧被同一批对象夹住。
//! - 期望值**手写字面量**（包名、出站 tag、action），不从实现读、不从夹具的「期望」格读。
//! - 正面（Android 上 `package_name` 规则逐字在场）+ 反面（Android 零 `process_name` 应用规则、
//!   direct 不重复；桌面零 `package_name`）+ 正对照（桌面 `process_name` 规则在场，证明读的表不空）。

mod support;

use std::collections::BTreeMap;

use polaris_config_engine::builder::{generate_sing_box_config, Platform};
use polaris_config_engine::user_config::app_config::UserConfig;
use serde_json::{json, Value};
use support::kernel_gate::outbound_deps_for;

/// 内置 telegram 预设的 Android 包名（`app_rules_preset_data.rs` telegram 行）——手写，不从夹具读。
const TG_PACKAGE: &str = "org.telegram.messenger";
/// 内置 telegram 预设的桌面进程名之一（同一行）。
const TG_PROCESS: &str = "Telegram";
const TG_SELECTOR: &str = "rule-sel-app-telegram";

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mobile-app-policy-writes.json"
    );
    let raw = std::fs::read_to_string(path).expect("读 mobile-app-policy-writes.json");
    serde_json::from_str(&raw).expect("解析 mobile-app-policy-writes.json")
}

fn pick_rule(fx: &Value, pick: &str) -> Value {
    fx["picks"]
        .as_array()
        .expect("picks")
        .iter()
        .find(|p| p["pick"] == pick)
        .unwrap_or_else(|| panic!("夹具缺 {pick} 那一档"))["rule"]
        .clone()
}

/// baseConfig + 两个节点（夹具 baseConfig 零节点；proxy 档的 `targetServerId` 指 `srv-hk-01`，
/// 缺席会被引用修剪链剪掉，量的就不是「指定节点」了）。
fn config_for(fx: &Value, rules: Vec<Value>, block_quic: bool) -> UserConfig {
    let mut v = fx["baseConfig"].clone();
    v["servers"] = json!([
        {"id": "srv-a", "name": "A", "protocol": "vless", "address": "198.51.100.1", "port": 443, "uuid": "uuid-a"},
        {"id": "srv-hk-01", "name": "HK", "protocol": "vless", "address": "203.0.113.7", "port": 443, "uuid": "uuid-hk"}
    ]);
    v["selectedServerId"] = json!("srv-a");
    v["appRules"] = Value::Array(rules);
    if block_quic {
        v["blockQuic"] = json!(true);
    }
    serde_json::from_value(v).expect("config 反序列化")
}

fn generate(cfg: &UserConfig, platform: &str) -> Value {
    let cfg = generate_sing_box_config(cfg, &BTreeMap::new(), &outbound_deps_for(platform))
        .unwrap_or_else(|e| panic!("[{platform}] 生成失败: {e}"));
    serde_json::to_value(cfg).expect("序列化")
}

fn rules(out: &Value) -> &Vec<Value> {
    out["route"]["rules"].as_array().expect("route.rules")
}

fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => vec![],
    }
}

/// 递归：规则树里（含 logical 子规则）有没有任何一处带 `key` 且值含 `needle`。
fn any_rule_has(rules: &[Value], key: &str, needle: &str) -> bool {
    rules.iter().any(|r| {
        str_list(&r[key]).iter().any(|s| s == needle)
            || r["rules"]
                .as_array()
                .is_some_and(|sub| any_rule_has(sub, key, needle))
    })
}

/// 规则树里一共有几个 `package_name` 键（任何值）。
fn package_name_keys(rules: &[Value]) -> usize {
    rules
        .iter()
        .map(|r| {
            usize::from(r.get("package_name").is_some())
                + r["rules"]
                    .as_array()
                    .map_or(0, |sub| package_name_keys(sub))
        })
        .sum()
}

/// 顶层 `package_name == [TG_PACKAGE]` 且非 udp443 配对的那条（配对规则带 `network`）。
fn tg_package_rules(out: &Value) -> Vec<&Value> {
    rules(out)
        .iter()
        .filter(|r| str_list(&r["package_name"]) == [TG_PACKAGE] && r.get("network").is_none())
        .collect()
}

fn tun_exclude_package(out: &Value) -> Vec<String> {
    out["inbounds"]
        .as_array()
        .expect("inbounds")
        .iter()
        .find(|i| i["tag"] == "tun-in")
        .map(|t| str_list(&t["exclude_package"]))
        .unwrap_or_default()
}

/// 期望的 package_name 规则：(action, outbound, no_drop)。
type WantRule = (&'static str, Option<&'static str>, Option<bool>);

/// **本批主判据**：UI 写出来的四档，喂 Android 生成后 route 层逐档的形态。
#[test]
fn android_mobile_picks_emit_package_name_for_proxy_and_block_only() {
    let fx = fixture();
    assert_eq!(
        fx["expectedPackage"], TG_PACKAGE,
        "夹具换了应用 —— 本文件的字面量要跟"
    );

    // (pick, 期望 action, 期望 outbound, 期望 no_drop)；None 行 = 不该有 package_name 规则。
    let table: [(&str, Option<WantRule>); 4] = [
        ("follow", Some(("route", Some(TG_SELECTOR), None))),
        ("proxy", Some(("route", Some(TG_SELECTOR), None))),
        ("block", Some(("reject", None, Some(true)))),
        ("direct", None),
    ];
    let mut emitted = 0usize;
    for (pick, want) in table {
        let out = generate(
            &config_for(&fx, vec![pick_rule(&fx, pick)], false),
            "android",
        );
        let hits = tg_package_rules(&out);
        let ctx = format!("[android/{pick}]");

        // Android 上 process_name 应用规则一条都不许有（核自身那条 `sing-box` 直连与本批无关）。
        assert!(
            !any_rule_has(rules(&out), "process_name", TG_PROCESS),
            "{ctx} Android 上仍发了 process_name 应用规则"
        );
        // 域名 / IP 腿不受本批影响，恒在（正对照：读的这张表不是空的）。
        assert!(
            any_rule_has(rules(&out), "rule_set", "geosite-telegram"),
            "{ctx} geosite 腿不见了 —— 夹具前提塌了"
        );

        match want {
            Some((action, outbound, no_drop)) => {
                emitted += 1;
                assert_eq!(hits.len(), 1, "{ctx} package_name 规则应恰一条：{hits:?}");
                let r = hits[0];
                assert_eq!(r["action"].as_str(), Some(action), "{ctx} action");
                assert_eq!(r["outbound"].as_str(), outbound, "{ctx} outbound");
                assert_eq!(r["no_drop"].as_bool(), no_drop, "{ctx} no_drop");
                // 指定节点 / 阻断都**不**把包排出隧道（排出去 = 直连出网，方向相反）。
                assert!(
                    tun_exclude_package(&out).is_empty(),
                    "{ctx} 非 direct 档进了 exclude_package"
                );
            }
            None => {
                // direct：系统边界那一层在，route 那一层不在 —— 同一档只有一份真值。
                assert_eq!(
                    tun_exclude_package(&out),
                    [TG_PACKAGE],
                    "{ctx} exclude_package"
                );
                assert_eq!(
                    package_name_keys(rules(&out)),
                    0,
                    "{ctx} direct 档不该在 route 里重复发 package_name"
                );
            }
        }
    }
    assert_eq!(emitted, 3, "三档该发 package_name 的档数塌了");
}

/// blockQuic 下「指定节点」要配一条同 matcher 的 udp443 reject（与桌面 process_name 腿同形）。
#[test]
fn android_proxy_pick_pairs_udp443_reject_on_package_name() {
    let fx = fixture();
    let out = generate(
        &config_for(&fx, vec![pick_rule(&fx, "proxy")], true),
        "android",
    );
    let rs = rules(&out);
    let route_idx = rs
        .iter()
        .position(|r| str_list(&r["package_name"]) == [TG_PACKAGE] && r["action"] == "route")
        .expect("proxy 档 package_name 规则缺席");
    let pair = &rs[route_idx - 1];
    assert_eq!(
        str_list(&pair["package_name"]),
        [TG_PACKAGE],
        "配对规则 matcher 不是包名：{pair}"
    );
    assert_eq!(pair["action"], "reject");
    assert_eq!(str_list(&pair["network"]), ["udp"]);

    // 反面：阻断档不配对（它自己就是 reject）。
    let out = generate(
        &config_for(&fx, vec![pick_rule(&fx, "block")], true),
        "android",
    );
    assert_eq!(
        rules(&out)
            .iter()
            .filter(|r| str_list(&r["package_name"]) == [TG_PACKAGE])
            .count(),
        1,
        "阻断档不该配 udp443 reject"
    );
}

/// 平台轴取自 `Platform::ALL`：只有 Android 发 `package_name`；其余平台发 `process_name`，且全文零 `package_name`。
#[test]
fn only_android_emits_package_name_every_other_platform_keeps_process_name() {
    let fx = fixture();
    let cfg = config_for(
        &fx,
        vec![
            pick_rule(&fx, "proxy"),
            json!({"appId": "netflix", "action": "block", "enabled": true}),
        ],
        true,
    );
    let mut checked = 0usize;
    for p in Platform::ALL.iter().copied() {
        // 平台 → builder 平台串。穷举 match：新增变体编译不过，逼着答题。
        let (plat, is_android) = match p {
            Platform::Mac => ("darwin", false),
            Platform::Win => ("win32", false),
            Platform::Linux => ("linux", false),
            Platform::Android => ("android", true),
            Platform::Ios => ("ios", false),
            Platform::Other => ("freebsd", false),
        };
        assert_eq!(Platform::parse(plat), p);
        let out = generate(&cfg, plat);
        let raw = serde_json::to_string(&out["route"]).unwrap();
        if is_android {
            assert_eq!(
                tg_package_rules(&out).len(),
                1,
                "[{plat}] telegram package_name 规则"
            );
            assert!(
                raw.contains("com.netflix.mediaclient"),
                "[{plat}] netflix 阻断包名缺席"
            );
            assert!(
                !any_rule_has(rules(&out), "process_name", TG_PROCESS),
                "[{plat}]"
            );
        } else {
            assert_eq!(
                package_name_keys(rules(&out)),
                0,
                "[{plat}] 非 Android 平台发了 package_name"
            );
            assert!(
                !raw.contains("package_name"),
                "[{plat}] route 里出现 package_name 字样"
            );
            // 桌面不变：两条应用规则**整对象**逐字等于本批之前的形态（手写字面量）。
            // `golden_config_snapshot` 的 appRules 用例是无进程名的自定义预设，**射程不含**
            // process_name 这条腿 ⇒ 桌面零变化的兜底在这里，不在那边。
            let tg_names = json!(["Telegram", "Telegram.exe", "Telegram Desktop"]);
            let tg: Vec<&Value> = rules(&out)
                .iter()
                .filter(|r| r["process_name"] == tg_names)
                .collect();
            assert_eq!(
                tg.len(),
                2,
                "[{plat}] telegram 应是 udp443 配对 + route 两条：{tg:?}"
            );
            assert_eq!(
                tg[1],
                &json!({"process_name": tg_names, "action": "route", "outbound": TG_SELECTOR}),
                "[{plat}] telegram 桌面应用规则形态变了"
            );
            assert_eq!(tg[0]["action"], "reject", "[{plat}] udp443 配对");
            let nf: Vec<&Value> = rules(&out)
                .iter()
                .filter(|r| r["process_name"] == json!(["Netflix", "Netflix.exe"]))
                .collect();
            assert_eq!(
                nf,
                [
                    &json!({"process_name": ["Netflix", "Netflix.exe"], "action": "reject", "no_drop": true})
                ],
                "[{plat}] netflix 桌面阻断规则形态变了"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, Platform::ALL.len());
    assert!(checked >= 6, "平台轴塌了");
}

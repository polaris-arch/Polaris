use serde_json::json;

use super::*;
use crate::user_config::system_proxy_bypass::DEFAULT_BYPASS_LAN;

/// 取 `path`（`.` 分隔）处的值。
fn at<'a>(cfg: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = cfg;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// **正面断言（表的自检）**：`INJECTED_FIELD_PATHS` 里的每一条，在空配置上跑完注入后都必须在场。
///
/// 没有这条，表就退化成一份"声称覆盖了什么"的散文 —— 加了路径却忘了写注入函数，
/// 前端守卫会照着表去管一个根本没被注入的字段，把责任推给前端却无人兑现。
#[test]
fn every_declared_path_is_actually_injected() {
    let mut cfg = json!({});
    ensure_effective_config(&mut cfg);
    for path in INJECTED_FIELD_PATHS {
        assert!(
            at(&cfg, path).is_some(),
            "INJECTED_FIELD_PATHS 声明了 {path:?}，但空配置注入后它仍缺席"
        );
    }
}

/// 反向：注入函数实际补出来的字段，必须都在表里登记。
///
/// 漏登记 = 前端守卫管不到它 = 那个字段的兜底可以悄悄长回来。
#[test]
fn injection_adds_nothing_undeclared() {
    let mut cfg = json!({});
    ensure_effective_config(&mut cfg);
    let top: Vec<String> = cfg
        .as_object()
        .expect("注入后仍应是对象")
        .keys()
        .cloned()
        .collect();
    for key in top {
        assert!(
            INJECTED_FIELD_PATHS.contains(&key.as_str()),
            "注入补出了顶层字段 {key:?} 却没登记进 INJECTED_FIELD_PATHS"
        );
    }
    let tun_keys: Vec<String> = cfg["tunConfig"]
        .as_object()
        .expect("tunConfig 应被注入成对象")
        .keys()
        .cloned()
        .collect();
    for key in tun_keys {
        let path = format!("tunConfig.{key}");
        // `tunConfig` 整体注入带来的是 `TunModeConfig::default()` 的字段（autoRoute/strictRoute），
        // 它们随该对象一起在场，不需要各自登记；只有**独立注入**的键必须登记。
        if ["autoRoute", "strictRoute"].contains(&key.as_str()) {
            continue;
        }
        assert!(
            INJECTED_FIELD_PATHS.contains(&path.as_str()),
            "注入补出了 {path:?} 却没登记进 INJECTED_FIELD_PATHS"
        );
    }
}

/// `tunConfig` 缺席 → 注入值必须**逐字等于** `TunModeConfig::default()` 的序列化形，
/// 而不是另抄一份 `{"autoRoute":true,…}` 字面量。
#[test]
fn tun_config_injection_mirrors_rust_default() {
    let mut cfg = json!({});
    ensure_effective_config(&mut cfg);
    let mut injected = cfg["tunConfig"].clone();
    // 本轮独立注入的键不属 Default 的序列化面，比对前摘掉。
    injected
        .as_object_mut()
        .unwrap()
        .remove("inboundExcludeCidrs");
    assert_eq!(
        injected,
        serde_json::to_value(TunModeConfig::default()).unwrap(),
        "tunConfig 注入值与 TunModeConfig::default() 分叉 —— 那就又成了第二份默认"
    );
    // mtu 必须仍然缺席：缺席 = 自动，写具体数会把当时的默认冻在盘上。
    assert!(
        cfg["tunConfig"].get("mtu").is_none(),
        "mtu 不该被注入（缺席即自动）"
    );
    // stack 必须缺席：TUN stack 已随上游弃用移除，前端拿到的生效形里不得再出现这个概念。
    assert!(
        cfg["tunConfig"].get("stack").is_none(),
        "stack 不该被注入（已无此设置项）：{}",
        cfg["tunConfig"]
    );
}

/// `inboundExcludeCidrs` 的注入值必须等于生成侧真正使用的生效值（`unwrap_or(&[])` ⇒ 空）。
///
/// 这条锁的是"注入 ≠ 发明默认值"：哪天有人想让它默认排 `100.64.0.0/10`，
/// 就必须先改生成侧的生效语义，改不动这一边就红。
#[test]
fn inbound_exclude_injection_mirrors_builder_effective_value() {
    let mut cfg = json!({ "tunConfig": { "autoRoute": true, "strictRoute": true } });
    ensure_effective_config(&mut cfg);
    assert_eq!(
        cfg["tunConfig"]["inboundExcludeCidrs"],
        json!([]),
        "注入值必须是空数组 = builder 的 unwrap_or(&[])"
    );

    // 生成侧的生效值取法（`builder::inbounds` 同款）在反序列化后复算一遍，两者必须一致。
    let tun: TunModeConfig = serde_json::from_value(cfg["tunConfig"].clone()).unwrap();
    let effective: &[String] = tun.inbound_exclude_cidrs.as_deref().unwrap_or(&[]);
    assert!(
        effective.is_empty(),
        "builder 侧生效值与注入值分叉：{effective:?}"
    );
}

/// 用户已有的具体值（含**清空后的 `[]`**）一律原样保留 —— 注入只补缺席，不覆盖意图。
#[test]
fn existing_values_are_never_overwritten() {
    let mut cfg = json!({
        "bypassLANList": ["10.0.0.0/8"],
        "tunConfig": {
            "mtu": 1400,
            "autoRoute": false,
            "strictRoute": false,
            "inboundExcludeCidrs": ["32.0.0.0/24", "fd7a:115c:a1e0::/48"]
        }
    });
    ensure_effective_config(&mut cfg);
    assert_eq!(cfg["bypassLANList"], json!(["10.0.0.0/8"]));
    assert_eq!(cfg["tunConfig"]["mtu"], json!(1400));
    assert_eq!(cfg["tunConfig"]["autoRoute"], json!(false));
    assert_eq!(
        cfg["tunConfig"]["inboundExcludeCidrs"],
        json!(["32.0.0.0/24", "fd7a:115c:a1e0::/48"])
    );

    // 用户清空过的空数组 ≠ 缺席，同样不许被"补"。
    let mut cleared = json!({ "tunConfig": { "inboundExcludeCidrs": [] } });
    ensure_effective_config(&mut cleared);
    assert_eq!(cleared["tunConfig"]["inboundExcludeCidrs"], json!([]));
}

/// 幂等：跑两次与跑一次结果相同（`config:get` 每次都会跑，不能逐次膨胀）。
#[test]
fn injection_is_idempotent() {
    let mut once = json!({});
    ensure_effective_config(&mut once);
    let mut twice = once.clone();
    ensure_effective_config(&mut twice);
    assert_eq!(once, twice);
}

/// 既有 `bypassLANList` 注入语义不得被本模块改动（27 条默认仍在）。
#[test]
fn bypass_lan_injection_still_wired() {
    let mut cfg = json!({});
    ensure_effective_config(&mut cfg);
    assert_eq!(
        cfg["bypassLANList"].as_array().unwrap().len(),
        DEFAULT_BYPASS_LAN.len()
    );
}

/// 畸形输入不 panic：`tunConfig` 是非对象（手改 config.json）时安静跳过，交给 store 校验报错。
#[test]
fn malformed_tun_config_does_not_panic() {
    let mut cfg = json!({ "tunConfig": "not-an-object" });
    ensure_effective_config(&mut cfg);
    assert_eq!(cfg["tunConfig"], json!("not-an-object"));

    let mut not_object = json!(["array-root"]);
    ensure_effective_config(&mut not_object);
    assert_eq!(not_object, json!(["array-root"]));
}

/// 畸形值不被"修复"：注入只补缺席/null。
///
/// 边界是投影不是修复 —— 前端把收到的这份原样回传保存，覆盖畸形值等于悄悄把用户的坏配置
/// 改写成默认再落盘。**用负向对照钉住这条**：把 `inboundExcludeCidrs` 手改成字符串，
/// 注入后必须仍是那个字符串。
#[test]
fn malformed_values_are_passed_through_not_repaired() {
    let mut cfg = json!({ "tunConfig": { "inboundExcludeCidrs": "32.0.0.0/24" } });
    ensure_effective_config(&mut cfg);
    assert_eq!(
        cfg["tunConfig"]["inboundExcludeCidrs"],
        json!("32.0.0.0/24"),
        "畸形值被静默改写 —— 坏配置应当说话，不该被抹平"
    );
}

/// `null` 视同缺席（JSON 里显式写 `null` 与不写这个键，语义上都是"没有值"）。
#[test]
fn explicit_null_is_treated_as_absent() {
    let mut cfg = json!({ "tunConfig": null });
    ensure_effective_config(&mut cfg);
    assert_eq!(cfg["tunConfig"]["autoRoute"], json!(true));
    assert_eq!(cfg["tunConfig"]["inboundExcludeCidrs"], json!([]));

    let mut nested = json!({ "tunConfig": { "strictRoute": false, "inboundExcludeCidrs": null } });
    ensure_effective_config(&mut nested);
    assert_eq!(nested["tunConfig"]["inboundExcludeCidrs"], json!([]));
    assert_eq!(nested["tunConfig"]["strictRoute"], json!(false));
}

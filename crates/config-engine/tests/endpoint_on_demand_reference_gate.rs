//! 🔴 **`on_demand` 送达 endpoint 之后，有没有效果** —— 现有门（`on_demand_reaches_every_endpoint_leg` /
//! `on_demand_decodes_on_every_endpoint_type`）只验「键送达 endpoint」「内核收得下这个键」，
//! 没验「送达之后这个键会不会被内核认」。
//!
//! # 背景（2026-09-11 用真控制面实测确认）
//!
//! sing-box 1.15 的 endpoint `on_demand` 由 `route/reference.go` 的 `ReferenceManager.update()` 驱动：
//! 它收集**全部静态路由规则**命名的 outbound（`collectRuleReferences`，逐条扫 `RouteRule.Outbound`，
//! 含 logical 规则递归），selector 只交出 `Now()`。不在这个引用集里的 on_demand endpoint 才会被
//! `SetKeepIdleConnections(false)` 挂起。**关键坑**：一条静态 `{"action":"route","ip_cidr":[...],
//! "outbound":"<tag>"}` 规则，无论流量是否命中，都算永久引用 —— 被它指到的 endpoint 永远不会被挂起。
//!
//! Polaris 侧的静态 force-route 规则来自 `builder::route` 块 0c（`should_force_route_subnets` gate
//! 的 endpoint 自身网段）：节点 engaged（被选中 / 被规则点名 / `alwaysRouteSubnets` 恒真）时才发。
//!
//! # 本门验什么
//!
//! 对一份带 `on_demand:true` 的 Tailscale 节点配置，钉住两种形态（用真实
//! `generate_sing_box_config` 的产出判，不自己拼规则表）：
//!  - engaged（被选中）⇒ 块 0c 发了一条 `outbound==该节点 tag` 的静态 force-route 规则 ⇒
//!    这条规则本身就让该 endpoint 恒被引用 ⇒ **`on_demand` 配了也不会生效**（这是正确行为，
//!    不是缺陷：force-route 规则本身表达的就是「这个节点的网段要恒可达」，与「按需连接」互斥，
//!    以前者为准）。
//!  - 非 engaged（存在但未被选中、未被任何规则点名、`alwaysRouteSubnets` 关）⇒ 没有任何静态规则
//!    点名它 ⇒ `on_demand` 在这个形态下才有机会生效。
//!
//! # 射程自曝
//!
//! 只判「静态规则是否命名了这个 outbound」这件**可判定的事实**本身，不跑真核、不判「内核真的挂起了
//! 连接」（那需要真机 + 时间推移，归运行时验证，不在生成面门的射程）。

mod support;

use std::collections::BTreeMap;

use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::singbox::RouteRule;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings,
};
use support::kernel_gate::{default_platform, outbound_deps_for};

/// sing-box `collectRuleReferences` 的最小对齐：静态规则里任何 `outbound == tag` 的引用（含
/// logical 子规则递归），无论会不会被流量命中。只做字段扫描，不重算规则集合本身——真值来自
/// `generate_sing_box_config` 的实际产出（下面两个 `#[test]` 都喂真实产出；只有 `detector_*`
/// 那个单测直接构造 `RouteRule` 来证明本函数本身不是恒真/恒假）。
fn rules_reference_outbound(rules: &[RouteRule], tag: &str) -> bool {
    rules.iter().any(|r| {
        r.outbound.as_deref() == Some(tag)
            || r.rules
                .as_deref()
                .is_some_and(|nested| rules_reference_outbound(nested, tag))
    })
}

/// `alwaysRouteSubnets` 显式关闭：这样「engaged」只由 `selected_server_id` 决定，两个场景的唯一
/// 差异就是选没选中 —— 若不关，缺省恒 true 会让两个场景的路由面完全一样，测不出因果。
fn ts_node(id: &str, on_demand: bool) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailscale,
        tailscale_settings: Some(Box::new(TailscaleSettings {
            always_route_subnets: Some(false),
            ..Default::default()
        })),
        on_demand: Some(on_demand),
        ..Default::default()
    }
}

#[test]
fn engaged_tailscale_on_demand_is_permanently_referenced_and_inert() {
    let input = UserConfig {
        servers: vec![ts_node("ts1", true)],
        selected_server_id: Some("ts1".into()),
        ..Default::default()
    };
    let deps = outbound_deps_for(&default_platform());
    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");

    // 正面断言：on_demand 真的抵达了 endpoint（否则下面的「被引用」判断没有讨论前提）。
    let ep = cfg
        .endpoints
        .as_ref()
        .and_then(|eps| eps.iter().find(|e| e.tag == "ts1"))
        .expect("engaged 的 ts1 endpoint 没生成");
    assert_eq!(ep.on_demand, Some(true), "on_demand 没抵达 ts1 endpoint");

    // 正面断言：块 0c 确实为 engaged 的 ts1 发了静态 force-route 规则。
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;
    assert!(
        rules_reference_outbound(rules, "ts1"),
        "ts1 被选中（engaged）却没有任何静态 route 规则指向它 —— force-route 没按预期发射，\
         下面「on_demand 会被这条规则钉住」的结论无从谈起"
    );
    // ⇒ 结论（写清为什么）：sing-box ReferenceManager 按「静态规则命名了这个 outbound」判定引用，
    // 与流量是否真的命中该规则无关。engaged 节点恒有这条 force-route 规则 ⇒ on_demand 永远看不到
    // 「未引用」状态 ⇒ 配了也不会生效。这不是缺陷，是两个特性的优先级关系。
}

#[test]
fn unengaged_tailscale_on_demand_has_no_static_reference() {
    let input = UserConfig {
        servers: vec![ts_node("ts1", true)],
        // "__direct__" 是「未选择真实节点」的哨兵 id（DIRECT_SERVER_ID）：selected_server_id 必须
        // 要么是某个真实节点、要么是这个哨兵，否则 generate 直接 Err("Selected server not found")。
        // 用它而非 None，才是「没有选中任何节点」这个状态在本 builder 里的真实合法表达。
        // 未选中、无规则点名、alwaysRouteSubnets 关 ⇒ 非 engaged。
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let deps = outbound_deps_for(&default_platform());
    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");

    // 正面断言：endpoint 仍然生成了（endpoint 的产出与「是否选中」无关，是两件事；若它压根没生成，
    // 下面「零引用」就是平凡的，不是本门要证明的东西）。
    let ep = cfg
        .endpoints
        .as_ref()
        .and_then(|eps| eps.iter().find(|e| e.tag == "ts1"))
        .expect("非 engaged 的 ts1 endpoint 应仍会生成");
    assert_eq!(ep.on_demand, Some(true), "on_demand 没抵达 ts1 endpoint");

    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;
    assert!(
        !rules_reference_outbound(rules, "ts1"),
        "ts1 未 engaged，却仍被某条静态 route 规则点名 —— on_demand 在这个形态下本该可生效，\
         但会被这条多余的引用永久钉住"
    );
}

/// 反向对照：证明 `rules_reference_outbound` 不是恒真也不是恒假 —— 直接构造合成规则喂给它，
/// 不依赖生成器（生产代码不允许改，只能这样证明判据本身有牙）。
#[test]
fn detector_finds_flat_and_nested_references_and_only_the_named_tag() {
    let flat = vec![RouteRule {
        action: Some("route".into()),
        outbound: Some("ts1".into()),
        ..Default::default()
    }];
    assert!(rules_reference_outbound(&flat, "ts1"));
    assert!(!rules_reference_outbound(&flat, "ts2"));

    let nested = vec![RouteRule {
        type_field: Some("logical".into()),
        mode: Some("and".into()),
        rules: Some(vec![RouteRule {
            outbound: Some("ts1".into()),
            ..Default::default()
        }]),
        ..Default::default()
    }];
    assert!(
        rules_reference_outbound(&nested, "ts1"),
        "logical 子规则里的 outbound 引用没被递归扫到"
    );

    let empty: Vec<RouteRule> = vec![];
    assert!(!rules_reference_outbound(&empty, "ts1"));
}

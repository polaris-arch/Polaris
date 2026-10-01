//! 🔴 **两个 Tailscale 节点同时 engaged，会不会撞出同 `ip_cidr` 指向不同 `outbound` 的死规则。**
//!
//! # 背景
//!
//! `ui/src/domain/endpoint-routes.ts` 的 `tailscaleSlotTaken` 单例槽目前禁止两个 Tailscale 节点
//! 同时存在，所以「两个 TS 都 engaged」这个形态现在从前端构造不出来。但后续批次要放宽那个槽，
//! 本门是放宽前的后端不变式闸门：**不依赖前端限制**，直接用两个 Tailscale `ServerConfig` 调真实
//! `generate_sing_box_config`，钉住「不会撞出死规则」这件事。
//!
//! 两个 Tailscale 节点的 tailnet 段（`TAILNET_CGNAT` / `TAILNET_ULA_V6`，见
//! `builder::endpoint_routes`）是**硬编码常量**，与节点身份无关 —— 若各自的 force-route 规则各发
//! 一遍，就会产出两条 `ip_cidr` 相同、`outbound` 不同的规则：sing-box 按 first-match 语义，先出现
//! 的规则吃掉全部流量，第二条永远匹配不到，是活生生的死规则。
//!
//! # 本门验什么、不验什么
//!
//! 只判「生成产物里存不存在这个死规则形态」，不判「为什么现在不存在」（那属于
//! `crates/config-engine/src/builder/route.rs` 块 0c 里 `claimed_cidrs` 的跨节点去重实现细节，
//! 是生产代码的事，本门不重复它、只问结果）。
//!
//! # 这不是本门的终点：去重不等于无害
//!
//! 下面 `two_fully_overlapping_tailscale_nodes_…` 里会看到 ts2（后声明者）在网段完全重合时**没有
//! 拿到任何 force-route 规则**——这确实防住了本门要抓的死规则形态（同 CIDR、不同 outbound），
//! 但代价是 ts2 的全部网段被静默吸收：**它是活的、engaged 的、用户以为它在工作，却一条流量都不会
//! 被送到它那里，界面上没有任何提示**。这本身是另一种缺陷（静默失效，不是良性去重），由
//! `endpoint_force_route_silent_absorption_gate.rs`（门③）单独钉住「能不能检出 + warn 会不会响」；
//! 本门只管「不会撞出两条一样的活规则」这一件事，不代表 ts2 那种情况是可以接受的。

mod support;

use std::collections::{BTreeMap, BTreeSet};

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_forced_route_cidrs, ObservedTailnetAddresses, TAILNET_CGNAT,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::singbox::RouteRule;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings,
};
use support::kernel_gate::{default_platform, outbound_deps_for};

fn ts_node(id: &str, routes: Vec<String>) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailscale,
        // `alwaysRouteSubnets` 不设 = 缺省 true：两个节点都恒 engaged，不必依赖选中/规则点名——
        // 这正是放宽单例槽后最容易撞见的默认组合（用户装两个 TS 节点，谁都没特意去关这个开关）。
        tailscale_settings: Some(Box::new(TailscaleSettings {
            routes,
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// 在 `rules` 里找任意两个**不同 outbound**（且都属于 `node_tags`）之间共享的 `ip_cidr`。
///
/// 只看 `node_tags` 范围内的 outbound：block 0c 之外还有旁路直连（`direct`）等规则，它们的 CIDR
/// 覆盖面本就比 mesh 网段宽得多、且是既有设计（first-match 顺序上 mesh 规则在前，direct 的重叠只是
/// 冗余不是死规则），与本门要抓的「两个 mesh 节点互相打架」是两件事，不能混进同一个判据里。
/// block 0c（生产代码，见 `route.rs` 918-945）从不产出带 `rules` 嵌套的 logical 规则，故这里不用递归。
fn duplicate_cidr_across_outbounds(
    rules: &[RouteRule],
    node_tags: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for r in rules {
        let (Some(cidrs), Some(outbound)) = (&r.ip_cidr, &r.outbound) else {
            continue;
        };
        if !node_tags.contains(outbound) {
            continue;
        }
        for c in cidrs {
            match owner.get(c) {
                Some(existing) if existing != outbound => {
                    conflicts.push((c.clone(), existing.clone(), outbound.clone()));
                }
                Some(_) => {}
                None => {
                    owner.insert(c.clone(), outbound.clone());
                }
            }
        }
    }
    conflicts
}

#[test]
fn two_fully_overlapping_tailscale_nodes_never_emit_duplicate_force_route_cidr() {
    let ts1 = ts_node("ts1", vec![]);
    let ts2 = ts_node("ts2", vec![]);
    // 判据不猜 cidr 字面量，直接问真实函数：两节点网段完全重合（都只有 tailnet 常量段）。
    // 观测面传空 = 本门原有语义逐字不变（两节点都只有 tailnet 常量段）。签名多一个入参是
    // A-0a 的接线，本门的判据一个字没改。
    let no_observed = ObservedTailnetAddresses::new();
    let expected_ts1_cidrs: BTreeSet<String> = endpoint_forced_route_cidrs(&ts1, &no_observed)
        .into_iter()
        .collect();
    let expected_ts2_cidrs: BTreeSet<String> = endpoint_forced_route_cidrs(&ts2, &no_observed)
        .into_iter()
        .collect();
    assert_eq!(
        expected_ts1_cidrs, expected_ts2_cidrs,
        "本场景的前提是两节点网段完全重合（都没有各自的 advertise routes）——前提塌了，\
         下面「零冲突」就不是在测去重机制，是在测空集"
    );

    let input = UserConfig {
        servers: vec![ts1, ts2],
        // 两者都不选中：alwaysRouteSubnets 缺省 true 已经让它俩恒 engaged。
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let deps = outbound_deps_for(&default_platform());
    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;

    // 正面断言：两个 endpoint 都真的生成了（否则下面的「零冲突」可能只是因为它们压根没进 endpoints）。
    let eps = cfg.endpoints.as_ref().expect("没有 endpoints 段");
    for tag in ["ts1", "ts2"] {
        assert!(eps.iter().any(|e| e.tag == tag), "{tag} endpoint 没生成");
    }

    // 正面断言：ts1（先声明者）确实拿到了 force-route 规则，覆盖 TAILNET_CGNAT —— 证明去重机制
    // 真的跑过了、不是因为两个节点都被剔除在外才凑出「零冲突」。
    let ts1_rule = rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some("ts1") && r.ip_cidr.is_some())
        .expect("先声明的 ts1（engaged）没有发出任何 force-route 规则");
    assert!(
        ts1_rule
            .ip_cidr
            .as_ref()
            .unwrap()
            .iter()
            .any(|c| c == TAILNET_CGNAT),
        "ts1 的 force-route 规则里没有 TAILNET_CGNAT，取材面不对"
    );

    // 正面断言 + 写清事实（不评判好坏）：ts2 网段与 ts1 完全重合，全部被跨节点去重吸收 ⇒ ts2
    // 本轮没有专属 force-route 规则。这确实不是「产出两条同 cidr 规则」那种死规则形态，但也不是
    // 「正确行为」——它是 ts2（一个活着、engaged 的节点）全部路由被静默吞掉、界面毫无提示的另一种
    // 缺陷，由门③（`endpoint_force_route_silent_absorption_gate.rs`）单独钉住是否可检出、
    // warn 是否会响。这里只确认「没有变成两条重复规则」这一件事。
    assert!(
        !rules
            .iter()
            .any(|r| r.outbound.as_deref() == Some("ts2") && r.ip_cidr.is_some()),
        "ts2 在两节点网段完全重合时不该再拿到独立的 force-route 规则（当前去重逻辑下它会被吸收）；\
         如果它有了，说明去重失效，正在朝「两条同 cidr 规则」的死规则形态滑落"
    );

    // 核心断言：ts1/ts2 之间不存在「同 CIDR、不同 outbound」——这正是两节点都发 force-route 时
    // 唯一会产生死规则的形态。
    let conflicts =
        duplicate_cidr_across_outbounds(rules, &["ts1".to_string(), "ts2".to_string()].into());
    assert!(
        conflicts.is_empty(),
        "两个 engaged Tailscale 节点的 force-route 规则出现同 CIDR 指向不同 outbound：{conflicts:?} \
         —— 第二条规则在 sing-box 里必是死规则（first-match，首条吃掉全部流量）"
    );
}

/// 同一场景的变体：ts2 带一段 ts1 没有的专属路由。证明上面那道门不是靠「ts2 永远拿不到规则」
/// 蒙混过关 —— ts2 在有独有网段时确实会拿到属于自己的规则，且那条规则与 ts1 的规则互不重叠。
#[test]
fn two_engaged_tailscale_nodes_with_distinct_routes_each_keep_their_own_cidr() {
    let ts1 = ts_node("ts1", vec![]);
    let ts2 = ts_node("ts2", vec!["10.9.0.0/24".to_string()]);

    let input = UserConfig {
        servers: vec![ts1, ts2],
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let deps = outbound_deps_for(&default_platform());
    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;

    // 正面断言：ts2 这次确实拿到了自己的规则，且只含它独有的那段（tailnet 常量段已被 ts1 吸收）。
    let ts2_rule = rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some("ts2") && r.ip_cidr.is_some())
        .expect("ts2 带独有 advertise route，理应拿到属于自己的 force-route 规则");
    assert_eq!(
        ts2_rule.ip_cidr.as_ref().unwrap(),
        &vec!["10.9.0.0/24".to_string()],
        "ts2 的独有规则形状不对"
    );

    let conflicts =
        duplicate_cidr_across_outbounds(rules, &["ts1".to_string(), "ts2".to_string()].into());
    assert!(
        conflicts.is_empty(),
        "ts1/ts2 各自的独有网段不该有交集，出现了：{conflicts:?}"
    );
}

/// 反向对照：证明 `duplicate_cidr_across_outbounds` 真的会抓到死规则形态 —— 生产代码不让改，
/// 只能直接构造「两条同 cidr、不同 outbound」的合成规则喂给它。
#[test]
fn detector_catches_the_duplicate_cidr_shape() {
    let bad = vec![
        RouteRule {
            action: Some("route".into()),
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("ts1".into()),
            ..Default::default()
        },
        RouteRule {
            action: Some("route".into()),
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("ts2".into()),
            ..Default::default()
        },
    ];
    let node_tags: BTreeSet<String> = ["ts1".to_string(), "ts2".to_string()].into();
    let conflicts = duplicate_cidr_across_outbounds(&bad, &node_tags);
    assert_eq!(
        conflicts,
        vec![(
            TAILNET_CGNAT.to_string(),
            "ts1".to_string(),
            "ts2".to_string()
        )],
        "合成的死规则形态没被抓到，判据本身没有牙"
    );

    // 同 cidr、同 outbound（同一节点自己发了两条规则含同一段，比如用户重复填了 route）不算冲突。
    let same_owner = vec![
        RouteRule {
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("ts1".into()),
            ..Default::default()
        },
        RouteRule {
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("ts1".into()),
            ..Default::default()
        },
    ];
    assert!(duplicate_cidr_across_outbounds(&same_owner, &node_tags).is_empty());

    // 不在 node_tags 范围内的 outbound（比如 direct 的旁路兜底）不算冲突——即便它也提到了同一段。
    let unrelated_outbound = vec![
        RouteRule {
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("ts1".into()),
            ..Default::default()
        },
        RouteRule {
            ip_cidr: Some(vec![TAILNET_CGNAT.to_string()]),
            outbound: Some("direct".into()),
            ..Default::default()
        },
    ];
    assert!(duplicate_cidr_across_outbounds(&unrelated_outbound, &node_tags).is_empty());
}

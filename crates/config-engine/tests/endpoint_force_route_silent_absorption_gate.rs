//! 🔴 **门③：一个 engaged 的 mesh 节点，全部网段被别的节点吸收走，这件事能不能被检出。**
//!
//! # 背景（`endpoint_force_route_duplicate_cidr_gate.rs` 门②实测暴露、当时钉错了方向）
//!
//! 门②证明了两个 engaged Tailscale 节点不会撞出「同 CIDR、不同 outbound」的死规则——
//! `route.rs` 块 0c 的 `claimed_cidrs` 会让后声明者的重叠段被跨节点去重吸收。但去重本身没错，
//! 错的是**这件事发生了却无人知晓**：被吸收干净的节点是活的、engaged 的、用户以为它在工作，
//! 它的 tailnet 流量却一条规则都不会送到它那里。
//!
//! # 本门要分清的两种「零覆盖」
//!
//! - **A（危险，要抓）**：`endpoint_forced_route_cidrs(node)` 本身非空（这个节点原本有段要发），
//!   但生成产物里它一条 `ip_cidr` 规则都没有 —— 全被别的节点抢先声明走了。
//! - **B（正常，不该报）**：`endpoint_forced_route_cidrs(node)` 本来就是空的（比如 WARP：
//!   全隧道 anycast 出口，天生没有「具体段」这回事，见 `crate::warp::is_warp_server`）。
//!
//! 判据必须能分开这两种，对 B 不许误报——否则每个 WARP 节点都会让门常驻红，没人会理它。
//!
//! # 与 warn 通道的关系（已用真实生成 + 日志捕获核实，见下）
//!
//! `route.rs` 块 0c 的 `force_route_conflicts` 计数**按「每个被去重丢弃的 cidr」+1**（`route.rs`
//! 927-938），A 形态发生时该节点的全部 cidr 都被丢弃 ⇒ 该计数结构上必然 > 0 ⇒ warn 必然发出
//! （不是本门这一次运行的巧合，是计数循环的写法决定的）。实测（见下方测试内联的
//! `assert!(warned, …)`）确认这条 warn 在本门复现的 A 形态下确实发出，文案含「重复声明」。
//! 故本门把它锁成不变式：**A 形态出现时 warn 必须出现**——哪天这条 warn 被删或改了口径，
//! 本门先红，不必等到用户真的踩上「节点活着却零路由」再发现。

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_forced_route_cidrs, tailnet_rule_file_base, ObservedTailnetAddresses,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::singbox::RouteRule;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings, WireGuardSettings,
};
use polaris_config_engine::user_config::LogLevel;
use support::kernel_gate::{default_platform, outbound_deps_for};

// `GenerateConfigDeps::log` 是裸函数指针（非闭包），捕不住环境，故用线程局部缓冲 + 裸函数——
// 与生产 `builder/tun_exclusion_preview.rs` 捕获 `InboundsDeps::log` 同一形态（那边已有先例，
// 这里只是在测试里照搬，不碰任何 `src/`）。
thread_local! {
    static CAPTURED: RefCell<Vec<(LogLevel, String)>> = const { RefCell::new(Vec::new()) };
}

fn capture_log(level: LogLevel, message: &str) {
    CAPTURED.with(|c| c.borrow_mut().push((level, message.to_owned())));
}

fn take_captured() -> Vec<(LogLevel, String)> {
    CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn ts_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailscale,
        // alwaysRouteSubnets 不设 = 缺省 true：恒 engaged，不依赖选中/规则点名。
        tailscale_settings: Some(Box::new(TailscaleSettings::default())),
        ..Default::default()
    }
}

/// WARP：全隧道 anycast WireGuard 出口。`endpoint_forced_route_cidrs` 恒空（`endpoint_routes.rs`
/// 56-60 的 `is_warp_server` 分支直接 `return vec![]`），是 B 形态的标准样本。
fn warp_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Wireguard,
        address: "engage.cloudflareclient.com".into(),
        wireguard_settings: Some(Box::new(WireGuardSettings {
            private_key: Some("cHJpdmF0ZWtleQ==".into()),
            peer_public_key: Some("cHVibGlja2V5".into()),
            local_address: vec!["172.16.0.2/32".into()],
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coverage {
    /// engaged 且自身有段要发，产出里也确实有它的 force-route 规则。
    Covered,
    /// 🔴 engaged 且自身有段要发，产出里却一条都没有 —— 被别的节点吸收干净。
    AbsorbedEmpty,
    /// 自身本来就没有段要发（如 WARP），不是缺陷。
    NothingToRoute,
}

/// 判据只问两件真事，不重算生成逻辑：`endpoint_forced_route_cidrs`（这个节点原本该发什么）+
/// 生成产物里它实际发没发（`rules` 来自真实 `generate_sing_box_config`）。
///
/// **A-0b 接线（原先此处硬传空观测）**：`observed` 必须是**本次生成用的那一份**
/// （`deps.observed_tailnet_addresses`），不是又造一个空 map —— 判据与生产用不同输入，本身就是
/// 一种分叉。今天 TS 节点恒有两条官方默认段 ⇒ 传空与传真值同值，这条改动**当下不改变任何
/// 结论**（如实登记：它不是一次缺陷修复，是把判据与生产的输入对齐）；它挡住的是将来
/// —— 默认段被收窄、或观测面扩到别的协议时，「只靠观测地址才有路由」的节点被误判成
/// `NothingToRoute`（B 形态）静默放过。
///
/// `has_rule` 同时认 `rule_set` 腿：A-0b 落盘后，块 0c 对有 tailnet 文件的 TS 节点发的是
/// `{rule_set: <tag>}` 而不是 `ip_cidr`（值在文件里、可热更）。只认 `ip_cidr` 会把那种节点
/// 一律读成「零覆盖」⇒ 本门对**正常工作**的节点转红（假红比不红更糟：它会教人忽略这个门）。
fn force_route_coverage(
    node: &ServerConfig,
    rules: &[RouteRule],
    tag: &str,
    observed: &ObservedTailnetAddresses,
) -> Coverage {
    if endpoint_forced_route_cidrs(node, observed).is_empty() {
        return Coverage::NothingToRoute;
    }
    let has_rule = rules.iter().any(|r| {
        r.outbound.as_deref() == Some(tag) && (r.ip_cidr.is_some() || r.rule_set.is_some())
    });
    if has_rule {
        Coverage::Covered
    } else {
        Coverage::AbsorbedEmpty
    }
}

#[test]
fn engaged_node_fully_absorbed_is_detectable_and_the_conflict_warn_fires() {
    let ts1 = ts_node("ts1");
    let ts2 = ts_node("ts2"); // 网段与 ts1 完全重合（都只有 tailnet 常量段）

    let input = UserConfig {
        servers: vec![ts1.clone(), ts2.clone()],
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let mut deps = outbound_deps_for(&default_platform());
    deps.log = capture_log;
    take_captured(); // 清掉本进程内其它用例可能残留的捕获（thread_local，同线程串行时保险起见清一次）

    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;
    let captured = take_captured();

    // 正面断言①：A 形态在当前代码下真实发生（不是假设）——ts2 engaged、有自己的 forced cidrs，
    // 产出里却零覆盖。
    assert_eq!(
        force_route_coverage(&ts2, rules, "ts2", &deps.observed_tailnet_addresses),
        Coverage::AbsorbedEmpty,
        "本门的前提场景（ts2 被 ts1 完全吸收）没有复现，下面「可检出」的断言就无的放矢"
    );
    // 对照：ts1（先声明者）分类不同，证明 Coverage 三态确实有区分力，不是恒返回同一个值。
    assert_eq!(
        force_route_coverage(&ts1, rules, "ts1", &deps.observed_tailnet_addresses),
        Coverage::Covered
    );

    // 正面断言②：ts2 endpoint 确实存在、活着——用户看得到这个节点，却猜不到它零路由。
    let eps = cfg.endpoints.as_ref().expect("没有 endpoints 段");
    assert!(
        eps.iter().any(|e| e.tag == "ts2"),
        "ts2 endpoint 应仍然生成（它是活的，只是路由被吸收干净）"
    );

    // 核心断言：A 形态出现时，块 0c 的 force_route_conflicts warn 必须发出（含「重复声明」字样）。
    // 实测：本场景下 ts2 的两段（TAILNET_CGNAT / TAILNET_ULA_V6）逐条被判重复声明，warn 文案为
    // 「2 个 endpoint 路由段被多个节点重复声明，已按节点顺序去重（先声明者生效）」。
    let warned = captured
        .iter()
        .any(|(lvl, msg)| *lvl == LogLevel::Warn && msg.contains("重复声明"));
    assert!(
        warned,
        "A 形态（{:?}）出现了，但 force_route_conflicts 的 warn 没有发出 —— 唯一的可观测线索\
         也哑了，节点零路由这件事彻底无人知晓。捕获到的日志：{:?}",
        Coverage::AbsorbedEmpty,
        captured
    );
}

#[test]
fn warp_node_has_nothing_to_route_and_is_not_flagged() {
    let warp = warp_node("warp1");
    let input = UserConfig {
        servers: vec![warp.clone()],
        selected_server_id: Some("warp1".into()),
        ..Default::default()
    };
    let deps = outbound_deps_for(&default_platform());
    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;

    // 正面断言：endpoint 确实生成了（WARP 走的是 catch-all 全隧道，不是被剔除）。
    assert!(
        cfg.endpoints
            .as_ref()
            .expect("没有 endpoints 段")
            .iter()
            .any(|e| e.tag == "warp1"),
        "warp1 endpoint 没生成，下面「不误报」的断言没有讨论前提"
    );

    // 核心断言：B 形态（天生没有具体段）不该被分类成 AbsorbedEmpty。
    assert_eq!(
        force_route_coverage(&warp, rules, "warp1", &deps.observed_tailnet_addresses),
        Coverage::NothingToRoute,
        "WARP 节点被误判成「被吸收」——它本来就没有具体网段要发，判据在这里应该保持沉默"
    );
}

/// 反向对照：直接构造合成规则证明 `force_route_coverage` 三态都有牙，不依赖生成器巧合。
#[test]
fn detector_distinguishes_all_three_states_on_synthetic_input() {
    let engaged_with_route: ServerConfig = ts_node("ts1");
    let engaged_empty: ServerConfig = warp_node("warp1");
    // 合成输入不经生成器，故观测面显式给空（= 这批合成 rules 所对应的那一份）。
    let no_observation = ObservedTailnetAddresses::new();

    let covered_rules = vec![RouteRule {
        action: Some("route".into()),
        ip_cidr: Some(vec!["100.64.0.0/10".into()]),
        outbound: Some("ts1".into()),
        ..Default::default()
    }];
    assert_eq!(
        force_route_coverage(&engaged_with_route, &covered_rules, "ts1", &no_observation),
        Coverage::Covered
    );

    let no_rules_at_all: Vec<RouteRule> = vec![];
    assert_eq!(
        force_route_coverage(
            &engaged_with_route,
            &no_rules_at_all,
            "ts1",
            &no_observation
        ),
        Coverage::AbsorbedEmpty,
        "有段可发、产出里却零覆盖，必须判成 AbsorbedEmpty"
    );

    assert_eq!(
        force_route_coverage(&engaged_empty, &no_rules_at_all, "warp1", &no_observation),
        Coverage::NothingToRoute,
        "本来就没有段要发的节点，不该被判成 AbsorbedEmpty"
    );
}

/// 🔴 **A-0b 回归**：走外化 tailnet rule-set 腿的节点，不得被读成「被吸收干净」。
///
/// A-0b 落盘之后，块 0c 对**有 tailnet 文件**的 TS 节点发的是 `{rule_set: "tailnet-<id>"}`
/// 而不是 `ip_cidr`（段值住在文件里，改了原子替换即热重载，不必重启核）。判据若只认 `ip_cidr`，
/// 这类**正常工作**的节点会被本门判成 A 形态 —— 一个对着正确实现常驻红的门，结局是被人忽略，
/// 于是它原本要抓的那个真缺陷也一起没人看了。
///
/// 本用例同时是「观测面接真值」那条改动的牙：它是本文件里唯一一个 `observed` 非空的场景。
#[test]
fn node_served_by_an_external_tailnet_rule_set_is_not_read_as_absorbed() {
    let tmp = tempfile::TempDir::new().expect("建临时目录");
    let dir = tmp.path().join("tailnet-rules");
    std::fs::create_dir_all(&dir).expect("建 tailnet-rules 目录");
    // 落一个真文件：块 0c 的存在性检查是 `ext_rule_file_exists`（真 existsSync），必须真存在。
    std::fs::write(
        dir.join(format!("{}.json", tailnet_rule_file_base("ts1"))),
        r#"{"version":1,"rules":[{"ip_cidr":["100.64.0.0/10","32.0.0.28/32"]}]}"#,
    )
    .expect("写 tailnet rule-set");

    let node = ts_node("ts1");
    let input = UserConfig {
        servers: vec![node.clone()],
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    };
    let mut deps = outbound_deps_for(&default_platform());
    deps.tailnet_rules_dir = dir.display().to_string();
    deps.observed_tailnet_addresses =
        ObservedTailnetAddresses::from([("ts1".to_string(), vec!["32.0.0.28".to_string()])]);

    let cfg = generate_sing_box_config(&input, &BTreeMap::new(), &deps).expect("生成配置");
    let rules = &cfg.route.as_ref().expect("没有 route 段").rules;

    // 前提：本场景确实走的是 rule_set 腿（否则下面的断言退化成旧路径的重测）。
    assert!(
        rules
            .iter()
            .any(|r| r.outbound.as_deref() == Some("ts1") && r.rule_set.is_some()),
        "前提没建立：产物里没有 rule_set 腿的 force-route 规则 —— 本用例失去讨论对象"
    );
    assert_eq!(
        force_route_coverage(&node, rules, "ts1", &deps.observed_tailnet_addresses),
        Coverage::Covered,
        "走外化 rule-set 的节点被误判成「被吸收干净」—— 判据只认 ip_cidr，看不见另一条发射腿"
    );
}

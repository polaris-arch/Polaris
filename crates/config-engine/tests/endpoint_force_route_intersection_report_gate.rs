//! 🔴 **门④：force-route 冲突的真值是「网段是否相交」，不是「有没有第二个同协议节点」。**
//!
//! # 本门存在的理由
//!
//! `ui/src/domain/endpoint-routes.ts` 的 Tailscale 单例槽给的理由是「同一设备的所有 Tailscale
//! 账号共用同一段网络地址（100.64.0.0/10 tailnet），多个会互相顶掉」。这句话的前提已被实测推翻：
//! 2026-09-11 用真控制面确认，自建 headscale 的 tailnet 实际发的是 `32.0.0.28` / `32.0.0.29`
//! （`prefixes.v4` 可自定义），与官方 `100.64.0.0/10` **不相交** —— 两个不同控制面的账号在地址
//! 空间上根本不打架。那个「共用同一段」的错觉来自 `endpoint_routes.rs` 的硬编码常量。
//!
//! 前端的创建期拦截因此撤掉（创建时节点还没连上控制面，前缀不可知，**创建时判不了相交**）。
//! 判据搬到有真值的地方：生成侧。本门守的就是那份判据的两个方向：
//!
//! - **不相交不报**（最容易做错成「逢两个必报」）：两个 TS 节点各自有互不相交的观测地址 ⇒
//!   两个节点**各自都拿到属于自己的 force-route 覆盖面**，没有任何节点被吃干净。
//! - **相交才报**：段真的撞上时，报告要说得出「谁和谁撞了、撞在哪一段、谁输了、它是否零覆盖」。
//!
//! # 三态复用，不另造一套
//!
//! `ForceRouteCoverage` 的 `Covered` / `AbsorbedEmpty` / `NothingToRoute` 就是 A-5 门③
//! （`endpoint_force_route_silent_absorption_gate.rs`）定下的那三态，生产侧只是把它搬进了
//! 可读结构。本门额外**对拍**：报告给的三态必须与门③那套「从真实生成产物读回来」的判法一致 ——
//! 否则报告就是第二份计算，而那正是本仓反复栽的那个形态。
//!
//! # 那条残留缺陷已在本批修掉，断言原地换方向（不删）
//!
//! 本门上一版如实登记：`endpoint_forced_route_cidrs` 对 Tailscale 恒**并上**官方两段常量
//! （A-0a 的既定语义：观测只追加、默认段一条不删），于是两个 TS 节点即便观测地址互不相交，
//! 那两条默认常量仍然字面量相同 ⇒ 后声明者的默认段被先声明者吸收（`absorbed_count == 2`）。
//! 那条断言写成 `== 2` 的目的就是「默认段与观测段谁优先」这个待决策项**改哪边都会先红**。
//!
//! 2026-09-11 决策落地：**有观测 ⇒ 不发默认段**（见 `endpoint_routes::ObservedTailnetAddresses`
//! 的「# 语义」一节）。于是 `two_disjoint_tailnets_…` 末段那条断言从 `absorbed == 两条默认段`
//! 变成 `absorbed == []`。**锚点保留、只换方向**：写死空向量而不是删掉这条断言，是为了让
//! 「哪天有人把默认段并回来」当场红在同一个位置。
//!
//! # 顺序也是判据（本批新增的一半）
//!
//! sing-box 的 `route.rules` 按规则顺序 **first-match**，不做最长前缀匹配。有观测的节点发 `/32`、
//! 无观测的节点发 `100.64.0.0/10` —— 两者字面不同 ⇒ 跨节点去重一条都拦不下，两条规则都会发出去。
//! 于是「谁排前面」直接决定 `/32` 那条是不是死规则。本门的 `observed_nodes_are_emitted_before_…`
//! 断言**下标**，不断言「两条都在」。

mod support;

use std::collections::BTreeMap;

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_force_route_report, endpoint_forced_route_cidrs, tailnet_rule_file_base, AbsorbedCidr,
    ForceRouteCoverage, ForceRouteLeg, ObservedTailnetAddresses, TAILNET_CGNAT,
    TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6, TAILNET_ULA_V6,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::singbox::RouteRule;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings, WireGuardSettings,
};
use support::kernel_gate::{default_platform, outbound_deps_for};

/// 不存在的 tailnet rule-set 目录 ⇒ 每个 TS 节点都走 inline 腿（= A-0b 落盘腿没跑的常态）。
const NO_TAILNET_DIR: &str = "/nonexistent/polaris-tailnet-rules";

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

/// WARP：全隧道 anycast WireGuard 出口，`endpoint_forced_route_cidrs` 恒空（B 形态标准样本）。
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

fn config_with(servers: Vec<ServerConfig>) -> UserConfig {
    UserConfig {
        servers,
        // 哨兵选中：把「选中谁」这一轴从本门里剔除，否则 engaged 判定会混进另一条理由。
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    }
}

fn observed(pairs: &[(&str, &[&str])]) -> ObservedTailnetAddresses {
    pairs
        .iter()
        .map(|(id, addrs)| {
            (
                (*id).to_string(),
                addrs.iter().map(|a| (*a).to_string()).collect(),
            )
        })
        .collect()
}

fn entry<'a>(
    report: &'a polaris_config_engine::builder::endpoint_routes::EndpointForceRouteReport,
    id: &str,
) -> &'a polaris_config_engine::builder::endpoint_routes::ServerForceRoute {
    report
        .servers
        .iter()
        .find(|s| s.server_id == id)
        .unwrap_or_else(|| panic!("报告里没有 {id} —— 它连 claimant 都没进，断言无的放矢"))
}

/// 门③那套「从真实生成产物读回来」的三态判法，逐字照搬（含 `rule_set` 腿）。本门用它对拍报告。
fn coverage_from_generated(
    node: &ServerConfig,
    rules: &[RouteRule],
    tag: &str,
    obs: &ObservedTailnetAddresses,
) -> ForceRouteCoverage {
    if endpoint_forced_route_cidrs(node, obs).is_empty() {
        return ForceRouteCoverage::NothingToRoute;
    }
    let has_rule = rules.iter().any(|r| {
        r.outbound.as_deref() == Some(tag) && (r.ip_cidr.is_some() || r.rule_set.is_some())
    });
    if has_rule {
        ForceRouteCoverage::Covered
    } else {
        ForceRouteCoverage::AbsorbedEmpty
    }
}

/// 生成一次真配置，返回 route.rules（本门所有「对拍生产」的断言都读它，不读中间态）。
fn generated_rules(config: &UserConfig, obs: &ObservedTailnetAddresses) -> Vec<RouteRule> {
    let mut deps = outbound_deps_for(&default_platform());
    deps.observed_tailnet_addresses = obs.clone();
    deps.tailnet_rules_dir = NO_TAILNET_DIR.to_string();
    let cfg = generate_sing_box_config(config, &BTreeMap::new(), &deps).expect("生成配置");
    cfg.route.expect("没有 route 段").rules
}

/// 正面对照：**一个** TS 节点不产生任何吸收。没有这条，下面「两个节点时报了 N 条」全无参照系
/// （恒报 N 条与「相交才报」在单点上无法区分）。
#[test]
fn a_single_tailscale_node_absorbs_nothing() {
    let config = config_with(vec![ts_node("ts1")]);
    let obs = observed(&[("ts1", &["32.0.0.28"])]);
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);

    assert_eq!(report.absorbed_count, 0);
    assert!(report.zero_coverage_server_ids.is_empty());
    let e = entry(&report, "ts1");
    assert_eq!(e.coverage, ForceRouteCoverage::Covered);
    assert_eq!(e.leg, ForceRouteLeg::Inline);
    assert!(
        e.emitted.contains(&"32.0.0.28/32".to_string()),
        "观测地址没进发射面，本门后面几条对「观测」的断言就都在测别的东西：{:?}",
        e.emitted
    );
}

/// 🔴 **判据 3（本批核心价值）**：两个 TS 节点、观测地址互不相交（一个自建 `32.0.0.x`、
/// 一个官方 `100.64.x.x`）⇒ **没有任何节点被吃干净，两个节点各自都拿到自己的覆盖面**。
///
/// 这是「逢两个必报」最容易被做错的地方：判据若还是看协议，这里会报一个假冲突并把第二个
/// 节点判成零覆盖 —— 而它明明有一条属于自己、谁也没抢走的 `32.0.0.28/32`。
#[test]
fn two_disjoint_tailnets_each_keep_their_own_coverage() {
    // 声明顺序：headscale 节点在前（先占），官方节点在后。
    let config = config_with(vec![ts_node("ts-headscale"), ts_node("ts-official")]);
    let obs = observed(&[
        ("ts-headscale", &["32.0.0.28"]),
        ("ts-official", &["100.64.5.5"]),
    ]);
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);

    // ① 没有节点被吃干净 —— 两个都 Covered。
    assert!(
        report.zero_coverage_server_ids.is_empty(),
        "不相交的两个 tailnet 之间不该出现零覆盖节点，实测：{:?}",
        report.zero_coverage_server_ids
    );
    assert_eq!(
        entry(&report, "ts-headscale").coverage,
        ForceRouteCoverage::Covered
    );
    assert_eq!(
        entry(&report, "ts-official").coverage,
        ForceRouteCoverage::Covered
    );

    // ② 各自那条**独有**的观测段都留在自己名下，一条都没被对方抢走。
    assert!(entry(&report, "ts-headscale")
        .emitted
        .contains(&"32.0.0.28/32".to_string()));
    assert!(entry(&report, "ts-official")
        .emitted
        .contains(&"100.64.5.5/32".to_string()));
    assert!(
        !entry(&report, "ts-official")
            .absorbed
            .iter()
            .any(|a| a.cidr == "100.64.5.5/32"),
        "官方节点自己的观测地址被判成「被吸收」—— 那条段只有它一个人声明过"
    );

    // ③ 对拍真实生成产物：两个节点各自都有一条指向自己的 force-route 规则，且那条规则里
    //    装的是**自己观测到的那个地址**，不是默认段。报告与产物在这一格必须说同一件事 ——
    //    只断言报告，等于只测了纯结算；只断言「有规则」，则默认段被并回来时照样绿。
    let rules = generated_rules(&config, &obs);
    for (id, own) in [
        ("ts-headscale", "32.0.0.28/32"),
        ("ts-official", "100.64.5.5/32"),
    ] {
        let rule = rules
            .iter()
            .find(|r| r.outbound.as_deref() == Some(id) && r.ip_cidr.is_some())
            .unwrap_or_else(|| {
                panic!("{id} 在真实产物里一条 ip_cidr 规则都没有 —— 报告说 Covered 就是假话")
            });
        let cidrs = rule.ip_cidr.as_ref().unwrap();
        assert!(
            cidrs.contains(&own.to_string()),
            "{id} 那条规则里没有它自己观测到的 {own}：{cidrs:?}"
        );
        for def in [TAILNET_CGNAT, TAILNET_ULA_V6] {
            assert!(
                !cidrs.iter().any(|c| c == def),
                "{id} 的产物规则里仍有默认段 {def} —— 两个不相交的 tailnet 会因为它重新抢同一段：{cidrs:?}"
            );
        }
    }

    // ④ **预期 delta（2026-09-11）：从「被吸收 2 条」变成「一条都不吸收」。**
    //
    //    改之前：两个节点的段集都含 `100.64.0.0/10` / `fd7a:115c:a1e0::/48` 这两条**字面相同**
    //    的默认常量 ⇒ 后声明的 ts-official 那两条被 ts-headscale 吸收。那不是误报 —— 先声明者
    //    确实把整个 `100.64.0.0/10` 拿走了，而 ts-official 真就在官方 tailnet 上，它自己的地址
    //    归了别人。这正是本批要修的那个真错路由。
    //
    //    改之后：有观测 ⇒ 不发默认段，两个节点各自只发自己观测到的主机段 ⇒ **tailnet 前缀层面
    //    字面不再相交**。剩下的两条是 MagicDNS 的服务地址（`100.100.100.100` /
    //    `fd7a:115c:a1e0::53`）—— 它们不随控制面变、任何 tailnet 都是这两个，故两个节点都要发，
    //    也因此必然撞车。这是**接受的固有二义**，不是回归：一个地址只能有一个出口，并集时代
    //    同样只能归先声明者（见 `endpoint_routes::TAILNET_MAGICDNS_V4` 的「# 多 tailnet 下的
    //    二义性」一节）。
    //
    //    断言**原地换方向、不删除**，且写死这个精确集合而不是 `len()>=0` 之类：它同时挡住两个
    //    方向 —— 默认段被并回来（会多出 `/10` 与 `/48` 两条）会红，MagicDNS 恒发被摘掉
    //    （会少两条）也会红。
    let official = entry(&report, "ts-official");
    assert_eq!(
        official.absorbed,
        vec![
            AbsorbedCidr {
                cidr: TAILNET_MAGICDNS_V4.into(),
                by_server_id: "ts-headscale".into(),
            },
            AbsorbedCidr {
                cidr: TAILNET_MAGICDNS_V6.into(),
                by_server_id: "ts-headscale".into(),
            },
        ],
        "被吸收的段不是「恰好两条 MagicDNS 服务地址」—— 多出 `100.64.0.0/10` / `fd7a:115c:a1e0::/48` \
         说明默认段被并回来了（两个不相交的 tailnet 会重新抢同一段）；少了说明 MagicDNS 恒发被摘掉了。\
         实际：{:?}",
        official.absorbed
    );
    assert_eq!(
        report.absorbed_count, 2,
        "不相交的两个 tailnet 之间，被吸收的只该是那两条协议常量（MagicDNS），tailnet 前缀一条都不该撞"
    );
    // 正面对照：上面的空集**不是**因为这个节点压根没段可发（那样它会是 NothingToRoute、① 先红），
    // 也不是因为它被吃干净（那样 emitted 为空）。没有这条，`absorbed == []` 在空集上恒真。
    assert!(
        !official.emitted.is_empty(),
        "ts-official 一条段都没发 —— 上面「没有段被吸收」就是在空集上恒真"
    );

    // ⑤ **判据 4**：有观测 ⇒ 两条官方默认常量一条都不发（这是 ④ 那个 0 的成因，不是巧合）。
    for id in ["ts-headscale", "ts-official"] {
        let e = entry(&report, id);
        for def in [TAILNET_CGNAT, TAILNET_ULA_V6] {
            assert!(
                !e.emitted.iter().any(|c| c == def),
                "{id} 有观测却仍发出默认段 {def} —— 语义应是取代而非并集。实际：{:?}",
                e.emitted
            );
        }
    }
}

/// 🔴 **判据 3：混合场景的发射顺序**——有观测的节点的规则，下标必须**严格小于**无观测节点的。
///
/// 这是本批最容易被漏掉的一半：`ts-bootstrap` 没有观测 ⇒ 发 `100.64.0.0/10`；`ts-live` 有观测
/// ⇒ 发 `100.64.5.5/32`。两条**字面不同** ⇒ 跨节点去重（`claimed_cidrs`）一条都拦不下，
/// `absorbed_count` 恒 0、两个节点都 `Covered` —— 报告层面看一切正常。但 sing-box 的
/// `route.rules` 是 first-match：`/10` 若排在前面，`100.64.5.5` 的流量全被它吃走，`ts-live` 那条
/// `/32` 是死规则。
///
/// 故断言**下标**，不断言「两条都在」：后者在顺序反了的世界里照样绿。
/// 声明顺序刻意把无观测的 `ts-bootstrap` 放在**前面** —— 若发射端没排序（照 `config.servers`
/// 原序发），本条当场红。
#[test]
fn observed_nodes_are_emitted_before_bootstrap_nodes() {
    let config = config_with(vec![ts_node("ts-bootstrap"), ts_node("ts-live")]);
    let obs = observed(&[("ts-live", &["100.64.5.5"])]);

    // 前提①：两个节点确实都发了 inline 规则（否则「谁在前」无从谈起）。
    let rules = generated_rules(&config, &obs);
    let idx_of = |tag: &str| {
        rules
            .iter()
            .position(|r| r.outbound.as_deref() == Some(tag) && r.ip_cidr.is_some())
            .unwrap_or_else(|| {
                panic!("{tag} 在产物里没有 inline force-route 规则 —— 本门失去讨论对象")
            })
    };
    let i_live = idx_of("ts-live");
    let i_boot = idx_of("ts-bootstrap");

    // 前提②：两条规则的段确实是「观测 /32」对「默认 /10」这一格（否则测的是别的场景）。
    assert!(
        rules[i_live]
            .ip_cidr
            .as_ref()
            .unwrap()
            .contains(&"100.64.5.5/32".to_string()),
        "ts-live 那条不是观测段：{:?}",
        rules[i_live].ip_cidr
    );
    assert!(
        rules[i_boot]
            .ip_cidr
            .as_ref()
            .unwrap()
            .contains(&TAILNET_CGNAT.to_string()),
        "ts-bootstrap 那条不是默认段：{:?}",
        rules[i_boot].ip_cidr
    );

    assert!(
        i_live < i_boot,
        "有观测的 ts-live（{}）排在无观测的 ts-bootstrap（{}）之后 —— \
         `100.64.0.0/10` 先匹配，ts-live 那条 `/32` 是死规则，而产物逐条看一切正常",
        i_live,
        i_boot
    );

    // 报告与产物必须同序（两处各排各的不会红在任何单侧断言上）。
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);
    let pos = |id: &str| {
        report
            .servers
            .iter()
            .position(|s| s.server_id == id)
            .expect("报告里没有该节点")
    };
    assert!(
        pos("ts-live") < pos("ts-bootstrap"),
        "报告的节点顺序与产物的规则顺序反了 —— 报告说的「谁先占」与内核 first-match 的实际结果对不上"
    );
    assert!(entry(&report, "ts-live").has_observation);
    assert!(!entry(&report, "ts-bootstrap").has_observation);

    // MagicDNS 的两条精确地址骑在**有观测那一批**里（排序键只看节点有无观测、不看段的形态），
    // 于是它们排在 ts-bootstrap 那条 `100.64.0.0/10` 之前 —— 正是精确地址该在的位置。
    // 少了这条，「quad100 恒发」与「发射顺序」这两件事各自绿、合起来仍可能把 quad100 埋在 `/10` 后面。
    for keep in [TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6] {
        assert!(
            rules[i_live]
                .ip_cidr
                .as_ref()
                .unwrap()
                .iter()
                .any(|c| c == keep),
            "MagicDNS 地址 {keep} 不在 ts-live（有观测、排在前面）那条规则里：{:?}",
            rules[i_live].ip_cidr
        );
        assert!(
            !rules[i_boot]
                .ip_cidr
                .as_ref()
                .unwrap()
                .iter()
                .any(|c| c == keep),
            "MagicDNS 地址 {keep} 跑到了 ts-bootstrap（无观测、排在后面）那条规则里 —— \
             bootstrap 腿本不该显式发它，靠 `/10` 的包含关系兜住即可"
        );
    }

    // 反向对照：把观测拿掉 ⇒ 两个节点同为无观测 ⇒ 回落声明顺序（ts-bootstrap 在前）。
    // 没有这条，上面的 `i_live < i_boot` 可能只是「恒按 id 排」之类的巧合。
    let bare =
        endpoint_force_route_report(&config, &ObservedTailnetAddresses::new(), NO_TAILNET_DIR);
    assert_eq!(
        bare.servers
            .iter()
            .map(|s| s.server_id.as_str())
            .collect::<Vec<_>>(),
        vec!["ts-bootstrap", "ts-live"],
        "都没有观测时必须回落 `config.servers` 的声明顺序（稳定排序）"
    );
}

/// 🔴 **判据 4-a**：两个 TS 节点都没有观测（段退回默认两段常量）⇒ 段完全重合 ⇒ 检出，
/// 且说得出输掉的是谁、它零覆盖。
#[test]
fn two_tailscale_nodes_without_observation_collide_and_the_loser_is_named() {
    let config = config_with(vec![ts_node("ts1"), ts_node("ts2")]);
    let obs = ObservedTailnetAddresses::new();
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);

    assert_eq!(
        report.zero_coverage_server_ids,
        vec!["ts2".to_string()],
        "被吃干净的节点没被点名 —— 用户看到的还是一个活着的、却一条流量都收不到的节点"
    );
    let ts2 = entry(&report, "ts2");
    assert_eq!(ts2.coverage, ForceRouteCoverage::AbsorbedEmpty);
    assert!(ts2.emitted.is_empty());
    // 这一位是本场景与「同一个 tailnet 的两个账号」的分界：段重合只是因为**两份一样的猜测**
    // （TS 段集恒含两条硬编码默认常量），不是账号撞车的证据。消费方少了它必然把两者读混。
    assert!(!ts2.has_observation);
    assert!(!entry(&report, "ts1").has_observation);
    assert_eq!(
        ts2.absorbed,
        vec![
            AbsorbedCidr {
                cidr: "100.64.0.0/10".into(),
                by_server_id: "ts1".into(),
            },
            AbsorbedCidr {
                cidr: "fd7a:115c:a1e0::/48".into(),
                by_server_id: "ts1".into(),
            },
        ],
        "说不出「撞在哪一段、被谁抢走」，报告就退化回那个只有计数的 warn"
    );
    assert_eq!(report.absorbed_count, 2);

    // 先声明者分类不同 ⇒ 三态确实有区分力，不是恒返回同一个值。
    assert_eq!(entry(&report, "ts1").coverage, ForceRouteCoverage::Covered);

    // 对拍真实生成产物：ts2 endpoint 活着，却一条 force-route 规则都没有。
    let rules = generated_rules(&config, &obs);
    assert!(
        !rules
            .iter()
            .any(|r| r.outbound.as_deref() == Some("ts2") && r.ip_cidr.is_some()),
        "本门的前提场景（ts2 被 ts1 吃干净）没有复现，上面的断言就无的放矢"
    );
}

/// 🔴 **判据 4-b**：两个节点的观测地址**真的相交**（同一个 tailnet 的两个账号，netmap 互见）
/// ⇒ 同样检出。这正是「至多一个 engaged」唯一真正成立的场景。
///
/// # 本批换掉了这条的取材面（预期 delta，不是放松）
///
/// 上一版给每个账号只喂它**自己**那一个地址，靠「两条默认段字面相同」凑出 `absorbed_count >= 2`
/// —— 也就是说它测到的其实是默认段撞车，不是同一 tailnet。默认段撤掉之后那个凑法自然失效。
///
/// 新取材面按 netmap 的真实形态给：观测值 = self ∪ **全部 peer**（见
/// `endpoint_routes::ObservedTailnetAddresses`），所以同一个 tailnet 上的两个账号会看见彼此
/// 的地址 ⇒ 段集真的相交。两份 netmap 刻意**不完全相同**（Tailscale 的 ACL 可以限制某个节点
/// 看得见哪些 peer），于是既能测出「相交被检出」，也能保住下面那条「相交≠该节点必然全废」。
#[test]
fn two_accounts_on_the_same_tailnet_collide() {
    let config = config_with(vec![ts_node("acct-a"), ts_node("acct-b")]);
    // 同一个官方 tailnet：两份 netmap 在 `100.64.2.2` / `100.64.3.3` 上重叠，各自另有一台
    // 对方看不见的机器（acct-a 独见 1.1，acct-b 独见 4.4）。
    let obs = observed(&[
        ("acct-a", &["100.64.1.1", "100.64.2.2", "100.64.3.3"]),
        ("acct-b", &["100.64.2.2", "100.64.3.3", "100.64.4.4"]),
    ]);
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);

    assert!(
        report.absorbed_count >= 2,
        "同一个 tailnet 的两个账号，netmap 重叠的那两台机器必然撞车，实测 absorbed_count={}",
        report.absorbed_count
    );
    let b = entry(&report, "acct-b");
    // `all()` 在空集上恒真 —— 先用一条正面断言把空集这个假绿堵掉（M2 变异实测：摘掉归属后
    // 只剩 `all()` 的话本条会照常绿）。
    assert!(
        !b.absorbed.is_empty(),
        "同一 tailnet 的后声明者一条被吸收的段都没有 —— 下面「抢占者是谁」的断言在空集上恒真"
    );
    assert!(
        b.absorbed.iter().all(|a| a.by_server_id == "acct-a"),
        "抢占者应恒为先声明的 acct-a：{:?}",
        b.absorbed
    );
    // acct-b 仍有那条只有它看得见的 `100.64.4.4/32` ⇒ **不是**零覆盖。
    // 相交≠该节点必然全废，报告不许把两件事混为一谈。
    assert_eq!(b.coverage, ForceRouteCoverage::Covered);
    assert!(
        b.emitted.contains(&"100.64.4.4/32".to_string()),
        "acct-b 独见的那台机器没留在它名下：{:?}",
        b.emitted
    );
    // 与 4-a 的对照：这里两个节点都**真的观测到了**地址，且都落在官方段里 ⇒ 撞车是真事实，
    // 不是「两个都还没连上控制面」。
    assert!(entry(&report, "acct-a").has_observation);
    assert!(b.has_observation);
}

/// B 形态不误报：WARP 天生没有「具体段」这回事 ⇒ `NothingToRoute`，不进零覆盖名单。
/// 少了这条，每个装了 WARP 的用户都会看到一条假冲突，于是这份报告整体被忽略。
#[test]
fn warp_has_nothing_to_route_and_is_never_flagged() {
    let config = config_with(vec![ts_node("ts1"), warp_node("warp1")]);
    let obs = ObservedTailnetAddresses::new();
    let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);

    assert_eq!(
        entry(&report, "warp1").coverage,
        ForceRouteCoverage::NothingToRoute
    );
    assert!(entry(&report, "warp1").absorbed.is_empty());
    assert!(report.zero_coverage_server_ids.is_empty());
}

/// 外化 tailnet rule-set 腿：段值住在文件里、块 0c 发的是 `{rule_set}` 而非 `ip_cidr` ⇒
/// 该节点既不消耗也不贡献 claim，且**不得**被读成「被吸收干净」（门③的同型回归）。
#[test]
fn external_tailnet_rule_set_leg_is_covered_and_absorbs_nothing() {
    let tmp = tempfile::TempDir::new().expect("建临时目录");
    let dir = tmp.path().join("tailnet-rules");
    std::fs::create_dir_all(&dir).expect("建目录");
    std::fs::write(
        dir.join(format!("{}.json", tailnet_rule_file_base("ts2"))),
        r#"{"version":1,"rules":[{"ip_cidr":["100.64.0.0/10"]}]}"#,
    )
    .expect("写 tailnet rule-set");

    let config = config_with(vec![ts_node("ts1"), ts_node("ts2")]);
    let obs = ObservedTailnetAddresses::new();
    let report = endpoint_force_route_report(&config, &obs, &dir.display().to_string());

    let ts2 = entry(&report, "ts2");
    assert_eq!(ts2.leg, ForceRouteLeg::ExternalRuleSet);
    assert_eq!(
        ts2.coverage,
        ForceRouteCoverage::Covered,
        "走外化 rule-set 的节点被误判成「被吸收干净」—— 判据看不见另一条发射腿"
    );
    assert!(ts2.absorbed.is_empty());
    assert_eq!(
        report.absorbed_count, 0,
        "外化腿的节点不参与字面量去重，计数不该被它撑起来"
    );
    assert!(report.zero_coverage_server_ids.is_empty());
}

/// 报告的三态必须与「从真实生成产物读回来」的三态逐节点相等。
///
/// 这是本门里唯一一条**跨实现**断言：报告是纯结算，产物是 `generate_sing_box_config` 真跑一遍。
/// 两者若分叉，报告就是第二份计算 —— 而那正是 `tun_exclusion_preview` 模块头注记的那次事故形态。
#[test]
fn report_coverage_matches_the_generated_artifact() {
    let cases: Vec<(&str, UserConfig, ObservedTailnetAddresses)> = vec![
        (
            "无观测、两 TS 全重合",
            config_with(vec![ts_node("ts1"), ts_node("ts2")]),
            ObservedTailnetAddresses::new(),
        ),
        (
            "观测不相交",
            config_with(vec![ts_node("ts1"), ts_node("ts2")]),
            observed(&[("ts1", &["32.0.0.28"]), ("ts2", &["100.64.5.5"])]),
        ),
        (
            "TS + WARP",
            config_with(vec![ts_node("ts1"), warp_node("warp1")]),
            ObservedTailnetAddresses::new(),
        ),
    ];

    for (name, config, obs) in cases {
        let report = endpoint_force_route_report(&config, &obs, NO_TAILNET_DIR);
        let rules = generated_rules(&config, &obs);
        for s in &config.servers {
            let from_artifact = coverage_from_generated(s, &rules, &s.id, &obs);
            let from_report = report
                .servers
                .iter()
                .find(|r| r.server_id == s.id)
                .map_or(ForceRouteCoverage::NothingToRoute, |r| r.coverage);
            assert_eq!(
                from_report, from_artifact,
                "[{name}] {} 的三态在报告与真实产物之间分叉 —— 报告成了第二份计算",
                s.id
            );
        }
    }
}

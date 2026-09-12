//! 🔴 **门④：endpoint 的 force-route 规则，必须排在任何「更宽的段吃掉它」的规则之前。**
//!
//! # 这道门守的那个真缺口
//!
//! sing-box 的 `route.rules` 是**首匹配生效**（first-match），不做最长前缀匹配。真机 E2E 产物
//! （随包核 1.15.0-alpha.2 连真 headscale、userspace tailscale endpoint）里，`#7` 是 endpoint 的
//! force-route（`outbound=polaris-e2e`、`rule_set=tailnet-e2e-ts`），`#8` 是「绕过局域网」的私网
//! 直连表（13 条 `ip_cidr` → `direct`）。而那张表里：
//!
//!  - `fc00::/7` **完整覆盖** tailnet ULA `fd7a:115c:a1e0::/48`；
//!  - `100.64.0.0/10` **完整覆盖** 官方 tailnet v4 段（字面就是同一条）。
//!
//! ⇒ **tailnet 两族的可达性，完全靠 `#7` 排在 `#8` 前面托着。** 今天是对的，靠的只是
//! `builder/route.rs` 里「块 0c」写在「1. 私有 IP 段直连」之前这个**书写顺序**。谁重构一次把
//! 旁路块提前、或把块 0c 挪后，结果是 **tailnet 静默不可达，而全部既有门照旧绿** ——
//! 产物里那条 force-route 规则明明还在，逐条看一切正常。
//!
//! 本文件落盘之前，生产侧没有任何门断言这条顺序：`route/tests/` 与 `tests/` 里关于 force-route
//! 的三道门（`endpoint_force_route_duplicate_cidr_gate` / `_silent_absorption_gate` /
//! `_intersection_report_gate`）钉的都是 **endpoint 之间**的事 —— 谁和谁撞段、谁被吸收干净、
//! 有观测的节点是否排在无观测的之前。**endpoint 与旁路块之间的那条缝，一个断言都没有。**
//!
//! # 判据的取材面（三条硬规矩）
//!
//! **① 从产物读，不从源码顺序读。** 判据跑真实 [`generate_sing_box_config`]，在产出的
//! `route.rules` 里按**下标**比较。断言「块 0c 的代码写在块 1 之前」等于把实现复刻一遍，
//! 实现换个写法（先收集后统一 push、抽成函数、改用 `insert`）就失效，而顺序可能真的反了。
//!
//! **② 「会覆盖它」是算出来的，不是写死的字面量。** 覆盖判据取
//! [`cidr_contains`]`(eater, victim)` —— **包含**，方向性的：问的是「有没有一条**更宽或等宽**的
//! 段把它整个吃掉」。
//!
//! 为什么不是 `cidr_overlaps_any`（相交）：相交是**无方向**的，`100.64.5.5/32` 与
//! `100.64.0.0/10` 相交、`100.64.0.0/10` 与 `100.64.5.5/32` 也相交，可这两种情形在 first-match
//! 下的后果完全不同 —— 前者（窄段在前）只截走它自己那一个地址，是**正常**的分流；后者（宽段
//! 在前）把整段流量吃光。相交判据会把前者也报成红，一道对着正确实现常驻红的门只会被人忽略。
//! 语义上要的就是**方向**：`eater ⊇ victim`。仓里已有现成的方向性谓词，不必新造。
//!
//! 写死 `fc00::/7` / `100.64.0.0/10` 同样不行：那两条是 `DEFAULT_BYPASS_LAN` 今天的内容，
//! 用户可改（`bypassLANList`）、清单也会演进。判据只问「产物里有没有更宽的段」，不问它叫什么。
//!
//! **③ 两族都要判。** v4 与 v6 各有正样本（见 [`inline_leg_force_route_precedes_every_wider_segment_in_both_families`]
//! 与 [`rule_set_leg_force_route_precedes_every_wider_segment_in_both_families`]）。只判一族的话，
//! 另一族被遮蔽时本门全绿 —— 而 `fc00::/7` 吃掉 tailnet ULA 恰恰是 v6 那一格。
//!
//! # 前提无取材面时：如实报告，不许静默绿
//!
//! 「没有任何更宽的段」这件事本身是合法结果（`bypassLAN` 关掉就是这样）。此时「force-route 排在
//! 覆盖者之前」退化成空断言 —— 而空断言在 `cargo test` 里长得和真通过一模一样。故判据把它做成
//! **第四态** [`Verdict::NoCoveringRuleAtAll`]：每个用例都必须显式声明自己期望哪一态，
//! 期望「有覆盖者且顺序正确」的用例遇到这一态会红在「前提没建立」上。
//! [`bypass_lan_off_has_no_covering_rule_at_all_and_says_so`] 把这一态连同**正面对照**
//! （同一份配置开着 `bypassLAN` 时覆盖面非空）一起钉住：它证明本门的取材面确实来自旁路块，
//! 而不是判据算不出来。
//!
//! # 三条发射腿都在射程内
//!
//! 块 0c 对一个节点有三种发射形态（`endpoint_routes::force_route_leg`），本门逐条覆盖：
//!
//! | 腿 | 产物形态 | 定位靠什么 | 段集取自 |
//! |---|---|---|---|
//! | `ForceRouteLeg::Inline` | `{ip_cidr:[…], outbound:<tag>}` | `outbound == <tag>` | **规则自带的 `ip_cidr`**（= 结算后真发出去的那一份） |
//! | `ForceRouteLeg::ExternalRuleSet` | `{rule_set:"tailnet-<id>", outbound:<tag>}` | `outbound == <tag>` | [`endpoint_forced_route_cidrs`]（段值在文件里，产物里只剩路径） |
//! | `ForceRouteLeg::PreferredBy` | `{preferred_by:[<tag>], outbound:<tag>}` | `outbound == <tag>` | [`endpoint_forced_route_cidrs`]（段由内核按 endpoint 路由表归位，无字面量） |
//!
//! 定位**一律**靠 `outbound == <tag>`，不靠「读它自己的 cidr」：后两条腿的规则里根本没有
//! `ip_cidr`，靠 cidr 定位会让它们整体落在门外（而 rule-set 腿恰是真机 E2E 里实际走的那一条）。
//!
//! Inline 腿的段集为什么取**规则自带的 `ip_cidr`** 而不是一律取 [`endpoint_forced_route_cidrs`]：
//! 后者是「它想发什么」，而跨节点结算（`settle_force_route_claims`）会把与更早声明者字面重复的
//! 段丢掉。用「想发什么」去比，一个段被前面的同类节点**合法吸收**（那是门③的题目：流量仍到达
//! 同一个 tailnet 的另一个 endpoint，不是漏给 direct）会被本门读成红。取「真发了什么」既是
//! 规矩①「从产物读」的直接落实，也顺手免掉这个假红。
//!
//! # 覆盖者的面为什么不限于 `outbound == "direct"`
//!
//! 判据扫的是**全部下标更小且带 `ip_cidr` 的规则**，不预设覆盖者是谁。理由是同一个故障形态
//! 不止旁路块一条腿：两个 Inline 组网节点，A 的 `allowed_ips` 是 `10.0.0.0/8`、B 是
//! `10.1.0.0/16` —— 字面不同 ⇒ 跨节点去重一条都拦不下 ⇒ 两条规则都发出去 ⇒ A 在前就把 B
//! 整个吃掉。那也是「更宽的段吃掉它」，和旁路块那一格是同一个根因。失败消息会报出吃掉它的
//! 那条规则的 `outbound`，读的人自己能分清是哪一类。
//!
//! **一处刻意不在本门射程内的「更前面的规则」：用户自定义规则（块 3）。** 块 0c 的注释写着
//! 「用户规则之后的功能性强制路由（reorder：原在用户规则之上，现下移）」—— 自定义规则排在
//! force-route **之前是有意设计**，用户显式写的 `ip_cidr` 就该赢。本判据不认这个例外（它只看
//! 下标），故一份带 `ip_cidr` 自定义规则的夹具会被报成 `Shadowed` 而那其实是既定优先级。
//! 本门的夹具因此**一条自定义规则都不放**：要扩夹具的人得先想清这一格，别把设计读成缺陷。
//!
//! **如实登记一格已知盲区（本门不修、也不该由本门修）**：两个**都还没有观测**的 Tailscale
//! 节点，一个走 Inline、一个走 ExternalRuleSet 时，两者的段集都是那两条默认常量 ⇒ Inline 那条
//! 若排在前面，本判据会（正确地）报红。`endpoint_routes.rs` 已把这一格登记为「字面量层看不见
//! 文件内容」的残留盲区、由发射顺序兜方向而不由结算兜计数。本门的夹具不构造这一格
//! （每个用例只放一个组网节点，或让两者形态不同），故不会因它常驻红；判据本身不为它开后门。

mod support;

use std::collections::BTreeMap;

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_forced_route_cidrs, ObservedTailnetAddresses,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::singbox::{RouteRule, SingBoxConfig};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::cidr::cidr_contains;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings, WireGuardSettings,
};
use support::kernel_gate::{default_platform, outbound_deps_for};

// ─────────────────────────── 判据本体 ───────────────────────────

/// 一条「更宽的段把它整个吃掉」的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shadow {
    /// 被吃掉的 force-route 段。
    victim: String,
    /// 吃掉它的那个网段（某条规则 `ip_cidr` 里的一项）。
    eater: String,
    /// 吃掉它的那条规则在 `route.rules` 里的下标。
    eater_index: usize,
    /// 那条规则的 `outbound` —— 用来分清「旁路块吃的」还是「另一个 endpoint 吃的」。
    eater_outbound: Option<String>,
}

impl Shadow {
    /// v6 判族：CIDR 字面量含 `:` ⇒ IPv6。两族必须分开统计，否则一族被遮蔽时另一族的
    /// 正样本会把断言托绿（`cidr_contains` 跨族恒 false，所以族内自洽）。
    fn is_v6(&self) -> bool {
        self.victim.contains(':')
    }
}

/// 单个 endpoint 的「顺序」判决。四态互斥，每个用例必须显式声明期望哪一态。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// 产物里找不到属于该 tag 的 force-route 规则（没 engaged / 段集为空 / 被吸收干净）。
    /// 本门没有讨论对象 —— 那几种情形归门①②③。
    NoForceRouteRule,
    /// 🟢 存在会覆盖它的更宽段，且**全部**排在它之后（下标更大）—— 顺序正确。
    CoveredButOrdered {
        force_index: usize,
        covering_after: Vec<Shadow>,
    },
    /// 🔴 存在下标**更小**的规则，其 `ip_cidr` 里有段把它的 force-route 段整个吃掉。
    Shadowed {
        force_index: usize,
        shadows: Vec<Shadow>,
    },
    /// 整份产物里没有任何段能覆盖它 —— 本门此刻**没有取材面**（不是通过）。
    NoCoveringRuleAtAll { force_index: usize },
}

/// 产物里属于 `tag` 的那条 force-route 规则的下标。
///
/// 三条腿共用同一个定位判据：`outbound == tag` + 带至少一个发射形态字段。多于一条时直接 panic
/// —— 夹具必须是无歧义的，否则下面比的是「第一条」而故障可能在第二条。
fn force_route_rule_index(rules: &[RouteRule], tag: &str) -> Option<usize> {
    let hits: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.outbound.as_deref() == Some(tag)
                && (r.ip_cidr.is_some() || r.rule_set.is_some() || r.preferred_by.is_some())
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        hits.len() <= 1,
        "产物里有 {} 条 outbound={tag} 的 force-route 规则（下标 {:?}）—— \
         夹具歧义：本判据比的是其中一条的下标，另一条的顺序无人看",
        hits.len(),
        hits
    );
    hits.first().copied()
}

/// 该 endpoint 本轮要保护的段集。
///
/// 优先取**产物里那条规则自带的 `ip_cidr`**（Inline 腿：那是跨节点结算之后真发出去的一份）；
/// 缺席则回落 [`endpoint_forced_route_cidrs`]（rule-set 腿的段值住在文件里、`preferred_by` 腿
/// 压根没有字面量，产物里都读不回来）。理由见本文件头「三条发射腿」一节。
fn protected_cidrs(
    server: &ServerConfig,
    rule: &RouteRule,
    observed: &ObservedTailnetAddresses,
) -> Vec<String> {
    match rule.ip_cidr.as_ref() {
        Some(list) if !list.is_empty() => list.clone(),
        _ => endpoint_forced_route_cidrs(server, observed),
    }
}

/// 核心判据：该 endpoint 的 force-route 规则，与**全部**「会覆盖它」的规则比下标。
fn force_route_order_verdict(
    server: &ServerConfig,
    tag: &str,
    rules: &[RouteRule],
    observed: &ObservedTailnetAddresses,
) -> Verdict {
    let Some(force_index) = force_route_rule_index(rules, tag) else {
        return Verdict::NoForceRouteRule;
    };
    let protected = protected_cidrs(server, &rules[force_index], observed);
    if protected.is_empty() {
        return Verdict::NoForceRouteRule;
    }

    let mut before: Vec<Shadow> = Vec::new();
    let mut after: Vec<Shadow> = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        if i == force_index {
            continue; // 自己不吃自己。
        }
        let Some(cidrs) = r.ip_cidr.as_ref() else {
            continue; // 不带 ip_cidr 的规则不构成「更宽的段」（域名/进程/rule_set 另论）。
        };
        for eater in cidrs {
            for victim in &protected {
                // 方向性：eater ⊇ victim。等宽同段也算（字面相同的两条规则，前者赢）。
                if !cidr_contains(eater, victim) {
                    continue;
                }
                let s = Shadow {
                    victim: victim.clone(),
                    eater: eater.clone(),
                    eater_index: i,
                    eater_outbound: r.outbound.clone(),
                };
                if i < force_index {
                    before.push(s);
                } else {
                    after.push(s);
                }
            }
        }
    }

    if !before.is_empty() {
        return Verdict::Shadowed {
            force_index,
            shadows: before,
        };
    }
    if after.is_empty() {
        return Verdict::NoCoveringRuleAtAll { force_index };
    }
    Verdict::CoveredButOrdered {
        force_index,
        covering_after: after,
    }
}

// ─────────────────────────── 断言外壳 ───────────────────────────

/// 期望「有覆盖者、且顺序正确」，返回那些排在后面的覆盖者供逐族检查。
/// 另外三态各有专属的失败说明 —— 尤其 `NoCoveringRuleAtAll` 必须红在「前提没建立」上，
/// 不能被读成通过。
fn expect_ordered(verdict: &Verdict, what: &str) -> Vec<Shadow> {
    match verdict {
        Verdict::CoveredButOrdered { covering_after, .. } => covering_after.clone(),
        Verdict::Shadowed {
            force_index,
            shadows,
        } => panic!(
            "🔴 {what}：force-route 规则在下标 {force_index}，但有更宽的段排在它**前面**，\
             first-match 会把这些段的流量整个吃掉（产物里那条 force-route 明明还在）：{shadows:#?}"
        ),
        Verdict::NoCoveringRuleAtAll { force_index } => panic!(
            "{what}：前提没建立 —— 产物里没有任何段能覆盖这个 endpoint（force-route 在下标 \
             {force_index}）。此时「排在覆盖者之前」是空断言，本用例失去讨论对象，\
             必须当红处理而不是当绿"
        ),
        Verdict::NoForceRouteRule => {
            panic!("{what}：产物里根本没有这个 endpoint 的 force-route 规则 —— 夹具没建立起来")
        }
    }
}

/// 逐族断言：该族至少有一个正样本（真的存在更宽的段），且其中至少一条来自 `direct`
/// （= 旁路块那条私网直连表，本门的主取材面）。
fn assert_family_sample(shadows: &[Shadow], v6: bool, what: &str) {
    let fam = if v6 { "IPv6" } else { "IPv4" };
    let in_family: Vec<&Shadow> = shadows.iter().filter(|s| s.is_v6() == v6).collect();
    assert!(
        !in_family.is_empty(),
        "{what}：{fam} 族没有任何正样本 —— 这一族此刻没有「会覆盖它的更宽段」，\
         于是该族的顺序无人断言。全部覆盖者：{shadows:#?}"
    );
    assert!(
        in_family
            .iter()
            .any(|s| s.eater_outbound.as_deref() == Some("direct")),
        "{what}：{fam} 族的覆盖者里没有一条是 direct —— 本门要钉的那条缝（旁路块吃掉组网段）\
         在这一族上没有取材面。该族覆盖者：{in_family:#?}"
    );
}

// ─────────────────────────── 夹具 ───────────────────────────

/// Tailscale 节点。`alwaysRouteSubnets` 不设 = 缺省 true ⇒ 恒 engaged，不依赖选中/规则点名。
fn ts_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Tailscale,
        tailscale_settings: Some(Box::new(TailscaleSettings::default())),
        ..Default::default()
    }
}

/// 「只走内网段」的 WireGuard 节点：`allowInternet=false` + 具体 `allowedIPs`
/// ⇒ 非全隧道 ⇒ [`force_route_leg`](polaris_config_engine::builder::endpoint_routes::force_route_leg)
/// 选 `PreferredBy` 腿（规则里既没有 `ip_cidr` 也没有 `rule_set`）。
fn wg_lan_node(id: &str, allowed: &[&str]) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        protocol: Protocol::Wireguard,
        address: "wg.example.invalid".into(),
        wireguard_settings: Some(Box::new(WireGuardSettings {
            private_key: Some("cHJpdmF0ZWtleQ==".into()),
            peer_public_key: Some("cHVibGlja2V5".into()),
            local_address: vec!["10.7.0.2/32".into()],
            allowed_ips: allowed.iter().map(|s| (*s).to_string()).collect(),
            allow_internet: Some(false),
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn config_with(servers: Vec<ServerConfig>) -> UserConfig {
    UserConfig {
        servers,
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    }
}

fn generate(
    config: &UserConfig,
    deps: &polaris_config_engine::builder::GenerateConfigDeps,
) -> SingBoxConfig {
    generate_sing_box_config(config, &BTreeMap::new(), deps).expect("生成配置")
}

fn rules_of(cfg: &SingBoxConfig) -> Vec<RouteRule> {
    cfg.route.as_ref().expect("没有 route 段").rules.clone()
}

// ─────────────────────────── 门 ───────────────────────────

/// 🔴 **Inline 腿 · 两族正样本。**
///
/// 一个无观测的 Tailscale 节点走 Inline 腿，发的是 tailnet 两族默认段
/// （v4 CGNAT + v6 ULA）。两者在默认旁路清单里各有一个更宽/等宽的吃家
/// （实测：`100.64.0.0/10` 被同字面的旁路条目等宽覆盖、`fd7a:115c:a1e0::/48` 被 `fc00::/7`
/// 包含）。本用例只断言「**算出来**的覆盖者全部排在它之后」，不写这两条字面量。
#[test]
fn inline_leg_force_route_precedes_every_wider_segment_in_both_families() {
    let node = ts_node("ts-inline");
    let input = config_with(vec![node.clone()]);
    let deps = outbound_deps_for(&default_platform());
    let rules = rules_of(&generate(&input, &deps));

    // 前提①：这条 force-route 真的走 Inline 腿（产物里它自带 ip_cidr）。
    let idx = force_route_rule_index(&rules, "ts-inline")
        .expect("产物里没有 ts-inline 的 force-route 规则");
    assert!(
        rules[idx].ip_cidr.is_some(),
        "前提没建立：ts-inline 这条规则不带 ip_cidr ⇒ 它走的不是 Inline 腿，\
         本用例失去「Inline」这个讨论对象。规则：{:#?}",
        rules[idx]
    );

    let verdict =
        force_route_order_verdict(&node, "ts-inline", &rules, &deps.observed_tailnet_addresses);
    let covering = expect_ordered(&verdict, "Inline 腿");
    assert_family_sample(&covering, false, "Inline 腿");
    assert_family_sample(&covering, true, "Inline 腿");
}

/// 🔴 **rule-set 腿 · 两族正样本**（真机 E2E 实际走的那一条）。
///
/// tailnet rule-set 文件落盘 ⇒ 块 0c 发的是 `{rule_set:"tailnet-<id>", outbound:<tag>}`，
/// 规则里**没有 `ip_cidr`**。故定位只能靠 `outbound == tag`、段集只能取
/// [`endpoint_forced_route_cidrs`]。这正是真机 `#7` 的形态。
#[test]
fn rule_set_leg_force_route_precedes_every_wider_segment_in_both_families() {
    let tmp = tempfile::TempDir::new().expect("建临时目录");
    let dir = tmp.path().join("tailnet-rules");
    std::fs::create_dir_all(&dir).expect("建 tailnet-rules 目录");
    // 块 0c 的存在性检查是真 `existsSync` 等价 ⇒ 文件必须真落盘。
    std::fs::write(
        dir.join(format!(
            "{}.json",
            polaris_config_engine::builder::endpoint_routes::tailnet_rule_file_base("ts-file")
        )),
        r#"{"version":1,"rules":[{"ip_cidr":["100.64.0.0/10","fd7a:115c:a1e0::/48"]}]}"#,
    )
    .expect("写 tailnet rule-set");

    let node = ts_node("ts-file");
    let input = config_with(vec![node.clone()]);
    let mut deps = outbound_deps_for(&default_platform());
    deps.tailnet_rules_dir = dir.display().to_string();
    let rules = rules_of(&generate(&input, &deps));

    // 前提①：真的走 rule-set 腿 —— 有 rule_set、且**没有** ip_cidr（故判据不能靠读它自己的段定位）。
    let idx =
        force_route_rule_index(&rules, "ts-file").expect("产物里没有 ts-file 的 force-route 规则");
    assert!(
        rules[idx].rule_set.is_some() && rules[idx].ip_cidr.is_none(),
        "前提没建立：ts-file 这条规则不是 rule-set 腿的形态（rule_set 在场且 ip_cidr 缺席）。\
         规则：{:#?}",
        rules[idx]
    );

    let verdict =
        force_route_order_verdict(&node, "ts-file", &rules, &deps.observed_tailnet_addresses);
    let covering = expect_ordered(&verdict, "rule-set 腿");
    assert_family_sample(&covering, false, "rule-set 腿");
    assert_family_sample(&covering, true, "rule-set 腿");
}

/// 🔴 **preferred_by 腿**（v4）。
///
/// 第三条腿：规则里既没有 `ip_cidr` 也没有 `rule_set`，只有 `preferred_by` —— 段由内核按
/// endpoint 自身路由表归位。它同样是 `route.rules` 里的一条普通规则，同样被 first-match 管，
/// 故一条排在它前面的 `192.168.0.0/16 → direct` 照样能把 `192.168.50.0/24` 整个吃掉。
///
/// 本腿天生只有 v4 样本（夹具给的是一段 v4 内网）；两族的覆盖由上面两个用例负责。
#[test]
fn preferred_by_leg_force_route_precedes_every_wider_segment() {
    let node = wg_lan_node("wg-lan", &["192.168.50.0/24"]);
    let input = config_with(vec![node.clone()]);
    let deps = outbound_deps_for(&default_platform());
    let rules = rules_of(&generate(&input, &deps));

    let idx =
        force_route_rule_index(&rules, "wg-lan").expect("产物里没有 wg-lan 的 force-route 规则");
    assert!(
        rules[idx].preferred_by.is_some()
            && rules[idx].ip_cidr.is_none()
            && rules[idx].rule_set.is_none(),
        "前提没建立：wg-lan 这条规则不是 preferred_by 腿的形态。规则：{:#?}",
        rules[idx]
    );

    let verdict =
        force_route_order_verdict(&node, "wg-lan", &rules, &deps.observed_tailnet_addresses);
    let covering = expect_ordered(&verdict, "preferred_by 腿");
    assert_family_sample(&covering, false, "preferred_by 腿");
}

/// 🟡 **前提无取材面时如实报告 + 正面对照。**
///
/// `bypassLAN` 关掉 ⇒ 私网直连表整条不发 ⇒ 产物里再没有任何段能覆盖 tailnet 两族 ⇒
/// 「force-route 排在覆盖者之前」这条命题**没有取材面**。本用例把那一态显式钉成
/// [`Verdict::NoCoveringRuleAtAll`]，而不是让上面那些断言在空集上静默通过。
///
/// 后半段是**正面对照**：同一份配置只把 `bypassLAN` 打开，覆盖面立刻非空、且吃家是 `direct`。
/// 少了这半段，「没有覆盖者」可能是判据算不出来（谓词写反、族分派漏了一支），
/// 和「确实不存在覆盖者」在测试输出里长得一样。
#[test]
fn bypass_lan_off_has_no_covering_rule_at_all_and_says_so() {
    let node = ts_node("ts-inline");
    let deps = outbound_deps_for(&default_platform());

    let off = UserConfig {
        bypass_lan: Some(false),
        ..config_with(vec![node.clone()])
    };
    let rules_off = rules_of(&generate(&off, &deps));
    let verdict_off = force_route_order_verdict(
        &node,
        "ts-inline",
        &rules_off,
        &deps.observed_tailnet_addresses,
    );
    let idx_off = force_route_rule_index(&rules_off, "ts-inline")
        .expect("关掉 bypassLAN 不该让 force-route 规则消失");
    assert_eq!(
        verdict_off,
        Verdict::NoCoveringRuleAtAll {
            force_index: idx_off
        },
        "bypassLAN 关掉时，产物里不应再有任何覆盖 tailnet 两族的段。实得：{verdict_off:#?}"
    );

    // 正面对照：同一份配置开着 bypassLAN ⇒ 覆盖面非空、来自 direct ⇒ 上面那个空集是真的空，
    // 不是判据哑了。
    let on = config_with(vec![node.clone()]);
    let rules_on = rules_of(&generate(&on, &deps));
    let covering_on = expect_ordered(
        &force_route_order_verdict(
            &node,
            "ts-inline",
            &rules_on,
            &deps.observed_tailnet_addresses,
        ),
        "正面对照（bypassLAN 开）",
    );
    assert_family_sample(&covering_on, false, "正面对照（bypassLAN 开）");
    assert_family_sample(&covering_on, true, "正面对照（bypassLAN 开）");
}

/// 🟢 **判据有牙（合成输入的反向对照）。**
///
/// 主反向对照是在生产源码上把块 0c 的发射挪到旁路块之后、看本门转红（收据随本批交付）。
/// 但那次变异不进仓，仓里必须留一条**常驻**的证据说明判据分得开两种顺序 —— 否则哪天
/// `cidr_contains` 被换成恒 false，上面三个用例会一起退化成
/// [`Verdict::NoCoveringRuleAtAll`]，那时它们会红在「前提没建立」上（这也是把那一态做成
/// 独立判决而不是直接绿的第二个收益），而本用例会直接红在「该红的没红」上。
///
/// 两族各给一个逆序样本：v4 等宽同段、v6 更宽包含。
#[test]
fn detector_flags_the_reversed_order_on_synthetic_rules_in_both_families() {
    let node = ts_node("ts-inline");
    let no_observation = ObservedTailnetAddresses::new();
    let forced = endpoint_forced_route_cidrs(&node, &no_observation);
    assert_eq!(
        forced.len(),
        2,
        "前提：无观测的 TS 节点应发两族默认段各一条，实得 {forced:?}"
    );

    let bypass = RouteRule {
        ip_cidr: Some(vec!["100.64.0.0/10".into(), "fc00::/7".into()]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        ..Default::default()
    };
    let force = RouteRule {
        ip_cidr: Some(forced.clone()),
        action: Some("route".into()),
        outbound: Some("ts-inline".into()),
        ..Default::default()
    };

    // 逆序（旁路在前）⇒ 必须红。
    let reversed = vec![bypass.clone(), force.clone()];
    let verdict = force_route_order_verdict(&node, "ts-inline", &reversed, &no_observation);
    let Verdict::Shadowed { shadows, .. } = &verdict else {
        panic!("旁路块排在 force-route 之前，判据却没报 Shadowed —— 它没有牙。实得：{verdict:#?}");
    };
    assert!(
        shadows.iter().any(|s| !s.is_v6()),
        "逆序下 IPv4 族没被报出来：{shadows:#?}"
    );
    assert!(
        shadows.iter().any(Shadow::is_v6),
        "逆序下 IPv6 族没被报出来：{shadows:#?}"
    );

    // 正序（force-route 在前）⇒ 同一批规则必须绿。两个方向都测，才排除「判据恒红」。
    let ordered = vec![force, bypass];
    let covering = expect_ordered(
        &force_route_order_verdict(&node, "ts-inline", &ordered, &no_observation),
        "合成正序",
    );
    assert_eq!(
        covering.len(),
        2,
        "合成正序下应恰好两条覆盖者（两族各一），实得 {covering:#?}"
    );
}

/// 🟢 **`cidr_contains` 用的是「包含」而不是「相交」——方向性必须是判据的一部分。**
///
/// 这条不是重测 `cidr` 模块的单测，而是钉住**本门的选择**：窄段在前是正常分流（不许报红），
/// 宽段在前才是故障。若哪天有人把 `cidr_contains` 换成 `cidr_overlaps_any`（两个名字长得像、
/// 参数形状也兼容），上面三个用例**不会红**（旁路段确实也与组网段相交），只有本用例会红。
#[test]
fn a_narrower_segment_in_front_is_not_a_defect_only_a_wider_one_is() {
    let node = ts_node("ts-inline");
    let no_observation = ObservedTailnetAddresses::new();
    let forced = endpoint_forced_route_cidrs(&node, &no_observation);
    let victim_v4 = forced
        .iter()
        .find(|c| !c.contains(':'))
        .expect("无观测 TS 节点应有一条 v4 默认段")
        .clone();

    // 窄段在前：一条把**单个地址**钉到别处的规则。first-match 只截走那一个地址，
    // 整段仍由后面的 force-route 承接 —— 正常，不许报红。
    let narrow_first = vec![
        RouteRule {
            ip_cidr: Some(vec!["100.64.5.5/32".into()]),
            action: Some("route".into()),
            outbound: Some("direct".into()),
            ..Default::default()
        },
        RouteRule {
            ip_cidr: Some(forced.clone()),
            action: Some("route".into()),
            outbound: Some("ts-inline".into()),
            ..Default::default()
        },
    ];
    assert!(
        cidr_contains(&victim_v4, "100.64.5.5/32"),
        "前提：{victim_v4} 确实包含 100.64.5.5/32（即两者相交），\
         否则下面「相交但不包含 ⇒ 不报红」这件事没有讨论对象"
    );
    assert!(
        !cidr_contains("100.64.5.5/32", &victim_v4),
        "前提：反方向不成立 —— 这正是「包含」有方向而「相交」没有的那个差别"
    );
    let verdict = force_route_order_verdict(&node, "ts-inline", &narrow_first, &no_observation);
    let idx = force_route_rule_index(&narrow_first, "ts-inline").expect("合成规则里有 force-route");
    assert_eq!(
        verdict,
        Verdict::NoCoveringRuleAtAll { force_index: idx },
        "一条更窄的段排在前面被报成了覆盖者 —— 判据退化成了「相交」，\
         这会让本门对着正确实现常驻红。实得：{verdict:#?}"
    );
}

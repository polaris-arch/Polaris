//! 🔴 **自建 tailnet 的真实前缀，能不能进到 force-route 的四个消费面。**
//!
//! # 背景（2026-09-11 用真控制面实测确认的真缺陷）
//!
//! `builder::endpoint_routes` 的 [`TAILNET_CGNAT`] = `100.64.0.0/10` 是**上游官方控制面的默认
//! 前缀**，不是协议常量：headscale 的 `prefixes.v4` 可自定义。实测某自建 tailnet 把地址发成
//! `32.0.0.28` / `32.0.0.29`（v6 仍是默认 `fd7a:115c:a1e0::/48`）。
//!
//! 后果链（每一环都实测可复现）：Polaris 为该节点发的 v4 force-route 规则命中不了任何流量 →
//! TUN 的 `auto_route`（`0.0.0.0/1` + `128.0.0.0/1`）照样把 `32.0.0.x` 吞进来 → 进 sing-box 路由 →
//! 没有任何规则把它送去 tailscale endpoint → 落到 `final`。而 `32.0.0.0/8` 是 IANA 分配给 AT&T 的
//! **真实公网段** —— 落 final 不是"退化成直连"那么轻，它去的是别人的地址空间。
//!
//! # 本门验什么
//!
//! [`endpoint_forced_route_cidrs`] 有**四个**消费者，规则发射只是其一。只修那一个，用户原始症状
//! （TUN 把 `32.0.0.x` 吞掉）不会好。本门按消费面逐个钉：
//!
//! | 消费面 | 本门的哪条 |
//! |---|---|
//! | `builder::route` 块 0c（规则发射） | [`observed_v4_address_reaches_the_force_route_rule`] |
//! | `builder::inbounds`（TUN 排除面 `engaged_mesh`） | [`observed_addresses_reach_the_tun_exclusion_face`] |
//! | `builder::hotswitch`（热切资格） | 在 `hotswitch` 的单测里钉不敏感性（见那边的 `sel_only_forces_subnets_is_insensitive_to_observed_addresses`）——该谓词只问「段集非空」，TS 默认两段恒非空 ⇒ 有无观测同值，**可证明等价**，不是漏改 |
//! | `builder::tunnel_conflict`（`ConflictInput::mesh_cidrs`） | 由 [`mesh_forced_route_cidrs`] 供段，本门的 [`observed_addresses_reach_mesh_cidrs_union`] 钉该供给面（该模块本身**在生产里还没有调用点**，只有单测，故只能钉到供给为止） |
//!
//! # 判据里为什么两层都要（函数层 + 生成产物层）
//!
//! 只测 [`endpoint_forced_route_cidrs`] 的返回值 = 测了方法体，测不出"生产路径真的在用它"；
//! 只测生成产物 = 一条断言横跨太多环节，红了定位不到。两条成对交才既有牙又可定位。
//!
//! # 降级腿（本批的硬要求）
//!
//! 观测面为空 + tailnet 文件不存在 ⇒ 产出必须与本批改动之前**逐字节相同**。本门钉住其
//! **可自证的那一半**（两种"文件都不存在"的路径互为逐字节相等 + 产出里不含任何 `tailnet-` tag +
//! inline 形状逐字节等于两条常量段）；"与改动前相同"的权威证据是仓内既有的冻结金样门
//! `golden_config_snapshot.rs`（`fixtures/config-snapshot.json` 是上游冻结样本、本仓重生不了，
//! 实测全文 `tailscale` 出现 0 次 ⇒ 本改动不该触碰它，它红就是改到了不该改的地方）。

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_forced_route_cidrs, mesh_forced_route_cidrs, tailnet_rule_file_base,
    ObservedTailnetAddresses, TAILNET_CGNAT, TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6,
    TAILNET_ULA_V6,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
use polaris_config_engine::singbox::{OneOrMany, RouteRule, SingBoxConfig};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::cidr::cidr_contains;
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings,
};
use polaris_config_engine::user_config::LogLevel;
use support::kernel_gate::{default_platform, outbound_deps_for};
use tempfile::TempDir;

/// 2026-09-11 真控制面实测值：自建 headscale 的 `prefixes.v4` 非默认，self 拿到 `32.0.0.28`。
/// **刻意用这个真实值而不是随手编一个**：`32.0.0.0/8` 是 IANA 分配给 AT&T 的公网段，
/// 它同时证明"落 final 会去到别人的地址空间"这件事不是假想。
const OBSERVED_V4: &str = "32.0.0.28";
/// 同一次实测的 v6 侧：v6 前缀仍是默认 `fd7a:115c:a1e0::/48`，故这个地址在**并集**语义下
/// 本来就被 [`TAILNET_ULA_V6`] 兜着。
///
/// 2026-09-11 语义改成「有观测 ⇒ 不发默认段」之后，它的作用**反过来**了：默认 `/48` 被撤掉，
/// 这个地址必须以 `/128` 独立出现在覆盖面里 —— 否则 v6 侧就是靠一条不再发射的段兜着，
/// 整个 v6 tailnet 静默失覆盖，而 v4 侧的断言全绿、看不出来。
const OBSERVED_V6: &str = "fd7a:115c:a1e0::1c";

fn ts_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        // `alwaysRouteSubnets` 不设 = 缺省 true ⇒ 恒 engaged，不依赖选中/规则点名。
        tailscale_settings: Some(Box::new(TailscaleSettings::default())),
        protocol: Protocol::Tailscale,
        ..Default::default()
    }
}

fn observed(entries: &[(&str, &[&str])]) -> ObservedTailnetAddresses {
    entries
        .iter()
        .map(|(id, addrs)| {
            (
                (*id).to_string(),
                addrs.iter().map(|a| (*a).to_string()).collect(),
            )
        })
        .collect()
}

fn config_with(servers: Vec<ServerConfig>) -> UserConfig {
    UserConfig {
        servers,
        // `"__direct__"` 是「未选择真实节点」的哨兵：selected_server_id 必须是某个真实节点或它，
        // 否则 generate 直接 Err。用它可以避开「选中节点 IP 排除」块往产物里追加无关条目。
        selected_server_id: Some("__direct__".into()),
        ..Default::default()
    }
}

/// 跑真实 `generate_sing_box_config`，`tailnet_rules_dir` / 观测面由入参控制。
fn generate(
    config: &UserConfig,
    tailnet_rules_dir: &str,
    obs: ObservedTailnetAddresses,
) -> SingBoxConfig {
    let mut deps = outbound_deps_for(&default_platform());
    deps.tailnet_rules_dir = tailnet_rules_dir.to_string();
    deps.observed_tailnet_addresses = obs;
    generate_sing_box_config(config, &BTreeMap::new(), &deps).expect("生成配置")
}

fn rules_of(cfg: &SingBoxConfig) -> &[RouteRule] {
    &cfg.route.as_ref().expect("没有 route 段").rules
}

/// 某 outbound 的 inline force-route 规则（块 0c 的 `ip_cidr` 腿）。
fn inline_force_route<'a>(rules: &'a [RouteRule], tag: &str) -> Option<&'a RouteRule> {
    rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some(tag) && r.ip_cidr.is_some())
}

/// 某 outbound 的 rule_set force-route 规则（块 0c 的外化腿）。
fn rule_set_force_route<'a>(rules: &'a [RouteRule], tag: &str) -> Option<&'a RouteRule> {
    rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some(tag) && r.rule_set.is_some())
}

// ============================================================================
// 消费面 ①：块 0c 规则发射
// ============================================================================

/// 🔴 缺陷回放：观测到 `32.0.0.28` 的 TS 节点，其 force-route 覆盖面必须含 `32.0.0.28/32`。
#[test]
fn observed_v4_address_reaches_the_force_route_rule() {
    let node = ts_node("ts1");
    let obs = observed(&[("ts1", &[OBSERVED_V4, OBSERVED_V6])]);

    // 层一（函数）：纯逻辑面确实把观测地址并进来了。
    let cidrs = endpoint_forced_route_cidrs(&node, &obs);
    assert!(
        cidrs.contains(&format!("{OBSERVED_V4}/32")),
        "观测到的 tailnet v4 地址 {OBSERVED_V4} 没进 force-route 覆盖面。\
         这正是 2026-09-11 实测的那个缺陷：硬编码 {TAILNET_CGNAT} 覆盖不到自建 tailnet 的真实前缀，\
         该节点的 v4 force-route 规则一条流量都命中不了。实际覆盖面：{cidrs:?}"
    );
    assert!(
        cidrs.contains(&format!("{OBSERVED_V6}/128")),
        "观测到的 tailnet v6 地址没被补成 /128 主机段。实际：{cidrs:?}"
    );
    // 🔴 **预期 delta（2026-09-11）：取代，不是并集。** 本批之前这里断言的是「默认段一条都
    // 不许少」。改掉的理由是并集在**多节点**下会造出真实错路由：两个 TS 节点即便 tailnet 完全
    // 不相交，那两条默认常量仍然字面相同 ⇒ 块 0c 的跨节点「首声明者占有」把后声明者的默认段
    // 吸收掉 ⇒ 先声明者把整个 `100.64.0.0/10` 拿走，后声明者自己的地址被送去别人那儿。
    // 完整理由见 `endpoint_routes::ObservedTailnetAddresses` 的「# 语义」一节。
    //
    // 默认段并没有被删掉，只是移到了「无观测」那一格作 bootstrap ——
    // 由 `without_observation_the_public_address_is_absent` 钉住。
    //
    // **但 MagicDNS 的两个服务地址必须补回来**：它们不随控制面变、也永远不会出现在 netmap 里
    // （不是 peer），被「取代」连坐删掉就等于让 `dig @100.100.100.100 <peer>` 这类直接寻址落
    // `final`。「含」与「不含」两条必须成对断言 —— 只断一半的话，把 quad100 一起删掉的实现
    // 照样绿，而那正是本条要挡的回归。
    for keep in [TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6] {
        assert!(
            cidrs.iter().any(|c| c == keep),
            "有观测的节点没发 MagicDNS 服务地址 {keep} —— 它不是 peer、观测面永远抓不到它，\
             被默认段的取代规则连坐删掉，应用直接寻址 quad100 的包会落 final。实际：{cidrs:?}"
        );
    }
    assert!(
        !cidrs.contains(&TAILNET_CGNAT.to_string()) && !cidrs.contains(&TAILNET_ULA_V6.to_string()),
        "有观测的节点仍发出了官方默认段 —— 两个不相交的 tailnet 会因为这两条字面相同的常量\
         重新抢同一段。实际：{cidrs:?}"
    );

    // 层二（生产路径）：真实生成产物里，块 0c 那条规则确实带上了它。
    // 函数被测 ≠ 生产在用它，两条判据必须成对交。
    let cfg = generate(&config_with(vec![node]), "/nonexistent/tailnet-rules", obs);
    let rule = inline_force_route(rules_of(&cfg), "ts1")
        .expect("engaged 的 ts1 没有发出任何 inline force-route 规则");
    let emitted = rule.ip_cidr.as_ref().unwrap();
    assert!(
        emitted.contains(&format!("{OBSERVED_V4}/32")),
        "纯逻辑面算出来了，但生成产物里那条规则没有它 —— 块 0c 没把观测面接上。实际：{emitted:?}"
    );
    assert!(
        emitted.contains(&format!("{OBSERVED_V6}/128")),
        "生成产物里 v6 观测段丢了 —— 默认 `/48` 已经不发了，少这一条 v6 侧就整段失覆盖。\
         实际：{emitted:?}"
    );
    assert!(
        !emitted.contains(&TAILNET_CGNAT.to_string())
            && !emitted.contains(&TAILNET_ULA_V6.to_string()),
        "纯逻辑面已经不发默认段，生成产物里却还有 —— 块 0c 在复算一份自己的段集。实际：{emitted:?}"
    );
    // 产物层同样成对断言（函数被测 ≠ 生产在用它）。
    for keep in [TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6] {
        assert!(
            emitted.iter().any(|c| c == keep),
            "生成产物里没有 MagicDNS 服务地址 {keep}。实际：{emitted:?}"
        );
    }
}

/// 🔴 **bootstrap 腿对 MagicDNS 的覆盖是算出来的，不是假设的。**
///
/// 有观测腿显式补发 [`TAILNET_MAGICDNS_V4`] / [`TAILNET_MAGICDNS_V6`]；无观测腿不补，理由是
/// 默认两段**本就包含**它们。那句「本就包含」在 `endpoint_routes.rs` 里只是一行注释 ——
/// 注释对执行没有强制力：哪天有人把 [`TAILNET_CGNAT`] 改窄（比如跟着某个控制面改成
/// `100.64.0.0/16`），quad100 会在 bootstrap 腿上**静默失覆盖**，而所有既有断言照常绿。
///
/// 本门把那句话变成一次真计算。它红 = 要么改默认段、要么 bootstrap 腿也得显式补发。
#[test]
fn magicdns_addresses_are_contained_by_the_default_segments() {
    for (outer, inner) in [
        (TAILNET_CGNAT, TAILNET_MAGICDNS_V4),
        (TAILNET_ULA_V6, TAILNET_MAGICDNS_V6),
    ] {
        assert!(
            cidr_contains(outer, inner),
            "{inner} 不在 {outer} 之内 —— 无观测（bootstrap）腿不显式补发 MagicDNS 地址的前提塌了，\
             该腿上 quad100 已静默失覆盖"
        );
    }
    // 正向对照：`cidr_contains` 不是恒真（否则上面两条没有信息量）。
    assert!(
        !cidr_contains(TAILNET_CGNAT, "32.0.0.28/32"),
        "cidr_contains 恒真 —— 上面的包含断言没有区分力"
    );

    // 与真实产出对拍：无观测腿确实**没有**把 MagicDNS 单独列出来（它靠包含关系兜着）。
    let bare = endpoint_forced_route_cidrs(&ts_node("ts1"), &ObservedTailnetAddresses::new());
    assert!(
        !bare
            .iter()
            .any(|c| c == TAILNET_MAGICDNS_V4 || c == TAILNET_MAGICDNS_V6),
        "无观测腿多发了 MagicDNS 条目 —— 降级腿的产出不再与观测面出现之前逐字节相同：{bare:?}"
    );
}

/// 反向对照：把观测注入拿掉 ⇒ 上面那条断言必须红（证明它不是恒真）。
///
/// 这条同时是**降级腿的一半**：无观测时 inline 形状必须逐字节等于两条常量段。
#[test]
fn without_observation_the_public_address_is_absent() {
    let node = ts_node("ts1");
    let cidrs = endpoint_forced_route_cidrs(&node, &ObservedTailnetAddresses::new());
    assert_eq!(
        cidrs,
        vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()],
        "无观测时的覆盖面必须逐字节等于两条官方常量段（= 本批改动之前的行为）"
    );

    let cfg = generate(
        &config_with(vec![node]),
        "/nonexistent/tailnet-rules",
        ObservedTailnetAddresses::new(),
    );
    let rule = inline_force_route(rules_of(&cfg), "ts1").expect("ts1 没有 inline force-route 规则");
    assert_eq!(
        rule.ip_cidr.as_ref().unwrap(),
        &vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()],
        "无观测时块 0c 的产出必须与本批改动之前逐字节相同"
    );
}

/// WireGuard 侧的不敏感性：观测面装的是 **Tailscale** 节点的地址，即便 map 的 key 撞上了一个
/// WG 节点的 id，也绝不能并进 WG 的段里 —— 那会让 `wireguard_peer_allowed_ips`（cryptokey
/// routing table）宣称这条 WG 隧道能送 tailnet 流量，而对端根本没有那些地址的路由 ⇒ 黑洞。
#[test]
fn observed_addresses_never_leak_into_non_tailscale_protocols() {
    let mut wg = ServerConfig {
        id: "ts1".into(), // 故意与观测面同 key
        name: "wg".into(),
        protocol: Protocol::Wireguard,
        address: "1.2.3.4".into(),
        port: 51820,
        ..Default::default()
    };
    wg.wireguard_settings = Some(Box::new(
        polaris_config_engine::user_config::server_config::WireGuardSettings {
            allowed_ips: vec!["10.0.0.0/24".into()],
            ..Default::default()
        },
    ));
    let obs = observed(&[("ts1", &[OBSERVED_V4])]);
    assert_eq!(
        endpoint_forced_route_cidrs(&wg, &obs),
        vec!["10.0.0.0/24".to_string()],
        "观测到的 tailnet 地址漏进了 WireGuard 的段集"
    );
}

// ============================================================================
// 消费面 ②：TUN 排除面（用户原始症状的直接判据）
// ============================================================================

thread_local! {
    static CAPTURED: RefCell<Vec<(LogLevel, String)>> = const { RefCell::new(Vec::new()) };
}

fn capture_log(level: LogLevel, message: &str) {
    CAPTURED.with(|c| c.borrow_mut().push((level, message.to_owned())));
}

fn take_captured() -> Vec<(LogLevel, String)> {
    CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// 用户在「连入来源排除」里声明了一段覆盖自建 tailnet 的网段（那正是这次事故里外部给的
/// 排查建议），配置里同时有一个 engaged 的 TS 节点。
fn tun_config_declaring(exclude: &str) -> UserConfig {
    let json = format!(
        r#"{{
            "servers": [
                {{ "id": "ts1", "name": "ts1", "protocol": "tailscale", "tailscaleSettings": {{}} }}
            ],
            "selectedServerId": null,
            "proxyModeType": "tun",
            "tunConfig": {{ "inboundExcludeCidrs": ["{exclude}"] }}
        }}"#
    );
    serde_json::from_str(&json).expect("测试 config 反序列化失败")
}

fn inbounds_deps(obs: ObservedTailnetAddresses) -> InboundsDeps {
    InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        update_in_port: None,
        subscription_update_in_port: None,
        probe_pool_ports: vec![],
        // darwin：非 linux ⇒「连入来源排除」这条腿真的会跑；own_lan 留空以免物理 LAN 减法干扰。
        platform: "darwin".into(),
        own_lan_cidrs: vec![],
        log: capture_log,
        observed_tailnet_addresses: obs,
    }
}

fn tun_exclude_of(config: &UserConfig, obs: ObservedTailnetAddresses) -> (Vec<String>, bool) {
    take_captured();
    let deps = inbounds_deps(obs);
    let inbounds = build_inbounds(config, None, &deps);
    let exclude = inbounds
        .iter()
        .find(|i| i.type_field == "tun")
        .and_then(|t| t.route_exclude_address.clone())
        .unwrap_or_default();
    let mesh_warned = take_captured()
        .iter()
        .any(|(lvl, msg)| *lvl == LogLevel::Warn && msg.contains("与生效组网"));
    (exclude, mesh_warned)
}

/// 🔴 消费面 ②：TUN 排除面必须看得见观测到的 tailnet 地址。
///
/// `engaged_mesh` 在 `builder::inbounds` 里的作用是**减法的减数**：与组网段相交的用户声明段
/// 会被整条丢弃（"mesh 优先，否则声明段把组网架空"）。所以"看得见"在这个面上的可观测形态就是
/// —— 用户声明的 `32.0.0.0/24` **不再**进 `route_exclude_address`，且组网重叠 warn 响。
///
/// 这正是用户原始症状的另一半：只修规则发射，这条段仍会被排出 TUN ⇒ 流量根本到不了 sing-box，
/// 修好的那条 force-route 规则一辈子见不到它。
#[test]
fn observed_addresses_reach_the_tun_exclusion_face() {
    let config = tun_config_declaring("32.0.0.0/24");

    // 对照组（反向对照，同时证明断言不是恒真）：无观测 ⇒ 该段与 100.64.0.0/10 不相交 ⇒ 存活。
    let (without, warned_without) = tun_exclude_of(&config, ObservedTailnetAddresses::new());
    assert!(
        without.contains(&"32.0.0.0/24".to_string()),
        "无观测时 32.0.0.0/24 本该原样进排除表（它与硬编码的 {TAILNET_CGNAT} 不相交）。实际：{without:?}"
    );
    assert!(
        !warned_without,
        "无观测时不该有组网重叠 warn —— 有就说明对照组本身已被污染"
    );

    // 实验组：观测到该 tailnet 的真实地址 ⇒ 组网优先，用户声明段被丢弃 + warn 响。
    let (with, warned_with) = tun_exclude_of(&config, observed(&[("ts1", &[OBSERVED_V4])]));
    assert!(
        !with.contains(&"32.0.0.0/24".to_string()),
        "TUN 排除面没看见观测到的 tailnet 地址：用户声明的 32.0.0.0/24 仍被排出 TUN ⇒ \
         `32.0.0.x` 的流量根本进不了 sing-box，块 0c 那条修好的 force-route 规则永远见不到它。\
         实际排除表：{with:?}"
    );
    assert!(
        warned_with,
        "声明段被组网优先丢弃了，却没有 warn —— 用户填了段、内核没排除，零线索。\
         捕获日志：{:?}",
        take_captured()
    );
}

/// 🔴 **判据 5：TUN 排除面排的是「观测段」，不再是「默认段」**（2026-09-11 语义变更的直接腿）。
///
/// 上面那条只证明了「观测地址进得来」。本条证明的是另一半：默认段**出去了**。
///
/// 取材点选 `100.80.0.0/16` —— 它在默认段 `100.64.0.0/10` **之内**、却与观测到的
/// `100.64.5.5/32` **不相交**。于是同一条用户声明在两侧走出相反的结局：
///  - 无观测：组网段 = 默认两段 ⇒ `100.80.0.0/16 ⊂ 100.64.0.0/10` ⇒ 相交 ⇒ 整条被丢 + warn；
///  - 有观测：组网段 = `100.64.5.5/32` ⇒ 不相交 ⇒ **原样保留**、不 warn。
///
/// 这也是并集语义的用户可见代价：并集时代，只要配置里有一个 TS 节点，用户填的任何一段
/// CGNAT 地址都会被那条 `/10` 吞掉 —— 哪怕那个节点的 tailnet 根本不在 CGNAT 上。
#[test]
fn tun_exclusion_face_follows_the_observed_prefix_not_the_default_one() {
    let config = tun_config_declaring("100.80.0.0/16");

    // 供给面（先钉住成因，否则下面排除表的差异可能来自任何无关原因）。
    let servers = [ts_node("ts1")];
    let obs = observed(&[("ts1", &["100.64.5.5"])]);
    let with_union = mesh_forced_route_cidrs(&servers, &obs);
    assert!(
        with_union.contains(&"100.64.5.5/32".to_string()),
        "组网段并集里没有观测地址：{with_union:?}"
    );
    assert!(
        !with_union
            .iter()
            .any(|c| c == TAILNET_CGNAT || c == TAILNET_ULA_V6),
        "有观测的节点仍把默认两段喂进了 TUN 排除面的减数：{with_union:?}"
    );
    let bare_union = mesh_forced_route_cidrs(&servers, &ObservedTailnetAddresses::new());
    assert!(
        bare_union.contains(&TAILNET_CGNAT.to_string()),
        "无观测时默认段没进来 —— bootstrap 腿断了：{bare_union:?}"
    );

    // 对照组（无观测）：默认段把这条无关声明整条吞掉 + warn 响。
    let (without, warned_without) = tun_exclude_of(&config, ObservedTailnetAddresses::new());
    assert!(
        !without.contains(&"100.80.0.0/16".to_string()),
        "无观测时 100.80.0.0/16 本该被默认段 {TAILNET_CGNAT} 整条吞掉（本条是对照组，\
         它绿才说明下面实验组的「留下来」是观测段带来的）。实际：{without:?}"
    );
    assert!(warned_without, "被组网优先丢弃却没有 warn —— 用户零线索");

    // 实验组（有观测）：组网段只剩 `100.64.5.5/32`，与该声明不相交 ⇒ 原样保留、不 warn。
    let (with, warned_with) = tun_exclude_of(&config, observed(&[("ts1", &["100.64.5.5"])]));
    assert!(
        with.contains(&"100.80.0.0/16".to_string()),
        "有观测时 100.80.0.0/16 仍被丢弃 —— TUN 排除面还在拿默认段 {TAILNET_CGNAT} 当减数，\
         用户填的段被一个根本不会发射的段架空。实际：{with:?}"
    );
    assert!(
        !warned_with,
        "该声明并没有被丢弃，却报了组网重叠 warn —— 诊断行与实际行为分叉。捕获：{:?}",
        take_captured()
    );
}

// ============================================================================
// 消费面 ④：mesh_cidrs 供给面（tunnel_conflict 的输入）
// ============================================================================

/// `builder::tunnel_conflict` 的 `ConflictInput::mesh_cidrs` 由 [`mesh_forced_route_cidrs`] 供段。
/// 该模块本身在生产里**还没有调用点**（全仓只有它自己的单测调 `detect_tunnel_conflicts`），
/// 故这里只能钉到"供给面已带上观测地址"为止；接线那一步归后续批次。
#[test]
fn observed_addresses_reach_mesh_cidrs_union() {
    let servers = [ts_node("ts1")];
    let obs = observed(&[("ts1", &[OBSERVED_V4])]);
    let union = mesh_forced_route_cidrs(&servers, &obs);
    assert!(
        union.contains(&format!("{OBSERVED_V4}/32")),
        "mesh 段并集里没有观测地址 ⇒ 外来隧道冲突判定（MeshOverlap）看不见自建 tailnet 的真实\
         前缀，用户那条 32.0.0.0/24 的本机隧道不会被判成「两个声索人」。实际：{union:?}"
    );
    // 反向对照：去掉观测 ⇒ 并集里没有它。
    let bare = mesh_forced_route_cidrs(&servers, &ObservedTailnetAddresses::new());
    assert!(
        !bare.iter().any(|c| c.starts_with("32.0.0.")),
        "无观测时并集里不该出现 32.0.0.x。实际：{bare:?}"
    );
}

// ============================================================================
// 外化腿（rule_set）与降级腿
// ============================================================================

/// 降级腿：tailnet 文件不存在 ⇒ 与本腿出现之前逐字节相同。
///
/// 自证的那一半：两条"文件都不存在"的路径（存在但空的目录 / 压根不存在的目录）产出逐字节相等，
/// 且产出里不含任何 `tailnet-` tag。与"改动前"的逐字节比对由冻结金样门
/// `golden_config_snapshot.rs` 持有（见本文件头注）。
#[test]
fn missing_tailnet_file_falls_back_to_inline_unchanged() {
    let temp = TempDir::new().expect("建临时目录");
    let existing_but_empty = temp.path().display().to_string();

    let a = generate(
        &config_with(vec![ts_node("ts1")]),
        &existing_but_empty,
        ObservedTailnetAddresses::new(),
    );
    let b = generate(
        &config_with(vec![ts_node("ts1")]),
        "/nonexistent/tailnet-rules",
        ObservedTailnetAddresses::new(),
    );
    let ja = serde_json::to_string(&a).expect("序列化 a");
    let jb = serde_json::to_string(&b).expect("序列化 b");
    assert_eq!(
        ja, jb,
        "「目录存在但没有该节点的文件」与「目录压根不存在」必须走同一条降级腿、产出逐字节相同"
    );
    assert!(
        !ja.contains("tailnet-"),
        "文件不存在时产出里出现了 tailnet rule_set —— 降级腿没做干净。产出：{ja}"
    );
    assert!(
        inline_force_route(rules_of(&a), "ts1").is_some()
            && rule_set_force_route(rules_of(&a), "ts1").is_none(),
        "文件不存在时必须只有 inline 腿"
    );
}

/// 外化腿：文件存在 ⇒ 注册 `type:"local"` 定义 + 发 `{rule_set}` 规则，且**不再**发 inline 规则。
///
/// 同时钉两件事：
///  · 是 **per-node** 决定，不是全局开关（ts2 没有文件 ⇒ 照走 inline）；
///  · 走外化腿的节点**不参与** `claimed_cidrs` 跨节点去重 ⇒ ts2 仍拿到完整的两条常量段。
///    这是预期（不同 tailnet 的地址空间本就不相交），也说明 `force_route_conflicts` 计数
///    不会因为本腿出现而多计/少计。
#[test]
fn existing_tailnet_file_switches_to_local_rule_set() {
    let temp = TempDir::new().expect("建临时目录");
    let dir = temp.path();
    let base = tailnet_rule_file_base("ts1");
    assert_eq!(
        base, "tailnet-ts1",
        "文件名 base 派生漂了，下面的路径断言就对不上"
    );
    let path = dir.join(format!("{base}.json"));
    std::fs::write(
        &path,
        r#"{"version":1,"rules":[{"ip_cidr":["32.0.0.28/32","32.0.0.29/32"]}]}"#,
    )
    .expect("写 tailnet 文件");

    let cfg = generate(
        &config_with(vec![ts_node("ts1"), ts_node("ts2")]),
        &dir.display().to_string(),
        ObservedTailnetAddresses::new(),
    );
    let rules = rules_of(&cfg);

    // 定义已注册，且形态与 custom_rules 的 L3 外化腿同款。
    let defs = cfg
        .route
        .as_ref()
        .unwrap()
        .rule_set
        .as_ref()
        .expect("没有 rule_set 定义段");
    let def = defs.iter().find(|d| d.tag == base).unwrap_or_else(|| {
        panic!(
            "没注册 {base} 的 rule_set 定义。已有：{:?}",
            defs.iter().map(|d| &d.tag).collect::<Vec<_>>()
        )
    });
    assert_eq!(def.type_field, "local", "必须是 local（非 remote）");
    assert_eq!(
        def.format, "source",
        "必须是 headless JSON source（非 binary srs）"
    );
    assert_eq!(
        def.path.as_deref(),
        Some(path.display().to_string().as_str()),
        "定义里的 path 与实际落盘路径不一致 ⇒ NewLocalRuleSet 首次 reloadFile 会失败、整个核起不来"
    );

    // ts1 走外化腿：有 rule_set 规则、没有 inline 规则。
    let ts1_rs = rule_set_force_route(rules, "ts1").expect("ts1 没发 rule_set force-route 规则");
    assert_eq!(
        ts1_rs.rule_set.as_ref().unwrap(),
        &OneOrMany::One(base.clone()),
        "rule_set 引用形状不对"
    );
    assert_eq!(ts1_rs.action.as_deref(), Some("route"));
    assert!(
        inline_force_route(rules, "ts1").is_none(),
        "ts1 同时发了 inline 与 rule_set 两条规则 —— 两条都指向同一个 outbound，后一条是死规则"
    );

    // ts2 没有文件 ⇒ 照走 inline，且拿到完整两段（证明 ts1 没有消耗 claimed_cidrs）。
    let ts2_inline = inline_force_route(rules, "ts2").expect("ts2 没有文件，应走 inline 腿");
    assert_eq!(
        ts2_inline.ip_cidr.as_ref().unwrap(),
        &vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()],
        "走外化腿的 ts1 不该参与 claimed_cidrs 去重，ts2 应仍拿到完整两段"
    );
}

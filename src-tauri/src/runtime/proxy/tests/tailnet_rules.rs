//! 🔴 **A-0b：观测到的 tailnet 地址，到底有没有变成盘上真值、并进到内核吃的那两个面。**
//!
//! # 背景（2026-09-11 真控制面实测）
//!
//! 自建 headscale 的 `prefixes.v4` 可自定义，实测把地址发成 `32.0.0.28`，而
//! `endpoint_routes::TAILNET_CGNAT` 硬编码 `100.64.0.0/10` —— 该节点的 v4 force-route 规则一条
//! 流量都命中不了，`32.0.0.x` 落 `final` 去了 IANA 分给 AT&T 的公网段。A-0a 把发射面改完了
//! （四个消费者），但真值源恒空；本组门验的就是 A-0b 补上的那个真值源。
//!
//! # 两个消费面都要验（只验一个治不了用户的症状）
//!
//! | 面 | 真值通道 | 本文件的哪条 |
//! |---|---|---|
//! | 块 0c 规则发射 | 盘上 `tailnet-<id>.json`（`rule_set` 引用） | [`observed_frame_reaches_file_route_rules_and_tun_exclusion`] |
//! | TUN 排除面 `engaged_mesh` | `GenerateConfigDeps::observed_tailnet_addresses`（**不读文件**） | 同上（后半段） |
//!
//! 这两条通道是**分开的**：文件只服务规则发射，TUN 排除面只吃内存 map。任何一条断了，用户的
//! 原始症状（TUN 把 `32.0.0.x` 吞掉 / 吞进来了却没规则送去 endpoint）就还在。

use super::*;

use polaris_config_engine::builder::endpoint_routes::{
    tailnet_rule_file_base, TAILNET_CGNAT, TAILNET_MAGICDNS_V4, TAILNET_ULA_V6,
};
use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::LogLevel;
use std::collections::BTreeMap;

use super::super::tailnet_rules::{
    bare_addr_of_host_cidr, observed_addresses_of_event, parse_tailnet_rule_file_cidrs,
};
use crate::runtime::tailscale_status::{
    TailscalePeerDetails, TailscaleStatusDetails, TailscaleStatusEvent, TailscaleStatusPeer,
    TailscaleUserGroupStatus,
};

/// 2026-09-11 真控制面实测值（刻意不编一个假的：`32.0.0.0/8` 是 IANA 分给 AT&T 的公网段，
/// 它本身就是「落 final 会去到别人地址空间」这件事的证据）。
const OBSERVED_V4: &str = "32.0.0.28";
/// 同一次实测的 v6 侧 —— v6 前缀仍是默认 `fd7a:115c:a1e0::/48`，故它**本就被默认段覆盖**。
/// 留着它是为了证明观测面是并集、不是替换。
const OBSERVED_V6: &str = "fd7a:115c:a1e0::1c";
/// 一台**离线** peer 的地址：`online: bool` 存在即证明离线 peer 仍在 netmap 里，漏掉它们会让
/// 那台机器上线的那一刻路由不到（而那正是用户会去按的时候）。
const OBSERVED_PEER_V4: &str = "32.0.0.29";

fn no_log(_: LogLevel, _: &str) {}

fn peer(host: &str, ips: &[&str], online: bool) -> TailscaleStatusPeer {
    TailscaleStatusPeer {
        host_name: host.into(),
        ip: (*ips.first().unwrap_or(&"")).into(),
        online,
        exit_node: false,
        exit_node_option: false,
        active: false,
        stable_id: None,
        details: TailscalePeerDetails {
            tailscale_ips: ips.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        },
    }
}

/// 一帧全量端点快照（`peers` 与 `details.user_groups[].peers` 同源，与真解码一致）。
fn frame(
    server_id: &str,
    self_ips: &[&str],
    peers: Vec<TailscaleStatusPeer>,
) -> TailscaleStatusEvent {
    TailscaleStatusEvent {
        server_id: server_id.into(),
        backend_state: "Running".into(),
        logged_in: true,
        auth_url: None,
        tailscale_ips: self_ips.iter().map(|s| (*s).to_string()).collect(),
        expired: false,
        peers: peers.clone(),
        details: TailscaleStatusDetails {
            user_groups: vec![TailscaleUserGroupStatus {
                peers,
                ..Default::default()
            }],
            ..Default::default()
        },
        can_share_files: false,
        waiting_file_count: 0,
        receiving_file_count: 0,
        unread_file_count: 0,
    }
}

/// 一个 engaged 的 TS 节点（`alwaysRouteSubnets` 缺省 true）+ 用户在「连入来源排除」里声明了
/// 一段覆盖自建 tailnet 的网段 —— 那正是这次事故里外部给的排查建议。
fn ts_config_json() -> serde_json::Value {
    serde_json::json!({
        "servers": [
            { "id": "ts1", "name": "ts1", "protocol": "tailscale", "tailscaleSettings": {} }
        ],
        "selectedServerId": "__direct__",
        "proxyMode": "smart",
        "proxyModeType": "tun",
        "tunConfig": { "inboundExcludeCidrs": ["32.0.0.0/24"] }
    })
}

pub(super) async fn prepare_tailnet_fixture(rt: &Arc<ProxyRuntime>, config: &UserConfig) {
    let raw = serde_json::to_value(config).unwrap();
    rt.write_tailnet_rule_files_for_generation(config, &raw, rt.gate.generation())
        .await
        .unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated = generate_sing_box_config(config, &BTreeMap::new(), &deps).unwrap();
    let bindings = rt.tailnet_bindings(&raw, &generated);
    let _publication = rt.gate.lock_generation_publication();
    rt.activate_tailnet_context(rt.gate.generation(), bindings);
}

pub(super) fn sync_fixture(rt: &Arc<ProxyRuntime>, events: &[TailscaleStatusEvent]) {
    assert!(apply_fixture(rt, events));
}

fn apply_fixture(rt: &Arc<ProxyRuntime>, events: &[TailscaleStatusEvent]) -> bool {
    use polaris_singbox_grpc::daemon as dm;
    let context = rt
        .tailnet_context()
        .expect("real emitted context must be prepared");
    let tags = context.tag_to_id();
    let peer = |peer: &TailscaleStatusPeer| dm::TailscalePeer {
        host_name: peer.host_name.clone(),
        online: peer.online,
        stable_id: peer.stable_id.clone().unwrap_or_default(),
        tailscale_i_ps: peer.details.tailscale_ips.clone(),
        ..Default::default()
    };
    let update = dm::TailscaleStatusUpdate {
        endpoints: events
            .iter()
            .map(|event| dm::TailscaleEndpointStatus {
                endpoint_tag: tags
                    .iter()
                    .find_map(|(tag, id)| (id == &event.server_id).then(|| tag.clone()))
                    .unwrap_or_else(|| "unknown-tag".into()),
                backend_state: event.backend_state.clone(),
                self_: Some(dm::TailscalePeer {
                    tailscale_i_ps: event.tailscale_ips.clone(),
                    stable_id: event
                        .details
                        .self_peer
                        .as_ref()
                        .and_then(|peer| peer.stable_id.clone())
                        .unwrap_or_default(),
                    ..Default::default()
                }),
                user_groups: event
                    .details
                    .user_groups
                    .iter()
                    .map(|group| dm::TailscaleUserGroup {
                        peers: group.peers.iter().map(&peer).collect(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .collect(),
    };
    rt.apply_ts_status_frame(&update, &tags, context.generation, context.token)
}

fn ts_user_config() -> UserConfig {
    serde_json::from_value(ts_config_json()).expect("测试 config 反序列化失败")
}

fn tailnet_file(rt: &Arc<ProxyRuntime>, id: &str) -> std::path::PathBuf {
    rt.tailnet_rules_dir()
        .join(format!("{}.json", tailnet_rule_file_base(id)))
}

/// 本平台无关的 TUN 排除面读回：`platform` 固定 darwin（linux 的 `route_exclude` 恒空，
/// 在本机跑会让断言没有对象），**观测面取自生产接线** `generate_deps` 的产物。
fn tun_exclude_with(
    config: &UserConfig,
    observed: polaris_config_engine::builder::endpoint_routes::ObservedTailnetAddresses,
) -> Vec<String> {
    let deps = InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: vec![],
        platform: "darwin".into(),
        own_lan_cidrs: vec![],
        log: no_log,
        observed_tailnet_addresses: observed,
    };
    build_inbounds(config, None, &deps)
        .iter()
        .find(|i| i.type_field == "tun")
        .and_then(|t| t.route_exclude_address.clone())
        .unwrap_or_default()
}

// ══════════════ 判据 ①：端到端（帧 → 文件 → 规则发射 + TUN 排除面）══════════════

/// 🔴 喂一帧含 `32.0.0.28` / `fd7a:115c:a1e0::1c` 的 STATUS ⇒ 文件含这两条 ⇒ 再跑 generate ⇒
/// 块 0c 的 force-route **与 TUN 排除面**都看得见它们。
///
/// **变异锁（判据 ⑤ 的对象）**：
/// - 摘掉 `generate_deps` 里 `observed_tailnet_addresses: self.observed_tailnet_snapshot()`
///   （改回 `Default::default()`）→ 后半段 TUN 排除面断言转红；
/// - 摘掉 `apply_ts_status_frame` 里的 `sync_tailnet_rule_files` → 文件断言转红；
/// - 摘掉 `start_inner` 里的 `write_tailnet_rule_files` → 见
///   [`start_lands_tailnet_rule_files_before_generate`]。
#[tokio::test]
async fn observed_frame_reaches_file_route_rules_and_tun_exclusion() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();

    // 起核前无声明/观测：真正的空 rule-set，不猜官方控制面的大段。
    prepare_tailnet_fixture(&rt, &config).await;
    let path = tailnet_file(&rt, "ts1");
    let first = std::fs::read_to_string(&path).expect("起核前腿必须落一个文件出来");
    let first_cidrs =
        parse_tailnet_rule_file_cidrs(&first).expect("落盘内容必须是可解析的 rule-set");
    assert_eq!(
        first_cidrs,
        Vec::<String>::new(),
        "cold negative knowledge must not synthesize official prefixes"
    );
    let first_json: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first_json["version"], 1);
    assert_eq!(first_json["rules"], serde_json::json!([]));

    // ── 运行中腿：一帧全量快照（self + 一台**离线** peer）。
    sync_fixture(
        &rt,
        &[frame(
            "ts1",
            &[OBSERVED_V4, OBSERVED_V6],
            vec![peer("peer-a", &[OBSERVED_PEER_V4], false)],
        )],
    );
    let after = std::fs::read_to_string(&path).expect("文件不得在热更里消失");
    let cidrs = parse_tailnet_rule_file_cidrs(&after).expect("热更后仍须是可解析的 rule-set");
    for expect in [
        format!("{OBSERVED_V4}/32"),
        format!("{OBSERVED_V6}/128"),
        format!("{OBSERVED_PEER_V4}/32"),
    ] {
        assert!(
            cidrs.contains(&expect),
            "观测地址 {expect} 没进盘上 rule-set（含离线 peer 那条）。实际：{cidrs:?}"
        );
    }
    assert!(
        !cidrs.contains(&TAILNET_CGNAT.to_string()) && !cidrs.contains(&TAILNET_ULA_V6.to_string()),
        "hot canonical must retire implicit broad while retaining explicit declarations: {cidrs:?}"
    );

    // ── 消费面 ①：真跑 generate（deps 取自生产装配 `generate_deps`，不是手拼）。
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json(), false);
    assert!(
        deps.observed_tailnet_addresses
            .get("ts1")
            .is_some_and(|v| v.iter().any(|a| a == OBSERVED_V4)),
        "`generate_deps` 没把观测面注进去 —— TUN 排除面唯一的入口就此断掉。实际：{:?}",
        deps.observed_tailnet_addresses
    );
    let cfg = generate_sing_box_config(&config, &BTreeMap::new(), &deps).expect("生成配置");
    let route = cfg.route.as_ref().expect("没有 route 段");
    let tag = tailnet_rule_file_base("ts1");
    let declared = route
        .rule_set
        .as_ref()
        .expect("文件已落盘，块 0c 必须注册 rule_set")
        .iter()
        .find(|rs| rs.tag == tag)
        .expect("产物里没有该节点的 tailnet rule_set 声明");
    assert_eq!(
        declared.path.as_deref().map(std::path::Path::new),
        Some(path.as_path()),
        "rule_set 指向的路径必须就是落盘侧写的那个文件（两边分家 = 该腿 100% 不可达）"
    );
    assert!(
        route.rules.iter().any(|r| {
            r.outbound.as_deref() == Some("ts1")
                && matches!(&r.rule_set, Some(polaris_config_engine::singbox::OneOrMany::One(t)) if t == &tag)
        }),
        "产物里没有一条把该 rule_set 送去 ts1 的 force-route 规则"
    );

    // ── 消费面 ②：TUN 排除面（**用户原始症状的直接判据**）。
    // `engaged_mesh` 是减法的减数：与组网段相交的用户声明段被整条丢弃。观测面看得见 ⇒
    // `32.0.0.0/24` 不再进 route_exclude_address ⇒ 流量能到 sing-box，上面那条规则才有对象。
    let exclude = tun_exclude_with(&config, deps.observed_tailnet_addresses.clone());
    assert!(
        !exclude.iter().any(|c| c == "32.0.0.0/24"),
        "TUN 排除面没看见观测到的 tailnet 地址：`32.0.0.0/24` 仍被排出 TUN ⇒ `32.0.0.x` 根本到不了 \
         sing-box，块 0c 那条规则一辈子见不到它。实际排除面：{exclude:?}"
    );
    // 正向对照：同一份配置、观测面为空 ⇒ 那条段**在**排除面里（证明上面的断言不是恒真）。
    let without = tun_exclude_with(&config, Default::default());
    assert!(
        without.iter().any(|c| c == "32.0.0.0/24"),
        "对照组失效：无观测时 `32.0.0.0/24` 本应进排除面，实际 {without:?} —— \
         上面那条断言没有区分力"
    );
}

// ══════════════ 判据 ②：空帧不清空 ══════════════

/// 🔴 收到不含任何地址的帧（核刚起、netmap 还没同步）⇒ 文件**逐字节不变**。
///
/// 清空 = 该节点 tailnet 全断，比「更新慢一拍」危险得多。变异锁：把
/// `sync_tailnet_rule_files` 里 `if addrs.is_empty() { continue; }` 删掉（改成按本帧重写）→ 转红。
#[tokio::test]
async fn empty_frame_never_shrinks_the_file() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    prepare_tailnet_fixture(&rt, &config).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let path = tailnet_file(&rt, "ts1");
    let before = std::fs::read_to_string(&path).expect("热更后文件应在");
    assert!(
        before.contains(&format!("{OBSERVED_V4}/32")),
        "前置没建立起来"
    );

    // 空帧：self 无地址、无 peer（真机上核刚起、netmap 未同步时就是这个形态）。
    sync_fixture(&rt, &[frame("ts1", &[], vec![])]);
    let after = std::fs::read_to_string(&path).expect("空帧不得让文件消失");
    assert_eq!(
        after, before,
        "空帧把已知观测冲掉了 —— 清空 = 整段 tailnet 不可路由"
    );

    // 全是非法字面量的帧同样什么都不做（写进 ip_cidr 会让核 netip.ParsePrefix 启动 FATAL）。
    sync_fixture(&rt, &[frame("ts1", &["not-an-ip", ""], vec![])]);
    assert_eq!(
        std::fs::read_to_string(&path).expect("文件应在"),
        before,
        "非法地址帧不得改写文件"
    );
}

// ══════════════ 判据 ③：运行中绝不删文件 ══════════════

/// 🔴 运行期热更腿在**任何**路径下都不得 unlink 已挂载的 tailnet 文件。
///
/// 行为断言只能覆盖被跑到的分支，故再加一条源码级：方法体里根本不存在 `remove_file`。
/// 二者成对 —— 前者证明今天的分支是对的，后者挡住将来有人在失败腿上加一句「清理一下」。
#[tokio::test]
async fn running_sync_never_unlinks_a_mounted_file() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    prepare_tailnet_fixture(&rt, &config).await;
    let path = tailnet_file(&rt, "ts1");

    // 依次走遍热更腿的每条早退：空帧 / 非法地址 / 同值 / 文件内容不可解析。
    sync_fixture(&rt, &[frame("ts1", &[], vec![])]);
    assert!(path.exists(), "空帧腿删了文件");
    sync_fixture(&rt, &[frame("ts1", &["bad"], vec![])]);
    assert!(path.exists(), "非法地址腿删了文件");
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(path.exists(), "首次并入删了文件");
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(path.exists(), "同值腿删了文件");
    std::fs::write(&path, "{ not json").expect("写坏文件");
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(
        path.exists(),
        "文件内容坏掉时热更腿删了它 —— 运行中删被挂载文件会致 sing-box reload 报错"
    );

    // 源码面：热更腿里不存在任何 unlink。
    let src = module_code("runtime/proxy");
    let body = method_body(
        &src,
        "    pub(super) fn commit_tailnet_events(&self, state: &mut TailnetRouteState, context: &TailnetRouteContext, events: &[TailscaleStatusEvent]) -> Result<(), String> {",
    );
    assert!(
        !body.is_empty(),
        "取材失败：`sync_tailnet_rule_files` 的签名行变了，本守卫已经在扫空字符串"
    );
    assert!(
        !body.contains("remove_file") && !body.contains("remove_dir"),
        "运行期热更腿里出现了删除调用。删除只允许发生在起核前（核未起、文件未挂载）。体：\n{body}"
    );
}

/// 文件不存在 ⇒ 热更腿**不创建**。
///
/// 核跑的是起核那一刻生成的 config：那份 config 里要么有这个 `rule_set` 引用，要么没有。
/// 缺席说明起核前腿失败了、块 0c 已回落 inline，此刻造一个文件核根本不会读；而造出来的内容
/// 只有观测段、缺配置期段，下次起核前腿若又没跑到，它就是一份**缺默认段**的 rule-set。
#[test]
fn running_sync_does_not_create_missing_files() {
    let (rt, _dir) = test_runtime();
    // Admit the real generated endpoint without running file preparation.
    let raw = ts_config_json();
    let config = ts_user_config();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated = generate_sing_box_config(&config, &BTreeMap::new(), &deps).unwrap();
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(rt.gate.generation(), rt.tailnet_bindings(&raw, &generated));
    }
    assert!(!tailnet_file(&rt, "ts1").exists());
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(
        !tailnet_file(&rt, "ts1").exists(),
        "热更腿凭空造了一个缺默认段的 rule-set 文件"
    );
    // 但内存观测面照收（下次起核的 TUN 排除面要用它）。
    assert!(
        rt.observed_tailnet_snapshot()
            .get("ts1")
            .is_some_and(|v| v.iter().any(|a| a == OBSERVED_V4)),
        "文件没落，内存观测面也丢了 —— TUN 排除面会跟着瞎"
    );
}

// ══════════════ 判据 ④：降级腿仍在（写不了 ⇒ 回落 inline，不 FATAL）══════════════

/// 🔴 落盘失败（目录建不出来）⇒ 块 0c 回落 inline，产出与 A-0a 之前**同义**，不 panic、不 FATAL。
///
/// 失败注入取「把 `<configDir>/tailnet-rules` 占成一个普通文件」——`create_dir_all` 必失败，
/// 且不依赖文件权限（跨平台、跑测用户是不是 root 都一样）。
#[tokio::test]
async fn write_failure_falls_back_to_inline_without_fatal() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    let dir = rt.tailnet_rules_dir();
    std::fs::create_dir_all(dir.parent().expect("configDir")).expect("建 configDir");
    std::fs::write(&dir, "占位：这不是目录").expect("把 tailnet-rules 占成文件");

    prepare_tailnet_fixture(&rt, &config).await; // 不得 panic
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]); // 不得 panic

    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json(), false);
    let cfg = generate_sing_box_config(&config, &BTreeMap::new(), &deps).expect("生成配置");
    let route = cfg.route.as_ref().expect("没有 route 段");
    assert!(
        route.rule_set.as_ref().is_none_or(|rs| !rs
            .iter()
            .any(|entry| entry.tag == tailnet_rule_file_base("ts1"))),
        "文件没落盘却注册了 rule_set —— `NewLocalRuleSet` 首次 reloadFile 会失败，整个核起不来"
    );
    let inline = route
        .rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some("ts1") && r.ip_cidr.is_some())
        .expect("降级腿必须回落 inline force-route");
    let cidrs = inline.ip_cidr.as_ref().expect("ip_cidr");
    assert!(
        cidrs.contains(&TAILNET_MAGICDNS_V4.to_string()),
        "回落 inline 也不能丢 MagicDNS 服务地址（协议常量，不随观测面取代）。实际：{cidrs:?}"
    );
    assert!(
        !cidrs.contains(&TAILNET_CGNAT.to_string()),
        "本节点有观测 ⇒ 降级腿也走「观测取代默认段」，不该退回默认段。\
         无观测节点的 bootstrap 腿另由 `write_failure_on_a_node_without_observation_keeps_bootstrap` 覆盖。\
         实际：{cidrs:?}"
    );
    // 观测面仍经内存注入（热更腿即便写不了文件也收了帧）⇒ inline 腿照样带上它。
    assert!(
        cidrs.contains(&format!("{OBSERVED_V4}/32")),
        "降级不等于丢观测：inline 腿吃的是 deps 的内存观测面。实际：{cidrs:?}"
    );
}

/// 有观测时的 inline 回落与无观测时的 empty 回落必须分别覆盖。
/// 保留这条失败路径：缺文件也不能把未知冷态升级为官方大段或空条件规则。
#[tokio::test]
async fn write_failure_on_a_node_without_observation_keeps_bootstrap() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    let dir = rt.tailnet_rules_dir();
    std::fs::create_dir_all(dir.parent().expect("configDir")).expect("建 configDir");
    std::fs::write(&dir, "占位：这不是目录").expect("把 tailnet-rules 占成文件");

    prepare_tailnet_fixture(&rt, &config).await; // 不得 panic
                                                 // 刻意不喂任何 status 帧：这就是「节点从未连上过控制面」的形态。
    assert!(
        rt.observed_tailnet_snapshot().is_empty(),
        "前置：本例的观测面必须是空的，否则测的不是 bootstrap 腿"
    );

    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json(), false);
    let cfg = generate_sing_box_config(&config, &BTreeMap::new(), &deps).expect("生成配置");
    let route = cfg.route.as_ref().expect("没有 route 段");
    assert!(
        !route
            .rules
            .iter()
            .any(|r| r.outbound.as_deref() == Some("ts1") && r.ip_cidr.is_some()),
        "missing file and unknown hosts must not create an inline force-route"
    );
    assert!(route.rule_set.as_ref().is_none_or(|rules| !rules
        .iter()
        .any(|entry| entry.tag == tailnet_rule_file_base("ts1"))));
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
}

// ══════════════ 持久层：盘上文件活过进程重启 ══════════════

/// 🔴 观测面唯一的持久层就是那个 JSON 文件：换一个 `ProxyRuntime`（= 模拟 app 重启）、
/// 内存里一片空白，起核前腿必须把上次已知集合读回来。
///
/// 没有这条，冷启动到收到首帧之间的那个窗口里，TUN 排除面看不见自建 tailnet 的地址 ——
/// 正是用户报障的那个形态。
#[tokio::test]
async fn last_known_set_survives_a_process_restart() {
    let dir = fresh_test_dir();
    let config = ts_user_config();
    {
        let rt = test_runtime_in(dir.clone());
        prepare_tailnet_fixture(&rt, &config).await;
        sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4, OBSERVED_V6], vec![])]);
    }
    let restarted = test_runtime_in(dir.clone());
    assert!(
        restarted.observed_tailnet_snapshot().is_empty(),
        "前置：新实例的内存观测面本应是空的（否则下面的断言没有信息量）"
    );
    prepare_tailnet_fixture(&restarted, &config).await;
    let observed = restarted.observed_tailnet_snapshot();
    assert!(
        observed
            .get("ts1")
            .is_some_and(|v| v.iter().any(|a| a == OBSERVED_V4)),
        "上次会话的观测面没从盘上读回来 ⇒ 冷启动期 TUN 排除面对自建 tailnet 瞎。实际：{observed:?}"
    );
    // 配置期段每次重算、观测段靠盘读回。语义是「有观测则取代默认段」（见
    // `endpoint_routes::ObservedTailnetAddresses` 的「# 语义」一节），所以重写后的文件应当是
    // 观测段 + 两条 MagicDNS 协议常量，**不含**会漂移的默认段。
    let cidrs = parse_tailnet_rule_file_cidrs(
        &std::fs::read_to_string(tailnet_file(&restarted, "ts1")).expect("文件应在"),
    )
    .expect("可解析");
    assert!(
        cidrs.contains(&format!("{OBSERVED_V4}/32")),
        "重启后重写的文件丢了上次会话的观测段。实际：{cidrs:?}"
    );
    assert!(
        cidrs.contains(&TAILNET_MAGICDNS_V4.to_string()),
        "MagicDNS 服务地址是协议常量、不随观测面取代（它不是 peer，永远不进 netmap）。实际：{cidrs:?}"
    );
    assert!(
        !cidrs.contains(&TAILNET_CGNAT.to_string()),
        "有观测的节点不该再发会漂移的默认段 —— 两个 tailnet 会撞在同样的两段上、后声明者被去重吸收。\
         实际：{cidrs:?}"
    );
}

/// 上一次会话留下的**坏文件**必须在起核前被修好 —— 否则 `NewLocalRuleSet` 首次 reloadFile
/// 失败，整个核起不来（这是唯一的修复点：热更腿按禁令不碰它）。
#[tokio::test]
async fn corrupt_file_is_rewritten_before_start() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    std::fs::create_dir_all(rt.tailnet_rules_dir()).expect("建目录");
    let path = tailnet_file(&rt, "ts1");
    std::fs::write(&path, "{ 这不是 JSON").expect("写坏文件");

    prepare_tailnet_fixture(&rt, &config).await;

    let cidrs = parse_tailnet_rule_file_cidrs(&std::fs::read_to_string(&path).expect("文件应在"))
        .expect("起核前腿必须把坏文件重写成可解析的 rule-set");
    assert_eq!(
        cidrs,
        Vec::<String>::new(),
        "坏文件应被按配置期段重写。实际：{cidrs:?}"
    );
    let repaired: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(repaired["version"], 1);
    assert_eq!(repaired["rules"], serde_json::json!([]));
}

// ══════════════ 接线门：start 路径真的会落盘 ══════════════

/// wiring 门：`start` 在 generate 之前真调 `write_tailnet_rule_files`。
///
/// 形态照搬 [`start_lands_custom_rule_files_before_generate`]：用 `POLARIS_SINGBOX_PATH`→目录
/// 逼 `resolve_core_binary` 失败（不起真核、不碰网络），此刻 tailnet 文件已落盘。
/// **变异锁**：删 `start_inner` 里那一行调用 → 文件不落 → 转红。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn start_lands_tailnet_rule_files_before_generate() {
    let (rt, dir) = test_runtime();
    rt.stale_sweep_disabled.store(true, Ordering::SeqCst);
    // **刻意不用 `ts_config_json()`（那份是 `proxyModeType: "tun"`）**：TUN 起核要先过提权
    // helper 门，而测试实例的 helper 恒「未装」⇒ 在落盘腿之前就 bail，本门会变成假绿的
    // 「文件确实没落，但根本没跑到那一步」。manual 模式直达 `resolve_core_binary`。
    let mut cfg = ts_config_json();
    cfg["proxyModeType"] = serde_json::json!("manual");
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("POLARIS_SINGBOX_PATH", &*dir);
    let r = rt.start(cfg).await;
    std::env::remove_var("POLARIS_SINGBOX_PATH");
    drop(_g);
    assert!(
        r.is_err(),
        "核二进制解析失败 → 起核失败（但 tailnet 文件已落盘）"
    );
    assert!(
        tailnet_file(&rt, "ts1").exists(),
        "start 路径须在 generate 前落盘 tailnet rule-set（未接线则文件不存在 ⇒ 块 0c 恒回落 inline）"
    );
}

/// wiring 门：**relay 每帧处理**（`apply_ts_status_frame`）真的会驱动热更腿。
///
/// 上面那些用例都直调 `sync_tailnet_rule_files`，测的是那条腿本身；**接线**是另一件事 ——
/// 首版实测：把 `apply_ts_status_frame` 里那行调用删掉，上面 11 条全绿（函数被测 ≠ 生产在用它）。
/// 故这条从真入口（proto 帧 + tag→id 逆映射）走一遍。
///
/// **变异锁**：删 `apply_ts_status_frame` 里的 `self.sync_tailnet_rule_files(&events);` → 转红。
#[tokio::test]
async fn relay_frame_wiring_drives_the_hot_update() {
    use polaris_singbox_grpc::daemon as dm;
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    prepare_tailnet_fixture(&rt, &config).await;
    let path = tailnet_file(&rt, "ts1");
    assert!(
        !std::fs::read_to_string(&path)
            .expect("起核前腿应落盘")
            .contains(OBSERVED_V4),
        "前置：此刻文件里还不该有观测地址（否则本门没有区分力）"
    );

    let context = rt.tailnet_context().unwrap();
    let tag_to_id = context.tag_to_id();
    let actual_tag = tag_to_id.keys().next().unwrap().clone();
    let update = dm::TailscaleStatusUpdate {
        endpoints: vec![dm::TailscaleEndpointStatus {
            endpoint_tag: actual_tag,
            backend_state: "Running".into(),
            self_: Some(dm::TailscalePeer {
                host_name: "self".into(),
                tailscale_i_ps: vec![OBSERVED_V4.into(), OBSERVED_V6.into()],
                ..Default::default()
            }),
            user_groups: vec![dm::TailscaleUserGroup {
                peers: vec![dm::TailscalePeer {
                    host_name: "peer-a".into(),
                    // **离线**：仍在 netmap 里，必须算进来。
                    online: false,
                    tailscale_i_ps: vec![OBSERVED_PEER_V4.into()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    assert!(rt.apply_ts_status_frame(&update, &tag_to_id, rt.gate.generation(), context.token));

    let cidrs = parse_tailnet_rule_file_cidrs(&std::fs::read_to_string(&path).expect("文件应在"))
        .expect("可解析");
    for expect in [
        format!("{OBSERVED_V4}/32"),
        format!("{OBSERVED_PEER_V4}/32"),
    ] {
        assert!(
            cidrs.contains(&expect),
            "relay 帧没有驱动热更腿（{expect} 没进文件）—— 接线断了，热更腿本身再对也没用。实际：{cidrs:?}"
        );
    }
    assert!(
        rt.observed_tailnet_snapshot()
            .get("ts1")
            .is_some_and(|v| v.iter().any(|a| a == OBSERVED_PEER_V4)),
        "内存观测面没跟上 ⇒ 下次起核的 TUN 排除面看不见这台离线 peer"
    );
}

// ══════════════ 纯函数 ══════════════

/// 地址集 = self ∪ 全部 peer，**含离线 peer**。
#[test]
fn observed_addresses_include_offline_peers() {
    let ev = frame(
        "ts1",
        &[OBSERVED_V4],
        vec![
            peer("offline", &[OBSERVED_PEER_V4], false),
            peer("online", &["32.0.0.30"], true),
        ],
    );
    let addrs = observed_addresses_of_event(&ev);
    for expect in [OBSERVED_V4, OBSERVED_PEER_V4, "32.0.0.30"] {
        assert!(
            addrs.contains(&expect.to_string()),
            "漏了 {expect}：{addrs:?}"
        );
    }
    // 顶层 `peers` 与 `details.user_groups[].peers` 同源 ⇒ 并集不得产生重复条目。
    let mut sorted = addrs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), addrs.len(), "地址集出现重复：{addrs:?}");
}

/// 只有主机位（`/32` / `/128`）才被当成观测面读回；配置期段每次重算，不许从盘上复活。
#[test]
fn only_host_cidrs_are_read_back_as_observations() {
    assert_eq!(bare_addr_of_host_cidr("32.0.0.28/32"), Some("32.0.0.28"));
    assert_eq!(
        bare_addr_of_host_cidr("fd7a:115c:a1e0::1c/128"),
        Some("fd7a:115c:a1e0::1c")
    );
    assert_eq!(bare_addr_of_host_cidr(TAILNET_CGNAT), None);
    assert_eq!(bare_addr_of_host_cidr(TAILNET_ULA_V6), None);
    assert_eq!(bare_addr_of_host_cidr("10.0.0.0/24"), None);
    assert_eq!(bare_addr_of_host_cidr("32.0.0.28"), None);
}

/// rule-set 解析：遍历全部 rule，坏内容 → None（由起核前腿重写，不留给内核去 FATAL）。
#[test]
fn rule_file_parse_reads_every_rule_and_rejects_garbage() {
    let parsed = parse_tailnet_rule_file_cidrs(
        r#"{"version":1,"rules":[{"ip_cidr":["1.1.1.1/32"]},{"ip_cidr":["2.2.2.2/32"]}]}"#,
    );
    assert_eq!(
        parsed,
        Some(vec!["1.1.1.1/32".to_string(), "2.2.2.2/32".to_string()])
    );
    assert_eq!(parse_tailnet_rule_file_cidrs("{ 坏"), None);
    assert_eq!(parse_tailnet_rule_file_cidrs("{}"), None);
    // 合法但没有 ip_cidr 的 rule-set → 空集（不是 None）：它可解析，只是没段。
    assert_eq!(
        parse_tailnet_rule_file_cidrs(r#"{"version":1,"rules":[{"domain":["a.com"]}]}"#),
        Some(vec![])
    );
}

// V01/V05: implicit provenance retires; explicit identical broad declarations
// survive, and removing a declaration cannot resurrect it from current file bytes.
#[tokio::test]
async fn canonical_hot_recompute_preserves_only_current_explicit_broad() {
    let (rt, _dir) = test_runtime();
    let mut raw = ts_config_json();
    raw["servers"][0]["tailscaleSettings"]["routes"] =
        serde_json::json!([TAILNET_CGNAT, TAILNET_ULA_V6, "10.8.0.0/24"]);
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    prepare_tailnet_fixture(&rt, &config).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let path = tailnet_file(&rt, "ts1");
    let cidrs = parse_tailnet_rule_file_cidrs(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(cidrs.contains(&TAILNET_CGNAT.into()) && cidrs.contains(&TAILNET_ULA_V6.into()));
    let before = rt.tailnet_context().unwrap();
    raw["servers"][0]["tailscaleSettings"]["routes"] = serde_json::json!([]);
    let typed = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated = generate_sing_box_config(&typed, &BTreeMap::new(), &deps).unwrap();
    {
        let _publication = rt.gate.lock_generation_publication();
        assert!(
            rt.activate_tailnet_context(before.generation, rt.tailnet_bindings(&raw, &generated))
        );
    }
    let after = rt.tailnet_context().unwrap();
    assert_ne!(before.token, after.token);
    let cidrs = parse_tailnet_rule_file_cidrs(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(!cidrs.contains(&TAILNET_CGNAT.into()) && !cidrs.contains(&TAILNET_ULA_V6.into()));
    assert!(!cidrs.contains(&"10.8.0.0/24".into()));
    assert!(cidrs.contains(&format!("{OBSERVED_V4}/32")));
}

// V06/V08/V09/V15: unknown disk history remains usable by next-start/TUN,
// but the first qualified current frame replaces it rather than certifying it.
#[tokio::test]
async fn weak_legacy_history_is_planning_only_and_first_frame_replaces_it() {
    let (rt, _dir) = test_runtime();
    let path = tailnet_file(&rt, "ts1");
    std::fs::create_dir_all(rt.tailnet_rules_dir()).unwrap();
    std::fs::write(
        &path,
        super::super::tailnet_rules::tailnet_rule_file_json(&["32.0.0.99/32".into()]),
    )
    .unwrap();
    let config = ts_user_config();
    prepare_tailnet_fixture(&rt, &config).await;
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
    assert_eq!(
        rt.observed_tailnet_for_config(&ts_config_json())["ts1"],
        vec!["32.0.0.99"]
    );
    let planned = tun_exclude_with(&config, rt.observed_tailnet_for_config(&ts_config_json()));
    assert!(!planned.contains(&"32.0.0.0/24".into()));
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert_eq!(rt.observed_tailnet_snapshot()["ts1"], vec![OBSERVED_V4]);
    assert!(!std::fs::read_to_string(path).unwrap().contains("32.0.0.99"));
    let mut different = ts_config_json();
    different["servers"][0]["tailscaleSettings"]["controlUrl"] =
        serde_json::json!("https://other.example");
    assert!(rt.observed_tailnet_for_config(&different).is_empty());
    // No new persistent provenance is fabricated: disk-only data in a new
    // process is weak again even though this process had a known authority.
    assert!(!rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
    rt.reset_ts_exit_block_state();
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
    assert_eq!(
        rt.observed_tailnet_for_config(&ts_config_json())["ts1"],
        vec![OBSERVED_V4]
    );
}

// V03/V10/V11/V12/V18: stale/adversarial frames cannot mutate any sink.
#[tokio::test]
async fn stale_generation_token_and_duplicate_tag_have_zero_commits() {
    use polaris_singbox_grpc::daemon as dm;
    let (rt, _dir) = test_runtime();
    prepare_tailnet_fixture(&rt, &ts_user_config()).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let context = rt.tailnet_context().unwrap();
    let tags = context.tag_to_id();
    let tag = tags.keys().next().unwrap().clone();
    let endpoint = dm::TailscaleEndpointStatus {
        endpoint_tag: tag,
        self_: Some(dm::TailscalePeer {
            tailscale_i_ps: vec!["32.0.0.77".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let update = dm::TailscaleStatusUpdate {
        endpoints: vec![endpoint.clone()],
    };
    let path = tailnet_file(&rt, "ts1");
    let bytes = std::fs::read(&path).unwrap();
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    let hosts = rt.observed_tailnet_snapshot();
    let cache = rt.mesh.tailscale_status_snapshot(true).statuses;
    assert!(!rt.apply_ts_status_frame(&update, &tags, context.generation, context.token + 1));
    let duplicated = dm::TailscaleStatusUpdate {
        endpoints: vec![endpoint.clone(), endpoint],
    };
    assert!(!rt.apply_ts_status_frame(&duplicated, &tags, context.generation, context.token));
    rt.bump_generation();
    assert!(!rt.apply_ts_status_frame(&update, &tags, context.generation, context.token));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    assert_eq!(rt.observed_tailnet_snapshot(), hosts);
    assert_eq!(rt.mesh.tailscale_status_snapshot(true).statuses, cache);
}

#[tokio::test]
async fn stale_prospective_start_cannot_write_or_import_legacy() {
    let (rt, _dir) = test_runtime();
    let generation = rt.gate.generation();
    rt.bump_generation();
    rt.write_tailnet_rule_files_for_generation(&ts_user_config(), &ts_config_json(), generation)
        .await
        .unwrap();
    assert!(!rt.tailnet_rules_dir().exists());
    assert!(rt.observed_tailnet_snapshot().is_empty());
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn known_authority_filters_previews_and_peeled_endpoints_have_no_writer() {
    let (rt, _dir) = test_runtime();
    prepare_tailnet_fixture(&rt, &ts_user_config()).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let mut raw = ts_config_json();
    raw["servers"][0]["tailscaleSettings"]["controlUrl"] =
        serde_json::json!("HTTPS://CONTROLPLANE.TAILSCALE.COM");
    assert!(rt.observed_tailnet_for_config(&raw).contains_key("ts1"));
    raw["servers"][0]["tailscaleSettings"]["controlUrl"] =
        serde_json::json!("https://controlplane.tailscale.com/other");
    assert!(rt.observed_tailnet_for_config(&raw).is_empty());
    let typed = ts_user_config();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json(), false);
    let mut generated = generate_sing_box_config(&typed, &BTreeMap::new(), &deps).unwrap();
    generated.endpoints = None;
    assert!(rt
        .tailnet_bindings(&ts_config_json(), &generated)
        .is_empty());
    let mut collision = ts_config_json();
    let duplicate = collision["servers"][0].clone();
    collision["servers"].as_array_mut().unwrap().push(duplicate);
    assert!(rt.tailnet_bindings(&collision, &generated).is_empty());
    assert!(rt.observed_tailnet_for_config(&collision).is_empty());
    let mut non_ts = ts_config_json();
    non_ts["servers"][0]["protocol"] = serde_json::json!("wireguard");
    assert!(rt.observed_tailnet_for_config(&non_ts).is_empty());
}

#[tokio::test]
async fn logical_epoch_retirement_and_actual_stable_id_change_do_not_mix_hosts() {
    let (rt, _dir) = test_runtime();
    let mut raw = ts_config_json();
    raw["meshRouteState"] = serde_json::json!({"schemaVersion":1,"localId":"local","revision":"1",
        "identities":[{"serverId":"ts1","identityEpoch":"epoch-one","controlAuthority":"https://controlplane.tailscale.com","selfStableId":"self-one","bindingState":"bound","evidenceSource":"existing-ledger"}],
        "observations":[],"reservations":[],"intent":{"revision":"1","desiredRun":"stopped"},"identityEffects":[]});
    let typed = serde_json::from_value(raw.clone()).unwrap();
    rt.write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
        .await
        .unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &raw, false);
    let generated = generate_sing_box_config(&typed, &BTreeMap::new(), &deps).unwrap();
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(rt.gate.generation(), rt.tailnet_bindings(&raw, &generated));
    }
    let mut event = frame("ts1", &[OBSERVED_V4], vec![]);
    let mut own = peer("self", &[OBSERVED_V4], true);
    own.stable_id = Some("self-one".into());
    event.details.self_peer = Some(own);
    sync_fixture(&rt, &[event.clone()]);
    assert!(rt.observed_tailnet_for_config(&raw).contains_key("ts1"));
    let mut changed_epoch = raw.clone();
    changed_epoch["meshRouteState"]["identities"][0]["identityEpoch"] =
        serde_json::json!("epoch-two");
    assert!(rt.observed_tailnet_for_config(&changed_epoch).is_empty());
    let mut retired = raw.clone();
    retired["meshRouteState"]["identities"][0]["bindingState"] = serde_json::json!("retired");
    assert!(rt.observed_tailnet_for_config(&retired).is_empty());
    // Absent stable ID and an empty/invalid frame cannot invent logout.
    let token = rt.tailnet_context().unwrap().token;
    sync_fixture(&rt, &[frame("ts1", &[], vec![])]);
    sync_fixture(&rt, &[frame("ts1", &["invalid"], vec![])]);
    assert_eq!(rt.tailnet_context().unwrap().token, token);
    assert!(rt.observed_tailnet_snapshot()["ts1"].contains(&OBSERVED_V4.into()));
    event.tailscale_ips = vec!["32.0.0.77".into()];
    event.details.self_peer.as_mut().unwrap().stable_id = Some("self-two".into());
    sync_fixture(&rt, &[event]);
    assert_ne!(rt.tailnet_context().unwrap().token, token);
    assert_eq!(rt.observed_tailnet_snapshot()["ts1"], vec!["32.0.0.77"]);
    assert!(rt.observed_tailnet_for_config(&raw).is_empty());
}

// Functional source admission has a byte budget, not the report's 4096-row cap.
#[tokio::test]
async fn functional_routes_above_report_cap_roundtrip_with_full_canonical_overhead() {
    let dir = fresh_test_dir();
    let rt = test_runtime_in(dir.clone());
    let mut raw = ts_config_json();
    raw["servers"][0]["tailscaleSettings"]["routes"] =
        serde_json::json!(["172.20.0.0/16", "fd11:22::/48"]);
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    prepare_tailnet_fixture(&rt, &config).await;
    for count in [4095usize, 4112] {
        let mut event = frame("ts1", &[], vec![]);
        event.tailscale_ips = vec!["invalid".into(); 4096];
        event.tailscale_ips.extend(vec!["10.0.0.1".into(); 4096]);
        let hosts: Vec<String> = (0..count)
            .map(|n| format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255))
            .collect();
        event.tailscale_ips.extend(hosts.iter().cloned());
        sync_fixture(&rt, &[event]);
        assert_eq!(rt.observed_tailnet_snapshot()["ts1"].len(), count);
        assert_eq!(
            rt.fresh_tailnet_snapshot(rt.gate.generation())["ts1"].len(),
            count
        );
        let expected = polaris_config_engine::builder::endpoint_routes::endpoint_forced_route_cidrs(
            &config.servers[0],
            &rt.observed_tailnet_snapshot(),
        );
        assert_eq!(expected.len(), count + 4); // MagicDNS + explicit v4/v6 routes.
        let content = std::fs::read_to_string(tailnet_file(&rt, "ts1")).unwrap();
        assert_eq!(
            content,
            super::super::tailnet_rules::tailnet_rule_file_json(&expected)
        );
        assert_eq!(parse_tailnet_rule_file_cidrs(&content), Some(expected));
        assert!(!rt.tailnet_routes_limited());
    }
    let before = std::fs::read(tailnet_file(&rt, "ts1")).unwrap();
    let restarted = test_runtime_in(dir.to_path_buf());
    prepare_tailnet_fixture(&restarted, &config).await;
    let retained = restarted.observed_tailnet_snapshot();
    assert!(retained["ts1"].len() > 4096);
    assert!(restarted
        .fresh_tailnet_snapshot(restarted.gate.generation())
        .is_empty());
    assert_eq!(
        std::fs::read(tailnet_file(&restarted, "ts1")).unwrap(),
        before
    );
    sync_fixture(&restarted, &[frame("ts1", &["10.1.1.1"], vec![])]);
    assert_eq!(
        restarted.observed_tailnet_snapshot()["ts1"],
        vec!["10.1.1.1"]
    );
    assert!(!std::fs::read_to_string(tailnet_file(&restarted, "ts1"))
        .unwrap()
        .contains("10.0.0.0/32"));
    assert_eq!(bare_addr_of_host_cidr("::1/32"), None);
    assert_eq!(bare_addr_of_host_cidr("10.0.0.1/128"), None);
    assert_eq!(bare_addr_of_host_cidr("not-an-ip/32"), None);
    assert!(
        parse_tailnet_rule_file_cidrs(r#"{"version":1,"rules":[{"ip_cidr":["invalid"]}]}"#)
            .is_none()
    );
}

#[tokio::test]
async fn functional_overflow_rejects_complete_frame_before_any_sink_advances() {
    use super::super::tailnet_rules::MAX_TAILNET_RULE_BYTES;
    let (rt, _dir) = test_runtime();
    prepare_tailnet_fixture(&rt, &ts_user_config()).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let bytes = std::fs::read(tailnet_file(&rt, "ts1")).unwrap();
    let hosts = rt.observed_tailnet_snapshot();
    let fresh = rt.fresh_tailnet_snapshot(rt.gate.generation());
    let status = rt.mesh.tailscale_status_snapshot(true).statuses;
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    let token = rt.tailnet_context().unwrap().token;
    for input_overflow in [false, true] {
        let mut event = frame("ts1", &[], vec![]);
        event.tailscale_ips = if input_overflow {
            vec!["invalid".repeat(MAX_TAILNET_RULE_BYTES / 7 + 1)]
        } else {
            (0..60000)
                .map(|n| format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255))
                .collect()
        };
        // A stable-ID learning token must also remain staged on rejection.
        event.details.self_peer = Some(peer("self", &[], true));
        event.details.self_peer.as_mut().unwrap().stable_id = Some("uncommitted-self".into());
        assert!(!apply_fixture(&rt, &[event]));
        assert!(rt.tailnet_routes_limited());
        assert_eq!(rt.tailnet_context().unwrap().token, token);
        assert_eq!(rt.observed_tailnet_snapshot(), hosts);
        assert_eq!(rt.fresh_tailnet_snapshot(rt.gate.generation()), fresh);
        assert_eq!(rt.mesh.tailscale_status_snapshot(true).statuses, status);
        assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
        assert_eq!(std::fs::read(tailnet_file(&rt, "ts1")).unwrap(), bytes);
    }
    sync_fixture(&rt, &[frame("ts1", &[], vec![])]);
    assert!(rt.tailnet_routes_limited()); // an empty frame cannot prove recovery.
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_PEER_V4], vec![])]);
    assert!(!rt.tailnet_routes_limited());
}

#[tokio::test]
async fn declaration_and_magicdns_byte_overflow_refuses_startup_before_import_or_write() {
    use super::super::tailnet_rules::MAX_TAILNET_RULE_BYTES;
    let (rt, _dir) = test_runtime();
    prepare_tailnet_fixture(&rt, &ts_user_config()).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let hosts = rt.observed_tailnet_snapshot();
    let bytes = std::fs::read(tailnet_file(&rt, "ts1")).unwrap();
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    let mut raw = ts_config_json();
    raw["servers"][0]["tailscaleSettings"]["routes"] = serde_json::json!((0..60000)
        .map(|n| format!("172.{}.{}.{}/32", (n >> 16) & 255, (n >> 8) & 255, n & 255))
        .collect::<Vec<_>>());
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let result = rt
        .write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
        .await;
    assert!(result.unwrap_err().contains("functional byte limit"));
    assert_eq!(rt.observed_tailnet_snapshot(), hosts);
    assert_eq!(std::fs::read(tailnet_file(&rt, "ts1")).unwrap(), bytes);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    let generated = generate_sing_box_config(
        &config,
        &BTreeMap::new(),
        &rt.generate_deps(9090, 0, 0, None, &[], &raw, false),
    )
    .unwrap();
    let bindings = rt.tailnet_bindings(&raw, &generated);
    assert!(!rt.tailnet_bindings_within_limit(&bindings));
    {
        let _publication = rt.gate.lock_generation_publication();
        rt.activate_tailnet_context(rt.gate.generation(), bindings);
    }
    assert!(rt.tailnet_context().is_none());
    assert!(rt.tailnet_routes_limited());
    assert_eq!(rt.observed_tailnet_snapshot(), hosts);
    assert_eq!(std::fs::read(tailnet_file(&rt, "ts1")).unwrap(), bytes);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    let oversized = " ".repeat(MAX_TAILNET_RULE_BYTES + 1);
    assert!(parse_tailnet_rule_file_cidrs(&oversized).is_none());
}

#[tokio::test]
async fn known_retained_planning_is_not_promoted_to_current_observation() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    prepare_tailnet_fixture(&rt, &config).await;
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    rt.reset_ts_exit_block_state();
    prepare_tailnet_fixture(&rt, &config).await;
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_PEER_V4], vec![])]);
    assert_eq!(
        rt.fresh_tailnet_snapshot(rt.gate.generation())["ts1"],
        vec![OBSERVED_PEER_V4]
    );
    assert!(rt.observed_tailnet_snapshot()["ts1"].contains(&OBSERVED_V4.into()));
    assert!(rt.observed_tailnet_snapshot()["ts1"].contains(&OBSERVED_PEER_V4.into()));
}

fn identity_raw_fixture() -> serde_json::Value {
    let mut raw = ts_config_json();
    raw["meshRouteState"] = serde_json::json!({"schemaVersion":1,"localId":"local","revision":"1",
        "identities":[{"serverId":"ts1","identityEpoch":"epoch-one","controlAuthority":"https://controlplane.tailscale.com","selfStableId":"self-one","bindingState":"bound","evidenceSource":"status"}],
        "observations":[],"reservations":[],"intent":{"revision":"1","desiredRun":"stopped"},"identityEffects":[]});
    raw
}

fn admit_tailnet_raw(
    rt: &Arc<ProxyRuntime>,
    raw: &serde_json::Value,
) -> polaris_config_engine::singbox::SingBoxConfig {
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(9090, 0, 0, None, &[], raw, false);
    let generated = generate_sing_box_config(&typed, &BTreeMap::new(), &deps).unwrap();
    let _publication = rt.gate.lock_generation_publication();
    rt.activate_tailnet_context(rt.gate.generation(), rt.tailnet_bindings(raw, &generated));
    generated
}

#[tokio::test]
async fn real_reconcile_retire_append_admits_only_successor_and_rejects_multiple_current() {
    use polaris_config_engine::user_config::mesh_identity_reconcile::{
        reconcile_identity_evidence, ReplacementIdentity, RetirementScopeSnapshot,
    };
    use polaris_config_engine::user_config::mesh_route_state::{
        MeshOwnerRef, MeshRoutePolicy, MeshRouteState,
    };
    let (rt, _dir) = test_runtime();
    let mut raw = identity_raw_fixture();
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    rt.write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
        .await
        .unwrap();
    admit_tailnet_raw(&rt, &raw);
    sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
    let state: MeshRouteState = serde_json::from_value(raw["meshRouteState"].clone()).unwrap();
    let policy: MeshRoutePolicy = serde_json::from_value(serde_json::json!({"schemaVersion":1,"candidateOrder":["ts1"],"assignments":[],"overrides":[],"migration":{"unknownPublicPool":"reject","builtinExceptionsVersion":1}})).unwrap();
    let next = reconcile_identity_evidence(
        &state,
        &policy,
        "ts1",
        ReplacementIdentity {
            epoch: "epoch-two",
            control_authority: "HTTPS://CONTROLPLANE.TAILSCALE.COM",
            self_stable_id: Some("self-two"),
        },
        Some(RetirementScopeSnapshot {
            owner_ref: MeshOwnerRef {
                server_id: "ts1".into(),
                identity_epoch: "epoch-one".into(),
            },
            state_revision: &state.revision,
            active_plan_id: None,
            configured_cidrs: Some(&[]),
            active_plan_owner_cidrs: Some(&[]),
        }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(next.identities.len(), 2);
    assert_eq!(
        next.identities[0].binding_state,
        polaris_config_engine::user_config::mesh_route_state::MeshBindingState::Retired
    );
    raw["meshRouteState"] = serde_json::to_value(next).unwrap();
    assert!(rt.observed_tailnet_for_config(&raw).is_empty());
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    rt.write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
        .await
        .unwrap();
    let generated = admit_tailnet_raw(&rt, &raw);
    assert_eq!(rt.tailnet_bindings(&raw, &generated).len(), 1);
    let mut successor = frame("ts1", &[OBSERVED_PEER_V4], vec![]);
    successor.details.self_peer = Some(peer("self", &[], true));
    successor.details.self_peer.as_mut().unwrap().stable_id = Some("self-two".into());
    sync_fixture(&rt, &[successor]);
    assert_eq!(
        rt.observed_tailnet_snapshot()["ts1"],
        vec![OBSERVED_PEER_V4]
    );
    assert!(!std::fs::read_to_string(tailnet_file(&rt, "ts1"))
        .unwrap()
        .contains(OBSERVED_V4));
    let mut duplicate = raw["meshRouteState"]["identities"][1].clone();
    duplicate["identityEpoch"] = serde_json::json!("epoch-three");
    duplicate["bindingState"] = serde_json::json!("bound");
    raw["meshRouteState"]["identities"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(rt.tailnet_bindings(&raw, &generated).is_empty());
    assert!(rt.observed_tailnet_for_config(&raw).is_empty());
    let old_file = std::fs::read(tailnet_file(&rt, "ts1")).unwrap();
    let typed = serde_json::from_value(raw.clone()).unwrap();
    assert!(rt
        .write_tailnet_rule_files_for_generation(&typed, &raw, rt.gate.generation())
        .await
        .is_err());
    assert_eq!(std::fs::read(tailnet_file(&rt, "ts1")).unwrap(), old_file);
}

#[tokio::test]
async fn retired_and_retiring_known_history_cannot_return_through_builder_file_exists() {
    for binding in ["retired", "retiring"] {
        for file_present in [true, false] {
            let (rt, _dir) = test_runtime();
            let mut raw = identity_raw_fixture();
            let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
            rt.write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
                .await
                .unwrap();
            admit_tailnet_raw(&rt, &raw);
            sync_fixture(&rt, &[frame("ts1", &[OBSERVED_V4], vec![])]);
            rt.reset_ts_exit_block_state();
            let path = tailnet_file(&rt, "ts1");
            if !file_present {
                std::fs::remove_file(&path).unwrap();
            }
            raw["meshRouteState"]["identities"][0]["bindingState"] = serde_json::json!(binding);
            let config = serde_json::from_value(raw.clone()).unwrap();
            rt.write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
                .await
                .unwrap();
            assert!(rt.observed_tailnet_snapshot().is_empty());
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(!content.contains(OBSERVED_V4));
            // Actual builder still registers by exists; the bytes it can read
            // now contain only current declarations, genuinely empty here.
            let generated = admit_tailnet_raw(&rt, &raw);
            let json = serde_json::to_value(&generated).unwrap();
            assert!(json["route"]["rule_set"]
                .as_array()
                .unwrap()
                .iter()
                .any(|rule| {
                    rule["path"]
                        .as_str()
                        .is_some_and(|rule_path| std::path::Path::new(rule_path) == path.as_path())
                }));
            assert!(rt.tailnet_bindings(&raw, &generated).is_empty());
            assert_eq!(
                parse_tailnet_rule_file_cidrs(&content),
                Some(Vec::<String>::new())
            );
            let empty: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert_eq!(empty["version"], 1);
            assert_eq!(empty["rules"], serde_json::json!([]));
            // No ledger metadata remains weak/unknown, rather than inferred logout.
            let missing = ts_config_json();
            assert_eq!(rt.tailnet_bindings(&missing, &generated).len(), 1);
        }
    }
}

#[tokio::test]
async fn retired_source_that_cannot_be_replaced_or_removed_stops_preparation() {
    let (rt, _dir) = test_runtime();
    let mut raw = identity_raw_fixture();
    raw["meshRouteState"]["identities"][0]["bindingState"] = serde_json::json!("retired");
    let path = tailnet_file(&rt, "ts1");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("block-rename"), "fixture").unwrap();
    let config = serde_json::from_value(raw.clone()).unwrap();
    assert!(rt
        .write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
        .await
        .unwrap_err()
        .contains("could not be replaced or removed"));
    assert!(rt.observed_tailnet_snapshot().is_empty());
}

#[tokio::test]
async fn later_endpoint_overflow_cannot_partially_publish_earlier_endpoint() {
    let (rt, _dir) = test_runtime();
    let mut raw = ts_config_json();
    let mut second = raw["servers"][0].clone();
    second["id"] = serde_json::json!("ts2");
    second["name"] = serde_json::json!("ts2");
    raw["servers"].as_array_mut().unwrap().push(second);
    let config: UserConfig = serde_json::from_value(raw).unwrap();
    prepare_tailnet_fixture(&rt, &config).await;
    sync_fixture(
        &rt,
        &[
            frame("ts1", &[OBSERVED_V4], vec![]),
            frame("ts2", &[OBSERVED_PEER_V4], vec![]),
        ],
    );
    let before = ["ts1", "ts2"].map(|id| std::fs::read(tailnet_file(&rt, id)).unwrap());
    let hosts = rt.observed_tailnet_snapshot();
    let fresh = rt.fresh_tailnet_snapshot(rt.gate.generation());
    let status = rt.mesh.tailscale_status_snapshot(true).statuses;
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    let mut overflowing = frame("ts2", &[], vec![]);
    overflowing.tailscale_ips = (0..60000)
        .map(|n| format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255))
        .collect();
    assert!(!apply_fixture(
        &rt,
        &[frame("ts1", &["32.0.0.77"], vec![]), overflowing]
    ));
    assert_eq!(
        ["ts1", "ts2"].map(|id| std::fs::read(tailnet_file(&rt, id)).unwrap()),
        before
    );
    assert_eq!(rt.observed_tailnet_snapshot(), hosts);
    assert_eq!(rt.fresh_tailnet_snapshot(rt.gate.generation()), fresh);
    assert_eq!(rt.mesh.tailscale_status_snapshot(true).statuses, status);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    assert!(rt.tailnet_routes_limited());
}

#[tokio::test]
async fn weak_binding_negative_knowledge_survives_stop_but_is_not_persisted_provenance() {
    let dir = fresh_test_dir();
    let rt = test_runtime_in(dir.clone());
    let raw = identity_raw_fixture();
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let path = tailnet_file(&rt, "ts1");
    std::fs::create_dir_all(rt.tailnet_rules_dir()).unwrap();
    std::fs::write(
        &path,
        super::super::tailnet_rules::tailnet_rule_file_json(&["32.0.0.99/32".into()]),
    )
    .unwrap();
    rt.write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
        .await
        .unwrap();
    admit_tailnet_raw(&rt, &raw);
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
    rt.reset_ts_exit_block_state();
    assert!(rt.tailnet_context().is_none());
    assert!(rt.observed_tailnet_for_config(&raw)["ts1"].contains(&"32.0.0.99".into()));
    let mut changed = raw.clone();
    changed["meshRouteState"]["identities"][0]["identityEpoch"] = serde_json::json!("epoch-two");
    assert!(rt.observed_tailnet_for_config(&changed).is_empty());
    let next_config: UserConfig = serde_json::from_value(changed.clone()).unwrap();
    {
        // A new process has no old captured context. Legacy remains an unknown,
        // weak planning hint under the existing compatibility policy.
        let restarted = test_runtime_in(dir.to_path_buf());
        restarted
            .write_tailnet_rule_files_for_generation(
                &next_config,
                &changed,
                restarted.gate.generation(),
            )
            .await
            .unwrap();
        admit_tailnet_raw(&restarted, &changed);
        assert!(
            restarted.observed_tailnet_for_config(&changed)["ts1"].contains(&"32.0.0.99".into())
        );
        assert!(restarted
            .fresh_tailnet_snapshot(restarted.gate.generation())
            .is_empty());
    }
    // The old process still knows its admitted C1 was incompatible. It may not
    // clear that fact via Stop and then reimport the same weak on-disk hosts.
    rt.write_tailnet_rule_files_for_generation(&next_config, &changed, rt.gate.generation())
        .await
        .unwrap();
    assert!(rt.observed_tailnet_snapshot().is_empty());
    assert!(!std::fs::read_to_string(path).unwrap().contains("32.0.0.99"));
}

#[tokio::test]
async fn cold_empty_production_write_is_match_none_without_freshness_or_ready() {
    let (rt, _dir) = test_runtime();
    let raw = ts_config_json();
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let generation = rt.gate.generation();
    rt.write_tailnet_rule_files_for_generation(&config, &raw, generation)
        .await
        .unwrap();
    let content = std::fs::read_to_string(tailnet_file(&rt, "ts1")).unwrap();
    let actual: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(actual.as_object().unwrap().len(), 2);
    assert_eq!(actual["version"], 1);
    assert_eq!(actual["rules"], serde_json::json!([]));
    assert_eq!(
        parse_tailnet_rule_file_cidrs(&content),
        Some(Vec::<String>::new())
    );
    assert!(rt.observed_tailnet_snapshot().is_empty());
    assert!(rt.fresh_tailnet_snapshot(generation).is_empty());
    assert!(rt.tailnet_context().is_none());
    assert!(rt.mesh_route_run.read().unwrap().is_none());
}

#[tokio::test]
async fn explicit_nonempty_production_write_keeps_legacy_canonical_bytes() {
    let (rt, _dir) = test_runtime();
    let mut raw = ts_config_json();
    let cidrs = vec![
        TAILNET_CGNAT.to_string(),
        TAILNET_ULA_V6.to_string(),
        "10.8.0.0/24".into(),
    ];
    raw["servers"][0]["tailscaleSettings"]["routes"] = serde_json::json!(cidrs);
    let config: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    rt.write_tailnet_rule_files_for_generation(&config, &raw, rt.gate.generation())
        .await
        .unwrap();
    // Exact pre-change nonempty format, including key/array order and whitespace.
    let legacy = serde_json::to_string_pretty(&serde_json::json!({
        "version": 1,
        "rules": [{ "ip_cidr": cidrs }],
    }))
    .unwrap();
    let bytes = std::fs::read(tailnet_file(&rt, "ts1")).unwrap();
    assert_eq!(bytes.as_slice(), legacy.as_bytes());
    assert_eq!(parse_tailnet_rule_file_cidrs(&legacy), Some(cidrs));
    assert!(rt.fresh_tailnet_snapshot(rt.gate.generation()).is_empty());
}

#[tokio::test]
async fn production_generation_writes_empty_nonempty_empty_without_stale_publication() {
    let (rt, _dir) = test_runtime();
    let empty_raw = ts_config_json();
    let empty_config: UserConfig = serde_json::from_value(empty_raw.clone()).unwrap();
    let first_generation = rt.gate.generation();
    rt.write_tailnet_rule_files_for_generation(&empty_config, &empty_raw, first_generation)
        .await
        .unwrap();
    let path = tailnet_file(&rt, "ts1");
    let empty_bytes = std::fs::read(&path).unwrap();
    let mut nonempty_raw = empty_raw.clone();
    nonempty_raw["servers"][0]["tailscaleSettings"]["routes"] = serde_json::json!(["10.8.0.0/24"]);
    let nonempty_config: UserConfig = serde_json::from_value(nonempty_raw.clone()).unwrap();
    rt.bump_generation();
    let second_generation = rt.gate.generation();
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    rt.write_tailnet_rule_files_for_generation(&nonempty_config, &nonempty_raw, first_generation)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), empty_bytes);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    rt.write_tailnet_rule_files_for_generation(&nonempty_config, &nonempty_raw, second_generation)
        .await
        .unwrap();
    let nonempty_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        parse_tailnet_rule_file_cidrs(std::str::from_utf8(&nonempty_bytes).unwrap()),
        Some(vec!["10.8.0.0/24".into()])
    );
    assert_ne!(nonempty_bytes, empty_bytes);
    rt.bump_generation();
    let third_generation = rt.gate.generation();
    let epoch = rt.tailnet_file_write_epoch.load(Ordering::SeqCst);
    rt.write_tailnet_rule_files_for_generation(&empty_config, &empty_raw, second_generation)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), nonempty_bytes);
    assert_eq!(rt.tailnet_file_write_epoch.load(Ordering::SeqCst), epoch);
    rt.write_tailnet_rule_files_for_generation(&empty_config, &empty_raw, third_generation)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), empty_bytes);
    let value: serde_json::Value = serde_json::from_slice(&empty_bytes).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["rules"], serde_json::json!([]));
    assert!(rt.observed_tailnet_snapshot().is_empty());
    assert!(rt.fresh_tailnet_snapshot(third_generation).is_empty());
    assert!(rt.mesh_route_run.read().unwrap().is_none());
}

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
        update_in_port: None,
        subscription_update_in_port: None,
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

    // ── 起核前腿：首次运行、无已知集合 ⇒ 官方默认两段（**不是空文件**）。
    rt.write_tailnet_rule_files(&config).await;
    let path = tailnet_file(&rt, "ts1");
    let first = std::fs::read_to_string(&path).expect("起核前腿必须落一个文件出来");
    let first_cidrs =
        parse_tailnet_rule_file_cidrs(&first).expect("落盘内容必须是可解析的 rule-set");
    assert_eq!(
        first_cidrs,
        vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()],
        "首次运行应落官方默认两段；空文件会让块 0c 走 rule_set 腿却一条段都不发 = 整段 tailnet 全断"
    );

    // ── 运行中腿：一帧全量快照（self + 一台**离线** peer）。
    rt.sync_tailnet_rule_files(&[frame(
        "ts1",
        &[OBSERVED_V4, OBSERVED_V6],
        vec![peer("peer-a", &[OBSERVED_PEER_V4], false)],
    )]);
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
        cidrs.contains(&TAILNET_CGNAT.to_string()) && cidrs.contains(&TAILNET_ULA_V6.to_string()),
        "观测地址把官方默认段顶掉了 —— 语义必须是并集。实际：{cidrs:?}"
    );

    // ── 消费面 ①：真跑 generate（deps 取自生产装配 `generate_deps`，不是手拼）。
    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json());
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
        declared.path.as_deref(),
        Some(path.to_string_lossy().as_ref()),
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
    rt.write_tailnet_rule_files(&config).await;
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]);
    let path = tailnet_file(&rt, "ts1");
    let before = std::fs::read_to_string(&path).expect("热更后文件应在");
    assert!(
        before.contains(&format!("{OBSERVED_V4}/32")),
        "前置没建立起来"
    );

    // 空帧：self 无地址、无 peer（真机上核刚起、netmap 未同步时就是这个形态）。
    rt.sync_tailnet_rule_files(&[frame("ts1", &[], vec![])]);
    let after = std::fs::read_to_string(&path).expect("空帧不得让文件消失");
    assert_eq!(
        after, before,
        "空帧把已知观测冲掉了 —— 清空 = 整段 tailnet 不可路由"
    );

    // 全是非法字面量的帧同样什么都不做（写进 ip_cidr 会让核 netip.ParsePrefix 启动 FATAL）。
    rt.sync_tailnet_rule_files(&[frame("ts1", &["not-an-ip", ""], vec![])]);
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
    rt.write_tailnet_rule_files(&config).await;
    let path = tailnet_file(&rt, "ts1");

    // 依次走遍热更腿的每条早退：空帧 / 非法地址 / 同值 / 文件内容不可解析。
    rt.sync_tailnet_rule_files(&[frame("ts1", &[], vec![])]);
    assert!(path.exists(), "空帧腿删了文件");
    rt.sync_tailnet_rule_files(&[frame("ts1", &["bad"], vec![])]);
    assert!(path.exists(), "非法地址腿删了文件");
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(path.exists(), "首次并入删了文件");
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(path.exists(), "同值腿删了文件");
    std::fs::write(&path, "{ not json").expect("写坏文件");
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]);
    assert!(
        path.exists(),
        "文件内容坏掉时热更腿删了它 —— 运行中删被挂载文件会致 sing-box reload 报错"
    );

    // 源码面：热更腿里不存在任何 unlink。
    let src = module_code("runtime/proxy");
    let body = method_body(
        &src,
        "    pub(super) fn sync_tailnet_rule_files(&self, events: &[TailscaleStatusEvent]) {",
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
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]);
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

    rt.write_tailnet_rule_files(&config).await; // 不得 panic
    rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4], vec![])]); // 不得 panic

    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json());
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

/// 🔴 **bootstrap 腿的降级面** —— 上一条测的是「有观测」那半，这条补「从未连过」那半。
///
/// 两半必须分开测：`endpoint_forced_route_cidrs` 对 Tailscale 是**二选一**而非并集
/// （有观测 ⇒ 观测段 + MagicDNS；无观测 ⇒ 默认两段），只测一半会让另一半的回归静默通过。
/// 此前这条腿在 `src-tauri` 侧完全没有覆盖 —— 「有观测」那条测试同时断言了默认段在场，
/// 于是看起来两件事都测了，实际一件都没锁住 bootstrap。
#[tokio::test]
async fn write_failure_on_a_node_without_observation_keeps_bootstrap() {
    let (rt, _dir) = test_runtime();
    let config = ts_user_config();
    let dir = rt.tailnet_rules_dir();
    std::fs::create_dir_all(dir.parent().expect("configDir")).expect("建 configDir");
    std::fs::write(&dir, "占位：这不是目录").expect("把 tailnet-rules 占成文件");

    rt.write_tailnet_rule_files(&config).await; // 不得 panic
                                                // 刻意不喂任何 status 帧：这就是「节点从未连上过控制面」的形态。
    assert!(
        rt.observed_tailnet_snapshot().is_empty(),
        "前置：本例的观测面必须是空的，否则测的不是 bootstrap 腿"
    );

    let deps = rt.generate_deps(9090, 0, 0, None, &[], &ts_config_json());
    let cfg = generate_sing_box_config(&config, &BTreeMap::new(), &deps).expect("生成配置");
    let route = cfg.route.as_ref().expect("没有 route 段");
    let inline = route
        .rules
        .iter()
        .find(|r| r.outbound.as_deref() == Some("ts1") && r.ip_cidr.is_some())
        .expect("降级腿必须回落 inline force-route");
    let cidrs = inline.ip_cidr.as_ref().expect("ip_cidr");
    assert!(
        cidrs.contains(&TAILNET_CGNAT.to_string()) && cidrs.contains(&TAILNET_ULA_V6.to_string()),
        "无观测节点必须发默认两段作 bootstrap —— 否则它在连上控制面之前整段 tailnet 不可路由。\
         实际：{cidrs:?}"
    );
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
        rt.write_tailnet_rule_files(&config).await;
        rt.sync_tailnet_rule_files(&[frame("ts1", &[OBSERVED_V4, OBSERVED_V6], vec![])]);
    }
    let restarted = test_runtime_in(dir.clone());
    assert!(
        restarted.observed_tailnet_snapshot().is_empty(),
        "前置：新实例的内存观测面本应是空的（否则下面的断言没有信息量）"
    );
    restarted.write_tailnet_rule_files(&config).await;
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

    rt.write_tailnet_rule_files(&config).await;

    let cidrs = parse_tailnet_rule_file_cidrs(&std::fs::read_to_string(&path).expect("文件应在"))
        .expect("起核前腿必须把坏文件重写成可解析的 rule-set");
    assert_eq!(
        cidrs,
        vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()],
        "坏文件应被按配置期段重写。实际：{cidrs:?}"
    );
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
    rt.write_tailnet_rule_files(&config).await;
    let path = tailnet_file(&rt, "ts1");
    assert!(
        !std::fs::read_to_string(&path)
            .expect("起核前腿应落盘")
            .contains(OBSERVED_V4),
        "前置：此刻文件里还不该有观测地址（否则本门没有区分力）"
    );

    let tag_to_id = BTreeMap::from([("ts1-tag".to_string(), "ts1".to_string())]);
    let update = dm::TailscaleStatusUpdate {
        endpoints: vec![dm::TailscaleEndpointStatus {
            endpoint_tag: "ts1-tag".into(),
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
    rt.apply_ts_status_frame(&update, &tag_to_id, rt.gate.generation());

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

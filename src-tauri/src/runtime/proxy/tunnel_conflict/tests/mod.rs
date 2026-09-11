use super::*;

use polaris_config_engine::builder::endpoint_routes::ObservedTailnetAddresses;
use polaris_config_engine::builder::tunnel_conflict::{emitted_conflict_criteria, ConflictKind};
use polaris_config_engine::user_config::server_config::{
    Protocol, ServerConfig, TailscaleSettings,
};
use polaris_system_integration::route_probe::ForeignTunnelSnapshot;

/// 2026-09-11 真控制面实测值：自建 headscale 的 `prefixes.v4` 非默认，self 拿到 `32.0.0.28`。
const OBSERVED_V4: &str = "32.0.0.28";

/// 现场那台机器上外来隧道宣告的段（`utun4` 上的 `32.0.0.0/24`）。
fn field_probe() -> Result<TunnelProbeOutcome, String> {
    Ok(TunnelProbeOutcome::Probed(ForeignTunnelSnapshot {
        tunnel_interfaces: vec!["utun4".into()],
        foreign: vec![
            RouteEntry {
                prefix: "32.0.0.0/24".into(),
                interface: "utun4".into(),
            },
            RouteEntry {
                prefix: "fd7a:115c:a1e0::/48".into(),
                interface: "utun4".into(),
            },
        ],
    }))
}

/// 一份「发射给内核的」TUN 配置：默认 FakeIP 段 + macOS 形态的 TUN 地址。
fn emitted_tun() -> SingBoxConfig {
    serde_json::from_value(serde_json::json!({
        "log": { "level": "info", "timestamp": true },
        "dns": { "servers": [
            { "tag": "fakeip", "type": "fakeip", "inet4_range": "198.18.0.0/15", "inet6_range": "2001:2::/48" }
        ]},
        "inbounds": [
            { "type": "tun", "tag": "tun-in", "address": ["172.19.0.1/30"] }
        ],
        "outbounds": [],
    }))
    .expect("夹具应是合法的 SingBoxConfig")
}

fn ts_node(id: &str) -> ServerConfig {
    ServerConfig {
        id: id.into(),
        name: id.into(),
        tailscale_settings: Some(Box::new(TailscaleSettings::default())),
        protocol: Protocol::Tailscale,
        ..Default::default()
    }
}

fn observed(id: &str, addrs: &[&str]) -> ObservedTailnetAddresses {
    let mut map = ObservedTailnetAddresses::new();
    map.insert(
        id.to_string(),
        addrs.iter().map(|a| (*a).to_string()).collect(),
    );
    map
}

/// 🔴 **判据 2（负向对照，本组最重要的一条）**：接线之后，
/// `self_hosted_tailnet_alone_is_not_a_conflict` 的语义必须仍然成立。
///
/// 本机有个自建 tailnet（`32.0.0.0/24` on `utun4`）、Polaris 这边没配任何组网节点
/// ⇒ 三组判据段一个都不与它相交 ⇒ **零告警**。
///
/// 逢隧道必报是这条链最要防的失败形态：一个每次连接都弹的告警，最后的下场是被无视或被删掉。
#[test]
fn self_hosted_tailnet_alone_is_still_not_a_conflict_after_wiring() {
    let criteria = emitted_conflict_criteria(
        &emitted_tun(),
        &[], // Polaris 未配组网节点
        &ObservedTailnetAddresses::new(),
    );
    let snapshot = snapshot_from_probe(field_probe(), criteria);
    let TunnelConflictSnapshot::Probed {
        conflicts, foreign, ..
    } = &snapshot
    else {
        panic!("探测成功应走 Probed：{snapshot:?}");
    };
    assert_eq!(foreign.len(), 2, "前提：那两条外来隧道路由确实被喂进来了");
    assert!(
        conflicts.is_empty(),
        "外来隧道段与三组判据段都不相交，接线之后也不该报警：{conflicts:?}"
    );
}

/// 🔴 **判据 1（正向）**：外来隧道段与 Polaris 自己的组网段相交 ⇒ 判出 `MeshOverlap`。
///
/// 与上一条共用同一份探测结果，只改「Polaris 这边有没有一个 tailnet 节点、观测到的是哪个前缀」——
/// 两条合起来才说明判定跟着**判据段**走，而不是跟着「本机有没有隧道」走。
#[test]
fn mesh_overlap_is_reported_through_the_wiring() {
    let criteria = emitted_conflict_criteria(
        &emitted_tun(),
        &[ts_node("ts1")],
        &observed("ts1", &[OBSERVED_V4]),
    );
    let snapshot = snapshot_from_probe(field_probe(), criteria);
    let TunnelConflictSnapshot::Probed { conflicts, .. } = &snapshot else {
        panic!("探测成功应走 Probed：{snapshot:?}");
    };
    assert!(
        conflicts.iter().any(|c| c.kind == ConflictKind::MeshOverlap
            && c.prefix == "32.0.0.0/24"
            && c.interface == "utun4"),
        "自建 tailnet 段被两个声索人争着，应判出 MeshOverlap：{conflicts:?}"
    );
}

/// 🔴 **判据 3（平台缺席自曝）**：mac/win 的返回是「未实现」，**不是空冲突列表**。
///
/// 三条断言分别挡住三种把它读成「无冲突」的路：
/// 快照的变体、线格式的 `status`、以及线格式里**不许出现** `conflicts` 键
/// （带一个空数组的话，渲染端 `conflicts.length === 0` 会得出"没冲突"）。
#[test]
fn unsupported_platform_never_looks_like_zero_conflicts() {
    let snapshot = snapshot_from_probe(
        Ok(TunnelProbeOutcome::Unsupported(Platform::Mac)),
        ConflictCriteria::default(),
    );
    assert_eq!(
        snapshot,
        TunnelConflictSnapshot::Unsupported { platform: "darwin" }
    );
    let wire = snapshot.to_wire();
    assert_eq!(wire["status"], "unsupported");
    assert!(
        wire.get("conflicts").is_none(),
        "未实现那一支不许带 `conflicts` 键 —— 空数组会被读成「看过了，没有冲突」：{wire}"
    );

    // 正向对照：同一条线格式在**探到了**那一支上确实带 `conflicts`，
    // 否则上面那条否定断言可能只是因为线格式里压根没有这个键。
    let probed = snapshot_from_probe(field_probe(), ConflictCriteria::default()).to_wire();
    assert_eq!(probed["status"], "probed");
    assert!(
        probed.get("conflicts").is_some(),
        "探到了那一支必须带 `conflicts`：{probed}"
    );
}

/// 探测命令失败同样不是「无冲突」。
#[test]
fn probe_failure_never_looks_like_zero_conflicts() {
    let snapshot = snapshot_from_probe(
        Err("ip: command not found".into()),
        ConflictCriteria::default(),
    );
    assert!(matches!(
        snapshot,
        TunnelConflictSnapshot::ProbeFailed { .. }
    ));
    let wire = snapshot.to_wire();
    assert_eq!(wire["status"], "probeFailed");
    assert!(wire.get("conflicts").is_none(), "{wire}");
}

/// 还没探过 ⇒ `notProbed`，同样不带 `conflicts`。这是 `ProxyRuntime` 的初值。
#[test]
fn not_probed_is_the_initial_state_and_carries_no_verdict() {
    let wire = TunnelConflictSnapshot::NotProbed.to_wire();
    assert_eq!(wire["status"], "notProbed");
    assert!(wire.get("conflicts").is_none(), "{wire}");
}

/// 🔴 **判据 4 的接线侧一半**：`own_interfaces` 两个来源都要进。
///
/// 配置里的 `interface_name`（Linux/Windows 显式命名）+ post-flight 观测到的出口别名
/// （macOS 的 `utunN` 只有这条腿认得出）。缺任一条，我方自己的路由都会被当成外来隧道。
#[test]
fn own_interfaces_take_both_the_configured_name_and_the_observed_alias() {
    let named: SingBoxConfig = serde_json::from_value(serde_json::json!({
        "log": { "level": "info", "timestamp": true },
        "inbounds": [{ "type": "tun", "tag": "tun-in", "interface_name": "polaris-tun0" }],
        "outbounds": [],
    }))
    .expect("夹具应是合法的 SingBoxConfig");
    assert_eq!(
        own_tunnel_interfaces(&named, Some("utun7")),
        vec!["polaris-tun0".to_string(), "utun7".to_string()]
    );
    // macOS 形态：配置里没有接口名（内核分配），只有观测别名。
    assert_eq!(
        own_tunnel_interfaces(&emitted_tun(), Some("utun7")),
        vec!["utun7".to_string()],
        "配置里没名字时，观测别名是唯一认得出自己的那条腿"
    );
    // 观测也读不到（探测失败 / 不可断言）⇒ 只剩配置里那条，不凭空编一个。
    assert_eq!(
        own_tunnel_interfaces(&named, None),
        vec!["polaris-tun0".to_string()]
    );
    assert!(
        own_tunnel_interfaces(&named, Some("   ")).len() == 1,
        "空白别名不是身份"
    );
}

/// 判定跟着**本次发射的产物**走：同一份探测结果，配置里的 TUN 地址换一个就换一个结论。
///
/// 钉的是「判据段来自读回、不是来自某个常量」——写死 `172.19.0.1/30` 的实现在这条上会绿，
/// 但把夹具改成与外来隧道相交的地址之后就会红。
#[test]
fn tun_address_criteria_follow_the_emitted_config() {
    let colliding: SingBoxConfig = serde_json::from_value(serde_json::json!({
        "log": { "level": "info", "timestamp": true },
        "inbounds": [{ "type": "tun", "tag": "tun-in", "address": ["32.0.0.1/24"] }],
        "outbounds": [],
    }))
    .expect("夹具应是合法的 SingBoxConfig");
    let criteria = emitted_conflict_criteria(&colliding, &[], &ObservedTailnetAddresses::new());
    let TunnelConflictSnapshot::Probed { conflicts, .. } =
        snapshot_from_probe(field_probe(), criteria)
    else {
        panic!("探测成功应走 Probed");
    };
    assert!(
        conflicts
            .iter()
            .any(|c| c.kind == ConflictKind::TunAddressOverlap),
        "TUN 自己的地址段与外来隧道撞车，应判出 TunAddressOverlap：{conflicts:?}"
    );
}

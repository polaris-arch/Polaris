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

// ══════════ 展示面：link-local / 组播只收在这一层（真机抓取喂进来）══════════
//
// 取材是 2026-09-12 p101（macOS 26.6.2 / Tailscale 断开）的**真机抓取**，逐字住在
// `crates/system-integration/src/route_probe/tests/fixtures/` 里。这里按 `@@@` 分节读它、
// 不动它一个字节 —— 夹具是证据，改一个字节这组断言就不再是「真机上会发生什么」。
//
// 为什么要跑完整条链（夹具 → `assemble_macos_probe` → `snapshot_from_probe` → `to_wire`）
// 而不是手搓 36 条 `RouteEntry`：手搓的是我对那台机器的复述，跑真解析器的才是那台机器。

/// 真机夹具在仓里的位置（锚在仓根，不锚在本文件 —— 理由见 `polaris-source-probe` 头注）。
const MACOS_FIELD_FIXTURE: &str =
    "crates/system-integration/src/route_probe/tests/fixtures/macos-p101-routes-ts-off-2026-09-12.txt";

/// 取夹具的一个 `@@@<ID>` 分节体（标记行独占一行、不进分节体；分节体逐字保留）。
///
/// 取不到分节直接 panic：一个静默的空分节会让 `assemble_macos_probe` 报「找不到列头」，
/// 那时失败信息说的是解析器，而真因是取材面塌了。
fn field_section(raw: &str, id: &str) -> String {
    let mut body: Option<String> = None;
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(name) = trimmed.strip_prefix("@@@") {
            let is_marker = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if is_marker {
                if body.is_some() {
                    break;
                }
                if name == id {
                    body = Some(String::new());
                }
                continue;
            }
        }
        if let Some(buf) = body.as_mut() {
            buf.push_str(line);
        }
    }
    body.unwrap_or_else(|| panic!("真机夹具里没有 @@@{id} 分节 —— 夹具形态变了，取材面已失效"))
}

/// 真机抓取过完整 mac 腿 ⇒ 探测结果；`extra` 是额外追加的路由（正向对照用，夹具本身不动）。
fn field_capture(extra: &[RouteEntry]) -> Result<TunnelProbeOutcome, String> {
    let raw = crate::test_support::repo_file(MACOS_FIELD_FIXTURE);
    let mut snapshot = polaris_system_integration::route_probe::assemble_macos_probe(
        &field_section(&raw, "V4"),
        &field_section(&raw, "V6"),
        &field_section(&raw, "IFCONFIG"),
        &[],
    )
    .expect("真机抓取应解析得动");
    snapshot.foreign.extend_from_slice(extra);
    Ok(TunnelProbeOutcome::Probed(snapshot))
}

/// 真机抓取（+ `extra`）→ 下发给设置页的那份线格式。
fn field_wire(extra: &[RouteEntry]) -> Value {
    let criteria = emitted_conflict_criteria(&emitted_tun(), &[], &ObservedTailnetAddresses::new());
    snapshot_from_probe(field_capture(extra), criteria).to_wire()
}

fn route(interface: &str, prefix: &str) -> RouteEntry {
    RouteEntry {
        prefix: prefix.into(),
        interface: interface.into(),
    }
}

/// 🔴 **判据 1**：那份真机抓取里 36 条 foreign **全是** link-local / 组播
/// ⇒ 展示面**零条**，但「探过了」这件事仍然看得见（`suppressedRoutes` = 36）。
///
/// 36 这个数字不是我填的，是夹具跑完真解析器的结果（探测侧同一条钉在
/// `route_probe::tests::macos_field_capture_is_36_routes_and_every_one_of_them_is_noise`）。
/// 它同时是本条的**正向对照**：展示面为零若只是因为压根没读到路由，这个数会是 0 而不是 36。
#[test]
fn field_capture_of_pure_noise_shows_nothing_yet_still_proves_it_probed() {
    let wire = field_wire(&[]);
    assert_eq!(wire["status"], "probed");
    assert_eq!(
        wire["foreignTunnels"],
        serde_json::json!([]),
        "36 条全是 link-local / 组播，展示面该一条不剩：{wire}"
    );
    assert_eq!(
        wire["suppressedRoutes"],
        serde_json::json!(36),
        "被收掉的条数必须如实报出来 —— 否则「探过了」与「压根没探」在界面上重新同形：{wire}"
    );
    assert_eq!(
        wire["conflicts"],
        serde_json::json!([]),
        "这两族与 FakeIP / Mesh / TUN 三组判据面不相交，本来就判不出冲突：{wire}"
    );
}

/// 🔴 **判据 2（正向对照）**：同一份抓取**加一条业务网段**（`32.0.0.0/24 on utun4`，
/// 2026-09-11 自建 headscale 实测的 tailnet v4 段）⇒ 那一条必须出现在展示面上。
///
/// 缺这条，判据 1 的「零条」可能只是因为展示面什么都不显示。
#[test]
fn one_business_range_in_the_same_capture_still_reaches_the_screen() {
    let wire = field_wire(&[route("utun4", "32.0.0.0/24")]);
    assert_eq!(
        wire["foreignTunnels"],
        serde_json::json!([{ "interface": "utun4", "prefix": "32.0.0.0/24" }]),
        "业务网段被过滤误杀了 —— 展示面本该只收 link-local / 组播：{wire}"
    );
    assert_eq!(
        wire["suppressedRoutes"],
        serde_json::json!(36),
        "多出来的那条不该被算进「收掉的」：{wire}"
    );
}

/// 🔴 **判据 3（最容易被误伤的一条）**：Tailscale 的 tailnet v6 段 `fd7a:115c:a1e0::/48`
/// 必须**留下**。它在 `fc00::/7` 里，但既不是 link-local 也不是组播。
///
/// 把「私网段」一并收掉的写法在这里转红 —— 而那正是唯一那条真要报的宣告。
#[test]
fn the_tailscale_ula_is_a_business_range_not_noise() {
    let wire = field_wire(&[route("utun4", "fd7a:115c:a1e0::/48")]);
    assert_eq!(
        wire["foreignTunnels"],
        serde_json::json!([{ "interface": "utun4", "prefix": "fd7a:115c:a1e0::/48" }]),
        "tailnet ULA 被当成噪声收掉了：{wire}"
    );
}

/// 过滤只作用在**展示面**：`conflicts` 与内存快照里的 `foreign` 仍是未经过滤的全量事实。
///
/// 钉的是「先滤再判」这种写法 —— 它会让判定跟着一张过滤表走，与本模块纪律 3 同一类失守。
/// 正向对照在同一条里：把 mesh 判据段设成 link-local 的 `169.254.0.0/16` 时，
/// 那条本会被展示面收掉的路由**照样**判出冲突。
#[test]
fn the_filter_never_reaches_the_verdict() {
    let colliding: SingBoxConfig = serde_json::from_value(serde_json::json!({
        "log": { "level": "info", "timestamp": true },
        "inbounds": [{ "type": "tun", "tag": "tun-in", "address": ["169.254.0.1/16"] }],
        "outbounds": [],
    }))
    .expect("夹具应是合法的 SingBoxConfig");
    let criteria = emitted_conflict_criteria(&colliding, &[], &ObservedTailnetAddresses::new());
    let snapshot = snapshot_from_probe(
        field_capture(&[route("utun4", "169.254.99.0/24")]),
        criteria,
    );
    let TunnelConflictSnapshot::Probed {
        foreign, conflicts, ..
    } = &snapshot
    else {
        panic!("探测成功应走 Probed：{snapshot:?}");
    };
    assert_eq!(
        foreign.len(),
        37,
        "快照里的 foreign 必须是全量事实（36 条噪声 + 那条 link-local 业务段）"
    );
    assert!(
        conflicts
            .iter()
            .any(|c| c.prefix == "169.254.99.0/24" && c.kind == ConflictKind::TunAddressOverlap),
        "判定面喂的是全量事实，展示面收掉的东西照样判得出冲突：{conflicts:?}"
    );
    let wire = snapshot.to_wire();
    assert_eq!(
        wire["foreignTunnels"],
        serde_json::json!([]),
        "展示面照旧收掉这一族（两条判据各管各的，这正是它们分开的意义）：{wire}"
    );
}

/// 两个展示面函数按构造互补：留下的 + 收掉的 ≡ 探测事实的全部。
///
/// 它们之所以是两个函数（而不是一个返回二元组的），是为了让 `to_wire` 的每一支保持
/// `=> json!({…})` 的直接字面量形状 —— 跨语言线格式门照那个形状取材。代价是「两半可能对不上」
/// 这个新缝，本条就是补它的：任一半改了判据、或某条同时落进/落不进两边，和都会对不上。
#[test]
fn display_face_partitions_the_probe_fact() {
    let TunnelConflictSnapshot::Probed { foreign, .. } = snapshot_from_probe(
        field_capture(&[
            route("utun4", "32.0.0.0/24"),
            route("utun4", "fd7a:115c:a1e0::/48"),
        ]),
        ConflictCriteria::default(),
    ) else {
        panic!("探测成功应走 Probed");
    };
    assert_eq!(foreign.len(), 38, "前提：36 条噪声 + 2 条业务段");
    assert_eq!(
        announced_routes(&foreign).len() + suppressed_route_count(&foreign),
        foreign.len(),
        "两半不互补 —— 有条目同时落进两边或两边都没进"
    );
    assert_eq!(announced_routes(&foreign).len(), 2);
    assert_eq!(suppressed_route_count(&foreign), 36);
}

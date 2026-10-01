//! TUN「连入排除清单」(`tunConfig.inboundExcludeCidrs`) 金样对拍 —— buildInbounds 派生的
//! `route_exclude_address` 内容。
//!
//! 对位 上游 `singbox-inbounds-builder.ts` L297-358 + `shared/tun-route-exclude.ts`
//! `computeUserTunExclude`。**变异锁**：本文件的每条 case 都被设计成「删掉一步减法就转红」——
//! 三条减法（mesh / fakeip / macOS 物理 LAN）各有一条只会被那一步剔掉的输入段，规范化（裸 IP 补掩码、
//! 拒非法/过宽）同理。expected 是**全量** `route_exclude_address` 数组（含顺序），不是 contains 断言：
//! 顺序漂移、多出一段、少一段都会失败。
//!
//! 为什么不进 `fixtures/inbounds.json`：那份是 TS 侧导出的对拍金样，Polaris 侧不该手工往里塞 case。

use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
use polaris_config_engine::user_config::UserConfig;

/// 本机物理 LAN（macOS 反向路由 guard 的输入；含主机位，走 CIDR 网络地址比较）。
const OWN_LAN: &str = "192.168.1.23/24";

/// 用户在 UI「连入排除清单」里填的原始条目。每条对应一条独立的判定路径：
///  · `172.16.5.0/24`   —— 与任何减法都不相交 → **必须**保留（否则整个特性又变装饰控件）。
///  · `10.66.7.0/24`    —— 落在 WG `allowedIPs` 10.66.0.0/16 内 → mesh 减法剔除（删 mesh 减法即转红）。
///  · `198.18.9.0/24`   —— 落在 FAKEIP_INET4_RANGE 198.18.0.0/15 内 → fakeip 减法剔除（删 fakeip 减法即转红）。
///  · `192.168.1.0/24`  —— 与本机物理 LAN 相交 → **仅 macOS** 剔除（删 darwin LAN 减法 / 误扩到 win32 均转红）。
///  · `203.0.113.7`     —— 裸 IP，须补 `/32`（不补则 sing-box `netip.ParsePrefix: no '/'` 启动 FATAL）。
///  · `256.1.1.1/24`    —— 八位组越界，须被严格校验剔除（松校验会放行 → 内核 FATAL）。
///  · `0.0.0.0/0`       —— catch-all，须被过宽下限剔除（放行 = 整个地址空间排出 TUN、代理静默失效）。
const USER_CIDRS: &str = r#"[
    "172.16.5.0/24",
    "10.66.7.0/24",
    "198.18.9.0/24",
    "192.168.1.0/24",
    "203.0.113.7",
    "256.1.1.1/24",
    "0.0.0.0/0"
]"#;

/// 组网 + FakeIP 场景的 UserConfig：一个 WG endpoint（`alwaysRouteSubnets` 缺省 true → engaged），
/// `dnsConfig` 缺省 → `usesFakeIp` 为 true → 非 Linux 平台 fakeip 段非空。
/// `selectedServerId` 留空是刻意的：避免节点 IP 排除块往 `route_exclude_address` 里追加无关条目，
/// 让金样只反映「连入排除」这一条派生链。
fn config_with(bypass_lan: bool) -> UserConfig {
    let json = format!(
        r#"{{
            "servers": [
                {{
                    "id": "wg-1",
                    "name": "mesh",
                    "protocol": "wireguard",
                    "address": "wg.example.com",
                    "port": 51820,
                    "wireguardSettings": {{ "allowedIPs": ["10.66.0.0/16"] }}
                }}
            ],
            "selectedServerId": null,
            "proxyModeType": "tun",
            "bypassLAN": {bypass_lan},
            "tunConfig": {{ "inboundExcludeCidrs": {USER_CIDRS} }}
        }}"#
    );
    serde_json::from_str(&json).expect("测试 config 反序列化失败")
}

fn deps(platform: &str) -> InboundsDeps {
    InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: vec![],
        platform: platform.into(),
        own_lan_cidrs: vec![OWN_LAN.into()],
        // 无运行期观测 ⇒ engaged_mesh 仅含 tailnet 默认常量段，与本字段出现之前同。
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    }
}

/// 取 TUN inbound 的 `route_exclude_address`（缺省 → None 表示字段整体不下发）。
fn tun_exclude(config: &UserConfig, deps: &InboundsDeps) -> Option<Vec<String>> {
    let inbounds = build_inbounds(config, None, deps);
    let tun = inbounds
        .iter()
        .find(|i| i.type_field == "tun")
        .expect("TUN 模式必须产出 tun inbound");
    tun.route_exclude_address.clone()
}

#[test]
fn darwin_inbound_exclude_golden() {
    // macOS：回环两条（平台基线）+ 用户段减 mesh / fakeip / 物理 LAN 后的两条。
    // 顺序 = 平台基线在前、用户段按声明顺序追加（对位 上游 excludeAddr.push(...userExclude.extra)）。
    let got = tun_exclude(&config_with(true), &deps("darwin"));
    let expected = ["127.0.0.0/8", "::1/128", "172.16.5.0/24", "203.0.113.7/32"].map(String::from);
    assert_eq!(
        got.as_deref(),
        Some(expected.as_slice()),
        "macOS route_exclude_address 金样不符"
    );
}

#[test]
fn win32_keeps_own_lan_but_still_subtracts_mesh_and_fakeip() {
    // Windows：物理 LAN **不**减（NE 反向路由是 macOS 专属约束；Win 侧 WinTun 的网关保护走 bypassLAN carve
    // 那条独立链路）。这里关 bypassLAN 让基线退化为回环两条 + Win DNS 回流护栏，金样才聚焦在用户段上。
    // mesh / fakeip / 规范化三条减法在 Win 上照旧生效。
    let got = tun_exclude(&config_with(false), &deps("win32")).expect("win32 应有排除段");
    let user_tail = &got[got.len() - 3..];
    assert_eq!(
        user_tail,
        ["172.16.5.0/24", "192.168.1.0/24", "203.0.113.7/32"].map(String::from),
        "win32 用户段派生不符：物理 LAN 段应保留，mesh/fakeip/非法段应被剔除"
    );
    // 前缀是平台基线（回环 + DNS 回流护栏），不含任何用户段派生物。
    assert!(
        !got[..got.len() - 3].iter().any(|c| c.starts_with("172.16.")
            || c.starts_with("10.66.")
            || c.starts_with("198.18.")),
        "win32 基线段不应混入用户段：{got:?}"
    );
}

#[test]
fn linux_emits_no_route_exclude_address_at_all() {
    // Linux 加法态：`route_exclude_address` 非空即触发策略路由表 2022 两族分解 → 服务端连入/allowLan 回包全断。
    // 「连入排除」在 Linux 上既不需要（内核策略路由天然保护回包）又是毒丸 → **恒不生成字段**。
    // 这条断言就是 UI「Linux 无效」说明与实现的一致性锁：任何往 Linux 分支塞排除段的改动都会转红。
    assert_eq!(
        tun_exclude(&config_with(true), &deps("linux")),
        None,
        "Linux 必须不下发 route_exclude_address（加法态毒丸）"
    );
}

#[test]
fn empty_list_is_byte_identical_to_absent() {
    // 空清单 / 未设 → 与今日字节完全一致（不得因本特性引入 `route_exclude_address: []` 或多余条目）。
    let baseline: UserConfig =
        serde_json::from_str(r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun"}"#)
            .unwrap();
    let empty: UserConfig = serde_json::from_str(
        r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun",
            "tunConfig":{"inboundExcludeCidrs":[]}}"#,
    )
    .unwrap();
    for platform in ["darwin", "win32", "linux"] {
        assert_eq!(
            tun_exclude(&empty, &deps(platform)),
            tun_exclude(&baseline, &deps(platform)),
            "[{platform}] 空清单必须与未设字段字节等价"
        );
    }
}

#[test]
fn all_dropped_leaves_platform_baseline_untouched() {
    // 全部条目都被减法/校验剔除 → 排除表须与「没填过」完全相同（不得留空洞或残渣）。
    let all_dropped: UserConfig = serde_json::from_str(
        r#"{
            "servers":[{"id":"wg-1","name":"mesh","protocol":"wireguard",
                        "address":"wg.example.com","port":51820,
                        "wireguardSettings":{"allowedIPs":["10.66.0.0/16"]}}],
            "selectedServerId": null,
            "proxyModeType":"tun",
            "tunConfig":{"inboundExcludeCidrs":["10.66.7.0/24","198.18.9.0/24","0.0.0.0/0"]}
        }"#,
    )
    .unwrap();
    let d = deps("darwin");
    assert_eq!(
        tun_exclude(&all_dropped, &d),
        Some(vec!["127.0.0.0/8".to_string(), "::1/128".to_string()]),
        "全剔除后应只剩 macOS 平台基线"
    );
}

/// Android × 用户声明段：**非回环段真的进，回环段在生成侧被剔**。
///
/// # 这条门补的是哪条缝
///
/// `golden_inbounds_android.rs` 那份夹具（`fixtures/inbounds-android.json`）里一条
/// `inboundExcludeCidrs` 都没有（`command grep -c` = 0），而本文件此前只跑 darwin/win32/linux ⇒
/// **Android × 连入来源排除**这个组合在全仓零门。2026-09-06 移动端
/// `settings/TunPage.tsx` 给这条腿接上 UI 之后，它第一次成为用户可达路径。
///
/// # 两半都要
///
///  · **正面**：`172.16.5.0/24` 必须真的出现在 `route_exclude_address` 里。少了这一半，
///    「Android 上恒空」这种实现也能让下面的否定断言全绿 —— 那正是本批要消灭的「填了不生效」。
///  · **反面**：`127.0.0.0/8` / `127.9.0.0/16` / `::1/128` 一条都不许发。
///    `VpnService.Builder.excludeRoute` 拒收回环前缀 ⇒ 一条被拒整个 `establish()` 失败，
///    症状是隧道一次都建不起来（依据见 `builder/inbounds.rs` android 基线那段的模拟器实测）。
///
/// `bypassLAN: false` 是为了让本条只量用户声明段这一条腿（开关开着时的派生段归下面的
/// `android_bypass_lan_*` 系列）。
#[test]
fn android_keeps_user_segments_but_drops_loopback_prefixes() {
    let cfg: UserConfig = serde_json::from_str(
        r#"{
            "servers": [],
            "selectedServerId": null,
            "proxyModeType": "tun",
            "bypassLAN": false,
            "tunConfig": { "inboundExcludeCidrs": [
                "172.16.5.0/24", "127.0.0.0/8", "127.9.0.0/16", "::1/128", "10.7.0.0/16"
            ] }
        }"#,
    )
    .expect("测试 config 反序列化失败");

    let got = tun_exclude(&cfg, &deps("android"));
    assert_eq!(
        got.as_deref(),
        Some(["172.16.5.0/24".to_string(), "10.7.0.0/16".to_string()].as_slice()),
        "Android 的用户声明段派生不符：非回环段必须原样保留、回环段必须被剔"
    );

    // iOS 同答（`drop_mobile_loopback_excludes` 的两条依据见该函数头注：Android 那半是实测，
    // iOS 那半是「排除回环买不到任何东西、代价上限是隧道建不起来」——与本仓 ios 平台基线
    // 那一臂同一条已采纳的推理）。
    assert_eq!(
        tun_exclude(&cfg, &deps("ios")).as_deref(),
        Some(["172.16.5.0/24".to_string(), "10.7.0.0/16".to_string()].as_slice()),
        "iOS 的用户声明段派生与 Android 不同答 —— 两者是同一条处置，别让其中一条悄悄漂走"
    );

    // 跨平台对照：同一份输入在 darwin 上那两条回环用户段**照样在**（`127.9.0.0/16` 只可能来自
    // 用户段，平台基线里没有它）⇒ 上面那两条否定断言是「移动端这一支剔的」，不是
    // 「这两条在哪儿都会被剔」，也不是「整个特性根本没接上」。
    let mac = tun_exclude(&cfg, &deps("darwin")).expect("darwin 应有排除段");
    assert!(
        mac.contains(&"127.9.0.0/16".to_string()),
        "darwin 也把回环用户段剔了 ⇒ 上面的移动端断言量的不是平台分支。实得 {mac:?}"
    );
}

/// 正向对照：Android 上关掉「绕过局域网」、一条用户段都没填时，`route_exclude_address` 整个字段不下发。
///
/// 与上一条成对：上一条证明「填了会进」，这一条证明「没填不会凭空长出来」——
/// 只有前者时，一个把平台基线又加回 android 的改动不会红。
/// （2026-09-25 起开关开着时 android 会发局域网段，故这里显式关掉开关，量的仍是平台基线。）
#[test]
fn android_emits_no_route_exclude_address_without_user_segments() {
    let baseline: UserConfig = serde_json::from_str(
        r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun","bypassLAN":false}"#,
    )
    .unwrap();
    assert_eq!(
        tun_exclude(&baseline, &deps("android")),
        None,
        "Android 平台基线必须恒空（回环流量根本不进 VPN 路由表，发它只会被 Builder 拒收）"
    );

    // 全部条目都是回环 ⇒ 剔完之后与「没填过」逐字节等价，不许留一个空数组。
    let all_loopback: UserConfig = serde_json::from_str(
        r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun","bypassLAN":false,
            "tunConfig":{"inboundExcludeCidrs":["127.0.0.0/8","::1/128"]}}"#,
    )
    .unwrap();
    assert_eq!(
        tun_exclude(&all_loopback, &deps("android")),
        None,
        "全剔除后应与未设字段字节等价，不得留 `route_exclude_address: []`"
    );
}

// ─── Android × 「绕过局域网」（2026-09-25 接线）───────────────────────────────────────
//
// 此前 android 臂恒空，`bypassLAN` / `bypassLANList` 在移动端登记为 platform-absent。依据只有
// 「`VpnService.Builder.excludeRoute` 拒收回环前缀」这一条实证 —— 它不覆盖 RFC1918。现在
// android 臂在开关开着时发 `bypass_lan_cidrs` 的结果，回环照样剔。下面几条逐格钉：
// 缺省清单全量金样 / 关 / 自定义清单 / 与用户声明段并存 / 组网段挖洞 / 桌面不变。

/// 缺省清单（`bypassLANList` 缺席 ⇒ `DEFAULT_BYPASS_LAN`）在 Android 上的**全量**排除表。
///
/// 每一段为什么长这样：
///  · `127.0.0.0/8` **不在** —— 回环前缀被 `drop_mobile_loopback_excludes` 剔（Builder 拒收）；
///  · `172.16.0.0/12` 被拆成四段、唯独缺 `172.19.0.0/16` —— 那是 TUN 自身网段（默认
///    `172.19.0.1/16`，隧道 DNS 地址 172.19.0.2 在里面），被挖掉；
///  · `224.0.0.0/4` / `240.0.0.0/4` **在** —— 只剔非法与 `/0`，不套 `normalize_tun_exclude_cidr` 的
///    `/8` 下限（那会每次起核把组播段剔掉）；
///  · 域名 / `localhost` / `*.local` 不在 —— `bypass_lan_cidrs` 只取网段；
///  · 顺序 = 默认清单的声明顺序（拆出来的四段按地址升序占原位）。
#[test]
fn android_bypass_lan_default_list_golden() {
    let cfg: UserConfig =
        serde_json::from_str(r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun"}"#)
            .unwrap();
    let expected = [
        "10.0.0.0/8",
        "100.64.0.0/10",
        "169.254.0.0/16",
        "172.16.0.0/15",
        "172.18.0.0/16",
        "172.20.0.0/14",
        "172.24.0.0/13",
        "192.0.0.0/24",
        "192.88.99.0/24",
        "192.168.0.0/16",
        "224.0.0.0/4",
        "233.252.0.0/24",
        "240.0.0.0/4",
        "fc00::/7",
        "fe80::/10",
    ]
    .map(String::from);
    let got = tun_exclude(&cfg, &deps("android"));
    assert_eq!(
        got.as_deref(),
        Some(expected.as_slice()),
        "Android 缺省「绕过局域网」排除表金样不符"
    );
}

/// 判据（本批验收的那一条）：**开着时含 RFC1918 三段、且一条回环都没有**；关掉时整个字段不发。
///
/// 与上一条全量金样分开写：金样可以被人按实得刷掉，这一条的期望是手写的判据本身。
/// 反面对照（关）写在同一个测里：只写正面会被「无论开关都发」骗过。
#[test]
fn android_bypass_lan_on_carries_rfc1918_and_no_loopback_off_carries_nothing() {
    let on: UserConfig =
        serde_json::from_str(r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun"}"#)
            .unwrap();
    let got =
        tun_exclude(&on, &deps("android")).expect("开着「绕过局域网」时 Android 必须下发排除段");
    // 172.16.0.0/12 被挖掉 TUN 自身网段后以子段形式出现，逐段判「被覆盖」不判字面相等。
    for (rfc1918, probe) in [
        ("10.0.0.0/8", "10.1.2.3"),
        ("172.16.0.0/12", "172.16.9.9"),
        ("192.168.0.0/16", "192.168.1.1"),
    ] {
        assert!(
            got.iter().any(|c| covers(c, probe)),
            "开关开着，{rfc1918}（探针 {probe}）没进 Android 的 route_exclude_address。实得 {got:?}"
        );
    }
    let loopback: Vec<&String> = got
        .iter()
        .filter(|c| {
            c.split('/')
                .next()
                .and_then(|a| a.parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        })
        .collect();
    assert!(
        loopback.is_empty(),
        "Android 的 route_exclude_address 带了回环前缀 {loopback:?} —— Builder 拒收，隧道建不起来"
    );
    // TUN 自身网段（隧道 DNS 地址 172.19.0.2 所在）不许被排除。
    assert!(
        !got.iter().any(|c| covers(c, "172.19.0.2")),
        "TUN 自身网段被排出了隧道（隧道 DNS 地址 172.19.0.2 落在 {got:?} 里）"
    );

    let off: UserConfig = serde_json::from_str(
        r#"{"servers":[],"selectedServerId":null,"proxyModeType":"tun","bypassLAN":false}"#,
    )
    .unwrap();
    assert_eq!(
        tun_exclude(&off, &deps("android")),
        None,
        "关掉「绕过局域网」后 Android 不许再发任何排除段"
    );
}

/// 自定义清单：只取网段、回环剔、非法与 `/0` 剔（warn）、IPv6 开时 TUN v6 网段被挖；
/// 并与用户声明段（`inboundExcludeCidrs`）并存 —— 顺序是绕过段在前、用户段在后。
#[test]
fn android_bypass_lan_custom_list_with_user_segments() {
    let cfg: UserConfig = serde_json::from_str(
        r#"{
            "servers": [],
            "selectedServerId": null,
            "proxyModeType": "tun",
            "enableIPv6": true,
            "bypassLAN": true,
            "bypassLANList": [
                "192.168.50.0/24", "127.0.0.0/8", "::1/128", "*.local", "localhost",
                "256.1.1.1/24", "0.0.0.0/0", "fd00::/8", "10.0.0.0/8"
            ],
            "tunConfig": { "inboundExcludeCidrs": ["172.16.5.0/24"] }
        }"#,
    )
    .expect("测试 config 反序列化失败");
    thread_local!(static SINK: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) });
    fn log(_: polaris_config_engine::user_config::log_level::LogLevel, m: &str) {
        SINK.with(|s| s.borrow_mut().push(m.to_string()));
    }
    let mut d = deps("android");
    d.log = log;
    let got = tun_exclude(&cfg, &d);
    let logs = SINK.with(|s| s.take());
    // fd00::/8 挖掉 TUN v6 网段 fdfe:dcba:9876::/126 之后拆成若干子段；这里先钉 v4 与顺序，
    // v6 判「被覆盖 / 未覆盖」。
    let got = got.expect("Android 应下发排除段");
    assert_eq!(
        got.first().map(String::as_str),
        Some("192.168.50.0/24"),
        "绕过段应排在最前（声明顺序）。实得 {got:?}"
    );
    assert!(got.contains(&"10.0.0.0/8".to_string()), "实得 {got:?}");
    assert_eq!(
        got.last().map(String::as_str),
        Some("172.16.5.0/24"),
        "用户声明段应追加在最后。实得 {got:?}"
    );
    assert!(
        got.iter().any(|c| covers(c, "fd00::1")),
        "fd00::/8 没进。实得 {got:?}"
    );
    assert!(
        !got.iter().any(|c| covers(c, "fdfe:dcba:9876::2")),
        "TUN v6 网段被排出了隧道。实得 {got:?}"
    );
    for bad in [
        "127.0.0.0/8",
        "::1/128",
        "256.1.1.1/24",
        "0.0.0.0/0",
        "*.local",
        "localhost",
    ] {
        assert!(
            !got.contains(&bad.to_string()),
            "{bad} 不该出现。实得 {got:?}"
        );
    }
    assert!(
        logs.iter()
            .any(|m| m.contains("「绕过局域网」剔除 2 条非法网段")),
        "非法 / `/0` 两条被剔却没出声。日志 {logs:?}"
    );

    // 桌面不变（反向对照）：同一份输入在 darwin 上绕过清单一条都不进排除表（darwin 走 route 规则），
    // 只有平台基线 + 用户声明段。
    assert_eq!(
        tun_exclude(&cfg, &deps("darwin")).as_deref(),
        Some(
            ["127.0.0.0/8", "::1/128", "172.16.5.0/24"]
                .map(String::from)
                .as_slice()
        ),
        "darwin 的排除表被移动端改动带歪了"
    );
}

/// 生效组网段从绕过段里挖掉：Tailscale 类节点 engaged 时，默认清单里的 `100.64.0.0/10` 不许把
/// tailnet 段排出隧道（否则组网节点静默不可达）。复用 Windows 那条 carve 算术，这里钉它在 android 上也生效。
#[test]
fn android_bypass_lan_carves_engaged_mesh() {
    let cfg: UserConfig = serde_json::from_str(
        r#"{
            "servers": [
                {
                    "id": "wg-1",
                    "name": "mesh",
                    "protocol": "wireguard",
                    "address": "wg.example.com",
                    "port": 51820,
                    "wireguardSettings": { "allowedIPs": ["10.66.0.0/16"] }
                }
            ],
            "selectedServerId": null,
            "proxyModeType": "tun"
        }"#,
    )
    .expect("测试 config 反序列化失败");
    let got = tun_exclude(&cfg, &deps("android")).expect("Android 应下发排除段");
    assert!(
        !got.iter().any(|c| covers(c, "10.66.1.1")),
        "engaged 组网段 10.66.0.0/16 被 10.0.0.0/8 排出了隧道。实得 {got:?}"
    );
    assert!(
        got.iter().any(|c| covers(c, "10.1.1.1")),
        "10.0.0.0/8 其余部分应仍被排除。实得 {got:?}"
    );
}

/// `cidr` 是否覆盖地址 `ip`（测试侧独立实现，不借生产的 cidr 工具，免得判据与被测同源）。
fn covers(cidr: &str, ip: &str) -> bool {
    let Some((net, len)) = cidr.split_once('/') else {
        return false;
    };
    let (Ok(net), Ok(ip), Ok(len)) = (
        net.parse::<std::net::IpAddr>(),
        ip.parse::<std::net::IpAddr>(),
        len.parse::<u32>(),
    ) else {
        return false;
    };
    match (net, ip) {
        (std::net::IpAddr::V4(n), std::net::IpAddr::V4(a)) => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            (u32::from(n) & mask) == (u32::from(a) & mask)
        }
        (std::net::IpAddr::V6(n), std::net::IpAddr::V6(a)) => {
            let mask = if len == 0 {
                0
            } else {
                u128::MAX << (128 - len)
            };
            (u128::from(n) & mask) == (u128::from(a) & mask)
        }
        _ => false,
    }
}

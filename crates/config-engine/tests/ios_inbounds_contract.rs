//! iOS 分支的**行为**断言 —— `build_inbounds` 的三处 iOS 答案各钉一条。
//!
//! # 为什么必须单开一道（2026-09-06 验收实测的缺口）
//!
//! 2026-09-06 那批给 iOS 在 `builder/inbounds.rs` 上写了三个答案：**不发 mixed 入站**、
//! **`route_exclude_address` 恒空**、**`strict_route` 一个键都不发**。三个答案当时**一条行为
//! 断言都没有** —— 变异实测把三处答案逐个翻转（把 `&& deps.platform != "ios"` 删掉、
//! 把 iOS 那条空集臂删掉、把 `|| deps.platform == "ios"` 删掉），`cargo test --workspace`
//! **全绿**。
//!
//! 而同形的三条 Android 答案各有一条专门的金样门（`golden_inbounds_android.rs` 的
//! `mixed_inbound_is_absent_on_android_and_present_elsewhere` /
//! `android_tun_route_exclude_carries_lan_but_never_loopback` /
//! `android_tun_never_carries_strict_route_and_desktop_still_does`）。
//! （2026-09-25：`route_exclude_address` 那条答案改为「平台基线恒空 + 开着「绕过局域网」时加局域网段、
//! 回环前缀一律剔」，与 Android 同做，见下面那条测的头注。）
//!
//! 两个平台在同一个文件里写着同向的答案，一个有门、一个没有 —— 这就是「门在但没牙」的
//! 另一种形态：牙长在隔壁那条腿上。
//!
//! # 判据的取材：**输入**借 Android 夹具，**期望值**逐条手写
//!
//! 本仓没有、也不该有 `fixtures/inbounds-ios.json`：自撰夹具靠字节对拍会退化成自证
//! （把实现的输出抄成期望，实现错了期望也跟着错），而 iOS 侧连一个可参照的第二实现都没有。
//! 故本文件**不做字节对拍**，只做结构断言，且：
//!
//! - **输入**取 `fixtures/inbounds-android.json` 里那些 case 的 `config`/`ports`（真实配置，
//!   含「16 条预设全 proxy」那份默认配置），把平台换成 `"ios"` 跑一遍；
//! - **期望值**是本文件里手写的字面量，不从任何夹具读。夹具被人「顺手对齐」成错的，本门仍红。
//!
//! # 每条都带反向对照，理由与 Android 那三条同源
//!
//! 只写「iOS 不发 X」会被「**哪个平台都不发 X**」骗过 —— 那是把桌面的对应能力一起删掉，
//! 却同样满足前半条。故每条都成对：iOS 侧断言缺席，桌面侧断言仍在场。
//!
//! 另有一条**正面**断言（[`ios_always_gets_a_tun_inbound_whatever_the_stored_mode`]）：
//! 上面三条全是否定式，「iOS 上一个入站都不生成」会让它们**同时**平凡成立，而那正是本仓
//! 最怕的那种静默阻断（隧道建得起来、界面显示已连接、流量进去没出口、全程不报错）。
//!
//! # 射程（本门证明不了什么）
//!
//! 本门跑在**宿主**上，判的是「给定 `platform == "ios"` 这个输入，生成侧输出什么」。
//! 它**证明不了** iOS 上真跑起来会怎样：本仓构不出 iOS 产物，`NEPacketTunnelNetworkSettings`
//! 收不收回环排除、`strict_route` 在 `tun_darwin.go` 上落在哪里，一条都没有实测。
//! 那些留在各自的代码注释里标着「未验证」，本门不假装覆盖它们。

use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
use polaris_config_engine::user_config::app_config::UserConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    input: CaseInput,
}

#[derive(Debug, Deserialize)]
struct CaseInput {
    config: serde_json::Value,
    ports: Ports,
}

#[derive(Debug, Clone, Deserialize)]
struct Ports {
    #[serde(rename = "probeDirect")]
    probe_direct: Option<u16>,
    #[serde(rename = "probeProxy")]
    probe_proxy: Option<u16>,
    #[serde(rename = "updateIn")]
    update_in: Option<u16>,
    #[serde(rename = "probePool", default)]
    probe_pool: Vec<u16>,
}

/// 只借**输入**：`fixtures/inbounds-android.json` 的 `output` 一列本门一个字都不读。
fn cases() -> Vec<Case> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/inbounds-android.json"
    );
    // 不静默缩量：夹具没了必须转红，否则「门没跑」与「全通过」在 CI 上长得一模一样。
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读 fixtures/inbounds-android.json 失败: {e}"));
    let fixture: Fixture = serde_json::from_str(&raw).expect("夹具解析失败");
    assert!(
        fixture.cases.len() >= 6,
        "夹具只剩 {} 条 case —— 取材面塌了",
        fixture.cases.len()
    );
    fixture.cases
}

fn run(config: &serde_json::Value, platform: &str, ports: &Ports) -> Vec<serde_json::Value> {
    let cfg: UserConfig = serde_json::from_value(config.clone())
        .unwrap_or_else(|e| panic!("[{platform}] config 反序列化失败: {e}"));
    let deps = InboundsDeps {
        probe_direct_port: ports.probe_direct,
        probe_proxy_port: ports.probe_proxy,
        update_in_port: ports.update_in,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: ports.probe_pool.clone(),
        platform: platform.to_owned(),
        own_lan_cidrs: vec![],
        // 本门只量 iOS 的入站契约；组网观测地址给空值（空 map ⟺ 本轮零观测，与观测面存在之前同形）。
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    };
    build_inbounds(&cfg, None, &deps)
        .iter()
        .map(|ib| serde_json::to_value(ib).unwrap())
        .collect()
}

fn tag_of(v: &serde_json::Value) -> &str {
    v.get("tag")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

fn tun_of<'a>(out: &'a [serde_json::Value], who: &str) -> &'a serde_json::Value {
    out.iter()
        .find(|v| tag_of(v) == "tun-in")
        .unwrap_or_else(|| panic!("[{who}] 期望有 tun-in 入站，实得 {out:?}"))
}

/// 桌面三平台 —— 每条否定断言的反向对照面。
const DESKTOP: [&str; 3] = ["darwin", "win32", "linux"];

/// 把配置的接管方式改成 TUN 档。
///
/// **只用在「tun 字段」那两条的桌面/Android 对照上**：夹具里有两条 case 存的是
/// `systemProxy` / `manual`，桌面在那两档下**本就不该有 tun 入站**（`desktop_still_honours_
/// the_stored_proxy_mode_type` 正是钉这件事的），拿它们当 tun 字段的对照会把一条正确行为判成回归。
/// iOS 侧**不做这一步**：`ProxyModeType::effective_on(Ios)` 恒 `Tun`，读的就是原样存量值 ——
/// 那正是要被覆盖的输入面。
fn as_tun_mode(config: &serde_json::Value) -> serde_json::Value {
    let mut cfg = config.clone();
    cfg["proxyModeType"] = serde_json::Value::String("tun".to_owned());
    cfg
}

/// **iOS 不生成零认证的 loopback `mixed-in`；桌面三平台仍生成。**
///
/// # 守的是什么
///
/// iOS 的 `127.0.0.1` 与 Android 一样是**全设备应用共享**的回环（无 per-app loopback 命名空间），
/// 而 `mixed` 入站结构体没有任何认证字段 ⇒ 发了就是一个零认证的本地全代理口，
/// 任何一个在前台跑着的第三方 app 都能连。iOS 上甚至连一个正当消费者都没有：
/// 系统里没有让用户把第三方应用的出流量指到 `127.0.0.1:P` 的设置面。
///
/// # 为什么反向对照必须写在同一个测里
///
/// 只断言「iOS 无 mixed」会被「谁都不发 mixed」骗过 —— 那是桌面本地代理入口整个消失，
/// 断网级回归，却同样满足前半条。
///
/// # 顺带钉住 Android 那条腿仍在
///
/// 判据是 `emits_mixed_inbound` 的 `Platform::Android | Platform::Ios => false` 臂（2026-09-25 α 批
/// 从字符串合取改成穷举 `match`）。只把 `Ios` 挪到另一臂，Android 侧照样绿 —— 故两条腿都在这里
/// 各查一次，本测能分辨「挪了哪一个」。
#[test]
fn mixed_inbound_is_absent_on_ios_and_present_on_desktop() {
    let cases = cases();
    let (mut ios_checked, mut desktop_checked) = (0usize, 0usize);
    for case in &cases {
        let out = run(&case.input.config, "ios", &case.input.ports);
        assert!(
            !out.iter().any(|v| tag_of(v) == "mixed-in"),
            "[{}] iOS 不许生成零认证的 loopback mixed 入站 —— 回环全设备共享、结构体零认证字段，\
             且 iOS 上没有任何设置面能把第三方应用的流量指过来（连正当消费者都没有）。实得 {out:?}",
            case.name
        );
        ios_checked += 1;

        // 姊妹腿：同一条合取判据的另一半。
        assert!(
            !run(&case.input.config, "android", &case.input.ports)
                .iter()
                .any(|v| tag_of(v) == "mixed-in"),
            "[{}] Android 侧的 mixed 排除腿被一起改掉了",
            case.name
        );

        // 反向对照：桌面必须仍有。
        for p in DESKTOP {
            let out = run(&case.input.config, p, &case.input.ports);
            assert!(
                out.iter().any(|v| tag_of(v) == "mixed-in"),
                "[{}] 桌面（{p}）必须保留 mixed-in —— 平台判据写歪会把桌面一起打掉。实得 {out:?}",
                case.name
            );
            desktop_checked += 1;
        }
    }
    // 取材面自检：两侧任一没跑到，上面的断言就恒真、本门零信息量。
    assert!(
        ios_checked >= 6 && desktop_checked >= 18,
        "取材面塌了（ios={ios_checked}, desktop={desktop_checked}）"
    );
}

/// **iOS 的 tun：`route_exclude_address` 不含回环前缀、缺省开「绕过局域网」时含局域网段；
/// 桌面（darwin）仍发回环排除。**
///
/// # 守的是什么
///
/// iOS 此前落在最后那个 `else` 上，拿到 `127.0.0.0/8` + `::1/128` —— 与 Android 那条阻断级缺陷
/// （`VpnService.Builder.excludeRoute()` 拒收回环前缀 ⇒ `establish()` 整个失败 ⇒ 隧道一次都
/// 建不起来，2026-09-04 模拟器实证）**同一个落点**。
///
/// 不发回环的承重依据**不是**那次实测（`NEPacketTunnelNetworkSettings` 收不收回环，
/// 本仓构不出产物、没验过），而是另一半、且那一半不依赖任何实测：**回环流量根本不进隧道的
/// 路由表**（内核在 lo 上直接闭环）⇒ 发这两条前缀买不到任何东西，而代价上限是阻断级。
/// 默认 `bypassLANList` 里就有 `127.0.0.0/8`，故这枚输入每份配置都会经过生成侧。
///
/// 正面那半（2026-09-25）：移动端 UI 两端共用一份，「绕过局域网」开关在 iOS 上也显示 ⇒
/// iOS 与 Android 同做；夹具 case 都没写 `bypassLAN`（缺省开）⇒ 局域网段必须在。只写反面的话，
/// 「iOS 臂退回恒空」会让本测全绿。
///
/// # 反向对照取 darwin 而不是三平台
///
/// 回环排除那条 `else` 只有 mac 与未知平台走得到（win32 / linux / android / ios 各有具名支）。
/// 拿三平台做对照会把「Linux 恒空」这条**正确的**空集也判成回归。
#[test]
fn ios_tun_route_exclude_carries_lan_but_never_loopback_and_darwin_keeps_loopback() {
    let cases = cases();
    let (mut ios_checked, mut darwin_checked) = (0usize, 0usize);
    for case in &cases {
        let out = run(&case.input.config, "ios", &case.input.ports);
        let tun = tun_of(&out, &format!("ios/{}", case.name));
        let excl: Vec<&str> = tun
            .get("route_exclude_address")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] 「绕过局域网」缺省开着，iOS 的 tun 却没有 route_exclude_address —— \
                     ios 臂退回了恒空。实得 {tun}",
                    case.name
                )
            })
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        // 期望值手写字面量，不从夹具读。
        for lan in ["10.0.0.0/8", "192.168.0.0/16"] {
            assert!(
                excl.contains(&lan),
                "[{}] 缺省清单里的 {lan} 没进 iOS 的 route_exclude_address。实得 {excl:?}",
                case.name
            );
        }
        for cidr in ["127.0.0.0/8", "::1/128"] {
            assert!(
                !excl.contains(&cidr),
                "[{}] iOS 的 tun 发了回环排除 {cidr} —— 回环流量根本不进隧道路由表，\
                 发它买不到任何东西；而若 NE 也像 VpnService 那样拒收回环前缀，代价是整条隧道\
                 建不起来。实得 {excl:?}",
                case.name
            );
        }
        ios_checked += 1;

        // 反向对照：mac 那条 `else` 仍逐字下发回环排除。少了它，「谁都不发」会把本测骗绿，
        // 而那是把 macOS 的回环排除静默删掉。
        let mac_out = run(
            &as_tun_mode(&case.input.config),
            "darwin",
            &case.input.ports,
        );
        let mac_tun = tun_of(&mac_out, &format!("darwin/{}", case.name));
        let mac_excl = mac_tun
            .get("route_exclude_address")
            .unwrap_or_else(|| panic!("[{}] macOS 的回环排除不见了：{mac_tun}", case.name));
        // 期望值手写字面量，不从夹具读。
        for cidr in ["127.0.0.0/8", "::1/128"] {
            assert!(
                mac_excl.to_string().contains(cidr),
                "[{}] macOS 的 route_exclude_address 少了 {cidr} —— 平台判据写歪会把桌面一起打掉。\
                 实得 {mac_excl}",
                case.name
            );
        }
        darwin_checked += 1;
    }
    assert!(
        ios_checked >= 6 && darwin_checked >= 6,
        "取材面塌了（ios={ios_checked}, darwin={darwin_checked}）"
    );
}

/// **iOS 的 tun 一个 `strict_route` 键都不发；桌面三平台仍发 `true`。**
///
/// # 守的是什么
///
/// 承重依据是仓内可查的那条：移动端 UI 是 Android/iOS **共用的一份**
/// （`ui/mobile.html` → `ui/src/mobile/*`），而 `ui/src/mobile/settings/TunPage.tsx` 里
/// 这个开关整个不显示。iOS 照发 `true` 就成了「UI 上没有这个东西、配置里却写着防绕行已开启」——
/// 与 Android 那条要避免的形态逐字相同。
///
/// **不拿 Android 那条源码链当依据**：那条的落点是 `sing-tun/tun_linux.go`，
/// 而 Apple 上参与编译的是 `tun_darwin.go`，本仓没读过它。
///
/// # 「不发键」与「发 false」不是一回事
///
/// 发 `false` 会把「本平台没有这个能力」与「用户主动关掉了」在配置里写成同一个样子；
/// 缺席才是诚实的表达。故断言是 `is_none()` 而不是 `== false`。
#[test]
fn ios_tun_never_carries_strict_route_and_desktop_still_does() {
    let cases = cases();
    let (mut ios_checked, mut desktop_checked) = (0usize, 0usize);
    for case in &cases {
        let out = run(&case.input.config, "ios", &case.input.ports);
        let tun = tun_of(&out, &format!("ios/{}", case.name));
        assert!(
            tun.get("strict_route").is_none(),
            "[{}] iOS 的 tun 发了 strict_route —— 移动端 UI（Android/iOS 共用的那一份）里\
             根本没有这个开关，发它等于在配置里写一句界面从未说过的话。实得 {tun}",
            case.name
        );
        ios_checked += 1;

        // 姊妹腿：`deps.platform == \"android\" || deps.platform == \"ios\"` 的另一半。
        let android_out = run(
            &as_tun_mode(&case.input.config),
            "android",
            &case.input.ports,
        );
        assert!(
            tun_of(&android_out, "android")
                .get("strict_route")
                .is_none(),
            "[{}] Android 侧的 strict_route 缺席腿被一起改掉了",
            case.name
        );

        // 反向对照：桌面仍逐字下发 `true`（防绕行泄漏的开关，缺省 true）。
        for p in DESKTOP {
            let out = run(&as_tun_mode(&case.input.config), p, &case.input.ports);
            let tun = tun_of(&out, &format!("{p}/{}", case.name));
            assert_eq!(
                tun.get("strict_route"),
                Some(&serde_json::json!(true)),
                "[{}] 桌面（{p}）的 strict_route 不见了 —— 那是防绕行泄漏的开关，\
                 平台判据写歪会把桌面一起打掉。实得 {tun}",
                case.name
            );
            desktop_checked += 1;
        }
    }
    assert!(
        ios_checked >= 6 && desktop_checked >= 18,
        "取材面塌了（ios={ios_checked}, desktop={desktop_checked}）"
    );
}

/// **正面断言**：iOS 上三个 `proxy_mode_type` 档位都产出 tun 入站，且三份逐字节相同。
///
/// # 没有这一条，上面三条否定断言会被「iOS 什么都不生成」同时骗过
///
/// 那不是理论风险：`UserConfig` 的 `proxy_mode_type` **缺省值是 `systemProxy`**，
/// 而 iOS 上「系统代理 / 手动」都没有承载物（无第三方 API 改系统 HTTP 代理；没有任何设置面
/// 能把应用流量指到本地端口）。照裸值分流 ⇒ mixed 本就不发、tun 又因为「不是 TUN 档」不发
/// ⇒ **一个用户流量入站都没有**：隧道建得起来、核跑得起来、界面显示已连接，流量进去没出口，
/// 且全程不报错。承重面是 `ProxyModeType::effective_on` 的 `Ios` 臂（恒 `Tun`），
/// 本条就是那条臂在生成侧的行为收据。
///
/// # 判据为什么是「三份逐字节相同」而不是「都非空」
///
/// 「都非空」会被「三档各生成一份不同的 tun」骗过 —— 那正是半开状态的样子：两条路都能跑，
/// 生成的东西不一样，而没有任何一处代码为那个差异答过题。
#[test]
fn ios_always_gets_a_tun_inbound_whatever_the_stored_mode() {
    let cases = cases();
    let base = cases
        .iter()
        .find(|c| c.name == "android_tun_no_mixed")
        .expect("夹具缺 android_tun_no_mixed");

    let mut per_mode: Vec<(&str, Vec<serde_json::Value>)> = Vec::new();
    for mode in ["systemProxy", "manual", "tun"] {
        let mut cfg = base.input.config.clone();
        cfg["proxyModeType"] = serde_json::Value::String(mode.to_owned());
        let out = run(&cfg, "ios", &base.input.ports);
        assert!(
            out.iter().any(|v| tag_of(v) == "tun-in"),
            "iOS + {mode}: 没有 tun 入站 —— 这份配置一个用户流量入站都没有，\
             隧道建得起来但流量进去没出口，且不报错。实得 {out:?}"
        );
        per_mode.push((mode, out));
    }
    for (mode, out) in &per_mode[1..] {
        assert_eq!(
            out, &per_mode[0].1,
            "iOS + {mode} 与 iOS + {} 产出的入站不一致 —— 那个平台上只有一种接管形态，\
             两份不同的配置里必有一份没人为它答过题",
            per_mode[0].0
        );
    }

    // 反向对照：桌面仍按存量档位办事，`systemProxy` 下**不许**凭空多出 TUN。
    // 少了它，「谁都恒发 tun」会把上面骗绿，而那是用户选了系统代理却被接管了整机路由。
    for p in DESKTOP {
        let mut cfg = base.input.config.clone();
        cfg["proxyModeType"] = serde_json::Value::String("systemProxy".to_owned());
        let out = run(&cfg, p, &base.input.ports);
        assert!(
            !out.iter().any(|v| tag_of(v) == "tun-in"),
            "{p} + systemProxy: 桌面上这一档是真实存在的接管形态，不许被接管成 TUN。实得 {out:?}"
        );
    }
}

//! Android 分支金样对拍 —— `build_inbounds` 的 Android 面（与 `golden_inbounds.rs` 同形态）。
//!
//! # 为什么必须单开一道，而不是靠既有 13 条桌面金样
//!
//! `build_inbounds` 有 25 个调用方，桌面金样全绿只说明**桌面那条路**没变；Android 分支是新长出来的
//! 一条腿，桌面金样在它上面**一个字节都没跑过**。「桌面测试没红」在这里是标准的「什么都没发生」型
//! 绿 —— 把 Android 分支整块删掉，桌面 13 条照样全绿。
//!
//! # 夹具的来历（与 `inbounds.json` 不同，读之前必看）
//!
//! `fixtures/inbounds.json` 是**冻结的 TS builder 导出**，那边有独立参照物。Android 分支在 TS 侧
//! 从不存在，没有第二个实现可导出 ⇒ `fixtures/inbounds-android.json` 是**本仓自撰**的期望值。
//! 自撰夹具单靠字节对拍会退化成自证（把实现的输出抄成期望，实现错了期望也跟着错），故本文件的
//! 判据是**两半**：
//!
//! 1. **字节对拍**（[`android_inbounds_match_golden`]）—— 抓任何未预期的形态漂移。
//! 2. **结构断言**（其余各测）—— 判据**不从夹具读**，逐条手写字面量，直接钉映射表本身。
//!    夹具即使被人「顺手对齐」成错的，这一半仍会红。
//!
//! # 判据清单（正面 + 反面 + 桌面不变，三样齐）
//!
//! | 面 | 断言 |
//! |---|---|
//! | 正面 | Android + TUN 模式下 `tun-in` 在场 |
//! | 正面 | `direct` 档的应用逐字进 `exclude_package`（值是手写字面量，不是从夹具抄的） |
//! | 正面 | Android 上 `proxy_mode_type` 三个档位**都**产出 tun 入站，且三份逐字节相同 |
//! | 反面 | Android 下 `mixed-in` **不在场**（任何 case、任何模式） |
//! | 反面 | `include_package` **恒不发射**（含「16 条预设全 proxy」这份默认配置） |
//! | 反面 | `proxy` / `block` 两档**不进** `exclude_package` |
//! | 反面 | Android 的 tun 的 `route_exclude_address` 不含回环前缀（缺省开「绕过局域网」时**必含** RFC1918 段；开/关与用户段见 `golden_tun_inbound_exclude.rs`）、**不发** `strict_route` |
//! | 桌面不变 | 同一份输入在 linux/darwin/win32 上 `mixed-in` 仍在场、两个 package 键都不发 |
//! | 桌面不变 | 桌面三平台 × `systemProxy`/`manual` 仍然**不发** tun 入站（存量档位仍然算数） |
//! | 正面 | Android 的六个回环探针/更新入站**全部**带注入的那份凭据（`users`）；缺凭据则整批不发（fail-closed） |
//! | 桌面不变 | 其余平台同样六个入站在场且**零** `users` 键（注入了凭据也不用） |

use polaris_config_engine::builder::inbounds::{
    build_inbounds, loopback_inbounds_require_auth, InboundsDeps,
};
use polaris_config_engine::builder::Platform;
use polaris_config_engine::singbox::InboundUser;
use polaris_config_engine::user_config::app_config::UserConfig;
use serde::Deserialize;

/// 回环入站凭据的**占位值**（夹具里写的就是这两个串）。
///
/// 生产上每次起核由 CSPRNG 现生成（`src-tauri` 的 `ProxyRuntime::loopback_auth_for_start`），随机值
/// 永不进夹具；金样只钉「这批入站带着**调用方注入的那一份**凭据」—— 注入什么就发射什么，
/// 占位值原样出现在输出里即证明发射的就是注入值、没有被替换也没有被丢掉。
fn placeholder_auth() -> InboundUser {
    InboundUser {
        username: "<loopback-user>".into(),
        password: "<loopback-password>".into(),
    }
}

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<AndroidCase>,
}

#[derive(Debug, Deserialize)]
struct AndroidCase {
    name: String,
    input: CaseInput,
    output: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CaseInput {
    config: serde_json::Value,
    platform: String,
    ports: Ports,
}

#[derive(Debug, Deserialize)]
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

fn load() -> Fixture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/inbounds-android.json"
    );
    // 不静默缩量：夹具没了必须转红，否则「门没跑」与「全通过」在 CI 上长得一模一样。
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读 fixtures/inbounds-android.json 失败: {e}"));
    let fixture: Fixture = serde_json::from_str(&raw).expect("Android 夹具解析失败");
    assert!(!fixture.cases.is_empty(), "Android 夹具为空");
    fixture
}

fn run_case(case: &AndroidCase) -> Vec<serde_json::Value> {
    let config: UserConfig = serde_json::from_value(case.input.config.clone())
        .unwrap_or_else(|e| panic!("[{}] config 反序列化失败: {e}", case.name));
    let deps = InboundsDeps {
        probe_direct_port: case.input.ports.probe_direct,
        probe_proxy_port: case.input.ports.probe_proxy,
        debug_probe_mixed_udp: false,
        update_in_port: case.input.ports.update_in,
        subscription_update_in_port: None,
        // 生产形态：运行期**恒**注入一份凭据（桌面也注入 —— 由生成侧按平台决定用不用）。
        // 桌面 case 因此同时是「桌面拿到凭据也不发射」的对照。
        loopback_auth: Some(placeholder_auth()),
        probe_pool_ports: case.input.ports.probe_pool.clone(),
        platform: case.input.platform.clone(),
        own_lan_cidrs: vec![],
        // 组网观测地址给空值（空 map ⟺ 本轮零运行期观测，行为与观测面存在之前逐字节相同）；
        // 本门量的是入站契约，与组网段结算无关。
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    };
    build_inbounds(&config, None, &deps)
        .iter()
        .map(|ib| serde_json::to_value(ib).unwrap())
        .collect()
}

/// 按「config JSON + 平台 + 端口」直接跑一次生成（不经夹具的 `output`）。
///
/// 结构断言用它按需**改写输入**（例如把 `proxyModeType` 换成三个档位），判据因此不依赖
/// 夹具里恰好存了哪几种组合 —— 与本文件头注「判据不从夹具读」同一条纪律。
fn run_config(config: &serde_json::Value, platform: &str, ports: &Ports) -> Vec<serde_json::Value> {
    let cfg: UserConfig = serde_json::from_value(config.clone())
        .unwrap_or_else(|e| panic!("[{platform}] config 反序列化失败: {e}"));
    let deps = InboundsDeps {
        probe_direct_port: ports.probe_direct,
        probe_proxy_port: ports.probe_proxy,
        debug_probe_mixed_udp: false,
        update_in_port: ports.update_in,
        subscription_update_in_port: None,
        loopback_auth: Some(placeholder_auth()),
        probe_pool_ports: ports.probe_pool.clone(),
        platform: platform.to_owned(),
        own_lan_cidrs: vec![],
        // 组网观测地址给空值（空 map ⟺ 本轮零运行期观测，行为与观测面存在之前逐字节相同）；
        // 本门量的是入站契约，与组网段结算无关。
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

fn tun_of<'a>(out: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    out.iter()
        .find(|v| tag_of(v) == "tun-in")
        .unwrap_or_else(|| panic!("[{name}] 期望有 tun-in 入站，实得 {out:?}"))
}

/// 逐 case 字节对拍（第一半判据）。
#[test]
fn android_inbounds_match_golden() {
    let fixture = load();
    let mut failures = Vec::new();
    for case in &fixture.cases {
        let got = run_case(case);
        if got != case.output {
            failures.push(format!(
                "[{}] @ {}\n  期望: {}\n  实得: {}",
                case.name,
                case.input.platform,
                serde_json::to_string(&case.output).unwrap(),
                serde_json::to_string(&got).unwrap()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "Android 金样 {}/{} 条失配:\n{}",
        failures.len(),
        fixture.cases.len(),
        failures.join("\n")
    );
}

/// **反面**：Android 上任何一条 case 都不许出现 `mixed-in`；**桌面**同一份输入必须仍有。
///
/// 两半写在同一个测里是刻意的：只写前半会被「哪个平台都不发 mixed」骗过（那是断网级回归，
/// 却同样满足「Android 无 mixed」）。后半就是那条反向对照，且它同时是「桌面不变」的守卫。
#[test]
fn mixed_inbound_is_absent_on_android_and_present_elsewhere() {
    let fixture = load();
    let (mut android_seen, mut desktop_seen) = (0usize, 0usize);
    for case in &fixture.cases {
        let out = run_case(case);
        let has_mixed = out.iter().any(|v| tag_of(v) == "mixed-in");
        if case.input.platform == "android" {
            android_seen += 1;
            assert!(
                !has_mixed,
                "[{}] Android 不许生成零认证的 loopback mixed 入站，实得 {out:?}",
                case.name
            );
        } else {
            desktop_seen += 1;
            assert!(
                has_mixed,
                "[{}] 桌面（{}）必须保留 mixed-in —— 平台判据写歪会把桌面一起打掉",
                case.name, case.input.platform
            );
        }
    }
    // 两侧都必须真有样本，否则上面的循环可以「一条都没跑」而全绿。
    assert!(
        android_seen >= 3 && desktop_seen >= 3,
        "夹具两侧样本不足（android={android_seen}, desktop={desktop_seen}）"
    );
}

/// **正面 + 方向**：`direct` 档逐字进 `exclude_package`，`proxy`/`block`/`disabled` 一律不进。
///
/// 期望值是**手写字面量**，不从夹具读 —— 夹具被改错时这条仍红。
#[test]
fn direct_app_rules_land_in_exclude_package_and_nothing_else_does() {
    let fixture = load();
    let case = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_direct_apps")
        .expect("夹具缺 android_tun_direct_apps");
    let out = run_case(case);
    let tun = tun_of(&out, &case.name);

    // 输入里 direct 且 enabled 的是 youtube / riot / github；riot 的 package_names 是空
    // （核实结论：Riot 在 Play 上没有单一官方客户端），故只剩两个包，顺序随 appRules。
    assert_eq!(
        tun.get("exclude_package"),
        Some(&serde_json::json!([
            "com.google.android.youtube",
            "com.github.android"
        ])),
        "exclude_package 必须恰好是 direct 档应用的包名"
    );

    // 方向守卫：netflix=proxy、telegram=block、spotify=direct 但 disabled，一个都不许在里面。
    // 「本该走代理的应用直连出网」正是 netflix 出现在这里的样子。
    let s = tun.get("exclude_package").unwrap().to_string();
    for forbidden in [
        "com.netflix.mediaclient", // proxy：排除它 = 本该走代理的应用直连出网
        "org.telegram.messenger",  // block：排除它 = 被阻断的应用拿到完整互联网
        "com.spotify.music",       // enabled=false：关掉的规则不许生效
    ] {
        assert!(
            !s.contains(forbidden),
            "{forbidden} 不该进 exclude_package，实得 {s}"
        );
    }
}

/// **反面（最要命的一条）**：`include_package` 恒不发射。
///
/// 它是白名单：非空即「只有列出的包进隧道，其余全部脱离 VPN」。而 `default_app_rules()` 把 16 条
/// 内置预设**全部默认置为 proxy** ⇒ 若把 proxy 档映到这里，一份用户零设置的默认配置就会把全设备
/// 其余流量踢出隧道明文直出。夹具里 `android_tun_all_presets_proxy` 就是那份默认配置。
#[test]
fn include_package_is_never_emitted() {
    let fixture = load();
    let mut checked = 0usize;
    for case in &fixture.cases {
        for ib in run_case(case) {
            assert!(
                ib.get("include_package").is_none(),
                "[{}] 不许发射 include_package（白名单语义），实得 {ib}",
                case.name
            );
        }
        checked += 1;
    }
    // 那份「16 条预设全 proxy」的默认配置必须真在夹具里跑过，否则本门没看过最危险的那个输入。
    let defaults = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_all_presets_proxy")
        .expect("夹具缺 android_tun_all_presets_proxy —— 少了这条本门就没看过默认配置");
    let out = run_case(defaults);
    let tun = tun_of(&out, &defaults.name);
    assert!(
        tun.get("exclude_package").is_none(),
        "全 proxy 的默认配置不该产生任何 exclude_package，实得 {tun}"
    );
    assert!(checked >= 3, "夹具样本不足");
}

/// **桌面不变**：同一份带 direct 应用规则的输入，在三个桌面平台上两个 package 键都不许出现。
///
/// Android 分支若漏了平台判据（例如判据写成恒真），桌面 TUN 会凭空多出 `exclude_package`，
/// 那是把用户的应用踢出隧道 —— 桌面上没有任何东西会消费它，但配置已经不是原来那份了。
#[test]
fn desktop_never_gets_package_keys() {
    let fixture = load();
    let mut seen = 0usize;
    for case in fixture
        .cases
        .iter()
        .filter(|c| c.input.platform != "android")
    {
        for ib in run_case(case) {
            assert!(
                ib.get("exclude_package").is_none() && ib.get("include_package").is_none(),
                "[{}] 桌面（{}）不许出现 package 键，实得 {ib}",
                case.name,
                case.input.platform
            );
        }
        seen += 1;
    }
    assert_eq!(seen, 3, "桌面三平台各须有一条同输入对照，实得 {seen}");
}

/// 平台枚举 → 生成侧吃的平台串（`process.platform` 口径）。
///
/// **穷举 `match`、无通配臂**：`Platform` 加变体时这里编译不过，逼人为新平台写下它的串 ——
/// 平台轴因此从 `Platform::ALL` 这一个枚举派生，而不是在测试里另抄一张名单
/// （另抄的名单漏一个平台时门照样全绿）。`Other` 取一个 `parse` 不认识的串，正是它在生产上的来历。
fn node_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "darwin",
        Platform::Win => "win32",
        Platform::Linux => "linux",
        Platform::Android => "android",
        Platform::Ios => "ios",
        Platform::Other => "freebsd",
    }
}

/// 五种回环探针/更新入站**全开**的端口组（`subscription-update-in` 与 `probe-in-k` 不在夹具里，
/// 故这里按需构造，判据不依赖夹具恰好存了哪几种组合）。
fn all_loopback_deps(platform: &str, auth: Option<InboundUser>) -> InboundsDeps {
    InboundsDeps {
        probe_direct_port: Some(31001),
        probe_proxy_port: Some(31002),
        debug_probe_mixed_udp: false,
        update_in_port: Some(31003),
        subscription_update_in_port: Some(31004),
        loopback_auth: auth,
        probe_pool_ports: vec![31005, 31006],
        platform: platform.to_owned(),
        own_lan_cidrs: vec![],
        observed_tailnet_addresses: Default::default(),
        log: |_, _| {},
    }
}

/// 回环入站的期望 tag 集（与 [`all_loopback_deps`] 的端口组逐一对应）。
const LOOPBACK_TAGS: [&str; 6] = [
    "probe-direct-in",
    "probe-proxy-in",
    "probe-in-0",
    "probe-in-1",
    "update-in",
    "subscription-update-in",
];

fn tun_config() -> UserConfig {
    serde_json::from_value(serde_json::json!({
        "proxyMode": "smart", "proxyModeType": "tun", "selectedServerId": null, "servers": []
    }))
    .expect("最小 TUN 配置可解析")
}

/// **回环入站凭据：逐平台正面断言**（本批 α 把原先的「射程自曝」改写成这一条）。
///
/// 原来那条测试把「Android 上 `probe-direct-in` / `probe-proxy-in` / `probe-in-k` / `update-in` /
/// 订阅更新入站仍零认证监听共享回环」钉成可见的现状，并写明「是另一批」—— 就是本批。
/// 在 Android 共享回环下，被 `exclude_package` 排出隧道的应用能连这批口拿到代理出口，
/// 按应用排除的承诺因此落空。
///
/// 判据（平台轴从 `Platform::ALL` 派生，逐平台二选一，两边都是**正面**断言）：
/// - 要求凭据的平台（今天只有 Android）：六个入站**全部在场**，且每一个的 `users` 恰为注入的那一份；
/// - 其余平台：六个入站**全部在场**，且一个 `users` 键都没有（桌面零认证不变，注入了凭据也不用）。
///
/// 两边都先断言「入站在场」：只断言「没有 users」会被「入站整个没发」骗过。
#[test]
fn loopback_inbounds_carry_credentials_exactly_where_required() {
    let cfg = tun_config();
    let mut required_seen = 0usize;
    let mut open_seen = 0usize;
    for platform in Platform::ALL.iter().copied() {
        let tag = node_tag(platform);
        assert_eq!(
            Platform::parse(tag),
            platform,
            "node_tag({platform:?}) = {tag:?} 解析不回同一个变体 —— 平台轴投影写歪了"
        );
        let out: Vec<serde_json::Value> = build_inbounds(
            &cfg,
            None,
            &all_loopback_deps(tag, Some(placeholder_auth())),
        )
        .iter()
        .map(|ib| serde_json::to_value(ib).unwrap())
        .collect();
        for want in LOOPBACK_TAGS {
            let ib = out
                .iter()
                .find(|v| tag_of(v) == want)
                .unwrap_or_else(|| panic!("[{tag}] 回环入站 {want} 不在场，实得 {out:?}"));
            assert_eq!(
                ib.get("listen").and_then(serde_json::Value::as_str),
                Some("127.0.0.1"),
                "[{tag}] {want} 必须仍只绑回环"
            );
            let users = ib.get("users");
            if loopback_inbounds_require_auth(platform) {
                assert_eq!(
                    users,
                    Some(&serde_json::json!([
                        {"username": "<loopback-user>", "password": "<loopback-password>"}
                    ])),
                    "[{tag}] {want} 必须带注入的那一份凭据"
                );
            } else {
                assert!(
                    users.is_none(),
                    "[{tag}] {want} 在不要求凭据的平台上不许发 users（桌面金样不变），实得 {ib}"
                );
            }
        }
        if loopback_inbounds_require_auth(platform) {
            required_seen += 1;
        } else {
            open_seen += 1;
        }
    }
    // 取材面自检：两条腿都真的跑过（否则上面任一分支的断言可能一次都没执行）。
    assert!(
        required_seen >= 1 && open_seen >= 3,
        "平台轴覆盖不足：要求凭据 {required_seen} / 零认证 {open_seen}"
    );
    // 裁定本身钉死：Android 要求，桌面三端不要求（改了判据就来这里改断言，并读 `loopback_inbounds_require_auth` 的文档）。
    assert!(loopback_inbounds_require_auth(Platform::Android));
    for desktop in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(
            !loopback_inbounds_require_auth(desktop),
            "{desktop:?} 不该要求凭据（桌面行为不变）"
        );
    }
}

/// **fail-closed**：要求凭据的平台上若没注入凭据，这批入站**整批不发射**，绝不退回零认证。
///
/// 正面同时断言 `tun-in` 仍在场：fail-closed 只压回环探针面，不能把用户流量入站一起带走。
#[test]
fn missing_credential_suppresses_loopback_inbounds_instead_of_opening_them() {
    let cfg = tun_config();
    let mut checked = 0usize;
    for platform in Platform::ALL
        .iter()
        .copied()
        .filter(|p| loopback_inbounds_require_auth(*p))
    {
        let tag = node_tag(platform);
        let out: Vec<serde_json::Value> = build_inbounds(&cfg, None, &all_loopback_deps(tag, None))
            .iter()
            .map(|ib| serde_json::to_value(ib).unwrap())
            .collect();
        let tags: Vec<&str> = out.iter().map(tag_of).collect();
        assert_eq!(
            tags,
            vec!["tun-in"],
            "[{tag}] 无凭据时回环入站必须整批缺席，只剩 tun-in"
        );
        checked += 1;
    }
    assert!(checked >= 1, "没有任何要求凭据的平台 —— 本门空跑");
}

/// Android 上**三个 `proxy_mode_type` 档位产出同一份入站**，tun 入站一个都不能少。
///
/// # 这条守的是一个阻断级缺陷，不是洁癖
///
/// Android 上「接管方式」这个选择根本不存在：应用能拿到的入网口只有 `VpnService` 给的那一个
/// tun fd。而 [`UserConfig`] 的 `proxy_mode_type` **缺省值是 `systemProxy`** ——
/// 全新安装的客户端拿到的就是它（老用户、从备份恢复、手工改过 config 的同理）。
///
/// 照存量档位分流的后果：`mixed-in` 在 Android 上本就不发（共享回环 + 零认证，见
/// `mixed_inbound_is_absent_on_android_and_present_elsewhere`），tun 入站又因为「不是 TUN 档」
/// 而不发 ⇒ **生成出来的配置里一个用户流量入站都没有**。隧道建得起来、核跑得起来、界面显示
/// 已连接，流量进去没有出口，而且全程不报错 —— 与「回环 `excludeRoute` 让 `establish()` 整个
/// 失败」同一档，只是那条会炸、这条静默空转。
///
/// # 为什么承重面在这里，而不是在 UI
///
/// UI 不再提供这个选项治不了**已经存下来的**配置。判据落在生成侧才对存量输入成立。
///
/// # 判据为什么是「三份逐字节相同」而不是「都非空」
///
/// 「都非空」会被「三档各生成一份不同的 tun」骗过 —— 那正是「半开状态」的样子：
/// 两条路都能跑，但生成的东西不一样，而没有任何一处代码为那个差异答过题。
#[test]
fn android_gets_the_same_inbounds_whatever_the_stored_proxy_mode_type() {
    let fixture = load();
    let base = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_no_mixed")
        .expect("夹具缺 android_tun_no_mixed");

    // 期望值不从夹具读：逐档跑一遍实现，互相对拍。
    let mut per_mode: Vec<(&str, Vec<serde_json::Value>)> = Vec::new();
    for mode in ["systemProxy", "manual", "tun"] {
        let mut case_input = base.input.config.clone();
        case_input["proxyModeType"] = serde_json::Value::String(mode.to_owned());
        let out = run_config(&case_input, "android", &base.input.ports);
        assert!(
            out.iter().any(|v| tag_of(v) == "tun-in"),
            "Android + {mode}: 没有 tun 入站 —— 这份配置一个用户流量入站都没有，\
             隧道建得起来但流量进去没出口，且不报错。实得 {out:?}"
        );
        per_mode.push((mode, out));
    }
    for (mode, out) in &per_mode[1..] {
        assert_eq!(
            out, &per_mode[0].1,
            "Android + {mode} 与 Android + {} 产出的入站不一致 —— 那个平台上只有一种接管形态，\
             两份不同的配置里必有一份没人为它答过题",
            per_mode[0].0
        );
    }
}

/// **反向对照**：桌面三平台 × `systemProxy`/`manual` 仍然按存量档位办事。
///
/// 上一条只证明了「Android 恒发 tun」。若判据写歪成「谁都恒发 tun」，上一条照样全绿，而桌面会
/// 凭空多出一张 TUN 网卡（用户选的是系统代理，却被接管了整机路由）。这条就是那个反向对照，
/// 同时它也是本批「桌面零回归」的正面证明之一：**六格全部**逐个断言，不是「我没改桌面」。
///
/// 冻结的 TS 导出金样（`fixtures/inbounds.json`）另有 `sys_linux`/`sys_mac`/`sys_win`/`manual`
/// 四条同向 case —— 那是**第二个实现**给出的同一答案；本条补齐它没覆盖的 darwin/win32 + manual。
#[test]
fn desktop_still_honours_the_stored_proxy_mode_type() {
    let fixture = load();
    let base = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_no_mixed")
        .expect("夹具缺 android_tun_no_mixed");

    let mut checked = 0usize;
    for platform in ["linux", "darwin", "win32"] {
        for mode in ["systemProxy", "manual"] {
            let mut cfg = base.input.config.clone();
            cfg["proxyModeType"] = serde_json::Value::String(mode.to_owned());
            let out = run_config(&cfg, platform, &base.input.ports);
            assert!(
                !out.iter().any(|v| tag_of(v) == "tun-in"),
                "{platform} + {mode}: 桌面上这两档是真实存在的接管形态，不许被接管成 TUN。实得 {out:?}"
            );
            assert!(
                out.iter().any(|v| tag_of(v) == "mixed-in"),
                "{platform} + {mode}: 桌面本地代理入站不该消失。实得 {out:?}"
            );
            checked += 1;
        }
        // 同平台的 TUN 档必须**有** tun 入站，否则上面两条会被「桌面什么都不发」骗过。
        let mut cfg = base.input.config.clone();
        cfg["proxyModeType"] = serde_json::Value::String("tun".to_owned());
        let out = run_config(&cfg, platform, &base.input.ports);
        assert!(
            out.iter().any(|v| tag_of(v) == "tun-in"),
            "{platform} + tun: 桌面 TUN 档必须仍发 tun 入站"
        );
    }
    assert_eq!(checked, 6, "桌面三平台 × 两档共六格，实跑 {checked} 格");
}

/// Android 的 tun：`route_exclude_address` 里**一条回环前缀都不许有**，而「绕过局域网」开着时
/// 局域网段**必须在**。
///
/// # 反面（回环）：这条不是「不需要」而是「发了就起不来」
///
/// `route_exclude_address` 在 Android 上落到 `VpnService.Builder.excludeRoute()`，该方法**拒收回环
/// 前缀**（`127.0.0.0/8` 抛 `Bad address`），而 Builder 的语义是任何一条被拒就整个 `establish()` 失败
/// ⇒ 症状是 `configure tun interface: Bad address`，**隧道一次都建不起来**。
/// 2026-09-04 模拟器实测，logcat 原文 `W/PolarisVpnService: 排除路由 127.0.0.0/8 被系统拒收`。
/// 默认 `bypassLANList` 里就有 `127.0.0.0/8` ⇒ 这枚毒输入今天**每份** Android 配置都会经过生成侧。
///
/// # 正面（局域网段）：2026-09-25 起 android 臂不再恒空
///
/// 夹具里的 android case 都没写 `bypassLAN`（缺省 = 开）⇒ 每条都该带上默认清单的 RFC1918 段。
/// 只有反面的话，「android 臂又退回恒空」会让本测全绿 —— 那正是本批要消灭的「开关按了不生效」。
/// 期望值手写字面量，不从夹具读。
///
/// 用户声明段那条腿（`inboundExcludeCidrs`）与开关开/关、自定义清单的逐格金样归
/// `golden_tun_inbound_exclude.rs` 的 android 系列。
///
/// # 为什么金样对拍不够，非要单列一条
///
/// 金样是**可再生成**的：谁按「实得」刷一遍 `inbounds-android.json`，回环排除重新长出来也照样绿，
/// 而症状（隧道建不起来）离夹具很远，没人会把两件事联系上。本条把**意图**钉在夹具之外，
/// 刷金样刷不掉它。同理于本文件里 `include_package_is_never_emitted` 那条。
#[test]
fn android_tun_route_exclude_carries_lan_but_never_loopback() {
    let fixture = load();
    let mut checked = 0usize;
    for case in &fixture.cases {
        if case.input.platform != "android" {
            continue;
        }
        // 🔴 取材面前提自检：本条量的是「绕过局域网」派生腿。夹具里若出现用户声明段或关掉开关的
        // case，正面断言的前提就变了 —— 当场炸，而不是让断言悄悄变成别的意思。
        assert!(
            case.input
                .config
                .get("tunConfig")
                .and_then(|t| t.get("inboundExcludeCidrs"))
                .is_none()
                && case.input.config.get("bypassLAN").is_none(),
            "[{}] 夹具里出现了 `inboundExcludeCidrs` 或 `bypassLAN` —— 本条的前提（缺省开关下的派生腿）没了。\
             开/关与用户段的逐格金样归 golden_tun_inbound_exclude.rs，别在这里放宽断言",
            case.name
        );
        let out = run_case(case);
        // 2026-09-05 起 Android 的每一条 case 都有 tun（`proxy_mode_type` 三档同答）。「没有 tun 就
        // 不计入 checked」保留：若哪天出现无 tun 的 android case，下面的取材面自检会因 checked 变小而红。
        let Some(tun) = out.iter().find(|v| tag_of(v) == "tun-in") else {
            continue;
        };
        let excl: Vec<&str> = tun
            .get("route_exclude_address")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] 「绕过局域网」缺省开着，Android 的 tun 却没有 route_exclude_address —— \
                     android 臂又退回了恒空。实得 {tun}",
                    case.name
                )
            })
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for lan in ["10.0.0.0/8", "192.168.0.0/16", "fe80::/10"] {
            assert!(
                excl.contains(&lan),
                "[{}] 缺省清单里的 {lan} 没进 Android 的 route_exclude_address。实得 {excl:?}",
                case.name
            );
        }
        let loopback: Vec<&&str> = excl
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
            "[{}] Android 的 route_exclude_address 带了回环前缀 {loopback:?} —— \
             Builder.excludeRoute 拒收回环，整条隧道会建不起来。实得 {excl:?}",
            case.name
        );
        checked += 1;
    }
    // 取材面自检：一条 android case 都没跑到的话，上面的断言恒真、本门零信息量。
    assert!(
        checked >= 6,
        "只检查了 {checked} 条 android case —— 取材面塌了，本门退化成恒绿"
    );
}

/// Android 的 tun **一个 `strict_route` 键都不许发**，而桌面照发。
///
/// # 它在 Android 上是彻底的空转（两层证据，都成立）
///
/// ① 源码链（sing-box v1.14.0 + sing-tun v0.9.0-beta.4）：libbox 的 `UsePlatformInterface()` 恒 true
/// ⇒ tun 走 `platformInterface.OpenInterface`、`tun.New()` 永不执行 ⇒ 没有 `NativeTun`；而该字段在
/// Linux/Android 侧的**唯一**落点在 `NativeTun::rules()`，只由 `NativeTun::Start()` 调用 —— tun fd
/// 这条路根本不走 `Start()`。
/// ② 真机（模拟器）正反双向对照，2026-09-04：起隧道前后 `iptables -S` / `ip6tables -S` **逐字节相同**
/// ⇒ 内核侧一条防绕行规则都没装；正对照是同一时刻 `ip rule` **+14 条**（Android netd 为
/// `VpnService` 装的 uidrange 规则）⇒ 观测手段确实看得见「规则被装上」，前一条的相等不是量错了。
///
/// # 为什么金样对拍不够，非要单列一条（同 [`android_tun_route_exclude_carries_lan_but_never_loopback`]）
///
/// 金样是**可再生成**的：谁按「实得」刷一遍 `inbounds-android.json`，`strict_route: true` 重新长
/// 回来也照样绿。而这条的症状**根本不在配置里** —— 它是一句写在配置里的假话：任何读配置排查
/// 问题的人（包括未来的我们）都会以为防绕行是开着的，而 UI 侧已经按同一条判据把开关撤掉了
/// （`ui/src/mobile/settings/TunPage.tsx`）。刷金样刷不掉本条。
///
/// # 反向对照写在同一个测里
///
/// 只写「Android 不发」会被「哪个平台都不发」骗过 —— 那是把桌面的防绕行泄漏保护一起删掉，
/// 却同样满足前半条。后半条钉住桌面仍逐字下发。
#[test]
fn android_tun_never_carries_strict_route_and_desktop_still_does() {
    let fixture = load();
    let (mut android_checked, mut desktop_checked) = (0usize, 0usize);
    for case in &fixture.cases {
        let out = run_case(case);
        let Some(tun) = out.iter().find(|v| tag_of(v) == "tun-in") else {
            continue;
        };
        if case.input.platform == "android" {
            assert!(
                tun.get("strict_route").is_none(),
                "[{}] Android 的 tun 发了 strict_route —— 那个字段在 libbox + VpnService tun fd 这条\
                 路上一行代码都不会跑（iptables 前后逐字节相同已实证），发它等于在配置里写一句假话。\
                 实得 {tun}",
                case.name
            );
            android_checked += 1;
        } else {
            assert_eq!(
                tun.get("strict_route"),
                Some(&serde_json::json!(true)),
                "[{}] 桌面（{}）的 strict_route 不见了 —— 那是防绕行泄漏的开关，平台判据写歪会把\
                 桌面一起打掉",
                case.name,
                case.input.platform
            );
            desktop_checked += 1;
        }
    }
    // 取材面自检：两侧任一塌掉，对应的断言就恒真、本门退化成恒绿。
    // 夹具里 6 条 android case **全部**该有 tun（三档同答）；少一条就说明 Android 丢了 tun 入站，
    // 那正是本批修掉的那个缺陷本身 —— 所以下限取实数 6，不留「少一条也算过」的余量。
    assert!(
        android_checked >= 6 && desktop_checked == 3,
        "取材面塌了（android={android_checked}, desktop={desktop_checked}）"
    );
}

/* ══════════════════ 移动端策略选择器 ←→ exclude_package 的**跨语言缝** ══════════════════ */

/// UI 端写出来的一档策略。夹具见 `fixtures/mobile-app-policy-writes.json`（两端读同一份）。
#[derive(Debug, Deserialize)]
struct MobilePick {
    pick: String,
    writable: bool,
    rule: serde_json::Value,
    #[serde(rename = "expectExcluded")]
    expect_excluded: bool,
}

#[derive(Debug, Deserialize)]
struct MobilePickFixture {
    #[serde(rename = "appId")]
    app_id: String,
    picks: Vec<MobilePick>,
    #[serde(rename = "expectedPackage")]
    expected_package: String,
    #[serde(rename = "baseConfig")]
    base_config: serde_json::Value,
}

fn load_mobile_picks() -> MobilePickFixture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mobile-app-policy-writes.json"
    );
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读 fixtures/mobile-app-policy-writes.json 失败: {e}"));
    let fixture: MobilePickFixture = serde_json::from_str(&raw).expect("移动端策略夹具解析失败");
    assert_eq!(
        fixture.picks.len(),
        4,
        "四档必须各有一条，少一条就有一档没判过"
    );
    fixture
}

/// 拿一条 `AppRule` 跑一次 Android 生成，取回 `tun-in.exclude_package`（缺席 = `None`）。
fn exclude_packages_for(base: &serde_json::Value, rule: &serde_json::Value) -> Option<Vec<String>> {
    let mut config = base.clone();
    config["appRules"] = serde_json::json!([rule]);
    let ports = Ports {
        probe_direct: None,
        probe_proxy: None,
        update_in: None,
        probe_pool: vec![],
    };
    let out = run_config(&config, "android", &ports);
    let tun = out.iter().find(|v| tag_of(v) == "tun-in")?;
    tun.get("exclude_package").map(|v| {
        v.as_array()
            .expect("exclude_package 不是数组")
            .iter()
            .map(|s| s.as_str().expect("包名不是字符串").to_owned())
            .collect()
    })
}

/// **本批的行为门**：移动端选了「直连」之后，配置生成侧真的把那个包名放进了 `exclude_package`；
/// 另外三档一个都不进。
///
/// # 为什么不能靠 [`direct_app_rules_land_in_exclude_package_and_nothing_else_does`]
///
/// 那条测的输入是**本仓自撰**的 `inbounds-android.json`：它证明「engine 认这种形态」，
/// 证明不了「UI 写出来的就是这种形态」。UI 那边把 `enabled` 写成 `false`、把「跟随全局」写成
/// 「删掉规则」、或者把 `action` 写成别的字符串，那条测一个字都不会变 —— 缝正好在两者之间。
/// 本条的输入**逐字来自** UI 端判据读的同一份夹具，缝因此被两条判据夹住：
/// `app-policy-write.test.ts` 钉「`appRuleForPick` 产出这些对象、且生产真的调用它」，
/// 本条钉「这些对象喂进 `build_inbounds` 之后包名真的进/不进」。
#[test]
fn mobile_policy_picks_reach_exclude_package_exactly_for_direct() {
    let fixture = load_mobile_picks();
    let mut direct_seen = 0usize;
    for pick in &fixture.picks {
        let got = exclude_packages_for(&fixture.base_config, &pick.rule);
        if pick.expect_excluded {
            direct_seen += 1;
            assert_eq!(
                got.as_deref(),
                Some([fixture.expected_package.clone()].as_slice()),
                "[{}] 这一档该把包名排出隧道，实得 {got:?}",
                pick.pick
            );
        } else {
            assert!(
                got.is_none(),
                "[{}] 这一档不该产生 exclude_package —— 排除一个「走代理 / 被阻断」的应用，\
                 方向与它的语义相反（前者变成直连出网，后者变成拿到完整互联网）。实得 {got:?}",
                pick.pick
            );
        }
        assert_eq!(
            pick.rule.get("appId").and_then(serde_json::Value::as_str),
            Some(fixture.app_id.as_str()),
            "[{}] 夹具里这一档换了 appId —— 期望包名那条字面量就对不上了",
            pick.pick
        );
    }
    // 正向对照：必须真的有一档进去过。全都判「不进」时，上面那串 `is_none()` 会被
    // 「这个预设根本没有包名 / 平台判据整条失效」骗过 —— 那两种情况下四档也全是 None。
    assert_eq!(direct_seen, 1, "夹具里没有任何一档期望进 exclude_package");
    // 且「可写」这一维两端不许各自漂：数目**从 TS 侧那份单一枚举里数出来**，不写死字面量。
    //
    // 🔴 写死过一次，栽过一次（2026-09-13）：那时白名单是两档，这里就钉了个 `2`；
    // 后来白名单放宽成四档全集（应用策略四档全部接通），夹具跟着改成四档可写，
    // 而这行字面量留在原地 ⇒ 本门红着，报的却是「可写档数变了」——听起来像引擎出了事，
    // 实际只是这行在数一个它自己没跟上的常量。**覆盖轴要从单一枚举派生**，
    // 另抄一份名单等于把「两端不许漂」这句话自己变成第三个会漂的地方。
    let writable_in_ts = app_policy_writable_count();
    assert_eq!(
        fixture.picks.iter().filter(|p| p.writable).count(),
        writable_in_ts,
        "夹具里的可写档数与 `APP_POLICY_WRITABLE` 对不上 —— 两端会各自认为自己是对的"
    );
}

/// 从 TS 侧那份**单一枚举**里数出「可写」有几档。
///
/// 取材面是 `ui/src/mobile/screens/rules/app-policy-write.ts` 里 `APP_POLICY_WRITABLE`
/// 那个 `new Set<AppPolicyWritablePick>([...])` 的字面量体。读不到、或数出 0 档时**直接 panic**：
/// 那意味着这条跨语言对拍失去了它的另一端，而不是「今天恰好零档可写」。
fn app_policy_writable_count() -> usize {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/src/mobile/screens/rules/app-policy-write.ts");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e} —— 本门的另一端没了", path.display()));
    let head = src
        .find("APP_POLICY_WRITABLE")
        .unwrap_or_else(|| panic!("{} 里找不到 APP_POLICY_WRITABLE", path.display()));
    let open = src[head..]
        .find('[')
        .unwrap_or_else(|| panic!("APP_POLICY_WRITABLE 后面没有数组字面量"))
        + head;
    let close = src[open..]
        .find(']')
        .unwrap_or_else(|| panic!("APP_POLICY_WRITABLE 的数组字面量没闭合"))
        + open;
    let n = src[open + 1..close]
        .split(',')
        .filter(|s| s.trim().starts_with('\''))
        .count();
    assert!(n > 0, "从 {} 里数出 0 档可写 —— 取材面塌了", path.display());
    n
}

/// **`appRoutingEnabled` 键缺席 = 开**（2026-09-24 裁定落地，原测 `app_routing_gate_requires_explicit_true`
/// 钉的是反面，本批按其头注「谁把任一侧的口径改齐，这条会红，届时连同上面那段一起处理」改写）。
///
/// # 裁定
///
/// 引擎五处读点（`inbounds.rs` 的 `effective_app_rules_proxy` / `route.rs` / `outbounds.rs` /
/// `endpoint_routes.rs` 两处）由 `== Some(true)` 改为 `!= Some(false)`，与两端界面
/// （`AppPolicyScreen.tsx` / `mobile/screens/rules/RulesScreen.tsx` 都是 `!== false`）及
/// `store::sanitize`（非 boolean 删键、不回填，头注写明「读取侧 `!== false` 视为开」）同侧。
///
/// # 触发面是「键缺席」，**不是**「从没动过总开关」
///
/// `store::default_config()` 显式写 `"appRoutingEnabled": false` ⇒ 全新安装键在且为 false，本裁定对它零影响。
/// 键缺席来自三个入口：历史配置、导入的旧备份、`sanitize` 删掉非 boolean 值。
///
/// 三态分开量：显式 true（正对照）/ 缺席（本裁定）/ 显式 false（反面，把「缺席」与「显式关」分开，
/// 免得缺席那条被「这条规则本来就进不去」骗过）。全平台 × 五站的回归面在 `tests/app_routing_gate_tristate.rs`。
#[test]
fn app_routing_gate_absent_key_means_on() {
    let fixture = load_mobile_picks();
    let direct = fixture
        .picks
        .iter()
        .find(|p| p.expect_excluded)
        .expect("夹具缺 direct 那一档");
    let want_pkgs = [fixture.expected_package.clone()];
    let want = Some(want_pkgs.as_slice());

    // 正对照：显式 true 时包名确实进去了（下面两条才有信息量）。
    let with_gate = exclude_packages_for(&fixture.base_config, &direct.rule);
    assert_eq!(
        with_gate.as_deref(),
        want,
        "显式 appRoutingEnabled:true 下都没进 —— 反向对照失去意义"
    );

    let mut without_gate = fixture.base_config.clone();
    assert!(
        without_gate
            .as_object_mut()
            .expect("baseConfig 不是对象")
            .remove("appRoutingEnabled")
            .is_some(),
        "baseConfig 里本来就没有 appRoutingEnabled —— 上面那条正对照量的就不是显式 true"
    );
    assert_eq!(
        exclude_packages_for(&without_gate, &direct.rule).as_deref(),
        want,
        "`appRoutingEnabled` 键缺席时应用分流没生效 —— 引擎口径退回了 `== Some(true)`，\
         而两端界面显示它开着"
    );

    let mut disabled_gate = fixture.base_config.clone();
    disabled_gate["appRoutingEnabled"] = serde_json::json!(false);
    assert!(
        exclude_packages_for(&disabled_gate, &direct.rule).is_none(),
        "显式 false 时包名仍然进了 exclude_package —— 总开关关不掉"
    );
}

/// 金样里「键缺席」那一条（`android_tun_direct_apps_gate_key_absent`）与显式 true 那一条
/// （`android_tun_direct_apps`）输入只差这一个键，期望产出必须**逐字节相同**。
///
/// 这条把金样的期望值钉在「另一条既有 case 的期望」上，而不是钉在实现的输出上 ——
/// 谁按实得刷一遍夹具把缺席那条刷成「关」的形态，这里会红。
#[test]
fn golden_absent_gate_case_matches_the_explicit_true_case() {
    let fixture = load();
    let on = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_direct_apps")
        .expect("夹具缺 android_tun_direct_apps");
    let absent = fixture
        .cases
        .iter()
        .find(|c| c.name == "android_tun_direct_apps_gate_key_absent")
        .expect("夹具缺 android_tun_direct_apps_gate_key_absent");

    assert_eq!(
        on.input.config["appRoutingEnabled"],
        serde_json::json!(true)
    );
    assert!(
        absent.input.config.get("appRoutingEnabled").is_none(),
        "缺席那条的输入里有 appRoutingEnabled 键 —— 量的不是存量形态"
    );
    let mut on_input = on.input.config.clone();
    on_input
        .as_object_mut()
        .unwrap()
        .remove("appRoutingEnabled");
    assert_eq!(
        absent.input.config, on_input,
        "两条输入除该键外还有别的差异"
    );
    assert_eq!(absent.input.platform, on.input.platform);

    // 正面：期望里 exclude_package 非空（否则两条相等可以是「都关」）。
    let tun = absent
        .output
        .iter()
        .find(|v| tag_of(v) == "tun-in")
        .expect("缺席那条的期望里没有 tun-in");
    assert!(
        tun["exclude_package"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "缺席那条的期望 exclude_package 为空"
    );
    assert_eq!(absent.output, on.output, "键缺席与显式 true 的期望产出不同");
}

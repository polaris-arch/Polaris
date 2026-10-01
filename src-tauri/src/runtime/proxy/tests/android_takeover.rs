//! **Android 上「接管方式」不是用户偏好、是平台事实** —— runtime 侧那一半的行为判据。
//!
//! # 判据要证的那件事
//!
//! `UserConfig::proxy_mode_type` 的存盘缺省值是 `systemProxy`（`app_config::default_proxy_mode_type`），
//! 而 Android 上只有 `VpnService` 的 tun fd 一种接管形态（成因全在
//! [`ProxyModeType::effective_on`] 的文档注释里）。于是**每一处**照裸值分流的 runtime 判据都是
//! 一个「Android 走错分支」，而全新安装 / 备份恢复 / 手改过 config 的客户端拿到的正是那个缺省值。
//!
//! 本模块钉住「三个存盘档位在 Android 上得到同一个答案，且那个答案是能用的那个」。
//!
//! # 与源码级门（`src-tauri/tests/android_takeover_is_a_platform_fact.rs`）的分工
//!
//! | 门 | 覆盖面 | 强制力 |
//! |---|---|---|
//! | 那道源码门 | 取材面里**每一处** `.is_tun()` / `ProxyModeType` 判别 | 新加一处读裸值 ⇒ 自动红（不枚举站点） |
//! | **本模块** | 上面那些站点里**行为可观测**的那几处 | 正面断言：三档各自产出什么 |
//!
//! 缺了源码门，本模块就退化成一张会腐烂的站点表（日后新加的站点它一格都盖不到）；缺了本模块，
//! 源码门只能证明「写法对了」，证明不了「产出的那个值是能用的」。两条成对交。
//!
//! # 为什么能在本机（Linux）跑出 Android 的答案
//!
//! `ProxyRuntime` 的接管方式判据全部经 `self.helper.platform()` 分流，而
//! `HelperRuntime::with_platform_for_tests` 把那个字段钉成给定平台（其余隔离一概不动，
//! `sys_ops` 仍是 `NeverInstalled`、`never_connect` 仍为 true）。没有这个注入口，Android 分叉
//! 在本机就一行运行期证据都没有。
//!
//! # 本机安全
//!
//! 全模块不起核、不碰宿主网络：`SystemProxyClearer` 一律注入替身（绝不调真 `networksetup`/
//! `gsettings`/`reg`），连接 flush 打的是一个**刚释放的空闲回环口**（无监听 ⇒ 必然 ConnectFailed），
//! TUN 出口探测在 Android 上根本不下发命令（闸不适用）。

use super::*;
use crate::runtime::proxy::connection_flush::FlushOutcome;

/// 三个**存盘**档位。Android 上它们必须彼此不可分辨。
const STORED_MODES: [ProxyModeType; 3] = [
    ProxyModeType::SystemProxy,
    ProxyModeType::Tun,
    ProxyModeType::Manual,
];

/// 最小 `UserConfig`（零节点 + 直连哨兵），只有 `proxy_mode_type` 一个自变量。
fn user_config_with_mode(mode: ProxyModeType) -> UserConfig {
    let mut config: UserConfig = serde_json::from_value(serde_json::json!({
        "servers": [],
        "selectedServerId": DIRECT_SERVER_ID,
        "proxyMode": "smart",
    }))
    .expect("最小 UserConfig 应可解析");
    config.proxy_mode_type = mode;
    config
}

// ══════════════════════════════════════════════════════════════════════════════
// ① 系统代理：Android 上三档都不去设（今天真机上就能看见的那条假错误）
// ══════════════════════════════════════════════════════════════════════════════

/// **Android：三个存盘档位都不调 `enable_system_proxy`。**
///
/// 缺陷原形：`proxy_ops` 的 Android 臂全是 `Err(UnsupportedPlatform)`（那个平台没有全局 HTTP
/// 代理设置面），于是照读裸值的客户端每次起核成功后都会拿到那条 `Err`，落
/// `set_nonfatal_error(SYSTEM_PROXY_FAILED)` ⇒ **用户当场看到「系统代理启用失败，流量未经代理
/// （当前为直连）」**，而那句话在 Android 上从头到尾是假的：流量正经由 `VpnService` 的 tun fd 走着。
///
/// 变异锁：把 `should_enable_system_proxy` 里的 `mode.effective_on(platform)` 改回 `mode`
/// → `SystemProxy` 那一档调到 enable → 本测转红并点名该档。
#[tokio::test]
async fn android_never_enables_system_proxy_whatever_is_stored() {
    for mode in STORED_MODES {
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let clearer: Box<dyn SystemProxyClearer> = Box::new(EnableRecordingClearer {
            enable_reqs: Arc::clone(&reqs),
            ..Default::default()
        });
        let (rt, _dir) = test_runtime_with_clearer_on(Platform::Android, clearer);
        rt.maybe_enable_system_proxy(&user_config_with_mode(mode), 7890)
            .await;
        assert!(
            reqs.lock().unwrap().is_empty(),
            "Android + 存盘 {mode:?}：不得去设 OS 系统代理 —— 那个平台没有可设的对象，\
             唯一后果是给用户弹一条与现实相反的错误"
        );
    }
}

/// **反向对照**：同一条路径在桌面上，`systemProxy` 档必须**真的**调到 enable。
///
/// 没有这条，上面那句「三档全空」可能只是因为整条装配坏了（clearer 没接上、方法早退在别处），
/// 那种绿没有信息量。
#[tokio::test]
async fn desktop_still_enables_system_proxy_for_the_systemproxy_mode() {
    let reqs = Arc::new(Mutex::new(Vec::new()));
    let clearer: Box<dyn SystemProxyClearer> = Box::new(EnableRecordingClearer {
        enable_reqs: Arc::clone(&reqs),
        ..Default::default()
    });
    let (rt, _dir) = test_runtime_with_clearer_on(Platform::Linux, clearer);
    rt.maybe_enable_system_proxy(&user_config_with_mode(ProxyModeType::SystemProxy), 7890)
        .await;
    let got = reqs.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "桌面 systemProxy 档必须仍然调 enable 一次");
    assert_eq!(got[0].http_port, 7890);
}

// ══════════════════════════════════════════════════════════════════════════════
// ② 连接 flush：Android 上三档都必须开枪
// ══════════════════════════════════════════════════════════════════════════════

/// **Android：三个存盘档位都走到管理 API（= 那一枪真的开了）。**
///
/// 缺陷原形：app 在隧道建立**之前**发起的连接已经泄漏成真实 IP，起核后它们的后续包仍走物理
/// 网卡直出。照读裸值 ⇒ 全新安装的 Android 客户端这一枪**永远不开**，那几条连接从此不会自愈，
/// 而界面显示「已连接」。
///
/// 断言写成「不是 `SkippedNotTun`、且真的落到建连结果上」而不是「返回值不等于某个东西」：
/// 端口取一个刚释放的空闲回环口（无监听）⇒ 必然 `ConnectFailed`/`CallFailed`，这正是
/// 「两条守卫都放行、代码真的去开枪了」的正面证据。
///
/// 变异锁：把 `flush_connections_once` 的 `mode.effective_on(self.helper.platform())` 改回
/// `mode` → `SystemProxy`/`Manual` 两档返 `SkippedNotTun` → 本测转红并点名该档。
#[tokio::test]
async fn android_flushes_connections_whatever_is_stored() {
    for mode in STORED_MODES {
        let (rt, _dir) = test_runtime_on(Platform::Android);
        *rt.status.write().unwrap() = ProxyStatus {
            running: true,
            ..Default::default()
        };
        let my_gen = rt.gate.generation();
        let dead_port = free_port(); // 监听已 drop ⇒ 无人接
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            rt.flush_connections_once(mode, my_gen, dead_port),
        )
        .await
        .expect("flush 腿必须自行了结，不得挂死在建连上");
        assert!(
            matches!(
                outcome,
                FlushOutcome::ConnectFailed(_) | FlushOutcome::CallFailed(_)
            ),
            "Android + 存盘 {mode:?}：必须真的走到管理 API 去 RST 旧连接（那里恒是 TUN 接管），\
             实得 {outcome:?}"
        );
    }
}

/// **反向对照**：同一条路径在桌面上，非 TUN 两档必须仍然被守卫①拦下。
///
/// 否则上面那条「三档都开枪」可能只是因为守卫①整个失效了 —— 那会让桌面 systemProxy/manual
/// 用户被无差别 RST 误伤（够不着表外的旧连接，只误伤已代理的连接，净负收益）。
#[tokio::test]
async fn desktop_still_skips_flush_for_non_tun_modes() {
    let (rt, _dir) = test_runtime_on(Platform::Linux);
    *rt.status.write().unwrap() = ProxyStatus {
        running: true,
        ..Default::default()
    };
    let my_gen = rt.gate.generation();
    for mode in [ProxyModeType::SystemProxy, ProxyModeType::Manual] {
        assert_eq!(
            rt.flush_connections_once(mode, my_gen, 1).await,
            FlushOutcome::SkippedNotTun,
            "桌面 {mode:?} 档仍必须被守卫①拦下"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// ③ 运行核接管方式的对外投影
// ══════════════════════════════════════════════════════════════════════════════

/// **Android：三个存盘档位读出的运行态接管方式都是 `Tun`。**
///
/// 唯一生产消费点是 `commands::proxy::system_proxy_get_status` 的第三道门。读裸值 ⇒ Android 上
/// 判「运行核是系统代理接管模式」成立 ⇒ 继续去读 OS 系统代理设置 ⇒ 拿 `Err` ⇒ 前端折成「未知」。
///
/// 变异锁：删掉 `running_effective_proxy_mode_type` 里的 `.effective_on(self.helper.platform())`
/// → `SystemProxy`/`Manual` 两档原样返回 → 本测转红。
#[test]
fn android_running_effective_mode_is_tun_whatever_is_stored() {
    let (rt, _dir) = test_runtime_on(Platform::Android);
    mark_running(&rt);
    for mode in STORED_MODES {
        *rt.startup_snapshot.write().unwrap() = Some(serde_json::json!({
            "servers": [],
            "selectedServerId": "__direct__",
            "proxyMode": "smart",
            "proxyModeType": match mode {
                ProxyModeType::SystemProxy => "systemProxy",
                ProxyModeType::Tun => "tun",
                ProxyModeType::Manual => "manual",
            },
        }));
        assert_eq!(
            rt.running_effective_proxy_mode_type(),
            Some(ProxyModeType::Tun),
            "Android + 存盘 {mode:?}：运行核实际在做的接管只有 VpnService 的 tun fd 一种"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// ④ TUN 出口夺取硬闸：Android 上三档都不挂闸、不付 grace 窗口
// ══════════════════════════════════════════════════════════════════════════════

/// **Android：三个存盘档位都不挂 TUN 出口夺取硬闸，且不下发任何路由查询。**
///
/// 那个平台读不出逐目的出口（`SystemRouteOps::exit_interface_for` 的 Android 臂恒 `Ok(None)`
/// 且不 spawn 命令），故 baseline 差分恒 `Indeterminate`（放行）。挂着闸唯一的效果是每次起核在
/// 主链上白付 `TUN_ROUTE_GRACE_POLLS × TUN_ROUTE_POLL_INTERVAL ≈ 3.5s`。
///
/// 时限断言取 2s：不闸那条路是同步早退（微秒级），而挂闸那条路的下界是 3.5s ——
/// 两者相差一个数量级，本断言不会被机器负载翻转。
///
/// 变异锁：删掉 `tun_route_gate_applies` 里的 `Platform::Android => false` 臂 → 三档全部改走
/// grace 轮询 → 本测因超时上限转红。
#[tokio::test]
async fn android_pays_no_grace_window_for_the_tun_route_gate() {
    let (rt, _dir) = test_runtime_on(Platform::Android);
    for mode in STORED_MODES {
        let started = std::time::Instant::now();
        assert_eq!(
            rt.capture_tun_route_baseline(mode).await,
            None,
            "Android + 存盘 {mode:?}：闸不适用 ⇒ 不该采 baseline"
        );
        // 喂一个**可读**的 baseline：若闸真的挂上了，`verify` 会一路轮询到 grace 耗尽。
        let verified = rt
            .verify_tun_route_captured(mode, ExitInterfaceId::from_alias("wlan0"))
            .await;
        assert_eq!(
            verified,
            Ok(None),
            "Android + 存盘 {mode:?}：闸不适用 ⇒ 必须直接放行且不返回接口身份"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "Android + 存盘 {mode:?}：起核主链上白付了 {elapsed:?} —— 说明 grace 轮询真的跑了"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// ⑤ 纯谓词决策向量：三档逐字段相同（正面断言 + 桌面反向对照）
// ══════════════════════════════════════════════════════════════════════════════

/// 起核路径上按接管方式分流的**纯谓词**决策，一次全取。
///
/// 做成一个可比较的结构体而不是逐条 assert：判据于是变成「三档产出的这一份**逐字段相同**」，
/// 与 config-engine 那道门「整份配置逐字节相同」同形 —— 差别只在 runtime 侧没有「整份产物」
/// 这种可比对象，只能取它的决策面。
#[derive(Debug, PartialEq, Eq)]
struct TakeoverDecisions {
    /// 是否经提权 helper 起核。
    via_helper: bool,
    /// 是否挂 TUN 出口夺取硬闸。
    tun_route_gate: bool,
    /// 是否做 wintun 适配器存在性探测。
    probe_wintun: bool,
    /// 起核重试预算（双 TUN 竞态放宽腿由接管方式参与判定）。
    retry_budget: StartRetryBudget,
    /// 本会话由 Polaris 管理的 TUN 接口身份。
    managed_tun: Option<ExitInterfaceId>,
    /// 逐目的绑定规划要不要覆盖当前活跃物理根。
    binding_roots: std::collections::BTreeSet<String>,
}

fn takeover_decisions(mode: ProxyModeType, platform: Platform, tag: &str) -> TakeoverDecisions {
    let config = user_config_with_mode(mode);
    TakeoverDecisions {
        via_helper: should_start_via_helper(mode, platform),
        tun_route_gate: tun_route_gate_applies(mode, platform),
        probe_wintun: should_probe_wintun_adapter(mode, tag),
        retry_budget: resolve_start_retry_budget(
            mode.effective_on(platform).is_tun(),
            &config.servers,
            tag,
        ),
        managed_tun: managed_tun_interface_for_session(&config, platform, None),
        binding_roots: crate::runtime::route_binding::automatic_runtime_binding_root_ids(&config),
    }
}

/// **Android：三个存盘档位的决策向量逐字段相同。**
///
/// 正面断言另立（只比「三份相同」会被「三份同样地空/同样地错」骗过）：那份决策必须是**能用**
/// 的那份 —— 不经 helper（那个平台没有提权 daemon，`platform_supported(Android) == false`）、
/// 不挂出口夺取闸（读不出逐目的出口）、不探 wintun（Windows 专属）。
#[test]
fn android_takeover_decisions_are_identical_for_every_stored_mode() {
    let decisions: Vec<(ProxyModeType, TakeoverDecisions)> = STORED_MODES
        .iter()
        .map(|mode| {
            (
                *mode,
                takeover_decisions(*mode, Platform::Android, "android"),
            )
        })
        .collect();

    // ── 正面断言：那份决策确实是能用的那份 ──
    for (mode, got) in &decisions {
        assert!(
            !got.via_helper,
            "Android + 存盘 {mode:?}：不得经提权 helper 起核 —— 那里走的是进程内 libbox"
        );
        assert!(
            !got.tun_route_gate,
            "Android + 存盘 {mode:?}：不得挂出口夺取硬闸 —— 该平台读不出逐目的出口，\
             闸没有取材面，挂上只白付 grace 窗口"
        );
        assert!(
            !got.probe_wintun,
            "Android + 存盘 {mode:?}：不得探 wintun 适配器 —— 那是 Windows 专属"
        );
    }

    // ── 三档逐字段相同 ──
    for (mode, got) in &decisions[1..] {
        assert_eq!(
            got, &decisions[0].1,
            "Android：存盘 {mode:?} 与 {:?} 算出了不同的起核决策 —— 那个平台上只有一种接管形态，\
             两份不同的决策里必有一份没人为它答过题",
            decisions[0].0
        );
    }
}

/// **反向对照**：同一组输入在桌面上，`systemProxy` 与 `tun` 必须算出**不同**的决策。
///
/// 上一条全绿也可能是因为 `takeover_decisions` 整个坏了（每格恒 false / 恒同一个值）。
/// 这条喂一对**已知应当不同**的输入，要求它真的报出不同 —— 同时它也是桌面零回归的一半。
#[test]
fn desktop_takeover_decisions_still_differ_between_stored_modes() {
    for (platform, tag) in [
        (Platform::Mac, "darwin"),
        (Platform::Win, "win32"),
        (Platform::Linux, "linux"),
    ] {
        let system_proxy = takeover_decisions(ProxyModeType::SystemProxy, platform, tag);
        let tun = takeover_decisions(ProxyModeType::Tun, platform, tag);
        assert_ne!(
            system_proxy, tun,
            "{platform:?}：systemProxy 与 tun 算出了同一份决策 —— 要么桌面被接管成恒 TUN，\
             要么本文件的决策向量根本报不出差异（那样上一条的『相同』也就没有信息量）"
        );
        assert!(
            !system_proxy.via_helper && tun.via_helper,
            "{platform:?}：只有 TUN 档该经提权 helper 起核"
        );
        assert!(
            !system_proxy.tun_route_gate && tun.tun_route_gate,
            "{platform:?}：只有 TUN 档该挂出口夺取硬闸"
        );
    }
}

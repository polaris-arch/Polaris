//! Android 起停核桥（Rust → Kotlin/libbox）的 Rust 半边。
//!
//! 设计与判据：`~/docs/polaris/design/polaris-android-rust-kotlin-bridge-design-2026-09-04.md`。
//! Kotlin 半边：`src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt`。
//! 两侧的命令面由 `scripts/check-android-bridge.mjs` 双向对拍（改一侧不改另一侧 ⇒ 门红）。
//!
//! # 为什么走 Tauri plugin 而不是裸 JNI
//!
//! 硬要求是「Android 起核失败 ⇒ Rust 侧一个**有码**的 `StartError`」，而
//! `invoke.reject(msg, code)` → `ErrorResponse.code` 是这条链上唯一不用自己造轮子的实现
//! （裸 JNI 要自己定义「返回串怎么编码 code」，那是第二份约定且没人守）。零新增依赖也是承重的：
//! `jni` 在 lock 里同时存在 0.21.1 与 0.22.4 两个传递版本，直接依赖必须挑一个，挑错是链接期才炸。
//!
//! # 🔴 超时是**判据的一部分**，不是保险丝
//!
//! `run_command`（`tauri-2.11.5/src/plugin/mobile.rs:440`）把调用投递到 `run_on_android_context`，
//! 后者在 `tauri-runtime-wry-2.11.4/src/lib.rs:2863` 直接转 wry 的 `dispatch(f)` —— 即压进主线程
//! （activity looper）的队列。**若 activity 不在，闭包只是排队、永不执行，`await` 永久挂起**，
//! 而 `PluginInvokeError` 自身**没有** timeout 变体。不套 `tokio::time::timeout` 就是挂死，
//! 那正是本批明令禁止的「静默没起来」。
//!
//! 起停核的**桥调用**今天仍只从 WebView 里的 IPC 发起（activity 必在）。但隧道本身已不止这一个
//! 入口（2026-09-25）：always-on VPN 与开机自动连接（Kotlin `BootReceiver`）会在**没有 Rust** 的
//! 时候由系统直接拉起 `PolarisVpnService`，配置读 Rust 上次落盘的同一份文件（Kotlin `SystemStart`）。
//! 那种核 Rust 不认识，由 [`system_started_core_running`] 对账后走标准起核收编。
//! 管理 API 认证缝（§8④）：系统拉起的核用的是上次落盘配置里的端口与密钥，与 Rust 上次起核时
//! 暴露面相同（回环 + 同一密钥），不新增面；Rust 收编时会按新配置重起。

#[cfg(target_os = "android")]
use std::time::Duration;

use polaris_core_supervisor::config_gate::{verdict_from_libbox_check, ConfigCheckVerdict};

use super::android_capacity::CapacityClosed;
use super::code;

/// The old Android bridge starts and stops through detached tasks. A dropped
/// waiter or timeout leaves a request that may still mutate the service; no
/// later global `stop: ()` ACK can clear that uncertainty for this registry.
#[derive(Clone)]
pub(super) struct AndroidRequestBirth {
    pub(super) identity: std::sync::Arc<()>,
    pub(super) main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
}

impl AndroidRequestBirth {
    pub(super) fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.identity, &other.identity)
    }
}

/// 桥的 Kotlin 侧插件标识（`register_android_plugin` 会拼成 `com/polaris2/app/vpn/PolarisVpnPlugin`）。
#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.polaris2.app.vpn";

/// 桥的 Kotlin 侧类名。改它必须同时改 Kotlin 侧的文件名与类名 —— 反射失配是运行期才炸。
#[cfg(target_os = "android")]
const PLUGIN_CLASS: &str = "PolarisVpnPlugin";

/// Tauri 插件名（`run_mobile_plugin_async` 按它找 Kotlin 侧 `PluginManager` 里的 handle）。
#[cfg(target_os = "android")]
const PLUGIN_NAME: &str = "polaris-vpn";

/// 起核桥超时。取 30s 的依据：Kotlin 侧 `startOrReloadService` 同步走完内核启动，生产规模配置
/// （二十余个 `.srs` + naive 出站逐个建 Cronet Engine）在低端机上可达十几秒；再短会把慢起误判成挂死。
///
/// 三个常量一律 `cfg(android)` 而不是 `#[allow(dead_code)]`：allow 会把**真正**变成死代码的那天
/// 一并盖住（本仓既定口径，见 `app_language.rs:68`）。相对大小由下方编译期断言守。
#[cfg(target_os = "android")]
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// 停核桥超时。停机只做「关 fd → 关内核 → stopSelf」，比起核短一档。
#[cfg(target_os = "android")]
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// 闸门 `checkConfig` 超时。纯 CPU 的 decode+initialize（不 `Start`、不绑端口、不开 `cache.db`），
/// 与桌面 `CONFIG_CHECK_TIMEOUT` 同量级；超时走 `Unavailable` ⇒ fail-open 放行到起核。
#[cfg(target_os = "android")]
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// 授权状态查询超时。Kotlin 侧只做一次 `VpnService.prepare` 的 binder 往返（且已在工作线程上），
/// 故它是四档里最短的一档：这条腿在**每次回到前台**被调，等久了不如尽快落成
/// [`VpnAuthState::Unknown`] 让界面照实说「读不到」。
#[cfg(target_os = "android")]
const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// 交互授权单独给两分钟；超时后的回调只结算授权，不得自行起核。
#[cfg(target_os = "android")]
const AUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// 本地状态读写超时（系统起核对账 `systemStartStatus`、开机自动连接开关的读/写）。Kotlin 侧只读桥内存
/// 两位 / 读写 `noBackupFilesDir` 里一个标记文件，与 `vpnAuthStatus` 同量级，取同一档。
#[cfg(target_os = "android")]
const LOCAL_STATE_TIMEOUT: Duration = Duration::from_secs(5);

/// 交系统安装器超时（W-21）。Kotlin 侧只做「查一次安装权限 + `FileProvider.getUriForFile` +
/// `startActivity`」，全是本地调用与一次 binder 往返，比 `checkConfig` 的 decode+initialize 还轻。
///
/// **它不是「装完」的预算**：`startActivity` 一返回这条腿就结束了，之后是系统安装器与用户的事，
/// 我们既不等也等不到。把它设成分钟级只会让「交不出去」这件事晚几十秒才被说出来。
#[cfg(target_os = "android")]
const INSTALL_APK_TIMEOUT: Duration = Duration::from_secs(10);

/// 已装应用枚举超时（W-09b）。它是六档里**唯一随机器规模变的**一档：一台装了两三百个应用的
/// 机器要为每条结果 `loadLabel`（读对方 APK 的资源），实测量级是几百毫秒到几秒。
///
/// 取 20s 的依据：比 `stop`（只拆隧道）宽，比 `start`（同步走完内核启动）窄 —— 它比停核重、
/// 比起核轻。再短会把「应用装得多」误判成挂死，而那条腿失败的用户可见后果是「挑不了应用」。
#[cfg(target_os = "android")]
const INSTALLED_APPS_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindableInterface {
    pub name: String,
    pub display_name: String,
    pub is_up: bool,
    pub addresses: Vec<String>,
}

#[cfg(target_os = "android")]
pub(crate) async fn bindable_interfaces() -> Result<Vec<BindableInterface>, String> {
    #[derive(serde::Deserialize)]
    struct BindableInterfacesResponse {
        interfaces: Vec<BindableInterface>,
    }
    let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
    call_with_budget::<BindableInterfacesResponse, _>(
        plugin,
        "listBindableInterfaces",
        (),
        LOCAL_STATE_TIMEOUT,
        None,
    )
    .await
    .map(|response| response.interfaces)
    .map_err(|error| match error {
        BridgeCallError::Invoke(_) => "Android network interface enumeration failed".to_owned(),
        BridgeCallError::TimedOut => "Android network interface enumeration timed out".to_owned(),
        BridgeCallError::TaskFailed(_) => {
            "Android network interface enumeration unavailable".to_owned()
        }
    })
}

/// 🔴 **六档超时的相对大小是判据的一部分，编译期就挡住**。
///
/// `authStatus` 只是一次 binder 往返（最短），`installApk` 是本地调用 + 一次 startActivity，
/// `check` 是纯 CPU 的 decode+initialize，`stop` 只拆隧道，`installedApps` 随已装应用数变，
/// `start` 要同步走完内核启动（最长）。任何一档反了，症状要么是「慢起被当成挂死」（check/start 反）
/// 要么是「挂死等到天荒地老」。放编译期而不是单测：这几个数只在 Android 编译单元里存在，
/// 写成 `#[test]` 就只能在一个永远不跑 Android target 的机器上装装样子。
#[cfg(target_os = "android")]
const _: () = {
    assert!(AUTH_STATUS_TIMEOUT.as_secs() < CHECK_TIMEOUT.as_secs());
    assert!(CHECK_TIMEOUT.as_secs() < STOP_TIMEOUT.as_secs());
    assert!(STOP_TIMEOUT.as_secs() < START_TIMEOUT.as_secs());
    assert!(START_TIMEOUT.as_secs() <= 60);
    // 新两档各自挂在链条上，而不是硬塞进那条全序里：`installApk` 与 `checkConfig` 允许同档
    // （两者都是「一次本地重活」量级），故这里用 `<=` 而不是 `<` —— 写 `<` 会逼下一个人为了
    // 满足一条并不存在的约束去改一个与它无关的数。
    assert!(AUTH_STATUS_TIMEOUT.as_secs() < INSTALL_APK_TIMEOUT.as_secs());
    assert!(INSTALL_APK_TIMEOUT.as_secs() <= CHECK_TIMEOUT.as_secs());
    assert!(STOP_TIMEOUT.as_secs() < INSTALLED_APPS_TIMEOUT.as_secs());
    assert!(INSTALLED_APPS_TIMEOUT.as_secs() < START_TIMEOUT.as_secs());
    // 本地状态档与授权档同量级（都是一次无重活的往返），必须短于任何「真干活」的一档。
    assert!(LOCAL_STATE_TIMEOUT.as_secs() <= AUTH_STATUS_TIMEOUT.as_secs());
    assert!(LOCAL_STATE_TIMEOUT.as_secs() < CHECK_TIMEOUT.as_secs());
};

/// 一条**用户可见的**已装应用（Kotlin `installedApp(…)` 工厂造的回包元素）。
///
/// 消费方是「自定义应用分流」的包名选择器：`CustomAppPreset` 只带 `processNames`（桌面语义），
/// 而 Android 侧 `addDisallowedApplication` 认的是 applicationId，故
/// `crates/config-engine/src/user_config/app_rules_preset.rs` 里自定义预设的 `package_names`
/// 今天恒空 —— 「自定义应用设成直连」在 Android 上一条规则都命不中，且是静默的。本类型是那一格
/// 的数据源。
///
/// 字段名与 Kotlin `installedApp` 里的三个 `put(…)` 逐字对齐，由
/// `scripts/check-android-bridge.mjs` 的 A12 双向对拍（函数名 `installedApp` ⇄ 结构体名
/// `InstalledApp` 同源，不维护映射表）。
///
/// `allow(dead_code)` **只挂在非 Android**：那里 [`installed_apps`] 结构上只可能返回 `Err`，
/// 三个字段自然没有读点。Android 编译单元里不挂 —— 真有一格变死时它照常自曝（本仓既定口径）。
#[cfg_attr(
    not(target_os = "android"),
    allow(
        dead_code,
        reason = "non-Android has no package manager; installed_apps() can only return Err"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledApp {
    /// 显示名（`ResolveInfo.loadLabel`）。同一个包在不同语言下不同，故**不能**当标识用。
    pub label: String,
    /// applicationId —— 这才是标识，也是路由规则真正要存的那一列。
    pub package_name: String,
    /// 是否系统应用（`FLAG_SYSTEM`）。只给渲染端分组用，后端不消费。
    pub system: bool,
}

/// 「把 APK 交给系统安装器」这一次的结局。
///
/// 🔴 `handed_off == false` **不是错误**，是一个有原因的事实：用户没授予「安装未知应用」是
/// 最常见的一种，而它的下一步（去那一页按开关）与「桥挂了」完全不同。把两者压成同一个 `Err`
/// 会让界面只能说一句「安装失败」—— 那正是本条腿要消灭的形态。
///
/// `reason` 的取值集由 Kotlin 侧 `PolarisVpnPlugin` 的 `REASON_*` 常量收口（本侧不解释、不翻译、
/// 不兜底，原样转述给调用方）。
#[cfg_attr(
    not(target_os = "android"),
    allow(
        dead_code,
        reason = "non-Android has no system package installer; the handoff only returns Err"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApkHandoff {
    /// 系统安装器真的被拉起来了。之后是用户与系统的事，本进程既不等也等不到结果。
    pub handed_off: bool,
    /// 没交出去的原因码（Kotlin 侧 `REASON_*`）。`handed_off == true` 时恒 `None`。
    pub reason: Option<String>,
}

/// 系统 VPN 授权状态的三态（设置页那颗状态芯片的真值）。
///
/// 🔴 `Unknown` **不是** `Denied` 的同义词。桥未接线 / 超时 / Kotlin 侧抛异常都落 `Unknown`，
/// 把它们折成 `Denied` 等于**编一个事实**（IA §2.4「没有登记来源的数据位不许编」），而且方向是
/// 有害的那一边：用户会被指去系统设置里授予一个其实已经给过的权限。
///
/// `allow(dead_code)` **只挂在非 Android**：那里 [`auth_status`] 结构上只可能返回 `Unknown`，
/// 另两支自然是死代码。Android 编译单元里不挂 —— 真有一支变死时它照常自曝（本仓既定口径：
/// 不用一个无条件的 allow 把「真的死了」那天一并盖住，见 `app_language.rs:68`）。
#[cfg_attr(
    not(target_os = "android"),
    allow(
        dead_code,
        reason = "non-Android has no system VPN authorization object; only Unknown is reachable"
    )
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum VpnAuthState {
    /// `VpnService.prepare()` 返回 `null` —— 起核不会再弹系统授权窗。
    Authorized,
    /// `VpnService.prepare()` 返回了待确认的 Intent —— 连接前需要请求系统授权。
    Denied,
    /// 读不到（桥未接线 / 超时 / 本平台没有这条腿）。**不许当成未授权**。
    Unknown,
}

/// 桥失败的两轴：用户可见消息 + [`code`] 模块里的结构化码。
pub(super) type BridgeError = (String, &'static str);

/// Builder calls accepted before `establish()` and fd ownership accepted by the exact
/// main-core attempt. This is observed scope, not a `PlatformReceipt::Complete` verdict.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidTunScope {
    pub auto_route: bool,
    pub routes: Vec<String>,
    pub excluded_routes: Vec<String>,
    pub skipped_excludes: Vec<String>,
    pub allowed_packages: Vec<String>,
    pub excluded_packages: Vec<String>,
    pub skipped_packages: Vec<String>,
}

/// Only a successful, same-attempt libbox start can produce this bridge response.
/// `tun=None` means OS protection is unknown, even when the core started.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidStartReceipt {
    pub run_id: String,
    pub birth_nonce: String,
    pub config_digest: String,
    pub claim: Option<String>,
    pub tun: Option<AndroidTunScope>,
}

impl AndroidStartReceipt {
    #[cfg(any(target_os = "android", test))]
    fn matches_request(&self, run_id: &str, config_digest: &str, claim: Option<&str>) -> bool {
        self.exact_target().is_valid()
            && self.run_id == run_id
            && self.config_digest == config_digest
            && self.claim.as_deref() == claim
    }

    pub(super) fn exact_target(&self) -> AndroidExactTarget {
        AndroidExactTarget {
            run_id: self.run_id.clone(),
            birth_nonce: self.birth_nonce.clone(),
        }
    }

    /// Necessary observed facts only. The coordinator still has to compare plan Q,
    /// app scope, excluded routes, claim, and live generation before any Complete.
    #[cfg(any(target_os = "android", test))]
    pub(super) fn managed_tun_evidence(&self) -> Option<&AndroidTunScope> {
        let tun = self.tun.as_ref()?;
        (self.claim.is_some()
            && tun.auto_route
            && !tun.routes.is_empty()
            && tun.skipped_excludes.is_empty()
            && tun.skipped_packages.is_empty())
        .then_some(tun)
    }
}

/// The attempt's birth nonce comes from Kotlin, not from the request or Service instance.
/// A run ID alone may be reused after Service recreation and is not a stop authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidExactTarget {
    pub run_id: String,
    pub birth_nonce: String,
}

impl AndroidExactTarget {
    pub(super) fn is_valid(&self) -> bool {
        !self.run_id.is_empty()
            && self.run_id.trim() == self.run_id
            && self.run_id.encode_utf16().count() <= 128
            && !self.birth_nonce.is_empty()
            && self.birth_nonce.trim() == self.birth_nonce
            && self.birth_nonce.encode_utf16().count() <= 128
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[cfg(any(target_os = "android", test))]
pub(super) enum AndroidExactStopState {
    Closed,
    AlreadyGone,
    Busy,
    Unknown,
}

/// Closed/AlreadyGone require the Kotlin registry's exact native-close tombstone.
/// Busy is custody only; Unknown never authorizes a replacement start.
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidExactStopReceipt {
    pub run_id: String,
    pub birth_nonce: String,
    pub state: AndroidExactStopState,
    pub reason: Option<String>,
}

#[cfg(any(target_os = "android", test))]
impl AndroidExactStopReceipt {
    fn matches_target(&self, target: &AndroidExactTarget) -> bool {
        self.run_id == target.run_id
            && self.birth_nonce == target.birth_nonce
            && match self.state {
                AndroidExactStopState::Unknown => self
                    .reason
                    .as_ref()
                    .is_some_and(|reason| !reason.is_empty()),
                _ => self.reason.is_none(),
            }
    }

    pub(super) fn confirms_closed(&self) -> bool {
        // Target-local native close only: not registry vacancy or Android OS Complete.
        matches!(
            self.state,
            AndroidExactStopState::Closed | AndroidExactStopState::AlreadyGone
        )
    }
}

#[cfg(target_os = "android")]
fn exact_main_core_result(
    result: Result<AndroidExactStopReceipt, BridgeCallError>,
    target: &AndroidExactTarget,
) -> Result<AndroidExactStopReceipt, String> {
    let receipt = result.map_err(|error| match error {
        BridgeCallError::Invoke(e) => format!("Android 确切主核桥失败：{e}"),
        BridgeCallError::TimedOut => "Android 确切主核桥超时，目标状态未知".to_string(),
        BridgeCallError::TaskFailed(e) => format!("Android 确切主核桥投递失败：{e}"),
    })?;
    if receipt.matches_target(target) {
        Ok(receipt)
    } else {
        Err("Android 确切主核回执身份或形状无效，目标状态未知".into())
    }
}

/** Read only: absent registry state, a lost process, and a different owner remain Unknown. */
#[cfg(any(target_os = "android", test))]
pub(super) async fn main_core_exact_status(
    target: &AndroidExactTarget,
) -> Result<AndroidExactStopReceipt, String> {
    if !target.is_valid() {
        return Err("Android 确切主核目标无效".into());
    }
    #[cfg(target_os = "android")]
    {
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        let result = call_with_budget::<AndroidExactStopReceipt, _>(
            plugin,
            "mainCoreExactStatus",
            AndroidExactTarget {
                run_id: target.run_id.clone(),
                birth_nonce: target.birth_nonce.clone(),
            },
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await;
        exact_main_core_result(result, target)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("本平台没有 Android 确切主核状态来源".into())
    }
}

/** Request close of only this exact attempt; request delivery is never an exit receipt. */
#[cfg(any(target_os = "android", test))]
pub(super) async fn stop_core_exact(
    target: &AndroidExactTarget,
) -> Result<AndroidExactStopReceipt, String> {
    if !target.is_valid() {
        return Err("Android 确切主核目标无效".into());
    }
    #[cfg(target_os = "android")]
    {
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        let result = call_with_budget::<AndroidExactStopReceipt, _>(
            plugin,
            "stopMainCoreExact",
            AndroidExactTarget {
                run_id: target.run_id.clone(),
                birth_nonce: target.birth_nonce.clone(),
            },
            STOP_TIMEOUT,
            None,
        )
        .await;
        exact_main_core_result(result, target)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("本平台没有 Android 确切主核停机来源".into())
    }
}

/// A registry snapshot is not a liveness probe, nor a NoOldCore receipt by itself.
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidMainCoreOwnership {
    pub state: String,
    pub run_id: Option<String>,
}

#[cfg(any(target_os = "android", test))]
pub(super) async fn main_core_ownership() -> Result<AndroidMainCoreOwnership, String> {
    #[cfg(target_os = "android")]
    {
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        let result = call_with_budget::<AndroidMainCoreOwnership, _>(
            plugin,
            "mainCoreOwnership",
            (),
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        .map_err(|error| match error {
            BridgeCallError::Invoke(e) => format!("Android 主核归属读取失败：{e}"),
            BridgeCallError::TimedOut => "Android 主核归属读取超时，归属未知".to_string(),
            BridgeCallError::TaskFailed(e) => format!("Android 主核归属投递失败：{e}"),
        })?;
        match (result.state.as_str(), result.run_id.as_deref()) {
            ("absent", None) => Ok(result),
            ("starting" | "acknowledged" | "closing" | "cleanupUnknown", Some(id))
                if !id.is_empty() =>
            {
                Ok(result)
            }
            _ => Err("Android 主核归属回执形状无效，归属未知".to_string()),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("本平台没有 Android 主核归属来源".to_string())
    }
}

/// Process-local fence status only. This does not acquire the fence, publish a
/// Preparing marker, or establish a global NoOldCore receipt.
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidLegacyDrainStatus {
    pub fence_id: String,
    pub process_nonce: String,
    pub state: String,
    pub run_id: Option<String>,
    pub closed_run_id: Option<String>,
    pub reason: Option<String>,
}

#[cfg(any(target_os = "android", test))]
impl AndroidLegacyDrainStatus {
    fn matches_request(&self, fence_id: &str) -> bool {
        if self.fence_id != fence_id || self.process_nonce.is_empty() {
            return false;
        }
        match self.state.as_str() {
            "vacant" => {
                self.run_id.is_none()
                    && self.reason.is_none()
                    && self.closed_run_id.as_ref().is_none_or(|id| !id.is_empty())
            }
            "owned" => {
                self.run_id.as_ref().is_some_and(|id| !id.is_empty())
                    && self.closed_run_id.is_none()
                    && self.reason.is_none()
            }
            "unknown" => self
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty()),
            _ => false,
        }
    }
}

#[cfg(any(target_os = "android", test))]
pub(super) async fn legacy_drain_status(
    fence_id: &str,
) -> Result<AndroidLegacyDrainStatus, String> {
    if fence_id.is_empty() || fence_id.trim() != fence_id {
        return Err("Android legacy fence ID 无效".into());
    }
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct FenceArgs {
            fence_id: String,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        let status = call_with_budget::<AndroidLegacyDrainStatus, _>(
            plugin,
            "legacyDrainStatus",
            FenceArgs {
                fence_id: fence_id.to_owned(),
            },
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        .map_err(|error| match error {
            BridgeCallError::Invoke(e) => format!("Android legacy fence 读取失败：{e}"),
            BridgeCallError::TimedOut => "Android legacy fence 读取超时，旧核未知".into(),
            BridgeCallError::TaskFailed(e) => format!("Android legacy fence 投递失败：{e}"),
        })?;
        if status.matches_request(fence_id) {
            Ok(status)
        } else {
            Err("Android legacy fence 回执身份或形状无效，旧核未知".into())
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("本平台没有 Android legacy fence 来源".into())
    }
}

/// Kotlin 侧 `reject(msg, code)` 的 code → 本仓码的**白名单**映射。
///
/// **必须是白名单而不是原样透传**：`code` 模块的头注写死了「只收录控制流位置能诚实断言的码」，
/// 而透传等于让 Kotlin 侧可以凭空造一个本仓从未声明的码，直接绕过前端覆盖门
/// （`proxy-error-key-coverage.test.ts` 的 G1 只对账 Rust `mod code` 的声明集）。
/// 认不出来的一律降到 `STARTUP_FAILED`：那是「起核腿失败」这一事实本身，不是猜测。
fn map_rejected_code(code: Option<&str>) -> &'static str {
    match code {
        Some(c) if c == code::VPN_PERMISSION_DENIED => code::VPN_PERMISSION_DENIED,
        Some(c) if c == code::SYSTEM_INTERFACE_UNSUPPORTED => code::SYSTEM_INTERFACE_UNSUPPORTED,
        Some(c) if CapacityClosed::from_code(Some(c)).is_some() => {
            code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED
        }
        Some(c) if c == ENDPOINT_RETIRED_NO_BIRTH => ENDPOINT_RETIRED_NO_BIRTH,
        _ => code::STARTUP_FAILED,
    }
}

/// Kotlin emits this only before VpnBridge.beginStart/native attempt creation.
/// It is an internal retry receipt, never exposed as a ProxyStatus error code.
pub(super) const ENDPOINT_RETIRED_NO_BIRTH: &str = "API_ENDPOINT_RETIRED";

#[cfg(target_os = "android")]
mod handle {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use tauri::plugin::PluginHandle;
    use tauri::Wry;

    /// 插件句柄。
    ///
    /// **`OnceLock` 而非构造参数**（同 `ProxyRuntime::error_emitter` 的理由）：`PluginApi` 只在
    /// Tauri `setup` 之后才有，而 `ProxyRuntime` 在 `AppRuntime::new(config_dir)` 里就得造出来。
    /// 未接线（setup 前的极早期失败）⇒ 起停核返一个**有码**的错误，绝不 panic、也绝不静默成功。
    pub(super) static PLUGIN: OnceLock<PluginHandle<Wry>> = OnceLock::new();

    /// 起核回执的记账位（`is_alive` 的 Android 腿真值）。
    ///
    /// **是「提交记录」而不是「存活探测」**，与 helper 腿的 `self.pid` 同一类事实：它说的是
    /// 「我们请服务起过核、且它回执成功，之后没人请它停」。核在本进程内（libbox），没有 pid 可
    /// `kill(0)`、没有 child 可 `try_wait`，而 `is_alive` 是**同步**闭包（`Fn()->bool`）—— 跨桥问一次
    /// `BoxService.state` 是异步往返，放进那个闭包只能 `block_on`，而它本就跑在 async 上下文里（必 panic）。
    ///
    /// **射程自曝**：核若在本进程内自行死掉（用户在系统设置里撤销 VPN 授权 → `onRevoke`），本位
    /// 不会翻转。起核期由就绪门的 TCP 探测兜住（管理口连不上 ⇒ `Timeout` ⇒ 有码失败）；就绪之后
    /// Android 上崩溃自愈本就不适用（`spawn_crash_monitor` 观察到 `child=None` ⇒ `Absent` ⇒ `Retire`）。
    /// 补上这条需要 Kotlin 侧主动向 Rust 推事件，属另一批的射程。
    pub(super) static CORE_STARTED: AtomicBool = AtomicBool::new(false);

    pub(super) fn set_core_started(v: bool) {
        CORE_STARTED.store(v, Ordering::SeqCst);
    }
}

/// 注册 Android 侧插件。**只在 `lib.rs` 的 builder 链上调用一次**。
///
/// 非泛型（恒 `Wry`）是为了让句柄能进一个 `OnceLock` 静态：`PluginHandle<R>` 带泛型参数就没法
/// 存成静态，而本仓的 app 恒是 `Wry`，为一个不存在的第二 runtime 付泛型的账是 YAGNI。
#[cfg(target_os = "android")]
pub(crate) fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .setup(|_app, api| {
            let plugin = api.register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)?;
            // 重复注册（理论上不会发生：builder 链上只有一处）⇒ 记日志不 panic，
            // 先到的那个句柄照样能用。
            if handle::PLUGIN.set(plugin).is_err() {
                log::warn!("Android 起停核桥插件被注册了两次，沿用先到的句柄");
            }
            log::info!("Android 起停核桥已注册：{PLUGIN_IDENTIFIER}/{PLUGIN_CLASS}");
            Ok(())
        })
        .build()
}

/// 一次跨桥调用的结局（[`call_with_budget`] 的返回三态）。
///
/// `TimedOut` 与 `TaskFailed` 分开：前者是「预算内没等到」（可能只是慢），后者是
/// 「投递腿自己塌了」（tauri 内部 panic / runtime 关停），两者的用户可见文案与后续动作不同。
#[cfg(target_os = "android")]
enum BridgeCallError {
    Invoke(tauri::plugin::mobile::PluginInvokeError),
    TimedOut,
    TaskFailed(String),
}

/// 跨桥调用一次 Kotlin 命令，带**不丢 future** 的超时预算。
///
/// # 🔴 为什么不能直接 `tokio::time::timeout(D, plugin.run_mobile_plugin_async(..))`
///
/// 超时腿会 **drop 掉** 那个 future，连同它内部持有的 `oneshot::Receiver`
/// （`tauri-2.11.5/src/plugin/mobile.rs:298` 的 `rx`）。而它注册给 Kotlin 的回执闭包写的是
/// `tx.lock().unwrap().take().unwrap().send(response).unwrap()`（同文件 :307）——
/// 接收端已经没了，`tokio::sync::oneshot::Sender::send` 返回 `Err(v)`，那个 `.unwrap()`
/// 就在**投递回执的那个线程**上 panic。该闭包由 wry 的 dispatch 在 activity looper 上执行、
/// 调用栈来自 JNI，panic 穿不过 `extern "C"` 边界 ⇒ 进程 abort，现场只剩一行 tombstone。
///
/// 触发条件不是理论上的：闭包排在 activity looper 队列里，**activity 不在时只排队不执行**
/// （见模块头注）。用户在起核期间把应用切到后台 → 预算先到 → 之后又切回前台 → 队列被排空
/// → 迟到回执落地 → abort。这正是「超时是判据的一部分」那条设计给自己留的反噬面。
///
/// # 改法：超时 = 不再等它，而不是把它丢掉
///
/// 调用交给一个**分离的 task** 持有到底（`JoinHandle` 被 drop **不**取消 task），超时只放弃
/// `JoinHandle`。接收端因此始终活着：迟到的回执被正常收下然后丢弃，panic 面整个不存在。
/// 代价是超时后那条 task 可能长期挂着（activity 再也不回来时）—— 一个 idle 的 `await`，
/// 不占 CPU、不持锁，比一次 abort 便宜得多。
///
/// # 与 K5 数据面那条「把超时判据搬到 Kotlin」**刻意不同**
///
/// 那一条盖的是「Kotlin 被调到了但慢」；本模块要盖的是「Kotlin 根本没被调到（前台无 Activity）」。
/// 后者 Kotlin 自己计不了时（它压根没跑），deadline 必须留在 Rust 侧 —— 照抄 K5 的改法会把
/// 那条腿退化成永久挂起，也就是本批明令禁止的「静默没起来」。
#[cfg(target_os = "android")]
async fn call_with_budget<T, P>(
    plugin: &'static tauri::plugin::PluginHandle<tauri::Wry>,
    command: &'static str,
    payload: P,
    budget: Duration,
    request_birth: Option<super::AndroidRequestBirth>,
) -> Result<T, BridgeCallError>
where
    T: serde::de::DeserializeOwned + Send + 'static,
    P: serde::Serialize + Send + 'static,
{
    let task = tokio::spawn(async move {
        // The detached task, not its cancellable waiter, keeps the birth
        // alive through a delayed Kotlin callback or Rust-side timeout.
        let _request_birth = request_birth;
        plugin.run_mobile_plugin_async::<T>(command, payload).await
    });
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(e))) => Err(BridgeCallError::Invoke(e)),
        Ok(Err(join)) => Err(BridgeCallError::TaskFailed(join.to_string())),
        Err(_) => Err(BridgeCallError::TimedOut),
    }
}

/// Debug 报告使用相同的保活调用器；失败不影响代理生命周期。
#[cfg(all(target_os = "android", debug_assertions))]
pub(crate) async fn debug_batch_qa(
    action: String,
    session_id: Option<String>,
    plan: Option<String>,
) -> Result<String, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DebugBatchQaArgs {
        action: String,
        session_id: Option<String>,
        plan: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct DebugBatchQaResponse {
        report: String,
    }
    let plugin = plugin_handle().map_err(|_| "Android batch bridge unavailable".to_owned())?;
    call_with_budget::<DebugBatchQaResponse, _>(
        plugin,
        "debugBatchQa",
        DebugBatchQaArgs {
            action,
            session_id,
            plan,
        },
        Duration::from_secs(10),
        None,
    )
    .await
    .map(|result| result.report)
    .map_err(|_| "Android batch request unavailable".to_owned())
}

#[cfg(all(target_os = "android", debug_assertions))]
pub(super) async fn debug_core_probe_scope(
    session_id: String,
) -> Result<super::android_probe_loan::DebugCoreProbeSessionScope, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DebugCoreProbeSessionArgs {
        session_id: String,
    }
    #[derive(serde::Deserialize)]
    struct DebugCoreProbeScopeResponse {
        scope: super::android_probe_loan::DebugCoreProbeSessionScope,
    }
    let plugin = plugin_handle().map_err(|_| "Core probe bridge unavailable".to_owned())?;
    call_with_budget::<DebugCoreProbeScopeResponse, _>(
        plugin,
        "debugCoreProbeScope",
        DebugCoreProbeSessionArgs { session_id },
        Duration::from_secs(5),
        None,
    )
    .await
    .map(|r| r.scope)
    .map_err(|_| "Core probe scope unavailable".to_owned())
}

/// Native plugin has no JS invoke_handler. Neither the public batch command nor any
/// config/report command accepts or returns this credential-bearing type.
#[cfg(all(target_os = "android", debug_assertions))]
pub(super) async fn debug_core_probe_loan(
    loan: super::android_probe_loan::DebugCoreProbeLoanPayload,
) -> Result<String, String> {
    #[derive(serde::Serialize)]
    struct DebugCoreProbeLoanEnvelopeArgs {
        loan: super::android_probe_loan::DebugCoreProbeLoanPayload,
    }
    #[derive(serde::Deserialize)]
    struct DebugCoreProbeLoanResponse {
        report: String,
    }
    let plugin = plugin_handle().map_err(|_| "Core probe bridge unavailable".to_owned())?;
    call_with_budget::<DebugCoreProbeLoanResponse, _>(
        plugin,
        "debugCoreProbeLoan",
        DebugCoreProbeLoanEnvelopeArgs { loan },
        Duration::from_secs(125), // original remote deadline/session guardian still closes resources
        None,
    )
    .await
    .map(|r| r.report)
    .map_err(|_| "Core probe loan unavailable".to_owned())
}

#[cfg(all(target_os = "android", debug_assertions))]
pub(super) async fn debug_pc_echo_prepare(
    session_id: String,
) -> Result<super::debug_pc_echo::PrivatePcReady, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        session_id: String,
    }
    #[derive(serde::Deserialize)]
    struct DebugPcEchoPrepareResponse {
        ready: super::debug_pc_echo::PrivatePcReady,
    }
    let plugin = plugin_handle().map_err(|_| "Current PC Ready unavailable".to_owned())?;
    call_with_budget::<DebugPcEchoPrepareResponse, _>(
        plugin,
        "debugPcEchoPrepare",
        Args { session_id },
        Duration::from_secs(5),
        None,
    )
    .await
    .map(|r| r.ready)
    .map_err(|_| "Current PC Ready unavailable".to_owned())
}

#[cfg(all(target_os = "android", debug_assertions))]
pub(crate) async fn collect_debug_diagnostics() -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Diagnostics {
        text: String,
    }
    let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
    match call_with_budget::<Diagnostics, _>(
        plugin,
        "collectDebugDiagnostics",
        (),
        Duration::from_secs(10),
        None,
    )
    .await
    {
        Ok(result) => Ok(result.text),
        Err(_) => Err("Android diagnostics unavailable".into()),
    }
}

#[cfg(all(target_os = "android", debug_assertions))]
pub(crate) async fn share_debug_report(report: String) -> Result<(), String> {
    #[derive(serde::Serialize)]
    struct Report {
        report: String,
    }
    let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
    match call_with_budget::<(), _>(
        plugin,
        "shareDebugReport",
        Report { report },
        Duration::from_secs(10),
        None,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(_) => Err("Could not open Android report sharing".into()),
    }
}

/// 连接前请求系统授权。沿用不丢回执 future 的调用器，防止超时后的授权回调触发 JNI panic。
#[cfg(target_os = "android")]
pub(super) async fn request_vpn_permission() -> Result<(), BridgeError> {
    let plugin = plugin_handle()?;
    match call_with_budget::<(), _>(
        plugin,
        "requestVpnPermission",
        (),
        AUTH_REQUEST_TIMEOUT,
        None,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(BridgeCallError::Invoke(e)) => Err(invoke_error(&e)),
        Err(BridgeCallError::TimedOut) => Err((
            "等待 VPN 授权超时，请完成系统授权后再次点击连接".to_string(),
            code::VPN_PERMISSION_DENIED,
        )),
        Err(BridgeCallError::TaskFailed(e)) => Err((
            format!("Android VPN 授权请求失败：{e}"),
            code::STARTUP_FAILED,
        )),
    }
}

/// 起核：把 config-engine 产出的**那一份字符串**交给 Kotlin 侧的 libbox。
///
/// 传的是内存字符串而不是盘上路径，且它与 `std::fs::write(config_path, &json)` 写下去的是
/// **同一个 `json` 变量**（不是两次序列化）⇒ 诊断包里那份与内核实际吃的那份不可能漂。
/// 一旦有人改成「从盘上读回来再传给 libbox」，诊断与内核就有了两条路径。
pub(super) fn main_start_dispatch_available() -> Result<(), BridgeError> {
    #[cfg(target_os = "android")]
    {
        plugin_handle().map(|_| ())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err(("Android 起核桥在本平台不存在".into(), code::STARTUP_FAILED))
    }
}

#[cfg(test)]
pub(super) async fn start_core(config_json: &str) -> Result<AndroidStartReceipt, BridgeError> {
    let run_id = format!(
        "legacy-{}{}",
        polaris_store::fs::random_tmp_suffix(),
        polaris_store::fs::random_tmp_suffix()
    );
    start_core_with_claim_and_birth(config_json, &run_id, None, None).await
}

pub(super) async fn start_core_with_birth(
    config_json: &str,
    request_birth: super::AndroidRequestBirth,
) -> Result<AndroidStartReceipt, BridgeError> {
    let run_id = format!(
        "legacy-{}{}",
        polaris_store::fs::random_tmp_suffix(),
        polaris_store::fs::random_tmp_suffix()
    );
    start_core_with_claim_and_birth(config_json, &run_id, None, Some(request_birth)).await
}

async fn start_core_with_claim_and_birth(
    config_json: &str,
    run_id: &str,
    claim: Option<&str>,
    request_birth: Option<super::AndroidRequestBirth>,
) -> Result<AndroidStartReceipt, BridgeError> {
    if run_id.trim().is_empty() || run_id.trim() != run_id || claim.is_some_and(str::is_empty) {
        return Err(("Android 起核身份无效".to_string(), code::STARTUP_FAILED));
    }
    let config_digest = polaris_updater::verify::sha256_hex(config_json.as_bytes());
    #[cfg(target_os = "android")]
    {
        // 载荷持有的是 `String` 而不是 `&str`：调用交给分离 task 持有到底（见
        // [`call_with_budget`]），故它必须 `'static`。**同源不变式没变**：这一份是
        // `std::fs::write(config_path, &json)` 那个 `json` 变量的逐字节副本（一次克隆，不是
        // 第二次序列化），诊断包里那份与内核实际吃的那份依然不可能漂。
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct MainStartArgs {
            config_content: String,
            run_id: String,
            config_digest: String,
            claim: Option<String>,
        }
        let plugin = plugin_handle()?;
        match call_with_budget::<AndroidStartReceipt, _>(
            plugin,
            "start",
            MainStartArgs {
                config_content: config_json.to_owned(),
                run_id: run_id.to_owned(),
                config_digest: config_digest.clone(),
                claim: claim.map(str::to_owned),
            },
            START_TIMEOUT,
            request_birth,
        )
        .await
        {
            Ok(receipt) if receipt.matches_request(run_id, &config_digest, claim) => {
                handle::set_core_started(true);
                Ok(receipt)
            }
            Ok(_) => Err((
                "Android 起核回执身份不符（内核可能仍在，须核实并关闭确切 run）".to_string(),
                code::STARTUP_FAILED,
            )),
            Err(BridgeCallError::Invoke(e)) => Err(invoke_error(&e)),
            Err(BridgeCallError::TimedOut) => Err((
                format!(
                    "Android 起核桥 {}s 无回应（前台无 Activity？）",
                    START_TIMEOUT.as_secs()
                ),
                code::STARTUP_FAILED,
            )),
            Err(BridgeCallError::TaskFailed(e)) => Err((
                format!("Android 起核桥投递腿异常：{e}"),
                code::STARTUP_FAILED,
            )),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (config_json, config_digest, claim, request_birth);
        Err((
            "Android 起核桥在本平台不存在（调用点应由 cfg! 守住）".to_string(),
            // 走同一个白名单取默认码：这条腿与 Android 腿的「认不出来的码降级到什么」必须是同一个
            // 答案，各写各的迟早分叉。
            map_rejected_code(None),
        ))
    }
}

/// 停核：**要确定回执**（与 `kill_core_via_helper` 同构）。
///
/// 停不掉就返 `Err`，调用方不得据此继续清运行态或起第二个核 —— Android 上 tun fd 由
/// `VpnService.prepare()` 仲裁，同一时刻只授权一个应用，前一条隧道没拆干净就起第二个必然打架。
#[cfg(test)]
pub(super) async fn stop_core() -> Result<(), String> {
    stop_core_with_birth(super::AndroidRequestBirth {
        identity: std::sync::Arc::new(()),
        main_token: None,
    })
    .await
}

pub(super) async fn stop_core_with_birth(
    request_birth: super::AndroidRequestBirth,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        let r =
            match call_with_budget::<(), _>(plugin, "stop", (), STOP_TIMEOUT, Some(request_birth))
                .await
            {
                Ok(()) => Ok(()),
                Err(BridgeCallError::Invoke(e)) => Err(format!("Android 停核桥失败：{e}")),
                Err(BridgeCallError::TimedOut) => Err(format!(
                    "Android 停核桥 {}s 无回应（隧道可能仍在）",
                    STOP_TIMEOUT.as_secs()
                )),
                Err(BridgeCallError::TaskFailed(e)) => {
                    Err(format!("Android 停核桥投递腿异常：{e}（隧道可能仍在）"))
                }
            };
        // 记账只在**确认停下**时清：桥失败时隧道可能还在，把记账清成「没核」会让下一次起核
        // 以为现场是干净的。
        if r.is_ok() {
            handle::set_core_started(false);
        }
        r
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = request_birth;
        Err("Android 停核桥在本平台不存在（调用点应由 cfg! 守住）".to_string())
    }
}

/// 起核前的内核闸门（`config_gate`）在 Android 上的取证腿：问 libbox `CheckConfig` 收不收这份配置。
///
/// 三态映射（含「桥挂了 ⇒ `Unavailable`」这条收口）在
/// [`verdict_from_libbox_check`] 里，本函数只负责把桥的两种失败折成它的 `Err`。
///
/// `pub(crate)`：`commands/proxy.rs::kernel_probe_outbound`（custom 协议兼容性探测）在 Android 上
/// 走同一条腿 —— 两处各问各的，迟早有一处的超时 / 失败折叠跟不上另一处。
#[cfg(any(target_os = "android", test))]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckResponse {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
}

#[cfg(any(target_os = "android", test))]
fn check_response_verdict(response: CheckResponse) -> Result<ConfigCheckVerdict, CapacityClosed> {
    if let Some(error) = CapacityClosed::from_code(response.error_code.as_deref()) {
        return Err(error);
    }
    Ok(verdict_from_libbox_check(Ok(response.error)))
}

pub(crate) async fn check_config(config_json: &str) -> ConfigCheckVerdict {
    match check_config_admitted(config_json).await {
        Ok(verdict) => verdict,
        // The primary config gate retains its old fail-open behavior. Its next
        // owner reservation independently rejects with the explicit capacity code.
        Err(error) => ConfigCheckVerdict::Unavailable(error.to_string()),
    }
}

pub(crate) async fn check_config_admitted(
    config_json: &str,
) -> Result<ConfigCheckVerdict, CapacityClosed> {
    #[cfg(target_os = "android")]
    {
        // 拥有式载荷的理由同 `start_core`：调用由分离 task 持有到底（见 [`call_with_budget`]）。
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CheckArgs {
            config_content: String,
        }
        let plugin = match plugin_handle() {
            Ok(p) => p,
            Err((msg, _)) => return Ok(verdict_from_libbox_check(Err(msg))),
        };
        let result = match call_with_budget::<CheckResponse, _>(
            plugin,
            "checkConfig",
            CheckArgs {
                config_content: config_json.to_owned(),
            },
            CHECK_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => return check_response_verdict(r),
            Err(BridgeCallError::Invoke(e)) => Err(format!("{e}")),
            Err(BridgeCallError::TimedOut) => {
                Err(format!("checkConfig 超时（>{}s）", CHECK_TIMEOUT.as_secs()))
            }
            Err(BridgeCallError::TaskFailed(e)) => Err(format!("checkConfig 投递腿异常：{e}")),
        };
        Ok(verdict_from_libbox_check(result))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = config_json;
        // 非 Android 走不到这里（调用点在解析不到核二进制时已经 fail-open 早退）。真走到了也
        // 只是多一条 fail-open 的放行，绝不 panic —— 起核路径上不接受「判据本身把进程搞崩」。
        Ok(verdict_from_libbox_check(Err(
            "本平台没有 Android 起核桥".to_string()
        )))
    }
}

/// 读一次系统 VPN 授权状态（设置页那一行的后端）。
///
/// # 为什么是「读一次」而不是订阅
///
/// 授权在**应用之外**被改（用户去系统设置里撤销 / 在系统弹窗里授予），Android 不向应用推这件事的
/// 变更事件 —— `onRevoke()` 只在**本应用的隧道被顶掉**时回调，而「从未起过核时被撤销」压根不触发它。
/// 故没有可订阅的事件源，只有拉取；拉取的**时机**由渲染端定（回到前台时读一次，见
/// `ui/src/mobile/settings/MobileSettingsScreen.tsx`）。
///
/// # 失败一律 `Unknown`，绝不折成 `Denied`
///
/// 见 [`VpnAuthState`]。三种失败各记一行日志（可归因），但对界面只说「读不到」。
pub(crate) async fn auth_status() -> VpnAuthState {
    #[cfg(target_os = "android")]
    {
        /// Kotlin 侧 `invoke.resolve(JSObject)` 的回包。
        ///
        /// 字段名与 Kotlin `vpnAuthStatus` 里的 `put("authorized", …)` 逐字对齐，由
        /// `scripts/check-android-bridge.mjs` 的 A10 双向对拍 —— 改一侧不改另一侧，两侧都编得过，
        /// 真机上表现为「状态永远读不到」（serde 缺字段即 `Invoke` 错）。
        #[derive(serde::Deserialize)]
        struct AuthStatusResponse {
            authorized: bool,
        }
        let plugin = match plugin_handle() {
            Ok(p) => p,
            Err((msg, _)) => {
                log::warn!("VPN 授权状态读不到（桥未接线）：{msg}");
                return VpnAuthState::Unknown;
            }
        };
        match call_with_budget::<AuthStatusResponse, _>(
            plugin,
            "vpnAuthStatus",
            (),
            AUTH_STATUS_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => {
                if r.authorized {
                    VpnAuthState::Authorized
                } else {
                    VpnAuthState::Denied
                }
            }
            Err(BridgeCallError::Invoke(e)) => {
                log::warn!("VPN 授权状态读不到（Kotlin 侧拒绝）：{e}");
                VpnAuthState::Unknown
            }
            Err(BridgeCallError::TimedOut) => {
                log::warn!(
                    "VPN 授权状态读不到（{}s 无回应，前台无 Activity？）",
                    AUTH_STATUS_TIMEOUT.as_secs()
                );
                VpnAuthState::Unknown
            }
            Err(BridgeCallError::TaskFailed(e)) => {
                log::warn!("VPN 授权状态读不到（投递腿异常）：{e}");
                VpnAuthState::Unknown
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        // 本平台没有系统 VPN 授权这个对象（桌面走提权助手）。返 `Unknown` 而不是 `Authorized`：
        // 「这里没有这件事」与「这件事成立」是两回事，后者会让不该出现的一行说一句假话。
        VpnAuthState::Unknown
    }
}

/// 此刻是否有一个**不是本进程 Rust 起的**核在跑（系统经 always-on / 开机自动连接拉起的）。
///
/// 这种核的配置来自 Rust 上一次落盘的 `singbox-runtime.json`（Kotlin `SystemStart`），而本进程的
/// `ProxyRuntime` 对它一无所知（进程被回收后系统重拉 ⇒ Rust 是全新的）。消费方是两条**既有**路径：
///  · 启动期自动连接腿（`startup_tasks::decide_auto_connect`）：有这种核 ⇒ 走一次标准 `proxy_start`
///    把它收编，界面才会显示「已连接」而不是停在「未连接」；
///  · 起核前的孤儿清扫（`cleanup_stale_cores` 的 Android 腿）：先停掉它，再按当前配置起 ——
///    与桌面「上次会话遗留的孤儿核」同一个处置。
///
/// 读不到一律 `false`（记日志）：误判成「没有」的代价是界面仍显示未连接、下一次用户点连接时由清扫腿
/// 兜住；误判成「有」的代价是无端重起一次隧道。前者更便宜。
pub(crate) async fn system_started_core_running() -> bool {
    #[cfg(target_os = "android")]
    {
        /// 字段名与 Kotlin `systemStartStatus` 的 `put("systemStarted", …)` 由 A10 双向对拍。
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SystemStartStatus {
            system_started: bool,
        }
        let Ok(plugin) = plugin_handle() else {
            log::warn!("系统起核对账：桥未接线，按「没有」处理");
            return false;
        };
        match call_with_budget::<SystemStartStatus, _>(
            plugin,
            "systemStartStatus",
            (),
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => r.system_started,
            Err(BridgeCallError::Invoke(e)) => {
                log::warn!("系统起核对账读不到（Kotlin 侧拒绝）：{e}");
                false
            }
            Err(BridgeCallError::TimedOut) => {
                log::warn!(
                    "系统起核对账读不到（{}s 无回应）",
                    LOCAL_STATE_TIMEOUT.as_secs()
                );
                false
            }
            Err(BridgeCallError::TaskFailed(e)) => {
                log::warn!("系统起核对账读不到（投递腿异常）：{e}");
                false
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// 写「开机自动连接」开关（`auto_start_set` 的 Android 腿）。真值住 Kotlin：开机那一刻只有
/// `BootReceiver` 在跑，Rust 不在 —— 与桌面「写 OS launch agent」同构。
#[cfg(target_os = "android")]
pub(crate) async fn set_boot_auto_connect(enabled: bool) -> Result<(), String> {
    {
        #[derive(serde::Serialize)]
        struct BootAutoConnectArgs {
            enabled: bool,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<(), _>(
            plugin,
            "setBootAutoConnect",
            BootAutoConnectArgs { enabled },
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(BridgeCallError::Invoke(e)) => Err(format!("写开机自动连接开关失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "写开机自动连接开关 {}s 无回应",
                LOCAL_STATE_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => {
                Err(format!("写开机自动连接开关投递腿异常：{e}"))
            }
        }
    }
}

/// 读「开机自动连接」开关（`auto_start_get_status` 的 Android 腿）。
#[cfg(target_os = "android")]
pub(crate) async fn boot_auto_connect() -> Result<bool, String> {
    {
        /// 字段名与 Kotlin `bootAutoConnectStatus` 的 `put("enabled", …)` 由 A10 双向对拍。
        #[derive(serde::Deserialize)]
        struct BootAutoConnectStatus {
            enabled: bool,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<BootAutoConnectStatus, _>(
            plugin,
            "bootAutoConnectStatus",
            (),
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => Ok(r.enabled),
            Err(BridgeCallError::Invoke(e)) => Err(format!("读开机自动连接开关失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "读开机自动连接开关 {}s 无回应",
                LOCAL_STATE_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => {
                Err(format!("读开机自动连接开关投递腿异常：{e}"))
            }
        }
    }
}

/// 写「系统备份」开关（`system_backup_set`）。真值住 Kotlin `noBackupFilesDir`：系统做备份时只有
/// `PolarisBackupAgent` 在跑（受限模式，Rust 不在），它读的就是这一份 —— 与开机自动连接同构。
///
/// 非 Android 平台没有「系统自动备份」这个对象 ⇒ `Err`，不静默 `Ok`（静默成功会让调用方以为写进去了）。
pub(crate) async fn set_system_backup(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Serialize)]
        struct SystemBackupArgs {
            enabled: bool,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<(), _>(
            plugin,
            "setSystemBackup",
            SystemBackupArgs { enabled },
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(BridgeCallError::Invoke(e)) => Err(format!("写系统备份开关失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "写系统备份开关 {}s 无回应",
                LOCAL_STATE_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => Err(format!("写系统备份开关投递腿异常：{e}")),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = enabled;
        Err("system backup toggle is not available on this platform".to_string())
    }
}

/// 读「系统备份」开关（`system_backup_get_status`）。读不到就说读不到（`Err`），不折成 `false`：
/// 「关着」与「不知道」在设置页上是两句话。非 Android 同 [`set_system_backup`]。
pub(crate) async fn system_backup() -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        /// 字段名与 Kotlin `systemBackupStatus` 的 `put("enabled", …)` 由 A10 双向对拍。
        #[derive(serde::Deserialize)]
        struct SystemBackupStatus {
            enabled: bool,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<SystemBackupStatus, _>(
            plugin,
            "systemBackupStatus",
            (),
            LOCAL_STATE_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => Ok(r.enabled),
            Err(BridgeCallError::Invoke(e)) => Err(format!("读系统备份开关失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "读系统备份开关 {}s 无回应",
                LOCAL_STATE_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => Err(format!("读系统备份开关投递腿异常：{e}")),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("system backup toggle is not available on this platform".to_string())
    }
}

/// 枚举**用户可见的**已装应用（W-09b：「自定义应用」表单的包名来源）。
///
/// # 为什么是 `Result` 而不是「读不到就给空表」
///
/// 空表与「这台机器上真的一个第三方应用都没有」在渲染端不可区分，而后者几乎不可能、前者
/// （包可见性没声明对 / 桥没接线）很可能。给空表等于**编一个事实**，而且方向有害：用户会以为
/// 自己的应用「不支持分流」。故三种失败一律 `Err(原因)`，界面照实说「读不到」。
///
/// # 本平台没有这件事 ⇒ 同样是 `Err`，不是空表
///
/// 桌面上「已装应用」这个对象存在，但它与 Android 的 applicationId 不是一回事（桌面走进程名，
/// 见 `commands::system::system_list_processes`）。在这里返空表会让一个跨平台的调用点以为
/// 「桌面上没有应用」。
pub(crate) async fn installed_apps() -> Result<Vec<InstalledApp>, String> {
    #[cfg(target_os = "android")]
    {
        /// Kotlin 侧 `invoke.resolve(JSObject)` 的回包。
        ///
        /// 顶层只有一个 `apps` 键（A10 对拍这一层），元素的三列由 A12 对拍 —— 两层分开守是刻意的：
        /// A10 拿的是 `@Command` 方法体里**全部深度**的 `put("键")`，把逐条目的 put 写进命令体
        /// 会让它拿元素的键去比顶层结构体的字段面。
        #[derive(serde::Deserialize)]
        struct InstalledAppsResponse {
            apps: Vec<InstalledApp>,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<InstalledAppsResponse, _>(
            plugin,
            "listInstalledApps",
            (),
            INSTALLED_APPS_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => Ok(r.apps),
            Err(BridgeCallError::Invoke(e)) => Err(format!("Android 已装应用枚举失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "Android 已装应用枚举 {}s 无回应（前台无 Activity？）",
                INSTALLED_APPS_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => {
                Err(format!("Android 已装应用枚举投递腿异常：{e}"))
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        Err(
            "已装应用枚举只有 Android 腿（桌面的对应物是进程名，见 system_list_processes）"
                .to_string(),
        )
    }
}

/// 把已下载的 APK 交给系统安装器（W-21 的第二段；第一段「下载」在 `update_download`）。
///
/// # 返回三态，而不是两态
///
/// - `Ok(handed_off: true)` —— 系统安装器已拉起。**不等于装成了**：之后是用户按不按确认的事，
///   本进程既不等也等不到（Android 上装完新包会把旧进程杀掉）。
/// - `Ok(handed_off: false, reason)` —— 交不出去，且**知道为什么**（最常见：没授予「安装未知
///   应用」，Kotlin 侧已经把用户送到那一页了）。这不是错误，是一个用户可以处理的事实。
/// - `Err` —— 桥本身出了问题（没接线 / 超时 / Kotlin 抛异常）。
///
/// 把中间那一态压进 `Err` 是本条腿最容易犯的错：它会让界面只剩「安装失败」一句话可说，
/// 而用户明明按一下开关就能继续。
pub(crate) async fn hand_apk_to_system_installer(apk_path: &str) -> Result<ApkHandoff, String> {
    #[cfg(target_os = "android")]
    {
        /// 拥有式载荷的理由同 `start_core`：调用由分离 task 持有到底（见 [`call_with_budget`]）。
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct InstallApkArgs {
            apk_path: String,
        }
        /// Kotlin 侧 `invoke.resolve(JSObject)` 的回包。
        ///
        /// `#[serde(default)]` 是承重的：`org.json.JSONObject.put(key, null)` 会**删掉**这个键，
        /// 所以交付成功时回包是 `{"handedOff":true}` 而不是 `{"handedOff":true,"reason":null}`。
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct InstallApkResponse {
            handed_off: bool,
            #[serde(default)]
            reason: Option<String>,
        }
        let plugin = plugin_handle().map_err(|(msg, _)| msg)?;
        match call_with_budget::<InstallApkResponse, _>(
            plugin,
            "installApk",
            InstallApkArgs {
                apk_path: apk_path.to_owned(),
            },
            INSTALL_APK_TIMEOUT,
            None,
        )
        .await
        {
            Ok(r) => Ok(ApkHandoff {
                handed_off: r.handed_off,
                // 原样转述，不翻译、不兜底：码的取值集由 Kotlin 侧 `REASON_*` 常量收口，
                // 在这里补一个「其他」会让一个本侧从未见过的码静默变成同一句话。
                reason: r.reason,
            }),
            Err(BridgeCallError::Invoke(e)) => Err(format!("Android 交系统安装器失败：{e}")),
            Err(BridgeCallError::TimedOut) => Err(format!(
                "Android 交系统安装器 {}s 无回应（前台无 Activity？）",
                INSTALL_APK_TIMEOUT.as_secs()
            )),
            Err(BridgeCallError::TaskFailed(e)) => {
                Err(format!("Android 交系统安装器投递腿异常：{e}"))
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = apk_path;
        Err("交系统安装器只有 Android 腿（桌面走 update_install 的脚本腿）".to_string())
    }
}

/// `is_alive` 的 Android 腿。语义与射程见 `handle::CORE_STARTED` 的文档
/// （该模块 `cfg(android)`，非 Android 编译单元里不存在，故此处不做 intra-doc 链接）。
pub(super) fn core_started() -> bool {
    #[cfg(target_os = "android")]
    {
        handle::CORE_STARTED.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// 数据面桥（`runtime/stats/source.rs`）取同一个插件句柄。
///
/// **刻意不注册第二个插件**：Kotlin 侧的数据面命令与起停核命令同住 `PolarisVpnPlugin`
/// （它们共享 `VpnBridge` 的生命周期与 `BoxService` 的线程约束），再登记一个插件只会多出一份
/// 可以和它漂开的注册面。`None` = 桥还没接线（setup 未跑完）⇒ 数据面这一轮不建流、退避重试，
/// 与桌面「管理 API 连不上」那条腿同语义 —— 数据面缺席不该把起核也拖下水，故这里不返有码错误。
#[cfg(target_os = "android")]
pub(crate) fn plugin() -> Option<&'static tauri::plugin::PluginHandle<tauri::Wry>> {
    handle::PLUGIN.get()
}

/// 取插件句柄；没接线就是一个**有码**的失败（绝不静默成功）。
#[cfg(target_os = "android")]
fn plugin_handle() -> Result<&'static tauri::plugin::PluginHandle<tauri::Wry>, BridgeError> {
    handle::PLUGIN.get().ok_or_else(|| {
        (
            "Android 起停核桥未注册（setup 未跑完？）".to_string(),
            code::STARTUP_FAILED,
        )
    })
}

/// `PluginInvokeError` → 两轴。
///
/// **按码分流必须从 `InvokeRejected(r)` 里取 `r.code`**，不能读 `to_string()`：`ErrorResponse` 的
/// `Display` 把两者拼成 `[CODE] - message`（`src/plugin/mobile.rs:147`），从那串里再解析一次
/// 就是第二份约定。
#[cfg(target_os = "android")]
fn invoke_error(e: &tauri::plugin::mobile::PluginInvokeError) -> BridgeError {
    match e {
        tauri::plugin::mobile::PluginInvokeError::InvokeRejected(r) => (
            r.message
                .clone()
                .unwrap_or_else(|| "Android 起核失败（Kotlin 侧未带消息）".to_string()),
            map_rejected_code(r.code.as_deref()),
        ),
        other => (
            format!("Android 起核桥调用失败：{other}"),
            code::STARTUP_FAILED,
        ),
    }
}

#[cfg(test)]
mod tests;

/// A speedtest host is identified by its own ID. A bridge timeout is not a
/// native close acknowledgement: Kotlin retains the ID until cleanup finishes.
#[cfg(any(target_os = "android", test))]
#[derive(Debug)]
pub(crate) enum SpeedtestStartError {
    Failed(String),
    CleanupUnknown(String),
    CapacityClosed(CapacityClosed),
}

#[cfg(target_os = "android")]
#[derive(Debug)]
pub(crate) enum LoginStartError {
    Failed(String),
    CapacityClosed(CapacityClosed),
}

#[cfg(any(target_os = "android", test))]
#[derive(Debug)]
enum TransientCloseError {
    Failed(String),
    CapacityClosed(CapacityClosed),
}

#[cfg(any(target_os = "android", test))]
impl std::fmt::Display for TransientCloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) => f.write_str(message),
            Self::CapacityClosed(error) => error.fmt(f),
        }
    }
}

#[cfg(any(target_os = "android", test))]
fn speedtest_start_failure(
    capacity: Option<CapacityClosed>,
    close: Result<(), TransientCloseError>,
) -> SpeedtestStartError {
    match close {
        // A real Close failure has precedence over an admission-only diagnostic.
        Err(TransientCloseError::Failed(_)) => SpeedtestStartError::CleanupUnknown(
            "Android 测速临时核关闭结果未知；本轮已停止".to_owned(),
        ),
        Ok(()) | Err(TransientCloseError::CapacityClosed(_)) if capacity.is_some() => {
            SpeedtestStartError::CapacityClosed(capacity.unwrap())
        }
        Ok(()) => SpeedtestStartError::Failed("Android 测速临时核启动失败或超时".to_owned()),
        Err(TransientCloseError::CapacityClosed(error)) => {
            SpeedtestStartError::CleanupUnknown(format!("Android 测速临时核关闭结果未知；{error}"))
        }
    }
}

#[cfg(target_os = "android")]
fn transient_capacity(error: &BridgeCallError) -> Option<CapacityClosed> {
    match error {
        BridgeCallError::Invoke(tauri::plugin::mobile::PluginInvokeError::InvokeRejected(
            rejection,
        )) => CapacityClosed::from_code(rejection.code.as_deref()),
        _ => None,
    }
}

#[cfg(target_os = "android")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TransientSpeedtestState {
    Starting,
    Running,
    Closing,
    CleanupUnknown,
    Closed,
    Unknown,
}

#[cfg(target_os = "android")]
pub(crate) async fn start_transient_speedtest(
    instance_id: &str,
    config_content: &str,
) -> Result<(), SpeedtestStartError> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StartArgs {
        instance_id: String,
        config_content: String,
    }
    let plugin = plugin_handle()
        .map_err(|_| SpeedtestStartError::Failed("Android 测速桥不可用".to_owned()))?;
    let result = call_with_budget::<(), _>(
        plugin,
        "startTransientSpeedtest",
        StartArgs {
            instance_id: instance_id.to_owned(),
            config_content: config_content.to_owned(),
        },
        START_TIMEOUT,
        None,
    )
    .await;
    if result.is_ok() {
        return Ok(());
    }
    // Even if start has not reached Kotlin yet, close records a permanent
    // tombstone for this ID and prevents a late callback from starting it.
    let capacity = result.as_ref().err().and_then(transient_capacity);
    Err(speedtest_start_failure(
        capacity,
        close_transient_speedtest_admitted(instance_id).await,
    ))
}

#[cfg(target_os = "android")]
pub(crate) async fn close_transient_speedtest(instance_id: &str) -> Result<(), String> {
    close_transient_speedtest_admitted(instance_id)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
async fn close_transient_speedtest_admitted(instance_id: &str) -> Result<(), TransientCloseError> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CloseArgs {
        instance_id: String,
    }
    let plugin = plugin_handle()
        .map_err(|_| TransientCloseError::Failed("Android 测速桥不可用".to_owned()))?;
    call_with_budget::<(), _>(
        plugin,
        "closeTransientSpeedtest",
        CloseArgs {
            instance_id: instance_id.to_owned(),
        },
        STOP_TIMEOUT,
        None,
    )
    .await
    .map(|_| ())
    .map_err(|error| match transient_capacity(&error) {
        Some(capacity) => TransientCloseError::CapacityClosed(capacity),
        None => TransientCloseError::Failed("Android 测速临时核关闭未确认".to_owned()),
    })
}

#[cfg(target_os = "android")]
pub(crate) async fn transient_speedtest_status(
    instance_id: &str,
) -> Result<TransientSpeedtestState, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StatusArgs {
        instance_id: String,
    }
    #[derive(serde::Deserialize)]
    struct StatusResponse {
        state: TransientSpeedtestState,
    }
    let plugin = plugin_handle().map_err(|_| "Android 测速桥不可用".to_owned())?;
    call_with_budget::<StatusResponse, _>(
        plugin,
        "transientSpeedtestStatus",
        StatusArgs {
            instance_id: instance_id.to_owned(),
        },
        LOCAL_STATE_TIMEOUT,
        None,
    )
    .await
    .map(|status| status.state)
    .map_err(|_| "Android 测速临时核状态不可用".to_owned())
}

/// Independent, non-VPN Tailscale login instances. The native host owns only the supplied attempt.
#[cfg(target_os = "android")]
pub(crate) async fn start_transient_login(
    instance_id: &str,
    config_content: &str,
) -> Result<(), LoginStartError> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TransientStartArgs {
        instance_id: String,
        config_content: String,
    }
    let plugin = plugin_handle().map_err(|(message, _)| LoginStartError::Failed(message))?;
    let result = call_with_budget::<(), _>(
        plugin,
        "startTransientLogin",
        TransientStartArgs {
            instance_id: instance_id.to_owned(),
            config_content: config_content.to_owned(),
        },
        START_TIMEOUT,
        None,
    )
    .await
    .map_err(|error| {
        if let Some(capacity) = transient_capacity(&error) {
            return LoginStartError::CapacityClosed(capacity);
        }
        LoginStartError::Failed(match error {
            BridgeCallError::Invoke(error) => {
                format!("Android 独立登录桥拒绝: {}", invoke_error(&error).0)
            }
            BridgeCallError::TimedOut => "Android 独立登录启动桥超时".to_owned(),
            BridgeCallError::TaskFailed(_) => "Android 独立登录启动桥投递失败".to_owned(),
        })
    });
    if result.is_err() {
        // Also closes a start callback that arrives after the bridge budget expired.
        let _ = close_transient_login(instance_id).await;
    }
    result.map(|_| ())
}

#[cfg(target_os = "android")]
pub(crate) async fn close_transient_login(instance_id: &str) -> Result<(), String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TransientCloseArgs {
        instance_id: String,
    }
    let plugin = plugin_handle().map_err(|(message, _)| message)?;
    call_with_budget::<(), _>(
        plugin,
        "closeTransientLogin",
        TransientCloseArgs {
            instance_id: instance_id.to_owned(),
        },
        STOP_TIMEOUT,
        None,
    )
    .await
    .map(|_| ())
    .map_err(|error| {
        transient_capacity(&error).map_or_else(
            || "Android 独立登录关闭未确认".to_owned(),
            |capacity| capacity.to_string(),
        )
    })
}

#[cfg(target_os = "android")]
pub(crate) async fn transient_login_running(instance_id: &str) -> Result<bool, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TransientStatusArgs {
        instance_id: String,
    }
    #[derive(serde::Deserialize)]
    struct TransientStatusResponse {
        running: bool,
    }
    let plugin = plugin_handle().map_err(|(message, _)| message)?;
    call_with_budget::<TransientStatusResponse, _>(
        plugin,
        "transientLoginStatus",
        TransientStatusArgs {
            instance_id: instance_id.to_owned(),
        },
        LOCAL_STATE_TIMEOUT,
        None,
    )
    .await
    .map(|status| status.running)
    .map_err(|_| "Android 独立登录状态不可用".to_owned())
}

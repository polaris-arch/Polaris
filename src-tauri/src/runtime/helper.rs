//! helper 运行时：`polaris-helper-client` + 平台 helper crate 装配。
//!
//! Polaris 锚点：`main/services/HelperManager.ts`（macOS LaunchDaemon / Windows SCM / Linux systemd
//! 提权 helper 的客户端：install / uninstall / getStatus / start / stop 核经 helper 提权）。
//!
//! 装配（C6-5 收口）：
//! - [`polaris_helper_client::HelperManager`]：helper 生命周期（install/uninstall + token + 状态探测）。
//! - [`polaris_helper_client::HelperClient`]：helper 通信（生产 `UnixConnector`（mac/linux）/
//!   `PipeConnector`（win），经 [`Connector`] trait）。
//!
//! **真机门**：`install`/`uninstall` 触发一次提权（osascript / UAC / pkexec）——本机（Linux 桌面）
//! 无 bundled helper 二进制 → `resolve_helper_binary` 失败 → 早返「二进制缺失」，**绝不触发提权**（安全）。
//! `start_core`/`stop_core` 经就绪 daemon 的 socket/pipe 起停 root/SYSTEM 受管核——app→helper→核端到端
//! 提权起核是本计划头号真机门（部署首验）。

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime::proxy::DirectCoreSlot;

#[cfg(any(test, target_os = "macos"))]
use polaris_helper_client::ClientError;
#[cfg(test)]
use polaris_helper_client::ConnectionStream;
use polaris_helper_client::Connector;
#[cfg(windows)]
use polaris_helper_client::PipeConnector;
#[cfg(unix)]
use polaris_helper_client::UnixConnector;
use polaris_helper_client::{
    read_token, EscalationOutcome, HelperClient, HelperManager, HelperStatus, InstallParams,
    InstallPaths, MaintenanceUtility, ManagerError, StdExecutor, StdSysOps, SysOps,
    READY_POLL_ATTEMPTS, READY_POLL_DELAY,
};
use polaris_helper_proto::{
    FlushDns, HelperBirthTarget, LinuxBirthStart, LinuxBirthStatus, LinuxBirthStop, LinuxDns,
    LinuxDnsSetParams, NativeBirthStart, NativeBirthStatus, NativeBirthStop, Platform, Request,
    Response, ResponseKind, RouteParams, Start, StartNotAdmitted, StartParams,
    Status as CoreStatus, Stop,
};
use polaris_system_integration::dns_flush::HelperFlushResult;
use polaris_system_integration::linux_resolved::LinuxResolvedOps;
use std::time::Duration;

/// app 侧 token 文件名（= 上游 `getUserDataPath()/helper-client.token`，`HelperManager.ts:478`）。
const HELPER_TOKEN_FILE: &str = "helper-client.token";

/// helper 起核通信超时：daemon 在 spawn 子核后立即回 `OK started <pid>`（不等就绪，就绪门在 proxy 侧），
/// 故留 15s 余量覆盖偶发慢盘/大 config 校验足矣。
const HELPER_START_TIMEOUT: Duration = Duration::from_secs(15);

/// helper 停核整次调用的总预算。Linux 的物理 Child 尚未 reap 时会在同一个上层
/// Stop permit 内轮询；通信错误仍最多补发一次，但所有尝试共用这 1.5 秒截止时间。
///
/// stop 携带受管 pid，daemon 侧按 exact child 身份校验，因此同一 pid 的重复请求是幂等的：Linux
/// 在 native reap 前回 `pending`，之后才回 `stopped/notrunning`；若 helper 已换成新核，则得到
/// `stop-mismatch` 且不杀。Windows 真机出现过一次 1.5s 默认预算刚过即丢回执，因此保留一次
/// 通信恢复机会，同时不把所有 helper 命令一并放宽。
const HELPER_STOP_TIMEOUT: Duration = Duration::from_millis(1_500);
const HELPER_STOP_MAX_RETRIES: u32 = 1;
const HELPER_STOP_RETRY_DELAY: Duration = Duration::from_millis(50);

/// A typed helper Start result. `NotAdmitted` is a proven no-spawn outcome
/// that still carries custody of the earlier Linux helper birth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelperStartResult {
    Started(u32),
    NotAdmitted(StartNotAdmitted),
    BirthStarted(HelperBirthTarget),
    BirthAlready(HelperBirthTarget),
    /// Keep exact custody so startup can stop a new birth with the wrong/missing content receipt.
    BirthRejected(HelperBirthTarget),
    BirthNotAdmitted {
        target: Option<HelperBirthTarget>,
        pending: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelperStopTarget {
    Legacy(u32),
    Birth(HelperBirthTarget),
}

impl HelperStopTarget {
    pub(crate) fn pid(self) -> u32 {
        match self {
            Self::Legacy(pid) => pid,
            Self::Birth(target) => target.pid.get(),
        }
    }
}

/// helper flush-dns 通信超时（对齐 上游 `HelperManager.flushDns` 的 5000ms；flush 为瞬时操作）。
const HELPER_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// helper route-add/route-del 通信超时（route 手术为瞬时操作，取 5s 同 flush 量级）。
const HELPER_ROUTE_TIMEOUT: Duration = Duration::from_secs(5);

/// SystemConfiguration 单事务通常为毫秒级；5s 只兜 configd 瞬态繁忙。超时属于结果未知，
/// 调用方不会再回落 networksetup 重复写入。
#[cfg(any(test, target_os = "macos"))]
const HELPER_MAC_PROXY_TIMEOUT: Duration = Duration::from_secs(5);

/// 一次 resolved 接管含 5 次写入、3 次读回与失败回滚；helper 内每个子命令各有 5s 上限。
const HELPER_LINUX_RESOLVED_TIMEOUT: Duration = Duration::from_secs(45);

/// helper 状态快照（上游 `HelperStatus` 镜像，序列化形与前端 `contracts/types/runtime.ts` 一致）。
///
/// 字段与前端 `HelperStatus` 逐字对齐（`SettingsHelper.tsx` 的 `deriveState` 消费 supported / upgradeable /
/// backgroundDisabled / needsRepair / ready / installed / version）。`loaded` / `backgroundDisabled` /
/// `pathMismatch` / `installedSingboxPath` 是 macOS 真机门（BTM/plist 烧录路径对比），本机不可判 → 惰值。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperStatusSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_core: Option<SelectedCoreSnapshot>,
    /// 当前平台是否支持提权 helper（mac/win/linux 三平台均有实现）。
    pub supported: bool,
    /// helper 二进制 + 描述符（plist/unit/SCM）是否在位。
    pub installed: bool,
    /// socket ping 成功且 proto ≥ 最低可用（可零提权驱动 TUN）。
    pub ready: bool,
    /// 协议需升级，或当前选定 CoreID 尚未安装。构建身份差异只供诊断。
    pub upgradeable: bool,
    /// 协议版本（ping/version 返回），未就绪为缺省。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// 当前 app 期望的 shared protocol 版本；前端不得再硬编码版本数字。
    pub expected_protocol_version: u32,
    /// 已安装 helper 自报的构建身份；缺省或差异只供诊断。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper_build_id: Option<String>,
    /// 当前 app 随包 helper 的期望构建身份，供诊断与 app/helper 同包对账。
    pub expected_build_id: String,
    /// daemon 是否被 launchd/systemd/SCM 加载；本层不主动探（避免每次 status spawn 进程）→ 恒 None（真机门）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded: Option<bool>,
    /// 已安装但无法就绪（token 丢失 / proto 过旧）→ 建议重装修复。
    pub needs_repair: bool,
    /// macOS 后台项（BTM）被禁——真机门（TCC 保护，Linux/GUI 读不到）→ 恒 false。
    pub background_disabled: bool,
    /// macOS 打包版 plist 烧录 sing-box 路径 ≠ 当前 app——真机门 → 恒 false。
    pub path_mismatch: bool,
    /// plist 烧录的 sing-box 路径（诊断展示）——真机门 → 恒 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_singbox_path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedCoreSnapshot {
    pub id: String,
    pub version: String,
    pub provider_fingerprint: String,
    pub state: SelectedCoreState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectedCoreState {
    Present,
    Absent,
    Invalid,
    Unavailable,
}

/// Helper 动作的稳定失败码。前端按码本地化，`diagnostic` 不得直接展示。
#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HelperActionErrorCode {
    Cancelled,
    AuthorizationIncomplete,
    PartialCleanup,
    ProxyRunning,
    Unsupported,
    MissingAsset,
    BundleMismatch,
    NotReady,
    InvalidInstallation,
    TokenWriteFailed,
    ScriptWriteFailed,
    AuthorizationLaunchFailed,
    InstallerFailed,
    UpgradeDeferred,
    Failed,
}

/// Explicit installation intent. Omitted IPC mode remains the non-disruptive ordinary path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperInstallMode {
    #[default]
    Ordinary,
    StopAndUpgrade,
}

impl HelperInstallMode {
    pub const fn supported_on(self, platform: Platform) -> bool {
        matches!(self, Self::Ordinary) || matches!(platform, Platform::Mac)
    }

    fn manager_mode(self) -> polaris_helper_client::manager::InstallMode {
        use polaris_helper_client::manager::InstallMode;
        match self {
            Self::Ordinary => InstallMode::Ordinary,
            Self::StopAndUpgrade => InstallMode::StopAndUpgrade,
        }
    }
}

/// pkexec 127 表示授权或执行未完成，不能单凭退出码断言缺少认证代理。
/// 安装与卸载共用分类；126 的用户取消语义由提权层独立保留。
const fn escalation_failure_code(platform: Platform, code: i32) -> HelperActionErrorCode {
    if matches!(platform, Platform::Linux) && code == 127 {
        HelperActionErrorCode::AuthorizationIncomplete
    } else {
        HelperActionErrorCode::Failed
    }
}

/// Preserve the typed preparation stage without exposing paths, tokens, or OS error text.
fn manager_install_failure_code(error: &ManagerError) -> HelperActionErrorCode {
    match error {
        ManagerError::InvalidInstallation(_) => HelperActionErrorCode::InvalidInstallation,
        ManagerError::HelperBinaryMissing(_) => HelperActionErrorCode::MissingAsset,
        ManagerError::TokenWrite(_) => HelperActionErrorCode::TokenWriteFailed,
        ManagerError::ScriptWrite(_) => HelperActionErrorCode::ScriptWriteFailed,
        ManagerError::Escalation(_) => HelperActionErrorCode::AuthorizationLaunchFailed,
        ManagerError::UnsupportedPlatform(_) => HelperActionErrorCode::Unsupported,
    }
}

/// A nonzero authorization/install command exit does not identify an inner failure cause.
fn install_command_failure_code(
    platform: Platform,
    code: i32,
    stderr: &str,
) -> HelperActionErrorCode {
    if matches!(platform, Platform::Win) && code == 75 {
        return HelperActionErrorCode::UpgradeDeferred;
    }
    // osascript wraps the script's exit 75 as exit 1. Recognize only the exact
    // installer marker (optionally its AppleScript exit suffix), never prose,
    // a bare exit code, or a guessed old-helper version. This is presentation
    // only: it grants no admission and does not retry or stop a service.
    if stderr.lines().any(|line| {
        matches!(
            line.trim(),
            "HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY"
                | "HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY (75)"
        )
    }) {
        HelperActionErrorCode::UpgradeDeferred
    } else if matches!(platform, Platform::Linux) && code == 127 {
        HelperActionErrorCode::AuthorizationIncomplete
    } else {
        HelperActionErrorCode::InstallerFailed
    }
}

/// These are installer-owned exit stages, never arbitrary child stderr or paths.
fn install_command_diagnostic(platform: Platform, code: i32) -> &'static str {
    if matches!(platform, Platform::Win) {
        match code {
            75 => "helper installer deferred at Windows upgrade admission",
            81 => "helper installer failed at Windows sealed staging",
            82 => "helper installer failed at Windows core installation",
            83 => "helper installer failed at Windows user registration",
            84 => "helper installer failed at Windows installed helper policy",
            85 => "helper installer failed at Windows service stop or removal",
            86 => "helper installer failed at Windows service replacement or start",
            87 => "helper installer failed and Windows rollback is incomplete",
            88 => "helper installer failed at Windows transaction cleanup",
            _ => "helper installer exited unsuccessfully",
        }
    } else {
        "helper installer exited unsuccessfully"
    }
}

/// The native Windows uninstaller reports partial completion explicitly.
const fn uninstall_failure_code(platform: Platform, code: i32) -> HelperActionErrorCode {
    if matches!(platform, Platform::Win) && code == 3 {
        HelperActionErrorCode::PartialCleanup
    } else {
        escalation_failure_code(platform, code)
    }
}

/// Safe presentation-only observation. A child can imitate these stderr markers;
/// this never proves the emitter, polkit authorization, or privileged cleanup.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelperExecutionDiagnostic {
    pub schema: &'static str,
    pub exit_code: i32,
    pub marker: HelperExecutionMarker,
    pub scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installer_failure: Option<HelperInstallerFailure>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelperInstallerFailure {
    pub schema: &'static str,
    pub stage: &'static str,
    pub recovery: &'static str,
}

fn helper_installer_failure(stderr: &str) -> Option<HelperInstallerFailure> {
    if stderr.len() > 16 * 1024 {
        return None;
    }
    let mut result: Option<HelperInstallerFailure> = None;
    for line in stderr.lines() {
        let Some(value) = line.strip_prefix("POLARIS_HELPER_FAILURE_V1:") else {
            continue;
        };
        let (stage, recovery) = value.split_once(':')?;
        let stage = match stage {
            "preparation" => "preparation",
            "directories" => "directories",
            "staging" => "staging",
            "lease" => "lease",
            "journal" => "journal",
            "core-install" => "core-install",
            "policy" => "policy",
            "service-snapshot" => "service-snapshot",
            "manager-identity" => "manager-identity",
            "manager-property" => "manager-property",
            "pid1-cgroup" => "pid1-cgroup",
            "cgroup-mount" => "cgroup-mount",
            "unit-config" => "unit-config",
            "service-stop" => "service-stop",
            "service-closure" => "service-closure",
            "registration" => "registration",
            "service-install" => "service-install",
            "service-start" => "service-start",
            "takeover" => "takeover",
            "uninstall-remove" => "uninstall-remove",
            "cleanup" => "cleanup",
            "rollback" => "rollback",
            "unknown" => "unknown",
            _ => return None,
        };
        let recovery = match recovery {
            "not-started" => "not-started",
            "cleanup-returned" => "cleanup-returned",
            "rollback-returned" => "rollback-returned",
            "recovery-unconfirmed" => "recovery-unconfirmed",
            _ => return None,
        };
        if let Some(prior) = &result {
            if prior.stage != stage || prior.recovery != "recovery-unconfirmed" {
                return None;
            }
        }
        result = Some(HelperInstallerFailure {
            schema: "polaris-helper-installer-failure-v1",
            stage,
            recovery,
        });
    }
    result
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HelperExecutionMarker {
    Unknown,
    NotSetuidRoot,
    NoAuthenticationAgent,
    NotAuthorized,
    TextAgentCreationFailed,
    TextAgentRegistrationFailed,
    AuthorizationCheckFailed,
}

fn helper_execution_diagnostic(
    platform: Platform,
    code: i32,
    stderr: &str,
) -> Option<HelperExecutionDiagnostic> {
    if platform != Platform::Linux {
        return None;
    }
    // Reviewed polkit 0.105 + Ubuntu 0.105-33ubuntu0.2 patch series.
    // Bound inspection, never copy suffixes containing paths/users/PAM text.
    // Conflicting markers, translations and unknown output remain unknown.
    let mut marker = HelperExecutionMarker::Unknown;
    if code == 127 && stderr.len() <= 16 * 1024 {
        for line in stderr.lines() {
            let current = match line.trim() {
                "pkexec must be setuid root" => HelperExecutionMarker::NotSetuidRoot,
                "Error executing command as another user: No authentication agent found." => {
                    HelperExecutionMarker::NoAuthenticationAgent
                }
                "Error executing command as another user: Not authorized" => {
                    HelperExecutionMarker::NotAuthorized
                }
                text if text.starts_with("Error creating textual authentication agent: ") => {
                    HelperExecutionMarker::TextAgentCreationFailed
                }
                text if text.starts_with("Error registering local authentication agent: ") => {
                    HelperExecutionMarker::TextAgentRegistrationFailed
                }
                text if text.starts_with("Error checking for authorization ") => {
                    HelperExecutionMarker::AuthorizationCheckFailed
                }
                _ => continue,
            };
            if marker != HelperExecutionMarker::Unknown && marker != current {
                marker = HelperExecutionMarker::Unknown;
                break;
            }
            marker = current;
        }
    }
    Some(HelperExecutionDiagnostic {
        schema: "polaris-helper-execution-diagnostic-v1",
        exit_code: code,
        marker,
        scope: "child-stderr-marker-not-verified-emitter",
        installer_failure: if code != 0 {
            helper_installer_failure(stderr)
        } else {
            None
        },
    })
}

/// install/uninstall 结果信封（前端 `helperApi.install/uninstall` 期望
/// `{ success, errorCode?, diagnostic?, status }`）。
///
/// 提权三态（[`EscalationOutcome`]）在此表达：成功 = `success:true`；用户取消 / 脚本失败 = `success:false`
/// + 稳定码。**外层仍是 `ApiResponse::ok`**（IPC 层不失败——用户取消提权是正常流程，非命令错误）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperActionResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_diagnostic: Option<HelperExecutionDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<HelperActionErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    pub status: HelperStatusSnapshot,
}

impl HelperActionResult {
    /// 仅供 Rust 日志/编排层说明失败；用户 UI 必须使用 [`Self::error_code`] 本地化。
    #[must_use]
    pub fn reason_for_log(&self) -> &str {
        self.diagnostic.as_deref().unwrap_or("helper action failed")
    }
}

/// Completion requires a compatible helper and a verified selected CoreID.
/// A newer compatible helper may be retained; BuildID is diagnostic only.
fn helper_install_reached_expected_build(status: &HelperStatusSnapshot) -> bool {
    status.ready
        && !status.upgradeable
        && status
            .selected_core
            .as_ref()
            .is_some_and(|core| core.state == SelectedCoreState::Present)
}

/// 从 client 侧 [`HelperStatus`] 投影到前端快照（补 supported + macOS 真机门惰值）。
fn snapshot_from(status: &HelperStatus, supported: bool) -> HelperStatusSnapshot {
    HelperStatusSnapshot {
        selected_core: None,
        supported,
        installed: status.installed,
        ready: status.ready,
        upgradeable: status.upgradeable,
        version: status.version,
        expected_protocol_version: polaris_helper_proto::proto_version::CURRENT,
        helper_build_id: status.build_id.clone(),
        expected_build_id: polaris_helper_proto::build_identity::current().to_owned(),
        loaded: None,
        needs_repair: status.needs_repair,
        background_disabled: false,
        path_mismatch: false,
        installed_singbox_path: None,
    }
}

/// 平台 → 是否有提权 helper 实现（纯映射，全变体可单测）。mac/win/linux 三平台均有 helper
/// （对齐 `runtime/proxy::should_start_via_helper` 的平台门：三平台 TUN 一律经 helper 起核）；`Other`
/// （freebsd/…）无对应实现。
///
/// 抽为自由 `const fn` 是为让**全 `Platform` 变体**在单一平台 gate 上可断言——给 mac/win 值以变异
/// 牙齿（`supported()` 读 `Platform::current()`，本机 gate 只走 Linux 一路，测不到 mac/win 逃逸面）。
///
/// **穷举 `match` 而非 `matches!`**（2026-09-04 K10）：这是整个 helper 子系统的入口闸 —— 它答
/// false 的平台，`HelperManager` / 安装脚本 / 提权 / `LinuxStart` 帧全部不可达。`matches!` 形态下
/// 新增一个平台变体会**静默**得到 false（碰巧对），下一个变体则可能碰巧错；穷举 match 让编译器
/// 在这里强制问一次「这个平台有没有 helper」。Android = false：核跑在应用进程内的 libbox 里，
/// 没有 daemon、没有提权通道、也没有需要提权的动作（tun fd 由 `VpnService` 授予）。
const fn platform_supported(platform: Platform) -> bool {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux => true,
        // iOS = false，与 Android 同答不同因（这里是整个 helper 子系统的入口闸，值得写清）：
        // Android 的依据是「核跑在应用进程内的 libbox 里，没有 daemon 也没有需要提权的动作」；
        // iOS 的依据更靠前 —— **平台不允许存在常驻 root daemon**（无 launchd 可注册面、无提权
        // 通道），而 tun fd 由系统按 packet-tunnel-provider entitlement 授予 NE 扩展，同样没有
        // 需要提权的动作。⇒ false 之后，`HelperManager` / 安装脚本 / 提权 / `LinuxStart` 帧
        // 在 iOS 上全部不可达（helper-client 那 7 处 `Ios` 臂的不可达性就源自这一行）。
        Platform::Android | Platform::Ios | Platform::Other => false,
    }
}

/// 卸载 helper 之前该不该先停核（纯判定，可穷举单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallPreflight {
    /// 代理正**经 helper** 运行 → 先用「仍在的 helper」零提权停核，再卸载。
    StopCoreFirst,
    /// 代理未运行，或核是 app 自己直起的（不经 helper）→ 直接卸载。
    ProceedDirectly,
}

/// 纯判定：`proxy_running && started_via_helper` 才需要前置停核。
///
/// # 为什么两个条件都要，缺一不可
///
/// - `proxy_running`：没核在跑就没什么可停，多发一次 stop 只是噪音。
/// - `started_via_helper`：核若是 app 自己直起的（非 TUN 路径），它不归 daemon 管，
///   卸载 helper 不会让它变成孤儿；此时停核等于无故断用户的网。
///
/// 契约锚点：`~/docs/polaris/design/polaris-上游-capability-contract.md:93`「卸载前零提权停核」
/// + 上游 `helper-handlers.ts:54`（`getStatus().running && isStartedViaHelper()`）。
///
/// # 不停会怎样（这条腿存在的理由）
///
/// TUN 跑着时直接卸 helper：daemon 连同 socket 一起消失，而它拉起的 **root/SYSTEM 受管核**
/// 还在跑并占着 TUN。此后 app 再想停它，用户态 `kill` 收 EPERM 杀不动，只能落 forceKill 裸弹
/// 一次没有任何引导的 osascript —— 用户看到的是「卸了个东西，然后全网断了，还冒出个要密码的框」。
#[must_use]
pub const fn decide_uninstall_preflight(
    proxy_running: bool,
    started_via_helper: bool,
) -> UninstallPreflight {
    if proxy_running && started_via_helper {
        UninstallPreflight::StopCoreFirst
    } else {
        UninstallPreflight::ProceedDirectly
    }
}

/// 卸载前置停核编排（`stop` 注入 → 可单测，不碰真代理）。返回三态 [`PreflightStopResult`]。
///
/// # 停不掉时**继续卸载**（与 `update_install` 的停代理腿刻意相反）
///
/// | 腿 | 停失败时 | 为什么 |
/// |---|---|---|
/// | `update_install`（`commands/updater.rs`） | **中止安装** | 带着跑着的核替换应用本体会留半死不活态；更新可以稍后重试，代价只是晚点更新 |
/// | 本腿（卸载 helper） | **继续卸载** | 卸载是用户明确要求的终态动作。中止的话用户卡在「helper 卸不掉」且核照样跑着 —— 既没卸成、也没停成，比只丢一次停核更糟 |
///
/// 逐字对齐 上游 `helper-handlers.ts:55` 的 `await proxyManager.stop().catch(() => {})`。
/// 失败记 `warn`（不静默）：卸载完成后可能残留一个无人管的 root 核，日志得能对上账。
///
/// # 「停不掉之后怎么办」由**调用方**定，本函数只如实报告
///
/// 上表那条「继续卸载」是 **helper 单卸载**这条腿的决策，而不是本函数的。完全卸载
/// （`commands::app_uninstall_all`）后面还要删配置、删应用本体，停不掉核就必须整体中止
/// （理由见 [`crate::runtime::uninstall::stop_core_outcome`]）。故返回值从早先的 `bool`
/// 换成 [`PreflightStopResult`]：**两个调用方各自决定政策，判定与停核动作仍只有这一份**。
pub async fn uninstall_preflight_stop<F, Fut>(
    proxy_running: bool,
    started_via_helper: bool,
    stop: F,
) -> PreflightStopResult
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    if decide_uninstall_preflight(proxy_running, started_via_helper)
        == UninstallPreflight::ProceedDirectly
    {
        return PreflightStopResult::NotNeeded;
    }
    if let Err(e) = stop().await {
        log::warn!(
            "卸载 helper 前停核失败（{e}）：helper 单卸载仍继续 —— 卸载后可能残留 root 受管核，需手动确认"
        );
        return PreflightStopResult::StopFailed(e);
    }
    PreflightStopResult::Stopped
}

/// 前置停核的三态结果（政策留给调用方，见 [`uninstall_preflight_stop`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightStopResult {
    /// 判定为无需停核（没跑，或核不经 helper 起）。
    NotNeeded,
    /// 真发起了停核且成功。
    Stopped,
    /// 真发起了停核但失败，带原因。
    StopFailed(String),
}

impl PreflightStopResult {
    /// 是否真发起过停核（= 旧 `bool` 返回值的语义，给既有真值表用）。
    #[cfg(test)]
    #[must_use]
    pub const fn attempted(&self) -> bool {
        !matches!(self, Self::NotNeeded)
    }

    /// 失败原因（成功/无需停核为 `None`）——完全卸载腿据此把停核映射成硬失败。
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::StopFailed(e) => Some(e),
            _ => None,
        }
    }
}

/// 「让 daemon 停掉它自己的受管 child」这一个能力的窄 trait。
///
/// **为什么要这层间接**：[`HelperRuntime`] 直连 [`InstallPaths`] 给的**系统路径** socket
/// （`/var/run/...`），单测既起不了真 daemon、也不许往系统路径写 —— 没有替身就等于起核收口腿
/// 完全无法被断言（「stop 有没有被调」只能靠读代码推理）。抽成窄 trait 后可注入可观测替身，
/// 把它变成有牙的门。
pub(crate) trait HelperStopOps: Send + Sync {
    /// 请 daemon 终止其受管 sing-box child。
    ///
    /// Linux requires the exact birth target; macOS/Windows retain their
    /// legacy PID target. An unconfirmed birth cannot issue Stop.
    fn stop_managed_core(&self, target: HelperStopTarget) -> Result<(), String>;
}

/// helper 对其受管 sing-box 的权威运行视图。
///
/// `created` / `image` 是 Windows helper 从**它持有的进程句柄**读到的两个事实（D2/D3），
/// 其它平台恒 `None`：app 是 Medium IL，对 SYSTEM child 的 `OpenProcess` 会被拒，这两件事只有
/// 权限边界另一侧的 helper 答得了。旧 helper 不回传 ⇒ 也是 `None` ⇒ 消费方按「不可观测」处理，
/// 绝不据此报崩溃或宣称自证通过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedCoreStatus {
    Running {
        pid: u32,
        /// 受管核的进程创建时间令牌（100ns tick）；跨 tick 变化 = 该 pid 被复用。
        created: Option<u64>,
        /// 受管核实跑二进制全路径。
        image: Option<String>,
    },
    Stopped,
    BirthRunning {
        target: HelperBirthTarget,
    },
    NativeBirthRunning {
        target: HelperBirthTarget,
        created: Option<u64>,
        image: Option<String>,
    },
    BirthUnidentified,
    BirthStopping {
        target: HelperBirthTarget,
    },
    BirthUnknown {
        target: HelperBirthTarget,
    },
    BirthEmpty,
}

impl HelperStopOps for HelperRuntime {
    fn stop_managed_core(&self, target: HelperStopTarget) -> Result<(), String> {
        self.stop_core(target)
    }
}

/// 完全卸载腿的注入面（窄 trait 的理由同 [`HelperStopOps`]：单测不许起 daemon、更不许弹提权框）。
///
/// 这里只做**信封转换**（`HelperActionResult` → `Result`）与**路径取值**，一条判定都不加 ——
/// 判定全在 [`crate::runtime::uninstall`] 的纯函数里。
impl crate::runtime::uninstall::HelperUninstallOps for HelperRuntime {
    fn supported(&self) -> bool {
        HelperRuntime::supported(self)
    }

    fn installed(&self) -> bool {
        self.status().installed
    }

    fn uninstall(&self) -> Result<(), String> {
        let r = HelperRuntime::uninstall(self);
        if r.success {
            Ok(())
        } else {
            // 提权三态里「用户取消」也落这里 —— 它对完全卸载就是一次失败（用户没授权 ⇒ 什么都没删），
            // 必须让 fail-fast 拦下后面的删除，而不是当成「跳过」继续删配置和应用本体。
            Err(r
                .diagnostic
                .unwrap_or_else(|| "helper uninstall did not complete".to_owned()))
        }
    }

    fn protected_core_dir(&self) -> String {
        // P4 起三平台同构：win 的 `core_dir`（`C:\ProgramData\Polaris\core`）真有受管核，
        // 且被卸载脚本的 `Remove-Item -Recurse C:\ProgramData\Polaris` 一并清掉 —— 原先那条
        // 「Windows 内核走应用侧，无受保护内核目录」的分支现在是谎话，删掉而不是改措辞。
        InstallPaths::for_platform(self.platform)
            .core_dir
            .display()
            .to_string()
    }
}

/// [`SysOps`] 工厂（每次 [`HelperRuntime::manager`] 现造一个：`HelperManager` 要 `Box<dyn SysOps>`
/// 的所有权，而 `SysOps: Send` 不要求 `Sync`，存不成共享实例）。
type SysOpsFactory = Arc<dyn Fn() -> Box<dyn SysOps> + Send + Sync>;

/// 测试态连接器：任何请求都在进程内稳定失败，绝不接触宿主已安装的特权 daemon。
#[cfg(test)]
struct NeverConnect;

#[cfg(test)]
impl Connector for NeverConnect {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        Err(ClientError::Connect(
            "测试替身：不连接真实 helper".to_owned(),
        ))
    }
}

/// helper 运行时（`State`-managed，单实例）。
#[derive(Default)]
struct CoreMutationFences {
    ever_mutated: bool,
    slots: Vec<Weak<Mutex<DirectCoreSlot>>>,
}

#[cfg(test)]
struct SharedTestConnector(Arc<dyn Connector + Sync>);

#[cfg(test)]
impl Connector for SharedTestConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        self.0.connect()
    }
}

pub struct HelperRuntime {
    dir: PathBuf,
    platform: Platform,
    /// 装/载探测的系统面。生产恒 [`StdSysOps`]；**单测必须注入替身**，理由见
    /// `HelperRuntime::never_installed_for_tests`。
    sys_ops: SysOpsFactory,
    /// **D2/D3(4) 的「start 拿初值」**：最近一次 `start` 响应回传的受管核身份 `(pid, created)`。
    /// 旧 helper / 非 Windows 不回传 ⇒ `None`。读写见 [`Self::managed_start_identity`]。
    start_identity: Mutex<Option<(u32, u64)>>,
    /// All runtimes sharing this helper retain their own direct Child fence.
    core_mutation_fences: Mutex<CoreMutationFences>,
    /// 与 `sys_ops` 同一隔离边界：测试 fixture 不仅要把“已安装”探测钉成 false，直接调用
    /// `start_core` 的测试也必须被结构性禁止连接真实 socket。
    #[cfg(test)]
    never_connect: bool,
    /// **测试专用**：把 [`Self::status`] 钉成给定快照，见 `Self::with_forced_status_for_tests`。
    #[cfg(test)]
    status_override: Option<HelperStatusSnapshot>,
    /// Explicit in-memory daemon transport; never resolves a host socket.
    #[cfg(test)]
    test_connector: Option<Arc<dyn Connector + Sync>>,
}

impl HelperRuntime {
    pub fn new(dir: PathBuf) -> Self {
        let platform = Platform::current();
        Self {
            dir,
            platform,
            sys_ops: Arc::new(|| Box::new(StdSysOps)),
            start_identity: Mutex::new(None),
            core_mutation_fences: Mutex::new(CoreMutationFences::default()),
            #[cfg(test)]
            never_connect: false,
            #[cfg(test)]
            status_override: None,
            #[cfg(test)]
            test_connector: None,
        }
    }

    pub(crate) fn bind_core_mutation_fence(
        &self,
        slot: Weak<Mutex<DirectCoreSlot>>,
    ) -> Result<(), String> {
        let mut registry = self
            .core_mutation_fences
            .lock()
            .map_err(|_| "helper core fence registry poisoned".to_string())?;
        registry.slots.retain(|slot| slot.strong_count() > 0);
        if registry.ever_mutated {
            let live = slot
                .upgrade()
                .ok_or_else(|| "new helper fence slot disappeared".to_string())?;
            live.lock()
                .map_err(|_| "child lock poisoned".to_string())?
                .touch_helper_for_registry()?;
        }
        registry.slots.push(slot);
        Ok(())
    }

    /// Check every live Child slot before an operation that may affect a
    /// helper-owned core. The registry lock serializes new runtime binding
    /// with this sweep; no lock is held across the following IPC.
    pub(crate) fn register_core_mutation(&self) -> Result<(), String> {
        let mut registry = self
            .core_mutation_fences
            .lock()
            .map_err(|_| "helper core fence registry poisoned".to_string())?;
        registry.ever_mutated = true;
        let mut index = 0;
        while index < registry.slots.len() {
            let Some(slot) = registry.slots[index].upgrade() else {
                registry.slots.swap_remove(index);
                continue;
            };
            slot.lock()
                .map_err(|_| "child lock poisoned".to_string())?
                .touch_helper_for_registry()?;
            index += 1;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn core_fence_count_for_test(&self) -> usize {
        self.core_mutation_fences.lock().unwrap().slots.len()
    }

    /// **测试专用**构造：`SysOps` 替身恒报「二进制/描述符/服务都不存在」⇒ [`Self::status`] 稳定判未装。
    ///
    /// # 为什么必须有这个
    ///
    /// 生产 [`Self::new`] 走 [`StdSysOps`] + [`InstallPaths::for_platform`] 的**系统路径**，于是
    /// `status()` 读的是**宿主真实安装态**。原先所有相关单测都默认「本机/CI 从不装 polaris-helper」，
    /// 这个前提一旦破（开发机上真装过一次 Polaris），后果不是「少测一点」，而是：
    ///
    /// 1. 12 条门当场转红（2026-07-28 在 5.238 实测，那台装过 Polaris）；
    /// 2. 更糟：`installed=true` 让 `compute_status_with_client` 的短路失效，单测**真的去连
    ///    `/Library/.../polaris-helper.sock` 这个特权 daemon**并发起 `start` —— 实测拿到 `ERR auth`
    ///    才没造成副作用，即「没起成核」靠的是 token 不匹配，不是靠设计。
    ///
    /// 即门的绿与否取决于跑测机器的状态，而非被测代码 —— 换台机器就换结论。注入替身后，
    /// 「不触碰系统 socket」从**巧合**变成**结构保证**。
    #[cfg(test)]
    pub(crate) fn never_installed_for_tests(dir: PathBuf) -> Self {
        struct NeverInstalled;
        impl SysOps for NeverInstalled {
            fn exists(&self, _path: &Path) -> bool {
                false
            }
            fn start_service(&self, _label: &str) -> Result<(), String> {
                Err("测试替身：不碰真实服务".to_owned())
            }
            fn stop_service(&self, _label: &str) -> Result<(), String> {
                Err("测试替身：不碰真实服务".to_owned())
            }
            fn is_loaded(&self, _label: &str) -> bool {
                false
            }
            fn service_exists(&self, _label: &str) -> bool {
                false
            }
        }
        Self {
            dir,
            platform: Platform::current(),
            sys_ops: Arc::new(|| Box::new(NeverInstalled)),
            start_identity: Mutex::new(None),
            core_mutation_fences: Mutex::new(CoreMutationFences::default()),
            never_connect: true,
            status_override: None,
            test_connector: None,
        }
    }

    /// **测试专用**构造：在 [`Self::never_installed_for_tests`] 的全部隔离之上，再把
    /// [`Self::status`] 钉成给定快照。
    ///
    /// # 为什么必须有这个
    ///
    /// 「helper 已装但可升级」这一格在本机**造不出来**：`never_installed_for_tests` 的 `SysOps`
    /// 替身恒报未装，而换成真 `StdSysOps` 就等于让单测去读宿主真实安装态并连特权 daemon 的 socket
    /// （那正是 `never_installed_for_tests` 存在的理由）。没有这个注入口，
    /// `run_helper_gate` 的「已装但可升级」腿就只有源码级断言、没有任何运行期证据。
    ///
    /// **只钉读取面，不放开任何写入面**：`sys_ops` 仍是 `NeverInstalled`、`never_connect` 仍为 true
    /// ⇒ `install`/`uninstall`/`start_core` 一律触不到真实系统（本条注入不构成新的宿主逃逸面）。
    #[cfg(test)]
    pub(crate) fn with_forced_status_for_tests(dir: PathBuf, status: HelperStatusSnapshot) -> Self {
        Self {
            status_override: Some(status),
            ..Self::never_installed_for_tests(dir)
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn with_test_connector_for_tests(
        dir: PathBuf,
        status: HelperStatusSnapshot,
        connector: Arc<dyn Connector + Sync>,
    ) -> Self {
        Self {
            test_connector: Some(connector),
            ..Self::with_forced_status_for_tests(dir, status)
        }
    }

    /// **测试专用**构造：在 [`Self::never_installed_for_tests`] 的全部隔离之上，把
    /// [`Self::platform`] 钉成给定平台。
    ///
    /// # 为什么必须有这个
    ///
    /// [`super::proxy::ProxyRuntime`] 的一批判据经 `self.helper.platform()` 分流 ——
    /// 「Android 上接管方式恒 TUN」这条不变式的**全部**运行期落点都在那里（起核后要不要去设
    /// 系统代理、要不要开那一枪连接 flush、要不要挂 TUN 出口夺取硬闸……）。生产构造把平台钉成
    /// `Platform::current()`，于是那些腿在本机（Linux）永远只跑得到桌面那一侧：
    /// **Android 分叉一行都没有运行期证据**，只剩源码级断言。
    ///
    /// 本仓已经为这种形态付过账（`with_forced_status_for_tests` 的头注记着同一件事）。
    ///
    /// **只钉平台这一个字段，不放开任何写入面**：`sys_ops` 仍是 `NeverInstalled`、
    /// `never_connect` 仍为 true ⇒ `install` / `uninstall` / `start_core` 一律触不到真实系统。
    /// 平台钉成 Android 反而**收紧**了逃逸面（`platform_supported(Android) == false`）。
    #[cfg(test)]
    pub(crate) fn with_platform_for_tests(dir: PathBuf, platform: Platform) -> Self {
        Self {
            platform,
            ..Self::never_installed_for_tests(dir)
        }
    }

    /// 当前平台。
    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    /// 当前平台是否有提权 helper 实现（mac/win/linux 三平台均有；未知平台无）。
    #[must_use]
    pub const fn supported(&self) -> bool {
        platform_supported(self.platform)
    }

    /// app 侧 token 文件路径。
    fn token_path(&self) -> PathBuf {
        self.dir.join(HELPER_TOKEN_FILE)
    }

    /// 构造生命周期管理器（生产 [`StdSysOps`]：真 launchctl/systemctl/sc + `fs::exists`；
    /// 单测经 `Self::never_installed_for_tests` 注入替身）。
    fn manager(&self) -> HelperManager {
        HelperManager::new(self.platform, self.token_path(), (self.sys_ops)())
    }

    /// 构造生产 [`HelperClient`]（每请求一连接，经生产 Connector）。
    ///
    /// **不实际连接**：`connect()` 延迟到 `send` 时——未装/未跑时 `send` 才返 `Connect` 错误（对齐
    /// 上游 `net.connect` 的惰性）。故 `status` 在未安装态（`compute_status_with_client` 先判 is_installed）
    /// **绝不触碰 socket** → 本机安全。
    fn build_client(&self) -> Result<HelperClient, String> {
        let paths = InstallPaths::for_platform(self.platform);
        let token = read_token(&self.token_path());
        #[cfg(test)]
        if let Some(connector) = &self.test_connector {
            return Ok(HelperClient::new(
                Box::new(SharedTestConnector(Arc::clone(connector))),
                self.platform,
                token,
            ));
        }
        #[cfg(test)]
        if self.never_connect {
            return Ok(HelperClient::new(
                Box::new(NeverConnect),
                self.platform,
                token,
            ));
        }
        let connector = Self::connector(&paths.socket)?;
        Ok(HelperClient::new(connector, self.platform, token))
    }

    #[cfg(unix)]
    fn connector(socket: &Path) -> Result<Box<dyn Connector>, String> {
        Ok(Box::new(UnixConnector::new(socket)))
    }

    #[cfg(windows)]
    fn connector(socket: &Path) -> Result<Box<dyn Connector>, String> {
        Ok(Box::new(PipeConnector::new(socket)))
    }

    #[cfg(not(any(unix, windows)))]
    fn connector(_socket: &Path) -> Result<Box<dyn Connector>, String> {
        Err("当前平台无 helper connector".to_owned())
    }

    /// Read-only helper/core snapshot, shared by queries, tray polling and
    /// maintenance replies. Never starts a stopped service: another App may be
    /// holding an install/uninstall transaction. Recovery requires explicit repair.
    #[must_use]
    pub fn status(&self) -> HelperStatusSnapshot {
        // 测试注入的钉死快照（`with_forced_status_for_tests`）。**必须在最前**：本方法余下的每一步
        // 都会去读系统面，而注入的全部意义就是不读它。
        #[cfg(test)]
        if let Some(forced) = &self.status_override {
            return forced.clone();
        }
        let supported = self.supported();
        if !supported {
            return HelperStatusSnapshot {
                supported,
                ..Default::default()
            };
        }
        let manager = self.manager();
        match self.build_client() {
            Ok(client) => {
                let mut status =
                    snapshot_from(&manager.compute_status_with_client(&client), supported);
                self.apply_selected_core_status(&mut status);
                status
            }
            Err(e) => {
                log::debug!("helper status：建 client 失败（{e}）→ 视作未就绪");
                let mut status = snapshot_from(&HelperStatus::default(), supported);
                self.apply_selected_core_status(&mut status);
                status
            }
        }
    }

    fn apply_selected_core_status(&self, status: &mut HelperStatusSnapshot) {
        let Ok(platform) = crate::runtime::core_identity::current_platform() else {
            return;
        };
        let Ok(manifest) = crate::runtime::core_identity::selected_manifest(platform) else {
            status.ready = false;
            status.needs_repair = true;
            return;
        };
        let Ok(id) = manifest.id(polaris_updater::sha256_hex) else {
            status.ready = false;
            status.needs_repair = true;
            return;
        };
        let state = if !status.ready {
            SelectedCoreState::Unavailable
        } else {
            match self.core_status() {
                Ok(polaris_helper_proto::core_protocol::CoreStatus::Present { .. }) => {
                    SelectedCoreState::Present
                }
                Ok(polaris_helper_proto::core_protocol::CoreStatus::Absent { .. }) => {
                    status.ready = false;
                    status.upgradeable = true;
                    SelectedCoreState::Absent
                }
                Ok(polaris_helper_proto::core_protocol::CoreStatus::Invalid { .. }) => {
                    status.ready = false;
                    status.needs_repair = true;
                    SelectedCoreState::Invalid
                }
                Err(_) => {
                    status.ready = false;
                    status.needs_repair = true;
                    SelectedCoreState::Unavailable
                }
            }
        };
        status.selected_core = Some(SelectedCoreSnapshot {
            id: id.as_str().into(),
            version: manifest.version,
            provider_fingerprint: manifest.provider_fingerprint,
            state,
        });
    }

    /// 组装失败结果（失败态复用一次真状态探测）。原始错误只进日志，回包只留稳定码和泛化诊断。
    fn action_failed(
        &self,
        code: HelperActionErrorCode,
        diagnostic: impl Into<String>,
    ) -> HelperActionResult {
        HelperActionResult {
            success: false,
            execution_diagnostic: None,
            error_code: Some(code),
            diagnostic: Some(diagnostic.into()),
            status: self.status(),
        }
    }

    /// 安装/修复 helper（上游 `helper:install`）——真流程：HelperManager.install（拷二进制 + 写 root token
    /// + 播种核 + 写描述符 + bootstrap，经提权跑一次 root 脚本）。返回三态信封。
    ///
    /// **真机门**：成功路径触发 osascript/UAC/pkexec 弹框。本机无 bundled helper → 早返「二进制缺失」，不弹框。
    #[must_use]
    pub fn install(&self) -> HelperActionResult {
        self.install_with_mode(HelperInstallMode::Ordinary)
    }

    /// StopAndUpgrade requires explicit confirmation covering all sessions managed by this helper.
    #[must_use]
    pub fn install_with_mode(&self, mode: HelperInstallMode) -> HelperActionResult {
        if !mode.supported_on(self.platform) {
            return self.action_failed(
                HelperActionErrorCode::Unsupported,
                "explicit helper stop-and-upgrade is supported only on macOS",
            );
        }
        if !self.supported() {
            return self.action_failed(HelperActionErrorCode::Unsupported, "helper unsupported");
        }
        let (params, _stage) = match self.install_params() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("helper install assets unavailable: {e}");
                return self.action_failed(
                    if e.starts_with("CORE_BUNDLE_MISMATCH:") {
                        HelperActionErrorCode::BundleMismatch
                    } else {
                        HelperActionErrorCode::MissingAsset
                    },
                    "helper installation inputs unavailable or invalid",
                );
            }
        };
        if let Err(error) = self.register_core_mutation() {
            log::warn!("helper install mutation preflight failed: {error}");
            return self.action_failed(HelperActionErrorCode::Failed, error);
        }
        let manager = self.manager();
        match manager.install_with_mode(&params, &StdExecutor, mode.manager_mode()) {
            Ok(EscalationOutcome::Success) => {
                // Service readiness and this App's installed content id are
                // separate receipts, including when a newer helper is retained.
                let mut status = match self.build_client() {
                    Ok(client) => snapshot_from(
                        &manager.wait_until_ready(&client, READY_POLL_ATTEMPTS, READY_POLL_DELAY),
                        true,
                    ),
                    Err(_) => self.status(),
                };
                self.apply_selected_core_status(&mut status);
                if helper_install_reached_expected_build(&status) {
                    HelperActionResult {
                        success: true,
                        execution_diagnostic: None,
                        error_code: None,
                        diagnostic: None,
                        status,
                    }
                } else {
                    HelperActionResult {
                        success: false,
                        execution_diagnostic: None,
                        error_code: Some(HelperActionErrorCode::NotReady),
                        diagnostic: Some(
                            "helper protocol or selected core is not ready after installation"
                                .to_owned(),
                        ),
                        status,
                    }
                }
            }
            Ok(EscalationOutcome::Cancelled) => self.action_failed(
                HelperActionErrorCode::Cancelled,
                "administrator authorization cancelled",
            ),
            Ok(EscalationOutcome::Failed { stderr, code }) => {
                log::warn!("helper install command failed (exit {code}): {stderr}");
                let mut result = self.action_failed(
                    install_command_failure_code(self.platform, code, &stderr),
                    install_command_diagnostic(self.platform, code),
                );
                result.execution_diagnostic =
                    helper_execution_diagnostic(self.platform, code, &stderr);
                result
            }
            Err(e) => {
                log::warn!("helper install failed: {e}");
                self.action_failed(
                    manager_install_failure_code(&e),
                    "helper install operation failed",
                )
            }
        }
    }

    /// 卸载 helper（上游 `helper:uninstall`）——真流程：HelperManager.uninstall（bootout/删描述符/删二进制/
    /// 删受保护目录，经提权跑）。返回三态信封。
    ///
    /// ⚠️ **前置停核不在本方法内**：本运行时不持有 `ProxyRuntime`（那是 `AppRuntime` 层的编排）。
    /// 「卸载前零提权停核」由命令层 [`crate::commands::helper_uninstall`] 经
    /// [`uninstall_preflight_stop`] 完成 —— 直接调本方法会跳过那道闸。
    #[must_use]
    pub fn uninstall(&self) -> HelperActionResult {
        if !self.supported() {
            return self.action_failed(HelperActionErrorCode::Unsupported, "helper unsupported");
        }
        if let Err(error) = self.register_core_mutation() {
            return self.action_failed(HelperActionErrorCode::Failed, error);
        }
        let manager = self.manager();
        let utility = if self.platform == Platform::Win {
            None
        } else {
            match maintenance_utility() {
                Ok(utility) => Some(utility),
                Err(error) => {
                    return self.action_failed(HelperActionErrorCode::MissingAsset, error)
                }
            }
        };
        match manager.uninstall_with_maintenance(&self.dir, utility.as_ref(), &StdExecutor) {
            Ok(EscalationOutcome::Success) => HelperActionResult {
                success: true,
                execution_diagnostic: None,
                error_code: None,
                diagnostic: None,
                status: self.status(),
            },
            Ok(EscalationOutcome::Cancelled) => self.action_failed(
                HelperActionErrorCode::Cancelled,
                "administrator authorization cancelled",
            ),
            Ok(EscalationOutcome::Failed { stderr, code }) => {
                log::warn!("helper uninstall command failed (exit {code}): {stderr}");
                let mut result = self.action_failed(
                    uninstall_failure_code(self.platform, code),
                    "helper uninstaller exited unsuccessfully",
                );
                result.execution_diagnostic =
                    helper_execution_diagnostic(self.platform, code, &stderr);
                result
            }
            Err(e) => {
                log::warn!("helper uninstall failed: {e}");
                self.action_failed(
                    HelperActionErrorCode::Failed,
                    "helper uninstall operation failed",
                )
            }
        }
    }

    /// 组装 [`InstallParams`]（解析 bundled 资源 + 当前 uid）。
    fn install_params(
        &self,
    ) -> Result<(InstallParams, crate::runtime::core_identity::InstallStage), String> {
        let src_binary = resolve_helper_binary()?;
        // 三平台同构：安装脚本用它**播种**受保护核目录（仅在目标缺失时）。名叫 bundled 实为
        // `resolve_core_binary()` = **现役核**（即随包核；debug / test 构型可被开发态超驰替换），如实登记在
        // `InstallParams::bundled_core` 的字段文档里。
        // win 的 `--singbox` 不再从这里取：P4 起由 `InstallPaths::win().core_dir` 派生受保护路径。
        let bundled_core = crate::runtime::proxy::resolve_core_binary()?;
        let stage = crate::runtime::core_identity::stage_installation(
            &bundled_core,
            &src_binary,
            &self.dir,
        )?;
        #[cfg(windows)]
        let user_identity = polaris_helper::platform::windows::native_caller::current_user()
            .map_err(|error| error.to_string())?;
        #[cfg(not(windows))]
        let user_identity =
            polaris_helper_proto::core_protocol::CallerIdentity::Unix(current_uid());
        let core_name = polaris_helper_proto::core_payload::core_filename(self.platform);
        let helper_name = if self.platform == Platform::Win {
            "polaris-helper.exe"
        } else {
            "polaris-helper"
        };
        let manifest_path = stage.directory.join("core-manifest.json");
        let manifest_sha256 = polaris_updater::sha256_hex(
            &stage
                .manifest
                .to_json()
                .map_err(|error| error.to_string())?,
        );
        let params = InstallParams {
            manifest_path,
            manifest_sha256,
            core_id: polaris_helper_proto::core_manifest::CoreId::parse(&stage.manifest.id)
                .map_err(|error| error.to_string())?,
            helper_sha256: stage
                .manifest
                .core
                .helper_sha256
                .clone()
                .ok_or("actual installer hash unavailable")?,
            helper_version: env!("CARGO_PKG_VERSION").into(),
            user_identity,
            src_binary: stage.directory.join(helper_name),
            bundled_core: stage.directory.join(core_name),
            conf_dir: self.dir.clone(),
            uid: current_uid(),
            script_dir: self.dir.clone(),
        };
        Ok((params, stage))
    }

    pub(crate) fn core_status(
        &self,
    ) -> Result<polaris_helper_proto::core_protocol::CoreStatus, String> {
        let id = crate::runtime::core_identity::selected_id(
            crate::runtime::core_identity::current_platform()?,
        )?;
        query_core_status(&self.build_client()?, &id)
    }

    pub(crate) fn require_selected_core(&self) -> Result<(), String> {
        match self.core_status()? {
            polaris_helper_proto::core_protocol::CoreStatus::Present { .. } => Ok(()),
            polaris_helper_proto::core_protocol::CoreStatus::Absent { .. } => {
                Err("HELPER_CORE_ABSENT: 提权助手尚未安装本版本配套内核，请更新提权助手".into())
            }
            polaris_helper_proto::core_protocol::CoreStatus::Invalid { .. } => {
                Err("HELPER_CORE_INVALID: 配套内核校验失败，请重新安装提权助手".into())
            }
        }
    }

    /// Start only the compiled selected CoreID after the administrator has
    /// installed its immutable objects. Runtime IPC never chooses executable
    /// paths or uploads bytes. The native receipt must echo this same id.
    /// Actual spawn/TUN and administrator installation remain native validation gates.
    pub fn start_core(
        &self,
        cfg: &Path,
        log: &Path,
        fwd: bool,
        ppid: Option<u32>,
    ) -> Result<HelperStartResult, String> {
        self.register_core_mutation()?;
        let client = self.build_client()?;
        let common = StartParams {
            cfg: cfg.to_string_lossy().into_owned(),
            log: log.to_string_lossy().into_owned(),
            fwd,
            parent_pid: ppid,
        };
        let id = crate::runtime::core_identity::selected_id(
            crate::runtime::core_identity::current_platform()?,
        )?;
        let params = polaris_helper_proto::core_protocol::PinnedStartParams {
            id: id.clone(),
            common,
        };
        let req = match self.platform {
            Platform::Linux => Request::LinuxStartPinned(params),
            Platform::Mac | Platform::Win => Request::NativeStartPinned(params),
            Platform::Other | Platform::Android | Platform::Ios => {
                return Err("pinned desktop helper unavailable on this platform".into())
            }
        };
        let received = client
            .send_with_timeout(&req, HELPER_START_TIMEOUT)
            .map_err(|error| format!("helper pinned 起核通信失败：{error}"))?;
        let resp = match received {
            Response::Ok(ResponseKind::PinnedStart { id: actual, start }) if actual == id => {
                pinned_receipt_response(self.platform, start)?
            }
            Response::Ok(ResponseKind::PinnedStart { start, .. }) => {
                return match start {
                    polaris_helper_proto::core_protocol::PinnedBirthStart::Linux(
                        LinuxBirthStart::Started { target, .. },
                    )
                    | polaris_helper_proto::core_protocol::PinnedBirthStart::Native(
                        NativeBirthStart::Started { target, .. },
                    ) => Ok(HelperStartResult::BirthRejected(target)),
                    _ => Err("HELPER_CORE_MISMATCH: helper content id differs".into()),
                };
            }
            Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Started {
                target,
                ..
            }))
            | Response::Ok(ResponseKind::NativeBirthStart(NativeBirthStart::Started {
                target,
                ..
            })) => return Ok(HelperStartResult::BirthRejected(target)),
            Response::Err(error) => {
                return Err(format_helper_mutation_error(
                    self.platform,
                    "pinned 起核",
                    &error,
                ))
            }
            _ => return Err("HELPER_CORE_MISMATCH: helper omitted pinned receipt".into()),
        };
        if self.platform == Platform::Linux {
            return classify_linux_birth_start_response(resp);
        }
        if matches!(self.platform, Platform::Mac | Platform::Win) {
            return match resp {
                Response::Ok(ResponseKind::NativeBirthStart(NativeBirthStart::Started {
                    target,
                    timing,
                    created,
                })) => {
                    if let Some(timing) = timing {
                        log::info!("helper native core start timing: {timing:?}");
                    }
                    self.remember_start_identity(target.pid.get(), created);
                    Ok(HelperStartResult::BirthStarted(target))
                }
                Response::Ok(ResponseKind::NativeBirthStart(NativeBirthStart::Already {
                    target,
                })) => Ok(HelperStartResult::BirthAlready(target)),
                Response::Ok(ResponseKind::NativeBirthStart(
                    NativeBirthStart::NotAdmittedPending { target },
                )) => Ok(HelperStartResult::BirthNotAdmitted {
                    target: Some(target),
                    pending: true,
                }),
                Response::Ok(ResponseKind::NativeBirthStart(
                    NativeBirthStart::NotAdmittedUnknown { target },
                )) => Ok(HelperStartResult::BirthNotAdmitted {
                    target,
                    pending: false,
                }),
                Response::Ok(other) => Err(format!(
                    "helper native birth 起核返回非预期响应：{other:?}；请升级或修复 helper"
                )),
                Response::Err(error) => Err(format_helper_mutation_error(
                    self.platform,
                    "native birth 起核",
                    &error,
                )),
            };
        }
        match resp {
            Response::Ok(ResponseKind::Start(Start::StartedTimed {
                pid,
                timing,
                created,
            })) => {
                log::info!(
                    "helper core start timing: forwarding={}ms process={}ms job={}ms log_handoff={}ms total={}ms",
                    timing.forwarding_ms,
                    timing.process_ms,
                    timing.job_ms,
                    timing.log_handoff_ms,
                    timing.total_ms
                );
                // D2/D3(4) 的「start 拿初值」：Windows 新 helper 会带回受管核的进程创建时间，
                // 存成崩溃监测的**初始**身份基线（消费点见 `proxy::recovery::spawn_crash_monitor`）。
                self.remember_start_identity(pid, created);
                Ok(HelperStartResult::Started(pid))
            }
            Response::Ok(ResponseKind::Start(Start::Started { pid } | Start::Already { pid })) => {
                // 无 timing 形态 ⇒ 没有 created 可存。**必须显式落 None**：留着上一次 start 的值，
                // 万一新核拿到同一个 pid，基线就会拿旧进程的创建时间去比新进程，判出一次假复用。
                self.remember_start_identity(pid, None);
                Ok(HelperStartResult::Started(pid))
            }
            Response::Ok(ResponseKind::Start(Start::NotAdmitted(blocker))) => {
                Ok(HelperStartResult::NotAdmitted(blocker))
            }
            Response::Ok(other) => Err(format!("helper 起核返回非预期响应：{other:?}")),
            Response::Err(e) => Err(format_helper_mutation_error(self.platform, "起核", &e)),
        }
    }

    /// 记下 `start` 回传的受管核身份（D2/D3(4) 的「start 拿初值」）。
    ///
    /// 顺带落一条诊断：真机上「有没有拿到身份令牌」是判断 pid 复用检出是否生效的第一现场，
    /// 缺了它只能靠猜。
    fn remember_start_identity(&self, pid: u32, created: Option<u64>) {
        match created {
            Some(created) => log::info!("helper 受管核身份令牌：pid={pid} created={created}"),
            None => {
                log::debug!("helper 未回传受管核身份令牌（旧 helper 或非 Windows）：pid={pid}")
            }
        }
        if let Ok(mut slot) = self.start_identity.lock() {
            *slot = created.map(|ticks| (pid, ticks));
        }
    }

    /// 特权 helper 通道**结构上是否存在**（app 侧 token 在位）。
    ///
    /// 用于 `dns_flush::flush_os_dns_cache` 的 Windows 腿前置判据（spec §3.1「if helper ready」）：
    /// 没装 helper 的机器每次起停核都吃一条「helper flush-dns 不可用」纯属噪音。
    ///
    /// # 为什么不是 `status().ready`
    ///
    /// Status is also read-only, but probing IPC for a best-effort DNS preflight
    /// would add avoidable latency. Token presence is only a structural hint.
    ///
    /// # 这条判据的射程（如实登记）
    ///
    /// token 由 `HelperManager::prepare_token` 在**安装时**写、`clear_token` 在卸载时删，故它回答的
    /// 是「这台机器上装过且没卸」。它**不**回答「helper 此刻跑不跑得起来」——那一格蓄意留给真正的
    /// 尝试：token 在而 helper 坏了，照常试、照常报，「真失败」不被这个门吞掉。
    pub(crate) fn client_token_present(&self) -> bool {
        !read_token(&self.token_path()).is_empty()
    }

    /// 崩溃监测的**初始**身份基线：最近一次 `start` 回传的 `(pid, created)`；没拿到 ⇒ `None`。
    ///
    /// # 为什么基线不能等第一次 `status` 再取（这条方法存在的全部理由）
    ///
    /// 稳态下两者同源同值，差别只在一格：核若在**首次 status 之前**自然死亡，而这期间另一方
    /// 发了一次 `start` —— helper 用新 child 替换受管 child、**关掉旧句柄** —— 那个 PID 从这一刻
    /// 起就重新可被系统复用（句柄没关时内核不复用 PID，这正是 D3 的立足点）。此时第一次 status
    /// 读到的创建时间已经属于**另一个进程**，把它登记成基线，后续每次复核都自己跟自己比：
    /// 恒 `Match`，复用永远判不出来。start 的值是「这个 pid 刚起来那一刻」的事实，不随后续换手而变。
    pub(crate) fn managed_start_identity(&self) -> Option<(u32, u64)> {
        self.start_identity.lock().ok().and_then(|slot| *slot)
    }

    /// 查询 helper 自己受管的核状态。
    ///
    /// app 进程对 root/SYSTEM child 的本地进程查询可能只能得到“未知”；helper 与 child 位于同一权限
    /// 边界内，它的 `status` 才是崩溃监测应使用的权威事实。该调用是同步 IPC，异步调用方必须放入
    /// `spawn_blocking`，避免阻塞 Tokio worker。
    pub(crate) fn managed_core_status(&self) -> Result<ManagedCoreStatus, String> {
        let client = self.build_client()?;
        managed_core_status_with_client(&client)
    }

    /// Probe support before any Linux helper Start reservation is armed. This
    /// status request is read-only; an old helper's ERR unknown must remain a
    /// recoverable upgrade error, not an unconfirmed possible child birth.
    pub(crate) fn require_linux_birth_capability(&self) -> Result<(), BirthCapabilityError> {
        if self.platform != Platform::Linux {
            return Ok(());
        }
        let client = self
            .build_client()
            .map_err(BirthCapabilityError::Unavailable)?;
        require_linux_birth_capability_with_client(&client)
    }

    /// Read-only native capability; called before a Mac/Windows external birth is armed.
    pub(crate) fn require_native_birth_capability(&self) -> Result<(), BirthCapabilityError> {
        if !matches!(self.platform, Platform::Mac | Platform::Win) {
            return Ok(());
        }
        let client = self
            .build_client()
            .map_err(BirthCapabilityError::Unavailable)?;
        require_native_birth_capability_with_client(&client)
    }

    /// 经 helper 停核（对称：经 helper 起的核经 helper stop）。daemon `stop` 摘其受管 child → 终止 →
    /// 收割。
    ///
    /// **`target` 是身份判据，不是可选诊断字段**（根因）：这条腿是同步阻塞 IPC —— 从发出 `stop`
    /// 到 daemon 真动手之间可以隔很久（socket 已删/daemon 无响应时更久）。这期间用户完全可能重装
    /// helper 并起了新核，daemon 手里的「受管 pid」此刻已经换成**新核**。若不带身份，daemon 按
    /// 「停当前受管的」执行 = 杀掉用户刚连上的核（表现为「刚连上就被静默断开」，且酷似核自己崩了）。
    /// app 侧的世代守卫够不着这一层：杀进程发生在 helper 进程里，故判据必须随请求下发。
    ///
    /// Linux only supplies a known exact birth target. An unconfirmed Start
    /// cannot authorize a PID-only or targetless Stop.
    ///
    /// 身份不匹配 → daemon 回 `stop-mismatch` 且**一个进程都不杀**；本方法把它落成 `Err`（连同两个
    /// pid），因为「本腿意图停的核」确实没被停 —— 报 `Ok` 会让调用方的日志说谎。
    ///
    /// **真机门**：真停 root/SYSTEM 受管核。
    pub(crate) fn stop_core(&self, target: HelperStopTarget) -> Result<(), String> {
        if target.pid() == 0 {
            return Err("helper Stop requires a nonzero PID".to_owned());
        }
        if self.platform == Platform::Linux && matches!(target, HelperStopTarget::Legacy(_)) {
            return Err("Linux helper Stop requires an exact birth target".into());
        }
        self.register_core_mutation()?;
        let client = self.build_client()?;
        match target {
            HelperStopTarget::Birth(target) => stop_birth_with_client_budget(
                &client,
                target,
                HELPER_STOP_TIMEOUT,
                HELPER_STOP_RETRY_DELAY,
            ),
            HelperStopTarget::Legacy(pid) => stop_core_with_client(&client, Some(pid)),
        }
    }

    /// **T3 提权清扫**：经 helper 清掉 root 起的孤儿核（用户态 `kill` 收 EPERM 杀不动的那些）。
    ///
    /// daemon 侧 `Cleanup` = `pkill -9 -f "<锁定的 singbox_bin> run"` + 摘 child——**模式锚在
    /// helper 自己锁定的核路径上**，故不会误杀用户系统装的无关 sing-box（与 [`stop_core`](Self::stop_core)
    /// 只停受管 child 不同：此处要清的正是 daemon 已不再持有句柄的历史遗留）。
    ///
    /// **调用约束**：它是无差别清扫（连当前受管核一起杀），故**只允许在起核前的清扫期调用**
    /// （`cleanup_stale_cores`，此时本会话尚无受管核）。运行期停核走 `stop_core`。
    ///
    /// **真机门**：真杀 root 进程需 mac/win 真机 + 已装就绪 helper。
    #[cfg(not(unix))]
    pub fn cleanup_cores(&self) -> Result<(), String> {
        self.register_core_mutation()?;
        let client = self.build_client()?;
        let resp = client
            .send(&Request::Cleanup)
            .map_err(|e| format!("helper 清扫通信失败：{e}"))?;
        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(e) => Err(format!("helper 清扫失败：{e}")),
        }
    }

    /// C5：经 root/SYSTEM helper 装/删 mesh 出口路由（mac：`route add/del -ifscope`；win：`route add/del`）。
    /// `op`="add"|"del"。**Linux 不经此腿**——app 自身 CAP_NET_ADMIN 直接 `ip route`（独立表 7732 + oif 规则，
    /// 见 `runtime/mesh.rs::HelperExitRouteOp`）；helper 协议蓄意无 Linux route 命令。
    ///
    /// 契约（对齐 上游 `MeshExitRouteManager.runRoute` 的 best-effort）：**永不 panic** —— 建 client /
    /// 通信 / 协议 / helper 侧失败一律收敛为 `false`，调用方（`HelperExitRouteOp::run_route`）据此不标 installed。
    ///
    /// **真机门**：真改宿主路由表需 mac/win 真机 + 已装就绪 helper；本机（Linux）不经此腿。
    #[must_use]
    pub fn route_op(&self, op: &str, iface: &str, cidrs: &[String]) -> bool {
        let client = match self.build_client() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("出口路由 helper：建 client 失败（{e}）→ route-{op} 失败");
                return false;
            }
        };
        let params = RouteParams {
            iface: iface.to_string(),
            cidrs: cidrs.to_vec(),
        };
        let req = if op == "add" {
            Request::RouteAdd(params)
        } else {
            Request::RouteDel(params)
        };
        match client.send_with_timeout(&req, HELPER_ROUTE_TIMEOUT) {
            Ok(Response::Ok(ResponseKind::Route)) => true,
            Ok(other) => {
                log::warn!("helper route-{op} 返回非预期响应（可能 helper 版本过旧）：{other:?}");
                false
            }
            Err(e) => {
                log::warn!("helper route-{op} 通信失败：{e}");
                false
            }
        }
    }

    /// Linux：经 root helper 接管 `systemd-resolved` 的 Polaris TUN 链路。
    ///
    /// 不预判协议版本：该命令是首次正式发布前加入的 v1 兼容扩展。旧开发版 helper 返回
    /// `ERR unknown` 时会原样成为显式降级错误，不静默假成功。
    pub fn linux_resolved_takeover(&self) -> Result<(), String> {
        if self.platform != Platform::Linux {
            return Err("Linux resolved takeover is unavailable on this platform".to_owned());
        }
        let client = self.build_client()?;
        let request = Request::LinuxDnsSet(LinuxDnsSetParams {
            interface_name: polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME.to_owned(),
            server_ip: polaris_helper_proto::linux_dns::CONTROLLED_DNS_IP.to_owned(),
        });
        match client.send_with_timeout(&request, HELPER_LINUX_RESOLVED_TIMEOUT) {
            Ok(Response::Ok(ResponseKind::LinuxDns(LinuxDns::Set))) => Ok(()),
            Ok(Response::Ok(other)) => {
                Err(format!("helper resolved 接管返回非预期响应：{other:?}"))
            }
            Ok(Response::Err(error)) => Err(format!("helper resolved 接管失败：{error}")),
            Err(error) => Err(format!("helper resolved 接管通信失败：{error}")),
        }
    }

    /// Linux：经 root helper 撤销 Polaris TUN 链路的 resolved 临时状态。
    pub fn linux_resolved_revert(&self) -> Result<(), String> {
        if self.platform != Platform::Linux {
            return Err("Linux resolved revert is unavailable on this platform".to_owned());
        }
        let client = self.build_client()?;
        let request = Request::LinuxDnsRevert {
            interface_name: polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME.to_owned(),
        };
        match client.send_with_timeout(&request, HELPER_LINUX_RESOLVED_TIMEOUT) {
            Ok(Response::Ok(ResponseKind::LinuxDns(LinuxDns::Reverted))) => Ok(()),
            Ok(Response::Ok(other)) => {
                Err(format!("helper resolved 还原返回非预期响应：{other:?}"))
            }
            Ok(Response::Err(error)) => Err(format!("helper resolved 还原失败：{error}")),
            Err(error) => Err(format!("helper resolved 还原通信失败：{error}")),
        }
    }

    /// C7：经 root/SYSTEM helper 刷系统 DNS 缓存（mac `flush-dns`：dscacheutil + HUP mDNSResponder 两层全清）。
    ///
    /// 契约（对齐 上游 `HelperManager.flushDns`）：**永不抛** —— 通信/协议失败收敛为 `ok:false`，调用方
    /// （`dns_flush::flush_os_dns_cache`）据此降级用户级 `dscacheutil`。`partial`=dscacheutil 成功但 HUP
    /// mDNSResponder 失败（app 不降级，用户级同样无权 HUP）。5s 超时。
    ///
    /// **真机门**：真正的 root helper `killall -HUP mDNSResponder` 需 mac 真机 + 已装 helper；本机 Linux
    /// 不经此腿（`flush_os_dns_cache` 仅 mac 调 helper，见其平台分派）。
    #[must_use]
    pub fn flush_dns(&self) -> HelperFlushResult {
        let client = match self.build_client() {
            Ok(c) => c,
            Err(e) => {
                return HelperFlushResult {
                    ok: false,
                    partial: None,
                    error: Some(e),
                };
            }
        };
        let request = flush_dns_request(self.platform());
        match client.send_with_timeout(&request, HELPER_FLUSH_TIMEOUT) {
            Ok(Response::Ok(ResponseKind::FlushDns(FlushDns::Flushed))) => HelperFlushResult {
                ok: true,
                partial: None,
                error: None,
            },
            Ok(Response::Ok(ResponseKind::FlushDns(FlushDns::FlushedPartial { tail }))) => {
                HelperFlushResult {
                    ok: true,
                    partial: Some(tail),
                    error: None,
                }
            }
            Ok(Response::Ok(other)) => HelperFlushResult {
                ok: false,
                partial: None,
                error: Some(format!("helper flush-dns 返回非预期响应：{other:?}")),
            },
            Ok(Response::Err(e)) => HelperFlushResult {
                ok: false,
                partial: None,
                error: Some(e.to_string()),
            },
            Err(e) => HelperFlushResult {
                ok: false,
                partial: None,
                error: Some(format!("helper flush-dns 通信失败：{e}")),
            },
        }
    }
}

fn flush_dns_request(platform: Platform) -> Request {
    match platform {
        Platform::Win => Request::WindowsFlushDnsNative,
        Platform::Mac | Platform::Linux | Platform::Android | Platform::Ios | Platform::Other => {
            Request::FlushDns
        }
    }
}

fn classify_linux_birth_start_response(response: Response) -> Result<HelperStartResult, String> {
    match response {
        Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Started {
            target,
            timing,
        })) => {
            if let Some(timing) = timing {
                log::info!(
                    "helper exact birth start timing: forwarding={}ms process={}ms job={}ms log_handoff={}ms total={}ms",
                    timing.forwarding_ms,
                    timing.process_ms,
                    timing.job_ms,
                    timing.log_handoff_ms,
                    timing.total_ms
                );
            }
            Ok(HelperStartResult::BirthStarted(target))
        }
        Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Already { target })) => {
            Ok(HelperStartResult::BirthAlready(target))
        }
        Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::NotAdmittedPending {
            target,
        })) => Ok(HelperStartResult::BirthNotAdmitted {
            target: Some(target),
            pending: true,
        }),
        Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::NotAdmittedUnknown {
            target,
        })) => Ok(HelperStartResult::BirthNotAdmitted {
            target,
            pending: false,
        }),
        Response::Ok(other) => Err(format!("helper exact birth 起核返回非预期响应：{other:?}")),
        Response::Err(error) => Err(format_helper_mutation_error(
            Platform::Linux,
            "起核",
            &error,
        )),
    }
}

#[cfg(any(test, target_os = "macos"))]
fn classify_mac_proxy_client_error(
    error: ClientError,
) -> polaris_system_integration::proxy_ops::MacProxyWriterError {
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    match error {
        // 连接尚未建立，helper 不可能已修改系统；可安全回落兼容路径。
        ClientError::Connect(_) => MacProxyWriterError::Unavailable(error.to_string()),
        // 写后 IO/超时/空响应的提交结果不明，禁止再走第二条写路径。
        ClientError::Timeout | ClientError::Io(_) | ClientError::EmptyResponse => {
            MacProxyWriterError::Failed(error.to_string())
        }
    }
}

/// 停核请求的可测试通信核。Linux `Pending` 在同一次调用内轮询，直到 exact-birth
/// reap ACK 或总截止时间；`Unknown` 立即 fail closed。通信错误最多补发一次。
fn stop_core_with_client(client: &HelperClient, want_pid: Option<u32>) -> Result<(), String> {
    stop_core_with_client_budget(
        client,
        want_pid,
        HELPER_STOP_TIMEOUT,
        HELPER_STOP_RETRY_DELAY,
    )
}

fn stop_core_with_client_budget(
    client: &HelperClient,
    want_pid: Option<u32>,
    total_timeout: Duration,
    retry_delay: Duration,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let mut transport_retries = 0;
    let mut pending_pid = None;
    let request = match client.platform() {
        Platform::Linux => Request::LinuxStop { pid: want_pid },
        Platform::Mac | Platform::Win | Platform::Android | Platform::Ios | Platform::Other => {
            Request::Stop { pid: want_pid }
        }
    };
    loop {
        let remaining = total_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(match pending_pid {
                Some(pid) => format!(
                    "helper 停核超时：pid={pid} 在 {total_timeout:?} 内未取得物理 reap 回执"
                ),
                None => format!("helper 停核通信超时：总预算 {total_timeout:?}"),
            });
        }
        let response = match client.send_with_timeout(&request, remaining) {
            Ok(response) => response,
            Err(_error) if transport_retries < HELPER_STOP_MAX_RETRIES => {
                transport_retries += 1;
                sleep_within_stop_budget(started, total_timeout, retry_delay)?;
                continue;
            }
            Err(error) => return Err(format!("helper 停核通信失败：{error}")),
        };
        match response {
            Response::Ok(ResponseKind::Stop(Stop::Mismatch { want, current })) => {
                return Err(format!(
                    "helper 未停核：其受管核已是 pid={current}（本腿意图停 pid={want}）\
                     → 判定为已被新会话接管，让位不动它"
                ));
            }
            Response::Ok(ResponseKind::Stop(Stop::Stopped { pid }))
                if want_pid.is_none_or(|want| want == pid) =>
            {
                return Err(format!(
                    "helper cleanup-unknown: legacy Stop ACK for pid={pid} lacks same-birth native exit proof"
                ));
            }
            Response::Ok(ResponseKind::Stop(Stop::NotRunning)) => {
                return Err(
                    "helper cleanup-unknown: legacy notrunning cannot prove this owned birth exited"
                        .into(),
                );
            }
            Response::Ok(ResponseKind::Stop(Stop::Pending { pid }))
                if want_pid.is_none_or(|want| want == pid) =>
            {
                pending_pid = Some(pid);
                sleep_within_stop_budget(started, total_timeout, retry_delay)?;
            }
            Response::Ok(ResponseKind::Stop(Stop::Unknown { pid }))
                if want_pid.is_none_or(|want| want == pid) =>
            {
                return Err(format!(
                    "helper 停核结果未知：pid={pid} 的物理 Child reap 失败，保留 custody"
                ));
            }
            Response::Ok(other) => {
                return Err(format!("helper 停核返回非预期响应：{other:?}"));
            }
            Response::Err(error) => {
                return Err(format_helper_mutation_error(
                    client.platform(),
                    "停核",
                    &error,
                ));
            }
        }
    }
}

fn stop_birth_with_client_budget(
    client: &HelperClient,
    target: HelperBirthTarget,
    total_timeout: Duration,
    retry_delay: Duration,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let mut transport_retries = 0;
    let mut pending = false;
    let request = match client.platform() {
        Platform::Linux => Request::LinuxStopBirth { target },
        Platform::Mac | Platform::Win => Request::NativeStopBirth { target },
        Platform::Android | Platform::Ios | Platform::Other => {
            return Err("helper exact birth Stop unsupported on this platform".into());
        }
    };
    loop {
        let remaining = total_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(if pending {
                format!("helper exact birth 停核超时：target={target:?} 未取得 native reap 回执")
            } else {
                format!("helper exact birth 停核通信超时：总预算 {total_timeout:?}")
            });
        }
        let response = match client.send_with_timeout(&request, remaining) {
            Ok(response) => response,
            Err(_) if transport_retries < HELPER_STOP_MAX_RETRIES => {
                transport_retries += 1;
                sleep_within_stop_budget(started, total_timeout, retry_delay)?;
                continue;
            }
            Err(error) => return Err(format!("helper exact birth 停核通信失败：{error}")),
        };
        // Only the platform's exact response family can enter the common local-stop classifier.
        let response = if matches!(client.platform(), Platform::Mac | Platform::Win) {
            match response {
                Response::Ok(ResponseKind::NativeBirthStop(stop)) => {
                    Response::Ok(ResponseKind::LinuxBirthStop(match stop {
                        NativeBirthStop::Stopped { target } => LinuxBirthStop::Stopped { target },
                        NativeBirthStop::Pending { target } => LinuxBirthStop::Pending { target },
                        NativeBirthStop::Unknown { target } => LinuxBirthStop::Unknown { target },
                        NativeBirthStop::Mismatch { requested, current } => {
                            LinuxBirthStop::Mismatch { requested, current }
                        }
                    }))
                }
                Response::Err(error) => Response::Err(error),
                Response::Ok(other) => {
                    return Err(format!(
                        "helper native birth Stop response family mismatch: {other:?}"
                    ));
                }
            }
        } else {
            response
        };
        match response {
            Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Stopped {
                target: receipt,
            })) if receipt == target => return Ok(()),
            Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Pending {
                target: receipt,
            })) if receipt == target => {
                pending = true;
                sleep_within_stop_budget(started, total_timeout, retry_delay)?;
            }
            Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Unknown {
                target: receipt,
            })) if receipt == target => {
                return Err(format!(
                    "helper exact birth 停核结果未知：target={target:?}，保留 custody"
                ));
            }
            Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Mismatch {
                requested,
                current,
            })) if requested == target => {
                return Err(format!(
                    "helper exact birth 停核身份失配：requested={requested:?} current={current:?}"
                ));
            }
            Response::Ok(other) => {
                return Err(format!(
                    "helper exact birth 停核回执身份或类型失配：{other:?}"
                ));
            }
            Response::Err(error) => {
                return Err(format_helper_mutation_error(
                    client.platform(),
                    "停核",
                    &error,
                ));
            }
        }
    }
}

fn is_helper_slot_occupied(error: &polaris_helper_proto::Error) -> bool {
    use polaris_helper_proto::ErrorCode;
    matches!(
        error.code,
        ErrorCode::Auth | ErrorCode::Start | ErrorCode::Other
    ) && matches!(
        error.detail.as_str(),
        "slot-owned-by-other-user"
            | "slot-owned-or-control-busy"
            | "slot-owned-or-core-occupied"
            | "core-occupied"
    )
}

fn pinned_receipt_response(
    platform: Platform,
    start: polaris_helper_proto::core_protocol::PinnedBirthStart,
) -> Result<Response, String> {
    use polaris_helper_proto::core_protocol::PinnedBirthStart;
    match (platform, start) {
        (Platform::Linux, PinnedBirthStart::Linux(start)) => {
            Ok(Response::Ok(ResponseKind::LinuxBirthStart(start)))
        }
        (Platform::Mac | Platform::Win, PinnedBirthStart::Native(start)) => {
            Ok(Response::Ok(ResponseKind::NativeBirthStart(start)))
        }
        (
            Platform::Linux
            | Platform::Mac
            | Platform::Win
            | Platform::Android
            | Platform::Ios
            | Platform::Other,
            _,
        ) => Err("HELPER_CORE_MISMATCH: wrong receipt platform".into()),
    }
}

fn format_helper_mutation_error(
    platform: Platform,
    operation: &str,
    error: &polaris_helper_proto::Error,
) -> String {
    if is_helper_slot_occupied(error) {
        return "HELPER_CORE_OCCUPIED: helper resource is occupied by an existing core".into();
    }
    if matches!(platform, Platform::Linux | Platform::Mac | Platform::Win)
        && is_unknown_command_reply(error)
    {
        return format!(
            "已安装的 helper 不支持原生 birth 安全协议，已拒绝{operation}；请升级或修复 helper"
        );
    }
    format!("helper {operation}失败：{error}")
}

fn sleep_within_stop_budget(
    started: std::time::Instant,
    total_timeout: Duration,
    delay: Duration,
) -> Result<(), String> {
    let remaining = total_timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(format!("helper 停核通信超时：总预算 {total_timeout:?}"));
    }
    std::thread::sleep(delay.min(remaining));
    Ok(())
}

/// 应答是否是「不认识这条命令」：**不带尾文**的裸 `ERR unknown`。
///
/// 只比错误码不够。`unknown` 这个码在 wire 上有两种来源，只有第一种说明助手太旧：
///
/// - 命令名不在解码表或分派表里。三个平台的服务端此时写的都是裸的一行 `ERR unknown`
///   （Linux `dispatch_locked` 的兜底臂；macOS `decode_request` 报未知命令；Windows
///   `parse_request` 解不出）。
/// - 认识命令、但这一次出了别的事，带着尾文：Linux 处理器的锁中毒（`ERR unknown <锁错误>`）、
///   Linux 状态探测遇到认不出归属的 custody（`ERR unknown birth-custody-unidentified`）、
///   Windows 收到行数不足的帧（`ERR unknown bad-frame`），以及协议层把任意错误折成的
///   `ERR unknown <原文>`（`polaris_helper_proto::Error` 的 `From<Box<dyn Error>>`）。
///
/// 把第二种也当成「助手太旧」会让用户去重装一个没问题的助手，崩溃自愈也据此收手。
pub(crate) fn is_unknown_command_reply(error: &polaris_helper_proto::Error) -> bool {
    error.code == polaris_helper_proto::ErrorCode::Unknown && error.detail.trim().is_empty()
}

/// 起核前只读能力探测没通过的两种性质。
///
/// 分开是因为用户下一步要做的事不同：[`HelperOutdated`](Self::HelperOutdated) 重试多少次都一样，
/// 要去升级 / 重装提权助手；[`Unavailable`](Self::Unavailable) 可能只是这一刻不通。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BirthCapabilityError {
    /// 已装的助手对探测命令回了裸 `ERR unknown`：它早于本应用起核所用的协议。
    ///
    /// 这是「不认识这条命令」的**确定**回答，不是通信失败 —— 只有它能诚实地断言「助手太旧」
    /// （判据见 [`is_unknown_command_reply`]）。
    HelperOutdated(String),
    /// 其余：通信失败、既有 custody 未清、非预期应答、带尾文的 `ERR unknown`。
    Unavailable(String),
    SlotOccupied(String),
}

impl BirthCapabilityError {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::HelperOutdated(message)
            | Self::Unavailable(message)
            | Self::SlotOccupied(message) => message,
        }
    }
}

fn require_linux_birth_capability_with_client(
    client: &HelperClient,
) -> Result<(), BirthCapabilityError> {
    use BirthCapabilityError::{HelperOutdated, Unavailable};
    if client.platform() != Platform::Linux {
        return Err(Unavailable(
            "exact birth capability probe requires Linux".to_owned(),
        ));
    }
    match client.send_with_timeout(&Request::LinuxStatusBirth, Duration::from_millis(1500)) {
        Ok(Response::Ok(ResponseKind::LinuxBirthStatus(_))) => Ok(()),
        Ok(Response::Err(error)) if is_helper_slot_occupied(&error) => {
            Err(BirthCapabilityError::SlotOccupied(
                "HELPER_CORE_OCCUPIED: helper resource is occupied by an existing core".into(),
            ))
        }
        Ok(Response::Err(error)) if is_unknown_command_reply(&error) => Err(HelperOutdated(
            "已安装的 Linux helper 不支持 exact birth 安全协议；请升级或修复 helper 后重试"
                .to_owned(),
        )),
        Ok(Response::Err(error)) => Err(Unavailable(format!(
            "Linux helper exact birth 能力探测失败：{error}"
        ))),
        Ok(Response::Ok(other)) => Err(Unavailable(format!(
            "Linux helper exact birth 能力探测返回非预期响应：{other:?}"
        ))),
        Err(error) => Err(Unavailable(format!(
            "Linux helper exact birth 能力探测通信失败：{error}"
        ))),
    }
}

fn require_native_birth_capability_with_client(
    client: &HelperClient,
) -> Result<(), BirthCapabilityError> {
    use BirthCapabilityError::{HelperOutdated, Unavailable};
    // 三条不通过的腿共用同一句开头，日志里据此一眼认出是这道门。
    let unsupported = |detail: &dyn std::fmt::Debug| {
        format!(
            "已安装的 helper 未提供 native birth 安全能力；请升级或修复 helper 后重试：{detail:?}"
        )
    };
    match client.send_with_timeout(&Request::NativeStatusBirth, Duration::from_millis(1500)) {
        Ok(Response::Ok(ResponseKind::NativeBirthStatus(
            NativeBirthStatus::Empty | NativeBirthStatus::Running { .. },
        ))) => Ok(()),
        Ok(Response::Ok(ResponseKind::NativeBirthStatus(status))) => Err(Unavailable(format!(
            "helper native birth 起核未获准：既有 custody={status:?}，等待同birth native Stop后重试"
        ))),
        Ok(Response::Err(error)) if is_helper_slot_occupied(&error) => {
            Err(BirthCapabilityError::SlotOccupied(
                "HELPER_CORE_OCCUPIED: helper resource is occupied by an existing core".into(),
            ))
        }
        Ok(Response::Err(error)) if is_unknown_command_reply(&error) => {
            Err(HelperOutdated(unsupported(&error)))
        }
        // 认识探测命令、但这一次没答上来（带尾文的 `ERR unknown`、别的错误码）：不是能力缺失。
        Ok(Response::Err(error)) => Err(Unavailable(format!(
            "helper native birth 能力探测失败：{error}"
        ))),
        result => Err(Unavailable(unsupported(&result))),
    }
}

fn managed_core_status_with_client(client: &HelperClient) -> Result<ManagedCoreStatus, String> {
    let response = client
        .send(&if client.platform() == Platform::Linux {
            Request::LinuxStatusBirth
        } else if matches!(client.platform(), Platform::Mac | Platform::Win) {
            Request::NativeStatusBirth
        } else {
            Request::Status
        })
        .map_err(|error| format!("helper 受管核状态通信失败：{error}"))?;
    if client.platform() == Platform::Linux {
        return match response {
            Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Running { target })) => {
                Ok(ManagedCoreStatus::BirthRunning { target })
            }
            Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Stopping { target })) => {
                Ok(ManagedCoreStatus::BirthStopping { target })
            }
            Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Unknown { target })) => {
                Ok(ManagedCoreStatus::BirthUnknown { target })
            }
            Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Empty)) => {
                Ok(ManagedCoreStatus::BirthEmpty)
            }
            Response::Ok(other) => Err(format!("helper exact birth 状态返回非预期响应：{other:?}")),
            Response::Err(error) => Err(format!("helper exact birth 状态查询失败：{error}")),
        };
    }
    if matches!(client.platform(), Platform::Mac | Platform::Win) {
        return match response {
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Running {
                target,
                created,
                image,
            })) => Ok(ManagedCoreStatus::NativeBirthRunning {
                target,
                created,
                image,
            }),
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Stopping {
                target,
            })) => Ok(ManagedCoreStatus::BirthStopping { target }),
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Unknown {
                target: Some(target),
            })) => Ok(ManagedCoreStatus::BirthUnknown { target }),
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Unknown {
                target: None,
            })) => Ok(ManagedCoreStatus::BirthUnidentified),
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Empty)) => {
                Ok(ManagedCoreStatus::BirthEmpty)
            }
            other => Err(format!("helper native birth 状态未获有效回执：{other:?}")),
        };
    }

    match response {
        Response::Ok(ResponseKind::Status(CoreStatus::Running {
            pid,
            created,
            image,
        })) => Ok(ManagedCoreStatus::Running {
            pid,
            created,
            image,
        }),
        Response::Ok(ResponseKind::Status(CoreStatus::Stopped)) => Ok(ManagedCoreStatus::Stopped),
        Response::Ok(other) => Err(format!("helper 受管核状态返回非预期响应：{other:?}")),
        Response::Err(error) => Err(format!("helper 受管核状态查询失败：{error}")),
    }
}

#[cfg(any(test, target_os = "macos"))]
fn classify_mac_proxy_response(
    response: Response,
) -> Result<(), polaris_system_integration::proxy_ops::MacProxyWriterError> {
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    match response {
        Response::Ok(ResponseKind::MacProxyTransaction) => Ok(()),
        // 旧 helper 不识别新命令，且 unknown/auth 都发生在 dispatch 修改前。
        Response::Err(error)
            if matches!(
                error.code,
                polaris_helper_proto::ErrorCode::Unknown | polaris_helper_proto::ErrorCode::Auth
            ) =>
        {
            Err(MacProxyWriterError::Unavailable(error.to_string()))
        }
        Response::Err(error) => Err(MacProxyWriterError::Failed(error.to_string())),
        Response::Ok(other) => Err(MacProxyWriterError::Failed(format!(
            "helper 返回非预期响应：{other:?}"
        ))),
    }
}

#[cfg(any(test, target_os = "macos"))]
fn mac_proxy_compare_capable_with_client(
    client: &HelperClient,
) -> Result<bool, polaris_system_integration::proxy_ops::MacProxyWriterError> {
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    let response = client
        .send_with_timeout(
            &Request::MacProxyCompareCapability,
            HELPER_MAC_PROXY_TIMEOUT,
        )
        .map_err(classify_mac_proxy_client_error)?;
    match classify_mac_proxy_response(response) {
        Ok(()) => Ok(true),
        Err(MacProxyWriterError::Unavailable(_)) => Ok(false),
        Err(error @ MacProxyWriterError::Failed(_)) => Err(error),
    }
}

#[cfg(target_os = "macos")]
impl polaris_system_integration::proxy_ops::MacProxyTransactionWriter for HelperRuntime {
    fn available(&self) -> bool {
        // 只看安装证据，不连 socket、不尝试拉起服务、更不会触发安装授权。System 模式没有 helper
        // 是正常形态，应直接让 system-integration 走用户态 networksetup。
        self.manager().is_installed()
    }

    fn compare_capable(
        &self,
    ) -> Result<bool, polaris_system_integration::proxy_ops::MacProxyWriterError> {
        use polaris_system_integration::proxy_ops::MacProxyWriterError;

        let client = self
            .build_client()
            .map_err(MacProxyWriterError::Unavailable)?;
        mac_proxy_compare_capable_with_client(&client)
    }

    fn execute(
        &self,
        payload_hex: &str,
    ) -> Result<(), polaris_system_integration::proxy_ops::MacProxyWriterError> {
        use polaris_system_integration::proxy_ops::MacProxyWriterError;

        let client = self
            .build_client()
            .map_err(MacProxyWriterError::Unavailable)?;
        let response = client
            .send_with_timeout(
                &Request::MacProxyCompareTransaction {
                    payload_hex: payload_hex.to_owned(),
                },
                HELPER_MAC_PROXY_TIMEOUT,
            )
            .map_err(classify_mac_proxy_client_error)?;
        classify_mac_proxy_response(response)
    }
}

/// 把共享的 [`HelperRuntime`] 适配到 Linux resolved 生命周期控制器。
pub(crate) struct HelperLinuxResolvedOps(pub(crate) Arc<HelperRuntime>);

impl LinuxResolvedOps for HelperLinuxResolvedOps {
    fn takeover(&self) -> Result<(), String> {
        self.0.linux_resolved_takeover()
    }

    fn revert(&self) -> Result<(), String> {
        self.0.linux_resolved_revert()
    }
}

/// POSIX real UID for the App's staged payload and administrator CLI caller.
/// getuid has no error sentinel; a missing Linux procfs never becomes root.
#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn maintenance_utility() -> Result<MaintenanceUtility, String> {
    let src_binary = resolve_helper_binary()?;
    let file = std::fs::File::open(&src_binary).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("bundled maintenance utility is not a regular nonempty file".into());
    }
    let helper_sha256 = polaris_updater::verify::sha256_reader_hex(std::io::Read::take(
        file,
        metadata.len().saturating_add(1),
    ))
    .map_err(|e| e.to_string())?;
    Ok(MaintenanceUtility {
        src_binary,
        helper_sha256,
    })
}

/// 解析 bundled `polaris-helper` 二进制（镜像 [`resolve_core_binary`](crate::runtime::proxy::resolve_core_binary)
/// 的前缀/平台目录策略）。找不到 → Err（install 据此早返「二进制缺失」，**不触发提权**）。
///
/// # 信任级（L2：本函数的产物直接喂**提权安装链**）
///
/// `POLARIS_HELPER_PATH` 逃生门在 **release** 侧只认 app 自有数据目录
/// （[`crate::runtime::env_trust`]），随包资源目录也不接受。理由是后果：这个路径随后被以
/// 管理员 / root 权限装成系统服务（systemd unit / launchd plist / Windows 服务），
/// 得到的是一个开机自启的 root 级常驻进程；而随包 helper 本就由下面的兜底腿解析得到，
/// 逃生门再指一次只会多一个入口、不多一分能力。
/// 判据不过 ⇒ 记 `ENV_PATH_UNTRUSTED` 并回落兜底腿，**绝不静默**。
/// 开发态（debug / test）逃生门仍是原样的第一优先级 —— helper 的本地迭代靠它装自编产物。
pub(super) fn resolve_helper_binary() -> Result<PathBuf, String> {
    let filename = if cfg!(windows) {
        "polaris-helper.exe"
    } else {
        "polaris-helper"
    };
    if let Some(helper) = crate::runtime::env_trust::adopt_trusted_env_path(
        "POLARIS_HELPER_PATH",
        // 名字必须以**字面量**留在 `env::var(` 调用处（见 `release_escape_hatches` 探测器①）。
        std::env::var("POLARIS_HELPER_PATH").ok(),
    )? {
        return Ok(helper);
    }
    let platform_dirs: &[&str] = if cfg!(target_os = "macos") {
        &["mac-arm64", "mac-x64"]
    } else if cfg!(windows) {
        &["win"]
    } else {
        &["linux"]
    };
    let selected_dirs = crate::runtime::core_identity::selected_payload_dirs(platform_dirs)?;
    let platform_dirs: Vec<&str> = selected_dirs.iter().map(String::as_str).collect();
    // 与 resolve_core_binary 共用同一布局兜底（含 macOS `_up_` 真实路径），钉在 proxy.rs 的纯函数里。
    let exe = std::env::current_exe().ok();
    let candidates = crate::runtime::proxy::bundle_resource_candidates(
        exe.as_deref().and_then(std::path::Path::parent),
        crate::runtime::proxy::dev_manifest_dir(),
        &platform_dirs,
        filename,
    );
    if let Some(helper) = crate::runtime::proxy::first_existing_bundle_candidate(&candidates) {
        return Ok(helper);
    }
    Err(format!(
        "未找到 polaris-helper 二进制（尝试过：{}）",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    ))
}

#[cfg(test)]
mod tests;

fn query_core_status(
    client: &HelperClient,
    id: &polaris_helper_proto::core_manifest::CoreId,
) -> Result<polaris_helper_proto::core_protocol::CoreStatus, String> {
    use polaris_helper_proto::core_protocol::CoreStatus;
    match client.send_with_timeout(
        &Request::CoreStatus { id: id.clone() },
        Duration::from_secs(5),
    ) {
        Ok(Response::Ok(ResponseKind::CoreStatus(status))) => {
            let reported = match &status {
                CoreStatus::Present { id, .. }
                | CoreStatus::Absent { id }
                | CoreStatus::Invalid { id, .. } => id,
            };
            if reported != id {
                return Err("HELPER_CORE_MISMATCH: status content id differs".into());
            }
            Ok(status)
        }
        _ => Err("HELPER_CORE_MISMATCH: pinned content status unavailable; update helper".into()),
    }
}

//! 更新运行时：把 `polaris-updater` 纯逻辑 crate 装配为持有真实 I/O 的运行时实例。
//!
//! 装配内容：
//!  - **随包内核清单**（`core-manifest.json`，编译期嵌入）：配套内核版本与补丁集标识。
//!  - **现役内核版本双读法**（[`UpdaterRuntime::read_core_version_line`] /
//!    [`UpdaterRuntime::read_core_version`]）—— 两者失败语义刻意不对称，见下。
//!  - **应用更新状态持久化**（`update-state.json`，原子写 tmp+rename）。
//!  - **mini 更新弹窗会话**（`updater::popup::PopupSession` + Tauri 窗口 transport）。
//!
//! 内核本身不在这里更新：应用只从安装包解析内核，应用内没有更换内核的入口。
//!
//! # 双读法的不对称
//!
//! | 函数 | 探测失败时 | 用途 |
//! |---|---|---|
//! | `read_core_version` | **回落配套版本** | 关于页那一格展示 |
//! | `read_core_version_line` | **返回 `""`** | 需要区分「读到了」与「读不到」的地方（内核信息卡） |
//!
//! 一次 spawn 失败在 `read_core_version` 眼里长得和「现役内核就是配套版本」一模一样，所以任何
//! 要把读数当证据用的地方只能用失败置空的那一个。单测 `core_version_readers_are_asymmetric`
//! 钉着这条不对称。
//!
//! # 核二进制路径
//!
//! 解析的单一真值是 [`crate::runtime::proxy::resolve_core_binary`]，本模块不复制第二份，经
//! [`UpdaterRuntime::with_core_binary`] 由 `lib.rs` 启动期注入。未注入时版本读取如实返回空串，
//! 不猜、不谎报。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use polaris_updater::extract_version_token;
use polaris_updater::popup::{PopupSession, UpdatePopupState};
use serde::{Deserialize, Serialize};

use crate::runtime::update_popup::TauriPopupTransport;

/// `core-manifest.json` 的编译期嵌入。
///
/// 清单是**构建期生成**的常量（随包内核的版本与来源），故编译期嵌入语义正确，
/// 且免去运行期 resource 路径解析的一整类失败。
const CORE_MANIFEST_JSON: &str = include_str!("../../core-manifest.json");

/// 随包内核清单（只取本模块需要的字段）。
#[derive(Debug, Clone, Deserialize)]
struct CoreManifest {
    #[serde(rename = "bundledCoreVersion")]
    bundled_core_version: String,
    #[serde(rename = "windowsBuild")]
    windows_build: Option<WindowsBuild>,
    #[serde(rename = "sourceBuild")]
    source_build: Option<serde_json::Value>,
    // Preserve presence, including null/arrays, without rejecting mobile parsing.
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct WindowsBuild {
    version: String,
}

/// 解析编译期嵌入的 `core-manifest.json`，取配套内核版本。
///
/// 解析失败 → 回落空串（**不 panic**：清单损坏不该让整个 App 起不来）。
fn bundled_core_version() -> String {
    bundled_core_version_from_manifest(
        CORE_MANIFEST_JSON,
        cfg!(target_os = "windows"),
        cfg!(target_os = "android"),
    )
    .unwrap_or_else(|e| {
        log::error!("core-manifest.json 解析失败 {e}：配套内核版本未知");
        String::new()
    })
}

/// **桌面**补丁集标识：显式 `desktopSourceBuild` 或旧 `sourceBuild` 的 `patchedSourceTree`。
/// 显式桌面块无效时返回 `None`，不会借用旧移动端图的标识。
///
/// 只在清单被认作冻结清单时给出（与 [`bundled_core_version_from_manifest`] 取 `sourceBuild.version`
/// 用同一个判据 [`frozen_source_build_version`]）：清单不完整时版本回落到上游基线，那时再给一个
/// 补丁集标识等于为一个没被确认的构建背书。移动端（Android、iOS）的内核各走各的补丁队列，
/// 不来自这份桌面源码清单，恒 `None`。
fn bundled_core_patch_set_from_manifest(raw: &str, mobile: bool) -> Option<String> {
    if mobile {
        return None;
    }
    let manifest = serde_json::from_str::<CoreManifest>(raw).ok()?;
    let source = if let Some(source) = manifest.extra.get("desktopSourceBuild") {
        frozen_desktop_source_version(
            &manifest,
            source,
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()?
                .get("windowsBuild")
                .is_some(),
        )?;
        source
    } else {
        let source = manifest.source_build.as_ref()?;
        if source.get("sourceMode").is_some() {
            return None;
        }
        frozen_source_build_version(source, &manifest.bundled_core_version)?;
        source
    };
    source["patchedSourceTree"].as_str().map(str::to_owned)
}

// Match the desktop JS inputs-only gate. Output hashes are first computed by
// native producers, so they are not prerequisites for the compiled baseline.
// Legacy null/partial inputs keep the existing baseline; a present desktop
// contract is selected separately and invalid inputs never fall back.
fn frozen_source_build_version<'a>(
    source: &'a serde_json::Value,
    upstream: &str,
) -> Option<&'a str> {
    fn valid_module(module: &str) -> bool {
        !module.is_empty()
            && module
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
    }
    fn module_partition<'a>(
        policy: &'a serde_json::Value,
        inventory: &std::collections::BTreeSet<&str>,
        absent_field: &str,
    ) -> Option<std::collections::BTreeSet<&'a str>> {
        let policy = policy.as_object()?;
        if policy.len() != 2 {
            return None;
        }
        let required = policy.get("requiredLinked")?.as_array()?;
        let absent = policy.get(absent_field)?.as_array()?;
        let mut classified = std::collections::BTreeSet::new();
        for module in required.iter().chain(absent) {
            let module = module.as_str()?;
            if !valid_module(module) || !classified.insert(module) {
                return None;
            }
        }
        if &classified != inventory {
            return None;
        }
        absent.iter().map(|module| module.as_str()).collect()
    }
    let hex = |value: Option<&str>, length| {
        value.is_some_and(|value| {
            value.len() == length
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    };
    for field in [
        "sourceManifestSha256",
        "provisionerSha256",
        "sourceReceiptFingerprint",
        "moduleGraphSha256",
    ] {
        if !hex(source[field].as_str(), 64) {
            return None;
        }
    }
    for field in ["patchedSourceTree", "buildTree"] {
        if !hex(source[field].as_str(), 40) {
            return None;
        }
    }
    let modules = source["dependencyModules"].as_array()?;
    let mut unique = std::collections::BTreeSet::new();
    for module in modules {
        let module = module.as_str()?;
        if !valid_module(module) || !unique.insert(module) {
            return None;
        }
    }
    let transport = source["transportPins"].as_object()?;
    if transport.is_empty() {
        return None;
    }
    for (module, version) in transport {
        let version = version.as_str()?.strip_prefix('v')?;
        if !valid_module(module)
            || version.is_empty()
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
        {
            return None;
        }
    }
    let platforms = source["platforms"].as_object()?;
    if platforms.len() != 4 {
        return None;
    }
    let transport_inventory = transport.keys().map(String::as_str).collect();
    for key in ["linux", "win", "mac-x64", "mac-arm64"] {
        let platform = platforms.get(key)?;
        if !hex(platform["buildTree"].as_str(), 40) {
            return None;
        }
        let allowed_absent =
            module_partition(&platform["patchedModules"], &unique, "allowedAbsent")?;
        if !allowed_absent.is_empty() {
            return None;
        }
        module_partition(
            &platform["transportModules"],
            &transport_inventory,
            "confirmedAbsent",
        )?;
    }
    let version = source["version"].as_str()?;
    let suffix = version.strip_prefix(&format!("{upstream}.polaris."))?;
    if suffix
        .as_bytes()
        .first()
        .is_some_and(|first| (b'1'..=b'9').contains(first))
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        Some(version)
    } else {
        None
    }
}

// Inputs-only desktop admission mirrors validateSourcePins in source-graph.mjs.
// A malformed present block withholds both version and patch identity.
fn frozen_desktop_source_version<'a>(
    manifest: &CoreManifest,
    source: &'a serde_json::Value,
    windows_overlay_present: bool,
) -> Option<&'a str> {
    let upstream = source["upstreamVersion"].as_str()?;
    if windows_overlay_present
        || manifest.windows_build.is_some()
        || source["sourceMode"].as_str()? != "fork-commit-v1"
        || upstream.is_empty()
        || !upstream.as_bytes()[0].is_ascii_digit()
        || !upstream
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
        || source["graphScope"].as_str()? != "core-source-only"
        || !source["dependencyModules"].as_array()?.is_empty()
        || source["buildTree"] != source["patchedSourceTree"]
    {
        return None;
    }
    for field in ["mainGoModSha256", "mainGoSumSha256"] {
        let value = source[field].as_str()?;
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
    }
    for platform in source["platforms"].as_object()?.values() {
        if platform["buildTree"] != source["buildTree"] {
            return None;
        }
    }
    frozen_source_build_version(source, upstream)
}

fn bundled_core_version_from_manifest(
    raw: &str,
    windows: bool,
    android: bool,
) -> Result<String, serde_json::Error> {
    bundled_core_version_with_ios(raw, windows, android, cfg!(target_os = "ios"))
}

fn bundled_core_version_with_ios(
    raw: &str,
    windows: bool,
    android: bool,
    ios: bool,
) -> Result<String, serde_json::Error> {
    let manifest = serde_json::from_str::<CoreManifest>(raw)?;
    if android {
        return Ok(manifest.bundled_core_version);
    }
    if !ios {
        if let Some(source) = manifest.extra.get("desktopSourceBuild") {
            return Ok(frozen_desktop_source_version(
                &manifest,
                source,
                serde_json::from_str::<serde_json::Value>(raw)?
                    .get("windowsBuild")
                    .is_some(),
            )
            .map_or_else(String::new, str::to_owned));
        }
    }
    if let Some(version) = manifest.source_build.as_ref().and_then(|source| {
        if source.get("sourceMode").is_some() {
            return None;
        }
        frozen_source_build_version(source, &manifest.bundled_core_version)
    }) {
        return Ok(version.to_owned());
    }
    Ok(if windows {
        manifest
            .windows_build
            .map_or(manifest.bundled_core_version, |build| build.version)
    } else {
        manifest.bundled_core_version
    })
}

/// 持久化的更新状态（`<config_dir>/update-state.json`）。
///
/// 旧版本在同一文件里还写过内核自动更新的字段（暂存记录、跨带提示、版本变更通知）；读取时
/// 它们被忽略，下一次写回即从盘上消失。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateStateFile {
    /// 用户「跳过此版本」的 App 版本号（= 上游 `UPDATE_SKIP`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_version: Option<String>,
    /// Windows 便携版：应用为了让用户手动覆盖而退出时留下的那两个位置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_portable_update: Option<PendingPortableUpdate>,
}

/// 便携版手动交接的两个位置及来源/目标版本；记录不等于安装完成证据。
///
/// 应用退出之后，写着这两个路径的那张卡片就不在了；而系统「打开文件夹」是否真的弹出了窗口，
/// 应用这边看不出来。留下这条记录，用户重开应用时卡片回到原处。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPortableUpdate {
    /// 更新压缩包的位置。
    pub archive: String,
    /// 程序所在目录（覆盖解压的目标）。
    pub program_dir: String,
    /// 留下记录时正在运行的应用版本。
    pub from_version: String,
    /// 正式资产名中的目标版本；旧记录缺此字段时，完成情况保持未确认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
}

/// 重启后能观察到的交接状态；没有「已安装」结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableHandoffStatus {
    AwaitingTargetVersion,
    CompletionUnverified,
}

impl PendingPortableUpdate {
    /// 目标版本已运行或压缩包不可继续使用时，不恢复交接卡。
    /// 同版本修复、旧记录和意外版本变化均无法证明是否覆盖，保持「完成未确认」。
    #[must_use]
    pub fn handoff_status(
        &self,
        running_version: &str,
        archive_exists: bool,
    ) -> Option<PortableHandoffStatus> {
        if !archive_exists {
            return None;
        }
        match self.target_version.as_deref() {
            Some(target) if target == running_version && target != self.from_version => None,
            Some(target) if target != self.from_version && running_version == self.from_version => {
                Some(PortableHandoffStatus::AwaitingTargetVersion)
            }
            _ => Some(PortableHandoffStatus::CompletionUnverified),
        }
    }
}

impl UpdateStateFile {
    /// 用户显式重新检查时清除交接提示；不改安装状态，不删除压缩包。
    pub fn clear_portable_handoff(&mut self) {
        self.pending_portable_update = None;
    }
}

/// 更新运行时。
pub struct UpdaterRuntime {
    /// 状态文件路径（`<config_dir>/update-state.json`）。
    state_path: PathBuf,
    /// 配套内核版本（编译期自 `core-manifest.json`）。
    bundled_core_version: String,
    /// 补丁集标识（见 [`bundled_core_patch_set_from_manifest`]）。
    bundled_core_patch_set: Option<String>,
    /// 核二进制路径（**注入**；None = 未注入，版本读取如实报未知）。
    core_binary: Mutex<Option<PathBuf>>,
    /// mini 更新弹窗会话（None = 弹窗未开）。
    popup: Mutex<Option<PopupSession<TauriPopupTransport>>>,
    /// 本次进程发出的**最后一帧** App 更新进度（`update:progress` 的原样载荷）。
    ///
    /// 设置页的更新卡状态全在组件本地 state 里，初值 idle。窗口销毁重建 / 切页 / 轻量模式回收之后
    /// 它对在途下载一无所知，只能等下一帧事件 —— 而下载**已经下完**时那一帧永不再来，卡片就永远
    /// 停在「检查更新」。下载本身跑在 `spawn_blocking`，窗口没了照跑，故「进度还在、UI 却不知道」
    /// 是常态而非边角；弹窗侧的 `popup` 槽兜不住它（`push_popup_state_inner` 在弹窗未开时直接返回，
    /// 用户只在设置页操作的那条路径上一帧都不留）。
    ///
    /// **只在内存、不落盘。** 进程重启后在途残件已被 `PartialDownload` 的 RAII Drop 清掉，成品则会被
    /// `cached_download_is_reusable` 那条复用腿重新认出来 —— 两种情形都不需要这份快照。而快照若落盘，
    /// 重启后 UI 会照着它说「下载中 47%」，此刻却没有任何下载在跑：那是拿一个假状态换掉一个真状态。
    last_progress: Mutex<Option<serde_json::Value>>,
    /// 内存态状态缓存（避免每次读盘；写时同步落盘）。
    state: Mutex<UpdateStateFile>,
}

impl UpdaterRuntime {
    /// 装配（`AppRuntime::new` 调用一次）。
    ///
    /// 启动即读一次状态文件；读失败（首次启动/损坏）→ 默认空状态（**不 panic**）。
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        let state_path = config_dir.join("update-state.json");
        let state = load_state_file(&state_path);
        Self {
            state_path,
            bundled_core_version: bundled_core_version(),
            bundled_core_patch_set: bundled_core_patch_set_from_manifest(
                CORE_MANIFEST_JSON,
                cfg!(any(target_os = "android", target_os = "ios")),
            ),
            // 路径解析归 `proxy::resolve_core_binary` 一份实现，`lib.rs` 启动期经
            // `with_core_binary` 注入；这里不自己找。
            core_binary: Mutex::new(None),
            popup: Mutex::new(None),
            last_progress: Mutex::new(None),
            state: Mutex::new(state),
        }
    }

    /// 注入核二进制路径（单一真值在 `proxy.rs`，此处只接收）。
    pub fn with_core_binary(&self, path: PathBuf) {
        if let Ok(mut g) = self.core_binary.lock() {
            *g = Some(path);
        }
    }

    /// 配套内核版本：本应用版本随包的那个内核构建。
    #[must_use]
    pub fn bundled_core_version(&self) -> &str {
        &self.bundled_core_version
    }

    /// 桌面补丁集标识（移动端、清单不含或未被认作冻结清单 → `None`）。
    #[must_use]
    pub fn bundled_core_patch_set(&self) -> Option<&str> {
        self.bundled_core_patch_set.as_deref()
    }

    // ── 活核版本双读法（**不对称失败语义**，见模块文档「双读法陷阱」）──

    /// 读现役内核 `sing-box version` 的**原始第一行**；**探测失败返回空串**。
    ///
    /// 绝不回落配套版本 —— 那会把「读不到」伪装成「就是配套版本」。
    #[must_use]
    pub fn read_core_version_line(&self) -> String {
        let Some(bin) = self.core_binary_path() else {
            // 未注入路径 = 探测不可能成功 → 空串（诚实失败，不猜）。
            return String::new();
        };
        match crate::runtime::win_console::no_console_window(Command::new(&bin).arg("version"))
            .output()
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .split('\n')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
            // 非零退出 / spawn 失败 → 空串（**不回落基线**）。
            Ok(out) => {
                log::warn!("sing-box version 非零退出 {:?}：版本行置空", out.status);
                String::new()
            }
            Err(e) => {
                log::warn!("sing-box version spawn 失败 {e}：版本行置空");
                String::new()
            }
        }
    }

    /// 读现役内核版本 token；**探测失败回落配套版本**。
    ///
    /// 回落语义使「探测失败」与「现役内核就是配套版本」不可区分，故本函数只供展示；要把读数
    /// 当证据用的地方必须用 [`Self::read_core_version_line`]。
    #[must_use]
    pub fn read_core_version(&self) -> String {
        let line = self.read_core_version_line();
        let tok = extract_version_token(&line);
        if tok.is_empty() {
            // 回落配套版本（调用点须自觉，见文档）。
            return self.bundled_core_version.clone();
        }
        tok
    }

    fn core_binary_path(&self) -> Option<PathBuf> {
        self.core_binary.lock().ok().and_then(|g| g.clone())
    }

    // ── 状态持久化 ──

    /// 读当前状态（内存缓存快照）。
    #[must_use]
    pub fn state(&self) -> UpdateStateFile {
        self.state.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 改状态并落盘（原子写 tmp+rename；= 上游 `saveAutoState` 的 merge + 原子写）。
    ///
    /// # Errors
    ///
    /// 落盘失败（磁盘满/权限）。**内存态已更新**（对齐上游 best-effort：落盘失败不回滚内存）。
    pub fn mutate_state<F: FnOnce(&mut UpdateStateFile)>(&self, f: F) -> Result<(), String> {
        let mut g = self
            .state
            .lock()
            .map_err(|e| format!("state 锁中毒: {e}"))?;
        f(&mut g);
        // 串行落盘，避免较旧快照在提示清除后重新覆盖状态文件。
        save_state_file(&self.state_path, &g)
    }

    /// 显式清除便携版提示：落盘成功后才更新内存；不删除 ZIP 或宣称安装成功。
    ///
    /// # Errors
    ///
    /// 锁或落盘失败时保留原内存记录，调用方须向用户报告失败。
    pub fn clear_portable_handoff(&self) -> Result<(), String> {
        let mut g = self
            .state
            .lock()
            .map_err(|e| format!("state 锁中毒: {e}"))?;
        if g.pending_portable_update.is_none() {
            return Ok(());
        }
        let mut next = g.clone();
        next.clear_portable_handoff();
        save_state_file(&self.state_path, &next)?;
        *g = next;
        Ok(())
    }

    // ── 弹窗会话 ──

    /// 弹窗会话（供 `update_popup` 模块建窗/推状态）。
    #[must_use]
    pub fn popup(&self) -> &Mutex<Option<PopupSession<TauriPopupTransport>>> {
        &self.popup
    }

    /// 当前弹窗状态（= 上游 `UPDATE_POPUP_STATE` 的主→弹窗载荷读取端）。
    ///
    /// 返回 `None` = 弹窗未开（**不编造 idle 态**：弹窗不存在与弹窗处于某态是两回事）。
    #[must_use]
    pub fn popup_state(&self) -> Option<UpdatePopupState> {
        self.popup
            .lock()
            .ok()?
            .as_ref()
            .and_then(|s| s.last_state().cloned())
    }

    // ── 最后一帧进度快照（成因、以及为什么不落盘，见 `last_progress` 字段文档）──

    /// 记下最后一帧进度。**终态帧（`downloaded` / `error`）同样留着，不清空** —— 用户切走再切回来
    /// 时要看到的正是「下载完成了」，清空只会把卡片打回 idle，那恰恰是本槽要修的那个缺陷本身。
    pub fn set_last_progress(&self, payload: serde_json::Value) {
        if let Ok(mut g) = self.last_progress.lock() {
            *g = Some(payload);
        }
    }

    /// 最后一帧进度快照。
    ///
    /// 返回 `None` = 本次进程一帧都没发过（**不编造 idle 帧**：没发过与「处于 idle 态」是两回事，
    /// 同 [`Self::popup_state`] 的理由）。
    #[must_use]
    pub fn last_progress(&self) -> Option<serde_json::Value> {
        self.last_progress.lock().ok()?.clone()
    }
}

/// 读状态文件；不存在/损坏 → 默认空态（首次启动即此路径）。
fn load_state_file(path: &Path) -> UpdateStateFile {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            // 损坏不该让更新域整个瘫掉；如实告警后按空态继续（用户最多丢一次 skip 记录）。
            log::warn!("update-state.json 解析失败 {e}：按空状态继续");
            UpdateStateFile::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UpdateStateFile::default(),
        Err(e) => {
            log::warn!("update-state.json 读取失败 {e}：按空状态继续");
            UpdateStateFile::default()
        }
    }
}

/// 原子写状态文件（tmp → rename；对齐 上游 `CoreUpdateStateStore` 的原子写）。
///
/// rename 同目录为原子 syscall：崩在半路也不会留下截断的 JSON（读到半截 JSON 会让状态静默归零）。
fn save_state_file(path: &Path, state: &UpdateStateFile) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("序列化更新状态失败: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败 {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .map_err(|e| format!("写临时状态文件失败 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // rename 失败 → 清残件，避免 .tmp 堆积。
        let _ = std::fs::remove_file(&tmp);
        format!("原子替换状态文件失败 {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests;

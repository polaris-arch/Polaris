//! GitHub release JSON → 更新检查结果的**纯逻辑**转换 + 平台/架构资产选择 + 更新源常量。
//!
//! ## 移植来源（上游 TS → Rust 纯逻辑，逐字保留行为）
//!
//! - `UpdateService.fetchReleases`（`UpdateService.ts:843`）+ `UpdateService.checkForUpdate`
//!   （`:146-240`）：拉 `api.github.com/repos/{owner}/{repo}/releases` → 过滤 prerelease → 按
//!   `published_at` 降序取最新 → 去 `v` 前缀 → `compareSemver` 判新 → `findSuitableAsset` 挑平台包
//!   → 组装 `UpdateInfo`。本模块把**除网络获取以外**的全部逻辑抽成纯函数 [`check_app_update`]
//!   （吃已拉回的 JSON 字节，宿主注入 HTTP），可 mock 单测。
//! - `update-asset.findSuitableUpdateAsset`（`update-asset.ts`，52 行）：App 安装包资产选择
//!   （每平台 loose/installed 双形态消歧，#72）→ [`find_suitable_update_asset`]。
//! - 更新源仓库常量 → [`APP_UPDATE_REPO`]。
//!
//! ## 为什么平台/架构是**参数**而非读 `process`
//!
//! 与 上游的 `findSuitable*` 同纪律：平台/架构由调用方注入（上游 传 `process.platform/arch`，
//! 本仓宿主传 `std::env::consts::OS/ARCH` 经 [`AssetPlatform::from_os`] / [`AssetArch::from_arch`]
//! 映射）。如此本函数**不读全局态**，可用平台真值表全覆盖单测。

use serde::{Deserialize, Serialize};

use crate::manifest::ManifestError;
use crate::version::compare_semver;

// ── 更新源仓库常量（单点定义，宿主不得自造第二份）────────────────────────────────

/// App 自更新源仓库 `(owner, repo)`（= 上游 `GITHUB_OWNER/GITHUB_REPO` 的 Polaris 对应）。
pub const APP_UPDATE_REPO: (&str, &str) = ("polaris-arch", "Polaris");

/// Public release asset suffixes; producers and packaging gates use the same contract.
pub const PORTABLE_ZIP_SUFFIX: &str = "x64-win-Portable.zip";
pub const ANDROID_APK_SUFFIX: &str = "arm64-v8a-android.apk";
pub const ANDROID_ARMV7_APK_SUFFIX: &str = "armeabi-v7a-android.apk";
pub const ANDROID_UNIVERSAL_APK_SUFFIX: &str = "universal-android.apk";

fn release_asset_matches(name: &str, suffix: &str) -> bool {
    let Some(version) = name
        .strip_prefix("Polaris_")
        .and_then(|rest| rest.strip_suffix(suffix))
        .and_then(|rest| rest.strip_suffix('_'))
    else {
        return false;
    };
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(core, pre)| (core, Some(pre)));
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && prerelease.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// 正式 Windows 便携资产的版本；与选包侧复用同一个发布命名判据。
/// 安装计划和重启后的手动交接记录不得再使用旧 `polaris-portable-` 前缀。
#[must_use]
pub fn portable_zip_version(name: &str) -> Option<&str> {
    if !release_asset_matches(name, PORTABLE_ZIP_SUFFIX) {
        return None;
    }
    name.strip_prefix("Polaris_")?
        .strip_suffix(PORTABLE_ZIP_SUFFIX)?
        .strip_suffix('_')
}

/// 构造 GitHub releases API URL（= 上游 `https://api.github.com/repos/${owner}/${repo}/releases`）。
#[must_use]
pub fn github_releases_api_url(owner: &str, repo: &str) -> String {
    format!("https://api.github.com/repos/{owner}/{repo}/releases")
}

// ── 目标平台 / 架构（对齐 NodeJS.Platform / process.arch 的分支面）──────────────────

/// 目标平台（上游三分支 `win32`/`darwin`/`linux`，本仓多一态 [`Android`](AssetPlatform::Android)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPlatform {
    Windows,
    Macos,
    Linux,
    /// Android（上游没有这一态：上游 是桌面 Electron 应用）。
    ///
    /// 只发布 ARMv8 / ARMv7；x86 / x86_64 不发包，也不进入 universal。
    Android,
}

/// 目标架构（`x64` / `arm64` / `armv7`；其余归 [`Other`](AssetArch::Other)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetArch {
    X64,
    Arm64,
    Armv7,
    Other,
}

impl AssetPlatform {
    /// 从 `std::env::consts::OS` 映射（宿主注入真实平台）。`None` = 本仓不为它发包的平台
    /// （iOS、各 BSD……）—— 那些平台上整条自更新腿没有对象。
    #[must_use]
    pub fn from_os(os: &str) -> Option<Self> {
        match os {
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

impl AssetArch {
    /// 从 `std::env::consts::ARCH` 映射（`x86_64` → X64、`aarch64` → Arm64、`arm` → Armv7，其余 → Other）。
    #[must_use]
    pub fn from_arch(arch: &str) -> Self {
        match arch {
            "x86_64" => Self::X64,
            "aarch64" => Self::Arm64,
            "arm" => Self::Armv7,
            _ => Self::Other,
        }
    }
}

// ── GitHub release JSON 形状（只取本模块需要的字段）───────────────────────────────

/// GitHub release 资产（`assets[]` 元素）。
#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    /// 资产文件名（平台/架构/形态的判据来源）。
    pub name: String,
    /// 直下 URL（= 上游 `asset.browser_download_url`）。
    #[serde(rename = "browser_download_url", default)]
    pub browser_download_url: String,
    /// 字节大小（= 上游 `asset.size`；缺失按 0）。
    #[serde(default)]
    pub size: u64,
    /// GitHub 给出的内容摘要，形如 `sha256:ab12…`（较新的 REST API 字段；旧 release 可能缺失）。
    ///
    /// **供应链增强（上游 没有这一层）**：下载件按它做 sha256 强校验，补上「HTTPS 只保传输、
    /// 镜像回退把信任面扩到 gh-proxy 运营方」的洞。信任根仍是 GitHub（摘要与资产同源），
    /// 故**不需要自建密钥**——防的是截断/镜像投毒，不是防 GitHub 本身。
    /// 缺失时回落 Content-Length 完整性校验（= 上游 基线），**不因缺摘要就拒绝更新**。
    #[serde(default)]
    pub digest: Option<String>,
}

/// GitHub release（`/releases` 数组元素）。
#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    /// 版本 tag（如 `v0.2.0`；= 上游 `tag_name`）。
    pub tag_name: String,
    /// release 标题（= 上游 `name`；缺省回落 tag）。
    #[serde(default)]
    pub name: Option<String>,
    /// 发布说明正文（= 上游 `body`）。
    #[serde(default)]
    pub body: Option<String>,
    /// 是否预览版（= 上游 `prerelease`）。
    #[serde(default)]
    pub prerelease: bool,
    /// 发布时间（RFC3339 UTC，如 `2024-05-01T12:00:00Z`；= 上游 `published_at`）。
    #[serde(default)]
    pub published_at: Option<String>,
    /// release 资产列表（= 上游 `assets`）。
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

// ── App 自更新检查结果（移植 UpdateService 的 UpdateInfo / UpdateCheckResult）──────────

/// App 更新信息（= 上游 `UpdateInfo`；字段名 camelCase，与 `ui` 的 `UpdateInfo` 契约逐字对齐）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    /// 原始 tag（**含 `v`**，= 上游 `UpdateInfo.version = latestRelease.tag_name`）。
    pub version: String,
    /// 标题（= 上游 `name || tag_name`）。
    pub title: String,
    /// 发布说明（= 上游 `body || ''`）。
    pub release_notes: String,
    /// 下载 URL（选中资产的 `browser_download_url`）。
    pub download_url: String,
    /// 文件大小（选中资产的 `size`）。
    pub file_size: u64,
    /// 发布时间。
    pub published_at: String,
    /// 是否预览版。
    pub is_prerelease: bool,
    /// 资产文件名。
    pub file_name: String,
    /// 选中资产的期望 sha256（由 GitHub `digest` 字段解析；旧 release 无该字段 → `None`）。
    ///
    /// 下载侧据此做强校验（**上游 没有这一层**）。`None` 时回落 Content-Length 完整性校验，
    /// **不因缺摘要就拒绝更新**——否则所有旧 release 都会更新不了。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// App 更新检查结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUpdateCheck {
    /// 无可用更新（已最新 / 无正式版 / 已跳过此版本 / 无适配平台资产）。
    NoUpdate,
    /// 有可用更新。
    Available(AppUpdateInfo),
}

/// GitHub release asset 的 `digest` 字段 → 裸 sha256 hex（**纯函数**）。
///
/// GitHub 返回形如 `sha256:ab12…`。非 sha256 算法（未来若加 blake3 之类）一律返 `None`
/// —— **绝不把不认识的摘要当 sha256 喂进 [`verify_hex_digest`](crate::verify::verify_hex_digest)**：
/// 那会必然 mismatch，把「本地不支持该摘要算法」伪装成「下载件被篡改」，成因错位。
#[must_use]
pub fn parse_asset_digest(digest: &str) -> Option<String> {
    let hex = digest.trim().strip_prefix("sha256:")?;
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

/// 去 tag 的前导 `v`（= 上游 `tag_name.replace(/^v/, '')`——仅小写 `v`，仅前导一处）。
///
/// `pub` 不是给外部自由用：[`check_app_update`] 拿它算比较侧（`strip_v(tag)`），而「跳过此版本」
/// 的**存储侧**必须用同一个函数归一化（W8）——两侧各自实现一份就是「v0.2.0 存进去、0.2.0 比
/// 出来，永不相等」的原发病理。导出它是为了让写点复用同一真值。
pub fn strip_v(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// 挑更新目标 release：过滤 prerelease 后按 `published_at` **降序取最新**
/// （= 上游 `validReleases.sort((a,b) => dateB - dateA)[0]`）。
///
/// `published_at` 为 RFC3339 UTC（固定宽度、恒带 `Z`），字典序 == 时间序，故直接比字符串等价于
/// 上游的 `new Date(...).getTime()` 比较，且零依赖、无需日期解析。缺失 `published_at`（草稿等）
/// 按空串处理（排到最旧，仅当它是唯一候选才被选中——对齐 上游 `new Date(null)=epoch0` 沉底）。
fn select_update_release(
    releases: &[GithubRelease],
    include_prerelease: bool,
) -> Option<&GithubRelease> {
    releases
        .iter()
        .filter(|r| include_prerelease || !r.prerelease)
        .max_by(|a, b| {
            a.published_at
                .as_deref()
                .unwrap_or("")
                .cmp(b.published_at.as_deref().unwrap_or(""))
        })
}

/// 检查 App 更新（移植 `UpdateService.checkForUpdate` 的纯逻辑部分）。
///
/// 步骤（逐字对齐 `UpdateService.ts:151-212`）：
///  1. 解析 releases JSON（网络获取由宿主注入，本函数只吃已拉回的字节 → 纯逻辑可单测）。
///  2. 过滤 prerelease + 按 `published_at` 降序取最新（`select_update_release`）。
///  3. 去 `v` 前缀 → `compare_semver > 0` 判新（空/不可解析按**无更新**处理，失败安全）。
///  4. 命中用户「跳过此版本」→ 无更新。
///  5. [`find_suitable_update_asset`] 挑平台/架构/形态资产；无适配 → 无更新。
///  6. 组装 [`AppUpdateInfo`]（`version` 保留原始 tag，对齐 上游）。
///
/// 第 5 步在**该 release 没发适配资产**时返 `None` ⇒ 本函数返 [`AppUpdateCheck::NoUpdate`]，
/// 即「有新版本，但没有你这台设备能装的包」与「已经是最新」在本函数的出口上**不可分辨**。
/// 调用方若需要分辨（Android 就需要：APK 资产是 2026-09-13 才开始发的，旧 release 一个都没有），
/// 在拿到 `NoUpdate` 之后再问一次 [`check_app_update_release_only`] —— 前四道闸两条腿共用，
/// 故「它说有、这条说没有」只可能是资产那一步的差别，不会是版本比对漂了。
///
/// # Errors
///
/// - [`ManifestError::ParseJson`]：releases JSON 解析失败（= 上游 `解析 GitHub API 响应失败`）。
pub fn check_app_update(
    releases_json: &str,
    current_version: &str,
    include_prerelease: bool,
    skipped_version: Option<&str>,
    platform: AssetPlatform,
    arch: AssetArch,
    loose_form: bool,
) -> Result<AppUpdateCheck, ManifestError> {
    newer_release_then(
        releases_json,
        current_version,
        include_prerelease,
        skipped_version,
        |release| app_update_info_for_release(release, platform, arch, loose_form),
    )
}

/// 检查 App 更新，但**跳过资产选择**：只回答「有没有比当前新的 release」。
///
/// # 为什么必须有第二条检查腿（不是「顺手加的通用性」）
///
/// [`check_app_update`] 的第 5 步（挑平台资产）选不到时返 [`AppUpdateCheck::NoUpdate`]，
/// 于是「有新版本但这个 release 没发你这台设备能装的包」会被说成「已是最新」——
/// 那正是 `commands/updater/app_update.rs` 头注写着「绝不」犯的错（把失败/未知伪装成已是最新）。
///
/// 这一档今天**真的会发生**在 Android 上：APK 资产是 2026-09-13 才开始发的
/// （`.github/workflows/android.yml` 的 `release-apk` job），在那之前的每一个 release 都没有
/// `Polaris_<version>_<ABI>-android.apk`。所以 `update_check` 的 Android 腿在拿到 `NoUpdate` 之后会再问一次本函数：
/// 它说有 ⇒ 如实报「有新版本」，只是没有可下载的资产，用户出口退回**打开发布页**。
///
/// # 产出的 [`AppUpdateInfo`] 三个资产字段是**空的，且必须如实为空**
///
/// `download_url` / `file_name` 空串、`file_size` 为 0 —— 这一档没有选中的资产，
/// 编一个 URL 出来会让下载腿去下一个不存在（或错形态）的东西。调用方据此**不得**发起下载：
/// 这一档的用户出口是发布页链接（移动端 `UpdatePage` 的「打开发布页」——
/// 前端的「下载」按钮按 `downloadUrl` 非空才渲染，正是靠这三个字段如实为空才关得掉）。
///
/// 前四道闸（通道过滤 / 取最新 / 比版本 / 跳过此版本）与 [`check_app_update`] **共用同一段实现**
/// （`newer_release_then`），故两条腿不可能在这四件事上漂。
///
/// # Errors
///
/// - [`ManifestError::ParseJson`]：releases JSON 解析失败（同 [`check_app_update`]）。
pub fn check_app_update_release_only(
    releases_json: &str,
    current_version: &str,
    include_prerelease: bool,
    skipped_version: Option<&str>,
) -> Result<AppUpdateCheck, ManifestError> {
    newer_release_then(
        releases_json,
        current_version,
        include_prerelease,
        skipped_version,
        |release| AppUpdateCheck::Available(release_only_update_info(release)),
    )
}

/// 两条检查腿共用的前四道闸：解析 → 取通道内最新 → 比版本 → 跳过此版本。
///
/// 抽出来是**为了让两条腿不可能漂**：这四件事里任何一件在某一条腿上写歪，
/// 表现都是「一个平台提示更新、另一个平台不提示」，而两侧各自的单测都绿。
fn newer_release_then(
    releases_json: &str,
    current_version: &str,
    include_prerelease: bool,
    skipped_version: Option<&str>,
    to_check: impl FnOnce(&GithubRelease) -> AppUpdateCheck,
) -> Result<AppUpdateCheck, ManifestError> {
    let releases: Vec<GithubRelease> =
        serde_json::from_str(releases_json).map_err(|e| ManifestError::ParseJson(e.to_string()))?;

    let Some(release) = select_update_release(&releases, include_prerelease) else {
        // 无（正式）发布版本（= 上游 `未找到发布版本`）。
        return Ok(AppUpdateCheck::NoUpdate);
    };

    let latest_version = strip_v(&release.tag_name);

    // 必须比 current 新（compare_semver 对空串/不可解析报错 → 失败安全按无更新，绝不误报有更新）。
    let is_newer = compare_semver(latest_version, current_version)
        .map(|ord| ord > 0)
        .unwrap_or(false);
    if !is_newer {
        return Ok(AppUpdateCheck::NoUpdate);
    }

    // 用户已跳过此版本（= 上游 `if (this.skippedVersion === latestVersion)`）。
    if skipped_version == Some(latest_version) {
        return Ok(AppUpdateCheck::NoUpdate);
    }

    Ok(to_check(release))
}

/// 「只比版本」那条腿的 [`AppUpdateInfo`]：版本/标题/说明/时间照抄，三个资产字段留空。
///
/// 留空是**判据的一部分**（`check_app_update_release_only_has_no_asset_fields`）：
/// 一个非空的 `download_url` 会让下游误以为这一档可以直接下载。
fn release_only_update_info(release: &GithubRelease) -> AppUpdateInfo {
    AppUpdateInfo {
        version: release.tag_name.clone(),
        title: release
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| release.tag_name.clone()),
        release_notes: release.body.clone().unwrap_or_default(),
        download_url: String::new(),
        file_size: 0,
        published_at: release.published_at.clone().unwrap_or_default(),
        is_prerelease: release.prerelease,
        file_name: String::new(),
        sha256: None,
    }
}

/// 解析所选通道的最新 release，并且仅在它与当前安装版本**完全相同**时返回安装清单。
///
/// 这是设置页“重新下载当前版本”的解析腿：下载、摘要校验与安装仍复用既有命令；这里仅放宽
/// “必须更高版本”这一道发现策略。比当前版本旧或新都返回 [`AppUpdateCheck::NoUpdate`]，因此不会
/// 借“重新安装”之名静默降级，也不会把真正的新版本伪装成同版本重装。
pub fn resolve_current_app_release(
    releases_json: &str,
    current_version: &str,
    include_prerelease: bool,
    platform: AssetPlatform,
    arch: AssetArch,
    loose_form: bool,
) -> Result<AppUpdateCheck, ManifestError> {
    let releases: Vec<GithubRelease> =
        serde_json::from_str(releases_json).map_err(|e| ManifestError::ParseJson(e.to_string()))?;

    let Some(release) = select_update_release(&releases, include_prerelease) else {
        return Ok(AppUpdateCheck::NoUpdate);
    };
    let is_current = compare_semver(strip_v(&release.tag_name), current_version)
        .map(|ord| ord == 0)
        .unwrap_or(false);
    if !is_current {
        return Ok(AppUpdateCheck::NoUpdate);
    }

    Ok(app_update_info_for_release(
        release, platform, arch, loose_form,
    ))
}

fn app_update_info_for_release(
    release: &GithubRelease,
    platform: AssetPlatform,
    arch: AssetArch,
    loose_form: bool,
) -> AppUpdateCheck {
    let Some(asset) = find_suitable_update_asset(&release.assets, platform, arch, loose_form)
    else {
        // 无适配当前平台的安装包（= 上游 `未找到适合当前平台的安装包`）。
        return AppUpdateCheck::NoUpdate;
    };

    AppUpdateCheck::Available(AppUpdateInfo {
        version: release.tag_name.clone(),
        title: release
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| release.tag_name.clone()),
        release_notes: release.body.clone().unwrap_or_default(),
        download_url: asset.browser_download_url.clone(),
        file_size: asset.size,
        published_at: release.published_at.clone().unwrap_or_default(),
        is_prerelease: release.prerelease,
        file_name: asset.name.clone(),
        sha256: asset.digest.as_deref().and_then(parse_asset_digest),
    })
}

// ── App 安装包资产选择（正式发布命名合同）──────────

/// 从 release 资产里挑适配 `(platform, arch, loose_form)` 的 **App 安装包**。
///
/// 移植自 `update-asset.findSuitableUpdateAsset`（#72：每平台 loose/installed 双形态须按**当前运行
/// 形态**选对应包，否则错配——便携被发 NSIS setup 会装出多余副本）：
///  - Windows x64: installed setup or Portable ZIP, selected by run form.
///  - macOS: exact x64 / aarch64 DMG; no cross-architecture fallback.
///  - Linux amd64: prefer AppImage for loose form, deb for installed form.
///  - Android: ARMv8 / ARMv7 native split, then ARM-only universal; other architectures return None.
///
/// All names use `Polaris_<version>_<architecture>-<platform>[-<form>].ext`.
/// Windows portable zip 由下游生成手动覆盖计划：确认后停核、打开压缩包与程序目录并退出；不自动替换。
#[must_use]
pub fn find_suitable_update_asset(
    assets: &[GithubAsset],
    platform: AssetPlatform,
    arch: AssetArch,
    loose_form: bool,
) -> Option<&GithubAsset> {
    let suffix = match (platform, arch) {
        (AssetPlatform::Windows, AssetArch::X64) => {
            if loose_form {
                PORTABLE_ZIP_SUFFIX
            } else {
                "x64-win-setup.exe"
            }
        }
        (AssetPlatform::Macos, AssetArch::Arm64) => "aarch64-mac.dmg",
        (AssetPlatform::Macos, AssetArch::X64) => "x64-mac.dmg",
        (AssetPlatform::Linux, AssetArch::X64) => {
            let preferred = if loose_form {
                "amd64-linux.AppImage"
            } else {
                "amd64-linux.deb"
            };
            let fallback = if loose_form {
                "amd64-linux.deb"
            } else {
                "amd64-linux.AppImage"
            };
            return assets
                .iter()
                .find(|a| release_asset_matches(&a.name, preferred))
                .or_else(|| {
                    assets
                        .iter()
                        .find(|a| release_asset_matches(&a.name, fallback))
                });
        }
        (AssetPlatform::Android, AssetArch::Arm64) => ANDROID_APK_SUFFIX,
        (AssetPlatform::Android, AssetArch::Armv7) => ANDROID_ARMV7_APK_SUFFIX,
        _ => return None,
    };
    // Prefer the native split; ARM-only universal is a valid fallback for either ARM ABI.
    assets
        .iter()
        .find(|a| release_asset_matches(&a.name, suffix))
        .or_else(|| {
            (platform == AssetPlatform::Android)
                .then(|| {
                    assets
                        .iter()
                        .find(|a| release_asset_matches(&a.name, ANDROID_UNIVERSAL_APK_SUFFIX))
                })
                .flatten()
        })
}

// ── 内核资产选择（移植 singbox-asset.findSuitableSingboxAsset，逐字保留）──────────────

#[cfg(test)]
mod tests;

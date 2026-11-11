//! 内核文件命名与配置根目录的单点。
//!
//! # 应用直起的内核取自安装包
//!
//! 应用应当运行与本应用版本配套的内核。默认从安装包解析；显式离线修复可安装到
//! current-user CoreStore 的唯一 compiled-ID 数据槽，再由同一 execution factory 准入。
//! 用户配置字段、环境变量、旧更新缓存均不能选择该槽；其中的 receipt 不是执行凭证。
//! 这保证的是**路径来源**，不是**内容**：安装目录在若干形态下本就由用户可写（Windows 当前用户
//! 安装、便携版、用户属主的 `.app`、解包运行的 AppImage），经提权助手运行时执行的还是助手目录
//! 里的副本。内容层面的配套校验不在本模块。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use polaris_helper_proto::{core_payload, Platform};

/// 进程级基目录（= `AppRuntime` 的 `config_dir`）。
///
/// 消费者是无 `AppHandle` 的自由函数（`env_trust` 的可信根、`geo_seed` 的规则目录），故须在启动期
/// 一次性注入；未注入时 [`base_dir`] 返 `None`，各消费者按「该目录缺席」处理。
static CORE_BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 注入基目录（`lib.rs` setup 里调一次；重复调用忽略后来者）。
pub fn init_base_dir(config_dir: PathBuf) {
    let _ = CORE_BASE_DIR.set(config_dir);
}

/// 基目录（未注入 → `None`）。
#[must_use]
pub fn base_dir() -> Option<&'static Path> {
    CORE_BASE_DIR.get().map(PathBuf::as_path)
}

// ── 文件名（纯函数）─────────────────────────────────────────────────────────

/// 平台核文件名（纯函数：`os` 取 `std::env::consts::OS` 的口径）。
#[must_use]
pub fn core_filename_for(os: &str) -> &'static str {
    // 真值在 `polaris-helper-proto`：helper 收文件的白名单读的是同一份。
    core_payload::core_filename(Platform::parse(os))
}

/// 本平台核文件名。
#[must_use]
pub fn core_filename() -> &'static str {
    core_filename_for(std::env::consts::OS)
}

/// 本平台 NaiveProxy 动态库文件名（`os` 取 `std::env::consts::OS` 口径）。
///
/// macOS 的 cronet 已静态编入随包核，因此没有动态 sidecar。
#[must_use]
pub fn core_sidecar_filename_for(os: &str) -> Option<&'static str> {
    core_payload::sidecar_filename(Platform::parse(os))
}

/// 与指定核心同行的 NaiveProxy 动态库路径。
#[must_use]
pub fn core_sidecar_path_for(core: &Path, os: &str) -> Option<PathBuf> {
    Some(core.parent()?.join(core_sidecar_filename_for(os)?))
}

#[cfg(test)]
mod tests;

//! 内核文件命名与配置根目录的单点，外加旧安装残留的启动期清理。
//!
//! # 应用直起的内核取自安装包
//!
//! 应用应当运行与本应用版本配套的内核，因此应用侧只从安装包的资源目录解析内核
//! （[`crate::runtime::proxy::resolve_core_binary`]），用户配置目录里不再有任何「可写现役核」。
//! 这保证的是**路径来源**，不是**内容**：安装目录在若干形态下本就由用户可写（Windows 当前用户
//! 安装、便携版、用户属主的 `.app`、解包运行的 AppImage），经提权助手运行时执行的还是助手目录
//! 里的副本。内容层面的配套校验不在本模块。
//!
//! 早期版本曾把内核复制到 `<config_dir>/core_update/` 并允许在线更新、手动上传、回滚；那些目录
//! 里的二进制来源不受本应用版本约束，升级后必须消失，见 [`remove_legacy_core_state`]。

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

// ── 旧安装残留清理 ───────────────────────────────────────────────────────────

/// 旧版本留在配置根下、可能装着非配套内核的目录项（均为配置根的**直接子项**）。
///
/// - `core_update/`：旧的可写现役核目录（内核、`sing-box.bak` 回滚备份、`.core-seed.json` 簿记、
///   在线更新或手动上传得到的二进制）。
/// - `core-staged/`：旧的内核自动更新暂存与解压工作目录。
/// - `core-promote/`：向提权助手递送内核时的暂存目录；每次递送后即清，这里收的是递送中途进程
///   被杀留下的那一份。
pub const LEGACY_CORE_STATE_NAMES: [&str; 3] = [
    "core_update",
    "core-staged",
    crate::runtime::core_promote::CORE_PROMOTE_DIR_NAME,
];

/// 清理 [`LEGACY_CORE_STATE_NAMES`]，返回实际删掉的路径。
///
/// 以当前用户身份执行，只碰配置根的直接子项：
///  - 名字必须是单个普通路径分量，拼出来的路径的父目录就是配置根（[`direct_child`]）；
///  - 用 `symlink_metadata` 判型，**不跟随符号链接**：目录项本身是链接时只删链接，链接指向的
///    内容原样保留；是目录时递归删除（`remove_dir_all` 对目录内的链接同样只删链接本身）。
///
/// 失败只记日志：残留目录已经不被任何代码读取，删不掉不影响起核，下次启动再试。
pub fn remove_legacy_core_state(config_root: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for name in LEGACY_CORE_STATE_NAMES {
        let Some(path) = direct_child(config_root, name) else {
            log::error!("旧内核残留清理：{name} 不是配置根的直接子项，已跳过");
            continue;
        };
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                log::warn!("旧内核残留清理：读取 {} 失败：{e}", path.display());
                continue;
            }
        };
        let file_type = metadata.file_type();
        let result = if file_type.is_dir() && !file_type.is_symlink() {
            std::fs::remove_dir_all(&path)
        } else {
            // 普通文件与符号链接走这里。Windows 上指向目录的链接要用 `remove_dir` 才删得掉
            // （它同样只删链接本身）。
            std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path))
        };
        match result {
            Ok(()) => {
                log::info!("已清理旧版本内核残留：{}", path.display());
                removed.push(path);
            }
            Err(e) => log::warn!("旧内核残留清理：删除 {} 失败：{e}", path.display()),
        }
    }
    removed
}

/// `name` 是单个普通路径分量时返回 `<config_root>/<name>`，否则 `None`。
///
/// 含分隔符、`..`、`.`、绝对路径或空串的名字都会让拼出来的路径离开配置根的直接子项这一层。
fn direct_child(config_root: &Path, name: &str) -> Option<PathBuf> {
    let mut components = Path::new(name).components();
    let (Some(std::path::Component::Normal(only)), None) = (components.next(), components.next())
    else {
        return None;
    };
    if only != std::ffi::OsStr::new(name) {
        return None;
    }
    let path = config_root.join(name);
    (path.parent() == Some(config_root)).then_some(path)
}

#[cfg(test)]
mod tests;

//! install-core —— linux 薄壳：公共核心下沉 [`crate::core_install`]，本文件
//! 只留 linux 平台 hook（按目录 fd 的清理）。
//!
//! ## 公共核心（已下沉）
//!
//! 收源文件 / sha256 校验 / 逐文件原子安装 一律走 [`crate::core_install`]，
//! 结果类型直接用公共 [`InstallResult`]。mac/linux 两份原本逐字同，合并后单一真值。
//!
//! 原先此处另有个 `InstallOutcome` 影子枚举：与 [`InstallResult`] **10 个 variant 逐一同构**
//!（唯一差异是拼写 `CoredirUnset` vs `CoreDirUnset`，纯历史命名不一致，wire 输出本就相同），
//! 外加 `to_common`/`from_common` 双向映射把公共枚举翻来覆去搬一遍。影子枚举 + 双射已删（G3.4）：
//! `install_core` 直接返回 `InstallResult`，各步的 `Err(e) => return e` 一步到位。
//!
//! ## linux 专属（本 crate 保留）
//!
//! - **清理**：受管目录里凡不是本次装入的**常规文件**一律删（`prune_unlisted_regular_files`）。
//!   受管目录是 `--coredir` 指的独立目录（helper 二进制不住在里面），其中出现的任何别的常规文件
//!   都只可能是旧版残留或早先被装进来的不该有的内容，没有需要保留的。按目录 fd 操作、
//!   不跟随链接：符号链接与子目录原样留着，不顺着它们去碰目录外的东西。
//!
//! ## 安全模型（对照 Go 源注释 :9-18）
//!
//! - 核二进制在 root-owned 受管目录（coreDir），未授权用户改不动。
//! - start 只跑锁定的 `coreDir/sing-box`，绝不跑客户端指定的任意路径（见 handler.rs 的 start 路径锁）。
//! - install-core 校验 `sha256(srcDir/sing-box) == wantHash` 后，逐文件 `.new + rename` 原子就位（防 TOCTOU）。
//! - 只写 coreDir、不接受任意路径；源文件只认白名单名字的单链接常规文件（见公共核心）。

use std::path::Path;

use nix::dir::Dir;
use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{fstatat, Mode, SFlag};
use nix::unistd::{unlinkat, UnlinkatFlags};

use crate::core_install::{install_received, receive_src, InstallResult, SRC_LIMITS};
use polaris_helper_proto::codec::is_valid_sha256_hex;
use polaris_helper_proto::Platform;

/// 执行 install-core（移植自 Go `installCore`，:183-244）。
///
/// 参数：
/// - `core_dir`：锁定的 root-owned 受管核目录（None = 未配置 → `ERR coredir-unset`）。
/// - `src_dir`：app 下载+预检的临时核源目录。
/// - `want_hash`：期望的 sing-box sha256（hex，64 字符）。
/// - `owner_uid`：已鉴权对端的 uid（`SO_PEERCRED`）。源目录与其中文件都必须属它所有。
///
/// 文件操作：收源文件并校验 sha256 → 建 coreDir → 逐文件 `.new + rename` 原子就位 →
/// 清掉受管目录里不属于本次安装的常规文件。
pub fn install_core(
    core_dir: Option<&Path>,
    src_dir: &str,
    want_hash: &str,
    owner_uid: u32,
) -> InstallResult {
    // :184-185: coreDir 未配置。
    let Some(core_dir) = core_dir else {
        return InstallResult::CoreDirUnset;
    };
    // :187-188: bad-args —— srcDir 空 或 wantHash 非 64 hex 字符。
    if src_dir.is_empty() || !is_valid_sha256_hex(want_hash) {
        return InstallResult::BadArgs;
    }
    // 公共核心：收源文件（:198-200）。各 Err 已是 InstallResult，原样返回。
    let received = receive_src(
        Path::new(src_dir),
        want_hash,
        Platform::Linux,
        Some(owner_uid),
        SRC_LIMITS,
    );
    let received = match received {
        Ok(r) => r,
        Err(e) => return e,
    };

    // 公共核心：校验 sing-box 哈希 + 逐文件 .new + rename 原子就位（:190-228，含 MkdirAll coreDir）。
    let names = match install_received(core_dir, received) {
        Ok(n) => n,
        Err(e) => return e,
    };

    prune_unlisted_regular_files(core_dir, &names);

    InstallResult::Installed
}

/// 删掉 coreDir 里不在 `keep_names` 的**常规文件**。best-effort，单项失败跳过。
///
/// 全程相对目录 fd：`fstatat(AT_SYMLINK_NOFOLLOW)` 判类型、`unlinkat` 删名字 —— 两者都只作用于
/// 这个目录里的那个目录项本身。符号链接、子目录、其它类型一律不动。
fn prune_unlisted_regular_files(core_dir: &Path, keep_names: &[String]) {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let Ok(mut dir) = Dir::open(core_dir, flags, Mode::empty()) else {
        return;
    };
    // 先收名字再删：边遍历边删同一个目录流，读到哪些项不确定。
    let names: Vec<_> = dir
        .iter()
        .flatten()
        .map(|entry| entry.file_name().to_owned())
        .collect();
    for name in names {
        if keep_names
            .iter()
            .any(|keep| keep.as_bytes() == name.to_bytes())
        {
            continue;
        }
        let Ok(stat) = fstatat(&dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) else {
            continue;
        };
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
            continue;
        }
        let _ = unlinkat(&dir, name.as_c_str(), UnlinkatFlags::NoRemoveDir);
    }
}

#[cfg(test)]
mod tests;

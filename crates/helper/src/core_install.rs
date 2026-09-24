//! install-core 公共核心（mac/linux 逐字等价部分下沉）。
//!
//! 把 app 下载+预检的临时内核 src，校验 sha256 后 root 写入锁定的受保护目录 coreDir +
//! 逐文件 `.new + rename` 原子就位 + 清陈旧残留。移植自 上游 `helper/helper.go:127-198`
//!（mac）与 `helper-linux/helper.go:183-244`（linux），两份核心文件操作逐字同。
//!
//! ## 职责边界
//!
//! 只放**跨平台逐字等价**的逻辑：sha256 校验、原子安装、通用 prune。OS 专属差异
//! （mac xattr/codesign、linux lib*.so 限定清理）由各 helper crate 调本模块前后 hook。
//!
//! ## 安全约束（逐字对照 Go 源）
//!
//! - **只写锁定的 coreDir**（`filepath.Join`，不接受任意路径）→ 防「持 token 写任意 root 路径」。
//! - **哈希校验主二进制 sing-box**：读全字节进内存校验（堵 TOCTOU：攻击者读后替换无效，读前替换则 hash
//!   不符被拒）。与 token（通道鉴权）互补两层。
//!
//! ## 移植纪律
//!
//! Go 源的「整个源目录逐文件 .new + rename 原子就位」+「清受保护目录多余文件」在本模块拆为：
//! - [`sha256_hex`]：sha256 → hex（纯函数，helper-common 内部 + verify 复用）。
//! - [`verify_singbox_hash`]：纯逻辑 sha256 校验（读文件 + sha256 + 比对）。
//! - [`list_src_files`]：枚举源目录文件（fs 操作）。
//! - [`atomic_install_files`]：逐文件 .new + rename + chmod（fs）。
//! - [`prune_extra_files`]：清受保护目录多余旧残留（通用版，keep_names 外全删，best-effort）。
//! - [`install_core_files`]：完整流程编排（参数校验 + 上四步），平台 hook 留各 helper crate。

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// sing-box 主二进制文件名（`helper.go:140,194`：`filepath.Join(srcDir, "sing-box")`）。
pub const SINGBOX_BIN_NAME: &str = "sing-box";

/// Windows 侧的 sing-box 主二进制文件名（PE 必须带 `.exe`，否则 `CreateProcessW` 找不到）。
///
/// 与 [`SINGBOX_BIN_NAME`] 并列而不是在函数里按 `cfg(windows)` 挑：本模块在 Linux 上也要能对
/// **两个**名字各跑一遍单测（三平台 helper 同 crate，win 分支的纯逻辑本就在本机测）。
pub const SINGBOX_BIN_NAME_WIN: &str = "sing-box.exe";

/// Windows 侧与核同目录的配套 DLL 名（cronet-naive 出站用；ACL 自检覆盖面的第二个白名单文件）。
///
/// **住在这里、不住在 `platform::windows::coreacl` 的理由是结构性的**（该路径在非 Windows 上
/// 不入编译，故此处只能写成普通代码体、不能写成 intra-doc 链接 —— 写成链接会让 `cargo doc`
/// 在 Linux 上红，这本身就是下面那条 cfg 论证的又一个实例）：`platform::windows`
/// 的门是 `#[cfg(any(target_os = "windows", test))]`，那个 `test` 只在 **polaris-helper 自己**的
/// test 编译期成立；src-tauri 在 Linux 上跑测试时 polaris-helper 是普通依赖（`cfg(test)` 关），
/// 整个 `platform::windows` 不入编译 ⇒ 放在那里的字面量**物理上进不了**
/// `src-tauri/src/runtime/core_paths/tests` 的跨 crate 等值门。本模块无 cfg，进得去。
///
/// 少了那道门的后果不是编译错而是门少一条腿且不自曝：cronet 升版改名时
/// `core_sidecar_filename_for("windows")` 与 `manager::WIN_CORE_SIDECAR_NAME` 被门钉着会一起改，
/// 这份不会 ⇒ 自检去问一个不存在的路径 ⇒ 落 `unreadable` ⇒ 只 warn、起核照常 ⇒ 真正躺在 exec
/// 目录里的那个 DLL 从此无人检查。
pub const CRONET_DLL_NAME_WIN: &str = "libcronet.dll";

/// install-core 结果（统一命名，融合 mac `InstallResult` 与 linux `InstallOutcome`，二者同构）。
///
/// 对照 Go `installCore` 的所有 return 分支（`helper.go:133-198` / `helper-linux/helper.go:183-244`）。
/// wire 序列化见 [`InstallResult::to_wire_line`]，输出格式与原 mac/linux 逐字一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallResult {
    /// `OK installed`（mac `helper.go:197` / linux `:243`）。
    Installed,
    /// `ERR coredir-unset`（mac `helper.go:135` / linux `:185`：coreDir 未配置）。
    CoreDirUnset,
    /// `ERR bad-args`（mac `helper.go:137` / linux `:187`：srcDir 空 或 wantHash 长度 != 64）。
    BadArgs,
    /// `ERR read-singbox <err>`（mac `helper.go:142` / linux `:192`：读主二进制失败）。
    ReadSingbox(String),
    /// `ERR hash-mismatch`（mac `helper.go:146` / linux `:196`：sha256 不符）。
    HashMismatch,
    /// `ERR readdir <err>`（mac `helper.go:149` / linux `:199`：枚举源目录失败）。
    ReadDir(String),
    /// `ERR mkdir <err>`（mac `helper.go:153` / linux `:203`：创建 coreDir 失败）。
    Mkdir(String),
    /// `ERR read <name> <err>`（mac `helper.go:164` / linux `:215`：读配套文件失败）。
    Read { name: String, detail: String },
    /// `ERR write <name> <err>`（mac `helper.go:171` / linux `:221`：写 .new 失败）。
    Write { name: String, detail: String },
    /// `ERR rename <name> <err>`（mac `helper.go:174` / linux `:225`：rename 失败）。
    Rename { name: String, detail: String },
    /// `ERR busy`（**Polaris 新增，上游无**：受管核在跑时拒绝安装 —— 目前只有 Windows 会产出）。
    ///
    /// Windows 既 rename 不动运行中的 exe，也 rename 不动已被加载的 DLL：不挡的话 `.new` 写得进去、
    /// `rename` 失败，安装半途而废且错误面目全非。mac/linux 的 rename 在同样情形下会成功（旧 inode
    /// 继续被运行中的进程持有），故那两支不产出本变体。
    Busy,
}

impl InstallResult {
    /// 转 wire 响应行（对照 Go `installCore` 的 return 字符串，handler 直接写出）。
    ///
    /// 输出格式与原 mac `From<InstallResult> for Response` 走 `ProtoError::with_detail` 的
    /// `read-singbox <d>` / `readdir <d>` / ... 逐字一致，也与原 linux `InstallOutcome::to_wire_line`
    /// 同构 —— 二者均源自同一 Go 源，故 wire 协议统一后无需迁移。
    #[must_use]
    pub fn to_wire_line(&self) -> String {
        match self {
            Self::Installed => "OK installed".to_string(),
            Self::CoreDirUnset => "ERR coredir-unset".to_string(),
            Self::BadArgs => "ERR bad-args".to_string(),
            Self::ReadSingbox(d) => format!("ERR read-singbox {d}"),
            Self::HashMismatch => "ERR hash-mismatch".to_string(),
            Self::ReadDir(d) => format!("ERR readdir {d}"),
            Self::Mkdir(d) => format!("ERR mkdir {d}"),
            Self::Read { name, detail } => format!("ERR read {name} {detail}"),
            Self::Write { name, detail } => format!("ERR write {name} {detail}"),
            Self::Rename { name, detail } => format!("ERR rename {name} {detail}"),
            Self::Busy => "ERR busy".to_string(),
        }
    }

    /// 是否成功（便于上层短路，对齐原 linux `InstallOutcome::is_ok`）。
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Installed)
    }

    /// 转 proto [`polaris_helper_proto::Response`]（**三平台唯一一份**）。
    ///
    /// 走既有的两个单一真值往返：[`InstallResult::to_wire_line`] 是 install-core 响应行的权威
    /// 形态，[`polaris_helper_proto::Error::parse`] 是 wire→`Error` 的权威解析（未知 token 归
    /// `Other` 且 detail 保留原文 ⇒ 往返无损）。
    ///
    /// **刻意不再有第二张 `variant → ErrorCode` 映射表**：mac 侧曾手写过一张，与本往返「结果
    /// 相同但机制不同」—— `to_wire_line` 一改只流向走往返的那侧，不流向那张表，下次加 variant
    /// 必漏一边。收敛前先出过逐 variant 的等价收据（11/11 逐字相同，含 detail），不是直接替换。
    ///
    /// 非 `Installed` 的 `to_wire_line` 恒以 `"ERR "` 开头 ⇒ `parse` 必 `Some`。真解析不出来说明
    /// `to_wire_line` 换了形态，那时诚实回 `unknown`（失败向关），不能假装装好了。
    #[must_use]
    pub fn to_response(&self) -> polaris_helper_proto::Response {
        use polaris_helper_proto::{Error as ProtoError, ErrorCode, Response, ResponseKind};
        if self.is_ok() {
            return Response::Ok(ResponseKind::Installed);
        }
        ProtoError::parse(&self.to_wire_line()).map_or_else(
            || Response::Err(ProtoError::new(ErrorCode::Unknown)),
            Response::Err,
        )
    }
}

/// sha256 → 小写 hex（对照 Go `sha256.Sum256` + `hex.EncodeToString`）。
///
/// 纯函数，[`verify_singbox_hash`] 内部复用，也供 helper-common 外部测试断言用。
/// 合并自原 linux `sha256_hex`（mac 原为内联 `Sha256::new` + `hex::encode`，等价）。
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// 校验 sing-box 主二进制 sha256（移植自 `helper.go:140-147`）。
///
/// 读 src_dir/`bin_name` 全字节，计算 sha256 与 want_hash 比对（大小写不敏感，对齐 Go `EqualFold`）。
/// 返回读到的字节（供后续原子写入复用，堵 TOCTOU —— 见 Go `helper.go:161`：`data := sbData`）。
///
/// `bin_name` 由调用方给（mac/linux 传 [`SINGBOX_BIN_NAME`]，Windows 传
/// [`SINGBOX_BIN_NAME_WIN`]）—— 它必须与 [`atomic_install_files`] 那侧传的是**同一个**名字，
/// 否则被校验的字节和被复用的字节不是同一个文件，TOCTOU 那层就空了。
pub fn verify_singbox_hash(
    src_dir: &Path,
    want_hash: &str,
    bin_name: &str,
) -> Result<Vec<u8>, InstallResult> {
    let sb_path = src_dir.join(bin_name);
    let sb_data = fs::read(&sb_path).map_err(|e| InstallResult::ReadSingbox(e.to_string()))?;
    // helper.go:144-146: sha256.Sum256 + hex + EqualFold
    let actual = sha256_hex(&sb_data);
    if !actual.eq_ignore_ascii_case(want_hash) {
        return Err(InstallResult::HashMismatch);
    }
    Ok(sb_data)
}

/// 枚举源目录的文件条目（移植自 `helper.go:148-151` 的 `os.ReadDir`）。
///
/// 返回所有**非目录**条目名（按字母序，对齐 Go ReadDir 的排序）。目录条目跳过（`helper.go:158`）。
pub fn list_src_files(src_dir: &Path) -> Result<Vec<String>, InstallResult> {
    let entries = fs::read_dir(src_dir).map_err(|e| InstallResult::ReadDir(e.to_string()))?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            // helper.go:158: if e.IsDir() { continue }
            let ft = e.file_type().ok()?;
            if ft.is_dir() {
                None
            } else {
                Some(e.file_name().to_string_lossy().into_owned())
            }
        })
        .collect();
    names.sort(); // Go os.ReadDir 返回已排序
    Ok(names)
}

/// 逐文件原子写入受保护目录（移植自 `helper.go:156-178`）。
///
/// 对照 Go：
/// ```text
/// for _, e := range entries {
///     if e.IsDir() { continue }
///     name := e.Name()
///     data := sbData          // sing-box 复用已校验字节
///     if name != "sing-box" { data = read(srcDir/name) }
///     dst := Join(coreDir, name); tmp := dst + ".new"
///     WriteFile(tmp, data, 0755)
///     Rename(tmp, dst)
///     Chmod(dst, 0755)
/// }
/// ```
///
/// `bin_name` 那一项用传入的 `sb_data`（已校验字节，堵 TOCTOU）；其它文件从 src_dir 读。
/// 判等必须用调用方给的 `bin_name`，不能写死 [`SINGBOX_BIN_NAME`]：Windows 的 `sing-box.exe`
/// 与 `"sing-box"` 不等 ⇒ 落进 else 分支被二次读盘，**编译得过但 hash 校验与实际写入的字节脱钩**。
/// tmp 路径构造逐字对照 Go `dst + ".new"`（`helper-linux/helper.go:218-228`）—— 直接在 dst
/// 后缀 `.new`，而非 mac 原版的「set_extension 替换扩展名」（原 mac 实现对无扩展名的 `sing-box`
/// 会变成 `sing-box.new` 而非 `sing-box.new`，二者结果一致；但对带扩展名的 `libcronet.dylib`
/// mac 原版产出 `libcronet.dylib.new` 也是对的——与 linux 同）。统一采用 linux 的「append .new」
/// 形式以更贴合 Go 源且更易读。
///
/// ## `bin_name` 恒第一个处理（**不是**按 `names` 的字母序）
///
/// 「`.new` + rename 保证半成品不会变成生效文件」这句只对**单个文件**成立，对**一组文件**不成立：
/// 装核过了判活闸后锁就放了，并发 start 把核起起来时，`libcronet.dll`（字母序在 `sing-box.exe`
/// 之前）**只在 cronet-naive 出站被用到时才加载** ⇒ 它的 rename 成功、`sing-box.exe` 的失败 ⇒
/// 整个函数 Err、prune 不跑 ⇒ 受保护目录留下「新 DLL + 旧 exe」的**持久化版本错配**（跑着的旧
/// exe 此后首次用到 cronet 出站就会加载新 DLL，ABI 不匹配）。
///
/// 我方受管核恒持有 exe 句柄 ⇒ 把 `bin_name` 排到最前，撞锁时**第一个** rename 就失败，零文件
/// 被改动即干净中止。unix 侧 rename 恒成功（旧 inode 仍被运行中的进程持有），顺序无行为影响。
pub fn atomic_install_files(
    src_dir: &Path,
    core_dir: &Path,
    names: &[String],
    sb_data: &[u8],
    bin_name: &str,
) -> Result<(), InstallResult> {
    // helper.go:152-154: MkdirAll(coreDir, 0755)
    fs::create_dir_all(core_dir).map_err(|e| InstallResult::Mkdir(e.to_string()))?;

    // `bin_name` 先行（见函数文档「`bin_name` 恒第一个处理」），其余保持 `names` 的原序。
    for name in names
        .iter()
        .filter(|n| n.as_str() == bin_name)
        .chain(names.iter().filter(|n| n.as_str() != bin_name))
    {
        let dst = core_dir.join(name);
        // helper.go:161-166: sing-box 复用 sbData，否则读 src/name
        let data: Vec<u8> = if name == bin_name {
            sb_data.to_vec()
        } else {
            fs::read(src_dir.join(name)).map_err(|e| InstallResult::Read {
                name: name.clone(),
                detail: e.to_string(),
            })?
        };
        // tmp = dst + ".new"（对照 linux `helper-linux/helper.go:218-228` 与 mac 原版语义）。
        let tmp = {
            let mut t = dst.as_os_str().to_os_string();
            t.push(".new");
            std::path::PathBuf::from(t)
        };
        // helper.go:170-172: WriteFile(tmp, data, 0755)
        fs::write(&tmp, &data).map_err(|e| InstallResult::Write {
            name: name.clone(),
            detail: e.to_string(),
        })?;
        // helper.go:173-176: Rename(tmp, dst)；失败清 tmp
        if let Err(e) = fs::rename(&tmp, &dst) {
            let _ = fs::remove_file(&tmp);
            return Err(InstallResult::Rename {
                name: name.clone(),
                detail: e.to_string(),
            });
        }
        // helper.go:177: Chmod(dst, 0755)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/// 清理受保护目录多余文件（移植自 `helper.go:179-192`，通用版）。
///
/// 删除 core_dir 中不在 keep_names 的旧文件（防 rollback 后残留陈旧配套）。
/// best-effort，单项失败跳过。linux 专属的「仅删 lib*.so* / sing-box」保守策略由各 helper
/// crate 自行在调用本函数后追加（或用更窄的 keep_names 配合）。
pub fn prune_extra_files(core_dir: &Path, keep_names: &[String]) {
    // helper.go:186-192: ReadDir(coreDir)，非目录且不在 keep_names 的删除
    let Ok(entries) = fs::read_dir(core_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !keep_names.contains(&name) {
            // helper.go:189: _ = os.Remove —— 失败忽略
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 完整 install-core 流程编排（不含 mac 专属 xattr/codesign、linux 专属 lib*.so 清理 ——
/// 那两步由各 helper crate 在调用本函数前后 hook）。
///
/// 严格对照 Go `installCore`(`helper.go:133-198`) 的文件操作部分：
/// 1. 参数校验（core_dir 非空、src_dir 非空、want_hash 64 字符 hex）
/// 2. 校验 sing-box sha256
/// 3. 枚举源目录文件
/// 4. mkdir core_dir
/// 5. 逐文件原子写入
/// 6. 清理多余旧文件（通用 prune）
///
/// 返回 `(InstallResult, names)`：names 为本次安装的源目录文件名列表（供平台 hook 用，
/// 如 linux 可据此判定哪些保留、哪些是 lib*.so* 残留需清）。成功时 result 为 [`InstallResult::Installed`]。
pub fn install_core_files(
    core_dir: &Path,
    src_dir: &Path,
    want_hash: &str,
    bin_name: &str,
) -> Result<Vec<String>, InstallResult> {
    // helper.go:134-138: 参数校验
    if core_dir.as_os_str().is_empty() {
        return Err(InstallResult::CoreDirUnset);
    }
    if src_dir.as_os_str().is_empty() || !is_valid_sha256_hex(want_hash) {
        return Err(InstallResult::BadArgs);
    }
    // helper.go:140-147: 校验 sing-box 哈希
    let sb_data = verify_singbox_hash(src_dir, want_hash, bin_name)?;
    // helper.go:148-151: 枚举源目录
    let names = list_src_files(src_dir)?;
    // helper.go:156-178: 逐文件原子写入（bin_name 与上面校验的是同一个名字 —— 单一入参保证）
    atomic_install_files(src_dir, core_dir, &names, &sb_data, bin_name)?;
    // helper.go:179-192: 清理多余旧文件
    prune_extra_files(core_dir, &names);
    Ok(names)
}

/// 便利：判断给定路径是否可作为 core_dir（非空 + 父目录存在）。
#[must_use]
pub fn is_valid_core_dir(core_dir: &Path) -> bool {
    !core_dir.as_os_str().is_empty() && core_dir.parent().is_some_and(|p| p.exists())
}

// want_hash 校验（64 字符 hex，大小写不敏感）：直接用 proto 的单一真值
// [`polaris_helper_proto::codec::is_valid_sha256_hex`]，逐字对照 Go `len(wantHash) != 64`。
//
// 合并前此处内联过一份等价实现，自辩理由是「本 crate（helper-common）不依赖 proto」。三平台 helper
// 合并成单 crate 后 helper-common 已不存在，本 crate **确实依赖 proto**（mac/win/linux 三支本来就都
// 依赖）—— 那条理由随之失效，留着内联副本反而让注释变成谎话（正是审计 §G1.3 刚消灭的那种误导注释）。
// 且同 crate 内 linux 那支（platform::linux::core_installer）本就用的是 proto 这份，副本使一个 crate
// 里同时存在两份同语义校验。故删副本、统一到 proto。
//
// 注：这不动 helper-proto 的 `[dependencies]`（仍为空）—— 只是消费既有的 helper → proto 边。
use polaris_helper_proto::codec::is_valid_sha256_hex;

#[cfg(test)]
mod tests;

//! install-core 公共核心（mac/linux 逐字等价部分下沉）。
//!
//! 把 app 下载+预检的临时内核 src，校验 sha256 后 root 写入锁定的受保护目录 coreDir +
//! 逐文件 `.new + rename` 原子就位 + 清陈旧残留。移植自 上游 `helper/helper.go:127-198`
//!（mac）与 `helper-linux/helper.go:183-244`（linux），两份核心文件操作逐字同。
//!
//! ## 职责边界
//!
//! 只放**跨平台逐字等价**的逻辑：收源文件、sha256 校验、原子安装、通用 prune。OS 专属差异
//! （mac xattr/codesign、linux 按目录 fd 的清理）由各 helper crate 调本模块前后 hook。
//!
//! ## 安全约束
//!
//! - **只写锁定的 coreDir**（`filepath.Join`，不接受任意路径）→ 防「持 token 写任意 root 路径」。
//! - **源目录是低权限方摆的**，本进程以高权限读它，故收文件这一步（[`receive_src`]）只认：
//!   该平台的两个精确文件名（核 + 配套库，见 [`polaris_helper_proto::core_payload`]）、相对已
//!   持有的目录句柄不跟随地打开的单链接常规文件、（unix）属调用者所有且组与其他人不可写、
//!   有上限的个数与大小（[`SrcLimits`]）。任何一项不符拒掉**整个请求**，不是跳过那一个文件
//!   —— 跳过等于让客户端试出哪些名字能进。
//! - **落盘的字节就是参与哈希的字节**：每个源文件只打开一次；[`install_received`] 从那个句柄
//!   一块一块读，同一块既喂 sha256 又写进 `.new`，不把整个文件读进内存。收下之后源路径上再换成
//!   什么都与落盘内容无关（堵 TOCTOU）。
//! - **哈希不符则受保护目录不变**：比对发生在全部 `.new` 写完之后、任何一次 rename 之前；
//!   不符就删掉 `.new` 返回。写的过程中 `.new` 对别人不可读（unix 0600；Windows 独占打开），
//!   没通过比对的字节不会被第三方读走。
//! - **就位要么全新要么全旧**：旧文件先留一个备份名，再逐个 rename 就位；任一步失败把已就位的
//!   换回去。上一次被打断留下的 `.new` / 备份在下一次安装开头收拾掉。
//! - 哈希只约束「写入 == 客户端声明」，不约束客户端声明什么；配套文件没有哈希可比。
//!
//! ## 流程
//!
//! - [`sha256_hex`]：sha256 → hex（纯函数）。
//! - [`receive_src`]：打开源目录 → 名字/个数白名单 → 逐文件不跟随打开并判类型、属主与大小。
//!   到这一步为止**不碰受保护目录**，也不读文件内容。
//! - [`install_received`]：收拾上次残留 → 全部流式写成 `.new` 并比对核的 sha256 → fsync →
//!   可回滚地 rename（主二进制先行）。
//! - [`prune_extra_files`]：清受保护目录多余旧残留（通用版，keep_names 外全删，best-effort）。
//! - [`install_core_files`]：完整流程编排（参数校验 + 上三步），平台 hook 留各 helper crate。

mod src_dir;

use polaris_helper_proto::{core_payload, Platform};
use sha2::{Digest, Sha256};
use src_dir::SrcDir;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// sing-box 主二进制文件名（`helper.go:140,194`：`filepath.Join(srcDir, "sing-box")`）。
///
/// 这三个常量的值都取自 [`polaris_helper_proto::core_payload`]（各平台精确文件名的唯一真值），
/// 留着名字是因为平台模块按名字引用它们。
pub const SINGBOX_BIN_NAME: &str = core_payload::UNIX_CORE_FILENAME;

/// Windows 侧的 sing-box 主二进制文件名（PE 必须带 `.exe`，否则 `CreateProcessW` 找不到）。
///
/// 与 [`SINGBOX_BIN_NAME`] 并列而不是在函数里按 `cfg(windows)` 挑：本模块在 Linux 上也要能对
/// **两个**名字各跑一遍单测（三平台 helper 同 crate，win 分支的纯逻辑本就在本机测）。
pub const SINGBOX_BIN_NAME_WIN: &str = core_payload::WIN_CORE_FILENAME;

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
pub const CRONET_DLL_NAME_WIN: &str = core_payload::WIN_SIDECAR_FILENAME;

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
    ///
    /// 也承载对源目录**整体**的拒绝：属主不符、有白名单外或重复的名字、条目过多、总量超限。
    ReadDir(String),
    /// `ERR mkdir <err>`（mac `helper.go:153` / linux `:203`：创建 coreDir 失败）。
    Mkdir(String),
    /// `ERR read <name> <err>`（mac `helper.go:164` / linux `:215`：读配套文件失败）。
    ///
    /// 也承载对单个配套文件的拒绝：不是单链接常规文件、属主不符、超过大小上限。
    /// `name` 恒在白名单内。
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
    /// `to_wire_line` 换了形态，那时诚实回 `unknown`（失败向关），不能假装装好了。带尾文：裸的
    /// `ERR unknown` 在客户端眼里是「这个 helper 不认识 install-core」。
    #[must_use]
    pub fn to_response(&self) -> polaris_helper_proto::Response {
        use polaris_helper_proto::{Error as ProtoError, ErrorCode, Response, ResponseKind};
        if self.is_ok() {
            return Response::Ok(ResponseKind::Installed);
        }
        ProtoError::parse(&self.to_wire_line()).map_or_else(
            || {
                Response::Err(ProtoError::with_detail(
                    ErrorCode::Unknown,
                    "install-result",
                ))
            },
            Response::Err,
        )
    }
}

/// sha256 → 小写 hex（对照 Go `sha256.Sum256` + `hex.EncodeToString`）。
///
/// 纯函数，[`receive_src`] 内部复用，也供外部测试断言用。
/// 合并自原 linux `sha256_hex`（mac 原为内联 `Sha256::new` + `hex::encode`，等价）。
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// 源目录的个数与大小上限。
///
/// 客户端摆多少、摆多大本进程都得以高权限读完并写进受保护目录所在的盘，故收之前先封顶。
/// 生产恒用 [`SRC_LIMITS`]；做成参数是为了单测能用几十字节的文件走到每一条超限分支。
#[derive(Debug, Clone, Copy)]
pub struct SrcLimits {
    /// 源目录条目数上限（任何类型的条目都计数）。
    pub max_files: usize,
    /// 单个文件字节数上限。
    pub max_file_bytes: u64,
    /// 全部文件字节数之和的上限。
    pub max_total_bytes: u64,
}

/// 生产上限。现行发布产物：核 70–86 MB（最大的是 mac-arm64）、cronet 9–12 MB，一次提升恰好
/// 这两个文件（macOS 只有核）。单文件 256 MiB 约为现行核的 3 倍；总量取两个文件各顶到单文件
/// 上限；个数只为给枚举封顶（白名单本身最多放进两个名字）。
pub const SRC_LIMITS: SrcLimits = SrcLimits {
    max_files: 8,
    max_file_bytes: 256 << 20,
    max_total_bytes: 512 << 20,
};

/// 已收下的一组源文件（[`receive_src`] 的产物，[`install_received`] 的唯一输入）。
///
/// 持有的是**句柄**，不是路径：每个文件都是判过类型与属主的那个打开着的句柄，此后的读只走它。
#[derive(Debug)]
pub struct ReceivedSrc {
    platform: Platform,
    want_hash: String,
    bin: File,
    sidecars: Vec<(String, File)>,
    limits: SrcLimits,
}

fn over_limit() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "size over limit")
}

/// 枚举出来的条目名 → 本次要收的文件名（字母序）。
///
/// 任何一个名字不在白名单（含不是合法 UTF-8 的）即拒整单；同一个名字出现两次也拒 ——
/// 目录在枚举途中被改动时 `readdir` 可以把一个名字报两遍，照单全收会对同一个目标写两次。
/// 白名单外的名字**不回显**（它由客户端任取，可含换行）。
fn checked_names(raw: Vec<OsString>, platform: Platform) -> Result<Vec<String>, InstallResult> {
    let mut names = Vec::with_capacity(raw.len());
    for entry in raw {
        match entry.to_str() {
            Some(name) if core_payload::name_allowed(name, platform) => names.push(name.to_owned()),
            _ => return Err(InstallResult::ReadDir("entry name not allowed".into())),
        }
    }
    names.sort();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(InstallResult::ReadDir("duplicate entry name".into()));
    }
    Ok(names)
}

/// 收下源目录里的文件：只认该平台白名单名字的单链接常规文件。
///
/// `owner_uid` 是已鉴权的调用者 uid（linux 取自 `SO_PEERCRED`，macos 取自对端凭据）：源目录与
/// 每个文件都必须属它所有、且组与其他人不可写；unix 上给 `None` 一律不收。Windows 这条腿
/// 不判属主（见 `src_dir` 模块文档）。
///
/// 本函数不写任何东西、也不读文件内容；返回 `Err` 时受保护目录原样未动。sha256 的比对在
/// [`install_received`] 里边写边算。
///
/// 失败沿用既有的响应行前缀，不新增错误类别：源目录打不开或属主不符、有白名单外的名字、
/// 名字重复、条目过多、总量超限 → `readdir`；主二进制 → `read-singbox`；配套文件 → `read <name>`。
pub fn receive_src(
    src_dir: &Path,
    want_hash: &str,
    platform: Platform,
    owner_uid: Option<u32>,
    limits: SrcLimits,
) -> Result<ReceivedSrc, InstallResult> {
    let readdir = |e: io::Error| InstallResult::ReadDir(e.to_string());
    let mut dir = SrcDir::open(src_dir, owner_uid).map_err(readdir)?;
    let names = checked_names(
        dir.entry_names(limits.max_files).map_err(readdir)?,
        platform,
    )?;

    let bin_name = core_payload::core_filename(platform);
    let read_singbox = |e: io::Error| InstallResult::ReadSingbox(e.to_string());
    let (bin, bin_len) = dir.open_regular(bin_name).map_err(read_singbox)?;
    if bin_len > limits.max_file_bytes {
        return Err(read_singbox(over_limit()));
    }
    let mut total = bin_len;
    let mut sidecars = Vec::new();
    for name in names.into_iter().filter(|n| n != bin_name) {
        let read = |e: io::Error| InstallResult::Read {
            name: name.clone(),
            detail: e.to_string(),
        };
        let (file, len) = dir.open_regular(&name).map_err(read)?;
        if len > limits.max_file_bytes {
            return Err(read(over_limit()));
        }
        total = total.saturating_add(len);
        sidecars.push((name, file));
    }
    if total > limits.max_total_bytes {
        return Err(readdir(over_limit()));
    }
    Ok(ReceivedSrc {
        platform,
        want_hash: want_hash.to_owned(),
        bin,
        sidecars,
        limits,
    })
}

/// `<name>.new`：写入中的临时件。
fn tmp_path(core_dir: &Path, name: &str) -> PathBuf {
    core_dir.join(format!("{name}.new"))
}

/// `<name>.bak`：就位期间旧文件的备份名。
fn backup_path(core_dir: &Path, name: &str) -> PathBuf {
    core_dir.join(format!("{name}.bak"))
}

/// 收拾上一次安装被打断（进程被杀、掉电）留下的东西。
///
/// 备份还在说明那次就位没走完：生效文件缺了就把备份换回去，没缺就删掉备份。`.new` 一律删。
/// 只碰白名单名字派生出的这几个路径，不扫目录。
fn recover_interrupted(core_dir: &Path, platform: Platform) {
    let names = [
        Some(core_payload::core_filename(platform)),
        core_payload::sidecar_filename(platform),
    ];
    for name in names.into_iter().flatten() {
        let (dst, bak) = (core_dir.join(name), backup_path(core_dir, name));
        if fs::symlink_metadata(&bak).is_ok() {
            if fs::symlink_metadata(&dst).is_ok() {
                let _ = fs::remove_file(&bak);
            } else {
                let _ = fs::rename(&bak, &dst);
            }
        }
        let _ = fs::remove_file(tmp_path(core_dir, name));
    }
}

/// 在受保护目录里新建一个**只有本进程读得到**的 `tmp`（不沿用同名旧对象）。
///
/// 写进去的字节在比对 sha256 之前还没通过任何校验，而受保护目录对所有用户可读：unix 上先建成
/// 0600，Windows 上以独占方式打开（句柄关掉之前别的进程打不开它）。
fn create_private(tmp: &Path) -> io::Result<File> {
    // `create_new` 保证拿到的是新建的文件，不会顺着一个既有的链接写到别处。
    let _ = fs::remove_file(tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(tmp)
}

/// 从 `src` 读到 EOF：每一块先喂哈希、再写进 `out`，返回 (字节数, sha256 hex)。
///
/// 超过 `max` 字节即失败（文件在判过大小之后仍可能变长）。读与写的错误分开报，调用方据此
/// 选响应行前缀。
fn copy_hashing(mut src: &File, out: &mut File, max: u64) -> Result<(u64, String), CopyError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => return Ok((total, hex::encode(hasher.finalize()))),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CopyError::Read(e)),
        };
        total += n as u64;
        if total > max {
            return Err(CopyError::Read(over_limit()));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(CopyError::Write)?;
    }
}

enum CopyError {
    Read(io::Error),
    Write(io::Error),
}

/// 一个已写好 `.new` 的文件。
struct Staged {
    name: String,
    tmp: PathBuf,
    dst: PathBuf,
    bak: PathBuf,
    /// 写阶段持有；就位前关掉（Windows 上开着的句柄会挡住 rename）。
    file: Option<File>,
}

/// 丢弃写阶段的全部产物。先截成空再删：Windows 上句柄一关别人就能打开它。
fn discard(staged: &mut [Staged]) {
    for item in staged {
        if let Some(file) = item.file.take() {
            let _ = file.set_len(0);
        }
        let _ = fs::remove_file(&item.tmp);
    }
}

/// 尽力把目录项的变更落盘。Windows 上 std 打不开目录句柄，跳过。
fn sync_dir(core_dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(core_dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = core_dir;
}

/// 把收下的文件装进受保护目录（移植自 `helper.go:156-178`），返回装入的文件名（字母序）。
///
/// 1. 收拾上一次被打断的残留（`recover_interrupted`）。
/// 2. **写**：每个文件从收下时的句柄流式写成 `<name>.new`，核同时算 sha256。任何一步失败、
///    或核的 sha256 与请求不符，删掉全部 `.new` 返回 —— 没有任何生效文件被改动。
/// 3. **落盘**：`.new` 定权限 0755、`fsync`，再 `fsync` 目录。掉电后不会剩一个 rename 过去了、
///    内容却没写下去的空核。
/// 4. **就位**：逐个文件「旧文件硬链出一个 `<name>.bak` → `.new` rename 到正式名」。任一步失败，
///    把已就位的换回备份（原先没有的就删掉），结果与安装前相同。全部就位后删备份。
///    用硬链而不是把旧文件改名挪开：正式名在整个过程中始终指向一个完整的文件。
///
/// ## 就位时 `bin_name` 恒第一个（**不是**字母序）
///
/// 装核过了判活闸后锁就放了，并发 start 把核起起来时，`libcronet.dll`（字母序在 `sing-box.exe`
/// 之前）**只在 cronet-naive 出站被用到时才加载** ⇒ 它的 rename 成功、`sing-box.exe` 的失败。
/// 我方受管核恒持有 exe 句柄 ⇒ 把核排到最前，撞锁时**第一个** rename 就失败，后面的不必动、
/// 也就不必回滚。unix 侧 rename 恒成功（旧 inode 仍被运行中的进程持有），顺序无行为影响。
pub fn install_received(core_dir: &Path, src: ReceivedSrc) -> Result<Vec<String>, InstallResult> {
    let created = fs::symlink_metadata(core_dir).is_err();
    // helper.go:152-154: MkdirAll(coreDir, 0755)
    fs::create_dir_all(core_dir).map_err(|e| InstallResult::Mkdir(e.to_string()))?;
    let result = install_into(core_dir, &src);
    // 这次才建出来的目录，装失败了就收回去（`remove_dir` 只删得掉空目录）：被拒的请求不在
    // 受保护位置留下任何东西，连一个空目录也不留。
    if result.is_err() && created {
        let _ = fs::remove_dir(core_dir);
    }
    result
}

/// 写阶段：每个文件从收下时的句柄流式写成 `<name>.new`，并比对核的 sha256。
///
/// 返回时每个 `.new` **仍是只有本进程读得到的**（句柄还开着、权限还没放开）：放开权限是
/// [`seal`] 的事，而它只在本函数成功返回之后才被调用 —— 没通过比对的字节从头到尾不会以
/// 他人可读的形态出现在受保护目录里。失败时已写的 `.new` 全部删掉。
fn write_tmp_files(core_dir: &Path, src: &ReceivedSrc) -> Result<Vec<Staged>, InstallResult> {
    let bin_name = core_payload::core_filename(src.platform);
    let mut staged: Vec<Staged> = Vec::new();
    let mut budget = src.limits.max_total_bytes;
    // 核排第一（见函数文档），其余保持字母序。
    let sources = std::iter::once((bin_name, &src.bin)).chain(
        src.sidecars
            .iter()
            .map(|(name, file)| (name.as_str(), file)),
    );
    for (name, source) in sources {
        let write = |e: io::Error| InstallResult::Write {
            name: name.to_owned(),
            detail: e.to_string(),
        };
        let tmp = tmp_path(core_dir, name);
        let mut out = match create_private(&tmp) {
            Ok(out) => out,
            Err(e) => {
                discard(&mut staged);
                return Err(write(e));
            }
        };
        // 读的是收下时那个句柄；上限在这里按实际字节数卡。
        let copied = copy_hashing(source, &mut out, budget.min(src.limits.max_file_bytes));
        staged.push(Staged {
            name: name.to_owned(),
            tmp,
            dst: core_dir.join(name),
            bak: backup_path(core_dir, name),
            file: Some(out),
        });
        let (len, hash) = match copied {
            Ok(done) => done,
            Err(error) => {
                discard(&mut staged);
                return Err(match error {
                    CopyError::Write(e) => write(e),
                    CopyError::Read(e) if name == bin_name => {
                        InstallResult::ReadSingbox(e.to_string())
                    }
                    CopyError::Read(e) => InstallResult::Read {
                        name: name.to_owned(),
                        detail: e.to_string(),
                    },
                });
            }
        };
        budget -= len;
        // helper.go:144-146: sha256.Sum256 + hex + EqualFold
        if name == bin_name && !hash.eq_ignore_ascii_case(&src.want_hash) {
            discard(&mut staged);
            return Err(InstallResult::HashMismatch);
        }
    }
    Ok(staged)
}

fn install_into(core_dir: &Path, src: &ReceivedSrc) -> Result<Vec<String>, InstallResult> {
    recover_interrupted(core_dir, src.platform);
    let mut staged = write_tmp_files(core_dir, src)?;

    for index in 0..staged.len() {
        if let Err(e) = seal(&mut staged[index]) {
            let name = staged[index].name.clone();
            discard(&mut staged);
            return Err(InstallResult::Write {
                name,
                detail: e.to_string(),
            });
        }
    }
    sync_dir(core_dir);

    for index in 0..staged.len() {
        if let Err(e) = put_in_place(&staged[index]) {
            roll_back(&staged[..index]);
            let _ = fs::remove_file(&staged[index].bak);
            discard(&mut staged[index..]);
            sync_dir(core_dir);
            return Err(InstallResult::Rename {
                name: staged[index].name.clone(),
                detail: e.to_string(),
            });
        }
    }
    for item in &staged {
        let _ = fs::remove_file(&item.bak);
    }
    sync_dir(core_dir);

    let mut names: Vec<String> = staged.into_iter().map(|item| item.name).collect();
    names.sort();
    Ok(names)
}

/// 写完的 `.new`：定权限 0755（helper.go:177，按句柄改、不受 umask 影响）、`fsync`、关句柄。
fn seal(item: &mut Staged) -> io::Result<()> {
    let Some(file) = item.file.take() else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all()
}

/// 一个文件就位：旧文件（若有）硬链出备份，再把 `.new` rename 到正式名。
fn put_in_place(item: &Staged) -> io::Result<()> {
    if fs::symlink_metadata(&item.dst).is_ok() {
        fs::hard_link(&item.dst, &item.bak)?;
    }
    // helper.go:173-176: Rename(tmp, dst)
    fs::rename(&item.tmp, &item.dst)
}

/// 把已就位的文件换回安装前的样子：有备份的 rename 回去，原先没有的删掉。
fn roll_back(placed: &[Staged]) {
    for item in placed.iter().rev() {
        if fs::symlink_metadata(&item.bak).is_ok() {
            let _ = fs::rename(&item.bak, &item.dst);
        } else {
            let _ = fs::remove_file(&item.dst);
        }
    }
}

/// 清理受保护目录多余文件（移植自 `helper.go:179-192`，通用版）。
///
/// 删除 core_dir 中不在 keep_names 的旧文件（防 rollback 后残留陈旧配套）。
/// best-effort，单项失败跳过。mac/windows 用；linux 的受保护目录走自己那份按目录 fd 的清理。
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

/// 完整 install-core 流程编排（不含 mac 专属 xattr/codesign —— 由 helper 在调用本函数后 hook；
/// linux 用自己的清理，不走本函数）。
///
/// 对照 Go `installCore`(`helper.go:133-198`) 的文件操作部分：
/// 1. 参数校验（core_dir 非空、src_dir 非空、want_hash 64 字符 hex）
/// 2. 收源文件（[`receive_src`]）
/// 3. mkdir core_dir + 校验 sha256 + 原子就位（[`install_received`]）
/// 4. 清理多余旧文件（通用 prune）
///
/// 核文件名由 `platform` 定（[`polaris_helper_proto::core_payload::core_filename`]）。
/// 成功返回本次装入的文件名列表。
pub fn install_core_files(
    core_dir: &Path,
    src_dir: &Path,
    want_hash: &str,
    platform: Platform,
    owner_uid: Option<u32>,
) -> Result<Vec<String>, InstallResult> {
    // helper.go:134-138: 参数校验
    if core_dir.as_os_str().is_empty() {
        return Err(InstallResult::CoreDirUnset);
    }
    if src_dir.as_os_str().is_empty() || !is_valid_sha256_hex(want_hash) {
        return Err(InstallResult::BadArgs);
    }
    let received = receive_src(src_dir, want_hash, platform, owner_uid, SRC_LIMITS)?;
    let names = install_received(core_dir, received)?;
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

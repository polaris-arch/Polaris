//! 下载错误类型 + FS trait 抽象（不在本 crate 触碰宿主网络/FS）。
//!
//! - [`DownloadError`]：下载失败的统一形状。下载本身由宿主实现
//!   （`src-tauri/src/runtime/http.rs` 的 `CoreDownloader`），本 crate 只定义错误类型。
//! - [`UpdateFs`]：下载目录的文件读写 + 原子 rename。生产实现包 std::fs；测试注入 `MockFs`。

use std::path::{Path, PathBuf};

use thiserror::Error;

/// 下载错误（trait 实现的失败统一形状，移植自 Polaris 下载失败分类）。
#[derive(Debug, Error)]
pub enum DownloadError {
    /// HTTP 非 2xx（= 上游 `Download failed: HTTP {code}`）。
    #[error("download failed: HTTP {0}")]
    HttpStatus(u16),
    /// 下载不完整：实收字节与期望不符（= 上游 `下载不完整：收到 {n} 字节，期望 {m}`）。
    #[error("download incomplete: received {received} bytes, expected {expected}")]
    Incomplete { received: u64, expected: u64 },
    /// 停滞超时（= 上游 `下载停滞超时（30s 无数据）`）。
    #[error("download stalled: no data for {0}ms")]
    Stalled(u64),
    /// 网络/IO 错误（= Polaris request 'error' / response 'error'）。
    #[error("download io error: {0}")]
    Io(#[from] std::io::Error),
    /// 其它（如 mirror 全部失败、URL 非法）。
    #[error("download error: {0}")]
    Other(String),
}

/// FS trait：staged 暂存 + 目标落位 + 原子 rename（移植自 `CoreUpdateService.installCoreFromDir`
/// 的非 Windows staged-rename 落位，`CoreUpdateService.ts:453-479`）。
///
/// 抽象出的最小 FS 面：写临时文件 + 原子 rename + 读/删/列目录。生产实现包 std::fs；测试可注入
/// tempfile 或 in-memory FS。**trait 不暴露权限/签名操作**（macOS codesign / Windows UAC 由宿主
/// privilege 层负责，超出本纯逻辑 crate 边界）。
pub trait UpdateFs {
    /// 把 `bytes` 写到 `path`（覆盖已存在）。= 上游 `fs.writeFileSync` / `fs.copyFileSync`。
    ///
    /// 调用方负责确保父目录存在（经 [`UpdateFs::create_dir_all`]）。
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error>;

    /// 打开 `path` 的**流式写句柄**（截断已存在内容；父目录须已存在，同 [`UpdateFs::write`]）。
    ///
    /// # 为什么 `write` 不够用
    ///
    /// [`UpdateFs::write`] 吃 `&[u8]` ⇒ 调用方必须先把整个负载攒在内存里。App 安装包是几十 MiB
    /// 到上百 MiB 量级，「整包入内存再落盘」把内存峰值与包体积绑死，还逼下载腿在校验前
    /// 一直持有全部字节。有了写句柄，下载腿才能边收边写、边写边算摘要。
    ///
    /// # 为什么返回 `Box<dyn std::io::Write + Send>` 而不是自造句柄 trait
    ///
    /// `std::io::Write` 就是 stdlib 给写句柄定的契约（`write_all` / `flush` 齐备），
    /// 再定义一个同形的本地 trait 只会多一层适配（简约阶梯：stdlib 优先）。
    /// `Send` 是硬要求 —— 句柄要被移进下载 task。
    ///
    /// # Errors
    ///
    /// 透传底层 IO 错误（父目录不存在 / 无权限 / 磁盘满）。
    fn open_write(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>, std::io::Error>;

    /// 把 `path` 已写出的内容**刷到存储介质**（`fsync`），返回后内容才算真的落了盘。
    ///
    /// # 为什么落位路径需要它
    ///
    /// 「写完 → rename」中间没有 `sync` 时，断电 / 内核崩溃后完全可能出现「dest 这个名字在、
    /// 内容却是零或半截」：rename 只保证目录项的原子替换，不保证被替换的那个 inode 的**数据**
    /// 已经离开 page cache。而 dest 一旦存在就会被复用判定与安装腿当成完整包
    /// （`commands/updater.rs` 的 `cached_download_is_reusable` / `update_install`）。
    ///
    /// # 实现须持**写句柄**
    ///
    /// Windows 的 `FlushFileBuffers` 要求句柄带写权限，只读句柄会直接失败；
    /// 故 [`StdFs`] 用 `OpenOptions::new().write(true)` 而不是 `File::open`。
    ///
    /// # Errors
    ///
    /// 透传底层 IO 错误（文件不存在 / 无写权限 / 介质错误）。
    fn sync_file(&self, path: &Path) -> Result<(), std::io::Error>;

    /// 读取 `path` 全部字节。= 上游 `fs.readFileSync`。
    fn read(&self, path: &Path) -> Result<Vec<u8>, std::io::Error>;

    /// 递归创建目录（已存在则 no-op）。= 上游 `fs.mkdirSync({ recursive: true })`。
    fn create_dir_all(&self, path: &Path) -> Result<(), std::io::Error>;

    /// 原子 rename：`from` → `to`（同目录 rename 为原子 syscall，覆盖 `to`）。
    /// = 上游 `fs.renameSync(tmp, dest)`（staged-rename 落位的最后一步）。
    ///
    /// 失败时调用方（[`promote_staged`](crate::verify::promote_staged)）负责删 `from` 残件。
    fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error>;

    /// 删除文件（不存在则 no-op，对齐 上游 `if (fs.existsSync(p)) fs.unlinkSync(p)`）。= 上游 `fs.unlinkSync`。
    fn remove_file(&self, path: &Path) -> Result<(), std::io::Error>;

    /// 递归删除目录（不存在则 no-op）。= 上游 `fs.rmSync({ recursive: true, force: true })`。
    fn remove_dir_all(&self, path: &Path) -> Result<(), std::io::Error>;

    /// 列出 `dir` 下的**文件**名（跳过子目录）。= 上游 `fs.readdirSync(dir).filter(isFile)`
    /// （`CoreUpdateService.installCoreFromDir:443-445` 的 staged-rename 文件清单来源）。
    fn list_files(&self, dir: &Path) -> Result<Vec<String>, std::io::Error>;

    /// 文件是否存在。= 上游 `fs.existsSync`。
    fn exists(&self, path: &Path) -> bool;

    /// 拼接路径的便利方法（trait object 友好：避免调用方到处 `path.join`）。
    fn join(&self, base: &Path, name: &str) -> PathBuf {
        base.join(name)
    }
}

/// 真实 std::fs 实现的生产 FS（测试也直接可用 tempfile 驱动，无需另造 mock）。
///
/// 设计取舍：Polaris 的 FS 操作就是直接调 Node `fs`，没有抽象层。本 crate 把它抽象成 trait 主要为
/// 让 staged 周期可注入「失败 mock」（测错误恢复路径），而非为替换 std::fs。因此生产实现 = std::fs
/// 直接转发，测试用 `MockFs`（测试替身）注入受控失败或 tempfile 隔离。
#[derive(Debug, Default, Clone, Copy)]
pub struct StdFs;

impl UpdateFs for StdFs {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
        std::fs::write(path, bytes)
    }

    fn open_write(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>, std::io::Error> {
        // `File::create` = 建新 / 截断已存在，与 `write` 的覆盖语义一致。
        Ok(Box::new(std::fs::File::create(path)?))
    }

    fn sync_file(&self, path: &Path) -> Result<(), std::io::Error> {
        // **必须带写权限**：Windows 的 `FlushFileBuffers` 对只读句柄直接失败（见 trait 文档）。
        // 不用 `File::create`（会截断）也不用 `append`（无谓地改变文件偏移语义）。
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .sync_all()
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, std::io::Error> {
        std::fs::read(path)
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
        std::fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> Result<(), std::io::Error> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            // 不存在视为成功（对齐 上游 `if exists then unlink` 的 no-op 语义）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), std::io::Error> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn list_files(&self, dir: &Path) -> Result<Vec<String>, std::io::Error> {
        let mut files = Vec::new();
        for ent in std::fs::read_dir(dir)? {
            let ent = ent?;
            if ent.file_type()?.is_file() {
                if let Some(name) = ent.file_name().to_str() {
                    files.push(name.to_string());
                }
            }
        }
        files.sort();
        Ok(files)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Mock FS（测试用）：包一个真实 [`StdFs`] 但根植于给定 tmp 目录，并支持注入「下一次 op 失败」。
///
/// 用真实 std::fs + tempfile 隔离目录（而非 in-memory map）：这样原子 rename / 权限等 std 行为
/// 与生产一致，mock 只额外提供「受控失败」和「根目录沙箱」两个测试杠杆。
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug)]
pub struct MockFs<'a> {
    /// 沙箱根：所有相对路径解析于此（绝对路径直传 std）。None = 不沙箱（= StdFs）。
    pub sandbox: Option<&'a Path>,
    /// 注入：下一次指定 op 返回此错误（一次性，触发后清空）。用于测错误恢复。
    pub next_fail: Option<MockFailOp>,
}

/// mock FS 可注入失败的 op 种类（测 staged 周期各阶段错误恢复）。
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockFailOp {
    Write,
    Rename,
    Remove,
    Read,
    List,
    /// 落位前的 `fsync` 失败（介质错误 / 磁盘满在 flush 时才暴露）。
    ///
    /// **不与 [`Self::Write`] 合流**：`write` 成功而 `sync` 失败正是本注入要覆盖的那一格
    /// —— 合流的话「写都写不进去」会盖掉「写进了 page cache 但没落到介质」这条腿。
    SyncFile,
}

#[cfg(any(test, feature = "test-utils"))]
impl<'a> MockFs<'a> {
    #[must_use]
    pub fn new(sandbox: &'a Path) -> Self {
        Self {
            sandbox: Some(sandbox),
            next_fail: None,
        }
    }

    /// 注入「下一次 `op` 永久失败」（命中后不清除，覆盖该 op 所有后续调用）。
    ///
    /// 注：trait 方法是 `&self`，无法 take（一次性）——故采用「永久失败某 op」语义。
    /// 测试如需「一次性失败」请用 `Rc<RefCell<MockFs>>` 自行控制；本 MockFs 覆盖 staged 错误恢复路径足够。
    pub fn fail_next(&mut self, op: MockFailOp) {
        self.next_fail = Some(op);
    }

    /// 解析路径：sandbox 存在且 path 相对时拼到 sandbox 下；绝对路径直传。
    fn resolve(&self, path: &Path) -> std::path::PathBuf {
        match self.sandbox {
            Some(root) if !path.is_absolute() => root.join(path),
            _ => path.to_path_buf(),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<'a> UpdateFs for MockFs<'a> {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
        // 注：MockFs::fail_next 需 &mut self，但 trait 方法是 &self。为保持 trait object 友好，
        // 此处用 Cell-like 手法（RefCell）会引入开销；改用内部可变性的标准做法是包 Rc<RefCell<MockFs>>。
        // 本 crate 测试直接用 StdFs + tempfile（见 staged 测试），MockFs 仅作为「可注入失败」的轻量占位，
        // 失败注入经 fail_next 预设、take_fail 在此处读 —— 但 &self 无法 take。
        // 结论：MockFs 的 fail 注入需要内部可变性。用 RefCell 包装 next_fail。
        // —— 为避免在 trait 签名上引入 &mut，这里把 next_fail 改成运行时检查（若 next_fail 命中则报错，
        // 但不清除，因无法 &mut）。测试如需「一次性失败」请用 Rc<RefCell<MockFs>> 自行控制。
        // 简化：本 MockFs 仅做 sandbox 解析 + 可选的「永久失败某 op」（不清除），覆盖 staged 错误恢复测试。
        if matches!(self.next_fail, Some(MockFailOp::Write)) {
            return Err(std::io::Error::other("mock injected failure: Write"));
        }
        std::fs::write(self.resolve(path), bytes)
    }

    fn open_write(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>, std::io::Error> {
        // 与 `write` 同归 `MockFailOp::Write`：二者是同一件事（往 path 写内容）的两种粒度，
        // 分成两个注入口会让「写失败」的测试覆盖按调用形式分叉。
        if matches!(self.next_fail, Some(MockFailOp::Write)) {
            return Err(std::io::Error::other("mock injected failure: Write"));
        }
        Ok(Box::new(std::fs::File::create(self.resolve(path))?))
    }

    fn sync_file(&self, path: &Path) -> Result<(), std::io::Error> {
        if matches!(self.next_fail, Some(MockFailOp::SyncFile)) {
            return Err(std::io::Error::other("mock injected failure: SyncFile"));
        }
        // 未注入失败时**真的做一次 fsync**（不是无脑 Ok）：这样「文件根本不存在就调 sync」
        // 在 mock 上同样会报错，与 StdFs 的行为一致。
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.resolve(path))?
            .sync_all()
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, std::io::Error> {
        if matches!(self.next_fail, Some(MockFailOp::Read)) {
            return Err(std::io::Error::other("mock injected failure: Read"));
        }
        std::fs::read(self.resolve(path))
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(self.resolve(path))
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
        if matches!(self.next_fail, Some(MockFailOp::Rename)) {
            return Err(std::io::Error::other("mock injected failure: Rename"));
        }
        std::fs::rename(self.resolve(from), self.resolve(to))
    }

    fn remove_file(&self, path: &Path) -> Result<(), std::io::Error> {
        if matches!(self.next_fail, Some(MockFailOp::Remove)) {
            // 注：staged 周期里 remove 失败多为 best-effort（清残件），不应阻断流程。
            // 但为测错误传播，这里仍报错；调用方 staged 逻辑对残件清理失败已吞掉。
            return Err(std::io::Error::other("mock injected failure: Remove"));
        }
        match std::fs::remove_file(self.resolve(path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), std::io::Error> {
        match std::fs::remove_dir_all(self.resolve(path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn list_files(&self, dir: &Path) -> Result<Vec<String>, std::io::Error> {
        if matches!(self.next_fail, Some(MockFailOp::List)) {
            return Err(std::io::Error::other("mock injected failure: List"));
        }
        let mut files = Vec::new();
        for ent in std::fs::read_dir(self.resolve(dir))? {
            let ent = ent?;
            if ent.file_type()?.is_file() {
                if let Some(name) = ent.file_name().to_str() {
                    files.push(name.to_string());
                }
            }
        }
        files.sort();
        Ok(files)
    }

    fn exists(&self, path: &Path) -> bool {
        self.resolve(path).exists()
    }
}

#[cfg(test)]
mod tests;

//! SHA256 校验纯逻辑 + 「已在盘的临时文件 → rename 就位」的落位编排。
//!
//! 校验吃增量喂入（或已算好的摘要）+ 期望 hash；落位吃 [`UpdateFs`] trait 注入，均可单测。
//! 真实 sha256 算法由 `sha2` crate 提供（与 helper 同口径）。

use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::traits::UpdateFs;

// 去重：复用 polaris-helper-proto 的 is_valid_sha256_hex（旧 verify.rs 本地副本已删）。
// `pub use` 使 `crate::verify::is_valid_sha256_hex`（manifest.rs 调用路径）+ 本模块内调用都解析到 helper-proto 版。
pub use polaris_helper_proto::codec::is_valid_sha256_hex;

/// SHA256 校验错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerifyError {
    /// 期望的 hash 不是合法的 64 字符 hex（移植自 `helper.go:137` 的 `len(wantHash) != 64` 校验）。
    #[error("invalid expected hash: not 64-char hex (got {0} chars)")]
    InvalidExpectedHash(usize),
    /// 字节流/文件的实际 hash 与期望不符（移植自 `helper.go:146` 的 hash-mismatch）。
    #[error("hash mismatch: expected {expected}, actual {actual}")]
    HashMismatch { expected: String, actual: String },
}

/// 计算字节的 SHA256 hex（小写，移植自 `file-hash.ts:sha256File` 的 `.digest('hex')`）。
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// `is_valid_sha256_hex`（校验 64 字符合法 hex）由本模块顶部的 `pub use` 从
// `polaris_helper_proto::codec` re-export，故 `crate::verify::is_valid_sha256_hex`（manifest.rs 的调用路径）继续可用。
// 用 `//` 而非 `///`：doc 注释必须挂在 item 上，此处后面并无 item（原为悬空 doc → clippy `empty_line_after_doc_comments`）。

/// 比对**已经算好**的实际摘要与期望摘要 —— 全 crate 摘要判定的**单点**。
///
/// # 为什么必须是单点（这不是洁癖）
///
/// 判定本身只有两行，但它有**两个变体**（[`VerifyError::InvalidExpectedHash`] =
/// 发布方把摘要写坏了 / [`VerifyError::HashMismatch`] = 包与摘要对不上），而两者的**处置相反**：
/// 前者重下一万次也不会好，后者才值得让用户重试。手搓一份
/// `!is_valid_sha256_hex(..) || !eq_ignore_ascii_case(..)` 就把这条分野压成了一个 bool ——
/// 调用方只能报一句「可能被截断或篡改」，把发布方的失误显示成投毒警告（生产的
/// `update_download` 腿此前正是这个形态）。
///
/// [`Sha256Stream::verify`] 委托本函数：各写一份必然在
/// 「大小写敏不敏感」「先验格式还是先比对」上分叉，而分叉只在真机大包上暴露。
///
/// # Errors
///
/// - [`VerifyError::InvalidExpectedHash`]：`expected_hex` 非 64 字符 hex。
/// - [`VerifyError::HashMismatch`]：实际 hash 与期望不符（大小写不敏感比对）。
pub fn verify_hex_digest(actual_hex: &str, expected_hex: &str) -> Result<(), VerifyError> {
    if !is_valid_sha256_hex(expected_hex) {
        return Err(VerifyError::InvalidExpectedHash(expected_hex.len()));
    }
    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(VerifyError::HashMismatch {
            expected: expected_hex.to_string(),
            actual: actual_hex.to_string(),
        })
    }
}

/// **增量** SHA-256：边收边算，不要求把整个负载留在内存里。
///
/// 应用安装包几十 MiB 到上百 MiB，「整包入内存再校验落盘」会把内存峰值与包体积绑死；
/// 流式下载边写边算，校验不需要再读一遍。
///
/// [`Self::verify`] 先验期望 hex 的格式（[`VerifyError::InvalidExpectedHash`]）、
/// 再做大小写不敏感比对（[`VerifyError::HashMismatch`]），判定委托 [`verify_hex_digest`]。
#[derive(Debug, Clone, Default)]
pub struct Sha256Stream {
    hasher: Sha256,
    len: u64,
}

impl Sha256Stream {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一段字节（可调任意多次；分片方式不影响最终摘要）。
    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
        self.len += bytes.len() as u64;
    }

    /// 已喂入的累计字节数。
    ///
    /// **生产消费点**：流式下载腿拿它与网络侧独立维护的 `received` 互校
    /// （`runtime/http.rs` 的 `HashingSink::finish` → `download_to_sink_with_progress`）。
    /// 两个计数分别由「网络收了多少」与「sink 真吃下多少」维护，对不上就说明
    /// 中间有一段字节没进 hasher —— 那会让摘要算在一份与盘上不同的内容上。
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// 是否一个字节都没喂过。
    ///
    /// 无生产调用点（如实登记）：clippy 的 `len_without_is_empty` 要求与 [`Self::len`] 配对，
    /// 单测也用它断言「空输入」。**不删**是因为删了 `len` 就得一并抑制那条 lint。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 结束累加，返回小写 hex 摘要（与 [`sha256_hex`] 同口径）。
    #[must_use]
    pub fn finish(self) -> String {
        hex::encode(self.hasher.finalize())
    }

    /// 结束累加并与期望摘要比对（大小写不敏感）。
    ///
    /// 判定委托 [`verify_hex_digest`]（全 crate 单点），本方法不留自己的比较逻辑。
    ///
    /// # Errors
    ///
    /// - [`VerifyError::InvalidExpectedHash`]：`expected_hex` 非 64 字符 hex。
    /// - [`VerifyError::HashMismatch`]：实际 hash 与期望不符。
    pub fn verify(self, expected_hex: &str) -> Result<(), VerifyError> {
        verify_hex_digest(&self.finish(), expected_hex)
    }
}

/// 流式计算一个 reader 的 SHA-256 hex（**不把内容整块读进内存**）。
///
/// 用于「文件已在盘、只想知道它的摘要」的场景（如复用判定）：`std::fs::read` + [`sha256_hex`]
/// 会为一次判定把整包搬进内存，而判定本身只需要 64 字节的结论。
///
/// # Errors
///
/// 透传 reader 的 IO 错误。
pub fn sha256_reader_hex<R: std::io::Read>(mut reader: R) -> std::io::Result<String> {
    // 64 KiB：足够摊薄 syscall 开销，又不会在栈/堆上占显眼的一块。
    let mut buf = vec![0u8; 64 * 1024];
    let mut stream = Sha256Stream::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(stream.finish()),
            Ok(n) => stream.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// 把**已经在盘**的临时文件提升为 `dest`：只做一次同目录 rename，**绝不把内容读回内存**。
///
/// 调用方刚流式写完一个几十 MiB 的临时文件；落位只需一次 rename，把整个文件读回内存再写一遍
/// 会抵消掉流式下载省下的内存与一次全量写。
///
/// `tmp` 必须由 [`tmp_name`] 生成（保证与 `dest` **同目录同卷** —— 跨卷 rename 不是原子操作，
/// 在 Windows 上还会直接失败）。
///
/// 失败语义：rename 失败即删 tmp 残件后抛出
/// （残件清理失败被吞）。dest 要么保持原样、要么是完整的新内容，**不存在半截态**。
///
/// # Errors
///
/// 透传 [`UpdateFs::rename`] 的 IO 错误。
pub fn promote_staged(fs: &dyn UpdateFs, tmp: &Path, dest: &Path) -> Result<(), std::io::Error> {
    if let Err(e) = fs.rename(tmp, dest) {
        let _ = fs.remove_file(tmp);
        return Err(e);
    }
    Ok(())
}

/// 生成**每次调用都不同**的临时名：`{dest}.polaris-new-{pid}-{seq}`。
///
/// # 为什么不能沿用固定的 `.polaris-new`
///
/// 固定名让「同一个 `dest` 的两次并发原子替换」互相踩：A 的 `write` 还没写完，B 的 `write`
/// 以 truncate 打开**同一个** tmp 从头覆盖 → 随后任一方的 `rename` 都可能把一个长度不对的
/// 半截文件搬成 `dest`。而先返回的那一方已经报了「成功 + 已校验」——校验对象是内存里的字节，
/// **不是**落盘后的文件，故这种破损完全不会被现有校验拦住（安装时才炸）。
///
/// 并发不是异常路径：`autoDownloadUpdate` 开启时，启动腿在后台下载的同时弹 remind 窗邀请用户
/// 点「更新」，两条腿写的正是同一个 dest。加唯一后缀后，两条腿各写各的 tmp，`dest` 恒是
/// 某一方的**完整**内容。
///
/// 上游 原名是 `${dest}.polaris-new`（`CoreUpdateService.ts:462`）；此处刻意分叉并留档。
///
/// **已知代价（如实登记）**：进程在 write 与 rename 之间被硬杀会留下带唯一后缀的残件
/// （固定名那版会被下一次写入覆盖掉）。两条正常失败路径（write / rename 报错）都会主动删。
///
/// 残件没有整目录重建来兜底：下载落在 `<cache>/updates/`，且主触发器不是硬崩 ——
/// `update_download` 是 async command，tmp 建立后唯一的 await 点是 `spawn_blocking(...).await`，
/// 下载途中退出 App 会让 tauri runtime **drop 掉那个 future**，清理全被绕过，而 blocking 线程
/// 仍可能把 tmp 写完。故调用方必须自带清扫（`commands/updater.rs` 的 `sweep_orphan_downloads`）。
///
/// `pub`：流式下载要**先**拿到 tmp 路径（下载直接写它）、再交 [`promote_staged`] 提升。
/// tmp 命名只此一份：各造一份必然在「是否与 dest 同目录」上分叉，而那正是原子性的前提
/// （由 `tmp_name_is_unique_per_call` 的同目录断言锁死）。
#[must_use]
pub fn tmp_name(dest: &Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    /// 进程内单调序号：同一毫秒内的多次调用也不会撞名。
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut s = dest.as_os_str().to_os_string();
    s.push(format!(".polaris-new-{}-{seq}", std::process::id()));
    std::path::PathBuf::from(s)
}

#[cfg(test)]
mod tests;

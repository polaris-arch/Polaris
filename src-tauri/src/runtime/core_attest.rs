//! Runtime process/version observation. Content identity is enforced separately
//! by the compiled bundle manifest and pinned helper receipt; version is diagnostic.
use std::path::{Path, PathBuf};

/// 「实跑二进制 == 本次期望核」的自证结论。
///
/// # 与 `proxy::attest_effective_exit`（出口自证）的**关键区别**
///
/// 那一条是**纯静态对账**（自述「纯函数、零 I/O」「不用探针」）：拿本次生成的 config 与落盘的用户
/// 意图互校 —— 两个输入同源于「意图」，故**意图正确而事实偏离**时它一律判通过。本条不重蹈：
/// 唯一的输入 `running` 来自**内核对该 pid 的记账**（linux `/proc/<pid>/exe`、mac `ps -o comm=`），
/// 版本来自**对那个文件真跑一次 `version`**。两者都是事实，不是意图的副本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreBinaryAttestation {
    /// 实跑 exe 就是本次解析出的那个文件 → 版本无需再问（app 直起腿的稳态）。
    SamePath,
    /// 路径不同但版本一致 → 受保护核已是现役核的副本（TUN 提权腿修好后的稳态）。
    SameVersion {
        /// 实跑二进制路径。
        running: PathBuf,
        /// 双方一致的版本行。
        version: String,
    },
    /// **版本不一致** —— 今天这个缺陷的正面命中（跑 alpha.45 而期望 beta.3）。
    VersionMismatch {
        /// 实跑二进制路径。
        running: PathBuf,
        /// 实跑二进制自报版本行。
        running_version: String,
        /// 本次期望的核版本行。
        expected_version: String,
    },
    /// 路径不同，且至少一侧版本读不出来 ⇒ **无法确认跑的是期望的核**。
    ///
    /// 判**告警**而非放行：既然实跑的是一个我们没直接挑的文件，"读不出它是什么" 与 "读出来不对"
    /// 对用户是同一件事——都不能宣称跑的是配套内核。
    VersionUnreadable {
        /// 实跑二进制路径。
        running: PathBuf,
        /// 实跑二进制自报版本行（可能为空）。
        running_version: String,
        /// 本次期望的核版本行（可能为空）。
        expected_version: String,
    },
    /// 拿不到实跑 exe（内核记账读不到 / 平台不支持）⇒ 无从判定。
    ///
    /// **不报通过、也不报错**：没有观测到不等于观测到没有问题。只落 warn 日志。
    Unobservable,
}

impl CoreBinaryAttestation {
    /// 是否构成「必须让用户看见」的降级（→ `set_nonfatal_error`）。
    #[must_use]
    pub const fn is_alarm(&self) -> bool {
        matches!(
            self,
            Self::VersionMismatch { .. } | Self::VersionUnreadable { .. }
        )
    }

    /// 用户可见文案（中文；路径与版本原样给出，便于用户/日志自查）。
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::SamePath => "内核自证通过：实跑二进制 == 本次解析的核".to_owned(),
            Self::SameVersion { running, version } => format!(
                "内核自证通过：实跑 {} 与本次期望同版本（{version}）",
                running.display()
            ),
            Self::VersionMismatch {
                running,
                running_version,
                expected_version,
            } => format!(
                "内核版本不一致：实际运行的是 {}（{running_version}），\
                 而本应用配套的是 {expected_version}。提权助手里的内核与本应用不配套——\
                 请重装提权助手。",
                running.display()
            ),
            Self::VersionUnreadable {
                running,
                running_version,
                expected_version,
            } => format!(
                "内核版本无法确认：实际运行的是 {}（版本读数「{}」），本次期望「{}」。\
                 无法确认运行的是本应用配套的内核。",
                running.display(),
                if running_version.is_empty() {
                    "读不到"
                } else {
                    running_version
                },
                if expected_version.is_empty() {
                    "读不到"
                } else {
                    expected_version
                },
            ),
            // 措辞刻意不含「通过」二字：本条是「没观测到」，而 `attest_unobservable_is_neither_pass_nor_alarm`
            // 正以子串断言钉死它 —— 日志里出现「…通过」会让人在事后排查时把未判定读成已判定。
            Self::Unobservable => {
                "内核自证未能进行：读不到该进程的可执行文件路径（无法判定，不作结论）".to_owned()
            }
        }
    }
}

/// 自证判定（**纯函数**；观测腿在 `proxy.rs`，此处只吃已观测到的事实）。
///
/// - `expected`：本次 `core_binary_for_start()` 解析出的核路径（app 的意图）。
/// - `running`：**内核记账的**该 pid 实跑 exe（`None` = 观测失败）。
/// - `expected_version` / `running_version`：对两个**文件**各跑一次 `version` 读到的原始首行；
///   读失败恒空串（**绝不回落随包基线**——那会把「读不到」伪装成「就是基线」，
///   与 `updater::read_core_version_line` 同一纪律）。
#[must_use]
pub fn attest_core_binary(
    expected: &Path,
    running: Option<&Path>,
    expected_version: &str,
    running_version: &str,
) -> CoreBinaryAttestation {
    let Some(running) = running else {
        return CoreBinaryAttestation::Unobservable;
    };
    if same_file_path(expected, running) {
        return CoreBinaryAttestation::SamePath;
    }
    let (rv, ev) = (running_version.trim(), expected_version.trim());
    if rv.is_empty() || ev.is_empty() {
        return CoreBinaryAttestation::VersionUnreadable {
            running: running.to_path_buf(),
            running_version: rv.to_owned(),
            expected_version: ev.to_owned(),
        };
    }
    if rv == ev {
        return CoreBinaryAttestation::SameVersion {
            running: running.to_path_buf(),
            version: rv.to_owned(),
        };
    }
    CoreBinaryAttestation::VersionMismatch {
        running: running.to_path_buf(),
        running_version: rv.to_owned(),
        expected_version: ev.to_owned(),
    }
}

/// Two observations refer to the same file. Linux proc-backed observations compare
/// device/inode identity; their canonical pathname can name a replacement or deleted file.
/// Other platforms retain lexical/canonical path comparison.
///
/// 先比字面（覆盖单测与绝大多数生产情形），再比 canonicalize 后的形（覆盖 symlink /
/// `/var`→`/private/var` 这类 macOS 特有的等价改写）。canonicalize 失败即退回字面结论，
/// **不因 I/O 失败就误判成"不同"**（那会平白造出一条假告警）。
fn same_file_path(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "linux")]
    if b.starts_with("/proc") {
        use std::os::unix::fs::MetadataExt;
        return match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(expected), Ok(loaded)) => {
                expected.dev() == loaded.dev() && expected.ino() == loaded.ino()
            }
            _ => false,
        };
    }
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests;

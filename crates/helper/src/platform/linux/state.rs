//! Handler 进程状态 + Core spawn 抽象（对应 上游 `helper-linux/helper.go` 的全局 `child`/`childDone` + AmbientCaps 拉核）。
//!
//! ## 设计
//!
//! Go 源用包级全局 `child *exec.Cmd` + `childDone chan struct{}` + `mu sync.Mutex` 持有当前 sing-box 子进程。
//! 本实现把它们实例化为 [`HandlerState`]（可在测试中独立构造，不依赖全局可变状态）。
//!
//! Core spawn（start 命令）经 [`CoreSpawner`] trait 抽象：
//! - 生产实现（`AmbientCapsSpawner`，§helper-rust-evaluation B3 真机项）：fork → setuid 回对端登录用户 →
//!   raise ambient CAP_NET_ADMIN/RAW/BIND_SERVICE → execve coreDir/sing-box。这是 Linux 安全模型的核心地雷。
//! - 测试 mock：返回固定 pid，记录 spawn/terminate/kill 调用。
//!
//! 真实 AmbientCaps fork 链由 `server::AmbientCapsSpawner` 实现；降权与能力传递仍须真机复验。

use polaris_helper_proto::{HelperBirthTarget, HelperBirthToken};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

/// Wire-visible identity is present only for a new exact-birth Start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BirthMode {
    Legacy,
    Exact(HelperBirthToken),
}

/// 已 spawn 的 sing-box 子进程句柄（对应 Go `child *exec.Cmd`）。
#[derive(Debug, Clone)]
pub struct CoreHandle {
    /// 子进程 pid（Go `child.Process.Pid`）。
    pub pid: u32,
    birth: Arc<()>,
    mode: BirthMode,
}

impl CoreHandle {
    /// One opaque physical spawn identity. A reused numeric PID never becomes
    /// the same child merely because it occupies the old number.
    #[must_use]
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            birth: Arc::new(()),
            mode: BirthMode::Legacy,
        }
    }

    #[must_use]
    pub(crate) fn exact(pid: NonZeroU32, token: HelperBirthToken) -> Self {
        Self {
            pid: pid.get(),
            birth: Arc::new(()),
            mode: BirthMode::Exact(token),
        }
    }

    #[must_use]
    pub(crate) fn target(&self) -> Option<HelperBirthTarget> {
        match self.mode {
            BirthMode::Legacy => None,
            BirthMode::Exact(birth) => Some(HelperBirthTarget {
                pid: NonZeroU32::new(self.pid)?,
                birth,
            }),
        }
    }

    #[must_use]
    pub fn same_birth(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.birth, &other.birth)
    }
}

/// Linux helper 已创建的核心及其可归因关键路径耗时。
#[derive(Debug, Clone)]
pub struct SpawnedCore {
    /// 交给生命周期状态持有的 child 身份。
    pub handle: CoreHandle,
    /// `Command::spawn`（含 pre-exec 降权与 ambient capabilities）耗时。
    pub process_ms: u64,
    /// stdout/stderr 与日志属主修正移交后台线程的耗时。
    pub log_handoff_ms: u64,
}

/// start 命令的 spawn 请求（对照 Go `exec.Command(coreBin(), "run", "-c", cfg)` + Credential + AmbientCaps，:431-442）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCoreRequest {
    /// Exact start supplies a token minted before any forwarding or spawn.
    pub birth: Option<HelperBirthToken>,
    /// sing-box 二进制路径（已校验 == coreDir/sing-box）。
    pub binary: PathBuf,
    /// 配置文件路径（已校验属主 == 对端 uid）。
    pub config: PathBuf,
    /// 日志文件路径（None = 不重定向）。
    pub log: Option<PathBuf>,
    /// allowLan 转发开关。
    pub fwd: bool,
    /// 父 app PID（父死看护；None = 不启看护）。
    pub parent_pid: Option<u32>,
    /// 降权目标 uid（对端登录用户）。
    pub uid: u32,
    /// 降权目标 gid（对端登录组）。
    pub gid: u32,
    /// 补充组 gid 列表（对端登录用户所属全部组，`setgroups` 用；对照 Go `Credential.Groups`，:435-439）。
    ///
    /// 在 fork 前于父进程经 [`supplementary_groups`](crate::platform::linux::auth::supplementary_groups)
    /// 解析（不在拉核子进程碰 NSS）。空 = `setgroups(&[])` 清空补充组（Go `Groups: nil` 等价，见该函数文档）。
    pub groups: Vec<u32>,
}

/// spawn 错误（对应 Go `c.Start()` 失败，:452-458）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpawnError {
    /// sing-box 启动失败（fork/execve/权限）。
    #[error("start {detail}")]
    Spawn { detail: String },
}

/// Native reap state that can block a new Linux helper Start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapBlockerState {
    /// The exact owned Child has not exited yet.
    Pending,
    /// `Child::try_wait` failed, so neither exit nor liveness is known.
    Unknown,
}

/// Start admission is decided from every native Child slot before `already`,
/// forwarding changes, or spawn. `Already` is only valid when the sole
/// unreaped slot is the exact Running birth held by [`HandlerState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartAdmission {
    Admitted,
    Already { pid: u32 },
    Blocked { pid: u32, state: ReapBlockerState },
}

/// Exact Start scans all physical slots. A Legacy/hidden or unobservable slot
/// cannot produce an exact identity, and therefore cannot be `Already`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BirthAdmission {
    Admitted,
    Already(HelperBirthTarget),
    Blocked {
        target: Option<HelperBirthTarget>,
        state: ReapBlockerState,
    },
}

/// One bounded Stop poll. Only `Reaped` authorizes a successful Stop ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReap {
    Reaped,
    Pending,
    Unknown,
}

/// Core spawn 抽象（trait 便于测试 mock；生产用 AmbientCaps fork+setuid+execve）。
///
/// 对照 Go 源 start 分支的 `c.Start()`（:452）+ stop 的 `terminateChild`（:246-256）+ cleanup 的 `Kill`（:383）。
pub trait CoreSpawner: Send + Sync {
    /// Validate the shared deployment only for a genuinely new Start, after
    /// parameter checks and before forwarding/spawn effects. Stop never calls
    /// this gate. Test spawners have no shared filesystem dependency.
    fn validate_start_environment(&self) -> Result<(), SpawnError> {
        Ok(())
    }
    /// Inspect every physical birth before a Start can reuse or spawn.
    fn start_admission(&self, current_running: Option<&CoreHandle>) -> StartAdmission {
        current_running.map_or(StartAdmission::Admitted, |handle| StartAdmission::Already {
            pid: handle.pid,
        })
    }
    fn birth_admission(&self, current_running: Option<&CoreHandle>) -> BirthAdmission {
        match current_running {
            Some(handle) => handle.target().map_or(
                BirthAdmission::Blocked {
                    target: None,
                    state: ReapBlockerState::Unknown,
                },
                BirthAdmission::Already,
            ),
            None => BirthAdmission::Admitted,
        }
    }
    /// Exact Stop checks the physical slot first, then a bounded native-reap
    /// tombstone. A missing target is Unknown, never no-owner success.
    fn stop_birth(&self, _target: &HelperBirthTarget) -> StopReap {
        StopReap::Unknown
    }
    /// A late Stop may acknowledge an earlier birth after a successor starts,
    /// but only when the old native Child has actually been reaped.
    fn reaped_birth(&self, _target: &HelperBirthTarget) -> bool {
        false
    }
    /// Legacy mutating commands cannot act while an exact birth is retained.
    fn has_exact_birth(&self) -> bool {
        false
    }
    /// spawn sing-box 子进程（AmbientCaps 拉核）。
    fn spawn(&self, req: &SpawnCoreRequest) -> Result<SpawnedCore, SpawnError>;
    /// Start or poll graceful termination. This call itself is bounded; the
    /// background worker owns TERM → ≤5s → KILL and native reap.
    fn terminate(&self, h: &CoreHandle) -> StopReap;
    /// 强杀 SIGKILL（Go `child.Process.Kill()`，:383）。
    fn kill(&self, h: &CoreHandle);
}

/// Handler-visible custody for the exact physical birth. A Stop never removes
/// `Stopping`; only a successful native reap may clear it.
#[derive(Debug, Clone)]
pub enum ManagedChild {
    Running(CoreHandle),
    Stopping(CoreHandle),
}

impl ManagedChild {
    #[must_use]
    pub fn handle(&self) -> &CoreHandle {
        match self {
            Self::Running(handle) | Self::Stopping(handle) => handle,
        }
    }

    #[must_use]
    pub fn running(&self) -> Option<&CoreHandle> {
        match self {
            Self::Running(handle) => Some(handle),
            Self::Stopping(_) => None,
        }
    }
}

/// Handler 进程状态（对应 Go 全局 `child`/`childDone`，实例化可测）。
#[derive(Debug)]
pub struct HandlerState {
    /// Exact current birth. `Stopping` remains present through Pending/Unknown.
    pub child: Option<ManagedChild>,
}

impl HandlerState {
    /// 构造空状态（无 child）。
    #[must_use]
    pub fn new() -> Self {
        Self { child: None }
    }
}

impl Default for HandlerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

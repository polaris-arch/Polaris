//! 命令执行缝（本 crate 与宿主 OS 的**唯一**交互点）。
//!
//! ## 为什么是「一条缝」
//!
//! 本 crate 的平台差异只是**构造命令字符串**（`reg.exe` / `networksetup` / `gsettings`），不是平台专属
//! 类型或依赖 —— 故用「运行时 `Platform` 枚举 + 注入 trait」而非 `#[cfg(target_os)]`（审计 §M1 第二形态）。
//! 收益是 **Linux CI 100% 编译 + 跑测三平台逻辑**：命令构造与输出解析全是纯函数，mock 一注入就能断言
//! 三平台的 argv/解析，不必真跑 OS 命令。
//!
//! **接线纪律（勿破坏）**：新增平台行为时，「决定跑什么命令 / 怎么解释输出」必须留在纯函数里，
//! 本模块只负责「把已决定的命令跑掉」。一旦把判定逻辑塞进 [`StdCommandRunner`]，那部分逻辑就退回
//! 「只有真机能测」—— 正是 §G5「cfg 门内代码 Linux 看不到 → 潜伏」的同型陷阱。
//!
//! ## 语义对齐
//!
//! [`CommandRunner::run`] 对齐上游 `execFileAsync` / `execAsync`：**非零退出 / 超时 / spawn 失败 → `Err`**
//! （上游是 reject，调用方 try/catch 降级）。argv 参数化下发、**不经 shell 插值** —— 上游
//! `SystemProxyManager.ts:506,753` 明写把 shell 字符串拼接改成 `execFileAsync` 正是为了杜绝命令注入
//! （bypass 域名 / gsettings host 均可被投毒）。

#![forbid(unsafe_code)]

use std::io::{self, Read};
use std::process::{Child, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 一条待执行命令（程序 + argv）。argv 参数化下发，不经 shell 插值（杜绝注入 —— 上游 execFileAsync 口径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Command {
    /// 便捷构造。
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// 一次命令执行的产出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

/// 命令执行器 —— 注入便于 mock；生产用 [`StdCommandRunner`]。
///
/// 契约：非零退出 / 超时 / spawn 失败 → `Err(原因)`（对齐上游 `execFileAsync` reject）。
pub trait CommandRunner {
    fn run(&self, cmd: &Command, timeout: Duration) -> Result<CommandOutput, String>;
}

/// `try_wait` 自适应轮询窗口。std 无 `wait_timeout`，而本 crate **禁止引入新依赖**（`wait-timeout` /
/// `tokio` 都不引）→ 用轮询实现硬超时。
///
/// `networksetup` / `reg.exe` / `gsettings` 常在数毫秒内完成；固定 10ms 会让启动关键路径上的每条命令
/// 都白等到下一格。1ms 起步、指数退避到原来的 10ms 上限，既缩短短命令尾延迟，又不增加挂起命令的
/// 长期轮询频率，超时与 kill 语义不变。
const INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(1);
const MAX_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn next_poll_interval(current: Duration, maximum: Duration) -> Duration {
    current.saturating_mul(2).min(maximum)
}

/// Windows `CREATE_NO_WINDOW`（winbase.h `0x0800_0000`）。
///
/// 宿主进程是 GUI 子系统（`src-tauri/src/main.rs` 的 `windows_subsystem = "windows"`）⇒ 自身**无控制台**。
/// 无控制台的父进程创建 console 子系统程序时，`CreateProcess` 会为子进程**新分配一个控制台窗口**
/// —— 用户看到黑框闪一下。而 `reg.exe` / `netsh` / `tasklist` 这些正是 console 程序。
///
/// **std 与 tokio 都不默认抑制**：`tokio::process::Command::creation_flags` 只是把值透传给
/// `std::os::windows::process::CommandExt::creation_flags`，tokio 侧无任何隐含默认
/// （实测 tokio-1.53.1 `src/process/mod.rs:675`）。故每个 Windows 可达的调用点都得显式加。
///
/// 不为一个常量引 `windows-sys`：本 crate 的硬纪律是零新依赖（见 [`INITIAL_POLL_INTERVAL`]）。
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 生产实现：`std::process::Command` + argv 参数化 + 硬超时（spawn → 轮询 `try_wait` → 超时 kill）。
///
/// **同步**：本 crate 的控制器（`SystemProxyController` / `SystemDnsController`）全是同步 API，
/// 故此处同步执行。调用方若在 async 语境，须自行 `spawn_blocking`（本 crate 不引 tokio）。
///
/// **管道排空**：stdout/stderr 各起一个读取线程。若改成「先轮询后读管道」，子进程输出超过管道缓冲
/// （典型 64KB）就会写阻塞 → 与轮询互等 → 死锁。`scutil --dns` 等命令输出虽小，但不留这个雷。
#[derive(Debug, Clone, Copy, Default)]
pub struct StdCommandRunner;

/// 仅这次原命令构造的资源观察；不是宿主全局 NoOwner。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCleanup {
    NotSpawned,
    NativeAndReadersReturned,
    Unknown,
}

/// 两条原排空线程分别记录实际 join 的结果，不用空字符串代替失败事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandReaderDisposition {
    Pending,
    Returned(String),
    ReadFailed(String),
    Panicked,
    NotStarted(String),
}

struct CommandReader {
    handle: Option<JoinHandle<io::Result<String>>>,
    disposition: CommandReaderDisposition,
}

impl CommandReader {
    fn start(pipe: Option<impl Read + Send + 'static>) -> Self {
        match std::thread::Builder::new().spawn(move || drain(pipe)) {
            Ok(handle) => Self {
                handle: Some(handle),
                disposition: CommandReaderDisposition::Pending,
            },
            Err(error) => Self {
                handle: None,
                disposition: CommandReaderDisposition::NotStarted(error.to_string()),
            },
        }
    }

    fn join_returned(&mut self) {
        if self.handle.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join();
        }
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.disposition = match handle.join() {
                Ok(Ok(output)) => CommandReaderDisposition::Returned(output),
                Ok(Err(error)) => CommandReaderDisposition::ReadFailed(error.to_string()),
                Err(_) => CommandReaderDisposition::Panicked,
            };
        }
    }

    fn legacy_output(&self) -> String {
        match &self.disposition {
            CommandReaderDisposition::Returned(output) => output.clone(),
            // 原 run 的 best-effort 输出行为；此转换不返回 cleanup 资格。
            _ => String::new(),
        }
    }

    fn observed_output(&self) -> Result<String, String> {
        match &self.disposition {
            CommandReaderDisposition::Returned(output) => Ok(output.clone()),
            CommandReaderDisposition::ReadFailed(error) => Err(format!("读取输出失败: {error}")),
            CommandReaderDisposition::Panicked => Err("排空线程 panic".to_owned()),
            CommandReaderDisposition::NotStarted(error) => {
                Err(format!("排空线程启动失败: {error}"))
            }
            CommandReaderDisposition::Pending => Err("排空线程尚未返回".to_owned()),
        }
    }
}

/// 原 Child 和两条原 reader 的独占 custody。没有 Clone/serde 或可由 mock 构造的 public 字段。
/// 预算耗尽、wait/kill 错误和 reader 未返回时，调用者保留此对象，后继 poll 仍观察同一批资源。
#[must_use = "retain the original command custody until cleanup is observed"]
pub struct PendingCommand {
    child: Child,
    program: String,
    status: Option<ExitStatus>,
    stdout: CommandReader,
    stderr: CommandReader,
    operation_error: Option<String>,
    cleanup_error: Option<String>,
    termination_requested: bool,
}

impl PendingCommand {
    /// 仅记录这个原 Child 的实际 wait 返回；两个 reader 均须实际成功 join/read 返回才为闭合。
    #[must_use]
    pub fn cleanup(&self) -> CommandCleanup {
        if self.status.is_some()
            && matches!(
                self.stdout.disposition,
                CommandReaderDisposition::Returned(_)
            )
            && matches!(
                self.stderr.disposition,
                CommandReaderDisposition::Returned(_)
            )
        {
            CommandCleanup::NativeAndReadersReturned
        } else {
            CommandCleanup::Unknown
        }
    }

    #[must_use]
    pub fn native_status(&self) -> Option<ExitStatus> {
        self.status
    }

    #[must_use]
    pub fn stdout_disposition(&self) -> &CommandReaderDisposition {
        &self.stdout.disposition
    }

    #[must_use]
    pub fn stderr_disposition(&self) -> &CommandReaderDisposition {
        &self.stderr.disposition
    }

    #[must_use]
    pub fn cleanup_error(&self) -> Option<&str> {
        self.cleanup_error.as_deref()
    }

    /// 操作失败与资源闭合独立：timeout/nonzero 仍是 Err；未知输出尚不产生成功操作结果。
    pub fn operation(&self) -> Option<Result<CommandOutput, String>> {
        if let Some(error) = &self.operation_error {
            return Some(Err(error.clone()));
        }
        let status = self.status?;
        if matches!(self.stdout.disposition, CommandReaderDisposition::Pending)
            || matches!(self.stderr.disposition, CommandReaderDisposition::Pending)
        {
            return None;
        }
        Some((|| {
            let stdout = self.stdout.observed_output()?;
            let stderr = self.stderr.observed_output()?;
            command_result(&self.program, status, stdout, stderr)
        })())
    }

    /// 请求关闭同一个原 Child；不是 kill ACK/资源闭合证明。
    pub fn request_stop(&mut self) {
        self.termination_requested = true;
        self.operation_error
            .get_or_insert_with(|| format!("{} 停止请求", self.program));
    }

    /// 有界观察，不把仍在等待的 handle 移走，也不另建后台 reaper。
    /// read/panic 错误为 sticky Unknown；wait/kill 不确定时仍保留原 Child 供后继重试。
    pub fn poll_cleanup(&mut self, budget: Duration) -> CommandCleanup {
        if self.termination_requested && self.status.is_none() {
            if let Err(error) = self.child.kill() {
                self.cleanup_error = Some(format!("{} kill 失败: {error}", self.program));
            }
        }
        let deadline = Instant::now() + budget;
        let mut poll_interval = INITIAL_POLL_INTERVAL;
        loop {
            if self.status.is_none() {
                match self.child.try_wait() {
                    Ok(Some(status)) => self.status = Some(status),
                    Ok(None) => {}
                    Err(error) => {
                        self.cleanup_error = Some(format!("{} 等待失败: {error}", self.program));
                    }
                }
            }
            // 两条都执行；stdout 失败不得跳过 stderr 的真实 join。
            self.stdout.join_returned();
            self.stderr.join_returned();
            let cleanup = self.cleanup();
            if cleanup != CommandCleanup::Unknown
                || (self.status.is_some()
                    && self.stdout.handle.is_none()
                    && self.stderr.handle.is_none())
                || Instant::now() >= deadline
            {
                return cleanup;
            }
            std::thread::sleep(
                poll_interval.min(deadline.saturating_duration_since(Instant::now())),
            );
            poll_interval = next_poll_interval(poll_interval, MAX_POLL_INTERVAL);
        }
    }
}

/// 只能由生产 StdCommandRunner 的原 spawn engine 生成；普通注入 CommandRunner 没有 proof 入口。
#[must_use = "observed command owns its original child and readers"]
pub struct ObservedCommand {
    spawn_error: Option<String>,
    pending: Option<PendingCommand>,
}

impl ObservedCommand {
    #[must_use]
    pub fn cleanup(&self) -> CommandCleanup {
        self.pending
            .as_ref()
            .map_or(CommandCleanup::NotSpawned, PendingCommand::cleanup)
    }

    pub fn operation(&self) -> Option<Result<CommandOutput, String>> {
        match &self.pending {
            Some(pending) => pending.operation(),
            None => self.spawn_error.as_ref().map(|error| Err(error.clone())),
        }
    }

    pub fn pending_mut(&mut self) -> Option<&mut PendingCommand> {
        self.pending.as_mut()
    }

    /// 原始资源随对象交还调用者；不跨线程/全局登记，也没有 Drop 自签 completion。
    pub fn into_pending(self) -> Option<PendingCommand> {
        self.pending
    }

    fn into_legacy_result(self) -> Result<CommandOutput, String> {
        let Some(mut pending) = self.pending else {
            return Err(self
                .spawn_error
                .unwrap_or_else(|| "missing command result".to_owned()));
        };
        if let Some(error) = pending.operation_error {
            // Root 批准的兼容边界：保旧 timeout/error 返回时机，历史未知尾部没有 completion 授权。
            // 保留旧 timeout 的原 Child wait 尝试；它不代表两条 reader 已返回。
            if pending.termination_requested && pending.status.is_none() {
                match pending.child.wait() {
                    Ok(status) => pending.status = Some(status),
                    Err(wait_error) => {
                        pending.cleanup_error =
                            Some(format!("{} 等待失败: {wait_error}", pending.program));
                    }
                }
            }
            return Err(error);
        }
        let Some(status) = pending.status else {
            return Err(format!("{} 等待结果未知", pending.program));
        };
        pending.stdout.join();
        pending.stderr.join();
        command_result(
            &pending.program,
            status,
            pending.stdout.legacy_output(),
            pending.stderr.legacy_output(),
        )
    }
}

impl StdCommandRunner {
    /// 同一个原 spawn；操作预算结束后返回原资源 custody，cleanup 需调用者显式观察。
    pub fn run_observed(&self, cmd: &Command, timeout: Duration) -> ObservedCommand {
        let mut builder = std::process::Command::new(&cmd.program);
        builder
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            builder.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = match builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                return ObservedCommand {
                    spawn_error: Some(format!("{} 启动失败: {error}", cmd.program)),
                    pending: None,
                }
            }
        };
        let stdout = CommandReader::start(child.stdout.take());
        let stderr = CommandReader::start(child.stderr.take());
        let reader_start_failed =
            matches!(stdout.disposition, CommandReaderDisposition::NotStarted(_))
                || matches!(stderr.disposition, CommandReaderDisposition::NotStarted(_));
        let mut pending = PendingCommand {
            child,
            program: cmd.program.clone(),
            status: None,
            stdout,
            stderr,
            operation_error: reader_start_failed
                .then(|| format!("{} 排空线程启动失败", cmd.program)),
            cleanup_error: None,
            termination_requested: reader_start_failed,
        };
        let deadline = Instant::now() + timeout;
        let mut poll_interval = INITIAL_POLL_INTERVAL;
        while pending.operation_error.is_none() {
            match pending.child.try_wait() {
                Ok(Some(status)) => {
                    pending.status = Some(status);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    pending.operation_error = Some(format!("{} 等待失败: {error}", cmd.program));
                    pending.termination_requested = true;
                    break;
                }
            }
            if Instant::now() >= deadline {
                pending.operation_error = Some(format!("{} 超时（{:?}）", cmd.program, timeout));
                pending.termination_requested = true;
                break;
            }
            std::thread::sleep(poll_interval);
            poll_interval = next_poll_interval(poll_interval, MAX_POLL_INTERVAL);
        }
        pending.poll_cleanup(Duration::ZERO);
        ObservedCommand {
            spawn_error: None,
            pending: Some(pending),
        }
    }
}

impl CommandRunner for StdCommandRunner {
    fn run(&self, cmd: &Command, timeout: Duration) -> Result<CommandOutput, String> {
        // 操作结果兼容层，不提供 NoOwner；后续 S1 必须直接消费 run_observed 并保原 pending。
        self.run_observed(cmd, timeout).into_legacy_result()
    }
}

fn command_result(
    program: &str,
    status: ExitStatus,
    stdout: String,
    stderr: String,
) -> Result<CommandOutput, String> {
    if !status.success() {
        return Err(format!(
            "{} 退出码 {}: {}",
            program,
            status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            stderr.trim()
        ));
    }
    Ok(CommandOutput { stdout, stderr })
}

// ── Windows System32 绝对路径（移植自 上游 `utils/win-system32.ts`）──

/// `%SystemRoot%`（`windir` 兜底，最终回落 `C:\Windows`）。env 注入便于单测。
/// 上游 `getSystemRoot`。
pub fn system_root(env_system_root: Option<&str>, env_windir: Option<&str>) -> String {
    env_system_root
        .filter(|s| !s.is_empty())
        .or(env_windir.filter(|s| !s.is_empty()))
        .unwrap_or("C:\\Windows")
        .to_string()
}

/// 解析 System32 下的二进制为绝对路径（`reg.exe` → `C:\Windows\System32\reg.exe`）。
///
/// **为什么不用裸命令名**：部分设备的进程 PATH 缺失 `C:\Windows\System32`（注册表 Path 值类型从
/// `REG_EXPAND_SZ` 退化为 `REG_SZ` 致 `%SystemRoot%` 不展开，或 PATH 超长被截断）→ 裸 `reg`/`netsh`/
/// `ipconfig` 解析失败报「'reg' 不是内部或外部命令」。**这不是权限问题**，绝对路径可完全绕开。
/// 上游 `system32`。
///
/// 恒生成反斜杠路径，与宿主 OS 无关 —— Linux 上单测亦得 Windows 路径（保持零 cfg 可测性）。
pub fn system32(binary: &str, env_system_root: Option<&str>, env_windir: Option<&str>) -> String {
    let root = system_root(env_system_root, env_windir);
    let root = root.trim_end_matches(['\\', '/']);
    format!("{root}\\System32\\{binary}")
}

/// 从进程环境解析 System32 二进制（生产入口；非 Windows 上取不到 env → 回落默认，无害）。
pub fn system32_from_env(binary: &str) -> String {
    let sr = std::env::var("SystemRoot").ok();
    let wd = std::env::var("windir").ok();
    system32(binary, sr.as_deref(), wd.as_deref())
}

/// 原管道真正读到 EOF 才返回输出；读失败保留错误，不替换成已完成事实。
fn drain(pipe: Option<impl Read>) -> io::Result<String> {
    let Some(mut pipe) = pipe else {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "missing captured output pipe",
        ));
    };
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 测试辅助：跨模块共享的命令执行 mock。
///
/// **这是本 crate「Linux 上跑测三平台」的枢纽** —— 任意 `SystemProxyOpsImpl::with_platform(mock, Platform::Mac)`
/// 都能在 Linux CI 上断言 mac 的 argv 与输出解析，全程不碰宿主网络。
#[cfg(test)]
pub mod exec_tests_helpers {
    use super::{Command, CommandOutput, CommandRunner};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    /// 记录调用的 mock（本 crate 全部三平台逻辑测试的注入点 —— 不触碰宿主）。
    #[derive(Default)]
    pub struct MockRunner {
        pub calls: RefCell<Vec<Command>>,
        /// 与 `calls` 同序的预算；用于证明可选清理没有偷用必要事务的宽超时。
        pub timeouts: RefCell<Vec<Duration>>,
        /// 按调用序返回的 stdout（队列；空则回空串）。
        pub stdouts: RefCell<Vec<String>>,
        /// 按「argv 含此子串」匹配返回 stdout（比队列更稳，不依赖调用序）。
        pub by_arg: RefCell<HashMap<String, String>>,
        /// 这些 program 的调用直接失败。
        pub fail_programs: Vec<String>,
        /// argv 含这些子串的调用直接失败。
        pub fail_args: Vec<String>,
    }

    impl MockRunner {
        /// 按 argv 子串挂 stdout。
        pub fn with_arg_stdout(self, arg_substr: &str, stdout: &str) -> Self {
            self.by_arg
                .borrow_mut()
                .insert(arg_substr.to_string(), stdout.to_string());
            self
        }

        /// 全部调用的 (program, argv) 快照。
        pub fn snapshot(&self) -> Vec<Command> {
            self.calls.borrow().clone()
        }

        /// 是否跑过某条 argv 含该子串的命令。
        pub fn ran_arg(&self, substr: &str) -> bool {
            self.calls
                .borrow()
                .iter()
                .any(|c| c.args.iter().any(|a| a.contains(substr)))
        }

        /// 首条 argv 含该子串的调用预算。
        pub fn timeout_for_arg(&self, substr: &str) -> Option<Duration> {
            self.calls
                .borrow()
                .iter()
                .zip(self.timeouts.borrow().iter().copied())
                .find_map(|(c, timeout)| {
                    c.args.iter().any(|a| a.contains(substr)).then_some(timeout)
                })
        }

        /// argv 含该子串的调用次数。
        pub fn count_arg(&self, substr: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c.args.iter().any(|a| a.contains(substr)))
                .count()
        }

        /// argv 含**恰好等于**该值的项的调用次数。
        ///
        /// 子串匹配在此处会骗人：`-setwebproxystate` 含 `-setwebproxy` → `count_arg` 把 state 命令
        /// 也算进去。断言「每服务设了几次代理」这类计数必须用精确匹配。
        pub fn count_arg_exact(&self, arg: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c.args.iter().any(|a| a == arg))
                .count()
        }
    }

    impl CommandRunner for MockRunner {
        fn run(&self, cmd: &Command, timeout: Duration) -> Result<CommandOutput, String> {
            self.calls.borrow_mut().push(cmd.clone());
            self.timeouts.borrow_mut().push(timeout);
            if self.fail_programs.iter().any(|p| p == &cmd.program) {
                return Err("mock failure (program)".into());
            }
            if self
                .fail_args
                .iter()
                .any(|f| cmd.args.iter().any(|a| a.contains(f)))
            {
                return Err("mock failure (arg)".into());
            }
            // 优先按 argv 子串匹配。
            for (k, v) in self.by_arg.borrow().iter() {
                if cmd.args.iter().any(|a| a.contains(k)) {
                    return Ok(CommandOutput {
                        stdout: v.clone(),
                        stderr: String::new(),
                    });
                }
            }
            let mut outs = self.stdouts.borrow_mut();
            let stdout = if outs.is_empty() {
                String::new()
            } else {
                outs.remove(0)
            };
            Ok(CommandOutput {
                stdout,
                stderr: String::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests;

//! OS DNS cache flushing: native Windows API, commands on macOS/Linux (best-effort).
//!
//! Execution is injected so orchestration tests never touch the host resolver.
//!
//! Cache failure is advisory. Commands have a 3s kill budget; native RPC has a 3s caller wait.

#![forbid(unsafe_code)]

use crate::exec::{Command, CommandRunner};
use polaris_helper_proto::Platform;
use std::time::Duration;

/// 单个外部命令硬超时（上游 `EXEC_TIMEOUT_MS`）。
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(3);

/// 一条刷缓存命令。
///
/// 单一真值在 [`crate::exec::Command`]（此前本模块的 `FlushCommand` 与 `proxy_ops::Command` 是两份
/// 逐字相同的 `{program, args}` —— 假差异，已合并）。别名保留移植血缘可读性。
pub type FlushCommand = Command;

/// 命令执行器（注入便于 mock；真实实现带超时）。
/// 失败返回 Err（调用方降级为告警，不抛）。
pub trait FlushExec {
    /// Queried at execution time, again after any blocking helper request.
    /// A cancelled Windows session end must permit subsequent ordinary flushes.
    fn windows_session_ending(&self) -> bool {
        false
    }

    /// Native Windows cache operation. An executor must explicitly provide it;
    /// an arbitrary CommandRunner must never silently fall back to a console child.
    fn windows_flush(&self, _timeout: Duration) -> Result<(), String> {
        Err("native Windows DNS cache executor unavailable".into())
    }

    fn exec(&self, cmd: &FlushCommand, timeout: Duration) -> Result<(), String>;
}

pub(crate) struct ProductionFlushExec;

impl FlushExec for ProductionFlushExec {
    fn windows_session_ending(&self) -> bool {
        #[cfg(windows)]
        {
            crate::windows_session::is_ending()
        }
        #[cfg(not(windows))]
        false
    }

    fn windows_flush(&self, timeout: Duration) -> Result<(), String> {
        #[cfg(windows)]
        {
            crate::windows_dns::begin_flush()?.wait(timeout)
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            Err("native Windows DNS cache executor unavailable on this platform".into())
        }
    }

    fn exec(&self, cmd: &FlushCommand, timeout: Duration) -> Result<(), String> {
        // A worker can have been queued before the OS started ending the session.
        // Recheck on the local execution leg, not only when the task was queued.
        if self.windows_session_ending() {
            return Err("Windows session ending; DNS flush not launched".into());
        }
        crate::exec::StdCommandRunner.run(cmd, timeout).map(|_| ())
    }
}

/// [`FlushExec`] 的生产实现：委托 [`CommandRunner`]（硬超时在其中落实）。
///
/// **不是多余的一层**：`FlushExec` 是 flush 的**语义**缝（契约=「失败返 Err，调用方降级为告警」），
/// `CommandRunner` 是**执行**缝。此 impl 让任意 runner（含生产 `StdCommandRunner`）直接当 flush 执行器用，
/// 同时保留 `FlushExec` 的独立 mock 面。
impl<R: CommandRunner> FlushExec for R {
    fn exec(&self, cmd: &FlushCommand, timeout: Duration) -> Result<(), String> {
        self.run(cmd, timeout).map(|_| ())
    }
}

/// macOS 用户级降级命令：`dscacheutil -flushcache`。
/// 上游 `flushOsDnsCache` darwin 降级腿。
pub fn mac_user_flush_command() -> FlushCommand {
    Command::new("/usr/bin/dscacheutil", ["-flushcache"])
}

/// Linux 命令：`resolvectl flush-caches`。
///
/// **无回退，与上游一致**（`os-dns-flush.ts:82`）。曾有一份 `helper/platform/linux/ops.rs::TokioDns`
/// 带 `resolvconf -u` 回退与本函数分叉；2026-07-16 调和时判定其**无上游、不可达、语义非刷缓存**并删除
/// （判据见该文件「系统 DNS 刷新」段）。**已知缺口**：非 systemd 且跑 nscd/dnsmasq 的机器不刷 —— 上游同样如此。
pub fn linux_flush_command() -> FlushCommand {
    Command::new("resolvectl", ["flush-caches"])
}

/// helper flush 结果（macOS root helper 通道）。上游 `helperFlushDns` 返回。
#[derive(Debug, Clone, Default)]
pub struct HelperFlushResult {
    pub ok: bool,
    pub partial: Option<String>,
    pub error: Option<String>,
}

/// helper flush 通道（mac root / win SYSTEM helper；缺省 None = 不可用走用户级降级）。
pub type HelperFlushFn<'a> = Option<&'a dyn Fn() -> HelperFlushResult>;

/// 特权 helper 腿（mac/win 共用）：成功 → `true`（调用方不再跑用户级命令）。
///
/// `ok` 且 `partial` → 只 warn **不降级**：该步用户级同样做不到，再跑一次只是噪音。
/// 不可用（未装 / 旧 helper 回 `ERR unknown` / 通信失败）→ warn 一条含原因的降级说明再回 `false`。
/// `fallback` 是降级目标的人话名字，进 warn 文案。
fn helper_flush_succeeded(
    helper_flush: HelperFlushFn,
    fallback: &str,
    on_warn: &mut dyn FnMut(&str),
) -> bool {
    let Some(helper) = helper_flush else {
        return false;
    };
    let r = helper();
    if r.ok {
        if let Some(partial) = r.partial {
            on_warn(&format!(
                "已刷新系统 DNS 缓存（helper 特权，partial：{partial}）"
            ));
        }
        return true;
    }
    on_warn(&format!(
        "helper flush-dns 不可用（{}），降级{fallback}",
        r.error.unwrap_or_else(|| "未知".into())
    ));
    false
}

/// 刷 OS DNS 缓存。best-effort、永不抛（失败仅 on_warn）。
///
/// - mac：helper 可用且 ok → 用 helper；否则降级 `dscacheutil -flushcache`。
/// - win：helper ready 且 ok → 用 SYSTEM 原生 DNS API；否则只试本地原生 API。
///   不可用/无权限仅告警，绝不回退创建命令行进程。
/// - linux：`resolvectl flush-caches`。
/// - android：no-op（见下方臂上注释）。
/// - 其它：no-op。
///
/// # `helper_ready`：Windows 腿的前置判据（spec §3.1 的「if helper ready」）
///
/// **只门住「装没装」这一格，不门住失败**：`false` ⇒ 这台机器结构上就没有特权通道（没装 helper /
/// 装过又卸了），此时每次起停核都发一条「helper flush-dns 不可用」是纯噪音 —— 结论恒定、用户也
/// 无从行动。`true` ⇒ 照常试、失败照常 warn 并降级，**真失败一格都不吞**（那条 warn 是有信息量的：
/// 它说的是「你装了 helper，但它这次没干成」）。
///
/// mac 腿蓄意不走这个门：那边的降级目标 `dscacheutil` 在用户级**真的能刷**，多试一次的成本与
/// Windows 腿（本地原生 API 仍可能被权限拒绝）不是一回事。
///
/// 上游 `flushOsDnsCache`。
pub fn flush_os_dns_cache<E: FlushExec>(
    platform: Platform,
    exec: &E,
    helper_flush: HelperFlushFn,
    helper_ready: bool,
    on_warn: &mut dyn FnMut(&str),
) -> bool {
    match platform {
        Platform::Mac => {
            if helper_flush_succeeded(helper_flush, "用户级 dscacheutil", on_warn) {
                return true;
            }
            // 用户级降级。
            exec.exec(&mac_user_flush_command(), EXEC_TIMEOUT)
                .map(|()| true)
                .unwrap_or_else(|e| {
                    on_warn(&format!("刷新系统 DNS 缓存失败（忽略）: {e}"));
                    false
                })
        }
        // The separate native helper command cannot execute an old helper's ipconfig leg.
        // Both fallback and helper paths use DNS API calls, including queued/in-flight work.
        Platform::Win => {
            if exec.windows_session_ending() {
                on_warn("Windows session ending; skipping DNS flush");
                return false;
            }
            if helper_ready && helper_flush_succeeded(helper_flush, "本地原生 DNS API", on_warn)
            {
                return true;
            }
            // Recheck after blocking IPC; optional cache work must not extend session end.
            if exec.windows_session_ending() {
                on_warn("Windows session ending; skipping local DNS flush fallback");
                return false;
            }
            exec.windows_flush(EXEC_TIMEOUT)
                .map(|()| true)
                .unwrap_or_else(|e| {
                    on_warn(&format!("刷新系统 DNS 缓存失败（忽略）: {e}"));
                    false
                })
        }
        Platform::Linux => exec
            .exec(&linux_flush_command(), EXEC_TIMEOUT)
            .map(|()| true)
            .unwrap_or_else(|e| {
                on_warn(&format!("刷新系统 DNS 缓存失败（忽略）: {e}"));
                false
            }),
        // Android：**没有应用可刷的系统 DNS 缓存**。解析全部发生在核自己的 tun fd 内
        // （FakeIP / hijack-dns 由 sing-box 持有，系统 resolver 不在链路上），起停核跨越的那条
        // 「受控/还原」边界在这个平台上根本不存在 ⇒ 无缓存残留可清。返 `true` 表示「本平台上
        // 这件事已经完成」，不是「假装刷过了」：唯一的消费者只用它给 Linux 报错（见
        // `runtime/proxy/dns_takeover.rs::flush_os_dns_cache_best_effort`）。
        Platform::Android => true,
        // iOS：同样返 `true`，但**独立成臂**——与 Android 同答不同因，合并臂会把两条不同的
        // 事实链断言成一条。
        //
        // iOS 的事实：应用（含 NE 扩展）没有任何刷系统 DNS 缓存的 API —— macOS 上的
        // `dscacheutil -flushcache` / `killall -HUP mDNSResponder` 在 iOS 上都没有对应面
        // （无可执行 shell、无进程信号面）。且与 Android 同构的那一半也成立：解析发生在核自己的
        // tun fd 内（FakeIP / hijack-dns 由 sing-box 持有），系统 resolver 不在链路上。
        //
        // 与 `Other` 的差别正是这条：`Other` 返 `false` 因为「不知道有没有缓存、也不知道怎么刷」；
        // iOS 返 `true` 因为**知道没有可刷的东西**。判据是「本平台上这件事已经完成」，不是
        // 「假装刷过了」。
        //
        // **未验证**：上述两条都是平台 API 面的事实推论，本仓构不出 iOS 产物，未经真机取证。
        Platform::Ios => true,
        // 未知平台：返 `false` = **「本次没有刷」**（2026-09-05 由 `true` 改）。
        //
        // Android 那条能答 `true`，靠的是一条**已知事实**：这个平台上没有应用可刷的系统缓存，
        // 所以「刷完了」为真。`Other` 拿不到同一条事实 —— 它的定义就是「本仓没有为这个平台答过题」，
        // 既不知道它有没有系统解析器缓存，也不知道该用什么命令刷。在这种情形下返 `true` 是**把
        // 「没做」报成「做完了」**，与 `mesh_system_supported_on_platform` 那条乐观兜底同形。
        //
        // 今天没有行为差：唯一消费者（`runtime/proxy/dns_takeover.rs::flush_os_dns_cache_best_effort`）
        // 只在 `Platform::current() == Linux` 时把 `false` 转成告警。改的是这个返回值**将来被别人
        // 消费时说的是不是真话**。
        Platform::Other => false,
    }
}

#[cfg(test)]
mod tests;

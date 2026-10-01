//! 崩溃自愈 owner：后台崩溃监测腿（直起 Child 身份；helper 既有世代 + pid 身份判据）、退避重启执行体、GiveUp 终态播报，
//! 以及「观察之后才读世代」的分类 seam 与「不可恢复重启错误」谓词。

use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use polaris_core_supervisor::{
    classify_child_exit, AutoRestartOutcome, ChildObservation, CrashRecoveryMachine,
    ExitClassification, FailureOutcome, LifecycleGate, RestartFate,
};
use serde_json::Value;

use crate::runtime::helper::ManagedCoreStatus;

use super::code;
use super::lifecycle::monotonic_now_ms;
use super::lifecycle::RestartLeg;
use super::process_supervision::{
    pid_alive, pid_identity_verdict, process_identity, PidIdentity, RunIdentity,
};
use super::route_replan::RuntimeBindingState;
use super::startup::with_helper_gate_suppressed;
use super::{ProxyRuntime, ProxyStatus, StartError};

#[derive(Clone)]
pub(super) struct CrashEvent {
    pub(super) generation: u64,
    pub(super) direct_run_identity: Option<RunIdentity>,
    pub(super) config: Option<Value>,
}

struct PendingReplay {
    ticket: u64,
    event: CrashEvent,
}

#[derive(Default)]
pub(super) struct CrashRecoveryState {
    machine: CrashRecoveryMachine,
    next_ticket: u64,
    pending_replay: Option<PendingReplay>,
}

#[cfg(test)]
impl CrashRecoveryState {
    pub(super) fn pending_replay_event(&self) -> Option<&CrashEvent> {
        self.pending_replay.as_ref().map(|pending| &pending.event)
    }
}

impl Deref for CrashRecoveryState {
    type Target = CrashRecoveryMachine;

    fn deref(&self) -> &Self::Target {
        &self.machine
    }
}

impl DerefMut for CrashRecoveryState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.machine
    }
}

enum RecoveryCursor {
    Observed(CrashEvent),
    Owned { generation: u64, config: Value },
}

impl RecoveryCursor {
    fn generation(&self) -> u64 {
        match self {
            Self::Observed(event) => event.generation,
            Self::Owned { generation, .. } => *generation,
        }
    }

    fn config(&self) -> Option<&Value> {
        match self {
            Self::Observed(event) => event.config.as_ref(),
            Self::Owned { config, .. } => Some(config),
        }
    }
}

#[cfg(test)]
pub(super) fn direct_replay_exit_proven<T, E>(observation: &Result<Option<T>, E>) -> bool {
    matches!(observation, Ok(Some(_)))
}

/// 崩溃监测轮询间隔（ms）。tokio `Child::wait()` 单持有者 → 监测只能轮询 `try_wait`（见
/// `spawn_crash_monitor`）；1s 与健康检查同量级，CPU 可忽略，崩溃检出延迟 ≤1s。
pub(super) const CRASH_MONITOR_POLL_MS: u64 = 1_000;

/// helper 腿的 pid **身份**复核间隔（单位：tick，1 tick = [`CRASH_MONITOR_POLL_MS`]）。
///
/// 不每 tick 复核：macOS 身份取材要 spawn `ps`；Windows 虽已改成原生句柄查询，也没有必要
/// 每秒重复读创建时间。10s 的检出延迟对一件**今天永远检不出**的事是纯增量，不是折衷
///（见 [`process_identity`]）。
pub(super) const PID_IDENTITY_RECHECK_TICKS: u64 = 10;

/// 用**观察完成后的最新世代**区分主动停核与崩溃。
///
/// helper 核的观察腿会做 Windows 进程身份查询；这段同步查询虽没有 `.await`，但在多线程 runtime
/// 上仍可能与另一 worker 上的 `stop()` 并行。若在查询**之前**缓存世代，时序会变成：读到旧世代 →
/// 用户 stop 先 bump 并停核 → 身份查询回报退出 → 拿旧世代误判 Crash，最终把用户刚停掉的 TUN
/// 又由崩溃自愈拉起。把世代读取封在分类点，复用 [`classify_child_exit`] 的既有判据，同时封死这条
/// stale-snapshot 窗口。
/// helper `status` 回传的 `created=` → 身份令牌（纯函数）。
///
/// 只做「u64 → 可比较的令牌」这一步，判定复用 [`pid_identity_verdict`] 的三态口径（缺任一侧材料
/// 判 `Unobservable`，不折成 `Mismatch`）。旧 helper 不回传 ⇒ `None` ⇒ 令牌缺失 ⇒ 不可观测。
pub(super) fn helper_identity_token(created: Option<u64>) -> Option<String> {
    created.map(|ticks| ticks.to_string())
}

pub(super) fn classify_observed_child_exit(
    gate: &LifecycleGate,
    my_generation: u64,
    observation: ChildObservation,
) -> ExitClassification {
    classify_child_exit(my_generation, gate.generation(), observation)
}

impl ProxyRuntime {
    /// 挂后台崩溃监测任务（上游 `singboxProcess.on('exit')` → `handleProcessExit` 的等价物）。
    ///
    /// **为何是轮询而非 `child.wait()`**：tokio `Child::wait()` 需 `&mut self` 单持有者，而主动停止
    /// 路径（`kill_core`）已经持有并 `wait()` 那个句柄 → 崩溃监测不能也去 `wait()`，只能短暂持锁
    /// `try_wait` 观察。轮询绝不跨 await 持 `child` 锁（否则 !Send 编译即拒 + 与 `kill_core` 抢锁）。
    ///
    /// The direct child is tracked by its spawn-bound identity. A request claim
    /// alone cannot retire that monitor: persistent Stop reservation may fail
    /// while the same child keeps running. Helper identity remains on its
    /// existing protocol path and is not promoted to this guarantee.
    pub(super) fn spawn_crash_monitor(
        self: &Arc<Self>,
        my_gen: u64,
        direct_run_identity: Option<RunIdentity>,
    ) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            // helper 腿的 pid 身份基线：`(基线取自哪个 pid, 令牌)`。见 [`process_identity`]。
            let mut identity: Option<(u32, String)> = None;
            // **helper 回传的**身份基线（D3）。与上面那个分开存：两者取材不同源（本地
            // `process_identity` vs helper `status` 的 `created=`），混用会在同一进程上判出假不匹配。
            //
            // **初值取自 start 响应**（D2/D3(4)「start 拿初值 + status 复核」按字面成立）：等第一次
            // status 再取初值会漏掉一格 —— 核在首次 status 之前自然死亡、期间另一方发 start 使旧句柄
            // 被替换关闭 ⇒ 旧 PID 重新可复用 ⇒ 那次 status 读到的已是另一个进程，拿它当基线后续恒
            // `Match`。理由全文见 [`HelperRuntime::managed_start_identity`]。拿不到（旧 helper /
            // 非 Windows）⇒ `None`，退回「首次 status 取初值」的原有口径，不误报。
            let mut helper_identity: Option<(u32, String)> = me
                .helper
                .managed_start_identity()
                .and_then(|(pid, created)| {
                    helper_identity_token(Some(created)).map(|token| (pid, token))
                });
            let mut identity_unobservable_logged = false;
            let mut ticks: u64 = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(CRASH_MONITOR_POLL_MS)).await;
                ticks += 1;
                // 观察核存活。C6-5：helper 核无本地 child 句柄 → 若按 child 观察必得 `Absent`→`Retire`
                //（永不自愈）。改用 pid 探活（对齐 上游 健康检查 `isProcessAlive(activePid)`）：pid 死=崩溃。
                // 直起路径仍走 child.try_wait（仅短暂持锁，绝不跨 await）。
                //
                // **pid 探活只回答「这个号码上有进程吗」**，不回答「是不是我那个」⇒ 核死后号码被复用
                // 时它恒真、崩溃自愈永不触发。故每 `PID_IDENTITY_RECHECK_TICKS` 个 tick 复核一次
                // 进程身份令牌（[`process_identity`]），换人即判退出。
                let observation = if direct_run_identity.is_none()
                    && me.core_via_helper.load(Ordering::SeqCst)
                {
                    match me.pid.lock().ok().and_then(|g| *g) {
                        Some(p) => {
                            if !pid_alive(p) {
                                ChildObservation::Exited
                            } else {
                                // 基线：首次观测到存活时取一次；记账换了 pid（新会话写了 `self.pid`）则重取，
                                // **不**拿旧 pid 的令牌去比新 pid（那会是一次必然的假不匹配）。
                                if identity.as_ref().is_none_or(|(bp, _)| *bp != p) {
                                    identity = process_identity(p).map(|tok| (p, tok));
                                    if identity.is_none() && !identity_unobservable_logged {
                                        identity_unobservable_logged = true;
                                        log::warn!(
                                            "崩溃监测：取不到 pid={p} 的进程身份令牌 → \
                                             本代只按 pid 探活（pid 复用不可发现）"
                                        );
                                    }
                                }
                                let due = ticks.is_multiple_of(PID_IDENTITY_RECHECK_TICKS);
                                let verdict = if due {
                                    pid_identity_verdict(
                                        identity.as_ref().map(|(_, t)| t.as_str()),
                                        process_identity(p).as_deref(),
                                    )
                                } else {
                                    PidIdentity::Match
                                };
                                if verdict == PidIdentity::Mismatch {
                                    log::warn!(
                                        "崩溃监测：pid={p} 的进程身份令牌已变 ⇒ 受管核实际已退出、\
                                         该号码被系统复用（探活恒真是假象）"
                                    );
                                    ChildObservation::Exited
                                } else if identity.is_none() || verdict == PidIdentity::Unobservable
                                {
                                    // Windows 标准权限 app 无法读取 SYSTEM child 的身份/退出码；本地探活
                                    // 会按“未知即存活”长期误报。向同权限边界内的 helper 查询权威状态。
                                    // 同步管道/socket IPC 必须移出 Tokio worker；通信失败不等于核已死，
                                    // 保守沿用刚得到的本地存活观察，等待下一 tick 重试。
                                    let helper = Arc::clone(&me.helper);
                                    match tokio::task::spawn_blocking(move || {
                                        helper.managed_core_status()
                                    })
                                    .await
                                    {
                                        Ok(Ok(ManagedCoreStatus::Stopped)) => {
                                            ChildObservation::Exited
                                        }
                                        Ok(Ok(ManagedCoreStatus::Running {
                                            pid, created, ..
                                        })) if pid == p => {
                                            // D3：pid 相同还不够——号码可能已被复用。helper 持着受管
                                            // 核的进程句柄，它回传的创建时间跨 tick 恒定；变了就是
                                            // 「这个号码上换了进程」。旧 helper 不回传 ⇒ 令牌 None ⇒
                                            // 判 Unobservable ⇒ 维持原有探活口径，绝不误报崩溃。
                                            let token = helper_identity_token(created);
                                            let verdict = pid_identity_verdict(
                                                helper_identity
                                                    .as_ref()
                                                    .filter(|(base_pid, _)| *base_pid == p)
                                                    .map(|(_, t)| t.as_str()),
                                                token.as_deref(),
                                            );
                                            if let Some(token) = token {
                                                helper_identity = Some((p, token));
                                            }
                                            if verdict == PidIdentity::Mismatch {
                                                log::warn!(
                                                    "崩溃监测：helper 报告 pid={p} 的进程创建时间已变 ⇒ \
                                                     受管核实际已退出、该号码被系统复用"
                                                );
                                                ChildObservation::Exited
                                            } else {
                                                ChildObservation::Alive
                                            }
                                        }
                                        Ok(Ok(ManagedCoreStatus::Running { pid, .. })) => {
                                            log::warn!(
                                                "崩溃监测：app 记账 pid={p}，helper 受管 pid={pid} ⇒ \
                                                 当前世代的核身份已失配"
                                            );
                                            ChildObservation::Exited
                                        }
                                        Ok(Ok(ManagedCoreStatus::NativeBirthRunning {
                                            target,
                                            created,
                                            ..
                                        })) => {
                                            let same_birth = match me.child.lock() {
                                                Ok(child) => child.helper_stop_target().is_some_and(|(_, recorded)| recorded == crate::runtime::helper::HelperStopTarget::Birth(target)),
                                                Err(_) => true,
                                            };
                                            let token = helper_identity_token(created);
                                            let verdict = pid_identity_verdict(
                                                helper_identity
                                                    .as_ref()
                                                    .filter(|(base_pid, _)| *base_pid == p)
                                                    .map(|(_, t)| t.as_str()),
                                                token.as_deref(),
                                            );
                                            if let Some(token) = token {
                                                helper_identity = Some((p, token));
                                            }
                                            if same_birth && verdict != PidIdentity::Mismatch {
                                                ChildObservation::Alive
                                            } else {
                                                ChildObservation::Exited
                                            }
                                        }
                                        Ok(Ok(ManagedCoreStatus::BirthRunning { target })) => {
                                            let same_birth = match me.child.lock() {
                                                Ok(child) => child.helper_stop_target().is_some_and(
                                                    |(_, recorded)| {
                                                        recorded
                                                            == crate::runtime::helper::HelperStopTarget::Birth(target)
                                                    },
                                                ),
                                                Err(_) => true,
                                            };
                                            if same_birth {
                                                ChildObservation::Alive
                                            } else {
                                                log::warn!(
                                                    "崩溃监测：Linux helper exact birth 与本代 custody 失配（pid={p}）"
                                                );
                                                ChildObservation::Exited
                                            }
                                        }
                                        Ok(Ok(
                                            ManagedCoreStatus::BirthStopping { .. }
                                            | ManagedCoreStatus::BirthUnknown { .. }
                                            | ManagedCoreStatus::BirthEmpty
                                            | ManagedCoreStatus::BirthUnidentified,
                                        )) => ChildObservation::Alive,
                                        Ok(Err(error)) => {
                                            if ticks == 1
                                                || ticks.is_multiple_of(PID_IDENTITY_RECHECK_TICKS)
                                            {
                                                log::warn!(
                                                    "崩溃监测：helper 权威状态暂不可用（{error}）→ \
                                                     本 tick 保守按本地存活，后续重试"
                                                );
                                            }
                                            ChildObservation::Alive
                                        }
                                        Err(error) => {
                                            log::warn!(
                                                "崩溃监测：helper 状态任务异常（{error}）→ \
                                                 本 tick 保守按本地存活，后续重试"
                                            );
                                            ChildObservation::Alive
                                        }
                                    }
                                } else {
                                    ChildObservation::Alive
                                }
                            }
                        }
                        // pid 已被清（停核/让位收口）→ 视作退场，非崩溃。
                        None => ChildObservation::Absent,
                    }
                } else if let Some(expected) = direct_run_identity.as_ref() {
                    let mut guard = match me.child.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            log::error!("崩溃监测：child lock poisoned: {e} → 退场");
                            return;
                        }
                    };
                    // Stopping retains the Child but retires this monitor. The
                    // future supervisor alone will observe and reap that run.
                    guard.observe_running(expected)
                } else {
                    ChildObservation::Absent
                };
                // helper 腿仍在观察**之后**读请求世代，避免 Windows 的同步身份查询与
                // 主动 stop 并行时误判。直起腿只看它实际持有的 Child 身份：Stop
                // claim 后若持久 CAS 失败，旧 Child 仍运行，监测也必须继续。
                let classification = if direct_run_identity.is_some() {
                    match observation {
                        ChildObservation::Alive => ExitClassification::KeepWatching,
                        ChildObservation::Absent => ExitClassification::Retire,
                        ChildObservation::Exited => ExitClassification::Crash,
                    }
                } else {
                    classify_observed_child_exit(&me.gate, my_gen, observation)
                };
                match classification {
                    ExitClassification::KeepWatching => {}
                    // 主动 stop/restart 接管（世代变 / 句柄被取）→ 退场，不触发自愈。
                    ExitClassification::Retire => return,
                    ExitClassification::Crash => {
                        let Some(event) = me
                            .reset_crashed_run_state(my_gen, direct_run_identity.as_ref())
                            .await
                        else {
                            return;
                        };
                        log::warn!(
                            "检测到 sing-box 意外退出（启动请求世代 {my_gen}，当前请求世代 {}）→ 触发崩溃自愈",
                            me.gate.generation()
                        );
                        me.run_crash_recovery(event).await;
                        return; // 自愈成功会起新核 + 新监测；失败/放弃则本核生命周期终结。
                    }
                }
            }
        });
    }

    pub(super) async fn reset_crashed_run_state(
        &self,
        my_gen: u64,
        direct_run_identity: Option<&RunIdentity>,
    ) -> Option<CrashEvent> {
        // Start/Stop take this gate before changing the physical run and the
        // shared TS state. Hold it through the route reset's await (including
        // its eager cancel) and every synchronous cleanup below.
        let _state_gate = self.mesh.tailscale_state_gate().await;
        // A replacement may land after try_wait released the child lock and
        // before this gate is acquired. A failed Stop claim, however, leaves
        // the same direct Child in place: its monitor must keep observing it.
        if direct_run_identity.as_ref().is_some_and(|expected| {
            !self
                .child
                .lock()
                .ok()
                .is_some_and(|mut slot| slot.running_exit_proven(expected))
        }) || (direct_run_identity.is_none() && self.gate.generation() != my_gen)
        {
            return None;
        }
        // Freeze the config while the old run still owns the TS state gate.
        // A later Start may replace current_config before a deduplicated crash
        // is replayed; reading it then would turn B's exit into C's request.
        let config = self.current_config.read().ok().and_then(|g| g.clone());
        self.mesh.exit_route_reset_state().await;
        self.stop_network_watcher();
        self.disarm_network_canary();
        if let Ok(mut state) = self.runtime_binding_state.lock() {
            *state = RuntimeBindingState::default();
        }
        self.mesh.clear_ts_status();
        self.mesh.clear_vpn_status();
        self.reset_login_fallback_state();
        self.reset_ts_exit_block_state();
        Some(CrashEvent {
            generation: my_gen,
            direct_run_identity: direct_run_identity.cloned(),
            config,
        })
    }

    /// Call only while holding the TS state gate. A direct replay requires the
    /// same Arc-bound Child slot and a fresh exited observation; a newer Start
    /// cannot install its Child until this check releases that gate.
    fn crash_event_still_exited_under_gate(&self, event: &CrashEvent) -> bool {
        if self.gate.generation() != event.generation {
            return false;
        }
        let Some(expected) = event.direct_run_identity.as_ref() else {
            // Helper events are classified by their birth generation. They
            // cannot authorize a cross-generation replay without a run token.
            return true;
        };
        self.child
            .lock()
            .ok()
            .is_some_and(|mut slot| slot.running_exit_proven(expected))
    }

    /// 崩溃自愈执行体：决策全在 [`CrashRecoveryMachine`]（退避 / 上限 / 让位 / 补发），本方法只执行
    /// 「退避 sleep + restart」的 I/O，并把结果反馈回状态机（上游 `attemptAutoRestart` 的 I/O 侧）。
    ///
    /// **绝不无限重启**：`should_auto_restart` 达 `MAX_RESTART_COUNT`(3) → `GiveUp` → 报错并退场；
    /// 60s 冷却窗口内计数不复位（紧密崩溃循环必收敛到 GiveUp）。
    pub(super) async fn run_crash_recovery(self: &Arc<Self>, event: CrashEvent) {
        let mut cursor = RecoveryCursor::Observed(event);
        loop {
            if let RecoveryCursor::Observed(event) = &cursor {
                let _state_gate = self.mesh.tailscale_state_gate().await;
                if !self.crash_event_still_exited_under_gate(event) {
                    return;
                }
            }
            let expected_generation = cursor.generation();
            let Some(cfg) = cursor.config().cloned() else {
                let _state_gate = self.mesh.tailscale_state_gate().await;
                if self.gate.generation() != expected_generation {
                    return;
                }
                let msg = "sing-box 意外退出，且无可用配置重启 → 放弃自愈".to_string();
                log::error!("{msg}");
                self.set_error_if_current(expected_generation, &msg, code::PROCESS_EXITED);
                return;
            };
            let outcome = {
                let mut m = self.crash_lock();
                // M-2′-G1：喂 handle_crash **真实的在途腿世代**（此前硬编码 `None`）。缺此，接管会话
                // （新代核）崩溃永不置 `crash_while_superseded` → 让位腿 replay=false → 新代核崩溃无人接管。
                // 单锁内读 getter + 决策（seam `drive_crash_decision`），绝不 TOCTOU（两次取锁间被改）。
                self.gate.with_current_generation(expected_generation, |_| {
                    let inflight = m.machine.restarting_gen();
                    let outcome = drive_crash_decision(
                        &mut m.machine,
                        monotonic_now_ms(),
                        expected_generation,
                    );
                    if outcome == AutoRestartOutcome::Dedup
                        && inflight.is_some_and(|gen| gen != expected_generation)
                    {
                        // Only a direct Child's Arc-bound identity is a replay
                        // proof. The helper leg has no equivalent immutable run
                        // identity yet; it remains fail-closed on replay.
                        if let RecoveryCursor::Observed(event) = &cursor {
                            if event.direct_run_identity.is_some() {
                                m.pending_replay = Some(PendingReplay {
                                    ticket: m.next_ticket,
                                    event: event.clone(),
                                });
                            }
                        }
                    }
                    if matches!(outcome, AutoRestartOutcome::Attempt { .. }) {
                        m.next_ticket = m.next_ticket.wrapping_add(1);
                    }
                    (outcome, m.next_ticket)
                })
            };
            let Some((outcome, ticket)) = outcome else {
                return;
            };
            match outcome {
                AutoRestartOutcome::GiveUp => {
                    let _state_gate = self.mesh.tailscale_state_gate().await;
                    if self.gate.generation() != expected_generation {
                        return;
                    }
                    // GiveUp 有两种成因，文案必须分开：换核验证窗口下这是**第一次**崩溃，
                    // 报「已达自愈上限（3 次/60s）」是字面为假。这条 message 是诊断载荷，
                    // 会进脱敏日志成为下次排查的起点；UI 只消费结构化码的本地化文案。
                    // 码沿用 `AUTO_RESTART_FAILED`（我们确实放弃了自动重启），不新增码：
                    // 新码要同步前端 `ProxyErrorCode` 与 5 份 locale，而这里的信息差在文案不在分类。
                    let msg = if self.crash_lock().auto_restart_suppressed() {
                        "新内核首次运行即异常退出（换核验证窗口内不自动重启）→ 将尝试回滚到原内核"
                            .to_string()
                    } else {
                        "sing-box 反复崩溃，已达自愈上限（3 次/60s）→ 放弃自动重启".to_string()
                    };
                    log::error!("{msg}");
                    self.set_error_if_current(expected_generation, &msg, code::AUTO_RESTART_FAILED);
                    return;
                }
                // 已有重启腿在途 / 用户已停 → 静默退场。
                AutoRestartOutcome::Dedup | AutoRestartOutcome::AbortedByUser => return,
                AutoRestartOutcome::Attempt {
                    attempt,
                    backoff,
                    generation,
                } => {
                    log::warn!("崩溃自愈：第 {attempt} 次尝试，退避 {backoff:?} 后重启");
                    tokio::time::sleep(backoff).await;
                    let (fate, replay_event) = {
                        let _state_gate = self.mesh.tailscale_state_gate().await;
                        let mut state = self.crash_lock();
                        let fate = state
                            .machine
                            .post_backoff(generation, self.gate.generation());
                        let replay_event =
                            if matches!(fate, RestartFate::Superseded { replay: true }) {
                                state.pending_replay.as_ref().and_then(|pending| {
                                    (pending.ticket == ticket
                                        && self.crash_event_still_exited_under_gate(&pending.event))
                                    .then(|| pending.event.clone())
                                })
                            } else {
                                None
                            };
                        if replay_event.is_some() {
                            state.pending_replay = None;
                        }
                        (fate, replay_event)
                    };
                    match fate {
                        RestartFate::AbortedByUser => {
                            log::info!("崩溃自愈：退避期间用户已主动停止 → 放弃重启");
                            return;
                        }
                        RestartFate::Superseded { .. } => {
                            if let Some(event) = replay_event {
                                log::info!("崩溃自愈：让位，但接管腿也崩溃 → 补发一次");
                                cursor = RecoveryCursor::Observed(event);
                                continue;
                            }
                            log::info!("崩溃自愈：退避期间被更新的 start/stop 接管 → 让位");
                            return;
                        }
                        // 非交互（上游 `start(cfg, {interactive:false})`）：崩溃自愈是**用户没做任何
                        // 操作**时自动发生的，此处弹系统授权框 = 凭空索要管理员密码，且崩溃循环里最多
                        // 连弹 MAX_RESTART_COUNT 次。抑制后退回类型化终态，待用户手动启停时经门引导。
                        RestartFate::Start => {
                            if let Err(error) = self.admit_legacy_start() {
                                let _state_gate = self.mesh.tailscale_state_gate().await;
                                if self.gate.generation() != generation {
                                    let _ = self.crash_lock().post_start(true);
                                    return;
                                }
                                log::warn!("崩溃自愈准入拒绝: {error}");
                                let _ = self.crash_lock().post_start_failure(true);
                                self.report_auto_restart_giveup_if_current(generation, &error);
                                return;
                            }
                            match with_helper_gate_suppressed(
                                self.restart_guarded_outcome(cfg.clone(), Some(generation)),
                            )
                            .await
                            {
                                RestartLeg::Finished(Ok(st), Some(_owned)) if st.running => {
                                    let _ = self.crash_lock().post_start(false);
                                    log::info!("崩溃自愈：重启成功（新 pid={}）", st.pid);
                                    return; // 新核已挂新监测。
                                }
                                // 就绪等待期被接管 → 让位，不报成功（lastStartSuperseded）。
                                RestartLeg::Finished(Ok(_), _) | RestartLeg::Superseded => {
                                    let _ = self.crash_lock().post_start(true);
                                    log::info!("崩溃自愈：重启就绪期被接管 → 让位");
                                    return;
                                }
                                RestartLeg::Finished(Err(e), owned_generation) => {
                                    log::error!("崩溃自愈：重启失败: {e}");
                                    // 不可恢复错误（helper 缺失/用户取消提权门 → 按码；权限/root 残留/
                                    // clash_api 端口占用 → 按 message 关键字）→ 立即终态放弃，不再空耗退避
                                    // （上游 isUnrecoverableRestartError，:6039/:6043）。整个 `e` 而非只
                                    // `e.message`：码腿要读 `e.code`，见 is_unrecoverable_restart_error 文档。
                                    let unrecoverable = is_unrecoverable_restart_error(&e);
                                    let failure =
                                        { self.crash_lock().post_start_failure(unrecoverable) };
                                    match failure {
                                        FailureOutcome::GiveUp => {
                                            self.report_auto_restart_giveup_if_current(
                                                owned_generation.unwrap_or(generation),
                                                &e,
                                            );
                                            return;
                                        }
                                        // 未达上限 → 自循环再试一次（下一轮 attempt 内按计数退避）。
                                        FailureOutcome::Retry => {
                                            cursor = RecoveryCursor::Owned {
                                                generation: owned_generation.unwrap_or(generation),
                                                config: cfg,
                                            };
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// 崩溃自愈 **GiveUp 腿的终态播报**：本次失败没有更具体的码时才补发 [`code::AUTO_RESTART_FAILED`]。
    ///
    /// **修的是什么**：`run_helper_gate` 非交互腿（:1513-1516）自己就 `set_error(HELPER_NOT_INSTALLED)`
    /// 发过一条，回 `Err` 后 [`is_unrecoverable_restart_error`] 判终态 → `post_start_failure(true)`
    /// 返 `GiveUp` → 此处再叠一条 `AUTO_RESTART_FAILED`。**两条码各自在前端触发 `toast.error` +
    /// `notifyDesktop`，且这两腿无人 `await` ⇒ 认领闸门不抑制** ⇒ 用户背靠背吃 2 toast + 2 桌面通知。
    ///
    /// **判据 = [`StartError::code`]，不是回读全局 `status().error_code`**：本文件 8 处
    /// `StartError::coded` 构造点（:1516/:1523/:1540/:1547/:1668/:1704/:1755/:1771）无一例外**紧邻**
    /// 一条同码同文案的 `self.set_error(..)`，而无码腿（`From<String>` 零成本升格的
    /// `.map_err(|e| format!(..))?`）**一条都不 set_error** ⇒ `code.is_some()` ⟺「本次失败已播报过更
    /// 具体的分类」。回读全局则会踩 A1 同款陈旧读（全局 `error_code` 只有 `stop()` 会清、多条腿根本
    /// 不写），理由见 `commands/proxy.rs::start_err_response` 文档。
    ///
    /// **刻意不修过头**：无码腿（config 解析/生成/建目录/写盘失败等）**必须**照常发
    /// `AUTO_RESTART_FAILED` —— 否则崩溃自愈放弃时前端一条提示都收不到，「静默」比「双报」更坏。
    pub(super) fn report_auto_restart_giveup(&self, e: &StartError) {
        if let Some(code) = e.code {
            // 已有更具体的终态码在前 → 只留日志，不叠发第二条事件。
            log::error!(
                "sing-box 崩溃自愈重启失败且达上限 → 放弃：{e}（已按 {code} 播报，不叠发）"
            );
            return;
        }
        let msg = format!("sing-box 崩溃自愈重启失败且达上限 → 放弃：{e}");
        self.set_error(&msg, code::AUTO_RESTART_FAILED);
    }

    /// Commit a terminal error in report -> status -> generation lock order.
    /// The publication guard keeps later claims behind the synchronous event,
    /// but inner/report/status guards are gone before event listeners run.
    fn set_error_if_current(&self, expected_generation: u64, msg: &str, error_code: &str) {
        let _publication = self.gate.lock_generation_publication();
        let mut route = self
            .mesh_route_run
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut status = self
            .status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let committed = self
            .gate
            .with_current_generation(expected_generation, |_| {
                *route = None;
                *status = ProxyStatus {
                    error: Some(msg.to_string()),
                    error_code: Some(error_code.to_string()),
                    ..ProxyStatus::default()
                };
            })
            .is_some();
        drop(status);
        drop(route);
        if committed {
            log::error!("{msg}");
            match self.error_emitter.get() {
                Some(emitter) => emitter.emit_proxy_error(msg, error_code),
                None => {
                    log::debug!("proxy error emitter 未接线 → 跳过 event:proxyError（状态已落值）")
                }
            }
        }
    }

    /// An error receipt may be delivered after a later Start claims.
    pub(super) fn report_auto_restart_giveup_if_current(
        &self,
        expected_generation: u64,
        error: &StartError,
    ) {
        if error.code.is_some() {
            // A more specific error was already emitted by this restart leg;
            // keep the historical suppression without another status write.
            if self.gate.generation() == expected_generation {
                self.report_auto_restart_giveup(error);
            }
            return;
        }
        let msg = format!("sing-box 崩溃自愈重启失败且达上限 → 放弃：{error}");
        self.set_error_if_current(expected_generation, &msg, code::AUTO_RESTART_FAILED);
    }

    /// 短暂借出崩溃自愈状态机（决策同步、单语句用完即释；**绝不跨 await 持锁**）。
    pub(super) fn crash_lock(&self) -> std::sync::MutexGuard<'_, CrashRecoveryState> {
        self.crash_recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 崩溃自愈决策 seam（`run_crash_recovery` 与其单测共用）：读机内在途腿世代 → 喂 `handle_crash`。
///
/// **为什么抽 seam**：`run_crash_recovery` 是重 I/O（退避 sleep + 真起核 = 真机门），其「把在途世代喂给
/// `handle_crash` 而非 `None`」这条 wiring 无法零进程单测。抽成纯 seam 后，`drive_crash_decision_feeds_
/// real_inflight_gen`（proxy 测）可确定性验：把 `m.restarting_gen()` 换回 `None` → replay 恒 false → 转红。
pub(super) fn drive_crash_decision(
    m: &mut CrashRecoveryMachine,
    now_ms: u64,
    current_generation: u64,
) -> AutoRestartOutcome {
    // M-2′-G1：真实在途腿世代（无在途腿 → None）。此前上层硬编码 None → 接管会话崩溃永不置补发标记。
    let inflight_gen = m.restarting_gen();
    m.handle_crash(now_ms, current_generation, inflight_gen)
}

/// 崩溃自愈重启失败是否「不可恢复」→ 立即终态放弃（不再空耗退避）。移植 上游
/// `isUnrecoverableRestartError`（:6039）。
///
/// **码优先，keyword 兜底 —— 两条腿都留，不是二选一**：
///
/// 1. **码腿（新）**：[`StartError::code`] 是判定点在**控制流位置**诚实断言出来的（见 [`code`] 模块
///    文档），比事后猜 message 关键字可靠。[`code::HELPER_GATE_ABORTED`]（用户刚亲口说了「不装」）与
///    [`code::HELPER_NOT_INSTALLED`]（前置条件缺失；非交互自愈下 `run_helper_gate` 连引导都不弹，
///    :1511-1514 直接落此码）两者**重试多少轮都不会自己变好**，故立即终态。
///
///    此前只有 keyword 腿时，这两条腿实际落在错误里的是中文文案 [`HELPER_GATE_ABORTED_MSG`](super::HELPER_GATE_ABORTED_MSG) /
///    [`HELPER_NOT_INSTALLED_MSG`](super::HELPER_NOT_INSTALLED_MSG)，**不命中下方任何一个关键词**（"提权助手，"≠"提权助手不可用"，
///    "提权 helper"里也没有"权限"）⇒ helper 缺失/用户取消时崩溃自愈会白烧满 `MAX_RESTART_COUNT`(3)
///    轮退避才放弃。
///
/// 2. **keyword 腿（原）**：覆盖**没有码**、以及**有码但码本身不表达终态性**的错误形态。
///    **为什么有码也仍要走这条腿**（而不是 `if let Some(c) = code { return matches!(c, ...) }`）：
///    spawn launch 失败腿把**原始 OS 错误**格式化进 message 后贴 [`code::STARTUP_FAILED`]
///    （:1699-1702），EACCES 的 "Permission denied" 正是从那儿来的。若「有码即跳过 keyword」，权限
///    拒绝会退回「烧满 3 轮退避 ~22s」—— 正是 keyword 腿当初要修的那个缺陷。
///
/// 故本函数是既有行为的**严格超集**：只新增 `true`，绝不把原本 `true` 的判成 `false`。瞬态失败
/// （起核超时 / 启动期退出 / 端口资源竞态）两条腿都不命中 ⇒ 照常重试。
pub(super) fn is_unrecoverable_restart_error(err: &StartError) -> bool {
    // 码腿：控制流位置诚实断言出的确定性终态。
    let coded_terminal = err
        .code
        .is_some_and(|c| c == code::HELPER_GATE_ABORTED || c == code::HELPER_NOT_INSTALLED);
    coded_terminal || is_unrecoverable_restart_message(&err.message)
}

/// [`is_unrecoverable_restart_error`] 的 message 关键字腿（权限/提权助手不可用/root 残留/clash_api
/// 端口占用等确定性失败，重试无意义）。CJK 字符 `to_lowercase` 为恒等（无大小写），ASCII 关键词经
/// 小写归一后匹配（如 "Permission denied"）。
pub(super) fn is_unrecoverable_restart_message(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("权限")
        || m.contains("permission")
        || m.contains("helper_gate_aborted")
        || m.contains("提权助手不可用")
        || m.contains("提权助手引导")
        || m.contains("root_orphan_blocked")
        || m.contains("root 残留")
        || m.contains("clash_api_port_busy")
        || m.contains("clash_api 端口")
}

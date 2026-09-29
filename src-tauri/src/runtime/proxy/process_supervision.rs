//! 进程监管 owner：受管核的杀停（直起 / helper 两腿）、启动期 stale-core 清扫与 root 孤儿提权清扫、
//! 起核后的内核二进制自证，以及 pid 层的观测原语（实跑 exe 路径 / 信号 / 探活 / 进程身份令牌）。
//!
//! [`pid_alive`] / [`send_signal`] 被 `proxy` 外部消费（`speedtest.rs` / `tailscale_login_core.rs` /
//! `win_console.rs`），façade 必须 `pub(crate) use` 再导出（§B.3）。

#[allow(dead_code)] // Stopping custody is dormant until its exact supervisor is wired.
mod direct_custody;
#[allow(dead_code)] // No production commit bridge exists in this slice.
mod direct_stop;
pub(crate) use direct_custody::DirectCoreSlot;
#[cfg(test)]
pub(super) use direct_custody::{
    DirectBirthCloseError, ReserveStoppingError, SlotAdmissionError, StopView, TakeRunningError,
};
pub(super) use direct_custody::{HelperStartToken, HelperStopNonce};
#[cfg(test)]
pub(super) use direct_stop::{
    commit_for_test, prepare_direct_stop, prepare_with_io_for_test, CommitDirectStopError,
    CommitRejected, DirectStopIo, DirectStopObservation, DirectStopProvenance, PrepareError,
    StopWaitOutcome,
};

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use polaris_core_supervisor::{scan_running_cores, stale_pids, ProcessKiller, Signal};
use tokio::process::Child;

use crate::runtime::helper::HelperStopOps;
use crate::runtime::win_console::no_console_window;

use super::core_binary::resolve_core_binary;
use super::startup::attestation_commit_allowed;
use super::{code, ProxyRuntime, StartError};

/// A detached Android Stop can finish after its waiter is cancelled. Drop
/// ends only the local booking; uncertainty remains sticky across retries.
pub(super) struct AndroidStopBooking<'a> {
    custody: &'a std::sync::Mutex<Option<super::AndroidGlobalCustody>>,
    birth: super::AndroidRequestBirth,
    nonce: Arc<()>,
    was_certain: bool,
    completed: bool,
}

impl AndroidStopBooking<'_> {
    fn birth(&self) -> super::AndroidRequestBirth {
        self.birth.clone()
    }

    pub(super) fn finish_with_gate(
        self,
        result: Result<(), String>,
        mesh: &crate::runtime::mesh::MeshRuntime,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        self.finish(result, |token| {
            mesh.release_tailscale_main_states_if_token(token, gate)
        })
    }

    pub(super) fn finish_without_main(self, result: Result<(), String>) -> Result<(), String> {
        self.finish(result, |_| {
            Err("Android global Stop lacks the TS gate for a main claim".into())
        })
    }

    fn finish(
        mut self,
        result: Result<(), String>,
        release_main: impl FnOnce(
            &crate::runtime::tailscale_login_core::MainBirthToken,
        ) -> Result<bool, String>,
    ) -> Result<(), String> {
        let mut guard = self
            .custody
            .lock()
            .map_err(|_| "Android global custody poisoned after Stop".to_string())?;
        let attempt = guard
            .as_mut()
            .filter(|attempt| {
                attempt.birth.same(&self.birth)
                    && attempt
                        .stop_inflight
                        .as_ref()
                        .is_some_and(|nonce| Arc::ptr_eq(nonce, &self.nonce))
            })
            .ok_or("Android global Stop custody changed")?;
        if !self.was_certain || attempt.historic_unknown || result.is_err() {
            attempt.stop_inflight = None;
            attempt.historic_unknown = true;
            self.completed = true;
            return result.and_then(|()| {
                Err(
                    "Android global cleanup-unknown: earlier detached Stop may still complete"
                        .into(),
                )
            });
        }
        if let Some(token) = &self.birth.main_token {
            match release_main(token) {
                Ok(true) => {}
                Ok(false) => {
                    attempt.stop_inflight = None;
                    attempt.historic_unknown = true;
                    self.completed = true;
                    return Err("Android main Stop ACK did not match its registry birth".into());
                }
                Err(error) => {
                    attempt.stop_inflight = None;
                    attempt.historic_unknown = true;
                    self.completed = true;
                    return Err(error);
                }
            }
        }
        // This is the same mutex critical section as nonce validation and
        // registry compare-remove. No new Start can observe an unbooked gap.
        *guard = None;
        self.completed = true;
        Ok(())
    }
}

impl Drop for AndroidStopBooking<'_> {
    fn drop(&mut self) {
        if !self.completed {
            if let Ok(mut guard) = self.custody.lock() {
                if let Some(attempt) = guard.as_mut().filter(|attempt| {
                    attempt.birth.same(&self.birth)
                        && attempt
                            .stop_inflight
                            .as_ref()
                            .is_some_and(|nonce| Arc::ptr_eq(nonce, &self.nonce))
                }) {
                    attempt.stop_inflight = None;
                    attempt.historic_unknown = true;
                }
            }
        }
    }
}

/// The blocking Stop returns this non-cloneable permit with its ACK. It stays
/// booked through final Child→pid validation; Drop only releases booking.
pub(super) struct HelperStopPermit {
    child: Arc<std::sync::Mutex<DirectCoreSlot>>,
    attempt: HelperStartToken,
    nonce: HelperStopNonce,
}

impl HelperStopPermit {
    pub(super) fn new(
        child: Arc<std::sync::Mutex<DirectCoreSlot>>,
        attempt: HelperStartToken,
        nonce: HelperStopNonce,
    ) -> Self {
        Self {
            child,
            attempt,
            nonce,
        }
    }

    pub(super) fn nonce(&self) -> &HelperStopNonce {
        &self.nonce
    }

    fn attempt(&self) -> &HelperStartToken {
        &self.attempt
    }
}

impl Drop for HelperStopPermit {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            child.finish_helper_stop_ipc(&self.attempt, &self.nonce);
        }
    }
}

/// Identity of one locally spawned core. The token is minted before spawn and
/// attached only to the resulting Child, so neither a reused PID nor a later
/// lifecycle request can impersonate that run.
#[derive(Clone)]
pub(super) struct RunIdentity(pub(super) Arc<RunToken>);

pub(super) struct RunToken {
    /// Opaque journal correlation, minted once per actual direct Child. The
    /// Arc is still the live instance proof; this string alone cannot prove a
    /// process after app restart or establish plan/artifact ownership.
    persisted_ref: String,
}

impl RunIdentity {
    pub(super) fn new() -> Self {
        Self(Arc::new(RunToken {
            persisted_ref: format!("direct-{}", uuid::Uuid::new_v4()),
        }))
    }

    pub(super) fn same_run(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn persisted_ref(&self) -> &str {
        &self.0.persisted_ref
    }
}

pub(super) struct DirectCoreRun {
    pub(super) child: Child,
    pub(super) identity: RunIdentity,
    #[allow(dead_code)] // Read when the managed coordinator's exact stop gate is wired.
    pub(super) origin: DirectRunOrigin,
    pub(super) main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
}

/// The legacy constructor records Legacy. A future managed coordinator must
/// guard both spawn and slot assignment before treating Managed facts as
/// authority; trusted proxy descendants can still reassign this field.
#[allow(dead_code)] // The managed coordinator is deliberately not enabled yet.
pub(super) enum DirectRunOrigin {
    Legacy,
    Managed(super::mesh_apply::run_birth::ManagedRunFacts),
}

impl DirectCoreRun {
    #[cfg(test)]
    pub(super) fn mark_managed_for_test(&mut self) {
        self.origin = DirectRunOrigin::Managed(
            super::mesh_apply::run_birth::ManagedRunFacts::fixture_for_test(
                self.identity.persisted_ref(),
            ),
        );
    }

    #[cfg(test)]
    pub(super) fn new(child: Child) -> Self {
        Self::with_identity(child, RunIdentity::new())
    }

    pub(super) fn with_identity(child: Child, identity: RunIdentity) -> Self {
        Self {
            child,
            identity,
            origin: DirectRunOrigin::Legacy,
            main_token: None,
        }
    }

    pub(super) fn with_main_token(
        child: Child,
        identity: RunIdentity,
        token: crate::runtime::tailscale_login_core::MainBirthToken,
    ) -> Self {
        let mut run = Self::with_identity(child, identity);
        run.main_token = Some(token);
        run
    }
}

/// SIGTERM→SIGKILL 宽限期（上游 `stopSingBoxProcess` 的 5s 优雅窗口，:5230）。
pub(super) const STOP_GRACE: Duration = Duration::from_secs(5);

/// stale-core 清扫 SIGTERM→SIGKILL 宽限期（对齐 上游 `killOrphanedProcessesLinux` 的 1.5s，:1132）。
pub(super) const STALE_KILL_GRACE: Duration = Duration::from_millis(1_500);

impl ProxyRuntime {
    fn book_android_global_start_locked(
        custody: &mut Option<super::AndroidGlobalCustody>,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<super::AndroidRequestBirth, String> {
        if custody.is_some() {
            return Err("Android global Start/Stop request is still owned".into());
        }
        let birth = super::AndroidRequestBirth {
            identity: Arc::new(()),
            main_token,
        };
        *custody = Some(super::AndroidGlobalCustody {
            birth: birth.clone(),
            stop_only: false,
            start_confirmed: false,
            historic_unknown: false,
            stop_inflight: None,
        });
        Ok(birth)
    }

    /// The TS gate is held by start_inner. Read the real generation while
    /// holding the same Android custody mutex that explicit Start holds across
    /// its generation claim; an older attempt cannot book after being replaced.
    pub(super) fn book_android_global_start_for_generation(
        &self,
        my_gen: u64,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<Option<super::AndroidRequestBirth>, String> {
        let mut custody = self
            .android_main_token
            .lock()
            .map_err(|_| "Android global custody poisoned".to_string())?;
        if self.gate.generation() != my_gen {
            return Ok(None);
        }
        Self::book_android_global_start_locked(&mut custody, main_token).map(Some)
    }

    #[cfg(test)]
    pub(super) fn book_android_global_start(
        &self,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<super::AndroidRequestBirth, String> {
        let mut custody = self
            .android_main_token
            .lock()
            .map_err(|_| "Android global custody poisoned".to_string())?;
        Self::book_android_global_start_locked(&mut custody, main_token)
    }

    pub(super) fn confirm_android_global_start(
        &self,
        birth: &super::AndroidRequestBirth,
    ) -> Result<(), String> {
        let mut custody = self
            .android_main_token
            .lock()
            .map_err(|_| "Android global custody poisoned after Start".to_string())?;
        let attempt = custody
            .as_mut()
            .filter(|attempt| attempt.birth.same(birth) && !attempt.stop_only)
            .ok_or("Android global Start custody changed")?;
        attempt.start_confirmed = true;
        Ok(())
    }

    pub(super) fn begin_android_stop_booking(
        &self,
        allow_main: bool,
    ) -> Result<AndroidStopBooking<'_>, String> {
        let mut guard = self
            .android_main_token
            .lock()
            .map_err(|_| "Android global custody poisoned".to_string())?;
        let nonce = Arc::new(());
        match guard.as_mut() {
            Some(attempt)
                if (allow_main || attempt.birth.main_token.is_none())
                    && (attempt.start_confirmed || attempt.stop_only)
                    && attempt.stop_inflight.is_none() =>
            {
                let was_certain = !attempt.historic_unknown;
                attempt.stop_inflight = Some(Arc::clone(&nonce));
                Ok(AndroidStopBooking {
                    custody: &self.android_main_token,
                    birth: attempt.birth.clone(),
                    nonce,
                    was_certain,
                    completed: false,
                })
            }
            Some(_) => Err(
                "Android global cleanup-unknown: a Start or Stop request may still complete".into(),
            ),
            None => {
                // Cold system-started core cleanup is also a global Stop.
                // It must book an independent birth before delivery.
                let birth = super::AndroidRequestBirth {
                    identity: Arc::new(()),
                    main_token: None,
                };
                *guard = Some(super::AndroidGlobalCustody {
                    birth: birth.clone(),
                    stop_only: true,
                    start_confirmed: false,
                    historic_unknown: false,
                    stop_inflight: Some(Arc::clone(&nonce)),
                });
                Ok(AndroidStopBooking {
                    custody: &self.android_main_token,
                    birth,
                    nonce,
                    was_certain: true,
                    completed: false,
                })
            }
        }
    }

    /// The caller holds the real TS gate across this entire existing stop leg.
    /// Registry removal is only a local bookkeeping consequence of its ACK.
    pub(super) async fn kill_core_and_release_main(
        &self,
        ts_gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        if cfg!(target_os = "android") {
            // Stop booking spans the detached request, registry compare-remove,
            // and global custody removal. The latter two share one short
            // Android-custody→registry critical section under the TS gate.
            super::android_bridge::main_start_dispatch_available().map_err(|(msg, _)| msg)?;
            let booking = self.begin_android_stop_booking(true)?;
            let result = super::android_bridge::stop_core_with_birth(booking.birth()).await;
            return booking.finish_with_gate(result, &self.mesh, ts_gate);
        }
        let token = self.main_token_for_stop()?;
        self.kill_core().await?;
        if let Some(token) = token {
            self.mesh
                .release_tailscale_main_states_if_token(&token, ts_gate)?;
        }
        Ok(())
    }

    /// Called only while the caller holds the real TS state gate. Freeze the
    /// birth token before kill_core can take a direct Child or retire a helper
    /// attempt; no empty-slot inference may clear a registry entry.
    pub(super) fn main_token_for_stop(
        &self,
    ) -> Result<Option<crate::runtime::tailscale_login_core::MainBirthToken>, String> {
        if cfg!(target_os = "android") {
            return self
                .android_main_token
                .lock()
                .map(|custody| {
                    custody.as_ref().and_then(|attempt| {
                        (attempt.start_confirmed
                            && !attempt.historic_unknown
                            && attempt.stop_inflight.is_none())
                        .then(|| attempt.birth.main_token.clone())
                        .flatten()
                    })
                })
                .map_err(|_| "Android global custody poisoned".into());
        }
        self.child
            .lock()
            .map(|child| child.main_token_for_stop(self.core_via_helper.load(Ordering::SeqCst)))
            .map_err(|_| "direct Child token custody poisoned".into())
    }

    /// Register every helper start/stop/cleanup before its blocking IPC is
    /// queued. Child is the serialization point for the direct-stop bridge.
    pub(crate) fn register_helper_backend(&self) -> Result<(), String> {
        self.helper.register_core_mutation()
    }

    #[cfg(test)]
    pub(super) fn register_helper_start_backend(&self) -> Result<HelperStartToken, String> {
        self.register_helper_start_backend_with_main(None)
    }

    pub(super) fn register_helper_start_backend_with_main(
        &self,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<HelperStartToken, String> {
        self.register_helper_backend()?;
        let mut child = self
            .child
            .lock()
            .map_err(|_| "child lock poisoned".to_string())?;
        let token = child
            .begin_helper_start_with_main(main_token)
            .map_err(|error| format!("helper IPC rejected by direct Child custody: {error:?}"))?;
        self.core_via_helper.store(true, Ordering::SeqCst);
        Ok(token)
    }

    /// **内核自证**：核就绪后校验「**实际跑起来的那个二进制**的版本 == 本次期望的核版本」。
    ///
    /// # 这一条为什么必须观测事实（血证）
    ///
    /// 同仓既有的[出口自证](Self::attest_selected_exit)是**纯静态对账**（自述「纯函数、零 I/O」
    /// 「不用探针 / 不查 selector」）：它拿本次生成的 config 与落盘的用户意图互校 —— 两个输入同源于
    /// 「意图」，故意图自洽而事实偏离时它一律判通过。今天这个缺陷正是在它眼皮底下溜过去的：
    /// app 请求 bin=`core_update/sing-box`(1.14.0-beta.3)，helper 实跑
    /// `/Library/Application Support/Polaris/core/sing-box`(1.14.0-alpha.45)，持续一天多、零告警。
    ///
    /// 故本方法**不**对账「我请求了什么 / 我配置了什么」，而是问系统两个事实问题：
    ///  1. **内核记账里，这个 pid 正在执行哪个文件？**（`running_exe_path`：linux `/proc/<pid>/exe`
    ///     符号链接、mac `ps -p <pid> -o comm=`）—— 与我们的请求完全独立的来源；
    ///  2. **那个文件自报什么版本？**（对它真跑一次 `sing-box version`）。
    ///
    /// # 判据与代价
    ///
    /// 路径相同 ⇒ 同一文件，直接通过，**零 spawn**（app 直起腿的稳态走这里）。
    /// 路径不同才各跑一次 `version`（TUN 提权腿的稳态：实跑受保护核副本，版本应相同）。
    /// 「读不出版本」判**告警**而非通过 —— 见 [`CoreBinaryAttestation::VersionUnreadable`]。
    /// 「读不到实跑 exe」判 [`Unobservable`](crate::runtime::core_promote::CoreBinaryAttestation::Unobservable)：只落 warn，
    /// **绝不写成「自证通过」**（没观测到 ≠ 观测到没问题）。
    ///
    /// [`CoreBinaryAttestation::VersionUnreadable`]: crate::runtime::core_promote::CoreBinaryAttestation::VersionUnreadable
    pub(super) fn spawn_running_core_binary_attestation(
        self: &Arc<Self>,
        pid: u32,
        expected: PathBuf,
        my_gen: u64,
    ) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            this.attest_running_core_binary(pid, &expected, my_gen)
                .await;
            log::info!(
                "起核后台耗时：内核二进制自证={}ms（pid={pid}）",
                started.elapsed().as_millis()
            );
        });
    }

    async fn attest_running_core_binary(&self, pid: u32, expected: &Path, my_gen: u64) {
        use crate::runtime::core_promote::{attest_core_binary, CoreBinaryAttestation};

        let expected = expected.to_path_buf();
        // D2：Windows 上 `running_exe_path` 恒 None（Medium IL app 读不了 SYSTEM child），自证因此
        // 恒判「未能进行」。helper 在权限边界另一侧、且持着受管核的句柄，它 status 回传的 `image=`
        // 就是同一个事实。只在**经 helper 起核**时取这条腿：直起腿本地就读得到，不必多一次 IPC。
        let helper = self
            .core_via_helper
            .load(Ordering::SeqCst)
            .then(|| Arc::clone(&self.helper));
        // 观测腿全是阻塞 syscall / 子进程 / 同步 IPC → spawn_blocking。
        let attestation = tokio::task::spawn_blocking(move || {
            let running = running_exe_path(pid)
                .or_else(|| helper_reported_core_image(helper.as_deref(), pid));
            // 路径相同就不必花两次 spawn 去问版本（同一文件，版本必同）。
            let (ev, rv) = match running.as_deref() {
                Some(r) if r != expected.as_path() => (
                    core_version_first_line(&expected),
                    core_version_first_line(r),
                ),
                _ => (String::new(), String::new()),
            };
            attest_core_binary(&expected, running.as_deref(), &ev, &rv)
        })
        .await;

        let attestation = match attestation {
            Ok(a) => a,
            Err(e) => {
                log::warn!("内核自证任务 join 失败（未判定通过）：{e}");
                return;
            }
        };
        let status = self.status();
        if !attestation_commit_allowed(self.gate.generation(), my_gen, &status, pid) {
            log::info!(
                "内核自证完成时启动世代或 pid 已变化 → 丢弃陈旧结论（世代 {my_gen}→{}，pid {pid}→{}）",
                self.gate.generation(),
                status.pid
            );
            return;
        }
        if attestation.is_alarm() {
            // 非终态：核确在跑，只是版本不对 → 保留 running/pid/端口，只落错误两轴 + 广播事件。
            self.set_nonfatal_error(&attestation.user_message(), code::CORE_BINARY_MISMATCH);
            return;
        }
        match attestation {
            // 「没观测到」既不是通过也不是错误：只留痕，绝不说「通过」。
            CoreBinaryAttestation::Unobservable => log::warn!("{}", attestation.user_message()),
            _ => log::info!("{}", attestation.user_message()),
        }
    }

    /// 杀核（接线 core-supervisor [`ProcessKiller`]）：SIGTERM → 宽限 → SIGKILL，并 reap 子进程。
    ///
    /// Empty = no-op；Stopping 由未来的 exact supervisor 持有，本 legacy 腿拒绝取出。
    /// Running 句柄被 take 后必 `wait()` 收割。
    /// helper 腿未确认停止时返回错误，调用方不得继续清运行态或启动第二个核。
    pub(super) async fn kill_core(&self) -> Result<(), String> {
        // Android：核在**本进程内**（libbox），没有 child 可杀、没有 pid 可发信号 —— 停核 = 请
        // `VpnService` 拆隧道。与 `kill_core_via_helper` 同构：**要确定回执**，停不掉就返 Err，
        // 调用方不得据此继续清运行态或起第二个核（tun fd 由 `VpnService.prepare()` 仲裁，
        // 同一时刻只授权一个应用，前一条没拆干净就起第二个必然打架）。
        //
        // `if cfg!` 而非 `#[cfg]` 早退：后者会让下面整段在 Android 编译单元里变成不可达代码。
        if cfg!(target_os = "android") {
            // Stale/system-core cleanup has no TS claim to retire, but its
            // global Stop still needs a nonce and sticky detached-task fence.
            super::android_bridge::main_start_dispatch_available().map_err(|(msg, _)| msg)?;
            let booking = self.begin_android_stop_booking(false)?;
            let result = super::android_bridge::stop_core_with_birth(booking.birth()).await;
            return booking.finish_without_main(result);
        }
        // C6-5：经 helper 起的核 → 经 helper stop（对称）。daemon 摘其受管 child → SIGTERM→宽限→SIGKILL
        // 收割（app 无本地 child 句柄）。阻塞 IPC 挪出 async worker。
        if self.core_via_helper.load(Ordering::SeqCst) {
            return self
                .kill_core_via_helper(Arc::clone(&self.helper) as Arc<dyn HelperStopOps>)
                .await;
        }
        let child_opt = match self.child.lock() {
            Ok(mut g) => g
                .take_running_legacy()
                .map_err(|_| "direct Child is reserved in Stopping custody".to_string())?,
            Err(e) => {
                log::error!("child lock poisoned: {e}");
                return Err(format!("child lock poisoned: {e}"));
            }
        };
        let Some(mut run) = child_opt else {
            return Ok(());
        };
        let pid = run.child.id().unwrap_or(0);
        if pid == 0 {
            // 已退出且被收割 → 仅 reap 残句柄。
            //
            // **同样要清 `self.pid`**：此前这条腿直接 return，把上一次 spawn 的 pid 留在字段里。这不是
            // 罕见角落 —— 核「起来就死」时就绪门的 `try_wait` 会先一步收割它，`child.id()` 随即变 None ⇒
            // 每一次起核失败都从这里走。留下的陈旧 pid 会被 `status()`、诊断、以及 stale 清扫的「受管
            // pid 排除表」当成活的受管核继续引用（排除表里挂个死 pid，等于给同号新进程发免死金牌）。
            let _ = run.child.wait().await;
            if let Ok(mut g) = self.pid.lock() {
                *g = None;
            }
            return Ok(());
        }
        log::info!("停核：pid={pid}（SIGTERM → {STOP_GRACE:?} 宽限 → SIGKILL）");
        // The escalation task survives cancellation of this async Stop. Keep
        // an existing restart/update lease through its final possible SIGKILL.
        let escalation_lease = self.config.retain_active_legacy_start_lease();
        let escalation = ProcessKiller::escalate_async(
            move |sig| {
                let _held = &escalation_lease;
                send_signal(pid, sig);
            },
            move || pid_alive(pid),
            STOP_GRACE,
        )
        .await;
        // 等进程退出（reap，防僵尸）。进程若拒 SIGTERM，升级 task 到点补 SIGKILL 解开此处。
        let _ = run.child.wait().await;
        // 进程已退出 → 取消挂起的 SIGKILL 升级（防 timer 泄漏 + 防 pid 复用误杀）。
        escalation.wait().await;
        if let Ok(mut g) = self.pid.lock() {
            *g = None;
        }
        log::info!("停核完成：pid={pid} 已退出并收割");
        Ok(())
    }

    /// [`kill_core`](Self::kill_core) 的 helper 分支：**带身份**请 daemon 停它自己的受管 child。
    ///
    /// `ops` 参数化（生产传 [`HelperRuntime`](crate::runtime::helper::HelperRuntime)）是为了让本腿可注入替身 —— 否则「请求带没带身份 pid」
    /// 「IPC 期间被接管时记账动没动」两条都只能靠读代码推理，没法变成有牙的门。
    ///
    /// **身份先于 await 取定**（根因）：`stop_inner` 的换代守卫只能在 `kill_core` **返回之后**让位，
    /// 够不着这条 IPC 内部 —— 而经 helper 停核是同步阻塞往返（socket 已删 / daemon 无响应时可以挂
    /// 很久），期间用户完全可能重装 helper 并起了新核。不带身份下发，daemon 就按「停我当前受管的
    /// 那个」执行 = 杀掉用户刚连上的新核（现象：刚连上就被静默断开，且酷似核自己崩了）。
    pub(super) async fn kill_core_via_helper(
        &self,
        ops: Arc<dyn HelperStopOps>,
    ) -> Result<(), String> {
        self.register_helper_backend()?;
        let (attempt, intended, nonce) = self
            .child
            .lock()
            .map_err(|_| "child lock poisoned".to_string())?
            .begin_helper_stop()
            .ok_or_else(|| {
                "helper cleanup-unknown: no confirmed idle Start attempt; refusing Stop(None) or concurrent Stop".to_string()
            })?;
        let permit = HelperStopPermit::new(Arc::clone(&self.child), attempt.clone(), nonce);
        // 阻塞 IPC 挪出 async worker。
        // An updater/restart may be cancelled while this blocking stop still
        // runs. Retain its existing legacy lease in the closure; a normal Stop
        // owns no such lease and must remain available in managed mode.
        let blocking_lease = self.config.retain_active_legacy_start_lease();
        let (result, permit) = match tokio::task::spawn_blocking(move || {
            let _blocking_lease = blocking_lease;
            (ops.stop_managed_core(Some(intended)), permit)
        })
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                let error = format!("helper 停核任务 join 失败：{e}");
                log::error!("{error}");
                return Err(error);
            }
        };
        // The permit remains booked while the ACK is checked and consumed.
        let outcome = match result {
            Ok(()) if self.clear_helper_core_bookkeeping(&permit, intended) => {
                log::info!("经 helper 停核完成（pid={intended}）");
                Ok(())
            }
            Ok(()) => {
                Err("helper cleanup-unconfirmed: Stop acknowledged but attempt changed".into())
            }
            Err(error) => {
                log::warn!("经 helper 停核未完成：{error}");
                Err(error)
            }
        };
        drop(permit);
        outcome
    }

    /// helper 停核腿的记账收口：**只清自己那笔**（[`kill_core`](Self::kill_core) 的 helper 分支专用）。
    ///
    /// `intended` = 本腿进 IPC 前拿到的受管 pid。IPC 往返期间 `self.pid` 可能已被**新会话**写成另一个
    /// pid（这正是身份判据要防的那条时序）。此时把它清成 `None` 的后果不是「多清一次」而是让新核**失联**：
    /// `status()` 的 helper 腿据 `self.pid` 探活、诊断据它报 pid、`cleanup_stale_cores` 的「受管 pid 排除表」
    /// 也据它——排除表里少了新核，下一次起核的孤儿清扫就会把它当孤儿杀掉（换个地方杀错进程）。
    ///
    /// 本方法只会在 helper 已确认 `stopped/notrunning` 后调用；必须仍持有
    /// 同一 Child 的唯一 Stop permit，且 attempt、pid、nonce 全相同才清账。
    /// 通信失败、取消或任何身份变动均保留 helper route。
    pub(super) fn clear_helper_core_bookkeeping(
        &self,
        permit: &HelperStopPermit,
        intended: u32,
    ) -> bool {
        if !Arc::ptr_eq(&permit.child, &self.child) {
            return false;
        }
        let Ok(mut child) = self.child.lock() else {
            log::error!("child lock poisoned：跳过 helper 停核记账收口");
            return false;
        };
        let Ok(mut g) = self.pid.lock() else {
            log::error!("pid lock poisoned：跳过 helper 停核记账收口");
            return false;
        };
        let current = *g;
        if current != Some(intended)
            || !child.confirm_helper_stop(permit.attempt(), intended, permit.nonce())
        {
            log::warn!(
                "helper 停核腿收口时发现受管 attempt/pid 记账已换人（{intended}→{current:?}）→ \
                 整段记账属新会话，不动它（清它等于让新核在 status/诊断/孤儿清扫排除表里集体失联）"
            );
            return false;
        }
        *g = None;
        self.core_via_helper.store(false, Ordering::SeqCst);
        true
    }

    /// **起核前**的 stale-core 清扫：杀掉遗留的**本 app** 孤儿核。跑在**每一次** `start()` 上
    /// （不是只在 app 启动期一次；孤儿也来自本会话中途失败的起核，见 `stale_sweep_disabled` 字段文档）。
    ///
    /// **安全第一性**（本任务核心）：只杀 cmdline 精确匹配 `resolve_core_binary()` 路径 + `run` 的进程
    /// （core-supervisor [`stale_pids`]），并排除 [`sweep_exclusions`](Self::sweep_exclusions) 给出的
    /// 「不是孤儿」的那些 pid（当前受管主核 + 在飞测速临时核 + 在飞 Tailscale 登录核）。
    /// **绝不 `pkill sing-box`**——用户机器上
    /// 可能装有无关的 sing-box。解析不到核二进制 / 非 Linux（扫描返空）→ 静默跳过（fail-closed，不误杀）。
    pub(super) async fn cleanup_stale_cores(&self) -> Result<(), StartError> {
        // 实跑计数：置于所有早退腿之前 —— 计的是「清扫这条腿被走到几次」，而非「杀掉几个孤儿」。
        self.stale_sweep_runs.fetch_add(1, Ordering::SeqCst);
        // ── Android 腿：孤儿的形态是「系统拉起的核」，不是进程（见 [`Self::stop_system_started_core`]）──
        if cfg!(target_os = "android") {
            return self.stop_system_started_core().await;
        }
        let binary = match resolve_core_binary() {
            Ok(b) => b,
            Err(e) => {
                log::debug!("stale 清扫：未解析到核二进制（{e}）→ 跳过");
                return Ok(());
            }
        };
        // **不 canonicalize**：spawner 用 `resolve_core_binary()` 的**字面**路径起核（`Command::new`），
        // /proc 里的 argv[0] 即那个字面路径；两次会话同一 resolve 逻辑 → 字面一致即可匹配。规范化反而会
        // 与含 symlink/`..` 的字面 argv[0] 失配、漏杀自己的孤儿（与 上游 pgrep 用字面 singboxPath 同源）。
        let candidates = scan_running_cores();
        // 排除表（受管主核 + 两种在飞瞬态核）**必须读在扫描之后**，顺序契约见 `sweep_exclusions`。
        let victims = stale_pids(&candidates, &binary, &self.sweep_exclusions());
        if victims.is_empty() {
            return Ok(());
        }
        log::warn!(
            "发现 {} 个上次遗留的孤儿核（本 app 二进制 {}），清理：{victims:?}",
            victims.len(),
            binary.display()
        );
        // SIGTERM → 宽限 → SIGKILL 存活者（对齐 上游 killOrphanedProcessesLinux）。
        for pid in &victims {
            send_signal(*pid, Signal::Sigterm);
        }
        tokio::time::sleep(STALE_KILL_GRACE).await;
        for pid in &victims {
            if pid_alive(*pid) {
                log::warn!("孤儿核 pid={pid} 宽限期未退 → SIGKILL");
                send_signal(*pid, Signal::Sigkill);
            }
        }
        // **T3 二次确认**：SIGKILL 后仍存活 = 用户态根本杀不动（`send_signal` 对 root 进程收 EPERM 且
        // 被 `let _ =` 吞掉，**杀失败与杀成功在调用处无从区分**）。故只能靠再探一次活来判定。
        tokio::time::sleep(STALE_KILL_GRACE).await;
        let survivors: Vec<u32> = victims.iter().copied().filter(|p| pid_alive(*p)).collect();
        if survivors.is_empty() {
            log::info!("孤儿核清理完成：{victims:?}");
            return Ok(());
        }
        self.escalate_root_orphans(&survivors).await
    }

    /// [`Self::cleanup_stale_cores`] 的 Android 腿。
    ///
    /// 核在本进程内（libbox），没有二进制可解析、没有 cmdline 可扫；但「上次会话留下、本运行时不认识
    /// 的核」这件事照样存在：always-on / 开机自动连接在**没有 Rust** 时由系统拉起的那个（配置是上次
    /// 落盘的）。同一个处置：起核前先停掉它，再由本次 start 按当前配置起 —— 不停的话 Kotlin 桥判
    /// 「已在运行」直接拒收这次起核，用户点连接永远连不上。本运行时自己起的核（`core_started`）
    /// 不是孤儿，不碰。停不掉 ⇒ 有码失败（隧道还在，不许在它上面起第二个）。
    async fn stop_system_started_core(&self) -> Result<(), StartError> {
        if super::android_bridge::core_started()
            || !super::android_bridge::system_started_core_running().await
        {
            return Ok(());
        }
        log::warn!("起核前发现系统拉起的核（本运行时未持有）→ 先停掉，再按当前配置起核");
        self.kill_core()
            .await
            .map_err(|e| StartError::coded(e, code::STARTUP_FAILED))
    }

    /// 清扫的**排除表** = 当前受管主核 pid + 此刻在飞的**测速临时核** pid + 此刻在飞的
    /// **Tailscale 瞬态登录核** pid。
    ///
    /// # 为什么这两种瞬态核都必须在这里
    ///
    /// 临时核的 argv 是 `<resolve_core_binary()> run -c <临时配置> --disable-color`
    /// （`SpawnRequest::argv` + `speedtest.rs` 的 `extra_args`）—— 与主核**同一个**二进制路径 + `run`
    /// token ⇒ [`is_our_core`](polaris_core_supervisor::is_our_core) 必然命中：它在候选集里长得跟
    /// 「上次会话遗留的孤儿」一模一样，而清扫这条腿跑在**每一次** `start()` 上（不是只在 app 启动期，
    /// 见 `lifecycle.rs` 的调用点）。于是「测速到一半点连接 / 开 TUN」这条用户日常操作序列必然撞上：
    ///
    /// 1. SIGTERM 掐死正在测的临时核 ⇒ 剩余节点整批作废（且核是被外部杀的，测速侧只看到「核没了」）；
    /// 2. 起核腿白等两段 [`STALE_KILL_GRACE`]（+3.0s）——用户报的「测速中启动 TUN，启动明显变慢」；
    /// 3. 万一那一刻用户态杀不动（EPERM 被 `send_signal` 的 `let _` 吞掉），还会升级到
    ///    [`code::ROOT_ORPHAN_BLOCKED`] 把这次起核**直接判死**。
    ///
    /// **Tailscale 瞬态登录核是同一个缺陷的姊妹腿**（`tailscale_login_core.rs` 的模块文档早就把它
    /// 登记在案、当时以「需要 mesh↔proxy 反向耦合」为由未修）：它同样走
    /// [`resolve_core_binary`] + `SpawnRequest`，argv 逐字同形。用户序列是「点了 Tailscale 登录、
    /// 正等着扫码，顺手去开 TUN」⇒ 登录核被掐死、登录 URL 作废，前端只看到「登录没反应」。
    /// 耦合方向本来就是现成的：`self.mesh` 已在手，`MeshRuntime` 已持有 `LoginCoreRegistry`，
    /// 缺的只是注册表里的 pid 字段（本批补上）。
    ///
    /// # 顺序契约（调用点持有）：本表必须读在 `scan_running_cores()` **之后**
    ///
    /// - 扫描之后才起的瞬态核 ⇒ 不在候选集里 ⇒ 本就杀不到它；
    /// - 扫描之前起的瞬态核 ⇒ 此刻要么仍在表里（被排除），要么已经退出并注销
    ///   （测速的 `TempCorePidGuard` 在收割后出表；登录核在确认 close/reap 后出表）。
    ///
    /// 反过来「先读表再扫描」就漏了一格：读表 → 瞬态核 spawn → 扫描，该 pid 既在候选集又不在表快照里。
    /// 测速腿仍依赖此读取顺序；登录腿另由 `start_guarded` 持有同一 TS state gate 覆盖
    /// 扫描到主核 spawn，登录 spawn/登记和旧实例关闭不会与清扫并发。
    ///
    /// # 两条腿各自的残余窗口（如实登记，别当成全覆盖）
    ///
    /// - **测速临时核**：spawn 返回到登记入表之间那一小段**同步**代码（起点其实是 fork），与主核
    ///   「spawn 完再记 `self.pid`」的窗口同构，是本仓既有的取舍。
    /// - **登录核**：已改为 spawn 后先登记、再 await STATUS；cancel 先确认 close/reap，后出表。
    ///   与起核前持有的 TS state gate 合起来封住本腿的扫描/登记窗口。
    /// - **测速腿**：`victims` 在两段 1500 ms 宽限**之前**冻结；孤儿 pid 若被 init 回收并在
    ///   宽限内由新测速临时核复用，排除表无从追踪该复用（需 pid 回绕，概率极低）。
    pub(super) fn sweep_exclusions(&self) -> Vec<u32> {
        let mut exclude: Vec<u32> = self.pid.lock().ok().and_then(|g| *g).into_iter().collect();
        let temp: Vec<u32> = crate::runtime::speedtest::inflight_temp_core_pids();
        let login: Vec<u32> = self.mesh.inflight_login_core_pids();
        // 有在飞瞬态核时留一行：否则「这次清扫到底排除了谁」在事后只能靠猜，而猜正是本条腿
        // 上一次失守的方式。两种瞬态核都不在飞时（绝大多数起核）不打，不给日志添恒常噪音。
        if !temp.is_empty() || !login.is_empty() {
            log::info!(
                "stale 清扫排除表纳入 {} 个在飞测速临时核 {temp:?} + {} 个在飞 Tailscale 登录核 \
                 {login:?}（受管主核 {exclude:?}）",
                temp.len(),
                login.len()
            );
        }
        exclude.extend(temp);
        exclude.extend(login);
        exclude
    }

    /// **T3**：用户态杀不动的 root 孤儿核 → 经 helper 提权清扫；清不掉则落诚实终态。
    ///
    /// 对齐 上游 `escalateKillRootOrphans` + `ROOT_ORPHAN_BLOCKED`。**为什么必须阻断起核而不是继续**：
    /// 活着的 root 孤儿一直独占 `<userData>/cache.db`，此时起任何新核都会
    /// `initialize cache-file: timeout`，**连切回 systemProxy 模式也起不来**——继续放行只会让用户撞上
    /// 一串无从归因的启动失败。报 [`code::ROOT_ORPHAN_BLOCKED`] 才指得出真正的动作。
    async fn escalate_root_orphans(&self, survivors: &[u32]) -> Result<(), StartError> {
        log::warn!(
            "{} 个孤儿核用户态杀不动（root 所有，EPERM）：{survivors:?} → 尝试经 helper 提权清扫",
            survivors.len()
        );
        // helper 未装 → 无提权腿，直接落终态（不假装尝试过）。
        if self.helper.status().installed {
            self.register_helper_backend()
                .map_err(|message| StartError::coded(message, code::ROOT_ORPHAN_BLOCKED))?;
            let helper = Arc::clone(&self.helper);
            // `cleanup_cores` 是同步阻塞 IPC → 挪出 async worker 线程（同 start_core/stop_core）。
            let blocking_lease = self.config.retain_active_legacy_start_lease();
            match tokio::task::spawn_blocking(move || {
                let _blocking_lease = blocking_lease;
                helper.cleanup_cores()
            })
            .await
            {
                Ok(Ok(())) => {
                    tokio::time::sleep(STALE_KILL_GRACE).await;
                    let still: Vec<u32> = survivors
                        .iter()
                        .copied()
                        .filter(|p| pid_alive(*p))
                        .collect();
                    if still.is_empty() {
                        log::info!("经 helper 提权清扫已清掉 root 孤儿核：{survivors:?}");
                        return Ok(());
                    }
                    // daemon 返成功但进程仍在 → 照实报，不采信回执（结果以探活为准）。
                    log::error!("helper 清扫返回成功，但 {still:?} 仍存活");
                }
                Ok(Err(e)) => log::error!("helper 提权清扫失败：{e}"),
                Err(e) => log::error!("helper 清扫任务 join 失败：{e}"),
            }
        } else {
            log::error!("helper 未安装 → 无提权腿可用，root 孤儿核 {survivors:?} 清不掉");
        }
        let msg = format!(
            "上次遗留的 sing-box 核（pid {survivors:?}）以管理员权限运行且无法清理，\
             它占用着内核缓存文件，任何模式都无法启动。请安装/修复 Helper 后重试，\
             或手动执行：sudo kill -9 {}",
            survivors
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
        self.set_error(&msg, code::ROOT_ORPHAN_BLOCKED);
        Err(StartError::coded(msg, code::ROOT_ORPHAN_BLOCKED))
    }
}

/// **观测腿**：内核记账里该 pid 正在执行的可执行文件路径（读不到 → `None`）。
///
/// 这是[内核自证](ProxyRuntime::attest_running_core_binary)的**事实来源**，其价值全在于它与
/// 「app 请求了什么」完全独立 —— 问的是操作系统「这个进程实际是从哪个文件起来的」。
///
/// - **linux**：读 `/proc/<pid>/exe` 符号链接（内核直给，最硬的一手证据）。二进制在进程起来后被
///   替换/删除时内核会给出 `<路径> (deleted)`，此处剥掉该后缀还原原路径（否则恒判不等 = 假告警）。
/// - **macOS**：`ps -p <pid> -o comm=`（无 `/proc`；`comm` 给的是完整路径而非 16 字节的 `p_comm`
///   短名——2026-07-31 在 p101 以普通用户查 root helper 实测得到完整 46 字符路径）。
///   受保护核路径含空格，故 `comm=` 必须是**唯一**输出字段，整行即路径。
/// - **windows**：返 `None`（无低成本 std 途径）。`None` ⇒ 判 `Unobservable` ⇒ 只 warn 不误报。
///   **P4 起该平台已有受保护核目录**（`C:\ProgramData\Polaris\core`），故本自证在 win 上不再是
///   「本就没价值」而是「这条腿还没接」——真正的 win 侧实跑映像来自 D2/D3 的 `status` 回传
///   `image=`（见 spec §3.1），不是本函数。
fn running_exe_path(pid: u32) -> Option<PathBuf> {
    // pid=0 = 调用方还没拿到真 pid（helper 未回传 / spawn 失败）→ 没有可观测对象。
    if pid == 0 {
        return None;
    }
    running_exe_path_impl(pid)
}

/// **第二观测腿**：helper `status` 回传的实跑映像（`image=`，D2）。
///
/// 同步 IPC，只在 [`running_exe_path`] 取不到时才调用（Windows 的 helper 腿）。通信/协议失败
/// 一律 `None` —— 读不到就是读不到，自证据此判 `Unobservable`（只 warn），绝不冒充通过。
fn helper_reported_core_image(
    helper: Option<&crate::runtime::helper::HelperRuntime>,
    pid: u32,
) -> Option<PathBuf> {
    let status = helper?.managed_core_status().ok()?;
    image_from_managed_status(&status, pid)
}

/// 纯判定：**只有 helper 手里的受管 pid 与本代 pid 相同**才采信它回传的 image。
///
/// pid 对不上说明 helper 正管着另一个会话的核 —— 拿它的映像去和本代期望值对账，判等判不等都是
/// 在回答另一个问题；那种结论比「没观测到」更坏，因为它看起来像事实。
pub(super) fn image_from_managed_status(
    status: &crate::runtime::helper::ManagedCoreStatus,
    want_pid: u32,
) -> Option<PathBuf> {
    match status {
        crate::runtime::helper::ManagedCoreStatus::Running { pid, image, .. }
            if *pid == want_pid =>
        {
            image.as_deref().map(PathBuf::from)
        }
        _ => None,
    }
}

/// [`running_exe_path`] 的 linux 实现：`/proc/<pid>/exe` 符号链接（内核直给）。
#[cfg(target_os = "linux")]
fn running_exe_path_impl(pid: u32) -> Option<PathBuf> {
    let p = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    // 内核对「映像已被替换/删除」的进程追加 " (deleted)"，剥掉还原真实路径（否则恒判不等 = 假告警）。
    let s = p.to_string_lossy();
    Some(
        s.strip_suffix(" (deleted)")
            .map_or_else(|| p.clone(), PathBuf::from),
    )
}

/// [`running_exe_path`] 的 macOS 实现：`ps -p <pid> -o comm=`（无 `/proc`）。
#[cfg(target_os = "macos")]
fn running_exe_path_impl(pid: u32) -> Option<PathBuf> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    // 进程已退出时 ps 可能成功但无输出 → 别把空串当成一个路径。
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// [`running_exe_path`] 的其余平台实现：无低成本 std 途径 → 恒 `None`（判 `Unobservable`，不误报）。
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn running_exe_path_impl(_pid: u32) -> Option<PathBuf> {
    None
}

/// **观测腿**：对**磁盘上那个文件**跑一次 `sing-box version`，取原始第一行；失败恒空串。
///
/// 与 `UpdaterRuntime::read_core_version_line` 同一纪律：**探测失败绝不回落随包基线** ——
/// 那会把「读不到」伪装成「就是基线」，正是自证最不能犯的错。此处更严：空串在
/// [`attest_core_binary`](crate::runtime::core_promote::attest_core_binary) 里被判**告警**而非通过。
fn core_version_first_line(bin: &Path) -> String {
    match no_console_window(std::process::Command::new(bin).arg("version")).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        Ok(out) => {
            log::warn!(
                "{} version 非零退出 {:?}：版本行置空",
                bin.display(),
                out.status
            );
            String::new()
        }
        Err(e) => {
            log::warn!("{} version spawn 失败 {e}：版本行置空", bin.display());
            String::new()
        }
    }
}

/// 发信号给 pid（core-supervisor [`ProcessKiller`] 的注入点）。
///
/// unix：`nix::sys::signal::kill`（safe wrapper，本文件 `forbid(unsafe_code)` 下不可直接 libc FFI）。
#[cfg(unix)]
pub(crate) fn send_signal(pid: u32, sig: Signal) {
    use nix::sys::signal::{kill, Signal as NixSignal};
    let nix_sig = match sig {
        Signal::Sigterm => NixSignal::SIGTERM,
        Signal::Sigkill => NixSignal::SIGKILL,
    };
    // 对已退出进程为安全 no-op（ESRCH）——吞掉。非法 pid 直接不发（见 checked_pid）。
    if let Some(p) = checked_pid(pid) {
        let _ = kill(p, nix_sig);
    }
}

/// windows 无 POSIX 信号：两级均退化为 `taskkill /F /T`（对齐 上游 Windows 停核路径）。
/// **未在本机验证**（本批真机验证限 Linux）。
#[cfg(windows)]
pub(crate) fn send_signal(pid: u32, _sig: Signal) {
    let _ = no_console_window(std::process::Command::new("taskkill").args([
        "/PID",
        &pid.to_string(),
        "/F",
        "/T",
    ]))
    .output();
}

/// `u32` pid → `nix::Pid`，**只放行真实单进程 pid**（`1..=i32::MAX`），否则 `None`。
///
/// **为什么必须有（安全，非洁癖）**：`pid as i32` 对 `pid > i32::MAX` 会**回绕成负数**，而 POSIX
/// `kill` 的负数/零 pid 是**广播语义**：`-1` = 给「本用户有权发信号的所有进程」发，`0` = 给整个
/// 当前进程组发。落到 [`send_signal`] 就是 `SIGKILL` 全场——把 app 自己和用户所有进程一起杀掉。
/// 落到 [`pid_alive`] 则是 `kill(-1,0)` 恒 `Ok` → 任何越界 pid 都被判「存活」，孤儿清扫永远收不了尾。
#[cfg(unix)]
fn checked_pid(pid: u32) -> Option<nix::unistd::Pid> {
    (pid >= 1 && pid <= i32::MAX as u32).then(|| nix::unistd::Pid::from_raw(pid as i32))
}

/// `kill(pid, 0)` 的 errno → 存活判定（纯逻辑，穷举各 errno 语义；探活的真值在此）。
///
/// **判定方向恒为「无死亡证据即判存活」**——五个消费点（起核门 / 就绪门 / 崩溃监测 /
/// 停核升级 / 孤儿清扫）里，误判「死」全是破坏性的（虚报起核失败、无谓重启、漏发 SIGKILL），
/// 误判「活」最多多发一次信号（对已死进程是 no-op）。故只有确证不存在才判不活。
// nix 是 unix-only 依赖，故必须 cfg(unix)——`test` cfg 在 windows `cargo test` 也为真，
// 若含 test 会在 windows 编入却找不到 nix crate（E0433）。测试端一并 cfg(unix)。
#[cfg(unix)]
pub(super) fn alive_from_probe(r: Result<(), nix::errno::Errno>) -> bool {
    use nix::errno::Errno;
    match r {
        // 有权发信号且进程在 → 存活。
        Ok(()) => true,
        // **EPERM = 进程存在，只是不属本用户**（helper 以 root 起的核，app 以普通用户探活）。
        // 把它当「不存在」正是 TUN 提权路径下「helper 报告已启动但进程不存在」的根因。
        Err(Errno::EPERM) => true,
        // ESRCH = 内核确认无此进程 → 唯一的「不活」判据。
        Err(Errno::ESRCH) => false,
        // 其余 errno（EINVAL 等）非死亡证据 → 保守判活，绝不据此宣告核已崩。
        Err(_) => true,
    }
}

/// pid 是否存活（宽限期到点的二次确认，防 race 误杀）。
#[cfg(unix)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    // 非法 pid（0 / 越 i32 回绕）不是「不确定」而是「压根不是个进程」→ 判不活，且**绝不**让它
    // 走到 kill 的广播语义上去（见 [`checked_pid`]）。
    let Some(p) = checked_pid(pid) else {
        return false;
    };
    // signal 0 = 仅探活不发信号。
    alive_from_probe(kill(p, None))
}

/// **进程身份令牌**：回答「这个 pid 上挂的还是不是原来那个进程」。
///
/// # 为什么需要它
///
/// helper 腿（三平台的 TUN 一律经 helper，见 `should_start_via_helper`）没有本地 child 句柄，
/// 崩溃监测只能靠 [`pid_alive`] —— 而 `kill(pid, 0)` / Win32 探活只回答「这个号码上有进程吗」，
/// **不回答「是不是我那个」**。核死后 pid 被系统复用，探活恒真 ⇒ 崩溃自愈永不触发，
/// 用户看到 `running: true` 而代理全断。直起腿不受影响（`child.try_wait()` 认的是句柄不是号码）。
///
/// # 为什么不复用 [`running_exe_path`]
///
/// 它在本场景最需要的两个平台上取不到材料：linux 的 `/proc/<pid>/exe` 对 root / setuid 降权后的
/// 进程，普通用户读会 `EACCES`（helper 腿的核正是这两类）；windows 侧它恒 `None`。
///
/// # 各平台取什么（性质相同：**活着期间恒定不变，换了进程必不同**）
///
/// - **linux**：`/proc/<pid>/stat` 的 starttime（第 22 字段）。该文件**世界可读**，不受属主与
///   dumpable 影响 —— 正是 exe 那条路取不到时仍取得到的那一格。
/// - **macos**：`ps -p <pid> -o lstart=`（跨用户可读，同 [`running_exe_path`] 的 mac 腿）。
/// - **windows**：`OpenProcess + GetProcessTimes` 的创建时间。它比旧 `tasklist` 映像名更强（同名
///   进程复用 PID 也能识别），且不再为每次探活启动一个约 3.5s 的外部进程。
/// - **其余平台**：`None` ⇒ 判 [`PidIdentity::Unobservable`]，只跳过、**绝不**据此报崩溃。
pub(super) fn process_identity(pid: u32) -> Option<String> {
    // pid=0 = 还没拿到真 pid → 没有可观测对象（同 `running_exe_path` 的口径）。
    if pid == 0 {
        return None;
    }
    process_identity_impl(pid)
}

#[cfg(target_os = "linux")]
fn process_identity_impl(pid: u32) -> Option<String> {
    parse_proc_stat_starttime(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[cfg(target_os = "macos")]
fn process_identity_impl(pid: u32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    // 进程已退出时 ps 可能成功但无输出 → 别把空串当成一个令牌。
    (!line.is_empty()).then_some(line)
}

#[cfg(windows)]
fn process_identity_impl(pid: u32) -> Option<String> {
    crate::runtime::windows_process::creation_identity(pid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_identity_impl(_pid: u32) -> Option<String> {
    None
}

/// `/proc/<pid>/stat` → starttime（第 22 字段，纯逻辑）。
///
/// **必须从最后一个 `)` 之后切**：第 2 字段 comm 被括号包着，且**可含空格与右括号**
/// （进程名由用户控制）⇒ 直接按空白切分会在这类进程上整体错位，取到一个恒变或恒不变的错字段。
/// 切完后首 token 是第 3 字段 state ⇒ starttime 是其中第 20 个（下标 19）。
#[cfg(any(target_os = "linux", test))]
pub(super) fn parse_proc_stat_starttime(stat: &str) -> Option<String> {
    let tail = &stat[stat.rfind(')')? + 1..];
    tail.split_whitespace().nth(19).map(str::to_owned)
}

/// pid 身份复核的三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PidIdentity {
    Match,
    Mismatch,
    Unobservable,
}

/// 基线令牌 × 当前令牌 → 三态（纯逻辑）。
///
/// **「没观测到」绝不折成「不匹配」**：取不到材料（平台不支持 / 读失败 / 进程刚好在这一刻消失）
/// 一律 [`PidIdentity::Unobservable`]。折成 `Mismatch` 会把一次读失败变成一次**假崩溃**，
/// 而假崩溃的下游是自动重启 —— 本仓在 `running_exe_path` 那条自证腿上写过同一句：
/// 没观测到 ≠ 观测到没问题。
pub(super) fn pid_identity_verdict(baseline: Option<&str>, current: Option<&str>) -> PidIdentity {
    match (baseline, current) {
        (Some(a), Some(b)) if a == b => PidIdentity::Match,
        (Some(_), Some(_)) => PidIdentity::Mismatch,
        _ => PidIdentity::Unobservable,
    }
}

#[cfg(windows)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    crate::runtime::windows_process::is_alive(pid)
}

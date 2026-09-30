//! Tailscale 瞬态登录核的**宿主层编排**（spawn / STATUS 订阅 / emit / 生命周期注册表）。
//!
//! 纯逻辑（config 生成、双写守卫、登录状态机）在 `polaris_mesh::tailscale_login`，已单测；
//! 本模块只做**运行时接线**：拉起一个独立的瞬态 sing-box、订阅**它自己的**管理 API
//! `SubscribeTailscaleStatus` 流、把帧里的 `authURL` 转成登录 URL 事件、把 `backendState == "Running"`
//! 当作登录成功并就地收核，并管理它的生死（kill-on-relogin / 超时自动杀 / 取消 / 自然退出 reap），
//! 与 `ProxyRuntime` 的常驻代理核**隔离**（独立注册表、独立 child 句柄；瞬态核绝不写进 proxy 的
//! pid 槽，故不会被误当作代理核）。
//!
//! ## 为什么 URL 只认 gRPC，不再扫 stdout（含「gRPC 腿失败要不要回退 stdout」的结论）
//!
//! 曾经的实现从核 stdout 正则抓 `Waiting for authentication: <url>`。改掉它有两条独立理由：
//!
//! 1. **那行是日志文案，不是契约**。上游 `protocol/tailscale/endpoint.go` 里它就是一句
//!    `logger.Info("Waiting for authentication: ", authURL)`；改文案、改前缀、改日志等级都不算破坏性
//!    变更，而 `TailscaleEndpointStatus.authURL` 是 proto 字段，字段号由 `crates/singbox-grpc` 的两道
//!    机械门看守（build.rs 对随包核 descriptor 对账 + `tests/bundled_core_wire.rs`）。
//! 2. **stdout 路径拿不到「登录成功」**。此前的登录成功判据是「无法判定」（`LoginState::NoStatusFallback`），
//!    于是核要么空跑到 5 分钟超时、要么靠用户手动取消 —— 期间它一直占着该节点的 `state_directory`。
//!    `backendState == "Running"` 是控制面给的**终局肯定**，拿到即收核。
//!
//! **gRPC 腿失败时不回退 stdout，硬失败**。取舍写在这里以免下一轮又被「多一条兜底更稳」翻回去：
//! - 「两份 URL 来源」正是本次要消灭的漂移。留一条 stdout 兜底 = 两个解析器、两种格式、两条各自
//!   可能先到的路径，而它们对「登录成功」的能力**不对等**：走上兜底那一刻，功能就悄悄退回改造前的
//!   形态（有 URL、判不了成功、核空跑到超时），且**没有任何人会看见这次降级**。本仓已有过一次同型
//!   教训：`reconnect.rs` 用没接 sink 的日志门面，静默让同一根因扛过两轮修复。
//! - 兜底能覆盖的失败面本来就很窄：api service bind 不上 → 核直接 FATAL 退出，stdout 同样什么都没有；
//!   配置形状不对 → 已被 spawn 前的 `sing-box check` 挡下。真正只属于 gRPC 腿的失败是「核活着但订阅
//!   建不起来」，而 `ReconnectingStream` 本身就带退避重连，这类抖动它自己会吞掉；真的一直连不上，
//!   由既有超时臂杀核并留下明确日志 —— 这是**响的**失败，不是静默降级。
//!
//! stdout/stderr 经已知密钥与登录链接脱敏后转日志，但不再是任何判据的来源。
//!
//! ## 诚实边界（务必读）
//! 本命令的**端到端价值 = 真 sing-box + 真出站 + 真 Tailscale 控制面**：起一个真核去连 Tailscale 控制服务器、
//! 把它吐的登录 URL 转发给用户。这条真机路径**在本 Linux 开发机上无法验证**（本仓禁跑触碰宿主网络的测试；
//! `sing-box check` 只验配置形状，验不了「核真的吐出登录 URL」这一运行时行为）。因此：
//! - 本模块的**全部可单测面**（注册表生命周期、命令决策流、去重、超时、取消、reap、STATUS→URL relay、
//!   Running→收核）都以注入的 mock [`LoginCoreSpawner`]/[`ConfigChecker`]/[`AuthUrlEmitter`]/
//!   [`LoginStatusSubscriber`] 单测——**无真进程、无网络、无真 sing-box、无真 gRPC**。
//! - **真 spawn + 控制面握手 + 真登录 URL** 一段**在此未验证**，门槛是一次真机会话（见
//!   `~/docs/polaris/design/polaris-tailscale-login-wiring.md` 的验收清单）。不得据本模块宣称「登录端到端可用」。
//!
//! ## 与 `cleanup_stale_cores` 的关系
//!
//! `ProxyRuntime::cleanup_stale_cores` 在**每一次起代理核前**清扫「本 app 二进制」的孤儿核。瞬态登录核
//! 与主核同一个 [`resolve_core_binary`] + [`SpawnRequest`]，argv 逐字同形（`<同一核二进制> run -c <cfg>
//! --disable-color`）⇒ `is_our_core` 必然命中 ⇒ 在候选集里它与「上次会话遗留的孤儿」不可区分。
//!
//! **在飞的登录核已被排除**：清扫的排除表经 `ProxyRuntime::sweep_exclusions` →
//! `MeshRuntime::inflight_login_core_pids` → [`LoginCoreRegistry::inflight_login_pids`] 读本注册表
//! （耦合方向是现成的：`ProxyRuntime` 已持有 `Arc<MeshRuntime>`，`MeshRuntime` 已持有本注册表）。
//! 一个**上次会话遗留**的登录核不在表里，届时被扫掉仍是**期望行为**（它本就该在应用退出时清）。
//!
//! PID 在 spawn 后、订阅 await 前登记，直到 terminate + reap 才注销；主核起停与登录共用 state gate。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot, watch};

mod attempts;
use attempts::{Attempt, AttemptGuard, Attempts};
pub use attempts::{LoginMode, LoginProgressReceipt, LoginRequest};

#[cfg(test)]
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::ServerConfig;
use polaris_core_supervisor::port_bookkeeping::TokioPortProvider;
use polaris_core_supervisor::{
    run_check_raw, PortAllocator, PortExclusions, ProcessKiller, RawCheck, SingBoxSpawner,
    SpawnError, SpawnRequest, StdioPolicy, TokioSpawner, CONFIG_CHECK_TIMEOUT,
};
use polaris_mesh::tailscale_login::{
    advance_login_state, build_tailscale_login_config, login_config_to_json, LoginEvent,
    LoginState, TailscaleLoginApiService, TAILSCALE_LOGIN_ENDPOINT_TAG,
};
use polaris_singbox_grpc::{daemon, Endpoint, ReconnectConfig, SingBoxApiClient};

use crate::events::broadcast;
use crate::runtime::proxy::core_log::pipe_to_log_with_secrets;
#[cfg(not(target_os = "android"))]
use crate::runtime::proxy::resolve_core_binary;
use crate::runtime::proxy::{pid_alive, send_signal};
use crate::runtime::tailscale_status::decode_tailscale_status;

/// 瞬态登录核的最大挂起时长：登录不完成（用户不去浏览器认证）时到点自动杀核，避免核无限挂着。
/// 交互登录需人去浏览器完成，故给宽松窗口（5 分钟）。
const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// 每个条目都对应一个 sing-box 子进程、两条日志管道和一条 gRPC 重连流；IPC 可并发调用，必须硬封顶。
const MAX_ACTIVE_LOGIN_CORES: usize = 8;

/// 杀瞬态核的优雅窗口（SIGTERM → 宽限 → SIGKILL）。对齐 `ProxyRuntime` 的 `STOP_GRACE`（5s）。
const LOGIN_STOP_GRACE: Duration = Duration::from_secs(5);

/// 瞬态登录核子进程日志行的 target。
///
/// 喂给全 app 唯一那份排空实现 [`crate::runtime::proxy::core_log::pipe_to_log`]。**不能用主核的
/// `SING_BOX_TARGET`**：落盘按字面 target 选文件，混进去会污染 `singbox.log` 的连续性，日志页按
/// 来源筛也分不出是哪个核。
const LOGIN_CORE_LOG_TARGET: &str = "tailscale-login";

/// Ready primary-core identity and ports, read under the shared state gate.
#[derive(Default, Clone)]
pub struct MainLoginSnapshot {
    pub alive: bool,
    pub generation: u64,
    pub api_secret: String,
    pub api_port: u16,
    pub http_port: Option<u16>,
    pub mixed_port: Option<u16>,
}

/// Production resolves the bundled binary; tests inject a path without running a real core.
type BinaryResolver = Arc<dyn Fn() -> Result<PathBuf, String> + Send + Sync>;

// ── 抽象 trait（生产真实现 / 测试 mock；这是「无真进程无网络单测」的关键）────────────────────────

/// 瞬态登录核子进程抽象。生产用 `tokio::process::Child` 包装；测试用内存 duplex 假子进程，
/// 使整条编排（spawn/emit/register/timeout/cancel/reap）可在无真进程无网络下驱动。
///
/// **这里没有取管道的方法**：两条流的去向在 [`SpawnRequest`] 的 [`StdioPolicy`] 里一次说清，
/// spawner 在返回之前就把读端交给了排空回调。child 只剩生命周期职责 —— 「拿到了 child 却忘记
/// 去读它的管道」这条路已经不存在。
#[async_trait]
pub trait LoginCoreChild: Send {
    /// 子进程 pid（假子进程返回占位值）。
    ///
    /// **不只是日志**：`start_attempt` 把它记进 [`LoginEntry::pid`]，
    /// [`LoginCoreRegistry::inflight_login_pids`] 据此喂 `ProxyRuntime::cleanup_stale_cores`
    /// 的排除表 —— 返错 pid 会让清扫放过一个真孤儿，返 `None` 只会让本核在飞时失去保护。
    fn pid(&self) -> Option<u32>;
    /// 等子进程自然退出并收割（cancel-safe：可在 `select!` 中反复创建/丢弃）。
    async fn wait(&mut self);
    /// 主动终止并收割：生产 SIGTERM→宽限→SIGKILL 后 `wait()`；测试置终止标记即返回。
    async fn terminate(&mut self);
    /// An Android close must be acknowledged by its own host before the state-directory claim is released.
    async fn after_exit(&mut self) -> Result<(), String> {
        Ok(())
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        self.terminate().await;
        Ok(())
    }
}

/// spawn 抽象：返回 [`LoginCoreChild`] 装箱句柄。生产 [`TokioLoginCoreSpawner`] 内部经 [`TokioSpawner`] 起真核。
#[async_trait]
pub trait LoginCoreSpawner: Send + Sync {
    /// spawn 一个瞬态登录核。失败返 [`SpawnError`]（ENOENT/EACCES）。
    ///
    /// **按值收请求**（与 [`SingBoxSpawner`] 同）：请求里的排空回调是 `FnOnce`，spawner 必须能
    /// 消费掉它。假 spawner 也一样要把自己那两条内存流喂给同一个回调，测试才走的是生产接线。
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError>;
}

/// `sing-box check` 抽象：spawn 前先验配置形状（fail-fast）。生产真跑 `sing-box check -c <file>`，测试 mock。
#[async_trait]
pub trait ConfigChecker: Send + Sync {
    /// 校验 `config_path` 是否为合法 sing-box 配置。非法 → Err（含核的诊断）。
    async fn check(&self, binary: &Path, config_path: &Path) -> Result<(), String>;
    /// Android admission failures are explicit causes; all existing checkers retain their behavior.
    async fn check_admitted(
        &self,
        binary: &Path,
        config_path: &Path,
    ) -> Result<(), crate::runtime::proxy::android_capacity::CheckFailure> {
        self.check(binary, config_path)
            .await
            .map_err(crate::runtime::proxy::android_capacity::CheckFailure::Rejected)
    }
}

/// 登录 URL 事件发射抽象。生产经 [`AppHandle`] 广播 `event:tailscaleAuthUrl`，测试捕获断言。
pub trait AuthUrlEmitter: Send + Sync {
    /// 发射一条登录 URL 事件（URL 首次出现或发生变更时发）。
    fn emit_auth_url(&self, server_id: &str, node_name: &str, url: &str);
    fn progress(
        &self,
        _server_id: &str,
        _attempt_id: &str,
        _phase: &str,
        _reason: Option<&str>,
        _url: Option<&str>,
    ) {
    }
}

/// Save the receipt before broadcasting. Android may suspend the WebView while Chrome owns the
/// foreground; the panel can read this exact attempt after focus even when an event was missed.
struct AttemptReceiptEmitter {
    inner: Arc<dyn AuthUrlEmitter>,
    attempt: Arc<Attempt>,
    attempt_id: String,
}

impl AuthUrlEmitter for AttemptReceiptEmitter {
    fn emit_auth_url(&self, server_id: &str, node_name: &str, url: &str) {
        self.inner.emit_auth_url(server_id, node_name, url);
    }

    fn progress(
        &self,
        server_id: &str,
        attempt_id: &str,
        phase: &str,
        reason: Option<&str>,
        url: Option<&str>,
    ) {
        if server_id == self.attempt.server_id && attempt_id == self.attempt_id {
            self.attempt.record_progress(LoginProgressReceipt {
                server_id: server_id.to_owned(),
                attempt_id: attempt_id.to_owned(),
                phase: phase.to_owned(),
                // Only stable UI reason codes cross this read API. Native error text could
                // contain a URL or secret and the UI already maps unknown codes to a generic
                // authorization error.
                reason: reason
                    .filter(|reason| {
                        matches!(
                            *reason,
                            "coreUnavailable"
                                | "ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED"
                                | "configurationCheckFailed"
                                | "configWriteFailed"
                                | "processStartFailed"
                                | "statusSubscriptionFailed"
                                | "statusStreamEnded"
                                | "processExited"
                                | "authorizationTimedOut"
                                | "mainCoreChanged"
                                | "mainCoreInUse"
                                | "stateQueryFailed"
                                | "saveFailed"
                                | "configurationRefreshFailed"
                                | "tooManyLogins"
                                | "invalidAuthUrl"
                        )
                    })
                    .map(str::to_owned),
            });
        }
        self.inner
            .progress(server_id, attempt_id, phase, reason, url);
    }
}

/// 瞬态核 STATUS 流（每帧 = 全量端点快照）。生产是 `SubscribeTailscaleStatus` 的自动重连流，
/// 测试是喂脚本帧的内存桩 —— 这条抽象是「无真 gRPC 单测整条登录编排」的关键。
#[async_trait]
pub trait LoginStatusStream: Send {
    /// 取下一帧。`None` = 流终止（生产上 [`polaris_singbox_grpc::ReconnectingStream`] 断开即重连，
    /// 正常不返 `None`；返了就是内部终止，由调用方按「没有更多帧」处理）。
    async fn recv(&mut self) -> Option<daemon::TailscaleStatusUpdate>;
}

/// STATUS 流订阅抽象：按瞬态核自己的 api 端口 + secret 建流。
#[async_trait]
pub trait LoginStatusSubscriber: Send + Sync {
    /// 订阅 `127.0.0.1:<port>` 的 `SubscribeTailscaleStatus`。`secret` 空串 → 免认证。
    async fn subscribe(
        &self,
        port: u16,
        secret: &str,
    ) -> Result<Box<dyn LoginStatusStream>, String>;
}

// ── 生产实现 ────────────────────────────────────────────────────────────────────────────────

/// `tokio::process::Child` 包装的生产子进程句柄。
///
/// ## 为什么有 [`Drop`] 守卫（不是洁癖）
///
/// `tokio::process::Child` 默认 `kill_on_drop == false`：句柄被丢弃时 tokio 只把它推进 orphan 队列
/// **等待收割**，子进程照常活着。而瞬态核（登录核 / 测速临时核）的 kill 全靠调用方显式
/// [`terminate`](LoginCoreChild::terminate) —— 只要 future 在 `spawn` 与 `terminate` 之间被丢弃或
/// panic 展开，就留下一个持续持有回环端口（测速临时核是 N 个）+ WG/WARP peer 会话的**孤儿
/// sing-box**，且用户完全看不见。兜底 sweep 只在下次起主核时跑，Windows 更是恒 no-op
/// （`core-supervisor/src/stale_core.rs` 的 `scan_running_cores` 在非 Linux/macOS 返空）。
///
/// 用 Drop 守卫而非 `Command::kill_on_drop(true)`：后者必须设在 **spawn 之前**的 `Command` 上，
/// 而 spawn 收口在 `core-supervisor` 的 `TokioSpawner`（主核与瞬态核共用，主核**不能**跟着 app
/// 的任意 future 生死）。守卫挂在瞬态核专属的这层包装上，射程正好。
///
/// `start_kill` 只发信号不阻塞（Drop 不能 await）；已退出/已收割的 child 返 Err，无害吞掉。
pub struct TokioLoginCoreChild {
    child: tokio::process::Child,
}

impl Drop for TokioLoginCoreChild {
    fn drop(&mut self) {
        // 正常路径（`terminate()` 已收割）到这里是 no-op；异常路径（future 被丢弃 / panic）靠这一发。
        let _ = self.child.start_kill();
    }
}

#[async_trait]
impl LoginCoreChild for TokioLoginCoreChild {
    fn pid(&self) -> Option<u32> {
        self.child.id()
    }
    async fn wait(&mut self) {
        let _ = self.child.wait().await;
    }
    async fn terminate(&mut self) {
        // 1:1 镜像 ProxyRuntime::kill_core 的收割纪律：SIGTERM→宽限→SIGKILL，退出后取消挂起升级
        // （防 timer 泄漏 + pid 复用误杀），并 `wait()` 收割防僵尸。
        let pid = self.child.id().unwrap_or(0);
        if pid == 0 {
            // 已退出且被收割 → 仅 reap 残句柄。
            let _ = self.child.wait().await;
            return;
        }
        let escalation = ProcessKiller::escalate_async(
            move |sig| send_signal(pid, sig),
            move || pid_alive(pid),
            LOGIN_STOP_GRACE,
        )
        .await;
        let _ = self.child.wait().await;
        escalation.wait().await;
    }
}

/// 生产 spawner：经 [`TokioSpawner`] 起真 sing-box，再适配为 [`LoginCoreChild`]。
pub struct TokioLoginCoreSpawner;

#[async_trait]
impl LoginCoreSpawner for TokioLoginCoreSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        // 装箱适配：把 `SpawnedChild` 换成 `LoginCoreChild`。请求原样透传 —— 排空回调在
        // `TokioSpawner::spawn` 内部就被调用完了，到这里 child 已经不带管道。
        let spawned = TokioSpawner::new().spawn(req)?;
        Ok(Box::new(TokioLoginCoreChild {
            child: spawned.child,
        }))
    }
}

#[cfg(target_os = "android")]
struct AndroidLoginCoreSpawner;
#[cfg(target_os = "android")]
struct AndroidLoginCoreChild {
    instance_id: String,
    closed: bool,
}

#[cfg(target_os = "android")]
#[async_trait]
impl LoginCoreSpawner for AndroidLoginCoreSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        let instance_id = req
            .config
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let failure = |message: String| SpawnError::Spawn {
            bin: PathBuf::from("android-libbox"),
            source: std::io::Error::other(message),
        };
        let config =
            std::fs::read_to_string(&req.config).map_err(|error| failure(error.to_string()))?;
        let mut child = AndroidLoginCoreChild {
            instance_id,
            closed: false,
        };
        crate::runtime::proxy::android_bridge::start_transient_login(&child.instance_id, &config)
            .await
            .map_err(|error| match error {
                crate::runtime::proxy::android_bridge::LoginStartError::Failed(message) => {
                    failure(message)
                }
                crate::runtime::proxy::android_bridge::LoginStartError::CapacityClosed(error) => {
                    error.spawn_error()
                }
            })?;
        // Android's native factory has copied the config into its own service. No snapshot is written.
        child.closed = false;
        Ok(Box::new(child))
    }
}

#[cfg(target_os = "android")]
impl Drop for AndroidLoginCoreChild {
    fn drop(&mut self) {
        if !self.closed {
            let id = self.instance_id.clone();
            tokio::spawn(async move {
                let _ = crate::runtime::proxy::android_bridge::close_transient_login(&id).await;
            });
        }
    }
}

#[cfg(target_os = "android")]
#[async_trait]
impl LoginCoreChild for AndroidLoginCoreChild {
    fn pid(&self) -> Option<u32> {
        None
    } // A libbox instance has no independent process PID.
    async fn wait(&mut self) {
        loop {
            if matches!(
                crate::runtime::proxy::android_bridge::transient_login_running(&self.instance_id)
                    .await,
                Ok(false)
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    async fn after_exit(&mut self) -> Result<(), String> {
        self.close_confirmed().await
    }
    async fn terminate(&mut self) {
        let _ = self.close_confirmed().await;
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        crate::runtime::proxy::android_bridge::close_transient_login(&self.instance_id).await?;
        self.closed = true;
        Ok(())
    }
}

#[cfg(target_os = "android")]
struct AndroidLoginConfigChecker;
#[cfg(target_os = "android")]
#[async_trait]
impl ConfigChecker for AndroidLoginConfigChecker {
    async fn check(&self, _binary: &Path, config_path: &Path) -> Result<(), String> {
        self.check_admitted(_binary, config_path)
            .await
            .map_err(|error| error.to_string())
    }
    async fn check_admitted(
        &self,
        _binary: &Path,
        config_path: &Path,
    ) -> Result<(), crate::runtime::proxy::android_capacity::CheckFailure> {
        use crate::runtime::proxy::android_capacity::CheckFailure;
        use polaris_core_supervisor::config_gate::ConfigCheckVerdict;
        let config = std::fs::read_to_string(config_path)
            .map_err(|error| CheckFailure::Rejected(error.to_string()))?;
        match crate::runtime::proxy::android_bridge::check_config_admitted(&config)
            .await
            .map_err(CheckFailure::CapacityClosed)?
        {
            ConfigCheckVerdict::Accepted => Ok(()),
            _ => Err(CheckFailure::Rejected(
                "Android 登录配置校验失败或不可用".to_owned(),
            )),
        }
    }
}

/// 生产 checker：真跑 `sing-box check -c <file>` 并按退出码判定。
///
/// 子进程本身由 [`run_check_raw`] 起 —— 全仓唯一的 `sing-box check` 实现（起核闸门、本处、
/// 「测试内核兼容性」按钮三处共用）。本处此前自己写了一遍，写漏的是**超时**：check 若挂住
/// （慢盘、杀软扫描、核二进制半损坏），`output()` 会一直等下去，而它挂在登录核与测速临时核的
/// 起核前置位上，于是整条登录/测速流程跟着永久挂起，用户侧表现为「点了没反应」。
///
/// 超时值取 [`CONFIG_CHECK_TIMEOUT`]（5s）而不是 `commands/proxy.rs::PROBE_CHECK_TIMEOUT`（8s）：
/// 这里与起核闸门同属**起核关键路径**（用户在等一条连接建立，不是在等一个按钮回话），两者的
/// 容忍度应当一致；那份常量的文档也正是按这条分界线写的。
pub struct SingBoxConfigChecker;

#[async_trait]
impl ConfigChecker for SingBoxConfigChecker {
    async fn check(&self, binary: &Path, config_path: &Path) -> Result<(), String> {
        match run_check_raw(binary, config_path, CONFIG_CHECK_TIMEOUT).await {
            RawCheck::Done { success: true, .. } => Ok(()),
            RawCheck::Done { stderr, stdout, .. } => {
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                Err(format!("sing-box check 判定登录配置无效: {detail}"))
            }
            RawCheck::SpawnFailed(e) => Err(format!("sing-box check 启动失败: {e}")),
            // 折叠前不存在的一支：此前无超时 ⇒ 这条路径的表现是永不返回。
            RawCheck::TimedOut { after_secs } => {
                Err(format!("sing-box check 超时（>{after_secs}s）"))
            }
        }
    }
}

/// 生产 STATUS 订阅器：连瞬态核自己的管理 API（h2c，127.0.0.1）建 `SubscribeTailscaleStatus` 自动重连流。
///
/// 与主核那条 relay（`runtime/proxy::spawn_tailscale_status_relay`）**互不相干**：各自端口、各自 secret、
/// 各自 stream 句柄，帧也不进主核的 `MeshRuntime::ts_status` 缓存（那份缓存的 `connected` 语义是
/// 「主核在跑」，写进瞬态核的帧会让它说谎）。
pub struct GrpcLoginStatusSubscriber;

#[async_trait]
impl LoginStatusSubscriber for GrpcLoginStatusSubscriber {
    async fn subscribe(
        &self,
        port: u16,
        secret: &str,
    ) -> Result<Box<dyn LoginStatusStream>, String> {
        // `connect` 建的是 **lazy** channel（`h2c::connect_h2c`）——此处不发生 I/O，故核尚未 bind 完
        // 也不会失败；真正的建流与重试在 `ReconnectingStream` 里。
        let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret)
            .await
            .map_err(|e| format!("连瞬态登录核管理 API 失败（port={port}）: {e}"))?;
        Ok(Box::new(GrpcLoginStatusStream {
            stream: client.subscribe_tailscale_status(ReconnectConfig::default()),
        }))
    }
}

/// [`GrpcLoginStatusSubscriber`] 建出的流。`ReconnectingStream` 自持 target/secret，与建它的
/// client 无生命周期纠缠，故此处只留流本体。
struct GrpcLoginStatusStream {
    stream: polaris_singbox_grpc::ReconnectingStream<daemon::TailscaleStatusUpdate>,
}

#[async_trait]
impl LoginStatusStream for GrpcLoginStatusStream {
    async fn recv(&mut self) -> Option<daemon::TailscaleStatusUpdate> {
        self.stream.recv().await
    }
}

/// 生产 emitter：经 [`AppHandle`] 广播 `event:tailscaleAuthUrl`。
///
/// payload 与前端 `onTailscaleAuth` 契约同形：`{ serverId, nodeName, url, transient }`
/// （`ui/src/ipc/api-client.ts:155`）。
pub struct AppHandleEmitter {
    /// 广播用的 Tauri 应用句柄。
    pub app: AppHandle,
}

impl AuthUrlEmitter for AppHandleEmitter {
    fn progress(
        &self,
        server_id: &str,
        attempt_id: &str,
        phase: &str,
        reason: Option<&str>,
        url: Option<&str>,
    ) {
        broadcast(
            &self.app,
            crate::events::channel::EVENT_TAILSCALE_LOGIN_PROGRESS,
            json!({
                "serverId": server_id, "attemptId": attempt_id, "phase": phase, "reason": reason, "url": url,
            }),
        );
        if phase == "awaitingAuth" {
            broadcast(
                &self.app,
                crate::events::channel::EVENT_TAILSCALE_AUTH_URL,
                json!({"serverId": server_id, "attemptId": attempt_id, "nodeName": "", "url": url, "transient": true}),
            );
        }
    }
    fn emit_auth_url(&self, _server_id: &str, _node_name: &str, _url: &str) {
        // Transient URLs travel only in the attempt-scoped progress event.
    }
}

// ── 注册表 + 编排 ────────────────────────────────────────────────────────────────────────────

/// 注册表条目：一个在飞瞬态登录核的控制句柄。child 本体由 supervisor 任务独占持有，本条目只留信号通道。
struct LoginEntry {
    /// 单调 epoch：区分同一 serverId 的不同代次登录（kill-on-relogin 后旧 supervisor 不得误删新表项）。
    epoch: u64,
    attempt_id: String,
    /// 该代次登录核的 OS pid（假 child 返占位值 ⇒ `None` 只在拿不到 pid 时出现）。
    ///
    /// **它不是日志字段**：`ProxyRuntime::cleanup_stale_cores` 的排除表经
    /// [`inflight_login_pids`](LoginCoreRegistry::inflight_login_pids) 读它 —— 瞬态登录核的 argv 与
    /// 主核同二进制 + `run`，不排除就会在起核时被当成上次会话遗留的孤儿杀掉（见模块头
    /// 「与 `cleanup_stale_cores` 的关系」）。
    pid: Option<u32>,
    /// 通知 supervisor kill+reap（cancel / kill-on-relogin 用）。
    cancel_tx: mpsc::UnboundedSender<()>,
    closed_rx: watch::Receiver<Option<Result<(), String>>>,
}

/// 注册表共享状态（supervisor 任务与命令层共享）。
#[derive(Default)]
struct Shared {
    /// serverId → 在飞登录核条目。
    entries: Mutex<HashMap<String, LoginEntry>>,
    /// One physical main-core attempt owns the entire peeled endpoint set.
    /// A single lock prevents a cancelled reservation from publishing half a set.
    main: Mutex<Option<MainClaim>>,
}

/// Opaque, registry-bound birth identity. Clones are only handed to the
/// physical direct Child, helper attempt, or Android request for custody.
#[derive(Clone)]
pub(crate) struct MainBirthToken {
    registry: Arc<RegistryIdentity>,
    birth: Arc<()>,
}

struct RegistryIdentity;

impl MainBirthToken {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.registry, &other.registry) && Arc::ptr_eq(&self.birth, &other.birth)
    }
}

struct MainClaim {
    token: MainBirthToken,
    directories: HashMap<String, PathBuf>,
    endpoints: HashMap<String, serde_json::Value>,
}

/// Before an external start can occur, cancellation may roll back the exact
/// reservation while the real TS gate remains held by the caller. Once armed,
/// Drop deliberately retains the claim even if the waiter disappears.
pub(crate) struct MainReservation<'a, 'g> {
    registry: &'a LoginCoreRegistry,
    gate: &'g tokio::sync::MutexGuard<'a, ()>,
    token: MainBirthToken,
    registered: bool,
    external_possible: bool,
}

impl MainReservation<'_, '_> {
    pub(crate) fn claim_token(&self) -> Option<MainBirthToken> {
        self.registered.then(|| self.token.clone())
    }

    pub(crate) fn arm_external_start(&mut self) {
        self.external_possible = true;
    }

    /// A synchronous spawn error or strict same-attempt helper Stop confirms
    /// no external writer remains. IPC errors and cancelled waiters cannot.
    pub(crate) fn confirmed_no_external_writer(&mut self) {
        self.external_possible = false;
    }
}

impl Drop for MainReservation<'_, '_> {
    fn drop(&mut self) {
        if self.registered && !self.external_possible {
            if let Err(error) = self
                .registry
                .release_main_states_if_token(&self.token, self.gate)
            {
                log::error!("Tailscale main reservation rollback failed: {error}");
            }
        }
    }
}

impl Shared {
    fn guard(&self) -> MutexGuard<'_, HashMap<String, LoginEntry>> {
        // 锁只在 insert/remove 的极短临界区持有（绝不跨 await），中毒极不可能；中毒仍恢复内层，不 panic。
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn take(&self, id: &str) -> Option<LoginEntry> {
        self.guard().remove(id)
    }

    fn insert(&self, id: String, entry: LoginEntry) {
        self.guard().insert(id, entry);
    }

    fn can_start(&self, id: &str) -> bool {
        let entries = self.guard();
        entries.contains_key(id) || entries.len() < MAX_ACTIVE_LOGIN_CORES
    }

    /// epoch 守卫下注销：仅当表项仍是本 supervisor 的代次时移除（防 kill-on-relogin 后误删新代次）。
    fn remove_if_epoch(&self, id: &str, epoch: u64) {
        let mut g = self.guard();
        if g.get(id).is_some_and(|e| e.epoch == epoch) {
            g.remove(id);
        }
    }

    /// 全部在飞条目的 pid 快照（丢 `None`）。临界区只做一次拷贝，绝不跨 await。
    fn pids(&self) -> Vec<u32> {
        self.guard().values().filter_map(|e| e.pid).collect()
    }

    fn contains(&self, id: &str) -> bool {
        self.guard().contains_key(id)
    }
}

/// [`start_attempt`](LoginCoreRegistry::start_attempt) 的结果；Started 表示仍待授权。
pub enum StartLoginOutcome {
    AndroidCapacityClosed(crate::runtime::proxy::android_capacity::CapacityClosed),
    /// 已起瞬态登录核（登录 URL 稍后经事件到达，非「已登录」）。
    Started,
    /// 双写守卫命中：该 TS endpoint 已在运行主核里，无需瞬态核（前端 `reason: 'inMainCore'`）。
    InMainCore,
    InMainCorePending,
    /// 起核前失败（resolve / 写配置 / check / spawn）。返 error，未留表项、未起核。
    Failed(String),
    /// The request was cancelled before its process was started.
    Cancelled,
}

/// Facts from this process's Tailscale login registry only. `Vacant` says nothing about an OS,
/// Android, or helper-owned process and must not by itself authorize identity retirement.
#[allow(dead_code, reason = "reserved for cross-registry owner reconciliation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TsRegistryOwnerState {
    Busy,
    Unknown,
    Vacant,
}

/// 瞬态登录核生命周期注册表。持有注入的 spawner/checker/binary-resolver（生产真实现，测试 mock）。
///
/// 支撑：kill-on-relogin、超时自动杀、取消、自然退出 reap。与 `ProxyRuntime` 的常驻代理核隔离。
pub struct LoginCoreRegistry {
    shared: Arc<Shared>,
    identity: Arc<RegistryIdentity>,
    spawner: Arc<dyn LoginCoreSpawner>,
    checker: Arc<dyn ConfigChecker>,
    subscriber: Arc<dyn LoginStatusSubscriber>,
    resolve_binary: BinaryResolver,
    timeout: Duration,
    epoch: AtomicU64,
    /// 串行化「检查旧代 → 起核 → 注册」事务，防同一 server 的并发 IPC 各自都看见空表，
    /// 后写者覆盖前写者的 cancel sender，留下无法再取消的孤儿核。
    start_gate: tokio::sync::Mutex<()>,
    attempts: Attempts,
}

impl LoginCoreRegistry {
    /// 生产装配：真 spawner + 真 `sing-box check` + 真 gRPC STATUS 订阅 + 真核解析 + 默认超时。
    #[must_use]
    pub fn production() -> Self {
        #[cfg(target_os = "android")]
        return Self::with_deps(
            Arc::new(AndroidLoginCoreSpawner),
            Arc::new(AndroidLoginConfigChecker),
            Arc::new(GrpcLoginStatusSubscriber),
            // Android uses the in-process libbox factory, never an executable lookup.
            Arc::new(|| Ok(PathBuf::from("android-libbox"))),
            DEFAULT_LOGIN_TIMEOUT,
        );
        #[cfg(not(target_os = "android"))]
        Self::with_deps(
            Arc::new(TokioLoginCoreSpawner),
            Arc::new(SingBoxConfigChecker),
            Arc::new(GrpcLoginStatusSubscriber),
            Arc::new(resolve_core_binary),
            DEFAULT_LOGIN_TIMEOUT,
        )
    }

    /// 注入装配（测试用 mock，或自定义超时）。
    #[must_use]
    pub fn with_deps(
        spawner: Arc<dyn LoginCoreSpawner>,
        checker: Arc<dyn ConfigChecker>,
        subscriber: Arc<dyn LoginStatusSubscriber>,
        resolve_binary: BinaryResolver,
        timeout: Duration,
    ) -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            identity: Arc::new(RegistryIdentity),
            spawner,
            checker,
            subscriber,
            resolve_binary,
            timeout,
            epoch: AtomicU64::new(1),
            start_gate: tokio::sync::Mutex::new(()),
            attempts: Attempts::default(),
        }
    }

    /// **此刻在飞**的瞬态登录核 pid 快照 —— `ProxyRuntime::cleanup_stale_cores` 的排除表来源。
    ///
    /// # 为什么必须有这条
    ///
    /// 瞬态登录核走的是同一个 [`resolve_core_binary`] + [`SpawnRequest`]，argv 与主核逐字同形
    /// （`<同一核二进制> run -c <cfg> --disable-color`）⇒ `is_our_core` 必然命中 ⇒ 在候选集里它与
    /// 「上次会话遗留的孤儿」不可区分。而清扫跑在**每一次**起核上，于是「点了 Tailscale 登录、
    /// 等着扫码时又去开 TUN」这条日常序列会把在飞登录核 SIGTERM 掐死：登录 URL 作废、
    /// 用户只看到「登录没反应」，且起核腿还要白等两段宽限。
    ///
    /// Spawn immediately registers its PID before STATUS subscription awaits. Cancellation retains
    /// the entry until terminate + reap. Primary cleanup runs under the same state gate.
    #[must_use]
    pub fn inflight_login_pids(&self) -> Vec<u32> {
        self.shared.pids()
    }

    /// **测试专用**：直接登记一条在飞条目（不起进程、不发任何信号），供**跨模块**的行为门
    /// （`ProxyRuntime::sweep_exclusions` 的孤儿清扫排除表）构造「登录核正在飞」这个状态。
    ///
    /// 放在这里而不是在 proxy 侧另造一张表：那样门测的就是测试自己写的表，而不是生产真的会读的
    /// 那一张。本函数只做 `insert`，读侧走的仍是生产的 [`inflight_login_pids`](Self::inflight_login_pids)。
    /// pid 字段本身「由 `child.pid()` 填」这一半由本模块的行为门（走生产 `start_attempt`）单独钉。
    #[cfg(test)]
    pub(crate) fn register_inflight_for_test(&self, server_id: &str, pid: u32) {
        let (cancel_tx, _cancel_rx) = mpsc::unbounded_channel();
        let (_, closed_rx) = watch::channel(None);
        self.shared.insert(
            server_id.to_owned(),
            LoginEntry {
                epoch: 0,
                attempt_id: "test-inflight".into(),
                pid: Some(pid),
                cancel_tx,
                closed_rx,
            },
        );
    }

    /// **测试专用**：注销一条在飞条目（不发信号）。用于「出表之后同一 pid 必须重新可被清扫」这一格。
    #[cfg(test)]
    pub(crate) fn deregister_inflight_for_test(&self, server_id: &str) {
        self.shared.take(server_id);
    }

    /// Cancel only this login and wait for an actual close receipt. Keep its claim on failure.
    pub async fn cancel_login(&self, server_id: &str) -> Result<bool, String> {
        self.cancel_matching_login(server_id, None).await
    }

    async fn cancel_matching_login(
        &self,
        server_id: &str,
        attempt_id: Option<&str>,
    ) -> Result<bool, String> {
        let Some((cancel_tx, mut closed)) = self
            .shared
            .guard()
            .get(server_id)
            .filter(|entry| attempt_id.is_none_or(|id| entry.attempt_id == id))
            .map(|entry| (entry.cancel_tx.clone(), entry.closed_rx.clone()))
        else {
            return Ok(false);
        };
        if matches!(&*closed.borrow_and_update(), Some(Ok(()))) {
            return Ok(true);
        }
        signal_and_wait_close(cancel_tx, closed).await
    }

    /// A failed native close keeps the state-directory claim, preventing a successor from
    /// opening the same Tailscale state.
    async fn cancel_and_wait(&self, server_id: &str) -> Result<(), String> {
        self.cancel_login(server_id).await.map(|_| ())
    }

    pub async fn prepare(&self, server_id: &str, attempt_id: &str) -> Result<(), String> {
        let _gate = self.state_gate().await;
        self.attempts.prepare(server_id, attempt_id).map(|_| ())
    }

    /// Read-only, attempt-scoped recovery of a missed renderer event. Native running status and
    /// state-directory existence cannot establish a successful authorization for this request.
    pub fn login_progress(
        &self,
        server_id: &str,
        attempt_id: &str,
    ) -> Option<LoginProgressReceipt> {
        self.attempts.progress(server_id, attempt_id)
    }

    /// Fence every request already prepared for this state directory. The caller keeps the
    /// state gate through its identity commit; a later prepare cannot register until then.
    /// A failed native close leaves its registry entry in place and fails this retirement.
    pub async fn retire_attempts_under_state_gate(
        &self,
        server_id: &str,
        _gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        let cancelled = self.attempts.retire_node_except(server_id, None)?;
        self.cancel_and_wait(server_id).await?;
        for attempt in cancelled {
            attempt.finished().await;
        }
        if self.shared.contains(server_id) || self.attempts.owns_state(server_id) {
            return Err("Tailscale login owner has not been reaped".into());
        }
        Ok(())
    }

    pub async fn cancel_attempt(&self, server_id: &str, attempt_id: &str) -> Result<(), String> {
        let attempt = self.attempts.cancel(server_id, attempt_id)?;
        self.cancel_matching_login(server_id, Some(attempt_id))
            .await?;
        if let Some(attempt) = attempt {
            attempt.finished().await;
        }
        Ok(())
    }

    /// Lock order: proxy lifecycle -> this gate. Login never waits for proxy lifecycle.
    pub async fn state_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.start_gate.lock().await
    }

    /// A registry-local fact while the caller continuously holds this exact registry's gate.
    /// The gate prevents new prepare/reserve/spawn admissions. A supervisor can remove an entry
    /// outside the gate after confirmed close or after_exit; that only clears local ownership.
    /// Each std mutex is read and released separately.
    #[allow(dead_code, reason = "reserved for cross-registry owner reconciliation")]
    pub(crate) fn owner_state_under_gate(
        &self,
        server_id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> TsRegistryOwnerState {
        if server_id.is_empty()
            || !std::ptr::eq(tokio::sync::MutexGuard::mutex(gate), &self.start_gate)
        {
            return TsRegistryOwnerState::Unknown;
        }
        let main_reserved = match self.shared.main.lock() {
            Ok(main) => main
                .as_ref()
                .is_some_and(|claim| claim.directories.contains_key(server_id)),
            Err(_) => return TsRegistryOwnerState::Unknown,
        };
        let attempt_busy = match self.attempts.local_owner_in_use(server_id) {
            Ok(busy) => busy,
            Err(()) => return TsRegistryOwnerState::Unknown,
        };
        let transient = match self.shared.entries.lock() {
            Ok(entries) => entries.get(server_id).map(|entry| {
                if entry.cancel_tx.is_closed() || entry.closed_rx.has_changed().is_err() {
                    TsRegistryOwnerState::Unknown
                } else {
                    match &*entry.closed_rx.borrow() {
                        None => TsRegistryOwnerState::Busy,
                        Some(_) => TsRegistryOwnerState::Unknown,
                    }
                }
            }),
            Err(_) => return TsRegistryOwnerState::Unknown,
        };
        match transient {
            Some(TsRegistryOwnerState::Unknown) => TsRegistryOwnerState::Unknown,
            Some(TsRegistryOwnerState::Busy) => TsRegistryOwnerState::Busy,
            _ if main_reserved || attempt_busy => TsRegistryOwnerState::Busy,
            _ => TsRegistryOwnerState::Vacant,
        }
    }

    /// The deletion callback runs only when neither the main core nor a login process owns state.
    pub async fn logout<F>(
        &self,
        server_id: &str,
        main_alive: &(dyn Fn() -> bool + Send + Sync),
        keep_attempt: Option<&str>,
        delete: F,
    ) -> std::io::Result<bool>
    where
        F: FnOnce(&tokio::sync::MutexGuard<'_, ()>) -> std::io::Result<()> + Send,
    {
        let _gate = self.state_gate().await;
        if self.main_claims(server_id) || self.main_owns(server_id, main_alive()) {
            return Ok(false);
        }
        if keep_attempt.is_some_and(|id| !self.attempts.can_preserve(server_id, id)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Only a prepared login request may be preserved",
            ));
        }
        if keep_attempt.is_none() {
            self.retire_attempts_under_state_gate(server_id, &_gate)
                .await
                .map_err(std::io::Error::other)?;
        } else {
            let cancelled = self
                .attempts
                .retire_node_except(server_id, keep_attempt)
                .map_err(std::io::Error::other)?;
            self.cancel_and_wait(server_id)
                .await
                .map_err(std::io::Error::other)?;
            for attempt in cancelled {
                attempt.finished().await;
            }
        }
        delete(&_gate)?;
        Ok(true)
    }

    /// Called while holding state_gate; includes check/subscription and process reap windows.
    pub fn state_in_use(&self, server_id: &str, main_alive: bool) -> bool {
        self.main_claims(server_id)
            || self.main_owns(server_id, main_alive)
            || self.shared.contains(server_id)
            || self.attempts.owns_state(server_id)
    }

    fn main_claims(&self, server_id: &str) -> bool {
        // A poisoned registry is unknown ownership, not permission to delete.
        self.shared.main.lock().map_or(true, |main| {
            main.as_ref()
                .is_some_and(|claim| claim.directories.contains_key(server_id))
        })
    }

    pub fn main_owns(&self, server_id: &str, main_alive: bool) -> bool {
        main_alive
            && self
                .shared
                .main
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .is_some_and(|claim| claim.directories.contains_key(server_id))
    }

    pub(crate) fn mint_main_birth(&self) -> MainBirthToken {
        MainBirthToken {
            registry: Arc::clone(&self.identity),
            birth: Arc::new(()),
        }
    }

    fn valid_main_gate(&self, gate: &tokio::sync::MutexGuard<'_, ()>) -> bool {
        std::ptr::eq(tokio::sync::MutexGuard::mutex(gate), &self.start_gate)
    }

    /// Reserve the entire final peeled TS set under this registry's real gate.
    /// No subset may silently survive an invalid state_directory or duplicate ID.
    pub(crate) async fn reserve_main_states<'a, 'g>(
        &'a self,
        generated: &serde_json::Value,
        root: &Path,
        gate: &'g tokio::sync::MutexGuard<'a, ()>,
        token: MainBirthToken,
    ) -> Result<MainReservation<'a, 'g>, String> {
        if !self.valid_main_gate(gate) || !Arc::ptr_eq(&token.registry, &self.identity) {
            return Err("Tailscale main reservation has wrong registry or gate".into());
        }
        let state_root = root.join("tailscale");
        let mut directories = HashMap::new();
        let mut endpoints = HashMap::new();
        for ep in generated
            .get("endpoints")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|ep| ep.get("type").and_then(serde_json::Value::as_str) == Some("tailscale"))
        {
            let dir = ep
                .get("state_directory")
                .and_then(serde_json::Value::as_str)
                .ok_or("Tailscale endpoint lacks a state_directory")?;
            let path = Path::new(dir);
            if path.parent() != Some(state_root.as_path()) {
                return Err("Tailscale endpoint state_directory is outside its state root".into());
            }
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty() && *name != "." && *name != "..")
                .ok_or("Tailscale endpoint has an invalid state_directory ID")?
                .to_owned();
            if directories.insert(id.clone(), path.to_path_buf()).is_some() {
                return Err("Tailscale endpoint state_directory is duplicated".into());
            }
            let relevant = ["tag", "auth_key", "control_url", "hostname", "ephemeral"]
                .into_iter()
                .filter_map(|key| ep.get(key).map(|value| (key.to_owned(), value.clone())))
                .collect::<serde_json::Map<_, _>>();
            endpoints.insert(id, serde_json::Value::Object(relevant));
        }
        if self
            .shared
            .main
            .lock()
            .map_err(|_| "Tailscale main registry lock poisoned")?
            .is_some()
        {
            return Err("Tailscale main reservation already owns a physical attempt".into());
        }
        if directories.is_empty() {
            // A valid config with no TS endpoints has no TS state owner.
            // Keep ordinary no-TS retry/cancellation behavior unchanged.
            return Ok(MainReservation {
                registry: self,
                gate,
                token,
                registered: false,
                external_possible: false,
            });
        }
        for id in directories.keys() {
            self.cancel_and_wait(id).await?;
        }
        let mut main = self
            .shared
            .main
            .lock()
            .map_err(|_| "Tailscale main registry lock poisoned")?;
        if main.is_some() {
            return Err("Tailscale main reservation changed during transient close".into());
        }
        *main = Some(MainClaim {
            token: token.clone(),
            directories,
            endpoints,
        });
        Ok(MainReservation {
            registry: self,
            gate,
            token,
            registered: true,
            external_possible: false,
        })
    }

    /// Registry-local compare-and-remove only. This does not prove that an OS,
    /// helper, or Android core stopped; callers need their existing stop ACK.
    pub(crate) fn release_main_states_if_token(
        &self,
        token: &MainBirthToken,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<bool, String> {
        if !self.valid_main_gate(gate) || !Arc::ptr_eq(&token.registry, &self.identity) {
            return Err("Tailscale main release has wrong registry or gate".into());
        }
        let mut main = self
            .shared
            .main
            .lock()
            .map_err(|_| "Tailscale main registry lock poisoned")?;
        if main.as_ref().is_some_and(|claim| claim.token.same(token)) {
            *main = None;
            return Ok(true);
        }
        Ok(false)
    }

    #[cfg(test)]
    pub(crate) fn poison_main_claim_lock_for_test(&self) {
        let shared = Arc::clone(&self.shared);
        let _ = std::thread::spawn(move || {
            let _guard = shared.main.lock().unwrap();
            panic!("poison main claim registry for test");
        })
        .join();
    }

    fn main_matches_request(&self, server: &ServerConfig, mode: LoginMode) -> bool {
        let endpoints = self
            .shared
            .main
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(endpoint) = endpoints
            .as_ref()
            .and_then(|claim| claim.endpoints.get(&server.id))
        else {
            return false;
        };
        let settings = server.tailscale_settings.as_ref();
        let normalized = |v: Option<&str>| {
            v.map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let key = if mode == LoginMode::Browser {
            None
        } else {
            settings.and_then(|ts| ts.auth_key.as_deref())
        };
        [
            ("auth_key", key),
            (
                "control_url",
                settings.and_then(|ts| ts.control_url.as_deref()),
            ),
            ("hostname", settings.and_then(|ts| ts.hostname.as_deref())),
        ]
        .iter()
        .all(|(name, value)| {
            normalized(*value)
                == normalized(endpoint.get(*name).and_then(serde_json::Value::as_str))
        }) && settings.is_some_and(|ts| ts.ephemeral == Some(true))
            == (endpoint
                .get("ephemeral")
                .and_then(serde_json::Value::as_bool)
                == Some(true))
    }

    #[cfg(test)]
    pub async fn start_login(
        &self,
        server: &ServerConfig,
        user_data: &Path,
        is_running: bool,
        running_config: Option<&UserConfig>,
        primary_api_port: u16,
        emitter: Arc<dyn AuthUrlEmitter>,
    ) -> StartLoginOutcome {
        if polaris_mesh::tailscale_login::tailscale_endpoint_in_running_core(
            &server.id,
            is_running,
            running_config,
        ) {
            return StartLoginOutcome::InMainCore;
        }
        let request = LoginRequest {
            attempt_id: format!("test-{}", self.epoch.fetch_add(1, Ordering::SeqCst)),
            mode: LoginMode::Browser,
        };
        self.prepare(&server.id, &request.attempt_id).await.unwrap();
        self.start_attempt(
            server,
            user_data,
            request,
            &|| MainLoginSnapshot {
                api_port: primary_api_port,
                http_port: running_config.and_then(|c| c.http_port),
                mixed_port: running_config.and_then(|c| c.mixed_port),
                ..Default::default()
            },
            emitter,
        )
        .await
    }

    /// Start a prepared request under the shared state gate. Main ownership and port exclusions
    /// come from its actual startup snapshot. A spawned child is registered before subscription;
    /// request cancellation and terminal events wait for reap. Started means authorization pending.
    #[cfg(test)]
    pub async fn start_attempt(
        &self,
        server: &ServerConfig,
        user_data: &Path,
        request: LoginRequest,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
    ) -> StartLoginOutcome {
        self.start_attempt_with_saved(
            server,
            user_data,
            request,
            &|| Ok(server.clone()),
            main_core,
            emitter,
        )
        .await
    }

    /// The saved server is loaded only after acquiring the state gate. The request's TS identity
    /// must still name that saved server; a stale renderer request cannot start an old state.
    pub async fn start_attempt_with_saved(
        &self,
        requested: &ServerConfig,
        user_data: &Path,
        request: LoginRequest,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
    ) -> StartLoginOutcome {
        let attempt = match self.attempts.get(&requested.id, &request.attempt_id) {
            Ok(attempt) => attempt,
            Err(reason) => return StartLoginOutcome::Failed(reason),
        };
        if attempt.claimed.swap(true, Ordering::SeqCst) {
            return StartLoginOutcome::Failed("attemptAlreadyUsed".into());
        }
        let mut request_guard = AttemptGuard(attempt.clone(), false);
        let emitter: Arc<dyn AuthUrlEmitter> = Arc::new(AttemptReceiptEmitter {
            inner: emitter,
            attempt: attempt.clone(),
            attempt_id: request.attempt_id.clone(),
        });
        emitter.progress(&requested.id, &request.attempt_id, "starting", None, None);
        let outcome = self
            .launch_attempt(
                requested,
                user_data,
                &request,
                &attempt,
                saved_server,
                main_core,
                emitter.clone(),
            )
            .await;
        if !attempt.is_finished() {
            match &outcome {
                StartLoginOutcome::Started => {}
                StartLoginOutcome::InMainCore => {
                    emitter.progress(&requested.id, &request.attempt_id, "mainCore", None, None);
                    attempt.finish();
                }
                StartLoginOutcome::InMainCorePending => {
                    emitter.progress(
                        &requested.id,
                        &request.attempt_id,
                        "mainCore",
                        Some("configurationPending"),
                        None,
                    );
                    attempt.finish();
                }
                StartLoginOutcome::Cancelled => {
                    emitter.progress(&requested.id, &request.attempt_id, "cancelled", None, None);
                    attempt.finish();
                }
                StartLoginOutcome::AndroidCapacityClosed(_) => {
                    emitter.progress(
                        &requested.id,
                        &request.attempt_id,
                        "failed",
                        Some(crate::runtime::proxy::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED),
                        None,
                    );
                    attempt.finish();
                }
                StartLoginOutcome::Failed(reason) => {
                    emitter.progress(
                        &requested.id,
                        &request.attempt_id,
                        "failed",
                        Some(reason),
                        None,
                    );
                    attempt.finish();
                }
            }
        }
        request_guard.1 = true;
        outcome
    }

    /// A steady Running core need not send another global frame. Subscribe once for a fresh
    /// initial snapshot; this query never starts/stops the primary core or keeps its stream alive.
    async fn confirm_main_request(
        &self,
        server: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        main: &MainLoginSnapshot,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
    ) -> StartLoginOutcome {
        let tag = self
            .shared
            .main
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|claim| claim.endpoints.get(&server.id))
            .and_then(|ep| ep.get("tag"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let Some(tag) = tag.filter(|_| main.api_port != 0) else {
            return StartLoginOutcome::Failed("statusSubscriptionFailed".into());
        };
        emitter.progress(
            &server.id,
            &request.attempt_id,
            "mainCore",
            Some("confirmingMainCore"),
            None,
        );
        let query = async {
            let mut stream = self
                .subscriber
                .subscribe(main.api_port, &main.api_secret)
                .await
                .map_err(|_| "statusSubscriptionFailed")?;
            let frame = stream.recv().await.ok_or("statusStreamEnded")?;
            let current = main_core();
            if !current.alive
                || current.generation != main.generation
                || current.api_port != main.api_port
            {
                return Err("mainCoreChanged");
            }
            let endpoint = frame
                .endpoints
                .iter()
                .find(|ep| ep.endpoint_tag == tag)
                .ok_or("statusSubscriptionFailed")?;
            let authorized = endpoint.backend_state == "Running"
                && !endpoint.self_.as_ref().is_some_and(|peer| peer.expired);
            let url = if authorized || endpoint.auth_url.is_empty() {
                None
            } else {
                Some(
                    crate::runtime::tailscale_status::validated_tailscale_auth_url(
                        &endpoint.auth_url,
                    )
                    .ok_or("invalidAuthUrl")?,
                )
            };
            Ok((authorized, url))
        };
        let result = tokio::select! {
            biased;
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            result = tokio::time::timeout(self.timeout.min(CONFIG_CHECK_TIMEOUT), query) => result,
        };
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
        }
        match result {
            Ok(Ok((authorized, url))) => {
                emitter.progress(
                    &server.id,
                    &request.attempt_id,
                    if authorized { "authorized" } else { "mainCore" },
                    None,
                    if authorized { None } else { url.as_deref() },
                );
                attempt.finish();
                StartLoginOutcome::InMainCore
            }
            Ok(Err(reason)) => StartLoginOutcome::Failed(reason.into()),
            Err(_) => {
                emitter.progress(
                    &server.id,
                    &request.attempt_id,
                    "timedOut",
                    Some("authorizationTimedOut"),
                    None,
                );
                attempt.finish();
                StartLoginOutcome::Failed("authorizationTimedOut".into())
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "login attempt inputs carry independent lifetime-bound authorities"
    )]
    async fn launch_attempt(
        &self,
        requested: &ServerConfig,
        user_data: &Path,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
    ) -> StartLoginOutcome {
        let _start_guard = tokio::select! {
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            guard = self.start_gate.lock() => guard,
        };
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
        }
        if self.attempts.registration_exhausted() {
            return StartLoginOutcome::Failed(attempts::RETIRED_LIMIT_ERROR.into());
        }
        let server = match saved_server() {
            Ok(server)
                if server.id == requested.id
                    && server.protocol
                        == polaris_config_engine::user_config::server_config::Protocol::Tailscale
                    && server.tailscale_settings == requested.tailscale_settings =>
            {
                server
            }
            _ => return StartLoginOutcome::Failed("savedTailscaleIdentityChanged".into()),
        };
        let main = main_core();
        // A delivered main Start can outlive its waiter while the alive probe
        // is still false. The persistent claim, not that probe, fences a
        // transient writer of the same state directory under this gate.
        if self.main_claims(&server.id) {
            if !main.alive {
                return StartLoginOutcome::InMainCorePending;
            }
            return if self.main_matches_request(&server, request.mode) {
                self.confirm_main_request(&server, request, attempt, &main, main_core, emitter)
                    .await
            } else {
                StartLoginOutcome::InMainCorePending
            };
        }
        if !self.shared.can_start(&server.id) {
            return StartLoginOutcome::Failed("tooManyLogins".into());
        }
        let mut server = server.clone();
        if request.mode == LoginMode::Browser {
            if let Some(ts) = server.tailscale_settings.as_mut() {
                ts.auth_key = None;
            }
        } else if server
            .tailscale_settings
            .as_ref()
            .and_then(|ts| ts.auth_key.as_deref())
            .is_none_or(|key| key.trim().is_empty())
        {
            return StartLoginOutcome::Failed("authKeyRequired".into());
        }
        // (b) 解析核二进制（复用 proxy 的解析，禁重复实现）。
        let binary = match (self.resolve_binary)() {
            Ok(b) => b,
            Err(_) => return StartLoginOutcome::Failed("coreUnavailable".into()),
        };

        // (c) 瞬态核管理 API：独立空闲端口 + 每次随机 secret。端口走既有簿记设施
        // （`resolve_tailscale_login_api_port`：bind(0) 取口、撞排除集重滚 5 次、仍撞则回落
        // control_api+2），secret 走与 clashApiSecret 同源的 CSPRNG。**secret 不是洁癖**：
        // 管理 API 虽只监听回环，但同机任意进程都能连上它读 tailnet 拓扑。
        let exclusions = PortExclusions::for_login_api(
            main.api_port,
            // UserConfig 无 controlPort 字段（`impl PortConfig for UserConfig` 恒 None）→ 走默认 9090。
            None,
            main.http_port,
            None,
            main.mixed_port,
        );
        let resolved =
            PortAllocator::new(TokioPortProvider).resolve_tailscale_login_api_port(&exclusions);
        if resolved.used_fallback {
            log::warn!(
                "瞬态登录核管理 API 端口 5 次解析均撞排除集 → 回落 {}",
                resolved.port
            );
        }
        let secret = match generate_login_api_secret() {
            Ok(s) => s,
            Err(e) => return StartLoginOutcome::Failed(e),
        };
        let api = TailscaleLoginApiService {
            port: resolved.port,
            secret,
        };

        // (d) 构造登录 config（恒带管理 api service → 恒有 STATUS 流）→ 写盘。
        //
        // 文件名带**代次**（epoch），不是只带 server id：收核后 supervisor 会删掉自己那份 config
        // （里面有 secret），而 kill-on-relogin 下新旧两代同时在场——路径若只按 server id 取，
        // 旧代 supervisor 的删除就会打在**新代**刚写好的那份上（它的 `terminate()` 有最长 5s 的
        // SIGTERM 宽限，删除随时可能晚于新核 spawn）。带代次后每份 config 只有一个主人。
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst);
        let cfg = match build_tailscale_login_config(&server, user_data, &api) {
            Ok(config) => config,
            Err(_) => return StartLoginOutcome::Failed("invalidNode".into()),
        };
        let json_cfg = login_config_to_json(&cfg);
        let config_path = user_data.join(format!(
            "tailscale-login-{}-{epoch}.json",
            sanitize_id(&server.id)
        ));
        let bytes = match serde_json::to_vec_pretty(&json_cfg) {
            Ok(b) => b,
            Err(_) => return StartLoginOutcome::Failed("configWriteFailed".into()),
        };
        // Independent authorization also works before the primary core has ever initialized its files.
        if create_login_parent(user_data)
            .and_then(|()| write_login_config_secure(&config_path, &bytes))
            .is_err()
        {
            return StartLoginOutcome::Failed("configWriteFailed".into());
        }
        // 从含 secret 的文件出现这一刻起，任何提前返回或 future 取消都必须清理；成功把路径
        // 移交 supervisor 后才解除。不能只在已知 Err 分支手写 remove，否则新增 await/return 会再漏。
        let mut config_guard = LoginConfigGuard::new(&config_path);

        // (e) sing-box check 先验配置形状（失败快退、不 spawn —— 这一段可单测）。
        tokio::select! {
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            result = self.checker.check_admitted(&binary, &config_path) => match result {
                Ok(()) => {},
                Err(crate::runtime::proxy::android_capacity::CheckFailure::CapacityClosed(error)) => return StartLoginOutcome::AndroidCapacityClosed(error),
                Err(_) => return StartLoginOutcome::Failed("configurationCheckFailed".into()),
            },
        }

        // (f) kill-on-relogin：先杀该 server 在飞的旧瞬态核（若有），再起新核。
        if let Err(error) = self.cancel_login(&server.id).await {
            return StartLoginOutcome::Failed(error);
        }
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
        }

        // (g) spawn 瞬态登录核（`run -c <cfg> --disable-color`，避免 ANSI 污染日志）。
        //
        // 两条流的去向在**请求里**一次说清：spawner 在返回之前就把读端交给这个回调，核从起来的
        // 第一毫秒起就有人读它。**纯诊断**：登录 URL 与登录成功都只认 STATUS 流（见模块头），
        // 这两条流不是任何判据的来源。target 用本腿自己的，不与主核混（见 `LOGIN_CORE_LOG_TARGET`）。
        let secrets: Vec<String> = [
            Some(api.secret.clone()),
            server
                .tailscale_settings
                .as_ref()
                .and_then(|ts| ts.auth_key.as_deref().map(str::trim).map(str::to_owned)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut req = SpawnRequest::new(
            &binary,
            &config_path,
            StdioPolicy::drain(move |stdout, stderr| {
                pipe_to_log_with_secrets(
                    stdout,
                    LOGIN_CORE_LOG_TARGET,
                    None,
                    None,
                    secrets.clone(),
                );
                pipe_to_log_with_secrets(stderr, LOGIN_CORE_LOG_TARGET, None, None, secrets);
            }),
        );
        req.extra_args = vec!["--disable-color".to_string()];
        req.working_dir = Some(user_data.to_path_buf());
        let child = match self.spawner.spawn(req).await {
            Ok(c) => c,
            Err(error) => {
                if let Some(capacity) =
                    crate::runtime::proxy::android_capacity::CapacityClosed::from_spawn(&error)
                {
                    return StartLoginOutcome::AndroidCapacityClosed(capacity);
                }
                return StartLoginOutcome::Failed("processStartFailed".into());
            }
        };

        // Register before the STATUS subscription awaits: a concurrent main-core start can
        // see this child and must wait for its confirmed close.
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (closed_tx, closed_rx) = watch::channel(None);
        let pid = child.pid();
        self.shared.insert(
            server.id.clone(),
            LoginEntry {
                epoch,
                attempt_id: request.attempt_id.clone(),
                pid,
                cancel_tx,
                closed_rx,
            },
        );
        let ctx = SuperviseCtx {
            shared: self.shared.clone(),
            attempt: attempt.clone(),
            attempt_id: request.attempt_id.clone(),
            server_id: server.id.clone(),
            node_name: server.name.clone(),
            // 瞬态核只含本节点一个 endpoint；固定 tag 与入站拒绝规则同一真值。
            // 复用主核那套解码器就得给它同一份 tag→id 映射；一并承担了「别的 tag 的帧一律丢弃」。
            tag_to_id: BTreeMap::from([(
                TAILSCALE_LOGIN_ENDPOINT_TAG.to_owned(),
                server.id.clone(),
            )]),
            config_path: config_path.clone(),
            epoch,
            deadline: tokio::time::Instant::now() + self.timeout,
            emitter,
            closed_tx,
        };
        config_guard.disarm();
        attempt.process_owned.store(true, Ordering::SeqCst);
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(subscribe_and_supervise(
            ctx,
            child,
            self.subscriber.clone(),
            api,
            cancel_rx,
            ready_tx,
        ));
        ready_rx
            .await
            .unwrap_or_else(|_| StartLoginOutcome::Failed("processExited".into()))
    }
}

async fn signal_and_wait_close(
    cancel_tx: mpsc::UnboundedSender<()>,
    mut closed: watch::Receiver<Option<Result<(), String>>>,
) -> Result<bool, String> {
    if cancel_tx.send(()).is_err() {
        return if matches!(&*closed.borrow(), Some(Ok(()))) {
            Ok(true)
        } else {
            Err("登录实例关闭监管器不可用".to_owned())
        };
    }
    if closed.changed().await.is_err() && !matches!(&*closed.borrow(), Some(Ok(()))) {
        return Err("登录实例关闭回执不可用".to_owned());
    }
    let result = closed
        .borrow()
        .clone()
        .ok_or_else(|| "登录实例关闭回执为空".to_owned())?;
    result.map(|()| true)
}

/// 瞬态核管理 API 的一次性 secret（CSPRNG 16 字节 → 32 位小写 hex）。
/// 与 `clashApiSecret` 同源生成器（[`crate::commands::config::generate_local_api_secret`]）：同一熵源、
/// 同一形状，熵源不可用 → Err（绝不产弱/空密钥而把管理面裸奔当成「降级可用」）。
fn generate_login_api_secret() -> Result<String, String> {
    crate::commands::config::generate_local_api_secret()
        .map_err(|e| format!("生成瞬态登录核管理 API secret 失败: {e}"))
}

/// supervisor 任务的入参束（避免 `too_many_arguments`）。
struct SuperviseCtx {
    shared: Arc<Shared>,
    attempt: Arc<Attempt>,
    attempt_id: String,
    server_id: String,
    node_name: String,
    /// 单条映射 `server.name → server.id`：喂给 [`decode_tailscale_status`]，顺带把「别的 tag」的
    /// 端点整段丢掉（瞬态核理论上只有一个 endpoint，但判据不该建立在「理论上」之上）。
    tag_to_id: BTreeMap<String, String>,
    /// 本次登录写盘的临时 config 路径，收核后删。
    ///
    /// 此前不删也只是留个垃圾文件；**自本批起它里面有 secret**（管理 API 的一次性 Bearer），
    /// 核一退它就是一份没人再用、却仍躺在盘上的凭据 —— 生命周期该跟核一致。
    config_path: PathBuf,
    epoch: u64,
    deadline: tokio::time::Instant,
    emitter: Arc<dyn AuthUrlEmitter>,
    closed_tx: watch::Sender<Option<Result<(), String>>>,
}

/// 瞬态登录核退出原因。
enum ExitReason {
    /// 核自然退出（无需主动 kill，直接 reap）。
    SelfExit,
    /// 用户取消 / kill-on-relogin。
    Cancelled,
    /// 超时未完成登录。
    TimedOut,
    /// STATUS 报 `backendState == "Running"`：登录成功、state 已落盘 → 主动收核。
    LoggedIn,
    /// STATUS 流内部终止（`ReconnectingStream` 正常永不如此）。没有流就没有任何判据来源
    /// （不回退 stdout，见模块头），继续挂着只是让核空跑到超时 → 就地收核。
    StatusStreamEnded,
    InvalidAuthUrl,
}

async fn subscribe_and_supervise(
    ctx: SuperviseCtx,
    mut child: Box<dyn LoginCoreChild>,
    subscriber: Arc<dyn LoginStatusSubscriber>,
    api: TailscaleLoginApiService,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
    ready: oneshot::Sender<StartLoginOutcome>,
) {
    let result = tokio::select! {
        () = ctx.attempt.cancellation() => Err(("cancelled", "cancelled")),
        _ = cancel_rx.recv() => Err(("cancelled", "cancelled")),
        () = tokio::time::sleep_until(ctx.deadline) => Err(("timedOut", "authorizationTimedOut")),
        result = subscriber.subscribe(api.port, &api.secret) => result.map_err(|_| ("failed", "statusSubscriptionFailed")),
    };
    match result {
        Ok(status) => {
            let _ = ready.send(StartLoginOutcome::Started);
            supervise(ctx, child, status, cancel_rx).await;
        }
        Err((phase, reason)) => {
            loop {
                let result = child.close_confirmed().await;
                if result.is_ok() {
                    break;
                }
                let _ = ctx.closed_tx.send(Some(result));
                // A failed Android close retains the registry entry and state claim. Another
                // cancel retries promptly; otherwise a bounded delay avoids a hot loop.
                tokio::select! {
                    _ = cancel_rx.recv() => {},
                    () = tokio::time::sleep(Duration::from_secs(5)) => {},
                }
            }
            remove_login_config(&ctx.config_path);
            ctx.shared.remove_if_epoch(&ctx.server_id, ctx.epoch);
            let _ = ctx.closed_tx.send(Some(Ok(())));
            ctx.emitter
                .progress(&ctx.server_id, &ctx.attempt_id, phase, Some(reason), None);
            ctx.attempt.finish();
            let outcome = if phase == "cancelled" {
                StartLoginOutcome::Cancelled
            } else {
                StartLoginOutcome::Failed(reason.into())
            };
            let _ = ready.send(outcome);
        }
    }
}

/// 单个瞬态登录核的后台 supervisor：消费 STATUS 流（authURL → 事件、Running → 收核）、
/// 扛超时/取消、退出后按 epoch 守卫注销。
async fn supervise(
    ctx: SuperviseCtx,
    mut child: Box<dyn LoginCoreChild>,
    mut status: Box<dyn LoginStatusStream>,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
) {
    // stdout/stderr 的排空**不在这里**：它在 `start_attempt` 构造 `SpawnRequest` 时就接好了，
    // spawner 返回之前已经生效。放在 supervise 里曾经意味着「spawn 与接管之间有一段没人读的
    // 窗口」，而那正是本轮根因缺陷的形态（测速临时核那条腿连这一步都没有）。
    // 登录状态机（`polaris_mesh`，纯逻辑已单测）：它同时承担「同一 URL 反复到达不重复通知用户」
    // 与「后到的 authURL 不得把已登录态打回去」两条不变式，此处不再另写去重标志。
    let mut state = LoginState::Idle;
    let sleep = tokio::time::sleep_until(ctx.deadline);
    tokio::pin!(sleep);

    let reason = loop {
        tokio::select! {
            _ = cancel_rx.recv() => break ExitReason::Cancelled,
            () = ctx.attempt.cancellation() => break ExitReason::Cancelled,
            () = &mut sleep => break ExitReason::TimedOut,
            () = child.wait() => break ExitReason::SelfExit,
            frame = status.recv() => {
                let Some(update) = frame else { break ExitReason::StatusStreamEnded };
                state = match apply_status_frame(&ctx, &state, &update) {
                    Ok(state) => state,
                    Err(()) => break ExitReason::InvalidAuthUrl,
                };
                if state == LoginState::LoggedIn {
                    break ExitReason::LoggedIn;
                }
            }
        }
    };

    match reason {
        ExitReason::SelfExit => {
            log::info!(
                "瞬态登录核自然退出并收割：server={} pid={:?}",
                ctx.server_id,
                child.pid()
            );
        }
        ExitReason::Cancelled => {
            log::info!("瞬态登录核取消 → 终止：server={}", ctx.server_id);
        }
        ExitReason::TimedOut => {
            log::warn!(
                "瞬态登录核未在授权期限内完成登录 → 超时终止：server={}",
                ctx.server_id
            );
        }
        ExitReason::LoggedIn => {
            // 控制面的终局肯定：已认证、state 已落盘 → 核没有再活着的理由，且它还占着该节点的
            // state_directory（主核要用同一份）。1:1 对齐 上游 `handleTransientTailscaleStatus`。
            log::info!(
                "Tailscale 登录成功（backendState=Running）→ 收瞬态登录核：server={}",
                ctx.server_id
            );
        }
        ExitReason::StatusStreamEnded => {
            log::warn!(
                "瞬态登录核 STATUS 流终止（无 URL/登录成功判据来源）→ 终止：server={}",
                ctx.server_id
            );
        }
        ExitReason::InvalidAuthUrl => {
            log::warn!(
                "瞬态登录核 STATUS 提供的授权地址无效：server={}",
                ctx.server_id
            );
        }
    }
    loop {
        let result = if matches!(reason, ExitReason::SelfExit) {
            child.after_exit().await
        } else {
            child.close_confirmed().await
        };
        if result.is_ok() {
            break;
        }
        let _ = ctx.closed_tx.send(Some(result));
        // A failed native close keeps both the child and its claim; cancellation/relogin can retry.
        tokio::select! {
            _ = cancel_rx.recv() => {},
            () = tokio::time::sleep(Duration::from_secs(5)) => {},
        }
    }
    // 核已收割 → 删掉带 secret 的临时 config。
    remove_login_config(&ctx.config_path);
    // reap 后注销（epoch 守卫：不误删 kill-on-relogin 后的新代次表项）。
    ctx.shared.remove_if_epoch(&ctx.server_id, ctx.epoch);
    let _ = ctx.closed_tx.send(Some(Ok(())));
    let (phase, reason) = match reason {
        ExitReason::LoggedIn => ("authorized", None),
        ExitReason::Cancelled => ("cancelled", None),
        ExitReason::TimedOut => ("timedOut", Some("authorizationTimedOut")),
        ExitReason::SelfExit => ("failed", Some("processExited")),
        ExitReason::StatusStreamEnded => ("failed", Some("statusStreamEnded")),
        ExitReason::InvalidAuthUrl => ("failed", Some("invalidAuthUrl")),
    };
    ctx.emitter
        .progress(&ctx.server_id, &ctx.attempt_id, phase, reason, None);
    ctx.attempt.finish();
}

/// 删掉本次登录写盘的临时 config（内含一次性管理 API secret）。best-effort：
/// 已被 kill-on-relogin 的新一代覆写、或早被删掉，都不是问题——本函数只保证「核死了就不留凭据」。
fn remove_login_config(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "删除瞬态登录核临时配置失败（内含一次性 secret）{}：{e}",
                path.display()
            );
        }
    }
}

/// 独占创建携密配置：Unix 0600；Windows 同既有 ConfigFs 写入，继承用户 app_config 目录 ACL。
/// 此处不声称单独设置了 Windows DACL。
///
/// `create_new` 同时拒绝已存在文件和符号链接，避免可预测文件名被预置后覆盖其它路径；旧的崩溃残件
/// 宁可让本次登录明确失败，也不能复用/截断一个身份不明的 inode。
fn create_login_parent(root: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root)
}

fn write_login_config_secure(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

struct LoginConfigGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> LoginConfigGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LoginConfigGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            remove_login_config(self.path);
        }
    }
}

/// 一帧全量端点快照 → 推进登录状态机（并在 URL 首现/变更时发事件）。
///
/// 解码复用主核那条 relay 的同一个投影器 [`decode_tailscale_status`]：`authURL` / `backendState`
/// 的读法只此一处，proto 再漂移时两条腿一起动，不会出现「主核修好了、登录核还错着」。
fn apply_status_frame(
    ctx: &SuperviseCtx,
    current: &LoginState,
    update: &daemon::TailscaleStatusUpdate,
) -> Result<LoginState, ()> {
    if update.endpoints.iter().any(|ep| {
        ctx.tag_to_id.contains_key(&ep.endpoint_tag)
            && !ep.auth_url.is_empty()
            && (ep.backend_state != "Running" || ep.self_.as_ref().is_some_and(|peer| peer.expired))
            && crate::runtime::tailscale_status::validated_tailscale_auth_url(&ep.auth_url)
                .is_none()
    }) {
        return Err(());
    }
    let mut state = current.clone();
    for ev in decode_tailscale_status(update, &ctx.tag_to_id) {
        // 登录成功判据 = **backendState 字面为 Running**，不是 `logged_in`（后者含 `Starting`，那还
        // 只是「在连」；上游 `handleTransientTailscaleStatus` 同样只认 Running）。
        if ev.backend_state == "Running" && !ev.expired {
            state = advance_login_state(&state, &LoginEvent::StatusRunning);
        }
        if let Some(url) = ev.auth_url {
            let next = advance_login_state(&state, &LoginEvent::AuthUrlSeen(url));
            // 状态真的变了才发：同一 URL 每帧都来（核只在换 URL 时才换值），发一次就够。
            if next != state {
                if let LoginState::AwaitingAuth(u) = &next {
                    ctx.emitter.progress(
                        &ctx.server_id,
                        &ctx.attempt_id,
                        "awaitingAuth",
                        None,
                        Some(u),
                    );
                    ctx.emitter.emit_auth_url(&ctx.server_id, &ctx.node_name, u);
                }
            }
            state = next;
        }
    }
    Ok(state)
}

/// server id → 安全文件名片段（防路径穿越；非字母数字/-/_ 归一为 `_`）。
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

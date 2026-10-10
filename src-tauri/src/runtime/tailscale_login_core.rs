//! Tailscale 瞬态登录核的**宿主层编排**（spawn / STATUS 订阅 / emit / 生命周期注册表）。
//!
//! 纯逻辑（config 生成、双写守卫、登录状态机）在 `polaris_mesh::tailscale_login`，已单测；
//! 本模块只做**运行时接线**：拉起一个独立的瞬态 sing-box、订阅**它自己的**管理 API
//! `SubscribeTailscaleStatus` 流、把帧里的 `authURL` 转成登录 URL 事件、把 `backendState == "Running"`
//! 当作登录成功并就地收核，并管理它的生死（kill-on-relogin / 超时自动杀 / 取消 / 自然退出 reap），
//! 与 `ProxyRuntime` 的常驻代理核**隔离**（独立注册表、独立 child 句柄；瞬态核绝不写进 proxy 的
//! pid 槽，故不会被误当作代理核）。
//!
//! iOS 首次登录消费正常主核 producer 的 ready ticket，再观察同一主核的 fresh STATUS。
//! 主核观察的完成、取消和超时只收订阅，不停止正常连接；已有平台的瞬态 custody 保留。
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot, watch};

mod attempts;
mod stale;
pub(crate) use attempts::Attempt;
use attempts::{AttemptGuard, Attempts};
pub use attempts::{LoginMode, LoginProgressReceipt, LoginRequest};
pub use stale::{ProcessStaleLoginSweeper, StaleLoginCoreSweeper};

#[cfg(test)]
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::ServerConfig;
use polaris_core_supervisor::port_bookkeeping::TokioPortProvider;
use polaris_core_supervisor::{
    run_check_raw, PortAllocator, PortExclusions, RawCheck, SingBoxSpawner, SpawnError,
    SpawnRequest, StdioPolicy, TokioSpawner, CONFIG_CHECK_TIMEOUT,
};
use polaris_mesh::tailscale_login::{
    advance_login_state, build_tailscale_login_config, login_config_to_json, LoginEvent,
    LoginState, TailscaleLoginApiService, TAILSCALE_LOGIN_ENDPOINT_TAG,
};
use polaris_singbox_grpc::{daemon, Endpoint, ReconnectConfig, SingBoxApiClient};

use crate::events::broadcast;
use crate::runtime::proxy::core_log::pipe_to_log_with_secrets_owned;
#[cfg(not(target_os = "android"))]
use crate::runtime::proxy::resolve_core_binary;
#[cfg(unix)]
use crate::runtime::proxy::send_signal;
use crate::runtime::proxy::{
    ActionBinding, MainPrerequisiteError, NormalMainAction, ProxyRuntime, ReadyMainTicket,
};
use crate::runtime::tailscale_status::decode_tailscale_status;

/// Android retires a replaced identity inside the backend transaction even when the node has
/// no stored key: its logout entry never preserves a renderer-owned login request.
const BACKEND_IDENTITY_REPLACEMENT: bool = cfg!(target_os = "android");

/// Resolve an explicit credential action from this one current raw node. Renderer metadata
/// and omitted secrets never supply a key or authorize retirement. This is pure preflight.
pub(crate) fn resolve_tailscale_credential_candidate(
    saved: &serde_json::Value,
    candidate: &serde_json::Value,
    request: &LoginRequest,
) -> Result<(serde_json::Value, bool), String> {
    resolve_tailscale_credential_candidate_for(
        saved,
        candidate,
        request,
        BACKEND_IDENTITY_REPLACEMENT,
    )
}

fn resolve_tailscale_credential_candidate_for(
    saved: &serde_json::Value,
    candidate: &serde_json::Value,
    request: &LoginRequest,
    backend_replacement: bool,
) -> Result<(serde_json::Value, bool), String> {
    use polaris_config_engine::user_config::effective_view::{
        park_tailscale_auth_key, retained_tailscale_auth_key, tailscale_control_authority,
        tailscale_credential_revision,
    };
    let id = candidate
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("candidateConfigurationChanged")?;
    let nodes: Vec<_> = saved
        .get("servers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|node| node.get("id").and_then(serde_json::Value::as_str) == Some(id))
        .collect();
    let [current] = nodes.as_slice() else {
        return Err("candidateConfigurationChanged".into());
    };
    let record = retained_tailscale_auth_key(current)?;
    let current_authority = tailscale_control_authority(current)?;
    let candidate_authority = tailscale_control_authority(candidate)?;
    let revision = tailscale_credential_revision(current);
    let replacement = backend_replacement && request.replace_identity;
    let backend_owned = replacement
        || request.reuse_retained_auth_key
        || record.is_some()
        || request.expected_credential_revision.is_some()
        || revision.is_some() && current_authority != candidate_authority;
    if !backend_owned {
        return Ok((candidate.clone(), false));
    }
    // A node without any stored key has no revision; only a replacement may proceed without one.
    if request.expected_credential_revision.as_deref() != revision.as_deref()
        || revision.is_none() && !replacement
    {
        return Err("credentialRevisionChanged".into());
    }
    let supplied_key = candidate
        .get("tailscaleSettings")
        .and_then(|settings| settings.get("authKey"))
        .and_then(serde_json::Value::as_str)
        .filter(|key| !key.trim().is_empty());
    if request.reuse_retained_auth_key
        && (request.mode != LoginMode::Authkey || supplied_key.is_some())
    {
        return Err("invalidCredentialIntent".into());
    }
    let mut candidate = candidate.clone();
    let settings = candidate
        .get_mut("tailscaleSettings")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("candidateConfigurationChanged")?;
    settings.remove("retainedAuthKey");
    settings.remove("retainedAuthKeyAvailable");
    settings.remove("tailscaleCredentialRevision");
    if request.mode == LoginMode::Authkey {
        let key = if request.reuse_retained_auth_key {
            let record = record.ok_or("retainedAuthKeyUnavailable")?;
            if record.control_authority != current_authority
                || record.control_authority != candidate_authority
            {
                return Err("retainedAuthKeyAuthorityChanged".into());
            }
            record.auth_key
        } else {
            supplied_key.ok_or("authKeyRequired")?.to_owned()
        };
        settings.insert("authKey".into(), serde_json::Value::String(key.clone()));
        settings.insert(
            "retainedAuthKey".into(),
            json!({
                "authKey":key,"controlAuthority":candidate_authority
            }),
        );
    } else {
        if supplied_key.is_some() {
            return Err("invalidCredentialIntent".into());
        }
        let mut parked = (*current).clone();
        park_tailscale_auth_key(&mut parked)?;
        settings.remove("authKey");
        if let Some(record) = parked
            .get("tailscaleSettings")
            .and_then(|s| s.get("retainedAuthKey"))
        {
            settings.insert("retainedAuthKey".into(), record.clone());
        }
    }
    if let Some(object) = candidate.as_object_mut() {
        object.remove("tailscaleCredentialIntent");
    }
    Ok((
        ProxyRuntime::merged_tailscale_candidate(saved, &candidate)?,
        true,
    ))
}

/// Whether a normal-main login request is a credential transaction on a platform with one.
fn uses_credential_transaction(
    candidate: &serde_json::Value,
    request: &LoginRequest,
    backend_replacement: bool,
) -> bool {
    candidate
        .get("tailscaleSettings")
        .is_some_and(|settings| settings.get("retainedAuthKey").is_some())
        || request.reuse_retained_auth_key
        || request.mode == LoginMode::Authkey && request.expected_credential_revision.is_some()
        || backend_replacement && request.replace_identity
}

pub(crate) fn park_saved_tailscale_key(
    saved: &mut serde_json::Value,
    id: &str,
) -> Result<(), String> {
    let nodes = saved
        .get_mut("servers")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("candidateConfigurationChanged")?;
    let matches: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            (node.get("id").and_then(serde_json::Value::as_str) == Some(id)).then_some(index)
        })
        .collect();
    let [index] = matches.as_slice() else {
        return Err("candidateConfigurationChanged".into());
    };
    if nodes[*index]
        .get("protocol")
        .and_then(serde_json::Value::as_str)
        != Some("tailscale")
    {
        return Err("candidateConfigurationChanged".into());
    }
    polaris_config_engine::user_config::effective_view::park_tailscale_auth_key(&mut nodes[*index])
}

/// 瞬态登录核的最大挂起时长：登录不完成（用户不去浏览器认证）时到点自动杀核，避免核无限挂着。
/// 交互登录需人去浏览器完成，故给宽松窗口（5 分钟）。
const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// 每个条目都对应一个 sing-box 子进程、两条日志管道和一条 gRPC 重连流；IPC 可并发调用，必须硬封顶。
const MAX_ACTIVE_LOGIN_CORES: usize = 8;

/// 杀瞬态核的优雅窗口（SIGTERM → 宽限 → SIGKILL）。对齐 `ProxyRuntime` 的 `STOP_GRACE`（5s）。
#[cfg(unix)]
const LOGIN_STOP_GRACE: Duration = Duration::from_secs(5);
const LOGIN_REAP_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Only the normal lifecycle producer supplies this ticket. The observer can read and
/// revalidate it, but cannot promote a running cache or a TCP connection to readiness.
struct ReadyMainLogin<'a> {
    proxy: &'a ProxyRuntime,
    ticket: ReadyMainTicket,
}

#[async_trait]
trait MainLoginBinding: Send + Sync {
    async fn validate(&self) -> Result<(), String>;
    fn target_tag(&self, _server_id: &str) -> Option<&str> {
        None
    }
}

#[async_trait]
impl MainLoginBinding for ReadyMainLogin<'_> {
    fn target_tag(&self, server_id: &str) -> Option<&str> {
        self.ticket.target_tag(server_id)
    }
    async fn validate(&self) -> Result<(), String> {
        self.proxy
            .validate_ready_main(&self.ticket)
            .await
            .map_err(|error| error.code().to_owned())
    }
}

struct NormalMainLoginInput<'a> {
    proxy: &'a Arc<ProxyRuntime>,
    saved: &'a serde_json::Value,
    identity_epoch: Option<String>,
    candidate: &'a serde_json::Value,
    action_generation: u64,
}

pub(crate) fn saved_tailscale_identity_epoch(
    saved: &serde_json::Value,
    server_id: &str,
) -> Result<Option<String>, String> {
    use polaris_config_engine::user_config::mesh_route_state::{MeshBindingState, MeshRouteState};
    let Some(raw) = saved.get("meshRouteState") else {
        return Ok(None);
    };
    let state: MeshRouteState = serde_json::from_value(raw.clone())
        .map_err(|_| "Saved mesh identity ledger is invalid".to_owned())?;
    let active: Vec<_> = state
        .identities
        .iter()
        .filter(|identity| {
            identity.server_id == server_id
                && matches!(
                    identity.binding_state,
                    MeshBindingState::Bound | MeshBindingState::Unbound
                )
        })
        .collect();
    let [identity] = active.as_slice() else {
        return Err("Saved Tailscale identity epoch is unavailable".into());
    };
    Ok(Some(identity.identity_epoch.clone()))
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
    /// An opaque fact from this exact native-bound temporary Child, never ordinary Close Ok.
    fn native_exit(&self) -> Option<NativeTransientExit> {
        None
    }
    /// Original native input correlation only. The actual scoped bridge, rather
    /// than these strings or an ordinary close ACK, supplies Store custody.
    #[cfg(target_os = "android")]
    fn android_tailscale_instance(&self) -> Option<(&str, &str)> {
        None
    }
    /// 等子进程自然退出并收割（cancel-safe：可在 `select!` 中反复创建/丢弃）。
    async fn wait(&mut self);
    /// Only an explicit implementation may attest that its owned child exited. The legacy
    /// void wait is still used by measurement watchers and cannot supply a cleanup receipt.
    async fn wait_result(&mut self) -> Result<(), String> {
        self.wait().await;
        Err("登录核退出未确认".into())
    }
    /// 主动终止并收割：生产 SIGTERM→宽限→SIGKILL 后 `wait()`；测试置终止标记即返回。
    async fn terminate(&mut self);
    /// Confirm the same child's cleanup after wait_result, including its native host on Android.
    async fn after_exit(&mut self) -> Result<(), String> {
        Err("登录核清理未确认".into())
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        self.terminate().await;
        Err("登录核关闭未确认".into())
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
    /// Preparation has no OS effect and supplies no exit/no-child evidence.
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        None
    }
    /// Some booking stays strict even if an injected implementation uses legacy spawn.
    /// None lets the concrete adapter reuse this factory for ordinary unbound login.
    async fn spawn_with_temp_native_birth(
        &self,
        req: SpawnRequest,
        _prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        self.spawn(req).await
    }
}

pub use temp_native::{
    LocalLoginNativeTerminal, LoginNativeBirthRef, NativeTransientExit, PreparedTempNativeBirth,
};
#[cfg(not(target_os = "android"))]
pub(crate) use temp_native::{LocalTempNativeTerminal, TempNativeBirthRef};

/// Private issuer data lives in the existing transient adapter, not a second registry.
mod temp_native {
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum NativeRole {
        Temp,
        Login,
        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        Foreign,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum NativeScope {
        SingleTempNativeChildV1,
        SingleLoginNativeChildV1,
        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        Foreign,
    }
    struct Binding {
        login: Option<(Arc<super::RegistryIdentity>, Arc<()>, u64)>,
        #[cfg(not(target_os = "android"))]
        custody: Arc<()>,
        #[cfg(not(target_os = "android"))]
        birth: Arc<()>,
    }
    enum NoChildReturn {
        FactoryReturned,
        AdmissionRejected,
    }
    #[derive(Default)]
    struct NativeCell {
        binding: OnceLock<Binding>,
        factory_entered: AtomicBool,
        no_child: OnceLock<NoChildReturn>,
        member: OnceLock<Arc<()>>,
        status: OnceLock<ExitStatus>,
        retired: AtomicBool,
        login_terminal: OnceLock<Option<ExitStatus>>,
        login_child: std::sync::Mutex<Option<super::LoginChildCustody>>,
    }
    /// Dispatch authority is move-only. Its constructors and native issuer are private.
    pub struct PreparedTempNativeBirth(Arc<NativeCell>);
    #[derive(Clone)]
    pub(crate) struct TempNativeBirthRef(Arc<NativeCell>);
    #[derive(Clone)]
    pub struct LoginNativeBirthRef(Arc<NativeCell>);
    /// Same-registry, same-birth local Login terminal. It is not a writer census or Stop lease.
    pub struct LocalLoginNativeTerminal {
        _birth: LoginNativeBirthRef,
        _status: Option<ExitStatus>,
    }
    pub(super) struct NativeAttachment {
        birth: TempNativeBirthRef,
        member: Arc<()>,
    }
    #[derive(Clone)]
    pub struct NativeTransientExit {
        birth: TempNativeBirthRef,
        member: Arc<()>,
        role: NativeRole,
        scope: NativeScope,
        status: ExitStatus,
    }
    #[cfg(not(target_os = "android"))]
    pub(crate) struct ValidatedTempNativeExit(NativeTransientExit);
    #[cfg(not(target_os = "android"))]
    pub(crate) struct LocalTempNativeTerminal {
        _exit: NativeTransientExit,
    }

    impl PreparedTempNativeBirth {
        pub(super) fn new() -> Self {
            Self(Arc::new(NativeCell::default()))
        }
        #[cfg(not(target_os = "android"))]
        pub(crate) fn bind(
            &self,
            custody: &Arc<()>,
            birth: &Arc<()>,
        ) -> Result<TempNativeBirthRef, String> {
            if self.0.factory_entered.load(Ordering::SeqCst)
                || self.0.no_child.get().is_some()
                || self.0.member.get().is_some()
                || self.0.retired.load(Ordering::SeqCst)
            {
                return Err("测速临时核 native birth 已进入或退休".into());
            }
            self.0
                .binding
                .set(Binding {
                    login: None,
                    custody: custody.clone(),
                    birth: birth.clone(),
                })
                .map_err(|_| "测速临时核 native birth 禁止重复绑定")?;
            Ok(TempNativeBirthRef(self.0.clone()))
        }
        pub(super) fn bind_login(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
        ) -> Result<LoginNativeBirthRef, String> {
            if self.0.factory_entered.load(Ordering::SeqCst)
                || self.0.no_child.get().is_some()
                || self.0.member.get().is_some()
                || self.0.retired.load(Ordering::SeqCst)
            {
                return Err("登录核 native birth 已进入或退休".into());
            }
            self.0
                .binding
                .set(Binding {
                    login: Some((registry.clone(), Arc::new(()), epoch)),
                    #[cfg(not(target_os = "android"))]
                    custody: Arc::new(()),
                    #[cfg(not(target_os = "android"))]
                    birth: Arc::new(()),
                })
                .map_err(|_| "登录核 native birth 禁止重复绑定")?;
            Ok(LoginNativeBirthRef(self.0.clone()))
        }
        pub(super) fn enter_factory(&self) -> Result<(), String> {
            if self.0.binding.get().is_none()
                || self.0.retired.load(Ordering::SeqCst)
                || self.0.no_child.get().is_some()
                || self
                    .0
                    .factory_entered
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
            {
                return Err("测速临时核 native factory 未绑定或重复进入".into());
            }
            Ok(())
        }
        pub(super) fn factory_returned_no_child(&self) {
            let _ = self.0.no_child.set(NoChildReturn::FactoryReturned);
        }
        pub(super) fn admission_rejected(&self) {
            let _ = self.0.no_child.set(NoChildReturn::AdmissionRejected);
        }
        pub(super) fn attach(&self) -> NativeAttachment {
            // Only the concrete factory's returned physical Child reaches this issuer.
            let member = Arc::new(());
            let _ = self.0.member.set(member.clone());
            NativeAttachment {
                birth: TempNativeBirthRef(self.0.clone()),
                member,
            }
        }
    }
    impl NativeAttachment {
        pub(super) fn observe(&self, status: ExitStatus) -> NativeTransientExit {
            let status = *self.birth.0.status.get_or_init(|| status);
            NativeTransientExit {
                birth: self.birth.clone(),
                member: self.member.clone(),
                role: if self
                    .birth
                    .0
                    .binding
                    .get()
                    .is_some_and(|binding| binding.login.is_some())
                {
                    NativeRole::Login
                } else {
                    NativeRole::Temp
                },
                scope: if self
                    .birth
                    .0
                    .binding
                    .get()
                    .is_some_and(|binding| binding.login.is_some())
                {
                    NativeScope::SingleLoginNativeChildV1
                } else {
                    NativeScope::SingleTempNativeChildV1
                },
                status,
            }
        }
    }
    impl NativeTransientExit {
        pub(super) fn matches_cell(&self) -> bool {
            self.birth.0.factory_entered.load(Ordering::SeqCst)
                && self.birth.0.no_child.get().is_none()
                && self
                    .birth
                    .0
                    .member
                    .get()
                    .is_some_and(|member| Arc::ptr_eq(member, &self.member))
                && self.birth.0.binding.get().is_some_and(|binding| {
                    if binding.login.is_some() {
                        self.role == NativeRole::Login
                            && self.scope == NativeScope::SingleLoginNativeChildV1
                    } else {
                        self.role == NativeRole::Temp
                            && self.scope == NativeScope::SingleTempNativeChildV1
                    }
                })
                && self.birth.0.status.get() == Some(&self.status)
        }
        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        pub(crate) fn status_for_test(&self) -> ExitStatus {
            self.status
        }
        /// Negative consumer input only: never used to create a native acceptance positive.
        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        pub(crate) fn corrupted_for_test(&self, fault: &str, foreign: &Self) -> Self {
            let mut fact = self.clone();
            match fault {
                "member" => fact.member = foreign.member.clone(),
                "role" => fact.role = NativeRole::Foreign,
                "scope" => fact.scope = NativeScope::Foreign,
                "status" => fact.status = foreign.status,
                _ => panic!("unknown negative native fact fault"),
            }
            fact
        }
    }
    #[cfg(not(target_os = "android"))]
    impl TempNativeBirthRef {
        pub(crate) fn verify_census_dispatch(
            &self,
            custody: &Arc<()>,
            birth: &Arc<()>,
        ) -> Result<(), String> {
            let binding = self
                .0
                .binding
                .get()
                .ok_or("Temp original binding missing")?;
            if binding.login.is_some()
                || !Arc::ptr_eq(&binding.custody, custody)
                || !Arc::ptr_eq(&binding.birth, birth)
            {
                return Err("Temp census belongs to a foreign custody/birth".into());
            }
            if self.0.factory_entered.load(Ordering::SeqCst)
                && self.0.member.get().is_none()
                && self.0.no_child.get().is_none()
            {
                Err("original Temp factory has not returned".into())
            } else {
                Ok(())
            }
        }

        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        pub(crate) fn replayed_preparation_for_test(&self) -> PreparedTempNativeBirth {
            PreparedTempNativeBirth(self.0.clone())
        }
        #[cfg(all(test, not(target_os = "android")))]
        #[cfg(unix)]
        pub(crate) fn factory_entered_for_test(&self) -> bool {
            self.0.factory_entered.load(Ordering::SeqCst)
        }
        fn validate_binding(&self, custody: &Arc<()>, birth: &Arc<()>) -> Result<(), String> {
            let binding = self
                .0
                .binding
                .get()
                .ok_or("测速临时核 native birth 未绑定")?;
            if binding.login.is_some()
                || !Arc::ptr_eq(&binding.custody, custody)
                || !Arc::ptr_eq(&binding.birth, birth)
                || self.0.retired.load(Ordering::SeqCst)
            {
                return Err("测速临时核 native 回执不属于原 custody/birth".into());
            }
            Ok(())
        }
        pub(crate) fn validate_exit(
            &self,
            fact: Option<NativeTransientExit>,
            custody: &Arc<()>,
            birth: &Arc<()>,
        ) -> Result<ValidatedTempNativeExit, String> {
            self.validate_binding(custody, birth)?;
            let fact = fact.ok_or("测速临时核 native 退出事实缺失")?;
            if !self.0.factory_entered.load(Ordering::SeqCst)
                || self.0.no_child.get().is_some()
                || !Arc::ptr_eq(&self.0, &fact.birth.0)
                || !self
                    .0
                    .member
                    .get()
                    .is_some_and(|member| Arc::ptr_eq(member, &fact.member))
                || fact.role != NativeRole::Temp
                || fact.scope != NativeScope::SingleTempNativeChildV1
                || self.0.status.get() != Some(&fact.status)
            {
                return Err("测速临时核 native 成员或退出状态不匹配".into());
            }
            // Clone/move every sealed reference before the original custody commit.
            Ok(ValidatedTempNativeExit(fact))
        }
        pub(crate) fn validate_no_child(
            &self,
            custody: &Arc<()>,
            birth: &Arc<()>,
        ) -> Result<(), String> {
            self.validate_binding(custody, birth)?;
            let entered = self.0.factory_entered.load(Ordering::SeqCst);
            let returned =
                matches!(self.0.no_child.get(), Some(NoChildReturn::FactoryReturned)) && entered;
            let rejected = matches!(
                self.0.no_child.get(),
                Some(NoChildReturn::AdmissionRejected)
            ) && !entered;
            if (!returned && !rejected)
                || self.0.member.get().is_some()
                || self.0.status.get().is_some()
            {
                return Err("测速临时核 native factory 责任尚未确认".into());
            }
            Ok(())
        }
        pub(crate) fn retire_no_child(&self) {
            self.0.retired.store(true, Ordering::SeqCst);
        }
    }
    impl LoginNativeBirthRef {
        pub(super) fn publish_child(&self, child: super::LoginChildCustody) {
            let mut current = self
                .0
                .login_child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(current.is_none(), "original Login child published twice");
            *current = Some(child);
        }
        pub(super) fn epoch(&self) -> Result<u64, String> {
            self.0
                .binding
                .get()
                .and_then(|binding| binding.login.as_ref())
                .map(|(_, _, epoch)| *epoch)
                .ok_or_else(|| "登录核 native binding 缺失".into())
        }
        pub(super) fn same(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.0, &other.0)
        }
        fn validate_binding(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
        ) -> Result<(), String> {
            let (owner, _, original_epoch) = self
                .0
                .binding
                .get()
                .and_then(|binding| binding.login.as_ref())
                .ok_or("登录核 native binding 缺失")?;
            if !Arc::ptr_eq(owner, registry) || *original_epoch != epoch {
                return Err("登录核 native 回执不属于原 registry/birth".into());
            }
            Ok(())
        }
        pub(super) fn verify_census_dispatch(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
        ) -> Result<(), String> {
            self.validate_binding(registry, epoch)?;
            if self.0.factory_entered.load(Ordering::SeqCst)
                && self.0.member.get().is_none()
                && self.0.no_child.get().is_none()
            {
                return Err("original Login factory has not returned".into());
            }
            Ok(())
        }

        pub(super) fn validate_exit(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
            fact: Option<NativeTransientExit>,
        ) -> Result<NativeTransientExit, String> {
            self.validate_binding(registry, epoch)?;
            let fact = fact.ok_or("登录核 native 退出事实缺失")?;
            if self.0.retired.load(Ordering::SeqCst)
                || !Arc::ptr_eq(&self.0, &fact.birth.0)
                || !fact.matches_cell()
                || fact.role != NativeRole::Login
                || fact.scope != NativeScope::SingleLoginNativeChildV1
            {
                return Err("登录核 native 成员或退出状态不匹配".into());
            }
            Ok(fact)
        }
        pub(super) fn validate_no_child(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
        ) -> Result<(), String> {
            self.validate_binding(registry, epoch)?;
            let entered = self.0.factory_entered.load(Ordering::SeqCst);
            let returned =
                matches!(self.0.no_child.get(), Some(NoChildReturn::FactoryReturned)) && entered;
            let rejected = matches!(
                self.0.no_child.get(),
                Some(NoChildReturn::AdmissionRejected)
            ) && !entered;
            if self.0.retired.load(Ordering::SeqCst)
                || (!returned && !rejected)
                || self.0.member.get().is_some()
                || self.0.status.get().is_some()
            {
                return Err("登录核 native factory 责任尚未确认".into());
            }
            Ok(())
        }
        pub(super) fn commit_terminal(&self, fact: Option<NativeTransientExit>) {
            let _ = self.0.login_terminal.set(fact.map(|fact| fact.status));
            self.0.retired.store(true, Ordering::SeqCst);
            self.0
                .login_child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
        }
        pub(super) fn terminal(
            &self,
            registry: &Arc<super::RegistryIdentity>,
            epoch: u64,
        ) -> Result<LocalLoginNativeTerminal, String> {
            self.validate_binding(registry, epoch)?;
            let status = *self
                .0
                .login_terminal
                .get()
                .ok_or("登录核 native/tail terminal 尚未完成")?;
            if !self.0.retired.load(Ordering::SeqCst) {
                return Err("登录核 native/tail terminal 尚未提交".into());
            }
            Ok(LocalLoginNativeTerminal {
                _birth: self.clone(),
                _status: status,
            })
        }
    }
    #[cfg(not(target_os = "android"))]
    impl ValidatedTempNativeExit {
        pub(crate) fn retire(self) -> LocalTempNativeTerminal {
            self.0.birth.0.retired.store(true, Ordering::SeqCst);
            LocalTempNativeTerminal { _exit: self.0 }
        }
    }
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

    /// Preserve both native capacity admission and producer lifecycle errors at the
    /// writer boundary. Desktop native checkers override this method for Lifecycle.
    async fn check_for_spawn(
        &self,
        binary: &Path,
        config_path: &Path,
    ) -> Result<(), ConfigCheckFailure> {
        use crate::runtime::proxy::android_capacity::CheckFailure;
        self.check_admitted(binary, config_path)
            .await
            .map_err(|error| match error {
                CheckFailure::Rejected(detail) => ConfigCheckFailure::Rejected(detail),
                CheckFailure::CapacityClosed(error) => {
                    ConfigCheckFailure::AndroidCapacityClosed(error)
                }
            })
    }
}

#[derive(Debug)]
pub enum ConfigCheckFailure {
    Rejected(String),
    AndroidCapacityClosed(crate::runtime::proxy::android_capacity::CapacityClosed),
    Lifecycle(polaris_core_supervisor::ValidationLifecycleError),
}

impl std::fmt::Display for ConfigCheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(detail) => f.write_str(detail),
            Self::AndroidCapacityClosed(error) => error.fmt(f),
            Self::Lifecycle(error) => write!(f, "sing-box check 生命周期未确认: {error}"),
        }
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
    fn progress_receipt(&self, receipt: &LoginProgressReceipt) {
        self.progress(
            &receipt.server_id,
            &receipt.attempt_id,
            &receipt.phase,
            receipt.reason.as_deref(),
            receipt.url.as_deref(),
        );
    }
}

#[cfg(target_os = "android")]
struct WarmEmitter;

#[cfg(target_os = "android")]
impl AuthUrlEmitter for WarmEmitter {
    fn emit_auth_url(&self, _server_id: &str, _node_name: &str, _url: &str) {}
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
        if server_id != self.attempt.server_id || attempt_id != self.attempt_id {
            return;
        }
        let context = self.attempt.main_context();
        let receipt = LoginProgressReceipt {
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
                            | "processWaitFailed"
                            | "authorizationTimedOut"
                            | "mainCoreChanged"
                            | "mainCoreInUse"
                            | "stateQueryFailed"
                            | "saveFailed"
                            | "configurationRefreshFailed"
                            | "tooManyLogins"
                            | "invalidAuthUrl"
                            | "savedTailscaleIdentityChanged"
                            | "configurationPending"
                            | "superseded"
                            | "readyUnknown"
                            | "targetNotInMain"
                            | "unsavedConfiguration"
                    )
                })
                .map(str::to_owned),
            // An active attempt can recover only a URL validated from its own fresh stream.
            // Terminal receipts discard it so browser return cannot revive an old URL.
            url: if matches!(phase, "awaitingAuth" | "mainCore") {
                url.and_then(crate::runtime::tailscale_status::validated_tailscale_auth_url)
            } else {
                None
            },
            main_generation: context.as_ref().map(|(generation, _)| *generation),
            identity_epoch: context.and_then(|(_, epoch)| epoch),
        };
        if !self.attempt.record_progress(receipt.clone()) {
            return;
        }
        self.inner.progress_receipt(&receipt);
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
/// sing-box**，且用户完全看不见。兜底 sweep 只在下次起主核时跑（Windows 的作业对象只管
/// 「主程序消失」，句柄被丢弃而主程序还活着时它不出手）。
///
/// 用 Drop 守卫而非 `Command::kill_on_drop(true)`：后者必须设在 **spawn 之前**的 `Command` 上，
/// 而 spawn 收口在 `core-supervisor` 的 `TokioSpawner`（主核与瞬态核共用，主核**不能**跟着 app
/// 的任意 future 生死）。守卫挂在瞬态核专属的这层包装上，射程正好。
///
/// `start_kill` 只作尽力请求（Drop 不能 await），不能形成退出事实。
pub struct TokioLoginCoreChild {
    child: Option<tokio::process::Child>,
    /// Set only after this exact Child's native wait succeeds. Missing PID is not exit evidence.
    reaped: bool,
    /// ECHILD loses the Unix wait identity. A reused numeric PID can never repair this birth.
    wait_identity_lost: bool,
    native_attachment: Option<temp_native::NativeAttachment>,
    native_exit: Option<NativeTransientExit>,
}

impl Drop for TokioLoginCoreChild {
    fn drop(&mut self) {
        // This is a last-owner kill request, never an exit receipt. A registered child remains
        // in registry custody if its supervisor disappears, so task Drop cannot release it.
        if self.wait_identity_lost {
            // Tokio's Unix Reaper::drop itself probes/waits the numeric PID again. ECHILD
            // invalidated that wait identity permanently, so quarantine this one native
            // handle rather than let destructors reap an unrelated reused-PID child.
            if let Some(child) = self.child.take() {
                std::mem::forget(child);
            }
        } else if !self.reaped {
            if let Some(child) = self.child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

#[async_trait]
impl LoginCoreChild for TokioLoginCoreChild {
    fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }
    fn native_exit(&self) -> Option<NativeTransientExit> {
        self.native_exit
            .as_ref()
            .filter(|fact| fact.matches_cell())
            .cloned()
    }
    async fn wait(&mut self) {
        if let Err(error) = self.wait_result().await {
            log::error!("{error}");
        }
    }
    async fn wait_result(&mut self) -> Result<(), String> {
        if self.reaped {
            return Ok(());
        }
        if self.wait_identity_lost {
            return Err("瞬态登录核等待身份已丢失，退出未确认".into());
        }
        let child = self.child.as_mut().ok_or("瞬态登录核句柄不可用")?;
        let status = match child.wait().await {
            Ok(status) => status,
            Err(error) => {
                #[cfg(unix)]
                if error.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32) {
                    self.wait_identity_lost = true;
                }
                return Err(format!("等待瞬态登录核退出失败: {error}"));
            }
        };
        self.native_exit = self
            .native_attachment
            .as_ref()
            .map(|native| native.observe(status));
        self.reaped = true;
        Ok(())
    }
    async fn after_exit(&mut self) -> Result<(), String> {
        if self.reaped {
            Ok(())
        } else {
            Err("瞬态登录核尚未收割".into())
        }
    }
    async fn terminate(&mut self) {
        if let Err(error) = self.close_confirmed().await {
            log::error!("{error}");
        }
    }
    async fn close_confirmed(&mut self) -> Result<(), String> {
        if self.reaped {
            return Ok(());
        }
        if self.wait_identity_lost {
            return Err("瞬态登录核等待身份已丢失，关闭未确认".into());
        }
        // The registry's exclusive Child lock covers this whole close. Probe before a
        // synchronous SIGTERM; a native wait error must never schedule a later PID signal.
        let status = match self
            .child
            .as_mut()
            .ok_or("瞬态登录核句柄不可用")?
            .try_wait()
        {
            Ok(status) => status,
            Err(error) => {
                #[cfg(unix)]
                if error.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32) {
                    self.wait_identity_lost = true;
                }
                return Err(format!("查询瞬态登录核退出失败: {error}"));
            }
        };
        if let Some(status) = status {
            self.native_exit = self
                .native_attachment
                .as_ref()
                .map(|native| native.observe(status));
            self.reaped = true;
            return Ok(());
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid().filter(|pid| *pid != 0) {
            send_signal(pid, polaris_core_supervisor::Signal::Sigterm);
            tokio::select! {
                result = self.wait_result() => return result,
                () = tokio::time::sleep(LOGIN_STOP_GRACE) => {},
            }
        }
        // This timer is owned by the close future. Cancellation leaves the Child in registry
        // custody and cannot leave a detached escalation aimed at a reused numeric PID.
        self.child
            .as_mut()
            .ok_or("瞬态登录核句柄不可用")?
            .start_kill()
            .map_err(|error| format!("终止瞬态登录核失败: {error}"))?;
        tokio::time::timeout(LOGIN_REAP_TIMEOUT, self.wait_result())
            .await
            .map_err(|_| "瞬态登录核收割超时，退出未确认".to_owned())?
    }
}

/// 生产 spawner：经 [`TokioSpawner`] 起真 sing-box，再适配为 [`LoginCoreChild`]。
pub struct TokioLoginCoreSpawner;

#[async_trait]
impl LoginCoreSpawner for TokioLoginCoreSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        self.spawn_with_temp_native_birth(req, None).await
    }
    fn prepare_temp_native_birth(&self) -> Option<PreparedTempNativeBirth> {
        Some(PreparedTempNativeBirth::new())
    }
    async fn spawn_with_temp_native_birth(
        &self,
        req: SpawnRequest,
        prepared: Option<PreparedTempNativeBirth>,
    ) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        let bin = req.binary.clone();
        // This central guard linearizes OS spawn admission, not the outer registry publication.
        // Login/speedtest keep their own admission gate across this ready return and synchronously
        // publish the owned Child without another await or fallible branch before shutdown can drain.
        let admitted = polaris_core_supervisor::with_check_admission(|| {
            if let Some(prepared) = &prepared {
                prepared
                    .enter_factory()
                    .map_err(|error| SpawnError::Spawn {
                        bin: bin.clone(),
                        source: std::io::Error::other(error),
                    })?;
            }
            // Drain executes after OS birth, before this call returns. A panic leaves entered
            // responsibility booked; only this concrete call's actual Err issues no-child.
            let spawned = match TokioSpawner::new().spawn(req) {
                Ok(spawned) => spawned,
                Err(error) => {
                    if let Some(prepared) = &prepared {
                        prepared.factory_returned_no_child();
                    }
                    return Err(error);
                }
            };
            Ok(Box::new(TokioLoginCoreChild {
                child: Some(spawned.child),
                reaped: false,
                wait_identity_lost: false,
                native_attachment: prepared.as_ref().map(PreparedTempNativeBirth::attach),
                native_exit: None,
            }) as Box<dyn LoginCoreChild>)
        });
        match admitted {
            Ok(result) => result,
            Err(error) => {
                // Central admission rejected without invoking the factory closure.
                if let Some(prepared) = &prepared {
                    prepared.admission_rejected();
                }
                Err(SpawnError::Spawn {
                    bin,
                    source: std::io::Error::other(error),
                })
            }
        }
    }
}

#[cfg(target_os = "android")]
struct AndroidLoginCoreSpawner;
#[cfg(target_os = "android")]
struct AndroidLoginCoreChild {
    instance_id: String,
    config_digest: String,
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
            config_digest: polaris_updater::sha256_hex(config.as_bytes()),
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
    fn android_tailscale_instance(&self) -> Option<(&str, &str)> {
        Some((&self.instance_id, &self.config_digest))
    }
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
    async fn wait_result(&mut self) -> Result<(), String> {
        self.wait().await;
        // Native running=false still requires after_exit's own close acknowledgement.
        Ok(())
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
        self.check_for_spawn(binary, config_path)
            .await
            .map_err(|error| error.to_string())
    }

    async fn check_for_spawn(
        &self,
        binary: &Path,
        config_path: &Path,
    ) -> Result<(), ConfigCheckFailure> {
        // Validation's original Child and its private input snapshot stay in central custody
        // on cancellation/error. Lifecycle failure never becomes diagnostic availability.
        match run_check_raw(binary, config_path, CONFIG_CHECK_TIMEOUT)
            .await
            .map_err(ConfigCheckFailure::Lifecycle)?
        {
            RawCheck::Done { success: true, .. } => Ok(()),
            RawCheck::Done { stderr, stdout, .. } => {
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                Err(ConfigCheckFailure::Rejected(format!(
                    "sing-box check 判定登录配置无效: {detail}"
                )))
            }
            RawCheck::SpawnFailed(e) => Err(ConfigCheckFailure::Rejected(format!(
                "sing-box check 启动失败: {e}"
            ))),
            RawCheck::OutputFailed(e) => Err(ConfigCheckFailure::Rejected(format!(
                "sing-box check 输出读取失败: {e}"
            ))),
            // 折叠前不存在的一支：此前无超时 ⇒ 这条路径的表现是永不返回。
            RawCheck::TimedOut { after_secs } => Err(ConfigCheckFailure::Rejected(format!(
                "sing-box check 超时（>{after_secs}s）"
            ))),
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
    fn progress_receipt(&self, receipt: &LoginProgressReceipt) {
        broadcast(
            &self.app,
            crate::events::channel::EVENT_TAILSCALE_LOGIN_PROGRESS,
            json!({"serverId": receipt.server_id, "attemptId": receipt.attempt_id,
                "phase": receipt.phase, "reason": receipt.reason, "url": receipt.url,
                "mainGeneration": receipt.main_generation, "identityEpoch": receipt.identity_epoch}),
        );
        if receipt.phase == "awaitingAuth" {
            broadcast(
                &self.app,
                crate::events::channel::EVENT_TAILSCALE_AUTH_URL,
                json!({"serverId": receipt.server_id, "attemptId": receipt.attempt_id,
                    "nodeName": "", "url": receipt.url, "transient": true}),
            );
        }
    }
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

/// Observes the two original log readers; EOF is distinct from an I/O failure or task Drop.
struct LoginStdioDrain {
    streams: watch::Sender<[Option<Result<(), String>>; 2]>,
    tasks: std::sync::OnceLock<tokio::sync::Mutex<[LoginDrainTask; 2]>>,
}
impl LoginStdioDrain {
    fn new() -> Arc<Self> {
        let (streams, _) = watch::channel([None, None]);
        Arc::new(Self {
            streams,
            tasks: std::sync::OnceLock::new(),
        })
    }
    fn install(&self, handles: [tokio::task::JoinHandle<std::io::Result<()>>; 2]) {
        let tasks = handles.map(|handle| LoginDrainTask {
            handle,
            result: None,
        });
        assert!(
            self.tasks.set(tokio::sync::Mutex::new(tasks)).is_ok(),
            "original Login drain installed twice"
        );
    }
    fn complete(&self, stream: usize, result: Result<(), String>) {
        self.streams.send_modify(|streams| {
            if streams[stream].is_none() {
                streams[stream] = Some(result);
            }
        });
    }
    async fn finished(&self) -> Result<(), String> {
        let mut streams = self.streams.subscribe();
        {
            let result = streams
                .wait_for(|streams| {
                    streams.iter().any(|stream| matches!(stream, Some(Err(_))))
                        || streams.iter().all(Option::is_some)
                })
                .await
                .map_err(|_| "登录核 stdio drain 丢失".to_owned())?;
            for stream in result.iter() {
                stream
                    .as_ref()
                    .ok_or("登录核 stdio EOF 未确认")?
                    .as_ref()
                    .map_err(Clone::clone)?;
            }
        }
        let mut tasks = self
            .tasks
            .get()
            .ok_or("登录核 stdio tasks 缺失")?
            .lock()
            .await;
        for task in tasks.iter_mut() {
            if task.result.is_none() {
                // Await the original stored handle by borrow; cancellation keeps ownership.
                let result = match (&mut task.handle).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(_)) => Err("登录核 stdio task I/O 未确认".into()),
                    Err(_) => Err("登录核 stdio task 未正常完成".into()),
                };
                task.result = Some(result);
            }
        }
        for task in tasks.iter() {
            task.result
                .as_ref()
                .ok_or("登录核 stdio task 尚未完成")?
                .as_ref()
                .map_err(Clone::clone)?;
        }
        Ok(())
    }
}
struct LoginDrainTask {
    handle: tokio::task::JoinHandle<std::io::Result<()>>,
    result: Option<Result<(), String>>,
}
struct LoginDrainReader<R> {
    reader: R,
    drain: Arc<LoginStdioDrain>,
    stream: usize,
    completed: bool,
}
impl<R> LoginDrainReader<R> {
    fn new(reader: R, drain: Arc<LoginStdioDrain>, stream: usize) -> Self {
        Self {
            reader,
            drain,
            stream,
            completed: false,
        }
    }
}
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for LoginDrainReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let remaining = buffer.remaining();
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(&mut this.reader).poll_read(context, buffer);
        let terminal = match &result {
            std::task::Poll::Ready(Ok(())) if remaining > 0 && buffer.filled().len() == before => {
                Some(Ok(()))
            }
            std::task::Poll::Ready(Err(_)) => Some(Err("登录核 stdio drain I/O 未确认".to_owned())),
            _ => None,
        };
        if let Some(terminal) = terminal {
            this.completed = true;
            this.drain.complete(this.stream, terminal);
        }
        result
    }
}
impl<R> Drop for LoginDrainReader<R> {
    fn drop(&mut self) {
        if !self.completed {
            self.drain
                .complete(self.stream, Err("登录核 stdio reader 未到 EOF".into()));
        }
    }
}

type LoginChildCustody = Arc<tokio::sync::Mutex<Box<dyn LoginCoreChild>>>;

#[cfg(target_os = "android")]
fn canonical_login_authority(server: &ServerConfig) -> Result<String, String> {
    let authority = server
        .tailscale_settings
        .as_ref()
        .and_then(|settings| settings.control_url.as_deref())
        .filter(|url| !url.is_empty())
        .unwrap_or("https://controlplane.tailscale.com");
    polaris_config_engine::user_config::mesh_identity_reconcile::canonical_control_authority(
        authority,
    )
    .map_err(|_| "profileBindingUnknown".into())
}

/// Begin's reply is not a release receipt. Every observed error must still run
/// the exact finish; an unknown finish overrides a commit result and keeps custody.
#[cfg(any(target_os = "android", test))]
async fn complete_scoped_action<T, B, F, G, E>(
    begin: B,
    commit: impl FnOnce() -> Result<T, String>,
    finish: F,
) -> Result<T, String>
where
    B: std::future::Future<Output = Result<G, String>>,
    F: FnOnce() -> E,
    E: std::future::Future<Output = Result<(), String>>,
{
    let result = begin.await.and_then(|_reservation| commit());
    finish().await?;
    result
}

/// The registry retains the exact child even if its detached supervisor panics or is aborted.
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
    /// Synthetic registry-only test entries have no physical child.
    _child: Option<LoginChildCustody>,
    native: Option<LoginNativeBirthRef>,
    drain: Option<Arc<LoginStdioDrain>>,
    config_path: Option<PathBuf>,
    #[cfg(target_os = "android")]
    android_instance: Option<(String, String)>,
    #[cfg(target_os = "android")]
    android_authority: Option<String>,
}

/// 注册表共享状态（supervisor 任务与命令层共享）。
#[derive(Default)]
struct Shared {
    identity: Arc<RegistryIdentity>,
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

#[derive(Default)]
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

    /// A native helper receipt may retire only this reservation's registered birth. Keep
    /// every field on mismatch/error so a failed bookkeeping commit remains retryable.
    pub(crate) fn release_confirmed_stop_claim(
        &mut self,
        expected: &MainBirthToken,
    ) -> Result<(), String> {
        if !self.registered || !self.token.same(expected) {
            return Err("Tailscale main reservation does not match the stopped birth".into());
        }
        if !self
            .registry
            .release_main_states_if_token(expected, self.gate)?
        {
            return Err("Tailscale main reservation is no longer the stopped birth".into());
        }
        self.registered = false;
        Ok(())
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
    fn retire_login_tail(
        &self,
        server_id: &str,
        epoch: u64,
        native: &LoginNativeBirthRef,
        fact: Option<NativeTransientExit>,
    ) -> Result<(), String> {
        let mut entries = self.entries.lock().map_err(|_| "登录核注册表不可用")?;
        let entry = entries
            .get(server_id)
            .filter(|entry| entry.epoch == epoch)
            .ok_or("登录核关闭回执不属于当前代次")?;
        if !entry
            .native
            .as_ref()
            .is_some_and(|original| original.same(native))
        {
            return Err("登录核关闭回执不属于原 birth".into());
        }
        if entry._child.is_some() {
            native.validate_exit(&self.identity, epoch, fact.clone())?;
            let drain = entry.drain.as_ref().ok_or("登录核 stdio custody 缺失")?;
            let streams = drain.streams.borrow();
            for stream in streams.iter() {
                stream
                    .as_ref()
                    .ok_or("登录核 stdio EOF 未确认")?
                    .as_ref()
                    .map_err(Clone::clone)?;
            }
            let tasks = drain
                .tasks
                .get()
                .ok_or("登录核 stdio tasks 缺失")?
                .try_lock()
                .map_err(|_| "登录核 stdio tasks 尚在收束")?;
            if !tasks.iter().all(|task| matches!(task.result, Some(Ok(())))) {
                return Err("登录核 stdio tasks 未正常完成".into());
            }
        } else {
            native.validate_no_child(&self.identity, epoch)?;
        }
        let config = entry
            .config_path
            .as_ref()
            .ok_or("登录核配置 custody 缺失")?;
        remove_login_config_confirmed(config)?;
        native.commit_terminal(fact);
        entries.remove(server_id);
        Ok(())
    }
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

/// [`LoginCoreRegistry::start_attempt_with_saved`] 的结果；Started 表示仍待授权。
pub enum StartLoginOutcome {
    AndroidCapacityClosed(crate::runtime::proxy::android_capacity::CapacityClosed),
    PrerequisiteFailed {
        reason: String,
        code: String,
    },
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

/// Match the Go Store constructor's lazy-directory canonicalization, while
/// retaining the original one-component config/tailscale containment.
#[cfg(any(target_os = "ios", target_os = "android", test))]
pub(crate) fn canonical_tailscale_claim_directory(directory: &Path) -> Result<PathBuf, String> {
    fn nearest_existing(path: &Path) -> std::io::Result<PathBuf> {
        if !path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid scoped path",
            ));
        }
        let mut parent = path;
        let mut remaining = Vec::new();
        loop {
            match parent.canonicalize() {
                Ok(mut canonical) => {
                    for name in remaining.into_iter().rev() {
                        canonical.push(name);
                    }
                    return Ok(canonical);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    remaining.push(parent.file_name().ok_or(error)?);
                    parent = parent
                        .parent()
                        .ok_or_else(|| std::io::Error::other("No scoped ancestor"))?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    let unknown = |_| "nativeRetirementUnknown".to_owned();
    let root = directory
        .parent()
        .filter(|root| root.file_name() == Some(std::ffi::OsStr::new("tailscale")))
        .ok_or("nativeRetirementUnknown")?;
    let config = root.parent().ok_or("nativeRetirementUnknown")?;
    let canonical = nearest_existing(directory).map_err(unknown)?;
    let canonical_root = nearest_existing(root).map_err(unknown)?;
    let canonical_config = nearest_existing(config).map_err(unknown)?;
    if canonical_root.parent() != Some(canonical_config.as_path())
        || canonical.parent() != Some(canonical_root.as_path())
    {
        return Err("nativeRetirementUnknown".into());
    }
    Ok(canonical)
}

fn prerequisite_failure(error: MainPrerequisiteError) -> StartLoginOutcome {
    StartLoginOutcome::PrerequisiteFailed {
        reason: error.to_string(),
        code: error.code().to_owned(),
    }
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

pub(crate) struct LoginProducerCapture<'a> {
    _gate: tokio::sync::MutexGuard<'a, ()>,
    view: LoginProducerView,
}

impl LoginProducerCapture<'_> {
    pub(crate) fn into_view(self) -> LoginProducerView {
        self.view
    }
}

#[derive(Clone)]
pub(crate) struct LoginProducerView {
    shared: Arc<Shared>,
    attempts: Vec<Arc<Attempt>>,
    entries: Vec<LoginProducerMember>,
}

#[derive(Clone)]
struct LoginProducerMember {
    id: String,
    epoch: u64,
    native: Option<LoginNativeBirthRef>,
    child: Option<LoginChildCustody>,
    closed: watch::Receiver<Option<Result<(), String>>>,
}

impl LoginProducerView {
    pub(crate) fn member_count(&self) -> usize {
        self.attempts.len() + self.entries.len()
    }

    pub(crate) fn verify_surrender(&self) -> Result<(), String> {
        let entries = self
            .shared
            .entries
            .lock()
            .map_err(|_| "Login registry poisoned")?;
        for attempt in &self.attempts {
            if attempt.claimed.load(Ordering::SeqCst)
                && !attempt.is_finished()
                && !attempt.process_owned.load(Ordering::SeqCst)
            {
                return Err("original Login pipeline has not returned to custody".into());
            }
        }
        for member in &self.entries {
            if let Some(native) = &member.native {
                native.verify_census_dispatch(&self.shared.identity, member.epoch)?;
                if native.terminal(&self.shared.identity, member.epoch).is_ok() {
                    continue;
                }
            }
            let Some(current) = entries
                .get(&member.id)
                .filter(|entry| entry.epoch == member.epoch)
            else {
                return Err("Login row disappeared without original native/tail terminal".into());
            };
            if !match (&member.native, &current.native) {
                (Some(a), Some(b)) => a.same(b),
                (None, None) => true,
                _ => false,
            } {
                return Err("Login original native binding changed".into());
            }
            let same_child = match (&member.child, &current._child) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => member.native.is_some(),
                _ => false,
            };
            if !same_child || !matches!(&*member.closed.borrow(), None | Some(Ok(()))) {
                return Err("original Login custody cannot receive the responsibility".into());
            }
        }
        Ok(())
    }
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
    /// Absent only in fixtures. The runtime installs it on every platform; where the login core
    /// is not a separate process the process scan is empty and the sweep finds nothing.
    stale_sweeper: Option<Arc<dyn StaleLoginCoreSweeper>>,
    timeout: Duration,
    epoch: AtomicU64,
    /// 串行化「检查旧代 → 起核 → 注册」事务，防同一 server 的并发 IPC 各自都看见空表，
    /// 后写者覆盖前写者的 cancel sender，留下无法再取消的孤儿核。
    start_gate: tokio::sync::Mutex<()>,
    closing: AtomicBool,
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
        let shared = Arc::new(Shared::default());
        Self {
            identity: shared.identity.clone(),
            shared,
            spawner,
            checker,
            subscriber,
            resolve_binary,
            stale_sweeper: None,
            timeout,
            epoch: AtomicU64::new(1),
            start_gate: tokio::sync::Mutex::new(()),
            closing: AtomicBool::new(false),
            attempts: Attempts::default(),
        }
    }

    /// Install the sweep that ends login cores left behind by a host process that died.
    #[must_use]
    pub fn with_stale_login_sweeper(mut self, sweeper: Arc<dyn StaleLoginCoreSweeper>) -> Self {
        self.stale_sweeper = Some(sweeper);
        self
    }

    #[cfg(test)]
    pub(crate) fn has_stale_login_sweeper(&self) -> bool {
        self.stale_sweeper.is_some()
    }

    /// A leftover login core is an unregistered writer of a node's state directory. End it
    /// before admitting a new login core or rewriting authentication; refuse while it lives.
    /// The caller holds the state gate, so the registered children cannot change underneath.
    async fn sweep_stale_login_cores(&self) -> Result<(), String> {
        let Some(sweeper) = &self.stale_sweeper else {
            return Ok(());
        };
        let Ok(binary) = (self.resolve_binary)() else {
            log::debug!(target: LOGIN_CORE_LOG_TARGET, "遗留登录核清扫：未解析到核二进制，未执行");
            return Ok(());
        };
        match sweeper.sweep(&binary, &self.shared.pids()).await {
            Ok(0) => Ok(()),
            Ok(ended) => {
                log::warn!(target: LOGIN_CORE_LOG_TARGET, "遗留登录核清扫：已结束 {ended} 个上次遗留的登录核");
                Ok(())
            }
            Err(reason) => {
                log::warn!(target: LOGIN_CORE_LOG_TARGET, "遗留登录核清扫：{reason}，拒绝本次准入");
                Err(reason)
            }
        }
    }

    /// Consume only this original birth's native wait plus stdio/config tail completion.
    /// This receipt neither seals registry membership nor proves SDK/OS writer retirement.
    pub fn login_native_terminal(
        &self,
        birth: &LoginNativeBirthRef,
    ) -> Result<LocalLoginNativeTerminal, String> {
        birth.terminal(&self.identity, birth.epoch()?)
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
                _child: None,
                native: None,
                drain: None,
                config_path: None,
                #[cfg(target_os = "android")]
                android_instance: None,
                #[cfg(target_os = "android")]
                android_authority: None,
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

    /// Close admission synchronously before an exit coordinator awaits any runtime.
    pub fn begin_shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.attempts.cancel_all();
        for entry in self.shared.guard().values() {
            let _ = entry.cancel_tx.send(());
        }
    }

    /// A local all-login drain. Only each registered epoch's native close receipt retires
    /// its Child/config/claim; lost supervisors and poison remain Unknown. The caller owns
    /// the combined exit deadline and may retry after an error without reopening admission.
    pub async fn shutdown_for_exit(&self) -> Result<(), String> {
        self.begin_shutdown();
        let _gate = self.start_gate.lock().await;
        self.attempts.cancel_all();
        let attempts = self.attempts.shutdown_snapshot()?;
        let entries: Vec<_> = self
            .shared
            .entries
            .lock()
            .map_err(|_| "登录核关闭注册表不可用".to_owned())?
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry.epoch,
                    entry.cancel_tx.clone(),
                    entry.closed_rx.clone(),
                    entry.native.clone(),
                )
            })
            .collect();
        // Request every retained birth before waiting for one of them.
        for (_, _, cancel, _, _) in &entries {
            let _ = cancel.send(());
        }
        let mut failure = None;
        for (id, epoch, cancel, mut closed, native) in entries {
            if !matches!(&*closed.borrow_and_update(), Some(Ok(()))) {
                if let Err(error) = signal_and_wait_close(cancel, closed).await {
                    failure.get_or_insert(format!("登录核 {id}/{epoch} 退出未确认：{error}"));
                }
            }
            if let Some(native) = native {
                if let Err(error) = self.login_native_terminal(&native) {
                    failure
                        .get_or_insert(format!("登录核 {id}/{epoch} native/tail 未确认：{error}"));
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        for attempt in attempts {
            attempt.finished().await;
        }
        if !self
            .shared
            .entries
            .lock()
            .map_err(|_| "登录核关闭注册表不可用".to_owned())?
            .is_empty()
        {
            return Err("登录核关闭后仍有未确认代次".into());
        }
        if self
            .attempts
            .shutdown_snapshot()?
            .iter()
            .any(|attempt| !attempt.is_finished() || attempt.process_owned.load(Ordering::SeqCst))
        {
            return Err("登录请求关闭后仍有未确认占用".into());
        }
        Ok(())
    }

    async fn cancel_matching_login(
        &self,
        server_id: &str,
        attempt_id: Option<&str>,
    ) -> Result<bool, String> {
        let Some((cancel_tx, mut closed, native)) = self
            .shared
            .guard()
            .get(server_id)
            .filter(|entry| attempt_id.is_none_or(|id| entry.attempt_id == id))
            .map(|entry| {
                (
                    entry.cancel_tx.clone(),
                    entry.closed_rx.clone(),
                    entry.native.clone(),
                )
            })
        else {
            return Ok(false);
        };
        let closed = if matches!(&*closed.borrow_and_update(), Some(Ok(()))) {
            true
        } else {
            signal_and_wait_close(cancel_tx, closed).await?
        };
        if let Some(native) = native {
            self.login_native_terminal(&native)?;
        }
        Ok(closed)
    }

    /// A failed native close keeps the state-directory claim, preventing a successor from
    /// opening the same Tailscale state.
    async fn cancel_and_wait(&self, server_id: &str) -> Result<(), String> {
        self.cancel_login(server_id).await.map(|_| ())
    }

    pub(crate) async fn pc_producer_view(&self) -> Result<LoginProducerCapture<'_>, String> {
        let gate = self.start_gate.lock().await;
        let attempts = self.attempts.shutdown_snapshot()?;
        let entries = self
            .shared
            .entries
            .lock()
            .map_err(|_| "Login registry poisoned")?
            .iter()
            .map(|(id, entry)| LoginProducerMember {
                id: id.clone(),
                epoch: entry.epoch,
                native: entry.native.clone(),
                child: entry._child.clone(),
                closed: entry.closed_rx.clone(),
            })
            .collect();
        Ok(LoginProducerCapture {
            _gate: gate,
            view: LoginProducerView {
                shared: Arc::clone(&self.shared),
                attempts,
                entries,
            },
        })
    }

    pub async fn prepare(&self, server_id: &str, attempt_id: &str) -> Result<(), String> {
        let _gate = self.state_gate().await;
        if self.closing.load(Ordering::SeqCst) {
            return Err("Polaris is shutting down".into());
        }
        polaris_core_supervisor::config_gate::with_check_producer_registration(|| {
            self.attempts.prepare(server_id, attempt_id)
        })
        .map_err(|error| error.to_string())?
        .map(|_| ())
    }

    #[cfg(target_os = "ios")]
    pub(crate) async fn logout_with_normal_main(
        &self,
        server_id: &str,
        proxy: &Arc<ProxyRuntime>,
        saved: &serde_json::Value,
        generation: u64,
    ) -> Result<(), (String, String)> {
        let failure = |reason: String| (reason, "TAILSCALE_LOGOUT_FAILED".to_owned());
        let _lease = proxy.tailscale_action_lease().map_err(failure)?;
        let request_id = format!("logout-{}", self.epoch.fetch_add(1, Ordering::SeqCst));
        self.prepare(server_id, &request_id)
            .await
            .map_err(failure)?;
        let attempt = self.attempts.get(server_id, &request_id).map_err(failure)?;
        if attempt.claimed.swap(true, Ordering::SeqCst) {
            return Err(failure("attemptAlreadyUsed".into()));
        }
        let mut guard = AttemptGuard(Arc::clone(&attempt), false);
        let mut saved = saved.clone();
        let binding = ActionBinding::new(
            NormalMainAction::TailscaleLogin,
            request_id.clone(),
            &saved,
            vec![server_id.to_owned()],
            saved_tailscale_identity_epoch(&saved, server_id).map_err(failure)?,
        )
        .map_err(|error| (error.to_string(), error.code().to_owned()))?
        .for_attempt(generation, Arc::clone(&attempt));
        let ready = match proxy.live_tailscale_main(binding.clone()).await {
            Ok(Some(ready)) => ready,
            Err(error) => return Err((error.to_string(), error.code().to_owned())),
            Ok(None) => {
                // Classify the captured entry before Stop can change its live status.
                // Only the genuine cold leg may clean original custody and start normally.
                let generation = proxy
                    .prepare_tailscale_action_origin(generation, &attempt)
                    .await
                    .map_err(failure)?;
                // Original cold custody cleanup completed before this commit. Hold
                // its real TS gate and action generation while publishing the no-key doc.
                let gate = self.state_gate().await;
                self.retire_other_attempts_under_state_gate(server_id, &gate, &attempt)
                    .await
                    .map_err(failure)?;
                self.assert_auth_state_available(server_id, &gate, Some(&attempt))
                    .map_err(failure)?;
                saved = proxy
                    .commit_tailscale_credential(
                        &saved,
                        server_id,
                        generation,
                        Some(&attempt),
                        |current| park_saved_tailscale_key(current, server_id),
                    )
                    .map_err(failure)?;
                drop(gate);
                let binding = ActionBinding::new(
                    NormalMainAction::TailscaleLogin,
                    request_id.clone(),
                    &saved,
                    vec![server_id.to_owned()],
                    saved_tailscale_identity_epoch(&saved, server_id).map_err(failure)?,
                )
                .map_err(|error| (error.to_string(), error.code().to_owned()))?
                .for_attempt(generation, Arc::clone(&attempt));
                match tokio::time::timeout(self.timeout, proxy.await_normal_main(binding)).await {
                    Ok(Ok(ready)) => ready,
                    Ok(Err(error)) => return Err((error.to_string(), error.code().to_owned())),
                    Err(_) => {
                        attempt.cancel();
                        return Err(failure("authorizationTimedOut".into()));
                    }
                }
            }
        };
        attempt.bind_main(
            ready.generation(),
            ready.identity_epoch().map(str::to_owned),
        );
        proxy
            .retire_tailscale_account(&ready, server_id, &attempt, &saved, None)
            .await
            .map_err(failure)?;
        attempt.finish();
        guard.1 = true;
        Ok(())
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

    #[cfg(test)]
    pub(crate) fn prepared_attempt_for_test(
        &self,
        server_id: &str,
        attempt_id: &str,
    ) -> Arc<Attempt> {
        self.attempts
            .get(server_id, attempt_id)
            .expect("original prepared request")
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

    #[cfg(any(target_os = "ios", target_os = "android", test))]
    pub(crate) async fn retire_other_attempts_under_state_gate(
        &self,
        server_id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: &Arc<Attempt>,
    ) -> Result<(), String> {
        if !self.valid_main_gate(gate) {
            return Err("Login state gate changed".into());
        }
        let cancelled = self.attempts.retire_node_except_claimed(server_id, keep)?;
        self.cancel_and_wait(server_id).await?;
        for attempt in cancelled {
            attempt.finished().await;
        }
        self.assert_auth_state_available(server_id, gate, Some(keep))
    }

    pub(crate) fn assert_auth_state_available(
        &self,
        server_id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: Option<&Arc<Attempt>>,
    ) -> Result<(), String> {
        if !self.valid_main_gate(gate)
            || self.main_claims(server_id)
            || self.shared.contains(server_id)
            || self.attempts.owns_state_except(server_id, keep)?
        {
            return Err("Tailscale state is in use".into());
        }
        Ok(())
    }

    pub async fn cancel_attempt(&self, server_id: &str, attempt_id: &str) -> Result<(), String> {
        let attempt = self.attempts.cancel(server_id, attempt_id)?;
        let closed = self
            .cancel_matching_login(server_id, Some(attempt_id))
            .await?;
        #[cfg(target_os = "android")]
        if let Some(original) = &attempt {
            if let Some(action) = original.android_action()? {
                if !action.active.load(Ordering::SeqCst) {
                    self.finish_android_action(original, &action).await?;
                }
            }
        }
        #[cfg(target_os = "android")]
        if let Some(original) = &attempt {
            if original.credential_activation().is_some()
                && (closed
                    || !original
                        .progress_receipt()
                        .is_some_and(|receipt| receipt.phase == "authorized"))
            {
                // An unpublished/absent child is not NoConstruction. The exact
                // selected native origin and whole family still have to close.
                self.compensate_android_credential(server_id, attempt_id, original)
                    .await?;
            }
        }
        if let Some(original) = attempt.as_ref().filter(|_| closed) {
            // Only the original registered child's real close/reap result admits this
            // compensation. Attempt done/cancelled/process_owned flags are not terminal.
            #[cfg(not(target_os = "android"))]
            let gate = self.state_gate().await;
            #[cfg(not(target_os = "android"))]
            if self.attempts.original_credential_row(attempt_id, original)
                && self
                    .assert_auth_state_available(server_id, &gate, Some(original))
                    .is_ok()
            {
                if let Some(activation) = original.credential_activation() {
                    if let Some(proxy) = activation.proxy.upgrade() {
                        proxy.commit_tailscale_credential(
                            &activation.saved,
                            server_id,
                            activation.generation,
                            None,
                            |current| park_saved_tailscale_key(current, server_id),
                        )?;
                    }
                }
            }
        }
        if let Some(attempt) = attempt {
            attempt.finished().await;
            #[cfg(target_os = "android")]
            if attempt.android_action()?.is_some() {
                // A concurrent pipeline may still be handing back its exact
                // reservation. Request completion never releases native custody.
                return Err("nativeRetirementUnknown".into());
            }
        }
        Ok(())
    }

    /// Lock order: proxy lifecycle -> this gate. Login never waits for proxy lifecycle.
    pub async fn state_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.start_gate.lock().await
    }

    #[cfg(target_os = "android")]
    async fn finish_android_action(
        &self,
        attempt: &Arc<Attempt>,
        action: &Arc<attempts::AndroidTargetAction>,
    ) -> Result<(), String> {
        use crate::runtime::proxy::android_bridge::tailscale_store;
        match &action.original {
            attempts::AndroidActionOrigin::Runtime(original) => {
                tailscale_store::finish_action(original, &action.state_file, &action.action_id)
                    .await?;
            }
            attempts::AndroidActionOrigin::Warm(tuple) => {
                // A failed create/response can have born an Entry without any
                // Store payload. Only this original tuple can close it.
                if tailscale_store::finish_warm(tuple).await.is_err() {
                    let reservation = tailscale_store::read_warm(tuple).await?;
                    if reservation.held_retirement().is_err() {
                        tailscale_store::close_warm(tuple, &format!("{}-close", action.action_id))
                            .await?;
                    }
                    tailscale_store::finish_warm(tuple).await?;
                }
            }
        }
        attempt.release_android_action(action)
    }

    #[cfg(target_os = "android")]
    async fn with_android_target_action<T>(
        &self,
        attempt: &Arc<Attempt>,
        original: &crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody,
        state_file: &str,
        commit: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if let Some(action) = attempt.android_action()? {
            let attempts::AndroidActionOrigin::Warm(tuple) = &action.original else {
                return Err("nativeRetirementUnknown".into());
            };
            if action.state_file != state_file {
                return Err("nativeRetirementUnknown".into());
            }
            // The warm reservation already fences the entire native family.
            // It cannot acquire a second target permit while that fence is held.
            let held = async {
                let reservation =
                    crate::runtime::proxy::android_bridge::tailscale_store::read_warm(tuple)
                        .await?;
                let (retired, snapshot) = reservation.held_retirement()?;
                if retired.original() != original.original() {
                    return Err("nativeRetirementUnknown".into());
                }
                Ok(snapshot)
            };
            return complete_scoped_action(held, commit, || {
                self.finish_android_action(attempt, &action)
            })
            .await;
        }
        let action = Arc::new(attempts::AndroidTargetAction {
            state_file: state_file.to_owned(),
            action_id: format!("ts-action-{}", self.epoch.fetch_add(1, Ordering::SeqCst)),
            active: AtomicBool::new(true),
            original: attempts::AndroidActionOrigin::Runtime(original.clone()),
        });
        attempt.reserve_android_action(action.clone())?;
        let _activity = attempts::AndroidActionActivity(action.clone());
        // Native begin can install the reservation before Rust rejects its reply.
        // Always finish the same original token, including that decode/error path.
        // Dropping this IPC future retains the token in its original Attempt.
        complete_scoped_action(
            crate::runtime::proxy::android_bridge::tailscale_store::target_action(
                original,
                state_file,
                &action.action_id,
                true,
            ),
            commit,
            || self.finish_android_action(attempt, &action),
        )
        .await
    }

    #[cfg(target_os = "android")]
    async fn close_android_tailscale_origin(
        &self,
        id: &str,
        proxy: &Arc<ProxyRuntime>,
        saved: &serde_json::Value,
        generation: u64,
        request_id: &str,
        attempt: &Arc<Attempt>,
    ) -> Result<
        (
            crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody,
            u64,
            String,
            Option<attempts::AndroidActionActivity>,
        ),
        String,
    > {
        use crate::runtime::proxy::android_bridge::tailscale_store;
        if let Some(main) = proxy
            .stop_android_tailscale_main_origin(id, saved, generation, request_id, attempt)
            .await?
        {
            return Ok((main.0, main.1, main.2, None));
        }
        let old: ServerConfig = serde_json::from_value(
            saved
                .get("servers")
                .and_then(serde_json::Value::as_array)
                .and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|node| node.get("id").and_then(serde_json::Value::as_str) == Some(id))
                })
                .ok_or("candidateConfigurationChanged")?
                .clone(),
        )
        .map_err(|_| "candidateConfigurationChanged")?;
        let authority = canonical_login_authority(&old)?;
        let gate = self.state_gate().await;
        let selected = self
            .shared
            .guard()
            .get(id)
            .map(|entry| {
                if entry.android_authority.as_deref() != Some(authority.as_str()) {
                    return Err("nativeRetirementUnknown".to_owned());
                }
                let metadata = entry
                    .android_instance
                    .clone()
                    .ok_or("nativeRetirementUnknown")?;
                Ok((entry.epoch, metadata, entry.attempt_id.clone()))
            })
            .transpose()?;
        let Some((epoch, metadata, old_request)) = selected else {
            // No Rust entry only chooses preparation. The original native
            // validation and whole-family reservation must authorize its birth.
            drop(gate);
            let saved_server =
                || proxy.saved_android_credential_target(saved, id, generation, attempt);
            let main_core = || MainLoginSnapshot {
                alive: proxy.tailscale_writer_alive(),
                generation: proxy.core_generation(),
                api_port: proxy.status().clash_api_port,
                ..Default::default()
            };
            let request = LoginRequest {
                attempt_id: request_id.to_owned(),
                mode: LoginMode::Browser,
                replace_identity: false,
                reuse_retained_auth_key: false,
                expected_credential_revision: None,
            };
            let mut activity = None;
            let outcome = self
                .launch_attempt(
                    &old,
                    proxy.android_tailscale_user_data(),
                    &request,
                    attempt,
                    &saved_server,
                    &main_core,
                    Arc::new(WarmEmitter),
                    Some((proxy, generation)),
                    Some(&mut activity),
                )
                .await;
            if !matches!(&outcome, StartLoginOutcome::Started) {
                return Err(match outcome {
                    StartLoginOutcome::Failed(error) => error,
                    StartLoginOutcome::Cancelled => "cancelled".into(),
                    _ => "nativeRetirementUnknown".into(),
                });
            }
            let action = attempt.android_action()?.ok_or("nativeRetirementUnknown")?;
            let attempts::AndroidActionOrigin::Warm(tuple) = &action.original else {
                return Err("nativeRetirementUnknown".into());
            };
            let reservation = tailscale_store::read_warm(tuple).await?;
            let (held_retired, _held_family) = reservation.held_retirement()?;
            // The supervisor observed the current original run before close.
            // The validation's cached metadata cannot substitute for that run.
            let original = attempt.android_store()?;
            if original.original().producer_kind != "Login"
                || original.original().logical_instance_id != tuple.logical_instance_id()
                || original.original().actual_config_digest != tuple.config_digest()
            {
                return Err("nativeRetirementUnknown".into());
            }
            let retired = tailscale_store::read_login_retirement(&original).await?;
            if retired.original() != held_retired.original() {
                return Err("nativeRetirementUnknown".into());
            }
            if proxy.core_generation() != generation || attempt.cancelled() {
                return Err("mainCoreChanged".into());
            }
            attempt.record_android_store(retired.clone())?;
            return Ok((
                retired,
                generation,
                TAILSCALE_LOGIN_ENDPOINT_TAG.to_owned(),
                activity,
            ));
        };
        let original = tailscale_store::observe_login(&metadata.0, &metadata.1).await?;
        if proxy.core_generation() != generation
            || attempt.cancelled()
            || !self
                .shared
                .guard()
                .get(id)
                .is_some_and(|entry| entry.epoch == epoch)
        {
            return Err("mainCoreChanged".into());
        }
        attempt.record_android_store(original.clone())?;
        // Capture first, then signal the original supervisor exactly once. Its
        // ordinary ACK remains distinct from this original native writer export.
        if !self.cancel_matching_login(id, Some(&old_request)).await? {
            return Err("nativeRetirementUnknown".into());
        }
        let retired = tailscale_store::read_login_retirement(&original).await?;
        drop(gate);
        Ok((
            retired,
            generation,
            TAILSCALE_LOGIN_ENDPOINT_TAG.to_owned(),
            None,
        ))
    }

    #[cfg(target_os = "android")]
    async fn activate_android_credential(
        &self,
        requested: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        normal: &NormalMainLoginInput<'_>,
    ) -> Result<(), String> {
        normal
            .proxy
            .tailscale_candidate_preflight(normal.saved, normal.candidate)?;
        let mut guard = AttemptGuard(attempt.clone(), false);
        let (retired, generation, tag, _warm_activity) = self
            .close_android_tailscale_origin(
                &requested.id,
                normal.proxy,
                normal.saved,
                normal.action_generation,
                &request.attempt_id,
                attempt,
            )
            .await?;
        let gate = self.state_gate().await;
        let cancelled = self
            .attempts
            .retire_node_except(&requested.id, Some(&request.attempt_id))?;
        self.cancel_and_wait(&requested.id).await?;
        for original in cancelled {
            original.finished().await;
        }
        self.assert_auth_state_available(&requested.id, &gate, Some(attempt))?;
        let directory = normal
            .proxy
            .android_tailscale_auth_directory(&requested.id)?;
        let export = retired.retired_export()?;
        let node = ProxyRuntime::selected_android_auth_node(
            &retired.original().original_observed_runs,
            &retired.original().actual_config_digest,
            &tag,
            directory.to_str().ok_or("profileBindingUnknown")?,
            &export,
        )?;
        self.with_android_target_action(attempt, &retired, &node.state_file, || {
            self.assert_auth_state_available(&requested.id, &gate, Some(attempt))?;
            let saved = normal.proxy.commit_tailscale_credential(
                normal.saved,
                &requested.id,
                generation,
                Some(attempt),
                |current| {
                    if request.replace_identity {
                        normal.proxy.retire_android_tailscale_auth(
                            &requested.id,
                            &gate,
                            attempt,
                            &node,
                        )?;
                    }
                    let replacement =
                        ProxyRuntime::merged_tailscale_candidate(current, normal.candidate)?;
                    let node = current
                        .get_mut("servers")
                        .and_then(serde_json::Value::as_array_mut)
                        .and_then(|nodes| {
                            nodes.iter_mut().find(|node| {
                                node.get("id").and_then(serde_json::Value::as_str)
                                    == Some(&requested.id)
                            })
                        })
                        .ok_or("candidateConfigurationChanged")?;
                    *node = replacement;
                    Ok(())
                },
            )?;
            // Bind the actual committed CAS before awaiting finish. A lost
            // finish must not discard the only compensation snapshot.
            attempt.record_credential_activation(normal.proxy, saved, generation);
            Ok(())
        })
        .await?;
        guard.1 = true;
        Ok(())
    }

    #[cfg(target_os = "android")]
    async fn compensate_android_credential(
        &self,
        id: &str,
        request_id: &str,
        attempt: &Arc<Attempt>,
    ) -> Result<(), String> {
        let original = attempt.android_store()?;
        use crate::runtime::proxy::android_bridge::tailscale_store;
        let retired = match original.original().producer_kind.as_str() {
            "Main" => tailscale_store::read_main_retirement(&original).await?,
            "Login" => tailscale_store::read_login_retirement(&original).await?,
            _ => return Err("nativeRetirementUnknown".into()),
        };
        let activation = attempt
            .credential_activation()
            .ok_or("nativeRetirementUnknown")?;
        let proxy = activation
            .proxy
            .upgrade()
            .ok_or("nativeRetirementUnknown")?;
        let gate = self.state_gate().await;
        if !self.attempts.original_credential_row(request_id, attempt) {
            return Err("credentialRevisionChanged".into());
        }
        self.assert_auth_state_available(id, &gate, Some(attempt))?;
        let directory = proxy.android_tailscale_auth_directory(id)?;
        let state_file = directory.join("tailscaled.state");
        let scopes: Vec<_> = original
            .original()
            .original_observed_runs
            .last()
            .ok_or("profileBindingUnknown")?
            .scopes
            .iter()
            .filter(|scope| {
                Some(scope.state_directory.as_str()) == directory.to_str()
                    && Some(scope.state_file.as_str()) == state_file.to_str()
            })
            .collect();
        let [scope] = scopes.as_slice() else {
            return Err("profileBindingUnknown".into());
        };
        if original.original().producer_kind == "Login" && scope.tag != TAILSCALE_LOGIN_ENDPOINT_TAG
        {
            return Err("profileBindingUnknown".into());
        }
        let node = ProxyRuntime::selected_android_auth_node(
            &retired.original().original_observed_runs,
            &retired.original().actual_config_digest,
            &scope.tag,
            directory.to_str().ok_or("profileBindingUnknown")?,
            &retired.retired_export()?,
        )?;
        self.with_android_target_action(attempt, &retired, &node.state_file, || {
            // Cancellation is intended here. The same original row, actual child
            // close, native writer family and exact promoted target CAS remain mandatory.
            if !self.attempts.original_credential_row(request_id, attempt) {
                return Err("credentialRevisionChanged".into());
            }
            self.assert_auth_state_available(id, &gate, Some(attempt))?;
            let saved = proxy.commit_tailscale_credential(
                &activation.saved,
                id,
                activation.generation,
                None,
                |current| park_saved_tailscale_key(current, id),
            )?;
            // A successful park with unknown finish keeps its own latest exact
            // CAS, so a same-request retry does not borrow the pre-park revision.
            attempt.record_credential_activation(&proxy, saved, activation.generation);
            Ok(())
        })
        .await
    }

    #[cfg(target_os = "android")]
    pub(crate) async fn logout_with_android_store(
        &self,
        id: &str,
        proxy: &Arc<ProxyRuntime>,
        saved: &serde_json::Value,
        generation: u64,
    ) -> Result<(), String> {
        let _lease = proxy.tailscale_action_lease()?;
        let request_id = format!("logout-{}", self.epoch.fetch_add(1, Ordering::SeqCst));
        self.prepare(id, &request_id).await?;
        let attempt = self.attempts.get(id, &request_id)?;
        if attempt.claimed.swap(true, Ordering::SeqCst) {
            return Err("attemptAlreadyUsed".into());
        }
        let mut guard = AttemptGuard(attempt.clone(), false);
        let (retired, generation, tag, _warm_activity) = self
            .close_android_tailscale_origin(id, proxy, saved, generation, &request_id, &attempt)
            .await?;
        let gate = self.state_gate().await;
        self.retire_other_attempts_under_state_gate(id, &gate, &attempt)
            .await?;
        let directory = proxy.android_tailscale_auth_directory(id)?;
        let node = ProxyRuntime::selected_android_auth_node(
            &retired.original().original_observed_runs,
            &retired.original().actual_config_digest,
            &tag,
            directory.to_str().ok_or("profileBindingUnknown")?,
            &retired.retired_export()?,
        )?;
        self.with_android_target_action(&attempt, &retired, &node.state_file, || {
            self.assert_auth_state_available(id, &gate, Some(&attempt))?;
            proxy
                .commit_tailscale_credential(saved, id, generation, Some(&attempt), |current| {
                    proxy.retire_android_tailscale_auth(id, &gate, &attempt, &node)?;
                    park_saved_tailscale_key(current, id)
                })
                .map(|_| ())
        })
        .await?;
        attempt.finish();
        guard.1 = true;
        Ok(())
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
        // A distinct kind: the command layer reports this refusal under its own code.
        self.sweep_stale_login_cores()
            .await
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::ResourceBusy, reason))?;
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

    pub(crate) fn assert_main_claims_drained(
        &self,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        if !self.valid_main_gate(gate) {
            return Err("Tailscale main drain has wrong registry gate".into());
        }
        let main = self
            .shared
            .main
            .lock()
            .map_err(|_| "Tailscale main ownership is unknown".to_owned())?;
        if main.is_some() {
            return Err("Tailscale main ownership remains unconfirmed".into());
        }
        Ok(())
    }

    /// The original birth owns the full endpoint census, including nodes other
    /// than the account being retired. This is registry scope, not a Stop proof.
    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) fn main_scope_if_token(
        &self,
        token: &MainBirthToken,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<Vec<(String, String, String)>, String> {
        if !self.valid_main_gate(gate) || !Arc::ptr_eq(&token.registry, &self.identity) {
            return Err("nativeRetirementUnknown".into());
        }
        let main = self
            .shared
            .main
            .lock()
            .map_err(|_| "nativeRetirementUnknown")?;
        let claim = main
            .as_ref()
            .filter(|claim| claim.token.same(token))
            .ok_or("nativeRetirementUnknown")?;
        claim
            .directories
            .iter()
            .map(|(id, directory)| {
                let tag = claim
                    .endpoints
                    .get(id)
                    .and_then(|ep| ep.get("tag"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|tag| !tag.is_empty())
                    .ok_or("nativeRetirementUnknown")?;
                let directory = canonical_tailscale_claim_directory(directory)?;
                let directory = directory.to_str().ok_or("nativeRetirementUnknown")?;
                Ok((
                    tag.to_owned(),
                    directory.to_owned(),
                    Path::new(directory)
                        .join("tailscaled.state")
                        .to_str()
                        .ok_or("nativeRetirementUnknown")?
                        .to_owned(),
                ))
            })
            .collect()
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
        if self.closing.load(Ordering::SeqCst) {
            return Err("Polaris is shutting down".into());
        }
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
            replace_identity: false,
            reuse_retained_auth_key: false,
            expected_credential_revision: None,
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
        self.start_attempt_inner(
            requested,
            user_data,
            request,
            saved_server,
            main_core,
            emitter,
            None,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "normal prerequisite retains the saved action binding"
    )]
    pub async fn start_attempt_with_normal_main(
        &self,
        requested: &ServerConfig,
        user_data: &Path,
        request: LoginRequest,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
        proxy: &Arc<ProxyRuntime>,
        saved: &serde_json::Value,
        identity_epoch: Option<String>,
        candidate: &serde_json::Value,
        action_generation: u64,
    ) -> StartLoginOutcome {
        self.start_attempt_inner(
            requested,
            user_data,
            request,
            saved_server,
            main_core,
            emitter,
            Some(NormalMainLoginInput {
                proxy,
                saved,
                identity_epoch,
                candidate,
                action_generation,
            }),
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "existing and normal-main paths share the same attempt custody"
    )]
    async fn start_attempt_inner(
        &self,
        requested: &ServerConfig,
        user_data: &Path,
        request: LoginRequest,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
        normal_main: Option<NormalMainLoginInput<'_>>,
    ) -> StartLoginOutcome {
        if self.closing.load(Ordering::SeqCst) {
            return StartLoginOutcome::Failed("Polaris is shutting down".into());
        }
        let attempt = match self.attempts.get(&requested.id, &request.attempt_id) {
            Ok(attempt) => attempt,
            Err(reason) => return StartLoginOutcome::Failed(reason),
        };
        let producer_origin = normal_main
            .as_ref()
            .map(|normal| (normal.proxy, normal.action_generation));
        #[cfg(not(target_os = "ios"))]
        let credential_transaction = normal_main.as_ref().is_some_and(|normal| {
            uses_credential_transaction(normal.candidate, &request, BACKEND_IDENTITY_REPLACEMENT)
        });
        #[cfg(not(target_os = "ios"))]
        let _credential_lease = if credential_transaction {
            let normal = normal_main.as_ref().expect("credential input is present");
            let lease = match normal.proxy.tailscale_action_lease() {
                Ok(lease) => lease,
                Err(error) => return StartLoginOutcome::Failed(error),
            };
            #[cfg(not(target_os = "android"))]
            let activation = self
                .activate_pc_credential(requested, &request, &attempt, main_core, normal)
                .await;
            #[cfg(target_os = "android")]
            let activation = self
                .activate_android_credential(requested, &request, &attempt, normal)
                .await;
            if let Err(error) = activation {
                return if attempt.cancelled() {
                    StartLoginOutcome::Cancelled
                } else {
                    StartLoginOutcome::Failed(error)
                };
            }
            Some(lease)
        } else {
            None
        };
        #[cfg(target_os = "android")]
        let producer_origin = if credential_transaction {
            let Some(activation) = attempt.credential_activation() else {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            };
            producer_origin.map(|(proxy, _)| (proxy, activation.generation))
        } else {
            producer_origin
        };
        #[cfg(target_os = "android")]
        let saved_after_activation = || -> Result<ServerConfig, String> {
            let activation = attempt
                .credential_activation()
                .ok_or("nativeRetirementUnknown")?;
            let proxy = activation
                .proxy
                .upgrade()
                .ok_or("nativeRetirementUnknown")?;
            proxy.saved_android_credential_target(
                &activation.saved,
                &requested.id,
                activation.generation,
                &attempt,
            )
        };
        #[cfg(target_os = "android")]
        let saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync) =
            if credential_transaction {
                &saved_after_activation
            } else {
                saved_server
            };
        #[cfg(not(target_os = "ios"))]
        let normal_main = if credential_transaction {
            None
        } else {
            normal_main
        };
        let claimed =
            match polaris_core_supervisor::config_gate::with_check_producer_registration(|| {
                attempt.claimed.swap(true, Ordering::SeqCst)
            }) {
                Ok(claimed) => claimed,
                Err(error) => return StartLoginOutcome::Failed(error.to_string()),
            };
        if claimed {
            return StartLoginOutcome::Failed("attemptAlreadyUsed".into());
        }
        let mut request_guard = AttemptGuard(attempt.clone(), false);
        let emitter: Arc<dyn AuthUrlEmitter> = Arc::new(AttemptReceiptEmitter {
            inner: emitter,
            attempt: attempt.clone(),
            attempt_id: request.attempt_id.clone(),
        });
        emitter.progress(&requested.id, &request.attempt_id, "starting", None, None);
        let outcome = if let Some(normal_main) = normal_main {
            self.launch_normal_main_attempt(
                requested,
                &request,
                &attempt,
                saved_server,
                main_core,
                emitter.clone(),
                normal_main,
            )
            .await
        } else {
            self.launch_attempt(
                requested,
                user_data,
                &request,
                &attempt,
                saved_server,
                main_core,
                emitter.clone(),
                producer_origin,
                #[cfg(target_os = "android")]
                None,
            )
            .await
        };
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
                StartLoginOutcome::PrerequisiteFailed { code, .. } => {
                    emitter.progress(
                        &requested.id,
                        &request.attempt_id,
                        "failed",
                        Some(code),
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

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    async fn activate_pc_credential(
        &self,
        requested: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        normal: &NormalMainLoginInput<'_>,
    ) -> Result<(), String> {
        normal
            .proxy
            .tailscale_candidate_preflight(normal.saved, normal.candidate)?;
        // Preserve only the original unclaimed prepared request. The ordinary logout
        // admission closes other transient owners and rejects the real main claim.
        let retired = self
            .logout(
                &requested.id,
                &|| main_core().alive,
                Some(&request.attempt_id),
                |gate| {
                    let saved = normal
                        .proxy
                        .commit_tailscale_credential(
                            normal.saved,
                            &requested.id,
                            normal.action_generation,
                            Some(attempt),
                            |current| {
                                if request.replace_identity {
                                    normal.proxy.retire_pc_tailscale_auth(
                                        &requested.id,
                                        gate,
                                        Some(attempt),
                                    )?;
                                }
                                let replacement = ProxyRuntime::merged_tailscale_candidate(
                                    current,
                                    normal.candidate,
                                )?;
                                let nodes = current
                                    .get_mut("servers")
                                    .and_then(serde_json::Value::as_array_mut)
                                    .ok_or("candidateConfigurationChanged")?;
                                let node = nodes
                                    .iter_mut()
                                    .find(|node| {
                                        node.get("id").and_then(serde_json::Value::as_str)
                                            == Some(&requested.id)
                                    })
                                    .ok_or("candidateConfigurationChanged")?;
                                *node = replacement;
                                Ok(())
                            },
                        )
                        .map_err(std::io::Error::other)?;
                    attempt.record_credential_activation(
                        normal.proxy,
                        saved,
                        normal.action_generation,
                    );
                    Ok(())
                },
            )
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::ResourceBusy {
                    stale::STALE_LOGIN_CORE_ALIVE.to_owned()
                } else {
                    "credentialCommitUnknown".to_owned()
                }
            })?;
        if !retired {
            log::info!(
                target: LOGIN_CORE_LOG_TARGET,
                "Tailscale 切换账号被拒：主连接正持有节点 {:?}（mainCoreInUse）",
                requested.id
            );
            return Err("mainCoreInUse".into());
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the ticket and saved identity are distinct authorities"
    )]
    async fn launch_normal_main_attempt(
        &self,
        requested: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
        normal: NormalMainLoginInput<'_>,
    ) -> StartLoginOutcome {
        if normal
            .candidate
            .get("id")
            .and_then(serde_json::Value::as_str)
            != Some(&requested.id)
        {
            return StartLoginOutcome::Failed("candidateConfigurationChanged".into());
        }
        #[cfg(target_os = "ios")]
        if request.replace_identity {
            return self
                .launch_normal_main_replacement(
                    requested, request, attempt, main_core, emitter, normal,
                )
                .await;
        }
        #[cfg(target_os = "ios")]
        let _credential_lease = if request.expected_credential_revision.is_some() {
            match normal.proxy.tailscale_action_lease() {
                Ok(lease) => Some(lease),
                Err(error) => return StartLoginOutcome::Failed(error),
            }
        } else {
            None
        };
        #[cfg(target_os = "ios")]
        let committed = if request.expected_credential_revision.is_some() {
            let generation = match normal
                .proxy
                .prepare_tailscale_action_origin(normal.action_generation, attempt)
                .await
            {
                Ok(generation) => generation,
                Err(error) => return StartLoginOutcome::Failed(error),
            };
            if normal.proxy.tailscale_writer_alive() {
                return StartLoginOutcome::InMainCorePending;
            }
            let gate = self.state_gate().await;
            if let Err(error) = self
                .retire_other_attempts_under_state_gate(&requested.id, &gate, attempt)
                .await
            {
                return StartLoginOutcome::Failed(error);
            }
            if let Err(error) =
                self.assert_auth_state_available(&requested.id, &gate, Some(attempt))
            {
                return StartLoginOutcome::Failed(error);
            }
            match normal.proxy.commit_tailscale_credential(
                normal.saved,
                &requested.id,
                generation,
                Some(attempt),
                |current| {
                    let replacement =
                        ProxyRuntime::merged_tailscale_candidate(current, normal.candidate)?;
                    let nodes = current
                        .get_mut("servers")
                        .and_then(serde_json::Value::as_array_mut)
                        .ok_or("candidateConfigurationChanged")?;
                    let node = nodes
                        .iter_mut()
                        .find(|node| {
                            node.get("id").and_then(serde_json::Value::as_str)
                                == Some(&requested.id)
                        })
                        .ok_or("candidateConfigurationChanged")?;
                    *node = replacement;
                    Ok(())
                },
            ) {
                Ok(saved) => Some((generation, saved)),
                Err(error) => return StartLoginOutcome::Failed(error),
            }
        } else {
            None
        };
        #[cfg(target_os = "ios")]
        let normal = NormalMainLoginInput {
            saved: committed.as_ref().map_or(normal.saved, |(_, saved)| saved),
            action_generation: committed
                .as_ref()
                .map_or(normal.action_generation, |(generation, _)| *generation),
            ..normal
        };
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
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
        let key = server
            .tailscale_settings
            .as_ref()
            .and_then(|ts| ts.auth_key.as_deref())
            .filter(|key| !key.trim().is_empty());
        if request.mode == LoginMode::Browser && key.is_some() {
            return StartLoginOutcome::InMainCorePending;
        }
        if request.mode == LoginMode::Authkey && key.is_none() {
            return StartLoginOutcome::Failed("authKeyRequired".into());
        }
        #[cfg(target_os = "ios")]
        let action_generation = match normal
            .proxy
            .prepare_tailscale_action_origin(normal.action_generation, attempt)
            .await
        {
            Ok(generation) => generation,
            Err(error) => {
                return if attempt.cancelled() {
                    StartLoginOutcome::Cancelled
                } else {
                    StartLoginOutcome::Failed(error)
                }
            }
        };
        #[cfg(not(target_os = "ios"))]
        let action_generation = normal.action_generation;
        let binding = match ActionBinding::new(
            NormalMainAction::TailscaleLogin,
            request.attempt_id.clone(),
            normal.saved,
            vec![server.id.clone()],
            normal.identity_epoch,
        ) {
            Ok(binding) => binding.for_attempt(action_generation, Arc::clone(attempt)),
            Err(error) => return prerequisite_failure(error),
        };
        emitter.progress(
            &server.id,
            &request.attempt_id,
            "preparingConnection",
            None,
            None,
        );
        // No TS state gate is held while the normal Start producer acquires its lifecycle gate.
        emitter.progress(
            &server.id,
            &request.attempt_id,
            "waitingForReady",
            None,
            None,
        );
        let ticket = tokio::select! {
            biased;
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            result = tokio::time::timeout(self.timeout, normal.proxy.await_normal_main(binding)) => match result {
                Ok(Ok(ticket)) => ticket,
                Ok(Err(error)) => return prerequisite_failure(error),
                Err(_) => {
                    emitter.progress(&server.id, &request.attempt_id, "timedOut", Some("authorizationTimedOut"), None);
                    attempt.cancel();
                    attempt.finish();
                    return StartLoginOutcome::Failed("authorizationTimedOut".into());
                },
            },
        };
        attempt.bind_main(
            ticket.generation(),
            ticket.identity_epoch().map(str::to_owned),
        );
        let bound = ReadyMainLogin {
            proxy: normal.proxy,
            ticket,
        };
        let main = main_core();
        if main.generation != bound.ticket.generation()
            || main.api_port != bound.ticket.api_port()
            || main.api_secret != bound.ticket.api_secret()
        {
            return StartLoginOutcome::Failed("mainCoreChanged".into());
        }
        let gate = tokio::select! {
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            gate = self.start_gate.lock() => gate,
        };
        let current = saved_server();
        if !current.is_ok_and(|current| {
            current.id == server.id && current.tailscale_settings == server.tailscale_settings
        }) || !self.main_matches_request(&server, request.mode)
            || bound.ticket.target_tag(&server.id).is_none()
        {
            return StartLoginOutcome::InMainCorePending;
        }
        drop(gate);
        self.confirm_main_request(
            &server,
            request,
            attempt,
            &main,
            main_core,
            saved_server,
            Some(&bound),
            emitter,
        )
        .await
    }

    #[cfg(target_os = "ios")]
    #[allow(
        clippy::too_many_arguments,
        reason = "the same claimed request retains original and candidate bindings"
    )]
    async fn launch_normal_main_replacement(
        &self,
        requested: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        emitter: Arc<dyn AuthUrlEmitter>,
        normal: NormalMainLoginInput<'_>,
    ) -> StartLoginOutcome {
        let key = requested
            .tailscale_settings
            .as_ref()
            .and_then(|settings| settings.auth_key.as_deref())
            .filter(|key| !key.trim().is_empty());
        if request.mode == LoginMode::Authkey && key.is_none() {
            return StartLoginOutcome::Failed("authKeyRequired".into());
        }
        if request.mode == LoginMode::Browser && key.is_some() {
            return StartLoginOutcome::Failed("candidateConfigurationChanged".into());
        }
        if let Err(error) = normal
            .proxy
            .tailscale_candidate_preflight(normal.saved, normal.candidate)
        {
            return StartLoginOutcome::Failed(error);
        }
        let _lease = match normal.proxy.tailscale_action_lease() {
            Ok(lease) => lease,
            Err(error) => return StartLoginOutcome::Failed(error),
        };
        #[cfg(target_os = "ios")]
        let action_generation = match normal
            .proxy
            .prepare_tailscale_action_origin(normal.action_generation, attempt)
            .await
        {
            Ok(generation) => generation,
            Err(error) => {
                return if attempt.cancelled() {
                    StartLoginOutcome::Cancelled
                } else {
                    StartLoginOutcome::Failed(error)
                }
            }
        };
        #[cfg(not(target_os = "ios"))]
        let action_generation = normal.action_generation;
        let binding = match ActionBinding::new(
            NormalMainAction::TailscaleLogin,
            request.attempt_id.clone(),
            normal.saved,
            vec![requested.id.clone()],
            normal.identity_epoch,
        ) {
            Ok(binding) => binding.for_attempt(action_generation, Arc::clone(attempt)),
            Err(error) => return prerequisite_failure(error),
        };
        emitter.progress(
            &requested.id,
            &request.attempt_id,
            "preparingConnection",
            None,
            None,
        );
        let old = tokio::select! {
            biased;
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            ready = tokio::time::timeout(self.timeout, normal.proxy.await_normal_main(binding)) => match ready {
                Ok(Ok(ticket)) => ticket,
                Ok(Err(error)) => return prerequisite_failure(error),
                Err(_) => { attempt.cancel(); return StartLoginOutcome::Failed("authorizationTimedOut".into()); }
            }
        };
        // Keep old ownership in the immutable ticket. Presentation binds only
        // the fresh Ready generation, so existing once-bound observers stay exact.
        emitter.progress(
            &requested.id,
            &request.attempt_id,
            "stoppingConnection",
            None,
            None,
        );
        // Stop is awaited to completion even if cancellation arrives. Its scoped
        // owner remains booked; the synchronous commit and next Start check cancel.
        emitter.progress(
            &requested.id,
            &request.attempt_id,
            "retiringIdentity",
            None,
            None,
        );
        let (generation, saved) = match normal
            .proxy
            .retire_tailscale_account(
                &old,
                &requested.id,
                attempt,
                normal.saved,
                Some(normal.candidate),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                return if attempt.cancelled() {
                    StartLoginOutcome::Cancelled
                } else {
                    StartLoginOutcome::Failed(error)
                }
            }
        };
        let Some(saved) = saved else {
            return StartLoginOutcome::Failed("candidateConfigurationChanged".into());
        };
        emitter.progress(
            &requested.id,
            &request.attempt_id,
            "savingCandidate",
            None,
            None,
        );
        let epoch = match saved_tailscale_identity_epoch(&saved, &requested.id) {
            Ok(epoch) => epoch,
            Err(error) => return StartLoginOutcome::Failed(error),
        };
        let saved_server = || {
            let current = normal.proxy.tailscale_action_saved_config()?;
            if current != saved {
                return Err("candidateConfigurationChanged".into());
            }
            let nodes: Vec<_> = current
                .get("servers")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter(|node| {
                    node.get("id").and_then(serde_json::Value::as_str) == Some(&requested.id)
                })
                .collect();
            let [node] = nodes.as_slice() else {
                return Err("candidateConfigurationChanged".into());
            };
            serde_json::from_value::<ServerConfig>((*node).clone())
                .map_err(|_| "candidateConfigurationChanged".into())
        };
        let candidate = match saved_server() {
            Ok(candidate) => candidate,
            Err(error) => return StartLoginOutcome::Failed(error),
        };
        let mut fresh = request.clone();
        fresh.replace_identity = false;
        emitter.progress(
            &requested.id,
            &request.attempt_id,
            "startingConnection",
            None,
            None,
        );
        Box::pin(self.launch_normal_main_attempt(
            &candidate,
            &fresh,
            attempt,
            &saved_server,
            main_core,
            emitter,
            NormalMainLoginInput {
                proxy: normal.proxy,
                saved: &saved,
                identity_epoch: epoch,
                candidate: normal.candidate,
                action_generation: generation,
            },
        ))
        .await
    }

    /// Observe a fresh target stream until authorization or an exact request terminal. Its
    /// cancellation and deadline close only this subscription; the normal connection persists.
    #[allow(
        clippy::too_many_arguments,
        reason = "observer retains independent request and main authorities"
    )]
    async fn confirm_main_request(
        &self,
        server: &ServerConfig,
        request: &LoginRequest,
        attempt: &Arc<Attempt>,
        main: &MainLoginSnapshot,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        binding: Option<&dyn MainLoginBinding>,
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
        if binding.is_some_and(|binding| binding.target_tag(&server.id) != Some(tag.as_str())) {
            return StartLoginOutcome::Failed("mainCoreChanged".into());
        }
        if attempt.main_context().is_none() {
            attempt.bind_main(main.generation, None);
        }
        emitter.progress(
            &server.id,
            &request.attempt_id,
            "mainCore",
            Some("confirmingMainCore"),
            None,
        );
        let query = async {
            self.check_main_observer(server, request.mode, main, main_core, saved_server, binding)
                .await?;
            let mut stream = self
                .subscriber
                .subscribe(main.api_port, &main.api_secret)
                .await
                .map_err(|_| "statusSubscriptionFailed".to_owned())?;
            let mut check = tokio::time::interval(Duration::from_millis(250));
            check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let frame = tokio::select! {
                    biased;
                    () = attempt.cancellation() => return Err("cancelled".to_owned()),
                    frame = stream.recv() => frame.ok_or_else(|| "statusStreamEnded".to_owned())?,
                    _ = check.tick() => {
                        self.check_main_observer(server, request.mode, main, main_core, saved_server, binding).await?;
                        continue;
                    },
                };
                self.check_main_observer(
                    server,
                    request.mode,
                    main,
                    main_core,
                    saved_server,
                    binding,
                )
                .await?;
                let endpoint = frame
                    .endpoints
                    .iter()
                    .find(|ep| ep.endpoint_tag == tag)
                    .ok_or_else(|| "statusSubscriptionFailed".to_owned())?;
                let expired = endpoint.self_.as_ref().is_some_and(|peer| peer.expired);
                let authorized = endpoint.backend_state == "Running" && !expired;
                let url = if authorized
                    || endpoint.auth_url.is_empty()
                    || (endpoint.backend_state != "NeedsLogin" && !expired)
                {
                    None
                } else {
                    Some(
                        crate::runtime::tailscale_status::validated_tailscale_auth_url(
                            &endpoint.auth_url,
                        )
                        .ok_or_else(|| "invalidAuthUrl".to_owned())?,
                    )
                };
                // The final synchronous checks and receipt emission have no await between them.
                self.check_main_observer_snapshot(
                    server,
                    request.mode,
                    main,
                    main_core,
                    saved_server,
                )?;
                if attempt.cancelled() {
                    return Err("cancelled".into());
                }
                emitter.progress(
                    &server.id,
                    &request.attempt_id,
                    if authorized { "authorized" } else { "mainCore" },
                    None,
                    if authorized { None } else { url.as_deref() },
                );
                if authorized {
                    return Ok(());
                }
            }
        };
        let result = tokio::select! {
            biased;
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            result = tokio::time::timeout(self.timeout, query) => result,
        };
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
        }
        match result {
            Ok(Ok(())) => {
                attempt.finish();
                StartLoginOutcome::InMainCore
            }
            Ok(Err(reason)) => StartLoginOutcome::Failed(reason),
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

    fn check_main_observer_snapshot(
        &self,
        server: &ServerConfig,
        mode: LoginMode,
        main: &MainLoginSnapshot,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
    ) -> Result<(), String> {
        let current = main_core();
        if !current.alive
            || current.generation != main.generation
            || current.api_port != main.api_port
            || current.api_secret != main.api_secret
            || !self.main_matches_request(server, mode)
        {
            return Err("mainCoreChanged".into());
        }
        if !saved_server().is_ok_and(|saved| {
            saved.id == server.id
                && saved.protocol == server.protocol
                && saved.tailscale_settings == server.tailscale_settings
        }) {
            return Err("savedTailscaleIdentityChanged".into());
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "fresh native binding supplements the saved request checks"
    )]
    async fn check_main_observer(
        &self,
        server: &ServerConfig,
        mode: LoginMode,
        main: &MainLoginSnapshot,
        main_core: &(dyn Fn() -> MainLoginSnapshot + Send + Sync),
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        binding: Option<&dyn MainLoginBinding>,
    ) -> Result<(), String> {
        self.check_main_observer_snapshot(server, mode, main, main_core, saved_server)?;
        if let Some(binding) = binding {
            binding.validate().await?;
        }
        self.check_main_observer_snapshot(server, mode, main, main_core, saved_server)
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
        producer_origin: Option<(&Arc<ProxyRuntime>, u64)>,
        #[cfg(target_os = "android")] mut warm_activity: Option<
            &mut Option<attempts::AndroidActionActivity>,
        >,
    ) -> StartLoginOutcome {
        #[cfg(target_os = "android")]
        let warm = warm_activity.is_some();
        #[cfg(not(target_os = "android"))]
        let warm = false;
        let _start_guard = tokio::select! {
            () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
            guard = self.start_gate.lock() => guard,
        };
        if self.closing.load(Ordering::SeqCst) {
            return StartLoginOutcome::Cancelled;
        }
        if attempt.cancelled() {
            return StartLoginOutcome::Cancelled;
        }
        if let Err(error) =
            polaris_core_supervisor::config_gate::assert_check_producer_registration()
        {
            return StartLoginOutcome::Failed(error.to_string());
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
            if warm {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            }
            if !main.alive {
                return StartLoginOutcome::InMainCorePending;
            }
            if !self.main_matches_request(&server, request.mode) {
                return StartLoginOutcome::InMainCorePending;
            }
            // Keeping this gate through a browser login would block normal Stop/retirement.
            drop(_start_guard);
            return self
                .confirm_main_request(
                    &server,
                    request,
                    attempt,
                    &main,
                    main_core,
                    saved_server,
                    None,
                    emitter,
                )
                .await;
        }
        if !self.shared.can_start(&server.id) {
            return StartLoginOutcome::Failed("tooManyLogins".into());
        }
        if let Err(reason) = self.sweep_stale_login_cores().await {
            return StartLoginOutcome::Failed(reason);
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
        let config_path = login_config_path(user_data, &server.id, epoch);
        let bytes = match serde_json::to_vec_pretty(&json_cfg) {
            Ok(b) => b,
            Err(_) => return StartLoginOutcome::Failed("configWriteFailed".into()),
        };
        // A config left by a killed host process would collide with this process's restarted
        // epoch numbering. Every registered login's config is excluded by its exact path.
        let live_configs: Vec<PathBuf> = self
            .shared
            .guard()
            .iter()
            .map(|(id, entry)| login_config_path(user_data, id, entry.epoch))
            .collect();
        stale::sweep_stale_login_configs(user_data, &live_configs);
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
        #[cfg(target_os = "android")]
        if warm {
            use crate::runtime::proxy::android_bridge::tailscale_store;
            use polaris_core_supervisor::config_gate::ConfigCheckVerdict;
            // A single original native validation captures the exact bytes that
            // the unchanged spawner subsequently reads. Cancellation does not
            // drop an invocation that may already be disposing SDK state.
            let config = match std::str::from_utf8(&bytes) {
                Ok(config) => config,
                Err(_) => return StartLoginOutcome::Failed("configurationCheckFailed".into()),
            };
            let check = match tailscale_store::check_config_for_tailscale(config).await {
                Ok(check) => check,
                Err(error) => return StartLoginOutcome::AndroidCapacityClosed(error),
            };
            if check.verdict() != &ConfigCheckVerdict::Accepted {
                return StartLoginOutcome::Failed("configurationCheckFailed".into());
            }
            let Some(validation) = check.custody() else {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            };
            let Some(logical_id) = config_path.file_stem().and_then(|stem| stem.to_str()) else {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            };
            let Some((proxy, generation)) = producer_origin else {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            };
            let state_file = match proxy.android_tailscale_auth_directory(&server.id) {
                Ok(directory) => directory.join("tailscaled.state"),
                Err(error) => return StartLoginOutcome::Failed(error),
            };
            let Some(state_file) = state_file.to_str() else {
                return StartLoginOutcome::Failed("nativeRetirementUnknown".into());
            };
            let action_id = format!("ts-warm-{epoch}");
            let tuple = match tailscale_store::make_warm_tuple(
                validation, state_file, &action_id, logical_id,
            ) {
                Ok(tuple)
                    if tuple.config_digest() == polaris_updater::sha256_hex(&bytes)
                        && tuple.logical_instance_id() == logical_id =>
                {
                    tuple
                }
                _ => return StartLoginOutcome::Failed("nativeRetirementUnknown".into()),
            };
            let action = Arc::new(attempts::AndroidTargetAction {
                state_file: state_file.to_owned(),
                action_id,
                active: AtomicBool::new(true),
                original: attempts::AndroidActionOrigin::Warm(tuple.clone()),
            });
            let admitted = proxy.with_tailscale_credential_birth(generation, attempt, || {
                attempt.reserve_android_action(action.clone())?;
                // The caller retains activity through close and FS/CAS, including
                // all await/error paths. Dropping it leaves the original tuple reachable.
                if let Some(activity) = warm_activity.as_mut() {
                    **activity = Some(attempts::AndroidActionActivity(action));
                }
                Ok::<(), String>(())
            });
            match admitted {
                Some(Ok(())) => {}
                Some(Err(error)) => return StartLoginOutcome::Failed(error),
                None => return StartLoginOutcome::Cancelled,
            }
            // No cancellation early-return between booking and this original
            // begin: even a lost reply remains recoverable through that tuple.
            if let Err(error) = tailscale_store::begin_warm(&tuple).await {
                return StartLoginOutcome::Failed(error);
            }
        }
        if !warm {
            tokio::select! {
                () = attempt.cancellation() => return StartLoginOutcome::Cancelled,
                result = self.checker.check_for_spawn(&binary, &config_path) => match result {
                    Ok(()) => {},
                    Err(ConfigCheckFailure::AndroidCapacityClosed(error)) => return StartLoginOutcome::AndroidCapacityClosed(error),
                    Err(_) => return StartLoginOutcome::Failed("configurationCheckFailed".into()),
                },
            }
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
        let prepared = self.spawner.prepare_temp_native_birth();
        let native = match prepared
            .as_ref()
            .map(|prepared| prepared.bind_login(&self.identity, epoch))
            .transpose()
        {
            Ok(native) => native,
            Err(_) => return StartLoginOutcome::Failed("processStartFailed".into()),
        };
        let drain = native.as_ref().map(|_| LoginStdioDrain::new());
        let log_drain = drain.clone().unwrap_or_else(LoginStdioDrain::new);
        let mut req = SpawnRequest::new(
            &binary,
            &config_path,
            StdioPolicy::drain(move |stdout, stderr| {
                let stdout = LoginDrainReader::new(stdout, log_drain.clone(), 0);
                let stderr = LoginDrainReader::new(stderr, log_drain.clone(), 1);
                let out = pipe_to_log_with_secrets_owned(
                    stdout,
                    LOGIN_CORE_LOG_TARGET,
                    None,
                    None,
                    secrets.clone(),
                );
                let err = pipe_to_log_with_secrets_owned(
                    stderr,
                    LOGIN_CORE_LOG_TARGET,
                    None,
                    None,
                    secrets,
                );
                log_drain.install([out, err]);
            }),
        );
        req.extra_args = vec!["--disable-color".to_string()];
        req.working_dir = Some(user_data.to_path_buf());
        if self.closing.load(Ordering::SeqCst) {
            return StartLoginOutcome::Cancelled;
        }
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (closed_tx, closed_rx) = watch::channel(None);
        let mut pending_entry = native.as_ref().map(|native| LoginEntry {
            epoch,
            attempt_id: request.attempt_id.clone(),
            pid: None,
            cancel_tx: cancel_tx.clone(),
            closed_rx: closed_rx.clone(),
            _child: None,
            native: Some(native.clone()),
            drain: drain.clone(),
            config_path: Some(config_path.clone()),
            #[cfg(target_os = "android")]
            android_instance: None,
            #[cfg(target_os = "android")]
            android_authority: None,
        });
        let mut book_pending = || {
            if let Some(entry) = pending_entry.take() {
                self.shared.insert(server.id.clone(), entry);
                config_guard.disarm();
                attempt.process_owned.store(true, Ordering::SeqCst);
            }
        };
        let spawned = match producer_origin {
            None => {
                let mut pending = self.spawner.spawn_with_temp_native_birth(req, prepared);
                std::future::poll_fn(|context| {
                    book_pending();
                    pending.as_mut().poll(context)
                })
                .await
            }
            Some((proxy, generation)) => {
                let mut pending = self.spawner.spawn_with_temp_native_birth(req, prepared);
                let mut admitted = false;
                let result = std::future::poll_fn(|context| {
                    if !admitted {
                        let Some(result) =
                            proxy.with_tailscale_credential_birth(generation, attempt, || {
                                book_pending();
                                pending.as_mut().poll(context)
                            })
                        else {
                            return std::task::Poll::Ready(None);
                        };
                        admitted = true;
                        return result.map(Some);
                    }
                    // Pending may already own a desktop/native resource. Join this exact
                    // future; cancellation does not turn dropping it into terminal evidence.
                    if proxy.core_generation() != generation {
                        attempt.cancel();
                    }
                    pending.as_mut().poll(context).map(Some)
                })
                .await;
                let Some(result) = result else {
                    attempt.cancel();
                    return StartLoginOutcome::Cancelled;
                };
                // Even a late Child must be published without an intervening await below.
                if proxy.core_generation() != generation {
                    attempt.cancel();
                }
                result
            }
        };
        let child = match spawned {
            Ok(c) => c,
            Err(error) => {
                if let Some(native) = native {
                    let shared = self.shared.clone();
                    let server_id = server.id.clone();
                    let attempt = attempt.clone();
                    let mut cancel_rx = cancel_rx;
                    tokio::spawn(async move {
                        loop {
                            let result = shared.retire_login_tail(&server_id, epoch, &native, None);
                            let complete = result.is_ok();
                            closed_tx.send_replace(Some(result));
                            if complete {
                                attempt.process_owned.store(false, Ordering::SeqCst);
                                attempt.finish();
                                break;
                            }
                            tokio::select! {
                                _ = cancel_rx.recv() => {},
                                () = tokio::time::sleep(Duration::from_secs(5)) => {},
                            }
                        }
                    });
                }
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
        let pid = child.pid();
        #[cfg(target_os = "android")]
        let android_instance = child
            .android_tailscale_instance()
            .map(|(id, digest)| (id.to_owned(), digest.to_owned()));
        #[cfg(target_os = "android")]
        let android_authority = canonical_login_authority(&server).ok();
        // No await between a successful spawn and custody publication. The supervisor only
        // borrows this child; dropping its task cannot erase the registry's physical owner.
        let child = Arc::new(tokio::sync::Mutex::new(child));
        if let Some(native) = &native {
            native.publish_child(child.clone());
            let mut entries = self.shared.guard();
            let entry = entries
                .get_mut(&server.id)
                .expect("original Login pending entry retained");
            assert!(
                entry.epoch == epoch
                    && entry
                        .native
                        .as_ref()
                        .is_some_and(|original| original.same(native)),
                "original Login pending birth retained"
            );
            entry.pid = pid;
            entry._child = Some(child.clone());
            #[cfg(target_os = "android")]
            {
                entry.android_instance = android_instance.clone();
                entry.android_authority = android_authority;
            }
        } else {
            self.shared.insert(
                server.id.clone(),
                LoginEntry {
                    epoch,
                    attempt_id: request.attempt_id.clone(),
                    pid,
                    cancel_tx,
                    closed_rx,
                    _child: Some(child.clone()),
                    native: None,
                    drain: None,
                    config_path: None,
                    #[cfg(target_os = "android")]
                    android_instance: android_instance.clone(),
                    #[cfg(target_os = "android")]
                    android_authority,
                },
            );
        }
        // Shutdown may have observed an empty table while spawn was pending. Publication
        // compensates synchronously for that exact birth before awaiting readiness.
        if self.closing.load(Ordering::SeqCst) {
            if let Some(entry) = self
                .shared
                .guard()
                .get(&server.id)
                .filter(|e| e.epoch == epoch)
            {
                let _ = entry.cancel_tx.send(());
            }
        }
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
            warm,
            #[cfg(target_os = "android")]
            android_instance,
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

#[cfg(target_os = "android")]
async fn capture_android_login_store(
    attempt: &Arc<Attempt>,
    instance: Option<&(String, String)>,
) -> Result<(), String> {
    let (id, digest) = instance.ok_or("nativeRetirementUnknown")?;
    let original =
        crate::runtime::proxy::android_bridge::tailscale_store::observe_login(id, digest).await?;
    attempt.record_android_store(original)
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
    /// A reserved cold observation uses the same Child supervisor to close,
    /// without subscribing or completing the account action as a login.
    warm: bool,
    #[cfg(target_os = "android")]
    android_instance: Option<(String, String)>,
}

/// 瞬态登录核退出原因。
enum ExitReason {
    /// 核自然退出（无需主动 kill，直接 reap）。
    SelfExit,
    WaitFailed(String),
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

async fn finish_login_tail(
    ctx: &SuperviseCtx,
    fact: Option<NativeTransientExit>,
) -> Result<(), String> {
    let native_tail = {
        let entries = ctx
            .shared
            .entries
            .lock()
            .map_err(|_| "登录核注册表不可用")?;
        let entry = entries
            .get(&ctx.server_id)
            .filter(|entry| entry.epoch == ctx.epoch)
            .ok_or("登录核关闭回执不属于当前代次")?;
        entry
            .native
            .clone()
            .map(|native| (native, entry.drain.clone()))
    };
    if let Some((native, drain)) = native_tail {
        native.validate_exit(&ctx.shared.identity, ctx.epoch, fact.clone())?;
        drain.ok_or("登录核 stdio custody 缺失")?.finished().await?;
        ctx.shared
            .retire_login_tail(&ctx.server_id, ctx.epoch, &native, fact)
    } else {
        remove_login_config(&ctx.config_path);
        ctx.shared.remove_if_epoch(&ctx.server_id, ctx.epoch);
        Ok(())
    }
}

async fn subscribe_and_supervise(
    ctx: SuperviseCtx,
    child: LoginChildCustody,
    subscriber: Arc<dyn LoginStatusSubscriber>,
    api: TailscaleLoginApiService,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
    ready: oneshot::Sender<StartLoginOutcome>,
) {
    #[cfg(target_os = "android")]
    if ctx.warm || ctx.attempt.credential_activation().is_some() {
        // Publication already owns this exact Child. A failed observation keeps
        // its scoped state Unknown without dropping the child or signing NoCtor.
        let _ = capture_android_login_store(&ctx.attempt, ctx.android_instance.as_ref()).await;
    }
    let result = if ctx.warm {
        Err(("warm", "nativeRetirementUnknown"))
    } else {
        tokio::select! {
            biased;
            () = ctx.attempt.cancellation() => Err(("cancelled", "cancelled")),
            _ = cancel_rx.recv() => Err(("cancelled", "cancelled")),
            () = tokio::time::sleep_until(ctx.deadline) => Err(("timedOut", "authorizationTimedOut")),
            result = subscriber.subscribe(api.port, &api.secret) => result.map_err(|_| ("failed", "statusSubscriptionFailed")),
        }
    };
    match result {
        Ok(status) => {
            let _ = ready.send(StartLoginOutcome::Started);
            supervise(ctx, child, status, cancel_rx).await;
        }
        Err((phase, reason)) => {
            let mut child = child.lock().await;
            loop {
                let result = match child.close_confirmed().await {
                    Ok(()) => finish_login_tail(&ctx, child.native_exit()).await,
                    Err(error) => Err(error),
                };
                if result.is_ok() {
                    break;
                }
                let _ = ctx.closed_tx.send(Some(result));
                // A failed close retains the exact child, registry entry and state claim. Another
                // cancel retries promptly; otherwise a bounded delay avoids a hot loop.
                tokio::select! {
                    _ = cancel_rx.recv() => {},
                    () = tokio::time::sleep(Duration::from_secs(5)) => {},
                }
            }
            let _ = ctx.closed_tx.send(Some(Ok(())));
            if !ctx.warm {
                ctx.emitter
                    .progress(&ctx.server_id, &ctx.attempt_id, phase, Some(reason), None);
                ctx.attempt.finish();
            } else {
                ctx.attempt.process_owned.store(false, Ordering::SeqCst);
                if ctx.attempt.cancelled() {
                    // Only request completion. The original action token still
                    // keeps any unconfirmed native reservation occupied.
                    ctx.attempt.finish();
                }
            }
            let outcome = if ctx.warm {
                // This only joins the original Child close. The caller must
                // still consume held_retirement before touching FS or CAS.
                StartLoginOutcome::Started
            } else if phase == "cancelled" {
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
    child: LoginChildCustody,
    mut status: Box<dyn LoginStatusStream>,
    mut cancel_rx: mpsc::UnboundedReceiver<()>,
) {
    let mut child = child.lock().await;
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
            result = child.wait_result() => break match result {
                Ok(()) => ExitReason::SelfExit,
                Err(error) => ExitReason::WaitFailed(error),
            },
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
        ExitReason::WaitFailed(ref error) => {
            log::error!(
                "瞬态登录核退出等待失败，保留占用并请求关闭：server={} {error}",
                ctx.server_id
            );
            let _ = ctx.closed_tx.send(Some(Err(error.clone())));
        }
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
        let result = match result {
            Ok(()) => finish_login_tail(&ctx, child.native_exit()).await,
            Err(error) => Err(error),
        };
        if result.is_ok() {
            break;
        }
        let _ = ctx.closed_tx.send(Some(result));
        // A failed close keeps both the exact child and its claim; cancellation/relogin can retry.
        tokio::select! {
            _ = cancel_rx.recv() => {},
            () = tokio::time::sleep(Duration::from_secs(5)) => {},
        }
    }
    // Native close and the original stdio/config tail committed before custody removal.
    let _ = ctx.closed_tx.send(Some(Ok(())));
    let (phase, reason) = match reason {
        ExitReason::LoggedIn => ("authorized", None),
        ExitReason::Cancelled => ("cancelled", None),
        ExitReason::TimedOut => ("timedOut", Some("authorizationTimedOut")),
        ExitReason::SelfExit => ("failed", Some("processExited")),
        ExitReason::WaitFailed(_) => ("failed", Some("processWaitFailed")),
        ExitReason::StatusStreamEnded => ("failed", Some("statusStreamEnded")),
        ExitReason::InvalidAuthUrl => ("failed", Some("invalidAuthUrl")),
    };
    ctx.emitter
        .progress(&ctx.server_id, &ctx.attempt_id, phase, reason, None);
    ctx.attempt.finish();
}

/// 删掉本次登录写盘的临时 config（内含一次性管理 API secret）。best-effort：
/// 已被 kill-on-relogin 的新一代覆写、或早被删掉，都不是问题——本函数只保证「核死了就不留凭据」。
fn remove_login_config_confirmed(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("登录核配置清理未确认".into()),
    }
}

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

/// One login epoch's temporary config. The epoch makes each file single-owner.
fn login_config_path(user_data: &Path, server_id: &str, epoch: u64) -> PathBuf {
    user_data.join(format!(
        "tailscale-login-{}-{epoch}.json",
        sanitize_id(server_id)
    ))
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

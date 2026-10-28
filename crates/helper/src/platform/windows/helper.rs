//! 协议分派核心（移植自 `helper-win/helper.go` 的 `handle()` switch 骨架）。
//!
//! ## 设计
//!
//! Go 源 `handle(conn)` 读 token+command+args 行 → `mu.Lock()` → switch command → 回复。本 Rust 实现把
//! 「读帧」与「处理」解耦：调用方（`service/win.rs` 的 serve 循环）只负责读回一整帧、把结果写出去；
//! 本模块的 [`WinHelper::handle_frame`] 负责切行 → 鉴权 → 解码为 [`polaris_helper_proto::Request`]
//! → [`WinHelper::handle`] 的 switch 分派 + 子进程状态管理 + 回复构造。这样：
//! - 分派逻辑（=Go switch 主体）跨平台纯逻辑，Linux 可单测。
//! - IO（命名管道读写）由 `service.rs` 承接（`#[cfg(windows)]`）。
//!
//! Native births keep their exact owned HANDLE in ProcOps and their target in
//! ChildState until native wait succeeds. Lock order is ChildState→owned HANDLE;
//! synchronous waits are bounded and no detached worker takes sole custody.
//! Terminal receipts are cached per birth. Legacy Start is rejected before any
//! forwarding/spawn; legacy Stop cannot detach native custody.

use crate::platform::windows::logic;
use crate::platform::windows::ops::{NetTableOps, ProcOps};
mod native_birth;
use crate::token::{is_authed_constant_time, TokenStore};
use native_birth::{stop_locked, NativeCustody};
use polaris_helper_proto::{Request, Response, ResponseKind};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Windows helper 错误（helper 内部分派/状态错误，非协议错误码）。
#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    /// sing-box 启动失败（对应 Go `ERR start %v`，详情保留 io 错误）。
    #[error("sing-box start failed: {0}")]
    Start(String),
    /// LISTEN 持有者枚举失败（对应 Go `ERR enum %v`）。
    #[error("enum listen table failed: {0}")]
    Enum(String),
}

/// 子进程状态（Go `child` + `childDone` 的 Rust 等价）。
#[derive(Debug, Default, Clone)]
struct ChildState {
    /// 当前 child sing-box 的 pid（None = 无 child）。
    pid: Option<u32>,
    closing: bool,
    native: Option<NativeCustody>,
    terminal: std::collections::VecDeque<(
        polaris_helper_proto::HelperBirthTarget,
        Option<polaris_helper_proto::core_protocol::CallerIdentity>,
    )>,
}

/// Windows helper 核心分派器（移植自 `helper.go` 的 `handle()` + 模块级 `mu/child/childDone`）。
///
/// 持有：
/// - `child_mu`：保护 `child` 状态（Go `mu sync.Mutex`）。
/// - `child`：当前 child sing-box pid（Go `child *exec.Cmd`）。
/// - `token`：token 存储（鉴权）。
/// - `proc`：进程操作（spawn/kill/reap/job）。
/// - `net`：网络表操作（freeport）。
/// - `singbox_bin`：安装时锁定的 sing-box 路径（Go `singboxBin`）。
/// - `conf_dir`：允许的配置目录（Go `confDir`）。
/// - `support_dir`：受保护核目录；卸载只使用 cleaner 的固定 OS 范围。
pub struct WinHelper<T, P, N> {
    /// `Arc<Mutex<..>>`：child 状态须在**父死看护后台线程**（W15）与管道命令线程间共享（Go 的
    /// 包级 `mu`/`child` 全局，goroutine 直接引用；Rust 用 Arc 共享所有权）。
    child_mu: Arc<Mutex<ChildState>>,
    // Admission and caller-scoped network writes commit under one reservation.
    control_mu: Mutex<()>,
    dns_admission: crate::platform::windows::ops::DnsFlushAdmission,
    token: T,
    /// `Arc<P>`：父死看护的 `on_parent_dead` 闭包（后台线程）须持 proc 收割 child，故需共享所有权。
    proc: Arc<P>,
    net: N,
    support_dir: String,
    #[cfg(test)]
    test_context: Option<TestNativeContext>,
}

#[cfg(test)]
pub(super) struct TestNativeContext {
    pub peer: crate::user_control::NativePeer,
    pub core: Arc<crate::core_store::VerifiedCore>,
    pub config: String,
    pub _directory: tempfile::TempDir,
}

/// 响应写完后要不要等对端把它读走（= 要不要 `FlushFileBuffers`）。
///
/// 命名管道服务端在 `DisconnectNamedPipe` 前不 flush，client 尚未读走的字节会被丢弃
/// （稳定复现 ERROR_PIPE_NOT_CONNECTED(233)）—— 所以正常响应必须等。但「等对端」这件事本身
/// 是可被滥用的：等多久完全由对端决定。故按**对端是否已鉴权**分两档。
///
/// 住在本模块（跨平台）而非 `service/win.rs`：档位由 [`WinHelper::handle_frame`] 决定，那是
/// Linux 上可测的判据；`service/win.rs` 只照档位执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushMode {
    /// 已鉴权的响应（含已鉴权但命令未知的 `ERR unknown`）：等对端读走（仍受 service 层
    /// `IoTimeoutGuard` 的 5s 上界约束）。
    WaitPeer,
    /// 鉴权失败 / 帧不合法：**不等对端**。这些响应的收件人恰恰是「还没证明自己是谁」
    /// 的进程，不能让它用「发一帧、不读」把 SYSTEM 服务的线程按住。代价：这条错误行可能被随后的
    /// `DisconnectNamedPipe` 丢弃，合法但 token 过期的 client 侧表现为管道断开而非 `ERR auth`
    /// （两者都进同一条「本次调用失败」分支，不改变 app 的处置）。
    NoWait,
}

/// 一帧的处理结果：要写回的整行（含 `\n`）+ flush 档位 + 写完后是否自退（uninstall）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameReply {
    /// 写回管道的响应行（含行尾 `\n`）。
    pub line: String,
    /// 写完后要不要等对端读走。
    pub flush: FlushMode,
    /// 写完并断开后 800ms 退出进程（Go `uninstall` 分支，helper.go:291-294）。
    pub exit_after: bool,
}

impl FrameReply {
    fn no_wait(line: String) -> Self {
        Self {
            line,
            flush: FlushMode::NoWait,
            exit_after: false,
        }
    }

    fn from_outcome(outcome: HandleOutcome) -> Self {
        match outcome {
            // 鉴权失败：对端未证明身份，不给它「按住服务线程」的机会。
            HandleOutcome::AuthFailed => Self::no_wait(format!(
                "{}\n",
                Response::Err(polaris_helper_proto::Error::new(
                    polaris_helper_proto::ErrorCode::Auth
                ))
                .to_wire_line()
            )),
            HandleOutcome::Respond(resp) => Self {
                line: format!("{}\n", resp.to_wire_line()),
                flush: FlushMode::WaitPeer,
                exit_after: false,
            },
            HandleOutcome::UninstallAndExit(resp) => Self {
                line: format!("{}\n", resp.to_wire_line()),
                flush: FlushMode::WaitPeer,
                exit_after: true,
            },
        }
    }
}

/// `ERR unknown\n`：已鉴权但命令解不出。**必须是裸的一行** —— 客户端据此判「这个 helper 不认识
/// 该命令」，带了尾文就不算。
fn unknown_line() -> String {
    format!(
        "{}\n",
        Response::Err(polaris_helper_proto::Error::new(
            polaris_helper_proto::ErrorCode::Unknown
        ))
        .to_wire_line()
    )
}

/// `ERR unknown bad-frame\n`：帧行数不足，连 token 与命令都凑不齐。带尾文，与上面那一行分开：
/// 这不是「不认识命令」，客户端不得据此判 helper 过旧。
fn bad_frame_line() -> String {
    format!(
        "{}\n",
        Response::Err(polaris_helper_proto::Error::with_detail(
            polaris_helper_proto::ErrorCode::Unknown,
            "bad-frame",
        ))
        .to_wire_line()
    )
}

/// 单次 handle 的结果（成功响应，或鉴权失败/其它需提前终止的情形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandleOutcome {
    /// 正常响应（已成功处理，回此 Response 给客户端）。
    Respond(Response),
    /// 鉴权失败（Go `ERR auth` + return，不进 switch）。
    AuthFailed,
    /// uninstall 后自退（Go `uninstall` 分支：回 OK uninstalling → 800ms 后 os.Exit(0)）。
    /// 调用方（serve 循环）据此触发进程退出。
    UninstallAndExit(Response),
}

impl<T, P, N> WinHelper<T, P, N>
where
    T: TokenStore,
    P: ProcOps + 'static, // 'static：父死看护闭包捕获 Arc<P> 进 'static 后台线程（W15）。
    N: NetTableOps,
{
    /// 构造。
    #[must_use]
    pub fn new(token: T, proc: P, net: N, support_dir: impl Into<String>) -> Self {
        Self {
            child_mu: Arc::new(Mutex::new(ChildState::default())),
            control_mu: Mutex::new(()),
            dns_admission: crate::platform::windows::ops::DnsFlushAdmission::default(),
            token,
            proc: Arc::new(proc),
            net,
            support_dir: support_dir.into(),
            #[cfg(test)]
            test_context: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_native_fixture(mut self, context: TestNativeContext) -> Self {
        self.test_context = Some(context);
        self
    }

    /// 处理一个已解析的 [`Request`]（移植自 `helper.go:161-393` 的 `handle()`）。
    ///
    /// # 参数
    ///
    /// - `client_token`：客户端发来的 token 行（已 trim `\r\n`）。
    /// - `req`：解析后的请求。
    ///
    /// # 返回
    ///
    /// [`HandleOutcome`]，调用方据此回复 + 决定是否退出。
    #[must_use]
    pub fn handle(&self, client_token: &str, req: Request) -> HandleOutcome {
        // Go helper.go:169: if tok == "" || tok != tokenValue() { ERR auth; return }
        if !self.is_authed(client_token) {
            return HandleOutcome::AuthFailed;
        }
        #[cfg(test)]
        if let Some(context) = &self.test_context {
            return self.dispatch(req, Some(&context.peer));
        }
        self.dispatch(req, None)
    }

    /// 处理命名管道读回的**一整帧**（切行 → 验 token → 解码 → 分派），产出要写回的那一行。
    ///
    /// `service/win.rs` 的 `handle_connection` 只负责读帧与把 [`FrameReply`] 写出去；顺序判据全在
    /// 这里，好让它在 Linux 上有门可跑。顺序：
    ///
    /// 1. 帧行数不足（连 token/命令都没有）⇒ `ERR unknown bad-frame` + [`FlushMode::NoWait`]（对端连身份都没报）。
    /// 2. **先验 token** ⇒ 不合法回 `ERR auth` + `NoWait`，**不看命令**：对已知 / 未知 / 参数解不出的
    ///    命令一律同一行，未鉴权对端探测不出 helper 认识哪些命令。
    /// 3. 已鉴权而命令解不出 ⇒ `ERR unknown` + [`FlushMode::WaitPeer`]：收件人已证明身份，这行必须
    ///    送达 —— 它是 app 判「这个 helper 不支持某命令」的唯一依据（旧顺序在验 token 之前 NoWait
    ///    回它，DisconnectNamedPipe 常把它丢掉，app 只看到 0 字节 / 233）。
    /// 4. 分派。
    #[must_use]
    pub fn handle_frame(&self, raw: &str) -> FrameReply {
        self.handle_frame_from_peer(raw, None)
    }

    pub fn handle_frame_from_peer(
        &self,
        raw: &str,
        peer: Option<&crate::user_control::NativePeer>,
    ) -> FrameReply {
        #[cfg(test)]
        let peer = peer.or_else(|| self.test_context.as_ref().map(|context| &context.peer));
        let Some(frame) = logic::split_frame(raw) else {
            return FrameReply::no_wait(bad_frame_line());
        };
        let authed = match peer {
            Some(peer) if peer.administrator => true,
            Some(peer) => self
                .registered_token(peer)
                .is_ok_and(|token| is_authed_constant_time(frame.token, &token)),
            None => self.is_authed(frame.token),
        };
        if !authed {
            return FrameReply::from_outcome(HandleOutcome::AuthFailed);
        }
        let Some(req) = logic::parse_request(frame.command, &frame.args) else {
            return FrameReply {
                line: unknown_line(),
                flush: FlushMode::WaitPeer,
                exit_after: false,
            };
        };
        FrameReply::from_outcome(self.dispatch(req, peer))
    }

    fn registered_token(&self, peer: &crate::user_control::NativePeer) -> std::io::Result<String> {
        #[cfg(test)]
        if self
            .test_context
            .as_ref()
            .is_some_and(|context| context.peer.identity == peer.identity)
        {
            return Ok(self.token.token_value());
        }
        crate::user_control::token(Path::new(&self.support_dir), &peer.identity)
    }

    fn registered_config(
        &self,
        peer: &crate::user_control::NativePeer,
    ) -> std::io::Result<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(context) = self
            .test_context
            .as_ref()
            .filter(|context| context.peer.identity == peer.identity)
        {
            return Ok(context.config.clone().into());
        }
        crate::user_control::conf_dir(Path::new(&self.support_dir), &peer.identity)
    }

    fn verified_core(
        &self,
        id: &polaris_helper_proto::core_manifest::CoreId,
    ) -> std::io::Result<Arc<crate::core_store::VerifiedCore>> {
        #[cfg(test)]
        if let Some(context) = &self.test_context {
            if context.core.manifest.id == id.as_str() {
                context.core.recheck_fixture()?;
                return Ok(Arc::clone(&context.core));
            }
            return Err(std::io::Error::other("fixture selected id differs"));
        }
        crate::pinned_core::verify(&self.derived_core_dir(), id).map(Arc::new)
    }

    /// token 常量时间比对（Go helper.go:169）。[`Self::handle`] 与 [`Self::handle_frame`] 共用。
    fn is_authed(&self, client_token: &str) -> bool {
        is_authed_constant_time(client_token, &self.token.token_value())
    }

    /// 鉴权已过之后的 switch 分派（Go `handle()` 的 switch 主体）。
    fn dispatch(
        &self,
        req: Request,
        peer: Option<&crate::user_control::NativePeer>,
    ) -> HandleOutcome {
        let _reservation = if matches!(
            &req,
            Request::NativeStartPinned(_)
                | Request::Cleanup
                | Request::PrepareHelperUpgrade
                | Request::CancelHelperUpgrade
                | Request::RouteAdd(_)
                | Request::RouteDel(_)
                | Request::IfaceMetric { .. }
                | Request::FlushDns
                | Request::WindowsFlushDnsNative
                | Request::FreePort { .. }
        ) {
            match self.control_mu.lock() {
                Ok(guard) => Some(guard),
                Err(_) => return native_busy(),
            }
        } else {
            None
        };
        if let Some(peer) = peer {
            let allowed = self.child_mu.lock().is_ok_and(|mut state| {
                if let Some(target) = state.native.as_ref().map(|native| native.target) {
                    if self.proc.poll_native_child(target)
                        == crate::platform::windows::ops::NativeChildPoll::Exited
                    {
                        let _ = native_birth::cache_exit(&mut state, self.proc.as_ref(), target);
                    }
                }
                peer.administrator
                    || state.native.as_ref().map_or(state.pid.is_none(), |native| {
                        native.owner.as_ref() == Some(&peer.identity)
                    })
            });
            if !allowed
                && !matches!(
                    req,
                    Request::Ping
                        | Request::Version
                        | Request::CoreStatus { .. }
                        | Request::NativeStopBirth { .. }
                )
            {
                return HandleOutcome::Respond(Response::Err(
                    polaris_helper_proto::Error::with_detail(
                        polaris_helper_proto::ErrorCode::Auth,
                        "slot-owned-by-other-user",
                    ),
                ));
            }
        }
        match req {
            Request::PrepareHelperUpgrade | Request::CancelHelperUpgrade => {
                if !peer.is_some_and(|peer| peer.administrator) {
                    return HandleOutcome::AuthFailed;
                }
                let Ok(mut state) = self.child_mu.lock() else {
                    return native_busy();
                };
                if state.pid.is_some()
                    || state.native.is_some()
                    || !self.proc.native_custody_empty()
                {
                    return native_busy();
                }
                state.closing = matches!(req, Request::PrepareHelperUpgrade);
                HandleOutcome::Respond(Response::Ok(if state.closing {
                    ResponseKind::HelperUpgradeAdmitted
                } else {
                    ResponseKind::HelperUpgradeCancelled
                }))
            }
            Request::CoreStatus { id } => {
                HandleOutcome::Respond(Response::Ok(ResponseKind::CoreStatus(
                    crate::pinned_core::status(Some(&self.derived_core_dir()), &id),
                )))
            }
            Request::NativeStartPinned(params) => self.handle_native_pinned(&params, peer),
            Request::Ping => HandleOutcome::Respond(Response::Ok(ResponseKind::Pong(
                // Go helper.go:179: Windows Getuid()=-1 破坏正则，固定发 0。
                polaris_helper_proto::Pong::current(0),
            ))),
            Request::Version => HandleOutcome::Respond(Response::Ok(ResponseKind::Version {
                proto_version: crate::platform::windows::PROTO_VERSION,
            })),
            Request::Status => {
                // Native custody can retire only after exact HANDLE wait and writer revoke.
                // Legacy status remains observational; poisoned or unmatched custody is unknown.
                let Ok(mut state) = self.child_mu.lock() else {
                    return native_busy();
                };
                if let Some(native) = &state.native {
                    let target = native.target;
                    if self.proc.poll_native_child(target)
                        == crate::platform::windows::ops::NativeChildPoll::Exited
                    {
                        let _ = native_birth::cache_exit(&mut state, self.proc.as_ref(), target);
                    }
                }
                match self.live_managed_pid(&mut state) {
                    Some(pid) => {
                        // D2/D3：把 helper 手里那个句柄读到的两个事实一并回传。app 是 Medium IL，
                        // 对 SYSTEM child 的 OpenProcess 会被拒 ⇒ 它自己既读不到创建时间（pid 复用
                        // 不可发现），也读不到实跑映像（内核自证恒「未能进行」）。
                        let identity = self.proc.managed_identity(pid);
                        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
                            polaris_helper_proto::Status::Running {
                                pid,
                                created: identity.created,
                                image: identity.image,
                            },
                        )))
                    }
                    None if self.proc.native_custody_empty() => HandleOutcome::Respond(
                        Response::Ok(ResponseKind::Status(polaris_helper_proto::Status::Stopped)),
                    ),
                    None => native_busy(),
                }
            }
            Request::Stop { pid: want } => {
                // Go helper.go:189-200: 持锁摘 child → 后台收割（不持 mu）→ OK stopped <pid> / notrunning
                //
                // **受管 pid 身份判据**（本协议新增）：判定与摘除同在 child_mu 临界区内。手里的核不是
                // 请求所指的那个 = 它属另一个会话（老 stop 腿在管道上挂住期间用户重装 helper / 重起了
                // 核）⇒ 诚实 no-op，绝不「反正要停就杀当前的」。见 `stop_pid_matches`。
                let pid = {
                    let Ok(mut state) = self.child_mu.lock() else {
                        return native_busy();
                    };
                    if state.native.is_some() || !self.proc.native_custody_empty() {
                        return native_busy();
                    }
                    if let Some(cur) = state.pid {
                        if !polaris_helper_proto::stop_pid_matches(want, cur) {
                            return HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                                polaris_helper_proto::Stop::Mismatch {
                                    want: want.unwrap_or(0),
                                    current: cur,
                                },
                            )));
                        }
                    }
                    state.pid.take() // 摘除（独占收割权）
                };
                match pid {
                    Some(pid) => {
                        // 摘除后不持锁、后台异步收割（Go: go terminateChild(c, done)）。
                        // W6 修：reap_child 内部起线程（不再同步阻塞 2s）→ stop 立即回复。
                        self.proc.reap_child(pid);
                        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                            polaris_helper_proto::Stop::Stopped { pid },
                        )))
                    }
                    None => HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                        polaris_helper_proto::Stop::NotRunning,
                    ))),
                }
            }
            Request::Cleanup => self.handle_cleanup(peer),
            Request::Uninstall => {
                HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                    polaris_helper_proto::ErrorCode::Other,
                    "native-cleaner-uac-required; use bundled cleaner UAC fallback",
                )))
            }
            Request::Start(_) => {
                HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                    polaris_helper_proto::ErrorCode::Unknown,
                    "retired-start",
                )))
            }
            Request::NativeStartBirth(_) => HandleOutcome::Respond(Response::Err(
                polaris_helper_proto::Error::new(polaris_helper_proto::ErrorCode::Unknown),
            )),
            Request::NativeStatusBirth if peer.is_some() => {
                self.handle_native_status(peer.expect("peer guarded"))
            }
            Request::NativeStopBirth { target } if peer.is_some() => {
                self.handle_native_stop(target, peer.expect("peer guarded"))
            }
            Request::NativeStatusBirth | Request::NativeStopBirth { .. } => {
                HandleOutcome::AuthFailed
            }
            Request::FreePort { port } => self.handle_freeport(port),
            // W9 修：route-add / route-del 须区分 add/delete（此前合并丢了 op 区分）。
            Request::RouteAdd(rp) => self.handle_route(&rp.iface, &rp.cidrs, false),
            Request::RouteDel(rp) => self.handle_route(&rp.iface, &rp.cidrs, true),
            Request::IfaceMetric { iface, metric } => self.handle_iface_metric(&iface, metric),
            // Both tokens enter the native API. New apps use the distinct capability token,
            // so an old installed helper can never execute its subprocess implementation.
            Request::FlushDns | Request::WindowsFlushDnsNative => self.handle_flush_dns(),
            // P4：Windows 也落受保护内核目录（此前只有 mac/linux 有这个 chokepoint，win 落 ERR unknown，
            // helper 以 LocalSystem 直接 exec 用户可写路径下的 sing-box.exe）。
            Request::InstallCore(_) => HandleOutcome::Respond(Response::Err(
                polaris_helper_proto::Error::new(polaris_helper_proto::ErrorCode::Unknown),
            )),
            // 以下命令 Windows helper 不支持（mac/linux 专属）—— Go default 分支回 ERR unknown。
            Request::LinuxStart(_)
            | Request::LinuxStartPinned(_)
            | Request::LinuxStop { .. }
            | Request::LinuxStartBirth(_)
            | Request::LinuxStatusBirth
            | Request::LinuxStopBirth { .. }
            | Request::LinuxDnsSet(_)
            | Request::LinuxDnsRevert { .. }
            | Request::MacProxyTransaction { .. }
            | Request::MacProxyCompareTransaction { .. }
            | Request::MacProxyCompareCapability
            | Request::DefaultRestore { .. } => HandleOutcome::Respond(Response::Err(
                polaris_helper_proto::Error::new(polaris_helper_proto::ErrorCode::Unknown),
            )),
        }
    }

    /// Flush the OS DNS cache through the SYSTEM helper.
    fn handle_flush_dns(&self) -> HandleOutcome {
        match self.proc.flush_dns(&self.dns_admission) {
            Ok(()) => HandleOutcome::Respond(Response::Ok(ResponseKind::FlushDns(
                polaris_helper_proto::FlushDns::Flushed,
            ))),
            Err(detail) => {
                HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                    polaris_helper_proto::ErrorCode::DnsCache,
                    detail,
                )))
            }
        }
    }

    /// Store root is derived from the protected machine support root.
    /// install-core 的落盘目标 = `<support>\core`（**派生**，不从命令行/wire 取 —— SCM ImagePath
    /// 是提权后可改的，少一个会被读的核路径参数就少一条注入向量）。
    ///
    /// 与起核自检共用同一个表达式：两处各写一遍 `"core"`，改一处漏一处就是「装到 A、守着 B」。
    fn derived_core_dir(&self) -> std::path::PathBuf {
        Path::new(&self.support_dir).join("core")
    }

    fn handle_cleanup(&self, peer: Option<&crate::user_control::NativePeer>) -> HandleOutcome {
        let Ok(mut state) = self.child_mu.lock() else {
            return native_busy();
        };
        // Recheck the owner in the exact stop critical section as well as
        // reserving control across dispatch admission and cleanup.
        if let Some(peer) = peer {
            if !peer.administrator
                && state.native.as_ref().map_or(state.pid.is_some(), |native| {
                    native.owner.as_ref() != Some(&peer.identity)
                })
            {
                return HandleOutcome::Respond(Response::Err(
                    polaris_helper_proto::Error::with_detail(
                        polaris_helper_proto::ErrorCode::Auth,
                        "slot-owned-by-other-user",
                    ),
                ));
            }
        }
        if let Some(native) = &state.native {
            let target = native.target;
            if !matches!(
                stop_locked(&mut state, self.proc.as_ref(), target),
                polaris_helper_proto::NativeBirthStop::Stopped { .. }
            ) {
                return native_busy();
            }
        }
        if !self.proc.native_custody_empty() {
            return native_busy();
        }
        if state.pid.is_some() {
            return native_busy();
        }
        // Only the helper's retained native custody is in scope. An empty slot
        // does not authorize sweeping other processes by image/name.
        HandleOutcome::Respond(Response::Ok(ResponseKind::Cleaned))
    }

    /// 返回仍存活的受管核 pid；确定已死时原子清除陈旧记账。
    fn live_managed_pid(&self, state: &mut ChildState) -> Option<u32> {
        if let Some(native) = &state.native {
            return Some(native.target.pid.get());
        }
        let pid = state.pid?;
        if self.proc.process_alive(pid) {
            Some(pid)
        } else {
            state.pid = None;
            None
        }
    }

    /// `freeport` 分支（Go helper.go:295-337）。
    fn handle_freeport(&self, port: u16) -> HandleOutcome {
        // Go helper.go:308-316: listenPidsForPort 失败 → ERR enum
        let Ok(native_state) = self.child_mu.lock() else {
            return native_busy();
        };
        if native_state.closing
            || native_state.native.is_some()
            || !self.proc.native_custody_empty()
        {
            return native_busy();
        }
        let pids = match self.net.listen_pids_for_port(port) {
            Ok(pids) => pids,
            Err(e) => {
                return HandleOutcome::Respond(Response::Err(
                    polaris_helper_proto::Error::with_detail(
                        polaris_helper_proto::ErrorCode::Enum,
                        e.to_string(),
                    ),
                ));
            }
        };
        // Go helper.go:313-315: 无 LISTEN → OK free
        if pids.is_empty() {
            return HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
                polaris_helper_proto::FreePort::Free,
            )));
        }
        // No retained native birth can authorize killing these port owners.
        // Report occupied ports; process-image equality is never custody proof.
        let foreign = pids
            .into_iter()
            .map(|pid| {
                let image = self.proc.process_image_name(pid);
                let base = logic::filepath_base(&image);
                if base.is_empty() || base == "." {
                    format!("pid:{pid}")
                } else {
                    base.to_owned()
                }
            })
            .collect();
        HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
            polaris_helper_proto::FreePort::Foreign { names: foreign },
        )))
    }

    /// `route-add` / `route-del` 分支（Go helper.go:212-241）。
    ///
    /// **修 W9**：此前是空壳（只校验 iface 即回 `OK route`，netsh 从未执行，注释谎称在 winproc）。现真跑
    /// netsh：iface 白名单 + 每个 CIDR 校验后，经 [`ProcOps::apply_route`] 对每个合法 CIDR 跑
    /// `netsh interface <fam> add|delete route ... store=active`（`del` 区分 add/delete）。生产 FFI 在
    /// [`crate::platform::windows::winproc`]（真机门验生效），本分派层做校验 + 每 CIDR 派发（可 mock 测）。
    fn handle_route(&self, iface: &str, cidrs: &[String], del: bool) -> HandleOutcome {
        // Go helper.go:218-221: !ifaceAllowed(iface) → ERR iface-denied
        if !logic::iface_allowed(iface) {
            return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                polaris_helper_proto::ErrorCode::IfaceDenied,
            )));
        }
        // Go helper.go:226-240: for c := range Split(cidrsLine, ","): 空跳过 / ParseCIDR 失败跳过 /
        // 否则跑 netsh。CIDR 校验用 helper-proto::codec::is_valid_cidr（stdlib 解析，严于 Go 手写）。
        for cidr in cidrs {
            let cidr = cidr.trim();
            if cidr.is_empty() {
                continue; // Go: c == "" → continue
            }
            if !polaris_helper_proto::codec::is_valid_cidr(cidr) {
                continue; // Go: net.ParseCIDR err → continue（静默跳过非法 CIDR）
            }
            self.proc.apply_route(iface, cidr, del);
        }
        // Go helper.go:241: OK route（幂等 best-effort，最终由 app 侧校验）
        HandleOutcome::Respond(Response::Ok(ResponseKind::Route))
    }

    /// `iface-metric` 分支（Go helper.go:242-275，退役保留兼容）。
    ///
    /// DESIGN-REVIEW(iface-metric-retired-stub)：Go 侧此命令**已退役保留兼容**（proto v3/v4，自 Windows
    /// 禁 System 起客户端不再调用；新客户端 EXPECTED_PROTO 不调）。本 Rust 侧**忠实保留退役桩**——校验
    /// iface 白名单 + metric 范围后即回 `OK iface-metric`，**不执行** PowerShell `Set-NetIPInterface`。
    /// 这是与 Go 的**有意差异**（Go 仍跑 PowerShell 以兼容已部署的旧 proto helper；新客户端不触发本路径，
    /// 故不实现 PowerShell 无功能损失）。若未来需兼容旧 proto helper 的真实降权，须补 PowerShell 执行。
    fn handle_iface_metric(&self, iface: &str, metric: u16) -> HandleOutcome {
        // Go helper.go:253-256: !ifaceAllowed → ERR iface-denied
        if !logic::iface_allowed(iface) {
            return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                polaris_helper_proto::ErrorCode::IfaceDenied,
            )));
        }
        // Go helper.go:257-261: metric 非法（Go atoi err 或 <0 或 >65535）→ ERR bad-metric
        // u16 天然 ∈ [0,65535]，无需额外检查（Go 的 <0/>65535 在 u16 下不可能）。
        let _ = metric;
        // Go helper.go:262-274: PowerShell Set-NetIPInterface（生产侧 exec，失败 → ERR set-metric）
        // 退役命令：生产侧直接 Ok（新客户端不再调用；保留仅为兼容已部署 proto≥3 helper）。
        HandleOutcome::Respond(Response::Ok(ResponseKind::IfaceMetric))
    }

    /// Permanently fence native Start under the same lock as spawn/publication.
    pub fn begin_shutdown(&self) -> Result<(), String> {
        let dns_result = self.dns_admission.close();
        let core_result = match self.child_mu.lock() {
            Ok(mut state) => {
                state.closing = true;
                Ok(())
            }
            Err(poison) => {
                // Recover only to fence admission, never to mint an empty or stopped receipt.
                poison.into_inner().closing = true;
                Err("helper child custody lock poisoned".into())
            }
        };
        core_result.and(dns_result)
    }

    /// One bounded native close attempt. Err retains both registries and the logger tail;
    /// normal service/console exit must retry or remain alive rather than discard custody.
    pub fn reap_child_on_exit(&self) -> Result<(), String> {
        if let Err(error) = self.begin_shutdown() {
            // DNS admission can be poisoned independently of native custody.
            // Keep DNS closed, but still attempt the original core's real drain.
            log::error!("helper shutdown admission diagnostic: {error}");
        }
        let mut state = self
            .child_mu
            .lock()
            .map_err(|_| "helper child custody lock poisoned")?;
        if let Some(native) = &state.native {
            let target = native.target;
            let outcome = stop_locked(&mut state, self.proc.as_ref(), target);
            if !matches!(
                outcome,
                polaris_helper_proto::NativeBirthStop::Stopped { .. }
            ) {
                return Err(format!(
                    "native child shutdown remains unconfirmed: {outcome:?}"
                ));
            }
        }
        if state.native.is_some() || state.pid.is_some() || !self.proc.native_custody_empty() {
            return Err("helper child custody is not confirmed empty".into());
        }
        Ok(())
    }
}

fn native_busy() -> HandleOutcome {
    HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
        polaris_helper_proto::ErrorCode::Other,
        "busy native child custody pending or unknown; exact birth Stop required",
    )))
}

#[cfg(test)]
mod tests;

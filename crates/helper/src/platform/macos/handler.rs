//! 协议分派（移植自 上游 `helper/helper.go:397-589` 的 `handle()`）。
//!
//! ## Go 源结构（`helper.go:397-419`）
//!
//! ```text
//! func handle(conn net.Conn) {
//!     defer conn.Close()
//!     conn.SetReadDeadline(5s)
//!     r := bufio.NewReader(conn)
//!     tok := readLine(r)          // 行1: token
//!     cmd := readLine(r)          // 行2: command
//!     if tok == "" || tok != tokenValue() {
//!         fmt.Fprintln(conn, "ERR auth"); return
//!     }
//!     if cmd == "freeport" { handleFreeport(...); return }   // 不持锁
//!     mu.Lock(); defer mu.Unlock()
//!     switch cmd { case "ping": ... case "start": ... }
//! }
//! ```
//!
//! ## 移植纪律
//!
//! 把「鉴权 + 命令分派」从 socket IO 中剥离为纯函数 [`dispatch`] —— 接收已读的 token/command/args 行
//! + 一个 [`MacServices`] 服务 bundle，返回 [`Response`]。这样：
//! - 协议分派逻辑（参数校验、白名单判定、错误码映射）跨平台可测（Linux CI 完整覆盖）。
//! - socket IO（accept/read/write/超时）留给 [`crate::platform::macos::server`] 的 mac-gated 部分。
//!
//! freeport 与 native Start 共用 server 命令锁，避免空槽检查后新核启动再遭 PID 清理。
//! 本 dispatch 是无状态的（状态在 [`MacServices`] 内），不模拟 Go 的 mu —— 并发控制由 server 层负责。

use crate::platform::macos::exec::{CommandRunner, EXEC_TIMEOUT};
use crate::platform::macos::flush_dns;
use crate::platform::macos::freeport;
use crate::platform::macos::route::{self, RouteOp};
use crate::platform::macos::whitelist;
use crate::token::{TokenCheck, TokenStore, check_token};
use polaris_helper_proto::request::{RouteParams, StartParams};
use polaris_helper_proto::response::{
    NativeBirthStart, NativeBirthStatus, NativeBirthStop, ResponseKind, StartTiming, Status, Stop,
};
use polaris_helper_proto::{
    Error as ProtoError, ErrorCode, HelperBirthTarget, Platform, Request, Response,
    stop_pid_matches,
};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// macOS helper 配置（对应 Go 的 `--singbox`/`--confdir`/`--support`/`--coredir` flag）。
///
/// 这些值在安装期锁定（改需 root），是安全边界的一部分：
/// - `singbox_bin`：sing-box 二进制路径锁定（杜绝「持 token 跑任意二进制」，`helper.go:6`）。
/// - `conf_dir`：允许的配置文件目录（`cfgAllowed` 白名单，`helper.go:245-252`）。
/// - `support_dir`：socket + token 所在目录。
/// - `core_dir`：install-core 只写此目录（防写任意路径，`helper.go:128`）。
#[derive(Debug, Clone)]
pub struct MacConfig {
    /// 锁定的 sing-box 二进制路径（`helper.go:592` flag `--singbox`）。
    pub singbox_bin: String,
    /// 允许的配置文件目录（`helper.go:593` flag `--confdir`）。
    pub conf_dir: String,
    /// socket + token 所在目录（`helper.go:594` flag `--support`，默认 `/Library/Application Support/Polaris`）。
    pub support_dir: String,
    /// install-core 的受保护内核目录（`helper.go:595` flag `--coredir`）。
    pub core_dir: String,
}

impl MacConfig {
    /// 构造默认配置（对应 Go main 的 flag 默认值）。
    #[must_use]
    pub fn new(
        singbox_bin: impl Into<String>,
        conf_dir: impl Into<String>,
        support_dir: impl Into<String>,
        core_dir: impl Into<String>,
    ) -> Self {
        Self {
            singbox_bin: singbox_bin.into(),
            conf_dir: conf_dir.into(),
            support_dir: support_dir.into(),
            core_dir: core_dir.into(),
        }
    }
}

/// child sing-box 进程的运行时状态（对应 Go 的 `child`/`childDone` 全局变量 + mu）。
///
/// 用 `Arc<Mutex<Option<ChildHandle>>>` 持有 —— 对齐 Go 的 `mu.Lock()` 互斥语义。
/// 实际 child 进程的 spawn/wait/terminate 由 [`crate::platform::macos::server`] 的 mac-gated 部分实现
/// （需 tokio/std::process + 信号处理）；本结构只持有状态快照（pid）。
#[derive(Debug, Clone, Default)]
pub struct ChildHandle {
    /// child sing-box 的 pid。
    pub pid: u32,
}

/// macOS helper 已创建的核心及其可归因关键路径耗时。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedCore {
    /// 新 child 的 pid。
    pub pid: u32,
    /// cache 属主准备 + `Command::spawn` 耗时。
    pub process_ms: u64,
    /// stdout/stderr 所有权移交日志后台线程的耗时。
    pub log_handoff_ms: u64,
}

/// macOS helper 的服务 bundle —— 把所有外部依赖（token 存储、命令执行、child 状态）打包成一个 trait，
/// 便于测试注入 mock、生产注入真实实现。
pub trait MacServices: Send + Sync {
    fn token_for_peer(&self, _uid: Option<u32>) -> String {
        self.token_store().token_value()
    }
    fn native_start_pinned(
        &self,
        _params: &polaris_helper_proto::core_protocol::PinnedStartParams,
        _uid: u32,
    ) -> Result<NativeBirthStart, SpawnError> {
        Err(SpawnError::NotImplemented)
    }
    fn native_caller_allowed(
        &self,
        uid: Option<u32>,
        _id: Option<&polaris_helper_proto::core_manifest::CoreId>,
    ) -> bool {
        uid == Some(0)
    }
    /// Unsupported services cannot attest native custody by calling the legacy void stop.
    fn native_start(&self, _params: &StartParams) -> Result<NativeBirthStart, SpawnError> {
        Err(SpawnError::NotImplemented)
    }
    fn native_status(&self) -> NativeBirthStatus {
        NativeBirthStatus::Unknown { target: None }
    }
    fn native_stop(&self, target: HelperBirthTarget) -> NativeBirthStop {
        NativeBirthStop::Unknown { target }
    }
    fn native_custody_active(&self) -> bool {
        false
    }
    fn legacy_start_supported(&self) -> bool {
        true
    }
    /// token 存储（读 `helper.token`）。
    fn token_store(&self) -> &dyn TokenStore;

    /// 命令执行器（route/lsof/ps/dscacheutil/codesign 等）。
    fn runner(&self) -> &dyn CommandRunner;

    /// child 进程状态（对应 Go 的 `child` 全局变量）。
    fn child(&self) -> &Mutex<Option<ChildHandle>>;

    /// 当前 helper 进程的 uid（mac `os.Getuid()`；测试可固定 0）。
    fn uid(&self) -> i64 {
        #[cfg(target_os = "macos")]
        {
            // os.Getuid() 等价 —— nix 0.31 的 getuid() 在 `user` feature 后（Cargo.toml 已开）。
            // Uid::current() 是 getuid() 的官方 Rusty 别名（nix unistd.rs:64-68，doc alias "getuid"）。
            nix::unistd::Uid::current().as_raw() as i64
        }
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
    }

    /// spawn sing-box child（mac-gated，由 server 层实现，`helper.go:538-578`）。
    ///
    /// 返回新 child 的 pid 与关键路径耗时。`parent_pid`=父 app PID（`Some`→起 watchParent 父死看护，
    /// `helper.go:576-578`；`None`→不看护，兼容旧客户端）。默认实现返回未实现错误（测试可 mock）。
    fn spawn_child(
        &self,
        _cfg: &str,
        _log: &str,
        _fwd: bool,
        _parent_pid: Option<u32>,
    ) -> Result<SpawnedCore, SpawnError> {
        Err(SpawnError::NotImplemented)
    }

    /// 终止 child（TERM→等→KILL，mac-gated，由 server 层实现）。
    ///
    /// `want_pid` = 客户端声明它意图停的那个受管 pid（`None` = 旧语义「停当前受管核」）。判据走
    /// [`stop_pid_matches`]：不匹配 ⇒ 手里这个核属另一个会话 ⇒ **不摘不杀**，回
    /// [`TerminateOutcome::Mismatch`]（诚实 no-op）。判定与摘除在**同一把 child 锁**下完成，
    /// 不给「判完才被换掉」留缝。
    fn terminate_child(&self, want_pid: Option<u32>) -> TerminateOutcome {
        terminate_managed_child(self.child(), want_pid)
    }

    /// 摘除 child 状态（cleanup 用，不发信号；对应 Go `helper.go:448` `child, childDone = nil, nil`）。
    ///
    /// 默认只清 [`child`](Self::child) 视图；生产实现（server 层）覆盖以同步清内部 `done`/`generation`
    /// 记账（否则收割身份代守卫会残留陈旧 done）。
    fn clear_child(&self) {
        *self
            .child()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// [`MacServices::terminate_child`] 的判定 + 摘除本体（**单一判定点**）。
///
/// 抽成自由函数而非留在 trait 默认体里，是为了让测试替身能**复用真判据**而不是各抄一份
/// （替身抄一份 = 测试测的是替身，身份判据被删掉也照样绿）。
///
/// 判定与摘除在同一把 child 锁下完成：不匹配 ⇒ 不摘、不杀，回 [`TerminateOutcome::Mismatch`]。
pub fn terminate_managed_child(
    child: &Mutex<Option<ChildHandle>>,
    want_pid: Option<u32>,
) -> TerminateOutcome {
    let mut guard = child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.as_ref() {
        Some(c) if !stop_pid_matches(want_pid, c.pid) => TerminateOutcome::Mismatch {
            want: want_pid.unwrap_or(0),
            current: c.pid,
        },
        Some(_) => guard.take().map_or(TerminateOutcome::NotRunning, |c| {
            TerminateOutcome::Stopped { pid: c.pid }
        }),
        None => TerminateOutcome::NotRunning,
    }
}

/// spawn child 的错误。
#[derive(Debug, Clone)]
pub enum SpawnError {
    /// 非 mac 环境 / 未注入实现。
    NotImplemented,
    /// 启动失败（对齐 Go `helper.go:552` 的 `ERR start <err>`）。
    Failed(String),
}

/// terminate child 的结果（对应 Go `helper.go:440-442`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateOutcome {
    /// `OK stopped <pid>`。
    Stopped { pid: u32 },
    /// `OK notrunning`。
    NotRunning,
    /// `OK stop-mismatch <want> <current>` —— 受管 pid 非请求所指 ⇒ 不动手（见
    /// [`stop_pid_matches`]）。
    Mismatch { want: u32, current: u32 },
}

/// 协议分派结果（一个请求一个响应）。
///
/// 这是纯逻辑：输入 token + [`Request`] + 服务 bundle，输出 [`Response`]。
/// socket IO 层负责读 token 行 → 构造 Request → 调本函数 → 写回 Response。
pub fn dispatch(
    services: &dyn MacServices,
    config: &MacConfig,
    token: &str,
    req: &Request,
) -> Response {
    dispatch_from_peer(services, config, token, req, None)
}

/// [`dispatch`] 加上对端 uid（`getpeereid`，由 socket 层在 accept 后取）。
///
/// 只有 install-core 消费它：源目录与其中文件必须属对端所有。`None` = 没取到 ⇒ install-core
/// 一律不收（其余命令不受影响）。生产的 socket 层恒走本函数；[`dispatch`] 是不带对端身份的
/// 入口，留给不涉及 install-core 的调用方。
pub fn dispatch_from_peer(
    services: &dyn MacServices,
    config: &MacConfig,
    token: &str,
    req: &Request,
    peer_uid: Option<u32>,
) -> Response {
    // helper.go:405-408: token 鉴权（首道边界）
    let stored = services.token_for_peer(peer_uid);
    if peer_uid != Some(0) && !matches!(check_token(token, &stored), TokenCheck::Authed) {
        return Response::Err(ProtoError::new(ErrorCode::Auth));
    }

    // Freeport can signal PID owners, so native custody fences it before any runner call.
    if let Request::FreePort { port } = req {
        if services.native_custody_active() {
            return legacy_native_required();
        }
        let port_str = port.to_string();
        return freeport::run_freeport(services.runner(), &port_str);
    }

    // 其余命令在「临界区」内处理 —— 这里用 child() mutex 体现互斥语义
    // （实际生产由 server 层在调 dispatch 前后包 mu；本函数内部对 child 状态的访问仍走 mutex）
    match req {
        Request::CoreStatus { id } => Response::Ok(ResponseKind::CoreStatus(
            crate::pinned_core::status(Some(Path::new(&config.core_dir)), id),
        )),
        Request::NativeStartPinned(params) => {
            let Some(uid) = peer_uid else {
                return Response::Err(ProtoError::new(ErrorCode::Auth));
            };
            if !services.native_caller_allowed(Some(uid), Some(&params.id)) {
                return Response::Err(ProtoError::with_detail(
                    ErrorCode::Start,
                    "slot-owned-or-core-occupied",
                ));
            }
            match services.native_start_pinned(params, uid) {
                Ok(start) => Response::Ok(ResponseKind::PinnedStart {
                    id: params.id.clone(),
                    start: polaris_helper_proto::core_protocol::PinnedBirthStart::Native(start),
                }),
                Err(SpawnError::NotImplemented) => Response::Err(ProtoError::with_detail(
                    ErrorCode::Start,
                    "pinned spawn unavailable",
                )),
                Err(SpawnError::Failed(error)) => {
                    Response::Err(ProtoError::with_detail(ErrorCode::Start, error))
                }
            }
        }
        Request::NativeStatusBirth => {
            if !services.native_caller_allowed(peer_uid, None) {
                return Response::Err(ProtoError::new(ErrorCode::Auth));
            }
            Response::Ok(ResponseKind::NativeBirthStatus(services.native_status()))
        }
        Request::NativeStopBirth { target } => {
            if !services.native_caller_allowed(peer_uid, None) {
                return Response::Err(ProtoError::new(ErrorCode::Auth));
            }
            Response::Ok(ResponseKind::NativeBirthStop(services.native_stop(*target)))
        }
        Request::NativeStartBirth(_) => Response::Err(ProtoError::new(ErrorCode::Unknown)),
        Request::Ping => {
            // helper.go:422-423: OK pong uid=<n> v<ver>
            Response::Ok(ResponseKind::Pong(polaris_helper_proto::Pong::current(
                services.uid(),
            )))
        }
        Request::Version => {
            // helper.go:424-425: OK <ver>
            Response::Ok(ResponseKind::Version {
                proto_version: crate::platform::macos::PROTO_VERSION,
            })
        }
        Request::Status => {
            if services.native_custody_active() {
                return legacy_native_required();
            }
            // helper.go:426-430
            let guard = services
                .child()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &*guard {
                // 身份 token（created/image）是 Windows 专属：mac 的 `running_exe_path`
                //（`ps -p <pid> -o comm=`）与 `lstart` 本就跨用户可读，app 侧不需要 helper 代读。
                Some(child) => Response::Ok(ResponseKind::Status(Status::Running {
                    pid: child.pid,
                    created: None,
                    image: None,
                })),
                None => Response::Ok(ResponseKind::Status(Status::Stopped)),
            }
        }
        Request::Stop { pid } => {
            if services.native_custody_active() {
                return legacy_native_required();
            }
            // helper.go:432-442（+ 受管 pid 身份判据，见 `terminate_child`）
            match services.terminate_child(*pid) {
                TerminateOutcome::Stopped { pid } => {
                    Response::Ok(ResponseKind::Stop(Stop::Stopped { pid }))
                }
                TerminateOutcome::NotRunning => Response::Ok(ResponseKind::Stop(Stop::NotRunning)),
                TerminateOutcome::Mismatch { want, current } => {
                    Response::Ok(ResponseKind::Stop(Stop::Mismatch { want, current }))
                }
            }
        }
        Request::Cleanup => {
            if services.native_custody_active() {
                return legacy_native_required();
            }
            // helper.go:444-449: pkill -9 -f "<singboxBin> run" + 摘 child
            let pattern = format!("{} run", config.singbox_bin);
            let _ = services
                .runner()
                .run(EXEC_TIMEOUT, "/usr/bin/pkill", &["-9", "-f", &pattern]);
            // helper.go:448: child, childDone = nil, nil（生产实现同步清 done/generation）
            services.clear_child();
            Response::Ok(ResponseKind::Cleaned)
        }
        Request::RouteAdd(rp) | Request::RouteDel(rp) => {
            // helper.go:450-480
            handle_route(services.runner(), rp, matches!(req, Request::RouteAdd(_)))
        }
        Request::DefaultRestore { gateway_ipv4 } => {
            // helper.go:481-491
            handle_default_restore(services.runner(), gateway_ipv4)
        }
        Request::FlushDns => {
            // helper.go:492-506
            flush_dns::flush_dns(services.runner()).into()
        }
        Request::MacProxyTransaction {
            payload_hex: _payload_hex,
        } => {
            #[cfg(target_os = "macos")]
            {
                match polaris_system_integration::execute_macos_proxy_transaction(_payload_hex) {
                    Ok(()) => Response::Ok(ResponseKind::MacProxyTransaction),
                    Err(error) => Response::Err(ProtoError::with_detail(
                        ErrorCode::SystemProxy,
                        error.replace(['\r', '\n'], " "),
                    )),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                Response::Err(ProtoError::new(ErrorCode::Unknown))
            }
        }
        Request::MacProxyCompareTransaction {
            payload_hex: _payload_hex,
        } => {
            #[cfg(target_os = "macos")]
            {
                match polaris_system_integration::execute_macos_proxy_transaction(_payload_hex) {
                    Ok(()) => Response::Ok(ResponseKind::MacProxyTransaction),
                    Err(error) => Response::Err(ProtoError::with_detail(
                        ErrorCode::SystemProxy,
                        error.replace(['\r', '\n'], " "),
                    )),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                Response::Err(ProtoError::new(ErrorCode::Unknown))
            }
        }
        // 纯 capability probe：只有新 helper 识别该 command 就返成功，绝不解码 payload、
        // 不打开 SCPreferences，因此 controller 可在任何系统写之前安全探测。
        Request::MacProxyCompareCapability => Response::Ok(ResponseKind::MacProxyTransaction),
        Request::Start(_) | Request::InstallCore(_) => {
            Response::Err(ProtoError::new(ErrorCode::Unknown))
        }
        // 以下命令不属于 mac helper 谱系（LinuxStart/IfaceMetric/Uninstall）
        Request::LinuxStart(_)
        | Request::LinuxStartPinned(_)
        | Request::LinuxStop { .. }
        | Request::LinuxStartBirth(_)
        | Request::LinuxStatusBirth
        | Request::LinuxStopBirth { .. }
        | Request::LinuxDnsSet(_)
        | Request::LinuxDnsRevert { .. }
        | Request::IfaceMetric { .. }
        | Request::WindowsFlushDnsNative
        | Request::Uninstall => Response::Err(ProtoError::new(ErrorCode::Unknown)),
        // FreePort 已在上方早返回；此处不可达（穷尽性兜底）
        Request::FreePort { .. } => unreachable!("freeport handled above"),
    }
}

/// route-add / route-del 处理（移植自 `helper.go:450-480`）。
fn handle_route(runner: &dyn CommandRunner, rp: &RouteParams, is_add: bool) -> Response {
    // helper.go:457-460: iface 白名单
    if !whitelist::iface_allowed(&rp.iface) {
        return Response::Err(ProtoError::new(ErrorCode::IfaceDenied));
    }
    let op = if is_add {
        RouteOp::Add
    } else {
        RouteOp::Delete
    };
    // helper.go:465-479: 逐 CIDR 构造 argv + 执行
    for cidr in &rp.cidrs {
        let cidr = cidr.trim();
        if cidr.is_empty() {
            continue;
        }
        if let Some(argv) = route::build_route_argv(op, &rp.iface, cidr) {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            // helper.go:478: 幂等 best-effort，忽略错误
            let _ = runner.run(EXEC_TIMEOUT, route::ROUTE_BIN, &refs);
        }
        // 非法 CIDR 跳过（helper.go:470-471 continue）
    }
    Response::Ok(ResponseKind::Route)
}

/// default-restore 处理（移植自 `helper.go:481-491`）。
fn handle_default_restore(runner: &dyn CommandRunner, gateway_ipv4: &str) -> Response {
    let gw = gateway_ipv4.trim();
    match route::build_default_restore_argv(gw) {
        Some(argv) => {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let _ = runner.run(EXEC_TIMEOUT, route::ROUTE_BIN, &refs);
            Response::Ok(ResponseKind::DefaultRestored)
        }
        None => Response::Err(ProtoError::new(ErrorCode::BadGateway)),
    }
}

fn legacy_native_required() -> Response {
    Response::Err(ProtoError::with_detail(
        ErrorCode::Start,
        "native birth protocol required; upgrade the client",
    ))
}

// wire 写方向已上提 helper-proto：见 [`polaris_helper_proto::Response::to_wire_line`]
//（与 `Response::parse` 成对，三平台共用）。原 mac 私有副本（`response_to_wire_line`/`ok_kind_to_wire`/
// `freeport_wire`/`flush_dns_wire`，61 行）已删 —— 调用方直接 `resp.to_wire_line()`。

#[cfg(test)]
mod tests;

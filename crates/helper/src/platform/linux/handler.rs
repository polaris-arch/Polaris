//! 命令分发器 —— 移植自 上游 `helper-linux/helper.go:333-482` 的 `handle(conn)`。
//!
//! ## 流程（逐行对照 Go 源 handle()）
//!
//! 1. SO_PEERCRED 取对端凭据（uid/gid）→ 失败 `ERR peercred`（:337-340）。
//! 2. 读 command 行（:343，linux 无 token 行）。
//! 3. ping / version 在鉴权前（:345-352，任何持 socket 者可探活）。
//! 4. isAuthorized(uid) → 失败 `ERR unauthorized`（:354-357）。
//! 5. 持 mu 锁，按 command 分发（:359-481）。
//!
//! ## 测试策略
//!
//! Go 源 `handle(conn)` 直接吃 net.Conn。本实现把连接读写 + 凭据获取抽象为 [`Conn`] trait，
//! 让命令处理在不碰真实 socket 的前提下全路径测试（注入伪造 uid + 预置读写行）。
//!
//! 核 spawn（start 命令）经 [`CoreSpawner`] trait 抽象（生产用真实 AmbientCaps
//! 派生，测试 mock）。进程状态放在 [`HandlerState`]（实例化可测）。

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use crate::line_io;
use polaris_helper_proto::command::{common as cmd, linux as lcmd};
use polaris_helper_proto::{
    parse_linux_birth_stop_args, parse_stop_pid, stop_pid_matches, Error as ProtoError, ErrorCode,
    HelperBirthToken, LinuxBirthStart, LinuxBirthStatus, LinuxBirthStop, LinuxDns, Response,
    ResponseKind, StartTiming, Stop,
};

use crate::platform::linux::auth::{
    is_authorized, owned_by, supplementary_groups, AuthError, PeerCred, PeerCredProvider,
};
use crate::platform::linux::freeport::{free_port, parse_ss_pids, FreePortDeps};
use crate::platform::linux::ops::SystemdOps;
use crate::platform::linux::resolved_dns::ResolvedDnsOps;
use crate::platform::linux::state::{
    BirthAdmission, CoreSpawner, HandlerState, ManagedChild, ReapBlockerState, SpawnCoreRequest,
    SpawnError, StartAdmission, StopReap,
};

/// Linux helper protoVersion（三平台统一演进，见 `polaris_helper_proto` crate 文档）。
pub const PROTO_VERSION: u32 = polaris_helper_proto::proto_version::CURRENT;

/// 5 秒读超时（移植自 Go `conn.SetReadDeadline(time.Now().Add(5 * time.Second))`，:335）。
pub const READ_TIMEOUT_SECS: u64 = polaris_helper_proto::codec::READ_TIMEOUT_SECS;

/// Commands are short protocol keywords, not argument frames. Bound this line
/// before authentication; include room for CRLF and future command names.
const MAX_COMMAND_LINE_BYTES: usize = 64;

/// handler 依赖（注入所有外部副作用，便于测试 mock）。
pub struct HandlerDeps<'a, P: PeerCredProvider, S: CoreSpawner, D: FreePortDeps, SD: SystemdOps> {
    /// 锁定的 root-owned 受管核目录（start 只跑 coreDir/sing-box）。
    pub core_dir: Option<&'a Path>,
    #[cfg(test)]
    pub(crate) test_core: Option<Arc<crate::core_store::VerifiedCore>>,
    /// 授权 uid 列表文件。
    pub auth_file: &'a Path,
    /// SO_PEERCRED 凭据提供者。
    pub peer_cred: &'a P,
    /// sing-box spawn 抽象（start 命令）。
    pub spawner: &'a S,
    /// freeport 进程操作依赖。
    pub freeport_deps: &'a D,
    /// systemd 操作（启停 helper 自身服务，对照任务职责 1）。
    pub systemd: &'a SD,
    /// systemd-resolved per-link 接管（生产为严格白名单的 `resolvectl` 实现）。
    pub resolved_dns: &'a dyn ResolvedDnsOps,
    /// ss 命令的输出提供者（freeport 用 `ss -ltnp` 找 LISTEN 持有者）。
    /// 抽象为闭包便于测试；生产用 `ss` 子进程。
    pub ss_provider: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    /// IP 转发开关的副作用闭包（生产 set_forward_prod，测试可记录调用）。
    pub set_forward: &'a (dyn Fn(bool) + Send + Sync),
}

/// 连接抽象（trait 便于测试 mock；生产用 tokio::net::UnixStream 经 adapter）。
///
/// 对应 Go `handle(conn net.Conn)`：读行 + 写行 + 取对端凭据。
pub trait Conn: Send {
    /// Read a complete command with a small bound before UID authorization.
    /// EOF, malformed UTF-8, truncation and overflow cannot become commands.
    fn read_command_line(&mut self) -> Result<Option<String>, StrictReadError>;
    /// 读一行（trim 尾部 \n/\r）。EOF / 读失败返回 ""（对齐 Go readLine 的 ReadString 行为）。
    fn read_line(&mut self) -> String;
    /// Exact-birth framing distinguishes EOF from an empty/extra line and
    /// treats timeout, invalid UTF-8, or overlong input as a hard error.
    fn read_line_strict(&mut self) -> Result<Option<String>, StrictReadError> {
        Err(StrictReadError)
    }
    /// 写一行（自动加 \n）。返回是否写成功。
    fn write_line(&mut self, line: &str) -> bool;
}

/// Exact command framing failed before a complete line or EOF was observed.
#[derive(Debug, Clone, Copy)]
pub struct StrictReadError;

/// 把任意 Read + Write 包成 BufRead 行 IO（生产 unix socket adapter 用）。
///
/// 读写本体已上提 [`crate::line_io`]（与 mac 共用单一真值）；本类型只做
/// linux [`Conn`] 契约的形状适配（EOF→`""`、写成功→`bool`）。
pub struct LineConn<RW: Read + Write> {
    inner: BufReader<RW>,
}

impl<RW: Read + Write> LineConn<RW> {
    #[must_use]
    pub fn new(io: RW) -> Self {
        Self {
            inner: BufReader::new(io),
        }
    }
}

impl<RW: Read + Write + Send> Conn for LineConn<RW> {
    fn read_command_line(&mut self) -> Result<Option<String>, StrictReadError> {
        line_io::read_line_terminated_bounded(&mut self.inner, MAX_COMMAND_LINE_BYTES)
            .map_err(|_| StrictReadError)
    }

    fn read_line(&mut self) -> String {
        // Conn 契约：EOF/读失败与空行一律 ""（对齐 Go readLine 的 ReadString 行为）。
        line_io::read_line_trimmed_bounded(
            &mut self.inner,
            polaris_helper_proto::codec::MAX_FRAME_BYTES,
        )
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn read_line_strict(&mut self) -> Result<Option<String>, StrictReadError> {
        line_io::read_line_trimmed_bounded(
            &mut self.inner,
            polaris_helper_proto::codec::MAX_FRAME_BYTES,
        )
        .map_err(|_| StrictReadError)
    }

    fn write_line(&mut self, line: &str) -> bool {
        // 写走底层 RW（BufReader 只缓冲读方向），语义同原实现。
        line_io::write_line(self.inner.get_mut(), line).is_ok()
    }
}

/// 处理一个连接（移植自 Go `handle`，:333-482）。
///
/// 返回处理是否成功（连接层错误由调用方处理）。所有 wire 响应已写入 conn。
pub fn handle<P, S, D, SD>(
    state: &Mutex<HandlerState>,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    // 1. SO_PEERCRED 取凭据（:337-340）。
    let Some(cred) = deps.peer_cred.peer_cred() else {
        // ERR peercred（Go: fmt.Fprintln(conn, "ERR peercred")）。
        let _ = conn.write_line(&format!("ERR {}", AuthError::Peercred.wire_token()));
        return;
    };

    // 2. 读 command 行（linux 无 token 行，:343）。
    let command = match conn.read_command_line() {
        Ok(Some(command)) => command,
        Ok(None) => return,
        Err(_) => {
            let _ = conn.write_line("ERR unknown");
            return;
        }
    };

    // 3. ping / version 在鉴权前（任何持 socket 者可探活，:345-352）。
    match command.as_str() {
        cmd::PING => {
            // shared Pong 统一追加 build identity；旧 app 会忽略该字段，新 app 可识别同 protocol 旧 helper。
            let response = Response::Ok(ResponseKind::Pong(polaris_helper_proto::Pong::current(
                i64::from(cred.uid),
            )));
            let _ = conn.write_line(&response.to_wire_line());
            return;
        }
        cmd::VERSION => {
            // OK <ver>（Go: fmt.Fprintf(conn, "OK %s\n", protoVersion)）。
            let _ = conn.write_line(&format!("OK {PROTO_VERSION}"));
            return;
        }
        _ => {}
    }

    // 4. 鉴权（:354-357）。
    if !is_authorized(cred.uid, deps.auth_file) {
        let _ = conn.write_line(&format!("ERR {}", AuthError::Unauthorized.wire_token()));
        return;
    }

    // Reserve native ownership before a slow DNS operation, then release the
    // child mutex so the owner can still stop/status its retained birth.
    if matches!(
        command.as_str(),
        lcmd::RESOLVED_DNS_SET | lcmd::RESOLVED_DNS_REVERT
    ) {
        let Ok(mut guard) = state.lock() else {
            let _ = conn.write_line("ERR unknown");
            return;
        };
        if guard.upgrade_closed
            || guard.dns_busy
            || guard
                .dns_owner
                .is_some_and(|owner| cred.uid != 0 && owner != cred.uid)
            || !deps.spawner.caller_allowed(
                cred.uid,
                guard.child.as_ref().map(ManagedChild::handle),
                None,
            )
        {
            let _ = conn.write_line("ERR slot-owned-or-control-busy");
            return;
        }
        let previous_owner = guard.dns_owner;
        let previous_birth = guard.dns_birth.clone();
        let reservation_birth = guard.child.as_ref().map(|child| child.handle().clone());
        guard.dns_birth = reservation_birth.clone().or(previous_birth.clone());
        guard.dns_busy = true; // reservation, not proof that DNS was modified
        drop(guard);
        let outcome = if command == lcmd::RESOLVED_DNS_REVERT {
            handle_resolved_dns_revert(deps.resolved_dns, conn)
        } else {
            handle_resolved_dns_set(deps.resolved_dns, conn)
        };
        if let Ok(mut guard) = state.lock() {
            match outcome {
                DnsChange::Applied => {
                    guard.dns_owner = Some(cred.uid);
                    guard.dns_birth = reservation_birth;
                }
                DnsChange::Restored => {
                    guard.dns_owner = None;
                    guard.dns_birth = None;
                }
                DnsChange::Unmodified => {
                    guard.dns_owner = previous_owner;
                    guard.dns_birth = previous_birth;
                }
                DnsChange::Unconfirmed => {
                    guard.dns_owner = previous_owner.or(Some(cred.uid));
                }
            }
            // The same flag also retains an unconfirmed live-birth SET.
            // Only a retired/replaced birth hands DNS restoration to this
            // completion; a successful retry on the retained birth keeps DNS.
            let dns_birth_current = guard
                .dns_birth
                .as_ref()
                .zip(guard.child.as_ref())
                .is_some_and(|(birth, child)| birth.same_birth(child.handle()));
            if guard.dns_cleanup_pending && !dns_birth_current && guard.dns_owner.is_some() {
                drop(guard);
                let cleanup_cleared = deps
                    .resolved_dns
                    .revert(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME)
                    .is_ok();
                if let Ok(mut guard) = state.lock() {
                    if cleanup_cleared {
                        guard.dns_owner = None;
                        guard.dns_birth = None;
                    }
                    guard.dns_cleanup_pending = !cleanup_cleared;
                    guard.dns_busy = false;
                }
            } else {
                // A failed rollback/revert leaves retained DNS responsibility;
                // another Start cannot reuse the slot until a real restore.
                guard.dns_cleanup_pending = match outcome {
                    DnsChange::Applied | DnsChange::Restored => false,
                    // A reaper can mark the temporary reservation pending
                    // while an invalid/truncated frame is being read. After
                    // restoring the previous custody, keep pending only when
                    // there is actual DNS responsibility left to discharge.
                    DnsChange::Unmodified => {
                        guard.dns_cleanup_pending
                            && (guard.dns_owner.is_some() || guard.dns_birth.is_some())
                    }
                    DnsChange::Unconfirmed => true,
                };
                guard.dns_busy = false;
            }
        }
        return;
    }

    // 5. 持锁按 command 分发（Go: mu.Lock(); defer mu.Unlock()）。
    let mut guard = match state.lock() {
        Ok(g) => g,
        Err(e) => {
            // 锁中毒（panic 残留）—— 极少见，回报 unknown。
            let _ = conn.write_line(&format!("ERR unknown {e}"));
            return;
        }
    };
    dispatch_locked(&mut guard, deps, &cred, &command, conn);
}

/// 持锁的命令分发（对照 Go switch cmd { ... }，:362-481）。
fn dispatch_locked<P, S, D, SD>(
    state: &mut HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    cred: &PeerCred,
    command: &str,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    if state.shutting_down {
        let _ = conn.write_line("ERR helper-shutting-down");
        return;
    }
    match command {
        cmd::PREPARE_HELPER_UPGRADE | cmd::CANCEL_HELPER_UPGRADE => {
            if cred.uid != 0 {
                let _ = conn.write_line("ERR unauthorized");
                return;
            }
            if !matches!(conn.read_line_strict(), Ok(None)) {
                let _ = conn.write_line("ERR bad-args");
                return;
            }
            if state.dns_busy
                || state.dns_cleanup_pending
                || state.dns_owner.is_some()
                || !matches!(
                    deps.spawner
                        .birth_admission(state.child.as_ref().map(ManagedChild::handle)),
                    BirthAdmission::Admitted
                )
            {
                let _ = conn.write_line("ERR helper-upgrade-busy");
                return;
            }
            state.child = None;
            state.upgrade_closed = command == cmd::PREPARE_HELPER_UPGRADE;
            let _ = conn.write_line(if state.upgrade_closed {
                "OK helper-upgrade-admitted"
            } else {
                "OK helper-upgrade-cancelled"
            });
        }
        cmd::CORE_STATUS => {
            let (Ok(Some(id)), Ok(None)) = (conn.read_line_strict(), conn.read_line_strict())
            else {
                let _ = conn.write_line("ERR bad-args");
                return;
            };
            let Ok(id) = polaris_helper_proto::core_manifest::CoreId::parse(&id) else {
                let _ = conn.write_line("ERR bad-args");
                return;
            };
            let response = Response::Ok(ResponseKind::CoreStatus(crate::pinned_core::status(
                deps.core_dir,
                &id,
            )));
            let _ = conn.write_line(&response.to_wire_line());
        }
        lcmd::STATUS_BIRTH_SAFE => {
            if caller_owns_slot(state, deps.spawner, cred, conn) {
                handle_birth_status(state, deps, conn);
            }
        }
        lcmd::STOP_BIRTH_SAFE => {
            handle_birth_stop(state, deps, cred, conn);
        }
        lcmd::START_PINNED_BIRTH => handle_start(state, deps, cred, conn),
        // Retired executable-path/content-write commands never dispatch, even
        // when no child is running. Only administrative CLI installs bytes.
        cmd::START | lcmd::START_REAP_SAFE | lcmd::START_BIRTH_SAFE | lcmd::INSTALL_CORE => {
            let _ = conn.write_line("ERR unknown");
        }
        _ if legacy_command_rejected(command, state, deps.spawner) => {
            let _ = conn.write_line("ERR unknown");
        }
        cmd::STATUS => handle_status(state, conn),
        cmd::STOP | lcmd::STOP_REAP_SAFE => handle_stop(state, deps, conn),
        cmd::CLEANUP => handle_cleanup(state, deps, cred, conn),
        cmd::FREEPORT => handle_freeport(deps, cred, conn),
        _ => {
            let _ = conn.write_line("ERR unknown");
        }
    }
}

fn caller_owns_slot(
    state: &HandlerState,
    spawner: &impl CoreSpawner,
    cred: &PeerCred,
    conn: &mut impl Conn,
) -> bool {
    if !spawner.caller_allowed(
        cred.uid,
        state.child.as_ref().map(ManagedChild::handle),
        None,
    ) {
        let _ = conn.write_line("ERR slot-owned-by-other-user");
        false
    } else {
        true
    }
}

fn birth_start_line(
    id: Option<&polaris_helper_proto::core_manifest::CoreId>,
    start: LinuxBirthStart,
) -> String {
    let kind = match id {
        Some(id) => ResponseKind::PinnedStart {
            id: id.clone(),
            start: polaris_helper_proto::core_protocol::PinnedBirthStart::Linux(start),
        },
        None => ResponseKind::LinuxBirthStart(start),
    };
    Response::Ok(kind).to_wire_line()
}

fn legacy_command_rejected(
    command: &str,
    state: &HandlerState,
    spawner: &impl CoreSpawner,
) -> bool {
    matches!(
        command,
        cmd::START
            | lcmd::START_REAP_SAFE
            | cmd::STOP
            | lcmd::STOP_REAP_SAFE
            | cmd::STATUS
            | cmd::CLEANUP
            | cmd::FREEPORT
            | lcmd::INSTALL_CORE
    ) && (state
        .child
        .as_ref()
        .is_some_and(|child| child.handle().target().is_some())
        || spawner.has_exact_birth())
}

// ===== 各命令处理（逐分支对照 Go 源）=====

/// A refused request is distinct from a confirmed rollback of existing DNS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DnsChange {
    Applied,
    Unmodified,
    Restored,
    Unconfirmed,
}

fn handle_resolved_dns_set(ops: &dyn ResolvedDnsOps, conn: &mut impl Conn) -> DnsChange {
    let interface_name = conn.read_line();
    let server_ip = conn.read_line();
    if !polaris_helper_proto::linux_dns::takeover_request_allowed(&interface_name, &server_ip) {
        let _ = conn.write_line("ERR resolved-dns request denied by Polaris resolved whitelist");
        return DnsChange::Unmodified;
    }
    let (response, outcome) = match ops.takeover(&interface_name, &server_ip) {
        Ok(()) => (
            Response::Ok(ResponseKind::LinuxDns(LinuxDns::Set)),
            DnsChange::Applied,
        ),
        Err(error) => {
            let outcome = if ops.revert(&interface_name).is_ok() {
                DnsChange::Restored
            } else {
                DnsChange::Unconfirmed
            };
            (
                Response::Err(ProtoError::with_detail(
                    ErrorCode::ResolvedDns,
                    single_line_wire_detail(&error),
                )),
                outcome,
            )
        }
    };
    let _ = conn.write_line(&response.to_wire_line());
    outcome
}

fn handle_resolved_dns_revert(ops: &dyn ResolvedDnsOps, conn: &mut impl Conn) -> DnsChange {
    let interface_name = conn.read_line();
    if interface_name != polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME {
        let _ = conn.write_line("ERR resolved-dns request denied by Polaris resolved whitelist");
        return DnsChange::Unmodified;
    }
    let (response, outcome) = match ops.revert(&interface_name) {
        Ok(()) => (
            Response::Ok(ResponseKind::LinuxDns(LinuxDns::Reverted)),
            DnsChange::Restored,
        ),
        Err(error) => (
            Response::Err(ProtoError::with_detail(
                ErrorCode::ResolvedDns,
                single_line_wire_detail(&error),
            )),
            DnsChange::Unconfirmed,
        ),
    };
    let _ = conn.write_line(&response.to_wire_line());
    outcome
}

/// helper wire 一次响应只能占一行；resolvectl stderr 可能含换行，必须先压平，避免截断诊断或注入伪帧。
fn single_line_wire_detail(detail: &str) -> String {
    detail.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// status（:363-368）：running `<pid>` 或 stopped。
fn handle_status(state: &HandlerState, conn: &mut impl Conn) {
    if let Some(h) = state.child.as_ref().map(ManagedChild::handle) {
        let _ = conn.write_line(&format!("OK running {}", h.pid));
    } else {
        let _ = conn.write_line("OK stopped");
    }
}

fn handle_birth_status<P, S, D, SD>(
    state: &HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    if !matches!(conn.read_line_strict(), Ok(None)) {
        let _ = conn.write_line("ERR bad-args");
        return;
    }
    let current = state.child.as_ref().map(ManagedChild::handle);
    let outcome = match deps.spawner.birth_admission(current) {
        BirthAdmission::Admitted => LinuxBirthStatus::Empty,
        BirthAdmission::Already(target) if state.child.as_ref().is_some_and(|child| {
            matches!(child, ManagedChild::Stopping(handle) if handle.target() == Some(target))
        }) => LinuxBirthStatus::Stopping { target },
        BirthAdmission::Already(target) => LinuxBirthStatus::Running { target },
        BirthAdmission::Blocked {
            target: Some(target),
            state: ReapBlockerState::Pending,
        } if state.child.as_ref().is_some_and(|child| {
            matches!(child, ManagedChild::Stopping(handle) if handle.target() == Some(target))
        }) => LinuxBirthStatus::Stopping { target },
        BirthAdmission::Blocked {
            target: Some(target),
            ..
        } => LinuxBirthStatus::Unknown { target },
        // 带尾文：裸的 `ERR unknown` 在客户端眼里是「这个 helper 不认识本命令」，而这里是认识
        // 命令、只是手里的 custody 认不出归属。
        BirthAdmission::Blocked { target: None, .. } => {
            let _ = conn.write_line("ERR unknown birth-custody-unidentified");
            return;
        }
    };
    let _ = conn.write_line(&Response::Ok(ResponseKind::LinuxBirthStatus(outcome)).to_wire_line());
}

fn handle_birth_stop<P, S, D, SD>(
    state: &mut HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    cred: &PeerCred,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    let (Ok(Some(pid)), Ok(Some(birth)), Ok(None)) = (
        conn.read_line_strict(),
        conn.read_line_strict(),
        conn.read_line_strict(),
    ) else {
        let _ = conn.write_line("ERR bad-args");
        return;
    };
    let Some(target) = parse_linux_birth_stop_args(&[&pid, &birth]) else {
        let _ = conn.write_line("ERR bad-args");
        return;
    };
    if !deps.spawner.caller_allowed(
        cred.uid,
        state.child.as_ref().map(ManagedChild::handle),
        Some(&target),
    ) {
        let _ = conn.write_line("ERR slot-owned-by-other-user");
        return;
    }
    if let Some(current) = state.child.as_ref().map(ManagedChild::handle) {
        match current.target() {
            Some(current) if current != target => {
                let response = if deps.spawner.reaped_birth(&target) {
                    LinuxBirthStop::Stopped { target }
                } else {
                    LinuxBirthStop::Mismatch {
                        requested: target,
                        current,
                    }
                };
                let _ = conn.write_line(
                    &Response::Ok(ResponseKind::LinuxBirthStop(response)).to_wire_line(),
                );
                return;
            }
            None => {
                // A newer Legacy child cannot hide native reap proof for an
                // older exact birth whose Stop ACK was lost. Never pass this
                // request to the spawner while the Legacy child is current.
                let response = if deps.spawner.reaped_birth(&target) {
                    LinuxBirthStop::Stopped { target }
                } else {
                    LinuxBirthStop::Unknown { target }
                };
                let _ = conn.write_line(
                    &Response::Ok(ResponseKind::LinuxBirthStop(response)).to_wire_line(),
                );
                return;
            }
            Some(_) => {}
        }
    }
    let same_current = state
        .child
        .as_ref()
        .is_some_and(|child| child.handle().target() == Some(target));
    if same_current {
        let handle = state.child.as_ref().unwrap().handle().clone();
        state.child = Some(ManagedChild::Stopping(handle));
    }
    let outcome = match deps.spawner.stop_birth(&target) {
        StopReap::Reaped => {
            if same_current {
                let handle = state.child.as_ref().unwrap().handle().clone();
                state.retire_birth(&handle);
                (deps.set_forward)(false);
            }
            LinuxBirthStop::Stopped { target }
        }
        StopReap::Pending => LinuxBirthStop::Pending { target },
        StopReap::Unknown => LinuxBirthStop::Unknown { target },
    };
    let _ = conn.write_line(&Response::Ok(ResponseKind::LinuxBirthStop(outcome)).to_wire_line());
}

/// stop（:369-380）：**受管 pid 身份校验** → 启动/轮询后台终止 → 仅在 native reap 后复位转发态。
///
/// 身份行（可选，本协议新增）：客户端声明它意图停的那个 pid。判据走
/// [`stop_pid_matches`] —— 不匹配 = 手里这个核属**另一个会话**（客户端的老 stop 腿在 IPC 上挂住
/// 期间，用户已经重装 helper / 重新起了核），此时杀它就是把用户刚连上的核静默掐掉。故不匹配一律
/// 诚实 no-op（`OK stop-mismatch <want> <current>`），绝不「反正要停就杀当前的」。
///
/// 读身份行发生在**持锁临界区内**（与 start/freeport/install-core 的参数行读同款）：连接级 5s 读
/// 超时（`server.rs` 的 `set_read_timeout`）是这条读的上界，客户端写完即 `shutdown` ⇒ 正常路径
/// 立刻 EOF 返 ""。
fn handle_stop<P, S, D, SD>(
    state: &mut HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    // 旧客户端不发这一行 → read_line 在 EOF 返 "" → None → 沿用「停当前受管核」旧语义。
    let want = parse_stop_pid(&conn.read_line());
    if let Some(h) = state.child.as_ref().map(ManagedChild::handle) {
        if !stop_pid_matches(want, h.pid) {
            let resp = Response::Ok(ResponseKind::Stop(Stop::Mismatch {
                want: want.unwrap_or(0),
                current: h.pid,
            }));
            let _ = conn.write_line(&resp.to_wire_line());
            return;
        }
    }
    let Some(handle) = state.child.as_ref().map(ManagedChild::handle).cloned() else {
        // `child == None` alone is not a reap proof: cleanup/older state may
        // still leave a native slot. Reuse the all-slot admission scan before
        // emitting the idempotent success response.
        match deps.spawner.start_admission(None) {
            StartAdmission::Admitted => {
                let _ = conn.write_line("OK notrunning");
            }
            StartAdmission::Blocked { pid, state } => write_stop_blocked(conn, pid, state),
            StartAdmission::Already { pid } => {
                write_stop_blocked(conn, pid, ReapBlockerState::Unknown);
            }
        }
        return;
    };

    let pid = handle.pid;
    state.child = Some(ManagedChild::Stopping(handle.clone()));
    match deps.spawner.terminate(&handle) {
        StopReap::Reaped => {
            // The exact native Child has been reaped. Only this branch may
            // clear custody, reset forwarding, and acknowledge Stop success.
            if state
                .child
                .as_ref()
                .is_some_and(|child| child.handle().same_birth(&handle))
            {
                state.retire_birth(&handle);
            }
            (deps.set_forward)(false);
            let response = Response::Ok(ResponseKind::Stop(Stop::Stopped { pid }));
            let _ = conn.write_line(&response.to_wire_line());
        }
        StopReap::Pending => write_stop_blocked(conn, pid, ReapBlockerState::Pending),
        StopReap::Unknown => write_stop_blocked(conn, pid, ReapBlockerState::Unknown),
    }
}

fn write_stop_blocked(conn: &mut impl Conn, pid: u32, state: ReapBlockerState) {
    let outcome = match state {
        ReapBlockerState::Pending => Stop::Pending { pid },
        ReapBlockerState::Unknown => Stop::Unknown { pid },
    };
    let _ = conn.write_line(&Response::Ok(ResponseKind::Stop(outcome)).to_wire_line());
}

/// Legacy cleanup stops only the retained child; never process-name discovery.
fn handle_cleanup<P, S, D, SD>(
    state: &mut HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    _cred: &PeerCred,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    if let Some(h) = state.child.as_ref().map(ManagedChild::handle).cloned() {
        // :383: kill child。
        deps.spawner.kill(&h);
        state.child = Some(ManagedChild::Stopping(h));
    }
    (deps.set_forward)(false);
    // Legacy cleanup has no authority over other same-user processes.
    // Only the retained child above may be stopped; never rediscover by name/PID.
    let _ = conn.write_line("OK cleaned");
}

/// freeport（:389-395）：按端口找 LISTEN 持有者。
fn handle_freeport<P, S, D, SD>(
    deps: &HandlerDeps<'_, P, S, D, SD>,
    cred: &PeerCred,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    // :390: 读 port 行。
    let port = conn.read_line();
    let port_trim = port.trim();
    // :391: 校验纯数字（Go: IndexFunc 非 '0'-'9' 即拒绝）。
    if port_trim.is_empty() || !port_trim.bytes().all(|b| b.is_ascii_digit()) {
        let _ = conn.write_line("ERR bad-port");
        return;
    }
    // :392: ss -H -ltnp 'sport = :<port>'。
    let ss_out = (deps.ss_provider)(port_trim);
    let Some(ss_out) = ss_out else {
        let _ = conn.write_line("ERR port-inspection-failed");
        return;
    };
    let pids = parse_ss_pids(&ss_out);
    // A listener without a visible PID remains occupied/unknown, never free.
    if !ss_out.trim().is_empty() && pids.is_empty() {
        let _ = conn.write_line("OK foreign unknown");
        return;
    }
    // :395: free_port 分发。wire 序列化走协议层单一真值（G3.1/G3.3）。
    let outcome = free_port(&pids, cred.uid, deps.freeport_deps);
    let resp = Response::Ok(ResponseKind::FreePort(outcome));
    let _ = conn.write_line(&resp.to_wire_line());
}

fn verified_selected<P, S, D, SD>(
    deps: &HandlerDeps<'_, P, S, D, SD>,
    root: &Path,
    id: &polaris_helper_proto::core_manifest::CoreId,
) -> std::io::Result<Arc<crate::core_store::VerifiedCore>>
where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    #[cfg(test)]
    if let Some(core) = &deps.test_core {
        if core.manifest.id != id.as_str() {
            return Err(std::io::Error::other("fixture selected id differs"));
        }
        core.recheck_fixture()?;
        return Ok(Arc::clone(core));
    }
    #[cfg(not(test))]
    let _ = deps;
    crate::pinned_core::verify(root, id).map(Arc::new)
}

/// The only executable admission route: a strict selected-id frame and a
/// verified immutable object, followed by native custody and caller checks.
fn handle_start<P, S, D, SD>(
    state: &mut HandlerState,
    deps: &HandlerDeps<'_, P, S, D, SD>,
    cred: &PeerCred,
    conn: &mut impl Conn,
) where
    P: PeerCredProvider,
    S: CoreSpawner,
    D: FreePortDeps,
    SD: SystemdOps,
{
    if state.upgrade_closed {
        let _ = conn.write_line("ERR helper-upgrade-admission-closed");
        return;
    }
    if state.dns_busy
        || state.dns_cleanup_pending
        || state
            .dns_owner
            .is_some_and(|owner| cred.uid != 0 && owner != cred.uid)
    {
        let _ = conn.write_line("ERR slot-owned-or-control-busy");
        return;
    }
    let mut lines = Vec::new();
    for _ in 0..4 {
        match conn.read_line_strict() {
            Ok(Some(line)) => lines.push(line),
            _ => {
                let _ = conn.write_line("ERR bad-args");
                return;
            }
        }
    }
    match conn.read_line_strict() {
        Ok(Some(line)) => lines.push(line),
        Ok(None) => {}
        Err(_) => {
            let _ = conn.write_line("ERR bad-args");
            return;
        }
    }
    if !matches!(conn.read_line_strict(), Ok(None)) {
        let _ = conn.write_line("ERR bad-args");
        return;
    }
    let Some(params) = polaris_helper_proto::core_protocol::PinnedStartParams::parse(&lines) else {
        let _ = conn.write_line("ERR bad-args");
        return;
    };
    if !caller_owns_slot(state, deps.spawner, cred, conn) {
        return;
    }
    let id = &params.id;
    let current = state.child.as_ref().map(ManagedChild::handle);
    match deps.spawner.birth_admission(current) {
        BirthAdmission::Admitted => state.child = None,
        BirthAdmission::Already(target) => {
            // This proof comes from the retained native registry, including
            // hidden slots. Never relabel a different core as this request's id.
            if !deps.spawner.birth_core_matches(&target, current, id) {
                let _ = conn.write_line("ERR core-occupied");
                return;
            }
            let Some(root) = deps.core_dir else {
                let _ = conn.write_line("ERR coredir-unset");
                return;
            };
            if verified_selected(deps, root, id).is_err() {
                let _ = conn.write_line("ERR core-invalid");
                return;
            }
            let result = if state.child.as_ref().is_some_and(|child|
                matches!(child, ManagedChild::Stopping(handle) if handle.target() == Some(target))) {
                LinuxBirthStart::NotAdmittedPending { target }
            } else { LinuxBirthStart::Already { target } };
            let _ = conn.write_line(&birth_start_line(Some(id), result));
            return;
        }
        BirthAdmission::Blocked {
            target,
            state: blocker,
        } => {
            let result = match (target, blocker) {
                (Some(target), ReapBlockerState::Pending) => {
                    LinuxBirthStart::NotAdmittedPending { target }
                }
                (target, _) => LinuxBirthStart::NotAdmittedUnknown { target },
            };
            let _ = conn.write_line(&birth_start_line(Some(id), result));
            return;
        }
    }
    let Some(root) = deps.core_dir else {
        let _ = conn.write_line("ERR coredir-unset");
        return;
    };
    let core = match verified_selected(deps, root, id) {
        Ok(core) => core,
        Err(_) => {
            let _ = conn.write_line("ERR core-invalid");
            return;
        }
    };
    let cfg = Path::new(&params.common.cfg);
    if !matches!(owned_by(cfg, cred.uid), Ok(true)) {
        let _ = conn.write_line("ERR config-not-owned");
        return;
    }
    let log = if params.common.log.is_empty() {
        None
    } else {
        let log = Path::new(&params.common.log);
        if log.parent() != cfg.parent() {
            let _ = conn.write_line("ERR log-path-denied");
            return;
        }
        Some(log.to_path_buf())
    };
    let birth = match mint_birth_token() {
        Ok(birth) => birth,
        Err(error) => {
            log::error!("Linux helper birth entropy unavailable: {error}");
            let _ = conn.write_line("ERR start birth-entropy");
            return;
        }
    };
    let total_started = Instant::now();
    let forwarding_started = Instant::now();
    (deps.set_forward)(params.common.fwd);
    let forwarding_ms = crate::elapsed_ms(forwarding_started);
    let process_prepare_started = Instant::now();
    let req = SpawnCoreRequest {
        birth: Some(birth),
        binary: core.directory.join("sing-box"),
        config: cfg.to_path_buf(),
        log,
        fwd: params.common.fwd,
        parent_pid: params.common.parent_pid,
        uid: cred.uid,
        gid: cred.gid,
        groups: supplementary_groups(cred.uid),
    };
    let process_prepare_ms = crate::elapsed_ms(process_prepare_started);
    match deps.spawner.spawn_pinned(&req, core) {
        Ok(started) => {
            let target = started.handle.target();
            state.child = Some(ManagedChild::Running(started.handle));
            match target {
                Some(target) => {
                    let timing = StartTiming {
                        forwarding_ms,
                        process_ms: started.process_ms.saturating_add(process_prepare_ms),
                        job_ms: 0,
                        log_handoff_ms: started.log_handoff_ms,
                        total_ms: crate::elapsed_ms(total_started),
                    };
                    let _ = conn.write_line(&birth_start_line(
                        Some(id),
                        LinuxBirthStart::Started {
                            target,
                            timing: Some(timing),
                        },
                    ));
                }
                None => {
                    let _ = conn.write_line("ERR start missing-birth");
                }
            }
        }
        Err(SpawnError::Spawn { detail }) => {
            (deps.set_forward)(false);
            let _ = conn.write_line(&format!("ERR start {detail}"));
        }
    }
}

fn mint_birth_token() -> std::io::Result<HelperBirthToken> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(HelperBirthToken::from_bytes(bytes))
}

#[cfg(test)]
mod tests;

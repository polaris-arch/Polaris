//! Unix socket 服务端（移植自 上游 `helper/helper.go:591-643` 的 `main()`）。
//!
//! ## Go 源结构（`helper.go:604-642`）
//!
//! ```text
//! sockPath := filepath.Join(supportDir, "helper.sock")
//! os.Remove(sockPath)
//! l, _ := net.Listen("unix", sockPath)
//! os.Chmod(sockPath, 0666)   // token 为安全边界
//! // SIGTERM/SIGINT 收割器（proto v3）
//! for { conn, _ := l.Accept(); go handle(conn) }
//! ```
//!
//! ## 移植纪律
//!
//! socket 监听 + accept 循环是 mac-gated 系统操作（`std::os::unix::net::UnixListener`）。
//! 帧解析（token 行 + command + args → [`Request`]）是跨平台纯逻辑 —— 抽为
//! [`decode_request`]，可在 Linux 上完整测「Go wire 形态 → Request」的解析正确性。
//!
//! ## 安全（`helper.go:3-9,611`）
//!
//! socket 权限 0666（任何本地进程可连）—— **token 行是唯一安全边界**（见 [`crate::token`]）。
//! 0666 是为让普通用户 app 能连；远程不可达（unix socket 仅本机）。

use crate::line_io::{read_line_trimmed_bounded, write_line, BoundedLineError};
use crate::platform::macos::handler::{dispatch_from_peer, MacConfig, MacServices};
use polaris_helper_proto::request::{InstallCoreParams, RouteParams, StartParams};
use polaris_helper_proto::{parse_stop_pid, Request};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Mutex;
use std::time::Duration;

/// socket 文件名（`helper.go:604`：`helper.sock`）。
pub const SOCK_FILENAME: &str = "helper.sock";

/// socket 权限 0666（`helper.go:611`，token 为安全边界）。
#[cfg(unix)]
pub const SOCK_MODE: u32 = 0o666;

/// 连接读超时（`helper.go:401`：5s）。
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// token、命令和任意参数行的统一 wire 上限。检查发生在读取过程中，而非分配完整行之后。
pub const MAX_WIRE_LINE_BYTES: usize = polaris_helper_proto::command::mac::MAX_WIRE_LINE_BYTES;

/// 帧解码错误（wire 形态不符合预期）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// 命令名未识别（Go `default: ERR unknown`，`helper.go:587`）。
    UnknownCommand(String),
    /// 缺少必要参数行（如 start 无 cfg）。
    MissingArg(&'static str),
    /// 任一命令或参数行超过 wire 硬上限。
    LineTooLong,
}

fn checked_line<I: Iterator<Item = String>>(
    lines: &mut I,
    required: Option<&'static str>,
) -> Result<Option<String>, DecodeError> {
    let line = lines.next();
    if line
        .as_ref()
        .is_some_and(|line| line.len() > MAX_WIRE_LINE_BYTES)
    {
        return Err(DecodeError::LineTooLong);
    }
    if line.is_none() {
        if let Some(name) = required {
            return Err(DecodeError::MissingArg(name));
        }
    }
    Ok(line)
}

fn required_line<I: Iterator<Item = String>>(
    lines: &mut I,
    name: &'static str,
) -> Result<String, DecodeError> {
    Ok(checked_line(lines, Some(name))?.expect("required line checked above"))
}

fn optional_line<I: Iterator<Item = String>>(lines: &mut I) -> Result<String, DecodeError> {
    Ok(checked_line(lines, None)?.unwrap_or_default())
}

/// 从「行流」解码出 [`Request`]（移植自 Go `handle()` 的 readLine 序列 + switch）。
///
/// 对照 Go `helper.go:403-585`：先读 command 行，再按命令读对应数量的参数行。
/// `lines` 是已读到的参数行迭代器（token 行已由调用方先消费做鉴权）。
///
/// 严格对照各 case 的 readLine 顺序：
/// - ping/version/status/stop/cleanup/flush-dns：无参数行
/// - freeport：1 行 port
/// - start：cfg/log/fwd/ppid(可选)
/// - route-add/route-del：iface/cidrs
/// - install-core：src/wantHash
/// - default-restore：gateway
/// - system-proxy-transaction：hex(JSON) 原子事务 payload
///
/// ppid 可选行（Go `helper.go:513`：`strconv.Atoi(readLine(r))`，EOF 返回 "" → 0 → 不启看护）。
pub fn decode_request<I: Iterator<Item = String>>(
    command: &str,
    lines: &mut I,
) -> Result<Request, DecodeError> {
    if command.len() > MAX_WIRE_LINE_BYTES {
        return Err(DecodeError::LineTooLong);
    }
    match command {
        "ping" => Ok(Request::Ping),
        "version" => Ok(Request::Version),
        "status" => Ok(Request::Status),
        polaris_helper_proto::command::common::NATIVE_STATUS_BIRTH => {
            Ok(Request::NativeStatusBirth)
        }
        polaris_helper_proto::command::common::NATIVE_STOP_BIRTH => {
            let pid = required_line(lines, "pid")?;
            let birth = required_line(lines, "birth")?;
            if checked_line(lines, None)?.is_some() {
                return Err(DecodeError::MissingArg("exact native birth frame"));
            }
            let target = polaris_helper_proto::parse_native_birth_stop_args(&[&pid, &birth])
                .ok_or(DecodeError::MissingArg("native birth target"))?;
            Ok(Request::NativeStopBirth { target })
        }
        // stop 的受管 pid 身份行是**可选**的（旧客户端不发 → LineIter 在 EOF 产 None → `None`，
        // 沿用「停当前受管核」旧语义）。见 `polaris_helper_proto::stop_pid_matches`。
        "stop" => Ok(Request::Stop {
            pid: parse_stop_pid(&optional_line(lines)?),
        }),
        "cleanup" => Ok(Request::Cleanup),
        "flush-dns" => Ok(Request::FlushDns),
        "system-proxy-transaction" => {
            let payload_hex = required_line(lines, "payload_hex")?.trim().to_owned();
            if payload_hex.is_empty() {
                return Err(DecodeError::MissingArg("payload_hex"));
            }
            Ok(Request::MacProxyTransaction { payload_hex })
        }
        "system-proxy-compare-transaction" => {
            let payload_hex = required_line(lines, "payload_hex")?.trim().to_owned();
            if payload_hex.is_empty() {
                return Err(DecodeError::MissingArg("payload_hex"));
            }
            Ok(Request::MacProxyCompareTransaction { payload_hex })
        }
        "system-proxy-compare-capability" => Ok(Request::MacProxyCompareCapability),
        "freeport" => {
            // helper.go:362: port := TrimSpace(readLine)
            let port_str = optional_line(lines)?.trim().to_owned();
            let port: u16 = port_str
                .parse()
                .map_err(|_| DecodeError::MissingArg("port"))?;
            Ok(Request::FreePort { port })
        }
        "start" | polaris_helper_proto::command::common::NATIVE_START_BIRTH => {
            // helper.go:508-513: cfg/log/fwd/ppid
            let cfg = required_line(lines, "cfg")?;
            let log = optional_line(lines)?;
            let fwd_str = optional_line(lines)?;
            let fwd = fwd_str.trim() == "1";
            // helper.go:513: ppid 可选（EOF → "" → 0 → None）
            let ppid_str = optional_line(lines)?;
            let parent_pid = ppid_str.trim().parse::<u32>().ok().filter(|&p| p > 0);
            let params = StartParams {
                cfg,
                log,
                fwd,
                parent_pid,
            };
            Ok(if command == "start" {
                Request::Start(params)
            } else {
                Request::NativeStartBirth(params)
            })
        }
        "route-add" | "route-del" => {
            // helper.go:455-456: iface/cidrs
            let iface = required_line(lines, "iface")?;
            let cidrs_line = optional_line(lines)?;
            let cidrs: Vec<String> = cidrs_line
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            let req = Request::RouteAdd(RouteParams { iface, cidrs });
            if command == "route-del" {
                // 重建为 RouteDel（字段同构）
                if let Request::RouteAdd(rp) = req {
                    return Ok(Request::RouteDel(rp));
                }
            }
            Ok(req)
        }
        "install-core" => {
            // helper.go:583-584: src/wantHash
            let src_dir = required_line(lines, "src_dir")?;
            let want_hash = required_line(lines, "want_hash")?.trim().to_owned();
            Ok(Request::InstallCore(InstallCoreParams {
                src_dir,
                want_hash,
            }))
        }
        "default-restore" => {
            // helper.go:485: gateway
            let gateway_ipv4 = required_line(lines, "gateway_ipv4")?.trim().to_owned();
            Ok(Request::DefaultRestore { gateway_ipv4 })
        }
        other => Err(DecodeError::UnknownCommand(other.to_owned())),
    }
}

/// 连接处理结果。
#[derive(Debug)]
pub enum ConnOutcome {
    /// 正常处理完毕。
    Done,
    /// 读/写 IO 错误。
    IoError(String),
}

// ===== 纯决策逻辑（跨平台可测；mac-gated 生命周期代码经此委托，杜绝 syscall 内藏判定）=====

/// chownRuntimeDirs 归还属主的运行时子目录名（移植自 `helper.go:222`）。
///
/// root 跑 sing-box 会把这些目录里的文件写成 root 600 → 登录用户跑读不了 → endpoint post-start FATAL。
pub const CHOWN_SUBDIRS: [&str; 3] = ["tailscale", "singbox-dashboard", "ui"];

/// root 核会直接写在 confDir 根部、退出后必须归还登录用户的运行时文件。
///
/// `cache.db` 不在 [`CHOWN_SUBDIRS`] 的树内；漏掉它会让 TUN(root) → 系统模式(user) 的下一次
/// 起核以 `initialize cache-file: permission denied` 退出。启动前另有 fd 级属主准备，退出归还是
/// 对旧 helper 遗留文件与异常退出的第二道收口。
pub const CHOWN_FILES: [&str; 1] = ["cache.db"];

/// 某条目是否需 `Lchown` 归还（移植自 `helper.go:237`）。
///
/// 仅 root（uid==0）写入的条目才归还 —— 已是登录用户属主的跳过，省无谓 Lchown（TUN 起停周期对可能很大的树）。
#[must_use]
pub fn should_chown_entry(entry_uid: u32) -> bool {
    entry_uid == 0
}

/// confDir 本身属 root 时是否跳过整个 chown（移植自 `helper.go:219-221`）。
///
/// confDir 应属登录用户（app 数据目录）；若属 root（异常）→ 不动，避免把运行时目录误归 root。
#[must_use]
pub fn should_skip_confdir_chown(confdir_uid: u32) -> bool {
    confdir_uid == 0
}

/// terminateChild 是否须升级 SIGKILL（移植自 `helper.go:282-286`）。
///
/// `exited`=进程是否已在宽限窗口内退出（`done` 触发）。未退出 → SIGKILL 强杀。
#[must_use]
pub fn terminate_needs_kill(exited: bool) -> bool {
    !exited
}

/// 处理一个已 accept 的连接（移植自 Go `handle()`，`helper.go:397-589`）。
///
/// 流程：
/// 1. 设读超时 5s（`helper.go:401`）。
/// 2. 读 token 行 + command 行。
/// 3. 解码 command + 参数行为 [`Request`]。
/// 4. `dispatch` 得 [`Response`](polaris_helper_proto::Response)。
/// 5. 写回 wire 行。
///
/// 泛型 `R: Read` 让测试可注入 `Cursor<&[u8]>`，生产接 `UnixStream`。
///
/// `command_mu` also serializes freeport with native admission: a PID cleanup cannot race Start.
/// All commands use the lock after authentication (the original Go used it for other commands).
/// 生产 serve 传 `Some(&command_mu)`；单线程测试传 `None`（无锁）。
pub fn process_connection<R: Read, W: Write>(
    reader: R,
    writer: W,
    services: &dyn MacServices,
    config: &MacConfig,
    command_mu: Option<&Mutex<()>>,
) -> ConnOutcome {
    process_connection_from_peer(reader, writer, services, config, command_mu, None)
}

/// [`process_connection`] 加上对端 uid（生产 serve 在 accept 后用 `getpeereid` 取，取不到传
/// `None`）。只有 install-core 消费它，语义见 [`dispatch_from_peer`]。
pub fn process_connection_from_peer<R: Read, W: Write>(
    reader: R,
    mut writer: W,
    services: &dyn MacServices,
    config: &MacConfig,
    command_mu: Option<&Mutex<()>>,
    peer_uid: Option<u32>,
) -> ConnOutcome {
    let mut buf = BufReader::new(reader);
    // helper.go:403-404: token 行 + command 行（锁前读，与 Go 一致）
    let token = match read_line_trimmed_bounded(&mut buf, MAX_WIRE_LINE_BYTES) {
        Ok(Some(s)) => s,
        Ok(None) | Err(BoundedLineError::Io(_)) => return ConnOutcome::Done,
        Err(BoundedLineError::TooLong { .. }) => return write_line_too_long(&mut writer),
    };
    let command = match read_line_trimmed_bounded(&mut buf, MAX_WIRE_LINE_BYTES) {
        Ok(Some(s)) => s,
        Ok(None) | Err(BoundedLineError::Io(_)) => return ConnOutcome::Done,
        Err(BoundedLineError::TooLong { .. }) => return write_line_too_long(&mut writer),
    };

    // 🔴 **鉴权早退必须在取锁之前**（`dispatch` 里那道保留作纵深）。
    //
    // socket 是 0666（设计如此，token 行是唯一安全边界，见模块头）。若先取锁再鉴权，一条**未鉴权**
    // 的连接就能：写 token+command 两行后不再写任何数据 → 取到全局
    // `command_mu` → 在锁内的 `decode_request` 阻塞读参数行，直到 5s 连接读超时才放锁。
    // 即「零 token、单条连接 = 独占 root daemon 命令锁 5 秒」；serve 每连接一线程且无并发上限，
    // 循环开 N 条即可把锁占满 —— 期间 GUI 侧 stop/start/status/flush-dns 全部排队超时，
    // 用户点「断开」得到通信失败，而 root sing-box 与 TUN 仍在跑，网络接管解除不了。
    //
    // linux 腿与 Go 源都是**先鉴权后取锁**（`linux/handler.rs` 的步骤 4→5、Go `helper.go` 的
    // `tok != tokenValue() → ERR auth; return` 在 `mu.Lock()` 之前），本处属移植时的次序回退。
    if !matches!(
        crate::token::check_token(&token, &services.token_store().token_value()),
        crate::token::TokenCheck::Authed
    ) {
        let resp = polaris_helper_proto::Response::Err(polaris_helper_proto::Error::new(
            polaris_helper_proto::ErrorCode::Auth,
        ));
        return write_response(&mut writer, &resp);
    }

    // Freeport's native-slot check and PID-based cleanup share Start's command lock.
    // Otherwise it could observe an empty slot, then kill a newly published owned Child.
    let _mu_guard = command_mu.map(|m| m.lock().unwrap_or_else(std::sync::PoisonError::into_inner));

    // 解码命令 + 参数行
    let mut lines_iter = LineIter {
        buf: &mut buf,
        too_long: false,
    };
    let decoded = decode_request(&command, &mut lines_iter);
    if lines_iter.too_long {
        return write_line_too_long(&mut writer);
    }
    let req = match decoded {
        Ok(r) => r,
        Err(DecodeError::LineTooLong) => return write_line_too_long(&mut writer),
        // helper.go:587: 未知命令 / 参数不足 → 统一兜底 ERR unknown（避免歧义）
        Err(DecodeError::UnknownCommand(_) | DecodeError::MissingArg(_)) => {
            let resp = polaris_helper_proto::Response::Err(polaris_helper_proto::Error::new(
                polaris_helper_proto::ErrorCode::Unknown,
            ));
            return write_response(&mut writer, &resp);
        }
    };
    let resp = dispatch_from_peer(services, config, &token, &req, peer_uid);
    write_response(&mut writer, &resp)
}

/// 行迭代器：包装 BufReader，逐行产出 String。供 [`decode_request`] 消费。
///
/// 读行本体已上提 [`crate::line_io::read_line_trimmed`]（与 linux 共用单一真值）；
/// 本类型只保留 mac 侧的 `Iterator` 适配形状。
struct LineIter<'a, R: BufRead> {
    buf: &'a mut R,
    too_long: bool,
}

impl<R: BufRead> Iterator for LineIter<'_, R> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        match read_line_trimmed_bounded(self.buf, MAX_WIRE_LINE_BYTES) {
            Ok(line) => line,
            Err(BoundedLineError::TooLong { .. }) => {
                self.too_long = true;
                None
            }
            Err(BoundedLineError::Io(_)) => None,
        }
    }
}

fn write_line_too_long<W: Write>(writer: &mut W) -> ConnOutcome {
    let response = polaris_helper_proto::Response::Err(polaris_helper_proto::Error::with_detail(
        polaris_helper_proto::ErrorCode::BadArgs,
        "line-too-long",
    ));
    write_response(writer, &response)
}

/// 写一个响应行（加 `\n`，对齐 Go `Fprintln`）—— 委托公共 [`line_io::write_line`]。
fn write_response<W: Write>(writer: &mut W, resp: &polaris_helper_proto::Response) -> ConnOutcome {
    match write_line(writer, &resp.to_wire_line()) {
        Ok(()) => ConnOutcome::Done,
        Err(e) => ConnOutcome::IoError(e.to_string()),
    }
}

// ===== 连接并发闸 =====
//
// 闸本体（[`ConnLimiter`] / [`ConnPermit`] / [`MAX_CONCURRENT_CONNECTIONS`]）是平台无关纯类型，
// 已上提 [`crate::platform::conn_limit`] 与 linux 腿共用（同形缺陷两侧同在，见该模块文档）。
// 本文件只保留 mac 侧的**接线**：accept 循环在 `thread::spawn` 之前取许可（见下方 `serve`）。

// ===== mac-gated：真 unix socket 服务端 + child 生命周期（M1/M3/M4/M11/M12/M13）=====

#[cfg(target_os = "macos")]
mod sys {
    use super::*;
    use crate::platform::accept_retry::{
        classify_accept_error, AcceptAction, LogThrottle, ACCEPT_BACKOFF, ACCEPT_LOG_INTERVAL,
    };
    use crate::platform::conn_limit::{ConnLimiter, MAX_CONCURRENT_CONNECTIONS};
    use crate::platform::macos::exec::{CommandRunner, SystemRunner, EXEC_TIMEOUT};
    use crate::platform::macos::handler::{ChildHandle, SpawnError, TerminateOutcome};
    use crate::platform::macos::native_birth::{NativeBirth, NativeChild, NativeCustody};
    use crate::platform::macos::proc_start::{
        classify_alive, kill_zero_exists, proc_start_time, AliveProbe, WATCH_TICK_INTERVAL,
    };
    use crate::token::{FileTokenStore, TokenStore};
    use nix::sys::signal::Signal;
    use polaris_helper_proto::response::{
        NativeBirthStart, NativeBirthStatus, NativeBirthStop, StartTiming,
    };
    use polaris_helper_proto::{HelperBirthTarget, HelperBirthToken};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// The registry retains the original Child through wait errors and the full chown tail.
    pub struct DaemonServices {
        token: FileTokenStore,
        runner: SystemRunner,
        config: MacConfig,
        command_mu: Mutex<()>,
        child: Mutex<Option<ChildHandle>>,
        native: Arc<NativeCustody>,
    }

    struct OwnedNativeChild(std::process::Child);
    impl NativeChild for OwnedNativeChild {
        fn try_wait(&mut self) -> std::io::Result<bool> {
            self.0.try_wait().map(|status| status.is_some())
        }
        fn terminate(&mut self) -> std::io::Result<()> {
            // Unix signal is synchronous under the same Child lock after a native poll.
            // ECHILD fences this operation permanently in NativeCustody.
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                Signal::SIGTERM,
            )
            .map_err(|error| std::io::Error::from_raw_os_error(error as i32))
        }
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.kill()
        }
    }

    struct StartAdmission<'a>(&'a NativeCustody, bool);
    impl Drop for StartAdmission<'_> {
        fn drop(&mut self) {
            if !self.1 {
                self.0.abort_start();
            }
        }
    }

    impl DaemonServices {
        #[must_use]
        pub fn new(config: MacConfig) -> Arc<Self> {
            Arc::new(Self {
                token: FileTokenStore::new(&config.support_dir),
                runner: SystemRunner::new(),
                config,
                command_mu: Mutex::new(()),
                child: Mutex::new(None),
                native: Arc::new(NativeCustody::default()),
            })
        }
        #[must_use]
        pub fn config(&self) -> &MacConfig {
            &self.config
        }
        #[must_use]
        pub fn command_mu(&self) -> &Mutex<()> {
            &self.command_mu
        }

        fn do_native_spawn(&self, params: &StartParams) -> Result<NativeBirthStart, SpawnError> {
            if let Err(not_admitted) = self.native.admit() {
                return Ok(not_admitted);
            }
            let mut admission = StartAdmission(&self.native, false);
            let total_started = Instant::now();
            // Identity is minted from OS entropy before spawning, never from a PID or clock.
            let mut entropy = [0; 16];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut file| file.read_exact(&mut entropy))
                .map_err(|error| {
                    SpawnError::Failed(format!("native birth entropy failed: {error}"))
                })?;
            let birth_token = HelperBirthToken::from_bytes(entropy);
            let forwarding_started = Instant::now();
            if params.fwd {
                let _ = self.runner.run(
                    EXEC_TIMEOUT,
                    "/usr/sbin/sysctl",
                    &["-w", "net.inet.ip.forwarding=1"],
                );
                let _ = self.runner.run(
                    EXEC_TIMEOUT,
                    "/usr/sbin/sysctl",
                    &["-w", "net.inet6.ip6.forwarding=1"],
                );
            }
            let forwarding_ms = crate::elapsed_ms(forwarding_started);
            let process_started = Instant::now();
            prepare_cache_for_user(&self.config.conf_dir).map_err(SpawnError::Failed)?;
            let mut command = std::process::Command::new(&self.config.singbox_bin);
            command.arg("run").arg("-c").arg(&params.cfg);
            if !self.config.conf_dir.is_empty() {
                command.current_dir(&self.config.conf_dir);
            }
            if !params.log.is_empty() {
                command.stdout(std::process::Stdio::piped());
                command.stderr(std::process::Stdio::piped());
            }
            let log_files = if params.log.is_empty() {
                None
            } else {
                match crate::platform::unix_log::preopen_log_files(
                    &self.config.conf_dir,
                    &params.log,
                ) {
                    Ok(files) => Some(files),
                    Err(error) => {
                        log::warn!(
                            "privileged core logging disabled: secure pre-open failed: {error}"
                        );
                        None
                    }
                }
            };
            let mut child = command
                .spawn()
                .map_err(|error| SpawnError::Failed(error.to_string()))?;
            let target = HelperBirthTarget {
                pid: std::num::NonZeroU32::new(child.id()).expect("native Child has nonzero PID"),
                birth: birth_token,
            };
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            // No background thread, parent probe or cancellation point before custody publication.
            let birth = self
                .native
                .publish(target, Box::new(OwnedNativeChild(child)));
            admission.1 = true;
            let process_ms = crate::elapsed_ms(process_started);
            let log_handoff_started = Instant::now();
            // Initialize Fresh/session synchronously while this admitted command still owns
            // the slot. A delayed outer logger thread could otherwise truncate a successor's log.
            let log_custody = if let Some(files) = log_files {
                polaris_log_budget::spawn_pipe_loggers_with_preopened_files_custodied(
                    stdout,
                    stderr,
                    files,
                    polaris_log_budget::DEFAULT_GENERATION_BYTES,
                )
            } else {
                polaris_log_budget::spawn_pipe_drainers_custodied(stdout, stderr)
            };
            birth.attach_log_custody(log_custody);
            let log_handoff_ms = crate::elapsed_ms(log_handoff_started);
            spawn_native_worker(
                Arc::clone(&self.native),
                Arc::clone(&birth),
                self.config.conf_dir.clone(),
            );
            if let Some(parent_pid) = params.parent_pid {
                spawn_native_parent_watch(
                    Arc::clone(&self.native),
                    birth,
                    parent_pid,
                    proc_start_time(parent_pid),
                );
            }
            Ok(NativeBirthStart::Started {
                target,
                created: None,
                timing: Some(StartTiming {
                    forwarding_ms,
                    process_ms,
                    job_ms: 0,
                    log_handoff_ms,
                    total_ms: crate::elapsed_ms(total_started),
                }),
            })
        }

        /// Normal daemon exit is rejected while any admitted spawn or exact birth is
        /// unconfirmed. The live daemon retains custody and accepts same-birth retries.
        pub fn shutdown_reap(&self) -> Result<(), String> {
            self.native.close_admission();
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if self.native.shutdown_complete()? {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(
                        "mac helper exit rejected: native child/tail or admitted spawn unconfirmed"
                            .into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    impl MacServices for DaemonServices {
        fn token_store(&self) -> &dyn TokenStore {
            &self.token
        }
        fn runner(&self) -> &dyn crate::platform::macos::exec::CommandRunner {
            &self.runner
        }
        fn child(&self) -> &Mutex<Option<ChildHandle>> {
            &self.child
        }
        fn native_start(&self, params: &StartParams) -> Result<NativeBirthStart, SpawnError> {
            self.do_native_spawn(params)
        }
        fn native_status(&self) -> NativeBirthStatus {
            self.native.status()
        }
        fn native_stop(&self, target: HelperBirthTarget) -> NativeBirthStop {
            self.native.stop(target)
        }
        fn native_custody_active(&self) -> bool {
            self.native.active()
        }
        // Old clients are rejected before any forwarding/cache/spawn, rather than stranded
        // after a successful legacy Start with no exact native Stop authority.
        fn legacy_start_supported(&self) -> bool {
            false
        }
        fn clear_child(&self) {
            // Legacy Cleanup cannot discard native custody, even if called outside dispatch.
        }
        fn terminate_child(&self, _want_pid: Option<u32>) -> TerminateOutcome {
            // A legacy PID or void termination never mints a native receipt or sends a signal.
            TerminateOutcome::NotRunning
        }
    }

    fn spawn_native_worker(custody: Arc<NativeCustody>, birth: Arc<NativeBirth>, conf_dir: String) {
        std::thread::spawn(move || {
            // A worker can stop or panic without owning the registry's retirement rights.
            while custody.is_current(&birth) {
                custody.drive(&birth, Instant::now(), &|| chown_runtime_dirs(&conf_dir));
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    fn spawn_native_parent_watch(
        custody: Arc<NativeCustody>,
        birth: Arc<NativeBirth>,
        parent_pid: u32,
        parent_start: Option<String>,
    ) {
        std::thread::spawn(move || loop {
            std::thread::sleep(WATCH_TICK_INTERVAL);
            if !custody.is_current(&birth) {
                return;
            }
            let exists = kill_zero_exists(parent_pid);
            let current = if exists && parent_start.is_some() {
                proc_start_time(parent_pid)
            } else {
                None
            };
            if matches!(
                classify_alive(exists, parent_start.as_deref(), current.as_deref()),
                AliveProbe::Dead | AliveProbe::PidReused
            ) {
                // Captured birth, never a naked child PID. Old callbacks cannot affect a successor.
                custody.stop(birth.target);
                return;
            }
        });
    }

    /// root 起核前把 cache.db 锁定为 confDir 属主。
    ///
    /// 不截断既有数据库；最后一段若为符号链接则 `O_NOFOLLOW` 拒绝。打开后只对 fd `fchown`，
    /// 不在用户可写目录里做「检查路径 → 再 chown 路径」的 TOCTOU。
    pub(super) fn prepare_cache_for_user(conf_dir: &str) -> Result<(), String> {
        if conf_dir.is_empty() {
            return Ok(());
        }
        let dir_meta = std::fs::metadata(conf_dir)
            .map_err(|e| format!("读取配置目录属主失败（{conf_dir}）：{e}"))?;
        let uid = dir_meta.uid();
        let gid = dir_meta.gid();
        if should_skip_confdir_chown(uid) {
            return Err(format!(
                "配置目录异常归 root 所有，拒绝准备 cache.db：{conf_dir}"
            ));
        }
        let cache = Path::new(conf_dir).join(CHOWN_FILES[0]);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&cache)
            .map_err(|e| format!("安全打开 cache.db 失败（{}）：{e}", cache.display()))?;
        std::os::unix::fs::fchown(&file, Some(uid), Some(gid))
            .map_err(|e| format!("归还 cache.db 属主失败（{}）：{e}", cache.display()))
    }

    /// A native receipt includes the complete ownership tail, not merely process exit.
    fn chown_runtime_dirs(conf_dir: &str) -> Result<(), String> {
        if conf_dir.is_empty() {
            return Ok(());
        }
        let meta = std::fs::metadata(conf_dir)
            .map_err(|error| format!("read runtime owner {conf_dir}: {error}"))?;
        let (uid, gid) = (meta.uid(), meta.gid());
        if should_skip_confdir_chown(uid) {
            return Err(format!(
                "runtime config directory unexpectedly belongs to root: {conf_dir}"
            ));
        }
        for name in CHOWN_SUBDIRS.into_iter().chain(CHOWN_FILES) {
            chown_tree(&Path::new(conf_dir).join(name), uid, gid)?;
        }
        Ok(())
    }

    fn chown_tree(root: &Path, uid: u32, gid: u32) -> Result<(), String> {
        let meta = match std::fs::symlink_metadata(root) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("read runtime entry {}: {error}", root.display())),
        };
        if should_chown_entry(meta.uid()) {
            std::os::unix::fs::lchown(root, Some(uid), Some(gid))
                .map_err(|error| format!("chown runtime entry {}: {error}", root.display()))?;
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(root)
                .map_err(|error| format!("read runtime tree {}: {error}", root.display()))?
            {
                let entry = entry.map_err(|error| format!("read runtime tree entry: {error}"))?;
                chown_tree(&entry.path(), uid, gid)?;
            }
        }
        Ok(())
    }

    /// 建 socket（0666）+ accept 循环，每连接一线程 dispatch（`helper.go:598-642`）。
    ///
    /// 每连接：5s read deadline（`helper.go:401`）+ 独立线程 `process_connection`（`helper.go:641`
    /// `go handle(conn)`）+ `command_mu` 补 Go 单锁纪律。返回 Err 当 socket 监听失败。
    ///
    /// 两处**补 Go 源没有的护栏**（复审 Medium）：
    /// - 起线程前先取 [`ConnLimiter`] 许可（[`MAX_CONCURRENT_CONNECTIONS`]）—— 0666 socket 上
    ///   线程是在鉴权之前起的，无上限即可被无 token 的本地进程耗尽。
    /// - accept 错误分类 + 退避（[`crate::platform::accept_retry`]）—— 一律 `continue` 会在
    ///   EMFILE 这类持续态下 100% CPU 忙转。
    pub fn serve(services: Arc<DaemonServices>) -> std::io::Result<()> {
        let support = Path::new(&services.config.support_dir);
        // helper.go:598-601: MkdirAll(supportDir, 0755)
        std::fs::create_dir_all(support)?;
        // helper.go:603: 纵深防御 Chmod 0755（确保普通用户可穿越，否则 app 连 socket EACCES）
        let _ = std::fs::set_permissions(support, std::fs::Permissions::from_mode(0o755));

        let sock_path = support.join(SOCK_FILENAME);
        // helper.go:605: Remove 旧 socket（防「地址已在使用」）
        let _ = std::fs::remove_file(&sock_path);

        let listener = UnixListener::bind(&sock_path)?;
        // helper.go:611: Chmod 0666（token 为安全边界）
        let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(SOCK_MODE));

        let limiter = ConnLimiter::new(MAX_CONCURRENT_CONNECTIONS);
        // 两个独立限频器：达上限与 accept 失败是两种不同的资源压力，共用一个会互相掩盖
        //（连接洪水期间的 EMFILE 就再也打不出来）。
        let limit_log = LogThrottle::new(ACCEPT_LOG_INTERVAL);
        let accept_log = LogThrottle::new(ACCEPT_LOG_INTERVAL);

        // helper.go:636-642: accept 循环
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    // 闸在**起线程之前**（也就在鉴权之前）：0666 socket 上任何本地进程都能连，
                    // 线程一旦起了就已经被对端按住 5s。超限即快速失败——`s` 在此 drop = 立即关连接。
                    let Some(permit) = limiter.try_acquire() else {
                        if limit_log.allow() {
                            log::warn!(
                                "helper: 在途连接达上限 {MAX_CONCURRENT_CONNECTIONS}，拒绝新连接"
                            );
                        }
                        continue;
                    };
                    // helper.go:401: 5s read deadline（set_read_timeout 取 &self）
                    let _ = s.set_read_timeout(Some(READ_TIMEOUT));
                    let svc = Arc::clone(&services);
                    // helper.go:641: go handle(conn)（每连接一线程）
                    std::thread::spawn(move || {
                        // 许可随线程结束（含 panic 展开）归还，见 ConnPermit 文档。
                        let _permit = permit;
                        // reader=stream；writer=try_clone 副本（读写各一独立 fd）
                        let writer = match s.try_clone() {
                            Ok(w) => w,
                            Err(_) => return,
                        };
                        // 对端 uid 由内核给出（`LOCAL_PEERCRED`）。token 仍是鉴权边界；uid 只用来
                        // 约束 install-core 的源文件属主。
                        let peer_uid = nix::unistd::getpeereid(&s)
                            .ok()
                            .map(|(uid, _gid)| uid.as_raw());
                        let _ = process_connection_from_peer(
                            s,
                            writer,
                            svc.as_ref(),
                            svc.config(),
                            Some(svc.command_mu()),
                            peer_uid,
                        );
                    });
                }
                // helper.go:638-639 是裸 continue —— EMFILE/ENFILE 持续态下就地忙转。
                // 与 linux daemon 同一份判据（accept_retry），瞬时态立即重试、其余退避 + 限频自曝。
                Err(e) => {
                    if classify_accept_error(&e) == AcceptAction::Backoff {
                        if accept_log.allow() {
                            log::warn!(
                                "helper: accept 失败（{e}），退避 {ACCEPT_BACKOFF:?} 后重试"
                            );
                        }
                        std::thread::sleep(ACCEPT_BACKOFF);
                    }
                    continue;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use sys::{serve, DaemonServices};

#[cfg(test)]
mod tests;

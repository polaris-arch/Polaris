//! Windows SCM 服务 + 命名管道监听（移植自 `helper-win/service.go` + `main.go`）。
//!
//! 实现两件事：
//! 1. **命名管道监听**（`service.go:37-44` `listen` + `service.go:48-57` `serve`）：创建 SDDL 防护的命名管道
//!    （`\\.\pipe\polaris-helper`，SDDL=`D:(A;;FA;;;SY)(A;;GRGW;;;IU)`），accept 循环，每连接一个线程跑//!    [`crate::platform::windows::helper::WinHelper::handle`]。
//! 2. **SCM 服务托管**（`service.go:64-93` `Execute` + `main.go:39-42`）：注册服务控制处理器，
//!    STOP/SHUTDOWN closes admission and the listener, then retains native custody
//!    while retrying close. Only confirmed native exit and writer revoke allow Stopped.

// 具体 item 才局部放开 crate 级 `#![deny(unsafe_code)]`：SCM/命名管道/SDDL 的 windows-sys FFI 调用
// （CreateNamedPipeW/ConnectNamedPipe/ConvertStringSecurityDescriptorToSecurityDescriptorW/
// StartServiceCtrlDispatcherW/RegisterServiceCtrlHandlerExW/SetServiceStatus/...）必须 unsafe。
// 每处 unsafe 块附 SAFETY 理由。
use super::frame::{cancel_after_deadline, read_frame_until, MessageRead};
use crate::platform::windows::helper::{FlushMode, WinHelper};
use crate::platform::windows::logic;
use crate::platform::windows::winproc::WinProcOps;
use crate::platform::windows::{PIPE_NAME, PIPE_SDDL, SERVICE_NAME};
use crate::token::FileTokenStore;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, LocalFree, DUPLICATE_SAME_ACCESS, ERROR_MORE_DATA, ERROR_NO_DATA,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, ERROR_SERVICE_CANNOT_ACCEPT_CTRL, FALSE, HANDLE,
    INVALID_HANDLE_VALUE, NO_ERROR,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR};
use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, SetNamedPipeHandleState, PIPE_NOWAIT,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

/// 命名管道实例数（accept 循环预创建的并行实例）。镜像 go-winio 默认行为。
const PIPE_INSTANCES: u32 = 4;

/// FILE_FLAG_FIRST_PIPE_INSTANCE（首实例独占，防重复创建）。windows-sys 常量值。
const FILE_FLAG_FIRST_PIPE_INSTANCE: FILE_FLAGS_AND_ATTRIBUTES = 0x0008_0000;

/// SDDL_REVISION_1（ConvertStringSecurityDescriptor 的 revision 参数）。
const SDDL_REVISION_1: u32 = 1;

/// ERROR_BROKEN_PIPE（ReadFile 时客户端关闭，正常 EOF）。
const ERROR_BROKEN_PIPE: u32 = 109;

/// 同步 IO 的超时预算（秒）。读腿沿用 W1 的 5s（镜像 Go `conn.SetReadDeadline(5s)`）；
/// 写腿取同一预算 —— 同一个连接、同一个对端，两腿的耐心没有理由不同。
const IO_TIMEOUT_SECS: u64 = 5;

// 响应的 flush 档位（`FlushMode`）住在跨平台的 `helper` 模块：档位由 `WinHelper::handle_frame`
// 按「对端是否已鉴权」决定（Linux 可测），本文件只照档位执行。

/// 全局停止标志（SCM STOP/SHUTDOWN 时置 true，accept 循环据此退出）。
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// SCM 状态句柄（`service_main_entry` 注册后写入，`ctrl_handler` 读它上报 STOP_PENDING）。
///
/// 存 `usize` 而非句柄类型：`static` 要求 `Sync`，而 `SERVICE_STATUS_HANDLE` 是裸指针别名。
/// 0 = 尚未注册。
static STATUS_HANDLE: AtomicUsize = AtomicUsize::new(0);
static STOP_CHECKPOINT: AtomicU32 = AtomicU32::new(0);

type ProductionHelper = WinHelper<FileTokenStore, WinProcOps, WinProcOps>;
// SCM callbacks need the same synchronous admission fence as Start. This Arc remains
// retained for this service process, including while an unconfirmed shutdown retries.
static SERVICE_HELPER: std::sync::OnceLock<Arc<ProductionHelper>> = std::sync::OnceLock::new();

/// 服务运行配置（main 解析 flags 后构造）。
pub struct ServiceConfig {
    /// support 目录；受保护内核目录是它下面的 `core\`（install-core 分支自行派生，不单列字段 ——
    /// 多一个可注入的核路径入口正是 P4 要收掉的东西）。
    pub support_dir: String,
}

impl ServiceConfig {
    #[must_use]
    pub fn default_with_support(support_dir: impl Into<String>) -> Self {
        Self {
            support_dir: support_dir.into(),
        }
    }
}

/// 以 SCM 服务身份运行（`main.go:39-42` `runService`）。
pub fn run_service(cfg: ServiceConfig) -> std::io::Result<()> {
    let cfg: &'static ServiceConfig = Box::leak(Box::new(cfg));
    set_global_config(cfg);
    start_service_ctrl_dispatcher()
}

/// --console dev 模式：前台跑 listener 无 SCM（`main.go:47-66` `runConsole`）。
pub fn run_console(cfg: ServiceConfig) -> std::io::Result<()> {
    let helper = build_helper(&cfg);
    let helper = Arc::new(helper);
    let helper_for_stop = helper.clone();
    ctrl_c_handler(move || match helper_for_stop.reap_child_on_exit() {
        Ok(()) => {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
        }
        Err(error) => {
            log::error!("console shutdown unconfirmed; helper custody retained: {error}");
        }
    })?;
    let served = serve(helper.clone());
    drain_for_exit(helper.as_ref(), || {});
    served
}

/// 构造生产 WinHelper（FileTokenStore + WinProcOps）。
fn build_helper(cfg: &ServiceConfig) -> ProductionHelper {
    let root = std::path::Path::new(&cfg.support_dir).join("core");
    if let Ok(store) =
        crate::core_store::CoreStore::open(&root, crate::core_store::OwnerPolicy::privileged())
    {
        if let Err(error) = store.retire_flat_layout() {
            log::warn!("legacy flat retirement retained: {error}");
        }
    }
    let token = FileTokenStore::new(&cfg.support_dir);
    let proc = WinProcOps::new();
    let net = WinProcOps::new();
    WinHelper::new(token, proc, net, &cfg.support_dir)
}

/// HANDLE（`*mut c_void`）的 Send 包装，用于把管道句柄移入 spawned 线程。
///
/// SAFETY: 命名管道句柄在本设计中仅被单一连接线程独占使用（创建 → ConnectNamedPipe → 读写 →
/// CloseHandle 全在 `handle_connection` 内），无跨线程共享，故可安全 Send。
struct SendHandle(HANDLE);
#[allow(
    unsafe_code,
    reason = "pipe HANDLE ownership moves to exactly one connection thread"
)]
unsafe impl Send for SendHandle {}
impl SendHandle {
    fn raw(self) -> HANDLE {
        self.0
    }
}

/// serve：accept 循环，每连接一个线程（`service.go:48-57`）。
#[allow(
    unsafe_code,
    reason = "closes the unconnected pipe HANDLE on the accept-loop error leg"
)]
fn serve<T, P, N>(helper: Arc<WinHelper<T, P, N>>) -> std::io::Result<()>
where
    T: crate::token::TokenStore + 'static,
    P: crate::platform::windows::ops::ProcOps + 'static,
    N: crate::platform::windows::ops::NetTableOps + 'static,
{
    let pipe_name_w = wide_null(OsString::from(PIPE_NAME));
    let sddl_w = wide_null(OsString::from(PIPE_SDDL));
    // 只有**第一个**实例带 FILE_FLAG_FIRST_PIPE_INSTANCE（防他人抢占同名管道）；后续实例不带，
    // 否则「工作线程还持有上一个实例」期间回头重建必然 ERROR_ACCESS_DENIED，
    // 整个连接处理期间没有监听实例。见 `logic::pipe_open_mode`。
    let mut first = true;
    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        // 创建一个管道实例（阻塞等连接）。镜像 Go winio.ListenPipe。
        let h = match create_pipe_instance(&pipe_name_w, &sddl_w, first) {
            Ok(h) => h,
            Err(e) => {
                // 创建失败：短暂 sleep 防忙等，重试（除非已 STOP_REQUESTED）。
                if STOP_REQUESTED.load(Ordering::SeqCst) {
                    return Ok(());
                }
                log::error!("create_pipe_instance failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        // 建成了一个实例 ⇒ 从此管道名已存在，后续再带首实例 flag 必被拒。
        first = false;
        // Poll connection admission so service stop is observed even without a client.
        if connect_pipe_until_stopped(h, || STOP_REQUESTED.load(Ordering::SeqCst)) {
            let helper = helper.clone();
            // HANDLE = *mut c_void 非 Send；先包成 SendHandle 再 move 入 spawned 线程（句柄在
            // handle_connection 内仅本线程独占使用，CloseHandle 也在内部，无并发访问）。
            let sh = SendHandle(h);
            std::thread::spawn(move || {
                handle_connection(sh.raw(), &helper);
            });
        } else {
            // SAFETY: 关未连接的实例，下一轮重建。
            unsafe { CloseHandle(h) };
        }
    }
    Ok(())
}

/// 钉住 [`logic::pipe_open_mode`] 里硬编码的位值 == `windows-sys` 常量。
///
/// 那两个位值必须硬编码（`logic` 模块在 Linux 上也编译，好让纯逻辑有门可跑；而 `windows-sys`
/// 只在 Windows target 的依赖图里）。这条编译期断言是它们之间唯一的对账 —— 任一侧改了就编不过，
/// 且 CI 的 `cargo check --target x86_64-pc-windows-msvc` 会把它跑到。
const _: () = {
    assert!(logic::pipe_open_mode(true) == PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE);
    assert!(logic::pipe_open_mode(false) == PIPE_ACCESS_DUPLEX);
};

/// 创建一个命名管道实例（SDDL 防护，message-framed，PIPE_WAIT 阻塞）。
///
/// `first` 决定要不要带 `FILE_FLAG_FIRST_PIPE_INSTANCE` —— 见 [`logic::pipe_open_mode`]，
/// 带错了会让**连接处理期间没有监听实例**。
#[allow(
    unsafe_code,
    reason = "creates one named-pipe HANDLE and frees its synchronous SDDL allocation"
)]
fn create_pipe_instance(name_w: &[u16], sddl_w: &[u16], first: bool) -> std::io::Result<HANDLE> {
    let mut sa: SECURITY_ATTRIBUTES = unsafe { std::mem::zeroed() };
    sa.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
    sa.bInheritHandle = FALSE;
    // SAFETY: sddl_w 是 NUL 结尾 UTF-16；API 成功时返回 LocalAlloc 所有权，必须在
    // CreateNamedPipeW 返回后 LocalFree。解析失败必须 fail-closed，不能静默降级到进程默认 DACL。
    let mut sd: *mut SECURITY_DESCRIPTOR = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut sd as *mut *mut SECURITY_DESCRIPTOR as *mut _,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || sd.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    sa.lpSecurityDescriptor = sd.cast();
    // SAFETY: CreateNamedPipeW 创建管道实例。**W1 修：PIPE_ACCESS_DUPLEX**（双向）——此前
    // PIPE_ACCESS_INBOUND 只给服务端只读句柄，WriteFile 响应据 Win32 契约回 ERROR_ACCESS_DENIED →
    // 请求/响应回路断（client 收不到响应）。DUPLEX 让服务端可读请求 + 写响应（对齐 Go winio 默认双向）。
    // FILE_FLAG_FIRST_PIPE_INSTANCE **只给首个实例**（见 `logic::pipe_open_mode`）。
    // Each request WriteFile is one native message; preserve that boundary.
    // The client may retain byte-read mode for the single response line.
    // PIPE_WAIT 阻塞（ConnectNamedPipe 阻塞等连接）。PIPE_REJECT_REMOTE_CLIENTS 拒绝远程客户端。
    let h = unsafe {
        CreateNamedPipeW(
            name_w.as_ptr(),
            logic::pipe_open_mode(first),
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_INSTANCES,
            4096,
            4096,
            0,
            &sa,
        )
    };
    // LocalFree 可能改线程 last-error；先保留 CreateNamedPipeW 的失败原因。
    let create_error = (h == INVALID_HANDLE_VALUE).then(std::io::Error::last_os_error);
    // SAFETY: sd 是上面 Convert... 成功返回且尚未释放的 LocalAlloc 指针；CreateNamedPipeW
    // 已同步消费 SECURITY_ATTRIBUTES，返回后不再借用。每次实例恰释放一次，失败腿同样覆盖。
    let free_result = unsafe { LocalFree(sd.cast()) };
    debug_assert!(
        free_result.is_null(),
        "LocalFree security descriptor failed"
    );
    if let Some(error) = create_error {
        return Err(error);
    }
    Ok(h)
}

/// Polling admission has no one-shot wake race before pipe creation. Only
/// ERROR_PIPE_CONNECTED indicates a connected client in PIPE_NOWAIT mode;
/// nonzero success merely makes the pipe available to a future client.
#[allow(unsafe_code, reason = "owned synchronous pipe mode and accept polling")]
fn connect_pipe_until_stopped(h: HANDLE, mut stopped: impl FnMut() -> bool) -> bool {
    let mode = PIPE_NOWAIT | PIPE_READMODE_MESSAGE;
    if unsafe { SetNamedPipeHandleState(h, &mode, std::ptr::null(), std::ptr::null()) } == 0 {
        return false;
    }
    loop {
        if stopped() {
            return false;
        }
        let available = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
        if available == 0 {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if error == ERROR_PIPE_CONNECTED {
                if stopped() {
                    return false;
                }
                let blocking = PIPE_WAIT | PIPE_READMODE_MESSAGE;
                return unsafe {
                    SetNamedPipeHandleState(h, &blocking, std::ptr::null(), std::ptr::null())
                } != 0;
            }
            if error != ERROR_PIPE_LISTENING {
                return false;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// ConnectNamedPipe：等客户端连接。返回 true = 已连接，false = 中断/失败。
#[allow(
    unsafe_code,
    reason = "ConnectNamedPipe synchronously borrows the live pipe HANDLE"
)]
#[cfg(test)]
fn connect_pipe(h: HANDLE) -> bool {
    // SAFETY: ConnectNamedPipe 阻塞等连接（PIPE_WAIT，lpOverlapped=NULL）。返回 0 = 失败。
    let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
    if ok != 0 {
        return true;
    }
    // ERROR_PIPE_CONNECTED = client connected between CreateNamedPipe and ConnectNamedPipe.
    // Import the SDK constant: this is a connected pipe, not an accept failure.
    let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    err == ERROR_PIPE_CONNECTED
}

/// 处理一个已连接的管道实例（对应 Go `handle(conn)` 的 IO 部分）。
#[allow(
    unsafe_code,
    reason = "capture peer while this worker retains its connected pipe"
)]
fn handle_connection<T, P, N>(h: HANDLE, helper: &WinHelper<T, P, N>)
where
    T: crate::token::TokenStore,
    // 'static：WinHelper 的 impl 因父死看护闭包捕获 Arc<P> 进 'static 线程而要求 P: 'static（W15）。
    P: crate::platform::windows::ops::ProcOps + 'static,
    N: crate::platform::windows::ops::NetTableOps,
{
    // One absolute deadline covers every fragment. PIPE_NOWAIT reads cannot
    // become stranded by a cancellation arriving just before an operation starts.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(IO_TIMEOUT_SECS);
    let raw = match read_frame(h, deadline) {
        Ok(raw) => raw,
        Err(_) => {
            cleanup_pipe(h);
            return;
        }
    };
    // 切行 → 验 token → 解码 → 分派 全在 `handle_frame`（跨平台、Linux 有门）；这里只管 IO。
    // 顺序与各腿的 flush 档位见该方法文档：token 不对时不看命令（已知/未知不可区分），
    // 已鉴权而命令未知的 `ERR unknown` 走 WaitPeer 可靠送达。
    // SAFETY: this worker owns the connected pipe until cleanup below; all reads
    // have completed and no other worker closes or replaces this handle.
    let peer = match unsafe { crate::platform::windows::native_caller::from_pipe(h) } {
        Ok(peer) => peer,
        Err(_) => {
            cleanup_pipe(h);
            return;
        }
    };
    let reply = helper.handle_frame_from_peer(&raw, Some(&peer));
    write_response(h, reply.line.as_bytes(), reply.flush);
    cleanup_pipe(h);
    if reply.exit_after {
        // Uninstall uses the same service/console drain instead of bypassing it with exit().
        match helper.reap_child_on_exit() {
            Ok(()) => {
                STOP_REQUESTED.store(true, Ordering::SeqCst);
            }
            Err(error) => log::error!("uninstall exit unconfirmed; helper retained: {error}"),
        }
    }
}

/// Read exactly one native message. ERROR_MORE_DATA chunks are accumulated;
/// only native message completion is a frame boundary. No short buffer, empty
/// poll, complete-looking argument prefix, or split UTF-8 sequence is dispatched.
/// Restore PIPE_WAIT only after the complete bounded request is validated, before
/// the existing response/flush path uses this owned handle.
#[allow(
    unsafe_code,
    reason = "polls the owned nonblocking message pipe and restores its wait mode"
)]
fn read_frame(h: HANDLE, deadline: std::time::Instant) -> std::io::Result<String> {
    let nonblocking = PIPE_NOWAIT | PIPE_READMODE_MESSAGE;
    // SAFETY: this connection worker exclusively owns the live handle; no I/O is
    // pending and no cancellation thread is involved in the read leg.
    if unsafe { SetNamedPipeHandleState(h, &nonblocking, std::ptr::null(), std::ptr::null()) } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let raw = read_frame_until(
        deadline,
        |fragment| {
            let mut got: u32 = 0;
            // SAFETY: PIPE_NOWAIT makes this synchronous read nonblocking; the
            // bounded fragment and byte count remain live through the call.
            let ok = unsafe {
                ReadFile(
                    h,
                    fragment.as_mut_ptr(),
                    fragment.len() as u32,
                    &mut got,
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 {
                return Ok(MessageRead {
                    bytes: got as usize,
                    complete: true,
                });
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_MORE_DATA) => Ok(MessageRead {
                    bytes: got as usize,
                    complete: false,
                }),
                Some(ERROR_NO_DATA) => Err(std::io::ErrorKind::WouldBlock.into()),
                Some(ERROR_BROKEN_PIPE) => Err(std::io::ErrorKind::UnexpectedEof.into()),
                _ => Err(error),
            }
        },
        std::time::Instant::now,
        std::thread::sleep,
    )?;
    if std::time::Instant::now() >= deadline {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    let blocking = PIPE_WAIT | PIPE_READMODE_MESSAGE;
    if unsafe { SetNamedPipeHandleState(h, &blocking, std::ptr::null(), std::ptr::null()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if std::time::Instant::now() >= deadline {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    Ok(raw)
}

/// Response-only cancellation guard. The absolute deadline is captured by the
/// connection worker before spawning the watchdog, not when that thread runs.
/// After expiry it repeatedly cancels until joined, covering operation-start
/// gaps between WriteFile and FlushFileBuffers. Native synchronous cancellation
/// and flush behavior still require Windows fixtures; this is not a proof that
/// every driver implements prompt cancellation.
///
/// The retained thread handle, not a pipe handle or numeric ID, is canceled.
/// Drop sets done, unparks, and joins before unrelated work or pipe cleanup.
struct IoTimeoutGuard {
    done: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[allow(
    unsafe_code,
    reason = "duplicates and retains the exact worker thread until synchronous cancellation is joined"
)]
impl IoTimeoutGuard {
    fn arm(deadline: std::time::Instant) -> std::io::Result<Self> {
        let mut thread = std::ptr::null_mut();
        // SAFETY: pseudo handles are resolved here on the connection worker,
        // before spawning the watchdog. The duplicate identifies this exact
        // thread, never a numeric thread ID or the watchdog's current thread.
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut thread,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: DuplicateHandle transferred one newly owned live handle. The
        // spawned closure (or failed spawn's dropped closure) closes it once.
        let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
        let done = Arc::new(AtomicBool::new(false));
        let done_t = done.clone();
        let join = std::thread::Builder::new().spawn(move || {
            cancel_after_deadline(
                deadline,
                || done_t.load(Ordering::SeqCst),
                std::time::Instant::now,
                std::thread::park_timeout,
                || {
                    // SAFETY: this retained handle names the initiating worker.
                    // Only response I/O runs on that worker while this guard is
                    // armed, and Drop joins before it can start unrelated work.
                    unsafe { CancelSynchronousIo(thread.as_raw_handle()) };
                },
            );
        })?;
        Ok(Self {
            done,
            join: Some(join),
        })
    }
}

impl Drop for IoTimeoutGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            j.thread().unpark(); // 唤醒看护线程立即退出（不等 deadline）
            let _ = j.join(); // 等看护退出 → 之后 cleanup 才可安全 CloseHandle
        }
    }
}

/// 写响应到管道。
#[allow(
    unsafe_code,
    reason = "WriteFile and FlushFileBuffers synchronously borrow the pipe HANDLE"
)]
fn write_response(h: HANDLE, data: &[u8], mode: FlushMode) {
    let mut written: u32 = 0;
    // The guard stays alive across WriteFile and the optional flush, and joins
    // before cleanup. Deadline checks avoid intentionally starting a new step
    // after expiry; repeated cancellation covers the remaining check/start race.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(IO_TIMEOUT_SECS);
    let _timeout = match IoTimeoutGuard::arm(deadline) {
        Ok(guard) => guard,
        Err(_) => return, // no response I/O without an owned cancellation path
    };
    if std::time::Instant::now() >= deadline {
        return;
    }
    // SAFETY: WriteFile 写响应。lpOverlapped=NULL → 同步阻塞。命名管道服务端在
    // DisconnectNamedPipe 前必须 FlushFileBuffers：同步 WriteFile 只保证字节进内核缓冲，若随即
    // disconnect，尚未被 client 读走的响应会被丢弃，client 稳定收到 ERROR_PIPE_NOT_CONNECTED(233)。
    // FlushFileBuffers 会等 client 取走这条很短的单行响应；生产 client 写完即同步读，协议本就要求
    // 一请求一连接。真机 A/B：同包 app/helper、token/pipe 均一致时，缺 flush 的 ping 仍 0 bytes/233。
    unsafe {
        let ok = WriteFile(
            h,
            data.as_ptr(),
            data.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        );
        // 未鉴权对端（FlushMode::NoWait）不走 flush：见 FlushMode 文档。
        if ok != 0
            && written == data.len() as u32
            && mode == FlushMode::WaitPeer
            && std::time::Instant::now() < deadline
        {
            let _ = FlushFileBuffers(h);
        }
    }
}

/// 断开并关闭管道实例。
#[allow(
    unsafe_code,
    reason = "disconnects and closes the exclusively owned pipe HANDLE once"
)]
fn cleanup_pipe(h: HANDLE) {
    // SAFETY: DisconnectNamedPipe + CloseHandle。
    unsafe {
        let _ = DisconnectNamedPipe(h);
        CloseHandle(h);
    }
}

// 线协议解码（`split_frame` / `parse_request`）已搬到 `logic`（跨平台）：本文件整模块 `cfg(windows)`，
// 解码器留在这里 = 本机 `cargo test` 永远碰不到它（批一 flush-dns、批三 install-core 两次「分派加了、
// 解码没加」都是这么漏过去的）。

// wire 写方向已上提 helper-proto：见 [`polaris_helper_proto::Response::to_wire_line`]
//（与 `Response::parse` 成对，三平台共用）。原 win 私有副本（`wire_response`/`wire_ok`，38 行）已删。
//
// 合并后 win 侧两个「兜底」分支的输出有变，二者均为 **win 不产的 mac/linux 专属响应**（原注释自述
// 「Windows helper 不产以下响应，兜底原文」），故对真实 win 流量无影响，且新输出更正确：
// - `FlushDns(FlushedPartial{tail})`：原恒输出 `OK flushed`（丢 tail，与 mac 不一致）→ 现 `OK flushed-partial <tail>`。
// - `OkRaw{rest:""}`：原输出 `OK <token> `（尾空格，parse 回来不等原值）→ 现 `OK <token>`（round-trip 无损）。

// ===== SCM 服务控制 =====

/// 全局 ServiceConfig（main → service_main_entry 经 OnceLock 传参）。
static GLOBAL_CFG: std::sync::OnceLock<&'static ServiceConfig> = std::sync::OnceLock::new();

pub(crate) fn set_global_config(cfg: &'static ServiceConfig) {
    let _ = GLOBAL_CFG.set(cfg);
}

fn global_config() -> Option<&'static ServiceConfig> {
    GLOBAL_CFG.get().copied()
}

/// 启动 SCM 服务控制分派器（`service.go:96-98` `svc.Run`）。
#[allow(
    unsafe_code,
    reason = "SCM synchronously borrows the terminated service dispatch table"
)]
fn start_service_ctrl_dispatcher() -> std::io::Result<()> {
    // name_w 必须是 'static 或与 table 同生命周期：用 Box::leak 让它活到进程结束。
    let name_w: &'static [u16] =
        Box::leak(wide_null(OsString::from(SERVICE_NAME)).into_boxed_slice());
    let table: [SERVICE_TABLE_ENTRYW; 2] = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name_w.as_ptr() as *mut _,
            lpServiceProc: Some(service_main_entry),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: StartServiceCtrlDispatcherW 阻塞，SCM 调用 service_main_entry。表以 NULL 终止。
    let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// SCM 调用入口（`service.go:64` `Execute`）。extern "system" 约定（windows-sys 要求）。
extern "system" fn service_main_entry(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let cfg = match global_config() {
        Some(c) => c,
        None => return,
    };
    let helper = build_helper(cfg);
    let helper = Arc::new(helper);
    if SERVICE_HELPER.set(helper.clone()).is_err() {
        log::error!("service helper custody was already registered");
        return;
    }
    let status_handle = register_ctrl_handler();
    if status_handle.is_null() {
        return;
    }
    // 交给 ctrl_handler 用于上报 STOP_PENDING（它拿不到这个局部变量）。
    STATUS_HANDLE.store(status_handle as usize, Ordering::SeqCst);
    // StartPending → Running。
    let _ = set_status(status_handle, SERVICE_START_PENDING, 0);
    let _ = set_status(
        status_handle,
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
    );
    // serve（阻塞）。STOP/SHUTDOWN 时 ctrl_handler 设 STOP_REQUESTED → serve 退出。
    let _ = serve(helper.clone());
    // StopPending is a request, not an exit receipt. This loop owns an Arc even after
    // the pipe listener stops and retries native close without depending on IPC clients.
    drain_for_exit(helper.as_ref(), || {
        let _ = set_status(status_handle, SERVICE_STOP_PENDING, 0);
    });
    let _ = set_status(status_handle, SERVICE_STOPPED, 0);
}

fn drain_for_exit<T, P, N>(helper: &WinHelper<T, P, N>, mut pending: impl FnMut())
where
    T: crate::token::TokenStore,
    P: crate::platform::windows::ops::ProcOps + 'static,
    N: crate::platform::windows::ops::NetTableOps,
{
    loop {
        match helper.reap_child_on_exit() {
            Ok(()) => return,
            Err(error) => {
                log::error!("service shutdown unconfirmed; custody retained: {error}");
                pending();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
}

/// 注册 SCM 控制处理器。
#[allow(
    unsafe_code,
    reason = "registers the static SCM callback for this service process"
)]
fn register_ctrl_handler() -> SERVICE_STATUS_HANDLE {
    let name_w = wide_null(OsString::from(SERVICE_NAME));
    // SAFETY: RegisterServiceCtrlHandlerExW 注册 ctrl_handler。返回状态句柄（NULL 失败）。
    unsafe { RegisterServiceCtrlHandlerExW(name_w.as_ptr(), Some(ctrl_handler), std::ptr::null()) }
}

/// SCM 控制处理器（`service.go:79-92`）。STOP/SHUTDOWN → 设 STOP_REQUESTED → serve 退出后 reap。
extern "system" fn ctrl_handler(
    ctrl: u32,
    _evt_type: u32,
    _evt_data: *mut std::ffi::c_void,
    _ctx: *mut std::ffi::c_void,
) -> u32 {
    handle_control(
        ctrl,
        || {
            let Some(helper) = SERVICE_HELPER.get() else {
                log::error!("service shutdown rejected: helper custody not registered");
                return false;
            };
            if let Err(error) = helper.begin_shutdown() {
                log::error!("service admission fence remains unknown: {error}");
            }
            // Existing custody is retained while the service drain retries.
            true
        },
        || {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            let h = STATUS_HANDLE.load(Ordering::SeqCst);
            if h != 0 {
                let _ = set_status(h as SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING, 0);
            }
            // A synchronous ConnectNamedPipe needs a self-connection to wake.
        },
    )
}

/// Test the SCM result contract without mutating process-wide service state.
fn handle_control(
    ctrl: u32,
    begin_shutdown: impl FnOnce() -> bool,
    request_stop: impl FnOnce(),
) -> u32 {
    match ctrl {
        SERVICE_CONTROL_STOP | windows_sys::Win32::System::Services::SERVICE_CONTROL_SHUTDOWN => {
            if !begin_shutdown() {
                return ERROR_SERVICE_CANNOT_ACCEPT_CTRL;
            }
            request_stop();
            NO_ERROR
        }
        _ => NO_ERROR, // Preserve the existing handling of other control codes.
    }
}

/// 报服务状态。
#[allow(
    unsafe_code,
    reason = "SetServiceStatus synchronously borrows an initialized status record"
)]
fn set_status(handle: SERVICE_STATUS_HANDLE, state: u32, accepts: u32) -> std::io::Result<()> {
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    status.dwCurrentState = state;
    status.dwControlsAccepted = accepts;
    status.dwWin32ExitCode = 0;
    if state == SERVICE_STOP_PENDING {
        status.dwCheckPoint = STOP_CHECKPOINT
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
    }
    status.dwWaitHint = 30000; // 30s
                               // SAFETY: SetServiceStatus 报状态。
    let ok = unsafe { SetServiceStatus(handle, &status) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

// ===== Ctrl+C 处理器（--console 模式）=====

/// 注册 Ctrl+C 处理器（SetConsoleCtrlHandler）。handler 在 Ctrl+C 时被回调。
#[allow(
    unsafe_code,
    reason = "registers one process-lifetime console callback with static storage"
)]
fn ctrl_c_handler<F>(handler: F) -> std::io::Result<()>
where
    F: Fn() + Send + 'static,
{
    use std::sync::Mutex;
    static HANDLER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);
    *HANDLER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(handler));
    // SAFETY: SetConsoleCtrlHandler 回调签名 = `unsafe extern "system" fn(u32) -> BOOL`。
    // 体内只取 Mutex 锁定的闭包调用，不做其他 unsafe 操作；返回 TRUE(1) 表示已处理。
    unsafe extern "system" fn raw_handler(_ctrl: u32) -> windows_sys::core::BOOL {
        if let Some(h) = HANDLER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            h();
        }
        windows_sys::Win32::Foundation::TRUE
    }
    // SAFETY: SetConsoleCtrlHandler 注册 raw_handler（PHANDLER_ROUTINE = Option<unsafe extern "system" fn>)。
    let ok =
        unsafe { SetConsoleCtrlHandler(Some(raw_handler), windows_sys::Win32::Foundation::TRUE) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

// ===== 辅助 =====

/// 宽字符串 → null 终止 UTF-16。
fn wide_null(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests;

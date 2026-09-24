//! Windows FFI 实现（`#[cfg(windows)]`，移植自 `helper-win/winproc.go` 的全部副作用原语）。
//!
//! 本文件实现 [`crate::platform::windows::ops::ProcOps`] + [`crate::platform::windows::ops::NetTableOps`] 的生产版本：直接调 windows-sys FFI
//! 替代 Go 的 `golang.org/x/sys/windows`。每处 unsafe 附 SAFETY 理由。

// 具体 item 才局部放开 crate 级 `#![deny(unsafe_code)]`：windows-sys FFI 调用
//（OpenProcess/TerminateProcess/CreateProcessW/...）必须 unsafe。每处 unsafe 块附 SAFETY 理由。
use crate::platform::windows::coreacl::{self, ObjectSecurity};
use crate::platform::windows::logic::{
    eq_ignore_ascii_case_path, filepath_base, filepath_dir, filter_listen_pids, is_locked_singbox,
    local_port_from_net_order, AF_INET, AF_INET6, MIB_TCP_STATE_LISTEN,
    TCP_TABLE_OWNER_PID_LISTENER,
};
use crate::platform::windows::ops::{CoreStart, ManagedIdentity, NetTableOps, ProcOps};
use crate::platform::windows::selfuninstall::self_uninstall_cmd_line;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_INVALID_PARAMETER, FALSE, FILETIME, GENERIC_ALL,
    GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
    ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ALL_ACCESS,
    FILE_APPEND_DATA, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_EXECUTE,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE,
    FILE_GENERIC_READ, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
    FILE_WRITE_DATA, FILE_WRITE_EA, SYNCHRONIZE, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    ATTACH_PARENT_PROCESS, CTRL_BREAK_EVENT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE,
    REG_DWORD, REG_OPTION_NON_VOLATILE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    DETACHED_PROCESS, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE, STARTUPINFOW,
};

/// IPv4 TCP owner PID 行（`winproc.go:259-266` `MIB_TCPROW_OWNER_PID`）。
///
/// 与 Windows API 内存布局逐字段对齐（`#[repr(C)]` 保证无 padding）。
#[repr(C)]
struct MibTcpRowOwnerPid {
    state: u32,
    local_addr: u32,
    local_port: u32, // 网络字节序，仅低 16 位有效
    remote_addr: u32,
    remote_port: u32,
    owning_pid: u32,
}

/// IPv6 TCP owner PID 行（`winproc.go:268-278` `MIB_TCP6ROW_OWNER_PID`）。
#[repr(C)]
struct MibTcp6RowOwnerPid {
    local_addr: [u8; 16],
    local_scope_id: u32,
    local_port: u32,
    remote_addr: [u8; 16],
    remote_scope_id: u32,
    remote_port: u32,
    state: u32,
    owning_pid: u32,
}

/// STILL_ACTIVE（`winproc.go:141`，GetExitCodeProcess 的活跃码 = 259）。
const STILL_ACTIVE_CODE: u32 = 259;

/// 钉住 [`crate::platform::windows::logic::CREATE_NO_WINDOW`] 里硬编码的位值 == `windows-sys` 常量。
///
/// 同 `service::win` 对 `logic::pipe_open_mode` 的断言：纯逻辑层要在 Linux 上可单测，就不能引
/// `windows-sys`；镜像值与真值的一致性交给这条 windows-only 编译期断言，两边一动即编不过。
const _: () = {
    assert!(crate::platform::windows::logic::CREATE_NO_WINDOW == CREATE_NO_WINDOW);
};

/// 受管核的进程句柄与随之读到的身份（D2/D3）。
///
/// **为什么要一直持有这个句柄**：Windows 只在进程对象**没有任何句柄**时才回收其 PID
/// （[Process Handles and Identifiers]）。此前 start 完就 `drop(child)`，句柄一关，核一退出
/// 号码立刻可被别的进程复用 —— 于是 `OpenProcess(pid)` 恒成功、helper 报 running、app 的崩溃
/// 自愈永不触发；`TerminateProcess(OpenProcess(pid))` 更会砍到那个无辜的复用者。
/// 句柄在手 ⇒ 这个 PID 在核活着与死后都还是它自己的，status/reap/terminate 三条腿据此都变成
/// 「对同一个进程对象」的操作，而不是「对同一个号码」的操作。
///
/// 与 Job Object 无关：`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 系于 **job 句柄**，多持一个进程句柄
/// 不改变它的语义（helper 一死，job 句柄随进程关闭，内核照样连坐杀 child）。
///
/// [Process Handles and Identifiers]: https://learn.microsoft.com/en-us/windows/win32/procthread/process-handles-and-identifiers
#[derive(Debug)]
struct ManagedChild {
    /// 该句柄所指进程的 pid（判「问的是不是手里这个」）。
    pid: u32,
    /// CreateProcess 交回的进程句柄，收割完成前不关。
    handle: OwnedHandle,
    /// 起核当时从**同一个句柄**读到的身份（created + image）。
    identity: ManagedIdentity,
}

/// Windows FFI 生产实现（对应 Go `winproc.go` 全部原语）。
#[derive(Debug)]
pub struct WinProcOps {
    /// 常驻 Job Object 句柄（`winproc.go:71` `hJob`）。
    /// 惰性创建（ensure_job），所有 child assign 进去 → helper 死则内核连坐杀 child。
    /// Mutex 保证 ensure_job 的「已建则直接返回」幂等性（`winproc.go:75-78`）。
    job: std::sync::Mutex<Option<HANDLE>>,
    /// 当前受管核的进程句柄 + 身份（D2/D3，见 [`ManagedChild`]）。收割时取出并关闭。
    managed: std::sync::Mutex<Option<ManagedChild>>,
}

impl Default for WinProcOps {
    fn default() -> Self {
        Self {
            job: std::sync::Mutex::new(None),
            managed: std::sync::Mutex::new(None),
        }
    }
}

impl WinProcOps {
    #[must_use]
    pub fn new() -> Self {
        let ops = Self::default();
        // Job Object 与核心内容无关，helper 服务启动时即可准备。start 路径仍会调用 ensure_job，
        // 因而此处瞬时失败不会丢掉后续重试机会；正常冷启则不再让用户请求承担创建成本。
        let _ = ops.ensure_job();
        ops
    }
}

// SAFETY: WinProcOps 的两把句柄（job 的裸 HANDLE、受管核的 OwnedHandle）都是内核对象句柄，
// 跨线程共享安全（Windows 句柄本身线程无关）。各自的 Mutex 串行化访问，满足 Send + Sync。
// 手写 impl 只为裸 HANDLE 那把（`*mut c_void` 非 Send/Sync）；OwnedHandle 本就是 Send + Sync。
#[allow(
    unsafe_code,
    reason = "the mutexes serialize access to the thread-independent job and child HANDLEs"
)]
unsafe impl Send for WinProcOps {}
#[allow(
    unsafe_code,
    reason = "the mutexes serialize access to the thread-independent job and child HANDLEs"
)]
unsafe impl Sync for WinProcOps {}

fn preopen_log_files(log_path: &str) -> std::io::Result<polaris_log_budget::PreopenedLogFiles> {
    let path = Path::new(log_path);
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path must have an absolute parent directory",
        )
    })?;
    if path.file_name().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path must name a file",
        ));
    }

    // CreateFileW 没有稳定的 std 级 relative-to-directory-handle API。逐级打开绝对 prefix，
    // OPEN_REPARSE_POINT 后验的是**同一个 handle**；每个 prefix 又不共享 DELETE，所以下一级
    // CreateFileW 执行期间该目录不能被 rename/换 junction。全部 prefix 持有到两个 final 打开为止。
    let mut prefixes = Vec::<File>::new();
    let mut cursor = PathBuf::new();
    let mut rooted = false;
    for component in parent.components() {
        match component {
            Component::Prefix(prefix) if cursor.as_os_str().is_empty() => {
                cursor.push(prefix.as_os_str());
            }
            Component::RootDir => {
                cursor.push(component.as_os_str());
                rooted = true;
                prefixes.push(open_directory_no_reparse(&cursor)?);
            }
            Component::CurDir => {}
            Component::Normal(name) if rooted => {
                cursor.push(name);
                prefixes.push(open_directory_no_reparse(&cursor)?);
            }
            Component::ParentDir | Component::Normal(_) | Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "log path must be absolute and contain no parent traversal",
                ));
            }
        }
    }
    if !rooted || prefixes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path must be absolute",
        ));
    }

    let current = open_regular_no_reparse(path)?;
    let rotated = open_regular_no_reparse(&polaris_log_budget::rotated_path(path))?;
    Ok(polaris_log_budget::PreopenedLogFiles::new(current, rotated))
}

fn open_directory_no_reparse(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let info = file_information(&file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !file.metadata()?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "log directory component must not be a reparse point",
        ));
    }
    Ok(file)
}

fn open_regular_no_reparse(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        // App diagnostics and an immediately following helper start may read/write the same objects;
        // deliberately omit SHARE_DELETE so their names cannot be replaced while privileged writes live.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let info = file_information(&file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !file.metadata()?.is_file()
        || info.nNumberOfLinks != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "log object must be a non-reparse, single-link regular file",
        ));
    }
    Ok(file)
}

#[allow(
    unsafe_code,
    reason = "GetFileInformationByHandle validates the exact already-open log object"
)]
fn file_information(file: &File) -> std::io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: `file` owns a live kernel handle and `info` points to writable storage of the exact
    // structure required by GetFileInformationByHandle for the duration of this synchronous call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful GetFileInformationByHandle initialized every field of `info`.
    Ok(unsafe { info.assume_init() })
}

#[allow(
    unsafe_code,
    reason = "owns the process-lifetime job HANDLE and transient process HANDLEs"
)]
impl WinProcOps {
    /// ensureJob（`winproc.go:75-96`）：惰性创建常驻 job 并设 KILL_ON_JOB_CLOSE。幂等。
    fn ensure_job(&self) -> Option<HANDLE> {
        let mut guard = self
            .job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(h) = *guard {
            return Some(h);
        }
        // SAFETY: CreateJobObjectW(NULL, NULL) 创建匿名 job（无名、默认安全描述符），线程安全。
        // 失败返回 NULL。句柄由本结构持有到进程退出（内核自动关闭 → KILL_ON_JOB_CLOSE 生效）。
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return None;
        }
        // SAFETY: SetInformationJobObject 设置 extended limit。info 在本栈帧存活期间调用有效。
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION 按 Win32 布局（#[repr(C)] 由 windows-sys 保证）。
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: 同上。失败 → 关句柄返 None（不泄漏）。
        let ok = unsafe {
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // SAFETY: 关闭未成功的 job 句柄，避免泄漏。
            unsafe { CloseHandle(h) };
            return None;
        }
        *guard = Some(h);
        Some(h)
    }

    /// assignToJob（`winproc.go:105-115`）：把 child pid assign 进常驻 job。best-effort。
    fn assign_to_job(&self, h_job: HANDLE, pid: u32) {
        if h_job.is_null() || pid == 0 {
            return;
        }
        // SAFETY: OpenProcess 取 child 句柄。PID 复用安全：调用点在 Start 成功后立即执行，
        // child 必活、pid 必指向本 child（Go winproc.go:102-103 同款论证）。失败返 NULL，无害。
        let h = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, pid) };
        if h.is_null() {
            return;
        }
        // SAFETY: AssignProcessToJobObject 把 h 进程 assign 进 h_job。best-effort：失败无害。随后关句柄。
        unsafe {
            let _ = AssignProcessToJobObject(h_job, h);
            CloseHandle(h);
        }
    }

    /// 记账新的受管核句柄，替换（并关闭）上一把 —— 上一把若还在，它的进程早已不是受管核。
    fn remember_managed_child(&self, child: ManagedChild) {
        let mut guard = self
            .managed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(child); // 旧 ManagedChild 在此 drop → CloseHandle（不泄漏）
    }

    /// 取出（并从槽里摘除）`pid` 的受管句柄；`pid` 不是手里那个 → `None`，不动槽。
    ///
    /// 摘除即交出关闭权：调用方收割完 drop 掉它，那一刻 PID 才允许被系统复用。
    fn take_managed_handle(&self, pid: u32) -> Option<OwnedHandle> {
        let mut guard = self
            .managed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_some_and(|c| c.pid == pid) {
            return guard.take().map(|c| c.handle);
        }
        None
    }

    /// 用**持有的句柄**判受管核存活；`pid` 不是手里那个 → `None`（调用方回落按 pid 探活）。
    fn managed_alive(&self, pid: u32) -> Option<bool> {
        let guard = self
            .managed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = guard.as_ref().filter(|c| c.pid == pid)?;
        Some(handle_alive(child.handle.as_raw_handle().cast()))
    }
}

#[allow(
    unsafe_code,
    reason = "Win32 process enumeration and creation own every returned HANDLE"
)]
impl ProcOps for WinProcOps {
    fn process_alive(&self, ppid: u32) -> bool {
        // D3：问的若是**受管核**，用持有的句柄回答 —— 按 pid 现开句柄的老路答的是「这个号码上有
        // 进程吗」，核死后号码被复用时它恒真。其余 pid（父死看护的 app ppid 等）仍走无状态自由函数
        //（winproc.go:123-143 processAlive；watchParent 后台线程共用，不捕获 &self）。
        self.managed_alive(ppid)
            .unwrap_or_else(|| process_alive_raw(ppid))
    }

    fn managed_identity(&self, pid: u32) -> ManagedIdentity {
        let guard = self
            .managed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .as_ref()
            .filter(|c| c.pid == pid)
            .map(|c| c.identity.clone())
            .unwrap_or_default()
    }

    fn terminate_pid(&self, pid: u32) -> std::io::Result<()> {
        // winproc.go:146-153 terminatePid。委托无状态自由函数（收割序列/freeport 共用，不捕获 &self）。
        terminate_pid_raw(pid)
    }

    fn process_image_name(&self, pid: u32) -> String {
        // winproc.go:157-169 processImageName：QueryFullProcessImageNameW。失败返回 ""。
        // SAFETY: OpenProcess(QUERY_LIMITED_INFORMATION) 取 pid 句柄。
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if h.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 260]; // MAX_PATH
        let mut size = buf.len() as u32;
        // SAFETY: QueryFullProcessImageNameW 写入 buf（wide string）。flags=0 = Win32 路径。
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
        // SAFETY: 关句柄。
        unsafe { CloseHandle(h) };
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..size as usize])
    }

    fn kill_all_singbox(&self, singbox_bin: &str) -> usize {
        // winproc.go:184-214 killAllSingbox：CreateToolhelp32Snapshot 枚举进程，按映像全路径 EqualFold
        // 匹配 singbox_bin → TerminateProcess。返回杀掉的实例数。
        if singbox_bin.is_empty() {
            return 0;
        }
        // SAFETY: CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) 取系统进程快照。返回 INVALID_HANDLE_VALUE 失败。
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut pe: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut killed = 0usize;
        let target_base = filepath_base(singbox_bin);
        // SAFETY: Process32FirstW/NextW 遍历快照。pe 由上方 zeroed + dwSize 初始化。
        let mut ok = unsafe { Process32FirstW(snap, &mut pe) };
        while ok != 0 {
            let pid = pe.th32ProcessID;
            if pid != 0 && pid != 4 {
                // System Idle / System 跳过（winproc.go:198-200）
                // 先按 ExeFile（basename，UTF-16）粗筛，命中再取全路径精确比对。
                let base = wide_to_string(&pe.szExeFile);
                if eq_ignore_ascii_case_path(&base, target_base) {
                    let img = self.process_image_name(pid);
                    // clippy collapsible_if：两层内 if 合并（&& 短路保 terminate 仅在锁定命中时调用）。
                    if is_locked_singbox(&img, singbox_bin) && terminate_pid_raw(pid).is_ok() {
                        killed += 1;
                    }
                }
            }
            // SAFETY: 同上。
            ok = unsafe { Process32NextW(snap, &mut pe) };
        }
        // SAFETY: 关快照句柄。
        unsafe { CloseHandle(snap) };
        killed
    }

    fn start_singbox(
        &self,
        singbox_bin: &str,
        cfg: &str,
        log_path: &str,
        fwd: bool,
    ) -> std::io::Result<CoreStart> {
        // winproc.go:21-49 startSingbox + winproc.go:414-427 enableIPForwarding。
        let total_started = Instant::now();
        let forwarding_started = Instant::now();
        if fwd {
            self.enable_ip_forwarding();
        }
        let forwarding_ms = crate::elapsed_ms(forwarding_started);
        // B3/W26：改用 std Command 的 pipe，不再把 child 直接绑到一个永不重开的 append handle。
        // shared writer 在已预开的 current/.1 对象之间 copy/truncate，形成跨平台硬上限。
        let mut cmd = std::process::Command::new(singbox_bin);
        cmd.args(["run", "-c", cfg])
            .stdin(std::process::Stdio::null());
        if !log_path.is_empty() {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        }
        // SYSTEM helper 必须在 spawn 前固定日志对象。每级目录 handle 都拒绝 reparse 且不共享
        // DELETE，直到 current/.1 两个 file object 都打开；之后轮转只操作这两个 handle。
        // 安全打开失败只关闭日志能力，pipe 仍会被排空，不阻断核心启动。
        let log_files = if log_path.is_empty() {
            None
        } else {
            match preopen_log_files(log_path) {
                Ok(files) => Some(files),
                Err(error) => {
                    log::warn!("privileged core logging disabled: secure pre-open failed: {error}");
                    None
                }
            }
        };
        // CWD = 配置文件所在目录（= 用户可写 config 目录）。**不设的后果不是噪音，是写错地方**：
        // helper 是 SCM 服务，进程 CWD 恒为 `C:\Windows\System32`，child 不设就继承它，而 sing-box
        // 对配置里的**相对**路径按 CWD 解析。1.14.0-beta.15 起 tailscale endpoint 的
        // `taildrop_directory` 默认值就是相对的 `Taildrop`，且在 initialize 阶段无条件 `MkdirAll(0700)`
        // ⇒ 目录建在 System32 里，tailnet peer 发来的文件也落在那；helper 跑在 SYSTEM 下，这个 mkdir
        // 还会**成功**，于是没有任何报错。另一条更老的同型：`services[].dashboard` 省略 `path` 时的
        // 联网下载兜底目录 `dashboard`。
        //
        // App 直起（`runtime/proxy.rs`）、Linux helper（`platform/linux/server.rs`）、macOS helper
        // （`platform/macos/server.rs`）三条腿早就设了，本腿是漏的那条 —— 同一根因下做对的三条腿，
        // 正是「这不是有意取舍」的证据。取父目录的方式与 Linux 腿同（配置文件的所在目录），只是不能用
        // `std::path`（见 `logic::filepath_dir`）。取不到父目录（裸文件名）→ 不设，保持旧行为。
        if let Some(cwd) = filepath_dir(cfg) {
            cmd.current_dir(cwd);
        }
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        let process_started = Instant::now();
        let mut child = cmd.spawn()?;
        let process_ms = crate::elapsed_ms(process_started);
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // D2/D3：**不再 drop child**。把 std 的 Child 拆成裸句柄自行持有 —— drop 会关闭父侧进程
        // 句柄，而句柄一关，核退出后这个 PID 立刻可被系统复用（见 `ManagedChild` 文档）。
        // SAFETY: `into_raw_handle` 转移所有权且不关闭句柄；此处立刻用 OwnedHandle 接管，
        // 关闭时机唯一（收割后 drop）。stdout/stderr 已在上面取走，不受影响。
        let handle = unsafe { OwnedHandle::from_raw_handle(child.into_raw_handle()) };
        // 身份从**这个句柄**读一次并缓存：句柄在手 ⇒ 读到的必是刚起的这个进程；
        // 之后 status 每次直接回缓存值，既不重复 FFI，也不会在核退出后读到复用者的数据。
        let identity = ManagedIdentity {
            created: process_created_ticks(handle.as_raw_handle().cast()),
            image: process_image_by_handle(handle.as_raw_handle().cast()),
        };
        let created = identity.created;
        self.remember_managed_child(ManagedChild {
            pid,
            handle,
            identity,
        });
        // ensureJob + assignToJob（winproc.go:40-47）：best-effort 防孤儿安全网。失败不阻断 start。
        let job_started = Instant::now();
        if let Some(h_job) = self.ensure_job() {
            self.assign_to_job(h_job, pid);
        }
        let job_ms = crate::elapsed_ms(job_started);

        // 有界日志 writer 的旧文件裁剪/轮转是磁盘 IO，不属于「child 已受 Job 保护后才能回 PID」的
        // 正确性关键路径。把两条 pipe 连同所有权交给后台线程：主程序可立即开始管理端口就绪探测；
        // pipe 在接线完成前仍由该线程持有，不会因父侧提前 drop 而给核心制造 broken pipe。
        let log_handoff_started = Instant::now();
        if !log_path.is_empty() {
            std::thread::spawn(move || {
                if let Some(files) = log_files {
                    polaris_log_budget::spawn_pipe_loggers_with_preopened_files(
                        stdout,
                        stderr,
                        files,
                        polaris_log_budget::DEFAULT_GENERATION_BYTES,
                    );
                } else {
                    polaris_log_budget::spawn_pipe_drainers(stdout, stderr);
                }
            });
        }
        let log_handoff_ms = crate::elapsed_ms(log_handoff_started);
        Ok(CoreStart {
            pid,
            created,
            timing: polaris_helper_proto::StartTiming {
                forwarding_ms,
                process_ms,
                job_ms,
                log_handoff_ms,
                total_ms: crate::elapsed_ms(total_started),
            },
        })
    }

    fn reap_child(&self, pid: u32) {
        // W6 修：后台异步收割（Go stop/cleanup/uninstall 的 `go terminateChild(c, done)`）——不阻塞
        // 管道回复（此前同步 sleep(2s) 阻塞 stop 回复）。线程捕获 pid + **摘下来的句柄**（D3：
        // 收割全程对同一个进程对象，绝不按号码重开），不捕获 &self（故无生命周期问题）。
        let handle = self.take_managed_handle(pid);
        std::thread::spawn(move || reap_sequence(pid, handle));
    }

    fn reap_child_blocking(&self, pid: u32) {
        // 同步收割（Go reapChildOnExit 的**同步** terminateChild）：服务停止/关机路径须在返回前杀完
        // child，否则异步收割线程随进程退出消失 → 孤儿。
        let handle = self.take_managed_handle(pid);
        reap_sequence(pid, handle);
    }

    fn apply_route(&self, iface: &str, cidr: &str, del: bool) {
        // W9 修：真跑 netsh（Go helper.go:222-240）。此前 helper 分派层空壳回 OK、netsh 从未执行。
        // 地址族由 cidr 是否含 ':' 决定（Go 同款）；store=active 非持久（重启自清）。best-effort（Go .Run()）。
        let op = if del { "delete" } else { "add" };
        let fam = if cidr.contains(':') { "ipv6" } else { "ipv4" };
        let _ = std::process::Command::new("netsh")
            .arg("interface")
            .arg(fam)
            .arg(op)
            .arg("route")
            .arg(format!("prefix={cidr}"))
            .arg(format!("interface={iface}"))
            .arg("store=active")
            .status();
    }

    fn spawn_watch_parent(
        &self,
        ppid: u32,
        child_pid: u32,
        is_current: Box<dyn Fn(u32) -> bool + Send>,
        on_parent_dead: Box<dyn Fn(u32) + Send>,
    ) {
        // W15 修：接线父死看护（Go helper.go:131-159 `watchParent` + `go watchParent`）。此前
        // parent_pid 被丢弃、看护零调用 → app 崩溃/taskkill（管道 stop 够不到）后 sing-box 成孤儿
        //（Job 只兜「helper 自己死」，管不到「app≠helper 死、helper 仍活」）。
        // 后台线程只捕获 pid + 闭包 + 无状态 process_alive_raw（不捕获 &self）。
        std::thread::spawn(move || {
            let interval = std::time::Duration::from_secs(1); // Go: time.NewTicker(time.Second)
            loop {
                std::thread::sleep(interval);
                if !is_current(child_pid) {
                    return; // 已被 stop/cleanup/新 start 摘除（Go: child != c）
                }
                // processAlive：仅「确定父已死」返回 false（宁漏勿误）。
                if !process_alive_raw(ppid) {
                    if !is_current(child_pid) {
                        return; // 与 stop 竞态：他人已摘除则由他收割
                    }
                    on_parent_dead(child_pid);
                    return;
                }
            }
        });
    }

    fn spawn_self_uninstall(&self, service_name: &str, support_dir: &str) {
        // winproc.go:233-245 spawnSelfUninstall：CreateProcessW(cmd.exe, CmdLine 原样下发,
        // DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP)。旁路须比 helper 活得久（不 assignToJob、继承 SYSTEM token）。
        let cmd_line = self_uninstall_cmd_line(service_name, support_dir);
        let cmd_exe = std::env::var_os("ComSpec").unwrap_or_else(|| {
            let root =
                std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows"));
            // clippy useless_conversion：root 已是 OsString，无需再 OsString::from。
            let mut p = root;
            p.push("\\System32\\cmd.exe");
            p
        });
        let app_w = wide_null(&cmd_exe);
        let mut cmd_w = wide_null(OsString::from(cmd_line));
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: CreateProcessW(cmd.exe, CmdLine 原样)。DETACHED_PROCESS → 无 console、不随 helper 退出被收。
        // CREATE_NEW_PROCESS_GROUP → 旁路独立进程组。不 assignToJob → 不受 KILL_ON_JOB_CLOSE 连坐。
        // 继承 helper 的 SYSTEM token（子进程默认继承父 token）→ 有权 sc delete / 删 ProgramData。
        // lpApplicationName 用绝对 cmd.exe 路径（不依赖 SYSTEM 服务的 %PATH%）。
        let ok = unsafe {
            CreateProcessW(
                app_w.as_ptr(),
                cmd_w.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                FALSE,
                DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
        if ok != 0 {
            // SAFETY: 关句柄（不 Wait —— 旁路须比 helper 活得久，Go: 不 Wait，winproc.go:244）。
            unsafe {
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
            }
        }
        // best-effort：失败只记 log（Go: _ = c.Start()）。
        log::warn!("spawn_self_uninstall completed (ok={ok})");
    }

    fn flush_dns(&self) -> Result<(), String> {
        // D4：SYSTEM 下跑 ipconfig /flushdns。命令构造与结果判据都在 `logic`（Linux 可测），
        // 本腿只负责执行 + 自捕两条流。**不能用共用的 `system-integration::exec`**：它只把 stderr
        // 带进错误串，而 ipconfig 的失败文字在 stdout —— 那份全局格式被按串解析的消费方依赖，
        // 改它射程远大于收益，故此处局部自捕。
        let cmd = crate::platform::windows::logic::flush_dns_command(
            std::env::var("SystemRoot").ok().as_deref(),
        );
        let mut child = std::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .creation_flags(cmd.creation_flags)
            .spawn()
            .map_err(|e| format!("{} 启动失败: {e}", cmd.program))?;
        // 两条流各起一个读线程，**先于任何等待**（本仓纪律：先排空再等 —— 反过来就是子进程写满
        // 管道等父进程读、父进程等子进程退出那个死锁）。此前这里是 `.output()`：排空是对的，
        // 但它**没有上界** —— ipconfig 卡在 DNS Client 服务上时，这条连接线程与它占的那个管道实例
        // 被一起扣到 ipconfig 自己返回为止（取值理由见 `logic::FLUSH_DNS_TIMEOUT_MS`）。
        let mut out_pipe = child.stdout.take();
        let mut err_pipe = child.stderr.take();
        let out_reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(pipe) = out_pipe.as_mut() {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        });
        let err_reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(pipe) = err_pipe.as_mut() {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        });
        // 有界等待**进程句柄**；到点硬杀 —— 杀掉之后两条管道立刻 EOF，两个读线程随即收工，
        // 故超时腿既不泄漏线程也不留孤儿。
        // SAFETY: 句柄由 `child` 持有，本调用期间必然有效；WaitForSingleObject 只读它、不关它。
        let timed_out = unsafe {
            WaitForSingleObject(
                child.as_raw_handle().cast(),
                crate::platform::windows::logic::FLUSH_DNS_TIMEOUT_MS,
            )
        } != WAIT_OBJECT_0;
        if timed_out {
            let _ = child.kill();
        }
        let status = child.wait();
        let stdout = out_reader.join().unwrap_or_default();
        let stderr = err_reader.join().unwrap_or_default();
        if timed_out {
            return Err(crate::platform::windows::logic::flush_dns_timeout_error(
                crate::platform::windows::logic::FLUSH_DNS_TIMEOUT_MS,
            ));
        }
        crate::platform::windows::logic::flush_dns_result(
            status.ok().and_then(|s| s.code()),
            &stdout,
            &stderr,
        )
    }

    fn enable_ip_forwarding(&self) {
        // winproc.go:414-427 enableIPForwarding：IPv4 注册表 IPEnableRouter=1（持久写入）+
        // IPv6 netsh（运行时开关）。best-effort，失败不阻塞 start。
        enable_ip_forwarding_ipv4_registry();
        let _ = std::process::Command::new("netsh")
            .args(["interface", "ipv6", "set", "global", "forwarding=enabled"])
            .spawn();
    }

    fn read_object_security(&self, path: &str) -> Result<ObjectSecurity, String> {
        // P4 ACL 自检的搬运腿（判据在 `coreacl`，本函数一个判断都不做）。委托无状态自由函数
        //（不捕获 &self，与本文件其它 FFI 原语同款组织）。
        read_object_security_raw(path)
    }

    fn list_dir_names(&self, dir: &str) -> Result<Vec<String>, String> {
        // 取材面的枚举腿：目录里**实际有什么**就问什么（硬编码两个白名单名会漏掉攻击者预创建、
        // 带去继承 DACL 的第三个文件 —— 见 `coreacl::extend_targets_with_dir_entries`）。
        // 子目录也一并返回：受保护目录里一个谁都能写的子目录同样是可注入面。
        let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir({dir}) failed: {e}"))?;
        Ok(entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect())
    }
}

impl NetTableOps for WinProcOps {
    fn list_listen_entries(
        &self,
    ) -> std::io::Result<Vec<crate::platform::windows::logic::ListenEntry>> {
        // winproc.go:302-406 listenPidsForPort：GetExtendedTcpTable 枚举 IPv4 + IPv6 LISTEN 持有者。
        let mut out = Vec::new();
        for &family in &[AF_INET, AF_INET6] {
            let rows = enum_tcp_listener_rows(family)?;
            for (pid, net_port) in rows {
                let port = local_port_from_net_order(net_port);
                out.push(crate::platform::windows::logic::ListenEntry { pid, port });
            }
        }
        Ok(out)
    }

    fn listen_pids_for_port(&self, port: u16) -> std::io::Result<Vec<u32>> {
        let entries = self.list_listen_entries()?;
        Ok(filter_listen_pids(&entries, port))
    }
}

/// sendCtrlBreak（`winproc.go:57-59`）：向指定进程组投递 CTRL_BREAK_EVENT。
///
/// 重要限制：GenerateConsoleCtrlEvent 只路由给与调用者共享 console 的进程。本 helper 作为 session-0 的
/// LocalSystem 服务无 console ⇒ 服务模式下这一发**必然失败**（实测 `GetLastError=6`
/// `ERROR_INVALID_HANDLE`）。仅 --console dev 模式（有 console）下直接有效。
///
/// ⚠️ 这里此前的注释写的是「服务模式无 console → 返回 0，**无害**」。前半对，后半错，
/// 而且是最贵的那一半：它把「这条路走不通」写成了「这件事做不到」，于是没人再去找第二条路，
/// Windows 上 sing-box 的 `Store.Close()` 永不执行就此成为既定现实。
/// 第二条路见 [`send_ctrl_break_via_child_console`]（2026-08-12 G3 探针实测成立）。
#[allow(
    unsafe_code,
    reason = "GenerateConsoleCtrlEvent borrows only the child process-group id"
)]
fn send_ctrl_break(pid: u32) -> std::io::Result<()> {
    // SAFETY: GenerateConsoleCtrlEvent 投递控制信号。pid 为 child 进程组组长。
    let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 本进程的 console 控制处理器：吞掉事件并报告「已处理」。
///
/// 装它是为了在 [`send_ctrl_break_via_child_console`] 借用子进程 console 的那个窗口里，
/// **不被自己发出的 CTRL_BREAK 带走**。返回 TRUE = 已处理，不走默认的「终止本进程」。
#[allow(
    unsafe_code,
    reason = "matches the Windows console-handler ABI and dereferences no raw data"
)]
unsafe extern "system" fn ignore_ctrl_handler(_event: u32) -> BOOL {
    1
}

/// 无 console 时的第二条路：**借用子进程自己的 console** 再投一次 CTRL_BREAK。
///
/// # 依据（2026-08-12 实测，非推断）
///
/// G3 探针（`crates/helper/probes/ctrl_break_probe.rs`，run `31591517111`）在**真实 Windows 服务**
/// （session 0、从没碰过 console）里逐格测过：
/// - 直接 `GenerateConsoleCtrlEvent` → `api_ok=false`、`err=6`；
/// - `FreeConsole` → `AttachConsole(child)` → 重投 → 子进程**走到优雅退出分支**（`attach_ok=true`）。
///
/// 两个常见误读一并否掉：
/// - MSDN 说 `CREATE_NO_WINDOW` 下 "the console handle for the application is not set" ——
///   实测 `AttachConsole(child)` **成功**，子进程确实有一个可 attach 的 console，只是不属于父进程那个；
/// - **不需要**先给服务 `AllocConsole`：上面那组数据就是在从没碰过 console 的进程上取得的。
///
/// # 两处必须按这个顺序
///
/// 1. `SetConsoleCtrlHandler` 必须传**自己的 handler**，不能用 `SetConsoleCtrlHandler(NULL, TRUE)` ——
///    后者的语义只是「忽略 CTRL+C」，**对 CTRL_BREAK 无效**，不装的话这一发可能把 helper 自己带走
///    （探针首跑就是这样静默退出的）。
/// 2. 收尾要 `FreeConsole` 再**摘掉** handler：一直挂着会让 --console dev 模式下的 Ctrl+C 也失效。
///    摘除排在 `FreeConsole` 之后 —— 先脱离 console 就已经不在收件范围里了。
#[allow(
    unsafe_code,
    reason = "temporarily attaches to the child console and restores parent-console state"
)]
fn send_ctrl_break_via_child_console(pid: u32) -> std::io::Result<()> {
    // 服务模式下本来就没有 console（返回 0，无害）；dev --console 模式下必须先脱离，
    // 否则 AttachConsole 会失败——一个进程同一时刻只能连一个 console。
    // SAFETY: 无参数。
    unsafe { FreeConsole() };

    // SAFETY: attach 到子进程的 console。
    if unsafe { AttachConsole(pid) } == 0 {
        let e = std::io::Error::last_os_error();
        // 尽力还原 dev 模式下刚被丢掉的那个 console（服务里没有可还原的，失败无害）。
        // SAFETY: 常量 ATTACH_PARENT_PROCESS。
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        return Err(e);
    }

    // SAFETY: handler 是 'static fn，无捕获。
    unsafe { SetConsoleCtrlHandler(Some(ignore_ctrl_handler), 1) };
    // SAFETY: 同 send_ctrl_break，此刻已与子进程共享 console。
    let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
    let err = if ok == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };

    // SAFETY: 别把子进程的 console 一直攥着。
    unsafe { FreeConsole() };
    // SAFETY: 摘掉自己的 handler（第二个参数 FALSE = remove），否则 dev 模式的 Ctrl+C 会永久失效。
    unsafe { SetConsoleCtrlHandler(Some(ignore_ctrl_handler), 0) };
    // SAFETY: 还原 dev 模式的 console。
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// 宽限期内轮询子进程是否已退出。返回 `true` = **确定已死**。
///
/// [`process_alive_raw`] 的口径是「宁漏勿误」（不确定按存活），故本函数返回 false 只表示
/// 「没能确认它死了」，不表示它还活着 —— 调用方据此走硬杀兜底，方向是安全的。
///
/// `handle` 在手时按**句柄**问（D3：进程对象没被回收，退出码读得准且绝不会读到复用者）；
/// 没有句柄（非受管 pid / 已被别处摘走）才回落按 pid 探活。
fn wait_child_exit(pid: u32, handle: Option<&OwnedHandle>, grace: std::time::Duration) -> bool {
    let alive = || match handle {
        Some(h) => handle_alive(h.as_raw_handle().cast()),
        None => process_alive_raw(pid),
    };
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !alive() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    !alive()
}

/// 收割序列（`helper.go:111-123` `terminateChild`）：优雅信号 CTRL_BREAK → 宽限 2s → 硬杀。
///
/// **无状态**（仅 pid，无 `&self`），供异步 [`WinProcOps::reap_child`] 与同步
/// [`WinProcOps::reap_child_blocking`] 共用（W6：命令路径异步不阻塞、退出路径同步防孤儿）。
fn reap_sequence(pid: u32, handle: Option<OwnedHandle>) {
    // ① 直接投递。dev --console 模式下本就有效；服务模式下必然失败（err=6），失败即走 ②。
    // 优雅信号走的是**进程组**（console 语义），只能按 pid 投——这一步没有句柄版本。
    if send_ctrl_break(pid).is_err() {
        // ② 借子进程自己的 console 再投一次。**这一步才是服务模式下唯一能走通的优雅通道**
        //    （2026-08-12 G3 探针实测，见该函数文档）。任一步失败都只是回到 ③ 硬杀，
        //    即「不比改动前更差」——天然 fail-safe，故失败只吞不报。
        let _ = send_ctrl_break_via_child_console(pid);
    }

    // ③ 宽限期内轮询而不是死等满 2s（helper.go:120 的宽限值不变）。
    //    改动前这里恒睡满 2 秒是**合理的**：那时优雅信号从来不生效，早醒也没有意义。
    //    现在它能生效了，轮询把「停核」从恒定 2 秒缩到实际退出耗时。
    let exited = wait_child_exit(pid, handle.as_ref(), std::time::Duration::from_millis(2000));

    // ④ 硬杀兜底。**确认已死就不发这一刀** —— 不是为了省一次系统调用，而是躲开
    //    「pid 已被回收并复用给别的进程」这一格：那种情况下这一刀会砍到无关进程。
    //    探活宁漏勿误，故 `exited==false` 时照旧硬杀，方向安全。
    //    句柄在手时**按句柄杀**：那把刀只可能落在我们起的那个进程上，连「复用」这一格都不存在
    //    （句柄未关 ⇒ PID 未被回收）。没有句柄才回落 `terminate_pid_raw`。
    if !exited {
        match handle.as_ref() {
            Some(h) => {
                let _ = terminate_handle(h.as_raw_handle().cast());
            }
            None => {
                let _ = terminate_pid_raw(pid);
            }
        }
    }
    // handle 在此 drop → CloseHandle：核已收割，此刻起系统才可以复用这个 PID。
}

/// 用**已持有的进程句柄**判存活（`GetExitCodeProcess`）。口径同 [`process_alive_raw`]：
/// 只有确定读到「已退出」才判死，查询失败按存活（宁漏勿误）。
#[allow(
    unsafe_code,
    reason = "reads the exit code of an already-owned process HANDLE"
)]
fn handle_alive(h: HANDLE) -> bool {
    let mut code: u32 = 0;
    // SAFETY: h 由 ManagedChild 的 OwnedHandle 持有，调用期间必然有效；code 是可写本栈对象。
    let ok = unsafe { GetExitCodeProcess(h, &mut code) };
    if ok == 0 {
        return true; // 查询失败 → 不确定 → 按存活
    }
    code == STILL_ACTIVE_CODE
}

/// 用**已持有的进程句柄**硬杀（对照 [`terminate_pid_raw`] 的按 pid 版本）。
#[allow(
    unsafe_code,
    reason = "terminates an already-owned process HANDLE without reopening it by pid"
)]
fn terminate_handle(h: HANDLE) -> std::io::Result<()> {
    // SAFETY: h 由调用方的 OwnedHandle 持有；exit code=1（与 terminate_pid_raw 同）。
    if unsafe { TerminateProcess(h, 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 进程创建时间（`GetProcessTimes`，100ns tick）—— D3 的身份令牌。读不到 → `None`。
///
/// 只取 creation time：其余三个（exit/kernel/user）随运行而变，不能当身份用。
#[allow(
    unsafe_code,
    reason = "reads the creation time of an already-owned process HANDLE"
)]
fn process_created_ticks(h: HANDLE) -> Option<u64> {
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: h 有效；四个 FILETIME 均为可写本栈对象（API 要求四个出参都非空）。
    let ok = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) };
    if ok == 0 {
        return None; // 读不到就说读不到，绝不返回 0 冒充一个创建时间
    }
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// 实跑二进制全路径（`QueryFullProcessImageNameW`）—— D2 的自证事实来源。读不到 → `None`。
///
/// 缓冲按 Win32 路径上限（32767 wide chars）一次给足：本函数每次起核只调一次，宁可多要一次分配，
/// 也不要在长路径上返回一个截断的路径 —— 截断的路径会与期望路径判不等，变成一次假告警。
#[allow(
    unsafe_code,
    reason = "reads the image path of an already-owned process HANDLE"
)]
fn process_image_by_handle(h: HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    let mut size = buf.len() as u32;
    // SAFETY: h 有效；buf 可写且 size 与其长度一致；flags=0 = Win32 路径形态。
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    (!path.is_empty()).then_some(path)
}

/// 硬杀指定 pid（`winproc.go:146-153` `terminatePid`）。**无状态**自由函数（收割序列 / trait `terminate_pid`
/// / kill_all_singbox 共用）。
#[allow(
    unsafe_code,
    reason = "opens, terminates and closes one transient process HANDLE"
)]
fn terminate_pid_raw(pid: u32) -> std::io::Result<()> {
    // SAFETY: OpenProcess(TERMINATE) 取 pid 句柄。pid 来自 freeport/cleanup 的 LISTEN 持有者枚举或收割。
    let h = unsafe { OpenProcess(PROCESS_TERMINATE, FALSE, pid) };
    if h.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: TerminateProcess(h, 1) 硬杀。exit code=1（Go 同款）。
    let ok = unsafe { TerminateProcess(h, 1) };
    // SAFETY: 关句柄。
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 判 ppid 是否存活（`winproc.go:123-143` `processAlive`）。**无状态**自由函数（watchParent 后台线程 /
/// trait `process_alive` 共用，不捕获 `&self`）。
///
/// 宁漏勿误（对齐 macOS `kill(ppid,0)`）：仅「确定已死」（`ERROR_INVALID_PARAMETER` = pid 不存在）返回
/// false；其余不确定（ACCESS_DENIED / 查询失败）按存活返回 true。
#[allow(
    unsafe_code,
    reason = "opens, probes and closes one transient process HANDLE"
)]
fn process_alive_raw(ppid: u32) -> bool {
    if ppid == 0 {
        return false;
    }
    let access = SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
    // SAFETY: OpenProcess 取父进程句柄。pid 来自客户端（已被 token 鉴权为可信调用方）。
    let h = unsafe { OpenProcess(access, FALSE, ppid) };
    if h.is_null() {
        // ERROR_INVALID_PARAMETER（87，pid 已不存在）→ 确定已死；其余不确定 → 按存活。
        return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
    }
    let mut code: u32 = 0;
    // SAFETY: GetExitCodeProcess 写入 code。h 由上方 OpenProcess 返回（非 NULL）。
    let ok = unsafe { GetExitCodeProcess(h, &mut code) };
    // SAFETY: 关句柄。
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return true; // 查询失败 → 不确定 → 按存活
    }
    code == STILL_ACTIVE_CODE
}

/// 枚举指定 family（IPv4/IPv6）的 LISTEN 持有者（pid + 网络序 port）。
///
/// 两步法：首次调 GetExtendedTcpTable 取所需缓冲大小（返回 ERROR_INSUFFICIENT_BUFFER），
/// 再次调用填充。空表（size=4，仅 dwNumEntries=0 头）防越界（winproc.go:345-353）。
#[allow(
    unsafe_code,
    reason = "parses length-checked aligned TCP-table buffers returned by iphlpapi"
)]
fn enum_tcp_listener_rows(family: u32) -> std::io::Result<Vec<(u32, u32)>> {
    let mut size: u32 = 0;
    // SAFETY: 首次调用取大小（pTcpTable=NULL）。返回 ERROR_INSUFFICIENT_BUFFER 预期。
    let rc = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            1, // sorted=TRUE（BOOL i32）
            family,
            TCP_TABLE_OWNER_PID_LISTENER as i32,
            0,
        )
    };
    // ERROR_INSUFFICIENT_BUFFER = 122
    if rc != 0 && rc != 122 {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: 第二次调用填充 buf（按 size 分配足够）。
    let rc = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            1,
            family,
            TCP_TABLE_OWNER_PID_LISTENER as i32,
            0,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    // 解析：首 4 字节 = dwNumEntries（行数），其后是行数组。空表防越界。
    if buf.len() < 4 {
        return Ok(Vec::new());
    }
    let n = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(n as usize);
    if family == AF_INET {
        let row_size = std::mem::size_of::<MibTcpRowOwnerPid>();
        for i in 0..n as usize {
            let off = 4 + i * row_size;
            if off + row_size > buf.len() {
                break; // 防越界（GetExtendedTcpTable 保证表完整，但保守）
            }
            // SAFETY: off 已校验在 buf 范围内；Windows 表的行布局由 #[repr(C)] 对齐。
            // buf 是 Vec<u8>，只承诺 1 字节对齐，不能构造 `&MibTcpRowOwnerPid`；按值非对齐读取。
            let row = unsafe {
                std::ptr::read_unaligned(buf.as_ptr().add(off).cast::<MibTcpRowOwnerPid>())
            };
            if row.state == MIB_TCP_STATE_LISTEN {
                out.push((row.owning_pid, row.local_port));
            }
        }
    } else {
        let row_size = std::mem::size_of::<MibTcp6RowOwnerPid>();
        for i in 0..n as usize {
            let off = 4 + i * row_size;
            if off + row_size > buf.len() {
                break;
            }
            // SAFETY: 同 IPv4；范围已校验，且必须按值非对齐读取，不能从 Vec<u8> 构造对齐引用。
            let row = unsafe {
                std::ptr::read_unaligned(buf.as_ptr().add(off).cast::<MibTcp6RowOwnerPid>())
            };
            if row.state == MIB_TCP_STATE_LISTEN {
                out.push((row.owning_pid, row.local_port));
            }
        }
    }
    Ok(out)
}

/// enableIPForwarding IPv4 注册表写入（`winproc.go:417-424`）。
///
/// HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\IPEnableRouter=1（DWORD）。
/// 持久写入（stop/卸载不还原 —— 已知限制 M3，镜像 Go 现状）。
#[allow(
    unsafe_code,
    reason = "opens, writes and closes the fixed TCP/IP Parameters registry key"
)]
fn enable_ip_forwarding_ipv4_registry() {
    let subkey: Vec<u16> = OsString::from("SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let value_name: Vec<u16> = OsString::from("IPEnableRouter")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: RegCreateKeyExW 打开/创建注册表键。KEY_SET_VALUE 权限。SYSTEM 有权。
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return;
    }
    let data: u32 = 1;
    // SAFETY: RegSetValueExW 写 IPEnableRouter=1（DWORD）。hkey 由上方 RegCreateKeyExW 返回。
    let _ = unsafe {
        RegSetValueExW(
            hkey,
            value_name.as_ptr(),
            0,
            REG_DWORD,
            &data as *const _ as *const u8,
            std::mem::size_of::<u32>() as u32,
        )
    };
    // SAFETY: 关键句柄。
    unsafe { RegCloseKey(hkey) };
}

// ===== P4：受保护核目录的 owner/DACL 读取（搬运腿，判据在 `coreacl`）=====

/// `ACCESS_ALLOWED_ACE_TYPE`。
///
/// 值取自 `Win32_System_SystemServices`（该 feature **未开**）。不为两个 `u8` 常量多开一个
/// windows-sys feature —— 同 `service::win` 里 `SDDL_REVISION_1` / `FILE_FLAG_FIRST_PIPE_INSTANCE`
/// 的先例（那两个也是 winbase.h 字面值）。这两个值是 ACE 的 ABI 常量，改动等于换协议。
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

/// `ACCESS_DENIED_ACE_TYPE`（同上）。
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

/// 钉住 [`coreacl`] 里硬编码的掩码 == `windows-sys` 常量的并集。
///
/// 那些位值**必须**写在 `coreacl` 里（它在 Linux 上也编译，好让判据有门可跑；而 `windows-sys`
/// 是 `[target.'cfg(windows)'.dependencies]`，Linux 上不在依赖图里）。这几条 windows-only
/// 编译期断言是两侧之间唯一的对账 —— 任一侧改了就编不过，且 CI 的
/// `cargo clippy --target x86_64-pc-windows-msvc` 会把它跑到。
///
/// 后两条断言不是重言：它们钉住的是**判据层**的两个结论 —— icacls `(F)` 必被判成写类、
/// `(RX)` 必**不**被判成写类。把 `FILE_ALL_ACCESS` 整值并进掩码（它含 `READ_CONTROL` 与
/// `SYNCHRONIZE`，而 `(RX)` 同样含这两位）会让第三条立刻编不过。
const _: () = {
    assert!(
        coreacl::WRITE_ACCESS_MASK
            == FILE_WRITE_DATA
                | FILE_APPEND_DATA
                | FILE_WRITE_EA
                | FILE_DELETE_CHILD
                | FILE_WRITE_ATTRIBUTES
                | DELETE
                | WRITE_DAC
                | WRITE_OWNER
                | GENERIC_ALL
                | GENERIC_WRITE
    );
    assert!(FILE_ALL_ACCESS & coreacl::WRITE_ACCESS_MASK != 0);
    assert!((FILE_GENERIC_READ | FILE_GENERIC_EXECUTE) & coreacl::WRITE_ACCESS_MASK == 0);
    assert!(coreacl::ACE_FLAG_INHERIT_ONLY == INHERIT_ONLY_ACE);
    // 只读执行那份掩码同样两侧对账；末一条钉住的是结论：icacls `(RX)` 必落进读执行类
    //（否则 `Users:(RX)` 缺失的 warn 自曝腿会恒不触发，而缺失恰恰是它要报的那个事）。
    assert!(
        coreacl::READ_EXECUTE_ACCESS_MASK
            == FILE_READ_DATA | FILE_EXECUTE | GENERIC_ALL | GENERIC_EXECUTE | GENERIC_READ
    );
    assert!((FILE_GENERIC_READ | FILE_GENERIC_EXECUTE) & coreacl::READ_EXECUTE_ACCESS_MASK != 0);
};

/// 读一个文件系统对象的 owner SID + DACL，搬运成纯数据（spec §3.4）。
///
/// `Err` 恒表示**读不到这个对象**，且只剩两格：`GetNamedSecurityInfoW` 自身失败、返回的安全描述符
/// 为 null。调用方按 Q9 warn 继续。
///
/// **描述符拿到手之后就再没有 `Err` 了**：owner SID 转串失败落 `owner_sid: None`，
/// `GetAclInformation` / `GetAce` / ACE 的 SID 转串失败各落一条 [`coreacl::AceKind::Unparsed`]
/// 哨兵 —— 那些都是「这一格判不了」，不是「这个对象读不到」，折过去就是一个失败向**开**的洞
///（一条判不出的 ACE 会把同对象上另一条真放宽的 ACE 一起藏掉）。
/// 「读到了、但被放宽」同样不在本函数的表达范围内：那是 [`coreacl::judge_object`] 的事。
///
/// **所有权纪律**：`GetNamedSecurityInfoW` 成功时返回一个 `LocalAlloc` 的安全描述符，
/// owner 与 DACL 两个出参都指向**它内部**（不各自分配）⇒ 只需释放 `sd` 一次，且必须在把
/// 需要的字节拷成 owned 数据之后。SACL 不要（读它需 `SE_SECURITY_NAME` 特权）、group 不要。
#[allow(
    unsafe_code,
    reason = "reads one object's security descriptor and frees its single LocalAlloc allocation"
)]
fn read_object_security_raw(path: &str) -> Result<ObjectSecurity, String> {
    let path_w = wide_null(OsString::from(path));
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: path_w 是 NUL 结尾 UTF-16；四个出参均为本栈帧的有效可写位置。成功（返回
    // ERROR_SUCCESS）时 sd 取得 LocalAlloc 所有权，owner/dacl 指向 sd 内部（不单独释放）。
    // 失败时三者不被写入所有权，无需释放。
    let rc = unsafe {
        GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if rc != 0 {
        // WIN32_ERROR：2 = ERROR_FILE_NOT_FOUND（如不带 cronet 的核里没有 libcronet.dll）、
        // 5 = ERROR_ACCESS_DENIED。原样带出码值供真机定位。
        return Err(format!("GetNamedSecurityInfoW({path}) failed: {rc}"));
    }
    if sd.is_null() {
        return Err(format!(
            "GetNamedSecurityInfoW({path}) returned a null security descriptor"
        ));
    }
    let parsed = parse_object_security(path, owner, dacl);
    // SAFETY: sd 是上面 GetNamedSecurityInfoW 成功返回且尚未释放的 LocalAlloc 指针；
    // parse_object_security 已把需要的字节拷成 owned（String / Vec），此后 owner/dacl 都不再被
    // 借用 ⇒ 恰好释放一次。
    let free_result = unsafe { LocalFree(sd.cast()) };
    // 泄漏要在 release 里也说得出来：`debug_assert!` 在生产二进制里整条消失，于是「每次自检漏一块
    // 安全描述符」这种缓增故障没有任何自曝腿（helper 是常驻服务，起核一次漏一次）。
    if !free_result.is_null() {
        log::warn!("LocalFree(security descriptor for {path}) failed; the allocation leaked");
    }
    Ok(parsed)
}

/// 把 owner SID 与 DACL 指针搬成 [`ObjectSecurity`]（**必须在 `LocalFree(sd)` 之前调**）。
#[allow(
    unsafe_code,
    reason = "reads borrowed SID and ACL bytes out of a live security descriptor"
)]
fn parse_object_security(path: &str, owner: PSID, dacl: *mut ACL) -> ObjectSecurity {
    // owner 为 NULL = 安全描述符里**没有属主**；SID 转串失败 = 有属主但判读不了。两者都落
    // `None`，由判据判成异常（`AclIssue::OwnerMissing`）。
    //
    // **刻意不折成 `Err`**：`Err` 的语义是「这个对象读不到」（→ warn 继续），而这里对象是读到了
    // 的，只是属主这一格没法呈现。折过去既是事实错误，也是本门里的一个失败向开的洞 ——
    // 「判不出属主」必须与「属主不对」同向处置。
    let owner_sid = if owner.is_null() {
        None
    } else {
        sid_to_string(owner)
    };
    // dacl 为 NULL = **NULL DACL**（Win32 语义：所有人完全访问）。与「0 条 ACE 的空 DACL」
    // 语义相反，故两者在 `ObjectSecurity` 里是不同的表示（None vs Some(vec![])）。
    if dacl.is_null() {
        return ObjectSecurity {
            owner_sid,
            dacl: None,
        };
    }
    let mut info: ACL_SIZE_INFORMATION = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: dacl 非空且指向 sd 内部的有效 ACL；info 是本栈帧的 ACL_SIZE_INFORMATION，
    // 长度按其 size_of 传入（API 据 class 决定写多少）。
    let ok = unsafe {
        GetAclInformation(
            dacl,
            std::ptr::addr_of_mut!(info).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        // **不折成 `Err`**：`Err` 的语义是「这个对象读不到」（→ warn 继续），可
        // `GetNamedSecurityInfoW` 刚刚成功返回，对象是**读到了**的 —— Q9 的「读不到 ≠ 被放宽」
        // 这条前提在这里不成立。读不出条数的 DACL 就是一份判不了的 DACL ⇒ 一条哨兵 ACE，由判据
        // 侧既有的失败向关处置（`AclIssue::UnparsedAce`）。判据零改动。
        log::warn!("GetAclInformation({path}) failed; treating the DACL as unreadable");
        return ObjectSecurity {
            owner_sid,
            dacl: Some(vec![coreacl::Ace::unreadable()]),
        };
    }
    let mut aces = Vec::with_capacity(info.AceCount as usize);
    for i in 0..info.AceCount {
        aces.push(read_ace(path, dacl, i));
    }
    ObjectSecurity {
        owner_sid,
        dacl: Some(aces),
    }
}

/// 读第 `index` 条 ACE（**必须在 `LocalFree(sd)` 之前调**）。
///
/// **不返回 `Result`**：本函数的任何失败都只意味着「这条 ACE 判不了」，而不是「这个对象读不到」
/// —— 后者会把一条判不出的 ACE 变成 warn 继续，于是同一对象上另一条**真放宽**的 ACE 被一起藏掉。
#[allow(
    unsafe_code,
    reason = "reads one borrowed ACE out of a live ACL by its Win32 layout"
)]
fn read_ace(path: &str, dacl: *mut ACL, index: u32) -> coreacl::Ace {
    let mut pace: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: dacl 指向 sd 内部的有效 ACL；index < AceCount（由调用方的循环上界保证）。
    // 成功时 pace 指向 ACL 内部的一条 ACE（**借用**，不拥有）。
    let ok = unsafe { GetAce(dacl, index, &mut pace) };
    if ok == 0 || pace.is_null() {
        log::warn!("GetAce({path}, {index}) failed; treating the ACE as unreadable");
        return coreacl::Ace::unreadable();
    }
    // ACE_HEADER 在**所有** ACE 类型里都位于起始处、布局相同 ⇒ AceType/AceFlags 恒可读。
    // SAFETY: pace 指向一条完整 ACE，其首字段即 ACE_HEADER（ACL 内的 ACE 按 DWORD 对齐）。
    let header = unsafe { *pace.cast::<ACE_HEADER>() };
    let flags = u32::from(header.AceFlags);
    let kind = match header.AceType {
        ACCESS_ALLOWED_ACE_TYPE => coreacl::AceKind::Allow,
        ACCESS_DENIED_ACE_TYPE => coreacl::AceKind::Deny,
        // 其余两族都走这里，但成因不同（细节见 `coreacl::AceKind::Unparsed`）：
        // - **object ACE**（5/6/7…）头部多一个 Flags + 最多两个 GUID ⇒ `SidStart` 偏移与
        //   `ACCESS_ALLOWED_ACE` **不同**，按那个布局读出来的 SID 是垃圾；
        // - **callback / conditional ACE**（9 / 0xA）布局**逐字节相同**，SID 与 mask 其实读得出来，
        //   但条件表达式（挂在 SID 之后）本批不解析 —— 条件为假时该 ACE 不生效，按 mask 直接判会假红。
        // 两族都只报原始类型号，让判据失败向关。
        other => {
            return coreacl::Ace {
                sid: String::new(),
                mask: 0,
                kind: coreacl::AceKind::Unparsed(other),
                flags,
            }
        }
    };
    // ACCESS_ALLOWED_ACE 与 ACCESS_DENIED_ACE 布局逐字节相同（Header/Mask/SidStart），
    // 故两族共用这一个布局读取。
    let ace = pace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: 同上；kind 已证实是 allow/deny 两族之一，其布局即 ACCESS_ALLOWED_ACE。
    let mask = unsafe { (*ace).Mask };
    // SidStart 是 SID 的**首字段**（不是指针）—— 取它的地址即 PSID。
    // SAFETY: 同上；addr_of! 不构造引用，避免对紧随其后的变长 SID 字节做越界假设。
    let psid: PSID = unsafe { std::ptr::addr_of!((*ace).SidStart) }
        .cast::<std::ffi::c_void>()
        .cast_mut();
    // SID 转串失败 ⇒ 「读到了这条 ACE，但说不出它授给谁」。与未知 ACE 类型**同一族**，同样报
    // `Unparsed`（失败向关：判不出受托主体就无法证明它不是一条授给非特权主体的授予）。
    //
    // **刻意不折成 `Err`**：那会把「这条 ACE 判不了」变成「这个对象读不到」→ warn 继续，于是
    // 一条 SID 判不出来的 ACE 就能把同一对象上另一条真放宽的 ACE 一起藏掉。
    let Some(sid) = sid_to_string(psid) else {
        return coreacl::Ace {
            sid: String::new(),
            mask,
            kind: coreacl::AceKind::Unparsed(header.AceType),
            flags,
        };
    };
    coreacl::Ace {
        sid,
        mask,
        kind,
        flags,
    }
}

/// PSID → `S-1-...` 串。失败返回 `None`（调用方折成读失败）。
///
/// **所有权纪律**：`ConvertSidToStringSidW` 成功时返回一个 `LocalAlloc` 的宽串，调用方负责
/// `LocalFree`（与安全描述符是两块独立分配，漏了就是每次自检泄漏一块）。
#[allow(
    unsafe_code,
    reason = "converts one borrowed SID and frees the string it allocates"
)]
fn sid_to_string(psid: PSID) -> Option<String> {
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: psid 指向调用方持有的有效 SID（sd 内部）。成功时 out 取得 LocalAlloc 所有权。
    let ok = unsafe { ConvertSidToStringSidW(psid, &mut out) };
    if ok == 0 || out.is_null() {
        return None;
    }
    // SAFETY: out 是 NUL 结尾的宽串（API 契约）；只读到首个 NUL 为止，不越界。
    let len = unsafe {
        let mut n = 0usize;
        while *out.add(n) != 0 {
            n += 1;
        }
        n
    };
    // SAFETY: [out, out+len) 是上面数出来的有效宽字符区间。
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
    // SAFETY: out 是 ConvertSidToStringSidW 成功返回且尚未释放的 LocalAlloc 指针；
    // 上一行已把字节拷进 owned String，此后不再借用 ⇒ 恰好释放一次。
    let free_result = unsafe { LocalFree(out.cast()) };
    // 同 `read_object_security_raw`：release 下也要自曝（每条 ACE 一块，比描述符那块漏得更快）。
    if !free_result.is_null() {
        log::warn!("LocalFree(SID string {s}) failed; the allocation leaked");
    }
    Some(s)
}

/// 宽字符串辅助：OsStr → null 终止 UTF-16（供 windows-sys API）。
fn wide_null(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// PROCESSENTRY32W.szExeFile（固定 260 wide）→ String。
fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

// 父死看护（watchParent，`helper.go:131-159`）现由 [`WinProcOps::spawn_watch_parent`] 承载（W15 接线：
// helper 分派层在 start 成功后调 trait 方法起后台线程；此前的 zero-call 自由函数版已删）。

#[cfg(test)]
mod tests;

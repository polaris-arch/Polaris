//! 孤儿扫描在 Windows 上的进程表访问：枚举 pid、留住一个进程的句柄，并经它读命令行、令牌用户与
//! 创建时间；清扫一侧的结束与探活也在这里。
//!
//! 只做系统调用的事；命令行怎么切、切完怎么判、属主怎么归类，都在上一层的纯函数里。

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    FALSE, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL,
    STATUS_INFO_LENGTH_MISMATCH, STILL_ACTIVE, UNICODE_STRING,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, IsWellKnownSid, TokenUser, WinLocalSystemSid, PSID, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

use super::TokenUserFacts;

/// 上一层按「两个 `u16` 之后补齐到一个指针宽度」读应答头里的指针；这里钉住它与系统头文件的布局一致。
const _: () = assert!(
    std::mem::offset_of!(UNICODE_STRING, Buffer) == std::mem::size_of::<usize>()
        && std::mem::size_of::<UNICODE_STRING>() == 2 * std::mem::size_of::<usize>()
);

/// 命令行应答缓冲区的初始字节数；不够时按系统报回的长度重试。
const INITIAL_REPLY_BYTES: usize = 2048;

/// 快照里全部进程的 pid；拍不了快照即空。
#[allow(unsafe_code, reason = "ToolHelp process snapshot is Win32 FFI")]
pub(super) fn process_ids() -> Vec<u32> {
    let mut pids = Vec::new();
    // SAFETY: 无指针参数；失败返回 INVALID_HANDLE_VALUE。
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return pids;
    }
    // SAFETY: `raw` 是刚交回、尚无其它所有者的有效快照句柄。
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: 快照句柄由 `snapshot` 保活；`entry` 是可写的本栈对象且 `dwSize` 已按约定填好。
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle().cast(), &mut entry) };
    while more != 0 {
        pids.push(entry.th32ProcessID);
        // SAFETY: 同上。
        more = unsafe { Process32NextW(snapshot.as_raw_handle().cast(), &mut entry) };
    }
    pids
}

/// `TokenUser` 应答缓冲区的 `u64` 个数：一个 `TOKEN_USER` 头（两个指针宽）加一个 SID 的上限
/// （`SECURITY_MAX_SID_SIZE` = 68 字节），128 字节有余。
const TOKEN_USER_WORDS: usize = 16;

fn raw_handle(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}

/// 一个进程令牌的用户 SID。
///
/// 系统把 `TOKEN_USER` 头与 SID 本体写进同一块缓冲区，头里的指针指向这块缓冲区内部 —— 所以
/// 缓冲区放在堆上：本值随便搬动，那个指针仍然有效。`u64` 元素保证头所需的对齐。
pub(super) struct TokenUserSid(Box<[u64; TOKEN_USER_WORDS]>);

impl TokenUserSid {
    /// 本进程令牌的用户；读不到即 `None`（那时没有任何候选算「同一用户」）。
    #[allow(unsafe_code, reason = "GetCurrentProcess is Win32 FFI")]
    pub(super) fn of_current_process() -> Option<Self> {
        // SAFETY: 无参数；返回的是不需关闭的伪句柄。
        Self::of(unsafe { GetCurrentProcess() })
    }

    /// `process` 须带 `PROCESS_QUERY_LIMITED_INFORMATION`。令牌打不开（别的用户或 SYSTEM 的进程，
    /// 本进程未提权时的常态）或读不出即 `None`。
    #[allow(
        unsafe_code,
        reason = "OpenProcessToken and GetTokenInformation are Win32 FFI"
    )]
    fn of(process: HANDLE) -> Option<Self> {
        let mut token = std::ptr::null_mut();
        // SAFETY: `process` 由调用方保活；只请求查询权限；`token` 是可写的本栈对象。
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        // SAFETY: `token` 是刚交回、尚无其它所有者的有效令牌句柄。
        let token = unsafe { OwnedHandle::from_raw_handle(token.cast()) };
        let mut reply = Box::new([0u64; TOKEN_USER_WORDS]);
        let mut written = 0u32;
        // SAFETY: 令牌句柄由 `token` 保活；缓冲区可写且长度参数就是它的字节数；`written` 可写。
        let ok = unsafe {
            GetTokenInformation(
                raw_handle(&token),
                TokenUser,
                reply.as_mut_ptr().cast(),
                std::mem::size_of_val(&*reply) as u32,
                &mut written,
            )
        };
        (ok != 0).then_some(Self(reply))
    }

    #[allow(unsafe_code, reason = "reads the TOKEN_USER header the system wrote")]
    fn sid(&self) -> PSID {
        // SAFETY: 本值只由成功的 `GetTokenInformation(TokenUser)` 构造，缓冲区开头是一个完整的
        // `TOKEN_USER`；`u64` 元素满足它的对齐。
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// 扫描时留住的一个进程：一把只带查询权限的句柄，加上当时读到的创建时间。
///
/// 句柄开着期间系统不会把这个 pid 分给别的进程，所以之后经它读到的、按这个 pid 另开的，都是
/// 扫描时看到的那一个。丢弃即关闭。
pub(super) struct HeldProcess {
    pid: u32,
    handle: OwnedHandle,
    /// 创建时间（100ns tick）；读不到即 `None`，那时不结束它。
    created: Option<u64>,
}

impl HeldProcess {
    /// 打不开（别的会话、受保护进程、已退出）即 `None`。
    #[allow(unsafe_code, reason = "OpenProcess is Win32 FFI")]
    pub(super) fn open(pid: u32) -> Option<Self> {
        // SAFETY: 只请求受限查询权限，不修改目标；失败返回空句柄。
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: `handle` 是刚交回、尚无其它所有者的有效进程句柄。
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        let created = creation_time(&handle);
        Some(Self {
            pid,
            handle,
            created,
        })
    }

    /// 该进程的整串命令行；读不到即 `None`。
    pub(super) fn command_line(&self) -> Option<String> {
        command_line(&self.handle)
    }

    /// 令牌用户的两件事实；令牌打不开即 `None`。`me` 是本进程令牌的用户。
    #[allow(unsafe_code, reason = "EqualSid and IsWellKnownSid are Win32 FFI")]
    pub(super) fn token_user_facts(&self, me: Option<&TokenUserSid>) -> Option<TokenUserFacts> {
        let user = TokenUserSid::of(raw_handle(&self.handle))?;
        // SAFETY: 两个 SID 指针各自指向仍存活的 `TokenUserSid` 缓冲区内部。
        let same_user = me.map(|me| unsafe { EqualSid(user.sid(), me.sid()) } != 0);
        // SAFETY: 同上。
        let local_system = unsafe { IsWellKnownSid(user.sid(), WinLocalSystemSid) } != 0;
        Some(TokenUserFacts {
            same_user,
            local_system,
        })
    }

    /// 结束这个进程，不连带子进程。
    ///
    /// 手里那把句柄只有查询权限，结束要另开一把带 `PROCESS_TERMINATE` 的。按 pid 开是安全的：
    /// 手里的句柄还开着，这个 pid 换不了人；创建时间再对一次，对不上（或任何一次没读到）就不动手。
    /// 系统拒绝（别的用户 / SYSTEM 的进程）或进程已退出时返回 `false`。
    #[allow(unsafe_code, reason = "OpenProcess and TerminateProcess are Win32 FFI")]
    pub(super) fn terminate(&self) -> bool {
        // SAFETY: 无指针参数；失败返回空句柄。
        let target = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                FALSE,
                self.pid,
            )
        };
        if target.is_null() {
            return false;
        }
        // SAFETY: `target` 是刚交回、尚无其它所有者的有效进程句柄。
        let target = unsafe { OwnedHandle::from_raw_handle(target.cast()) };
        if !super::same_birth(self.created, creation_time(&target)) {
            return false;
        }
        // SAFETY: `target` 带结束权限且由本栈保活；退出码是任意非零值。
        unsafe { TerminateProcess(raw_handle(&target), 1) != 0 }
    }

    /// 是否还在跑。只有系统明确给出退出码才判已退出；查询失败按还在跑算（没有死亡证据）。
    #[allow(unsafe_code, reason = "GetExitCodeProcess is Win32 FFI")]
    pub(super) fn is_running(&self) -> bool {
        let mut exit_code = 0u32;
        // SAFETY: 句柄由 `self` 保活；`exit_code` 是可写的本栈对象。
        let ok = unsafe { GetExitCodeProcess(raw_handle(&self.handle), &mut exit_code) };
        ok == 0 || exit_code == STILL_ACTIVE as u32
    }
}

/// 进程的创建时间（100ns tick）；读不到即 `None`。句柄须带 `PROCESS_QUERY_LIMITED_INFORMATION`。
#[allow(unsafe_code, reason = "GetProcessTimes is Win32 FFI")]
fn creation_time(process: &OwnedHandle) -> Option<u64> {
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: 句柄由调用方保活；四个 FILETIME 均为可写的本栈对象。
    let ok = unsafe {
        GetProcessTimes(
            raw_handle(process),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// 该进程的整串命令行；读不到即 `None`。句柄须带 `PROCESS_QUERY_LIMITED_INFORMATION`。
#[allow(unsafe_code, reason = "NtQueryInformationProcess is NT FFI")]
fn command_line(process: &OwnedHandle) -> Option<String> {
    // `u64` 元素保证应答头（含一个指针）所需的对齐。
    let mut reply = vec![0u64; INITIAL_REPLY_BYTES / 8];
    for _ in 0..3 {
        let capacity = reply.len() * 8;
        let mut needed = 0u32;
        // SAFETY: 进程句柄由调用方保活；缓冲区可写且长度参数就是它的字节数；`needed` 可写。
        let status = unsafe {
            NtQueryInformationProcess(
                raw_handle(process),
                ProcessCommandLineInformation,
                reply.as_mut_ptr().cast(),
                capacity as u32,
                &mut needed,
            )
        };
        if status >= 0 {
            let bytes: Vec<u8> = reply.iter().flat_map(|word| word.to_ne_bytes()).collect();
            return super::command_line_from_reply(&bytes, reply.as_ptr() as usize);
        }
        let too_small = matches!(
            status,
            STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL | STATUS_BUFFER_OVERFLOW
        );
        if !too_small || needed as usize <= capacity {
            return None;
        }
        reply = vec![0u64; (needed as usize).div_ceil(8)];
    }
    None
}

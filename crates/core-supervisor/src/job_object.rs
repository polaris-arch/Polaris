//! Windows 作业对象：主程序直起的内核随主程序一起结束。
//!
//! # 守的是什么
//!
//! `tokio::process::Child` 没有 `kill_on_drop`，Windows 也没有「父进程死则子进程收到信号」的机制：
//! 主程序被强杀、崩溃或被任务管理器结束时，它直起的 sing-box 会继续活着，占着端口与缓存文件。
//! 带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的作业对象把这件事交给内核：作业的**最后一个句柄**
//! 关闭时，作业里的进程全部被结束。句柄由本进程持有到进程结束，进程以任何方式消失，内核都会
//! 关掉它的句柄表。
//!
//! # 进程级单例
//!
//! 三条直起腿（主核、测速临时核、Tailscale 登录核）都经 [`TokioSpawner`](crate::TokioSpawner)，
//! 共用一个作业（[`process_job`]）。句柄放在 `static` 里，Rust 不析构 `static`，所以正常运行期
//! 没有任何路径会关掉它；停核照旧走各自的 `Child`，与作业无关。
//!
//! # 只纳入被显式交进来的进程
//!
//! 主程序自己**不**进这个作业，所以它起的别的子进程（安装脚本、卸载程序、`taskkill`、重启后的
//! 新实例）不受影响。
//!
//! # 残余窗口（如实登记）
//!
//! 进程由 `CreateProcess` 创建之后才被纳入。主程序若恰在这两步之间消失，那一个内核不在作业里。
//! 消掉这个窗口要在创建时就指定作业（`PROC_THREAD_ATTRIBUTE_JOB_LIST`），而 std / tokio 的
//! `Command` 在稳定版上给不出这个属性；先挂起再恢复的做法只是把「在跑的孤儿」换成「挂起的孤儿」，
//! 还多出一条「恢复失败则内核永远起不来」的腿。这个窗口里漏下的内核由下一次起核前的孤儿清扫
//! （[`scan_running_cores`](crate::scan_running_cores)）收走。

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// 一个匿名作业对象，最后一个句柄关闭时结束其中全部进程。
///
/// 丢弃本值即关闭句柄；它若是这个作业仅有的句柄，作业里的进程随即被结束。
#[derive(Debug)]
pub struct KillOnCloseJob {
    handle: OwnedHandle,
}

impl KillOnCloseJob {
    /// 建一个带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的匿名作业。
    ///
    /// # Errors
    ///
    /// 建作业或设限制失败时返回系统错误；后一种情形下刚建出的句柄已关闭。
    #[allow(
        unsafe_code,
        reason = "CreateJobObjectW and SetInformationJobObject are Win32 FFI"
    )]
    pub fn new() -> io::Result<Self> {
        // SAFETY: 两个指针参数都允许为空（默认安全描述符、匿名作业）；失败返回空句柄。
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` 是刚由 CreateJobObjectW 交回、尚无其它所有者的有效句柄。
        let handle = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` 在调用期间存活，长度参数就是它的大小；`raw` 由上面的 `handle` 保活。
        let ok = unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle().cast()
    }

    /// 把一个进程纳入本作业。
    ///
    /// 进程若已在别的作业里，Windows 8 起会把本作业嵌套进去；更早的系统或不允许嵌套的外层作业
    /// 会让这一步失败。
    ///
    /// # Errors
    ///
    /// 系统拒绝纳入时返回系统错误。
    #[allow(unsafe_code, reason = "AssignProcessToJobObject is Win32 FFI")]
    pub fn assign(&self, process: BorrowedHandle<'_>) -> io::Result<()> {
        // SAFETY: 两个句柄在调用期间都由各自的所有者保活。
        let ok = unsafe { AssignProcessToJobObject(self.raw(), process.as_raw_handle().cast()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// 该进程此刻是否在本作业里。
    ///
    /// # Errors
    ///
    /// 查询失败时返回系统错误。
    #[allow(unsafe_code, reason = "IsProcessInJob is Win32 FFI")]
    pub fn contains(&self, process: BorrowedHandle<'_>) -> io::Result<bool> {
        let mut inside: BOOL = 0;
        // SAFETY: 两个句柄在调用期间都由各自的所有者保活；`inside` 是可写的本栈变量。
        let ok = unsafe { IsProcessInJob(process.as_raw_handle().cast(), self.raw(), &mut inside) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(inside != 0)
    }
}

static PROCESS_JOB: OnceLock<Result<KillOnCloseJob, String>> = OnceLock::new();

/// 本进程的作业（首次调用时创建，此后恒为同一个）。
///
/// # Errors
///
/// 创建失败时返回原因；失败同样被记住，不在每次起核时重试。
pub fn process_job() -> Result<&'static KillOnCloseJob, &'static str> {
    PROCESS_JOB
        .get_or_init(|| KillOnCloseJob::new().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(String::as_str)
}

/// 把刚起的内核纳入本进程的作业。
///
/// # Errors
///
/// 作业建不出来、子进程已被收割或系统拒绝纳入时返回原因。
#[allow(
    unsafe_code,
    reason = "borrows the process HANDLE that the tokio Child owns"
)]
pub fn enroll_child(child: &tokio::process::Child) -> Result<(), String> {
    let job = process_job()?;
    let raw = child
        .raw_handle()
        .ok_or("the child has already been reaped")?;
    // SAFETY: `raw` 由 `child` 持有，而 `child` 的借用覆盖本次调用；句柄不会在此期间关闭。
    let process = unsafe { BorrowedHandle::borrow_raw(raw) };
    job.assign(process).map_err(|error| error.to_string())
}

/// [`TokioSpawner`](crate::TokioSpawner) 的接线点：纳入失败只留一行日志，不影响起核。
pub(crate) fn enroll_spawned_core(child: &tokio::process::Child) {
    let pid = child.id();
    match enroll_child(child) {
        Ok(()) => log::debug!("core pid={pid:?} joined the kill-on-close job"),
        Err(error) => log::warn!(
            "core pid={pid:?} is NOT in the kill-on-close job ({error}); \
             it will outlive this process if the process is killed"
        ),
    }
}

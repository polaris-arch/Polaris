//! 当前联网是否计费（只读查询）。
//!
//! - Windows：`NetworkInformation` 给出的联网成本；
//! - macOS：路径监视器回报的当前路径是否「昂贵」；
//! - 其余平台本模块不答（Linux 由调用方问 NetworkManager，手机走各自的原生查询）。
//!
//! 取不到一律 `None`，不当作非计费。系统读数到三态的折算是纯函数，不带平台门，任何平台都能单测；
//! 真正去问系统的两段各自编译期隔离。

/// Windows `NetworkCostType` 的取值（0 是未知）。
const WINDOWS_COST_UNRESTRICTED: i32 = 1;
const WINDOWS_COST_FIXED: i32 = 2;
const WINDOWS_COST_VARIABLE: i32 = 3;

/// macOS `nw_path_status_satisfied`：路径当前可用。
const NW_PATH_STATUS_SATISFIED: i32 = 1;

/// Windows 的联网成本 → 是否计费。漫游、已超流量上限、按量或有上限的套餐都算计费；
/// 不限量算非计费；成本未知（且没有漫游与超限这两个明确信号）即不可得。
#[must_use]
pub const fn fold_windows_cost(
    cost_type: i32,
    roaming: bool,
    over_data_limit: bool,
) -> Option<bool> {
    if roaming || over_data_limit {
        return Some(true);
    }
    match cost_type {
        WINDOWS_COST_FIXED | WINDOWS_COST_VARIABLE => Some(true),
        WINDOWS_COST_UNRESTRICTED => Some(false),
        _ => None,
    }
}

/// macOS 的当前路径 → 是否计费。路径不可用（没有网络）时「昂贵」位没有意义，按不可得。
#[must_use]
pub const fn fold_macos_path(status: i32, expensive: bool) -> Option<bool> {
    if status == NW_PATH_STATUS_SATISFIED {
        Some(expensive)
    } else {
        None
    }
}

/// 问系统当前联网是否计费；`None` 是不可得。
///
/// 会阻塞：Windows 上是一次同步的系统调用，macOS 上至多等到路径回报的超时。异步调用方要放到
/// 阻塞线程上。
#[must_use]
pub fn metered() -> Option<bool> {
    #[cfg(windows)]
    {
        windows_metered()
    }
    #[cfg(target_os = "macos")]
    {
        macos::metered()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

#[cfg(windows)]
const _: () = {
    use windows::Networking::Connectivity::NetworkCostType;
    assert!(NetworkCostType::Unrestricted.0 == WINDOWS_COST_UNRESTRICTED);
    assert!(NetworkCostType::Fixed.0 == WINDOWS_COST_FIXED);
    assert!(NetworkCostType::Variable.0 == WINDOWS_COST_VARIABLE);
};

/// 没有联网配置（未联网）或任何一步出错，一律不可得。
#[cfg(windows)]
fn windows_metered() -> Option<bool> {
    use windows::Networking::Connectivity::NetworkInformation;

    let cost = NetworkInformation::GetInternetConnectionProfile()
        .ok()?
        .GetConnectionCost()
        .ok()?;
    fold_windows_cost(
        cost.NetworkCostType().ok()?.0,
        cost.Roaming().ok()?,
        cost.OverDataLimit().ok()?,
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::{Block, RcBlock};

    /// 等首次路径回报的上限。监视器启动后随即回报当前路径；到时没等到即不可得。
    const PATH_TIMEOUT: Duration = Duration::from_secs(2);
    /// `QOS_CLASS_UTILITY`。
    const QOS_CLASS_UTILITY: isize = 0x11;

    #[allow(
        unsafe_code,
        reason = "Network.framework declarations are this module's native ABI boundary"
    )]
    #[link(name = "Network", kind = "framework")]
    unsafe extern "C" {
        fn nw_path_monitor_create() -> *mut c_void;
        fn nw_path_monitor_set_update_handler(
            monitor: *mut c_void,
            handler: &Block<dyn Fn(*mut c_void)>,
        );
        fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
        fn nw_path_monitor_start(monitor: *mut c_void);
        fn nw_path_monitor_cancel(monitor: *mut c_void);
        fn nw_path_get_status(path: *mut c_void) -> i32;
        fn nw_path_is_expensive(path: *mut c_void) -> bool;
        fn nw_release(object: *mut c_void);
    }

    #[allow(
        unsafe_code,
        reason = "libdispatch declaration is this module's native ABI boundary"
    )]
    unsafe extern "C" {
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
    }

    /// 起一个路径监视器，取它回报的第一条路径后即取消。
    #[allow(
        unsafe_code,
        reason = "each native call carries its own SAFETY argument"
    )]
    pub(super) fn metered() -> Option<bool> {
        let (sender, receiver) = mpsc::sync_channel::<(i32, bool)>(1);
        // 回调在系统的并发队列上执行：闭包只捕获通道发送端（可跨线程共享），只留第一条回报。
        let handler = RcBlock::new(move |path: *mut c_void| {
            if path.is_null() {
                return;
            }
            // SAFETY: path 是监视器传入的有效路径对象，在本次回调返回前由监视器持有；
            // 两个函数只读它，不接管所有权。
            let report = unsafe { (nw_path_get_status(path), nw_path_is_expensive(path)) };
            let _ = sender.try_send(report);
        });
        // SAFETY: 无参创建，返回 +1 引用的监视器（或空），由本函数末尾的 nw_release 配平。
        let monitor = unsafe { nw_path_monitor_create() };
        if monitor.is_null() {
            return None;
        }
        // SAFETY: monitor 非空且尚未启动。回调块由监视器自行复制并持有到取消完成；全局队列不需要
        // 释放。按接口约定先设回调与队列再启动。
        unsafe {
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0));
            nw_path_monitor_start(monitor);
        }
        let report = receiver.recv_timeout(PATH_TIMEOUT);
        // SAFETY: monitor 仍是创建时取得的那一个引用；取消后不再有新的回调，随后放掉本函数的引用，
        // 此后不再使用它。
        unsafe {
            nw_path_monitor_cancel(monitor);
            nw_release(monitor);
        }
        report
            .ok()
            .and_then(|(status, expensive)| super::fold_macos_path(status, expensive))
    }
}

#[cfg(test)]
mod tests;

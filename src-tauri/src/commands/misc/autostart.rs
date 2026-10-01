use crate::response::{ok_void, ApiResponse};
use tauri::AppHandle;
#[cfg(desktop)]
use tauri::Manager;

/// 上游 `AUTO_START_SET`：开机自启开关的**执行侧**（`config.autoStart` 由前端另行 `update` 落盘）。
///
/// 三条平台腿，真值都住在「开机那一刻真正干活的一方」：
///  · 桌面：OS launch agent（autostart 插件）；
///  · Android：Kotlin 侧标记文件，由 `BootReceiver` 在开机后读（此时 Rust 不在），语义是
///    「开机后自动连接」—— 移动端没有「只把应用拉起来」这件事可做；
///  · 其余移动平台（iOS）：没有对应物，**显式报错**而不是静默 ok（静默成功会把开关点亮成「已开」，
///    而开机时什么都不会发生）。
///
/// async 是为了 Android 腿的跨桥往返；桌面腿仍是同步调用。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub async fn auto_start_set(app: AppHandle, enabled: bool) -> ApiResponse<()> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        match crate::runtime::proxy::android_bridge::set_boot_auto_connect(enabled).await {
            Ok(()) => ok_void(),
            Err(e) => ApiResponse::err(e),
        }
    }
    #[cfg(all(mobile, not(target_os = "android")))]
    {
        let _ = (app, enabled);
        ApiResponse::err("autostart is not available on this platform".to_string())
    }
    #[cfg(desktop)]
    {
        let autostart = app.state::<tauri_plugin_autostart::AutoLaunchManager>();
        let res = if enabled {
            autostart.enable()
        } else {
            autostart.disable()
        };
        match res {
            Ok(()) => ok_void(),
            Err(e) => ApiResponse::err(format!("{e}")),
        }
    }
}

/// 上游 `AUTO_START_GET_STATUS`：自启状态（读执行侧真值，不读 `config.autoStart`）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub async fn auto_start_get_status(app: AppHandle) -> ApiResponse<bool> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        // 读不到就说读不到（`err`），不折成 `false`：「关着」与「不知道」在设置页上是两句话。
        match crate::runtime::proxy::android_bridge::boot_auto_connect().await {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::err(e),
        }
    }
    // 读侧回 `false`（而不是报错）：这是**状态查询**，「没有开机自启」正是该平台的真值。
    #[cfg(all(mobile, not(target_os = "android")))]
    {
        let _ = app;
        ApiResponse::ok(false)
    }
    #[cfg(desktop)]
    {
        let autostart = app.state::<tauri_plugin_autostart::AutoLaunchManager>();
        ApiResponse::ok(autostart.is_enabled().unwrap_or(false))
    }
}

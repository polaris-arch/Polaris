//! 窗口控制类 command（Polaris window:minimize/maximizeToggle/close/isMaximized + app 排序）。
//!
//! 映射 channel：
//! - `window:minimize` → [`window_minimize`]
//! - `window:maximizeToggle` → [`window_maximize_toggle`]
//! - `window:close` → [`window_close`]
//! - `window:isMaximized` → [`window_is_maximized`]
//! - `app:restart` → [`app_restart`]（U-7「需重启 App 才生效」的设置改动后由用户确认触发）
//! - `app:startupConfigFlags` → [`app_startup_config_flags`]（U-7 判据基线：本次进程启动时读到的三键值）
//! - `renderer:ready` → [`renderer_ready`]（renderer mount 健康门信号）
//! - `fatal:retry` → [`fatal_retry`]（终局错误页「重新加载」按钮）
//! - `renderer:log` → [`renderer_log`]（renderer 错误转发到 Rust 日志）
//!
//! Linux 嵌入式标题栏自绘 min/max/close；Mac 原生红绿灯 / Win titleBarOverlay 系统按钮无需。
//! 最大化态变更广播 event:windowMaximizeChanged（标题栏跟随）。

#[cfg(target_os = "android")]
use std::sync::atomic::Ordering;

use serde_json::json;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::events::channel::EVENT_WINDOW_MAXIMIZE_CHANGED;
use crate::response::{ok_void, ApiResponse};
use crate::window_health::MountGateEvent;

/// 广播主窗最大化真值。按钮命令与原生窗口事件桥共用这一处，避免 payload/channel 漂移。
pub(crate) fn emit_window_maximize_changed(app: &AppHandle, maximized: bool) {
    crate::events::broadcast(
        app,
        EVENT_WINDOW_MAXIMIZE_CHANGED,
        json!({ "maximized": maximized }),
    );
}

/// 上游 `WINDOW_MINIMIZE`：最小化主窗口。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn window_minimize(window: WebviewWindow) -> ApiResponse<()> {
    // 移动端没有「最小化」这个窗口状态（Activity 的可见性由系统与返回键决定），Tauri 也不提供
    // 该方法。命令仍注册、仍返回 ok：IPC 契约面只留一份。
    // [不选「整条 command `cfg(desktop)` 掉」：那要同时分叉 `generate_handler![]`、前端 invoke 面
    //  与 `check-ipc-args` 的三方对拍；而移动端一旦调到它，拿到的是不可诊断的 command not found]
    // [不选「像 autostart 那样显式报错」：这条命令只由桌面自绘标题栏的按钮触发，移动端不渲染
    //  标题栏 ⇒ 没有会被误导的调用方；而 autostart 是设置页的持久开关，静默成功会点亮一个假状态]
    #[cfg(desktop)]
    let _ = window.minimize();
    #[cfg(mobile)]
    let _ = window;
    ok_void()
}

/// 上游 `WINDOW_MAXIMIZE_TOGGLE`：切换最大化/还原 + 广播 event:windowMaximizeChanged。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn window_maximize_toggle(app: AppHandle, window: WebviewWindow) -> ApiResponse<()> {
    // 移动端 Activity 恒占满可用区，没有可切换的最大化状态 —— 既不调窗口 API（Tauri 在 mobile
    // 上就没有这两个方法），**也不广播**一个必然为假的 `maximized=true`：那条事件会让标题栏
    // 图标与真实窗口状态对不上。命令保留注册的理由同 [`window_minimize`]。
    #[cfg(desktop)]
    {
        let maximized = window.is_maximized().unwrap_or(false);
        if maximized {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
        // 广播新最大化态（标题栏图标跟随）。
        let new_max = !maximized;
        emit_window_maximize_changed(&app, new_max);
    }
    #[cfg(mobile)]
    let _ = (app, window);
    ok_void()
}

/// 上游 `WINDOW_CLOSE`：关闭主窗口。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn window_close(window: WebviewWindow) -> ApiResponse<()> {
    let _ = window.close();
    ok_void()
}

/// 上游 `WINDOW_IS_MAXIMIZED`：是否最大化。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn window_is_maximized(window: WebviewWindow) -> ApiResponse<bool> {
    ApiResponse::ok(window.is_maximized().unwrap_or(false))
}

/// `app:restart`：必须先确认所有本地 owner 已关闭，才设置重启意图并 request_restart。
/// Tauri 的 RESTART_EXIT_CODE 忽略 prevent_exit，故不能把准备工作留给 ExitRequested。
/// 准备失败不置 QuitState/RestartState；本地 custody 留在原进程，用户可重试。
#[tauri::command]
pub async fn app_restart(app: AppHandle) -> ApiResponse<()> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let ready = match crate::exit_lifecycle::prepare_desktop_exit(&app).await {
            Ok(ready) => ready,
            Err(error) => {
                log::error!("重启前退出准备失败: {error}");
                return ApiResponse::err("后台连接尚未确认关闭，应用保持运行，请重试重启");
            }
        };
        match crate::exit_lifecycle::commit_desktop_exit(
            &app,
            ready,
            crate::exit_lifecycle::ExitKind::Restart,
        ) {
            Ok(()) => ok_void(),
            Err(error) => ApiResponse::err(error),
        }
    }
    #[cfg(target_os = "android")]
    {
        app.state::<crate::QuitState>()
            .0
            .store(true, Ordering::SeqCst);
        app.state::<crate::RestartState>()
            .0
            .store(true, Ordering::SeqCst);
        app.request_restart();
        ok_void()
    }
    #[cfg(target_os = "ios")]
    {
        // Restart only the host; the system-owned NE session is preserved by
        // the iOS ExitRequested/Exit dispatch and is reconciled on cold start.
        app.state::<crate::QuitState>()
            .0
            .store(true, std::sync::atomic::Ordering::SeqCst);
        app.state::<crate::RestartState>()
            .0
            .store(true, std::sync::atomic::Ordering::SeqCst);
        app.request_restart();
        ok_void()
    }
}

/// `app:startupConfigFlags`：本次进程**启动时**读到的「需重启 App 才生效」三键的生效值（U-7 判据基线）。
///
/// 只读、无副作用。渲染端拿它当基线判「重启到底会不会改变什么」——拿磁盘现值当基线会在
/// 「改走又改回」时误报一次重启（而重启会断代理），详见 `lib.rs` 的 [`crate::StartupConfigFlags`]。
///
/// 值在 `setup` 里定格，进程生命周期内不变；webview 自愈重载后重新拉取拿到的仍是同一份，
/// 这正是**不能**在渲染端自行快照的原因（重载会让渲染端的"启动值"漂移到重载那一刻的磁盘值）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn app_startup_config_flags(app: AppHandle) -> ApiResponse<crate::StartupConfigFlags> {
    ApiResponse::ok(*app.state::<crate::StartupConfigFlags>())
}

/// `app:takeCleanExitFlag`：上次进程是不是**正常退出**的？—— **读即清**（spec §2.5 Q1-b 清除时机 ④）。
///
/// 真 ⇒ 上次走完了退出腿（托盘「退出」/ ⌘Q / 末窗关闭 / `app:restart`），渲染端据此在 hydrate **之前**
/// 清掉持久化的暂存；假 ⇒ 强杀 / 崩溃 / 断电，或者进程压根没退（webview 自愈重载、C16 轻量模式销毁
/// 重建）—— 照常恢复。为什么是「留标记 + 下次读」而不是退出时通知 webview，见 [`crate::clean_exit`]。
///
/// **每个进程只有第一次调用会返回真**：标记在读的同一次系统调用里被消费掉。这是不变式而非实现细节 ——
/// 不清的话，正常退出一次之后每次启动都会清 staged，强杀那条恢复腿永远走不到。
///
/// 运行时未装配（极早期）⇒ 返 `false`（保守：恢复而非清除，方向与 Q1-b「宁可多恢复一次」一致）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn app_take_clean_exit_flag(app: AppHandle) -> ApiResponse<bool> {
    let clean = app
        .try_state::<crate::runtime::AppRuntime>()
        .is_some_and(|rt| crate::clean_exit::take(rt.config.dir()));
    ApiResponse::ok(clean)
}

/// 上游 `RENDERER_READY`：renderer 成功 mount 信号（主进程 mount 健康门）。
///
/// 经此确认 renderer 进程活着且当前真实页面已越过 Suspense fallback、DOM 真的挂上 —— C 类白屏
/// （进程活着但 DOM 空）不发任何平台事件，「约定回发 ready + 主进程超时」是唯一侦测手段。正常发出点
/// 见 `ui/src/components/screens/ScreenRouter.tsx` 的内容提交边界；根 ErrorBoundary / 同步静态 fallback
/// 各自也会回报可交互兜底已挂上，抑制无谓的 3s 上屏等待与终局升级。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn renderer_ready(app: AppHandle) -> ApiResponse<()> {
    crate::window_health::dispatch(&app, MountGateEvent::RendererReady);
    ok_void()
}

/// 上游 `FATAL_RETRY`：终局错误页「重新加载」——**复位 mount 门 + 导航回真实应用**。
///
/// 不变式 6「真恢复」：原实现只 `window.eval("location.reload()")`，门仍停在 `finalized`
/// → 恢复后的页面若再白屏就彻底无兜底，按钮沦为一次性假承诺。现经
/// [`crate::window_health::retry_from_fatal`] 先 `reset()` 门再导航，重载后的新文档会重新武装。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn fatal_retry(app: AppHandle) -> ApiResponse<()> {
    crate::window_health::retry_from_fatal(&app);
    ok_void()
}

/// 上游 `RENDERER_LOG`：renderer 错误转发到 Rust 日志（限频 + 截断）。
///
/// Tauri 没有 Electron `console-message` 那样的主进程事件，故由 renderer 侧主动上报
/// （`ui/src/main.tsx` 钩 console.error / window.onerror / unhandledrejection）。
/// 这是 C 类白屏「零可观测」的根治 —— 在此之前白屏时日志里一行痕迹都没有。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn renderer_log(app: AppHandle, level: String, message: String) -> ApiResponse<()> {
    crate::window_health::forward_renderer_log(&app, &level, &message);
    ok_void()
}

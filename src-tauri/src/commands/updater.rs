//! 更新类 command：应用自更新、内核版本信息、卸载。
//!
//! | command | 说明 |
//! |---|---|
//! | `version_get_info` / `update_check` / `update_download` / `update_get_progress` / `update_install` / `update_skip` / `update_open_releases` | 应用自更新 |
//! | `update_popup_*` | mini 更新弹窗 |
//! | `core_get_version_info` | 内核信息（只读：随包内核自报版本、清单声明版本、补丁集标识） |
//! | `app_uninstall_all` | 卸载编排，见 [`crate::runtime::uninstall`] |
//!
//! 内核没有任何写命令：应用内不提供检查、下载、上传、回滚或恢复出厂。
//!
//! # 未接线的命令返 `success:false` 而非 `{ok:false}`
//!
//! `ApiResponse::ok(json!({"ok": false}))` 的信封是 `success:true`，前端 `ipc-client.ts` 不 throw，
//! 于是「功能没实现」在调用侧长得和「一次正常的业务性失败」一模一样。未接线者一律用
//! [`ApiResponse::err_with_code`](crate::response::ApiResponse::err_with_code)（`success:false` +
//! 结构化 `code`）。**没实现就不能返成功信封。**
//!
//! # 供应链：应用安装包的真伪与完整性靠什么
//!
//! | 层 | 手段 |
//! |---|---|
//! | 传输 | HTTPS（rustls）+ `safe_redirect_fetch` 逐跳 SSRF 闸 |
//! | 完整性 | Content-Length 比对（[`CoreDownloader`](crate::runtime::http::CoreDownloader)）**＋ sha256 强校验**（GitHub release asset 的 `digest` 字段） |
//! | 真伪 | OS 层。走 ad-hoc 签名（不买 Developer ID / Authenticode）⇒ macOS 必须在安装脚本里清 quarantine，Windows 必须提前告知 SmartScreen 的点法。判定逻辑见 [`crate::runtime::update_install::install_advisory`] |
//!
//! 不引 `tauri-plugin-updater`，**不需要任何应用级签名密钥对**。

mod app_update;
mod app_update_policy;
mod core_info;
mod shared;
#[path = "updater/uninstall.rs"]
mod uninstall_command;

pub use app_update::{
    update_check, update_clear_portable_handoff, update_download, update_get_progress,
    update_install, update_open_releases, update_popup_action, update_popup_show,
    update_request_install_permission, update_skip, version_get_info,
};
pub use core_info::core_get_version_info;
pub use uninstall_command::app_uninstall_all;

// 仅供同 crate 的启动任务复用。
pub(crate) use app_update::{is_portable_layout, GITHUB_FETCH_TIMEOUT_MS, MAX_GITHUB_JSON_BYTES};
pub(crate) use app_update_policy::app_update_channel_is_prerelease;
use shared::fetch_releases_json;
#[cfg(test)]
use shared::github_status_error;
pub(crate) use shared::updater_downloader;

#[cfg(test)]
mod tests;

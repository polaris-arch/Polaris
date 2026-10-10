//! 内核信息命令：只读。

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::response::ApiResponse;
use crate::runtime::AppRuntime;
use polaris_updater::extract_version_token;

/// 内核信息卡的数据（只读）。
///
/// - `packagedVersion`：对**安装包里的内核文件**实跑一次 `version` 读到的版本 token；读不到即
///   空串，不回落清单版本（那会把「读不到」显示成「就是清单版本」）。它说的是那个文件，不是
///   此刻在跑的进程：经提权助手起核时执行的是助手目录里的副本，两者可能不同。
/// - `bundledVersion`：随包清单（编译期嵌入的 `core-manifest.json`）声明的内核版本。
/// - `patchSet`：桌面补丁集标识（清单 `sourceBuild.patchedSourceTree`）；移动端与清单不含时 `null`。
///
/// 读版本要起一个子进程，故放到阻塞线程上，不占主线程。
#[tauri::command]
pub async fn core_get_version_info(app: AppHandle) -> Result<ApiResponse<Value>, ()> {
    let read = tokio::task::spawn_blocking(move || {
        let state = app.state::<AppRuntime>();
        let u = state.updater();
        core_version_info(
            &u.read_core_version_line(),
            u.bundled_core_version(),
            u.bundled_core_patch_set(),
        )
    })
    .await;
    Ok(match read {
        Ok(info) => ApiResponse::ok(info),
        Err(e) => ApiResponse::err(format!("读取内核信息失败: {e}")),
    })
}

/// [`core_get_version_info`] 的载荷（纯函数；`version_line` 是原始版本行，读不到时为空串）。
pub(super) fn core_version_info(
    version_line: &str,
    bundled_version: &str,
    patch_set: Option<&str>,
) -> Value {
    json!({
        "packagedVersion": extract_version_token(version_line),
        "bundledVersion": bundled_version,
        "patchSet": patch_set,
    })
}

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
/// - `bundledVersion`：实际所选随包构建的内核版本，与默认 HTTP UA 同源；不可得时为 `unknown`。
/// - `patchSet`：桌面补丁集标识（清单 `sourceBuild.patchedSourceTree`）；移动端与清单不含时 `null`。
///
/// 读版本要起一个子进程，故放到阻塞线程上，不占主线程。
#[tauri::command]
pub async fn core_get_version_info(
    app: AppHandle,
    capabilities_only: Option<bool>,
) -> Result<ApiResponse<Value>, ()> {
    if capabilities_only.unwrap_or(false) {
        let capabilities = app
            .state::<AppRuntime>()
            .proxy
            .tls_fingerprint_capabilities()
            .await;
        return Ok(ApiResponse::ok(json!({"tlsFingerprints": capabilities})));
    }
    let read = tokio::task::spawn_blocking(move || {
        let state = app.state::<AppRuntime>();
        let u = state.updater();
        let mut info=core_version_info(
            &u.read_core_version_line(),
            u.bundled_core_version(),
            u.bundled_core_patch_set(),
        );
        // Admission diagnostics never run an unknown executable to ask its
        // version. Recovery links use the existing verified App reinstall flow.
        info["admission"]=if let Err(error)=polaris_core_supervisor::admitted_payload::check_selected_capability(crate::runtime::core_identity::ExecPurpose::CheckConfig) {
            json!({"state":"rejected","kind":format!("{:?}",error.kind),"recovery":"none"})
        } else {match crate::runtime::proxy::resolve_core_candidate() {
            Ok(path)=>match crate::runtime::core_identity::acquire_at(&path,crate::runtime::core_identity::ExecPurpose::CheckConfig) {
                Ok(payload)=>json!({"state":"admitted","coreId":payload.id().as_str(),"sourceCommit":payload.source_commit(),"policySha256":payload.policy_digest(),"recovery":"none"}),
                Err(error)=>json!({"state":"rejected","kind":format!("{:?}",error.kind),"recovery":match error.recovery {polaris_core_supervisor::admitted_payload::RecoveryAction::ReinstallCurrentApp=>"reinstallCurrentApp",_=>"none"}}),
            },
            Err(error)=> {
                let error=crate::runtime::core_identity::selection_error(error,crate::runtime::core_identity::ExecPurpose::CheckConfig);
                json!({"state":"rejected","kind":format!("{:?}",error.kind),"recovery":if error.kind.is_payload_damage(){"reinstallCurrentApp"}else{"none"}})
            },
        }};
        info
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

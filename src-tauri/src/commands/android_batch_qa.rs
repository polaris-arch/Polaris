//! One finite Debug bridge. Release/non-Android stubs create no worker or network resource.
use crate::response::ApiResponse;

#[tauri::command]
pub async fn debug_android_batch_qa(
    app: tauri::AppHandle,
    action: String,
    session_id: Option<String>,
    plan: Option<String>,
) -> ApiResponse<String> {
    #[cfg(all(target_os = "android", debug_assertions))]
    {
        if !matches!(
            action.as_str(),
            "prepare" | "arm" | "snapshot" | "probe" | "health" | "close" | "cleanupObserve"
        ) || session_id.as_ref().is_some_and(|s| s.len() > 64)
            || plan.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return ApiResponse::err("Batch request outside finite profile");
        }
        if action == "probe" {
            use tauri::Manager;
            let proxy = app.state::<crate::runtime::AppRuntime>().proxy.clone();
            let Some(session) = session_id else {
                return ApiResponse::err("Core probe session required");
            };
            return match proxy.debug_android_core_probe(session).await {
                Ok(report) => ApiResponse::ok(report),
                Err(error) => ApiResponse::err(error),
            };
        }
        match crate::runtime::proxy::android_bridge::debug_batch_qa(action, session_id, plan).await
        {
            Ok(report) => ApiResponse::ok(report),
            Err(error) => ApiResponse::err(error),
        }
    }
    #[cfg(not(all(target_os = "android", debug_assertions)))]
    {
        let _ = (app, action, session_id, plan);
        ApiResponse::err("Debug Android batch QA is disabled")
    }
}

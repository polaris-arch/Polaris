//! One finite Debug bridge. Release/non-Android stubs create no worker or network resource.
use crate::response::ApiResponse;

#[tauri::command]
pub async fn debug_android_batch_qa(
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
        match crate::runtime::proxy::android_bridge::debug_batch_qa(action, session_id, plan).await
        {
            Ok(report) => ApiResponse::ok(report),
            Err(error) => ApiResponse::err(error),
        }
    }
    #[cfg(not(all(target_os = "android", debug_assertions)))]
    {
        let _ = (action, session_id, plan);
        ApiResponse::err("Debug Android batch QA is disabled")
    }
}

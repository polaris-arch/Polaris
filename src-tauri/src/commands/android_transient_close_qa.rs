//! ACC-01 injection only arms a current Android engine. The process must restart to clear it.
use crate::response::ApiResponse;

#[tauri::command]
pub async fn debug_android_transient_close_targets() -> ApiResponse<serde_json::Value> {
    #[cfg(all(target_os = "android", debug_assertions))]
    {
        match crate::runtime::proxy::android_bridge::debug_transient_close_targets().await {
            Ok(targets) => ApiResponse::ok(targets),
            Err(error) => ApiResponse::err(error),
        }
    }
    #[cfg(not(all(target_os = "android", debug_assertions)))]
    {
        ApiResponse::err("Debug transient close timeout is disabled")
    }
}

#[tauri::command]
pub async fn debug_android_transient_close_timeout(
    target_kind: String,
    instance_id: String,
    target_token: String,
) -> ApiResponse<String> {
    #[cfg(all(target_os = "android", debug_assertions))]
    {
        if !matches!(target_kind.as_str(), "speedtest" | "login")
            || instance_id.is_empty()
            || instance_id.len() > 256
            || !instance_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            || target_token.len() != 16
            || !target_token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return ApiResponse::err("Invalid transient close timeout target");
        }
        match crate::runtime::proxy::android_bridge::debug_transient_close_timeout(
            target_kind,
            instance_id,
            target_token,
        )
        .await
        {
            Ok(state) => ApiResponse::ok(state),
            Err(error) => ApiResponse::err(error),
        }
    }
    #[cfg(not(all(target_os = "android", debug_assertions)))]
    {
        let _ = (target_kind, instance_id, target_token);
        ApiResponse::err("Debug transient close timeout is disabled")
    }
}

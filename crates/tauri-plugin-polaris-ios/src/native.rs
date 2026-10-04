use super::lifecycle_state::LifecycleState;
use serde::de::DeserializeOwned;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::plugin::PluginHandle;

tauri::ios_plugin_binding!(init_plugin_polaris_ios);
static PLUGIN: OnceLock<PluginHandle<tauri::Wry>> = OnceLock::new();
static STATE: OnceLock<Mutex<LifecycleState>> = OnceLock::new();

fn state() -> &'static Mutex<LifecycleState> {
    STATE.get_or_init(|| Mutex::new(LifecycleState::default()))
}

pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("polaris-ios")
        .setup(|_, api| {
            let plugin = api.register_ios_plugin(init_plugin_polaris_ios)?;
            PLUGIN
                .set(plugin)
                .map_err(|_| "iOS VPN plugin was registered twice")?;
            // This cache observes NE liveness, never ownership or Go disposal evidence.
            tauri::async_runtime::spawn(async {
                loop {
                    let _ = status().await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
            Ok(())
        })
        .build()
}

#[derive(serde::Deserialize)]
struct Directory {
    path: String,
}

pub fn shared_directory() -> Result<std::path::PathBuf, String> {
    let plugin = PLUGIN.get().ok_or("iOS VPN plugin is not initialized")?;
    let response: Directory = plugin
        .run_mobile_plugin("sharedDirectory", serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    Ok(response.path.into())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub status: i32,
    pub running: bool,
    pub active: bool,
    pub profile_exists: bool,
    pub ownership: String,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub config_digest: Option<String>,
    pub runtime_stopped: Option<bool>,
    pub cleanup_evidence: String,
    pub cleanup_error: Option<String>,
    pub last_error: Option<String>,
    pub lifecycle: Option<String>,
}

impl TunnelStatus {
    pub fn require_idle(&self) -> Result<(), String> {
        if self.active {
            return Err(
                "系统仍有 Polaris iOS VPN 会话；请先在系统设置中停止现有 VPN，再返回连接。".into(),
            );
        }
        if let Some(error) = self.cleanup_error.as_ref().or(self.last_error.as_ref()) {
            return Err(format!(
                "iOS VPN 上次操作未确认完成：{error}。请检查系统 VPN 状态。"
            ));
        }
        if self.profile_exists
            && (self.ownership != "ownedSessionReported"
                || self.runtime_stopped != Some(true)
                || self.lifecycle.as_deref() != Some("stopped"))
        {
            return Err(
                "已有 iOS VPN 配置尚无可靠的扩展停止报告；清理状态未知，请检查系统设置后再连接。"
                    .into(),
            );
        }
        Ok(())
    }
}

async fn call<T: DeserializeOwned + Send + 'static>(
    command: &'static str,
    payload: serde_json::Value,
    seconds: u64,
) -> Result<T, String> {
    let plugin = PLUGIN
        .get()
        .ok_or("iOS VPN plugin is not initialized")?
        .clone();
    // Keep the detached receiver alive after timeout for Tauri's native callback.
    // It never writes lifecycle state; only the caller may commit the receipt.
    let task = tauri::async_runtime::spawn(async move {
        plugin
            .run_mobile_plugin_async::<T>(command, payload)
            .await
            .map_err(|e| e.to_string())
    });
    match tokio::time::timeout(Duration::from_secs(seconds), task).await {
        Ok(result) => result.map_err(|e| e.to_string())?,
        Err(_) => Err(format!(
            "iOS VPN {command} timed out; check the system VPN status before retrying"
        )),
    }
}

pub async fn status() -> Result<TunnelStatus, String> {
    let generation = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .generation();
    let result = call::<TunnelStatus>("status", serde_json::json!({}), 12).await;
    state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .observe(
            generation,
            result.as_ref().ok().map(|snapshot| snapshot.running),
        );
    result
}

async fn lifecycle_call(command: &'static str, config: Option<&str>) -> Result<(), String> {
    let generation = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .begin()?;
    let _receipt = RequestReceipt(generation);
    let request_id = generation.to_string();
    let payload = match config {
        Some(config) => serde_json::json!({"configContent": config, "requestID": request_id}),
        None => serde_json::json!({"requestID": request_id}),
    };
    let result =
        call::<TunnelStatus>(command, payload, if command == "start" { 45 } else { 30 }).await;
    let running = result.as_ref().ok().map(|snapshot| snapshot.running);
    state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .finish(generation, running);
    result.map(|_| ())
}

// Dropping the caller's future leaves the native task alive for its callback,
// but releases only this generation's pending receipt as unknown.
struct RequestReceipt(u64);
impl Drop for RequestReceipt {
    fn drop(&mut self) {
        if let Ok(mut state) = state().lock() {
            state.finish(self.0, None);
        }
    }
}

pub async fn start(config: &str) -> Result<(), String> {
    lifecycle_call("start", Some(config)).await
}
pub async fn stop() -> Result<(), String> {
    lifecycle_call("stop", None).await
}
pub fn started() -> bool {
    state().lock().map(|state| state.running()).unwrap_or(false)
}

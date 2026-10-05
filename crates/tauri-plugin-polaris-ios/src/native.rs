use super::lifecycle_state::LifecycleState;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::plugin::mobile::PluginInvokeError;
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
    pub extension_generation: Option<u64>,
    pub uncertain_settings_generation: Option<u64>,
}

/// An ordinary action session receipt, produced only by this start adapter.
/// It grants neither Go disposal evidence nor TS state ownership.
#[derive(Clone, Debug)]
pub struct ReadySessionReceipt {
    native_generation: u64,
    shared_generation: u64,
    session_id: String,
    request_id: String,
    config_digest: String,
    extension_generation: u64,
}

impl ReadySessionReceipt {
    fn from_start(
        snapshot: &TunnelStatus,
        native_generation: u64,
        shared_generation: u64,
        request_id: &str,
    ) -> Result<Self, String> {
        snapshot.require_ready()?;
        if snapshot.request_id.as_deref() != Some(request_id) {
            return Err("ReadyUnknown: iOS start returned another request's receipt".into());
        }
        Ok(Self {
            native_generation,
            shared_generation,
            session_id: snapshot
                .session_id
                .clone()
                .ok_or("ReadyUnknown: missing session identity")?,
            request_id: request_id.to_owned(),
            config_digest: snapshot
                .config_digest
                .clone()
                .ok_or("ReadyUnknown: missing config digest")?,
            extension_generation: snapshot
                .extension_generation
                .ok_or("ReadyUnknown: missing extension generation")?,
        })
    }

    fn payload(&self, nonce: &str) -> serde_json::Value {
        serde_json::json!({
            "sessionID": self.session_id, "requestID": self.request_id,
            "configDigest": self.config_digest, "extensionGeneration": self.extension_generation,
            "observationNonce": nonce,
        })
    }
}

impl TunnelStatus {
    fn require_ready(&self) -> Result<(), String> {
        if self.status != 3
            || !self.running
            || !self.active
            || !self.profile_exists
            || self.ownership != "ownedSessionReported"
            || self.lifecycle.as_deref() != Some("running")
            || self.runtime_stopped != Some(false)
            || self
                .extension_generation
                .filter(|generation| *generation > 0)
                .is_none()
            || self.uncertain_settings_generation.is_some()
            || self.cleanup_error.is_some()
            || self.last_error.is_some()
            || self.session_id.as_deref().is_none_or(str::is_empty)
            || self.request_id.as_deref().is_none_or(str::is_empty)
            || self.config_digest.as_deref().is_none_or(str::is_empty)
        {
            return Err(
                "ReadyUnknown: iOS session has no exact successful live extension receipt".into(),
            );
        }
        Ok(())
    }

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

enum NativeCallFailure {
    Terminal(String),
    Unknown(String),
}

impl NativeCallFailure {
    fn message(self) -> String {
        match self {
            Self::Terminal(message) | Self::Unknown(message) => message,
        }
    }
}

async fn call<T: DeserializeOwned + Send + 'static>(
    command: &'static str,
    payload: serde_json::Value,
    seconds: u64,
    start_generation: Option<u64>,
) -> Result<T, NativeCallFailure> {
    let plugin = PLUGIN
        .get()
        .ok_or_else(|| NativeCallFailure::Unknown("iOS VPN plugin is not initialized".into()))?
        .clone();
    // Keep the detached receiver alive after timeout/drop for Tauri's callback.
    // A genuine terminal callback may release only its exact registration;
    // it cannot produce readiness or write another generation's running state.
    let task = tauri::async_runtime::spawn(async move {
        let result = plugin
            .run_mobile_plugin_async::<T>(command, payload)
            .await
            .map_err(|error| match error {
                PluginInvokeError::InvokeRejected(response) => {
                    NativeCallFailure::Terminal(response.message.unwrap_or_else(|| {
                        "StartupFailed: Native VPN rejected the request without a message".into()
                    }))
                }
                other => NativeCallFailure::Unknown(other.to_string()),
            });
        if result.is_ok() || matches!(&result, Err(NativeCallFailure::Terminal(_))) {
            if let Some(generation) = start_generation {
                if let Ok(mut state) = state().lock() {
                    state.native_terminal(generation);
                }
            }
        }
        result
    });
    match tokio::time::timeout(Duration::from_secs(seconds), task).await {
        Ok(result) => result.map_err(|error| NativeCallFailure::Unknown(error.to_string()))?,
        Err(_) => Err(NativeCallFailure::Unknown(format!(
            "iOS VPN {command} timed out; check the system VPN status before retrying"
        ))),
    }
}

pub async fn status() -> Result<TunnelStatus, String> {
    let generation = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .generation();
    let result = call::<TunnelStatus>("status", serde_json::json!({}), 12, None)
        .await
        .map_err(NativeCallFailure::message);
    state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .observe(
            generation,
            result.as_ref().ok().map(|snapshot| snapshot.running),
        );
    result
}

fn fresh_request_id() -> Result<String, String> {
    let mut nonce = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut nonce))
        .map_err(|error| format!("Cannot register iOS request identity: {error}"))?;
    Ok(nonce.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn lifecycle_call(
    command: &'static str,
    config: Option<&str>,
    shared_generation: Option<u64>,
) -> Result<(TunnelStatus, u64, String), String> {
    let request_id = fresh_request_id()?;
    let generation = {
        let mut state = state().lock().map_err(|_| "iOS VPN state lock poisoned")?;
        match shared_generation {
            Some(shared) => state.begin_start(shared, request_id.clone())?,
            None => state.begin()?,
        }
    };
    let _receipt = RequestReceipt(generation);
    let payload = match config {
        Some(config) => serde_json::json!({"configContent": config, "requestID": request_id}),
        None => serde_json::json!({"requestID": request_id}),
    };
    let result = match call::<TunnelStatus>(
        command,
        payload,
        if command == "start" { 45 } else { 30 },
        config.map(|_| generation),
    )
    .await
    {
        Err(NativeCallFailure::Unknown(message)) => {
            state()
                .lock()
                .map_err(|_| "iOS VPN state lock poisoned")?
                .abandon(generation);
            return Err(message);
        }
        result => result,
    };
    let running = result.as_ref().ok().map(|snapshot| snapshot.running);
    let committed = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .finish(generation, running);
    if !committed {
        return Err("StartCancelled: iOS lifecycle receipt was superseded or revoked".into());
    }
    result
        .map(|snapshot| (snapshot, generation, request_id))
        .map_err(NativeCallFailure::message)
}

// Dropping the caller's future leaves the native task alive for its callback,
// but abandons only this generation's observation as unknown. Its submitted
// start remains registered until native terminal or explicit user revocation.
struct RequestReceipt(u64);
impl Drop for RequestReceipt {
    fn drop(&mut self) {
        if let Ok(mut state) = state().lock() {
            state.abandon(self.0);
        }
    }
}

pub async fn start(config: &str, shared_generation: u64) -> Result<ReadySessionReceipt, String> {
    let (snapshot, generation, request_id) =
        lifecycle_call("start", Some(config), Some(shared_generation)).await?;
    let receipt =
        ReadySessionReceipt::from_start(&snapshot, generation, shared_generation, &request_id)?;
    if !state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .accepts_receipt(generation, shared_generation)
    {
        return Err("StartCancelled: iOS start was revoked before receipt delivery".into());
    }
    Ok(receipt)
}
pub async fn stop() -> Result<(), String> {
    lifecycle_call("stop", None, None).await.map(|_| ())
}

/// Called immediately after the normal user Stop claim, before the TS gate.
pub async fn revoke_pending_start_through(
    captured_previous_generation: u64,
    stop_generation: u64,
) -> Result<(), String> {
    if stop_generation <= captured_previous_generation {
        return Err("iOS Stop has an invalid generation binding".into());
    }
    let expected_request_id = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .revoke_start_through(captured_previous_generation);
    let Some(expected_request_id) = expected_request_id else {
        return Ok(());
    };
    // This is an intent acknowledgement, never cleanup proof. Swift also remembers
    // the nonce when this dispatch wins the race against the old start dispatch.
    let _: serde_json::Value = call(
        "revokePendingStart",
        serde_json::json!({
            "expectedStartRequestID": expected_request_id,
            "stopRequestID": format!("{stop_generation}:{}", fresh_request_id()?),
        }),
        12,
        None,
    )
    .await
    .map_err(NativeCallFailure::message)?;
    Ok(())
}

pub async fn observe_session(receipt: &ReadySessionReceipt) -> Result<(), String> {
    let accepts = || -> Result<bool, String> {
        Ok(state()
            .lock()
            .map_err(|_| "iOS VPN state lock poisoned")?
            .accepts_receipt(receipt.native_generation, receipt.shared_generation))
    };
    if !accepts()? {
        return Err("ReadyUnknown: iOS session receipt was superseded".into());
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Observation {
        observation_nonce: String,
        snapshot: TunnelStatus,
    }
    let nonce = fresh_request_id()?;
    let observed: Observation = call("observeSession", receipt.payload(&nonce), 12, None)
        .await
        .map_err(NativeCallFailure::message)?;
    observed.snapshot.require_ready()?;
    if !accepts()?
        || observed.observation_nonce != nonce
        || observed.snapshot.session_id.as_deref() != Some(receipt.session_id.as_str())
        || observed.snapshot.request_id.as_deref() != Some(receipt.request_id.as_str())
        || observed.snapshot.config_digest.as_deref() != Some(receipt.config_digest.as_str())
        || observed.snapshot.extension_generation != Some(receipt.extension_generation)
    {
        return Err("ReadyUnknown: iOS live observation does not match the ready session".into());
    }
    Ok(())
}
pub fn started() -> bool {
    state().lock().map(|state| state.running()).unwrap_or(false)
}

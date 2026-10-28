use super::lifecycle_state::LifecycleState;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
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

#[derive(Clone, Debug, serde::Deserialize)]
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
    pub tailscale_store_scope: Option<TailscaleStoreExport>,
    tailscale_store_retirement: Option<NativeScopedRetirement>,
}

pub use polaris_mesh::tailscale_state::retirement::{
    TailscaleProfileState, TailscaleStateFileState, TailscaleStoreExport,
    TailscaleStoreRetirementInstance, TailscaleStoreRetirementNode, TailscaleWriterState,
};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeScopedRetirement {
    contract_version: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    #[serde(rename = "startRequestID")]
    start_request_id: String,
    config_digest: String,
    source_extension_generation: u64,
    stop_extension_generation: u64,
    #[serde(rename = "stopRequestID")]
    stop_request_id: String,
    observation_nonce: String,
    global_cleanup_evidence: String,
    store_retirement: TailscaleStoreExport,
}

/// Issued only after the original Stop's fresh nonce-bound provider response.
/// This proves scoped StateStore writers, never global resource cleanup.
#[derive(Clone, Debug)]
pub struct TailscaleStoreRetirementReceipt(NativeScopedRetirement);
impl TailscaleStoreRetirementReceipt {
    pub fn session_id(&self) -> &str {
        &self.0.session_id
    }
    pub fn start_request_id(&self) -> &str {
        &self.0.start_request_id
    }
    pub fn config_digest(&self) -> &str {
        &self.0.config_digest
    }
    pub fn source_extension_generation(&self) -> u64 {
        self.0.source_extension_generation
    }
    pub fn stop_extension_generation(&self) -> u64 {
        self.0.stop_extension_generation
    }
    pub fn stop_request_id(&self) -> &str {
        &self.0.stop_request_id
    }
    pub fn observation_nonce(&self) -> &str {
        &self.0.observation_nonce
    }
    pub fn instances(&self) -> &[TailscaleStoreRetirementInstance] {
        self.0.store_retirement.instances()
    }
}

#[derive(Debug)]
pub struct StoppedSessionReceipt {
    snapshot: TunnelStatus,
    retirement: Option<TailscaleStoreRetirementReceipt>,
}
impl StoppedSessionReceipt {
    fn from_stop(snapshot: TunnelStatus, request_id: &str, nonce: &str) -> Self {
        let retirement = snapshot
            .tailscale_store_retirement
            .as_ref()
            .filter(|wire| {
                wire.contract_version == "polaris-ios-ts-store-retirement-v1"
                    && wire.global_cleanup_evidence == "CleanupUnknown"
                    && snapshot.cleanup_evidence == "CleanupUnknown"
                    && !snapshot.active
                    && !snapshot.running
                    && matches!(snapshot.status, 0 | 1)
                    && snapshot.runtime_stopped == Some(true)
                    && snapshot.lifecycle.as_deref() == Some("stopped")
                    && snapshot.ownership == "ownedSessionReported"
                    && snapshot.cleanup_error.is_none()
                    && snapshot.last_error.is_none()
                    && snapshot.uncertain_settings_generation.is_none()
                    && !wire.session_id.is_empty()
                    && !wire.start_request_id.is_empty()
                    && snapshot.session_id.as_deref() == Some(wire.session_id.as_str())
                    && snapshot.request_id.as_deref() == Some(wire.start_request_id.as_str())
                    && snapshot.config_digest.as_deref() == Some(wire.config_digest.as_str())
                    && snapshot.extension_generation == Some(wire.stop_extension_generation)
                    && wire.source_extension_generation > 0
                    && wire.source_extension_generation.checked_add(1)
                        == Some(wire.stop_extension_generation)
                    && wire.stop_request_id == request_id
                    && !request_id.is_empty()
                    && wire.observation_nonce == nonce
                    && !nonce.is_empty()
                    && wire
                        .store_retirement
                        .validate(&wire.config_digest, true)
                        .is_ok()
            })
            .cloned()
            .map(TailscaleStoreRetirementReceipt);
        Self {
            snapshot,
            retirement,
        }
    }
    pub fn snapshot(&self) -> &TunnelStatus {
        &self.snapshot
    }
    pub fn retirement(&self) -> Option<&TailscaleStoreRetirementReceipt> {
        self.retirement.as_ref()
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeNoStoreTerminal {
    contract_version: String,
    #[serde(rename = "requestID")]
    request_id: String,
    start_intent_finished: bool,
    submission_attempted: bool,
    store_construction: String,
}

struct NativeStartCell {
    owner: Arc<()>,
    native_generation: u64,
    shared_generation: u64,
    request_id: String,
    config: String,
    no_store: OnceLock<NativeNoStoreTerminal>,
    dispatched: std::sync::atomic::AtomicBool,
}

/// The one original dispatcher custody, retained before native submission.
#[derive(Clone)]
pub struct StartSessionCustody(Arc<NativeStartCell>);
impl std::fmt::Debug for StartSessionCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartSessionCustody")
            .field("request_id", &self.0.request_id)
            .field("native_generation", &self.0.native_generation)
            .field("shared_generation", &self.0.shared_generation)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct NativeNoStoreReceipt {
    owner: Arc<()>,
    request_id: String,
}
impl NativeNoStoreReceipt {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn belongs_to(&self, custody: &StartSessionCustody) -> bool {
        Arc::ptr_eq(&self.owner, &custody.0.owner) && self.request_id == custody.0.request_id
    }
}
impl StartSessionCustody {
    pub fn native_generation(&self) -> u64 {
        self.0.native_generation
    }
    pub fn shared_generation(&self) -> u64 {
        self.0.shared_generation
    }
    pub fn request_id(&self) -> &str {
        &self.0.request_id
    }
    fn retain_no_store(&self, terminal: &NativeNoStoreTerminal) {
        if terminal.contract_version == "polaris-ios-native-no-store-v1"
            && terminal.request_id == self.0.request_id
            && terminal.start_intent_finished
            && !terminal.submission_attempted
            && terminal.store_construction == "NoStoreConstruction"
        {
            let _ = self.0.no_store.set(terminal.clone());
        }
    }
    pub fn no_store_terminal(&self) -> Option<NativeNoStoreReceipt> {
        self.0.no_store.get().map(|_| NativeNoStoreReceipt {
            owner: self.0.owner.clone(),
            request_id: self.0.request_id.clone(),
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum NativeStartResponse {
    NoStore {
        #[serde(rename = "nativeNoStoreTerminal")]
        terminal: NativeNoStoreTerminal,
        #[serde(rename = "startError")]
        error: String,
    },
    Ready(TunnelStatus),
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
    original_instances: Vec<TailscaleStoreRetirementInstance>,
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
        let scope = snapshot
            .tailscale_store_scope
            .as_ref()
            .ok_or("ReadyUnknown: Original Go instance scope is absent")?;
        scope.validate(snapshot.config_digest.as_deref().unwrap_or(""), false)?;
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
            original_instances: scope.instances().to_vec(),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }
    pub fn extension_generation(&self) -> u64 {
        self.extension_generation
    }
    pub fn original_instances(&self) -> &[TailscaleStoreRetirementInstance] {
        &self.original_instances
    }

    fn payload(&self, nonce: &str) -> serde_json::Value {
        serde_json::json!({
            "sessionID": self.session_id, "requestID": self.request_id,
            "configDigest": self.config_digest, "extensionGeneration": self.extension_generation,
            "observationNonce": nonce,
        })
    }
}

/// Actual current manager observation after a cold host boot; no local birth token.
#[derive(Clone, Debug)]
pub struct ObservedSessionReceipt {
    session_id: String,
    request_id: String,
    config_digest: String,
    extension_generation: u64,
    original_instances: Vec<TailscaleStoreRetirementInstance>,
}
impl ObservedSessionReceipt {
    fn from_live(snapshot: &TunnelStatus) -> Result<Self, String> {
        snapshot.require_ready()?;
        let scope = snapshot
            .tailscale_store_scope
            .as_ref()
            .ok_or("ReadyUnknown: Current original Go scope is absent")?;
        scope.validate(snapshot.config_digest.as_deref().unwrap_or(""), false)?;
        Ok(Self {
            session_id: snapshot
                .session_id
                .clone()
                .ok_or("ReadyUnknown: Missing current session")?,
            request_id: snapshot
                .request_id
                .clone()
                .ok_or("ReadyUnknown: Missing current request")?,
            config_digest: snapshot
                .config_digest
                .clone()
                .ok_or("ReadyUnknown: Missing current config")?,
            extension_generation: snapshot
                .extension_generation
                .ok_or("ReadyUnknown: Missing current generation")?,
            original_instances: scope.instances().to_vec(),
        })
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }
    pub fn extension_generation(&self) -> u64 {
        self.extension_generation
    }
    pub fn original_instances(&self) -> &[TailscaleStoreRetirementInstance] {
        &self.original_instances
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
    response_observer: Option<Box<dyn FnOnce(&T) + Send>>,
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
        if let (Ok(response), Some(observer)) = (&result, response_observer) {
            observer(response); // Late callbacks harvest only their captured original custody.
        }
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
    let result = call::<TunnelStatus>("status", serde_json::json!({}), 12, None, None)
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

async fn stop_call(expected: serde_json::Value) -> Result<StoppedSessionReceipt, String> {
    let request_id = fresh_request_id()?;
    let nonce = fresh_request_id()?;
    let generation = {
        let mut state = state().lock().map_err(|_| "iOS VPN state lock poisoned")?;
        state.begin()?
    };
    let _receipt = RequestReceipt(generation);
    let mut payload = expected;
    payload["requestID"] = serde_json::json!(request_id);
    payload["observationNonce"] = serde_json::json!(nonce);
    let result = match call::<TunnelStatus>("stop", payload, 30, None, None).await {
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
        .map(|snapshot| StoppedSessionReceipt::from_stop(snapshot, &request_id, &nonce))
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

pub fn prepare_start(config: &str, shared_generation: u64) -> Result<StartSessionCustody, String> {
    if config.is_empty() {
        return Err("StartupFailed: iOS configuration is empty".into());
    }
    let request_id = fresh_request_id()?;
    let native_generation = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .begin_start(shared_generation, request_id.clone())?;
    Ok(StartSessionCustody(Arc::new(NativeStartCell {
        owner: Arc::new(()),
        native_generation,
        shared_generation,
        request_id,
        config: config.to_owned(),
        no_store: OnceLock::new(),
        dispatched: std::sync::atomic::AtomicBool::new(false),
    })))
}

pub async fn start_prepared(custody: &StartSessionCustody) -> Result<ReadySessionReceipt, String> {
    let generation = custody.native_generation();
    if state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .generation()
        != generation
        || custody
            .0
            .dispatched
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return Err(
            "StartCancelled: Original iOS start dispatch was revoked or already submitted".into(),
        );
    }
    let _receipt = RequestReceipt(generation);
    let original = custody.clone();
    let result = call::<NativeStartResponse>(
        "start",
        serde_json::json!({
            "configContent": custody.0.config, "requestID": custody.request_id(),
        }),
        45,
        Some(generation),
        Some(Box::new(move |response| {
            if let NativeStartResponse::NoStore { terminal, .. } = response {
                original.retain_no_store(terminal);
            }
        })),
    )
    .await;
    if matches!(&result, Err(NativeCallFailure::Unknown(_))) {
        state()
            .lock()
            .map_err(|_| "iOS VPN state lock poisoned")?
            .abandon(generation);
        return Err(result
            .err()
            .ok_or("ReadyUnknown: Missing original start result")?
            .message());
    }
    let running = match &result {
        Ok(NativeStartResponse::Ready(snapshot)) => Some(snapshot.running),
        _ => None,
    };
    if !state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .finish(generation, running)
    {
        return Err("StartCancelled: iOS lifecycle receipt was superseded or revoked".into());
    }
    let snapshot = match result.map_err(NativeCallFailure::message)? {
        NativeStartResponse::Ready(snapshot) => snapshot,
        NativeStartResponse::NoStore { error, .. } => return Err(error),
    };
    let receipt = ReadySessionReceipt::from_start(
        &snapshot,
        generation,
        custody.shared_generation(),
        custody.request_id(),
    )?;
    if !state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .accepts_receipt(generation, custody.shared_generation())
    {
        return Err("StartCancelled: iOS start was revoked before receipt delivery".into());
    }
    Ok(receipt)
}
pub async fn start(config: &str, shared_generation: u64) -> Result<ReadySessionReceipt, String> {
    let custody = prepare_start(config, shared_generation)?;
    start_prepared(&custody).await
}
pub async fn stop() -> Result<StoppedSessionReceipt, String> {
    stop_call(serde_json::json!({})).await
}
pub async fn stop_session(receipt: &ReadySessionReceipt) -> Result<StoppedSessionReceipt, String> {
    stop_call(
        serde_json::json!({"expectedStartRequestID": receipt.request_id,
        "expectedSessionID": receipt.session_id, "expectedConfigDigest": receipt.config_digest,
        "expectedExtensionGeneration": receipt.extension_generation}),
    )
    .await
}
pub async fn stop_start(custody: &StartSessionCustody) -> Result<StoppedSessionReceipt, String> {
    stop_call(serde_json::json!({"expectedStartRequestID": custody.request_id()})).await
}
pub async fn stop_observed(
    receipt: &ObservedSessionReceipt,
) -> Result<StoppedSessionReceipt, String> {
    stop_call(
        serde_json::json!({"expectedStartRequestID": receipt.request_id,
        "expectedSessionID": receipt.session_id, "expectedConfigDigest": receipt.config_digest,
        "expectedExtensionGeneration": receipt.extension_generation}),
    )
    .await
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
    let observed: Observation = call("observeSession", receipt.payload(&nonce), 12, None, None)
        .await
        .map_err(NativeCallFailure::message)?;
    observed.snapshot.require_ready()?;
    if !accepts()?
        || observed.observation_nonce != nonce
        || observed.snapshot.session_id.as_deref() != Some(receipt.session_id.as_str())
        || observed.snapshot.request_id.as_deref() != Some(receipt.request_id.as_str())
        || observed.snapshot.config_digest.as_deref() != Some(receipt.config_digest.as_str())
        || observed.snapshot.extension_generation != Some(receipt.extension_generation)
        || observed
            .snapshot
            .tailscale_store_scope
            .as_ref()
            .is_none_or(|scope| {
                scope.validate(&receipt.config_digest, false).is_err()
                    || scope.instances() != receipt.original_instances.as_slice()
            })
    {
        return Err("ReadyUnknown: iOS live observation does not match the ready session".into());
    }
    Ok(())
}
pub async fn observe_current_session() -> Result<ObservedSessionReceipt, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Observation {
        observation_nonce: String,
        snapshot: TunnelStatus,
    }
    let generation = state()
        .lock()
        .map_err(|_| "iOS VPN state lock poisoned")?
        .generation();
    let nonce = fresh_request_id()?;
    let observed: Observation = call(
        "observeCurrentSession",
        serde_json::json!({"observationNonce": nonce}),
        12,
        None,
        None,
    )
    .await
    .map_err(NativeCallFailure::message)?;
    if observed.observation_nonce != nonce
        || state()
            .lock()
            .map_err(|_| "iOS VPN state lock poisoned")?
            .generation()
            != generation
    {
        return Err("ReadyUnknown: Current native observation was superseded".into());
    }
    ObservedSessionReceipt::from_live(&observed.snapshot)
}
pub fn started() -> bool {
    state().lock().map(|state| state.running()).unwrap_or(false)
}

#[cfg(test)]
mod retirement_tests;

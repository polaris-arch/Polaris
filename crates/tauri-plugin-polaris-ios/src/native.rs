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
    tailscale_directory_retirement: Option<NativeDirectoryRetirement>,
}

pub use polaris_mesh::tailscale_state::retirement::{
    TailscaleProfileState, TailscaleStateFileState, TailscaleStoreExport,
    TailscaleStoreRetirementInstance, TailscaleStoreRetirementNode, TailscaleWriterState,
};

pub use polaris_mesh::tailscale_state::directory_retirement::{
    TailscaleDirectoryExport, TailscaleDirectoryInstance,
};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeDirectoryRetirement {
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
    original_scope: TailscaleDirectoryExport,
    directory_retirement: TailscaleDirectoryExport,
}

/// Original native Stop custody, separate from authentication projection.
/// This is not a deletion permit: consumers must still bind the durable intent,
/// acquire the exclusive incarnation lease and recheck the directory object.
#[derive(Clone, Debug)]
pub struct TailscaleDirectoryRetirementReceipt(NativeDirectoryRetirement);
impl TailscaleDirectoryRetirementReceipt {
    pub fn instances(&self) -> &[TailscaleDirectoryInstance] {
        self.0.directory_retirement.instances()
    }
    pub fn session_id(&self) -> &str { &self.0.session_id }
    pub fn start_request_id(&self) -> &str { &self.0.start_request_id }
    pub fn config_digest(&self) -> &str { &self.0.config_digest }
    pub fn source_extension_generation(&self) -> u64 { self.0.source_extension_generation }
    pub fn stop_extension_generation(&self) -> u64 { self.0.stop_extension_generation }
    pub fn stop_request_id(&self) -> &str { &self.0.stop_request_id }
    pub fn observation_nonce(&self) -> &str { &self.0.observation_nonce }
}

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
    directory_retirement: Option<TailscaleDirectoryRetirementReceipt>,
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
                        .validate_history(&wire.config_digest, true)
                        .is_ok()
            })
            .cloned()
            .map(TailscaleStoreRetirementReceipt);
        let directory_retirement = snapshot
            .tailscale_directory_retirement
            .as_ref()
            .filter(|wire| {
                wire.contract_version == "polaris-ios-ts-directory-retirement-v1"
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
                    && wire.directory_retirement.instances().iter().any(|run| !run.nodes.is_empty())
                    && wire.original_scope.validate(&wire.config_digest, false).is_ok()
                    && wire.original_scope.observed_runs().is_ok_and(|originals| {
                        wire.directory_retirement.validate_observed_runs(&originals, true).is_ok()
                    })
            })
            .cloned()
            .map(TailscaleDirectoryRetirementReceipt);
        Self {
            snapshot,
            retirement,
            directory_retirement,
        }
    }
    pub fn snapshot(&self) -> &TunnelStatus {
        &self.snapshot
    }
    pub fn directory_retirement(&self) -> Option<&TailscaleDirectoryRetirementReceipt> {
        self.directory_retirement.as_ref()
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

// Shared validation does not construct either kind of native custody.
fn validate_reload_scope(session_id: &str, request_id: &str, config_digest: &str,
    extension_generation: u64, original_instances: &[TailscaleStoreRetirementInstance],
    snapshot: &TunnelStatus) -> Result<(u64, Vec<TailscaleStoreRetirementInstance>), String> {
        snapshot.require_ready()?;
        let mismatch = || "ReadyUnknown: Reload changed the original native owner or run census".to_owned();
        if snapshot.session_id.as_deref() != Some(session_id)
            || snapshot.request_id.as_deref() != Some(request_id)
            || snapshot.config_digest.as_deref() != Some(config_digest) {
            return Err(mismatch());
        }
        let generation = snapshot.extension_generation.ok_or_else(mismatch)?;
        let advance = generation.checked_sub(extension_generation).ok_or_else(mismatch)?;
        let scope = snapshot.tailscale_store_scope.as_ref().ok_or_else(mismatch)?;
        scope.validate_history(config_digest, false)?;
        let current = scope.instances();
        if advance == 0 {
            if current != original_instances { return Err(mismatch()); }
            return Ok((generation, current.to_vec()));
        }
        let added = current.len().checked_sub(original_instances.len()).ok_or_else(mismatch)?;
        if original_instances.is_empty() || added == 0 || u64::try_from(added).ok() != Some(advance) {
            return Err(mismatch());
        }
        let originals: std::collections::BTreeMap<_, _> = original_instances.iter()
            .map(|run| (run.run_nonce.as_str(), run)).collect();
        if originals.len() != original_instances.len() { return Err(mismatch()); }
        for original in original_instances {
            let run = current.iter().find(|run| run.run_nonce == original.run_nonce).ok_or_else(mismatch)?;
            if run.config_digest != original.config_digest || run.nodes.len() != original.nodes.len() {
                return Err(mismatch());
            }
            // Mutable terminal/file revisions are not readiness custody. Compare
            // the immutable scope metadata; actual Stop still validates retirement.
            let scopes = |nodes: &[TailscaleStoreRetirementNode]| -> std::collections::BTreeSet<(String, String, String)> {
                nodes.iter().map(|node| (node.tag.clone(), node.state_directory.clone(), node.state_file.clone())).collect()
            };
            if scopes(&run.nodes) != scopes(&original.nodes) { return Err(mismatch()); }
        }
        if current.iter().filter(|run| !originals.contains_key(run.run_nonce.as_str()))
            .any(|run| run.config_digest != config_digest) { return Err(mismatch()); }
        Ok((generation, current.to_vec()))
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
        scope.validate_history(snapshot.config_digest.as_deref().unwrap_or(""), false)?;
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

    fn from_reload_observation(&self, snapshot: &TunnelStatus) -> Result<Self, String> {
        let (generation, instances) = validate_reload_scope(&self.session_id, &self.request_id,
            &self.config_digest, self.extension_generation, &self.original_instances, snapshot)?;
        let mut updated = self.clone();
        updated.extension_generation = generation;
        updated.original_instances = instances;
        Ok(updated)
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
        scope.validate_history(snapshot.config_digest.as_deref().unwrap_or(""), false)?;
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
    fn from_reload_observation(&self, snapshot: &TunnelStatus) -> Result<Self, String> {
        let (generation, instances) = validate_reload_scope(&self.session_id, &self.request_id,
            &self.config_digest, self.extension_generation, &self.original_instances, snapshot)?;
        let mut updated = self.clone();
        updated.extension_generation = generation;
        updated.original_instances = instances;
        Ok(updated)
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceConditions {
    /// Required: old/malformed native replies fail closed.
    pub app_active: bool,
    pub metered: Option<bool>,
    pub power_save: bool,
}

/// Read only App conditions; does not observe or mutate the NE custody cache.
pub async fn device_conditions() -> Result<DeviceConditions, String> {
    call("deviceConditions", serde_json::json!({}), 3, None, None)
        .await
        .map_err(NativeCallFailure::message)
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
    stop_call_bound(expected, None).await
}

fn begin_bound_stop(state: &mut LifecycleState,
    expected_host_generation: Option<&std::sync::atomic::AtomicU64>) -> Result<u64, String> {
    if expected_host_generation.is_some_and(|expected| !state.accepts_stop_target(expected.load(std::sync::atomic::Ordering::SeqCst))) {
        return Err("CleanupUnknown: Stop target was superseded before dispatch".into());
    }
    let generation = state.begin()?;
    // Retry custody advances atomically with this dispatch, never with Ready.
    // All clones share it; successor dispatch still invalidates this generation.
    if let Some(target) = expected_host_generation {
        target.store(generation, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(generation)
}

async fn stop_call_bound(expected: serde_json::Value, expected_host_generation: Option<&std::sync::atomic::AtomicU64>) -> Result<StoppedSessionReceipt, String> {
    let request_id = fresh_request_id()?;
    let nonce = fresh_request_id()?;
    let generation = {
        let mut state = state().lock().map_err(|_| "iOS VPN state lock poisoned")?;
        begin_bound_stop(&mut state, expected_host_generation)?
    };
    let _receipt = RequestReceipt(generation);
    let allow_cleanup = expected["stopOnly"] != true;
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
        .map(|snapshot| if allow_cleanup { StoppedSessionReceipt::from_stop(snapshot, &request_id, &nonce) }
            else { StoppedSessionReceipt { snapshot, retirement: None, directory_retirement: None } })
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
    start_prepared_mode(custody, false).await
}

fn expected_directory_source_identity() -> Result<String, String> {
    let source: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/mobile-core/source-contract.json"))
        .map_err(|_| "StartupFailed: Bundled mobile producer contract is invalid")?;
    directory_source_identity(&source)
}

fn directory_source_identity(source: &serde_json::Value) -> Result<String, String> {
    let commit = source["sourceCommit"].as_str().unwrap_or("");
    if source["schema"] != "polaris-mobile-fork-input-v1"
        || source["repository"] != "polaris-arch/sing-box-polaris"
        || source["repositoryId"].as_u64() != Some(1421657509)
        || source["sourceURL"] != "https://github.com/polaris-arch/sing-box-polaris"
        || source["sourceOverlay"] != 0
        || source["patches"].as_array().is_none_or(|patches| !patches.is_empty())
        || commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("StartupFailed: Bundled mobile producer identity is unsupported".into());
    }
    Ok(commit.to_owned())
}

/// The selected App-only bootstrap protocol never sends managed physical paths
/// or configuration bytes to an unverified provider instance.
pub async fn start_managed_prepared(custody: &StartSessionCustody) -> Result<ReadySessionReceipt, String> {
    start_prepared_mode(custody, true).await
}

async fn start_prepared_mode(custody: &StartSessionCustody, managed: bool) -> Result<ReadySessionReceipt, String> {
    let expected_source = if managed { Some(expected_directory_source_identity()?) } else { None };
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
            "managedBootstrap": managed, "expectedDirectorySourceIdentity": expected_source,
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

/// Stop-only custody. There is deliberately no conversion to Ready or Observed
/// readiness; revoked original starts cannot be revived through this capability.
#[derive(Clone, Debug)]
pub struct StopSessionTarget {
    host_generation: Arc<std::sync::atomic::AtomicU64>,
    cleanup_proof_eligible: Arc<std::sync::atomic::AtomicBool>,
    session_id: String,
    request_id: String,
    config_digest: String,
    extension_generation: u64,
    original_instances: Vec<TailscaleStoreRetirementInstance>,
}
impl StopSessionTarget {
    pub fn cleanup_proof_eligible(&self) -> bool { self.cleanup_proof_eligible.load(std::sync::atomic::Ordering::SeqCst) }
    pub fn without_cleanup_proof(&self) -> Self {
        self.cleanup_proof_eligible.store(false, std::sync::atomic::Ordering::SeqCst);
        self.clone()
    }
    pub fn request_id(&self) -> &str { &self.request_id }
    pub fn session_id(&self) -> &str { &self.session_id }
    pub fn config_digest(&self) -> &str { &self.config_digest }
    pub fn extension_generation(&self) -> u64 { self.extension_generation }
    pub fn original_instances(&self) -> &[TailscaleStoreRetirementInstance] { &self.original_instances }
}

fn refine_stop_scope(session_id: &str, request_id: &str, config_digest: &str,
    extension_generation: u64, original_instances: &[TailscaleStoreRetirementInstance],
    nonce: &str, observed: Option<(&str, &TunnelStatus)>) -> Option<(u64, Vec<TailscaleStoreRetirementInstance>)> {
    let (returned_nonce, snapshot) = observed?;
    if returned_nonce != nonce { return None; }
    validate_reload_scope(session_id, request_id, config_digest,
        extension_generation, original_instances, snapshot).ok()
}

async fn prepare_stop_target(session_id: &str, request_id: &str, config_digest: &str,
    extension_generation: u64, original_instances: &[TailscaleStoreRetirementInstance],
    required_host_generation: Option<u64>) -> Result<StopSessionTarget, String> {
    let host_generation = {
        let state = state().lock().map_err(|_| "iOS VPN state lock poisoned")?;
        let generation = required_host_generation.unwrap_or_else(|| state.generation());
        if !state.accepts_stop_target(generation) {
            return Err("CleanupUnknown: Original stop owner was replaced or has an active dispatch".into());
        }
        generation
    };
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Observation { observation_nonce: String, snapshot: TunnelStatus }
    let nonce = fresh_request_id()?;
    let observed: Result<Observation, NativeCallFailure> = call("observeReloadSession", serde_json::json!({
        "sessionID": session_id, "requestID": request_id, "configDigest": config_digest,
        "extensionGeneration": extension_generation, "observationNonce": nonce,
    }), 12, None, None).await;
    if !state().lock().map_err(|_| "iOS VPN state lock poisoned")?
        .accepts_stop_target(host_generation) {
        return Err("CleanupUnknown: Original stop observation was superseded".into());
    }
    // Failed observation is not failed Stop authority. Only exact positive
    // refinement may carry cleanup evidence; the original identity remains
    // addressable during reloadAccepted, reasserting, or an unresponsive worker.
    let refined = refine_stop_scope(session_id, request_id, config_digest, extension_generation,
        original_instances, &nonce, observed.as_ref().ok().map(|value| (value.observation_nonce.as_str(), &value.snapshot)));
    let eligible = refined.is_some();
    let (generation, instances) = refined.unwrap_or_else(|| (extension_generation, original_instances.to_vec()));
    Ok(StopSessionTarget { host_generation: Arc::new(std::sync::atomic::AtomicU64::new(host_generation)),
        cleanup_proof_eligible: Arc::new(std::sync::atomic::AtomicBool::new(eligible)), session_id: session_id.to_owned(), request_id: request_id.to_owned(),
        config_digest: config_digest.to_owned(), extension_generation: generation, original_instances: instances })
}

pub async fn prepare_ready_stop(receipt: &ReadySessionReceipt) -> Result<StopSessionTarget, String> {
    prepare_stop_target(&receipt.session_id, &receipt.request_id, &receipt.config_digest,
        receipt.extension_generation, &receipt.original_instances, Some(receipt.native_generation)).await
}
pub async fn prepare_observed_stop(receipt: &ObservedSessionReceipt) -> Result<StopSessionTarget, String> {
    prepare_stop_target(&receipt.session_id, &receipt.request_id, &receipt.config_digest,
        receipt.extension_generation, &receipt.original_instances, None).await
}
pub async fn stop_target(target: &StopSessionTarget) -> Result<StoppedSessionReceipt, String> {
    let mut expected = serde_json::json!({"expectedStartRequestID": target.request_id,
        "expectedSessionID": target.session_id, "expectedConfigDigest": target.config_digest});
    let eligible = target.cleanup_proof_eligible.swap(false, std::sync::atomic::Ordering::SeqCst);
    expected["stopOnly"] = serde_json::json!(!eligible);
    if eligible {
        expected["expectedExtensionGeneration"] = serde_json::json!(target.extension_generation);
    }
    let mut stopped = stop_call_bound(expected, Some(target.host_generation.as_ref())).await?;
    if !eligible {
        // Even a coincidental positive wire response cannot upgrade this
        // explicitly unrefined Stop into cleanup authority.
        stopped.retirement = None;
        stopped.directory_retirement = None;
    } else {
        target.cleanup_proof_eligible.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(stopped)
}

/// Refresh the existing same-process original owner after a provider reload.
/// This does not create custody from a cold-host observation or weaken the
/// generation checks used by ordinary observe_session / stop_session.
pub async fn refresh_ready_session(receipt: &ReadySessionReceipt) -> Result<ReadySessionReceipt, String> {
    let accepts = || -> Result<bool, String> {
        Ok(state().lock().map_err(|_| "iOS VPN state lock poisoned")?
            .accepts_receipt(receipt.native_generation, receipt.shared_generation))
    };
    if !accepts()? { return Err("ReadyUnknown: Original reload custody was superseded".into()); }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Observation { observation_nonce: String, snapshot: TunnelStatus }
    let nonce = fresh_request_id()?;
    let observed: Observation = call("observeReloadSession", receipt.payload(&nonce), 12, None, None)
        .await.map_err(NativeCallFailure::message)?;
    if !accepts()? || observed.observation_nonce != nonce {
        return Err("ReadyUnknown: Reload observation nonce or original custody changed".into());
    }
    receipt.from_reload_observation(&observed.snapshot)
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
                scope.validate_history(&receipt.config_digest, false).is_err()
                    || scope.instances() != receipt.original_instances.as_slice()
            })
    {
        return Err("ReadyUnknown: iOS live observation does not match the ready session".into());
    }
    Ok(())
}
/// Refresh a previously observed original manager without manufacturing local
/// start custody. Its same owner and complete original run history stay bound.
pub async fn refresh_observed_session(receipt: &ObservedSessionReceipt) -> Result<ObservedSessionReceipt, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Observation { observation_nonce: String, snapshot: TunnelStatus }
    let generation = state().lock().map_err(|_| "iOS VPN state lock poisoned")?.generation();
    let nonce = fresh_request_id()?;
    let observed: Observation = call("observeReloadSession", serde_json::json!({
        "sessionID": receipt.session_id, "requestID": receipt.request_id,
        "configDigest": receipt.config_digest, "extensionGeneration": receipt.extension_generation,
        "observationNonce": nonce,
    }), 12, None, None).await.map_err(NativeCallFailure::message)?;
    if observed.observation_nonce != nonce
        || state().lock().map_err(|_| "iOS VPN state lock poisoned")?.generation() != generation {
        return Err("ReadyUnknown: Observed reload custody was superseded".into());
    }
    receipt.from_reload_observation(&observed.snapshot)
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

#[cfg(test)]
mod directory_retirement_tests;

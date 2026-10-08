//! Normal-main prerequisites use the existing lifecycle producer. Tickets are ordinary
//! operation authority only: they never prove disposal or permit TS state mutation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_helper_proto::Platform;
use serde_json::Value;
use tokio::sync::watch;

use super::mesh_apply::owner_proof::ProducerCell;
use super::{lifecycle::StartLeg, LocalHttpProxy, ProxyRuntime, ProxyStatus, StartError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NormalMainAction {
    TailscaleLogin,
    ManualSpeedTest,
    ManualNetworkCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionRequirement {
    IndependentExistingPath,
    NormalMainRequired,
}

impl NormalMainAction {
    pub(crate) fn requirement(self, platform: Platform) -> ActionRequirement {
        match self {
            Self::ManualNetworkCheck => ActionRequirement::NormalMainRequired,
            Self::TailscaleLogin | Self::ManualSpeedTest if platform == Platform::Ios => {
                ActionRequirement::NormalMainRequired
            }
            Self::TailscaleLogin | Self::ManualSpeedTest => {
                ActionRequirement::IndependentExistingPath
            }
        }
    }
}

/// Frozen by an existing action request, never deserialized from renderer input.
#[derive(Clone)]
pub(crate) struct ActionBinding {
    action: NormalMainAction,
    request_id: String,
    saved_digest: String,
    target_ids: Vec<String>,
    identity_epoch: Option<String>,
    expected_generation: Option<u64>,
    attempt: Option<Arc<crate::runtime::tailscale_login_core::Attempt>>,
}

impl ActionBinding {
    pub(crate) fn new(
        action: NormalMainAction,
        request_id: String,
        saved: &Value,
        target_ids: Vec<String>,
        identity_epoch: Option<String>,
    ) -> Result<Self, MainPrerequisiteError> {
        if request_id.trim().is_empty() || identity_epoch.as_deref() == Some("") {
            return Err(MainPrerequisiteError::ReadyUnknown);
        }
        let mut unique = BTreeSet::new();
        for id in &target_ids {
            if id.is_empty()
                || !unique.insert(id)
                || !saved
                    .get("servers")
                    .and_then(Value::as_array)
                    .is_some_and(|servers| {
                        servers
                            .iter()
                            .any(|server| server.get("id").and_then(Value::as_str) == Some(id))
                    })
            {
                return Err(MainPrerequisiteError::TargetMissing);
            }
        }
        Ok(Self {
            action,
            request_id,
            saved_digest: digest(saved)?,
            target_ids,
            identity_epoch,
            expected_generation: None,
            attempt: None,
        })
    }

    pub(crate) fn for_attempt(
        mut self,
        expected_generation: u64,
        attempt: Arc<crate::runtime::tailscale_login_core::Attempt>,
    ) -> Self {
        self.expected_generation = Some(expected_generation);
        self.attempt = Some(attempt);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MainPrerequisiteError {
    Failed(StartError),
    Superseded,
    ConfigurationChanged,
    TargetMissing,
    ReadyUnknown,
    UnsavedConfiguration,
}

impl MainPrerequisiteError {
    pub(crate) fn code(&self) -> &str {
        match self {
            Self::Failed(error) => error.code.unwrap_or(super::code::STARTUP_FAILED),
            Self::Superseded => "superseded",
            Self::ConfigurationChanged => "configurationPending",
            Self::TargetMissing => "targetNotInMain",
            Self::ReadyUnknown => "readyUnknown",
            Self::UnsavedConfiguration => "unsavedConfiguration",
        }
    }
}

impl std::fmt::Display for MainPrerequisiteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => std::fmt::Display::fmt(error, formatter),
            _ => formatter.write_str(self.code()),
        }
    }
}

pub(super) struct ReadyMainCore {
    generation: u64,
    saved_digest: String,
    emission_digest: String,
    targets: BTreeMap<String, String>,
    status: ProxyStatus,
    api_secret: String,
    probe_ports: Vec<u16>,
    local_http_proxy: Option<LocalHttpProxy>,
    committed: AtomicBool,
    #[cfg(target_os = "ios")]
    ios_receipt: tauri_plugin_polaris_ios::ReadySessionReceipt,
    #[cfg(target_os = "android")]
    raw_config_digest: Option<String>,
}

/// Producer-only fields; bools, events and cached status cannot mint this value.
#[derive(Clone)]
pub(crate) struct ReadyMainTicket {
    core: Arc<ReadyMainCore>,
    binding: ActionBinding,
}

impl ReadyMainTicket {
    pub(crate) fn generation(&self) -> u64 {
        self.core.generation
    }
    pub(crate) fn api_port(&self) -> u16 {
        self.core.status.clash_api_port
    }
    pub(crate) fn api_secret(&self) -> &str {
        &self.core.api_secret
    }
    pub(crate) fn start_time(&self) -> Option<u64> {
        self.core.status.start_time
    }
    pub(crate) fn target_tag(&self, id: &str) -> Option<&str> {
        self.binding
            .target_ids
            .iter()
            .any(|target| target == id)
            .then(|| self.core.targets.get(id).map(String::as_str))
            .flatten()
    }
    pub(crate) fn request_id(&self) -> &str {
        &self.binding.request_id
    }
    pub(crate) fn identity_epoch(&self) -> Option<&str> {
        self.binding.identity_epoch.as_deref()
    }
    pub(crate) fn probe_ports(&self) -> &[u16] {
        &self.core.probe_ports
    }
    pub(crate) fn local_http_proxy(&self) -> Option<LocalHttpProxy> {
        self.core.local_http_proxy.clone()
    }
}

#[derive(Clone)]
pub(super) enum NormalStartCompletion {
    Pending(u64),
    Starting(u64),
    Finished(Result<ProxyStatus, StartError>, Option<u64>),
    Superseded,
}

#[derive(Clone)]
pub(super) struct NormalStart {
    digest: String,
    completion: watch::Sender<NormalStartCompletion>,
    identity: Arc<ProducerCell>,
    attempt: Option<Arc<crate::runtime::tailscale_login_core::Attempt>>,
}

/// Current observer and retained producer responsibilities share the original short lock.
#[derive(Default)]
pub(super) struct NormalStarts {
    current: Option<NormalStart>,
    retained: Vec<NormalStart>,
    pub(super) pause: Option<Arc<super::mesh_apply::pc_owner_census::PcPauseCustody>>,
}

impl NormalStarts {
    pub(super) fn as_ref(&self) -> Option<&NormalStart> {
        self.current.as_ref()
    }

    pub(super) fn assert_unpaused(&self) -> Result<(), String> {
        if self.pause.is_some() {
            Err("PC producers are paused".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn producer_refs(&self) -> Vec<Arc<ProducerCell>> {
        self.retained
            .iter()
            .map(|start| Arc::clone(&start.identity))
            .collect()
    }

    pub(super) fn book_restart(
        &mut self,
        domain: Arc<crate::runtime::config::StopRuntimeDomain>,
        base: u64,
        claimed: Option<u64>,
    ) -> Arc<ProducerCell> {
        let identity = ProducerCell::queued(domain, base);
        let (completion, _) = watch::channel(claimed.map_or(
            NormalStartCompletion::Pending(base),
            NormalStartCompletion::Starting,
        ));
        self.retained.push(NormalStart {
            digest: "restart-dispatch".into(),
            completion,
            identity: Arc::clone(&identity),
            attempt: None,
        });
        identity
    }

    fn replace(&mut self, start: NormalStart) {
        self.prune();
        self.retained.push(start.clone());
        self.current = Some(start);
    }

    fn prune(&mut self) {
        self.retained.retain(|producer| {
            !producer.identity.reclaimable() || producer.identity.has_native_tail()
        });
    }

    fn admitted(&self, generation: u64) -> Option<Arc<ProducerCell>> {
        self.retained
            .iter()
            .find(|producer| {
                producer.identity.has_generation(generation) && !producer.identity.reclaimable()
            })
            .map(|producer| Arc::clone(&producer.identity))
    }
}

pub(super) struct NormalStartClaim {
    pub(super) completion: watch::Sender<NormalStartCompletion>,
    pub(super) expected: u64,
    identity: Arc<ProducerCell>,
    attempt: Option<Arc<crate::runtime::tailscale_login_core::Attempt>>,
}

impl NormalStartClaim {
    pub(super) fn while_active<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        match &self.attempt {
            Some(attempt) => attempt.while_active(action),
            None => Some(action()),
        }
    }
    pub(super) fn admitted(&self, generation: u64) {
        self.identity.admitted(generation);
    }

    #[cfg(target_os = "ios")]
    pub(super) fn has_generation(&self, generation: u64) -> bool {
        self.identity.has_generation(generation)
    }

    pub(super) fn finish_dispatch(&self) {
        self.identity.finish_dispatch();
    }

    pub(super) fn owns(&self, current: Option<&NormalStart>) -> bool {
        current.is_some_and(|current| Arc::ptr_eq(&current.identity, &self.identity))
    }
}

fn digest(value: &Value) -> Result<String, MainPrerequisiteError> {
    serde_json::to_vec(value)
        .map(|bytes| polaris_updater::sha256_hex(&bytes))
        .map_err(|_| MainPrerequisiteError::ReadyUnknown)
}

pub(super) fn merge_tailscale_candidate(
    current: &mut Value,
    saved: &Value,
    candidate: &Value,
) -> Result<(), String> {
    if current != saved {
        return Err("candidateConfigurationChanged".into());
    }
    let candidate = candidate
        .as_object()
        .ok_or("candidateConfigurationChanged")?;
    let id = candidate
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("candidateConfigurationChanged")?;
    if candidate.get("protocol").and_then(Value::as_str) != Some("tailscale") {
        return Err("candidateConfigurationChanged".into());
    }
    if candidate
        .get("tailscaleSettings")
        .and_then(Value::as_object)
        .is_none()
    {
        return Err("candidateConfigurationChanged".into());
    }
    let servers = current
        .get_mut("servers")
        .and_then(Value::as_array_mut)
        .ok_or("candidateConfigurationChanged")?;
    let matches: Vec<_> = servers
        .iter()
        .enumerate()
        .filter_map(|(index, server)| {
            (server.get("id").and_then(Value::as_str) == Some(id)).then_some(index)
        })
        .collect();
    let [index] = matches.as_slice() else {
        return Err("candidateConfigurationChanged".into());
    };
    let server = servers[*index]
        .as_object_mut()
        .ok_or("candidateConfigurationChanged")?;
    if server.get("protocol").and_then(Value::as_str) != Some("tailscale") {
        return Err("candidateConfigurationChanged".into());
    }
    for (key, value) in candidate {
        // Provenance belongs to the original saved node, not the renderer.
        if key == "sourceTag" {
            continue;
        }
        if key == "tailscaleSettings" {
            let mut replacement = value
                .as_object()
                .ok_or("candidateConfigurationChanged")?
                .clone();
            if let Some(previous) = server.get(key).and_then(Value::as_object) {
                // Modeled candidate settings replace the old values, including
                // omission of authKey for browser login. Preserve unmodeled data.
                use polaris_config_engine::user_config::server_config::TAILSCALE_CANDIDATE_SETTINGS_FIELDS;
                for (name, old) in previous {
                    if name == "sourceTag"
                        || !TAILSCALE_CANDIDATE_SETTINGS_FIELDS.contains(&name.as_str())
                            && !replacement.contains_key(name)
                    {
                        replacement.insert(name.clone(), old.clone());
                    }
                }
            }
            server.insert(key.clone(), Value::Object(replacement));
        } else {
            server.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn final_targets(effective: &UserConfig, emitted: &Value) -> BTreeMap<String, String> {
    let tags: BTreeSet<&str> = ["outbounds", "endpoints"]
        .iter()
        .flat_map(|key| {
            emitted
                .get(*key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|entry| entry.get("tag").and_then(Value::as_str))
        .collect();
    ProxyRuntime::endpoint_tag_to_id(effective)
        .into_iter()
        .filter(|(tag, _)| tags.contains(tag.as_str()))
        .map(|(tag, id)| (id, tag))
        .collect()
}

/// The production bound selector and its tests share both transport construction and the
/// actual ManagementApi write. A newer runtime can invalidate the operation, never retarget it.
pub(super) async fn select_ticket_probe<Api, Connect, ConnectFuture>(
    ticket: &ReadyMainTicket,
    slot: usize,
    member_tag: &str,
    check: impl Fn() -> bool,
    connect: Connect,
) -> bool
where
    Api: polaris_switch_engine::ManagementApi,
    Connect: FnOnce(u16, String) -> ConnectFuture,
    ConnectFuture: std::future::Future<Output = Api>,
{
    if member_tag.is_empty()
        || slot >= ticket.probe_ports().len()
        || !ticket
            .binding
            .target_ids
            .iter()
            .any(|id| ticket.target_tag(id) == Some(member_tag))
        || !check()
    {
        return false;
    }
    let api = connect(ticket.api_port(), ticket.api_secret().to_owned()).await;
    if !check() {
        return false;
    }
    let selected = api
        .select_outbound(&format!("probe-selector-{slot}"), member_tag)
        .await
        .is_ok();
    selected && check()
}

fn check_binding(
    ticket: &ReadyMainTicket,
    generation: u64,
    running: bool,
    busy: bool,
    saved_digest: &str,
    current: Option<&Arc<ReadyMainCore>>,
) -> Result<(), MainPrerequisiteError> {
    if generation != ticket.generation() || !running || busy {
        return Err(MainPrerequisiteError::Superseded);
    }
    if saved_digest != ticket.binding.saved_digest || saved_digest != ticket.core.saved_digest {
        return Err(MainPrerequisiteError::ConfigurationChanged);
    }
    if !ticket.core.committed.load(Ordering::SeqCst)
        || ticket.api_port() == 0
        || current.is_none_or(|current| {
            !Arc::ptr_eq(current, &ticket.core)
                || current.emission_digest != ticket.core.emission_digest
        })
    {
        return Err(MainPrerequisiteError::ReadyUnknown);
    }
    if ticket
        .binding
        .target_ids
        .iter()
        .any(|id| !ticket.core.targets.contains_key(id))
    {
        return Err(MainPrerequisiteError::TargetMissing);
    }
    Ok(())
}

#[cfg(any(target_os = "ios", test))]
fn check_tailscale_logout_cold(
    running: bool,
    local_pending: bool,
) -> Result<bool, MainPrerequisiteError> {
    if local_pending {
        return Err(MainPrerequisiteError::ReadyUnknown);
    }
    Ok(!running)
}

#[async_trait]
trait MainReadinessProbe: Sync {
    async fn observe(&self, core: &ReadyMainCore) -> Result<(), MainPrerequisiteError>;
}

async fn validate_with_probe(
    probe: &impl MainReadinessProbe,
    ticket: &ReadyMainTicket,
    check: impl Fn() -> Result<(), MainPrerequisiteError>,
) -> Result<(), MainPrerequisiteError> {
    check()?;
    probe.observe(&ticket.core).await?;
    check()
}

#[async_trait]
impl MainReadinessProbe for ProxyRuntime {
    async fn observe(&self, core: &ReadyMainCore) -> Result<(), MainPrerequisiteError> {
        #[cfg(target_os = "ios")]
        tauri_plugin_polaris_ios::observe_session(&core.ios_receipt)
            .await
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        // A fresh actual unary RPC is required. TCP/channel construction alone grants nothing.
        let client = polaris_singbox_grpc::SingBoxApiClient::connect(
            polaris_singbox_grpc::Endpoint::new("127.0.0.1", core.status.clash_api_port),
            core.api_secret.clone(),
        )
        .await
        .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        client
            .get_clash_mode_status()
            .await
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        Ok(())
    }
}

impl ProxyRuntime {
    /// Select only the current run captured before Close. Historical Bound profiles
    /// and validation runs cannot substitute for the selected runtime's identity.
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn selected_android_auth_node(
        observed: &[polaris_mesh::tailscale_state::retirement::ObservedTailscaleStoreRun],
        actual_digest: &str,
        tag: &str,
        directory: &str,
        export: &polaris_mesh::tailscale_state::retirement::TailscaleStoreExport,
    ) -> Result<polaris_mesh::tailscale_state::retirement::TailscaleStoreRetirementNode, String>
    {
        use polaris_mesh::tailscale_state::retirement::{
            TailscaleProfileState, TailscaleWriterState,
        };
        export
            .validate_observed_runs(observed, true)
            .map_err(|_| "nativeRetirementUnknown")?;
        let current = observed
            .last()
            .filter(|run| run.config_digest == actual_digest)
            .ok_or("profileBindingUnknown")?;
        let file = std::path::Path::new(directory).join("tailscaled.state");
        let file = file.to_str().ok_or("profileBindingUnknown")?;
        let selected: Vec<_> = current
            .scopes
            .iter()
            .filter(|scope| {
                scope.tag == tag && scope.state_directory == directory && scope.state_file == file
            })
            .collect();
        if selected.len() != 1 {
            return Err("profileBindingUnknown".into());
        }
        let node = export
            .instances()
            .iter()
            .find(|run| run.run_nonce == current.run_nonce && run.config_digest == actual_digest)
            .and_then(|run| {
                run.nodes.iter().find(|node| {
                    node.tag == tag && node.state_directory == directory && node.state_file == file
                })
            })
            .ok_or("profileBindingUnknown")?;
        if node.profile_state == TailscaleProfileState::Bound
            && node.writer_state != TailscaleWriterState::SealedDrained
        {
            return Err("profileBindingUnknown".into());
        }
        Ok(node.clone())
    }

    #[cfg(target_os = "android")]
    pub(crate) fn android_tailscale_user_data(&self) -> &std::path::Path {
        self.config.dir()
    }

    #[cfg(target_os = "android")]
    pub(crate) fn android_tailscale_auth_directory(
        &self,
        id: &str,
    ) -> Result<std::path::PathBuf, String> {
        crate::runtime::tailscale_login_core::canonical_tailscale_claim_directory(
            &self
                .mesh
                .tailscale_state_dir(id)
                .map_err(|_| "profileBindingUnknown")?,
        )
    }

    #[cfg(target_os = "android")]
    pub(crate) fn retire_android_tailscale_auth(
        &self,
        id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
        node: &polaris_mesh::tailscale_state::retirement::TailscaleStoreRetirementNode,
    ) -> Result<(), String> {
        self.mesh
            .tailscale_retire_android_auth_under_gate(id, gate, attempt, node)
            .map_err(|_| "stateRevisionChanged".into())
    }

    #[cfg(target_os = "android")]
    pub(crate) async fn stop_android_tailscale_main_origin(
        self: &Arc<Self>,
        id: &str,
        saved: &Value,
        generation: u64,
        request_id: &str,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
    ) -> Result<
        Option<(
            super::android_bridge::tailscale_store::AndroidStoreCustody,
            u64,
            String,
        )>,
        String,
    > {
        use super::android_bridge::tailscale_store;
        // An absent Rust running flag only chooses a route. The separate cold
        // admission protocol must establish native writer facts before warm birth.
        let birth = self
            .android_main_token
            .lock()
            .map_err(|_| "nativeRetirementUnknown")?
            .as_ref()
            .map(|owner| {
                if owner.stop_only
                    || !owner.start_confirmed
                    || owner.historic_unknown
                    || owner.stop_inflight.is_some()
                {
                    return Err("nativeRetirementUnknown");
                }
                Ok((
                    owner.birth.clone(),
                    owner
                        .exact_target
                        .clone()
                        .ok_or("nativeRetirementUnknown")?,
                ))
            })
            .transpose()?;
        let Some((birth, target)) = birth else {
            return if self.tailscale_writer_alive() {
                Err("nativeRetirementUnknown".into())
            } else {
                Ok(None)
            };
        };
        let core = self
            .ready_main_for_generation(generation)
            .map_err(|_| "nativeRetirementUnknown")?;
        if !core.committed.load(Ordering::SeqCst) {
            return Err("nativeRetirementUnknown".into());
        }
        if !core.targets.contains_key(id) {
            // A completed unrelated main is not this account's identity source.
            // Its writer membership must still pass the native family begin census.
            return Ok(None);
        }
        let binding = ActionBinding::new(
            NormalMainAction::TailscaleLogin,
            request_id.to_owned(),
            saved,
            vec![id.to_owned()],
            None,
        )
        .map_err(|_| "nativeRetirementUnknown")?
        .for_attempt(generation, attempt.clone());
        let ticket = ReadyMainTicket { core, binding };
        self.check_ready_main(&ticket)
            .map_err(|error| error.code().to_owned())?;
        let tag = ticket.target_tag(id).ok_or("targetNotInMain")?.to_owned();
        let token = birth.main_token.as_ref().ok_or("nativeRetirementUnknown")?;
        let gate = self.mesh.tailscale_state_gate().await;
        let expected: BTreeSet<_> = self
            .mesh
            .tailscale_main_scope_if_token(token, &gate)?
            .into_iter()
            .collect();
        let original = tailscale_store::observe_main(Some(&target)).await?;
        if Some(original.original().actual_config_digest.as_str())
            != ticket.core.raw_config_digest.as_deref()
            || original
                .original()
                .original_observed_runs
                .last()
                .is_none_or(|run| {
                    run.config_digest != original.original().actual_config_digest
                        || run
                            .scopes
                            .iter()
                            .map(|scope| {
                                (
                                    scope.tag.clone(),
                                    scope.state_directory.clone(),
                                    scope.state_file.clone(),
                                )
                            })
                            .collect::<BTreeSet<_>>()
                            != expected
                        || run.scopes.len() != expected.len()
                })
            || !self
                .android_main_token
                .lock()
                .map_err(|_| "nativeRetirementUnknown")?
                .as_ref()
                .is_some_and(|owner| owner.birth.same(&birth))
        {
            return Err("nativeRetirementUnknown".into());
        }
        self.check_ready_main(&ticket)
            .map_err(|_| "mainCoreChanged")?;
        attempt.record_android_store(original.clone())?;
        drop(gate);
        let stopped = self.stop_for_tailscale_action(generation, attempt).await?;
        // The ordinary Stop ACK is not a scoped receipt. Read the original Entry
        // after the one original Stop, without a second targetless native Stop.
        let retired = tailscale_store::read_main_retirement(&original).await?;
        Ok(Some((retired, stopped, tag)))
    }

    pub(crate) fn merged_tailscale_candidate(
        saved: &Value,
        candidate: &Value,
    ) -> Result<Value, String> {
        let mut preview = saved.clone();
        merge_tailscale_candidate(&mut preview, saved, candidate)?;
        let id = candidate
            .get("id")
            .and_then(Value::as_str)
            .ok_or("candidateConfigurationChanged")?;
        preview
            .get("servers")
            .and_then(Value::as_array)
            .and_then(|nodes| {
                nodes
                    .iter()
                    .find(|node| node.get("id").and_then(Value::as_str) == Some(id))
            })
            .cloned()
            .ok_or_else(|| "candidateConfigurationChanged".to_owned())
    }
    #[cfg(target_os = "ios")]
    pub(crate) fn tailscale_action_saved_config(&self) -> Result<Value, String> {
        self.config
            .current()
            .map_err(|_| "candidateConfigurationChanged".into())
    }

    pub(crate) fn tailscale_action_lease(
        &self,
    ) -> Result<crate::runtime::config::LegacyStartLease, String> {
        let starts = self
            .normal_start
            .lock()
            .map_err(|_| "Main admission poisoned")?;
        starts.assert_unpaused()?;
        self.config
            .lease_legacy_start()
            .map_err(|_| "Managed mesh account retirement requires its managed proof".into())
    }

    pub(crate) fn tailscale_candidate_preflight(
        &self,
        saved: &Value,
        candidate: &Value,
    ) -> Result<(), String> {
        let mut preview = saved.clone();
        merge_tailscale_candidate(&mut preview, saved, candidate)
    }

    #[cfg(target_os = "android")]
    pub(crate) fn saved_android_credential_target(
        &self,
        saved: &Value,
        id: &str,
        generation: u64,
        attempt: &crate::runtime::tailscale_login_core::Attempt,
    ) -> Result<polaris_config_engine::user_config::server_config::ServerConfig, String> {
        attempt
            .while_active(|| {
                self.gate.with_current_generation(generation, |_| {
                    let current = self
                        .config
                        .current()
                        .map_err(|_| "credentialRevisionChanged")?;
                    if current != *saved {
                        return Err("credentialRevisionChanged".to_owned());
                    }
                    let nodes: Vec<_> = current
                        .get("servers")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|node| node.get("id").and_then(Value::as_str) == Some(id))
                        .collect();
                    let [node] = nodes.as_slice() else {
                        return Err("credentialRevisionChanged".into());
                    };
                    serde_json::from_value((*node).clone())
                        .map_err(|_| "credentialRevisionChanged".into())
                })
            })
            .flatten()
            .ok_or_else(|| "mainCoreChanged".to_owned())?
    }

    pub(crate) fn retire_pc_tailscale_auth(
        &self,
        id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: Option<&Arc<crate::runtime::tailscale_login_core::Attempt>>,
    ) -> Result<(), String> {
        self.mesh
            .tailscale_retire_pc_auth_under_gate(id, gate, keep)
            .map_err(|_| "stateRevisionChanged".to_owned())
    }

    /// The caller still holds its original TS state gate and has closed the actual owner.
    /// This only fences the current action generation/cancel cell and original saved CAS.
    pub(crate) fn commit_tailscale_credential(
        &self,
        saved: &Value,
        id: &str,
        generation: u64,
        attempt: Option<&Arc<crate::runtime::tailscale_login_core::Attempt>>,
        commit: impl FnOnce(&mut Value) -> Result<(), String>,
    ) -> Result<Value, String> {
        let starts = self
            .normal_start
            .lock()
            .map_err(|_| "Main admission poisoned")?;
        starts.assert_unpaused()?;
        let action = || {
            self.gate
                .with_current_generation(generation, |_| {
                    let (result, saved) = self
                        .config
                        .update_tailscale_credential(saved, id, |current| match commit(current) {
                            Ok(()) => crate::runtime::config::Decision::Write(Ok(())),
                            Err(error) => crate::runtime::config::Decision::Skip(Err(error)),
                        })
                        .map_err(|_| "credentialRevisionChanged".to_owned())?;
                    result?;
                    saved.ok_or_else(|| "credentialRevisionChanged".to_owned())
                })
                .ok_or_else(|| "mainCoreChanged".to_owned())?
        };
        match attempt {
            Some(attempt) => attempt
                .while_active(action)
                .ok_or_else(|| "cancelled".to_owned())?,
            None => action(),
        }
    }

    /// Admit only the first synchronous poll of this original producer. A Pending
    /// result is already in flight and must be joined through its existing custody.
    pub(crate) fn with_tailscale_credential_birth<T>(
        &self,
        generation: u64,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
        poll: impl FnOnce() -> T,
    ) -> Option<T> {
        let starts = self.normal_start.lock().ok()?;
        starts.assert_unpaused().ok()?;
        attempt
            .while_active(|| self.gate.with_current_generation(generation, |_| poll()))
            .flatten()
    }

    /// Attach an already active provider to the existing custody slot, without
    /// inventing a local birth token. Its original scoped Stop precedes any new
    /// normal Start. An unresolved slot stays occupied even without a token.
    #[cfg(target_os = "ios")]
    pub(crate) async fn prepare_tailscale_action_origin(
        self: &Arc<Self>,
        generation: u64,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
    ) -> Result<u64, String> {
        if attempt.cancelled() || self.core_generation() != generation {
            return Err("mainCoreChanged".into());
        }
        let existing = self
            .ios_ready_session
            .read()
            .map_err(|_| "nativeRetirementUnknown")?
            .clone();
        if let Some(owner) = existing {
            if owner.allows_successor()
                || matches!(&owner.origin, super::IosMainOrigin::LocalStart(_))
                    && owner.ready.is_some()
                    && self.core_running()
            {
                return Ok(generation);
            }
            return self.stop_for_tailscale_action(generation, attempt).await;
        }
        let status = tauri_plugin_polaris_ios::status()
            .await
            .map_err(|_| "nativeRetirementUnknown")?;
        if !status.active {
            return if !attempt.cancelled() && self.core_generation() == generation {
                Ok(generation)
            } else {
                Err("mainCoreChanged".into())
            };
        }
        let observed = tauri_plugin_polaris_ios::observe_current_session()
            .await
            .map_err(|_| "nativeRetirementUnknown")?;
        let gate = self.mesh.tailscale_state_gate().await;
        self.mesh.assert_tailscale_main_claims_drained(&gate)?;
        attempt
            .while_active(|| {
                self.gate.with_current_generation(generation, |_| {
                    let mut slot = self
                        .ios_ready_session
                        .write()
                        .map_err(|_| "nativeRetirementUnknown")?;
                    if slot.is_some() {
                        return Err("nativeRetirementUnknown".to_owned());
                    }
                    *slot = Some(super::IosMainCustody {
                        generation,
                        config_digest: observed.config_digest().to_owned(),
                        origin: super::IosMainOrigin::Observed(observed),
                        main_token: None,
                        ready: None,
                        stopped: None,
                    });
                    Ok(())
                })
            })
            .flatten()
            .ok_or("mainCoreChanged")??;
        drop(gate);
        self.stop_for_tailscale_action(generation, attempt).await
    }

    #[cfg(target_os = "ios")]
    pub(crate) async fn retire_tailscale_account(
        self: &Arc<Self>,
        ticket: &ReadyMainTicket,
        id: &str,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
        saved: &Value,
        candidate: Option<&Value>,
    ) -> Result<(u64, Option<Value>), String> {
        self.validate_ready_main(ticket)
            .await
            .map_err(|_| "mainCoreChanged")?;
        let stopped_generation = self
            .stop_for_tailscale_action(ticket.generation(), attempt)
            .await?;
        let gate = self.mesh.tailscale_state_gate().await;
        self.mesh
            .retire_tailscale_other_attempts_under_gate(id, &gate, attempt)
            .await?;
        let tag = ticket.target_tag(id).ok_or("profileBindingUnknown")?;
        let (fingerprint, revision) = {
            use tauri_plugin_polaris_ios::{
                TailscaleProfileState, TailscaleStateFileState, TailscaleWriterState,
            };
            let owner = self
                .ios_ready_session
                .read()
                .map_err(|_| "nativeRetirementUnknown")?;
            let owner = owner
                .as_ref()
                .filter(|owner| {
                    owner.generation == ticket.generation() && owner.main_token.is_none()
                })
                .ok_or("nativeRetirementUnknown")?;
            let ready = owner
                .ready
                .as_ref()
                .filter(|ready| ready.request_id() == ticket.core.ios_receipt.request_id())
                .ok_or("nativeRetirementUnknown")?;
            let receipt = owner
                .stopped
                .as_ref()
                .and_then(|stopped| stopped.retirement())
                .filter(|receipt| {
                    receipt.start_request_id() == ready.request_id()
                        && receipt.session_id() == ready.session_id()
                        && receipt.config_digest() == ready.config_digest()
                        && receipt.source_extension_generation() == ready.extension_generation()
                })
                .ok_or("nativeRetirementUnknown")?;
            let original: Vec<_> = ready
                .original_instances()
                .iter()
                .filter(|run| {
                    run.config_digest == owner.config_digest
                        && run.terminal == TailscaleWriterState::Unknown
                        && run.nodes.iter().any(|node| node.tag == tag)
                })
                .collect();
            let [original] = original.as_slice() else {
                return Err("profileBindingUnknown".into());
            };
            let run = receipt
                .instances()
                .iter()
                .find(|run| {
                    run.run_nonce == original.run_nonce
                        && run.config_digest == original.config_digest
                })
                .ok_or("nativeRetirementUnknown")?;
            let nodes: Vec<_> = run.nodes.iter().filter(|node| node.tag == tag).collect();
            let [node] = nodes.as_slice() else {
                return Err("profileBindingUnknown".into());
            };
            if node.writer_state != TailscaleWriterState::SealedDrained
                || node.state_file_state != TailscaleStateFileState::Regular
                || node.state_file_revision.len() != 64
                || !node
                    .state_file_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("profileBindingUnknown".into());
            }
            let fingerprint = if node.profile_state == TailscaleProfileState::Bound {
                if node.profile_fingerprint.len() != 64
                    || !node
                        .profile_fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err("profileBindingUnknown".into());
                }
                Some(node.profile_fingerprint.clone())
            } else {
                None
            };
            (fingerprint, node.state_file_revision.clone())
        };
        attempt
            .while_active(|| {
                self.gate.with_current_generation(stopped_generation, |_| {
                    if self
                        .config
                        .current()
                        .map_err(|_| "candidateConfigurationChanged")?
                        != *saved
                    {
                        return Err("candidateConfigurationChanged".into());
                    }
                    let provenance = fingerprint.as_deref().map_or(
                polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences,
                polaris_mesh::tailscale_state::AuthProjectionProvenance::BoundFingerprint);
                    use crate::runtime::config::Decision;
                    let (result, saved) = self
                        .config
                        .update_tailscale_credential(saved, id, |current| {
                            if self
                                .mesh
                                .tailscale_retire_auth_under_gate(
                                    id,
                                    &gate,
                                    Some(attempt),
                                    provenance,
                                    Some(&revision),
                                )
                                .is_err()
                            {
                                return Decision::Skip(Err("stateRevisionChanged".to_owned()));
                            }
                            let result = match candidate {
                                Some(candidate) => {
                                    merge_tailscale_candidate(current, saved, candidate)
                                }
                                None => {
                                    crate::runtime::tailscale_login_core::park_saved_tailscale_key(
                                        current, id,
                                    )
                                }
                            };
                            match result {
                                Ok(()) => Decision::Write(Ok(())),
                                Err(error) => Decision::Skip(Err(error)),
                            }
                        })
                        .map_err(|_| "candidateConfigurationChanged")?;
                    result?;
                    Ok((stopped_generation, saved))
                })
            })
            .flatten()
            .ok_or("mainCoreChanged")?
    }
    #[cfg(all(test, not(target_os = "ios")))]
    pub(crate) fn config_for_commit_test(&self) -> &crate::runtime::config::ConfigManager {
        &self.config
    }

    #[cfg(all(test, not(target_os = "ios")))]
    pub(crate) fn credential_fixture_for_test(
        registry: crate::runtime::tailscale_login_core::LoginCoreRegistry,
    ) -> (
        Arc<Self>,
        Arc<crate::runtime::mesh::MeshRuntime>,
        crate::test_support::TestDir,
        Arc<polaris_core_supervisor::LifecycleGate>,
    ) {
        tests::credential_runtime(registry)
    }
    #[cfg(all(test, not(target_os = "ios")))]
    pub(crate) fn ready_commit_fixture_for_test(
    ) -> (Arc<Self>, ReadyMainTicket, crate::test_support::TestDir) {
        let (runtime, directory) = tests::runtime();
        let (core, binding) = tests::ready_for(&runtime);
        (runtime, ReadyMainTicket { core, binding }, directory)
    }
    fn join_current_normal_start(
        &self,
        saved: &Value,
    ) -> Result<Option<watch::Receiver<NormalStartCompletion>>, MainPrerequisiteError> {
        let expected_digest = digest(saved)?;
        let current = self
            .normal_start
            .lock()
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        let generation = self.core_generation();
        Ok(current
            .as_ref()
            .filter(|start| {
                start.digest == expected_digest
                    && match &*start.completion.borrow() {
                        NormalStartCompletion::Pending(owner)
                        | NormalStartCompletion::Starting(owner) => *owner == generation,
                        NormalStartCompletion::Finished(Ok(status), Some(owner)) => {
                            status.running && *owner == generation
                        }
                        _ => false,
                    }
            })
            .map(|start| start.completion.subscribe()))
    }

    fn ready_main_for_generation(
        &self,
        generation: u64,
    ) -> Result<Arc<ReadyMainCore>, MainPrerequisiteError> {
        if self.core_generation() != generation {
            return Err(MainPrerequisiteError::Superseded);
        }
        let core = self
            .ready_main
            .read()
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        match core.as_ref() {
            Some(core) if core.generation == generation => Ok(Arc::clone(core)),
            Some(_) => Err(MainPrerequisiteError::Superseded),
            None => Err(MainPrerequisiteError::ReadyUnknown),
        }
    }

    async fn wait_ready_main_stable(
        &self,
        core: &ReadyMainCore,
        binding: &ActionBinding,
        budget: Duration,
    ) -> Result<(), MainPrerequisiteError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            if self.core_generation() != core.generation || !self.core_running() {
                return Err(MainPrerequisiteError::Superseded);
            }
            let saved = self
                .config
                .current()
                .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
            if digest(&saved)? != binding.saved_digest {
                return Err(MainPrerequisiteError::ConfigurationChanged);
            }
            if !core.committed.load(Ordering::SeqCst) {
                return Err(MainPrerequisiteError::ReadyUnknown);
            }
            if !self.core_lifecycle_busy()
                && !self.status().starting
                && self.network_settle.is_settled()
            {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(MainPrerequisiteError::ReadyUnknown);
            }
            if self
                .sleep_unless_superseded(core.generation, remaining.min(Duration::from_millis(25)))
                .await
            {
                return Err(MainPrerequisiteError::Superseded);
            }
        }
    }

    /// Existing guarded restarts publish into this same current completion. A newer
    /// explicitly registered producer is never replaced by an older guarded intent.
    pub(super) fn guarded_start_completion(
        &self,
        config: &Value,
        expected: Option<u64>,
    ) -> Result<
        (
            watch::Receiver<NormalStartCompletion>,
            Option<NormalStartClaim>,
        ),
        StartLeg,
    > {
        let config_digest = digest(config)
            .map_err(|error| StartLeg::Finished(Err(StartError::from(error.to_string())), None))?;
        let mut current = self.normal_start.lock().map_err(|_| {
            StartLeg::Finished(
                Err(StartError::from(
                    "normal Start completion poisoned".to_owned(),
                )),
                None,
            )
        })?;
        current
            .assert_unpaused()
            .and_then(|_| {
                polaris_core_supervisor::config_gate::assert_check_producer_registration()
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| StartLeg::Finished(Err(StartError::from(e)), None))?;
        let base = self.core_generation();
        if expected.is_some_and(|expected| expected != base) {
            return Err(StartLeg::Superseded);
        }
        if let Some(start) = current.as_ref() {
            let active = matches!(&*start.completion.borrow(), NormalStartCompletion::Pending(generation) | NormalStartCompletion::Starting(generation) if *generation == base);
            if active {
                return if start.digest == config_digest {
                    Ok((start.completion.subscribe(), None))
                } else {
                    Err(StartLeg::Superseded)
                };
            }
        }
        let (completion, receiver) = watch::channel(NormalStartCompletion::Pending(base));
        let identity = ProducerCell::queued(Arc::clone(&self.stop_domain), base);
        current.replace(NormalStart {
            digest: config_digest,
            completion: completion.clone(),
            identity: Arc::clone(&identity),
            attempt: None,
        });
        Ok((
            receiver,
            Some(NormalStartClaim {
                completion,
                expected: base,
                identity,
                attempt: None,
            }),
        ))
    }

    pub(super) fn capture_ready_main(
        &self,
        generation: u64,
        saved: &Value,
        effective: &UserConfig,
        emitted: &Value,
        status: &ProxyStatus,
    ) -> Result<(), StartError> {
        #[cfg(target_os = "android")]
        let raw_config_digest = {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(self.runtime_config_path())
                .ok()
                .and_then(|file| file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok())
                .filter(|_| bytes.len() <= 4 * 1024 * 1024)
                .and_then(|_| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(|actual| actual == emitted)
                .map(|_| polaris_updater::sha256_hex(&bytes))
        };
        #[cfg(target_os = "ios")]
        let ios_receipt = self
            .ios_ready_session
            .read()
            .ok()
            .and_then(|receipt| {
                receipt
                    .as_ref()
                    .filter(|owner| owner.generation == generation)
                    .and_then(|owner| owner.ready.clone())
            })
            .ok_or_else(|| StartError::from("readyUnknown".to_owned()))?;
        let core = Arc::new(ReadyMainCore {
            generation,
            saved_digest: digest(saved).map_err(|error| StartError::from(error.to_string()))?,
            emission_digest: digest(emitted)
                .map_err(|error| StartError::from(error.to_string()))?,
            targets: final_targets(effective, emitted),
            status: status.clone(),
            api_secret: crate::runtime::management_api::clash_api_secret_in(saved),
            probe_ports: self
                .speed_probe_targets()
                .map(|targets| targets.pool_ports)
                .unwrap_or_default(),
            local_http_proxy: self.local_http_proxy(),
            committed: AtomicBool::new(false),
            #[cfg(target_os = "ios")]
            ios_receipt,
            #[cfg(target_os = "android")]
            raw_config_digest,
        });
        self.gate.with_current_generation(generation, |_live| {
            if let Ok(mut evidence) = self.ready_main.write() {
                *evidence = Some(core);
            }
        });
        Ok(())
    }

    fn commit_ready_main(&self, generation: u64) -> bool {
        self.gate
            .with_current_generation(generation, |_live| {
                if let Ok(evidence) = self.ready_main.read() {
                    if let Some(core) = evidence
                        .as_ref()
                        .filter(|core| core.generation == generation)
                    {
                        core.committed.store(true, Ordering::SeqCst);
                        return true;
                    }
                }
                false
            })
            .unwrap_or(false)
    }

    /// Serialize commit and its ready signals with Start/Stop publication. The
    /// inner gate and evidence locks are released before invoking event listeners,
    /// so a synchronous status reader can reenter the ordinary read API.
    pub(super) fn publish_committed_ready_main(
        &self,
        generation: u64,
        publish: impl FnOnce(),
    ) -> bool {
        let _publication = self.gate.lock_generation_publication();
        if !self.commit_ready_main(generation) {
            return false;
        }
        publish();
        true
    }

    /// Joins only the current same-config normal producer. Dropping its waiter never stops it.
    pub(super) fn normal_start_completion(
        self: &Arc<Self>,
        config: Value,
    ) -> Result<watch::Receiver<NormalStartCompletion>, StartError> {
        self.register_normal_start(config, true, None)
    }

    /// Explicit Start is a new user intent, even when its configuration is unchanged.
    pub(super) fn explicit_start_completion(
        self: &Arc<Self>,
        config: Value,
    ) -> Result<watch::Receiver<NormalStartCompletion>, StartError> {
        self.register_normal_start(config, false, None)
    }

    fn register_normal_start(
        self: &Arc<Self>,
        config: Value,
        join_existing: bool,
        scope: Option<(u64, Arc<crate::runtime::tailscale_login_core::Attempt>)>,
    ) -> Result<watch::Receiver<NormalStartCompletion>, StartError> {
        let config_digest = digest(&config).map_err(|error| StartError::from(error.to_string()))?;
        let mut current = self
            .normal_start
            .lock()
            .map_err(|_| StartError::from("normal Start completion poisoned".to_owned()))?;
        current.assert_unpaused().map_err(StartError::from)?;
        polaris_core_supervisor::config_gate::assert_check_producer_registration()
            .map_err(|e| StartError::from(e.to_string()))?;
        let attempt = scope.as_ref().map(|(_, attempt)| Arc::clone(attempt));
        let register = || {
            // Capture under the same short lock held by admission/claim/Starting publication.
            let base = self.core_generation();
            if scope
                .as_ref()
                .is_some_and(|(expected, _)| *expected != base)
            {
                return Err(StartError::from("superseded".to_owned()));
            }
            if let Some(start) = current
                .as_ref()
                .filter(|start| join_existing && start.digest == config_digest)
            {
                let join = match &*start.completion.borrow() {
                    NormalStartCompletion::Pending(generation) => *generation == base,
                    NormalStartCompletion::Starting(generation) => *generation == base,
                    _ => false,
                };
                if join {
                    return Ok(start.completion.subscribe());
                }
            }
            let (completion, receiver) = watch::channel(NormalStartCompletion::Pending(base));
            let identity = ProducerCell::queued(Arc::clone(&self.stop_domain), base);
            current.replace(NormalStart {
                digest: config_digest,
                completion: completion.clone(),
                identity: Arc::clone(&identity),
                attempt: attempt.clone(),
            });
            let runtime = Arc::clone(self);
            // The spawned producer continues this request, rather than inheriting unrelated
            // tasks' interactivity or reauthorizing a suppressed background helper prompt.
            let interactive = super::startup::helper_gate_interactive();
            tokio::spawn(async move {
                let claim = NormalStartClaim {
                    completion: completion.clone(),
                    expected: base,
                    identity,
                    attempt,
                };
                let start = runtime.start_guarded_with_completion(config, None, Some(&claim));
                let start = async {
                    if interactive {
                        start.await
                    } else {
                        super::startup::with_helper_gate_suppressed(start).await
                    }
                };
                tokio::pin!(start);
                #[cfg(target_os = "ios")]
                let leg = if let Some(attempt) = &claim.attempt {
                    tokio::select! {
                        biased;
                        () = attempt.cancellation() => {
                            // Retain and await the original producer. Dropping its waiter
                            // would strand custody during a permission/native callback.
                            let stop = runtime.claim_cancelled_normal_start(&claim);
                            if let Some((original, generation)) = stop {
                                let _ = tauri_plugin_polaris_ios::revoke_pending_start_through(original, generation).await;
                            }
                            let leg = start.await;
                            if let Some((_, generation)) = stop {
                                if runtime.stop_inner(super::lifecycle::StopClaim::AlreadyClaimed(generation)).await
                                    .is_ok_and(|commit| commit == Some(generation)) {
                                    runtime.clear_system_proxy().await;
                                }
                            }
                            leg
                        }
                        leg = &mut start => leg,
                    }
                } else {
                    start.await
                };
                #[cfg(not(target_os = "ios"))]
                let leg = start.await;
                claim.finish_dispatch();
                runtime.prune_normal_producers();
                completion.send_replace(match leg {
                    StartLeg::Finished(result, generation) => {
                        NormalStartCompletion::Finished(result, generation)
                    }
                    StartLeg::Superseded => NormalStartCompletion::Superseded,
                });
            });
            Ok(receiver)
        };
        match scope.as_ref().map(|(_, attempt)| attempt) {
            Some(attempt) => attempt
                .while_active(register)
                .ok_or_else(|| StartError::from("cancelled".to_owned()))?,
            None => register(),
        }
    }

    pub(super) fn normal_start_attempt(
        &self,
        generation: u64,
    ) -> Result<Option<Arc<crate::runtime::tailscale_login_core::Attempt>>, StartError> {
        self.normal_start
            .lock()
            .map(|starts| {
                starts
                    .retained
                    .iter()
                    .find(|start| start.identity.has_generation(generation))
                    .and_then(|start| start.attempt.clone())
            })
            .map_err(|_| StartError::from("normal Start completion poisoned".to_owned()))
    }

    /// Capture before entering native custody; overwritten admitted A stays discoverable.
    pub(super) fn restart_dispatch(
        &self,
        claimed: Option<u64>,
    ) -> Result<Arc<ProducerCell>, StartError> {
        let mut starts = self
            .normal_start
            .lock()
            .map_err(|_| StartError::from("Main admission poisoned".to_owned()))?;
        let original = claimed.and_then(|generation| starts.retained.iter().find(|start|
            start.digest == "restart-dispatch" && matches!(&*start.completion.borrow(), NormalStartCompletion::Starting(g) if *g == generation))
            .map(|start| Arc::clone(&start.identity)));
        if let Err(error) = starts.assert_unpaused() {
            // This is the original timer continuation returning, not observer Drop.
            if let Some(original) = original {
                original.finish_dispatch();
            }
            return Err(StartError::from(error));
        }
        if let Some(original) = original {
            return Ok(original);
        }
        polaris_core_supervisor::config_gate::assert_check_producer_registration()
            .map_err(|e| StartError::from(e.to_string()))?;
        let base = self.core_generation();
        Ok(starts.book_restart(Arc::clone(&self.stop_domain), base, claimed))
    }

    pub(super) fn finish_preclaimed_restart(&self, generation: u64) {
        if let Ok(starts) = self.normal_start.lock() {
            for start in &starts.retained {
                if start.digest == "restart-dispatch"
                    && matches!(&*start.completion.borrow(), NormalStartCompletion::Starting(g) if *g == generation)
                {
                    start.identity.finish_dispatch();
                }
            }
        }
    }

    pub(super) fn admitted_native_producer(
        &self,
        generation: u64,
    ) -> Result<Option<Arc<ProducerCell>>, StartError> {
        self.normal_start
            .lock()
            .map(|starts| starts.admitted(generation))
            .map_err(|_| StartError::from("normal Start completion poisoned".to_owned()))
    }

    pub(super) fn prune_normal_producers(&self) {
        if let Ok(mut starts) = self.normal_start.lock() {
            starts.prune();
        }
    }

    pub(crate) async fn await_normal_main(
        self: &Arc<Self>,
        binding: ActionBinding,
    ) -> Result<ReadyMainTicket, MainPrerequisiteError> {
        if binding
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.cancelled())
            || binding
                .expected_generation
                .is_some_and(|expected| expected != self.core_generation())
        {
            return Err(MainPrerequisiteError::Superseded);
        }
        let saved = self
            .config
            .current()
            .map_err(|error| MainPrerequisiteError::Failed(StartError::from(error.to_string())))?;
        if digest(&saved)? != binding.saved_digest {
            return Err(MainPrerequisiteError::ConfigurationChanged);
        }
        if !self.core_running() && self.config.has_staged_pending() {
            return Err(MainPrerequisiteError::UnsavedConfiguration);
        }
        // PC/Android callers keep their independent paths; a consumer may explicitly reuse a live main.
        let _requirement = binding.action.requirement(self.helper.platform());
        let evidence = self.ready_main.read().ok().and_then(|core| core.clone());
        let complete_live = self.core_running()
            && !self.status().starting
            && !self.core_lifecycle_busy()
            && evidence.as_ref().is_some_and(|core| {
                core.generation == self.core_generation() && core.committed.load(Ordering::SeqCst)
            });
        let core = if complete_live {
            evidence.ok_or(MainPrerequisiteError::ReadyUnknown)?
        } else {
            let mut completion = if self.core_running() {
                // running is published before the complete transaction. Only its own
                // same-snapshot producer may finish that window; never start another core.
                self.join_current_normal_start(&saved)?
                    .ok_or(MainPrerequisiteError::ReadyUnknown)?
            } else {
                match binding.expected_generation.zip(binding.attempt.clone()) {
                    Some(scope) => self.register_normal_start(saved, true, Some(scope)),
                    None => self.normal_start_completion(saved),
                }
                .map_err(MainPrerequisiteError::Failed)?
            };
            let deadline = tokio::time::sleep(Duration::from_secs(90));
            tokio::pin!(deadline);
            let ready_generation = loop {
                let notified = self.gen_changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                // This lock also protects the guarded restart's Stop-to-Start watch update.
                let (state, current_generation) = {
                    let _claim_guard = self
                        .normal_start
                        .lock()
                        .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
                    (
                        completion.borrow_and_update().clone(),
                        self.core_generation(),
                    )
                };
                match state {
                    NormalStartCompletion::Starting(base) if current_generation != base => {
                        return Err(MainPrerequisiteError::Superseded);
                    }
                    NormalStartCompletion::Finished(Err(error), _) => {
                        return Err(MainPrerequisiteError::Failed(error))
                    }
                    NormalStartCompletion::Finished(Ok(status), Some(generation))
                        if status.running && current_generation == generation =>
                    {
                        break generation;
                    }
                    NormalStartCompletion::Finished(_, _) | NormalStartCompletion::Superseded => {
                        return Err(MainPrerequisiteError::Superseded)
                    }
                    _ => {}
                }
                tokio::select! {
                    result = completion.changed() => if result.is_err() { return Err(MainPrerequisiteError::ReadyUnknown); },
                    () = notified => {},
                    () = &mut deadline => return Err(MainPrerequisiteError::ReadyUnknown),
                }
            };
            self.ready_main_for_generation(ready_generation)?
        };
        self.wait_ready_main_stable(&core, &binding, Duration::from_secs(90))
            .await?;
        let ticket = ReadyMainTicket { core, binding };
        self.validate_ready_main(&ticket).await?;
        Ok(ticket)
    }

    /// Hot credential logout may use only this original completed live producer. A stopped
    /// runtime selects the separately fenced cold leg; a partial live producer is Unknown.
    #[cfg(target_os = "ios")]
    pub(crate) async fn live_tailscale_main(
        &self,
        binding: ActionBinding,
    ) -> Result<Option<ReadyMainTicket>, MainPrerequisiteError> {
        let generation = binding
            .expected_generation
            .ok_or(MainPrerequisiteError::ReadyUnknown)?;
        let attempt = binding
            .attempt
            .as_ref()
            .ok_or(MainPrerequisiteError::ReadyUnknown)?;
        let cold = attempt
            .while_active(|| {
                self.gate.with_current_generation(generation, |_| {
                    let owner = self
                        .ios_ready_session
                        .read()
                        .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
                    let committed = self
                        .ready_main
                        .read()
                        .map_err(|_| MainPrerequisiteError::ReadyUnknown)?
                        .as_ref()
                        .is_some_and(|ready| {
                            ready.generation == generation && ready.committed.load(Ordering::SeqCst)
                        });
                    let local_pending = owner.as_ref().is_some_and(|owner| {
                        matches!(&owner.origin, super::IosMainOrigin::LocalStart(_))
                            && !owner.allows_successor()
                            && (owner.ready.is_none() || !committed)
                    });
                    let running = self
                        .status
                        .read()
                        .map_err(|_| MainPrerequisiteError::ReadyUnknown)?
                        .running;
                    check_tailscale_logout_cold(running, local_pending)
                })
            })
            .flatten()
            .ok_or(MainPrerequisiteError::Superseded)??;
        if cold {
            return Ok(None);
        }
        let core = self.ready_main_for_generation(generation)?;
        let ticket = ReadyMainTicket { core, binding };
        self.validate_ready_main(&ticket).await?;
        Ok(Some(ticket))
    }

    /// 起核就绪时捕获的**已发射配置**摘要（只读）。只在就绪凭据仍属于 `generation` 那一代核时
    /// 给出：换代之后旧摘要不得被当成新核的。
    pub(crate) fn ready_main_emission_digest(&self, generation: u64) -> Option<String> {
        self.ready_main
            .read()
            .ok()?
            .as_ref()
            .filter(|core| core.generation == generation)
            .map(|core| core.emission_digest.clone())
    }

    pub(crate) fn check_ready_main(
        &self,
        ticket: &ReadyMainTicket,
    ) -> Result<(), MainPrerequisiteError> {
        let saved = self
            .config
            .current()
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
        let current = self
            .ready_main
            .read()
            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?
            .clone();
        let status = self.status();
        if status.clash_api_port != ticket.api_port()
            || status.start_time != ticket.start_time()
            || self.local_http_proxy() != ticket.local_http_proxy()
            || self
                .speed_probe_targets()
                .map(|targets| targets.pool_ports)
                .unwrap_or_default()
                != ticket.probe_ports()
        {
            return Err(MainPrerequisiteError::Superseded);
        }
        check_binding(
            ticket,
            self.core_generation(),
            self.core_running(),
            self.core_lifecycle_busy() || !self.network_settle.is_settled(),
            &digest(&saved)?,
            current.as_ref(),
        )
    }

    /// Serialize only an in-memory action result with actual generation and saved writers.
    /// Lock order: publication -> live generation -> saved writer/cache -> action -> detector.
    /// No callback may await, emit, reenter configuration or call a lifecycle method.
    pub(crate) fn with_ready_main_commit<T>(
        &self,
        ticket: &ReadyMainTicket,
        commit: impl FnOnce() -> T,
    ) -> Result<T, MainPrerequisiteError> {
        let _publication = self.gate.lock_generation_publication();
        self.gate
            .with_current_generation(ticket.generation(), |live| {
                self.config
                    .try_with_saved_commit(|saved| {
                        // Read raw state, avoiding status()/local_http_proxy() which relock the gate.
                        let status = self
                            .status
                            .read()
                            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
                        let snapshot = self
                            .switch_snapshot
                            .read()
                            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
                        let current = self
                            .ready_main
                            .read()
                            .map_err(|_| MainPrerequisiteError::ReadyUnknown)?;
                        let local_proxy = super::select_local_http_proxy(
                            status.mixed_port,
                            snapshot.as_ref().and_then(|s| s.probe_proxy_port),
                            snapshot.as_ref().and_then(|s| s.loopback_auth.clone()),
                        );
                        let ports = snapshot
                            .as_ref()
                            .map(|s| s.probe_pool_ports.as_slice())
                            .unwrap_or_default();
                        if status.clash_api_port != ticket.api_port()
                            || status.start_time != ticket.start_time()
                            || local_proxy != ticket.local_http_proxy()
                            || ports != ticket.probe_ports()
                        {
                            return Err(MainPrerequisiteError::Superseded);
                        }
                        check_binding(
                            ticket,
                            live.generation(),
                            status.running,
                            live.is_busy() || !self.network_settle.is_settled(),
                            &digest(saved)?,
                            current.as_ref(),
                        )?;
                        Ok(commit())
                    })
                    .map_err(|_| MainPrerequisiteError::ReadyUnknown)?
            })
            .unwrap_or(Err(MainPrerequisiteError::Superseded))
    }

    /// Bound native report and fresh management RPC bracketed by shared/config checks.
    pub(crate) async fn validate_ready_main(
        &self,
        ticket: &ReadyMainTicket,
    ) -> Result<(), MainPrerequisiteError> {
        validate_with_probe(self, ticket, || self.check_ready_main(ticket)).await
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests;

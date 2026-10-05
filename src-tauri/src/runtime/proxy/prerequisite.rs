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
        })
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

pub(super) struct NormalStart {
    digest: String,
    completion: watch::Sender<NormalStartCompletion>,
    identity: Arc<ProducerCell>,
}

/// Current observer and retained producer responsibilities share the original short lock.
#[derive(Default)]
pub(super) struct NormalStarts {
    current: Option<NormalStart>,
    retained: Vec<Arc<ProducerCell>>,
}

impl NormalStarts {
    pub(super) fn as_ref(&self) -> Option<&NormalStart> {
        self.current.as_ref()
    }

    fn replace(&mut self, start: NormalStart) {
        self.prune();
        self.retained.push(Arc::clone(&start.identity));
        self.current = Some(start);
    }

    fn prune(&mut self) {
        self.retained.retain(|producer| !producer.reclaimable());
    }

    fn admitted(&self, generation: u64) -> Option<Arc<ProducerCell>> {
        self.retained
            .iter()
            .find(|producer| producer.has_generation(generation))
            .cloned()
    }
}

pub(super) struct NormalStartClaim {
    pub(super) completion: watch::Sender<NormalStartCompletion>,
    pub(super) expected: u64,
    identity: Arc<ProducerCell>,
}

impl NormalStartClaim {
    pub(super) fn admitted(&self, generation: u64) {
        self.identity.admitted(generation);
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
    #[cfg(all(test, not(target_os = "ios")))]
    pub(crate) fn config_for_commit_test(&self) -> &crate::runtime::config::ConfigManager {
        &self.config
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
        });
        Ok((
            receiver,
            Some(NormalStartClaim {
                completion,
                expected: base,
                identity,
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
        #[cfg(target_os = "ios")]
        let ios_receipt = self
            .ios_ready_session
            .read()
            .ok()
            .and_then(|receipt| {
                receipt
                    .as_ref()
                    .filter(|(owner, _)| *owner == generation)
                    .map(|(_, receipt)| receipt.clone())
            })
            .ok_or_else(|| StartError::from("readyUnknown".to_owned()))?;
        let core = Arc::new(ReadyMainCore {
            generation,
            saved_digest: digest(saved).map_err(|error| StartError::from(error.to_string()))?,
            emission_digest: digest(emitted)
                .map_err(|error| StartError::from(error.to_string()))?,
            targets: final_targets(effective, emitted),
            status: status.clone(),
            api_secret: saved
                .get("clashApiSecret")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            probe_ports: self
                .speed_probe_targets()
                .map(|targets| targets.pool_ports)
                .unwrap_or_default(),
            local_http_proxy: self.local_http_proxy(),
            committed: AtomicBool::new(false),
            #[cfg(target_os = "ios")]
            ios_receipt,
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
        self.register_normal_start(config, true)
    }

    /// Explicit Start is a new user intent, even when its configuration is unchanged.
    pub(super) fn explicit_start_completion(
        self: &Arc<Self>,
        config: Value,
    ) -> Result<watch::Receiver<NormalStartCompletion>, StartError> {
        self.register_normal_start(config, false)
    }

    fn register_normal_start(
        self: &Arc<Self>,
        config: Value,
        join_existing: bool,
    ) -> Result<watch::Receiver<NormalStartCompletion>, StartError> {
        let config_digest = digest(&config).map_err(|error| StartError::from(error.to_string()))?;
        let mut current = self
            .normal_start
            .lock()
            .map_err(|_| StartError::from("normal Start completion poisoned".to_owned()))?;
        // Capture under the same short lock held by admission/claim/Starting publication.
        let base = self.core_generation();
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
            };
            let start = runtime.start_guarded_with_completion(config, None, Some(&claim));
            let leg = if interactive {
                start.await
            } else {
                super::startup::with_helper_gate_suppressed(start).await
            };
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
    }

    /// Capture before entering native custody; overwritten admitted A stays discoverable.
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
                self.normal_start_completion(saved)
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

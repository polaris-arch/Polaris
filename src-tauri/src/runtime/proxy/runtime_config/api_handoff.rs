//! Local API listener handoff only. Receipt observation is separate from graph
//! persistence, and an unknown Apply is never submitted again.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tokio::sync::Notify;

const PROTOCOL: &str = "listener-handoff-hmac-v1";
const HANDOFF_AUTH_LIFETIME: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ApiEndpoint {
    pub host: String,
    pub port: u16,
    pub secret: String,
}
impl ApiEndpoint {
    pub(super) fn from_generated(config: &SingBoxConfig) -> Option<Self> {
        let services = config.services.as_ref()?;
        if services.len() != 1 || services[0].type_field != "api" {
            return None;
        }
        let s = &services[0];
        let mut ip: std::net::IpAddr = s.listen.parse().ok()?;
        if let std::net::IpAddr::V6(v6) = ip {
            if let Some(v4) = v6.to_ipv4_mapped() {
                ip = v4.into();
            }
        }
        if !ip.is_loopback() || s.listen_port == 0 {
            return None;
        }
        Some(Self {
            host: ip.to_string(),
            port: s.listen_port,
            secret: s.secret.clone().unwrap_or_default(),
        })
    }
    fn canonical(&self) -> String {
        std::net::SocketAddr::new(self.host.parse().expect("validated API IP"), self.port)
            .to_string()
    }
    pub(super) async fn connect(&self) -> Result<SingBoxApiClient, RuntimeConfigError> {
        SingBoxApiClient::connect_captured(
            Endpoint::new(&self.host, self.port),
            self.secret.clone(),
        )
        .await
        .map_err(RuntimeConfigError::from)
    }
}

// These are actual API relay borrowers, not a count of dropped futures. Seal
// prevents a task racing with handoff from entering. Each relay returns its
// guard only after its receive loop, stream and callbacks have unwound.
#[derive(Default)]
pub(crate) struct ApiRelayEpoch {
    state: Mutex<(bool, usize)>,
    changed: Notify,
    idle: Notify,
    tasks: Mutex<Vec<Arc<ApiRelayTask>>>,
    joining: tokio::sync::Mutex<()>,
    failed: AtomicBool,
}
pub(crate) struct ApiRelayBorrow {
    epoch: Arc<ApiRelayEpoch>,
    counted: bool,
}
impl ApiRelayEpoch {
    pub(crate) fn borrow(self: &Arc<Self>) -> ApiRelayBorrow {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let counted = !state.0;
        if counted {
            state.1 += 1;
        }
        ApiRelayBorrow {
            epoch: self.clone(),
            counted,
        }
    }
    pub(crate) fn invalidate(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).0 = true;
        self.changed.notify_waiters();
    }
    pub(crate) fn spawn(
        self: &Arc<Self>,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let _ = self.spawn_owned(future);
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).0
    }
    pub(crate) fn spawn_owned(
        self: &Arc<Self>,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Option<Arc<ApiRelayTask>> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.0 {
            return None;
        }
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.retain(|task| match task.try_join() {
            None => true,
            Some(result) => {
                if result.is_err() {
                    self.failed.store(true, Ordering::Release)
                }
                false
            }
        });
        let epoch = self.clone();
        let task = tokio::spawn(async move {
            tokio::select! {_ = epoch.cancelled()=>{},_ = future=>{}}
        });
        let record = Arc::new(ApiRelayTask {
            abort: task.abort_handle(),
            task: tokio::sync::Mutex::new(Some(task)),
            failed: AtomicBool::new(false),
            expected_abort: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        tasks.push(record.clone());
        Some(record)
    }
    async fn cancelled(&self) {
        loop {
            let mut wait = std::pin::pin!(self.changed.notified());
            wait.as_mut().enable();
            if self.state.lock().unwrap_or_else(|p| p.into_inner()).0 {
                return;
            }
            wait.await;
        }
    }
    pub(in crate::runtime::proxy) async fn seal_and_join(&self) -> Result<(), ()> {
        let _joining = self.joining.lock().await;
        self.state.lock().unwrap_or_else(|p| p.into_inner()).0 = true;
        self.changed.notify_waiters();
        // The shared records retain JoinHandles across cancellation of either
        // the epoch waiter or an individual consumer's close future.
        let tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner()).clone();
        for task in tasks {
            if task.join().await.is_err() {
                self.failed.store(true, Ordering::Release)
            }
        }
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|task| !task.done.load(Ordering::Acquire));
        loop {
            let mut wait = std::pin::pin!(self.idle.notified());
            wait.as_mut().enable();
            if self.state.lock().unwrap_or_else(|p| p.into_inner()).1 == 0 {
                return if self.failed.load(Ordering::Acquire) {
                    Err(())
                } else {
                    Ok(())
                };
            }
            wait.await;
        }
    }
}
pub(crate) struct ApiRelayTask {
    abort: tokio::task::AbortHandle,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    failed: AtomicBool,
    expected_abort: AtomicBool,
    done: AtomicBool,
}
impl ApiRelayTask {
    fn completed(&self, result: Result<(), tokio::task::JoinError>) {
        if let Err(error) = result {
            if !error.is_cancelled() || !self.expected_abort.load(Ordering::Acquire) {
                self.failed.store(true, Ordering::Release)
            }
        }
        self.done.store(true, Ordering::Release);
    }
    fn try_join(&self) -> Option<Result<(), ()>> {
        use futures::FutureExt;
        if self.done.load(Ordering::Acquire) {
            return Some(if self.failed.load(Ordering::Acquire) {
                Err(())
            } else {
                Ok(())
            });
        }
        let mut slot = self.task.try_lock().ok()?;
        let task = slot.as_mut()?;
        if !task.is_finished() {
            return None;
        }
        let result = task.now_or_never()?;
        self.completed(result);
        *slot = None;
        Some(if self.failed.load(Ordering::Acquire) {
            Err(())
        } else {
            Ok(())
        })
    }
    pub(crate) fn abort(&self) {
        self.expected_abort.store(true, Ordering::Release);
        self.abort.abort();
    }
    pub(crate) async fn join(&self) -> Result<(), ()> {
        let mut slot = self.task.lock().await;
        if let Some(task) = slot.as_mut() {
            let result = task.await;
            self.completed(result);
            *slot = None;
        }
        if self.failed.load(Ordering::Acquire) {
            Err(())
        } else {
            Ok(())
        }
    }
    pub(crate) async fn close(&self) {
        self.abort();
        let _ = self.join().await;
    }
}
impl ApiRelayBorrow {
    pub(crate) fn is_closed(&self) -> bool {
        self.epoch.state.lock().unwrap_or_else(|p| p.into_inner()).0
    }
    pub(crate) async fn cancelled(&self) {
        loop {
            let mut wait = std::pin::pin!(self.epoch.changed.notified());
            wait.as_mut().enable();
            if self.epoch.state.lock().unwrap_or_else(|p| p.into_inner()).0 {
                return;
            }
            wait.await;
        }
    }
}
impl polaris_singbox_grpc::ApiClientLifetime for ApiRelayBorrow {
    fn cancelled(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.cancelled())
    }
}
impl Drop for ApiRelayBorrow {
    fn drop(&mut self) {
        if self.counted {
            let mut state = self.epoch.state.lock().unwrap_or_else(|p| p.into_inner());
            state.1 -= 1;
            if state.1 == 0 {
                self.epoch.idle.notify_waiters();
            }
        }
    }
}
pub(crate) async fn relay_cancelled(borrow: &Option<ApiRelayBorrow>) {
    match borrow {
        Some(b) => b.cancelled().await,
        None => std::future::pending().await,
    }
}

pub(super) struct ApiChannelHandoff {
    pub origin: Arc<SingBoxApiClient>,
    pub endpoint: ApiEndpoint,
    pub origin_epoch: Arc<ApiRelayEpoch>,
    pub next_epoch: Arc<ApiRelayEpoch>,
    saved_projection: Option<ApiProjectionPlan>,
    confirmed_active: Mutex<Option<daemon::RuntimeIdentity>>,
    verifier: Arc<Mutex<Option<Arc<Verifier>>>>,
    confirming: tokio::sync::Mutex<()>,
    revoked: AtomicBool,
    deadline: tokio::time::Instant,
    confirmed: AtomicBool,
}
pub(super) struct ApiProjectionPlan {
    owner: std::sync::Weak<super::super::prerequisite::ReadyMainCore>,
    original: String,
    current: String,
}
#[derive(Clone)]
pub(super) struct ApiSavedProjection {
    owner: std::sync::Weak<super::super::prerequisite::ReadyMainCore>,
    original: String,
    current: String,
    active: daemon::RuntimeIdentity,
    epoch: Arc<ApiRelayEpoch>,
    endpoint: ApiEndpoint,
}
struct Verifier {
    key: Mutex<[u8; 32]>,
    deadline: tokio::time::Instant,
    revoked: AtomicBool,
    generation: String,
    endpoint: String,
}
impl Drop for Verifier {
    fn drop(&mut self) {
        self.key
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .fill(0);
    }
}
impl Verifier {
    fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
        self.key.lock().unwrap_or_else(|p| p.into_inner()).fill(0);
    }
    fn live(&self) -> bool {
        !self.revoked.load(Ordering::Acquire) && tokio::time::Instant::now() < self.deadline
    }
}
fn clear_verifier(slot: &Mutex<Option<Arc<Verifier>>>) {
    if let Some(v) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
        v.revoke();
    }
}
impl Drop for ApiChannelHandoff {
    fn drop(&mut self) {
        clear_verifier(&self.verifier);
    }
}
impl ApiChannelHandoff {
    pub(super) fn new(
        origin: Arc<SingBoxApiClient>,
        endpoint: ApiEndpoint,
        epoch: Arc<ApiRelayEpoch>,
        saved_projection: Option<ApiProjectionPlan>,
    ) -> Self {
        let deadline = tokio::time::Instant::now() + HANDOFF_AUTH_LIFETIME;
        let verifier = Arc::new(Mutex::new(None));
        let weak = Arc::downgrade(&verifier);
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(material) = weak.upgrade() {
                clear_verifier(&material);
            }
        });
        Self {
            origin,
            endpoint,
            origin_epoch: epoch,
            next_epoch: Arc::default(),
            saved_projection,
            confirmed_active: Mutex::new(None),
            verifier,
            confirming: tokio::sync::Mutex::new(()),
            revoked: AtomicBool::new(false),
            deadline,
            confirmed: AtomicBool::new(false),
        }
    }
    pub(super) fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
        clear_verifier(&self.verifier);
    }
    pub(super) async fn has_verifier(&self) -> bool {
        if tokio::time::Instant::now() >= self.deadline {
            self.revoke();
        }
        self.verifier
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|v| v.live())
    }
    pub(super) fn confirmed(&self) -> bool {
        self.confirmed.load(Ordering::Acquire)
    }
    pub(super) fn published_projection(&self) -> Option<ApiSavedProjection> {
        if !self.confirmed() {
            return None;
        }
        self.projection_for(self.confirmed_active.lock().ok()?.clone()?)
    }
    fn projection_for(&self, active: daemon::RuntimeIdentity) -> Option<ApiSavedProjection> {
        let p = self.saved_projection.as_ref()?;
        Some(ApiSavedProjection {
            owner: p.owner.clone(),
            original: p.original.clone(),
            current: p.current.clone(),
            active,
            epoch: self.next_epoch.clone(),
            endpoint: self.endpoint.clone(),
        })
    }
}
fn unconfirmed() -> Box<RuntimeApplyResult> {
    let mut result = RuntimeApplyResult::status("unconfirmed");
    result.reasons.push(RuntimeApplyReason {
        code: "API_HANDOFF_UNCONFIRMED",
        path: "services.api".into(),
    });
    Box::new(result)
}
fn decode32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, part) in value.as_bytes().chunks_exact(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(part).ok()?, 16).ok()?;
    }
    Some(out)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(bytes: &[u8]) -> String {
    let suite = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .expect("fixed TLS13 suite");
    hex(suite.common.hash_provider.hash(bytes).as_ref())
}
// RFC2104, using the existing ring implementation exposed by rustls. Only a
// long key reduction is needed before the public64-byte OkmBlock constructor.
fn hmac256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let suite = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .expect("fixed TLS13 suite");
    let reduced;
    let key = if key.len() > 64 {
        reduced = suite.common.hash_provider.hash(key);
        reduced.as_ref()
    } else {
        key
    };
    let tag = suite
        .hkdf_provider
        .hmac_sign(&rustls::crypto::tls13::OkmBlock::new(key), data);
    let mut result = [0; 32];
    result.copy_from_slice(tag.as_ref());
    result
}
fn transcript(
    kind: &str,
    generation: &str,
    txn: &str,
    active: &daemon::RuntimeIdentity,
    endpoint: &str,
    challenge: &str,
    body: &str,
) -> String {
    [
        PROTOCOL,
        kind,
        generation,
        txn,
        &active.incarnation,
        &active.applied_revision.to_string(),
        &active.effective_digest,
        endpoint,
        challenge,
        body,
    ]
    .join("\n")
}
fn verify_wire_proof(
    v: &Verifier,
    transaction: &str,
    envelope: &polaris_singbox_grpc::RuntimeStatusEnvelope,
    nonce: &str,
    require_ack: bool,
) -> bool {
    if !v.live() {
        return false;
    }
    let Some(m) = envelope.handoff.as_ref() else {
        return false;
    };
    let Some(active) = envelope.status.active.as_ref() else {
        return false;
    };
    if (require_ack && !m.acknowledged)
        || m.protocol != PROTOCOL
        || m.generation != v.generation
        || m.endpoint != v.endpoint
        || m.key.is_some()
    {
        return false;
    }
    let kind = if m.acknowledged { "accepted" } else { "proof" };
    let proof = hmac256(
        &*v.key.lock().unwrap_or_else(|p| p.into_inner()),
        transcript(
            kind,
            &v.generation,
            transaction,
            active,
            &v.endpoint,
            nonce,
            &digest(&envelope.status_wire),
        )
        .as_bytes(),
    );
    m.proof
        .as_deref()
        .and_then(decode32)
        .is_some_and(|p| polaris_helper::token::const_time_eq(&p, &proof))
}

fn operation_matches(
    pending: &PendingOperation,
    envelope: &polaris_singbox_grpc::RuntimeStatusEnvelope,
) -> bool {
    let s = &envelope.status;
    let Some(op) = s.operation.as_ref() else {
        return false;
    };
    s.contract_version == 1
        && s.lookup_state == daemon::RuntimeLookupState::Found as i32
        && op.contract_version == 1
        && op.transaction_id == pending.plan.transaction_id
        && op.candidate_digest == pending.plan.candidate_digest
        && op.impact_digest == pending.plan.impact_digest
        && identity_equal(op.before.as_ref(), pending.plan.expected.as_ref())
        && identity_equal(s.active.as_ref(), op.active.as_ref())
        && op.active.as_ref().is_some_and(|a| {
            pending.pairing.incarnation.as_deref() == Some(a.incarnation.as_str())
                && a.effective_digest == pending.plan.candidate_digest
        })
        && matches!(
            daemon::RuntimeOperationState::try_from(op.state),
            Ok(daemon::RuntimeOperationState::Committed
                | daemon::RuntimeOperationState::CommittedDurabilityPending)
        )
}

impl ProxyRuntime {
    pub(super) fn api_saved_projection_for(
        &self,
        next: &Value,
        baseline: &RuntimeConfigBaseline,
    ) -> Option<ApiProjectionPlan> {
        let old = self.current_config.read().ok()?.as_ref()?.clone();
        let mut before = old.as_object()?.clone();
        let mut after = next.as_object()?.clone();
        for key in ["controlApiPort", "clashApiSecret"] {
            before.remove(key);
            after.remove(key);
        }
        if before != after {
            return None;
        }
        let old_digest = super::super::prerequisite::digest(&old).ok()?;
        let new_digest = super::super::prerequisite::digest(next).ok()?;
        let core = self.ready_main.read().ok()?.clone()?;
        if core.generation != baseline.generation || !core.is_committed() {
            return None;
        }
        let original = if let Some(p) = baseline.api_saved_projection.as_ref() {
            if p.current != old_digest
                || !p
                    .owner
                    .upgrade()
                    .is_some_and(|owner| Arc::ptr_eq(&owner, &core))
                || !Arc::ptr_eq(&p.epoch, &baseline.api_epoch)
                || baseline.management.as_ref() != Some(&p.endpoint)
            {
                return None;
            }
            p.original.clone()
        } else {
            if core.saved_digest != old_digest {
                return None;
            }
            old_digest
        };
        Some(ApiProjectionPlan {
            owner: Arc::downgrade(&core),
            original,
            current: new_digest,
        })
    }
    pub(super) async fn runtime_management_client(
        &self,
        generation: u64,
    ) -> Result<Arc<SingBoxApiClient>, ()> {
        let (control, endpoint) = {
            let baseline = self.runtime_config_baseline.read().map_err(|_| ())?;
            let b = baseline
                .as_ref()
                .filter(|b| b.generation == generation)
                .ok_or(())?;
            (b.control.clone(), b.management.clone())
        };
        if let Some(control) = control {
            return if control.captured_transport_live() {
                Ok(control)
            } else {
                Err(())
            };
        }
        // Legacy fixtures/constructors have no captured startup channel. They
        // cannot admit an API handoff; ordinary unchanged configurations retain
        // their previous optional API behavior.
        endpoint
            .ok_or(())?
            .connect()
            .await
            .map(Arc::new)
            .map_err(|_| ())
    }
    pub(in crate::runtime::proxy) fn bind_runtime_api_control(
        &self,
        generation: u64,
        client: Arc<SingBoxApiClient>,
    ) {
        let epoch = {
            let Ok(mut baseline) = self.runtime_config_baseline.write() else {
                return;
            };
            let Some(b) = baseline.as_mut().filter(|b| b.generation == generation) else {
                return;
            };
            b.control = Some(client.clone());
            b.api_epoch.clone()
        };
        let epoch = Arc::downgrade(&epoch);
        client.on_captured_transport_closed(Arc::new(move || {
            if let Some(epoch) = epoch.upgrade() {
                epoch.invalidate();
            }
        }));
    }
    pub(in crate::runtime::proxy) fn runtime_api_baseline_present(&self) -> bool {
        self.runtime_config_baseline
            .read()
            .map(|b| b.is_some())
            .unwrap_or(true)
    }
    pub(in crate::runtime::proxy) fn api_view_for_ready_ticket(
        &self,
        core: &Arc<super::super::prerequisite::ReadyMainCore>,
        current_saved: &str,
    ) -> Option<super::super::prerequisite::ReadyMainAPIView> {
        if self.core_generation() != core.generation || self.core_lifecycle_busy() {
            return None;
        }
        if !self
            .ready_main
            .read()
            .ok()?
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, core))
        {
            return None;
        }
        let baseline = self.runtime_config_baseline.read().ok()?;
        let b = baseline
            .as_ref()
            .filter(|b| b.generation == core.generation)?;
        if b.api_epoch.is_closed() {
            return None;
        }
        let endpoint = b.management.as_ref()?;
        let active = if let Some(p) = b.api_saved_projection.as_ref() {
            if p.original != core.saved_digest
                || p.current != current_saved
                || !p
                    .owner
                    .upgrade()
                    .is_some_and(|owner| Arc::ptr_eq(&owner, core))
                || !Arc::ptr_eq(&p.epoch, &b.api_epoch)
                || endpoint != &p.endpoint
            {
                return None;
            }
            Some(p.active.clone())
        } else {
            if current_saved != core.saved_digest
                || endpoint.port != core.status.clash_api_port
                || endpoint.secret != core.api_secret
            {
                return None;
            }
            None
        };
        Some(super::super::prerequisite::ReadyMainAPIView {
            port: endpoint.port,
            secret: endpoint.secret.clone(),
            saved_digest: current_saved.into(),
            epoch: b.api_epoch.clone(),
            active,
        })
    }
    pub(in crate::runtime::proxy) fn api_ticket_view_is_current(
        &self,
        view: Option<&super::super::prerequisite::ReadyMainAPIView>,
        generation: u64,
        port: u16,
        secret: &str,
    ) -> bool {
        let Ok(baseline) = self.runtime_config_baseline.read() else {
            return false;
        };
        let Some(b) = baseline.as_ref().filter(|b| b.generation == generation) else {
            return view.is_none();
        };
        let Some(v) = view else { return false };
        !b.api_epoch.is_closed()
            && Arc::ptr_eq(&v.epoch, &b.api_epoch)
            && b.management
                .as_ref()
                .is_some_and(|e| e.port == port && e.secret == secret)
            && match &v.active {
                None => b.api_saved_projection.is_none(),
                Some(active) => b
                    .api_saved_projection
                    .as_ref()
                    .is_some_and(|p| &p.active == active),
            }
    }
    pub(in crate::runtime::proxy) fn managed_api_call(
        &self,
        expected_port: u16,
    ) -> Result<Option<(Arc<SingBoxApiClient>, ApiRelayBorrow)>, ()> {
        let generation = self.core_generation();
        let baseline = self.runtime_config_baseline.read().map_err(|_| ())?;
        let Some(b) = baseline.as_ref() else {
            return Ok(None);
        };
        if b.generation != generation || b.api_epoch.is_closed() {
            return Err(());
        }
        let e = b.management.as_ref().ok_or(())?;
        if e.port != expected_port {
            return Err(());
        }
        let lease = b.api_epoch.borrow();
        if lease.is_closed() {
            return Err(());
        }
        // Ordinary calls share this already captured carrier. A buffered or
        // cancelled request can never redial a later occupant of the old tuple.
        let control = b
            .control
            .as_ref()
            .filter(|c| c.captured_transport_live())
            .ok_or(())?;
        Ok(Some((control.clone(), lease)))
    }
    pub(in crate::runtime::proxy) fn managed_readback_binding(
        &self,
        ticket: Option<&super::super::ReadyMainTicket>,
    ) -> Result<Option<super::super::ApiReadbackBinding>, ()> {
        let generation = self.core_generation();
        if ticket.is_some_and(|ticket| ticket.generation() != generation) {
            return Err(());
        }
        let baseline = self.runtime_config_baseline.read().map_err(|_| ())?;
        let Some(b) = baseline.as_ref() else {
            return if ticket.is_some_and(|t| t.api_view.is_some()) {
                Err(())
            } else {
                Ok(None)
            };
        };
        if b.generation != generation || b.api_epoch.is_closed() {
            return Err(());
        }
        let endpoint = b.management.as_ref().ok_or(())?;
        if let Some(ticket) = ticket {
            let view = ticket.api_view.as_ref().ok_or(())?;
            if !Arc::ptr_eq(&view.epoch, &b.api_epoch)
                || view.port != endpoint.port
                || view.secret != endpoint.secret
                || match &view.active {
                    None => b.api_saved_projection.is_some(),
                    Some(active) => b
                        .api_saved_projection
                        .as_ref()
                        .is_none_or(|p| &p.active != active),
                }
            {
                return Err(());
            }
        }
        Ok(Some(super::super::ApiReadbackBinding {
            port: endpoint.port,
            secret: endpoint.secret.clone(),
            epoch: Some(b.api_epoch.clone()),
            control: Some(
                b.control
                    .as_ref()
                    .filter(|c| c.captured_transport_live())
                    .ok_or(())?
                    .clone(),
            ),
        }))
    }
    pub(in crate::runtime::proxy) fn spawn_runtime_api_relay(
        &self,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let epoch = self.runtime_config_baseline.read().ok().and_then(|b| {
            b.as_ref()
                .filter(|b| b.generation == self.gate.generation())
                .map(|b| b.api_epoch.clone())
        });
        if let Some(epoch) = epoch {
            epoch.spawn(future);
        } else {
            tokio::spawn(future);
        }
    }
    pub(crate) fn runtime_api_relay_borrow(&self) -> Option<ApiRelayBorrow> {
        self.runtime_config_baseline
            .read()
            .ok()?
            .as_ref()
            .filter(|b| b.generation == self.gate.generation())
            .map(|b| b.api_epoch.borrow())
    }
    pub(in crate::runtime::proxy) fn verified_runtime_api_endpoint(
        &self,
    ) -> Option<(String, u16, String)> {
        let baseline = self.runtime_config_baseline.read().ok()?;
        let b = baseline
            .as_ref()
            .filter(|b| b.generation == self.gate.generation())?;
        if b.api_epoch.is_closed() {
            return None;
        }
        b.management
            .as_ref()
            .map(|e| (e.host.clone(), e.port, e.secret.clone()))
    }
    pub(super) async fn confirm_runtime_api_handoff(
        self: &Arc<Self>,
        pending: &PendingOperation,
    ) -> Result<(), Box<RuntimeApplyResult>> {
        let Some(h) = pending.api_handoff.as_ref() else {
            return Ok(());
        };
        if h.confirmed() {
            return Ok(());
        }
        match tokio::time::timeout_at(
            h.deadline,
            self.confirm_runtime_api_handoff_before_deadline(pending),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                h.revoke();
                Err(unconfirmed())
            }
        }
    }
    async fn confirm_runtime_api_handoff_before_deadline(
        self: &Arc<Self>,
        pending: &PendingOperation,
    ) -> Result<(), Box<RuntimeApplyResult>> {
        let h = pending.api_handoff.as_ref().ok_or_else(unconfirmed)?;
        let _confirming = h.confirming.lock().await;
        if h.confirmed() {
            return Ok(());
        }
        if h.revoked.load(Ordering::Acquire) || tokio::time::Instant::now() >= h.deadline {
            h.revoke();
            return Err(unconfirmed());
        }
        if self.gate.generation() != pending.generation || !self.core_running() {
            return Err(unconfirmed());
        }
        let req = || daemon::RuntimeConfigStatusRequest {
            contract_version: 1,
            transaction_id: pending.plan.transaction_id.clone(),
            runtime_incarnation: pending.pairing.incarnation.clone().unwrap_or_default(),
        };
        let mut material = h.verifier.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if material.is_none() {
            let response = h
                .origin
                .get_runtime_config_status_handoff(req(), None, None)
                .await
                .map_err(|_| unconfirmed())?;
            if !operation_matches(pending, &response) {
                return Err(unconfirmed());
            }
            let m = response.handoff.ok_or_else(unconfirmed)?;
            if m.protocol != PROTOCOL
                || m.generation.len() != 32
                || !m.generation.bytes().all(|b| b.is_ascii_hexdigit())
                || m.endpoint != h.endpoint.canonical()
                || m.proof.is_some()
                || m.acknowledged
            {
                return Err(unconfirmed());
            }
            let v = Arc::new(Verifier {
                key: Mutex::new(
                    decode32(m.key.as_deref().ok_or_else(unconfirmed)?).ok_or_else(unconfirmed)?,
                ),
                deadline: h.deadline,
                revoked: AtomicBool::new(false),
                generation: m.generation,
                endpoint: m.endpoint,
            });
            {
                let mut slot = h.verifier.lock().unwrap_or_else(|p| p.into_inner());
                if h.revoked.load(Ordering::Acquire)
                    || tokio::time::Instant::now() >= h.deadline
                    || self.gate.generation() != pending.generation
                    || !self.core_running()
                {
                    v.revoke();
                    return Err(unconfirmed());
                }
                *slot = Some(v.clone());
            }
            material = Some(v);
        }
        let v = material.as_ref().ok_or_else(unconfirmed)?;
        let client = h.endpoint.connect().await.map_err(|_| unconfirmed())?;
        let mut nonce = [0; 32];
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut nonce)
            .map_err(|_| unconfirmed())?;
        let nonce = hex(&nonce);
        let response = client
            .get_runtime_config_status_handoff(req(), Some(&nonce), None)
            .await
            .map_err(|_| unconfirmed())?;
        if !operation_matches(pending, &response) {
            return Err(unconfirmed());
        }
        let active = response.status.active.as_ref().ok_or_else(unconfirmed)?;
        // A previous ACK may have committed even when its response was lost.
        // A fresh accepted-domain proof recovers that receipt without Apply.
        let already_accepted = response.handoff.as_ref().is_some_and(|m| m.acknowledged);
        if !verify_wire_proof(
            v,
            &pending.plan.transaction_id,
            &response,
            &nonce,
            already_accepted,
        ) {
            return Err(unconfirmed());
        }
        if self.gate.generation() != pending.generation
            || self.gate.is_busy()
            || !self.core_running()
        {
            return Err(unconfirmed());
        }
        // The new peer is proven before cancelling any original App borrower.
        // Waiting here observes real relay returns, not a guessed timeout.
        h.origin_epoch
            .seal_and_join()
            .await
            .map_err(|_| unconfirmed())?;
        if self.gate.generation() != pending.generation
            || self.gate.is_busy()
            || !self.core_running()
        {
            return Err(unconfirmed());
        }
        let accepted = if already_accepted {
            response
        } else {
            let ack = hex(&hmac256(
                &*v.key.lock().unwrap_or_else(|p| p.into_inner()),
                transcript(
                    "ack",
                    &v.generation,
                    &pending.plan.transaction_id,
                    active,
                    &v.endpoint,
                    &nonce,
                    "",
                )
                .as_bytes(),
            ));
            client
                .get_runtime_config_status_handoff(req(), Some(&nonce), Some(&ack))
                .await
                .map_err(|_| unconfirmed())?
        };
        if !operation_matches(pending, &accepted) {
            return Err(unconfirmed());
        }
        if !verify_wire_proof(v, &pending.plan.transaction_id, &accepted, &nonce, true) {
            return Err(unconfirmed());
        }
        // Keep the bounded verifier until local publication succeeds: a lost
        // capabilities response still needs accepted-domain ACK-loss recovery.
        let caps = paired_runtime_capabilities(&client, &pending.pairing).await?;
        if caps.active.as_ref() != accepted.status.active.as_ref() {
            return Err(unconfirmed());
        }
        {
            let _publication = self.gate.lock_generation_publication();
            if self.gate.generation() != pending.generation
                || self.gate.is_busy()
                || h.revoked.load(Ordering::Acquire)
                || tokio::time::Instant::now() >= h.deadline
            {
                return Err(unconfirmed());
            }
            let projection = h
                .projection_for(accepted.status.active.clone().ok_or_else(unconfirmed)?)
                .ok_or_else(unconfirmed)?;
            let owner = projection.owner.upgrade().ok_or_else(unconfirmed)?;
            if !owner.is_committed()
                || !self
                    .ready_main
                    .read()
                    .map_err(|_| unconfirmed())?
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &owner))
            {
                return Err(unconfirmed());
            }
            let mut baseline = self
                .runtime_config_baseline
                .write()
                .map_err(|_| unconfirmed())?;
            let b = baseline
                .as_mut()
                .filter(|b| b.generation == pending.generation && b.pairing == pending.pairing)
                .ok_or_else(unconfirmed)?;
            // Acquire every fallible publication guard before changing any view.
            let mut status = self.status.write().map_err(|_| unconfirmed())?;
            let mut confirmed_active = h.confirmed_active.lock().map_err(|_| unconfirmed())?;
            // Endpoint, epoch and the verified API-only view are one publication.
            // A normal producer must never see the new tuple without its alias.
            b.api_saved_projection = Some(projection);
            b.management = Some(h.endpoint.clone());
            b.api_epoch = h.next_epoch.clone();
            b.control = Some(Arc::new(client));
            status.clash_api_port = h.endpoint.port;
            *confirmed_active = accepted.status.active.clone();
            h.confirmed.store(true, Ordering::Release);
        }
        clear_verifier(&h.verifier);
        let control = self
            .runtime_config_baseline
            .read()
            .ok()
            .and_then(|b| b.as_ref().and_then(|b| b.control.clone()));
        if let Some(control) = control {
            self.bind_runtime_api_control(pending.generation, control);
        }
        self.gen_changed.notify_waiters();
        // Only the former borrower set was sealed; no unrelated App generation,
        // helper lifecycle or physical route authority is changed.
        let pipe = self
            .runtime_config_baseline
            .read()
            .ok()
            .and_then(|b| b.as_ref().and_then(|b| b.api_log_handoff.clone()));
        self.spawn_core_log_relay(pending.generation, h.endpoint.port, pipe);
        self.spawn_native_auto_relay(pending.generation, h.endpoint.port);
        // An API-only recipe retains the actual emitted tag namespace. Do not
        // reconstruct it from unfiltered saved servers after startup peeling.
        let tags = Arc::new(
            self.switch_snapshot
                .read()
                .ok()
                .and_then(|s| s.clone())
                .map(|s| {
                    s.id_to_tag
                        .into_iter()
                        .map(|(id, tag)| (tag, id))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default(),
        );
        self.spawn_openconnect_status_relay(pending.generation, h.endpoint.port, tags.clone());
        self.spawn_openvpn_status_relay(pending.generation, h.endpoint.port, tags.clone());
        if let Some(context) = self.tailnet_context() {
            self.spawn_tailscale_status_relay(
                pending.generation,
                h.endpoint.port,
                Arc::new(context.tag_to_id()),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

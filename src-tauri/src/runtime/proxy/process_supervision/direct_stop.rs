//! Dormant exact-stop worker for a locally owned desktop Child. Only tests can
//! commit its prepared worker; no production stop or Apply path reaches it.

use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::direct_custody::{
    BackendFence, DirectBirthCloseError, DirectCoreSlot, SlotAdmissionError, SlotInstance,
    StopPoll, StopView, WorkerNonce,
};
use crate::runtime::config::{StopReservationCheck, StopReservationReceipt, StopRuntimeDomain};
use crate::runtime::mesh::MeshRuntime;
use crate::runtime::proxy::process_supervision::RunIdentity;
use crate::runtime::proxy::ProxyRuntime;
use polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan;
use polaris_store::StoreError;

const READY: u8 = 0;
const COMMITTED: u8 = 1;
const GONE_BEFORE: u8 = 2;
const GONE_AFTER: u8 = 3;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::runtime::proxy) trait DirectStopIo: Send + Sync {
    fn try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>>;
    fn start_kill(&self, child: &mut Child) -> io::Result<()>;
}

struct NativeStopIo;

impl DirectStopIo for NativeStopIo {
    fn try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        child.try_wait()
    }

    fn start_kill(&self, child: &mut Child) -> io::Result<()> {
        child.start_kill()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum PrepareError {
    Unsupported,
    LockPoisoned,
    Slot(SlotAdmissionError),
    WorkerUnavailable,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum CommitRejected {
    Slot(SlotAdmissionError),
    WorkerUnavailable,
}

/// Drop aborts an uncommitted worker. The worker is ready before this value
/// can be returned, but it cannot signal while the slot is still Running.
pub(in crate::runtime::proxy) struct PreparedDirectStop {
    domain: Arc<StopRuntimeDomain>,
    slot: Arc<Mutex<DirectCoreSlot>>,
    instance: Arc<SlotInstance>,
    identity: RunIdentity,
    nonce: WorkerNonce,
    alive: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for PreparedDirectStop {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

impl PreparedDirectStop {
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn rebind_slot_for_test(
        &mut self,
        slot: Arc<Mutex<DirectCoreSlot>>,
    ) {
        self.slot = slot;
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn gone_before_for_test(&self) -> bool {
        self.phase.load(Ordering::Acquire) == GONE_BEFORE
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn abort_handle_for_test(&self) -> tokio::task::AbortHandle {
        self.worker
            .as_ref()
            .expect("prepared worker")
            .abort_handle()
    }
}

/// Read-only view. Dropping it has no effect on the detached worker or Child.
pub(in crate::runtime::proxy) struct DirectStopObservation {
    domain: Arc<StopRuntimeDomain>,
    slot: Arc<Mutex<DirectCoreSlot>>,
    instance: Arc<SlotInstance>,
    identity: RunIdentity,
    nonce: WorkerNonce,
    alive: Arc<AtomicBool>,
}

/// Runtime-bound local terminal proof for one reaped direct Child. The Child
/// stays in Stopping; this is not a global owner or lifecycle receipt.
pub(in crate::runtime::proxy) struct DirectBirthClosed {
    domain: Arc<StopRuntimeDomain>,
    slot: Arc<Mutex<DirectCoreSlot>>,
    instance: Arc<SlotInstance>,
    identity: RunIdentity,
    nonce: WorkerNonce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum DirectStopProvenance {
    LegacyExact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum StopWaitOutcome {
    Pending,
    Reaped,
    RetainedFailure,
    WorkerGone,
    Obsolete,
}

impl DirectStopObservation {
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn clone_for_test(&self) -> Self {
        Self {
            domain: Arc::clone(&self.domain),
            slot: Arc::clone(&self.slot),
            instance: Arc::clone(&self.instance),
            identity: self.identity.clone(),
            nonce: self.nonce.clone(),
            alive: Arc::clone(&self.alive),
        }
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn identity_for_test(&self) -> RunIdentity {
        self.identity.clone()
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn rebind_slot_for_test(
        &mut self,
        slot: Arc<Mutex<DirectCoreSlot>>,
    ) {
        self.slot = slot;
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn rebind_domain_for_test(
        &mut self,
        domain: Arc<StopRuntimeDomain>,
    ) {
        self.domain = domain;
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn rebind_identity_for_test(&mut self, identity: RunIdentity) {
        self.identity = identity;
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn corrupt_nonce_for_test(&mut self) {
        self.nonce = WorkerNonce::new();
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn matches_closed_for_test(
        &self,
        closed: &DirectBirthClosed,
    ) -> bool {
        Arc::ptr_eq(&self.domain, &closed.domain)
            && Arc::ptr_eq(&self.slot, &closed.slot)
            && Arc::ptr_eq(&self.instance, &closed.instance)
            && self.identity.same_run(&closed.identity)
            && self.nonce.same(&closed.nonce)
    }

    pub(in crate::runtime::proxy) fn provenance(&self) -> DirectStopProvenance {
        DirectStopProvenance::LegacyExact
    }

    pub(in crate::runtime::proxy) fn worker_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(in crate::runtime::proxy) fn view(&self) -> StopView {
        self.slot
            .lock()
            .map(|slot| slot.view_exact_stop(&self.instance, &self.identity, &self.nonce))
            .unwrap_or(StopView::Obsolete)
    }

    /// An observer timeout reports Pending; it never aborts the worker.
    pub(in crate::runtime::proxy) async fn wait_for(&self, timeout: Duration) -> StopWaitOutcome {
        let wait = async {
            loop {
                match self.view() {
                    StopView::Reaped | StopView::BirthClosed => return StopWaitOutcome::Reaped,
                    StopView::RetainedFailure => return StopWaitOutcome::RetainedFailure,
                    StopView::Obsolete => return StopWaitOutcome::Obsolete,
                    StopView::Armed | StopView::KillRequested if !self.worker_alive() => {
                        return StopWaitOutcome::WorkerGone;
                    }
                    StopView::Armed | StopView::KillRequested => {
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .unwrap_or(StopWaitOutcome::Pending)
    }
}

struct WorkerPolicy {
    poll_interval: Duration,
    reap_timeout: Duration,
    #[cfg(test)]
    pause_after_kill: Option<Arc<tokio::sync::Notify>>,
}

impl Default for WorkerPolicy {
    fn default() -> Self {
        Self {
            poll_interval: POLL_INTERVAL,
            reap_timeout: REAP_TIMEOUT,
            #[cfg(test)]
            pause_after_kill: None,
        }
    }
}

struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct WorkerContext {
    mesh: Arc<MeshRuntime>,
    slot: Arc<Mutex<DirectCoreSlot>>,
    instance: Arc<SlotInstance>,
    identity: RunIdentity,
    nonce: WorkerNonce,
    alive: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    io: Arc<dyn DirectStopIo>,
    policy: WorkerPolicy,
}

pub(in crate::runtime::proxy) async fn prepare_direct_stop(
    runtime: &Arc<ProxyRuntime>,
    expected: &RunIdentity,
) -> Result<PreparedDirectStop, PrepareError> {
    prepare_with_io(
        runtime,
        expected,
        Arc::new(NativeStopIo),
        WorkerPolicy::default(),
    )
    .await
}

async fn prepare_with_io(
    runtime: &Arc<ProxyRuntime>,
    expected: &RunIdentity,
    io: Arc<dyn DirectStopIo>,
    policy: WorkerPolicy,
) -> Result<PreparedDirectStop, PrepareError> {
    if cfg!(target_os = "android") || runtime.core_via_helper.load(Ordering::SeqCst) {
        return Err(PrepareError::Unsupported);
    }
    let slot = Arc::clone(&runtime.child);
    let instance = {
        let guard = slot.lock().map_err(|_| PrepareError::LockPoisoned)?;
        guard
            .admit_exact_stop(expected)
            .map_err(PrepareError::Slot)?;
        guard.instance()
    };
    let identity = expected.clone();
    let nonce = WorkerNonce::new();
    let alive = Arc::new(AtomicBool::new(false));
    let phase = Arc::new(AtomicU8::new(READY));
    let (ready_tx, ready_rx) = oneshot::channel();
    let worker = tokio::spawn(run_worker(
        WorkerContext {
            mesh: Arc::clone(&runtime.mesh),
            slot: Arc::clone(&slot),
            instance: Arc::clone(&instance),
            identity: identity.clone(),
            nonce: nonce.clone(),
            alive: Arc::clone(&alive),
            phase: Arc::clone(&phase),
            io,
            policy,
        },
        ready_tx,
    ));
    let prepared = PreparedDirectStop {
        domain: Arc::clone(&runtime.stop_domain),
        slot,
        instance,
        identity,
        nonce,
        alive,
        phase,
        worker: Some(worker),
    };
    ready_rx
        .await
        .map_err(|_| PrepareError::WorkerUnavailable)?;
    if prepared.phase.load(Ordering::Acquire) != READY {
        return Err(PrepareError::WorkerUnavailable);
    }
    Ok(prepared)
}

async fn run_worker(context: WorkerContext, ready: oneshot::Sender<()>) {
    let WorkerContext {
        mesh,
        slot,
        instance,
        identity,
        nonce,
        alive,
        phase,
        io,
        policy,
    } = context;
    alive.store(true, Ordering::Release);
    let _alive_guard = AliveGuard(alive);
    let _phase_guard = WorkerPhaseGuard(phase);
    if ready.send(()).is_err() {
        return;
    }
    let mut deadline = None;
    loop {
        let outcome = {
            let _ts_gate = mesh.tailscale_state_gate().await;
            let Ok(mut custody) = slot.lock() else {
                return;
            };
            custody.poll_exact_stop(
                &instance,
                &identity,
                &nonce,
                deadline.is_some_and(|until| Instant::now() >= until),
                |child| io.try_wait(child),
                |child| io.start_kill(child),
            )
        };
        match outcome {
            StopPoll::Reaped | StopPoll::RetainedFailure | StopPoll::Obsolete => return,
            StopPoll::KillRequested => {
                deadline.get_or_insert_with(|| Instant::now() + policy.reap_timeout);
                #[cfg(test)]
                if let Some(pause) = &policy.pause_after_kill {
                    pause.notified().await;
                }
            }
            StopPoll::Pending => {
                deadline.get_or_insert_with(|| Instant::now() + policy.reap_timeout);
            }
            StopPoll::Uncommitted => {}
        }
        tokio::time::sleep(policy.poll_interval).await;
    }
}

struct WorkerPhaseGuard(Arc<AtomicU8>);

impl Drop for WorkerPhaseGuard {
    fn drop(&mut self) {
        let _ = self
            .0
            .compare_exchange(READY, GONE_BEFORE, Ordering::AcqRel, Ordering::Acquire);
        let _ = self
            .0
            .compare_exchange(COMMITTED, GONE_AFTER, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Busy preserves both single-use inputs for a retry. Every other failure
/// drops the prepared worker while the physical Child stays Running.
pub(in crate::runtime::proxy) enum CommitDirectStopError {
    Busy {
        receipt: Box<StopReservationReceipt>,
        prepared: PreparedDirectStop,
    },
    Rejected,
    Store(StoreError),
}

impl ProxyRuntime {
    /// Hand off only a reaped, same-birth direct Child to a local terminal
    /// proof. This dormant entry has no normal Stop/Apply caller. It does not
    /// attest global ownership or finish the lifecycle/config transaction.
    pub(in crate::runtime::proxy) async fn close_reaped_direct_birth(
        self: &Arc<Self>,
        observation: &DirectStopObservation,
    ) -> Result<DirectBirthClosed, DirectBirthCloseError> {
        if cfg!(target_os = "android") {
            return Err(DirectBirthCloseError::Unsupported);
        }
        if !Arc::ptr_eq(&observation.domain, &self.stop_domain)
            || !Arc::ptr_eq(&observation.slot, &self.child)
        {
            return Err(DirectBirthCloseError::WrongRuntime);
        }
        let ts_gate = self.mesh.tailscale_state_gate().await;
        let mut slot = self
            .child
            .lock()
            .map_err(|_| DirectBirthCloseError::LockPoisoned)?;
        if self.core_via_helper.load(Ordering::SeqCst) {
            return Err(DirectBirthCloseError::HelperTouched);
        }
        let proof = DirectBirthClosed {
            domain: Arc::clone(&self.stop_domain),
            slot: Arc::clone(&self.child),
            instance: Arc::clone(&observation.instance),
            identity: observation.identity.clone(),
            nonce: observation.nonce.clone(),
        };
        slot.close_reaped_birth(
            &observation.instance,
            &observation.identity,
            &observation.nonce,
            |token| {
                self.mesh
                    .release_tailscale_main_states_if_token(token, &ts_gate)
            },
        )?;
        Ok(proof)
    }

    /// The dormant production bridge for one exact local Legacy Child. This
    /// deliberately has no normal Stop/Start/Apply call site. The runtime owns
    /// all authority; callers supply only the typed CAS result and worker.
    pub(in crate::runtime::proxy) async fn commit_reserved_direct_stop(
        self: &Arc<Self>,
        receipt: StopReservationReceipt,
        plan: &ManagedMeshRoutePlan,
        mut prepared: PreparedDirectStop,
    ) -> Result<DirectStopObservation, CommitDirectStopError> {
        if cfg!(target_os = "android")
            || !receipt.belongs_to(&self.stop_domain)
            || !Arc::ptr_eq(&prepared.domain, &self.stop_domain)
            || !Arc::ptr_eq(&prepared.slot, &self.child)
            || receipt.stop_target_run_ref() != prepared.identity.persisted_ref()
        {
            return Err(CommitDirectStopError::Rejected);
        }
        let Ok(expected_generation) = receipt.claim().lifecycle_generation.parse::<u64>() else {
            return Err(CommitDirectStopError::Rejected);
        };
        let _ts_gate = self.mesh.tailscale_state_gate().await;
        let mut slot = self
            .child
            .lock()
            .map_err(|_| CommitDirectStopError::Rejected)?;
        if !Arc::ptr_eq(&slot.instance(), &prepared.instance)
            || slot.backend() != BackendFence::DirectOnly
            || self.core_via_helper.load(Ordering::SeqCst)
            || slot.admit_exact_stop(&prepared.identity).is_err()
        {
            return Err(CommitDirectStopError::Rejected);
        }
        let Some(check) = self
            .gate
            .with_current_generation(expected_generation, |live| {
                self.config.try_with_current_stop_reservation(
                    receipt,
                    live,
                    &self.stop_domain,
                    plan,
                    |_, _| {
                        // The worker's terminal transition races this one CAS.
                        // After success, only infallible local moves are allowed.
                        if prepared
                            .phase
                            .compare_exchange(READY, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
                            .is_err()
                        {
                            return None;
                        }
                        slot.commit_armed_unchecked(prepared.nonce.clone());
                        let observation = DirectStopObservation {
                            domain: Arc::clone(&self.stop_domain),
                            slot: Arc::clone(&prepared.slot),
                            instance: Arc::clone(&prepared.instance),
                            identity: prepared.identity.clone(),
                            nonce: prepared.nonce.clone(),
                            alive: Arc::clone(&prepared.alive),
                        };
                        drop(prepared.worker.take());
                        Some(observation)
                    },
                )
            })
        else {
            return Err(CommitDirectStopError::Rejected);
        };
        match check {
            Ok(StopReservationCheck::Busy(receipt)) => {
                Err(CommitDirectStopError::Busy { receipt, prepared })
            }
            Ok(StopReservationCheck::Current(Some(observation))) => Ok(observation),
            Ok(StopReservationCheck::Current(None) | StopReservationCheck::Rejected) => {
                Err(CommitDirectStopError::Rejected)
            }
            Err(error) => Err(CommitDirectStopError::Store(error)),
        }
    }
}

/// The only commit door in this slice. A failed commit drops `prepared` and
/// aborts its worker; a successful commit detaches that worker from callers.
#[cfg(test)]
pub(in crate::runtime::proxy) fn commit_for_test(
    slot: &mut DirectCoreSlot,
    expected: &RunIdentity,
    mut prepared: PreparedDirectStop,
) -> Result<DirectStopObservation, CommitRejected> {
    if !prepared.identity.same_run(expected) {
        return Err(CommitRejected::Slot(SlotAdmissionError::WrongRun));
    }
    if !Arc::ptr_eq(&slot.instance(), &prepared.instance) {
        return Err(CommitRejected::Slot(SlotAdmissionError::WrongSlot));
    }
    slot.admit_exact_stop(expected)
        .map_err(CommitRejected::Slot)?;
    if prepared
        .phase
        .compare_exchange(READY, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(CommitRejected::WorkerUnavailable);
    }
    slot.commit_armed_unchecked(prepared.nonce.clone());
    let observation = DirectStopObservation {
        domain: Arc::clone(&prepared.domain),
        slot: Arc::clone(&prepared.slot),
        instance: Arc::clone(&prepared.instance),
        identity: prepared.identity.clone(),
        nonce: prepared.nonce.clone(),
        alive: Arc::clone(&prepared.alive),
    };
    drop(prepared.worker.take()); // detach; observation Drop cannot cancel it
    Ok(observation)
}

#[cfg(test)]
pub(in crate::runtime::proxy) async fn prepare_with_io_for_test(
    runtime: &Arc<ProxyRuntime>,
    expected: &RunIdentity,
    io: Arc<dyn DirectStopIo>,
    poll_interval: Duration,
    reap_timeout: Duration,
    pause_after_kill: Option<Arc<tokio::sync::Notify>>,
) -> Result<PreparedDirectStop, PrepareError> {
    prepare_with_io(
        runtime,
        expected,
        io,
        WorkerPolicy {
            poll_interval,
            reap_timeout,
            pause_after_kill,
        },
    )
    .await
}

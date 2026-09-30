//! Direct desktop Child custody. Ordinary Stop retains its Child through
//! cancellation/errors; exact-worker Stopping remains permanently reserved.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::runtime::helper::HelperStopTarget;
use crate::runtime::proxy::process_supervision::{DirectCoreRun, DirectRunOrigin, RunIdentity};
use polaris_core_supervisor::ChildObservation;
use polaris_helper_proto::{HelperBirthTarget, StartNotAdmitted};
use tokio::process::Child;

pub(crate) struct DirectCoreSlot {
    instance: Arc<SlotInstance>,
    backend: BackendFence,
    helper_start: Option<HelperStartAttempt>,
    state: SlotState,
}

impl Default for DirectCoreSlot {
    fn default() -> Self {
        Self {
            instance: Arc::new(SlotInstance),
            backend: BackendFence::DirectOnly,
            helper_start: None,
            state: SlotState::Empty,
        }
    }
}

pub(super) struct SlotInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BackendFence {
    DirectOnly,
    HelperTouched,
}

/// A helper Start remains owned even when its async waiter disappears. Only
/// an exact acknowledged Stop may retire the same opaque attempt.
#[derive(Clone, Debug)]
pub(in crate::runtime::proxy) struct HelperStartToken(Arc<()>);

impl HelperStartToken {
    fn new() -> Self {
        Self(Arc::new(()))
    }

    pub(in crate::runtime::proxy) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

struct HelperStartAttempt {
    token: HelperStartToken,
    main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    phase: HelperStartPhase,
    stop_inflight: Option<HelperStopNonce>,
}

/// Non-cloneable Stop booking identity. The permit owns this nonce until an
/// ACK is checked under Child→pid or the blocking task is fully gone.
pub(in crate::runtime::proxy) struct HelperStopNonce(Arc<()>);

impl HelperStopNonce {
    fn new() -> Self {
        Self(Arc::new(()))
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

enum HelperStartPhase {
    Inflight,
    Known(HelperStopTarget),
    /// The current Start spawned nothing, but adopted exact custody of an
    /// earlier Linux helper birth that still blocks admission.
    NotAdmitted(HelperStopTarget),
    Unconfirmed,
}

impl HelperStartPhase {
    fn exact_stop_target(&self) -> Option<HelperStopTarget> {
        match self {
            Self::Known(HelperStopTarget::Legacy(0))
            | Self::NotAdmitted(HelperStopTarget::Legacy(0)) => None,
            Self::Known(target) | Self::NotAdmitted(target) => Some(*target),
            Self::Inflight | Self::Unconfirmed => None,
        }
    }
}

#[derive(Clone)]
pub(in crate::runtime::proxy) struct WorkerNonce(Arc<()>);

impl WorkerNonce {
    pub(super) fn new() -> Self {
        Self(Arc::new(()))
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Holds an exclusive borrow of a proven Empty slot through synchronous spawn.
/// The spawned Child can then be installed without a rejection/drop path.
pub(in crate::runtime::proxy) struct EmptyDirectSlot<'a> {
    slot: &'a mut DirectCoreSlot,
}

impl EmptyDirectSlot<'_> {
    pub(in crate::runtime::proxy) fn install_running(self, run: DirectCoreRun) {
        self.slot.state = SlotState::Running(run);
    }
}

#[derive(Default)]
enum SlotState {
    #[default]
    Empty,
    Running(DirectCoreRun),
    Stopping(DirectStoppingCustody),
}

struct DirectStoppingCustody {
    run: DirectCoreRun,
    phase: StopPhase,
}

enum StopPhase {
    /// Ordinary Stop keeps the Child here while its cancellable waiter owns
    /// only the nonce. Native wait success stays cached through commit errors.
    NativeWait {
        nonce: WorkerNonce,
        inflight: bool,
        exit: Option<ExitStatus>,
    },
    Armed {
        nonce: WorkerNonce,
    },
    KillRequested {
        nonce: WorkerNonce,
    },
    Reaped(DirectChildReaped),
    BirthClosed(DirectChildReaped),
    RetainedFailure {
        nonce: WorkerNonce,
        reason: StopFailure,
    },
}

impl StopPhase {
    fn nonce(&self) -> &WorkerNonce {
        match self {
            Self::NativeWait { nonce, .. }
            | Self::Armed { nonce }
            | Self::KillRequested { nonce }
            | Self::RetainedFailure { nonce, .. } => nonce,
            Self::Reaped(proof) | Self::BirthClosed(proof) => &proof.nonce,
        }
    }
}

/// Private proof minted only by `try_wait Ok(Some)` on the custody Child.
/// It is not a release permit and cannot be copied to another run.
struct DirectChildReaped {
    identity: RunIdentity,
    nonce: WorkerNonce,
    status: ExitStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopFailure {
    TryWait,
    StartKill,
    Timeout,
    #[cfg(test)]
    UnarmedFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum SlotAdmissionError {
    Empty,
    WrongRun,
    WrongSlot,
    Busy,
    HelperTouched,
    ManagedOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopPoll {
    Uncommitted,
    KillRequested,
    Pending,
    Reaped,
    RetainedFailure,
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum StopView {
    Armed,
    KillRequested,
    Reaped,
    BirthClosed,
    RetainedFailure,
    Obsolete,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum DirectBirthCloseError {
    Unsupported,
    WrongRuntime,
    WrongSlot,
    WrongRun,
    HelperTouched,
    ManagedOrigin,
    NotReaped,
    AlreadyClosed,
    ClaimMismatch,
    Registry(String),
    LockPoisoned,
}

/// This is only an observation of the reservation. Dropping it cannot release
/// the Child, which remains owned by `DirectCoreSlot::Stopping`.
pub(in crate::runtime::proxy) struct DirectStoppingObservation {
    identity: RunIdentity,
}

impl DirectStoppingObservation {
    pub(in crate::runtime::proxy) fn same_run(&self, run: &RunIdentity) -> bool {
        self.identity.same_run(run)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum ReserveStoppingError {
    Empty,
    WrongRun,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime::proxy) enum TakeRunningError {
    Stopping,
}

impl DirectCoreSlot {
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn replace_instance_for_test(&mut self) {
        self.instance = Arc::new(SlotInstance);
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn mark_running_managed_for_test(&mut self) {
        let SlotState::Running(run) = &mut self.state else {
            panic!("managed origin fixture requires Running custody");
        };
        run.mark_managed_for_test();
    }

    /// Must be called under the real Child mutex before a helper IPC can be
    /// queued. This fence is monotone even when that IPC is cancelled or fails.
    pub(in crate::runtime::proxy) fn touch_helper(&mut self) -> Result<(), SlotAdmissionError> {
        if matches!(self.state, SlotState::Stopping(_)) {
            return Err(SlotAdmissionError::Busy);
        }
        self.backend = BackendFence::HelperTouched;
        Ok(())
    }

    pub(crate) fn touch_helper_for_registry(&mut self) -> Result<(), String> {
        self.touch_helper()
            .map_err(|error| format!("helper mutation rejected by Child custody: {error:?}"))
    }

    pub(in crate::runtime::proxy) fn begin_helper_start(
        &mut self,
    ) -> Result<HelperStartToken, SlotAdmissionError> {
        self.begin_helper_start_with_main(None)
    }

    pub(in crate::runtime::proxy) fn begin_helper_start_with_main(
        &mut self,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<HelperStartToken, SlotAdmissionError> {
        self.touch_helper()?;
        if self.helper_start.is_some() {
            return Err(SlotAdmissionError::Busy);
        }
        let token = HelperStartToken::new();
        self.helper_start = Some(HelperStartAttempt {
            token: token.clone(),
            main_token,
            phase: HelperStartPhase::Inflight,
            stop_inflight: None,
        });
        Ok(token)
    }

    /// Freeze the token from the actual backend custody before Stop takes or
    /// retires that custody. An empty slot provides no authority to release.
    pub(in crate::runtime::proxy) fn main_token_for_stop(
        &self,
        via_helper: bool,
    ) -> Option<crate::runtime::tailscale_login_core::MainBirthToken> {
        if via_helper {
            self.helper_start.as_ref()?.main_token.clone()
        } else {
            match &self.state {
                SlotState::Running(run) => run.main_token.clone(),
                SlotState::Stopping(custody)
                    if matches!(custody.phase, StopPhase::NativeWait { .. }) =>
                {
                    custody.run.main_token.clone()
                }
                SlotState::Stopping(_) | SlotState::Empty => None,
            }
        }
    }

    pub(in crate::runtime::proxy) fn helper_main_claim_for_stop(
        &self,
    ) -> Option<(
        HelperStartToken,
        Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    )> {
        let attempt = self.helper_start.as_ref()?;
        Some((attempt.token.clone(), attempt.main_token.clone()))
    }

    /// Called from the blocking Start worker, including after its async
    /// caller was cancelled. The caller holds Child then pid while publishing.
    pub(in crate::runtime::proxy) fn finish_helper_start(
        &mut self,
        token: &HelperStartToken,
        pid: Option<u32>,
    ) -> bool {
        let Some(attempt) = &mut self.helper_start else {
            return false;
        };
        if !attempt.token.same(token) || !matches!(attempt.phase, HelperStartPhase::Inflight) {
            return false;
        }
        attempt.phase = match pid {
            Some(pid) => HelperStartPhase::Known(HelperStopTarget::Legacy(pid)),
            None => HelperStartPhase::Unconfirmed,
        };
        true
    }

    pub(in crate::runtime::proxy) fn finish_helper_birth_start(
        &mut self,
        token: &HelperStartToken,
        target: HelperBirthTarget,
    ) -> bool {
        let Some(attempt) = &mut self.helper_start else {
            return false;
        };
        if !attempt.token.same(token) || !matches!(attempt.phase, HelperStartPhase::Inflight) {
            return false;
        }
        attempt.phase = HelperStartPhase::Known(HelperStopTarget::Birth(target));
        true
    }

    pub(in crate::runtime::proxy) fn finish_helper_start_not_admitted(
        &mut self,
        token: &HelperStartToken,
        blocker: StartNotAdmitted,
    ) -> bool {
        let Some(attempt) = &mut self.helper_start else {
            return false;
        };
        if !attempt.token.same(token) || !matches!(attempt.phase, HelperStartPhase::Inflight) {
            return false;
        }
        attempt.phase = HelperStartPhase::NotAdmitted(HelperStopTarget::Legacy(blocker.pid()));
        true
    }

    pub(in crate::runtime::proxy) fn finish_helper_birth_not_admitted(
        &mut self,
        token: &HelperStartToken,
        target: Option<HelperBirthTarget>,
    ) -> bool {
        let Some(attempt) = &mut self.helper_start else {
            return false;
        };
        if !attempt.token.same(token) || !matches!(attempt.phase, HelperStartPhase::Inflight) {
            return false;
        }
        attempt.phase = target.map_or(HelperStartPhase::Unconfirmed, |target| {
            HelperStartPhase::NotAdmitted(HelperStopTarget::Birth(target))
        });
        true
    }

    pub(in crate::runtime::proxy) fn helper_start_inflight(
        &self,
        token: &HelperStartToken,
    ) -> bool {
        self.helper_start.as_ref().is_some_and(|attempt| {
            attempt.token.same(token) && matches!(attempt.phase, HelperStartPhase::Inflight)
        })
    }

    pub(in crate::runtime::proxy) fn helper_stop_target(
        &self,
    ) -> Option<(HelperStartToken, HelperStopTarget)> {
        let attempt = self.helper_start.as_ref()?;
        let target = attempt.phase.exact_stop_target()?;
        Some((attempt.token.clone(), target))
    }

    /// Reserve the only helper Stop IPC for this attempt before queuing its
    /// blocking worker. A cancelled async caller cannot admit a second Stop
    /// while the first request could still reach the daemon.
    pub(in crate::runtime::proxy) fn begin_helper_stop(
        &mut self,
    ) -> Option<(HelperStartToken, HelperStopTarget, HelperStopNonce)> {
        let attempt = self.helper_start.as_mut()?;
        let target = attempt.phase.exact_stop_target()?;
        if attempt.stop_inflight.is_some() {
            return None;
        }
        let nonce = HelperStopNonce::new();
        attempt.stop_inflight = Some(HelperStopNonce(Arc::clone(&nonce.0)));
        Some((attempt.token.clone(), target, nonce))
    }

    pub(in crate::runtime::proxy) fn begin_exact_helper_stop(
        &mut self,
        token: &HelperStartToken,
        target: HelperStopTarget,
    ) -> Option<HelperStopNonce> {
        let Some(attempt) = &mut self.helper_start else {
            return None;
        };
        if !attempt.token.same(token)
            || attempt.phase.exact_stop_target() != Some(target)
            || attempt.stop_inflight.is_some()
        {
            return None;
        }
        let nonce = HelperStopNonce::new();
        attempt.stop_inflight = Some(HelperStopNonce(Arc::clone(&nonce.0)));
        Some(nonce)
    }

    pub(in crate::runtime::proxy) fn finish_helper_stop_ipc(
        &mut self,
        token: &HelperStartToken,
        nonce: &HelperStopNonce,
    ) {
        if let Some(attempt) = &mut self.helper_start {
            if attempt.token.same(token)
                && attempt
                    .stop_inflight
                    .as_ref()
                    .is_some_and(|reserved| reserved.same(nonce))
            {
                attempt.stop_inflight = None;
            }
        }
    }

    pub(in crate::runtime::proxy) fn confirm_helper_stop(
        &mut self,
        token: &HelperStartToken,
        target: HelperStopTarget,
        nonce: &HelperStopNonce,
    ) -> bool {
        if !self.helper_stop_matches(token, target, nonce) {
            return false;
        }
        self.helper_start = None;
        true
    }

    pub(in crate::runtime::proxy) fn helper_stop_matches(
        &self,
        token: &HelperStartToken,
        target: HelperStopTarget,
        nonce: &HelperStopNonce,
    ) -> bool {
        self.helper_start.as_ref().is_some_and(|attempt| {
            attempt.token.same(token)
                && attempt.phase.exact_stop_target() == Some(target)
                && attempt
                    .stop_inflight
                    .as_ref()
                    .is_some_and(|reserved| reserved.same(nonce))
        })
    }

    pub(in crate::runtime::proxy) fn helper_pid_bookkeeping_matches(
        &self,
        token: &HelperStartToken,
        target: HelperStopTarget,
        recorded_pid: Option<u32>,
    ) -> bool {
        let Some(attempt) = &self.helper_start else {
            return false;
        };
        if !attempt.token.same(token) {
            return false;
        }
        match attempt.phase {
            HelperStartPhase::Known(known) => known == target && recorded_pid == Some(target.pid()),
            HelperStartPhase::NotAdmitted(blocker) => blocker == target && recorded_pid.is_none(),
            HelperStartPhase::Inflight | HelperStartPhase::Unconfirmed => false,
        }
    }

    pub(in crate::runtime::proxy) fn has_helper_start(&self) -> bool {
        self.helper_start.is_some()
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn helper_stop_inflight_for_test(&self) -> bool {
        self.helper_start
            .as_ref()
            .is_some_and(|attempt| attempt.stop_inflight.is_some())
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn helper_start_not_admitted_for_test(&self) -> bool {
        self.helper_start
            .as_ref()
            .is_some_and(|attempt| matches!(attempt.phase, HelperStartPhase::NotAdmitted(_)))
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn known_helper_start_for_test(
        &mut self,
        pid: u32,
    ) -> HelperStartToken {
        let token = self.begin_helper_start().expect("helper start fixture");
        assert!(self.finish_helper_start(&token, Some(pid)));
        token
    }

    pub(super) fn backend(&self) -> BackendFence {
        self.backend
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn helper_touched_for_test(&self) -> bool {
        self.backend == BackendFence::HelperTouched
    }

    pub(super) fn instance(&self) -> Arc<SlotInstance> {
        Arc::clone(&self.instance)
    }

    pub(super) fn admit_exact_stop(
        &self,
        expected: &RunIdentity,
    ) -> Result<(), SlotAdmissionError> {
        if self.backend != BackendFence::DirectOnly {
            return Err(SlotAdmissionError::HelperTouched);
        }
        match &self.state {
            SlotState::Empty => Err(SlotAdmissionError::Empty),
            SlotState::Running(run) if !run.identity.same_run(expected) => {
                Err(SlotAdmissionError::WrongRun)
            }
            SlotState::Running(run) if !matches!(run.origin, DirectRunOrigin::Legacy) => {
                Err(SlotAdmissionError::ManagedOrigin)
            }
            SlotState::Running(_) => Ok(()),
            SlotState::Stopping(_) => Err(SlotAdmissionError::Busy),
        }
    }

    /// Called only after the bridge has validated this same locked slot and
    /// atomically committed the worker. No fallible operation may follow CAS.
    pub(super) fn commit_armed_unchecked(&mut self, nonce: WorkerNonce) {
        let SlotState::Running(run) = std::mem::replace(&mut self.state, SlotState::Empty) else {
            unreachable!("bridge holds the checked Running slot");
        };
        self.state = SlotState::Stopping(DirectStoppingCustody {
            run,
            phase: StopPhase::Armed { nonce },
        });
    }

    /// Called with the real TS gate and this Child lock held. Registry
    /// compare-remove is the last fallible step; only then is the exact
    /// Reaped proof advanced to a permanent non-Empty local terminal state.
    pub(super) fn close_reaped_birth(
        &mut self,
        instance: &Arc<SlotInstance>,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
        release: impl FnOnce(
            &crate::runtime::tailscale_login_core::MainBirthToken,
        ) -> Result<bool, String>,
    ) -> Result<(), DirectBirthCloseError> {
        if !Arc::ptr_eq(&self.instance, instance) {
            return Err(DirectBirthCloseError::WrongSlot);
        }
        if self.backend != BackendFence::DirectOnly || self.helper_start.is_some() {
            return Err(DirectBirthCloseError::HelperTouched);
        }
        let SlotState::Stopping(custody) = &mut self.state else {
            return Err(DirectBirthCloseError::NotReaped);
        };
        if !custody.run.identity.same_run(expected) || !custody.phase.nonce().same(nonce) {
            return Err(DirectBirthCloseError::WrongRun);
        }
        if !matches!(custody.run.origin, DirectRunOrigin::Legacy) {
            return Err(DirectBirthCloseError::ManagedOrigin);
        }
        let proof = match &custody.phase {
            StopPhase::Reaped(proof)
                if proof.identity.same_run(expected) && proof.nonce.same(nonce) =>
            {
                proof
            }
            StopPhase::BirthClosed(_) => return Err(DirectBirthCloseError::AlreadyClosed),
            StopPhase::Armed { .. }
            | StopPhase::KillRequested { .. }
            | StopPhase::NativeWait { .. }
            | StopPhase::Reaped(_)
            | StopPhase::RetainedFailure { .. } => return Err(DirectBirthCloseError::NotReaped),
        };
        let closed_phase = StopPhase::BirthClosed(DirectChildReaped {
            identity: proof.identity.clone(),
            nonce: proof.nonce.clone(),
            status: proof.status,
        });
        if let Some(token) = &custody.run.main_token {
            if !release(token).map_err(DirectBirthCloseError::Registry)? {
                return Err(DirectBirthCloseError::ClaimMismatch);
            }
        }
        custody.phase = closed_phase;
        custody.run.main_token = None;
        Ok(())
    }

    /// One nonblocking exact attempt, always under the worker's TS→Child
    /// lock order. Neither closure may identify a process by PID.
    pub(super) fn poll_exact_stop<F, G>(
        &mut self,
        instance: &Arc<SlotInstance>,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
        expired: bool,
        try_wait: F,
        start_kill: G,
    ) -> StopPoll
    where
        F: FnOnce(&mut Child) -> io::Result<Option<ExitStatus>>,
        G: FnOnce(&mut Child) -> io::Result<()>,
    {
        if !Arc::ptr_eq(&self.instance, instance) {
            return StopPoll::Obsolete;
        }
        let custody = match &mut self.state {
            SlotState::Running(run) if run.identity.same_run(expected) => {
                return StopPoll::Uncommitted;
            }
            SlotState::Stopping(custody)
                if custody.run.identity.same_run(expected) && custody.phase.nonce().same(nonce) =>
            {
                custody
            }
            SlotState::Empty | SlotState::Running(_) | SlotState::Stopping(_) => {
                return StopPoll::Obsolete;
            }
        };
        let armed = match &custody.phase {
            StopPhase::NativeWait { .. } => return StopPoll::Obsolete,
            StopPhase::Armed { .. } => true,
            StopPhase::KillRequested { .. } => false,
            StopPhase::Reaped(proof) | StopPhase::BirthClosed(proof) => {
                debug_assert!(proof.identity.same_run(expected));
                let _ = proof.status;
                return StopPoll::Reaped;
            }
            StopPhase::RetainedFailure { reason, .. } => {
                let _ = reason;
                return StopPoll::RetainedFailure;
            }
        };
        match custody.run.try_wait_with(try_wait) {
            Ok(Some(status)) => {
                custody.phase = StopPhase::Reaped(DirectChildReaped {
                    identity: expected.clone(),
                    nonce: nonce.clone(),
                    status,
                });
                StopPoll::Reaped
            }
            Err(_) => {
                custody.phase = StopPhase::RetainedFailure {
                    nonce: nonce.clone(),
                    reason: StopFailure::TryWait,
                };
                StopPoll::RetainedFailure
            }
            Ok(None) if expired => {
                custody.phase = StopPhase::RetainedFailure {
                    nonce: nonce.clone(),
                    reason: StopFailure::Timeout,
                };
                StopPoll::RetainedFailure
            }
            Ok(None) if !armed => StopPoll::Pending,
            Ok(None) => match custody.run.signal_with(start_kill) {
                Ok(()) => {
                    custody.phase = StopPhase::KillRequested {
                        nonce: nonce.clone(),
                    };
                    StopPoll::KillRequested
                }
                Err(_) => {
                    custody.phase = StopPhase::RetainedFailure {
                        nonce: nonce.clone(),
                        reason: StopFailure::StartKill,
                    };
                    StopPoll::RetainedFailure
                }
            },
        }
    }

    pub(super) fn view_exact_stop(
        &self,
        instance: &Arc<SlotInstance>,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
    ) -> StopView {
        if !Arc::ptr_eq(&self.instance, instance) {
            return StopView::Obsolete;
        }
        let SlotState::Stopping(custody) = &self.state else {
            return StopView::Obsolete;
        };
        if !custody.run.identity.same_run(expected) || !custody.phase.nonce().same(nonce) {
            return StopView::Obsolete;
        }
        match &custody.phase {
            StopPhase::NativeWait { .. } => StopView::Obsolete,
            StopPhase::Armed { .. } => StopView::Armed,
            StopPhase::KillRequested { .. } => StopView::KillRequested,
            StopPhase::Reaped(_) => StopView::Reaped,
            StopPhase::BirthClosed(_) => StopView::BirthClosed,
            StopPhase::RetainedFailure { .. } => StopView::RetainedFailure,
        }
    }

    pub(in crate::runtime::proxy) fn is_empty(&self) -> bool {
        matches!(self.state, SlotState::Empty)
    }

    pub(in crate::runtime::proxy) fn empty_for_install(&mut self) -> Option<EmptyDirectSlot<'_>> {
        if self.is_empty() {
            Some(EmptyDirectSlot { slot: self })
        } else {
            None
        }
    }

    /// A rejected install returns ownership of the still-live Child to the
    /// caller; neither Running nor Stopping can be overwritten.
    pub(in crate::runtime::proxy) fn install_running(
        &mut self,
        run: DirectCoreRun,
    ) -> Result<(), Box<DirectCoreRun>> {
        if !self.is_empty() {
            return Err(Box::new(run));
        }
        self.state = SlotState::Running(run);
        Ok(())
    }

    /// Ordinary Stop may retry its own retained custody, but never takes an
    /// exact worker's Child or admits two waiters for the same native handle.
    pub(in crate::runtime::proxy) fn begin_native_stop(
        &mut self,
    ) -> Result<Option<(RunIdentity, WorkerNonce)>, String> {
        if self.helper_start.is_some() {
            return Err("direct Stop blocked by helper Start custody".into());
        }
        let nonce = WorkerNonce::new();
        match &mut self.state {
            SlotState::Empty => return Ok(None),
            SlotState::Stopping(custody) => match &mut custody.phase {
                StopPhase::NativeWait {
                    nonce: booked,
                    inflight,
                    ..
                } if !*inflight => {
                    *booked = nonce.clone();
                    *inflight = true;
                    return Ok(Some((custody.run.identity.clone(), nonce)));
                }
                _ => return Err("direct Child is reserved in Stopping custody".into()),
            },
            SlotState::Running(_) => {}
        }
        let SlotState::Running(run) = std::mem::replace(&mut self.state, SlotState::Empty) else {
            unreachable!("the same mutable slot was just checked as Running");
        };
        let identity = run.identity.clone();
        self.state = SlotState::Stopping(DirectStoppingCustody {
            run,
            phase: StopPhase::NativeWait {
                nonce: nonce.clone(),
                inflight: true,
                exit: None,
            },
        });
        Ok(Some((identity, nonce)))
    }

    fn native_stop_custody(
        &mut self,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
    ) -> Result<&mut DirectStoppingCustody, String> {
        let SlotState::Stopping(custody) = &mut self.state else {
            return Err("direct Stop custody changed".into());
        };
        if !custody.run.identity.same_run(expected)
            || !matches!(&custody.phase, StopPhase::NativeWait {
                nonce: booked, inflight: true, ..
            } if booked.same(nonce))
        {
            return Err("direct Stop run or booking changed".into());
        }
        Ok(custody)
    }

    /// Poll one cancellation-safe Tokio Child wait under the short slot lock.
    /// The Child and successful native exit fact outlive the caller's future.
    pub(in crate::runtime::proxy) fn poll_native_wait(
        &mut self,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
        cx: &mut Context<'_>,
        poll: impl FnOnce(&mut Child, &mut Context<'_>) -> Poll<io::Result<ExitStatus>>,
    ) -> Poll<Result<ExitStatus, String>> {
        let custody = match self.native_stop_custody(expected, nonce) {
            Ok(custody) => custody,
            Err(error) => return Poll::Ready(Err(error)),
        };
        let StopPhase::NativeWait { exit, .. } = &mut custody.phase else {
            unreachable!("native booking was just checked");
        };
        if let Some(status) = *exit {
            return Poll::Ready(Ok(status));
        }
        match custody.run.poll_wait_with(cx, poll) {
            Poll::Ready(Ok(status)) => {
                *exit = Some(status);
                Poll::Ready(Ok(status))
            }
            Poll::Ready(Err(error)) => {
                Poll::Ready(Err(format!("direct Child native wait failed: {error}")))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    pub(in crate::runtime::proxy) fn signal_native_stop(
        &mut self,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
        signal: impl FnOnce(&mut Child) -> io::Result<()>,
    ) -> Result<(), String> {
        let custody = self.native_stop_custody(expected, nonce)?;
        if matches!(&custody.phase, StopPhase::NativeWait { exit: Some(_), .. }) {
            return Ok(());
        }
        custody
            .run
            .signal_with(signal)
            .map_err(|error| format!("direct Child stop signal failed: {error}"))
    }

    /// Caller holds TS (when releasing main), then Child→pid. Registry
    /// compare-remove is the last fallible step, before custody is made Empty.
    /// This consumes only a local owned-Child exit, not platform NoOwner proof.
    pub(in crate::runtime::proxy) fn retire_native_stop(
        &mut self,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
        release_main: impl FnOnce(
            &crate::runtime::tailscale_login_core::MainBirthToken,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let custody = self.native_stop_custody(expected, nonce)?;
        if !matches!(&custody.phase, StopPhase::NativeWait { exit: Some(_), .. }) {
            return Err("direct Child exit is not confirmed by native wait".into());
        }
        if let Some(token) = &custody.run.main_token {
            release_main(token)?;
        }
        self.state = SlotState::Empty;
        Ok(())
    }

    /// Cancellation/error relinquishes only this waiter. The Child, main
    /// claim and cached exit stay reserved, so a later Stop can safely retry.
    pub(in crate::runtime::proxy) fn finish_native_stop_booking(
        &mut self,
        expected: &RunIdentity,
        nonce: &WorkerNonce,
    ) {
        if let Ok(custody) = self.native_stop_custody(expected, nonce) {
            if let StopPhase::NativeWait { inflight, .. } = &mut custody.phase {
                *inflight = false;
            }
        }
    }

    /// An Arc-bound transition only. No signal, wait, receipt or release is
    /// authorized by the observation returned here.
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn reserve_stopping_without_worker_for_test(
        &mut self,
        expected: &RunIdentity,
    ) -> Result<DirectStoppingObservation, ReserveStoppingError> {
        let identity = match &self.state {
            SlotState::Empty => return Err(ReserveStoppingError::Empty),
            SlotState::Stopping(_) => return Err(ReserveStoppingError::Busy),
            SlotState::Running(run) if !run.identity.same_run(expected) => {
                return Err(ReserveStoppingError::WrongRun);
            }
            SlotState::Running(run) => run.identity.clone(),
        };
        let SlotState::Running(run) = std::mem::replace(&mut self.state, SlotState::Empty) else {
            unreachable!("the same mutable slot was just checked as Running");
        };
        // Legacy custody test fixture: it cannot be armed by a later worker.
        self.state = SlotState::Stopping(DirectStoppingCustody {
            run,
            phase: StopPhase::RetainedFailure {
                nonce: WorkerNonce::new(),
                reason: StopFailure::UnarmedFixture,
            },
        });
        Ok(DirectStoppingObservation { identity })
    }

    /// The legacy Stop path may only extract a Running Child. A Stopping Child
    /// is reserved for a future exact supervisor and must stay in custody.
    pub(in crate::runtime::proxy) fn take_running_legacy(
        &mut self,
    ) -> Result<Option<DirectCoreRun>, TakeRunningError> {
        match self.state {
            SlotState::Empty => Ok(None),
            SlotState::Stopping(_) => Err(TakeRunningError::Stopping),
            SlotState::Running(_) => {
                let SlotState::Running(run) = std::mem::replace(&mut self.state, SlotState::Empty)
                else {
                    unreachable!("the same mutable slot was just checked as Running");
                };
                Ok(Some(run))
            }
        }
    }

    pub(in crate::runtime::proxy) fn observe_running(
        &mut self,
        expected: &RunIdentity,
    ) -> ChildObservation {
        match &mut self.state {
            SlotState::Running(run) if run.identity.same_run(expected) => {
                match run.try_wait_with(Child::try_wait) {
                    Ok(None) => ChildObservation::Alive,
                    Ok(Some(_)) => ChildObservation::Exited,
                    Err(error) => {
                        log::warn!(
                            "direct native observation unknown; retaining run state: {error}"
                        );
                        ChildObservation::Alive
                    }
                }
            }
            SlotState::Empty | SlotState::Running(_) | SlotState::Stopping(_) => {
                ChildObservation::Absent
            }
        }
    }

    pub(in crate::runtime::proxy) fn is_running_alive(&mut self) -> bool {
        match &mut self.state {
            SlotState::Running(run) => matches!(run.try_wait_with(Child::try_wait), Ok(None)),
            SlotState::Empty | SlotState::Stopping(_) => false,
        }
    }

    pub(in crate::runtime::proxy) fn running_matches(&self, expected: &RunIdentity) -> bool {
        matches!(&self.state, SlotState::Running(run) if run.identity.same_run(expected))
    }

    pub(in crate::runtime::proxy) fn running_exit_proven(
        &mut self,
        expected: &RunIdentity,
    ) -> bool {
        match &mut self.state {
            SlotState::Running(run) if run.identity.same_run(expected) => {
                matches!(run.try_wait_with(Child::try_wait), Ok(Some(_)))
            }
            SlotState::Empty | SlotState::Running(_) | SlotState::Stopping(_) => false,
        }
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn install_running_for_test(&mut self, run: DirectCoreRun) {
        assert!(self.install_running(run).is_ok(), "test slot must be empty");
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn take_running_for_test(&mut self) -> Option<DirectCoreRun> {
        self.take_running_legacy()
            .expect("test cannot extract a Stopping Child")
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn running_for_test(&mut self) -> Option<&mut DirectCoreRun> {
        match &mut self.state {
            SlotState::Running(run) => Some(run),
            SlotState::Empty | SlotState::Stopping(_) => None,
        }
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn is_stopping_for_test(&self) -> bool {
        matches!(&self.state, SlotState::Stopping(_))
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn is_reaped_for_test(&self) -> bool {
        matches!(&self.state, SlotState::Stopping(custody) if matches!(&custody.phase, StopPhase::Reaped(_) | StopPhase::BirthClosed(_)))
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn stopping_child_for_test(&mut self) -> Option<&mut Child> {
        match &mut self.state {
            SlotState::Stopping(custody) => Some(custody.run.child_for_test()),
            SlotState::Empty | SlotState::Running(_) => None,
        }
    }
}

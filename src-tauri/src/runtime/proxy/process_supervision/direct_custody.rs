//! Direct desktop Child custody. Stopping never releases its Child in this
//! slice, including after the exact worker has reaped it.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;

use crate::runtime::proxy::process_supervision::{DirectCoreRun, DirectRunOrigin, RunIdentity};
use polaris_core_supervisor::ChildObservation;
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
#[derive(Clone)]
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
    Known(u32),
    Unconfirmed,
}

#[derive(Clone)]
pub(super) struct WorkerNonce(Arc<()>);

impl WorkerNonce {
    pub(super) fn new() -> Self {
        Self(Arc::new(()))
    }

    fn same(&self, other: &Self) -> bool {
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
    Armed {
        nonce: WorkerNonce,
    },
    KillRequested {
        nonce: WorkerNonce,
    },
    Reaped(DirectChildReaped),
    RetainedFailure {
        nonce: WorkerNonce,
        reason: StopFailure,
    },
}

impl StopPhase {
    fn nonce(&self) -> &WorkerNonce {
        match self {
            Self::Armed { nonce }
            | Self::KillRequested { nonce }
            | Self::RetainedFailure { nonce, .. } => nonce,
            Self::Reaped(proof) => &proof.nonce,
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
    RetainedFailure,
    Obsolete,
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
        self.touch_helper()?;
        if self.helper_start.is_some() {
            return Err(SlotAdmissionError::Busy);
        }
        let token = HelperStartToken::new();
        self.helper_start = Some(HelperStartAttempt {
            token: token.clone(),
            phase: HelperStartPhase::Inflight,
            stop_inflight: None,
        });
        Ok(token)
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
            Some(pid) => HelperStartPhase::Known(pid),
            None => HelperStartPhase::Unconfirmed,
        };
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

    pub(in crate::runtime::proxy) fn helper_stop_target(&self) -> Option<(HelperStartToken, u32)> {
        let attempt = self.helper_start.as_ref()?;
        let HelperStartPhase::Known(pid) = attempt.phase else {
            return None;
        };
        Some((attempt.token.clone(), pid))
    }

    /// Reserve the only helper Stop IPC for this attempt before queuing its
    /// blocking worker. A cancelled async caller cannot admit a second Stop
    /// while the first request could still reach the daemon.
    pub(in crate::runtime::proxy) fn begin_helper_stop(
        &mut self,
    ) -> Option<(HelperStartToken, u32, HelperStopNonce)> {
        let attempt = self.helper_start.as_mut()?;
        let HelperStartPhase::Known(pid) = attempt.phase else {
            return None;
        };
        if attempt.stop_inflight.is_some() {
            return None;
        }
        let nonce = HelperStopNonce::new();
        attempt.stop_inflight = Some(HelperStopNonce(Arc::clone(&nonce.0)));
        Some((attempt.token.clone(), pid, nonce))
    }

    pub(in crate::runtime::proxy) fn begin_exact_helper_stop(
        &mut self,
        token: &HelperStartToken,
        pid: u32,
    ) -> Option<HelperStopNonce> {
        let Some(attempt) = &mut self.helper_start else {
            return None;
        };
        if !attempt.token.same(token)
            || !matches!(attempt.phase, HelperStartPhase::Known(known) if known == pid)
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
        pid: u32,
        nonce: &HelperStopNonce,
    ) -> bool {
        let Some(attempt) = &self.helper_start else {
            return false;
        };
        if !attempt.token.same(token)
            || !matches!(attempt.phase, HelperStartPhase::Known(known) if known == pid)
            || !attempt
                .stop_inflight
                .as_ref()
                .is_some_and(|reserved| reserved.same(nonce))
        {
            return false;
        }
        self.helper_start = None;
        true
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
            StopPhase::Armed { .. } => true,
            StopPhase::KillRequested { .. } => false,
            StopPhase::Reaped(proof) => {
                debug_assert!(proof.identity.same_run(expected));
                let _ = proof.status;
                return StopPoll::Reaped;
            }
            StopPhase::RetainedFailure { reason, .. } => {
                let _ = reason;
                return StopPoll::RetainedFailure;
            }
        };
        match try_wait(&mut custody.run.child) {
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
            Ok(None) => match start_kill(&mut custody.run.child) {
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
            StopPhase::Armed { .. } => StopView::Armed,
            StopPhase::KillRequested { .. } => StopView::KillRequested,
            StopPhase::Reaped(_) => StopView::Reaped,
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
                match run.child.try_wait() {
                    Ok(None) => ChildObservation::Alive,
                    Ok(Some(_)) | Err(_) => ChildObservation::Exited,
                }
            }
            SlotState::Empty | SlotState::Running(_) | SlotState::Stopping(_) => {
                ChildObservation::Absent
            }
        }
    }

    pub(in crate::runtime::proxy) fn is_running_alive(&mut self) -> bool {
        match &mut self.state {
            SlotState::Running(run) => matches!(run.child.try_wait(), Ok(None)),
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
                matches!(run.child.try_wait(), Ok(Some(_)))
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
        matches!(&self.state, SlotState::Stopping(custody) if matches!(&custody.phase, StopPhase::Reaped(_)))
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn stopping_child_for_test(&mut self) -> Option<&mut Child> {
        match &mut self.state {
            SlotState::Stopping(custody) => Some(&mut custody.run.child),
            SlotState::Empty | SlotState::Running(_) => None,
        }
    }
}

//! Persistent custody for the original macOS Child and its post-exit ownership tail.
//!
//! The old PID snapshot/reaper cannot represent failed native waits or an unfinished chown.
//! Keep one original Child here; workers borrow it, and only wait + the entire tail retire it.

use polaris_helper_proto::response::{NativeBirthStart, NativeBirthStatus, NativeBirthStop};
use polaris_helper_proto::HelperBirthTarget;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

const STOP_CACHE_LIMIT: usize = 16;
pub(super) const STOP_GRACE: Duration = Duration::from_secs(5);

/// Operations are made only while this exact Child's lock is held.
pub(super) trait NativeChild: Send {
    fn try_wait(&mut self) -> io::Result<bool>;
    fn terminate(&mut self) -> io::Result<()>;
    fn kill(&mut self) -> io::Result<()>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Stopping,
    NativeExitedTailPending,
    Unknown,
}

struct OwnedChild {
    child: Box<dyn NativeChild>,
    log_custody: Option<polaris_log_budget::PipeLogCustody>,
    phase: Phase,
    native_exited: bool,
    wait_identity_lost: bool,
    term_deadline: Option<Instant>,
    kill_sent: bool,
}

pub(super) struct NativeBirth {
    pub(super) target: HelperBirthTarget,
    owned: Mutex<OwnedChild>,
    // Installed once before the worker starts. Authorization under slot never
    // borrows the child/tail mutex, so exit cannot form slot/owned ABBA.
    core: OnceLock<(Arc<crate::core_store::VerifiedCore>, u32)>,
    stop_requested: AtomicBool,
}

impl NativeBirth {
    pub(super) fn attach_core(&self, core: Arc<crate::core_store::VerifiedCore>, uid: u32) {
        assert!(
            self.core.set((core, uid)).is_ok(),
            "native core identity already attached"
        );
    }
    /// Attach before starting the worker. Registry custody retains the writer capability if
    /// that worker disappears; Poison is retained and can never become a stopped receipt.
    pub(super) fn attach_log_custody(&self, log_custody: polaris_log_budget::PipeLogCustody) {
        self.owned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .log_custody = Some(log_custody);
    }
}

#[derive(Default)]
struct Slot {
    current: Option<Arc<NativeBirth>>,
    stopped: VecDeque<(HelperBirthTarget, Option<u32>)>,
    starting: bool,
    closing: bool,
    // Daemon shutdown is monotonic; upgrade cancellation must never clear it.
    shutting_down: bool,
}

#[derive(Default)]
pub(super) struct NativeCustody {
    slot: Mutex<Slot>,
}

impl NativeCustody {
    /// Seed the existing admission fence before publishing services or opening a listener.
    pub(super) fn with_upgrade_closed(upgrade_closed: bool) -> Self {
        Self {
            slot: Mutex::new(Slot {
                closing: upgrade_closed,
                ..Slot::default()
            }),
        }
    }

    pub(super) fn caller_may_stop(&self, uid: Option<u32>, target: HelperBirthTarget) -> bool {
        let Some(uid) = uid else {
            return false;
        };
        let Ok(slot) = self.slot.lock() else {
            return false;
        };
        if let Some((_, owner)) = slot.stopped.iter().find(|(birth, _)| *birth == target) {
            return uid == 0 || *owner == Some(uid);
        }
        let Some(birth) = &slot.current else {
            return uid == 0;
        };
        uid == 0 || birth.core.get().is_some_and(|(_, owner)| *owner == uid)
    }
    pub(super) fn upgrade_admission(&self, close: bool) -> bool {
        let Ok(mut slot) = self.slot.lock() else {
            return false;
        };
        if slot.shutting_down || slot.current.is_some() || slot.starting {
            return false;
        }
        slot.closing = close;
        true
    }
    pub(super) fn caller_allowed(
        &self,
        uid: Option<u32>,
        id: Option<&polaris_helper_proto::core_manifest::CoreId>,
    ) -> bool {
        let Some(uid) = uid else {
            return false;
        };
        let Ok(slot) = self.slot.lock() else {
            return false;
        };
        if slot.shutting_down || slot.starting || slot.closing {
            return false;
        }
        let Some(current) = &slot.current else {
            return true;
        };
        match current.core.get() {
            Some((core, owner)) => {
                (uid == 0 || uid == *owner) && id.is_none_or(|id| core.manifest.id == id.as_str())
            }
            None => uid == 0 && id.is_none(),
        }
    }
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(super) fn current(&self) -> Option<Arc<NativeBirth>> {
        self.slot().current.clone()
    }

    pub(super) fn is_current(&self, birth: &Arc<NativeBirth>) -> bool {
        self.slot
            .lock()
            .is_ok_and(|slot| slot.current.as_ref().is_some_and(|c| Arc::ptr_eq(c, birth)))
    }

    pub(super) fn active(&self) -> bool {
        self.slot.lock().map_or(true, |slot| {
            slot.current.is_some() || slot.starting || slot.closing || slot.shutting_down
        })
    }

    /// Reserve before forwarding/cache/spawn. Failure does not authorize any write.
    #[cfg(test)]
    pub(super) fn admit(&self) -> Result<(), NativeBirthStart> {
        self.admit_for(None)
    }

    pub(super) fn admit_pinned(
        &self,
        uid: u32,
        id: &polaris_helper_proto::core_manifest::CoreId,
    ) -> Result<(), NativeBirthStart> {
        self.admit_for(Some((uid, id)))
    }

    fn admit_for(
        &self,
        selected: Option<(u32, &polaris_helper_proto::core_manifest::CoreId)>,
    ) -> Result<(), NativeBirthStart> {
        let current = {
            let mut slot =
                self.slot
                    .lock()
                    .map_err(|poison| NativeBirthStart::NotAdmittedUnknown {
                        target: poison.get_ref().current.as_ref().map(|birth| birth.target),
                    })?;
            if slot.shutting_down || slot.closing || slot.starting {
                return Err(NativeBirthStart::NotAdmittedUnknown {
                    target: slot.current.as_ref().map(|b| b.target),
                });
            }
            match slot.current.clone() {
                Some(current) => {
                    if let Some((uid, id)) = selected {
                        if !current.core.get().is_some_and(|(core, owner)| {
                            (uid == 0 || uid == *owner) && core.manifest.id == id.as_str()
                        }) {
                            return Err(NativeBirthStart::NotAdmittedUnknown { target: None });
                        }
                    }
                    current
                }
                None => {
                    slot.starting = true;
                    return Ok(());
                }
            }
        };
        Err(match self.status_of(&current) {
            NativeBirthStatus::Running { target, .. } => NativeBirthStart::Already { target },
            NativeBirthStatus::Stopping { target } => {
                NativeBirthStart::NotAdmittedPending { target }
            }
            _ => NativeBirthStart::NotAdmittedUnknown {
                target: Some(current.target),
            },
        })
    }

    pub(super) fn abort_start(&self) {
        self.slot().starting = false;
    }

    /// Called synchronously immediately after spawn, before handing anything to a worker.
    pub(super) fn publish(
        &self,
        target: HelperBirthTarget,
        child: Box<dyn NativeChild>,
    ) -> Arc<NativeBirth> {
        let mut slot = self.slot();
        let unknown = self.slot.is_poisoned();
        // Only the admitted command can publish; shutdown may close admission meanwhile.
        assert!(slot.starting && slot.current.is_none());
        let birth = Arc::new(NativeBirth {
            target,
            owned: Mutex::new(OwnedChild {
                child,
                log_custody: None,
                phase: if unknown {
                    Phase::Unknown
                } else {
                    Phase::Running
                },
                native_exited: false,
                wait_identity_lost: false,
                term_deadline: None,
                kill_sent: false,
            }),
            stop_requested: AtomicBool::new(slot.closing || slot.shutting_down),
            core: OnceLock::new(),
        });
        slot.current = Some(Arc::clone(&birth));
        slot.starting = false;
        birth
    }

    pub(super) fn status(&self) -> NativeBirthStatus {
        let (current, starting) = {
            let slot = match self.slot.lock() {
                Ok(slot) => slot,
                Err(poison) => {
                    return NativeBirthStatus::Unknown {
                        target: poison.get_ref().current.as_ref().map(|birth| birth.target),
                    };
                }
            };
            (slot.current.clone(), slot.starting)
        };
        match current {
            Some(birth) => self.status_of(&birth),
            None if starting => NativeBirthStatus::Unknown { target: None },
            None => NativeBirthStatus::Empty,
        }
    }

    fn status_of(&self, birth: &NativeBirth) -> NativeBirthStatus {
        match birth.owned.try_lock() {
            Ok(owned) => match owned.phase {
                Phase::Running if !birth.stop_requested.load(Ordering::Acquire) => {
                    NativeBirthStatus::Running {
                        target: birth.target,
                        created: None,
                        image: None,
                    }
                }
                Phase::Unknown => NativeBirthStatus::Unknown {
                    target: Some(birth.target),
                },
                _ => NativeBirthStatus::Stopping {
                    target: birth.target,
                },
            },
            Err(TryLockError::WouldBlock) => NativeBirthStatus::Stopping {
                target: birth.target,
            },
            Err(TryLockError::Poisoned(_)) => NativeBirthStatus::Unknown {
                target: Some(birth.target),
            },
        }
    }

    /// Cache is checked first: an old successful retry cannot touch a new birth.
    pub(super) fn stop(&self, target: HelperBirthTarget) -> NativeBirthStop {
        let current = {
            let slot = match self.slot.lock() {
                Ok(slot) => slot,
                Err(poison) => {
                    if let Some(birth) = poison
                        .get_ref()
                        .current
                        .as_ref()
                        .filter(|b| b.target == target)
                    {
                        birth.stop_requested.store(true, Ordering::Release);
                    }
                    return NativeBirthStop::Unknown { target };
                }
            };
            if slot.stopped.iter().any(|(birth, _)| *birth == target) {
                return NativeBirthStop::Stopped { target };
            }
            slot.current.clone()
        };
        let Some(birth) = current else {
            return NativeBirthStop::Unknown { target };
        };
        if birth.target != target {
            return NativeBirthStop::Mismatch {
                requested: target,
                current: birth.target,
            };
        }
        birth.stop_requested.store(true, Ordering::Release);
        match self.status_of(&birth) {
            NativeBirthStatus::Unknown { .. } => NativeBirthStop::Unknown { target },
            _ => NativeBirthStop::Pending { target },
        }
    }

    pub(super) fn close_admission(&self) {
        let current = {
            let mut slot = self.slot();
            slot.shutting_down = true;
            slot.closing = true;
            slot.current.clone()
        };
        if let Some(birth) = current {
            birth.stop_requested.store(true, Ordering::Release);
        }
    }

    /// Closing alone is insufficient: an admitted spawn may not have published its Child.
    pub(super) fn shutdown_complete(&self) -> Result<bool, String> {
        let slot = self
            .slot
            .lock()
            .map_err(|_| "native custody slot unavailable".to_owned())?;
        Ok(slot.shutting_down && !slot.starting && slot.current.is_none())
    }

    /// One bounded native poll. A tail may block, but the registry still owns the Child and
    /// commands can report Pending without borrowing that lock. No success precedes the tail.
    pub(super) fn drive(
        &self,
        birth: &Arc<NativeBirth>,
        now: Instant,
        tail: &dyn Fn() -> Result<(), String>,
    ) {
        if !self.is_current(birth) {
            return;
        }
        let mut owned = match birth.owned.try_lock() {
            Ok(owned) => owned,
            Err(_) => return,
        };
        if owned.wait_identity_lost {
            return;
        }
        let requested = birth.stop_requested.swap(false, Ordering::AcqRel);
        if requested {
            owned.phase = if owned.native_exited {
                Phase::NativeExitedTailPending
            } else {
                Phase::Stopping
            };
        } else if owned.phase == Phase::Unknown {
            return;
        }
        if !owned.native_exited {
            match owned.child.try_wait() {
                Ok(true) => {
                    owned.native_exited = true;
                    owned.phase = Phase::NativeExitedTailPending;
                }
                Ok(false) => {}
                Err(error) => {
                    #[cfg(unix)]
                    if error.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32) {
                        owned.wait_identity_lost = true;
                    }
                    owned.phase = Phase::Unknown;
                    log::warn!("mac native child wait unconfirmed: {error}");
                    return;
                }
            }
        }
        if !owned.native_exited {
            if owned.phase == Phase::Stopping {
                if owned.term_deadline.is_none() {
                    if let Err(error) = owned.child.terminate() {
                        owned.phase = Phase::Unknown;
                        log::warn!("mac native TERM unconfirmed: {error}");
                        return;
                    }
                    owned.term_deadline = Some(now + STOP_GRACE);
                } else if owned.term_deadline.is_some_and(|deadline| now >= deadline)
                    && !owned.kill_sent
                {
                    if let Err(error) = owned.child.kill() {
                        owned.phase = Phase::Unknown;
                        log::warn!("mac native kill unconfirmed: {error}");
                        return;
                    }
                    owned.kill_sent = true;
                }
            }
            return;
        }
        if let Some(log_custody) = &owned.log_custody {
            if let Err(error) = log_custody.revoke() {
                owned.phase = Phase::Unknown;
                log::warn!("mac native log writer revocation unconfirmed: {error}");
                return;
            }
        }
        if let Err(error) = tail() {
            owned.phase = Phase::Unknown;
            log::warn!("mac native post-exit tail unconfirmed: {error}");
            return;
        }
        // The complete tail and compare/retire are one synchronous commit while the same
        // birth remains locked. No old chown can run after the successor is admitted.
        let mut slot = match self.slot.lock() {
            Ok(slot) => slot,
            Err(_) => {
                owned.phase = Phase::Unknown;
                return;
            }
        };
        if slot.current.as_ref().is_some_and(|c| Arc::ptr_eq(c, birth)) {
            slot.stopped
                .push_back((birth.target, birth.core.get().map(|(_, owner)| *owner)));
            if slot.stopped.len() > STOP_CACHE_LIMIT {
                slot.stopped.pop_front();
            }
            slot.current = None;
        }
    }
}

#[cfg(test)]
mod tests;

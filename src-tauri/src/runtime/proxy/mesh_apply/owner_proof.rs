//! Local direct-native observations. These never attest callbacks, SDKs, OS resources or NoOwner.

use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime::config::StopRuntimeDomain;
use crate::runtime::proxy::process_supervision::direct_custody::{SlotInstance, WorkerNonce};
use crate::runtime::proxy::process_supervision::RunIdentity;

pub(in crate::runtime::proxy) struct ProducerCell {
    domain: Arc<StopRuntimeDomain>,
    queue_base: u64,
    admitted_generation: AtomicU64,
    dispatch_finished: AtomicBool,
    factories: Mutex<Vec<Arc<NativeFactory>>>,
}

impl ProducerCell {
    pub(in crate::runtime::proxy) fn queued(
        domain: Arc<StopRuntimeDomain>,
        base: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            domain,
            queue_base: base,
            admitted_generation: AtomicU64::new(0),
            dispatch_finished: AtomicBool::new(false),
            factories: Mutex::new(Vec::new()),
        })
    }

    pub(in crate::runtime::proxy) fn admitted(&self, generation: u64) {
        self.admitted_generation.store(generation, Ordering::SeqCst);
    }

    pub(in crate::runtime::proxy) fn has_generation(&self, generation: u64) -> bool {
        generation != 0 && self.admitted_generation.load(Ordering::SeqCst) == generation
    }

    pub(in crate::runtime::proxy) fn belongs_to(
        &self,
        domain: &Arc<StopRuntimeDomain>,
        generation: u64,
    ) -> bool {
        Arc::ptr_eq(&self.domain, domain)
            && self.has_generation(generation)
            && !self.dispatch_finished.load(Ordering::SeqCst)
    }

    pub(in crate::runtime::proxy) fn finish_dispatch(&self) {
        // Called only after the producer future returned, never from observer/drop paths.
        self.dispatch_finished.store(true, Ordering::SeqCst);
    }

    pub(in crate::runtime::proxy) fn reclaimable(&self) -> bool {
        if !self.dispatch_finished.load(Ordering::SeqCst) {
            return false;
        }
        self.factories.lock().is_ok_and(|factories| {
            factories
                .iter()
                .all(|factory| matches!(factory.phase.load(Ordering::SeqCst), RETIRED | NO_CHILD))
        })
    }

    pub(in crate::runtime::proxy) fn enter_factory(
        self: &Arc<Self>,
        run: RunIdentity,
        birth_generation: u64,
    ) -> Result<Arc<NativeFactory>, String> {
        let mut factories = self
            .factories
            .lock()
            .map_err(|_| "native producer metadata poisoned")?;
        if self.dispatch_finished.load(Ordering::SeqCst) || !self.has_generation(birth_generation) {
            return Err("native producer is not the admitted continuation".into());
        }
        let factory = Arc::new(NativeFactory {
            producer: Arc::downgrade(self),
            run,
            birth_generation,
            phase: AtomicU8::new(FACTORY_ENTERED),
        });
        factories.push(Arc::clone(&factory));
        Ok(factory)
    }

    pub(in crate::runtime::proxy) fn census(self: &Arc<Self>) -> Result<MainProducerView, String> {
        let factories = self
            .factories
            .lock()
            .map_err(|_| "Main producer metadata poisoned")?
            .clone();
        Ok(MainProducerView {
            producer: Arc::clone(self),
            factories,
        })
    }

    pub(in crate::runtime::proxy) fn has_native_tail(&self) -> bool {
        self.factories.lock().map_or(true, |factories| {
            factories.iter().any(|factory| {
                matches!(
                    factory.phase.load(Ordering::SeqCst),
                    ATTACHED | NATIVE_EXITED | RETIRED
                )
            })
        })
    }

    #[cfg(test)]
    pub(in crate::runtime::proxy) fn observation(&self) -> (u64, u64, bool, Vec<u8>) {
        (
            self.queue_base,
            self.admitted_generation.load(Ordering::SeqCst),
            self.dispatch_finished.load(Ordering::SeqCst),
            self.factories
                .lock()
                .unwrap()
                .iter()
                .map(|factory| factory.phase.load(Ordering::SeqCst))
                .collect(),
        )
    }
}

/// Keeps the original producer and each original factory, including native-only retirement.
pub(in crate::runtime::proxy) struct MainProducerView {
    producer: Arc<ProducerCell>,
    factories: Vec<Arc<NativeFactory>>,
}

impl MainProducerView {
    pub(in crate::runtime::proxy) fn phases(&self) -> Vec<u8> {
        self.factories
            .iter()
            .map(|factory| factory.phase.load(Ordering::SeqCst))
            .collect()
    }

    pub(in crate::runtime::proxy) fn verify_surrender(
        &self,
        slot: &crate::runtime::proxy::process_supervision::direct_custody::DirectCoreSlot,
    ) -> Result<(), String> {
        if !self.producer.dispatch_finished.load(Ordering::SeqCst) {
            return Err("original Main dispatch has not returned".into());
        }
        let current = self
            .producer
            .factories
            .lock()
            .map_err(|_| "Main producer metadata poisoned")?;
        if current.len() != self.factories.len()
            || !current
                .iter()
                .zip(&self.factories)
                .all(|(a, b)| Arc::ptr_eq(a, b))
            || self
                .factories
                .iter()
                .any(|factory| factory.phase.load(Ordering::SeqCst) == FACTORY_ENTERED)
        {
            return Err("original Main factory membership is unresolved".into());
        }
        if self.factories.iter().any(|factory| {
            matches!(
                factory.phase.load(Ordering::SeqCst),
                ATTACHED | NATIVE_EXITED
            ) && !slot.running_matches(&factory.run)
        }) {
            return Err("original Main Child is not back in its physical custody".into());
        }
        Ok(())
    }

    pub(in crate::runtime::proxy) fn queue_base(&self) -> u64 {
        self.producer.queue_base
    }
}

pub(in crate::runtime::proxy) const FACTORY_ENTERED: u8 = 1;
pub(in crate::runtime::proxy) const ATTACHED: u8 = 2;
pub(in crate::runtime::proxy) const NATIVE_EXITED: u8 = 3;
pub(in crate::runtime::proxy) const RETIRED: u8 = 4;
pub(in crate::runtime::proxy) const NO_CHILD: u8 = 5;

// The catalog owns these small records. Their backlink is weak, while an attached
// seal owns the producer strongly: catalog -> factory -weak-> producer, seal -> both.
pub(in crate::runtime::proxy) struct NativeFactory {
    producer: Weak<ProducerCell>,
    run: RunIdentity,
    birth_generation: u64,
    phase: AtomicU8,
}

impl NativeFactory {
    pub(in crate::runtime::proxy) fn returned_no_child(&self) {
        self.phase.store(NO_CHILD, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeRole {
    Main,
    #[cfg(test)]
    WrongRole,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeScope {
    SingleDirectNativeChildV1,
    #[cfg(test)]
    WrongScope,
}

struct DirectOwnerBinding {
    producer: Arc<ProducerCell>,
    factory: Arc<NativeFactory>,
    slot: Arc<SlotInstance>,
    member: Arc<()>,
    main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    role: NativeRole,
    scope: NativeScope,
}

#[derive(Clone)]
pub(in crate::runtime::proxy) struct SealedNativeMembers(Arc<DirectOwnerBinding>);

impl SealedNativeMembers {
    // Sole production caller is EmptyDirectSlot's actual returned-Child attachment.
    pub(in crate::runtime::proxy) fn attached(
        factory: Arc<NativeFactory>,
        slot: Arc<SlotInstance>,
        main_token: Option<crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Self {
        let producer = factory
            .producer
            .upgrade()
            .expect("actual factory retains its producer");
        factory.phase.store(ATTACHED, Ordering::SeqCst);
        Self(Arc::new(DirectOwnerBinding {
            producer,
            factory,
            slot,
            member: Arc::new(()),
            main_token,
            role: NativeRole::Main,
            scope: NativeScope::SingleDirectNativeChildV1,
        }))
    }

    pub(in crate::runtime::proxy) fn observe_exit(
        &self,
        status: ExitStatus,
        nonce: WorkerNonce,
    ) -> NativeExited {
        self.0.factory.phase.store(NATIVE_EXITED, Ordering::SeqCst);
        NativeExited {
            members: self.clone(),
            status,
            first_worker: nonce,
        }
    }

    pub(in crate::runtime::proxy) fn consume_native_exit(
        &self,
        fact: &NativeExited,
        slot: &Arc<SlotInstance>,
        run: &RunIdentity,
        main_token: Option<&crate::runtime::tailscale_login_core::MainBirthToken>,
    ) -> Result<ValidatedNativeExit, String> {
        let binding = &self.0;
        let observed = &fact.members.0;
        if !Arc::ptr_eq(binding, observed)
            || !Arc::ptr_eq(&binding.producer.domain, &observed.producer.domain)
            || binding.producer.domain.boot_id() != observed.producer.domain.boot_id()
            || !Arc::ptr_eq(&binding.slot, slot)
            || !Arc::ptr_eq(&binding.member, &observed.member)
            || !Arc::ptr_eq(&binding.producer, &observed.producer)
            || !binding.factory.run.same_run(run)
            || binding.factory.birth_generation != observed.factory.birth_generation
            || !binding
                .producer
                .has_generation(binding.factory.birth_generation)
            || binding.producer.queue_base > binding.factory.birth_generation
            || !match (&binding.main_token, main_token) {
                (None, None) => true,
                (Some(bound), Some(actual)) => bound.same(actual),
                _ => false,
            }
            || binding.role != NativeRole::Main
            || binding.scope != NativeScope::SingleDirectNativeChildV1
        {
            return Err("native exit does not belong to this sealed direct Child".into());
        }
        // The original worker is provenance; current booking is checked by custody.
        let _provenance = &fact.first_worker;
        Ok(ValidatedNativeExit(self.clone()))
    }
}

#[derive(Clone)]
pub(in crate::runtime::proxy) struct NativeExited {
    members: SealedNativeMembers,
    status: ExitStatus,
    first_worker: WorkerNonce,
}

impl NativeExited {
    pub(in crate::runtime::proxy) fn status(&self) -> ExitStatus {
        self.status
    }
}

pub(in crate::runtime::proxy) struct ValidatedNativeExit(SealedNativeMembers);

/// Private binding prevents an ownerless terminal constructor. It is a local
/// same-Child observation only; this type has no global/action conversion.
pub(in crate::runtime::proxy) struct LocalNativeTerminal {
    _members: SealedNativeMembers,
}

impl LocalNativeTerminal {
    #[cfg(test)]
    pub(in crate::runtime::proxy) fn same_run_for_test(&self, run: &RunIdentity) -> bool {
        self._members.0.factory.run.same_run(run)
    }
}

impl ValidatedNativeExit {
    pub(in crate::runtime::proxy) fn retire(self) -> LocalNativeTerminal {
        // The Arc clone was captured in validation, before registry release.
        // After compare-remove: only an atomic mark and infallible moves.
        self.0 .0.factory.phase.store(RETIRED, Ordering::SeqCst);
        LocalNativeTerminal { _members: self.0 }
    }
}

#[cfg(test)]
mod tests;

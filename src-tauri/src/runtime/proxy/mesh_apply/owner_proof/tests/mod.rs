use super::*;

fn fixture_shape() -> (SealedNativeMembers, RunIdentity, Arc<SlotInstance>) {
    // Negative consumer fixtures only. No synthetic seal grants a positive proof.
    let domain = StopRuntimeDomain::with_boot_for_test(Arc::default(), "fixture-boot");
    let producer = ProducerCell::queued(domain, 7);
    producer.admitted(9);
    let run = RunIdentity::new();
    let factory = producer.enter_factory(run.clone(), 9).unwrap();
    let slot = Arc::new(SlotInstance);
    (
        SealedNativeMembers::attached(factory, Arc::clone(&slot), None),
        run,
        slot,
    )
}

fn status() -> ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[test]
fn strict_native_consumer_rejects_foreign_domain_slot_producer_birth_and_member() {
    let (expected, run, slot) = fixture_shape();
    let (foreign, _, _) = fixture_shape();
    let fact = foreign.observe_exit(status(), WorkerNonce::fixture_for_test());
    assert!(expected
        .consume_native_exit(&fact, &slot, &run, None)
        .is_err());
    assert_eq!(expected.0.factory.phase.load(Ordering::SeqCst), ATTACHED);
    // Identical boot string and matching scalar gen cannot replace opaque domain/cell/birth.
    assert_eq!(
        expected.0.producer.domain.boot_id(),
        foreign.0.producer.domain.boot_id()
    );
    let fact = expected.observe_exit(status(), WorkerNonce::fixture_for_test());
    assert!(expected
        .consume_native_exit(&fact, &Arc::new(SlotInstance), &run, None)
        .is_err());
    assert!(expected
        .consume_native_exit(&fact, &slot, &RunIdentity::new(), None)
        .is_err());
    let mut altered = DirectOwnerBinding {
        producer: Arc::clone(&expected.0.producer),
        factory: Arc::clone(&expected.0.factory),
        slot: Arc::clone(&slot),
        member: Arc::new(()),
        main_token: None,
        role: NativeRole::Main,
        scope: NativeScope::SingleDirectNativeChildV1,
    };
    let foreign_member = NativeExited {
        members: SealedNativeMembers(Arc::new(altered)),
        status: status(),
        first_worker: WorkerNonce::fixture_for_test(),
    };
    assert!(expected
        .consume_native_exit(&foreign_member, &slot, &run, None)
        .is_err());
    altered = DirectOwnerBinding {
        producer: Arc::clone(&foreign.0.producer),
        factory: Arc::clone(&expected.0.factory),
        slot: Arc::clone(&slot),
        member: Arc::clone(&expected.0.member),
        main_token: None,
        role: NativeRole::Main,
        scope: NativeScope::SingleDirectNativeChildV1,
    };
    let foreign_cell = NativeExited {
        members: SealedNativeMembers(Arc::new(altered)),
        status: status(),
        first_worker: WorkerNonce::fixture_for_test(),
    };
    assert!(expected
        .consume_native_exit(&foreign_cell, &slot, &run, None)
        .is_err());
    assert_eq!(
        expected.0.factory.phase.load(Ordering::SeqCst),
        NATIVE_EXITED,
        "validation failure never marks retired"
    );
}

#[test]
fn strict_native_consumer_rejects_wrong_role_scope_and_birth_generation() {
    for fault in ["role", "scope", "generation", "queue-base"] {
        let (mut seal, run, slot) = fixture_shape();
        let binding = Arc::get_mut(&mut seal.0).unwrap();
        match fault {
            "role" => binding.role = NativeRole::WrongRole,
            "scope" => binding.scope = NativeScope::WrongScope,
            "generation" => {
                binding.factory = Arc::new(NativeFactory {
                    producer: Arc::downgrade(&binding.producer),
                    run: run.clone(),
                    birth_generation: 10,
                    phase: AtomicU8::new(ATTACHED),
                })
            }
            "queue-base" => {
                let producer = ProducerCell::queued(Arc::clone(&binding.producer.domain), 10);
                producer.admitted(9);
                binding.producer = producer;
            }
            _ => unreachable!(),
        }
        let fact = seal.observe_exit(status(), WorkerNonce::fixture_for_test());
        assert!(
            seal.consume_native_exit(&fact, &slot, &run, None).is_err(),
            "{fault}"
        );
        assert_eq!(seal.0.factory.phase.load(Ordering::SeqCst), NATIVE_EXITED);
    }
}

#[test]
fn entered_factory_and_poisoned_metadata_cannot_be_reclaimed_on_dispatch_finish() {
    let domain = StopRuntimeDomain::with_boot_for_test(Arc::default(), "fixture-boot");
    let producer = ProducerCell::queued(domain, 7);
    producer.admitted(9);
    let _entered = producer.enter_factory(RunIdentity::new(), 9).unwrap();
    producer.finish_dispatch();
    assert!(
        !producer.reclaimable(),
        "no returned native result means Unknown"
    );
    let poisoned = Arc::clone(&producer);
    std::thread::spawn(move || {
        let _guard = poisoned.factories.lock().unwrap();
        panic!("fixture metadata poison");
    })
    .join()
    .unwrap_err();
    assert!(
        !producer.reclaimable(),
        "metadata errors retain responsibility"
    );
}

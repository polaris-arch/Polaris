use super::*;

#[test]
fn labels_are_closed_and_unknown_names_are_not_emitted() {
    for (name, label) in [
        (
            "platform::macos::exec::tests::system_runner_run_echo",
            TestSpawnLabel::RunEcho,
        ),
        (
            "platform::macos::exec::tests::system_runner_output_echo",
            TestSpawnLabel::OutputEcho,
        ),
        (
            "platform::macos::exec::tests::system_runner_nonzero_exit",
            TestSpawnLabel::NonzeroExit,
        ),
    ] {
        assert_eq!(TestSpawnLabel::from_test_name(Some(name)), label);
    }
    assert_eq!(
        TestSpawnLabel::from_test_name(Some("secret/path/argument")),
        TestSpawnLabel::Other
    );
    assert_eq!(TestSpawnLabel::from_test_name(None), TestSpawnLabel::Other);
}

#[test]
fn operation_context_survives_copy_to_wait_thread() {
    let context = TestSpawnObservation::new();
    let next = TestSpawnObservation::new();
    assert_ne!(context.operation, next.operation);
    assert!(test_spawn_correlation().is_none());
    let observed = std::thread::spawn(move || {
        TEST_SPAWN_OBSERVATION.with(|slot| slot.set(Some(context)));
        let observed = test_spawn_correlation();
        TEST_SPAWN_OBSERVATION.with(|slot| slot.set(None));
        observed
    })
    .join()
    .unwrap();
    assert_eq!(observed, Some((context.operation, "other")));
    assert!(test_spawn_correlation().is_none());
}

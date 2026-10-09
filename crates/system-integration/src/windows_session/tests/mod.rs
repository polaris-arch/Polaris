use super::*;

#[test]
fn cancellation_reopens_os_admission_but_final_exit_stays_fenced() {
    let final_exit = Mutex::new(false);
    assert!(admit_if_open(&final_exit, || true).is_err());
    drop(admit_if_open(&final_exit, || false).unwrap());
    fence_final_exit(&final_exit);
    assert!(admit_if_open(&final_exit, || false).is_err());
}

#[test]
fn final_exit_and_spawn_share_the_lock_and_poison_rejects_spawn() {
    let final_exit = Mutex::new(false);
    let admission = admit_if_open(&final_exit, || false).unwrap();
    assert!(matches!(
        final_exit.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    drop(admission);
    assert!(final_exit.try_lock().is_ok());
    let _ = std::panic::catch_unwind(|| {
        let _admission = admit_if_open(&final_exit, || false).unwrap();
        panic!("injected spawn panic");
    });
    fence_final_exit(&final_exit);
    assert!(admit_if_open(&final_exit, || false).is_err());
}

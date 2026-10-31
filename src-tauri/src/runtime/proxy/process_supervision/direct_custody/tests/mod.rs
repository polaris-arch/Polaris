use super::*;
#[test]
fn recovery_reservation_uses_real_slot_and_pending_birth_state_not_pid_liveness() {
    let mut slot = DirectCoreSlot::default();
    slot.reserve_for_recovery().unwrap();
    assert!(slot.empty_for_install().is_none());
    assert!(slot.touch_helper().is_err());
    assert!(slot.reserve_for_recovery().is_err());
    slot.release_recovery_reservation();
    assert!(slot.empty_for_install().is_some());
    slot.begin_helper_start().unwrap();
    assert!(slot.reserve_for_recovery().is_err());
}

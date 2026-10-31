use super::*;

#[test]
fn recovery_refuses_running_starting_and_unconfirmed_pid() {
    assert_eq!(require_idle(&ProxyStatus::default()), Ok(()));
    for status in [
        ProxyStatus {
            running: true,
            ..Default::default()
        },
        ProxyStatus {
            starting: true,
            ..Default::default()
        },
        ProxyStatus {
            pid: 123,
            ..Default::default()
        },
    ] {
        assert_eq!(require_idle(&status), Err(CoreRecoveryState::StopRequired));
    }
}

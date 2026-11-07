use super::*;
#[test]
fn non_windows_kernel_identity_cannot_settle_windows_check() {
    let key = CheckKey {
        core: CoreId::parse(&"a".repeat(64)).unwrap(),
        correlation: HelperBirthToken::from_bytes([9; 16]),
    };
    assert!(
        windows_helper_check_request(CallerIdentity::Unix(0), &CheckRequest::Release { key })
            .is_err()
    );
}

use super::*;
#[test]
fn versions_and_protocols_never_downgrade_the_shared_helper() {
    assert!(!may_replace("1.1.0", 2, "1.0.0", 2).unwrap());
    assert!(!may_replace("1.0.0", 3, "1.0.0", 2).unwrap());
    assert!(may_replace("1.0.0", 2, "1.0.0", 2).unwrap());
    assert!(may_replace("1.0.0-alpha.2", 2, "1.0.0", 2).unwrap());
    assert!(!may_replace("1.0.0", 2, "1.0.0-alpha.2", 2).unwrap());
    assert!(may_replace("not-a-version", 2, "1.0.0", 2).is_err());
}

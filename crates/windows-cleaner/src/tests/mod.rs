use super::*;
fn ace(kind: u8, flags: u8, mask: u32, sid: &str) -> Ace {
    Ace {
        kind,
        flags,
        mask,
        sid: sid.into(),
    }
}
#[test]
fn roles_are_closed() {
    for (args, expected) in [
        (vec!["--launch"], Some(Role::Launcher)),
        (vec!["--worker"], Some(Role::Worker)),
        (vec![], None),
        (vec!["--worker", "C:\\other"], None),
        (vec!["--service", "Other"], None),
        (vec!["--worker", "--pid=1"], None),
    ] {
        assert_eq!(role(args.into_iter().map(Into::into)), expected);
    }
}
#[test]
fn protected_owner_and_every_effective_grant_are_checked() {
    let trusted = [ace(0, 0, WRITE_MASK, SYSTEM), ace(0, 0, WRITE_MASK, ADMINS)];
    assert!(trusted_security(ADMINS, Some(&trusted), None, false));
    assert!(!trusted_security("user", Some(&trusted), None, false));
    assert!(!trusted_security(ADMINS, None, None, false));
    for bit in [
        2, 4, 16, 64, 256, 0x10000, 0x40000, 0x80000, 0x10000000, 0x40000000,
    ] {
        assert!(!trusted_security(
            ADMINS,
            Some(&[ace(0, 0, bit, "user")]),
            None,
            false
        ));
    }
    assert!(trusted_security(
        ADMINS,
        Some(&[ace(0, 0, 0x1200a9, "user")]),
        None,
        false
    ));
    assert!(trusted_security(
        ADMINS,
        Some(&[ace(0, 8, WRITE_MASK, "user")]),
        None,
        false
    ));
    assert!(trusted_security(
        ADMINS,
        Some(&[ace(1, 0, WRITE_MASK, "user")]),
        None,
        false
    ));
    assert!(!trusted_security(
        ADMINS,
        Some(&[ace(9, 0, 0, "user")]),
        None,
        false
    ));
}
#[test]
fn source_user_is_permitted_only_in_launch_policy() {
    let source = [ace(0, 0, WRITE_MASK, "caller")];
    assert!(trusted_security(
        "caller",
        Some(&source),
        Some("caller"),
        false
    ));
    assert!(!trusted_security("caller", Some(&source), None, false));
    assert!(trusted_security(
        SYSTEM,
        Some(&[ace(0, 0, 4, "user")]),
        None,
        true
    ));
    assert!(!trusted_security(
        SYSTEM,
        Some(&[ace(0, 0, 64, "user")]),
        None,
        true
    ));
}
#[test]
fn unknown_exit_does_not_become_success() {
    assert_eq!(Outcome::from_code(0), Outcome::Success);
    assert_eq!(Outcome::from_code(3), Outcome::Partial);
    assert_eq!(Outcome::from_code(20), Outcome::Cancelled);
    assert_eq!(Outcome::from_code(259), Outcome::LaunchFailed);
}

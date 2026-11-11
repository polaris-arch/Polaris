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
        (
            vec!["--sweep-installed-cores"],
            Some(Role::SweepInstalledCores),
        ),
        (vec!["--sweep-installed-cores", "--pid", "1"], None),
        (vec!["--install-worker"], None),
        (vec!["--install-worker", "--command", "any"], None),
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
fn shared_ancestor_creation_and_attributes_do_not_grant_leaf_mutation() {
    // C's ProgramData projection: Everyone 0012019F / Users 00000116.
    let grants = [
        ace(0, 0, 0x0012_019f, "S-1-1-0"),
        ace(0, 0, 0x116, "S-1-5-32-545"),
    ];
    assert!(trusted_security(SYSTEM, Some(&grants), None, true));
    assert!(!trusted_security(SYSTEM, Some(&grants), None, false));
    for bit in [0x40, 0x10000, 0x40000, 0x80000, 0x10000000, 0x40000000] {
        assert!(!trusted_security(
            SYSTEM,
            Some(&[ace(0, 0, bit, "foreign")]),
            None,
            true
        ));
    }
}

#[test]
fn appcontainer_intersection_never_trusts_a_foreign_primary_grant_or_owner() {
    for sid in [
        "S-1-15-2-1",
        "S-1-15-2-2",
        "S-1-15-2-10-20-30-40-50-60-70",
        "S-1-15-3-1",
    ] {
        let capability = ace(0, 0, 0x001f_01ff, sid);
        let grants = [ace(0, 0, WRITE_MASK, "caller"), capability];
        assert!(trusted_security(
            "caller",
            Some(&grants),
            Some("caller"),
            true
        ));
        assert!(trusted_security(
            "caller",
            Some(&grants),
            Some("caller"),
            false
        ));
        assert!(!trusted_security("caller", Some(&grants), None, false));
        assert!(!trusted_security(sid, Some(&grants), Some("caller"), true));
        for ancestor in [false, true] {
            for bit in [0x40, 0x10000, 0x40000, 0x80000, 0x10000000] {
                let foreign = [ace(0, 0, 0x001f_01ff, sid), ace(0, 0, bit, "foreign")];
                assert!(!trusted_security(SYSTEM, Some(&foreign), None, ancestor));
            }
        }
        for bit in [2, 4, 16, 256] {
            let foreign = [ace(0, 0, 0x001f_01ff, sid), ace(0, 0, bit, "foreign")];
            assert!(!trusted_security(SYSTEM, Some(&foreign), None, false));
        }
    }
    for sid in [
        "S-1-15",
        "S-1-15-4-1",
        "S-1-15-3",
        "S-1-15-3-",
        "S-1-15-3-+1",
        "S-1-15-3-4294967296",
    ] {
        assert!(!trusted_security(
            SYSTEM,
            Some(&[ace(0, 0, WRITE_MASK, sid)]),
            None,
            true
        ));
    }
}
#[test]
fn unknown_exit_does_not_become_success() {
    assert_eq!(Outcome::from_code(0), Outcome::Success);
    assert_eq!(Outcome::from_code(3), Outcome::Partial);
    assert_eq!(Outcome::from_code(20), Outcome::Cancelled);
    assert_eq!(Outcome::from_code(259), Outcome::LaunchFailed);
}

#[cfg(windows)]
#[test]
fn install_role_requires_exact_data_file() {
    let args = [
        "--install-worker",
        "--request",
        r"C:\Users\user\install-request.json",
    ];
    assert!(matches!(
        role(args.into_iter().map(Into::into)),
        Some(Role::InstallWorker(_))
    ));
    for path in ["relative/install-request.json", r"C:\x\different.json"] {
        assert_eq!(
            role(
                ["--install-worker", "--request", path]
                    .into_iter()
                    .map(Into::into)
            ),
            None
        );
    }
}

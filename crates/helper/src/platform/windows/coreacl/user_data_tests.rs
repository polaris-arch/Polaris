use super::*;
const USER: &str = "S-1-5-21-101-202-303-1001";
fn data() -> ObjectSecurity {
    ObjectSecurity {
        owner_sid: Some(USER.into()),
        dacl: Some(vec![Ace {
            sid: USER.into(),
            mask: 0x001f01ff,
            kind: AceKind::Allow,
            flags: 3,
        }]),
    }
}
#[test]
fn data_scope_does_not_weaken_privileged_policy() {
    assert!(judge_current_user_data(&data(), USER, true, false).is_ok());
    assert!(!judge_object("data", &data()).is_empty());
    assert!(judge_current_user_data(&data(), "", true, false).is_err());
    assert!(judge_current_user_data(&data(), "S-1-5-21-1-2-3-9999", true, false).is_err());
}
#[test]
fn data_rejects_unsafe_grants_and_unknown_security_but_accepts_minimal_reads() {
    for mask in [
        2, 4, 0x10, 0x40, 0x100, 0x10000, 0x40000, 0x80000, 0x40000000, 0x10000000,
    ] {
        let mut object = data();
        object.dacl.as_mut().unwrap().push(Ace {
            sid: "S-1-1-0".into(),
            mask,
            kind: AceKind::Allow,
            flags: 0,
        });
        assert!(
            judge_current_user_data(&object, USER, true, false).is_err(),
            "mask={mask:x}"
        );
    }
    let mut object = data();
    object.dacl.as_mut().unwrap().push(Ace {
        sid: "S-1-1-0".into(),
        mask: 0x120089,
        kind: AceKind::Allow,
        flags: 0,
    });
    assert!(judge_current_user_data(&object, USER, false, false).is_ok());
    object.dacl.as_mut().unwrap().push(Ace::unreadable());
    assert_eq!(
        judge_current_user_data(&object, USER, true, false),
        Err(UserDataAclError::UnsupportedAce)
    );
    object.dacl = None;
    assert!(judge_current_user_data(&object, USER, true, false).is_err());
    object.owner_sid = None;
    assert_eq!(
        judge_current_user_data(&object, USER, true, false),
        Err(UserDataAclError::Unavailable)
    );
}
#[test]
fn unrelated_name_creation_is_not_child_replacement_authority() {
    for mask in [2, 4] {
        let mut object = data();
        object.dacl.as_mut().unwrap().push(Ace {
            sid: "S-1-1-0".into(),
            mask,
            kind: AceKind::Allow,
            flags: 0,
        });
        assert!(judge_current_user_data(&object, USER, true, true).is_ok());
        assert!(judge_current_user_data(&object, USER, true, false).is_err());
    }
}

#[test]
fn only_native_system_volume_classification_accepts_exact_installer_owner() {
    let mut object = data();
    object.owner_sid = Some(TRUSTED_INSTALLER_SID.into());
    assert!(judge_system_volume_data_ancestor(&object, USER).is_ok());
    assert!(judge_current_user_data(&object, USER, true, true).is_err());
    assert!(judge_current_user_data(&object, USER, true, false).is_err());
    assert!(!judge_object("data", &object).is_empty());
    for owner in ["S-1-5-80-1-2-3-4-5", "S-1-5-21-101-202-303-9999"] {
        object.owner_sid = Some(owner.into());
        assert!(judge_system_volume_data_ancestor(&object, USER).is_err());
    }
    object.owner_sid = None;
    assert_eq!(
        judge_system_volume_data_ancestor(&object, USER),
        Err(UserDataAclError::Unavailable)
    );
}

#[test]
fn installer_owner_does_not_authorize_dangerous_aces_or_extra_writers() {
    for sid in ["S-1-1-0", TRUSTED_INSTALLER_SID, "S-1-15-3-1234"] {
        for mask in [0x40, 0x10000, 0x40000, 0x80000, 0x40000000, 0x10000000] {
            let mut object = data();
            object.owner_sid = Some(TRUSTED_INSTALLER_SID.into());
            object.dacl.as_mut().unwrap().push(Ace {
                sid: sid.into(),
                mask,
                kind: AceKind::Allow,
                flags: 0,
            });
            assert!(
                judge_system_volume_data_ancestor(&object, USER).is_err(),
                "sid={sid} mask={mask:x}"
            );
        }
    }
}

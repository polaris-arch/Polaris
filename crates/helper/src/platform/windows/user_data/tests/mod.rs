use super::*;
fn private_profile_fixture() -> (ProcessIdentity, PathBuf, PathBuf) {
    #[link(name = "userenv")]
    unsafe extern "system" {
        fn GetUserProfileDirectoryW(token: HANDLE, path: *mut u16, size: *mut u32) -> i32;
    }
    use std::os::windows::ffi::OsStringExt;
    let process = ProcessIdentity::capture().unwrap();
    let mut profile = vec![0u16; 32768];
    let mut size = profile.len() as u32;
    // SAFETY: actual held primary token; bounded writable native output.
    assert_ne!(
        unsafe {
            GetUserProfileDirectoryW(
                process._token.as_raw_handle(),
                profile.as_mut_ptr(),
                &mut size,
            )
        },
        0
    );
    let end = profile.iter().position(|c| *c == 0).unwrap();
    let profile = PathBuf::from(std::ffi::OsString::from_wide(&profile[..end]));
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = profile.join(format!(
        "polaris-windows-data-native-{}-{nonce}",
        std::process::id()
    ));
    create_private_directory(&fixture, &process.sid).unwrap();
    (process, profile, fixture)
}

#[test]
fn new_data_producer_uses_token_user_and_never_reowns_existing_objects() {
    use std::io::Write;
    let (process, _, fixture) = private_profile_fixture();
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0;
    // SAFETY: query only the actual held primary token into its exact native ABI.
    assert_ne!(
        unsafe {
            GetTokenInformation(
                process._token.as_raw_handle(),
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            )
        },
        0
    );
    assert_eq!(size as usize, std::mem::size_of::<TOKEN_ELEVATION>());
    let directory = fixture.join("new-source");
    crate::core_store::create_current_user_data_directory(&directory).unwrap();
    let file_path = directory.join("new-member");
    let mut file = crate::core_store::create_current_user_data_file(&file_path).unwrap();
    file.write_all(b"retained inert source").unwrap();
    file.sync_all().unwrap();
    assert_eq!(winproc::file_owner(&file).unwrap(), process.sid);
    let id = identity(&file).unwrap();
    assert_eq!(
        crate::core_store::create_current_user_data_file(&file_path)
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        crate::core_store::create_current_user_data_directory(&directory)
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(identity(&file).unwrap(), id);
    assert_eq!(std::fs::read(&file_path).unwrap(), b"retained inert source");
    let default = directory.join("native-default-owner");
    std::fs::write(&default, b"do not reown this existing object").unwrap();
    let default = open(&default, false, false).unwrap();
    let before = security(&default).unwrap();
    assert!(crate::core_store::create_current_user_data_file(
        &directory.join("native-default-owner")
    )
    .is_err());
    assert_eq!(security(&default).unwrap(), before);
    let scope = CurrentUserData::open(&directory).unwrap();
    assert!(scope
        .create_file(&fixture.join("outside-source"), false)
        .is_err());
    assert!(!fixture.join("outside-source").exists());
    assert!(std::fs::rename(&directory, fixture.join("replaced-source")).is_err());
    let bounded = scope
        .create_file(&scope.root().join("bounded-member"), false)
        .unwrap();
    assert_eq!(winproc::file_owner(&bounded).unwrap(), process.sid);
    println!("NATIVE_DATA_CREATION TokenUser={} elevated={} default_owner={:?} explicit_owner={:?} existing_identity_retained=true", process.sid, elevation.TokenIsElevated != 0, before.owner_sid, security(&file).unwrap().owner_sid);
}
#[test]
fn native_unreadable_descriptor_stays_unknown_without_ordinary_path_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("inert-data");
    std::fs::write(&path, b"inert").unwrap();
    // Deliberately omit READ_CONTROL on this fresh fixture's actual handle.
    let held = OpenOptions::new()
        .read(true)
        .access_mode(FILE_READ_ATTRIBUTES)
        .open(&path)
        .unwrap();
    assert_eq!(security(&held).unwrap_err().kind(), io::ErrorKind::Other);
}
#[test]
fn native_null_dacl_is_rejected_on_the_actual_held_object() {
    let token = ProcessIdentity::capture().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("inert-data");
    std::fs::write(&path, b"inert").unwrap();
    let held = OpenOptions::new()
        .read(true)
        .access_mode(0x0002_0000 | 0x0004_0000 | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    // SAFETY: only this fresh inert fixture's held handle is changed.
    let status = unsafe {
        SetSecurityInfo(
            held.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, 0);
    let actual = winproc::file_security(&held).unwrap();
    assert!(actual.dacl.is_none());
    assert_eq!(
        coreacl::judge_current_user_data(&actual, &token.sid, false, false),
        Err(coreacl::UserDataAclError::Untrusted)
    );
}
#[test]
fn native_token_owned_data_acl_is_separate_from_system_admin_store() {
    let token = ProcessIdentity::capture().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private-data");
    create_private_directory(&path, &token.sid).unwrap();
    let held = open(&path, true, true).unwrap();
    let sec = winproc::file_security(&held).unwrap();
    assert_eq!(sec.owner_sid.as_deref(), Some(token.sid.as_str()));
    assert!(coreacl::judge_current_user_data(&sec, &token.sid, true, false).is_ok());
    assert!(!coreacl::judge_object("user-data", &sec).is_empty());
    assert!(require_ntfs(&held).is_ok());
    assert!(final_path(&held).unwrap().is_absolute());
    assert!(std::fs::rename(&path, temp.path().join("moved")).is_err());
}
#[test]
fn native_hardlink_and_reparse_are_not_regular_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("member");
    std::fs::write(&path, b"data-only").unwrap();
    let held = open(&path, false, false).unwrap();
    let id = identity(&held).unwrap();
    std::fs::hard_link(&path, temp.path().join("alias")).unwrap();
    assert!(open(&path, false, false).is_err());
    assert_eq!(identity(&held).unwrap(), id);
    let link = temp.path().join("symlink");
    match std::os::windows::fs::symlink_file(&path, &link) {
        Ok(()) => assert!(open(&link, false, false).is_err()),
        Err(e) => {
            assert_eq!(e.raw_os_error(), Some(1314));
            eprintln!("reparse fixture unsupported: privilege not held; no system change");
        }
    }
}

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
fn native_profile_namespace_and_actual_root_parent_barrier_facts() {
    let (process, profile, fixture) = private_profile_fixture();
    let root = fixture.join("selected-core-recovery");
    create_private_directory(&root, &process.sid).unwrap();
    let mut ancestors = Vec::new();
    for path in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let held = open(path, true, true).unwrap();
        let sec = security(&held).unwrap();
        let system_volume_root = sec.owner_sid.as_deref() == Some(coreacl::TRUSTED_INSTALLER_SID)
            && is_native_system_volume_root(&held).unwrap();
        ancestors.push(serde_json::json!({"path":path,"owner":sec.owner_sid,
            "native_id":format!("{:?}",identity(&held).unwrap()),
            "ordinary_data_policy":format!("{:?}",coreacl::judge_current_user_data(&sec,&process.sid,true,path!=root)),
            "native_system_volume_root":system_volume_root,
            "effective_namespace_policy":format!("{:?}",judge_anchor(&sec,&process.sid,path!=root,system_volume_root)),
            "allow_aces":sec.dacl.as_ref().map(|a|a.iter().map(|v|format!("{:?}:{}:{:#x}:{:#x}",v.kind,v.sid,v.mask,v.flags)).collect::<Vec<_>>())}));
    }
    let namespace = CurrentUserData::open(&root, false);
    let namespace_result = match &namespace {
        Ok(scope) => {
            scope.guard().unwrap();
            "accepted".to_string()
        }
        Err(e) => format!("{:?}: {e}", e.kind()),
    };
    let mut barriers = Vec::new();
    for (role, path) in [("root", &root), ("immediate_parent", &fixture)] {
        let file = open(path, true, true).unwrap();
        let sec = security(&file).unwrap();
        assert!(coreacl::judge_current_user_data(&sec, &process.sid, true, false).is_ok());
        assert!(!coreacl::judge_object("user data", &sec).is_empty());
        require_ntfs(&file).unwrap();
        assert!(!is_native_system_volume_root(&file).unwrap());
        let anchor = Anchor {
            id: identity(&file).unwrap(),
            path: final_path(&file).unwrap(),
            file,
            ancestor: false,
            system_volume_root: false,
        };
        let mut row = serde_json::json!({"role":role,"path":path,"guid":anchor.path,"id":format!("{:?}",anchor.id),"local_data_acl":"accepted","system_admin_policy":"rejected","desired_access":"GENERIC_READ|GENERIC_WRITE (includes READ_CONTROL)","flags":"BACKUP_SEMANTICS|OPEN_REPARSE_POINT","share":"READ|WRITE"});
        match open_barrier_anchor(&anchor) {
            Err(e) => {
                row["phase"] = "CreateFileW".into();
                row["last_error"] = serde_json::json!(e.raw_os_error());
                row["error"] = e.to_string().into();
                row["FlushFileBuffers"] = "NOT_RUN: no write handle".into();
            }
            Ok(write) => {
                row["held_id_equal"] = serde_json::json!(identity(&write).unwrap() == anchor.id);
                row["phase"] = "FlushFileBuffers".into();
                match flush_barrier_file(&write) {
                    Ok(()) => row["result"] = "success".into(),
                    Err(e) => {
                        let raw = e.raw_os_error().or_else(|| {
                            e.get_ref()
                                .and_then(|s| s.downcast_ref::<io::Error>())
                                .and_then(io::Error::raw_os_error)
                        });
                        row["last_error"] = serde_json::json!(raw);
                        row["kind"] = format!("{:?}", e.kind()).into();
                        row["error"] = e.to_string().into();
                    }
                }
            }
        }
        barriers.push(row);
    }
    let initialize = crate::core_store::CoreStore::initialize_current_user_data(&root);
    let initialize = match initialize {
        Ok(_) => "accepted".to_string(),
        Err(e) => format!("{:?}: {e}", e.kind()),
    };
    println!(
        "PROFILE_NATIVE_FACTS {}",
        serde_json::json!({"profile_from_primary_token":profile,"primary_sid":process.sid,"fixture":fixture,"ancestry":ancestors,"actual_scope":namespace_result,"production_CoreStore_initialize":initialize,"same_production_callee_barriers":barriers,"primitive_evidence_does_not_waive_namespace":true,"fixture_retained_for_inspection":true})
    );
}

#[test]
fn native_publication_preserves_read_lease_and_flushes_only_changed_data_parents() {
    use crate::core_store::{CoreStore, InstallOutcome};
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest, InstalledCoreManifest};
    let (process, _, fixture) = private_profile_fixture();
    let source = fixture.join("inert-input");
    create_private_directory(&source, &process.sid).unwrap();
    // Inert bytes test platform I/O/leases only; never App admission or a
    // trusted carrier. The separate App test uses actual compiled resources.
    let files = [
        ("sing-box.exe", b"inert core data".as_slice()),
        ("libcronet.dll", b"inert sidecar data".as_slice()),
    ]
    .into_iter()
    .map(|(name, bytes)| {
        use std::io::Write;
        let mut file =
            crate::core_store::create_current_user_data_file(&source.join(name)).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        assert_eq!(winproc::file_owner(&file).unwrap(), process.sid);
        CoreFile {
            name: name.into(),
            bytes: bytes.len() as u64,
            sha256: crate::core_hash::sha256_hex(bytes),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "win".into(),
        version: "1.2.3".into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: Some("b".repeat(64)),
        cleaner_sha256: Some("c".repeat(64)),
    };
    let id = core.id(crate::core_hash::sha256_hex).unwrap();
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.as_str().into(),
        app_version: "1.0.0".into(),
        core,
    };
    let root = fixture.join("store");
    let store = CoreStore::initialize_current_user_data(&root).unwrap();
    let first = store.install(&manifest, &source, None, None);
    println!(
        "NATIVE_DATA_PUBLICATION first={first:?} fixture={}",
        fixture.display()
    );
    assert_eq!(first.unwrap(), InstallOutcome::Installed);
    let held = store.selected_data(&id).unwrap();
    // Every publication object is made by the same data producer, including
    // store children, payloads, locks and the durable manifest receipt.
    for path in [
        root.join("versions"),
        root.join("manifests"),
        root.join(".staging"),
        root.join(".trash"),
        root.join("leases"),
        root.join(".core-store.lock"),
        root.join("leases").join(format!("{}.lock", id.as_str())),
        root.join("manifests").join(format!("{}.json", id.as_str())),
        held.directory.clone(),
        held.directory.join("sing-box.exe"),
        held.directory.join("libcronet.dll"),
    ] {
        let file = open(
            &path,
            std::fs::symlink_metadata(&path).unwrap().is_dir(),
            false,
        )
        .unwrap();
        assert_eq!(
            winproc::file_owner(&file).unwrap(),
            process.sid,
            "{}",
            path.display()
        );
    }
    assert!(held.data_is_current());
    // Closing the publication's separate writable flush handle must not
    // release this independently held read-only shared occupancy lease.
    assert_eq!(
        store.install(&manifest, &source, None, None).unwrap(),
        InstallOutcome::AlreadyPresent
    );
    let binary = held.directory.join("sing-box.exe");
    std::fs::write(&binary, b"damaged").unwrap();
    assert!(!held.data_is_current());
    assert!(store.install(&manifest, &source, None, None).is_err());
    drop(held);
    let repaired = store.install(&manifest, &source, None, None);
    println!("NATIVE_DATA_PUBLICATION repair={repaired:?}");
    assert_eq!(repaired.unwrap(), InstallOutcome::Installed);
    assert!(store.selected_data(&id).unwrap().data_is_current());
    // A real partial receipt must stay damage, never absence or fallback.
    let receipt = root.join("manifests").join(format!("{}.json", id.as_str()));
    let retained = fixture.join("retained-receipt.json");
    std::fs::rename(&receipt, &retained).unwrap();
    assert!(store.selected_data(&id).is_err());
    std::fs::rename(&retained, &receipt).unwrap();
    let unknown = root
        .join("versions")
        .join(id.as_str())
        .join("retain-unknown");
    std::fs::write(&unknown, b"retain unknown member").unwrap();
    assert!(store.selected_data(&id).is_err());
    assert!(store.install(&manifest, &source, None, None).is_err());
    assert_eq!(std::fs::read(&unknown).unwrap(), b"retain unknown member");
    assert_eq!(
        store.verify_for_start(&id).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    println!("NATIVE_DATA_PUBLICATION closure=PASS inert_data_only=true fixture_retained=true");
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
    let scope = CurrentUserData::open(&directory, false).unwrap();
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
#[test]
fn native_namespace_and_barrier_capability_is_reported_without_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("selected-data");
    match CurrentUserData::open(&path, true) {
        Ok(scope) => {
            scope.guard().unwrap();
            scope.check(scope.root()).unwrap();
            match scope.barrier() {
                Ok(()) => eprintln!("native-data-scope and directory-barrier supported"),
                Err(e) => {
                    assert!(matches!(
                        e.kind(),
                        io::ErrorKind::Unsupported
                            | io::ErrorKind::PermissionDenied
                            | io::ErrorKind::Other
                    ));
                    eprintln!("native-data-scope barrier unavailable: {e}");
                }
            }
            assert!(std::fs::rename(temp.path(), temp.path().with_extension("swapped")).is_err());
        }
        Err(e) => {
            assert!(matches!(
                e.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported | io::ErrorKind::Other
            ));
            eprintln!("native-data-scope unavailable under this exact temporary ancestry: {e}");
        }
    }
}

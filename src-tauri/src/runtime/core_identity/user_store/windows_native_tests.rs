//! Real compiled recovery carrier and current primary-token profile. Data only;
//! no synthetic execution policy, global app configuration or system changes.
use super::*;
use std::sync::atomic::AtomicBool;

#[allow(
    unsafe_code,
    reason = "bounded primary-token KnownFolder query in native regression"
)]
fn profile() -> PathBuf {
    use std::os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    }
    #[link(name = "userenv")]
    unsafe extern "system" {
        fn GetUserProfileDirectoryW(token: Handle, path: *mut u16, size: *mut u32) -> i32;
    }
    let mut token = std::ptr::null_mut();
    // SAFETY: valid current process pseudo-handle and exact output ABI.
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), 8, &mut token) },
        0
    );
    // SAFETY: successful OpenProcessToken transfers ownership of this handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: held primary token and bounded writable output.
    assert_ne!(
        unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), buffer.as_mut_ptr(), &mut size) },
        0
    );
    let end = buffer.iter().position(|v| *v == 0).unwrap();
    PathBuf::from(std::ffi::OsString::from_wide(&buffer[..end]))
}

#[test]
fn normal_compiled_stage_persists_repairs_under_real_profile_without_execution_authority() {
    let config = profile().join(format!(
        "polaris-windows-user-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    // This real production constructor creates a protected directory at birth.
    // Its own store children are just unrelated fixture siblings; no fake scope
    // or handcrafted root proof is supplied to the actual App consumer below.
    let private_fixture = CoreStore::initialize_current_user_data(&config).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("resources/win/polaris-helper.exe");
    let selected = super::super::selected_manifest("win").unwrap();
    let stage =
        super::super::stage_recovery_installation(&helper, &config, &AtomicBool::new(false))
            .unwrap();
    assert_eq!(stage.manifest.core, selected);
    assert!(selected_at(&config, &selected).unwrap().is_none());
    assert_eq!(
        install(&config, &stage).unwrap(),
        UserRecoveryState::Installed
    );
    let first = selected_at(&config, &selected).unwrap().unwrap();
    assert!(first.data_is_current());
    let directory = first.directory.clone();
    let id = selected.id(polaris_updater::sha256_hex).unwrap();
    let store = CoreStore::open_current_user_data(&config.join(USER_STORE)).unwrap();
    assert_eq!(
        store.verify_for_start(&id).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    drop(store);
    assert_eq!(
        install(&config, &stage).unwrap(),
        UserRecoveryState::AlreadyPresent
    );
    let reopened = selected_at(&config, &selected).unwrap().unwrap();
    assert_eq!(reopened.directory, directory);
    assert!(reopened.data_is_current());
    drop(reopened);
    let held = first; // Held across idempotent publication and its RW flush.
    let binary = directory.join("sing-box.exe");
    let original = std::fs::read(&binary).unwrap();
    std::fs::write(&binary, b"damaged data; never execute").unwrap();
    assert!(!held.data_is_current());
    assert!(selected_at(&config, &selected).is_err());
    // A real shared version lease, not a fixture boolean, forbids replacement.
    assert!(install(&config, &stage).is_err());
    assert_eq!(
        std::fs::read(&binary).unwrap(),
        b"damaged data; never execute"
    );
    drop(held);
    assert_eq!(
        install(&config, &stage).unwrap(),
        UserRecoveryState::Installed
    );
    let repaired = selected_at(&config, &selected).unwrap().unwrap();
    assert!(repaired.data_is_current());
    assert_eq!(
        std::fs::read(repaired.directory.join("sing-box.exe")).unwrap(),
        original
    );
    let repaired_directory = repaired.directory.clone();
    drop(repaired);
    // Exercise the actual production consumer's selected-slot observation,
    // not just its private ordinary-directory hasher or the installer alone.
    use crate::runtime::helper::recovery_consumer::{
        selected_store_requirement, CoreRecoveryState as S,
    };
    assert_eq!(
        selected_store_requirement(&config, &selected),
        Err(S::NotNeeded)
    );
    for damage in ["missing", "size", "hash"] {
        let held = selected_at(&config, &selected).unwrap().unwrap();
        let binary = held.directory.join(&selected.files[0].name);
        let original = std::fs::read(&binary).unwrap();
        match damage {
            "missing" => std::fs::remove_file(&binary).unwrap(),
            "size" => std::fs::write(&binary, b"wrong size").unwrap(),
            _ => std::fs::write(&binary, vec![b'x'; original.len()]).unwrap(),
        }
        assert_eq!(
            selected_store_requirement(&config, &selected),
            Err(S::StopRequired)
        );
        assert!(install(&config, &stage).is_err());
        drop(held);
        assert_eq!(
            selected_store_requirement(&config, &selected),
            Ok(Some(())),
            "{damage}"
        );
        assert_eq!(
            install(&config, &stage).unwrap(),
            UserRecoveryState::Installed
        );
        assert_eq!(
            selected_store_requirement(&config, &selected),
            Err(S::NotNeeded)
        );
        let repaired = selected_at(&config, &selected).unwrap().unwrap();
        assert_eq!(
            std::fs::read(repaired.directory.join(&selected.files[0].name)).unwrap(),
            original
        );
    }
    let receipt = config
        .join(USER_STORE)
        .join("manifests")
        .join(format!("{}.json", id.as_str()));
    let retained_receipt = config.join("retained-native-receipt.json");
    std::fs::rename(&receipt, &retained_receipt).unwrap();
    assert!(selected_at(&config, &selected).is_err());
    assert_eq!(
        selected_store_requirement(&config, &selected),
        Err(S::ReinstallRequired)
    );
    std::fs::rename(&retained_receipt, &receipt).unwrap();
    let unknown = repaired_directory.join("retain-unknown-data");
    std::fs::write(&unknown, b"do not delete unknown member").unwrap();
    assert!(selected_at(&config, &selected).is_err());
    assert_eq!(
        selected_store_requirement(&config, &selected),
        Err(S::ReinstallRequired)
    );
    assert!(install(&config, &stage).is_err());
    assert_eq!(
        std::fs::read(&unknown).unwrap(),
        b"do not delete unknown member"
    );
    assert!(stage.directory.is_dir());
    assert!(helper.is_file());
    println!(
        "WINDOWS_USER_RECOVERY_NATIVE {}",
        serde_json::json!({
            "fixture":config,"core_id":id.as_str(),"version":selected.version,
            "normal_compiled_stage":true,"Installed":true,"AlreadyPresent":true,
            "reopened_persisted_selection":true,"corrupt_selection_refused":true,
            "live_lease_repair_refused":true,"repair_after_real_lease_drop":true,
        "shared_lease_survives_idempotent_publication":true,"partial_receipt_refused":true,
            "unknown_member_refused_and_retained":true,"execution_authority":false,
            "old_prepared_native":"NOT_RUN: actual Windows execution provider remains Unsupported",
            "fixture_retained_for_inspection":true
        })
    );
    // Keep successful evidence for the user's later unified cleanup. These are
    // only newly created fixture paths; existing profiles/configs are untouched.
    std::mem::forget(stage);
    drop(private_fixture);
}

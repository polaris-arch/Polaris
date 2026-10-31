//! Actual ordinary-user primitive tests, not SYSTEM issuer/admission proofs.
//! No test constructor creates a Cohort, AdmittedPayload or PreparedExecution.
use super::*;
use std::io::Write;
use std::sync::OnceLock;

fn directory() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "polaris-atomic-factory-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn fixture() -> &'static PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let root=directory();
        // The protected PE subset is AMD64. Discover the existing MSVC/SDK
        // command together, including INCLUDE/LIB/PATH on ordinary CI shells.
        let mut compiler=find_msvc_tools::find("x86_64-pc-windows-msvc","cl.exe")
            .expect("MSVC compiler and Windows SDK required for native atomic tests");
        assert!(Path::new(compiler.get_program()).is_absolute());
        let source=Path::new(env!("CARGO_MANIFEST_DIR")).join("src/admitted_payload/windows/issuer/spawn/child.c");
        let image=root.join("inert.exe");
        let argv=vec!["/nologo".to_owned(),"/GS-".into(),"/Zl".into(),source.display().to_string(),format!("/Fo{}",root.join("inert.obj").display()),format!("/Fe{}",image.display()),"/link".into(),"/NODEFAULTLIB".into(),"/ENTRY:mainCRTStartup".into(),"/SUBSYSTEM:CONSOLE".into(),"kernel32.lib".into()];
        let status=compiler.args(&argv).current_dir(&root).status().unwrap();assert!(status.success());
        let bytes=fs::read(&image).unwrap();assert_eq!(pe::imports(&bytes).unwrap(),vec!["kernel32.dll"]);
        println!("ATOMIC_FACTORY_FIXTURE {}",serde_json::json!({"compiler":compiler.get_program(),"source":source,"argv":argv,"image":image,"bytes":bytes.len(),"sha256":sha256_hex(&bytes)}));image
    })
}
fn native_case(cancel: bool) {
    let root = directory();
    let image = root.join("owned.exe");
    fs::copy(fixture(), &image).unwrap();
    let held = guard(&image, false).unwrap();
    let before = identity(&held, true).unwrap();
    let id = CoreId::parse(&"a".repeat(64)).unwrap();
    fs::create_dir(root.join("leases")).unwrap();
    let marker = occupancy_path(&root, &id);
    fs::write(&marker, []).unwrap();
    let marker_guard = guard(&marker, false).unwrap();
    let (input, mut control) = pipe().unwrap();
    let mut child = create_suspended(
        &image,
        &root.join("unused.dll"),
        &input,
        &root,
        &[&held, &marker_guard],
    )
    .unwrap();
    // The actual factory, including HANDLE_LIST/JOB_LIST/mitigations, has run.
    // Child is not yet allowed its first instruction and already owns this guard.
    drop(held);
    drop(marker_guard);
    drop(input);
    assert_eq!(
        maintenance_file(&root, &id).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&image)
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    assert_ne!(
        unsafe { ResumeThread(child.thread.as_raw_handle()) },
        u32::MAX
    );
    let mut ready = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(&ready, b"ready");
    let process = child.process.as_ref().unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    assert_eq!(
        maintenance_file(&root, &id).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&image)
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    if cancel {
        drop(child.job.take());
    } else {
        control.write_all(b"x").unwrap();
    }
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
    let mut code = 0;
    assert_ne!(
        unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) },
        0
    );
    if !cancel {
        assert_eq!(code, 0);
    }
    // Real native wait, then and only then does the inherited deny-write guard end.
    assert!(maintenance_file(&root, &id).unwrap().is_some());
    let reopened = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(7)
        .open(&image)
        .unwrap();
    assert_eq!(identity(&reopened, true).unwrap(), before);
    println!(
        "ATOMIC_FACTORY_NATIVE {}",
        serde_json::json!({"cancel_job":cancel,"pid":child.pid,"actual_wait":0,"actual_exit":code,"held_volume":before.volume.to_string(),"held_id":before.id,"held_bytes":before.bytes,"before_first_instruction_guard":true,"child_held_guard_without_parent_file":true,"SYSTEM_issuer_or_Prepared":false})
    );
    drop(reopened);
    drop(child);
    fs::remove_file(&image).unwrap();
    fs::remove_file(&marker).unwrap();
    fs::remove_dir(root.join("leases")).unwrap();
    fs::remove_dir(&root).unwrap();
}
#[test]
fn native_atomic_guard_and_job_survive_parent_file_release_until_actual_wait() {
    native_case(false);
}
#[test]
fn native_atomic_job_close_is_fail_stop_without_post_spawn_assign() {
    native_case(true);
}

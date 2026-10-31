use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;

#[test]
fn windows_policy_and_actual_callers_keep_execution_unavailable() {
    let error = AdmissionError::unsupported_exact_object();
    assert_eq!(error.kind, AdmissionErrorKind::UnsupportedExactObject);
    assert_eq!(error.recovery, RecoveryAction::None);
    let source = "a".repeat(40);
    let tree = "b".repeat(40);
    for loader in [
        "linux-sealed-fd-v1",
        "windows-guarded-image-v1",
        "",
        "unknown",
    ] {
        let policy = serde_json::json!({"schema":"polaris-trusted-execution-v1",
            "sourceCommit":source,"sourceTree":tree,"loader":loader,"purposes":["run-main-v1"]});
        assert_eq!(
            SelectedPolicy::declaration(&policy, &source, &tree, "win")
                .unwrap_err()
                .kind,
            AdmissionErrorKind::UnsupportedExactObject
        );
    }
    let path = std::env::current_exe().unwrap();
    for purpose in [
        ExecPurpose::RunMain,
        ExecPurpose::RunLogin,
        ExecPurpose::RunSpeedtest,
        ExecPurpose::CheckConfig,
        ExecPurpose::TlsInventory,
    ] {
        assert_eq!(
            admit_selected(UntrustedPayload::bundled(&path), purpose)
                .unwrap_err()
                .kind,
            AdmissionErrorKind::UnsupportedExactObject
        );
    }
}

fn directory(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "polaris-main-image-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        label
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn rw(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(7)
        .open(path)
        .unwrap()
}
// This is a private observation of the exact proposed cold-open primitive,
// NEVER an execution owner. The cached-section test disproves that promotion.
fn observed_cold_file(path: &Path) -> std::io::Result<File> {
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::Prefix(p) if matches!(p.kind(), Prefix::Disk(_)) => {}
            Component::RootDir => rooted = true,
            Component::Normal(n) if rooted => {
                let n = n.to_string_lossy();
                if n.contains(':') || n.ends_with(&['.', ' '][..]) {
                    return Err(std::io::Error::other("alias"));
                }
            }
            _ => return Err(std::io::Error::other("nonlocal/relative alias")),
        }
    }
    let f = OpenOptions::new()
        .access_mode(0x8000_0020)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    let s = native::standard(&f)?;
    use std::os::windows::fs::MetadataExt;
    if !rooted
        || !f.metadata()?.is_file()
        || f.metadata()?.file_attributes() & 0x400 != 0
        || s.links != 1
        || s.deleted != 0
        || s.length == 0
        || s.length > 96 * 1024 * 1024
    {
        return Err(std::io::Error::other("not one direct bounded local object"));
    }
    Ok(f)
}
fn digest(f: &File) -> String {
    let mut f = f.try_clone().unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).unwrap();
    sha256_hex(&bytes)
}

#[test]
fn old_writer_and_handleless_writable_view_refuse_the_actual_cold_open() {
    let d = directory("write-view");
    let p = d.join("whole-test-image.exe");
    fs::copy(std::env::current_exe().unwrap(), &p).unwrap();
    let writer = rw(&p);
    assert_eq!(observed_cold_file(&p).unwrap_err().raw_os_error(), Some(32));
    let section = native::Section::new(&writer, 4);
    let view = section.view(2);
    drop(section);
    drop(writer);
    assert_eq!(observed_cold_file(&p).unwrap_err().raw_os_error(), Some(32));
    drop(view);
    let held = observed_cold_file(&p).unwrap();
    let before = native::identity(&held).unwrap();
    let hash = digest(&held);
    println!(
        "SHARE_VIEW_NATIVE {}",
        serde_json::json!({"held":before,"sha256":hash,"fixture":p})
    );
    assert_eq!(native::identity(&held).unwrap(), before);
    drop(held);
    let alias = d.join("hardlink.exe");
    fs::hard_link(&p, &alias).unwrap();
    assert!(observed_cold_file(&p).is_err()); // link count rechecked, never a safe alias
}

struct Fixture {
    root: PathBuf,
}
fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let root = directory("compiled-fixture");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/admitted_payload/windows/tests");
        let compiler = std::env::var_os("POLARIS_NATIVE_MSVC_CL")
            .expect("explicit native test requires existing MSVC cl path");
        assert!(Path::new(&compiler).is_absolute());
        for (tag, value) in [("A", "65"), ("B", "66")] {
            let d = root.join(tag);
            fs::create_dir(&d).unwrap();
            let base = vec![
                "/nologo".into(),
                "/GS-".into(),
                "/Zl".into(),
                "/O1".into(),
                format!("/DVALUE={value}"),
            ];
            let mut args = base.clone();
            args.extend([
                "/c".into(),
                source.join("dependency.c").to_string_lossy().into(),
                format!("/Fo{}", d.join("dependency.obj").display()),
            ]);
            compile(&compiler, &root, &args);
            compile(
                &compiler,
                &root,
                &[
                    "/nologo".into(),
                    "/LD".into(),
                    d.join("dependency.obj").to_string_lossy().into(),
                    "/link".into(),
                    "/NODEFAULTLIB".into(),
                    "/ENTRY:DllMain".into(),
                    format!("/OUT:{}", d.join("observer.dll").display()),
                    format!("/IMPLIB:{}", d.join("observer.lib").display()),
                    "kernel32.lib".into(),
                ],
            );
            let mut args = base;
            args.extend([
                source.join("main.c").to_string_lossy().into(),
                format!("/Fo{}", d.join("main.obj").display()),
                format!("/Fe{}", d.join("image.exe").display()),
                "/link".into(),
                "/NODEFAULTLIB".into(),
                "/ENTRY:mainCRTStartup".into(),
                "/SUBSYSTEM:CONSOLE".into(),
                "/MANIFEST:NO".into(),
                d.join("observer.lib").to_string_lossy().into(),
                "kernel32.lib".into(),
            ]);
            compile(&compiler, &root, &args);
        }
        let (exit, marker) = actual_child(
            &root.join("B/image.exe"),
            &root.join("B"),
            &root.join("baseline-B-marker"),
        );
        assert_eq!(exit, 85);
        assert_eq!(marker, "DLL_B\nENTRY_B\n");
        println!("BASELINE_B_NATIVE exit={exit} marker={marker:?}");
        Fixture { root }
    })
}
fn compile(cl: &std::ffi::OsStr, cwd: &Path, args: &[String]) {
    let output = std::process::Command::new(cl)
        .current_dir(cwd)
        .args(args)
        .env("VSLANG", "1033")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "MSVC native fixture: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn case(label: &str) -> (PathBuf, PathBuf) {
    let d = directory(label);
    let p = d.join("image.exe");
    fs::copy(fixture().root.join("A/image.exe"), &p).unwrap();
    fs::copy(
        fixture().root.join("A/observer.dll"),
        d.join("observer.dll"),
    )
    .unwrap();
    (d, p)
}
fn actual_child(exe: &Path, cwd: &Path, marker: &Path) -> (i32, String) {
    use std::os::windows::process::CommandExt;
    // Test-only whole PE, no Core or PreparedExecution. Parent Cargo's reviewed
    // Job owns this process; output() performs the actual child wait.
    let output = std::process::Command::new(exe)
        .current_dir(cwd)
        .env_clear()
        .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
        .env("POLARIS_PRIVATE_IMAGE_MARKER", marker)
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        fs::read_to_string(marker).unwrap(),
    )
}

#[test]
#[ignore = "explicit native whole-PE compiler fixture; no production admission or system actions"]
fn cached_SEC_IMAGE_and_retained_writer_can_disagree_with_cold_hash_and_CreateProcess() {
    for mapped in [false, true] {
        let (d, p) = case(if mapped {
            "cached-view"
        } else {
            "cached-section"
        });
        let a = fs::read(&p).unwrap();
        let b = fs::read(fixture().root.join("B/image.exe")).unwrap();
        assert_eq!(a.len(), b.len());
        assert_ne!(sha256_hex(&a), sha256_hex(&b));
        let mut writer = rw(&p);
        let section = native::Section::new(&writer, 0x0100_0002);
        let view = mapped.then(|| section.view(4));
        if let Some(view) = &view {
            view.touch_entry(&a);
        }
        writer.write_all(&b).unwrap();
        writer.sync_all().unwrap();
        drop(writer);
        let held = observed_cold_file(&p).unwrap();
        let id = native::identity(&held).unwrap();
        assert_eq!(digest(&held), sha256_hex(&b));
        let (exit, marker) = actual_child(&p, &d, &d.join("marker"));
        // Record the actual image independently of disk/query-image metadata.
        // This explicit regression profile reproduces the unsafe cached-A case.
        assert_eq!(exit, 65);
        assert_eq!(marker, "DLL_A\nENTRY_A\n");
        assert_eq!(native::identity(&held).unwrap(), id);
        assert_eq!(
            admit_selected(UntrustedPayload::bundled(&p), ExecPurpose::RunMain)
                .unwrap_err()
                .kind,
            AdmissionErrorKind::UnsupportedExactObject
        );
        println!(
            "CACHED_IMAGE_NATIVE {}",
            serde_json::json!({"mapped_view":mapped,"held":id,"cold_sha256":digest(&held),"disk_B":sha256_hex(&b),"actual_marker":marker,"actual_exit":exit,"fixture":d,"execution_admitted":false})
        );
        drop(view);
        drop(section);
    }
}

#[test]
#[ignore = "explicit native whole-PE compiler fixture; no production admission or system actions"]
fn held_EXE_and_DLL_do_not_close_late_dot_local_namespace_before_entry() {
    let (d, p) = case("late-local");
    let exe = observed_cold_file(&p).unwrap();
    let dll = observed_cold_file(&d.join("observer.dll")).unwrap();
    let mut ancestors = Vec::new();
    let mut prefixes: Vec<_> = d.ancestors().filter(|p| p.has_root()).collect();
    prefixes.reverse();
    for path in prefixes {
        ancestors.push(
            OpenOptions::new()
                .read(true)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(path)
                .unwrap(),
        );
    }
    let local = d.join("image.exe.local");
    fs::create_dir(&local).unwrap();
    fs::copy(
        fixture().root.join("B/observer.dll"),
        local.join("observer.dll"),
    )
    .unwrap();
    let (exit, marker) = actual_child(&p, &d, &d.join("marker"));
    assert_eq!(exit, 66);
    assert_eq!(marker, "DLL_B\nENTRY_A\n");
    assert_eq!(
        admit_selected(UntrustedPayload::bundled(&p), ExecPurpose::RunMain)
            .unwrap_err()
            .kind,
        AdmissionErrorKind::UnsupportedExactObject
    );
    println!(
        "EARLY_DLL_NATIVE {}",
        serde_json::json!({"held_EXE":native::identity(&exe).unwrap(),"held_original_DLL":native::identity(&dll).unwrap(),"ancestor_handles_no_shareDELETE":ancestors.len(),"actual_marker":marker,"actual_exit":exit,"fixture":d,"execution_admitted":false})
    );
}

#[allow(
    unsafe_code,
    reason = "bounded Win32 handles/views in self-owned native regressions, not an executor"
)]
mod native {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    type H = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            file: H,
            security: H,
            protect: u32,
            high: u32,
            low: u32,
            name: *const u16,
        ) -> H;
        fn MapViewOfFile(section: H, access: u32, high: u32, low: u32, bytes: usize) -> H;
        fn UnmapViewOfFile(view: H) -> i32;
        fn GetFileInformationByHandleEx(file: H, class: u32, out: H, bytes: u32) -> i32;
        fn GetFinalPathNameByHandleW(file: H, out: *mut u16, len: u32, flags: u32) -> u32;
    }
    pub struct Section(OwnedHandle);
    impl Section {
        pub fn new(file: &File, protection: u32) -> Self {
            // SAFETY: live file, anonymous mapping, exact documented ABI.
            let raw = unsafe {
                CreateFileMappingW(
                    file.as_raw_handle(),
                    std::ptr::null_mut(),
                    protection,
                    0,
                    0,
                    std::ptr::null(),
                )
            };
            assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
            // SAFETY: successful call returned one owned handle, not borrowed.
            Self(unsafe { OwnedHandle::from_raw_handle(raw) })
        }
        pub fn view(&self, access: u32) -> View {
            // SAFETY: live mapping and OS-selected address/whole-view extent.
            let v = unsafe { MapViewOfFile(self.0.as_raw_handle(), access, 0, 0, 0) };
            assert!(!v.is_null(), "{}", std::io::Error::last_os_error());
            View(v)
        }
    }
    pub struct View(H);
    impl View {
        pub fn touch_entry(&self, pe: &[u8]) {
            let offset = u32::from_le_bytes(pe[0x3c..0x40].try_into().unwrap()) as usize;
            let rva = u32::from_le_bytes(pe[offset + 40..offset + 44].try_into().unwrap()) as usize;
            assert!(rva < 0x20000);
            // SAFETY: compiler-owned fixture's validated PE entry belongs to its
            // live SEC_IMAGE view. Volatile read forces the old code page resident.
            std::hint::black_box(unsafe { std::ptr::read_volatile(self.0.cast::<u8>().add(rva)) });
        }
    }
    impl Drop for View {
        fn drop(&mut self) {
            // SAFETY: precisely the live base returned by MapViewOfFile, once.
            assert_ne!(unsafe { UnmapViewOfFile(self.0) }, 0);
        }
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct Standard {
        pub allocated: i64,
        pub length: i64,
        pub links: u32,
        pub deleted: u8,
        pub directory: u8,
    }
    pub fn standard(file: &File) -> std::io::Result<Standard> {
        let mut s = Standard::default();
        // SAFETY: exact FileStandardInfo ABI and writable native-size buffer.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                1,
                std::ptr::from_mut(&mut s).cast(),
                std::mem::size_of::<Standard>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(s)
    }
    #[repr(C)]
    #[derive(Default)]
    struct Id {
        volume: u64,
        id: [u8; 16],
    }
    pub fn identity(file: &File) -> std::io::Result<serde_json::Value> {
        let mut id = Id::default();
        // SAFETY: FileIdInfo(18), exact native struct, live held file.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                18,
                std::ptr::from_mut(&mut id).cast(),
                std::mem::size_of::<Id>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut name = vec![0u16; 32768];
        // SAFETY: bounded writable UTF16 buffer, VOLUME_NAME_GUID(1).
        let n = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                name.as_mut_ptr(),
                name.len() as u32,
                1,
            )
        };
        if n == 0 || n as usize >= name.len() {
            return Err(std::io::Error::last_os_error());
        }
        let s = standard(file)?;
        Ok(
            serde_json::json!({"volumeSerial":id.volume.to_string(),"fileID128":id.id,"volumeGUID_path":String::from_utf16_lossy(&name[..n as usize]),"length":s.length,"links":s.links}),
        )
    }
}

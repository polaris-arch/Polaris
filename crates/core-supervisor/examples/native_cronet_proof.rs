//! Formal CI probe for the compiled desktop Core and its dynamic Cronet sidecar.
//! The Node gate runs the C ABI smoke and Core probe in separate child processes.

#[cfg(any(target_os = "linux", windows))]
mod native {
    use std::env;
    use std::ffi::{c_char, c_void, CStr};
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use polaris_core_supervisor::selected_core::{current_platform, formal_manifest};
    use serde_json::{json, Map, Value};
    use sha2::{Digest, Sha256};
    use tokio::process::{Child, Command};
    use tokio::time::{sleep, timeout, Instant};

    type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;

    const CRONET_EXPORTS: [&str; 6] = [
        "Cronet_Engine_Create",
        "Cronet_EngineParams_Create",
        "Cronet_Engine_StartWithParams",
        "Cronet_UrlRequest_Create",
        "Cronet_UrlRequest_InitWithParams",
        "Cronet_UrlRequest_Start",
    ];
    const MAX_OBSERVATION: u64 = 4 * 1024 * 1024;
    const START_DEADLINE: Duration = Duration::from_secs(20);
    const STOP_DEADLINE: Duration = Duration::from_secs(5);
    const LOADER_ENV: [&str; 4] = [
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
    ];

    pub(super) async fn run() -> ProbeResult<Value> {
        if env::var("POLARIS_NO_KERNEL_RUN").as_deref() == Ok("1") {
            return Err("Local no-kernel boundary forbids native payload execution".into());
        }
        let action = env::args().nth(1);
        if !matches!(action.as_deref(), Some("capi" | "core")) || env::args_os().count() != 2 {
            return Err("Exactly one native proof operation is required".into());
        }
        let key = current_platform()?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()?;
        let candidate = env::var("CORE_CANDIDATE").or_else(|_| env::var("GITHUB_SHA"))?;
        let status = Command::new("node")
            .args(["scripts/desktop-core/validation-origin.mjs", &candidate])
            .current_dir(&root)
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .status();
        if !timeout(Duration::from_secs(30), status).await??.success() {
            return Err("Formal native payload admission failed".into());
        }
        let manifest = formal_manifest(key)?;
        let [core_file, library_file] = manifest.files.as_slice() else {
            return Err("Expected a Core and a Cronet sidecar in the compiled manifest".into());
        };
        let directory = root.join("resources").join(key);
        let core = directory.join(&core_file.name);
        let library = directory.join(&library_file.name);
        verify_file(&core, &core_file.sha256, core_file.bytes)?;
        verify_file(&library, &library_file.sha256, library_file.bytes)?;
        let receipt = match action.as_deref() {
            Some("capi") => capi(&library, &library_file.sha256, library_file.bytes)?,
            Some("core") => {
                probe_core(&core, &library, &library_file.sha256, library_file.bytes).await?
            }
            _ => unreachable!("validated operation"),
        };
        verify_file(&core, &core_file.sha256, core_file.bytes)?;
        verify_file(&library, &library_file.sha256, library_file.bytes)?;
        Ok(receipt)
    }

    fn verify_file(path: &Path, expected_sha: &str, expected_bytes: u64) -> ProbeResult<()> {
        let info = path.symlink_metadata()?;
        if !path.is_absolute() || !info.is_file() || linked(&info) {
            return Err("Non-regular native payload".into());
        }
        for parent in path.ancestors().skip(1) {
            let info = parent.symlink_metadata()?;
            if !info.is_dir() || linked(&info) {
                return Err("Native payload has a linked ancestor".into());
            }
        }
        let file = File::open(path)?;
        if file.metadata()?.len() != expected_bytes {
            return Err("Native payload size differs".into());
        }
        let mut digest = Sha256::new();
        let mut reader = file.take(expected_bytes + 1);
        let mut buffer = [0; 64 * 1024];
        let mut bytes = 0;
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            digest.update(&buffer[..count]);
        }
        if bytes != expected_bytes || digest_hex(&digest.finalize()) != expected_sha {
            return Err("Native payload hash differs".into());
        }
        Ok(())
    }

    fn digest_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[cfg(target_os = "linux")]
    fn linked(meta: &fs::Metadata) -> bool {
        meta.is_symlink()
    }

    #[cfg(windows)]
    fn linked(meta: &fs::Metadata) -> bool {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        meta.is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    fn checked_owner(path: &Path, library: &Path, sha: &str, size: u64) -> ProbeResult<()> {
        verify_file(path, sha, size)?;
        if !same_file::is_same_file(path, library)? {
            return Err("Native Cronet resolved outside the canonical file identity".into());
        }
        Ok(())
    }

    fn mapped_receipt(
        paths: &[PathBuf],
        library: &Path,
        sha: &str,
        size: u64,
    ) -> ProbeResult<Value> {
        let [mapped] = paths else {
            return Err("Actual Core must map exactly one Cronet library".into());
        };
        checked_owner(mapped, library, sha, size)?;
        Ok(json!({ "path": mapped, "sha256": sha }))
    }

    fn capi(library: &Path, sha: &str, size: u64) -> ProbeResult<Value> {
        type Create = unsafe extern "C" fn() -> *mut c_void;
        type Start = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
        type Shutdown = unsafe extern "C" fn(*mut c_void) -> i32;
        type Destroy = unsafe extern "C" fn(*mut c_void);
        type Version = unsafe extern "C" fn(*mut c_void) -> *const c_char;

        // The path and digest come from the compiled release. The fixed
        // signatures follow Chromium's generated cronet.idl_c.h.
        let lib = platform::open(library)?;
        let mut symbols = Map::new();
        for name in CRONET_EXPORTS {
            // Resolve each named entry without invoking it.
            let address = *unsafe { lib.get::<unsafe extern "C" fn()>(name.as_bytes())? };
            let owner = platform::symbol_owner(address as *const c_void)?;
            checked_owner(&owner, library, sha, size)?;
            symbols.insert(
                name.into(),
                json!({ "address": format!("{:p}", address as *const c_void), "owner": owner }),
            );
        }
        // SAFETY: the library is pinned and held alive, with the generated C
        // signatures above. All allocated C handles are retired before unload.
        let version = unsafe {
            let create = lib.get::<Create>(b"Cronet_Engine_Create")?;
            let create_params = lib.get::<Create>(b"Cronet_EngineParams_Create")?;
            let start = lib.get::<Start>(b"Cronet_Engine_StartWithParams")?;
            let shutdown = lib.get::<Shutdown>(b"Cronet_Engine_Shutdown")?;
            let destroy = lib.get::<Destroy>(b"Cronet_Engine_Destroy")?;
            let destroy_params = lib.get::<Destroy>(b"Cronet_EngineParams_Destroy")?;
            let version = lib.get::<Version>(b"Cronet_Engine_GetVersionString")?;
            let engine = create();
            let params = create_params();
            let result = if engine.is_null() || params.is_null() {
                Err("Native C API allocation failed")
            } else if start(engine, params) != 0 {
                Err("Native Cronet engine initialization failed")
            } else {
                let pointer = version(engine);
                let version = (!pointer.is_null())
                    .then(|| CStr::from_ptr(pointer).to_str().ok())
                    .flatten()
                    .filter(|text| !text.is_empty() && text.is_ascii())
                    .map(str::to_owned);
                if shutdown(engine) != 0 {
                    Err("Native Cronet engine shutdown failed")
                } else {
                    version.ok_or("Native Cronet version missing or invalid")
                }
            };
            if !engine.is_null() {
                destroy(engine);
            }
            if !params.is_null() {
                destroy_params(params);
            }
            result?
        };
        Ok(json!({
            "schema": "polaris-cronet-capi-smoke-v1", "librarySha256": sha,
            "symbols": symbols, "version": version, "engineInitialized": true,
            "scope": "six loader resolutions and engine create/start/shutdown; not full ABI",
        }))
    }

    async fn probe_core(core: &Path, library: &Path, sha: &str, size: u64) -> ProbeResult<Value> {
        // No inbound, TUN, listener, DNS, or external traffic.
        let config = serde_json::to_vec(&json!({
            "log": {"level": "info", "timestamp": false},
            "outbounds": [{"type": "naive", "tag": "probe", "server": "127.0.0.1",
                "server_port": 9, "username": "probe", "password": "probe",
                "tls": {"enabled": true, "server_name": "localhost"}}],
        }))?;
        let scratch = tempfile::tempdir()?;
        let config_path = scratch.path().join("config.json");
        let log_path = scratch.path().join("core.log");
        fs::write(&config_path, &config)?;
        let log = File::create(&log_path)?;
        let mut command = Command::new(core);
        command
            .args(["run", "-c"])
            .arg(config_path)
            .current_dir(core.parent().ok_or("missing Core directory")?)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        for key in LOADER_ENV {
            command.env_remove(key);
        }
        let mut child = command.spawn()?;
        let pid = child.id().ok_or("Missing owned Core PID")?;
        let observed = observe(&mut child, &log_path, library, sha, size).await;
        stop(&mut child, pid).await?;
        let mapped = observed?;
        let log = String::from_utf8_lossy(&bounded_read(&log_path)?).into_owned();
        Ok(json!({
            "mappedLibrary": mapped, "started": true, "pid": pid, "log": log,
            "configurationSha256": digest_hex(&Sha256::digest(&config)),
            "scope": "actual naive core initialization, no inbound/TUN/request/device acceptance",
        }))
    }

    fn bounded_read(path: &Path) -> ProbeResult<Vec<u8>> {
        let mut result = Vec::new();
        File::open(path)?
            .take(MAX_OBSERVATION + 1)
            .read_to_end(&mut result)?;
        if result.len() as u64 > MAX_OBSERVATION {
            return Err("Native observation exceeds its size limit".into());
        }
        Ok(result)
    }

    async fn observe(
        child: &mut Child,
        log: &Path,
        library: &Path,
        sha: &str,
        size: u64,
    ) -> ProbeResult<Value> {
        #[cfg(windows)]
        polaris_core_supervisor::job_object::enroll_child(child)?;
        let deadline = Instant::now() + START_DEADLINE;
        loop {
            let output = bounded_read(log)?;
            if child.try_wait()?.is_some() {
                let text = String::from_utf8_lossy(&output);
                let tail: String = text.chars().rev().take(1000).collect();
                return Err(format!(
                    "Naive Core exited before startup: {}",
                    tail.chars().rev().collect::<String>()
                )
                .into());
            }
            if String::from_utf8_lossy(&output).contains("sing-box started") {
                let paths = platform::mapped_paths(child)?;
                if !paths.is_empty() {
                    let mapped = mapped_receipt(&paths, library, sha, size)?;
                    if child.try_wait()?.is_some() {
                        return Err("Core exited during the mapping observation".into());
                    }
                    return Ok(mapped);
                }
            }
            if Instant::now() >= deadline {
                return Err("Naive startup and canonical Cronet mapping not observed".into());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stop(child: &mut Child, _pid: u32) -> ProbeResult<()> {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            let pid = rustix::process::Pid::from_raw(i32::try_from(_pid)?)
                .ok_or("Invalid owned Core PID")?;
            // The unreaped, owned child keeps this PID reserved until wait.
            if rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_ok()
                && timeout(STOP_DEADLINE, child.wait())
                    .await
                    .is_ok_and(|result| result.is_ok())
            {
                return Ok(());
            }
        }
        child.start_kill()?;
        timeout(STOP_DEADLINE, child.wait()).await??;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    mod platform {
        use super::*;
        use nix::libc;
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        pub(super) fn open(path: &Path) -> ProbeResult<libloading::Library> {
            // SAFETY: open only the checked, compiled Cronet binary with local symbols.
            let lib = unsafe {
                libloading::os::unix::Library::open(Some(path), libc::RTLD_NOW | libc::RTLD_LOCAL)?
            };
            Ok(lib.into())
        }

        pub(super) fn symbol_owner(address: *const c_void) -> ProbeResult<PathBuf> {
            // SAFETY: dladdr fills the stack structure and owns its returned
            // string for as long as the already-held library remains loaded.
            let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
            if unsafe { libc::dladdr(address, &mut info) } == 0 || info.dli_fname.is_null() {
                return Err("Cannot resolve Cronet symbol owner".into());
            }
            let name = unsafe { CStr::from_ptr(info.dli_fname) };
            Ok(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
        }

        pub(super) fn mapped_paths(child: &Child) -> ProbeResult<Vec<PathBuf>> {
            let pid = child.id().ok_or("Missing owned Core PID")?;
            linux_maps(&bounded_read(Path::new(&format!("/proc/{pid}/maps")))?)
        }

        fn linux_maps(bytes: &[u8]) -> ProbeResult<Vec<PathBuf>> {
            let mut result = std::collections::BTreeSet::new();
            for line in bytes.split(|&byte| byte == b'\n') {
                let mut name = line.trim_ascii_start();
                for _ in 0..5 {
                    let Some(separator) = name.iter().position(u8::is_ascii_whitespace) else {
                        name = &[];
                        break;
                    };
                    name = name[separator..].trim_ascii_start();
                }
                if !name.starts_with(b"/") {
                    continue;
                }
                let mut decoded = Vec::with_capacity(name.len());
                let mut index = 0;
                while index < name.len() {
                    if let Some([b'\\', a @ b'0'..=b'3', b @ b'0'..=b'7', c @ b'0'..=b'7']) =
                        name.get(index..index + 4)
                    {
                        decoded.push((a - b'0') * 64 + (b - b'0') * 8 + (c - b'0'));
                        index += 4;
                    } else {
                        decoded.push(name[index]);
                        index += 1;
                    }
                }
                let path = Path::new(OsStr::from_bytes(&decoded));
                if path.file_name() == Some(OsStr::new("libcronet.so (deleted)")) {
                    return Err("Core maps a deleted Cronet file".into());
                }
                if path.file_name() == Some(OsStr::new("libcronet.so")) {
                    result.insert(path.to_owned());
                }
            }
            Ok(result.into_iter().collect())
        }

        #[cfg(test)]
        #[test]
        fn maps_decode_spaces_and_deduplicate_segments_but_reject_deleted_files() {
            let text = b"a-b r-xp 0 00:01 1 /tmp/with\\040space/libcronet.so\n\
                         b-c rw-p 0 00:01 1 /tmp/with\\040space/libcronet.so\n\
                         d-e r-xp 0 00:01 2 /lib/libc.so.6\n";
            assert_eq!(
                linux_maps(text).unwrap(),
                [PathBuf::from("/tmp/with space/libcronet.so")]
            );
            assert!(linux_maps(b"a-b r-xp 0 00:01 1 /tmp/libcronet.so (deleted)").is_err());
        }
    }

    #[cfg(windows)]
    mod platform {
        use super::*;
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::Foundation::HMODULE;
        use windows_sys::Win32::System::LibraryLoader::{
            GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        use windows_sys::Win32::System::ProcessStatus::{
            K32EnumProcessModulesEx, K32GetModuleFileNameExW, LIST_MODULES_ALL,
        };

        pub(super) fn open(path: &Path) -> ProbeResult<libloading::Library> {
            // SAFETY: open only the attached verified file, with restricted
            // DLL search flags rather than searching the ambient cwd.
            let library = unsafe {
                libloading::os::windows::Library::load_with_flags(
                    path,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )?
            };
            Ok(library.into())
        }

        pub(super) fn symbol_owner(address: *const c_void) -> ProbeResult<PathBuf> {
            let mut owner = std::ptr::null_mut();
            let mut name = vec![0u16; 32768];
            // SAFETY: FROM_ADDRESS interprets this argument as a code pointer,
            // and the loaded library remains alive until the caller finishes.
            let ok = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    address.cast(),
                    &mut owner,
                )
            };
            if ok == 0 {
                return Err("Cannot resolve Cronet symbol owner".into());
            }
            let length = unsafe { GetModuleFileNameW(owner, name.as_mut_ptr(), name.len() as u32) };
            module_path(&name, length)
        }

        pub(super) fn mapped_paths(child: &Child) -> ProbeResult<Vec<PathBuf>> {
            let handle = child.raw_handle().ok_or("Missing owned Core handle")?;
            let mut modules: Vec<HMODULE> = vec![std::ptr::null_mut(); 4096];
            let capacity = std::mem::size_of_val(modules.as_slice()) as u32;
            let mut needed = 0;
            // SAFETY: the original Child holds the process and all buffers
            // remain writable for the length supplied to the OS.
            let ok = unsafe {
                K32EnumProcessModulesEx(
                    handle.cast(),
                    modules.as_mut_ptr(),
                    capacity,
                    &mut needed,
                    LIST_MODULES_ALL,
                )
            };
            let width = std::mem::size_of::<HMODULE>() as u32;
            if ok == 0 || needed > capacity || !needed.is_multiple_of(width) {
                return Err("Cannot read complete actual Core module inventory".into());
            }
            let mut name = vec![0u16; 32768];
            let mut paths = std::collections::BTreeSet::new();
            for module in &modules[..(needed / width) as usize] {
                let length = unsafe {
                    K32GetModuleFileNameExW(
                        handle.cast(),
                        *module,
                        name.as_mut_ptr(),
                        name.len() as u32,
                    )
                };
                let path = module_path(&name, length)?;
                if path
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("libcronet.dll"))
                {
                    paths.insert(path);
                }
            }
            Ok(paths.into_iter().collect())
        }

        fn module_path(name: &[u16], length: u32) -> ProbeResult<PathBuf> {
            let length = length as usize;
            if length == 0 || length >= name.len() {
                return Err("Cannot read complete native module path".into());
            }
            Ok(PathBuf::from(OsString::from_wide(&name[..length])))
        }
    }

    #[cfg(test)]
    #[test]
    fn mapped_receipt_checks_complete_bytes_and_original_file_identity() {
        let scratch = tempfile::tempdir().unwrap();
        let parent = scratch.path().canonicalize().unwrap();
        let expected = parent.join("canonical.so");
        let other = parent.join("copy.so");
        for file in [&expected, &other] {
            fs::write(file, b"canonical").unwrap();
        }
        let digest = digest_hex(&Sha256::digest(b"canonical"));
        let read = |paths: &[PathBuf]| mapped_receipt(paths, &expected, &digest, 9);
        assert_eq!(
            read(std::slice::from_ref(&expected)).unwrap()["sha256"],
            digest
        );
        assert!(read(&[]).is_err());
        assert!(read(std::slice::from_ref(&other)).is_err());
        assert!(read(&[expected.clone(), other]).is_err());
        fs::write(&expected, b"changed!!").unwrap();
        assert!(read(std::slice::from_ref(&expected)).is_err());
    }
}

#[cfg(any(target_os = "linux", windows))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    match native::run().await {
        Ok(receipt) => println!("{receipt}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn main() {
    eprintln!("Native Cronet proof requires Linux or Windows");
    std::process::exit(1);
}

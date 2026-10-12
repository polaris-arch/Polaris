//! Administrative content installation. This command is never exposed on IPC.
use crate::{
    core_objects::PayloadDir,
    core_store::{CoreStore, OwnerPolicy},
};
use polaris_helper_proto::core_manifest::InstalledCoreManifest;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct InstallArgs {
    pub root: PathBuf,
    pub source: PathBuf,
    pub manifest: PathBuf,
}

impl InstallArgs {
    pub fn parse(args: &[String]) -> Result<Self, &'static str> {
        if args.len() != 7 || args[0] != "core-install" {
            return Err("invalid core-install arguments");
        }
        let mut root = None;
        let mut source = None;
        let mut manifest = None;
        for pair in args[1..].chunks_exact(2) {
            let value = PathBuf::from(&pair[1]);
            if !value.is_absolute() || pair[1].contains(['\n', '\r', '\0']) {
                return Err("absolute path required");
            }
            let slot = match pair[0].as_str() {
                "--root" => &mut root,
                "--source" => &mut source,
                "--manifest" => &mut manifest,
                _ => return Err("unknown core-install argument"),
            };
            if slot.replace(value).is_some() {
                return Err("duplicate core-install argument");
            }
        }
        let value = Self {
            root: root.ok_or("missing root")?,
            source: source.ok_or("missing source")?,
            manifest: manifest.ok_or("missing manifest")?,
        };
        if value.manifest != value.source.join("core-manifest.json") {
            return Err("manifest must belong to source directory");
        }
        Ok(value)
    }
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn install(args: &InstallArgs) -> io::Result<String> {
    if !privileged()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "administrator required",
        ));
    }
    #[cfg(unix)]
    let source_uid = {
        use std::os::unix::fs::MetadataExt;
        Some(std::fs::symlink_metadata(&args.source)?.uid())
    };
    #[cfg(not(unix))]
    let source_uid = None;
    let objects = PayloadDir::open(&args.source, source_uid)?;
    let (file, bytes) = objects.open_regular("core-manifest.json")?;
    if bytes > 64 * 1024 {
        return Err(invalid("manifest too large"));
    }
    let mut raw = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut raw)?;
    let manifest =
        InstalledCoreManifest::parse(&raw, crate::core_install::sha256_hex).map_err(invalid)?;
    if manifest.core.platform != current_platform()? {
        return Err(invalid("core platform differs from installer"));
    }
    // Check the actual utility bytes, not its filename, a version string, or the
    // helper installed by another App. The staged manifest hash is mandatory here.
    let helper = manifest
        .core
        .helper_sha256
        .as_deref()
        .ok_or_else(|| invalid("missing helper hash"))?;
    if hash_file(&std::env::current_exe()?)? != helper {
        return Err(invalid("installer helper hash differs"));
    }
    let store = CoreStore::initialize(&args.root, OwnerPolicy::privileged())?;
    store.install(&manifest, &args.source, source_uid)?;
    store.retain_latest_three(&[])?;
    Ok(format!(
        "core-installed {} {}",
        manifest.id, manifest.core.version
    ))
}

fn hash_file(path: &Path) -> io::Result<String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("invalid helper object"));
    }
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn current_platform() -> io::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux"),
        ("windows", "x86_64") => Ok("win"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        _ => Err(invalid("unsupported desktop platform")),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn privileged() -> io::Result<bool> {
    Ok(nix::unistd::Uid::effective().is_root())
}

#[cfg(windows)]
#[allow(unsafe_code, reason = "read-only process token elevation check")]
fn privileged() -> io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = std::ptr::null_mut();
    // SAFETY: current-process pseudo handle and live output pointer; no impersonation or writes.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut value = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut bytes = 0;
    // SAFETY: buffer has exactly the requested native structure size; token is owned until CloseHandle.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut value as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )
    };
    let error = (result == 0).then(io::Error::last_os_error);
    // SAFETY: token is the successful OpenProcessToken handle, closed exactly once.
    unsafe { CloseHandle(token) };
    if let Some(error) = error {
        return Err(error);
    }
    Ok(value.TokenIsElevated != 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn privileged() -> io::Result<bool> {
    Ok(false)
}

/// Return None for daemon argv. Administrative argv never falls through to daemon mode.
pub fn run(args: &[String]) -> Option<std::process::ExitCode> {
    if args == ["--print-version"] {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Some(std::process::ExitCode::SUCCESS);
    }
    if args.first().map(String::as_str) != Some("core-install") {
        return None;
    }
    let result = InstallArgs::parse(args)
        .map_err(invalid)
        .and_then(|args| install(&args));
    Some(match result {
        Ok(line) => {
            println!("OK {line}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("core-install: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
mod tests;

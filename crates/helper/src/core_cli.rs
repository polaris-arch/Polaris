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
    pub source_owner: Option<polaris_helper_proto::core_protocol::CallerIdentity>,
}

impl InstallArgs {
    pub fn parse(args: &[String]) -> Result<Self, &'static str> {
        if !matches!(args.len(), 7 | 9) || args[0] != "core-install" {
            return Err("invalid core-install arguments");
        }
        let mut root = None;
        let mut source = None;
        let mut manifest = None;
        for pair in args[1..7].as_chunks::<2>().0 {
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
            source_owner: if args.len() == 9 {
                if args[7] != "--source-owner" {
                    return Err("unknown source owner argument");
                }
                Some(crate::user_control::parse_identity(&args[8]).ok_or("invalid source owner")?)
            } else {
                None
            },
        };
        if value.manifest.file_name().and_then(|name| name.to_str()) != Some("core-manifest.json") {
            return Err("manifest basename must be core-manifest.json");
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
        let observed = std::fs::symlink_metadata(&args.source)?.uid();
        match &args.source_owner {
            Some(polaris_helper_proto::core_protocol::CallerIdentity::Unix(uid))
                if *uid == observed =>
            {
                Some(*uid)
            }
            None => Some(observed),
            _ => return Err(invalid("source owner differs")),
        }
    };
    #[cfg(not(unix))]
    let source_uid = None;
    #[cfg(windows)]
    {
        if let Some(polaris_helper_proto::core_protocol::CallerIdentity::Windows(sid)) =
            &args.source_owner
        {
            use crate::platform::windows::{ops::ProcOps, winproc::WinProcOps};
            let security = WinProcOps::default()
                .read_object_security(&args.source.to_string_lossy())
                .map_err(|_| invalid("source owner unavailable"))?;
            if security.owner_sid.as_deref() != Some(sid.as_str()) {
                return Err(invalid("source owner differs"));
            }
        } else if args.source_owner.is_some() {
            return Err(invalid("invalid native source owner"));
        }
    }
    let manifest_parent = args
        .manifest
        .parent()
        .ok_or_else(|| invalid("manifest parent unavailable"))?;
    OwnerPolicy::privileged().check(manifest_parent)?;
    OwnerPolicy::privileged().check(&args.manifest)?;
    let objects = PayloadDir::open(manifest_parent, OwnerPolicy::privileged().uid)?;
    let (file, bytes) = objects.open_regular("core-manifest.json")?;
    if bytes > 64 * 1024 {
        return Err(invalid("manifest too large"));
    }
    let mut raw = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut raw)?;
    let manifest =
        InstalledCoreManifest::parse(&raw, crate::core_hash::sha256_hex).map_err(invalid)?;
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
    let source_sid = match &args.source_owner {
        Some(polaris_helper_proto::core_protocol::CallerIdentity::Windows(sid)) => {
            Some(sid.as_str())
        }
        _ => None,
    };
    store.install(&manifest, &args.source, source_uid, source_sid)?;
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
    if args.first().map(String::as_str) == Some("register-user") {
        let result = (|| {
            if !privileged()? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "administrator required",
                ));
            }
            if args.len() != 9
                || args[1] != "--support"
                || args[3] != "--identity"
                || args[5] != "--confdir"
                || args[7] != "--token-file"
            {
                return Err(invalid("invalid user registration arguments"));
            }
            let caller = crate::user_control::parse_identity(&args[4])
                .ok_or_else(|| invalid("invalid user identity"))?;
            if !Path::new(&args[2]).is_absolute()
                || !Path::new(&args[6]).is_absolute()
                || !Path::new(&args[8]).is_absolute()
            {
                return Err(invalid("absolute registration paths required"));
            }
            crate::user_control::register(
                Path::new(&args[2]),
                &caller,
                Path::new(&args[6]),
                Path::new(&args[8]),
            )
        })();
        return Some(match result {
            Ok(()) => {
                println!("user-registration-written");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("user registration: {error}");
                std::process::ExitCode::FAILURE
            }
        });
    }
    if args.first().is_some_and(|name| {
        matches!(
            name.as_str(),
            "helper-policy"
                | "helper-record"
                | "prepare-upgrade"
                | "cancel-upgrade"
                | "retain-probe"
        )
    }) {
        let result = (|| {
            if !privileged()? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "administrator required",
                ));
            }
            match args[0].as_str() {
                "retain-probe" => {
                    if args.len() != 5
                        || args[1] != "--support"
                        || args[3] != "--id"
                        || !Path::new(&args[2]).is_absolute()
                    {
                        return Err(invalid("invalid retained helper arguments"));
                    }
                    let id = polaris_helper_proto::core_manifest::CoreId::parse(&args[4])
                        .map_err(invalid)?;
                    crate::admin_probe::retained_ready(Path::new(&args[2]), &id)?;
                    Ok("helper-retain-ready".to_owned())
                }
                "prepare-upgrade" | "cancel-upgrade" => {
                    if args.len() != 3
                        || args[1] != "--support"
                        || !Path::new(&args[2]).is_absolute()
                    {
                        return Err(invalid("invalid upgrade arguments"));
                    }
                    crate::admin_probe::upgrade_admission(
                        Path::new(&args[2]),
                        args[0] == "prepare-upgrade",
                    )?;
                    Ok("helper-upgrade-admission-confirmed".to_owned())
                }
                "helper-policy" | "helper-record" => {
                    if args.len() != 5
                        || args[1] != "--binary"
                        || args[3] != "--metadata"
                        || !Path::new(&args[2]).is_absolute()
                        || !Path::new(&args[4]).is_absolute()
                    {
                        return Err(invalid("invalid helper receipt arguments"));
                    }
                    if args[0] == "helper-policy" {
                        Ok(format!(
                            "helper-policy {}",
                            crate::helper_policy::policy(Path::new(&args[2]), Path::new(&args[4]))?
                        ))
                    } else {
                        crate::helper_policy::record(Path::new(&args[2]), Path::new(&args[4]))?;
                        Ok("helper-record-written".to_owned())
                    }
                }
                _ => unreachable!(),
            }
        })();
        return Some(match result {
            Ok(line) => {
                println!("{line}");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("helper administration: {error}");
                std::process::ExitCode::FAILURE
            }
        });
    }
    if args.first().map(String::as_str) == Some("admin-probe") {
        let result = (|| {
            if args.len() != 3 || args[1] != "--support" || !Path::new(&args[2]).is_absolute() {
                return Err(invalid("invalid admin-probe arguments"));
            }
            if !privileged()? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "administrator required",
                ));
            }
            crate::admin_probe::probe(Path::new(&args[2]))
        })();
        return Some(match result {
            Ok(line) => {
                println!("{line}");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("admin-probe: {error}");
                std::process::ExitCode::FAILURE
            }
        });
    }
    if args == ["--print-version"] {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Some(std::process::ExitCode::SUCCESS);
    }
    if args.first().map(String::as_str) != Some("core-install") {
        if args.first().is_some_and(|name| !name.starts_with('-')) {
            eprintln!("unknown helper subcommand");
            return Some(std::process::ExitCode::FAILURE);
        }
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

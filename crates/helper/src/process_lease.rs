//! Inherited Unix file-description locks for installer/updater scripts.
//! No IPC exposure, child execution, unlink or caller-identity fallback.
use std::io;
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(
    unsafe_code,
    deprecated,
    reason = "native fstat validates an untrusted inherited fd without borrowing an invalid Rust fd"
)]
pub(crate) fn apply(path: &Path, descriptor: i32, release: bool) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if descriptor < 3 || !path.is_absolute() {
        return Err(io::Error::other("invalid inherited lease"));
    }
    let mut value = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    // SAFETY: fstat accepts an invalid integer fd and returns EBADF; the output
    // pointer has room for stat and is read only after successful initialization.
    if unsafe { nix::libc::fstat(descriptor, value.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the complete native stat output.
    let actual = unsafe { value.assume_init() };
    let named = std::fs::symlink_metadata(path)?;
    let uid = nix::unistd::Uid::effective().as_raw();
    #[cfg(target_os = "macos")]
    let actual_dev = u64::try_from(actual.st_dev).ok();
    #[cfg(target_os = "linux")]
    let actual_dev = Some(actual.st_dev);
    if !named.is_file()
        || named.file_type().is_symlink()
        || named.nlink() != 1
        || named.uid() != uid
        || named.mode() & 0o077 != 0
        || actual_dev != Some(named.dev())
        || actual.st_ino != named.ino()
        || actual.st_uid != uid
        || actual.st_nlink != 1
    {
        return Err(io::Error::other("inherited lease file identity differs"));
    }
    nix::fcntl::flock(
        descriptor,
        if release {
            nix::fcntl::FlockArg::Unlock
        } else {
            nix::fcntl::FlockArg::LockExclusiveNonblock
        },
    )?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn apply(_path: &Path, _descriptor: i32, _release: bool) -> io::Result<()> {
    Err(io::Error::other("Unix inherited lease unsupported"))
}

pub fn run(args: &[String]) -> Option<std::process::ExitCode> {
    let release = match args.first().map(String::as_str) {
        Some("lease-acquire") => false,
        Some("lease-release") => true,
        _ => return None,
    };
    let result = (|| {
        if args.len() != 5
            || args[1] != "--lock"
            || args[3] != "--fd"
            || args[2].contains(['\n', '\r', '\0'])
        {
            return Err(io::Error::other("invalid lease arguments"));
        }
        let fd = args[4]
            .parse()
            .map_err(|_| io::Error::other("invalid lease fd"))?;
        apply(Path::new(&args[2]), fd, release)
    })();
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("helper process lease: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

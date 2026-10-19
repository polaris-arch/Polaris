//! Read-only installer ancestry validation. Never normalizes an old installation.
//! Holds every opened component and refuses symlinks, non-root ownership, POSIX
//! write grants or macOS ACL write grants before a migration can use the object.
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

pub struct ProtectedPath {
    pub path: PathBuf,
    pub file: File,
    _ancestors: Vec<File>,
}

impl ProtectedPath {
    pub fn open(path: &Path, directory: bool) -> io::Result<Self> {
        use nix::fcntl::{openat, OFlag};
        use nix::sys::stat::Mode;
        let components: Vec<_> = path.components().collect();
        if components.first() != Some(&Component::RootDir)
            || components.len() > 64
            || components
                .iter()
                .skip(1)
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(rejected("invalid protected path"));
        }
        let mut ancestors = Vec::new();
        let mut current = File::open("/")?;
        validate(&current, true)?;
        for (index, component) in components.iter().enumerate().skip(1) {
            let Component::Normal(name) = component else {
                unreachable!()
            };
            let is_directory = index + 1 < components.len() || directory;
            let mut flags =
                OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
            if is_directory {
                flags |= OFlag::O_DIRECTORY;
            }
            let next = File::from(openat(&current, Path::new(name), flags, Mode::empty())?);
            validate(&next, is_directory)?;
            ancestors.push(current);
            current = next;
        }
        validate(&current, directory)?;
        Ok(Self {
            path: path.to_owned(),
            file: current,
            _ancestors: ancestors,
        })
    }

    pub fn unchanged(&self) -> io::Result<()> {
        let original = self.file.metadata()?;
        let current = Self::open(&self.path, original.is_dir())?;
        let observed = current.file.metadata()?;
        if original.dev() != observed.dev() || original.ino() != observed.ino() {
            return Err(rejected("protected path identity changed"));
        }
        Ok(())
    }
}

fn rejected(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}

fn validate(file: &File, directory: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(rejected("unprotected installation object"));
    }
    #[cfg(target_os = "macos")]
    validate_acl(file, false)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn no_extended_acl(file: &File) -> io::Result<()> {
    validate_acl(file, true)
}

// Darwin ACL grants can exceed mode bits. Deny ACEs do not grant authority;
// any allow ACE with a mutation permission is rejected, even for root, rather
// than attempting to infer membership or inheritance across identities.
#[cfg(any(target_os = "macos", test))]
fn grants_mutation(tag: u32, permissions: u32) -> bool {
    const MUTATION: u32 =
        (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8) | (1 << 10) | (1 << 12) | (1 << 13);
    tag == 1 && permissions & MUTATION != 0
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "bounded native ACL read using held file descriptor"
)]
fn validate_acl(file: &File, require_empty: bool) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;
    // Apple Libc include/sys/acl.h: ACL_TYPE_EXTENDED=0x100, FIRST=0,
    // NEXT=-1; acl_get_entry succeeds with 0 (not POSIX/Linux's 1).
    unsafe extern "C" {
        fn acl_valid(acl: *mut c_void) -> i32;
        fn acl_get_entry(acl: *mut c_void, id: i32, entry: *mut *mut c_void) -> i32;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut u32) -> i32;
        fn acl_get_permset(entry: *mut c_void, set: *mut *mut c_void) -> i32;
        fn acl_get_perm_np(set: *mut c_void, permission: u32) -> i32;
        fn acl_free(acl: *mut c_void) -> i32;
    }
    let Some(acl) = read_acl(file.as_raw_fd())? else {
        return Ok(());
    };
    let result = (|| {
        // SAFETY: non-null ACL was returned by the matching native API.
        if unsafe { acl_valid(acl) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for index in 0..=128 {
            let mut entry = std::ptr::null_mut();
            // SAFETY: valid ACL and writable output; entry stays owned by acl.
            if unsafe { acl_get_entry(acl, if index == 0 { 0 } else { -1 }, &mut entry) } != 0 {
                let error = io::Error::last_os_error();
                // For a valid ACL and FIRST/NEXT, EINVAL marks exhausted entries.
                if error.raw_os_error() == Some(nix::libc::EINVAL) {
                    return Ok(());
                }
                return Err(error);
            }
            if index == 128 || entry.is_null() {
                return Err(rejected("ACL enumeration incomplete"));
            }
            if require_empty {
                return Err(rejected("noncanonical private-file ACL"));
            }
            let mut tag = 0;
            let mut set = std::ptr::null_mut();
            // SAFETY: entry is an ACL-owned entry and outputs are valid locals.
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0
                || unsafe { acl_get_permset(entry, &mut set) } != 0
                || set.is_null()
            {
                return Err(io::Error::last_os_error());
            }
            if !matches!(tag, 1 | 2) {
                return Err(rejected("unknown ACL entry type"));
            }
            let mut permissions = 0;
            for bit in [2, 4, 5, 6, 8, 10, 12, 13] {
                // SAFETY: native permset remains valid while acl is retained.
                match unsafe { acl_get_perm_np(set, 1 << bit) } {
                    0 => {}
                    1 => permissions |= 1 << bit,
                    _ => return Err(io::Error::last_os_error()),
                }
            }
            if grants_mutation(tag, permissions) {
                return Err(rejected("ACL grants installation mutation"));
            }
        }
        Err(rejected("ACL enumeration incomplete"))
    })();
    // SAFETY: acl is the owned result above; no entry is accessed afterwards.
    let freed = unsafe { acl_free(acl) };
    if freed != 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

/// Separate a successful native "no ACL" observation from syscall failure.
/// acl_get_fd_np collapses both into NULL, so use its underlying filesec query.
#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "read-only Darwin fstatx/filesec ABI with owned native results"
)]
fn read_acl(fd: i32) -> io::Result<Option<*mut std::ffi::c_void>> {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn filesec_init() -> *mut c_void;
        fn filesec_free(value: *mut c_void);
        #[cfg_attr(target_arch = "x86_64", link_name = "fstatx_np$INODE64")]
        fn fstatx_np(fd: i32, metadata: *mut nix::libc::stat, value: *mut c_void) -> i32;
        fn filesec_query_property(value: *mut c_void, property: i32, present: *mut i32) -> i32;
        fn filesec_get_property(value: *mut c_void, property: i32, output: *mut c_void) -> i32;
    }
    // SAFETY: allocation has no input; its sole owned result is freed below.
    let security = unsafe { filesec_init() };
    if security.is_null() {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        // SAFETY: SDK stat layout is writable, exact size; fd is only queried.
        let mut metadata: nix::libc::stat = unsafe { std::mem::zeroed() };
        let status = unsafe {
            *nix::libc::__error() = 0;
            fstatx_np(fd, &mut metadata, security)
        };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut present = 0;
        // SAFETY: security is initialized by successful fstatx; FILESEC_ACL=5.
        if unsafe { filesec_query_property(security, 5, &mut present) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if present == 0 {
            return Ok(None);
        }
        let mut acl: *mut c_void = std::ptr::null_mut();
        // SAFETY: output receives an independently allocated ACL, later acl_free'd.
        if unsafe { filesec_get_property(security, 5, (&mut acl as *mut *mut c_void).cast()) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if acl.is_null() || acl as usize == 1 {
            return Err(rejected("invalid native ACL result"));
        }
        Ok(Some(acl))
    })();
    // SAFETY: free only the filesec object; the copied ACL has independent ownership.
    unsafe { filesec_free(security) };
    result
}

#[cfg(test)]
mod tests;

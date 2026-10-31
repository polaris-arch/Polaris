use super::*;
use nix::fcntl::{fcntl, FcntlArg, SealFlag};
use sha2::Digest;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionStage {
    CopiedChunk,
    BeforeSeal,
    Sealed,
    Hashed,
}
#[cfg(test)]
type Hook = Box<dyn FnMut(AdmissionStage, &str, &File) -> Result<(), AdmissionError>>;
#[cfg(test)]
thread_local! {static TEST_HOOK:std::cell::RefCell<Option<Hook>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
fn observe(stage: AdmissionStage, name: &str, file: &File) -> Result<(), AdmissionError> {
    TEST_HOOK.with(|cell| match cell.borrow_mut().as_mut() {
        Some(hook) => hook(stage, name, file),
        None => Ok(()),
    })
}
#[cfg(test)]
pub(super) fn with_hook<R>(hook: Hook, action: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_HOOK.with(|h| *h.borrow_mut() = None);
        }
    }
    TEST_HOOK.with(|h| {
        assert!(h.borrow().is_none());
        *h.borrow_mut() = Some(hook);
    });
    let _reset = Reset;
    action()
}

const MAX_MEMBER: u64 = 96 * 1024 * 1024;
fn stamp(m: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32, u32, u32, u64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.mode(),
        m.uid(),
        m.gid(),
        m.nlink(),
    )
}
fn security_stamp(m: &std::fs::Metadata) -> (u64, u64, u32, u32, u32) {
    (m.dev(), m.ino(), m.mode(), m.uid(), m.gid())
}
#[allow(
    unsafe_code,
    reason = "bounded fgetxattr observes Linux POSIX ACLs on already-held directory objects"
)]
fn acl(file: &File) -> Result<Vec<u8>, AdmissionError> {
    let mut bytes = [0u8; 4096];
    // SAFETY: both buffers are valid and the descriptor is borrowed for this
    // synchronous call. The fixed bound rejects oversized/unknown protection.
    let length = unsafe {
        nix::libc::fgetxattr(
            file.as_raw_fd(),
            b"system.posix_acl_access\0".as_ptr().cast(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if length < 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(nix::libc::ENODATA) | Some(nix::libc::EOPNOTSUPP)
        ) {
            return Ok(Vec::new());
        }
        return Err(AdmissionError::new(
            AdmissionErrorKind::Unknown,
            format!("cannot observe POSIX ACL: {error}"),
        ));
    }
    Ok(bytes[..length as usize].to_vec())
}
#[derive(Debug)]
struct Ancestor {
    file: File,
    stamp: (u64, u64, u32, u32, u32),
    acl: Vec<u8>,
}
impl Ancestor {
    fn capture(file: &File) -> Result<Self, AdmissionError> {
        Ok(Self {
            file: file.try_clone()?,
            stamp: security_stamp(&file.metadata()?),
            acl: acl(file)?,
        })
    }
    fn current(&self) -> bool {
        self.file
            .metadata()
            .is_ok_and(|m| security_stamp(&m) == self.stamp)
            && acl(&self.file).is_ok_and(|a| a == self.acl)
    }
}
#[derive(Debug)]
struct Observation {
    ancestors: Vec<Ancestor>,
    path: PathBuf,
    source: File,
    acl: Vec<u8>,
    stamp: (u64, u64, u64, i64, i64, i64, i64, u32, u32, u32, u64),
}
#[derive(Debug)]
struct Member {
    sealed: File,
    security: (u64, u64, u32, u32, u32),
    acl: Vec<u8>,
    source: Observation,
}
#[derive(Debug)]
pub(super) struct ProtectedMembers {
    members: Vec<Member>,
    directory: Observation,
    binary: usize,
    sidecar: usize,
    #[cfg(test)]
    counters: (u64, u64, u64),
}

fn open_source(path: &Path, directory: bool) -> Result<Observation, AdmissionError> {
    // O_NONBLOCK prevents a hostile FIFO/device from hanging admission. Every
    // component is opened relative to the held, no-follow directory chain.
    let mut parent = File::open("/")?;
    if !path.is_absolute() {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Mismatch,
            "absolute payload path required",
        ));
    }
    let names: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::RootDir => None,
            std::path::Component::Normal(n) => Some(Ok(n)),
            std::path::Component::ParentDir => Some(Ok(std::ffi::OsStr::new(".."))),
            std::path::Component::CurDir => None,
            _ => Some(Err(())),
        })
        .collect::<Result<_, _>>()
        .map_err(|_| {
            AdmissionError::new(
                AdmissionErrorKind::Mismatch,
                "normalized payload path required",
            )
        })?;
    let mut ancestors = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        ancestors.push(Ancestor::capture(&parent)?);
        let last = i + 1 == names.len();
        let flags = nix::fcntl::OFlag::O_RDONLY
            | nix::fcntl::OFlag::O_CLOEXEC
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_NONBLOCK
            | if !last || directory {
                nix::fcntl::OFlag::O_DIRECTORY
            } else {
                nix::fcntl::OFlag::empty()
            };
        parent = File::from(
            nix::fcntl::openat(&parent, *name, flags, nix::sys::stat::Mode::empty()).map_err(
                |error| match error {
                    nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => AdmissionError::new(
                        AdmissionErrorKind::Mismatch,
                        "payload namespace contains a link or non-directory",
                    ),
                    other => AdmissionError::from(std::io::Error::from(other)),
                },
            )?,
        );
    }
    let metadata = parent.metadata()?;
    if (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Mismatch,
            "payload object is not a single regular member",
        ));
    }
    Ok(Observation {
        ancestors,
        path: path.into(),
        stamp: stamp(&metadata),
        acl: acl(&parent)?,
        source: parent,
    })
}
impl Observation {
    fn is_current(&self) -> bool {
        self.ancestors.iter().all(Ancestor::current)
            && acl(&self.source).is_ok_and(|a| a == self.acl)
            && self
                .source
                .metadata()
                .is_ok_and(|m| stamp(&m) == self.stamp)
            && std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| !m.file_type().is_symlink() && stamp(&m) == self.stamp)
            && open_source(&self.path, self.source.metadata().is_ok_and(|m| m.is_dir())).is_ok_and(
                |o| {
                    o.stamp == self.stamp
                        && o.acl == self.acl
                        && o.ancestors.len() == self.ancestors.len()
                        && o.ancestors
                            .iter()
                            .zip(&self.ancestors)
                            .all(|(a, b)| a.stamp == b.stamp && a.acl == b.acl)
                },
            )
    }
}
fn required_seals() -> SealFlag {
    SealFlag::F_SEAL_WRITE | SealFlag::F_SEAL_GROW | SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_SEAL
}
pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    if input.binary.file_name() != Some(std::ffi::OsStr::new("sing-box")) {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Mismatch,
            "unexpected selected executable name",
        ));
    }
    if policy.loader != "linux-sealed-fd-v1" {
        return Err(AdmissionError::new(
            AdmissionErrorKind::LoaderPolicy,
            "sealed FD policy required",
        ));
    }
    let dir = input.binary.parent().ok_or_else(|| {
        AdmissionError::new(AdmissionErrorKind::Mismatch, "missing member directory")
    })?;
    let directory = open_source(dir, true)?;
    let mut members = Vec::with_capacity(policy.manifest.files.len());
    #[cfg(test)]
    let mut counters = (0u64, 0u64, 0u64);
    let mut binary = None;
    let mut sidecar = None;
    for expected in &policy.manifest.files {
        let member = (|| -> Result<Member, AdmissionError> {
            if expected.bytes > MAX_MEMBER {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Mismatch,
                    "selected member exceeds fixed stream budget",
                ));
            }
            let mut source = open_source(&dir.join(&expected.name), false)?;
            if source.source.metadata()?.len() != expected.bytes {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Mismatch,
                    "member size differs",
                ));
            }
            let name = std::ffi::CString::new("polaris-admitted-member").expect("constant name");
            let fd = nix::sys::memfd::memfd_create(
                name.as_c_str(),
                nix::sys::memfd::MFdFlags::MFD_ALLOW_SEALING
                    | nix::sys::memfd::MFdFlags::MFD_CLOEXEC,
            )
            .map_err(std::io::Error::from)?;
            let mut sealed = File::from(fd);
            let mut total = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let remaining = expected.bytes + 1 - total;
                if remaining == 0 {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Mismatch,
                        "member grew beyond selected size",
                    ));
                }
                let limit = buffer.len().min(remaining as usize);
                let count = source.source.read(&mut buffer[..limit])?;
                if count == 0 {
                    break;
                }
                #[cfg(test)]
                {
                    counters.1 += count as u64;
                }
                total += count as u64;
                sealed.write_all(&buffer[..count])?;
                #[cfg(test)]
                observe(AdmissionStage::CopiedChunk, &expected.name, &sealed)?;
            }
            if total != expected.bytes {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Mismatch,
                    "copied member size differs",
                ));
            }
            #[cfg(test)]
            observe(AdmissionStage::BeforeSeal, &expected.name, &sealed)?;
            // Helper drops to the authenticated caller before exec. The sealed
            // core must remain executable and its sidecar readable by that uid;
            // mode is explicit rather than relying on memfd defaults/umask.
            sealed
                .set_permissions(std::fs::Permissions::from_mode(
                    if expected.name == "sing-box" {
                        0o555
                    } else {
                        0o444
                    },
                ))
                .map_err(|e| {
                    if matches!(
                        e.raw_os_error(),
                        Some(nix::libc::EPERM) | Some(nix::libc::EACCES)
                    ) {
                        AdmissionError::new(
                            AdmissionErrorKind::UnsupportedExactObject,
                            "kernel policy does not permit the required sealed-object access",
                        )
                    } else {
                        AdmissionError::from(e)
                    }
                })?;
            fcntl(&sealed, FcntlArg::F_ADD_SEALS(required_seals())).map_err(|e| {
                AdmissionError::new(
                    AdmissionErrorKind::UnsupportedExactObject,
                    format!("required exact-object seals unavailable: {e}"),
                )
            })?;
            let seals = fcntl(&sealed, FcntlArg::F_GET_SEALS).map_err(std::io::Error::from)?;
            if seals & required_seals().bits() != required_seals().bits() {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::UnsupportedExactObject,
                    "required seals were not established",
                ));
            }
            #[cfg(test)]
            observe(AdmissionStage::Sealed, &expected.name, &sealed)?;
            if sealed.metadata()?.len() != expected.bytes {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Mismatch,
                    "sealed object size differs",
                ));
            }
            // The ONLY full hash reads this same object AFTER immutable seals
            // are established. Copy-stage FD writes cannot escape verification.
            // This adds a bounded RAM read on cold admission, not another disk
            // copy/hash, and is never repeated on a valid warm cohort.
            sealed.rewind()?;
            let mut hash = sha2::Sha256::new();
            let mut hashed = 0u64;
            loop {
                let count = sealed.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hashed += count as u64;
                if hashed > expected.bytes {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Mismatch,
                        "sealed object exceeds hash budget",
                    ));
                }
                hash.update(&buffer[..count]);
            }
            #[cfg(test)]
            {
                counters.0 += 1;
            }
            if hashed != expected.bytes || super::lowercase_hex(&hash.finalize()) != expected.sha256
            {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Mismatch,
                    "sealed object digest differs",
                ));
            }
            #[cfg(test)]
            observe(AdmissionStage::Hashed, &expected.name, &sealed)?;
            #[cfg(test)]
            {
                counters.2 += 1;
            }
            if !source.is_current() {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::Changed,
                    "source changed while sealing",
                ));
            }
            match expected.name.as_str() {
                "sing-box" => binary = Some(members.len()),
                "libcronet.so" => sidecar = Some(members.len()),
                _ => {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Mismatch,
                        "unexpected Linux member",
                    ))
                }
            }
            let security = security_stamp(&sealed.metadata()?);
            let acl = acl(&sealed)?;
            Ok(Member {
                sealed,
                security,
                acl,
                source,
            })
        })()
        .map_err(|mut error| {
            error.member = Some(expected.name.clone());
            error
        })?;
        members.push(member);
    }
    let protected = ProtectedMembers {
        members,
        directory,
        binary: binary
            .ok_or_else(|| AdmissionError::new(AdmissionErrorKind::Missing, "missing core"))?,
        sidecar: sidecar
            .ok_or_else(|| AdmissionError::new(AdmissionErrorKind::Missing, "missing sidecar"))?,
        #[cfg(test)]
        counters,
    };
    if !protected.is_current() {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Changed,
            "cohort changed while sealing",
        ));
    }
    Ok(protected)
}
impl ProtectedMembers {
    pub(super) fn binary_for_custody(&self) -> std::io::Result<File> {
        rustix::io::fcntl_dupfd_cloexec(&self.members[self.binary].sealed, 3)
            .map(File::from)
            .map_err(std::io::Error::from)
    }
    #[cfg(test)]
    pub(super) fn counters(&self) -> (u64, u64, u64) {
        self.counters
    }
    pub(super) fn is_current(&self) -> bool {
        self.directory.is_current()
            && self.members.iter().all(|m| {
                m.source.is_current()
                    && m.sealed
                        .metadata()
                        .is_ok_and(|s| security_stamp(&s) == m.security)
                    && acl(&m.sealed).is_ok_and(|a| a == m.acl)
                    && fcntl(&m.sealed, FcntlArg::F_GET_SEALS).is_ok_and(|actual| {
                        actual & required_seals().bits() == required_seals().bits()
                    })
            })
    }
    #[allow(
        unsafe_code,
        reason = "pre_exec only clears CLOEXEC on child-specific duplicates with async-signal-safe fcntl"
    )]
    pub(super) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        use std::os::unix::process::CommandExt;
        // Both parent owners AND these per-child duplicates remain CLOEXEC in
        // the parent. No global FD ever becomes inheritable in another spawn.
        let binary: OwnedFd = rustix::io::fcntl_dupfd_cloexec(&self.members[self.binary].sealed, 3)
            .map_err(std::io::Error::from)?;
        let sidecar: OwnedFd =
            rustix::io::fcntl_dupfd_cloexec(&self.members[self.sidecar].sealed, 3)
                .map_err(std::io::Error::from)?;
        let bin_fd = binary.as_raw_fd();
        let lib_fd = sidecar.as_raw_fd();
        let mut cmd = std::process::Command::new(format!("/proc/self/fd/{bin_fd}"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("POLARIS_CRONET_LIBRARY_FD", lib_fd.to_string());
        // SAFETY: only fcntl on preallocated child duplicates occurs after fork;
        // errors use the standard Command exec-error pipe. Captures keep both
        // descriptors open until spawn returns, including a failed exec.
        unsafe {
            cmd.pre_exec(move || {
                let _owners = (&binary, &sidecar);
                for fd in [bin_fd, lib_fd] {
                    if nix::libc::fcntl(fd, nix::libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(tokio::process::Command::from(cmd))
    }
}

/// A lease is occupancy data accompanying an already admitted Run. Duplicate
/// the same open-file-description; reopening its pathname would lose the lock.
/// The parent's descriptors stay CLOEXEC, including on a failed exec.
#[allow(
    unsafe_code,
    reason = "pre_exec clears only a dedicated child lease duplicate"
)]
pub(super) fn attach_store_lease(
    command: &mut std::process::Command,
    lease: &File,
) -> Result<(), AdmissionError> {
    use std::os::unix::process::CommandExt;
    let metadata = lease.metadata()?;
    if !metadata.is_file() || metadata.len() != 0 || metadata.nlink() != 1 {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Mismatch,
            "invalid store lease object",
        ));
    }
    if fcntl(lease, FcntlArg::F_GETFD).map_err(std::io::Error::from)? & nix::libc::FD_CLOEXEC == 0 {
        return Err(AdmissionError::new(
            AdmissionErrorKind::LoaderPolicy,
            "parent store lease must remain CLOEXEC",
        ));
    }
    let owned = rustix::io::fcntl_dupfd_cloexec(lease, 3).map_err(std::io::Error::from)?;
    let fd = owned.as_raw_fd();
    command.env("POLARIS_CORE_STORE_LEASE_FD", fd.to_string());
    // SAFETY: allocation and duplication happen before fork. fcntl is async-
    // signal-safe and Command retains its ordinary exec-error pipe semantics.
    unsafe {
        command.pre_exec(move || {
            let _owner = &owned;
            if nix::libc::fcntl(fd, nix::libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[allow(
    unsafe_code,
    reason = "Helper child identity and capability changes require pre_exec syscalls"
)]
pub(super) fn attach_helper_identity(
    command: &mut std::process::Command,
    identity: LinuxHelperIdentity,
) {
    use std::os::unix::process::CommandExt;
    #[repr(C)]
    struct CapHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    // SAFETY: inputs are fully allocated before fork. The closure only calls
    // prctl/setgroups/setgid/setuid/capget/capset with stack-resident buffers.
    // Unlike caps::raise, this does not allocate between fork and exec.
    unsafe {
        command.pre_exec(move || {
            let fail = || std::io::Error::last_os_error();
            if nix::libc::prctl(nix::libc::PR_SET_KEEPCAPS, 1, 0, 0, 0) == -1 {
                return Err(fail());
            }
            if nix::libc::setgroups(identity.groups.len(), identity.groups.as_ptr()) == -1 {
                return Err(fail());
            }
            if nix::libc::setgid(identity.gid) == -1 || nix::libc::setuid(identity.uid) == -1 {
                return Err(fail());
            }
            let mut header = CapHeader {
                version: 0x20080522,
                pid: 0,
            };
            let mut caps = [CapData::default(); 2];
            if nix::libc::syscall(
                nix::libc::SYS_capget,
                &mut header as *mut CapHeader,
                caps.as_mut_ptr(),
            ) == -1
            {
                return Err(fail());
            }
            caps[0].inheritable |= (1 << 10) | (1 << 12) | (1 << 13);
            if nix::libc::syscall(
                nix::libc::SYS_capset,
                &header as *const CapHeader,
                caps.as_ptr(),
            ) == -1
            {
                return Err(fail());
            }
            for capability in [10, 12, 13] {
                if nix::libc::prctl(
                    nix::libc::PR_CAP_AMBIENT,
                    nix::libc::PR_CAP_AMBIENT_RAISE,
                    capability,
                    0,
                    0,
                ) == -1
                {
                    return Err(fail());
                }
            }
            Ok(())
        });
    }
}

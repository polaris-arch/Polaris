//! Root Helper RunMain only. A read-only FD to an old data inode is not a
//! protection claim: each cold owner creates and hashes its own private inode.
use super::*;
use crate::protected_install_path::{no_extended_acl, ProtectedPath};
use nix::fcntl::{openat, OFlag};
use nix::sys::stat::{fchmod, mkdirat, Mode};
use nix::unistd::{unlinkat, UnlinkatFlags};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

const MAX_MEMBER: u64 = 96 * 1024 * 1024;
const LOADER: &str = "macos-root-private-v1";
fn rejected(kind: AdmissionErrorKind, detail: &str) -> AdmissionError {
    AdmissionError::new(kind, detail)
}
#[allow(
    unsafe_code,
    reason = "read actual real/effective kernel uid, never a caller-selected scope"
)]
pub(super) fn require_root() -> Result<(), AdmissionError> {
    // SAFETY: getuid/geteuid have no pointer arguments or side effects.
    if unsafe { nix::libc::getuid() != 0 || nix::libc::geteuid() != 0 } {
        return Err(rejected(
            AdmissionErrorKind::UnsupportedExactObject,
            "private execution requires the existing root Helper",
        ));
    }
    Ok(())
}
#[derive(Debug, PartialEq, Eq, Clone)]
struct Stamp(u64, u64, u64, i64, i64, i64, i64, u32, u32, u32, u64);
fn stamp(m: &std::fs::Metadata) -> Stamp {
    Stamp(
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
#[derive(Debug)]
struct Member {
    file: File,
    path: PathBuf,
    stamp: Stamp,
}
impl Member {
    fn current(&self) -> bool {
        self.file.metadata().is_ok_and(|m| stamp(&m) == self.stamp)
            && std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| !m.file_type().is_symlink() && stamp(&m) == self.stamp)
            && no_extended_acl(&self.file).is_ok()
    }
}
#[derive(Debug)]
struct Generation {
    parent: ProtectedPath,
    directory: File,
    name: String,
    path: PathBuf,
    stamp: Stamp,
    member_identity: Option<(u64, u64)>,
}
impl Generation {
    fn current(&self) -> bool {
        self.parent.unchanged().is_ok()
            && no_extended_acl(&self.parent.file).is_ok()
            && self
                .directory
                .metadata()
                .is_ok_and(|m| stamp(&m) == self.stamp)
            && std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| !m.file_type().is_symlink() && stamp(&m) == self.stamp)
            && no_extended_acl(&self.directory).is_ok()
    }
}
impl Drop for Generation {
    fn drop(&mut self) {
        // Never sweep a previous daemon's generation or a replaced namespace.
        // The last execution/reader owner reaches here only after actual wait.
        if !self.current() {
            log::warn!("private Core generation cleanup unconfirmed: namespace changed");
            return;
        }
        match openat(
            &self.directory,
            "sing-box",
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                if !file.metadata().is_ok_and(|m| {
                    m.is_file()
                        && m.uid() == 0
                        && m.nlink() == 1
                        && self.member_identity == Some((m.dev(), m.ino()))
                }) || no_extended_acl(&file).is_err()
                {
                    log::warn!("private Core generation cleanup unconfirmed: member changed");
                    return;
                }
                if let Err(error) =
                    unlinkat(&self.directory, "sing-box", UnlinkatFlags::NoRemoveDir)
                {
                    log::warn!("private Core member retirement failed: {error}");
                    return;
                }
            }
            Err(nix::errno::Errno::ENOENT) => {}
            Err(error) => {
                log::warn!("private Core member retirement unconfirmed: {error}");
                return;
            }
        }
        if let Err(error) = unlinkat(
            &self.parent.file,
            self.name.as_str(),
            UnlinkatFlags::RemoveDir,
        ) {
            log::warn!("private Core generation retirement failed: {error}");
        }
    }
}
#[derive(Debug)]
pub(super) struct ProtectedMembers {
    // Field order closes the member before retiring its exact private directory.
    binary: Member,
    source: ProtectedPath,
    source_stamp: Stamp,
    generation: Generation,
    system: Vec<ProtectedPath>,
    pub(super) lease: ProtectedPath,
}
fn private_mode(file: &File, directory: bool, mode: u32) -> Result<(), AdmissionError> {
    let m = file.metadata()?;
    if m.uid() != 0
        || m.mode() & 0o7777 != mode
        || (directory && !m.is_dir())
        || (!directory && (!m.is_file() || m.nlink() != 1))
    {
        return Err(rejected(
            AdmissionErrorKind::Changed,
            "private Core object protection differs",
        ));
    }
    no_extended_acl(file)?;
    Ok(())
}
fn open_flags(directory: bool) -> OFlag {
    OFlag::O_RDONLY
        | OFlag::O_NOFOLLOW
        | OFlag::O_CLOEXEC
        | OFlag::O_NONBLOCK
        | if directory {
            OFlag::O_DIRECTORY
        } else {
            OFlag::empty()
        }
}
fn create_generation(parent: ProtectedPath) -> Result<Generation, AdmissionError> {
    // This must precede mkdir: no inherited ACL may expose the new inode during
    // construction. We reject rather than chmod/chown/normalize an old object.
    parent.unchanged()?;
    no_extended_acl(&parent.file)?;
    let mut entropy = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let name = format!(".execution-{}", lowercase_hex(&entropy));
    mkdirat(&parent.file, name.as_str(), Mode::from_bits_truncate(0o700))
        .map_err(std::io::Error::from)?;
    let directory = File::from(
        openat(&parent.file, name.as_str(), open_flags(true), Mode::empty())
            .map_err(std::io::Error::from)?,
    );
    private_mode(&directory, true, 0o700)?;
    let path = parent.path.join(&name);
    let stamp = stamp(&directory.metadata()?);
    Ok(Generation {
        parent,
        directory,
        name,
        path,
        stamp,
        member_identity: None,
    })
}
fn copy_checked(
    source: &mut File,
    output: &mut File,
    expected: &polaris_helper_proto::core_manifest::CoreFile,
) -> Result<(), AdmissionError> {
    use sha2::Digest;
    if expected.bytes == 0
        || expected.bytes > MAX_MEMBER
        || source.metadata()?.len() != expected.bytes
    {
        return Err(rejected(
            AdmissionErrorKind::Mismatch,
            "selected Core size exceeds fixed stream budget",
        ));
    }
    source.seek(SeekFrom::Start(0))?;
    let copied = std::io::copy(&mut source.take(expected.bytes + 1), output)?;
    if copied != expected.bytes {
        return Err(rejected(
            AdmissionErrorKind::Changed,
            "Core data changed during private copy",
        ));
    }
    output.sync_all()?;
    output.seek(SeekFrom::Start(0))?;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let size = output.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        length += size as u64;
        if length > expected.bytes {
            return Err(rejected(AdmissionErrorKind::Changed, "private output grew"));
        }
        hash.update(&buffer[..size]);
    }
    if length != expected.bytes || lowercase_hex(&hash.finalize()) != expected.sha256 {
        return Err(rejected(
            AdmissionErrorKind::Mismatch,
            "private output differs from compiled Core",
        ));
    }
    Ok(())
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, AdmissionError> {
    let raw = bytes.get(offset..offset + 4).ok_or_else(|| {
        rejected(
            AdmissionErrorKind::LoaderPolicy,
            "truncated Mach-O metadata",
        )
    })?;
    Ok(u32::from_le_bytes(raw.try_into().expect("four bytes")))
}
fn command_string(bytes: &[u8], offset: usize) -> Result<&str, AdmissionError> {
    let raw = bytes.get(offset..).ok_or_else(|| {
        rejected(
            AdmissionErrorKind::LoaderPolicy,
            "invalid Mach-O loader offset",
        )
    })?;
    let end = raw.iter().position(|c| *c == 0).ok_or_else(|| {
        rejected(
            AdmissionErrorKind::LoaderPolicy,
            "unterminated Mach-O loader name",
        )
    })?;
    let value = std::str::from_utf8(&raw[..end])
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            rejected(
                AdmissionErrorKind::LoaderPolicy,
                "invalid Mach-O loader name",
            )
        })?;
    // No relative, parent, empty, or environment/search-based load names.
    if !value.starts_with('/')
        || Path::new(value).components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(rejected(
            AdmissionErrorKind::LoaderPolicy,
            "non-absolute Mach-O dependency",
        ));
    }
    Ok(value)
}
fn validate_macho(file: &mut File) -> Result<(), AdmissionError> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let cpu = if cfg!(target_arch = "aarch64") {
        0x0100000c
    } else {
        0x01000007
    };
    let count = u32_at(&header, 16)? as usize;
    let size = u32_at(&header, 20)? as usize;
    if u32_at(&header, 0)? != 0xfeedfacf
        || u32_at(&header, 4)? != cpu
        || u32_at(&header, 12)? != 2
        || count == 0
        || count > 4096
        || size > 1024 * 1024
    {
        return Err(rejected(
            AdmissionErrorKind::LoaderPolicy,
            "unsupported native Mach-O execution graph",
        ));
    }
    let mut commands = vec![0; size];
    file.read_exact(&mut commands)?;
    let mut offset = 0usize;
    let mut signature = false;
    let mut linker = false;
    for _ in 0..count {
        let id = u32_at(&commands, offset)?;
        let length = u32_at(&commands, offset + 4)? as usize;
        if length < 8 || length % 8 != 0 || length > commands.len().saturating_sub(offset) {
            return Err(rejected(
                AdmissionErrorKind::LoaderPolicy,
                "invalid Mach-O load command",
            ));
        }
        let command = &commands[offset..offset + length];
        match id {
            0xc | 0x80000018 | 0x8000001f | 0x80000023 | 0x20 => {
                if length < 24 || u32_at(command, 8)? < 24 {
                    return Err(rejected(
                        AdmissionErrorKind::LoaderPolicy,
                        "invalid dylib command",
                    ));
                }
                let name = command_string(command, u32_at(command, 8)? as usize)?;
                if !name.starts_with("/usr/lib/") && !name.starts_with("/System/Library/") {
                    return Err(rejected(
                        AdmissionErrorKind::LoaderPolicy,
                        "non-system dynamic dependency requires its own protection proof",
                    ));
                }
            }
            0xe => {
                if command_string(command, u32_at(command, 8)? as usize)? != "/usr/lib/dyld" {
                    return Err(rejected(
                        AdmissionErrorKind::LoaderPolicy,
                        "unexpected dynamic linker",
                    ));
                }
                linker = true;
            }
            0x8000001c | 0x27 | 0x10 | 0x21 | 0x2c => {
                return Err(rejected(
                    AdmissionErrorKind::LoaderPolicy,
                    "search, environment or encrypted Mach-O graph unsupported",
                ));
            }
            0x1d => {
                let start = u32_at(command, 8)? as u64;
                let size = u32_at(command, 12)? as u64;
                if signature
                    || size == 0
                    || start < 32 + commands.len() as u64
                    || start
                        .checked_add(size)
                        .is_none_or(|end| file.metadata().map_or(true, |m| end > m.len()))
                {
                    return Err(rejected(
                        AdmissionErrorKind::LoaderPolicy,
                        "invalid post-sign Core range",
                    ));
                }
                signature = true;
            }
            // Known non-search metadata in the compiled thin Mach-O profile.
            0x19 | 0x2 | 0xb | 0x1b | 0x32 | 0x2a | 0x80000028 | 0x26 | 0x29 | 0x80000022
            | 0x80000033 | 0x80000034 | 0x24 | 0x4 | 0x5 => {}
            _ => {
                return Err(rejected(
                    AdmissionErrorKind::LoaderPolicy,
                    "unrecognized Mach-O load command requires a loader review",
                ));
            }
        }
        offset += length;
    }
    if offset != commands.len() || !signature || !linker {
        return Err(rejected(
            AdmissionErrorKind::LoaderPolicy,
            "incomplete signed native loader graph",
        ));
    }
    Ok(())
}
pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    require_root()?;
    if !input.root_helper
        || policy.loader != LOADER
        || policy.purposes != [ExecPurpose::RunMain]
        || policy.manifest.files.len() != 1
        || input.binary.file_name() != Some(std::ffi::OsStr::new("sing-box"))
    {
        return Err(rejected(
            AdmissionErrorKind::UnsupportedCapability,
            "only compiled root Helper RunMain is admitted",
        ));
    }
    let directory = input
        .binary
        .parent()
        .ok_or_else(|| rejected(AdmissionErrorKind::Mismatch, "missing store version"))?;
    let versions = directory
        .parent()
        .ok_or_else(|| rejected(AdmissionErrorKind::Mismatch, "missing store root"))?;
    if directory.file_name() != Some(std::ffi::OsStr::new(policy.id.as_str()))
        || versions.file_name() != Some(std::ffi::OsStr::new("versions"))
    {
        return Err(rejected(
            AdmissionErrorKind::Mismatch,
            "execution input is not the compiled CoreStore selection",
        ));
    }
    let store = versions
        .parent()
        .ok_or_else(|| rejected(AdmissionErrorKind::Mismatch, "missing root store"))?;
    let source = ProtectedPath::open(&input.binary, false)?;
    let source_stamp = stamp(&source.file.metadata()?);
    let parent = ProtectedPath::open(store, true)?;
    let lease = ProtectedPath::open(
        &store
            .join("leases")
            .join(format!("{}.lock", policy.id.as_str())),
        false,
    )?;
    let mut generation = create_generation(parent)?;
    let expected = &policy.manifest.files[0];
    if expected.name != "sing-box" {
        return Err(rejected(
            AdmissionErrorKind::Mismatch,
            "unexpected compiled member",
        ));
    }
    let mut output = File::from(
        openat(
            &generation.directory,
            "sing-box",
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    generation.stamp = stamp(&generation.directory.metadata()?);
    let identity = output.metadata()?;
    generation.member_identity = Some((identity.dev(), identity.ino()));
    private_mode(&output, false, 0o600)?;
    let mut data = source.file.try_clone()?;
    copy_checked(&mut data, &mut output, expected)?;
    fchmod(&output, Mode::from_bits_truncate(0o500)).map_err(std::io::Error::from)?;
    output.sync_all()?;
    drop(output); // no writable handle survives into the execution owner
    let mut file = File::from(
        openat(
            &generation.directory,
            "sing-box",
            open_flags(false),
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    private_mode(&file, false, 0o500)?;
    validate_macho(&mut file)?;
    generation.directory.sync_all()?;
    generation.stamp = stamp(&generation.directory.metadata()?);
    let binary = Member {
        stamp: stamp(&file.metadata()?),
        file,
        path: generation.path.join("sing-box"),
    };
    let protected = ProtectedMembers {
        binary,
        source,
        source_stamp,
        generation,
        system: vec![
            ProtectedPath::open(Path::new("/usr/lib"), true)?,
            ProtectedPath::open(Path::new("/System/Library"), true)?,
        ],
        lease,
    };
    if !protected.is_current() {
        return Err(rejected(
            AdmissionErrorKind::Changed,
            "cohort changed while privately copying",
        ));
    }
    Ok(protected)
}
impl ProtectedMembers {
    pub(super) fn is_current(&self) -> bool {
        // Metadata, namespace and protection only. No warm content hashing.
        self.binary.current()
            && self.generation.current()
            && self.source.unchanged().is_ok()
            && self
                .source
                .file
                .metadata()
                .is_ok_and(|m| stamp(&m) == self.source_stamp)
            && self.system.iter().all(|p| p.unchanged().is_ok())
            && self.lease.unchanged().is_ok()
    }
    pub(super) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        Err(rejected(
            AdmissionErrorKind::UnsupportedCapability,
            "root Helper plans cannot use the ordinary spawner",
        ))
    }
    pub(super) fn root_command(&self) -> Result<std::process::Command, AdmissionError> {
        require_root()?;
        if !self.is_current() {
            return Err(rejected(
                AdmissionErrorKind::Changed,
                "root private execution owner retired or changed",
            ));
        }
        let mut command = std::process::Command::new(&self.binary.path);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        Ok(command)
    }
}
#[allow(
    unsafe_code,
    reason = "child-only CLOEXEC change on a dedicated duplicate; parent FD is never inheritable"
)]
pub(super) fn attach_store_lease(
    command: &mut std::process::Command,
    lease: &File,
    expected: &ProtectedPath,
) -> Result<Arc<File>, AdmissionError> {
    use std::os::unix::process::CommandExt;
    let m = lease.metadata()?;
    let selected = expected.file.metadata()?;
    let flags =
        nix::fcntl::fcntl(lease, nix::fcntl::FcntlArg::F_GETFD).map_err(std::io::Error::from)?;
    if m.dev() != selected.dev()
        || m.ino() != selected.ino()
        || !m.is_file()
        || m.uid() != 0
        || m.len() != 0
        || m.nlink() != 1
        || flags & nix::libc::FD_CLOEXEC == 0
    {
        return Err(rejected(
            AdmissionErrorKind::Mismatch,
            "root occupancy lease differs",
        ));
    }
    // Independently enforce real shared occupancy; an arbitrary matching File
    // or erased caller owner must not stand in for a lock. This is a fresh OFD,
    // never the cache's read-only observation, and cannot downgrade an exclusive
    // repair lock even if a buggy caller supplied that descriptor.
    let fresh = ProtectedPath::open(&expected.path, false)?;
    let owner = shared_occupancy(fresh.file)?;
    expected.unchanged()?;
    let actual = owner.metadata()?;
    if actual.dev() != selected.dev() || actual.ino() != selected.ino() {
        return Err(rejected(
            AdmissionErrorKind::Changed,
            "CoreStore lease was replaced before birth",
        ));
    }
    let child = owner.try_clone()?;
    let fd = child.as_raw_fd();
    command.env("POLARIS_CORE_STORE_LEASE_FD", fd.to_string());
    // SAFETY: captures keep a preallocated duplicate alive through exec. Only
    // async-signal-safe fcntl runs in the child; errors use Command's exec pipe.
    unsafe {
        command.pre_exec(move || {
            let _owner = &child;
            if nix::libc::fcntl(fd, nix::libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(owner)
}

fn shared_occupancy(file: File) -> Result<Arc<File>, AdmissionError> {
    match file.try_lock_shared() {
        Ok(()) => Ok(Arc::new(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(rejected(
            AdmissionErrorKind::Busy,
            "CoreStore is exclusively occupied",
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;

// Called only after Root/purpose/owner admission. This plans fixed data arguments;
// it cannot create an executable object or spawn a child.
pub(super) fn configure_main_command(
    command: &mut std::process::Command,
    config: &Path,
    working_directory: &Path,
    pipe: bool,
) {
    command
        .arg("run")
        .arg("-c")
        .arg(config)
        .current_dir(working_directory)
        .stdin(std::process::Stdio::null());
    if pipe {
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    } else {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
}

//! Linux immutable preparation. No production run or arbitrary-Child adapter.
//! The actual fixed no-network probe is private and compiled only in unit tests.
//! Dynamic ELF still trusts the host loader/libraries and host CLOEXEC discipline.

use nix::fcntl::{fcntl, FcntlArg, SealFlag};
use nix::sys::memfd::{memfd_create, MFdFlags};
use nix::sys::stat::{fchmod, Mode};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub const MAX_BINARY_BYTES: usize = 96 * 1024 * 1024;
pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
pub const PINNED_B609_LINUX_X86_64_SHA256: &str =
    "64f6d8613f9c7d42ef9a8e90dd9fca7290f176c5b482714915c04353911559c0";
const SEALS: SealFlag = SealFlag::F_SEAL_WRITE
    .union(SealFlag::F_SEAL_GROW)
    .union(SealFlag::F_SEAL_SHRINK)
    .union(SealFlag::F_SEAL_SEAL);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxInputProfile {
    B609PlainTcpCheckV1,
    #[cfg(test)]
    NoNetworkUnitFixtureV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutableObligation {
    CacheWriterLeaseAndSelectorReadback,
    StateStoreWriterLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderTrust {
    HostSystemLoaderAndLibraries,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareError {
    Unsupported,
    ResourceBudget,
    InvalidExecutable,
    BinaryMismatch,
    SourceChanged,
    Io,
}

/// Owned physical protection, not a permission. Fields and handles remain
/// private, with no Clone, Debug, or public File/path constructor.
pub struct LinuxProtectedInputs {
    binary: File,
    config: File,
    binary_digest: [u8; 32],
    config_digest: [u8; 32],
    profile: LinuxInputProfile,
    obligations: Vec<MutableObligation>,
}

impl LinuxProtectedInputs {
    #[must_use]
    pub const fn binary_digest(&self) -> &[u8; 32] {
        &self.binary_digest
    }
    #[must_use]
    pub const fn config_digest(&self) -> &[u8; 32] {
        &self.config_digest
    }
    #[must_use]
    pub const fn profile(&self) -> LinuxInputProfile {
        self.profile
    }
    #[must_use]
    pub fn mutable_obligations(&self) -> &[MutableObligation] {
        &self.obligations
    }
    #[must_use]
    pub const fn loader_trust(&self) -> LoaderTrust {
        LoaderTrust::HostSystemLoaderAndLibraries
    }

    /// Rechecks the physical seals on the same held objects. This does not
    /// prove any mutable writer handoff or a future launch profile.
    pub fn validate_protection(&self, expected: LinuxInputProfile) -> Result<(), PrepareError> {
        if self.profile != expected {
            return Err(PrepareError::Unsupported);
        }
        for file in [&self.binary, &self.config] {
            let seals =
                fcntl(file, FcntlArg::F_GET_SEALS).map_err(|_| PrepareError::Unsupported)?;
            if seals & SEALS.bits() != SEALS.bits() {
                return Err(PrepareError::Unsupported);
            }
        }
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_elf(bytes: &[u8]) -> Result<(), PrepareError> {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
        return Err(PrepareError::InvalidExecutable);
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    let expected = match std::env::consts::ARCH {
        "x86_64" => 62,
        "aarch64" => 183,
        _ => return Err(PrepareError::Unsupported),
    };
    if machine != expected {
        return Err(PrepareError::InvalidExecutable);
    }
    Ok(())
}

fn sealed_image(bytes: &[u8], executable: bool) -> Result<File, PrepareError> {
    let name = if executable {
        "polaris-s4-binary"
    } else {
        "polaris-s4-config"
    };
    let fd = memfd_create(name, MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING)
        .map_err(|_| PrepareError::Unsupported)?;
    let mut file = File::from(fd);
    file.write_all(bytes).map_err(|_| PrepareError::Io)?;
    fchmod(
        &file,
        if executable {
            Mode::S_IRUSR | Mode::S_IXUSR
        } else {
            Mode::S_IRUSR
        },
    )
    .map_err(|_| PrepareError::Io)?;
    fcntl(&file, FcntlArg::F_ADD_SEALS(SEALS)).map_err(|_| PrepareError::Unsupported)?;
    let actual = fcntl(&file, FcntlArg::F_GET_SEALS).map_err(|_| PrepareError::Unsupported)?;
    if actual & SEALS.bits() != SEALS.bits() {
        return Err(PrepareError::Unsupported);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| PrepareError::Io)?;
    let mut copied = Vec::with_capacity(bytes.len());
    file.read_to_end(&mut copied)
        .map_err(|_| PrepareError::Io)?;
    if copied != bytes {
        return Err(PrepareError::SourceChanged);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| PrepareError::Io)?;
    // Safe atomic CLOEXEC duplication; executable never collides with fd0.
    let high = rustix::io::fcntl_dupfd_cloexec(&file, 3).map_err(|_| PrepareError::Io)?;
    Ok(File::from(high))
}

/// Own bytes first, then establish the kernel protection. Nonempty rules are
/// explicitly unsupported until their final config FD/path mapping is defined.
pub fn prepare_linux_protected_inputs(
    binary_bytes: Vec<u8>,
    config_bytes: Vec<u8>,
    immutable_rules: &[(String, Vec<u8>)],
    profile: LinuxInputProfile,
    obligations: Vec<MutableObligation>,
) -> Result<LinuxProtectedInputs, PrepareError> {
    if !immutable_rules.is_empty() {
        return Err(PrepareError::Unsupported);
    }
    if binary_bytes.is_empty()
        || binary_bytes.len() > MAX_BINARY_BYTES
        || config_bytes.is_empty()
        || config_bytes.len() > MAX_CONFIG_BYTES
    {
        return Err(PrepareError::ResourceBudget);
    }
    validate_elf(&binary_bytes)?;
    let binary_digest = digest(&binary_bytes);
    match profile {
        LinuxInputProfile::B609PlainTcpCheckV1 => {
            if std::env::consts::ARCH != "x86_64"
                || hex_digest(&binary_digest) != PINNED_B609_LINUX_X86_64_SHA256
            {
                return Err(PrepareError::BinaryMismatch);
            }
        }
        #[cfg(test)]
        LinuxInputProfile::NoNetworkUnitFixtureV1 => {}
    }
    let config_digest = digest(&config_bytes);
    let inputs = LinuxProtectedInputs {
        binary: sealed_image(&binary_bytes, true)?,
        config: sealed_image(&config_bytes, false)?,
        binary_digest,
        config_digest,
        profile,
        obligations,
    };
    inputs.validate_protection(profile)?;
    Ok(inputs)
}

fn same_source(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

pub fn read_linux_binary_source(path: &Path) -> Result<Vec<u8>, PrepareError> {
    read_linux_binary_source_with_stat_hook(path, || {})
}

fn read_linux_binary_source_with_stat_hook(
    path: &Path,
    after_stat: impl FnOnce(),
) -> Result<Vec<u8>, PrepareError> {
    if !path.is_absolute() {
        return Err(PrepareError::Unsupported);
    }
    let before = std::fs::symlink_metadata(path).map_err(|_| PrepareError::Io)?;
    if !before.is_file() || before.len() > MAX_BINARY_BYTES as u64 {
        return Err(PrepareError::Unsupported);
    }
    after_stat();
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| PrepareError::Io)?;
    let opened = file.metadata().map_err(|_| PrepareError::Io)?;
    if !opened.is_file() || opened.len() > MAX_BINARY_BYTES as u64 {
        return Err(PrepareError::Unsupported);
    }
    if !same_source(&before, &opened) {
        return Err(PrepareError::SourceChanged);
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_BINARY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PrepareError::Io)?;
    let after = std::fs::symlink_metadata(path).map_err(|_| PrepareError::SourceChanged)?;
    let held_after = file.metadata().map_err(|_| PrepareError::Io)?;
    if bytes.is_empty()
        || bytes.len() > MAX_BINARY_BYTES
        || bytes.len() as u64 != opened.len()
        || !same_source(&opened, &after)
        || !same_source(&opened, &held_after)
    {
        return Err(PrepareError::SourceChanged);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;

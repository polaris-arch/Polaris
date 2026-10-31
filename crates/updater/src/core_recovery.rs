//! Strict, non-executing extraction of the independently bundled recovery data.
//! The caller supplies the compiled selection, an already opened source and a
//! private staging parent. This module neither chooses a source nor installs,
//! admits or executes the resulting files. Normal startup must not call it.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;
use polaris_helper_proto::codec::is_valid_sha256_hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use thiserror::Error;

const MAX_ARCHIVE: u64 = 200 * 1024 * 1024;
const MAX_MEMBER: u64 = 200 * 1024 * 1024;
const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
const MAX_MEMBERS: usize = 1024;
const MAX_ENTRIES: usize = 2048;
const BUFFER: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum RecoveryFormat {
    #[serde(rename = "tar.gz")]
    TarGz,
    #[serde(rename = "zip")]
    Zip,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryArchive {
    pub resource: String,
    pub format: RecoveryFormat,
    pub bytes: u64,
    pub sha256: String,
    pub root: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryMember {
    /// Full archive member name, including its single selected root.
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreRecoverySpec {
    pub schema: String,
    pub platform: String,
    pub core_id: String,
    pub archive: RecoveryArchive,
    pub members: Vec<RecoveryMember>,
    /// Full archive paths, derived from the same compiled core/sidecar pins.
    pub payload_members: Vec<String>,
}

#[derive(Debug, Error)]
pub enum CoreRecoveryError {
    #[error("core recovery cancelled")]
    Cancelled,
    #[error("invalid compiled recovery selection")]
    InvalidSelection,
    #[error("recovery source differs from the compiled archive")]
    ArchiveMismatch,
    #[error("unsafe or invalid recovery archive")]
    InvalidArchive,
    #[error("recovery member differs from the compiled selection")]
    MemberMismatch,
    #[error("recovery byte or member budget exceeded")]
    BudgetExceeded,
    #[error("core recovery storage operation failed: {0}")]
    Io(#[from] io::Error),
}

/// Only data. Drop removes this newly created staging directory, never the
/// source archive, an existing installation, user data or an installation journal.
#[derive(Debug)]
pub struct StagedCoreRecovery {
    directory: TempDir,
    core_id: String,
    members: Vec<RecoveryMember>,
}
impl StagedCoreRecovery {
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    pub fn core_id(&self) -> &str {
        &self.core_id
    }
    /// Full source member names; each is stored at directory()/its final leaf.
    pub fn members(&self) -> &[RecoveryMember] {
        &self.members
    }
}

type Result<T> = std::result::Result<T, CoreRecoveryError>;
fn check_cancel(cancelled: &mut impl FnMut() -> bool) -> Result<()> {
    if cancelled() {
        Err(CoreRecoveryError::Cancelled)
    } else {
        Ok(())
    }
}
fn component(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_+.-".contains(&b))
        && !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
fn safe_name(name: &str) -> bool {
    name.len() <= 512 && name.split('/').all(component)
}

struct Inventory<'a> {
    files: BTreeMap<&'a str, &'a RecoveryMember>,
    directories: BTreeSet<String>,
    payload: BTreeSet<&'a str>,
    seen: BTreeSet<String>,
    seen_files: BTreeSet<String>,
}
impl<'a> Inventory<'a> {
    fn new(spec: &'a CoreRecoverySpec) -> Result<Self> {
        if spec.schema != "polaris-core-recovery-v1"
            || !["linux", "win", "mac-x64", "mac-arm64"].contains(&spec.platform.as_str())
            || !is_valid_sha256_hex(&spec.core_id)
            || !component(&spec.archive.root)
            || !is_valid_sha256_hex(&spec.archive.sha256)
        {
            return Err(CoreRecoveryError::InvalidSelection);
        }
        let prefix = format!("core-recovery/{}/", spec.platform);
        let resource = spec
            .archive
            .resource
            .strip_prefix(&prefix)
            .ok_or(CoreRecoveryError::InvalidSelection)?;
        if !component(resource)
            || !(match spec.archive.format {
                RecoveryFormat::TarGz => resource.ends_with(".tar.gz") && spec.platform != "win",
                RecoveryFormat::Zip => resource.ends_with(".zip") && spec.platform == "win",
            })
        {
            return Err(CoreRecoveryError::InvalidSelection);
        }
        if spec.archive.bytes == 0
            || spec.archive.bytes > MAX_ARCHIVE
            || spec.members.is_empty()
            || spec.members.len() > MAX_MEMBERS
        {
            return Err(CoreRecoveryError::BudgetExceeded);
        }
        let mut files = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut aliases = BTreeMap::new();
        let mut total = 0u64;
        for member in &spec.members {
            if !safe_name(&member.name)
                || !member.name.starts_with(&(spec.archive.root.clone() + "/"))
                || !is_valid_sha256_hex(&member.sha256)
                || files.insert(member.name.as_str(), member).is_some()
            {
                return Err(CoreRecoveryError::InvalidSelection);
            }
            total = total
                .checked_add(member.bytes)
                .ok_or(CoreRecoveryError::BudgetExceeded)?;
            if member.bytes > MAX_MEMBER || total > MAX_EXPANDED {
                return Err(CoreRecoveryError::BudgetExceeded);
            }
            let parts: Vec<_> = member.name.split('/').collect();
            for end in 1..=parts.len() {
                let name = parts[..end].join("/");
                if let Some(old) = aliases.insert(name.to_ascii_lowercase(), name.clone()) {
                    if old != name {
                        return Err(CoreRecoveryError::InvalidSelection);
                    }
                }
                if end != parts.len() {
                    directories.insert(name);
                }
            }
        }
        if directories.len() + files.len() > MAX_ENTRIES
            || directories
                .iter()
                .any(|name| files.contains_key(name.as_str()))
        {
            return Err(CoreRecoveryError::InvalidSelection);
        }
        let payload: BTreeSet<_> = spec.payload_members.iter().map(String::as_str).collect();
        if payload.is_empty()
            || payload.len() > 2
            || payload.len() != spec.payload_members.len()
            || payload.iter().any(|name| {
                !files.contains_key(name)
                    || name
                        .strip_prefix(&(spec.archive.root.clone() + "/"))
                        .is_none_or(|leaf| !component(leaf))
            })
        {
            return Err(CoreRecoveryError::InvalidSelection);
        }
        Ok(Self {
            files,
            directories,
            payload,
            seen: BTreeSet::new(),
            seen_files: BTreeSet::new(),
        })
    }

    fn entry(
        &mut self,
        name: &str,
        directory: bool,
        bytes: u64,
    ) -> Result<Option<&'a RecoveryMember>> {
        if !safe_name(name) || !self.seen.insert(name.to_owned()) || self.seen.len() > MAX_ENTRIES {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        if directory {
            if bytes != 0 || !self.directories.contains(name) {
                return Err(CoreRecoveryError::InvalidArchive);
            }
            Ok(None)
        } else {
            let member = *self
                .files
                .get(name)
                .ok_or(CoreRecoveryError::InvalidArchive)?;
            if bytes != member.bytes {
                return Err(CoreRecoveryError::MemberMismatch);
            }
            self.seen_files.insert(name.to_owned());
            Ok(Some(member))
        }
    }
    fn finish(&self) -> Result<()> {
        if self.seen_files.len() != self.files.len() {
            return Err(CoreRecoveryError::MemberMismatch);
        }
        Ok(())
    }
}

fn private_parent(parent: &Path) -> Result<()> {
    // A data-stage sanity check, not the privileged install path/ACL proof.
    // The real activation caller must retain its existing protected-path gate.
    if !parent.is_absolute() {
        return Err(CoreRecoveryError::InvalidSelection);
    }
    for path in parent.ancestors() {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(CoreRecoveryError::InvalidSelection);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(CoreRecoveryError::InvalidSelection);
            }
        }
    }
    Ok(())
}

fn member_data(
    reader: &mut impl Read,
    member: &RecoveryMember,
    output: Option<PathBuf>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<()> {
    let mut file = if let Some(path) = output {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Some(options.open(path)?)
    } else {
        None
    };
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; BUFFER];
    loop {
        check_cancel(cancelled)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|_| CoreRecoveryError::InvalidArchive)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(CoreRecoveryError::BudgetExceeded)?;
        if bytes > member.bytes {
            return Err(CoreRecoveryError::MemberMismatch);
        }
        hash.update(&buffer[..read]);
        if let Some(file) = &mut file {
            file.write_all(&buffer[..read])?;
        }
    }
    if bytes != member.bytes || hex::encode(hash.finalize()) != member.sha256 {
        return Err(CoreRecoveryError::MemberMismatch);
    }
    if let Some(file) = file {
        file.sync_all()?;
    }
    check_cancel(cancelled)
}

fn output_path(
    stage: &TempDir,
    inventory: &Inventory<'_>,
    member: &RecoveryMember,
) -> Option<PathBuf> {
    inventory.payload.contains(member.name.as_str()).then(|| {
        stage
            .path()
            .join(member.name.rsplit('/').next().unwrap_or(""))
    })
}

fn unpack_tar(
    snapshot: Cursor<Vec<u8>>,
    spec: &CoreRecoverySpec,
    stage: &TempDir,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<()> {
    let decoder = GzDecoder::new(BufReader::new(snapshot));
    let mut archive = tar::Archive::new(decoder.take(MAX_EXPANDED + 1));
    let mut inventory = Inventory::new(spec)?;
    for entry in archive
        .entries()
        .map_err(|_| CoreRecoveryError::InvalidArchive)?
        .raw(true)
    {
        check_cancel(cancelled)?;
        let mut entry = entry.map_err(|_| CoreRecoveryError::InvalidArchive)?;
        let kind = entry.header().entry_type();
        // Raw iteration deliberately rejects PAX/longname/sparse/link headers;
        // the selected native archives need only ordinary files/directories.
        if !(kind.is_file() || kind.is_dir())
            || entry.link_name_bytes().is_some()
            || entry
                .header()
                .mode()
                .map_err(|_| CoreRecoveryError::InvalidArchive)?
                & 0o7022
                != 0
        {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        let raw = entry.path_bytes();
        let raw = std::str::from_utf8(&raw).map_err(|_| CoreRecoveryError::InvalidArchive)?;
        let name = if kind.is_dir() {
            raw.strip_suffix('/').unwrap_or(raw)
        } else {
            raw
        }
        .to_owned();
        if let Some(member) = inventory.entry(&name, kind.is_dir(), entry.size())? {
            let output = output_path(stage, &inventory, member);
            member_data(&mut entry, member, output, cancelled)?;
        }
    }
    inventory.finish()?;
    // Do not hide a second tar/gzip stream or arbitrary bytes after tar EOF.
    let mut expanded = archive.into_inner();
    let mut buffer = [0u8; BUFFER];
    loop {
        check_cancel(cancelled)?;
        let read = expanded
            .read(&mut buffer)
            .map_err(|_| CoreRecoveryError::InvalidArchive)?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|b| *b != 0) {
            return Err(CoreRecoveryError::InvalidArchive);
        }
    }
    if expanded.limit() == 0 {
        return Err(CoreRecoveryError::BudgetExceeded);
    }
    let mut compressed = expanded.into_inner().into_inner();
    if compressed.read(&mut buffer[..1])? != 0 {
        return Err(CoreRecoveryError::InvalidArchive);
    }
    Ok(())
}

// ZIP's named index replaces duplicate raw names. Check coverage using the
// library's public record offsets, so no local/central record can disappear in
// that projection. This selected Zip32 profile has no comments, extras, data
// descriptors or ZIP64 records; no ZIP header is parsed here.
fn zip_coverage(mut ranges: Vec<(u64, u64)>, start: u64, end: u64) -> Result<()> {
    ranges.sort_unstable();
    let mut cursor = start;
    for (start, end) in ranges {
        if start != cursor || end <= start {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        cursor = end;
    }
    if cursor != end {
        return Err(CoreRecoveryError::InvalidArchive);
    }
    Ok(())
}

fn unpack_zip(
    snapshot: Cursor<Vec<u8>>,
    spec: &CoreRecoverySpec,
    stage: &TempDir,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(snapshot).map_err(|_| CoreRecoveryError::InvalidArchive)?;
    if archive.len() > MAX_ENTRIES || archive.offset() != 0 || !archive.comment().is_empty()
        // The public library API returns None for a central descriptor flag,
        // even if the local header omits it. Never accept that mixed profile.
        || archive.decompressed_size().is_none_or(|bytes| bytes > u128::from(MAX_EXPANDED))
    {
        return Err(CoreRecoveryError::InvalidArchive);
    }
    let directory_start = archive.central_directory_start();
    let mut inventory = Inventory::new(spec)?;
    let mut local_ranges = Vec::new();
    let mut central_ranges = Vec::new();
    let mut metadata = BTreeMap::new();
    for index in 0..archive.len() {
        check_cancel(cancelled)?;
        // Metadata pass only: the full data is streamed/hash-checked once below,
        // through the library's local-header reader, not its deduplicated index.
        let file = archive
            .by_index_raw(index)
            .map_err(|_| CoreRecoveryError::InvalidArchive)?;
        let name = file.name().to_owned();
        let directory = file.is_dir();
        let mode = file.unix_mode().unwrap_or(0);
        let kind = mode & 0o170000;
        if file.encrypted()
            || file.is_symlink()
            || !file.comment().is_empty()
            || file.extra_data().is_some_and(|extra| !extra.is_empty())
            || (kind != 0 && kind != if directory { 0o040000 } else { 0o100000 })
            || !matches!(
                file.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            )
            || file.name_raw() != name.as_bytes()
            || mode & 0o7000 != 0
            || (directory && (file.size() != 0 || file.compressed_size() != 0))
        {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        // Accept the original Windows publisher's measured mode metadata only;
        // none of those modes are applied to the private output or authorize a launch.
        let windows_mode = spec.platform == "win"
            && (mode == 0o100666
                || (mode == 0o100777 && name == format!("{}/sing-box.exe", spec.archive.root)));
        if mode & 0o022 != 0 && !windows_mode {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        let data_start = file.data_start().ok_or(CoreRecoveryError::InvalidArchive)?;
        let end = data_start
            .checked_add(file.compressed_size())
            .ok_or(CoreRecoveryError::InvalidArchive)?;
        let start = file.header_start();
        if data_start.checked_sub(start) != Some(30 + file.name_raw().len() as u64)
            || end > directory_start
        {
            return Err(CoreRecoveryError::InvalidArchive);
        }
        local_ranges.push((start, end));
        let central_start = file.central_header_start();
        let central_end = central_start
            .checked_add(46 + file.name_raw().len() as u64)
            .ok_or(CoreRecoveryError::InvalidArchive)?;
        central_ranges.push((central_start, central_end));
        let shape = (
            file.compression(),
            file.compressed_size(),
            file.size(),
            file.crc32(),
            directory,
        );
        let inventory_name = if directory {
            name.strip_suffix('/').unwrap_or(&name)
        } else {
            &name
        };
        inventory.entry(inventory_name, directory, file.size())?;
        metadata.insert(name, (shape, start, end));
    }
    inventory.finish()?;
    zip_coverage(local_ranges, 0, directory_start)?;
    zip_coverage(
        central_ranges,
        directory_start,
        spec.archive
            .bytes
            .checked_sub(22)
            .ok_or(CoreRecoveryError::InvalidArchive)?,
    )?;
    let mut source = archive.into_inner();
    source.seek(SeekFrom::Start(0))?;
    let mut inventory = Inventory::new(spec)?;
    while source.stream_position()? < directory_start {
        check_cancel(cancelled)?;
        let start = source.stream_position()?;
        let expected_end;
        {
            let mut file = zip::read::read_zipfile_from_stream(&mut source)
                .map_err(|_| CoreRecoveryError::InvalidArchive)?
                .ok_or(CoreRecoveryError::InvalidArchive)?;
            let name = file.name().to_owned();
            let directory = file.is_dir();
            let shape = (
                file.compression(),
                file.compressed_size(),
                file.size(),
                file.crc32(),
                directory,
            );
            let (expected, expected_start, end) = metadata
                .get(&name)
                .ok_or(CoreRecoveryError::InvalidArchive)?;
            if &shape != expected
                || start != *expected_start
                || file.encrypted()
                || file.name_raw() != name.as_bytes()
                || file.extra_data().is_some_and(|extra| !extra.is_empty())
            {
                return Err(CoreRecoveryError::InvalidArchive);
            }
            expected_end = *end;
            let name = if directory {
                name.strip_suffix('/').unwrap_or(&name)
            } else {
                &name
            };
            if let Some(member) = inventory.entry(name, directory, file.size())? {
                let output = output_path(stage, &inventory, member);
                member_data(&mut file, member, output, cancelled)?;
            }
        }
        if source.stream_position()? != expected_end {
            return Err(CoreRecoveryError::InvalidArchive);
        }
    }
    if source.stream_position()? != directory_start {
        return Err(CoreRecoveryError::InvalidArchive);
    }
    inventory.finish()
}

/// Freeze/hash the compressed input once before any archive parser sees it.
/// Output remains data-only and must still traverse the existing maintenance,
/// compiled-manifest and installation transaction checks at its real caller.
pub fn stage_core_recovery<R: Read + Seek>(
    source: &mut R,
    expected: &CoreRecoverySpec,
    stage_parent: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> Result<StagedCoreRecovery> {
    Inventory::new(expected)?;
    check_cancel(&mut cancelled)?;
    private_parent(stage_parent)?;
    // Own the bounded compressed bytes in process memory. Even an already open
    // source can change after EOF: never seek/reopen it as parser input. No
    // filesystem alias or prior writable file handle can mutate this snapshot.
    let capacity =
        usize::try_from(expected.archive.bytes).map_err(|_| CoreRecoveryError::BudgetExceeded)?;
    let mut compressed = Vec::new();
    compressed
        .try_reserve_exact(capacity)
        .map_err(|_| CoreRecoveryError::BudgetExceeded)?;
    source.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; BUFFER];
    loop {
        check_cancel(&mut cancelled)?;
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(CoreRecoveryError::BudgetExceeded)?;
        if bytes > expected.archive.bytes {
            return Err(CoreRecoveryError::ArchiveMismatch);
        }
        hash.update(&buffer[..read]);
        compressed.extend_from_slice(&buffer[..read]);
    }
    if bytes != expected.archive.bytes || hex::encode(hash.finalize()) != expected.archive.sha256 {
        return Err(CoreRecoveryError::ArchiveMismatch);
    }
    let snapshot = Cursor::new(compressed);
    let mut builder = tempfile::Builder::new();
    builder.prefix("core-recovery-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // tempfile's default directory mode is0777 before umask. Supply the
        // private mode to mkdir itself, never expose a wider temporary stage.
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(stage_parent)?;
    match expected.archive.format {
        RecoveryFormat::TarGz => unpack_tar(snapshot, expected, &directory, &mut cancelled)?,
        RecoveryFormat::Zip => unpack_zip(snapshot, expected, &directory, &mut cancelled)?,
    }
    check_cancel(&mut cancelled)?;
    let members = expected
        .members
        .iter()
        .filter(|m| expected.payload_members.contains(&m.name))
        .cloned()
        .collect::<Vec<_>>();
    let names: BTreeSet<_> = members
        .iter()
        .map(|m| m.name.rsplit('/').next().unwrap_or("").to_owned())
        .collect();
    let actual: BTreeSet<_> = fs::read_dir(directory.path())?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<_>>()?;
    if names != actual {
        return Err(CoreRecoveryError::InvalidArchive);
    }
    Ok(StagedCoreRecovery {
        directory,
        core_id: expected.core_id.clone(),
        members,
    })
}

#[cfg(test)]
mod tests;

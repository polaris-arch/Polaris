//! Cooperative Windows retirement: retained admission/lifetime locks and native
//! file identity authorize work; persisted JSON alone never grants custody.
#![allow(unsafe_code, reason = "Windows handle identity, locks and handle-bound deletion")]
use polaris_mesh::tailscale_state::directory_retirement::{lower_hex, DirectoryDeletionIntent};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::OVERLAPPED;
const PLATFORM: &str = "windows-ntfs-v1";
const ROOT: &str = "tailscale-windows-v1";
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const DELETE_ACCESS: u32 = 0x10000;
const READ_CONTROL_ACCESS: u32 = 0x20000;
fn unknown() -> io::Error { io::Error::new(io::ErrorKind::WouldBlock, "CleanupUnknown: Windows directory capability unavailable") }
fn nonce(value: &str) -> bool { lower_hex(value, 32) }
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity { volume: u64, file_id: String }
fn identity(file: &File) -> io::Result<Identity> {
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), std::mem::size_of::<FILE_ID_INFO>() as u32) } == 0 { return Err(io::Error::last_os_error()); }
    Ok(Identity { volume: id.VolumeSerialNumber, file_id: hex(&id.FileId.Identifier) })
}
fn check_object(file: &File, directory: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || metadata.is_dir() != directory { return Err(unknown()); }
    if !directory {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 { return Err(io::Error::last_os_error()); }
        if info.nNumberOfLinks != 1 { return Err(unknown()); }
    }
    Ok(())
}
fn open_object(path: &Path, directory: bool, delete: bool, write: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 });
    if directory || delete { options.access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL_ACCESS | SYNCHRONIZE | if delete { DELETE_ACCESS } else { 0 }); }
    let file = options.open(path)?;
    check_object(&file, directory)?;
    Ok(file)
}
fn valid_component(value: &str) -> bool {
    if value.is_empty() || value == "." || value == ".." || value.ends_with(['.', ' ']) || value.chars().any(|c| c < ' ' || "\\/:*?\"<>|".contains(c)) { return false; }
    let base = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered_device = (base.starts_with("COM") || base.starts_with("LPT"))
        && base[3..].chars().count() == 1
        && base[3..].chars().any(|c| matches!(c, '1'..='9' | '¹' | '²' | '³'));
    !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") && !numbered_device
}
fn path_parts(path: &Path) -> io::Result<(String, Vec<&str>)> {
    let raw = path.to_str().ok_or_else(unknown)?;
    let bytes = raw.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' || raw.contains('/') || raw.contains('\0') { return Err(unknown()); }
    let parts: Vec<_> = raw[3..].split('\\').collect();
    if parts.iter().any(|v| !valid_component(v)) { return Err(unknown()); }
    Ok((raw[..3].to_owned(), parts))
}
fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let size = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), buffer.len() as u32, VOLUME_NAME_GUID) };
    if size == 0 { return Err(io::Error::last_os_error()); }
    if size as usize >= buffer.len() { return Err(unknown()); }
    let value = String::from_utf16(&buffer[..size as usize]).map_err(|_| unknown())?;
    if !value.starts_with(r"\\?\Volume{") { return Err(unknown()); }
    Ok(PathBuf::from(value))
}
fn pin_chain(path: &Path) -> io::Result<Vec<File>> {
    let (drive, parts) = path_parts(path)?;
    let wide: Vec<u16> = drive.encode_utf16().chain(Some(0)).collect();
    if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED { return Err(unknown()); }
    let first = open_object(Path::new(&drive), true, false, false)?;
    let mut current = final_path(&first)?;
    if !current.to_str().is_some_and(|v| v.ends_with("}\\")) { return Err(unknown()); } // reject SUBST
    let mut held = vec![first];
    for component in parts { current.push(component); held.push(open_object(&current, true, false, false)?); }
    let mut fs = [0u16; 32];
    if unsafe { GetVolumeInformationByHandleW(held.last().ok_or_else(unknown)?.as_raw_handle(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) } == 0 { return Err(io::Error::last_os_error()); }
    let end = fs.iter().position(|v| *v == 0).ok_or_else(unknown)?;
    if String::from_utf16(&fs[..end]).ok().as_deref() != Some("NTFS") { return Err(unknown()); }
    Ok(held)
}
fn lock(file: &File, exclusive: bool) -> io::Result<()> {
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { LockFileEx(file.as_raw_handle(), LOCKFILE_FAIL_IMMEDIATELY | if exclusive { LOCKFILE_EXCLUSIVE_LOCK } else { 0 }, 0, 1, 0, &mut overlapped) } == 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}
fn bounded_read(file: &mut File, maximum: u64) -> io::Result<Vec<u8>> {
    if file.metadata()?.len() > maximum { return Err(unknown()); }
    let mut bytes = Vec::new(); file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum { return Err(unknown()); } Ok(bytes)
}
fn absent(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) { Ok(_) => Ok(false), Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true), Err(e) => Err(e) }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Domain { version: u32, platform: String, root_volume: u64, root_file_id: String, domain_nonce: String }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32, platform: String, server_id: String, incarnation: String, directory_epoch: String,
    root_volume: u64, root_file_id: String, directory_volume: u64, directory_file_id: String,
    phase: String, delete_intent_nonce: String,
}
impl Marker {
    fn identity(&self) -> String { format!("{PLATFORM}:{}:{}:{}:{}", self.root_volume, self.root_file_id, self.directory_volume, self.directory_file_id) }
    fn valid(&self, root: &Identity) -> bool {
        self.version == 1 && self.platform == PLATFORM && valid_component(&self.server_id) && nonce(&self.incarnation) && nonce(&self.directory_epoch)
            && self.root_volume == root.volume && self.root_file_id == root.file_id && self.directory_volume == root.volume && nonce(&self.directory_file_id)
            && ((self.phase == "Open" && self.delete_intent_nonce.is_empty()) || (matches!(self.phase.as_str(), "Deleting" | "Deleted") && nonce(&self.delete_intent_nonce)))
    }
    fn matches(&self, intent: &DirectoryDeletionIntent) -> bool {
        self.server_id == intent.server_id && self.incarnation == intent.incarnation && self.identity() == intent.directory_identity
            && nonce(&intent.delete_intent_nonce) && (self.phase == "Open" || self.delete_intent_nonce == intent.delete_intent_nonce)
    }
}
fn valid_transition(before: &Marker, after: &Marker) -> bool {
    if before.server_id != after.server_id || before.root_volume != after.root_volume || before.root_file_id != after.root_file_id { return false; }
    if before.phase == "Deleted" && after.phase == "Open" { return before.incarnation != after.incarnation && before.directory_epoch != after.directory_epoch; }
    if before.incarnation != after.incarnation || before.directory_volume != after.directory_volume || before.directory_file_id != after.directory_file_id { return false; }
    match (before.phase.as_str(), after.phase.as_str()) {
        ("Open", "Open") => before.directory_epoch != after.directory_epoch,
        ("Open", "Deleting") => before.directory_epoch == after.directory_epoch,
        ("Deleting", "Deleting" | "Deleted") => before.directory_epoch == after.directory_epoch && before.delete_intent_nonce == after.delete_intent_nonce,
        _ => false,
    }
}
fn parse_journal(bytes: &[u8], root: &Identity) -> io::Result<Marker> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_RECORD_BYTES || bytes.last() != Some(&b'\n') { return Err(unknown()); }
    let mut previous: Option<Marker> = None;
    for line in bytes[..bytes.len()-1].split(|b| *b == b'\n') {
        if line.is_empty() || line.len() > 16384 { return Err(unknown()); }
        let record: Marker = serde_json::from_slice(line).map_err(|_| unknown())?;
        if !record.valid(root) || previous.as_ref().is_some_and(|p| !valid_transition(p, &record)) || previous.is_none() && record.phase != "Open" { return Err(unknown()); }
        previous = Some(record);
    }
    previous.ok_or_else(unknown)
}
/// Keep custody until caller durably acknowledges its deletion journal.
pub(crate) struct DirectoryLease {
    _ancestors: Vec<File>, _control: File, _admission: File, _lifetime: File,
    target: Option<File>, marker_file: File, marker: Marker, root: Identity,
    directory_path: PathBuf, state_directory: PathBuf, exclusive: bool,
}
impl DirectoryLease {
    pub(crate) fn acquire(config: &Path, server_id: &str, exclusive: bool) -> io::Result<Self> {
        if !valid_component(server_id) || actual_directory(config, server_id)? != config.join(ROOT).join(server_id) { return Err(unknown()); }
        let ancestors = pin_chain(&config.join(ROOT))?;
        let root_handle = ancestors.last().ok_or_else(unknown)?;
        let root_path = final_path(root_handle)?;
        let root = identity(root_handle)?;
        let config_path = final_path(ancestors.get(ancestors.len().checked_sub(2).ok_or_else(unknown)?).ok_or_else(unknown)?)?;
        let control_path = config_path.join(format!(".polaris-directory-control-win-{}-{}", root.volume, root.file_id));
        let control = open_object(&control_path, true, false, false)?;
        security::check_private(root_handle)?; security::check_private(&control)?;
        let admission = open_object(&control_path.join("root.admission.lock"), false, false, true)?;
        security::check_private(&admission)?; lock(&admission, true)?;
        if !absent(&control_path.join(".domain-unsafe"))? { return Err(unknown()); }
        let mut domain_file = open_object(&control_path.join("domain.json"), false, false, false)?;
        security::check_private(&domain_file)?;
        let domain: Domain = serde_json::from_slice(&bounded_read(&mut domain_file, 4096)?).map_err(|_| unknown())?;
        if domain.version != 1 || domain.platform != PLATFORM || domain.root_volume != root.volume || domain.root_file_id != root.file_id || !nonce(&domain.domain_nonce) { return Err(unknown()); }
        let mut stamp = open_object(&root_path.join(".polaris-directory-domain"), false, false, false)?;
        security::check_private(&stamp)?;
        if bounded_read(&mut stamp, 33)? != format!("{}\n", domain.domain_nonce).as_bytes() { return Err(unknown()); }
        let mut selected = None;
        for entry in std::fs::read_dir(&control_path)? {
            let entry = entry?; let name = entry.file_name().into_string().map_err(|_| unknown())?;
            if !name.starts_with("sha256-") || !name.ends_with(".jsonl") { continue; }
            if name.len() != 77 || !lower_hex(name.get(7..71).ok_or_else(unknown)?, 64) { return Err(unknown()); }
            let mut file = open_object(&entry.path(), false, false, true)?; security::check_private(&file)?;
            let marker = parse_journal(&bounded_read(&mut file, MAX_RECORD_BYTES)?, &root)?;
            if name != format!("sha256-{}.jsonl", polaris_updater::sha256_hex(marker.server_id.as_bytes())) { return Err(unknown()); }
            if marker.server_id == server_id { if selected.is_some() { return Err(unknown()); } selected = Some((file, marker)); }
        }
        let (marker_file, marker) = selected.ok_or_else(unknown)?;
        let lifetime = open_object(&control_path.join(format!("directory-{}-{}.lock", marker.directory_volume, marker.directory_file_id)), false, false, true)?;
        security::check_private(&lifetime)?; lock(&lifetime, exclusive)?;
        let directory_path = root_path.join(server_id);
        let target_absent = absent(&directory_path)?;
        let target = if marker.phase == "Deleted" || marker.phase == "Deleting" && target_absent {
            if !target_absent { return Err(unknown()); } None
        } else {
            let target = open_object(&directory_path, true, exclusive, false)?; security::check_private(&target)?;
            let id = identity(&target)?;
            if id.volume != marker.directory_volume || id.file_id != marker.directory_file_id { return Err(unknown()); }
            let mut stamp = open_object(&directory_path.join(".polaris-directory-incarnation"), false, false, false)?;
            security::check_private(&stamp)?;
            if bounded_read(&mut stamp, 33)? != format!("{}\n", marker.incarnation).as_bytes() { return Err(unknown()); }
            Some(target)
        };
        Ok(Self { _ancestors: ancestors, _control: control, _admission: admission, _lifetime: lifetime, target, marker_file, marker, root, directory_path, state_directory: config.join(ROOT).join(server_id), exclusive })
    }
    pub(crate) fn snapshot_intent(&self) -> io::Result<DirectoryDeletionIntent> {
        if self.marker.phase != "Open" || self.target.is_none() { return Err(unknown()); }
        Ok(DirectoryDeletionIntent { server_id: self.marker.server_id.clone(), incarnation: self.marker.incarnation.clone(), directory_identity: self.marker.identity(), state_directory: self.state_directory.to_str().ok_or_else(unknown)?.to_owned(), delete_intent_nonce: uuid::Uuid::new_v4().simple().to_string() })
    }
    pub(crate) fn acquire_intent(config: &Path, intent: &DirectoryDeletionIntent) -> io::Result<Self> {
        if Path::new(&intent.state_directory) != config.join(ROOT).join(&intent.server_id) { return Err(unknown()); }
        let lease = Self::acquire(config, &intent.server_id, true)?;
        if !lease.marker.matches(intent) { return Err(unknown()); } Ok(lease)
    }
    pub(crate) fn committed_intent(&self, intent: &DirectoryDeletionIntent) -> io::Result<bool> {
        if !self.marker.matches(intent) { return Err(unknown()); } Ok(self.marker.phase != "Open")
    }
    fn append(&mut self, marker: Marker) -> io::Result<()> {
        if !marker.valid(&self.root) || !valid_transition(&self.marker, &marker) { return Err(unknown()); }
        let mut bytes = serde_json::to_vec(&marker).map_err(|_| unknown())?; bytes.push(b'\n');
        if self.marker_file.metadata()?.len() + bytes.len() as u64 > MAX_RECORD_BYTES { return Err(unknown()); }
        self.marker_file.seek(SeekFrom::End(0))?; self.marker_file.write_all(&bytes)?; self.marker_file.sync_all()?; self.marker = marker; Ok(())
    }
    pub(crate) fn commit(&mut self, intent: &DirectoryDeletionIntent, current: &serde_json::Value) -> io::Result<()> {
        if !self.exclusive || !self.marker.matches(intent) { return Err(unknown()); }
        if let Some(servers) = current.get("servers").and_then(|v| v.as_array()) {
            let root_path = self.directory_path.parent().ok_or_else(unknown)?;
            for server in servers {
                let Some(id) = server.get("id").and_then(|v| v.as_str()) else { continue; };
                if !valid_component(id) { return Err(unknown()); }
                match open_object(&root_path.join(id), true, false, false) {
                    Ok(file) => { let id = identity(&file)?; if id.volume == self.marker.directory_volume && id.file_id == self.marker.directory_file_id { return Err(unknown()); } }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}, Err(error) => return Err(error),
                }
            }
        }
        if self.marker.phase == "Deleted" { return Ok(()); }
        if self.marker.phase == "Deleting" && self.target.is_none() {
            if !absent(&self.directory_path)? { return Err(unknown()); }
            let mut next = self.marker.clone(); next.phase = "Deleted".into(); return self.append(next);
        }
        if self.marker.phase == "Open" {
            let mut next = self.marker.clone(); next.phase = "Deleting".into(); next.delete_intent_nonce = intent.delete_intent_nonce.clone(); self.append(next)?;
        }
        remove_children(&self.directory_path, 0)?;
        // Removing the incarnation is last. Crash before target removal retains
        // Unknown rather than inferring replacement safety from historical IDs.
        let stamp = open_object(&self.directory_path.join(".polaris-directory-incarnation"), false, true, false)?;
        dispose(&stamp)?; drop(stamp);
        let target = self.target.take().ok_or_else(unknown)?; dispose(&target)?; drop(target);
        if !absent(&self.directory_path)? { return Err(unknown()); }
        let mut next = self.marker.clone(); next.phase = "Deleted".into(); self.append(next)
    }
}
fn dispose(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true as _ };
    if unsafe { SetFileInformationByHandle(file.as_raw_handle(), FileDispositionInfo, (&info as *const FILE_DISPOSITION_INFO).cast(), std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) } == 0 { return Err(io::Error::last_os_error()); } Ok(())
}
fn remove_children(path: &Path, depth: usize) -> io::Result<()> {
    if depth > 128 { return Err(unknown()); }
    for (count, entry) in std::fs::read_dir(path)?.enumerate() {
        if count >= 100_000 { return Err(unknown()); }
        let entry = entry?;
        if depth == 0 && entry.file_name() == ".polaris-directory-incarnation" { continue; }
        let name = entry.file_name().into_string().map_err(|_| unknown())?;
        if !valid_component(&name) { return Err(unknown()); }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err(unknown()); }
        let child = open_object(&entry.path(), metadata.is_dir(), true, false)?;
        if metadata.is_dir() { remove_children(&entry.path(), depth + 1)?; }
        dispose(&child)?;
    }
    Ok(())
}
pub(crate) fn actual_directory(config: &Path, server_id: &str) -> io::Result<PathBuf> {
    if !valid_component(server_id) { return Err(unknown()); }
    // Legacy state remains on its existing path and does not acquire a native
    // physical-retirement capability. Do not impose the new NTFS requirements
    // on an already-existing legacy directory.
    let logical = config.join("tailscale").join(server_id);
    if !absent(&logical)? { return Ok(logical); }
    let _ancestors = pin_chain(config)?;
    Ok(config.join(ROOT).join(server_id))
}
pub(crate) fn auth_lease(config: &Path, id: &str) -> io::Result<Option<DirectoryLease>> {
    if actual_directory(config, id)? == config.join("tailscale").join(id) { return Ok(None); }
    let lease = DirectoryLease::acquire(config, id, true)?;
    if lease.marker.phase != "Open" || lease.target.is_none() { return Err(unknown()); }
    Ok(Some(lease))
}
fn configuration_envelope(root: &Path) -> Result<serde_json::Value, String> {
    path_parts(root).map_err(|e| e.to_string())?;
    let contract: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/desktop-core/release-contract.json")).map_err(|_| "invalid compiled Windows source contract")?;
    let source = contract["platforms"]["win"]["sourceCommit"].as_str().ok_or("missing compiled Windows source identity")?;
    if !lower_hex(source, 40) { return Err("invalid compiled Windows source identity".into()); }
    Ok(serde_json::json!({"version":1,"root":root.to_str().ok_or("Windows root must be UTF-8")?,"run_nonce":uuid::Uuid::new_v4().simple().to_string(),"expected_source":source}))
}
pub(crate) fn bind_configuration(config: &mut polaris_config_engine::singbox::SingBoxConfig, root: &Path) -> Result<(), String> {
    config.polaris_windows_directory = if config.endpoints.as_ref().is_some_and(|v| v.iter().any(|e| e.type_field == "tailscale")) { Some(configuration_envelope(root)?) } else { None }; Ok(())
}
pub(crate) fn bind_configuration_json(config: &mut serde_json::Value, root: &Path) -> Result<(), String> {
    let has = config.get("endpoints").and_then(|v| v.as_array()).is_some_and(|v| v.iter().any(|e| e.get("type").and_then(|v| v.as_str()) == Some("tailscale")));
    let object = config.as_object_mut().ok_or("invalid Windows config object")?;
    if has { object.insert("polaris_windows_directory".into(), configuration_envelope(root)?); } else { object.remove("polaris_windows_directory"); } Ok(())
}
mod security;
#[cfg(test)]
mod tests;

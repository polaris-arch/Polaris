//! Private SYSTEM birth authority. No caller-selected SID, receipt, existing
//! inode or generic File can construct a Cohort. Unknown generations persist.
use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::{fs, io, mem, ptr};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, FILE_PERSISTENT_ACLS};
use windows_sys::Win32::System::Threading::*;

mod spawn;
pub(super) use spawn::{spawn, ChildCustody};

const MAX_IMAGE: u64 = 96 * 1024 * 1024;
fn error(text: &'static str) -> AdmissionError {
    AdmissionError::new(AdmissionErrorKind::LoaderPolicy, text)
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn wide(value: impl AsRef<std::ffi::OsStr>) -> io::Result<Vec<u16>> {
    let mut v: Vec<_> = value.as_ref().encode_wide().collect();
    if v.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "embedded NUL"));
    }
    v.push(0);
    Ok(v)
}
fn owned(h: HANDLE) -> io::Result<OwnedHandle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: success transfers exactly one freshly returned Win32 HANDLE.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

pub(super) fn system_actor() -> io::Result<()> {
    let mut thread = ptr::null_mut();
    // SAFETY: query only this thread; no impersonation token may issue a cohort.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut thread) } != 0 {
        drop(owned(thread)?);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "impersonating issuer refused",
        ));
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
        return Err(io::Error::last_os_error());
    }
    let mut raw = ptr::null_mut();
    // SAFETY: capture the actual current primary token, not RPC metadata.
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) })?;
    let token = owned(raw)?;
    let mut bytes = 0;
    // SAFETY: native size-only query followed by aligned bounded storage.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut bytes,
        );
    }
    if bytes < mem::size_of::<TOKEN_USER>() as u32 || bytes > 65536 {
        return Err(io::Error::other("invalid token user size"));
    }
    let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
    // SAFETY: buffer is aligned and has the required writable capacity.
    check(unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    })?;
    // SAFETY: successful query initialized TOKEN_USER and its contained SID.
    if unsafe {
        IsWellKnownSid(
            (*(buf.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            WinLocalSystemSid,
        )
    } == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "execution issuer requires the SYSTEM primary token",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    volume: u64,
    id: [u8; 16],
    bytes: u64,
}
fn identity(f: &File, single: bool) -> io::Result<Identity> {
    let mut info: FILE_ID_INFO = unsafe { mem::zeroed() };
    // SAFETY: exact live handle and correctly sized writable output.
    check(unsafe {
        GetFileInformationByHandleEx(
            f.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            mem::size_of_val(&info) as u32,
        )
    })?;
    let s = f.metadata()?;
    let mut ordinary: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    // SAFETY: same live File and native output size.
    check(unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut ordinary) })?;
    if s.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || single && (!s.is_file() || ordinary.nNumberOfLinks != 1)
    {
        return Err(io::Error::other("direct single-link object required"));
    }
    Ok(Identity {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
        bytes: s.len(),
    })
}
fn guard(path: &Path, directory: bool) -> io::Result<File> {
    fs::OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL | if directory { 0 } else { FILE_EXECUTE })
        .share_mode(FILE_SHARE_READ | if directory { FILE_SHARE_WRITE } else { 0 })
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)
}

fn occupancy_path(root: &Path, id: &CoreId) -> PathBuf {
    root.join("leases")
        .join(format!("{}.execution", id.as_str()))
}
pub(super) fn maintenance_file(root: &Path, id: &CoreId) -> io::Result<Option<File>> {
    let path = occupancy_path(root, id);
    let opened = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path);
    match opened {
        Ok(file) => {
            identity(&file, true)?;
            Ok(Some(file))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "execution occupancy is active or unknown",
        )),
        Err(e) => Err(e),
    }
}
fn final_path(f: &File) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 32768];
    // SAFETY: live object and capacity measured in WCHARs; reject truncation.
    let n = unsafe {
        GetFinalPathNameByHandleW(
            f.as_raw_handle(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    if n == 0 || n >= buf.len() {
        return Err(io::Error::other("volume GUID path unavailable"));
    }
    let text = String::from_utf16(&buf[..n]).map_err(io::Error::other)?;
    if !text.starts_with(r"\\?\Volume{") {
        return Err(io::Error::other("local volume GUID required"));
    }
    Ok(PathBuf::from(text))
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    fn private() -> io::Result<Self> {
        let text = wide("O:SYG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")?;
        let mut sd = ptr::null_mut();
        // SAFETY: native parser receives a terminated constant SDDL; owns result.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                ptr::null_mut(),
            )
        })?;
        Ok(Self(sd))
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn private_parent(f: &File) -> io::Result<()> {
    let (mut owner, mut dacl, mut sd) = (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
    // SAFETY: GetSecurityInfo returns owner/DACL borrowed from one allocated SD.
    let status = unsafe {
        GetSecurityInfo(
            f.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut sd,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let descriptor = Descriptor(sd);
    let privileged = |sid| unsafe {
        IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
    };
    if dacl.is_null() || !privileged(owner) {
        return Err(io::Error::other(
            "execution parent owner/DACL not privileged",
        ));
    }
    let (mut system, mut admin) = (false, false);
    // SAFETY: DACL is held by descriptor; GetAce validates each indexed entry.
    for i in 0..unsafe { (*dacl).AceCount } {
        let mut raw = ptr::null_mut();
        check(unsafe { GetAce(dacl, i as u32, &mut raw) })?;
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
            continue;
        }
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
            || usize::from(header.AceSize) < mem::size_of::<ACCESS_ALLOWED_ACE>()
        {
            return Err(io::Error::other("unsupported execution parent ACE"));
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = ptr::addr_of!(ace.SidStart).cast_mut().cast();
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(io::Error::other("invalid execution parent SID"));
        }
        let writes = FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | FILE_DELETE_CHILD
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER
            | GENERIC_ALL
            | GENERIC_WRITE;
        if ace.Mask & writes != 0 && !privileged(sid) {
            return Err(io::Error::other(
                "untrusted execution parent mutation grant",
            ));
        }
        system |= unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } != 0
            && ace.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS;
        admin |= unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) } != 0
            && ace.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS;
    }
    drop(descriptor);
    if !system || !admin {
        return Err(io::Error::other(
            "missing privileged execution parent grants",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct Cohort {
    root: PathBuf,
    final_exe: PathBuf,
    final_sidecar: PathBuf,
    files: Vec<(PathBuf, File, Identity)>,
    directories: Vec<File>,
    parents: Vec<File>,
    system: Vec<File>,
    occupancy: File,
    _birth_data_lease: File,
}
impl Cohort {
    pub(super) fn is_current(&self) -> bool {
        system_actor().is_ok()
            && self.files.iter().all(|(p, f, id)| {
                identity(f, true).is_ok_and(|n| n == *id)
                    && guard(p, false)
                        .and_then(|g| identity(&g, true))
                        .is_ok_and(|n| n == *id)
            })
            && fs::read_dir(&self.root).is_ok_and(|rows| {
                let names = rows
                    .map(|r| r.map(|e| e.file_name()))
                    .collect::<io::Result<Vec<_>>>();
                names.is_ok_and(|v| {
                    v.len() == 2
                        && v.iter()
                            .all(|n| n == "sing-box.exe" || n == "libcronet.dll")
                })
            })
    }
}
impl Drop for Cohort {
    fn drop(&mut self) {
        // Only this lifetime's exact names, never a stale receipt/unknown tree.
        // Unknown children deliberately retain an Arc and cannot enter here.
        let expected: Vec<_> = self
            .files
            .iter()
            .filter_map(|(p, _, _)| p.file_name().map(|x| x.to_os_string()))
            .collect();
        let actual = fs::read_dir(&self.root).and_then(|rows| {
            rows.map(|row| row.map(|r| r.file_name()))
                .collect::<io::Result<Vec<_>>>()
        });
        let safe = actual.is_ok_and(|names| {
            names.len() == expected.len() && names.iter().all(|n| expected.contains(n))
        }) && self.files.iter().all(|(p, f, id)| {
            identity(f, true).is_ok_and(|n| n == *id)
                && guard(p, false)
                    .and_then(|g| identity(&g, true))
                    .is_ok_and(|n| n == *id)
        });
        let paths: Vec<_> = self.files.iter().map(|(p, _, _)| p.clone()).collect();
        self.files.clear();
        if safe {
            for p in paths {
                if fs::remove_file(p).is_err() {
                    return;
                }
            }
            self.directories.clear();
            let _ = fs::remove_dir(&self.root);
        }
        // Parent handles remain through cleanup. No recursive delete.
    }
}

pub(super) fn issue(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<Cohort, AdmissionError> {
    system_actor()?;
    if input.binary.file_name() != Some(std::ffi::OsStr::new("sing-box.exe")) {
        return Err(unavailable());
    }
    let version = input.binary.parent().ok_or_else(unavailable)?;
    let versions = version.parent().ok_or_else(unavailable)?;
    let store = versions.parent().ok_or_else(unavailable)?;
    let support = store.parent().ok_or_else(unavailable)?;
    if version.file_name() != Some(std::ffi::OsStr::new(policy.id.as_str()))
        || versions.file_name() != Some(std::ffi::OsStr::new("versions"))
        || store.file_name() != Some(std::ffi::OsStr::new("core"))
        || !support.is_absolute()
    {
        return Err(unavailable());
    }
    let mut parents = Vec::new();
    let mut p = PathBuf::new();
    for c in support.components() {
        use std::path::{Component, Prefix};
        match c {
            Component::Prefix(p) if matches!(p.kind(), Prefix::Disk(_)) => {}
            Component::RootDir => {}
            Component::Normal(n)
                if !n.to_string_lossy().ends_with(&['.', ' '][..])
                    && !n.to_string_lossy().contains(':') => {}
            _ => return Err(unavailable()),
        }
        p.push(c);
        if p.has_root() {
            let f = guard(&p, true)?;
            identity(&f, false)?;
            parents.push(f);
        }
    }
    private_parent(parents.last().ok_or_else(unavailable)?)?;
    let descriptor = Descriptor::private()?;
    let sa = descriptor.attributes();
    // Keep the real existing StartShared lock across marker creation/open and
    // the entire issuer lifetime. Never manufacture a missing id.lock.
    let lease_dir = store.join("leases");
    let held_leases = guard(&lease_dir, true)?;
    private_parent(&held_leases)?;
    parents.push(held_leases);
    let data_lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(lease_dir.join(format!("{}.lock", policy.id.as_str())))?;
    identity(&data_lease, true)?;
    private_parent(&data_lease)?;
    data_lease
        .try_lock_shared()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let marker = occupancy_path(store, &policy.id);
    // This file is occupancy data only. Existing marker identity never mints
    // execution; only the fresh image owner below can do that. Data-exclusive
    // mutation always precedes its mandatory RW gate, closing missing-marker races.
    let created = unsafe {
        CreateFileW(
            wide(&marker)?.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &sa,
            CREATE_NEW,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if created != INVALID_HANDLE_VALUE {
        let file = File::from(owned(created)?);
        file.sync_all()?;
        drop(file);
    } else if !matches!(io::Error::last_os_error().raw_os_error(), Some(80 | 183)) {
        return Err(io::Error::last_os_error().into());
    }
    let occupancy = guard(&marker, false)?;
    identity(&occupancy, true)?;
    private_parent(&occupancy)?;
    let mut random = [0u8; 24];
    // SAFETY: system RNG initializes an exactly sized local nonce buffer.
    if unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            random.as_mut_ptr(),
            random.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } != 0
    {
        return Err(error("issuer random unavailable"));
    }
    let root = support.join(format!(".execution-{}", lowercase_hex(&random)));
    // SAFETY: CREATE_NEW directory with a private birth SD; never adopt existing.
    check(unsafe { CreateDirectoryW(wide(&root)?.as_ptr(), &sa) })?;
    let d = guard(&root, true)?;
    let volume = identity(&d, false)?.volume;
    let (mut fs_name, mut fs_flags) = ([0u16; 32], 0u32);
    // SAFETY: query the actual held cohort volume, not a drive-name guess.
    check(unsafe {
        GetVolumeInformationByHandleW(
            d.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut fs_flags,
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    })?;
    let n = fs_name
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(fs_name.len());
    if String::from_utf16_lossy(&fs_name[..n]) != "NTFS" || fs_flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(error("issuer requires NTFS persistent security"));
    }
    let mut cohort = Cohort {
        root: root.clone(),
        final_exe: PathBuf::new(),
        final_sidecar: PathBuf::new(),
        files: Vec::new(),
        directories: vec![d],
        parents,
        system: Vec::new(),
        occupancy,
        _birth_data_lease: data_lease,
    };
    let mut imports = Vec::new();
    if policy.manifest.files.len() != 2 {
        return Err(error(
            "Windows execution requires exact Core/sidecar members",
        ));
    }
    for member in &policy.manifest.files {
        if !["sing-box.exe", "libcronet.dll"].contains(&member.name.as_str())
            || member.bytes == 0
            || member.bytes > MAX_IMAGE
        {
            return Err(error("unsupported Windows executable cohort"));
        }
        let source = guard(&version.join(&member.name), false)?;
        if identity(&source, true)?.volume != volume {
            return Err(error("cross-volume issuer input refused"));
        }
        let path = root.join(&member.name);
        // SAFETY: exclusive CREATE_NEW, no inherited handle, private SD at birth.
        let mut output = File::from(owned(unsafe {
            CreateFileW(
                wide(&path)?.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                &sa,
                CREATE_NEW,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        })?);
        let mut source = source.take(member.bytes + 1);
        let written = io::copy(&mut source, &mut output)?;
        if written != member.bytes {
            return Err(error("selected input size differs"));
        }
        output.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        (&mut output)
            .take(member.bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != member.bytes || sha256_hex(&bytes) != member.sha256 {
            return Err(error("selected output digest differs"));
        }
        imports.extend(pe::imports(&bytes)?);
        output.sync_all()?;
        let born = identity(&output, true)?;
        drop(output);
        let file = guard(&path, false)?;
        if identity(&file, true)? != born {
            return Err(error("issued image identity changed"));
        }
        let final_name = final_path(&file)?;
        if member.name == "sing-box.exe" {
            cohort.final_exe = final_name;
        } else {
            cohort.final_sidecar = final_name;
        }
        cohort.files.push((path, file, born));
    }
    if cohort.final_exe.as_os_str().is_empty() || cohort.final_sidecar.as_os_str().is_empty() {
        return Err(error("incomplete execution cohort"));
    }
    spawn::hold_system_imports(&imports, &mut cohort)?;
    if !cohort.is_current() {
        return Err(error("new cohort custody changed"));
    }
    Ok(cohort)
}

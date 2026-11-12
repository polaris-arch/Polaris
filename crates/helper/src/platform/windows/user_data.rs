//! Current-process recovery DATA custody. Never an execution provider or an
//! implicit interpretation of OwnerPolicy::uid. No caller-selected SID/policy.
#![allow(
    unsafe_code,
    reason = "bounded native token, descriptor and held-file queries"
)]

use super::{coreacl, winproc};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::{
    ffi::OsStrExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::path::{Component, Path, PathBuf, Prefix};
use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, FILE_WRITE_THROUGH,
    },
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::{
    Foundation::*, Security::Authorization::*, Security::*, Storage::FileSystem::*,
    System::Threading::*,
};

fn changed(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}
fn unsupported(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, detail)
}
fn native_query_failure() -> io::Error {
    // An unreadable native observation is Unknown, not evidence of damage or
    // permission to use an ordinary path. Preserve the native cause as source.
    io::Error::other(io::Error::last_os_error())
}
fn security(file: &File) -> io::Result<coreacl::ObjectSecurity> {
    winproc::file_security(file).map_err(io::Error::other)
}
fn acl_error(error: coreacl::UserDataAclError) -> io::Error {
    match error {
        coreacl::UserDataAclError::Unavailable => {
            io::Error::other("user data security unavailable")
        }
        coreacl::UserDataAclError::UnsupportedAce => {
            unsupported("user data ACE cannot be evaluated")
        }
        coreacl::UserDataAclError::Untrusted => {
            io::Error::new(io::ErrorKind::PermissionDenied, "untrusted user data ACL")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity(u32, u64);
fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut value = std::mem::MaybeUninit::uninit();
    // SAFETY: File owns the live handle and the output has the exact native ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), value.as_mut_ptr()) } == 0 {
        return Err(native_query_failure());
    }
    // SAFETY: successful native call initializes the output.
    Ok(unsafe { value.assume_init() })
}
fn identity(file: &File) -> io::Result<Identity> {
    let i = information(file)?;
    let id = (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow);
    if id == 0 || id == u64::MAX {
        return Err(unsupported(
            "filesystem did not provide a stable file identity",
        ));
    }
    Ok(Identity(i.dwVolumeSerialNumber, id))
}

// Derive the one exceptional namespace anchor from the OS, not an environment
// variable, manifest or caller-selected drive. GUID spelling AND held FileID
// must match the root of the volume containing the actual Windows directory.
fn is_native_system_volume_root(file: &File) -> io::Result<bool> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemWindowsDirectoryW(path: *mut u16, size: u32) -> u32;
    }
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0u16; 32768];
    // SAFETY: bounded native output, no process-controlled path input.
    let count = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if count == 0 {
        return Err(native_query_failure());
    }
    if count as usize >= buffer.len() {
        return Err(unsupported("system Windows path exceeds native bound"));
    }
    let mut windows = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count as usize]));
    let mut components = windows.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => prefix,
        _ => {
            return Err(unsupported(
                "system Windows directory is not on a local drive",
            ));
        }
    };
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(r"\");
    // The documented root-installed Windows form is just "C:", not C:\.
    match components.next() {
        None => windows = root.clone(),
        Some(Component::RootDir) => {}
        _ => return Err(changed("noncanonical native Windows directory")),
    }
    let held_root = open(&root, true, true)?;
    let held_windows = open(&windows, true, true)?;
    require_ntfs(&held_root)?;
    require_ntfs(&held_windows)?;
    let root_id = identity(&held_root)?;
    let root_guid = final_path(&held_root)?;
    if identity(&held_windows)?.0 != root_id.0
        || !final_path(&held_windows)?.starts_with(&root_guid)
    {
        return Err(changed("system Windows directory changed volumes"));
    }
    Ok(identity(file)? == root_id && final_path(file)? == root_guid)
}
fn open(path: &Path, directory: bool, pin: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | if pin { 0 } else { FILE_SHARE_DELETE })
        .open(path)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                io::Error::other(e)
            } else {
                e
            }
        })?;
    let i = information(&file)?;
    if i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || file.metadata()?.is_dir() != directory
        || (!directory && (!file.metadata()?.is_file() || i.nNumberOfLinks != 1))
    {
        return Err(changed(
            "user data object is redirected, multiply linked or has a wrong type",
        ));
    }
    Ok(file)
}
fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut name = vec![0u16; 32768];
    // SAFETY: bounded output and a live handle; GUID form avoids mutable DOS aliases.
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if count == 0 {
        return Err(native_query_failure());
    }
    if count as usize >= name.len() {
        return Err(unsupported("user data path exceeds native bound"));
    }
    let name =
        String::from_utf16(&name[..count as usize]).map_err(|_| changed("invalid native path"))?;
    if !name.starts_with(r"\\?\Volume{") {
        return Err(unsupported("user data requires a local volume GUID"));
    }
    Ok(PathBuf::from(name))
}
// FILE_PERSISTENT_ACLS = 0x8, GetVolumeInformationByHandleW contract.
fn require_ntfs(file: &File) -> io::Result<()> {
    let mut flags = 0;
    let mut name = [0u16; 32];
    // SAFETY: optional outputs are null; remaining bounded native outputs are live.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return Err(native_query_failure());
    }
    let n = name.iter().position(|x| *x == 0).unwrap_or(name.len());
    if String::from_utf16_lossy(&name[..n]) != "NTFS" || flags & 0x0000_0008 == 0 {
        return Err(unsupported(
            "current user data requires NTFS persistent ACLs and native identities",
        ));
    }
    Ok(())
}

struct ProcessIdentity {
    _token: OwnedHandle,
    sid: String,
}
impl ProcessIdentity {
    fn capture() -> io::Result<Self> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: query the actual current process primary token, never a supplied SID.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(native_query_failure());
        }
        // SAFETY: successful OpenProcessToken transfers one owned native handle.
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut needed = 0;
        // SAFETY: size-only query with a null buffer.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        if needed < std::mem::size_of::<TOKEN_USER>() as u32 || needed > 65536 {
            return Err(io::Error::other("process SID size unavailable"));
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: aligned storage has at least the queried byte size.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(native_query_failure());
        }
        // SAFETY: successful TokenUser query initialized this aligned structure.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        // SAFETY: SID is borrowed from the live native token information buffer.
        if unsafe { IsValidSid(user.User.Sid) } == 0 {
            return Err(io::Error::other("invalid process SID"));
        }
        let sid = winproc::sid_to_string(user.User.Sid)
            .ok_or_else(|| io::Error::other("process SID unavailable"))?;
        Ok(Self { _token: token, sid })
    }
}

struct Anchor {
    file: File,
    path: PathBuf,
    id: Identity,
    ancestor: bool,
    system_volume_root: bool,
}
/// Opaque, native-captured data scope. No serde/wire/manifest constructor.
pub(crate) struct CurrentUserData {
    process: ProcessIdentity,
    root: PathBuf,
    anchors: Vec<Anchor>,
    volume: u32,
}
impl CurrentUserData {
    pub(crate) fn open(root: &Path) -> io::Result<Self> {
        let process = ProcessIdentity::capture()?;
        let mut parts = root.components();
        let prefix = match parts.next() {
            Some(Component::Prefix(p))
                if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
            {
                p
            }
            _ => {
                return Err(unsupported(
                    "user data requires an absolute local drive path",
                ));
            }
        };
        if !matches!(parts.next(), Some(Component::RootDir)) {
            return Err(changed("relative user data root"));
        }
        let names = parts
            .map(|p| match p {
                Component::Normal(n)
                    if !n.to_string_lossy().contains(':')
                        && !n.to_string_lossy().ends_with(['.', ' ']) =>
                {
                    Ok(n.to_os_string())
                }
                _ => Err(changed("noncanonical user data ancestry")),
            })
            .collect::<io::Result<Vec<_>>>()?;
        if names.is_empty() || names.len() > 64 {
            return Err(changed("invalid user data depth"));
        }
        let mut drive = PathBuf::from(prefix.as_os_str());
        drive.push(r"\");
        let file = open(&drive, true, true)?;
        require_ntfs(&file)?;
        let id = identity(&file)?;
        let mut path = final_path(&file)?;
        let sec = security(&file)?;
        let system_volume_root = sec.owner_sid.as_deref() == Some(coreacl::TRUSTED_INSTALLER_SID)
            && is_native_system_volume_root(&file)?;
        judge_anchor(&sec, &process.sid, true, system_volume_root)?;
        let mut anchors = vec![Anchor {
            file,
            path: path.clone(),
            id,
            ancestor: true,
            system_volume_root,
        }];
        for (index, name) in names.iter().enumerate() {
            path.push(name);
            let last = index + 1 == names.len();
            let file = open(&path, true, true)?;
            let actual = identity(&file)?;
            if actual.0 != id.0 {
                return Err(changed("cross-volume user data ancestry"));
            }
            coreacl::judge_current_user_data(&security(&file)?, &process.sid, true, !last)
                .map_err(acl_error)?;
            let canonical = final_path(&file)?;
            // Each already-held parent denies delete sharing and each child
            // rejects reparse points. Retain the actual canonical spelling,
            // rather than confusing legitimate NTFS casing/8.3 aliases with
            // a different native object. The held FileID is authoritative.
            path = canonical;
            anchors.push(Anchor {
                file,
                path: path.clone(),
                id: actual,
                ancestor: !last,
                system_volume_root: false,
            });
        }
        let value = Self {
            process,
            root: path,
            anchors,
            volume: id.0,
        };
        value.guard()?;
        value.check(&value.root)?;
        Ok(value)
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn sid(&self) -> &str {
        &self.process.sid
    }
    // Pin every parent component before creating a new member. This neither
    // follows a reparse parent nor repairs the owner/ACL of an existing name.
    fn creation_parent(&self, path: &Path) -> io::Result<Vec<File>> {
        self.guard()?;
        self.member_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| changed("new data parent absent"))?;
        let relative = parent
            .strip_prefix(&self.root)
            .map_err(|_| changed("new data parent outside scope"))?;
        let mut at = self.root.clone();
        let mut held = vec![open(&at, true, true)?];
        self.check_held(&held[0], true)?;
        for part in relative.components() {
            at.push(part.as_os_str());
            let file = open(&at, true, true)?;
            self.check_held(&file, true)?;
            held.push(file);
        }
        Ok(held)
    }
    pub(crate) fn create_directory(&self, path: &Path) -> io::Result<()> {
        let parents = self.creation_parent(path)?;
        let file = create_member(parents.last().unwrap(), path, self.sid(), true, false)?;
        self.stamp_named_held(&file, path, true)?;
        Ok(())
    }
    pub(crate) fn create_file(&self, path: &Path, write_through: bool) -> io::Result<File> {
        let parents = self.creation_parent(path)?;
        let file = create_member(
            parents.last().unwrap(),
            path,
            self.sid(),
            false,
            write_through,
        )?;
        self.stamp_named_held(&file, path, false)?;
        Ok(file)
    }
    pub(crate) fn guard(&self) -> io::Result<()> {
        for anchor in &self.anchors {
            if identity(&anchor.file)? != anchor.id
                || identity(&open(&anchor.path, true, false)?)? != anchor.id
            {
                return Err(changed("user data ancestor identity changed"));
            }
            judge_anchor(
                &security(&anchor.file)?,
                self.sid(),
                anchor.ancestor,
                anchor.system_volume_root,
            )?;
        }
        Ok(())
    }
    pub(crate) fn check(&self, path: &Path) -> io::Result<()> {
        self.guard()?;
        self.member_path(path)?;
        let directory = std::fs::symlink_metadata(path)?.is_dir();
        self.check_held(&open(path, directory, false)?, directory)
    }
    fn member_path(&self, path: &Path) -> io::Result<()> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| changed("object outside selected data namespace"))?;
        if relative.components().any(|c| {
            !matches!(c, Component::Normal(n)
            if !n.to_string_lossy().contains(':') && !n.to_string_lossy().ends_with(['.', ' ']))
        }) {
            return Err(changed("noncanonical data member"));
        }
        Ok(())
    }
    pub(crate) fn check_held(&self, file: &File, directory: bool) -> io::Result<()> {
        self.guard()?;
        let i = information(file)?;
        if i.dwVolumeSerialNumber != self.volume
            || i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || file.metadata()?.is_dir() != directory
            || (!directory && i.nNumberOfLinks != 1)
        {
            return Err(changed("user data held identity or type differs"));
        }
        self.member_path(&final_path(file)?)?;
        let sec = security(file)?;
        coreacl::judge_current_user_data(&sec, self.sid(), directory, false).map_err(acl_error)?;
        if directory {
            let mut child = sec;
            if let Some(aces) = &mut child.dacl {
                aces.retain(|a| a.flags & 3 != 0);
                if !aces.iter().any(|a| {
                    a.sid == self.sid() && a.kind == coreacl::AceKind::Allow && a.touches_write()
                }) {
                    return Err(unsupported(
                        "user data lacks explicit trusted child inheritance",
                    ));
                }
                for ace in aces {
                    ace.flags &= !coreacl::ACE_FLAG_INHERIT_ONLY;
                }
            }
            coreacl::judge_current_user_data(&child, self.sid(), true, false).map_err(acl_error)?;
        }
        Ok(())
    }
    /// Bind a consumer's actual opened object, not a separately checked name.
    pub(crate) fn stamp_named_held(
        &self,
        held: &File,
        path: &Path,
        directory: bool,
    ) -> io::Result<String> {
        self.member_path(path)?;
        self.check_held(held, directory)?;
        if identity(held)? != identity(&open(path, directory, false)?)? {
            return Err(changed("data name no longer identifies held object"));
        }
        let i = information(held)?;
        Ok(format!(
            "{:?}:{}:{}:{}:{}:{}:{}:{}:{}",
            identity(held)?,
            i.nFileSizeHigh,
            i.nFileSizeLow,
            i.dwFileAttributes,
            i.ftCreationTime.dwHighDateTime,
            i.ftCreationTime.dwLowDateTime,
            i.ftLastWriteTime.dwHighDateTime,
            i.ftLastWriteTime.dwLowDateTime,
            i.nNumberOfLinks
        ))
    }
}

// Public callers supply only a new leaf, never an owner or security policy.
// Native parent custody and TokenUser are captured afresh; only CREATE_NEW is
// permitted. Elevated tokens' default owner (often BA) is deliberately unused.
pub(crate) fn create_current_user_directory(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| changed("new data parent absent"))?;
    let scope = CurrentUserData::open(parent)?;
    let leaf = path
        .file_name()
        .ok_or_else(|| changed("new data leaf absent"))?;
    scope.create_directory(&scope.root().join(leaf))
}
pub(crate) fn create_current_user_file(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| changed("new data parent absent"))?;
    let scope = CurrentUserData::open(parent)?;
    let leaf = path
        .file_name()
        .ok_or_else(|| changed("new data leaf absent"))?;
    scope.create_file(&scope.root().join(leaf), false)
}

fn create_member(
    parent: &File,
    path: &Path,
    sid: &str,
    directory: bool,
    write_through: bool,
) -> io::Result<File> {
    let name = path
        .file_name()
        .ok_or_else(|| changed("new data leaf absent"))?;
    let mut units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty() || units.iter().any(|u| [0, 47, 92, 58].contains(u)) {
        return Err(changed("invalid new data leaf"));
    }
    let length = u16::try_from(units.len() * 2).map_err(|_| changed("new data leaf too long"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: units.as_mut_ptr(),
    };
    let inheritance = if directory { "OICI" } else { "" };
    let sddl: Vec<u16> = format!(
        "O:{sid}D:P(A;{inheritance};FA;;;{sid})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)"
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: native-captured SID, fixed SDDL, terminated input and owned output.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &name,
        Attributes: 0x40 | 0x1000, // OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE
        SecurityDescriptor: descriptor.cast(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut raw = std::ptr::null_mut();
    let mut status = std::mem::MaybeUninit::<IO_STATUS_BLOCK>::uninit();
    // SAFETY: held parent, one bounded relative leaf and owned descriptor remain
    // live throughout CREATE_NEW. No existing object's security is modified.
    let result = unsafe {
        NtCreateFile(
            &mut raw,
            READ_CONTROL | FILE_GENERIC_READ | if directory { 0 } else { FILE_GENERIC_WRITE },
            &attributes,
            status.as_mut_ptr(),
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_CREATE,
            FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_NON_DIRECTORY_FILE
                }
                | if write_through { FILE_WRITE_THROUGH } else { 0 },
            std::ptr::null(),
            0,
        )
    };
    // SAFETY: one LocalAlloc allocation returned by the conversion above.
    unsafe { LocalFree(descriptor) };
    if result < 0 {
        // SAFETY: translates the returned native status without changing state.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    // SAFETY: successful NtCreateFile transfers exactly one owned file HANDLE.
    let file = unsafe { File::from_raw_handle(raw) };
    if winproc::file_owner(&file)? != sid {
        return Err(changed("new data owner differs from TokenUser"));
    }
    Ok(file)
}

fn judge_anchor(
    sec: &coreacl::ObjectSecurity,
    sid: &str,
    ancestor: bool,
    system_volume_root: bool,
) -> io::Result<()> {
    if system_volume_root && ancestor {
        coreacl::judge_system_volume_data_ancestor(sec, sid)
    } else {
        coreacl::judge_current_user_data(sec, sid, true, ancestor)
    }
    .map_err(acl_error)
}

// Test fixture producer: production data scopes never create their root.
#[cfg(test)]
fn create_private_directory(path: &Path, sid: &str) -> io::Result<()> {
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: SID is native captured, terminated SDDL is live, revision is fixed.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: bounded terminated path and owned descriptor remain live at creation.
    let result = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let error = if result == 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    // SAFETY: conversion returned this LocalAlloc allocation, released once.
    unsafe {
        LocalFree(descriptor);
    }
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

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
    pub(crate) fn open(root: &Path, create: bool) -> io::Result<Self> {
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
            if create && last {
                match std::fs::symlink_metadata(&path) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        create_private_directory(&path, &process.sid)?
                    }
                    Err(e) => return Err(e),
                    Ok(_) => {}
                }
            }
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
    /// Metadata/FileID recheck of the admitted data observation; no hot rescan.
    pub(crate) fn stamp(&self, path: &Path) -> io::Result<String> {
        let directory = std::fs::symlink_metadata(path)?.is_dir();
        let held = open(path, directory, false)?;
        self.stamp_named_held(&held, path, directory)
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
    /// Do not pretend a directory flush is supported or flush the whole volume.
    pub(crate) fn barrier(&self) -> io::Result<()> {
        self.guard()?;
        if self.anchors.len() < 2 {
            return Err(changed("data root and parent custody absent"));
        }
        // Only the last directory can be created by this scope. Flush its
        // metadata first and then its parent's new directory entry; flushing
        // the child alone cannot justify a newly durable recovery namespace.
        for anchor in self.anchors.iter().rev().take(2) {
            flush_barrier_file(&open_barrier_anchor(anchor)?)?;
        }
        self.guard()
    }
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

// Shared by the real transaction precondition and the native capability test.
// These I/O primitives confer no namespace or execution authority by themselves.
fn open_barrier_anchor(anchor: &Anchor) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&anchor.path)?;
    if identity(&file)? != anchor.id {
        return Err(changed("user data barrier identity changed"));
    }
    Ok(file)
}
fn flush_barrier_file(file: &File) -> io::Result<()> {
    match file.sync_all() {
        // Preserve the actual native error as the cause, including lastError.
        Err(e) if matches!(e.raw_os_error(), Some(1 | 6 | 50)) => {
            Err(io::Error::new(io::ErrorKind::Unsupported, e))
        }
        result => result,
    }
}

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
mod tests {
    use super::*;
    fn private_profile_fixture() -> (ProcessIdentity, PathBuf, PathBuf) {
        #[link(name = "userenv")]
        unsafe extern "system" {
            fn GetUserProfileDirectoryW(token: HANDLE, path: *mut u16, size: *mut u32) -> i32;
        }
        use std::os::windows::ffi::OsStringExt;
        let process = ProcessIdentity::capture().unwrap();
        let mut profile = vec![0u16; 32768];
        let mut size = profile.len() as u32;
        // SAFETY: actual held primary token; bounded writable native output.
        assert_ne!(
            unsafe {
                GetUserProfileDirectoryW(
                    process._token.as_raw_handle(),
                    profile.as_mut_ptr(),
                    &mut size,
                )
            },
            0
        );
        let end = profile.iter().position(|c| *c == 0).unwrap();
        let profile = PathBuf::from(std::ffi::OsString::from_wide(&profile[..end]));
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = profile.join(format!(
            "polaris-windows-data-native-{}-{nonce}",
            std::process::id()
        ));
        create_private_directory(&fixture, &process.sid).unwrap();
        (process, profile, fixture)
    }

    #[test]
    fn native_profile_namespace_and_actual_root_parent_barrier_facts() {
        let (process, profile, fixture) = private_profile_fixture();
        let root = fixture.join("selected-core-recovery");
        create_private_directory(&root, &process.sid).unwrap();
        let mut ancestors = Vec::new();
        for path in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let held = open(path, true, true).unwrap();
            let sec = security(&held).unwrap();
            let system_volume_root = sec.owner_sid.as_deref()
                == Some(coreacl::TRUSTED_INSTALLER_SID)
                && is_native_system_volume_root(&held).unwrap();
            ancestors.push(serde_json::json!({"path":path,"owner":sec.owner_sid,
                "native_id":format!("{:?}",identity(&held).unwrap()),
                "ordinary_data_policy":format!("{:?}",coreacl::judge_current_user_data(&sec,&process.sid,true,path!=root)),
                "native_system_volume_root":system_volume_root,
                "effective_namespace_policy":format!("{:?}",judge_anchor(&sec,&process.sid,path!=root,system_volume_root)),
                "allow_aces":sec.dacl.as_ref().map(|a|a.iter().map(|v|format!("{:?}:{}:{:#x}:{:#x}",v.kind,v.sid,v.mask,v.flags)).collect::<Vec<_>>())}));
        }
        let namespace = CurrentUserData::open(&root, false);
        let namespace_result = match &namespace {
            Ok(scope) => {
                scope.guard().unwrap();
                "accepted".to_string()
            }
            Err(e) => format!("{:?}: {e}", e.kind()),
        };
        let mut barriers = Vec::new();
        for (role, path) in [("root", &root), ("immediate_parent", &fixture)] {
            let file = open(path, true, true).unwrap();
            let sec = security(&file).unwrap();
            assert!(coreacl::judge_current_user_data(&sec, &process.sid, true, false).is_ok());
            assert!(!coreacl::judge_object("user data", &sec).is_empty());
            require_ntfs(&file).unwrap();
            assert!(!is_native_system_volume_root(&file).unwrap());
            let anchor = Anchor {
                id: identity(&file).unwrap(),
                path: final_path(&file).unwrap(),
                file,
                ancestor: false,
                system_volume_root: false,
            };
            let mut row = serde_json::json!({"role":role,"path":path,"guid":anchor.path,"id":format!("{:?}",anchor.id),"local_data_acl":"accepted","system_admin_policy":"rejected","desired_access":"GENERIC_READ|GENERIC_WRITE (includes READ_CONTROL)","flags":"BACKUP_SEMANTICS|OPEN_REPARSE_POINT","share":"READ|WRITE"});
            match open_barrier_anchor(&anchor) {
                Err(e) => {
                    row["phase"] = "CreateFileW".into();
                    row["last_error"] = serde_json::json!(e.raw_os_error());
                    row["error"] = e.to_string().into();
                    row["FlushFileBuffers"] = "NOT_RUN: no write handle".into();
                }
                Ok(write) => {
                    row["held_id_equal"] =
                        serde_json::json!(identity(&write).unwrap() == anchor.id);
                    row["phase"] = "FlushFileBuffers".into();
                    match flush_barrier_file(&write) {
                        Ok(()) => row["result"] = "success".into(),
                        Err(e) => {
                            let raw = e.raw_os_error().or_else(|| {
                                e.get_ref()
                                    .and_then(|s| s.downcast_ref::<io::Error>())
                                    .and_then(io::Error::raw_os_error)
                            });
                            row["last_error"] = serde_json::json!(raw);
                            row["kind"] = format!("{:?}", e.kind()).into();
                            row["error"] = e.to_string().into();
                        }
                    }
                }
            }
            barriers.push(row);
        }
        let initialize = crate::core_store::CoreStore::initialize_current_user_data(&root);
        let initialize = match initialize {
            Ok(_) => "accepted".to_string(),
            Err(e) => format!("{:?}: {e}", e.kind()),
        };
        println!(
            "PROFILE_NATIVE_FACTS {}",
            serde_json::json!({"profile_from_primary_token":profile,"primary_sid":process.sid,"fixture":fixture,"ancestry":ancestors,"actual_scope":namespace_result,"production_CoreStore_initialize":initialize,"same_production_callee_barriers":barriers,"primitive_evidence_does_not_waive_namespace":true,"fixture_retained_for_inspection":true})
        );
    }

    #[test]
    fn native_publication_preserves_read_lease_and_flushes_only_changed_data_parents() {
        use crate::core_store::{CoreStore, InstallOutcome};
        use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest, InstalledCoreManifest};
        let (process, _, fixture) = private_profile_fixture();
        let source = fixture.join("inert-input");
        create_private_directory(&source, &process.sid).unwrap();
        // Inert bytes test platform I/O/leases only; never App admission or a
        // trusted carrier. The separate App test uses actual compiled resources.
        let files = [
            ("sing-box.exe", b"inert core data".as_slice()),
            ("libcronet.dll", b"inert sidecar data".as_slice()),
        ]
        .into_iter()
        .map(|(name, bytes)| {
            std::fs::write(source.join(name), bytes).unwrap();
            CoreFile {
                name: name.into(),
                bytes: bytes.len() as u64,
                sha256: crate::core_hash::sha256_hex(bytes),
            }
        })
        .collect();
        let core = CoreManifest {
            platform: "win".into(),
            version: "1.2.3".into(),
            provider_fingerprint: "a".repeat(64),
            files,
            helper_sha256: Some("b".repeat(64)),
            cleaner_sha256: Some("c".repeat(64)),
        };
        let id = core.id(crate::core_hash::sha256_hex).unwrap();
        let manifest = InstalledCoreManifest {
            schema: "polaris-core-manifest-v1".into(),
            id: id.as_str().into(),
            app_version: "1.0.0".into(),
            core,
        };
        let root = fixture.join("store");
        let store = CoreStore::initialize_current_user_data(&root).unwrap();
        let first = store.install(&manifest, &source, None, None);
        println!(
            "NATIVE_DATA_PUBLICATION first={first:?} fixture={}",
            fixture.display()
        );
        assert_eq!(first.unwrap(), InstallOutcome::Installed);
        let held = store.selected_data(&id).unwrap();
        assert!(held.data_is_current());
        // Closing the publication's separate writable flush handle must not
        // release this independently held read-only shared occupancy lease.
        assert_eq!(
            store.install(&manifest, &source, None, None).unwrap(),
            InstallOutcome::AlreadyPresent
        );
        let binary = held.directory.join("sing-box.exe");
        std::fs::write(&binary, b"damaged").unwrap();
        assert!(!held.data_is_current());
        assert!(store.install(&manifest, &source, None, None).is_err());
        drop(held);
        let repaired = store.install(&manifest, &source, None, None);
        println!("NATIVE_DATA_PUBLICATION repair={repaired:?}");
        assert_eq!(repaired.unwrap(), InstallOutcome::Installed);
        assert!(store.selected_data(&id).unwrap().data_is_current());
        // A real partial receipt must stay damage, never absence or fallback.
        let receipt = root.join("manifests").join(format!("{}.json", id.as_str()));
        let retained = fixture.join("retained-receipt.json");
        std::fs::rename(&receipt, &retained).unwrap();
        assert!(store.selected_data(&id).is_err());
        std::fs::rename(&retained, &receipt).unwrap();
        let unknown = root
            .join("versions")
            .join(id.as_str())
            .join("retain-unknown");
        std::fs::write(&unknown, b"retain unknown member").unwrap();
        assert!(store.selected_data(&id).is_err());
        assert!(store.install(&manifest, &source, None, None).is_err());
        assert_eq!(std::fs::read(&unknown).unwrap(), b"retain unknown member");
        assert_eq!(
            store.verify_for_start(&id).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        println!("NATIVE_DATA_PUBLICATION closure=PASS inert_data_only=true fixture_retained=true");
    }
    #[test]
    fn native_unreadable_descriptor_stays_unknown_without_ordinary_path_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("inert-data");
        std::fs::write(&path, b"inert").unwrap();
        // Deliberately omit READ_CONTROL on this fresh fixture's actual handle.
        let held = OpenOptions::new()
            .read(true)
            .access_mode(FILE_READ_ATTRIBUTES)
            .open(&path)
            .unwrap();
        assert_eq!(security(&held).unwrap_err().kind(), io::ErrorKind::Other);
    }
    #[test]
    fn native_null_dacl_is_rejected_on_the_actual_held_object() {
        let token = ProcessIdentity::capture().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("inert-data");
        std::fs::write(&path, b"inert").unwrap();
        let held = OpenOptions::new()
            .read(true)
            .access_mode(0x0002_0000 | 0x0004_0000 | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        // SAFETY: only this fresh inert fixture's held handle is changed.
        let status = unsafe {
            SetSecurityInfo(
                held.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, 0);
        let actual = winproc::file_security(&held).unwrap();
        assert!(actual.dacl.is_none());
        assert_eq!(
            coreacl::judge_current_user_data(&actual, &token.sid, false, false),
            Err(coreacl::UserDataAclError::Untrusted)
        );
    }
    #[test]
    fn native_token_owned_data_acl_is_separate_from_system_admin_store() {
        let token = ProcessIdentity::capture().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private-data");
        create_private_directory(&path, &token.sid).unwrap();
        let held = open(&path, true, true).unwrap();
        let sec = winproc::file_security(&held).unwrap();
        assert_eq!(sec.owner_sid.as_deref(), Some(token.sid.as_str()));
        assert!(coreacl::judge_current_user_data(&sec, &token.sid, true, false).is_ok());
        assert!(!coreacl::judge_object("user-data", &sec).is_empty());
        assert!(require_ntfs(&held).is_ok());
        assert!(final_path(&held).unwrap().is_absolute());
        assert!(std::fs::rename(&path, temp.path().join("moved")).is_err());
    }
    #[test]
    fn native_hardlink_and_reparse_are_not_regular_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("member");
        std::fs::write(&path, b"data-only").unwrap();
        let held = open(&path, false, false).unwrap();
        let id = identity(&held).unwrap();
        std::fs::hard_link(&path, temp.path().join("alias")).unwrap();
        assert!(open(&path, false, false).is_err());
        assert_eq!(identity(&held).unwrap(), id);
        let link = temp.path().join("symlink");
        match std::os::windows::fs::symlink_file(&path, &link) {
            Ok(()) => assert!(open(&link, false, false).is_err()),
            Err(e) => {
                assert_eq!(e.raw_os_error(), Some(1314));
                eprintln!("reparse fixture unsupported: privilege not held; no system change");
            }
        }
    }
    #[test]
    fn native_namespace_and_barrier_capability_is_reported_without_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("selected-data");
        match CurrentUserData::open(&path, true) {
            Ok(scope) => {
                scope.guard().unwrap();
                scope.check(scope.root()).unwrap();
                match scope.barrier() {
                    Ok(()) => eprintln!("native-data-scope and directory-barrier supported"),
                    Err(e) => {
                        assert!(matches!(
                            e.kind(),
                            io::ErrorKind::Unsupported
                                | io::ErrorKind::PermissionDenied
                                | io::ErrorKind::Other
                        ));
                        eprintln!("native-data-scope barrier unavailable: {e}");
                    }
                }
                assert!(
                    std::fs::rename(temp.path(), temp.path().with_extension("swapped")).is_err()
                );
            }
            Err(e) => {
                assert!(matches!(
                    e.kind(),
                    io::ErrorKind::PermissionDenied
                        | io::ErrorKind::Unsupported
                        | io::ErrorKind::Other
                ));
                eprintln!("native-data-scope unavailable under this exact temporary ancestry: {e}");
            }
        }
    }
}

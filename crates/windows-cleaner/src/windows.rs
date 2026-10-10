//! All Windows handles have one owner. Target paths are derived from the OS;
//! descendants are opened relative to retained parents, never by joined paths.
use crate::{Ace, Outcome, IMAGE_NAME, SERVICE, SUPPORT_LEAF, SYSTEM};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FileDispositionInformationEx, NtCreateFile, NtSetInformationFile, FILE_CREATE,
    FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION_EX, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Com::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(test)]
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::ProcessStatus::K32GetMappedFileNameW;
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::*;
#[cfg(test)]
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

fn error(what: &str) -> io::Error {
    io::Error::other(what)
}
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn handle(file: &File) -> HANDLE {
    file.as_raw_handle().cast()
}
fn bool_ok(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
struct LocalMem(HLOCAL);
impl Drop for LocalMem {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn sid_string(sid: PSID) -> io::Result<String> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(error("invalid SID"));
    }
    let mut text = null_mut();
    bool_ok(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let _text = LocalMem(text.cast());
    let mut len = 0;
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(text, len)
    }))
}

fn token_info() -> io::Result<(String, bool)> {
    let mut raw = null_mut();
    bool_ok(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) })?;
    let token = OwnedHandle(raw);
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0u64; (bytes as usize).div_ceil(8)];
    bool_ok(unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    })?;
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let sid = sid_string(user.User.Sid)?;
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    bool_ok(unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )
    })?;
    Ok((sid, elevation.TokenIsElevated != 0))
}

fn security(file: &File, caller: Option<&str>, ancestor: bool) -> io::Result<()> {
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut descriptor = null_mut();
    let code = unsafe {
        GetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let _descriptor = LocalMem(descriptor.cast());
    let owner = sid_string(owner)?;
    if acl.is_null() {
        return Err(error("null DACL"));
    }
    let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    bool_ok(unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    })?;
    let mut aces = Vec::new();
    for index in 0..info.AceCount {
        let mut ptr = null_mut();
        bool_ok(unsafe { GetAce(acl, index, &mut ptr) })?;
        let header = unsafe { &*ptr.cast::<ACE_HEADER>() };
        if header.AceFlags & 8 != 0 {
            continue;
        }
        if !matches!(header.AceType, 0 | 1) || usize::from(header.AceSize) < 12 {
            return Err(error("unsupported ACE"));
        }
        let ace = unsafe { &*ptr.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = sid_string((&ace.SidStart as *const u32).cast_mut().cast())?;
        aces.push(Ace {
            kind: header.AceType,
            flags: header.AceFlags,
            mask: ace.Mask,
            sid,
        });
    }
    if crate::trusted_security(&owner, Some(&aces), caller, ancestor) {
        Ok(())
    } else {
        Err(error("untrusted owner/DACL"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    volume: u32,
    high: u32,
    low: u32,
}
fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = unsafe { zeroed() };
    bool_ok(unsafe { GetFileInformationByHandle(handle(file), &mut info) })?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(error("reparse point"));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && info.nNumberOfLinks != 1 {
        return Err(error("hard-linked file"));
    }
    Ok(info)
}
fn identity(file: &File) -> io::Result<Identity> {
    let i = info(file)?;
    Ok(Identity {
        volume: i.dwVolumeSerialNumber,
        high: i.nFileIndexHigh,
        low: i.nFileIndexLow,
    })
}

fn child(
    parent: &File,
    name: &OsStr,
    delete: bool,
    create_dir: bool,
    descriptor: PSECURITY_DESCRIPTOR,
) -> io::Result<File> {
    let units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty()
        || units == [46]
        || units == [46, 46]
        || units.iter().any(|u| [0, 47, 92, 58].contains(u))
    {
        return Err(error("not a single component"));
    }
    let length = u16::try_from(units.len() * 2).map_err(|_| error("name too long"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: units.as_ptr().cast_mut(),
    };
    let attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: handle(parent),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | 0x1000, // OBJ_DONT_REPARSE
        SecurityDescriptor: descriptor.cast(),
        SecurityQualityOfService: null(),
    };
    let mut raw = null_mut();
    let mut ios: IO_STATUS_BLOCK = unsafe { zeroed() };
    let desired = READ_CONTROL
        | FILE_READ_ATTRIBUTES
        | FILE_LIST_DIRECTORY
        | SYNCHRONIZE
        | if delete { DELETE } else { 0 };
    // FILE_SHARE_READ alone rejects existing writable/deletable handles as well
    // as future replacements. No FILE_SHARE_DELETE, including for directories.
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            desired,
            &attrs,
            &mut ios,
            null(),
            0,
            FILE_SHARE_READ,
            if create_dir { FILE_CREATE } else { FILE_OPEN },
            FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if create_dir { FILE_DIRECTORY_FILE } else { 0 },
            null(),
            0,
        )
    };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    let file = unsafe { File::from_raw_handle(raw.cast()) };
    info(&file)?;
    Ok(file)
}

struct Chain {
    dirs: Vec<File>,
}
impl Chain {
    fn open(path: &Path, caller: Option<&str>) -> io::Result<Self> {
        let mut components = path.components();
        let disk = match components.next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
                _ => return Err(error("only local disk paths are supported")),
            },
            _ => return Err(error("not an absolute disk path")),
        };
        if components.next() != Some(Component::RootDir) {
            return Err(error("missing disk root"));
        }
        let root = wide(OsStr::new(&format!("\\\\?\\{}:\\", char::from(disk))));
        let raw = unsafe {
            CreateFileW(
                root.as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
                FILE_SHARE_READ,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let root = unsafe { File::from_raw_handle(raw.cast()) };
        info(&root)?;
        security(&root, caller, true)?;
        let mut dirs = vec![root];
        for component in components {
            let Component::Normal(name) = component else {
                return Err(error("unexpected component"));
            };
            let next = child(dirs.last().unwrap(), name, false, false, null_mut())?;
            if info(&next)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(error("ancestor is not directory"));
            }
            security(&next, caller, true)?;
            dirs.push(next);
        }
        Ok(Self { dirs })
    }
    fn last(&self) -> &File {
        self.dirs.last().unwrap()
    }
}

fn fixed_program_data() -> PathBuf {
    // The existing installer and helper source contract use this exact root.
    // A redirected OS KnownFolder must not hide the actually installed tree.
    // No environment variable, registry path or caller-selected target enters IO.
    PathBuf::from(crate::SUPPORT_PARENT)
}

fn entries(file: &File) -> io::Result<Vec<OsString>> {
    let mut buffer = vec![0u64; 8192];
    let mut restart = true;
    let mut names = Vec::new();
    loop {
        let class = if restart {
            FileFullDirectoryRestartInfo
        } else {
            FileFullDirectoryInfo
        };
        restart = false;
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle(file),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        };
        if ok == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(names);
            }
            return Err(e);
        }
        let bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), buffer.len() * 8) };
        let mut at = 0usize;
        loop {
            let len_at = at
                .checked_add(std::mem::offset_of!(FILE_FULL_DIR_INFO, FileNameLength))
                .ok_or_else(|| error("directory overflow"))?;
            let name_at = at
                .checked_add(std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName))
                .ok_or_else(|| error("directory overflow"))?;
            let word = |offset: usize| -> io::Result<u32> {
                Ok(u32::from_le_bytes(
                    bytes
                        .get(offset..offset + 4)
                        .ok_or_else(|| error("malformed directory record"))?
                        .try_into()
                        .unwrap(),
                ))
            };
            let len = word(len_at)? as usize;
            if !len.is_multiple_of(2) || len == 0 {
                return Err(error("malformed name"));
            }
            let name = bytes
                .get(
                    name_at
                        ..name_at
                            .checked_add(len)
                            .ok_or_else(|| error("directory overflow"))?,
                )
                .ok_or_else(|| error("truncated name"))?;
            let units: Vec<u16> = name
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if units != [46] && units != [46, 46] {
                if names.len() >= 4096 {
                    return Err(error("too many entries"));
                }
                names.push(OsString::from_wide(&units));
            }
            let next = word(at)? as usize;
            if next == 0 {
                break;
            }
            if next < name_at + len - at || !next.is_multiple_of(8) {
                return Err(error("invalid next record"));
            }
            at = at
                .checked_add(next)
                .ok_or_else(|| error("directory overflow"))?;
        }
    }
}

struct Object {
    file: File,
    identity: Identity,
}
fn plan(file: File, depth: usize, all: &mut Vec<Object>) -> io::Result<()> {
    if depth >= 64 || all.len() >= 4096 {
        return Err(error("cleanup tree limit"));
    }
    security(&file, None, false)?;
    let i = identity(&file)?;
    let is_dir = info(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let children = if is_dir { entries(&file)? } else { Vec::new() };
    let index = all.len();
    all.push(Object { file, identity: i });
    for name in children {
        let file = child(&all[index].file, &name, true, false, null_mut())?;
        plan(file, depth + 1, all)?;
    }
    Ok(())
}

fn delete_same(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFORMATION_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_FORCE_IMAGE_SECTION_CHECK
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    // Preserve deletion-stage NTSTATUS: SetFileInformationByHandle collapses
    // STATUS_CANNOT_DELETE (mapped image) and true access denial into error 5.
    // This handle was already opened/verified; no pathname fallback is used.
    let status = unsafe {
        NtSetInformationFile(
            handle(file),
            &mut iosb,
            (&disposition as *const FILE_DISPOSITION_INFORMATION_EX).cast(),
            size_of::<FILE_DISPOSITION_INFORMATION_EX>() as u32,
            FileDispositionInformationEx,
        )
    };
    if status < 0 {
        Err(crate::service::deletion_failure(status, unsafe {
            RtlNtStatusToDosError(status)
        }))
    } else {
        Ok(())
    }
}

fn cleanup_tree(parent: &File) -> io::Result<()> {
    let root = match child(parent, OsStr::new(SUPPORT_LEAF), true, false, null_mut()) {
        Ok(root) => root,
        Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Ok(()),
        Err(e) => return Err(e),
    };
    if info(&root)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(error("support target is not a directory"));
    }
    let mut objects = Vec::new();
    plan(root, 0, &mut objects)?;
    // Pin and validate the full tree before deleting those same objects,
    // bottom-up. A definite temporary sharing/image error releases this entire
    // plan; the caller must reopen and revalidate from the fixed trusted root.
    while let Some(object) = objects.pop() {
        if identity(&object.file)? != object.identity {
            return Err(error("support object identity changed"));
        }
        security(&object.file, None, false)?;
        delete_same(&object.file)?;
    }
    Ok(())
}

fn bounded_pause(deadline: Instant) -> bool {
    if Instant::now() >= deadline {
        return false;
    }
    std::thread::sleep(Duration::from_millis(100));
    Instant::now() < deadline
}

struct FixedService {
    manager: ScHandle,
    service: Option<ScHandle>,
    name: Vec<u16>,
    deadline: Instant,
    deleting: bool,
}
impl crate::service::Ops for FixedService {
    fn state(&mut self) -> io::Result<crate::service::State> {
        use crate::service::State;
        if self.deleting {
            let raw =
                unsafe { OpenServiceW(self.manager.0, self.name.as_ptr(), SERVICE_QUERY_STATUS) };
            if !raw.is_null() {
                drop(ScHandle(raw));
                return Ok(State::DeletePending);
            }
            let e = io::Error::last_os_error();
            return match e.raw_os_error().map(|n| n as u32) {
                Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(State::Absent),
                Some(ERROR_SERVICE_MARKED_FOR_DELETE) => Ok(State::DeletePending),
                _ => Err(e),
            };
        }
        if self.service.is_none() {
            let raw = unsafe {
                OpenServiceW(
                    self.manager.0,
                    self.name.as_ptr(),
                    SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE,
                )
            };
            if raw.is_null() {
                let e = io::Error::last_os_error();
                return match e.raw_os_error().map(|n| n as u32) {
                    Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(State::Absent),
                    Some(ERROR_SERVICE_MARKED_FOR_DELETE) => {
                        self.deleting = true;
                        Ok(State::DeletePending)
                    }
                    _ => Err(e),
                };
            }
            self.service = Some(ScHandle(raw));
        }
        let mut status: SERVICE_STATUS_PROCESS = unsafe { zeroed() };
        let mut bytes = 0;
        if unsafe {
            QueryServiceStatusEx(
                self.service.as_ref().unwrap().0,
                SC_STATUS_PROCESS_INFO,
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
                size_of::<SERVICE_STATUS_PROCESS>() as u32,
                &mut bytes,
            )
        } == 0
        {
            let e = io::Error::last_os_error();
            return match e.raw_os_error().map(|n| n as u32) {
                Some(ERROR_SERVICE_MARKED_FOR_DELETE) => {
                    self.service.take();
                    self.deleting = true;
                    Ok(State::DeletePending)
                }
                Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
                    self.service.take();
                    Ok(State::Absent)
                }
                _ => Err(e),
            };
        }
        Ok(match status.dwCurrentState {
            SERVICE_STOPPED => State::Stopped,
            SERVICE_START_PENDING => State::Starting,
            SERVICE_STOP_PENDING => State::Stopping,
            _ => State::Running,
        })
    }
    fn stop(&mut self) -> io::Result<crate::service::Stop> {
        use crate::service::Stop;
        let mut status = unsafe { zeroed() };
        if unsafe {
            ControlService(
                self.service.as_ref().unwrap().0,
                SERVICE_CONTROL_STOP,
                &mut status,
            )
        } != 0
        {
            return Ok(Stop::Requested);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error().map(|n| n as u32) {
            Some(ERROR_SERVICE_NOT_ACTIVE) => Ok(Stop::NotActive),
            Some(ERROR_SERVICE_CANNOT_ACCEPT_CTRL) => Ok(Stop::CannotAccept),
            _ => Err(e),
        }
    }
    fn delete(&mut self) -> io::Result<()> {
        if unsafe { DeleteService(self.service.as_ref().unwrap().0) } == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_SERVICE_MARKED_FOR_DELETE as i32) {
                return Err(e);
            }
        }
        self.deleting = true;
        self.service.take(); // SCM removal cannot complete while our handle remains open.
        Ok(())
    }
    fn pause(&mut self) -> bool {
        bounded_pause(self.deadline)
    }
}
fn remove_service() -> io::Result<()> {
    let raw = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut service = FixedService {
        manager: ScHandle(raw),
        service: None,
        name: wide(OsStr::new(SERVICE)),
        deadline: Instant::now() + Duration::from_secs(30),
        deleting: false,
    };
    crate::service::remove(&mut service).map_err(|failure| match failure {
        crate::service::Failure::Operation(e) => e,
        crate::service::Failure::Timeout { deleting } => error(if deleting {
            "service delete still pending"
        } else {
            "service stop timeout"
        }),
    })
}

pub fn run_worker() -> Outcome {
    if !token_info().is_ok_and(|(sid, elevated)| elevated || sid == SYSTEM) {
        return Outcome::Refused;
    }
    // Refuse an untrusted fixed ancestor before mutating SCM.
    match Chain::open(&fixed_program_data(), None) {
        Ok(data) => drop(data),
        Err(e) => {
            eprintln!("fixed ancestor refused: {e}");
            return Outcome::Refused;
        }
    }
    if let Err(e) = remove_service() {
        eprintln!("service cleanup partial: {e}");
        return Outcome::Partial;
    }
    // STOPPED and service removal can precede actual helper process exit.
    // Never trust a stopped service PID or kill it. Only definite temporary
    // sharing/image errors may retry a fresh root/identity/ACL/full-tree plan.
    let deadline = Instant::now() + Duration::from_secs(30);
    match crate::service::retry_cleanup(
        || Chain::open(&fixed_program_data(), None).and_then(|data| cleanup_tree(data.last())),
        || bounded_pause(deadline),
    ) {
        Ok(()) => Outcome::Success,
        Err(e) => {
            eprintln!("support cleanup partial: {e}");
            Outcome::Partial
        }
    }
}

fn final_path(file: &File, flags: u32) -> io::Result<OsString> {
    let mut buf = vec![0u16; 32768];
    let len = unsafe {
        GetFinalPathNameByHandleW(handle(file), buf.as_mut_ptr(), buf.len() as u32, flags)
    };
    if len == 0 || len as usize >= buf.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(OsString::from_wide(&buf[..len as usize]))
}

struct Image {
    chain: Chain,
    file: File,
    identity: Identity,
    path: OsString,
}
impl Image {
    fn open(path: &Path, caller: Option<&str>) -> io::Result<Self> {
        if path.file_name() != Some(OsStr::new(IMAGE_NAME)) {
            return Err(error("wrong cleaner artifact"));
        }
        let chain = Chain::open(
            path.parent().ok_or_else(|| error("no image parent"))?,
            caller,
        )?;
        let file = child(
            chain.last(),
            OsStr::new(IMAGE_NAME),
            false,
            false,
            null_mut(),
        )?;
        if info(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(error("image is directory"));
        }
        security(&file, caller, false)?;
        let identity = identity(&file)?;
        let path = final_path(&file, VOLUME_NAME_DOS)?;
        Ok(Self {
            chain,
            file,
            identity,
            path,
        })
    }
    fn current() -> io::Result<Self> {
        let (sid, _) = token_info()?;
        let image = Self::open(&std::env::current_exe()?, Some(&sid))?;
        // Resolve the executable's loaded mapping, not merely a check of its
        // pathname. Compare it with the pinned object's NT volume path while
        // retaining every directory and the no-write/no-delete image handle.
        let mut mapped = vec![0u16; 32768];
        let module = unsafe { GetModuleHandleW(null()) };
        let len = unsafe {
            K32GetMappedFileNameW(
                GetCurrentProcess(),
                module.cast(),
                mapped.as_mut_ptr(),
                mapped.len() as u32,
            )
        };
        if len == 0 || len as usize >= mapped.len() {
            return Err(io::Error::last_os_error());
        }
        let mapped = OsString::from_wide(&mapped[..len as usize]);
        if mapped != final_path(&image.file, VOLUME_NAME_NT)? {
            return Err(error("loaded image differs from pinned image"));
        }
        Ok(image)
    }
    fn check(&self) -> io::Result<()> {
        let _ = &self.chain;
        if identity(&self.file)? == self.identity {
            Ok(())
        } else {
            Err(error("image identity changed"))
        }
    }
}

fn elevate(image: Image) -> Outcome {
    if image.check().is_err() {
        return Outcome::Refused;
    }
    let verb = wide(OsStr::new("runas"));
    let image_path = wide(&image.path);
    let args = wide(OsStr::new("--worker"));
    // Each call owns its COM apartment on this thread. Changed-mode errors
    // fail closed instead of silently borrowing a GUI thread's apartment.
    let hr = unsafe {
        CoInitializeEx(
            null(),
            COINIT_APARTMENTTHREADED as u32 | COINIT_DISABLE_OLE1DDE as u32,
        )
    };
    if hr < 0 {
        return Outcome::LaunchFailed;
    }
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _com = Com;
    let mut execute: SHELLEXECUTEINFOW = unsafe { zeroed() };
    execute.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    execute.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    execute.lpVerb = verb.as_ptr();
    execute.lpFile = image_path.as_ptr();
    execute.lpParameters = args.as_ptr();
    execute.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut execute) } == 0 {
        return if io::Error::last_os_error().raw_os_error() == Some(ERROR_CANCELLED as i32) {
            Outcome::Cancelled
        } else {
            Outcome::LaunchFailed
        };
    }
    if execute.hProcess.is_null() {
        return Outcome::LaunchFailed;
    }
    let process = OwnedHandle(execute.hProcess);
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Outcome::LaunchFailed;
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Outcome::LaunchFailed;
    }
    let outcome = Outcome::from_code(code);
    drop(process);
    drop(image); // Actual image and ancestors stay locked through exit.
    outcome
}

pub fn launch_current() -> Outcome {
    match Image::current() {
        Ok(image) => elevate(image),
        Err(e) => {
            eprintln!("cleaner custody refused: {e}");
            Outcome::Refused
        }
    }
}

pub fn launch_bundle() -> Outcome {
    let result = (|| {
        let (sid, _) = token_info()?;
        let exe = std::env::current_exe()?;
        let parent = exe.parent().ok_or_else(|| error("no app parent"))?;
        // Same fixed resource layout as the Windows package, no PATH lookup.
        let installed = parent
            .join("_up_")
            .join("resources")
            .join("win")
            .join(IMAGE_NAME);
        // The portable zip has the second fixed layout. Fall back only when
        // the installed artifact is absent, never after a custody refusal.
        match Image::open(&installed, Some(&sid)) {
            Err(e)
                if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32)
                    || e.raw_os_error() == Some(ERROR_PATH_NOT_FOUND as i32) =>
            {
                Image::open(
                    &parent.join("resources").join("win").join(IMAGE_NAME),
                    Some(&sid),
                )
            }
            result => result,
        }
    })();
    match result {
        Ok(image) => elevate(image),
        Err(e) => {
            eprintln!("bundled cleaner refused: {e}");
            Outcome::Refused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    fn parent(path: &Path) -> File {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .unwrap()
    }
    #[test]
    fn relative_open_rejects_traversal_ads_and_separators() {
        let temp = tempfile::tempdir().unwrap();
        let dir = parent(temp.path());
        for name in [
            "..",
            ".",
            "nested\\file",
            "nested/file",
            "file:stream",
            "",
            "a\0b",
        ] {
            assert!(
                child(&dir, OsStr::new(name), true, false, null_mut()).is_err(),
                "{name:?}"
            );
        }
    }
    #[test]
    fn pinned_object_blocks_replacement_and_writes_until_close() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("payload");
        std::fs::write(&path, b"fixture").unwrap();
        let dir = parent(temp.path());
        let pinned = child(&dir, OsStr::new("payload"), false, false, null_mut()).unwrap();
        let id = identity(&pinned).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::rename(&path, temp.path().join("replacement")).is_err());
        assert_eq!(identity(&pinned).unwrap(), id);
        drop(pinned);
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_ok());
    }
    #[test]
    fn hard_link_refused_without_mutating_either_name() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::write(&first, b"sentinel").unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        assert!(child(
            &parent(temp.path()),
            OsStr::new("first"),
            true,
            false,
            null_mut()
        )
        .is_err());
        assert_eq!(std::fs::read(&second).unwrap(), b"sentinel");
    }
    #[test]
    fn directory_enumeration_and_deletion_use_opened_object() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("sentinel"), b"keep").unwrap();
        std::fs::write(temp.path().join("delete-me"), b"delete").unwrap();
        let dir = parent(temp.path());
        let names = entries(&dir).unwrap();
        assert!(names.contains(&OsString::from("sentinel")));
        assert!(names.contains(&OsString::from("delete-me")));
        let target = child(&dir, OsStr::new("delete-me"), true, false, null_mut()).unwrap();
        delete_same(&target).unwrap();
        drop(target);
        assert!(!temp.path().join("delete-me").exists());
        assert_eq!(
            std::fs::read(temp.path().join("sentinel")).unwrap(),
            b"keep"
        );
    }
    #[test]
    fn delayed_exit_file_hold_retries_relative_open_after_release() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("held-file");
        std::fs::write(&path, b"fixture").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        // Proxy for a helper that announced STOPPED while its process/file
        // handle remains alive. Disposable files only; no SCM or Polaris IO.
        let first_error = child(
            &parent(temp.path()),
            OsStr::new("held-file"),
            true,
            false,
            null_mut(),
        )
        .unwrap_err();
        assert!(crate::service::temporary_file_error(&first_error));
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut attempts = 1; // The held-object attempt above must fail before release.
        crate::service::retry_cleanup(
            || {
                attempts += 1;
                let dir = parent(temp.path());
                let target = child(&dir, OsStr::new("held-file"), true, false, null_mut())?;
                delete_same(&target)
            },
            || bounded_pause(deadline),
        )
        .unwrap();
        release.join().unwrap();
        assert!(attempts > 1);
        assert!(!path.exists());
    }
    #[test]
    fn persistent_file_hold_times_out_without_deleting_object() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("held-file");
        std::fs::write(&path, b"fixture").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);
        let error = crate::service::retry_cleanup(
            || {
                let dir = parent(temp.path());
                let target = child(&dir, OsStr::new("held-file"), true, false, null_mut())?;
                delete_same(&target)
            },
            || bounded_pause(deadline),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        drop(held);
        assert_eq!(std::fs::read(path).unwrap(), b"fixture");
    }
    struct ImageSection {
        _mapping: OwnedHandle,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
    }
    impl ImageSection {
        fn open(path: &Path) -> io::Result<Self> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(path)?;
            let raw = unsafe {
                CreateFileMappingW(
                    handle(&file),
                    null(),
                    PAGE_READONLY | SEC_IMAGE,
                    0,
                    0,
                    null(),
                )
            };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mapping = OwnedHandle(raw);
            let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 0) };
            if view.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            // Close the original file: only a real SEC_IMAGE section/view holds
            // the copied PE. No ordinary sharing lock substitutes for this case.
            drop(file);
            Ok(Self {
                _mapping: mapping,
                view,
            })
        }
    }
    impl Drop for ImageSection {
        fn drop(&mut self) {
            assert_ne!(unsafe { UnmapViewOfFile(self.view) }, 0);
        }
    }
    fn image_fixture() -> (tempfile::TempDir, PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let (sid, _) = token_info().unwrap();
        (temp, path, sid)
    }
    fn delete_image_fixture(path: &Path, sid: &str) -> io::Result<()> {
        // Explicit fixture-only caller SID; production support trees always
        // require SYSTEM/Admin ownership. Reopen/revalidate the ancestor chain
        // and this single-object plan on every attempt, just as production does.
        let chain = Chain::open(path.parent().unwrap(), Some(sid))?;
        let target = child(
            chain.last(),
            OsStr::new("image.exe"),
            true,
            false,
            null_mut(),
        )?;
        let id = identity(&target)?;
        security(&target, Some(sid), false)?;
        if identity(&target)? != id {
            return Err(error("fixture identity changed"));
        }
        delete_same(&target)
    }
    #[test]
    fn sec_image_delayed_release_retries_native_status_after_full_revalidation() {
        let (temp, path, sid) = image_fixture();
        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let mapped_path = &path;
            let worker = scope.spawn(move || {
                let mapping = ImageSection::open(mapped_path).unwrap();
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                std::thread::sleep(Duration::from_millis(150));
                drop(mapping);
            });
            ready_rx.recv().unwrap();
            let first = delete_image_fixture(&path, &sid).unwrap_err();
            assert_eq!(
                first
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<crate::service::DeleteStatus>()
                    .unwrap()
                    .status,
                STATUS_CANNOT_DELETE
            );
            assert!(crate::service::temporary_file_error(&first));
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            crate::service::retry_cleanup(
                || delete_image_fixture(&path, &sid),
                || bounded_pause(deadline),
            )
            .unwrap();
            worker.join().unwrap();
        });
        assert!(!path.exists());
        drop(temp);
    }
    #[test]
    fn sec_image_persistent_mapping_times_out_without_unlinking_file() {
        let (_temp, path, sid) = image_fixture();
        let mapping = ImageSection::open(&path).unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);
        let e = crate::service::retry_cleanup(
            || delete_image_fixture(&path, &sid),
            || bounded_pause(deadline),
        )
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(path.exists());
        drop(mapping);
    }
    #[test]
    fn native_delete_without_delete_right_is_access_denied_and_never_retried() {
        let (_temp, path, _sid) = image_fixture();
        let mut attempts = 0;
        let e = crate::service::retry_cleanup(
            || {
                attempts += 1;
                // Actual kernel access-check rejection, not a synthetic error 5.
                let file = std::fs::File::open(&path)?;
                delete_same(&file)
            },
            || panic!("real permission denial must not wait"),
        )
        .unwrap_err();
        let status = e
            .get_ref()
            .unwrap()
            .downcast_ref::<crate::service::DeleteStatus>()
            .unwrap();
        assert_eq!(status.status, STATUS_ACCESS_DENIED);
        assert_eq!(attempts, 1);
        assert!(path.exists());
    }
    #[test]
    fn junction_is_refused_and_external_sentinel_is_intact() {
        // A directory junction needs no developer mode or elevation. Build
        // its reparse buffer only in disposable fixture directories.
        let temp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("sentinel"), b"keep").unwrap();
        let junction = temp.path().join("junction");
        std::fs::create_dir(&junction).unwrap();
        let mount = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&junction)
            .unwrap();
        let substitute = format!(r"\??\{}", external.path().display())
            .encode_utf16()
            .collect::<Vec<_>>();
        let print = external
            .path()
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let path_bytes = (substitute.len() + print.len() + 2) * 2;
        let mut data = Vec::new();
        data.extend_from_slice(&0xa0000003u32.to_le_bytes());
        data.extend_from_slice(&((8 + path_bytes) as u16).to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        for word in [
            0,
            (substitute.len() * 2) as u16,
            ((substitute.len() + 1) * 2) as u16,
            (print.len() * 2) as u16,
        ] {
            data.extend_from_slice(&word.to_le_bytes());
        }
        for word in substitute
            .into_iter()
            .chain(Some(0))
            .chain(print)
            .chain(Some(0))
        {
            data.extend_from_slice(&word.to_le_bytes());
        }
        let mut returned = 0;
        bool_ok(unsafe {
            DeviceIoControl(
                handle(&mount),
                0x000900a4,
                data.as_ptr().cast(),
                data.len() as u32,
                null_mut(),
                0,
                &mut returned,
                null_mut(),
            )
        })
        .unwrap();
        drop(mount);
        assert!(child(
            &parent(temp.path()),
            OsStr::new("junction"),
            true,
            false,
            null_mut()
        )
        .is_err());
        assert_eq!(
            std::fs::read(external.path().join("sentinel")).unwrap(),
            b"keep"
        );
        std::fs::remove_dir(junction).unwrap();
    }
}

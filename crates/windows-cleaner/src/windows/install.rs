//! Windows installation IO. Each unsafe call below is confined to a native
//! object operation; all output buffers, pointer ranges and HANDLE lifetimes
//! are checked here. No script interpreter or caller-selected command exists.
use super::*;
use crate::install::{
    self as transaction, Ops, Policy, Request, Selection, SourceIdentity, State, Transaction,
};
use crate::InstallOutcome;
use polaris_helper_proto::core_manifest::InstalledCoreManifest;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

// windows-sys Services constants mirror SCMR service object access rights.
const EXISTING_SERVICE_ACCESS: u32 =
    SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_STOP | SERVICE_START | READ_CONTROL;
const FRESH_SERVICE_ACCESS: u32 =
    EXISTING_SERVICE_ACCESS | SERVICE_CHANGE_CONFIG | DELETE | WRITE_DAC | ACCESS_SYSTEM_SECURITY;
const DACL: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;RP;;;IU)";
const FRESH_SD: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;RP;;;IU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)";
const PRIVATE_SD: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const PUBLIC_SD: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)";
const SUPPORT: &str = r"C:\ProgramData\Polaris";
const BINARY: &str = r"C:\ProgramData\Polaris\polaris-helper.exe";
const BIN_PATH: &str =
    r#""C:\ProgramData\Polaris\polaris-helper.exe" --support "C:\ProgramData\Polaris""#;

fn entry_present(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
struct CreatedFile {
    path: PathBuf,
    armed: bool,
}
impl Drop for CreatedFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// This enables only an already assigned privilege in this worker's own token,
// solely for the original fresh-service SACL. It cannot grant an absent right.
struct AuditPrivilege {
    token: OwnedHandle,
    previous: TOKEN_PRIVILEGES,
    active: bool,
}
impl AuditPrivilege {
    fn enable() -> io::Result<Self> {
        let mut token = null_mut();
        // SAFETY: current-process token, exact query/adjust rights, owned output.
        bool_ok(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut token,
            )
        })?;
        let token = OwnedHandle(token);
        let mut luid = unsafe { zeroed() };
        // SAFETY: constant privilege name, live LUID output; no caller choice.
        bool_ok(unsafe {
            LookupPrivilegeValueW(
                null(),
                wide(OsStr::new("SeSecurityPrivilege")).as_ptr(),
                &mut luid,
            )
        })?;
        let desired = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let mut previous: TOKEN_PRIVILEGES = unsafe { zeroed() };
        let mut bytes = 0;
        // SAFETY: one-entry fixed-size structures, saved state restores exactly
        // the attributes this call changed. ERROR_NOT_ALL_ASSIGNED is failure.
        unsafe {
            SetLastError(0);
        }
        bool_ok(unsafe {
            AdjustTokenPrivileges(
                token.0,
                0,
                &desired,
                size_of::<TOKEN_PRIVILEGES>() as u32,
                &mut previous,
                &mut bytes,
            )
        })?;
        let code = unsafe { GetLastError() };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        Ok(Self {
            token,
            previous,
            active: true,
        })
    }
    fn close(&mut self) -> io::Result<()> {
        if self.active && self.previous.PrivilegeCount != 0 {
            // SAFETY: exact saved token state; token remains exclusively owned.
            unsafe {
                SetLastError(0);
            }
            bool_ok(unsafe {
                AdjustTokenPrivileges(self.token.0, 0, &self.previous, 0, null_mut(), null_mut())
            })?;
            let code = unsafe { GetLastError() };
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        self.active = false;
        Ok(())
    }
}
impl Drop for AuditPrivilege {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct Descriptor(LocalMem);
impl Descriptor {
    fn parse(text: &str) -> io::Result<Self> {
        let mut raw = null_mut();
        // SAFETY: NUL-terminated UTF16 input, owned LocalAlloc output.
        bool_ok(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(OsStr::new(text)).as_ptr(),
                1,
                &mut raw,
                null_mut(),
            )
        })?;
        Ok(Self(LocalMem(raw.cast())))
    }
    fn ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0 .0.cast()
    }
    fn text(&self, sections: u32) -> io::Result<String> {
        let mut out = null_mut();
        // SAFETY: live validated descriptor and owned native string output.
        bool_ok(unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                self.ptr(),
                1,
                sections,
                &mut out,
                null_mut(),
            )
        })?;
        let _out = LocalMem(out.cast());
        let mut n = 0;
        // Native API owns a terminated SD string. Bound traversal defensively.
        while n < 65536 && unsafe { *out.add(n) } != 0 {
            n += 1;
        }
        if n == 65536 {
            return Err(error("security text too large"));
        }
        Ok(
            String::from_utf16(unsafe { std::slice::from_raw_parts(out, n) })
                .map_err(|_| error("invalid security text"))?,
        )
    }
}
// SAFETY: descriptor owns validated native memory; requested sections select
// only pointers returned from that descriptor. File HANDLE outlives the call.
fn set_security(file: &File, descriptor: &Descriptor, sections: u32) -> io::Result<()> {
    let mut owner = null_mut();
    let mut group = null_mut();
    let mut dacl = null_mut();
    let mut defaulted = 0;
    let mut present = 0;
    // SAFETY: descriptor and output pointers live through these synchronous reads.
    unsafe {
        if sections & OWNER_SECURITY_INFORMATION != 0 {
            bool_ok(GetSecurityDescriptorOwner(
                descriptor.ptr(),
                &mut owner,
                &mut defaulted,
            ))?;
        }
        if sections & GROUP_SECURITY_INFORMATION != 0 {
            bool_ok(GetSecurityDescriptorGroup(
                descriptor.ptr(),
                &mut group,
                &mut defaulted,
            ))?;
        }
        bool_ok(GetSecurityDescriptorDacl(
            descriptor.ptr(),
            &mut present,
            &mut dacl,
            &mut defaulted,
        ))?;
    }
    if present == 0 || dacl.is_null() {
        return Err(error("missing restore DACL"));
    }
    // SAFETY: retained file handle, pointers owned by descriptor; filesystem API.
    let code = unsafe {
        SetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            sections,
            owner,
            group,
            dacl,
            null_mut(),
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
// SAFETY: GetSecurityInfo allocates a self-relative descriptor owned by
// LocalMem; only owner/group/DACL are queried and HANDLE remains borrowed.
fn file_descriptor(file: &File) -> io::Result<Descriptor> {
    let mut sd = null_mut();
    // SAFETY: retained file and LocalAlloc result, never a path re-open.
    let code = unsafe {
        GetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let owned = LocalMem(sd.cast());
    // SAFETY: GetSecurityInfo allocated this descriptor; validate before decode.
    if unsafe { IsValidSecurityDescriptor(sd) } == 0 {
        return Err(error("invalid file descriptor"));
    }
    Ok(Descriptor(owned))
}
// SAFETY: native descriptor owns the returned SID until sid_string finishes.
fn file_owner(file: &File) -> io::Result<String> {
    let sd = file_descriptor(file)?;
    let mut owner = null_mut();
    let mut defaulted = 0;
    bool_ok(unsafe { GetSecurityDescriptorOwner(sd.ptr(), &mut owner, &mut defaulted) })?;
    sid_string(owner)
}
// SAFETY: descriptor/ACL owners remain live while GetAclInformation/GetAce
// enumerate, with explicit byte ranges before dereferencing variable layouts.
fn private_registration_parent(file: &File) -> io::Result<()> {
    security(file, None, false)?;
    let sd = file_descriptor(file)?;
    let mut acl = null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    bool_ok(unsafe {
        GetSecurityDescriptorDacl(sd.ptr(), &mut present, &mut acl, &mut defaulted)
    })?;
    if present == 0 || acl.is_null() || unsafe { IsValidAcl(acl) } == 0 {
        return Err(error("registration parent DACL missing"));
    }
    let mut size: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    bool_ok(unsafe {
        GetAclInformation(
            acl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    })?;
    for index in 0..size.AceCount {
        let mut ptr = null_mut();
        bool_ok(unsafe { GetAce(acl, index, &mut ptr) })?;
        // SAFETY: GetAce enumerates a validated ACL. Explicit ranges also bound
        // our header/SID reads; unsupported ACE layouts are rejected below.
        let begin = acl as usize;
        let bytes = unsafe { (*acl).AclSize } as usize;
        let at = ptr as usize;
        if at < begin
            || at
                .checked_add(size_of::<ACE_HEADER>())
                .is_none_or(|end| end > begin + bytes)
        {
            return Err(error("ACE header out of ACL range"));
        }
        let header = unsafe { &*ptr.cast::<ACE_HEADER>() };
        if at
            .checked_add(header.AceSize as usize)
            .is_none_or(|end| end > begin + bytes)
        {
            return Err(error("ACE out of ACL range"));
        }
        if header.AceFlags & 8 != 0 {
            continue;
        }
        if header.AceType == 0 {
            if header.AceSize < 12 {
                return Err(error("short registration ACE"));
            }
            let ace = unsafe { &*ptr.cast::<ACCESS_ALLOWED_ACE>() };
            let sid_ptr = (&ace.SidStart as *const u32).cast_mut().cast();
            // SAFETY: SID header is in this admitted ACE; its computed length
            // must fit before conversion can inspect any subauthorities.
            if unsafe { GetLengthSid(sid_ptr) } as usize > header.AceSize as usize - 8 {
                return Err(error("ACE SID out of range"));
            }
            let sid = sid_string(sid_ptr)?;
            if sid != SYSTEM && sid != crate::ADMINS {
                return Err(error("registration parent grants nonprivileged access"));
            }
        } else if header.AceType != 1 {
            return Err(error("unsupported registration parent ACE"));
        }
    }
    Ok(())
}
fn open_security(path: &Path, write_owner: bool) -> io::Result<File> {
    let name = wide(path.as_os_str());
    // SAFETY: exact absolute path; OPEN_REPARSE and share-read reject old write
    // handles before any ACL mutation. Returned HANDLE has exactly one owner.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | READ_CONTROL | WRITE_DAC | if write_owner { WRITE_OWNER } else { 0 },
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
    let file = unsafe { File::from_raw_handle(raw.cast()) };
    info(&file)?;
    Ok(file)
}

/// A mutable installation namespace still needs a continuous no-rename chain.
/// First reject old directory writers with the ordinary strict admission. Then
/// overlap same-object read handles that allow our own Win32 child publication;
/// neither chain ever shares DELETE. The final private namespace ACL prevents
/// untrusted new writers after admission. Source custody keeps the strict Chain.
fn publication_chain(path: &Path, caller: Option<&str>) -> io::Result<Chain> {
    let admitted = Chain::open(path, caller)?;
    security(admitted.last(), caller, false)?;
    let mut dirs = Vec::with_capacity(admitted.dirs.len());
    for (index, old) in admitted.dirs.iter().enumerate() {
        let name = wide(&final_path(old, VOLUME_NAME_DOS)?);
        // SAFETY: every old ancestor remains held without DELETE sharing while
        // reopening its exact final name. The new read handle is identity/ACL
        // checked before any old handle can close; no write right is requested.
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(raw.cast()) };
        if identity(&file)? != identity(old)? {
            return Err(error("publication ancestor identity changed"));
        }
        security(&file, caller, index + 1 != admitted.dirs.len())?;
        dirs.push(file);
    }
    // `admitted` drops only after the complete replacement chain owns custody.
    Ok(Chain { dirs })
}

fn apply_path_security(path: &Path, text: &str, owner: bool) -> io::Result<()> {
    let f = open_security(path, owner)?;
    set_security(
        &f,
        &Descriptor::parse(text)?,
        DACL_SECURITY_INFORMATION
            | PROTECTED_DACL_SECURITY_INFORMATION
            | if owner {
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION
            } else {
                0
            },
    )
}
fn object(file: &File) -> io::Result<[u32; 3]> {
    let i = identity(file)?;
    Ok([i.volume, i.high, i.low])
}
fn read_bounded(file: &File, limit: u64) -> io::Result<Vec<u8>> {
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(0))?;
    let mut b = Vec::new();
    f.take(limit + 1).read_to_end(&mut b)?;
    if b.len() as u64 > limit {
        return Err(error("file exceeds bound"));
    }
    Ok(b)
}
fn hash(file: &File) -> io::Result<String> {
    let len = file.metadata()?.len();
    if len == 0 || len > transaction::MAX_FILE {
        return Err(error("invalid installation file size"));
    }
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(0))?;
    let mut digest = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut count = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > len {
            return Err(error("installation file grew"));
        }
        digest.update(&buf[..n]);
    }
    if count != len {
        return Err(error("installation file changed size"));
    }
    Ok(hex::encode(digest.finalize()))
}
fn nonce() -> io::Result<String> {
    let mut b = [0u8; 16];
    // SAFETY: fixed writable output, system CSPRNG, no fallback.
    if unsafe {
        BCryptGenRandom(
            null_mut(),
            b.as_mut_ptr(),
            b.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(error("native entropy unavailable"));
    }
    Ok(hex::encode(b))
}
struct Source {
    chain: Chain,
    files: Vec<(String, File)>,
}
impl Source {
    fn open(request: &Request, seal_user_stage: bool) -> io::Result<Self> {
        request.validate()?;
        let path = Path::new(&request.stage);
        if seal_user_stage {
            let (sid, _) = token_info()?;
            if sid != request.original_sid {
                return Err(error("source user differs from native caller"));
            }
            // Give the selected administrator read access without allowing it
            // or another user to mutate the input. No persistent host ACL changes.
            let sd = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;GRGX;;;BA)");
            let _first = Chain::open(path, Some(&sid))?;
            apply_path_security(path, &sd, false)?;
            for name in [
                "core-manifest.json",
                "polaris-helper.exe",
                IMAGE_NAME,
                "sing-box.exe",
                "libcronet.dll",
            ] {
                let p = path.join(name);
                if entry_present(&p)? {
                    apply_path_security(&p, &sd, false)?;
                }
            }
        }
        let chain = Chain::open(path, Some(&request.original_sid))?;
        if file_owner(chain.last())? != request.original_sid {
            return Err(error("stage owner differs from original native user"));
        }
        let manifest_file = child(
            chain.last(),
            OsStr::new("core-manifest.json"),
            false,
            false,
            null_mut(),
        )?;
        security(&manifest_file, Some(&request.original_sid), false)?;
        if hash(&manifest_file)? != request.manifest_sha256 {
            return Err(error("manifest bytes differ from App pin"));
        }
        let manifest = InstalledCoreManifest::parse(&read_bounded(&manifest_file, 65536)?, |b| {
            hex::encode(sha2::Sha256::digest(b))
        })
        .map_err(error)?;
        if manifest.id != request.core_id
            || manifest.core.platform != "win"
            || manifest.core.helper_sha256.as_deref() != Some(request.helper_sha256.as_str())
            || manifest.core.cleaner_sha256.as_deref() != Some(request.cleaner_sha256.as_str())
        {
            return Err(error("installation selection differs"));
        }
        let mut files = vec![("core-manifest.json".into(), manifest_file)];
        for (name, expected, bytes) in manifest
            .core
            .files
            .iter()
            .map(|f| (f.name.as_str(), f.sha256.as_str(), Some(f.bytes)))
            .chain([
                ("polaris-helper.exe", request.helper_sha256.as_str(), None),
                (IMAGE_NAME, request.cleaner_sha256.as_str(), None),
            ])
        {
            let file = child(chain.last(), OsStr::new(name), false, false, null_mut())?;
            security(&file, Some(&request.original_sid), false)?;
            if hash(&file)? != expected
                || bytes.is_some_and(|n| file.metadata().map(|m| m.len() != n).unwrap_or(true))
            {
                return Err(error("staged bytes differ"));
            }
            files.push((name.into(), file));
        }
        let source = Self { chain, files };
        if !seal_user_stage
            && (source.ids()? != request.source_identities
                || object(source.chain.last())? != request.stage_identity)
        {
            return Err(error("source file identity differs across UAC"));
        }
        Ok(source)
    }
    fn ids(&self) -> io::Result<Vec<SourceIdentity>> {
        self.files
            .iter()
            .map(|(n, f)| {
                Ok(SourceIdentity {
                    name: n.clone(),
                    object: object(f)?,
                    bytes: f.metadata()?.len(),
                })
            })
            .collect()
    }
    fn file(&self, name: &str) -> io::Result<&File> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
            .ok_or_else(|| error("missing staged member"))
    }
}
fn create_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    Ok(())
}

// Caller-private request, never a product target. Nonrecursive cleanup cannot
// follow a substituted link into another tree. Unknown worker completion leaks
// this owner together with the held objects deliberately until process exit.
struct CreatedRequest {
    parent: PathBuf,
    file: bool,
    armed: bool,
}
impl CreatedRequest {
    fn cleanup(&mut self) -> io::Result<()> {
        if self.file {
            std::fs::remove_file(self.parent.join("install-request.json"))?;
            self.file = false;
        }
        std::fs::remove_dir(&self.parent)?;
        self.armed = false;
        Ok(())
    }
}
impl Drop for CreatedRequest {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.cleanup();
        }
    }
}

/// Only explicit install calls enter this function; normal startup never hashes
/// the maintenance PE or creates another version/capability process.
pub fn launch_install(mut request: Request) -> io::Result<InstallOutcome> {
    let source = Source::open(&request, true)?;
    request.source_identities = source.ids()?;
    request.stage_identity = object(source.chain.last())?;
    let image = Image::open(
        &Path::new(&request.stage).join(IMAGE_NAME),
        Some(&request.original_sid),
    )?;
    if hash(&image.file)? != request.cleaner_sha256 {
        return Err(error("maintenance image differs from compiled cleaner pin"));
    }
    let request_parent = Path::new(&request.stage)
        .parent()
        .ok_or_else(|| error("stage parent missing"))?
        .join(format!("helper-request.{}", nonce()?));
    std::fs::create_dir(&request_parent)?;
    let mut created = CreatedRequest {
        parent: request_parent.clone(),
        file: false,
        armed: true,
    };
    let sd = format!(
        "D:P(A;OICI;FA;;;{})(A;OICI;FA;;;SY)(A;OICI;GRGX;;;BA)",
        request.original_sid
    );
    apply_path_security(&request_parent, &sd, false)?;
    let request_path = request_parent.join("install-request.json");
    let bytes = serde_json::to_vec(&request).map_err(|_| error("request serialization failed"))?;
    if bytes.len() > transaction::MAX_REQUEST {
        return Err(error("request too large"));
    }
    created.file = true;
    create_private_file(&request_path, &bytes)?;
    let request_chain = Chain::open(&request_parent, Some(&request.original_sid))?;
    let request_file = child(
        request_chain.last(),
        OsStr::new("install-request.json"),
        false,
        false,
        null_mut(),
    )?;
    security(&request_file, Some(&request.original_sid), false)?;
    image.check()?;
    let hr = unsafe {
        CoInitializeEx(
            null(),
            COINIT_APARTMENTTHREADED as u32 | COINIT_DISABLE_OLE1DDE as u32,
        )
    };
    if hr < 0 {
        return Err(error("native installation COM initialization failed"));
    }
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }
    let _com = Com;
    // No secret, executable selector or interpolated code enters argv. The one
    // path is created here and cannot contain a quote or end in a separator.
    let verb = wide(OsStr::new("runas"));
    let executable = wide(&image.path);
    let args = wide(OsStr::new(&format!(
        "--install-worker --request \"{}\"",
        request_path
            .to_str()
            .ok_or_else(|| error("request path is not Unicode"))?
    )));
    let mut execute: SHELLEXECUTEINFOW = unsafe { zeroed() };
    execute.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    execute.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    execute.lpVerb = verb.as_ptr();
    execute.lpFile = executable.as_ptr();
    execute.lpParameters = args.as_ptr();
    execute.nShow = SW_HIDE;
    let result = if unsafe { ShellExecuteExW(&mut execute) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            Ok(InstallOutcome::Cancelled)
        } else {
            Err(e)
        }
    } else if execute.hProcess.is_null() {
        // Completion is unknowable. Keep all source/request/image custody, so
        // App stage Drop cannot remove live input. Never kill an unknown child.
        std::mem::forget((source, image, request_chain, request_file, created));
        return Ok(InstallOutcome::Completed(87));
    } else {
        let process = OwnedHandle(execute.hProcess);
        if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
            std::mem::forget((process, source, image, request_chain, request_file, created));
            return Ok(InstallOutcome::Completed(87));
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
            Ok(InstallOutcome::Completed(87))
        } else {
            Ok(InstallOutcome::Completed(i32::try_from(code).unwrap_or(23)))
        }
    };
    drop(request_file);
    drop(request_chain);
    drop(image);
    drop(source);
    // This is caller-private data created for this operation, never a product
    // target. Missing/changed/locked cleanup is a failure, not silent success.
    if created.cleanup().is_err() {
        return Ok(InstallOutcome::Completed(
            if matches!(result, Ok(InstallOutcome::Completed(87))) {
                87
            } else {
                88
            },
        ));
    }
    result
}

struct Buffer {
    words: Vec<u64>,
    len: usize,
}
impl Buffer {
    fn new(size: u32) -> io::Result<Self> {
        if size == 0 || size > 65536 {
            return Err(error("native query size unavailable"));
        }
        Ok(Self {
            words: vec![0; (size as usize).div_ceil(8)],
            len: size as usize,
        })
    }
    fn ptr(&self) -> *const u8 {
        self.words.as_ptr().cast()
    }
    fn mut_ptr(&mut self) -> *mut u8 {
        self.words.as_mut_ptr().cast()
    }
    fn contains(&self, p: *const u8, n: usize) -> bool {
        let start = self.ptr() as usize;
        let at = p as usize;
        at >= start && at.checked_add(n).is_some_and(|end| end <= start + self.len)
    }
    fn text(&self, p: *const u16) -> io::Result<String> {
        if p.is_null() {
            return Ok(String::new());
        }
        let mut units = Vec::new();
        let mut at = p;
        loop {
            if !self.contains(at.cast(), 2) {
                return Err(error("SCM string out of range"));
            }
            let u = unsafe { std::ptr::read_unaligned(at) };
            if u == 0 {
                break;
            }
            units.push(u);
            at = unsafe { at.add(1) };
        }
        String::from_utf16(&units).map_err(|_| error("invalid SCM Unicode"))
    }
    fn value<T: Copy>(&self) -> io::Result<T> {
        if size_of::<T>() > self.len {
            return Err(error("short SCM query"));
        }
        Ok(unsafe { std::ptr::read_unaligned(self.ptr().cast::<T>()) })
    }
}
// SAFETY: same retained service handle; sizing error/upper cap are mandatory,
// aligned buffers own all returned pointer-bearing Config/Config2 structures.
fn config_buffer(service: SC_HANDLE, level: Option<u32>) -> io::Result<Buffer> {
    let mut needed = 0;
    let ok = unsafe {
        match level {
            None => QueryServiceConfigW(service, null_mut(), 0, &mut needed),
            Some(n) => QueryServiceConfig2W(service, n, null_mut(), 0, &mut needed),
        }
    };
    if ok != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(error("SCM query size failed"));
    }
    let mut b = Buffer::new(needed)?;
    let mut after = 0;
    bool_ok(unsafe {
        match level {
            None => QueryServiceConfigW(service, b.mut_ptr().cast(), b.len as u32, &mut after),
            Some(n) => QueryServiceConfig2W(service, n, b.mut_ptr(), b.len as u32, &mut after),
        }
    })?;
    if after as usize > b.len {
        return Err(error("SCM query grew"));
    }
    Ok(b)
}
// SAFETY: SCM writes a bounded aligned buffer; the returned self-relative SD
// stays live through every descriptor/ACE conversion and is never LocalFree'd.
fn query_service_security(service: SC_HANDLE, sections: u32) -> io::Result<Buffer> {
    let mut n = 0;
    let ok = unsafe { QueryServiceObjectSecurity(service, sections, null_mut(), 0, &mut n) };
    if ok != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(error("SCM SD size failed"));
    }
    let mut b = Buffer::new(n)?;
    let mut after = 0;
    bool_ok(unsafe {
        QueryServiceObjectSecurity(
            service,
            sections,
            b.mut_ptr().cast(),
            b.len as u32,
            &mut after,
        )
    })?;
    if after as usize > b.len
        || unsafe { IsValidSecurityDescriptor(b.ptr().cast_mut().cast()) } == 0
    {
        return Err(error("invalid SCM descriptor"));
    }
    Ok(b)
}
// SAFETY: validated self-relative SD buffer lives through SID/string decode;
// only the separately allocated string is LocalFree-owned.
fn service_security(service: SC_HANDLE) -> io::Result<(String, String, String)> {
    let b = query_service_security(
        service,
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
    )?;
    let sd = b.ptr().cast_mut().cast();
    let mut owner = null_mut();
    let mut group = null_mut();
    let mut defaulted = 0;
    bool_ok(unsafe { GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted) })?;
    bool_ok(unsafe { GetSecurityDescriptorGroup(sd, &mut group, &mut defaulted) })?;
    let mut text = null_mut();
    bool_ok(unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            null_mut(),
        )
    })?;
    let _text = LocalMem(text.cast());
    let mut len = 0;
    while len < 65536 && unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    if len == 65536 {
        return Err(error("SCM SD text too large"));
    }
    Ok((
        sid_string(owner)?,
        sid_string(group)?,
        String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) })
            .map_err(|_| error("SCM SD Unicode"))?,
    ))
}
fn fresh_sacl(service: SC_HANDLE) -> io::Result<()> {
    let b = query_service_security(service, SACL_SECURITY_INFORMATION)?;
    let mut text = null_mut();
    // SAFETY: valid self-relative SCM SD remains held; API allocates the string.
    bool_ok(unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            b.ptr().cast_mut().cast(),
            1,
            SACL_SECURITY_INFORMATION,
            &mut text,
            null_mut(),
        )
    })?;
    let _text = LocalMem(text.cast());
    let mut n = 0;
    // SAFETY: successful conversion returns a NUL-terminated allocated string.
    while n < 65536 && unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    if n == 65536 {
        return Err(error("fresh SACL text too large"));
    }
    let actual = String::from_utf16(unsafe { std::slice::from_raw_parts(text, n) })
        .map_err(|_| error("fresh SACL Unicode"))?;
    if actual != Descriptor::parse(FRESH_SD)?.text(SACL_SECURITY_INFORMATION)? {
        return Err(error("fresh SACL differs"));
    }
    Ok(())
}
// SAFETY: Config values are copied unaligned; strings/action arrays remain
// inside their live buffers and action alignment/count are checked first.
fn read_policy(service: SC_HANDLE) -> io::Result<Policy> {
    let b = config_buffer(service, None)?;
    let c: QUERY_SERVICE_CONFIGW = b.value()?;
    let (owner, group, dacl) = service_security(service)?;
    let description = config_buffer(service, Some(SERVICE_CONFIG_DESCRIPTION))?;
    let desc: SERVICE_DESCRIPTIONW = description.value()?;
    let f = config_buffer(service, Some(SERVICE_CONFIG_FAILURE_ACTIONS))?;
    let failure: SERVICE_FAILURE_ACTIONSW = f.value()?;
    if failure.cActions != 3
        || (failure.lpsaActions as usize) % std::mem::align_of::<SC_ACTION>() != 0
        || !f.contains(failure.lpsaActions.cast(), 3 * size_of::<SC_ACTION>())
    {
        return Err(error("unsupported SCM recovery"));
    }
    let actions = unsafe { std::slice::from_raw_parts(failure.lpsaActions, 3) };
    let word = |level| -> io::Result<u32> { config_buffer(service, Some(level))?.value::<u32>() };
    let privileges = config_buffer(service, Some(SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO))?;
    let p: SERVICE_REQUIRED_PRIVILEGES_INFOW = privileges.value()?;
    Ok(Policy {
        account: b.text(c.lpServiceStartName)?,
        command: b.text(c.lpBinaryPathName)?,
        display: b.text(c.lpDisplayName)?,
        description: description.text(desc.lpDescription)?,
        owner,
        group,
        dacl,
        service_type: c.dwServiceType,
        start_type: c.dwStartType,
        error_control: c.dwErrorControl,
        tag: c.dwTagId,
        load_group: b.text(c.lpLoadOrderGroup)?,
        empty_dependencies: b.text(c.lpDependencies)?.is_empty(),
        recovery_reset: failure.dwResetPeriod,
        recovery_command: f.text(failure.lpCommand)?,
        reboot_message: f.text(failure.lpRebootMsg)?,
        action_types: actions.iter().map(|a| a.Type as u32).collect(),
        action_delays: actions.iter().map(|a| a.Delay).collect(),
        non_crash: word(SERVICE_CONFIG_FAILURE_ACTIONS_FLAG)? != 0,
        sid_type: word(SERVICE_CONFIG_SERVICE_SID_INFO)?,
        launch_protected: word(SERVICE_CONFIG_LAUNCH_PROTECTED)?,
        empty_privileges: privileges.text(p.pmszRequiredPrivileges)?.is_empty(),
    })
}
// SAFETY: owned SC_HANDLE and fixed-size, fully initialized output storage;
// QueryServiceStatusEx must return the complete PROCESS structure.
fn status(service: SC_HANDLE) -> io::Result<u32> {
    let mut s: SERVICE_STATUS_PROCESS = unsafe { zeroed() };
    let mut n = 0;
    bool_ok(unsafe {
        QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            (&mut s as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut n,
        )
    })?;
    if n as usize != size_of::<SERVICE_STATUS_PROCESS>() {
        return Err(error("short SCM status"));
    }
    Ok(s.dwCurrentState)
}
fn wait_state(service: SC_HANDLE, wanted: u32) -> io::Result<()> {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if status(service)? == wanted {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(error("SCM state timeout"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn stop_service(service: SC_HANDLE) -> io::Result<()> {
    if status(service)? != SERVICE_STOPPED {
        let mut s = unsafe { zeroed() };
        bool_ok(unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut s) })?;
        wait_state(service, SERVICE_STOPPED)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    path: PathBuf,
    backup: PathBuf,
    present: bool,
    hash: Option<String>,
    sddl: Option<String>,
}
struct Installer {
    request: Request,
    source: Source,
    manager: ScHandle,
    service: Option<ScHandle>,
    old: Option<State>,
    admitted_policy: Option<Policy>,
    utility: Option<PathBuf>,
    files: FileTransaction,
    expected_dacl: String,
    unknown_child: std::cell::Cell<bool>,
    audit: Option<AuditPrivilege>,
}
impl Installer {
    // SAFETY: fixed local SCM, minimum connect/create rights; ScHandle owns
    // successful output exactly once, with no caller-selected service object.
    fn open(request: Request, source: Source) -> io::Result<Self> {
        let raw = unsafe {
            OpenSCManagerW(
                null(),
                null(),
                SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let files = FileTransaction::production(&request.original_sid);
        Ok(Self {
            request,
            source,
            manager: ScHandle(raw),
            service: None,
            old: None,
            admitted_policy: None,
            utility: None,
            files,
            expected_dacl: Descriptor::parse(DACL)?.text(DACL_SECURITY_INFORMATION)?,
            unknown_child: std::cell::Cell::new(false),
            audit: None,
        })
    }
    fn svc(&self) -> io::Result<SC_HANDLE> {
        self.service
            .as_ref()
            .map(|s| s.0)
            .ok_or_else(|| error("SCM object absent"))
    }
    fn tx(&self) -> io::Result<&Path> {
        self.files.tx()
    }
    fn utility(&self, args: &[&OsStr]) -> io::Result<(String, i32)> {
        let exe = self
            .utility
            .as_ref()
            .ok_or_else(|| error("sealed utility absent"))?;
        let chain = Chain::open(exe.parent().unwrap(), None)?;
        let file = child(
            chain.last(),
            OsStr::new("polaris-helper.exe"),
            false,
            false,
            null_mut(),
        )?;
        security(&file, None, false)?;
        if hash(&file)? != self.request.helper_sha256 {
            return Err(error("sealed helper bytes differ"));
        }
        // SAFETY/ownership: Command's Child owns the process HANDLE. No shell,
        // CREATE_NO_WINDOW for this console utility; both bounded pipe readers
        // join and the real process is waited even on a reader error.
        let child = Command::new(exe)
            .args(args)
            .env_clear()
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Guard is installed immediately after birth and drops before the
        // earlier declared file/chain, including an unwind during reader setup.
        let mut owned = UtilityChild::new(child, &self.unknown_child);
        let (exit, bytes) = owned.capture(&mut NativeReaderThreads)?;
        let code = exit
            .code()
            .ok_or_else(|| error("utility exit unavailable"))?;
        let output = String::from_utf8(bytes)
            .map_err(|_| error("utility output Unicode"))?
            .trim()
            .to_owned();
        Ok((output, code))
    }
    fn checked(&self, args: &[&OsStr]) -> io::Result<String> {
        let (output, code) = self.utility(args)?;
        if code != 0 {
            return Err(error("native helper operation failed"));
        }
        Ok(output)
    }
    fn fixed_probe(&self, verb: &str) -> io::Result<String> {
        self.checked(&[
            OsStr::new(verb),
            OsStr::new("--support"),
            OsStr::new(SUPPORT),
        ])
    }
    fn assert_service(&self) -> io::Result<()> {
        if let Some(service) = &self.service {
            let policy = read_policy(service.0)?;
            if self
                .admitted_policy
                .as_ref()
                .is_some_and(|admitted| admitted != &policy)
                || !policy.supported(BIN_PATH, &self.expected_dacl)
            {
                return Err(error("unsupported existing SCM policy; preserved"));
            }
            // An independent read from the same retained SCM object replaces
            // CIM. A racing config change is Unknown, never normalization.
            if read_policy(service.0)? != policy {
                return Err(error("SCM observations disagree"));
            }
        } else {
            let raw = unsafe {
                OpenServiceW(
                    self.manager.0,
                    wide(OsStr::new(SERVICE)).as_ptr(),
                    SERVICE_QUERY_CONFIG,
                )
            };
            if !raw.is_null() {
                drop(ScHandle(raw));
                return Err(error("SCM object appeared outside installation"));
            }
            if io::Error::last_os_error().raw_os_error()
                != Some(ERROR_SERVICE_DOES_NOT_EXIST as i32)
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
    fn registration_path(&self) -> PathBuf {
        self.files.registration_path.clone()
    }
}

type ReaderJob = Box<dyn FnOnce() -> io::Result<Vec<u8>> + Send>;
type ReaderHandle = std::thread::JoinHandle<io::Result<Vec<u8>>>;
trait ReaderThreads {
    fn spawn(&mut self, name: &'static str, job: ReaderJob) -> io::Result<ReaderHandle>;
}
struct NativeReaderThreads;
impl ReaderThreads for NativeReaderThreads {
    fn spawn(&mut self, name: &'static str, job: ReaderJob) -> io::Result<ReaderHandle> {
        std::thread::Builder::new().name(name.into()).spawn(job)
    }
}
fn read_utility_pipe(mut pipe: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut overflow = false;
    loop {
        let n = pipe.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n <= 65536 {
            bytes.extend_from_slice(&chunk[..n]);
        } else {
            overflow = true;
        }
    }
    // Continue draining after the cap: memory remains bounded, but a full
    // inherited pipe must never prevent waiting for the actual child.
    if overflow {
        return Err(error("utility output too large"));
    }
    Ok(bytes)
}
struct UtilityChild<'a> {
    child: std::process::Child,
    uncertain: &'a std::cell::Cell<bool>,
    readers: [Option<ReaderHandle>; 2],
    terminal: bool,
    joined: usize,
}
impl<'a> UtilityChild<'a> {
    fn new(child: std::process::Child, uncertain: &'a std::cell::Cell<bool>) -> Self {
        Self {
            child,
            uncertain,
            readers: [None, None],
            terminal: false,
            joined: 0,
        }
    }
    fn await_exit(&mut self) -> std::process::ExitStatus {
        // Never substitute a PID observation/termination acknowledgement for
        // the owned Child's actual terminal wait. Keep custody on wait errors.
        loop {
            match self.child.wait() {
                Ok(exit) => {
                    self.terminal = true;
                    return exit;
                }
                Err(_) => {
                    self.uncertain.set(true);
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
    fn join_readers(&mut self) -> [io::Result<Vec<u8>>; 2] {
        let mut results = [Ok(Vec::new()), Ok(Vec::new())];
        for (index, slot) in self.readers.iter_mut().enumerate() {
            if let Some(reader) = slot.take() {
                results[index] = reader
                    .join()
                    .unwrap_or_else(|_| Err(error("utility reader failed")));
                self.joined += 1;
            }
        }
        results
    }
    fn stop_and_collect(&mut self) {
        self.uncertain.set(true);
        // Only this just-created owned utility may be stopped. Pipe startup
        // failure can leave a pipe undrained, so stop before waiting/joining.
        // kill failure is not exit proof: await_exit still owns the lifetime.
        let _ = self.child.kill();
        self.await_exit();
        let _ = self.join_readers();
    }
    fn capture(
        &mut self,
        threads: &mut impl ReaderThreads,
    ) -> io::Result<(std::process::ExitStatus, Vec<u8>)> {
        let setup = (|| -> io::Result<()> {
            let stdout = self
                .child
                .stdout
                .take()
                .ok_or_else(|| error("utility stdout absent"))?;
            self.readers[0] = Some(threads.spawn(
                "polaris-utility-out",
                Box::new(move || read_utility_pipe(stdout)),
            )?);
            let stderr = self
                .child
                .stderr
                .take()
                .ok_or_else(|| error("utility stderr absent"))?;
            self.readers[1] = Some(threads.spawn(
                "polaris-utility-err",
                Box::new(move || read_utility_pipe(stderr)),
            )?);
            Ok(())
        })();
        if let Err(cause) = setup {
            // Close any unclaimed pipe before stop/wait. Created readers still
            // drain and are joined; none is detached on either startup failure.
            self.child.stdout.take();
            self.child.stderr.take();
            self.stop_and_collect();
            return Err(cause);
        }
        let exit = self.await_exit();
        let [output, errors] = self.join_readers();
        if self.uncertain.get() {
            return Err(error("owned utility wait uncertain; recovery required"));
        }
        let output = output?;
        let _ = errors?;
        Ok((exit, output))
    }
}
impl Drop for UtilityChild<'_> {
    fn drop(&mut self) {
        if !self.terminal {
            self.child.stdout.take();
            self.child.stderr.take();
            self.stop_and_collect();
        } else {
            let _ = self.join_readers();
        }
    }
}

/// Snapshot, restore, retirement and lock custody shared by the real installer
/// and Windows inert-SCM tests. No fixture reimplements filesystem recovery.
struct FileTransaction {
    support_path: PathBuf,
    registration_path: PathBuf,
    caller: Option<String>,
    private_sd: String,
    support: Option<Chain>,
    tx: Option<PathBuf>,
    lock: Option<File>,
    snapshots: Vec<Snapshot>,
    registration: Option<Snapshot>,
}
impl FileTransaction {
    fn production(original_sid: &str) -> Self {
        Self {
            support_path: SUPPORT.into(),
            registration_path: Path::new(SUPPORT)
                .join("users")
                .join(format!("s{original_sid}"))
                .join("registration"),
            caller: None,
            private_sd: PRIVATE_SD.into(),
            support: None,
            tx: None,
            lock: None,
            snapshots: vec![],
            registration: None,
        }
    }
    fn caller(&self) -> Option<&str> {
        self.caller.as_deref()
    }
    fn tx(&self) -> io::Result<&Path> {
        self.tx
            .as_deref()
            .ok_or_else(|| error("transaction absent"))
    }
    fn registration_path(&self) -> PathBuf {
        self.registration_path.clone()
    }
    fn secure_dir(&self, path: &Path, public: bool) -> io::Result<()> {
        let _parent = Chain::open(
            path.parent()
                .ok_or_else(|| error("managed parent missing"))?,
            self.caller(),
        )?;
        if !entry_present(&path)? {
            std::fs::create_dir(path)?;
            apply_path_security(
                path,
                if public && self.caller().is_none() {
                    PUBLIC_SD
                } else {
                    &self.private_sd
                },
                self.caller().is_none(),
            )?;
        } else {
            let chain = Chain::open(path, self.caller())?;
            drop(chain);
            apply_path_security(
                path,
                if public && self.caller().is_none() {
                    PUBLIC_SD
                } else {
                    &self.private_sd
                },
                self.caller().is_none(),
            )?;
        }
        let _confirmed = Chain::open(path, self.caller())?;
        Ok(())
    }
    fn copy_to(&self, input: &File, path: &Path, expected: Option<&str>) -> io::Result<()> {
        let mut source = input.try_clone()?;
        source.seek(SeekFrom::Start(0))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(path)?;
        let size = input.metadata()?.len();
        if size > transaction::MAX_FILE {
            return Err(error("copy too large"));
        }
        if std::io::copy(&mut source.take(size + 1), &mut output)? != size {
            return Err(error("copy size differs"));
        }
        output.sync_all()?;
        drop(output);
        apply_path_security(path, &self.private_sd, self.caller().is_none())?;
        let parent = Chain::open(path.parent().unwrap(), self.caller())?;
        let f = child(
            parent.last(),
            path.file_name().unwrap(),
            false,
            false,
            null_mut(),
        )?;
        if expected.is_some_and(|expected| hash(&f).map(|h| h != expected).unwrap_or(true)) {
            return Err(error("copied bytes differ"));
        }
        Ok(())
    }
    fn snapshot_file(
        &self,
        path: PathBuf,
        backup: PathBuf,
        registration: bool,
    ) -> io::Result<Snapshot> {
        let present = entry_present(&path)?;
        let mut s = Snapshot {
            path,
            backup,
            present,
            hash: None,
            sddl: None,
        };
        if present {
            let chain = Chain::open(s.path.parent().unwrap(), self.caller())?;
            let file = child(
                chain.last(),
                s.path.file_name().unwrap(),
                false,
                false,
                null_mut(),
            )?;
            security(&file, self.caller(), false)?;
            s.hash = Some(hash(&file)?);
            if registration {
                s.sddl = Some(file_descriptor(&file)?.text(
                    OWNER_SECURITY_INFORMATION
                        | GROUP_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION,
                )?);
            }
            self.copy_to(&file, &s.backup, s.hash.as_deref())?;
            if hash(&file)? != *s.hash.as_ref().unwrap()
                || registration
                    && file_descriptor(&file)?.text(
                        OWNER_SECURITY_INFORMATION
                            | GROUP_SECURITY_INFORMATION
                            | DACL_SECURITY_INFORMATION,
                    )? != *s.sddl.as_ref().unwrap()
            {
                return Err(error("registration changed during snapshot"));
            }
        }
        Ok(s)
    }
    fn restore_snapshot(&self, s: &Snapshot, registration: bool) -> io::Result<()> {
        restore_snapshot_with_policy(s, registration, self.caller(), &self.private_sd)
    }
    fn prepare(&mut self) -> io::Result<()> {
        self.secure_dir(&self.support_path, true)?;
        let support = publication_chain(&self.support_path, self.caller())?;
        let lock = self.support_path.join("helper-install.lock");
        if entry_present(&lock)? {
            let f = child(
                support.last(),
                OsStr::new("helper-install.lock"),
                false,
                false,
                null_mut(),
            )?;
            security(&f, self.caller(), false)?;
        }
        self.lock = Some(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
                .share_mode(0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&lock)?,
        );
        info(self.lock.as_ref().unwrap())?;
        set_security(
            self.lock.as_ref().unwrap(),
            &Descriptor::parse(&self.private_sd)?,
            DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION
                | if self.caller().is_none() {
                    OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION
                } else {
                    0
                },
        )?;
        self.support = Some(support);
        let tx = self
            .support_path
            .join(format!("helper-upgrade.{}", nonce()?));
        self.secure_dir(&tx, false)?;
        self.tx = Some(tx.clone());
        Ok(())
    }
    fn snapshot(&mut self, replace: bool) -> io::Result<()> {
        if replace {
            for name in ["polaris-helper.exe", IMAGE_NAME, "helper-installed.json"] {
                self.snapshots.push(self.snapshot_file(
                    self.support_path.join(name),
                    self.tx()?.join(format!("{name}.rollback")),
                    false,
                )?);
            }
        }
        let registration = self.snapshot_file(
            self.registration_path(),
            self.tx()?.join("registration.rollback"),
            true,
        )?;
        create_private_file(
            &self.tx()?.join("registration.snapshot.json"),
            &serde_json::to_vec(&registration).map_err(|_| error("snapshot serialization"))?,
        )?;
        apply_path_security(
            &self.tx()?.join("registration.snapshot.json"),
            &self.private_sd,
            self.caller().is_none(),
        )?;
        self.registration = Some(registration);
        Ok(())
    }
    fn replace_payload(&mut self, helper_hash: &str, cleaner_hash: &str) -> io::Result<()> {
        for (name, expected) in [
            ("polaris-helper.exe", helper_hash),
            (IMAGE_NAME, cleaner_hash),
        ] {
            let source = self.tx()?.join(name);
            let chain = Chain::open(source.parent().unwrap(), self.caller())?;
            let file = child(chain.last(), OsStr::new(name), false, false, null_mut())?;
            let target = self.support_path.join(name);
            let mut last = None;
            for attempt in 0..10 {
                let copy = (|| -> io::Result<()> {
                    if entry_present(&target)? {
                        let c = Chain::open(&self.support_path, self.caller())?;
                        let existing = child(c.last(), OsStr::new(name), false, false, null_mut())?;
                        security(&existing, self.caller(), false)?;
                        drop(existing);
                    }
                    let mut input = file.try_clone()?;
                    input.seek(SeekFrom::Start(0))?;
                    let n = file.metadata()?.len();
                    let mut out = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .share_mode(0)
                        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                        .open(&target)?;
                    info(&out)?;
                    // OPEN_REPARSE + same-handle link/type admission precedes
                    // the first destructive write, including truncation.
                    out.set_len(0)?;
                    if std::io::copy(&mut input.take(n + 1), &mut out)? != n {
                        return Err(error("replacement size differs"));
                    }
                    out.sync_all()?;
                    drop(out);
                    apply_path_security(&target, &self.private_sd, self.caller().is_none())?;
                    let c = Chain::open(&self.support_path, self.caller())?;
                    let f = child(c.last(), OsStr::new(name), false, false, null_mut())?;
                    if hash(&f)? != expected {
                        return Err(error("replacement hash differs"));
                    }
                    Ok(())
                })();
                match copy {
                    Ok(()) => {
                        last = None;
                        break;
                    }
                    Err(e) => {
                        last = Some(e);
                        if attempt < 9 {
                            std::thread::sleep(Duration::from_millis(300));
                        }
                    }
                }
            }
            if let Some(e) = last {
                return Err(e);
            }
        }
        Ok(())
    }
    fn restore_payload(&mut self) -> io::Result<()> {
        for s in &self.snapshots {
            self.restore_snapshot(s, false)?;
        }
        Ok(())
    }
    fn restore_registration(&mut self) -> io::Result<()> {
        self.restore_snapshot(
            self.registration
                .as_ref()
                .ok_or_else(|| error("registration snapshot missing"))?,
            true,
        )
    }
    fn finish(&mut self, keep: bool) -> io::Result<()> {
        let result = (|| -> io::Result<()> {
            if !keep {
                if let Some(tx) = &self.tx {
                    let chain = Chain::open(tx, self.caller())?;
                    let mut objects = Vec::new();
                    for name in entries(chain.last())? {
                        plan_with_caller(
                            child(chain.last(), &name, true, false, null_mut())?,
                            0,
                            &mut objects,
                            self.caller(),
                        )?;
                    }
                    while let Some(object) = objects.pop() {
                        if identity(&object.file)? != object.identity {
                            return Err(error("transaction cleanup object changed"));
                        }
                        security(&object.file, self.caller(), false)?;
                        delete_same(&object.file)?;
                    }
                    drop(chain);
                    let support = self
                        .support
                        .as_ref()
                        .ok_or_else(|| error("support custody missing"))?;
                    let file = child(
                        support.last(),
                        tx.file_name().unwrap(),
                        true,
                        false,
                        null_mut(),
                    )?;
                    delete_same(&file)
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        })();
        self.lock.take();
        result
    }
}

fn restore_snapshot_with_policy(
    s: &Snapshot,
    registration: bool,
    caller: Option<&str>,
    temporary_sd: &str,
) -> io::Result<()> {
    let parent = publication_chain(
        s.path
            .parent()
            .ok_or_else(|| error("restore parent absent"))?,
        caller,
    )?;
    if !s.present {
        match child(
            parent.last(),
            s.path.file_name().unwrap(),
            true,
            false,
            null_mut(),
        ) {
            Ok(f) => delete_same(&f)?,
            Err(e) if matches!(e.raw_os_error(), Some(2 | 3)) => {}
            Err(e) => return Err(e),
        }
        if entry_present(&s.path)? {
            return Err(error("registration absence not restored"));
        }
        return Ok(());
    }
    let backup_parent = publication_chain(s.backup.parent().unwrap(), caller)?;
    let backup = child(
        backup_parent.last(),
        s.backup.file_name().unwrap(),
        false,
        false,
        null_mut(),
    )?;
    if hash(&backup)? != *s.hash.as_ref().ok_or_else(|| error("backup hash absent"))? {
        return Err(error("backup hash differs"));
    }
    let temporary = s
        .path
        .parent()
        .unwrap()
        .join(format!("registration.restore.{}", nonce()?));
    let mut input = backup.try_clone()?;
    input.seek(SeekFrom::Start(0))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)?;
    let mut temporary_owner = CreatedFile {
        path: temporary.clone(),
        armed: true,
    };
    let n = backup.metadata()?.len();
    if std::io::copy(&mut input.take(n + 1), &mut output)? != n {
        return Err(error("restore copy size changed"));
    }
    output.sync_all()?;
    drop(output);
    // Only the private temporary is sealed, never the final registration.
    apply_path_security(&temporary, temporary_sd, caller.is_none())?;
    let f = child(
        parent.last(),
        temporary.file_name().unwrap(),
        false,
        false,
        null_mut(),
    )?;
    if hash(&f)? != *s.hash.as_ref().unwrap() {
        return Err(error("temporary hash differs"));
    }
    drop(f);
    if entry_present(&s.path)? {
        let existing = child(
            parent.last(),
            s.path.file_name().unwrap(),
            false,
            false,
            null_mut(),
        )?;
        drop(existing);
        // SAFETY: parents remain held, both names derived here, null backup is
        // a real native NULL. No ignore-merge or unsupported write-through flag.
        bool_ok(unsafe {
            ReplaceFileW(
                wide(s.path.as_os_str()).as_ptr(),
                wide(temporary.as_os_str()).as_ptr(),
                null(),
                0,
                null(),
                null(),
            )
        })?;
    } else {
        bool_ok(unsafe {
            MoveFileExW(
                wide(temporary.as_os_str()).as_ptr(),
                wide(s.path.as_os_str()).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        })?;
    }
    temporary_owner.armed = false; // Native publication consumed the private name.
    if registration {
        let descriptor = Descriptor::parse(
            s.sddl
                .as_deref()
                .ok_or_else(|| error("registration security absent"))?,
        )?;
        let mut control = 0;
        let mut revision = 0;
        bool_ok(unsafe {
            GetSecurityDescriptorControl(descriptor.ptr(), &mut control, &mut revision)
        })?;
        let f = open_security(&s.path, true)?;
        set_security(
            &f,
            &descriptor,
            OWNER_SECURITY_INFORMATION
                | GROUP_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | if control & SE_DACL_PROTECTED != 0 {
                    PROTECTED_DACL_SECURITY_INFORMATION
                } else {
                    UNPROTECTED_DACL_SECURITY_INFORMATION
                },
        )?;
        drop(f);
    } else {
        apply_path_security(&s.path, temporary_sd, caller.is_none())?;
    }
    let restored = child(
        parent.last(),
        s.path.file_name().unwrap(),
        false,
        false,
        null_mut(),
    )?;
    if hash(&restored)? != *s.hash.as_ref().unwrap()
        || registration
            && file_descriptor(&restored)?.text(
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            )? != *s.sddl.as_ref().unwrap()
    {
        return Err(error("registration bytes or security not restored"));
    }
    Ok(())
}

impl Ops for Installer {
    fn uncertain(&self) -> bool {
        self.unknown_child.get()
    }
    // SAFETY: constant service name; retain actual handle and original policy
    // before managed mutation. Absent is only ERROR_SERVICE_DOES_NOT_EXIST.
    fn inspect(&mut self) -> io::Result<Option<State>> {
        let raw = unsafe {
            OpenServiceW(
                self.manager.0,
                wide(OsStr::new(SERVICE)).as_ptr(),
                EXISTING_SERVICE_ACCESS,
            )
        };
        if raw.is_null() {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
                self.old = None;
                return Ok(None);
            }
            return Err(e);
        }
        self.service = Some(ScHandle(raw));
        self.admitted_policy = Some(read_policy(raw)?);
        self.assert_service()?;
        let state = match status(raw)? {
            SERVICE_RUNNING => State::Running,
            SERVICE_STOPPED => State::Stopped,
            _ => return Err(error("unsupported initial SCM state")),
        };
        self.old = Some(state);
        Ok(self.old)
    }
    fn prepare(&mut self) -> io::Result<()> {
        self.files.prepare()?;
        let tx = self.tx()?.to_path_buf();
        for (name, digest) in [
            ("polaris-helper.exe", self.request.helper_sha256.as_str()),
            ("core-manifest.json", self.request.manifest_sha256.as_str()),
            (IMAGE_NAME, self.request.cleaner_sha256.as_str()),
        ] {
            self.files
                .copy_to(self.source.file(name)?, &tx.join(name), Some(digest))?;
        }
        self.utility = Some(tx.join("polaris-helper.exe"));
        create_private_file(&tx.join("user-token"), self.request.token.as_bytes())?;
        apply_path_security(&tx.join("user-token"), PRIVATE_SD, true)?;
        self.files
            .secure_dir(&Path::new(SUPPORT).join("core"), true)?;
        // Existing private registration parents are checked, never normalized
        // before their original security has been proved.
        for p in [
            Path::new(SUPPORT).join("users"),
            self.registration_path().parent().unwrap().to_path_buf(),
        ] {
            if entry_present(&p)? {
                let chain = Chain::open(&p, None)?;
                private_registration_parent(chain.last())?;
            }
        }
        Ok(())
    }
    fn install_core(&mut self) -> io::Result<()> {
        let manifest = self.tx()?.join("core-manifest.json");
        self.checked(&[
            OsStr::new("core-install"),
            OsStr::new("--root"),
            OsStr::new(r"C:\ProgramData\Polaris\core"),
            OsStr::new("--source"),
            OsStr::new(&self.request.stage),
            OsStr::new("--manifest"),
            manifest.as_os_str(),
            OsStr::new("--source-owner"),
            OsStr::new(&format!("s{}", self.request.original_sid)),
        ])?;
        Ok(())
    }
    fn selection(&mut self) -> io::Result<Selection> {
        let output = self.checked(&[
            OsStr::new("helper-policy"),
            OsStr::new("--binary"),
            OsStr::new(BINARY),
            OsStr::new("--metadata"),
            OsStr::new(r"C:\ProgramData\Polaris\helper-installed.json"),
        ])?;
        match output.as_str() {
            "helper-policy retain" => Ok(Selection::Retain),
            "helper-policy replace" | "helper-policy replace-legacy" => Ok(Selection::Replace),
            _ => Err(error("invalid helper policy output")),
        }
    }
    fn retained_capability(&mut self, old: Option<State>) -> io::Result<()> {
        if old == Some(State::Running) {
            self.fixed_probe("admin-probe")?;
        } else {
            let c = Chain::open(Path::new(SUPPORT), None)?;
            let f = child(
                c.last(),
                OsStr::new("polaris-helper.exe"),
                false,
                false,
                null_mut(),
            )?;
            if hash(&f)? != self.request.helper_sha256 {
                return Err(error("stopped retained helper capability unavailable"));
            }
        }
        Ok(())
    }
    fn same_registration(&mut self) -> io::Result<bool> {
        let p = self.registration_path();
        if !entry_present(&p)? {
            return Ok(false);
        }
        let c = Chain::open(p.parent().unwrap(), None)?;
        let f = child(
            c.last(),
            OsStr::new("registration"),
            false,
            false,
            null_mut(),
        )?;
        security(&f, None, false)?;
        let expected = format!(
            "s{}\n{}\n{}\n",
            self.request.original_sid, self.request.config, self.request.token
        );
        Ok(read_bounded(&f, 65536)? == expected.as_bytes())
    }
    fn retain_probe(&mut self) -> io::Result<bool> {
        let (_, code) = self.utility(&[
            OsStr::new("retain-probe"),
            OsStr::new("--support"),
            OsStr::new(SUPPORT),
            OsStr::new("--id"),
            OsStr::new(&self.request.core_id),
        ])?;
        Ok(code == 0)
    }
    fn snapshot(&mut self, replace: bool) -> io::Result<()> {
        self.files.snapshot(replace)
    }
    fn admit(&mut self) -> io::Result<bool> {
        let (_, code) = self.utility(&[
            OsStr::new("prepare-upgrade"),
            OsStr::new("--support"),
            OsStr::new(SUPPORT),
        ])?;
        match code {
            0 => Ok(true),
            75 => Ok(false),
            _ => Err(error("upgrade admission completion unknown")),
        }
    }
    fn revalidate(&mut self) -> io::Result<()> {
        self.assert_service()
    }
    fn stop(&mut self) -> io::Result<()> {
        if let Some(s) = &self.service {
            stop_service(s.0)?;
        }
        Ok(())
    }
    fn register_user(&mut self) -> io::Result<()> {
        let users = Path::new(SUPPORT).join("users");
        self.files.secure_dir(&users, false)?;
        self.files
            .secure_dir(self.registration_path().parent().unwrap(), false)?;
        let token = self.tx()?.join("user-token");
        self.checked(&[
            OsStr::new("register-user"),
            OsStr::new("--support"),
            OsStr::new(SUPPORT),
            OsStr::new("--identity"),
            OsStr::new(&format!("s{}", self.request.original_sid)),
            OsStr::new("--confdir"),
            OsStr::new(&self.request.config),
            OsStr::new("--token-file"),
            token.as_os_str(),
        ])?;
        Ok(())
    }
    fn replace_payload(&mut self) -> io::Result<()> {
        self.files
            .replace_payload(&self.request.helper_sha256, &self.request.cleaner_sha256)?;
        self.checked(&[
            OsStr::new("helper-record"),
            OsStr::new("--binary"),
            OsStr::new(BINARY),
            OsStr::new("--metadata"),
            OsStr::new(r"C:\ProgramData\Polaris\helper-installed.json"),
        ])?;
        Ok(())
    }
    // SAFETY: fresh-only state-machine branch, fixed service/account/command;
    // the exact returned SCM object owns every subsequent config/rollback call.
    fn create(&mut self) -> io::Result<()> {
        self.audit = Some(AuditPrivilege::enable()?);
        let name = wide(OsStr::new(SERVICE));
        let command = wide(OsStr::new(BIN_PATH));
        let raw = unsafe {
            CreateServiceW(
                self.manager.0,
                name.as_ptr(),
                name.as_ptr(),
                FRESH_SERVICE_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                command.as_ptr(),
                null(),
                null_mut(),
                null(),
                null(),
                null(),
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        self.service = Some(ScHandle(raw));
        Ok(())
    }
    // SAFETY: live fresh SCM handle, owned descriptor and fixed action array;
    // no pointer escapes ChangeServiceConfig2W. Verify SACL before success.
    fn configure_fresh(&mut self) -> io::Result<()> {
        let service = self.svc()?;
        let sd = Descriptor::parse(FRESH_SD)?;
        // Missing normal SACL privilege is an error; never drop the SACL or
        // change account/desktop privilege policy to make this operation pass.
        bool_ok(unsafe {
            SetServiceObjectSecurity(
                service,
                DACL_SECURITY_INFORMATION | SACL_SECURITY_INFORMATION,
                sd.ptr(),
            )
        })?;
        let mut actions = [SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5000,
        }; 3];
        let mut f = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: 86400,
            lpRebootMsg: null_mut(),
            lpCommand: null_mut(),
            cActions: 3,
            lpsaActions: actions.as_mut_ptr(),
        };
        bool_ok(unsafe {
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_FAILURE_ACTIONS,
                (&mut f as *mut SERVICE_FAILURE_ACTIONSW).cast(),
            )
        })?;
        fresh_sacl(service)?;
        self.admitted_policy = Some(read_policy(service)?);
        self.assert_service()
    }
    // SAFETY: retained exact SCM object, zero argument count and native NULL.
    fn start(&mut self) -> io::Result<()> {
        let s = self.svc()?;
        bool_ok(unsafe { StartServiceW(s, 0, null()) })?;
        wait_state(s, SERVICE_RUNNING)?;
        if status(s)? != SERVICE_RUNNING {
            return Err(error("SCM start not confirmed"));
        }
        self.assert_service()
    }
    // SAFETY: only the retained candidate SCM object can be stopped/deleted;
    // original existing services never reach DeleteService.
    fn stop_candidate(&mut self, old: Option<State>, tx: &Transaction) -> io::Result<()> {
        if old.is_some() {
            self.assert_service()?;
            stop_service(self.svc()?)?;
        } else if tx.created {
            self.assert_service()?;
            let s = self.svc()?;
            stop_service(s)?;
            bool_ok(unsafe { DeleteService(s) })?;
            self.service.take();
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                let raw = unsafe {
                    OpenServiceW(
                        self.manager.0,
                        wide(OsStr::new(SERVICE)).as_ptr(),
                        SERVICE_QUERY_STATUS,
                    )
                };
                if raw.is_null() {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
                        break;
                    }
                    if e.raw_os_error() != Some(ERROR_SERVICE_MARKED_FOR_DELETE as i32) {
                        return Err(e);
                    }
                } else {
                    drop(ScHandle(raw));
                }
                if Instant::now() >= until {
                    return Err(error("fresh SCM deletion timeout"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        } else if tx.create_attempted {
            return Err(error("service creation completion unknown"));
        }
        Ok(())
    }
    fn restore_payload(&mut self) -> io::Result<()> {
        self.files.restore_payload()
    }
    fn restore_registration(&mut self) -> io::Result<()> {
        self.files.restore_registration()
    }
    fn restore_previous(&mut self, old: Option<State>) -> io::Result<()> {
        transaction::restore_service(self, old)
    }
    fn cancel_admission(&mut self) -> io::Result<()> {
        self.fixed_probe("cancel-upgrade")?;
        Ok(())
    }
    fn finish(&mut self, keep: bool) -> io::Result<()> {
        if self.unknown_child.get() {
            self.files.finish(true)?;
            return Err(error("uncertain utility transaction retained"));
        }
        if let Some(audit) = self.audit.as_mut() {
            audit.close()?;
        }
        self.audit.take();
        self.files.finish(keep)
    }
}

impl transaction::RecoveryOps for Installer {
    fn recovery_policy(&mut self) -> io::Result<()> {
        self.assert_service()
    }
    fn recovery_start(&mut self) -> io::Result<()> {
        bool_ok(unsafe { StartServiceW(self.svc()?, 0, null()) })
    }
    fn status_begin(&mut self) -> io::Result<()> {
        status(self.svc()?)?;
        Ok(())
    }
    fn status_wait_running(&mut self) -> io::Result<()> {
        wait_state(self.svc()?, SERVICE_RUNNING)
    }
    fn status_refresh(&mut self) -> io::Result<Option<State>> {
        self.final_state()
    }
    fn status_end(&mut self) {} // Borrow ends; original SCM HANDLE stays retained.
    fn final_state(&mut self) -> io::Result<Option<State>> {
        match status(self.svc()?)? {
            SERVICE_RUNNING => Ok(Some(State::Running)),
            SERVICE_STOPPED => Ok(Some(State::Stopped)),
            SERVICE_START_PENDING | SERVICE_STOP_PENDING => Ok(None),
            _ => Err(error("unsupported SCM recovery state")),
        }
    }
}

pub fn run_install_worker(path: &Path, compiled_anchor: Option<&'static str>) -> i32 {
    let result = (|| -> io::Result<i32> {
        if !path.to_str().is_some_and(transaction::local_path)
            || path.file_name() != Some(OsStr::new("install-request.json"))
        {
            return Err(error("exact local request path required"));
        }
        let anchor = crate::install::Anchor::parse(
            compiled_anchor
                .ok_or_else(|| error("maintenance producer anchor missing"))?
                .as_bytes(),
        )?;
        if !token_info().is_ok_and(|(sid, elevated)| elevated || sid == SYSTEM) {
            return Err(error("administrator required"));
        }
        // Request is already held no-write/no-delete by the App. Acquire its
        // exact object before reading its data, then hold it through finish.
        let preliminary = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        info(&preliminary)?;
        let request = Request::parse(&read_bounded(
            &preliminary,
            transaction::MAX_REQUEST as u64,
        )?)?;
        anchor.admits(&request)?;
        let chain = Chain::open(path.parent().unwrap(), Some(&request.original_sid))?;
        let held = child(
            chain.last(),
            OsStr::new("install-request.json"),
            false,
            false,
            null_mut(),
        )?;
        security(&held, Some(&request.original_sid), false)?;
        if identity(&held)? != identity(&preliminary)? {
            return Err(error("request identity changed"));
        }
        drop(preliminary);
        let image = Image::open(&std::env::current_exe()?, Some(&request.original_sid))?;
        if hash(&image.file)? != request.cleaner_sha256 {
            return Err(error("worker image differs"));
        }
        // Bind the already loaded mapping too; do not equate current_exe path
        // equality with the actual PE section used to enter this worker.
        let mut mapped = vec![0u16; 32768];
        let module = unsafe { GetModuleHandleW(null()) };
        let n = unsafe {
            K32GetMappedFileNameW(
                GetCurrentProcess(),
                module.cast(),
                mapped.as_mut_ptr(),
                mapped.len() as u32,
            )
        };
        if n == 0 || n as usize >= mapped.len() || {
            let expected: Vec<u16> = final_path(&image.file, VOLUME_NAME_NT)?
                .encode_wide()
                .collect();
            // SAFETY: both UTF-16 slices remain live; checked lengths fit i32.
            (unsafe {
                windows_sys::Win32::Globalization::CompareStringOrdinal(
                    mapped.as_ptr(),
                    n as i32,
                    expected.as_ptr(),
                    expected.len() as i32,
                    1,
                )
            }) != windows_sys::Win32::Globalization::CSTR_EQUAL
        } {
            return Err(error("loaded maintenance image differs"));
        }
        let source = Source::open(&request, false)?;
        let mut installer = Installer::open(request, source)?;
        let code = transaction::run(&mut installer);
        drop(installer);
        drop(image);
        drop(held);
        drop(chain);
        Ok(code)
    })();
    // Before transaction entry failures are sealed-stage failures, not a
    // successful rollback or a user-cancellation classification.
    result.unwrap_or(81)
}

#[cfg(test)]
pub(crate) mod tests;

//! Stable Win32 STARTUPINFOEX factory. Job membership and the exact inheritable
//! object set are applied by CreateProcess, before any child instruction.
use super::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

fn system32() -> io::Result<PathBuf> {
    let mut b = vec![0u16; 32768];
    // SAFETY: output capacity is in WCHARs; no environment-selected system path.
    let n = unsafe { GetSystemDirectoryW(b.as_mut_ptr(), b.len() as u32) } as usize;
    if n == 0 || n >= b.len() {
        return Err(io::Error::other("System32 unavailable"));
    }
    Ok(PathBuf::from(
        String::from_utf16(&b[..n]).map_err(io::Error::other)?,
    ))
}
pub(super) fn hold_system_imports(
    imports: &[String],
    cohort: &mut Cohort,
) -> Result<(), AdmissionError> {
    let system = system32()?;
    // Private selected Cronet is the sole non-OS dependency. Every other
    // accepted import must exist in the native System32 trust boundary, never
    // ambient CWD/PATH. Unknown/API-set/SxS/delay forms stay unsupported.
    const OS: &[&str] = &[
        "kernel32.dll",
        "ntdll.dll",
        "advapi32.dll",
        "user32.dll",
        "shell32.dll",
        "ws2_32.dll",
        "bcrypt.dll",
        "secur32.dll",
        "crypt32.dll",
        "normaliz.dll",
        "iphlpapi.dll",
        "msvcrt.dll",
        "ole32.dll",
        "oleaut32.dll",
        "winhttp.dll",
        "shlwapi.dll",
        "userenv.dll",
        "version.dll",
        "winmm.dll",
        "wininet.dll",
    ];
    for name in imports {
        if name == "libcronet.dll" {
            continue;
        }
        if !OS.contains(&name.as_str()) {
            return Err(error("Windows import is outside the supported closure"));
        }
        let f = guard(&system.join(name), false)?;
        if !f.metadata()?.is_file() {
            return Err(error("System32 import is not a regular file"));
        }
        final_path(&f)?;
        cohort.system.push(f);
    }
    Ok(())
}

struct Attributes {
    _storage: Vec<usize>,
    pointer: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut n = 0;
        // SAFETY: documented size query; storage is pointer-aligned and stable.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 3, 0, &mut n);
        }
        if n == 0 || n > 65536 {
            return Err(io::Error::other("attribute list size unavailable"));
        }
        let mut storage = vec![0usize; n.div_ceil(mem::size_of::<usize>())];
        let pointer = storage.as_mut_ptr().cast();
        check(unsafe { InitializeProcThreadAttributeList(pointer, 3, 0, &mut n) })?;
        Ok(Self {
            _storage: storage,
            pointer,
        })
    }
    fn set<T>(&mut self, key: u32, values: &[T]) -> io::Result<()> {
        // SAFETY: caller keeps every supplied slice alive through CreateProcess.
        check(unsafe {
            UpdateProcThreadAttribute(
                self.pointer,
                0,
                key as usize,
                values.as_ptr().cast(),
                mem::size_of_val(values),
                ptr::null_mut(),
                ptr::null(),
            )
        })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer);
        }
    }
}
fn duplicate(f: &File) -> io::Result<OwnedHandle> {
    let mut h = ptr::null_mut();
    // SAFETY: duplicate our live handle, same rights; only this duplicate inherits.
    check(unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            f.as_raw_handle(),
            GetCurrentProcess(),
            &mut h,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    owned(h)
}
fn pipe() -> io::Result<(File, File)> {
    let (mut r, mut w) = (ptr::null_mut(), ptr::null_mut());
    let sa = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 0,
    };
    // SAFETY: anonymous private pipe endpoints transferred to independent Files.
    check(unsafe { CreatePipe(&mut r, &mut w, &sa, 0) })?;
    Ok((File::from(owned(r)?), File::from(owned(w)?)))
}
fn new_job() -> io::Result<OwnedHandle> {
    // SAFETY: unnamed, non-inheritable private job, no unrelated processes.
    let job = owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: owned job and the exact native structure lifetime/size.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            mem::size_of_val(&info) as u32,
        )
    })?;
    Ok(job)
}

pub(in super::super) struct ChildCustody {
    process: OwnedHandle,
    _job: OwnedHandle,
    cohort: Option<Arc<Cohort>>,
}
impl Drop for ChildCustody {
    fn drop(&mut self) {
        // SAFETY: original process HANDLE is retained; timeout/failure is Unknown.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
            if let Some(owner) = self.cohort.take() {
                let _ = Arc::into_raw(owner);
            }
        }
        // Job closes after this body: fail-stop, never revoke a live child's
        // inherited physical guards or recursively delete an Unknown cohort.
    }
}

fn create_suspended(
    image: &Path,
    sidecar: &Path,
    config: &File,
    cwd: &Path,
    guards: &[&File],
) -> Result<Suspended, AdmissionError> {
    let job = new_job()?;
    let (stdout, out_write) = pipe()?;
    let (stderr, err_write) = pipe()?;
    let mut inherit = Vec::new();
    for f in [config, &out_write, &err_write]
        .into_iter()
        .chain(guards.iter().copied())
    {
        inherit.push(duplicate(f)?);
    }
    let handles: Vec<_> = inherit.iter().map(AsRawHandle::as_raw_handle).collect();
    let jobs = [job.as_raw_handle()];
    // Creation-time restrictions, not parent-global SetDefaultDllDirectories.
    // No remote/low-integrity images; prefer System32; disable extension points.
    let mitigation = [(1u64 << 52) | (1u64 << 56) | (1u64 << 60) | (1u64 << 32)];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, &mitigation)?;
    let system = system32()?;
    let windows = system.parent().ok_or_else(|| error("SystemRoot absent"))?;
    let env = format!(
        "PATH={}\0POLARIS_CRONET_LIBRARY_PATH={}\0SystemRoot={}\0\0",
        system.display(),
        sidecar.display(),
        windows.display()
    );
    let environment: Vec<u16> = env.encode_utf16().collect();
    let application = wide(image)?;
    let cwd = wide(cwd)?;
    // Only the fixed RunMain argv exists. A path quote cannot become an option.
    let mut command = wide(format!("\"{}\" run -c stdin", image.display()))?;
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.pointer;
    let mut info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    // SAFETY: all application/argv/env/attributes/handle slices stay live through
    // the call. Job list and handle list apply atomically before first instruction.
    check(unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | CREATE_NEW_PROCESS_GROUP,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    })?;
    let process = owned(info.hProcess)?;
    let thread = owned(info.hThread)?;
    drop(inherit);
    drop(out_write);
    drop(err_write);
    Ok(Suspended {
        process: Some(process),
        thread,
        job: Some(job),
        stdout: Some(stdout),
        stderr: Some(stderr),
        pid: info.dwProcessId,
    })
}

struct Suspended {
    process: Option<OwnedHandle>,
    thread: OwnedHandle,
    job: Option<OwnedHandle>,
    stdout: Option<File>,
    stderr: Option<File>,
    pid: u32,
}
impl Drop for Suspended {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            // SAFETY: this is exclusively our original, not-yet-published child.
            if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
                if let Some(job) = self.job.take() {
                    drop(job);
                }
                unsafe {
                    WaitForSingleObject(process.as_raw_handle(), 10_000);
                }
            }
        }
    }
}

pub(in super::super) fn spawn(
    owner: &Arc<Cohort>,
    config: File,
    cwd: &Path,
) -> Result<WindowsStartedChild, AdmissionError> {
    system_actor()?;
    if !owner.is_current() || !cwd.is_absolute() {
        return Err(error("Windows plan changed before spawn"));
    }
    let guards: Vec<&File> = owner
        .files
        .iter()
        .map(|(_, f, _)| f)
        .chain(owner.directories.iter())
        .chain(owner.parents.iter())
        .chain(owner.system.iter())
        .chain(std::iter::once(&owner.occupancy))
        .collect();
    let mut suspended = create_suspended(
        &owner.final_exe,
        &owner.final_sidecar,
        &config,
        cwd,
        &guards,
    )?;
    // The process is suspended and already in its job with inherited guards.
    // A changed plan is cancelled without permitting its first instruction.
    if !owner.is_current() {
        return Err(error("queued Windows plan retired"));
    }
    let process = suspended.process.as_ref().expect("owned suspended child");
    let mut duplicate_process = ptr::null_mut();
    // SAFETY: retain this exact process independently for the custody owner.
    check(unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            process.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate_process,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    let custody = WindowsChildCustody {
        _inner: ChildCustody {
            process: owned(duplicate_process)?,
            _job: suspended.job.take().expect("atomic job"),
            cohort: Some(Arc::clone(owner)),
        },
    };
    // SAFETY: all physical custody was applied atomically before first execution.
    if unsafe { ResumeThread(suspended.thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error().into());
    }
    Ok(WindowsStartedChild {
        process: suspended.process.take().expect("owned child"),
        stdout: suspended.stdout.take().expect("stdout"),
        stderr: suspended.stderr.take().expect("stderr"),
        custody,
        pid: suspended.pid,
    })
}

#[cfg(test)]
mod tests;

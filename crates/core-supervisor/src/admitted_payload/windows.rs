//! SYSTEM Helper admission requires fresh private objects, a closed loader
//! namespace and an atomic native factory. Ordinary App admission is refused.
use super::*;
use std::fs::File;
use std::os::windows::io::OwnedHandle;

#[allow(
    unsafe_code,
    reason = "private Win32 issuer and restricted native factory"
)]
mod issuer;
mod pe;

const LOADER: &str = "windows-helper-private-cohort-v1";

/// Occupancy only, never an execution credential. CoreStore calls this AFTER
/// its normal exclusive id.lock and BEFORE any repair/GC/retention publication.
/// A child's inherited READ/share-READ guard rejects this mandatory RW open,
/// even when a read-only caller could acquire an unrelated LockFileEx lock.
pub struct WindowsMaintenanceGuard {
    _file: File,
}
impl WindowsMaintenanceGuard {
    pub fn for_store(root: &Path, id: &CoreId) -> std::io::Result<Option<Self>> {
        issuer::maintenance_file(root, id).map(|f| f.map(|f| Self { _file: f }))
    }
}

pub(super) fn unavailable() -> AdmissionError {
    AdmissionError::new(
        AdmissionErrorKind::UnsupportedExactObject,
        "trusted Windows execution requires a matching supported release and SYSTEM Helper issuer",
    )
}

#[derive(Debug)]
pub(super) struct ProtectedMembers {
    inner: Arc<issuer::Cohort>,
}

pub(super) fn declaration(
    value: &serde_json::Value,
    source: &str,
    tree: &str,
) -> Result<(Vec<ExecPurpose>, String), AdmissionError> {
    const FIELDS: [&str; 5] = ["schema", "sourceCommit", "sourceTree", "loader", "purposes"];
    if !value
        .as_object()
        .is_some_and(|o| o.len() == 5 && FIELDS.iter().all(|k| o.contains_key(*k)))
        || value["schema"] != "polaris-trusted-execution-v1"
        || value["sourceCommit"] != source
        || value["sourceTree"] != tree
        || value["loader"] != LOADER
        || value["purposes"] != serde_json::json!(["check-config-v1", "run-main-v1"])
    {
        return Err(unavailable());
    }
    Ok((
        vec![ExecPurpose::CheckConfig, ExecPurpose::RunMain],
        LOADER.into(),
    ))
}

pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    if policy.loader != LOADER
        || policy.purposes != [ExecPurpose::CheckConfig, ExecPurpose::RunMain]
    {
        return Err(unavailable());
    }
    Ok(ProtectedMembers {
        inner: Arc::new(issuer::issue(input, policy)?),
    })
}
impl ProtectedMembers {
    pub(super) fn is_current(&self) -> bool {
        self.inner.is_current()
    }
    pub(super) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        Err(unavailable())
    }
}

/// Created only by the native factory. No mutable Command or authority-bearing
/// raw File constructor is exposed to the caller.
pub struct WindowsStartedChild {
    process: OwnedHandle,
    stdout: Option<File>,
    stderr: Option<File>,
    custody: WindowsChildCustody,
    pid: u32,
}
impl WindowsStartedChild {
    pub fn into_parts(self) -> (u32, OwnedHandle, File, File, WindowsChildCustody) {
        (
            self.pid,
            self.process,
            self.stdout.expect("main stdout"),
            self.stderr.expect("main stderr"),
            self.custody,
        )
    }
}
/// Keep beside ManagedChild through actual wait/Unknown. Early drop closes the
/// fail-stop job and retains the cohort as Unknown; inherited file guards stay
/// with the child until kernel process teardown, independently of parent locks.
pub struct WindowsChildCustody {
    _inner: issuer::ChildCustody,
}
impl std::fmt::Debug for WindowsChildCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsChildCustody")
            .finish_non_exhaustive()
    }
}
impl PreparedExecution {
    pub fn spawn_windows_main(
        &self,
        config: File,
        cwd: &Path,
    ) -> Result<WindowsStartedChild, AdmissionError> {
        // Serialize queued-plan recheck through ResumeThread with replacement/
        // retirement. No future may outlive this gate by retaining a Command.
        let slot = SLOT.get_or_init(Default::default).lock().map_err(|_| {
            AdmissionError::new(AdmissionErrorKind::Unknown, "admission slot poisoned")
        })?;
        if !slot.as_ref().is_some_and(|p| Arc::ptr_eq(p, &self.payload)) {
            return Err(unavailable());
        }
        self.payload.prepare(self.purpose)?;
        if self.purpose != ExecPurpose::RunMain {
            return Err(unavailable());
        }
        issuer::spawn(
            &self.payload.protected.inner,
            config,
            cwd,
            ExecPurpose::RunMain,
        )
    }
}

#[cfg(test)]
mod tests;

/// Check child custody never exposes a process handle or mutable native command.
pub(crate) struct WindowsCheckChild {
    child: WindowsStartedChild,
}
#[allow(
    unsafe_code,
    reason = "wait and terminate only the retained exact Check process HANDLE"
)]
impl WindowsCheckChild {
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        use std::os::windows::{io::AsRawHandle, process::ExitStatusExt};
        use windows_sys::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        };
        match unsafe { WaitForSingleObject(self.child.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.child.process.as_raw_handle(), &mut code) } == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(Some(std::process::ExitStatus::from_raw(code)))
            }
            _ => Err(std::io::Error::last_os_error()),
        }
    }
    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        if unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(
                self.child.process.as_raw_handle(),
                1,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    pub(crate) fn take_stdout(&mut self) -> Option<File> {
        self.child.stdout.take()
    }
    pub(crate) fn take_stderr(&mut self) -> Option<File> {
        self.child.stderr.take()
    }
}
pub(crate) struct WindowsCheckInput {
    execution: Option<PreparedExecution>,
    snapshot: issuer::check_input::CheckSnapshot,
}
impl WindowsCheckInput {
    pub(crate) fn append(&mut self, offset: usize, bytes: &[u8]) -> Result<(), AdmissionError> {
        self.snapshot.append(offset, bytes)
    }
    pub(crate) fn seal(&mut self, len: usize, digest: &[u8; 32]) -> Result<(), AdmissionError> {
        self.snapshot.seal(len, digest)
    }
    pub(crate) fn retire(&mut self) -> Result<(), AdmissionError> {
        self.snapshot.retire()
    }
    pub(crate) fn spawn(&mut self) -> Result<WindowsCheckChild, AdmissionError> {
        let execution = self.execution.take().ok_or_else(unavailable)?;
        let slot = SLOT
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| unavailable())?;
        if !slot
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, &execution.payload))
            || execution.purpose != ExecPurpose::CheckConfig
        {
            return Err(unavailable());
        }
        execution.payload.prepare(ExecPurpose::CheckConfig)?;
        let input = self.snapshot.input()?;
        let cwd = execution.payload.protected.inner.root_directory();
        let child = issuer::spawn(
            &execution.payload.protected.inner,
            input,
            &cwd,
            ExecPurpose::CheckConfig,
        )?;
        Ok(WindowsCheckChild { child })
    }
}
impl PreparedExecution {
    pub(crate) fn windows_check_input(
        self,
        _lease: &File,
    ) -> Result<WindowsCheckInput, AdmissionError> {
        if self.purpose != ExecPurpose::CheckConfig {
            return Err(unavailable());
        }
        self.payload.prepare(self.purpose)?;
        let snapshot =
            issuer::check_input::CheckSnapshot::create(Arc::clone(&self.payload.protected.inner))?;
        Ok(WindowsCheckInput {
            execution: Some(self),
            snapshot,
        })
    }
}

#[allow(
    unsafe_code,
    reason = "OS CSPRNG produces correlation data, never an execution credential"
)]
pub(crate) fn check_correlation() -> Result<polaris_helper_proto::HelperBirthToken, AdmissionError>
{
    let mut bytes = [0u8; 16];
    if unsafe {
        windows_sys::Win32::Security::Cryptography::BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            windows_sys::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } != 0
    {
        return Err(unavailable());
    }
    Ok(polaris_helper_proto::HelperBirthToken::from_bytes(bytes))
}

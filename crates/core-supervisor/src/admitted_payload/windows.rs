//! SYSTEM Helper admission requires fresh private objects, a closed loader
//! namespace and an atomic native factory. The ordinary App hashes and runs by path.
use super::*;
use std::fs::File;
use std::os::windows::io::OwnedHandle;

#[allow(
    unsafe_code,
    reason = "private Win32 issuer and restricted native factory"
)]
mod issuer;
mod pe;

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

/// SYSTEM Helper RunMain owns a private cohort; the ordinary App hashes and runs by path.
#[derive(Debug)]
pub(super) enum ProtectedMembers {
    Issuer(Arc<issuer::Cohort>),
    Direct(super::direct::DirectMembers),
}

pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    if issuer::system_actor().is_err() {
        return super::direct::protect_members(input, policy).map(ProtectedMembers::Direct);
    }
    Ok(ProtectedMembers::Issuer(Arc::new(issuer::issue(
        input, policy,
    )?)))
}
impl ProtectedMembers {
    pub(super) fn is_current(&self) -> bool {
        match self {
            Self::Issuer(cohort) => cohort.is_current(),
            Self::Direct(direct) => direct.is_current(),
        }
    }
    pub(super) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        match self {
            Self::Issuer(_) => Err(unavailable()),
            Self::Direct(direct) => Ok(direct.command()),
        }
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
        runtime_directory: &Path,
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
        let ProtectedMembers::Issuer(cohort) = &self.payload.protected else {
            return Err(unavailable());
        };
        if self.purpose != ExecPurpose::RunMain {
            return Err(unavailable());
        }
        issuer::spawn(
            cohort,
            config,
            cwd,
            ExecPurpose::RunMain,
            Some(runtime_directory),
        )
    }
}

#[cfg(test)]
mod tests;

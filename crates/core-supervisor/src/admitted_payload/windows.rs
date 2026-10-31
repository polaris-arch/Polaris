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
        || value["purposes"] != serde_json::json!(["run-main-v1"])
    {
        return Err(unavailable());
    }
    Ok((vec![ExecPurpose::RunMain], LOADER.into()))
}

pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<ProtectedMembers, AdmissionError> {
    if policy.loader != LOADER || policy.purposes != [ExecPurpose::RunMain] {
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
    stdout: File,
    stderr: File,
    custody: WindowsChildCustody,
    pid: u32,
}
impl WindowsStartedChild {
    pub fn into_parts(self) -> (u32, OwnedHandle, File, File, WindowsChildCustody) {
        (
            self.pid,
            self.process,
            self.stdout,
            self.stderr,
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
        issuer::spawn(&self.payload.protected.inner, config, cwd)
    }
}

#[cfg(test)]
mod issuer_contract_tests {
    use super::*;
    #[test]
    fn windows_contract_is_exact_compiled_run_main_only() {
        let source = "a".repeat(40);
        let tree = "b".repeat(40);
        let policy = serde_json::json!({"schema":"polaris-trusted-execution-v1","sourceCommit":source,"sourceTree":tree,"loader":LOADER,"purposes":["run-main-v1"]});
        assert_eq!(
            SelectedPolicy::declaration(&policy, &source, &tree, "win")
                .unwrap()
                .0,
            vec![ExecPurpose::RunMain]
        );
        for field in ["sourceCommit", "sourceTree", "loader", "purposes", "schema"] {
            let mut bad = policy.clone();
            bad[field] = serde_json::Value::Null;
            assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
        }
        let mut bad = policy.clone();
        bad["allowUntrusted"] = true.into();
        assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
        bad = policy;
        bad["purposes"] = serde_json::json!(["run-main-v1", "run-login-v1"]);
        assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
    }
    #[test]
    fn ordinary_primary_token_cannot_issue_or_adopt_any_file() {
        // No fixture SID, policy or authority constructor bypasses the actual
        // primary-token check. SYSTEM is an unperformed service acceptance gate.
        assert!(
            issuer::system_actor().is_err(),
            "this ordinary test must not run as SYSTEM"
        );
    }
}

#[cfg(test)]
mod tests;

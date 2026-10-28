//! Persistent custody for the repository's single native validation producer.
//! A cancelled caller never owns the only Child or its private config snapshot.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::future::{poll_fn, Future};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::sync::Notify;

use super::RawCheck;

const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Arc<CheckCustody>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationLifecycleError {
    #[error("validation admission is closing")]
    Closing,
    #[error("validation cleanup remains unconfirmed: {0}")]
    CleanupUnconfirmed(String),
}

fn unknown(error: impl std::fmt::Display) -> ValidationLifecycleError {
    ValidationLifecycleError::CleanupUnconfirmed(error.to_string())
}

fn registry() -> &'static Arc<CheckCustody> {
    REGISTRY.get_or_init(|| Arc::new(CheckCustody::default()))
}

pub fn begin_check_shutdown() -> Result<(), ValidationLifecycleError> {
    registry().begin_shutdown()
}

pub async fn shutdown_checks_for_exit() -> Result<(), ValidationLifecycleError> {
    registry().shutdown().await
}

pub fn assert_check_admission() -> Result<(), ValidationLifecycleError> {
    registry().assert_admission()
}

/// Hold validation admission across a synchronous writer spawn/publication. The closure
/// must not await or recursively enter validation custody.
pub fn with_check_admission<T>(spawn: impl FnOnce() -> T) -> Result<T, ValidationLifecycleError> {
    let state = registry().state.lock().map_err(unknown)?;
    CheckCustody::admit(&state)?;
    Ok(spawn())
}

pub async fn settle_check_cleanup() -> Result<(), ValidationLifecycleError> {
    registry().settle_debts().await
}

trait CheckIo: Send + Sync {
    fn try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        child.try_wait()
    }
    fn poll_wait(&self, child: &mut Child, cx: &mut Context<'_>) -> Poll<io::Result<ExitStatus>> {
        let wait = child.wait();
        tokio::pin!(wait);
        wait.poll(cx)
    }
    fn start_kill(&self, child: &mut Child) -> io::Result<()> {
        child.start_kill()
    }
}
struct NativeIo;
impl CheckIo for NativeIo {}

#[derive(Default)]
struct OutputCapture {
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
    errors: Mutex<Vec<String>>,
    done: AtomicUsize,
    changed: Notify,
}

impl OutputCapture {
    fn read<R: AsyncRead + Unpin + Send + 'static>(
        self: &Arc<Self>,
        pipe: Option<R>,
        stderr: bool,
    ) {
        let output = Arc::clone(self);
        tokio::spawn(async move {
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                if let Err(error) = pipe.read_to_end(&mut bytes).await {
                    if let Ok(mut errors) = output.errors.lock() {
                        errors.push(error.to_string());
                    }
                }
                let buffer = if stderr {
                    &output.stderr
                } else {
                    &output.stdout
                };
                if let Ok(mut buffer) = buffer.lock() {
                    *buffer = bytes;
                }
            }
            output.done.fetch_add(1, Ordering::SeqCst);
            output.changed.notify_waiters();
        });
    }

    async fn finished(&self) {
        loop {
            let notified = self.changed.notified();
            if self.done.load(Ordering::SeqCst) == 2 {
                return;
            }
            notified.await;
        }
    }

    fn result(&self, success: bool) -> RawCheck {
        let Ok(errors) = self.errors.lock() else {
            return RawCheck::OutputFailed("validation output custody poisoned".into());
        };
        if !errors.is_empty() {
            return RawCheck::OutputFailed(errors.join("; "));
        }
        match (self.stdout.lock(), self.stderr.lock()) {
            (Ok(stdout), Ok(stderr)) => RawCheck::Done {
                success,
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            },
            _ => RawCheck::OutputFailed("validation output custody poisoned".into()),
        }
    }
}

// These identities attest only this custody's admitted native Child. They carry
// no App lifecycle generation, Go disposal claim or broader resource authority.
#[derive(Default)]
struct ValidationIssuerIdentity;
struct CheckRequestIdentity;
struct ValidationBirthIdentity;
struct ValidationMemberIdentity;

#[derive(Clone)]
struct CheckRequestRef {
    id: u64,
    issuer: Arc<ValidationIssuerIdentity>,
    identity: Arc<CheckRequestIdentity>,
}

impl CheckRequestRef {
    fn same(&self, other: &Self) -> bool {
        self.id == other.id
            && Arc::ptr_eq(&self.issuer, &other.issuer)
            && Arc::ptr_eq(&self.identity, &other.identity)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValidationRole {
    Validation,
    #[cfg(all(test, unix))]
    Foreign,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValidationScope {
    SingleValidationNativeChildV1,
    #[cfg(all(test, unix))]
    Foreign,
}

#[derive(Clone)]
struct ValidationNativeMembers {
    request: CheckRequestRef,
    birth: Arc<ValidationBirthIdentity>,
    member: Arc<ValidationMemberIdentity>,
    role: ValidationRole,
    scope: ValidationScope,
}

impl ValidationNativeMembers {
    fn same(&self, other: &Self) -> bool {
        self.request.same(&other.request)
            && Arc::ptr_eq(&self.birth, &other.birth)
            && Arc::ptr_eq(&self.member, &other.member)
            && self.role == ValidationRole::Validation
            && other.role == self.role
            && self.scope == ValidationScope::SingleValidationNativeChildV1
            && other.scope == self.scope
    }
}

enum NativeFactoryObservation {
    PreFactory,
    Entered,
    ReturnedNoChild,
    Attached(ValidationNativeMembers),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeExitSource {
    PollWait,
    TryWait,
}

#[derive(Clone)]
struct ValidationNativeExited {
    members: ValidationNativeMembers,
    status: ExitStatus,
    source: NativeExitSource,
}

struct ValidatedCheckNativeExit(ValidationNativeExited);
struct LocalValidationNativeTerminal {
    _native_exit: ValidationNativeExited,
}

struct CheckRun {
    request: CheckRequestRef,
    factory: NativeFactoryObservation,
    native_exited: Option<ValidationNativeExited>,
    native_terminal: Option<LocalValidationNativeTerminal>,
    child: Option<Child>,
    snapshot: PathBuf,
    exit: Option<ExitStatus>,
    debt: bool,
    lost_wait_ownership: bool,
    io: Arc<dyn CheckIo>,
}

impl CheckRun {
    fn cache_native_exit(
        &mut self,
        status: ExitStatus,
        source: NativeExitSource,
    ) -> io::Result<()> {
        self.exit = Some(status);
        let NativeFactoryObservation::Attached(members) = &self.factory else {
            self.debt = true;
            return Err(io::Error::other("native exit has no attached binding"));
        };
        self.native_exited = Some(ValidationNativeExited {
            members: members.clone(),
            status,
            source,
        });
        Ok(())
    }

    fn validate_retirement(
        &self,
    ) -> Result<Option<ValidatedCheckNativeExit>, ValidationLifecycleError> {
        if self.native_terminal.is_some() || self.lost_wait_ownership {
            return Err(unknown("validation native ownership is unavailable"));
        }
        match &self.factory {
            NativeFactoryObservation::PreFactory | NativeFactoryObservation::ReturnedNoChild
                if self.child.is_none() && self.exit.is_none() && self.native_exited.is_none() =>
            {
                Ok(None) // Own snapshot cleanup, never a native terminal.
            }
            NativeFactoryObservation::Attached(members) => {
                let Some(fact) = &self.native_exited else {
                    return Err(unknown("same Child native exit has not been confirmed"));
                };
                if self.child.is_none()
                    || !members.request.same(&self.request)
                    || !members.same(&fact.members)
                    || self.exit != Some(fact.status)
                    || !matches!(
                        fact.source,
                        NativeExitSource::PollWait | NativeExitSource::TryWait
                    )
                {
                    return Err(unknown("validation native exit binding does not match"));
                }
                // Clone before the last fallible cleanup step; tail failure retains
                // the original Child, binding and fact for the next attempt.
                Ok(Some(ValidatedCheckNativeExit(fact.clone())))
            }
            _ => Err(unknown("validation factory responsibility remains unknown")),
        }
    }

    fn record_error(&mut self, _error: &io::Error) {
        self.debt = true;
        // ECHILD is 10 on the supported Unix PC targets (Linux and Darwin).
        // A cached PID no longer grants permission to wait or signal after this error.
        #[cfg(unix)]
        if _error.raw_os_error() == Some(10) {
            self.lost_wait_ownership = true;
        }
    }

    fn poll_exit(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<ExitStatus>> {
        if let Some(exit) = self.exit {
            if self.native_exited.is_none() {
                return Poll::Ready(Err(io::Error::other("cached native exit has no binding")));
            }
            return Poll::Ready(Ok(exit));
        }
        if self.lost_wait_ownership {
            return Poll::Ready(Err(io::Error::other("native wait ownership was lost")));
        }
        let Some(child) = self.child.as_mut() else {
            return Poll::Ready(Err(io::Error::other("validation has no born Child")));
        };
        match self.io.poll_wait(child, cx) {
            Poll::Ready(Ok(exit)) => Poll::Ready(
                self.cache_native_exit(exit, NativeExitSource::PollWait)
                    .map(|()| exit),
            ),
            Poll::Ready(Err(error)) => {
                self.record_error(&error);
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn request_close(&mut self) -> io::Result<()> {
        self.debt = true;
        if self.exit.is_some() || self.child.is_none() {
            return Ok(());
        }
        if self.lost_wait_ownership {
            return Err(io::Error::other("native wait ownership was lost"));
        }
        let result = self
            .io
            .try_wait(self.child.as_mut().expect("retained Child"));
        match result {
            Ok(Some(exit)) => {
                return self.cache_native_exit(exit, NativeExitSource::TryWait);
            }
            Err(error) => {
                self.record_error(&error);
                return Err(error);
            }
            Ok(None) => {}
        }
        self.io
            .start_kill(self.child.as_mut().expect("retained Child"))
    }
}

impl Drop for CheckRun {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.lost_wait_ownership {
            if let Some(child) = self.child.take() {
                // Tokio's Unix Reaper Drop would otherwise wait the obsolete PID again.
                std::mem::forget(child);
            }
        }
    }
}

#[derive(Default)]
struct CheckState {
    closing: bool,
    settling: bool,
    runs: BTreeMap<u64, CheckRun>,
}

#[derive(Default)]
struct CheckCustody {
    issuer: Arc<ValidationIssuerIdentity>,
    state: Mutex<CheckState>,
    cleanup_gate: tokio::sync::Mutex<()>,
}

impl CheckCustody {
    fn run_mut<'a>(
        &self,
        state: &'a mut CheckState,
        request: &CheckRequestRef,
    ) -> Result<Option<&'a mut CheckRun>, ValidationLifecycleError> {
        if !Arc::ptr_eq(&self.issuer, &request.issuer) {
            return Err(unknown("validation request belongs to another custody"));
        }
        let run = state.runs.get_mut(&request.id);
        if run.as_ref().is_some_and(|run| !run.request.same(request)) {
            return Err(unknown("validation request identity does not match"));
        }
        Ok(run)
    }

    fn assert_admission(&self) -> Result<(), ValidationLifecycleError> {
        let state = self.state.lock().map_err(unknown)?;
        Self::admit(&state)
    }

    fn admit(state: &CheckState) -> Result<(), ValidationLifecycleError> {
        if state.closing {
            return Err(ValidationLifecycleError::Closing);
        }
        if state.settling || state.runs.values().any(|run| run.debt) {
            return Err(unknown("a previous validation birth retains cleanup debt"));
        }
        Ok(())
    }

    fn begin_shutdown(&self) -> Result<(), ValidationLifecycleError> {
        self.state.lock().map_err(unknown)?.closing = true;
        Ok(())
    }

    async fn wait_exit(
        &self,
        request: &CheckRequestRef,
    ) -> Result<ExitStatus, ValidationLifecycleError> {
        poll_fn(|cx| {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(error) => return Poll::Ready(Err(unknown(error))),
            };
            let run = match self.run_mut(&mut state, request) {
                Ok(run) => run,
                Err(error) => return Poll::Ready(Err(error)),
            };
            let Some(run) = run else {
                return Poll::Ready(Err(unknown("validation birth was already retired")));
            };
            run.poll_exit(cx).map_err(unknown)
        })
        .await
    }

    fn request_close(&self, request: &CheckRequestRef) -> Result<(), ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        let Some(run) = self.run_mut(&mut state, request)? else {
            return Ok(()); // the exact request has already retired; never address a replacement
        };
        run.request_close().map_err(unknown)
    }

    fn retire(&self, request: &CheckRequestRef) -> Result<(), ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        let Some(run) = self.run_mut(&mut state, request)? else {
            return Ok(());
        };
        let validated = run.validate_retirement()?;
        match std::fs::remove_file(&run.snapshot) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                run.debt = true;
                return Err(unknown(error));
            }
        }
        // Snapshot removal above is the LAST fallible step. No new lookup, lock,
        // validation or allocation follows it; commit only this exact run.
        run.native_terminal = validated.map(|validated| LocalValidationNativeTerminal {
            _native_exit: validated.0,
        });
        state.runs.remove(&request.id);
        Ok(())
    }

    async fn close_confirmed(
        &self,
        request: &CheckRequestRef,
    ) -> Result<(), ValidationLifecycleError> {
        self.request_close(request)?;
        let born = {
            let mut state = self.state.lock().map_err(unknown)?;
            self.run_mut(&mut state, request)?
                .is_some_and(|run| run.child.is_some())
        };
        if born {
            tokio::time::timeout(CLEANUP_BUDGET, self.wait_exit(request))
                .await
                .map_err(|_| unknown("same Child native wait timed out"))??;
        }
        self.retire(request)
    }

    async fn shutdown(&self) -> Result<(), ValidationLifecycleError> {
        self.begin_shutdown()?;
        tokio::time::timeout(CLEANUP_BUDGET, self.shutdown_inner())
            .await
            .map_err(|_| unknown("validation drain budget expired"))?
    }

    async fn shutdown_inner(&self) -> Result<(), ValidationLifecycleError> {
        let _cleanup = self.cleanup_gate.lock().await;
        let requests: Vec<_> = self
            .state
            .lock()
            .map_err(unknown)?
            .runs
            .values()
            .map(|run| run.request.clone())
            .collect();
        self.drain_requests(requests).await?;
        if !self.state.lock().map_err(unknown)?.runs.is_empty() {
            return Err(unknown("validation births remain in custody"));
        }
        Ok(())
    }

    async fn settle_debts(&self) -> Result<(), ValidationLifecycleError> {
        tokio::time::timeout(CLEANUP_BUDGET, self.settle_debts_inner())
            .await
            .map_err(|_| unknown("validation cleanup budget expired"))?
    }

    async fn settle_debts_inner(&self) -> Result<(), ValidationLifecycleError> {
        let _cleanup = self.cleanup_gate.lock().await;
        let requests = {
            let mut state = self.state.lock().map_err(unknown)?;
            if state.closing {
                return Err(ValidationLifecycleError::Closing);
            }
            state.settling = true;
            state
                .runs
                .values()
                .filter(|run| run.debt)
                .map(|run| run.request.clone())
                .collect()
        };
        let booking = SettlementBooking(self);
        self.drain_requests(requests).await?;
        drop(booking);
        self.assert_admission()
    }

    async fn drain_requests(
        &self,
        requests: Vec<CheckRequestRef>,
    ) -> Result<(), ValidationLifecycleError> {
        let mut errors = Vec::new();
        // Request all closes before the first wait, even when one native wait is lost.
        for request in &requests {
            if let Err(error) = self.request_close(request) {
                errors.push(error.to_string());
            }
        }
        for request in requests {
            if let Err(error) = self.close_confirmed(&request).await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(unknown(errors.join("; ")))
        }
    }

    fn spawn(
        &self,
        binary: &Path,
        config: &Path,
        io: Arc<dyn CheckIo>,
    ) -> Result<Result<(CheckRequestRef, Arc<OutputCapture>), String>, ValidationLifecycleError>
    {
        // Closing, copy, synchronous spawn and publication share one admission lock.
        // No cancel point can expose a born but unregistered Child.
        let mut state = self.state.lock().map_err(unknown)?;
        Self::admit(&state)?;
        let id = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
        let request = CheckRequestRef {
            id,
            issuer: Arc::clone(&self.issuer),
            identity: Arc::new(CheckRequestIdentity),
        };
        let parent = config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = config.file_name().unwrap_or_default().to_string_lossy();
        let snapshot = parent.join(format!(".polaris-check-{}-{id}-{name}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut destination = match options.open(&snapshot) {
            Ok(file) => file,
            Err(error) => return Ok(Err(error.to_string())),
        };
        let run = CheckRun {
            request: request.clone(),
            factory: NativeFactoryObservation::PreFactory,
            native_exited: None,
            native_terminal: None,
            child: None,
            snapshot: snapshot.clone(),
            exit: None,
            debt: false,
            lost_wait_ownership: false,
            io,
        };
        // Retain the original admission before entering the synchronous factory.
        // A panic/unreturned factory cannot become no-child from an empty slot.
        state.runs.insert(id, run);
        let run = state.runs.get_mut(&id).expect("admitted validation run");
        let copied =
            File::open(config).and_then(|mut source| io::copy(&mut source, &mut destination));
        drop(destination);
        if let Err(error) = copied {
            run.debt = true;
            drop(state);
            self.retire(&request)?;
            return Ok(Err(error.to_string()));
        }
        let mut builder = tokio::process::Command::new(binary);
        builder
            .arg("--disable-color")
            .arg("check")
            .arg("-c")
            .arg(&snapshot)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        builder.creation_flags(0x0800_0000);
        run.factory = NativeFactoryObservation::Entered;
        let child = match builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                run.debt = true;
                run.factory = NativeFactoryObservation::ReturnedNoChild;
                drop(state);
                self.retire(&request)?;
                return Ok(Err(error.to_string()));
            }
        };
        run.child = Some(child);
        run.factory = NativeFactoryObservation::Attached(ValidationNativeMembers {
            request: request.clone(),
            birth: Arc::new(ValidationBirthIdentity),
            member: Arc::new(ValidationMemberIdentity),
            role: ValidationRole::Validation,
            scope: ValidationScope::SingleValidationNativeChildV1,
        });
        let child = run.child.as_mut().expect("attached validation Child");
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let output = Arc::new(OutputCapture::default());
        output.read(stdout, false);
        output.read(stderr, true);
        Ok(Ok((request, output)))
    }

    async fn run(
        self: &Arc<Self>,
        binary: &Path,
        config: &Path,
        timeout: Duration,
        io: Arc<dyn CheckIo>,
    ) -> Result<RawCheck, ValidationLifecycleError> {
        let (request, output) = match self.spawn(binary, config, io)? {
            Ok(started) => started,
            Err(error) => return Ok(RawCheck::SpawnFailed(error)),
        };
        let mut booking = CheckBooking {
            custody: Arc::clone(self),
            request: request.clone(),
            active: true,
        };
        let wait = async {
            let exit = self.wait_exit(&request).await?;
            output.finished().await;
            Ok::<_, ValidationLifecycleError>(output.result(exit.success()))
        };
        let raw = match tokio::time::timeout(timeout, wait).await {
            Ok(Ok(raw)) => {
                self.retire(&request)?;
                raw
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                self.close_confirmed(&request).await?;
                RawCheck::TimedOut {
                    after_secs: timeout.as_secs_f32(),
                }
            }
        };
        booking.active = false;
        Ok(raw)
    }
}

struct CheckBooking {
    custody: Arc<CheckCustody>,
    request: CheckRequestRef,
    active: bool,
}

struct SettlementBooking<'a>(&'a CheckCustody);
impl Drop for SettlementBooking<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.settling = false;
        }
    }
}
impl Drop for CheckBooking {
    fn drop(&mut self) {
        if self.active {
            let _ = self.custody.request_close(&self.request);
        }
    }
}

pub(super) async fn run_native_check(
    binary: &Path,
    config: &Path,
    timeout: Duration,
) -> Result<RawCheck, ValidationLifecycleError> {
    registry().settle_debts().await?;
    registry()
        .run(binary, config, timeout, Arc::new(NativeIo))
        .await
}

#[cfg(test)]
mod tests;

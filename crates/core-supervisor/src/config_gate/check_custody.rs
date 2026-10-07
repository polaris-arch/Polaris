//! Persistent custody for the repository's single native validation producer.
//! A cancelled caller never owns the only Child or its private config snapshot.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::future::{poll_fn, Future};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::task::JoinHandle;

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

/// Register an original producer under the same cutoff lock. This grants no
/// native birth: its sole factory still requires full `with_check_admission`.
/// The closure must not await or recursively enter validation custody.
pub fn with_check_producer_registration<T>(
    register: impl FnOnce() -> T,
) -> Result<T, ValidationLifecycleError> {
    let state = registry().state.lock().map_err(unknown)?;
    CheckCustody::admit_registration(&state)?;
    Ok(register())
}

pub fn assert_check_producer_registration() -> Result<(), ValidationLifecycleError> {
    with_check_producer_registration(|| ())
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
    tasks: Mutex<[CaptureTask; 2]>,
    join_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct CaptureTask {
    handle: Option<JoinHandle<Result<(), String>>>,
    completed: Option<Result<(), String>>,
}

impl OutputCapture {
    fn read<R: AsyncRead + Unpin + Send + 'static>(
        self: &Arc<Self>,
        pipe: Option<R>,
        stderr: bool,
    ) -> Result<(), ValidationLifecycleError> {
        let output = Arc::clone(self);
        let handle = tokio::spawn(async move {
            {
                let Some(mut pipe) = pipe else {
                    return Err("declared validation pipe is missing".into());
                };
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
            Ok(())
        });
        let mut tasks = self.tasks.lock().map_err(unknown)?;
        let task = &mut tasks[usize::from(stderr)];
        if task.handle.is_some() || task.completed.is_some() {
            return Err(unknown("validation reader was already registered"));
        }
        task.handle = Some(handle);
        Ok(())
    }

    async fn finished(&self) -> Result<(), ValidationLifecycleError> {
        // Concurrent run/Stop waiters must not overwrite a JoinHandle's waker.
        // Cancellation releases this gate without moving either owned handle.
        let _joining = self.join_gate.lock().await;
        poll_fn(|cx| {
            let mut tasks = match self.tasks.lock() {
                Ok(tasks) => tasks,
                Err(error) => return Poll::Ready(Err(unknown(error))),
            };
            let mut pending = false;
            for task in tasks.iter_mut() {
                if task.completed.is_none() {
                    let Some(handle) = task.handle.as_mut() else {
                        return Poll::Ready(Err(unknown("validation reader was not registered")));
                    };
                    match std::pin::Pin::new(handle).poll(cx) {
                        Poll::Ready(result) => {
                            task.completed =
                                Some(result.unwrap_or_else(|error| Err(error.to_string())));
                            task.handle = None;
                        }
                        Poll::Pending => pending = true,
                    }
                }
            }
            if pending {
                return Poll::Pending;
            }
            Poll::Ready(Self::validate_tasks(&tasks))
        })
        .await
    }

    fn validate_tasks(tasks: &[CaptureTask; 2]) -> Result<(), ValidationLifecycleError> {
        for task in tasks {
            match &task.completed {
                Some(Ok(())) => {}
                Some(Err(error)) => return Err(unknown(error)),
                None => return Err(unknown("validation reader task has not completed")),
            }
        }
        Ok(())
    }

    fn validate_finished(&self) -> Result<(), ValidationLifecycleError> {
        let tasks = self.tasks.lock().map_err(unknown)?;
        Self::validate_tasks(&tasks)
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

// The output policy comes from the concrete factory, never from absent pipes.
enum CheckTail {
    PathSnapshot {
        created: bool,
        path: PathBuf,
        output: Arc<OutputCapture>,
    },
    #[cfg(target_os = "linux")]
    OwnedSealedInputs {
        _binary: File,
        _config: File,
        _stdio: NullCheckStdio,
        command: Option<tokio::process::Command>,
    },
}

#[cfg(target_os = "linux")]
struct NullCheckStdio;

impl CheckTail {
    fn output(&self) -> Option<Arc<OutputCapture>> {
        match self {
            Self::PathSnapshot { output, .. } => Some(Arc::clone(output)),
            #[cfg(target_os = "linux")]
            Self::OwnedSealedInputs { .. } => None,
        }
    }

    fn validate_finished(&self) -> Result<(), ValidationLifecycleError> {
        match self {
            Self::PathSnapshot { output, .. } => output.validate_finished(),
            #[cfg(target_os = "linux")]
            Self::OwnedSealedInputs { .. } => Ok(()),
        }
    }

    fn retire(&self) -> io::Result<()> {
        match self {
            Self::PathSnapshot { created: false, .. } => Ok(()),
            Self::PathSnapshot { path, .. } => match std::fs::remove_file(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            },
            #[cfg(target_os = "linux")]
            Self::OwnedSealedInputs { .. } => Ok(()),
        }
    }

    #[cfg(all(test, unix))]
    fn path_snapshot(&self) -> &PathBuf {
        match self {
            Self::PathSnapshot { path, .. } => path,
            #[cfg(target_os = "linux")]
            Self::OwnedSealedInputs { .. } => panic!("sealed input has no path snapshot"),
        }
    }
}

#[derive(Default)]
struct CheckCompletion {
    terminal: AtomicBool,
    worker_pending: AtomicBool,
}

struct CheckRun {
    completion: Arc<CheckCompletion>,
    request: CheckRequestRef,
    factory: NativeFactoryObservation,
    native_exited: Option<ValidationNativeExited>,
    native_terminal: Option<LocalValidationNativeTerminal>,
    child: Option<Child>,
    tail: CheckTail,
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
                self.tail.validate_finished()?;
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
    pause: Option<Arc<()>>,
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
        Self::admit_registration(state)?;
        if state.settling || state.runs.values().any(|run| run.debt) {
            return Err(unknown("a previous validation birth retains cleanup debt"));
        }
        Ok(())
    }

    fn admit_registration(state: &CheckState) -> Result<(), ValidationLifecycleError> {
        if state.closing {
            return Err(ValidationLifecycleError::Closing);
        }
        if state.pause.is_some() {
            return Err(unknown("PC producers are paused"));
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

    async fn wait_output(&self, request: &CheckRequestRef) -> Result<(), ValidationLifecycleError> {
        let output = {
            let mut state = self.state.lock().map_err(unknown)?;
            self.run_mut(&mut state, request)?
                .and_then(|run| run.tail.output())
        };
        if let Some(output) = output {
            output.finished().await?;
        }
        Ok(())
    }

    fn retire(&self, request: &CheckRequestRef) -> Result<(), ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        let Some(run) = self.run_mut(&mut state, request)? else {
            return Ok(());
        };
        let validated = match run.validate_retirement() {
            Ok(validated) => validated,
            Err(error) => {
                run.debt = true;
                return Err(error);
            }
        };
        if let Err(error) = run.tail.retire() {
            run.debt = true;
            return Err(unknown(error));
        }
        // Snapshot removal above is the LAST fallible step. No new lookup, lock,
        // validation or allocation follows it; commit only this exact run.
        run.native_terminal = validated.map(|validated| LocalValidationNativeTerminal {
            _native_exit: validated.0,
        });
        let completion = Arc::clone(&run.completion);
        // Drop the exact owned images before publishing their local terminal.
        state.runs.remove(&request.id);
        completion.terminal.store(true, Ordering::SeqCst);
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
        tokio::time::timeout(CLEANUP_BUDGET, async {
            if born {
                self.wait_exit(request).await?;
                self.wait_output(request).await?;
            }
            Ok::<_, ValidationLifecycleError>(())
        })
        .await
        .map_err(|_| unknown("same Child native wait/output timed out"))??;
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
        let output = Arc::new(OutputCapture::default());
        let run = CheckRun {
            completion: Arc::new(CheckCompletion::default()),
            request: request.clone(),
            factory: NativeFactoryObservation::PreFactory,
            native_exited: None,
            native_terminal: None,
            child: None,
            tail: CheckTail::PathSnapshot {
                created: false,
                path: snapshot.clone(),
                output: Arc::clone(&output),
            },
            exit: None,
            debt: false,
            lost_wait_ownership: false,
            io,
        };
        // Retain the original admission before entering the synchronous factory.
        // A panic/unreturned factory cannot become no-child from an empty slot.
        state.runs.insert(id, run);
        let mut destination = match options.open(&snapshot) {
            Ok(file) => file,
            Err(error) => {
                drop(state);
                self.retire(&request)?;
                return Ok(Err(error.to_string()));
            }
        };
        let run = state.runs.get_mut(&id).expect("admitted validation run");
        match &mut run.tail {
            CheckTail::PathSnapshot { created, .. } => *created = true,
            #[cfg(target_os = "linux")]
            CheckTail::OwnedSealedInputs { .. } => {}
        }
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
        if let Err(error) = Self::attach_child(run, &mut builder) {
            drop(state);
            self.retire(&request)?;
            return Ok(Err(error));
        }
        let child = run.child.as_mut().expect("attached validation Child");
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_drain = output.read(stdout, false);
        let stderr_drain = output.read(stderr, true);
        if let Err(error) = stdout_drain.and(stderr_drain) {
            run.debt = true;
            return Err(error);
        }
        Ok(Ok((request, output)))
    }

    fn attach_child(
        run: &mut CheckRun,
        builder: &mut tokio::process::Command,
    ) -> Result<(), String> {
        run.factory = NativeFactoryObservation::Entered;
        let child = match builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                run.debt = true;
                run.factory = NativeFactoryObservation::ReturnedNoChild;
                return Err(error.to_string());
            }
        };
        run.child = Some(child);
        run.factory = NativeFactoryObservation::Attached(ValidationNativeMembers {
            request: run.request.clone(),
            birth: Arc::new(ValidationBirthIdentity),
            member: Arc::new(ValidationMemberIdentity),
            role: ValidationRole::Validation,
            scope: ValidationScope::SingleValidationNativeChildV1,
        });
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn queue_owned_sealed(
        &self,
        mut command: tokio::process::Command,
        binary: File,
        mut config: File,
        io: Arc<dyn CheckIo>,
    ) -> Result<Result<CheckRequestRef, String>, ValidationLifecycleError> {
        use std::io::{Seek, SeekFrom};
        use std::os::fd::AsRawFd;
        let mut state = self.state.lock().map_err(unknown)?;
        Self::admit(&state)?;
        // These checks establish owned immutable inputs, not a pinned execution
        // profile. The strict caller alone supplies its exact check argv and pin.
        let prepared = (|| -> io::Result<File> {
            validate_sealed_file(&binary)?;
            validate_sealed_file(&config)?;
            if binary.as_raw_fd() < 3
                || command.as_std().get_program()
                    != std::ffi::OsStr::new(&format!("/proc/self/fd/{}", binary.as_raw_fd()))
            {
                return Err(io::Error::other(
                    "owned executable does not match check command",
                ));
            }
            config.seek(SeekFrom::Start(0))?;
            rustix::io::fcntl_dupfd_cloexec(&config, 3)
                .map(File::from)
                .map_err(io::Error::from)
        })();
        let stdin = match prepared {
            Ok(stdin) => stdin,
            Err(error) => return Ok(Err(error.to_string())),
        };
        command
            .env_clear()
            .current_dir("/")
            .stdin(std::process::Stdio::from(stdin))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let id = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
        let request = CheckRequestRef {
            id,
            issuer: Arc::clone(&self.issuer),
            identity: Arc::new(CheckRequestIdentity),
        };
        state.runs.insert(
            id,
            CheckRun {
                completion: Arc::new(CheckCompletion {
                    terminal: AtomicBool::new(false),
                    worker_pending: AtomicBool::new(true),
                }),
                request: request.clone(),
                factory: NativeFactoryObservation::PreFactory,
                native_exited: None,
                native_terminal: None,
                child: None,
                tail: CheckTail::OwnedSealedInputs {
                    _binary: binary,
                    _config: config,
                    _stdio: NullCheckStdio,
                    command: Some(command),
                },
                exit: None,
                debt: false,
                lost_wait_ownership: false,
                io,
            },
        );
        Ok(Ok(request))
    }

    #[cfg(target_os = "linux")]
    fn dispatch_owned_sealed(
        &self,
        request: &CheckRequestRef,
    ) -> Result<Result<CheckRequestRef, String>, ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        if let Err(error) = Self::admit(&state) {
            drop(state);
            self.retire(request)?;
            return Err(error);
        }
        let run = self
            .run_mut(&mut state, request)?
            .ok_or_else(|| unknown("queued validation request disappeared"))?;
        if !matches!(run.factory, NativeFactoryObservation::PreFactory) {
            return Err(unknown("queued validation request was already dispatched"));
        }
        let mut command = match &mut run.tail {
            CheckTail::OwnedSealedInputs { command, .. } => command
                .take()
                .ok_or_else(|| unknown("queued check command already dispatched"))?,
            _ => return Err(unknown("queued request lacks original owned command")),
        };
        if let Err(error) = Self::attach_child(run, &mut command) {
            drop(command);
            drop(state);
            self.retire(request)?;
            return Ok(Err(error));
        }
        Ok(Ok(request.clone()))
    }

    #[cfg(all(target_os = "linux", test))]
    fn spawn_owned_sealed(
        &self,
        command: tokio::process::Command,
        binary: File,
        config: File,
        io: Arc<dyn CheckIo>,
    ) -> Result<Result<CheckRequestRef, String>, ValidationLifecycleError> {
        match self.queue_owned_sealed(command, binary, config, io)? {
            Ok(request) => {
                let completion =
                    Arc::clone(&self.state.lock().map_err(unknown)?.runs[&request.id].completion);
                let result = self.dispatch_owned_sealed(&request);
                completion.worker_pending.store(false, Ordering::SeqCst);
                result
            }
            Err(error) => Ok(Err(error)),
        }
    }

    #[cfg(target_os = "linux")]
    async fn supervise_owned_sealed(
        self: &Arc<Self>,
        command: tokio::process::Command,
        binary: File,
        config: File,
        timeout: Duration,
        spawned: Option<tokio::sync::oneshot::Sender<u32>>,
    ) -> Result<RawCheck, ValidationLifecycleError> {
        let request = match self.queue_owned_sealed(command, binary, config, Arc::new(NativeIo))? {
            Ok(queued) => queued,
            Err(error) => return Ok(RawCheck::SpawnFailed(error)),
        };
        let completion = {
            let mut state = self.state.lock().map_err(unknown)?;
            Arc::clone(
                &self
                    .run_mut(&mut state, &request)?
                    .ok_or_else(|| unknown("queued check missing"))?
                    .completion,
            )
        };
        let custody = Arc::clone(self);
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        // Only the coordinator is detached. The original registry owns the
        // Child and both images before the sole synchronous spawn can return.
        tokio::spawn(async move {
            let result = async {
                if let Err(error) = custody.settle_debts().await {
                    custody.retire(&request)?;
                    return Err(error);
                }
                if tx.is_closed() {
                    custody.retire(&request)?;
                    return Ok(RawCheck::SpawnFailed(
                        "check caller cancelled before birth".into(),
                    ));
                }
                let request = match custody.dispatch_owned_sealed(&request)? {
                    Ok(request) => request,
                    Err(error) => return Ok(RawCheck::SpawnFailed(error)),
                };
                let mut booking = CheckBooking {
                    custody: Arc::clone(&custody),
                    request: request.clone(),
                    active: true,
                };
                if let Some(notify) = spawned {
                    let mut state = custody.state.lock().map_err(unknown)?;
                    if let Some(pid) = custody
                        .run_mut(&mut state, &request)?
                        .and_then(|run| run.child.as_ref().and_then(Child::id))
                    {
                        let _ = notify.send(pid);
                    }
                }
                let raw = tokio::select! {
                    biased;
                    _ = tx.closed() => {
                        custody.close_confirmed(&request).await?;
                        RawCheck::SpawnFailed("check caller cancelled".into())
                    }
                    _ = tokio::time::sleep(timeout) => {
                        custody.close_confirmed(&request).await?;
                        RawCheck::TimedOut { after_secs: timeout.as_secs_f32() }
                    }
                    status = custody.wait_exit(&request) => {
                        let status = status?;
                        custody.retire(&request)?;
                        RawCheck::Done {
                            success: status.success(),
                            stdout: String::new(),
                            stderr: String::new(),
                        }
                    }
                };
                booking.active = false;
                Ok(raw)
            }
            .await;
            completion.worker_pending.store(false, Ordering::SeqCst);
            let _ = tx.send(result);
        });
        rx.await
            .map_err(|_| unknown("owned check coordinator did not return"))?
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
            output.finished().await?;
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

#[cfg(target_os = "linux")]
fn validate_sealed_file(file: &File) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, SealFlag};
    let seals = SealFlag::F_SEAL_WRITE
        | SealFlag::F_SEAL_GROW
        | SealFlag::F_SEAL_SHRINK
        | SealFlag::F_SEAL_SEAL;
    let actual = fcntl(file, FcntlArg::F_GET_SEALS).map_err(io::Error::from)?;
    let flags = fcntl(file, FcntlArg::F_GETFD).map_err(io::Error::from)?;
    if actual & seals.bits() != seals.bits() || flags & nix::libc::FD_CLOEXEC == 0 {
        return Err(io::Error::other(
            "owned check input protection is incomplete",
        ));
    }
    Ok(())
}

/// Retain a check's actual sealed executable/config in the existing validation
/// custody. This returns only diagnostic RawCheck and local lifecycle errors;
/// neither the command nor its optional PID notification grants a launch permit.
#[cfg(target_os = "linux")]
pub async fn supervise_owned_sealed_check(
    command: tokio::process::Command,
    binary: File,
    config: File,
    timeout: Duration,
    spawned: Option<tokio::sync::oneshot::Sender<u32>>,
) -> Result<RawCheck, ValidationLifecycleError> {
    registry()
        .supervise_owned_sealed(command, binary, config, timeout, spawned)
        .await
}

/// A same-issuer reversible admission cutoff. Drop deliberately does not reopen it.
pub struct AdmissionPause {
    issuer: Arc<ValidationIssuerIdentity>,
    identity: Arc<()>,
    members: Vec<(CheckRequestRef, Arc<CheckCompletion>)>,
}

/// Opaque references to original Check rows, including queued owned images.
pub struct CheckProducerView {
    custody: Arc<CheckCustody>,
    members: Vec<(CheckRequestRef, Arc<CheckCompletion>)>,
}

impl AdmissionPause {
    pub fn is_current(&self) -> Result<(), ValidationLifecycleError> {
        let custody = registry();
        let state = custody.state.lock().map_err(unknown)?;
        if Arc::ptr_eq(&custody.issuer, &self.issuer)
            && state
                .pause
                .as_ref()
                .is_some_and(|id| Arc::ptr_eq(id, &self.identity))
        {
            Ok(())
        } else {
            Err(unknown("pause does not belong to the current custody"))
        }
    }

    pub fn check_members(&self) -> Result<CheckProducerView, ValidationLifecycleError> {
        self.is_current()?;
        let custody = registry();
        Ok(CheckProducerView {
            custody: Arc::clone(custody),
            members: self.members.clone(),
        })
    }

    pub fn surrender(&self) -> Result<(), ValidationLifecycleError> {
        self.check_members()?.verify_original_custody()?;
        let custody = registry();
        let mut state = custody.state.lock().map_err(unknown)?;
        if !Arc::ptr_eq(&custody.issuer, &self.issuer)
            || !state
                .pause
                .as_ref()
                .is_some_and(|id| Arc::ptr_eq(id, &self.identity))
        {
            return Err(unknown("foreign or already surrendered pause"));
        }
        if state.closing {
            return Err(ValidationLifecycleError::Closing);
        }
        if state.settling {
            return Err(unknown("original Check settlement has not returned"));
        }
        state.pause = None;
        Ok(())
    }
}

impl CheckProducerView {
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    pub fn verify_original_custody(&self) -> Result<(), ValidationLifecycleError> {
        let state = self.custody.state.lock().map_err(unknown)?;
        for (request, completion) in &self.members {
            if completion.worker_pending.load(Ordering::SeqCst) {
                return Err(unknown("original Check coordinator has not returned"));
            }
            if let Some(run) = state.runs.get(&request.id) {
                if !run.request.same(request)
                    || !Arc::ptr_eq(&run.completion, completion)
                    || run.debt
                    || run.lost_wait_ownership
                    || matches!(run.factory, NativeFactoryObservation::Entered)
                {
                    return Err(unknown("original Check dispatch is unresolved"));
                }
            } else if !completion.terminal.load(Ordering::SeqCst) {
                return Err(unknown("Check row disappeared without original retirement"));
            }
        }
        Ok(())
    }
}

pub fn pause_check_producers() -> Result<AdmissionPause, ValidationLifecycleError> {
    let custody = registry();
    let mut state = custody.state.try_lock().map_err(unknown)?;
    if state.closing {
        return Err(ValidationLifecycleError::Closing);
    }
    if state.pause.is_some() {
        return Err(unknown("PC producers already paused"));
    }
    let identity = Arc::new(());
    let members = state
        .runs
        .values()
        .map(|run| (run.request.clone(), Arc::clone(&run.completion)))
        .collect();
    state.pause = Some(Arc::clone(&identity));
    Ok(AdmissionPause {
        issuer: Arc::clone(&custody.issuer),
        identity,
        members,
    })
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

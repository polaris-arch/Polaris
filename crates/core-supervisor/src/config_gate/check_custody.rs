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

struct CheckRun {
    child: Option<Child>,
    snapshot: PathBuf,
    exit: Option<ExitStatus>,
    debt: bool,
    lost_wait_ownership: bool,
    io: Arc<dyn CheckIo>,
}

impl CheckRun {
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
            return Poll::Ready(Ok(exit));
        }
        if self.lost_wait_ownership {
            return Poll::Ready(Err(io::Error::other("native wait ownership was lost")));
        }
        let Some(child) = self.child.as_mut() else {
            return Poll::Ready(Err(io::Error::other("validation has no born Child")));
        };
        match self.io.poll_wait(child, cx) {
            Poll::Ready(Ok(exit)) => {
                self.exit = Some(exit);
                Poll::Ready(Ok(exit))
            }
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
                self.exit = Some(exit);
                return Ok(());
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
    state: Mutex<CheckState>,
    cleanup_gate: tokio::sync::Mutex<()>,
}

impl CheckCustody {
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

    async fn wait_exit(&self, request: u64) -> Result<ExitStatus, ValidationLifecycleError> {
        poll_fn(|cx| {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(error) => return Poll::Ready(Err(unknown(error))),
            };
            let Some(run) = state.runs.get_mut(&request) else {
                return Poll::Ready(Err(unknown("validation birth was already retired")));
            };
            run.poll_exit(cx).map_err(unknown)
        })
        .await
    }

    fn request_close(&self, request: u64) -> Result<(), ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        let Some(run) = state.runs.get_mut(&request) else {
            return Ok(()); // the exact request has already retired; never address a replacement
        };
        run.request_close().map_err(unknown)
    }

    fn retire(&self, request: u64) -> Result<(), ValidationLifecycleError> {
        let mut state = self.state.lock().map_err(unknown)?;
        let Some(run) = state.runs.get_mut(&request) else {
            return Ok(());
        };
        if run.child.is_some() && run.exit.is_none() {
            return Err(unknown("same Child native exit has not been confirmed"));
        }
        match std::fs::remove_file(&run.snapshot) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                run.debt = true;
                return Err(unknown(error));
            }
        }
        state.runs.remove(&request);
        Ok(())
    }

    async fn close_confirmed(&self, request: u64) -> Result<(), ValidationLifecycleError> {
        self.request_close(request)?;
        let born = self
            .state
            .lock()
            .map_err(unknown)?
            .runs
            .get(&request)
            .is_some_and(|run| run.child.is_some());
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
            .keys()
            .copied()
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
                .iter()
                .filter(|(_, run)| run.debt)
                .map(|(id, _)| *id)
                .collect()
        };
        let booking = SettlementBooking(self);
        self.drain_requests(requests).await?;
        drop(booking);
        self.assert_admission()
    }

    async fn drain_requests(&self, requests: Vec<u64>) -> Result<(), ValidationLifecycleError> {
        let mut errors = Vec::new();
        // Request all closes before the first wait, even when one native wait is lost.
        for request in &requests {
            if let Err(error) = self.request_close(*request) {
                errors.push(error.to_string());
            }
        }
        for request in requests {
            if let Err(error) = self.close_confirmed(request).await {
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
    ) -> Result<Result<(u64, Arc<OutputCapture>), String>, ValidationLifecycleError> {
        // Closing, copy, synchronous spawn and publication share one admission lock.
        // No cancel point can expose a born but unregistered Child.
        let mut state = self.state.lock().map_err(unknown)?;
        Self::admit(&state)?;
        let request = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
        let parent = config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = config.file_name().unwrap_or_default().to_string_lossy();
        let snapshot = parent.join(format!(
            ".polaris-check-{}-{request}-{name}",
            std::process::id()
        ));
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
        let mut run = CheckRun {
            child: None,
            snapshot: snapshot.clone(),
            exit: None,
            debt: false,
            lost_wait_ownership: false,
            io,
        };
        let copied =
            File::open(config).and_then(|mut source| io::copy(&mut source, &mut destination));
        drop(destination);
        if let Err(error) = copied {
            run.debt = true;
            state.runs.insert(request, run);
            drop(state);
            self.retire(request)?;
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
        let mut child = match builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                run.debt = true;
                state.runs.insert(request, run);
                drop(state);
                self.retire(request)?;
                return Ok(Err(error.to_string()));
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        run.child = Some(child);
        state.runs.insert(request, run);
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
            request,
            active: true,
        };
        let wait = async {
            let exit = self.wait_exit(request).await?;
            output.finished().await;
            Ok::<_, ValidationLifecycleError>(output.result(exit.success()))
        };
        let raw = match tokio::time::timeout(timeout, wait).await {
            Ok(Ok(raw)) => {
                self.retire(request)?;
                raw
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                self.close_confirmed(request).await?;
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
    request: u64,
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
            let _ = self.custody.request_close(self.request);
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

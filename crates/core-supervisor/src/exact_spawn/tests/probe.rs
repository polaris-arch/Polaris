//! Private unit-test fixture custody. This module is absent from production.

use super::{digest, read_linux_binary_source, LinuxInputProfile, LinuxProtectedInputs};
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const FIXTURE: &str = "exact_spawn::tests::no_network_fixture";
const PREFIX_BYTES: usize = 512;
const CLEANUP_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeError {
    Unsupported,
    Spawn,
    CancelledBeforeSpawn,
    CleanupUncertain,
    DrainFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinishReason {
    Exited,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamSummary {
    pub bytes: u64,
    pub digest: [u8; 32],
    pub prefix: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(super) struct ProbeCompletion {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub reason: FinishReason,
    pub reaped: bool,
    pub cleanup_uncertain: bool,
    pub stdout: StreamSummary,
    pub stderr: StreamSummary,
    pub binary_digest: [u8; 32],
    pub config_digest: [u8; 32],
}

#[derive(Clone, Copy)]
pub(super) enum Fault {
    None,
    SpawnFailure,
    WaitFailureOnce,
}

/// Only this handle moves between callers. The actual Child is never sent.
pub(super) struct ProbeCustody {
    cancel: Option<oneshot::Sender<()>>,
    result: Option<oneshot::Receiver<Result<ProbeCompletion, ProbeError>>>,
    owner: Option<JoinHandle<Result<ProbeCompletion, ProbeError>>>,
}

impl Drop for ProbeCustody {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Cancelling a borrowed result future must notify the existing cleanup owner,
/// even when the caller keeps the custody handle alive.
struct CancelResultOnDrop<'a> {
    cancel: &'a mut Option<oneshot::Sender<()>>,
    armed: bool,
}

impl CancelResultOnDrop<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelResultOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Some(cancel) = self.cancel.take() {
                let _ = cancel.send(());
            }
        }
    }
}

impl ProbeCustody {
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }

    pub fn result(
        &mut self,
    ) -> impl std::future::Future<Output = Result<ProbeCompletion, ProbeError>> + '_ {
        // Arm before creating the async body, so dropping an unpolled borrowed
        // wait also requests cleanup while custody remains with the caller.
        let mut cancellation = CancelResultOnDrop {
            cancel: &mut self.cancel,
            armed: true,
        };
        let result_slot = &mut self.result;
        async move {
            let receiver = result_slot.as_mut().ok_or(ProbeError::CleanupUncertain)?;
            let result = receiver.await;
            cancellation.disarm();
            result_slot.take();
            result.map_err(|_| ProbeError::CleanupUncertain)?
        }
    }

    /// Explicitly settle the still-owned cleanup task even after an uncertain
    /// result. If this future is cancelled, Drop requests cleanup; owner stays.
    pub async fn settle(mut self) -> Result<ProbeCompletion, ProbeError> {
        self.owner
            .take()
            .ok_or(ProbeError::CleanupUncertain)?
            .await
            .map_err(|_| ProbeError::CleanupUncertain)?
    }
}

type Observation = oneshot::Receiver<Result<ProbeCompletion, ProbeError>>;

async fn drain(mut stream: impl AsyncRead + Unpin) -> Result<StreamSummary, ProbeError> {
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    let mut prefix = Vec::with_capacity(PREFIX_BYTES);
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| ProbeError::DrainFailed)?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or(ProbeError::DrainFailed)?;
        hash.update(&buffer[..count]);
        let retain = count.min(PREFIX_BYTES - prefix.len());
        prefix.extend_from_slice(&buffer[..retain]);
        // Beyond the memory budget we continue draining, never stop reading
        // a pipe and then wait for a child that may be blocked writing it.
    }
    Ok(StreamSummary {
        bytes,
        digest: hash.finalize().into(),
        prefix,
    })
}

fn report_uncertain(result: &mut Option<oneshot::Sender<Result<ProbeCompletion, ProbeError>>>) {
    if let Some(sender) = result.take() {
        let _ = sender.send(Err(ProbeError::CleanupUncertain));
    }
}

async fn reap_retaining(
    child: &mut Child,
    result: &mut Option<oneshot::Sender<Result<ProbeCompletion, ProbeError>>>,
    uncertain: &mut bool,
) -> ExitStatus {
    let _ = child.start_kill();
    loop {
        match tokio::time::timeout(CLEANUP_BUDGET, child.wait()).await {
            Ok(Ok(status)) => return status,
            // Retain child and protected images in this owner on both errors
            // and deadline. An early uncertain result does not detach custody.
            _ => {
                *uncertain = true;
                report_uncertain(result);
                let _ = child.start_kill();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// No Command/argv/env input, no external Child conversion. Binary identity
/// must be the current test image containing the exact ignored fixture below.
pub(super) fn spawn_no_network_probe(
    inputs: LinuxProtectedInputs,
    timeout: Duration,
    spawned: Option<oneshot::Sender<u32>>,
    fault: Fault,
) -> Result<(ProbeCustody, Observation), ProbeError> {
    if inputs.profile() != LinuxInputProfile::NoNetworkUnitFixtureV1 {
        return Err(ProbeError::Unsupported);
    }
    inputs
        .validate_protection(LinuxInputProfile::NoNetworkUnitFixtureV1)
        .map_err(|_| ProbeError::Unsupported)?;
    let current = std::env::current_exe().map_err(|_| ProbeError::Unsupported)?;
    if digest(&read_linux_binary_source(&current).map_err(|_| ProbeError::Unsupported)?)
        != inputs.binary_digest
    {
        return Err(ProbeError::Unsupported);
    }
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| ProbeError::Unsupported)?;
    let (cancel_tx, mut cancel_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();
    let (observed_tx, observed_rx) = oneshot::channel();
    // The task and cancellation receiver exist before any actual spawn.
    let owner = runtime.spawn(async move {
        let mut result_tx = Some(result_tx);
        let result = async {
            if !matches!(cancel_rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                return Err(ProbeError::CancelledBeforeSpawn);
            }
            let executable = if matches!(fault, Fault::SpawnFailure) {
                "/nonexistent/polaris-s4-probe".to_owned()
            } else { format!("/proc/self/fd/{}", inputs.binary.as_raw_fd()) };
            let stdin = rustix::io::fcntl_dupfd_cloexec(&inputs.config, 3)
                .map(std::fs::File::from).map_err(|_| ProbeError::Unsupported)?;
            let mut command = Command::new(executable);
            command.args(["--exact", FIXTURE, "--ignored", "--nocapture", "--test-threads=1"])
                .env_clear().current_dir("/").stdin(Stdio::from(stdin))
                .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| ProbeError::Spawn)?;
            let pid = child.id().unwrap_or(0);
            // Command's two piped handles exist after successful spawn. No
            // caller cancellation point occurs before their immediate transfer.
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            let mut out = tokio::spawn(drain(stdout));
            let mut err = tokio::spawn(drain(stderr));
            if let Some(sender) = spawned { let _ = sender.send(pid); }
            let mut uncertain = pid == 0;
            if uncertain { report_uncertain(&mut result_tx); }
            let (status, reason) = if matches!(fault, Fault::WaitFailureOnce) {
                // Explicit synthetic wait-error seam, while the actual Child
                // remains here until the real kill+wait below completes.
                uncertain = true; report_uncertain(&mut result_tx);
                (reap_retaining(&mut child, &mut result_tx, &mut uncertain).await, FinishReason::Cancelled)
            } else {
                tokio::select! {
                    biased;
                    _ = &mut cancel_rx => (reap_retaining(&mut child, &mut result_tx, &mut uncertain).await, FinishReason::Cancelled),
                    _ = tokio::time::sleep(timeout) => (reap_retaining(&mut child, &mut result_tx, &mut uncertain).await, FinishReason::TimedOut),
                    status = child.wait() => match status {
                        Ok(status) => (status, FinishReason::Exited),
                        Err(_) => {
                            uncertain = true; report_uncertain(&mut result_tx);
                            (reap_retaining(&mut child, &mut result_tx, &mut uncertain).await, FinishReason::Cancelled)
                        }
                    },
                }
            };
            // Retain both drain tasks on a deadline; do not abort them or
            // claim pipe completion merely because the process was reaped.
            let mut stdout = None;
            let mut stderr = None;
            let deadline = tokio::time::sleep(CLEANUP_BUDGET);
            tokio::pin!(deadline);
            let mut deadline_elapsed = false;
            while stdout.is_none() || stderr.is_none() {
                tokio::select! {
                    value = &mut out, if stdout.is_none() => stdout = Some(value),
                    value = &mut err, if stderr.is_none() => stderr = Some(value),
                    _ = &mut deadline, if !deadline_elapsed => {
                        deadline_elapsed = true; uncertain = true;
                        report_uncertain(&mut result_tx);
                    }
                }
            }
            let completion = ProbeCompletion {
                pid, exit_code:status.code(), reason, reaped:true, cleanup_uncertain:uncertain,
                stdout:stdout.expect("drain settled").map_err(|_| ProbeError::DrainFailed)??,
                stderr:stderr.expect("drain settled").map_err(|_| ProbeError::DrainFailed)??,
                binary_digest:inputs.binary_digest, config_digest:inputs.config_digest,
            };
            Ok(completion)
        }.await;
        if let Some(sender) = result_tx.take() { let _ = sender.send(result.clone()); }
        let _ = observed_tx.send(result.clone());
        result
    });
    Ok((
        ProbeCustody {
            cancel: Some(cancel_tx),
            result: Some(result_rx),
            owner: Some(owner),
        },
        observed_rx,
    ))
}

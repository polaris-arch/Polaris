//! Root Helper adapter within the original CheckCustody rows and cutoff lock.
//! A socket caller never owns the sole child, reader or private input.
use super::*;
use polaris_helper_proto::check_config::{
    CheckKey, CheckReply, CheckRequest, CheckStatus, OutputStream, CHUNK_BYTES, MAX_CONFIG_BYTES,
    OUTPUT_BYTES,
};
use polaris_helper_proto::core_manifest::CoreId;
use polaris_helper_proto::core_protocol::CallerIdentity;
use polaris_helper_proto::HelperBirthToken;
use std::pin::Pin;
use std::time::Instant;
#[cfg(target_os = "macos")]
type SystemCheckClient = polaris_helper_client::check_config::SystemMacCheckClient;
#[cfg(windows)]
type SystemCheckClient = polaris_helper_client::check_config::SystemWindowsCheckClient;

pub(super) fn is_remote(
    custody: &CheckCustody,
    request: &CheckRequestRef,
) -> Result<bool, ValidationLifecycleError> {
    let mut s = custody.state.lock().map_err(unknown)?;
    Ok(custody
        .run_mut(&mut s, request)?
        .is_some_and(|r| matches!(r.tail, CheckTail::MacRemote { .. })))
}

/// App-side original producer responsibility. No local native exit/issuer is
/// invented for a remote child. Only the closed, OS-root-authenticated system
/// transport can finish it. A generic same-key wire reply is not an issuer.
///
/// ```compile_fail
/// use polaris_helper_client::HelperClient;
/// use polaris_helper_proto::core_manifest::CoreId;
/// fn spoofed_done_and_released_cannot_retire(client: HelperClient, core: CoreId) {
///     let _ = polaris_core_supervisor::config_gate::run_macos_helper_check(
///         client, core, b"{}".to_vec());
/// }
/// ```
pub async fn run_macos_helper_check(
    client: SystemCheckClient,
    core: CoreId,
    config: Vec<u8>,
) -> Result<RawCheck, ValidationLifecycleError> {
    use sha2::Digest;
    if config.is_empty() || config.len() > MAX_CONFIG_BYTES {
        return Err(unknown("CheckConfig captured input must be 1..=8MiB"));
    }
    let digest: [u8; 32] = sha2::Sha256::digest(&config).into();
    let custody = registry();
    let cancel = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let completion = Arc::new(CheckCompletion {
        terminal: AtomicBool::new(false),
        worker_pending: AtomicBool::new(true),
    });
    let request = {
        let mut s = custody.state.lock().map_err(unknown)?;
        CheckCustody::admit(&s)?;
        let id = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
        let request = CheckRequestRef {
            id,
            issuer: Arc::clone(&custody.issuer),
            identity: Arc::new(CheckRequestIdentity),
        };
        s.runs.insert(
            id,
            CheckRun {
                mac: None,
                mac_child: None,
                payload: None,
                lease_owner: None,
                completion: Arc::clone(&completion),
                request: request.clone(),
                factory: NativeFactoryObservation::Entered,
                native_exited: None,
                native_terminal: None,
                child: None,
                tail: CheckTail::MacRemote {
                    cancel: Arc::clone(&cancel),
                    unknown: Arc::clone(&failed),
                },
                exit: None,
                debt: true,
                lost_wait_ownership: false,
                io: Arc::new(NativeIo),
            },
        );
        request
    };
    let mut booking = CheckBooking {
        custody: Arc::clone(custody),
        request: request.clone(),
        active: true,
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = Arc::clone(custody);
    tokio::task::spawn_blocking(move || {
        // This worker remains responsible after the async receiver is dropped.
        struct Returned(Arc<CheckCompletion>);
        impl Drop for Returned {
            fn drop(&mut self) {
                self.0.worker_pending.store(false, Ordering::SeqCst);
            }
        }
        let _returned = Returned(completion);
        let result = client.run_check(core, &config, digest, &cancel);
        let result = match result {
            Ok(out) => {
                let raw = match out.status {
                    CheckStatus::Done | CheckStatus::Rejected => RawCheck::Done {
                        success: out.status == CheckStatus::Done,
                        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    },
                    CheckStatus::TimedOut => RawCheck::TimedOut {
                        after_secs: super::super::CONFIG_CHECK_TIMEOUT.as_secs_f32(),
                    },
                    CheckStatus::OutputFailed => RawCheck::OutputFailed(
                        "Helper CheckConfig retained output failed (excess bytes drained)".into(),
                    ),
                    CheckStatus::Cancelled => RawCheck::SpawnFailed(
                        "Helper CheckConfig caller cancelled after confirmed cleanup".into(),
                    ),
                    CheckStatus::SpawnFailed => RawCheck::SpawnFailed(
                        "Helper CheckConfig closed factory refused birth".into(),
                    ),
                    _ => {
                        failed.store(true, Ordering::SeqCst);
                        let _ = tx.send(Err(unknown("Helper CheckConfig terminal type differs")));
                        return;
                    }
                };
                (|| {
                    {
                        let mut s = worker.state.lock().map_err(unknown)?;
                        let run = worker
                            .run_mut(&mut s, &request)?
                            .ok_or_else(|| unknown("remote Check producer disappeared"))?;
                        run.factory = NativeFactoryObservation::RemoteHelperReturned;
                    }
                    worker.retire(&request)?;
                    Ok(raw)
                })()
            }
            Err(polaris_helper_client::check_config::HelperCheckFailure::NotAdmitted(error)) => {
                // This concrete client observed refusal before Begin admission;
                // no remote native members or cleanup receipt are invented.
                (|| {
                    {
                        let mut s = worker.state.lock().map_err(unknown)?;
                        let run = worker
                            .run_mut(&mut s, &request)?
                            .ok_or_else(|| unknown("original remote producer disappeared"))?;
                        run.factory = NativeFactoryObservation::RemoteHelperReturned;
                    }
                    worker.retire(&request)?;
                    Err(ValidationLifecycleError::Admission(AdmissionError::new(
                        crate::admitted_payload::AdmissionErrorKind::UnsupportedCapability,
                        error,
                    )))
                })()
            }
            Err(polaris_helper_client::check_config::HelperCheckFailure::Retired(error)) => {
                // A failed exchange is still a failure, but this concrete worker
                // subsequently received the exact terminal Release acknowledgement.
                (|| {
                    {
                        let mut s = worker.state.lock().map_err(unknown)?;
                        let run = worker
                            .run_mut(&mut s, &request)?
                            .ok_or_else(|| unknown("original remote producer disappeared"))?;
                        run.factory = NativeFactoryObservation::RemoteHelperReturned;
                    }
                    worker.retire(&request)?;
                    Ok(RawCheck::SpawnFailed(error))
                })()
            }
            Err(error) => {
                failed.store(true, Ordering::SeqCst);
                Err(unknown(error))
            }
        };
        if result.is_err()
            && !worker
                .state
                .lock()
                .is_ok_and(|s| !s.runs.contains_key(&request.id))
        {
            failed.store(true, Ordering::SeqCst);
        }
        let _ = tx.send(result);
    });
    let result = rx
        .await
        .map_err(|_| unknown("remote Check coordinator lost; original registry retained"))?;
    if result.is_ok() {
        booking.active = false;
    }
    result
}

/// The normal Mac daemon exit loop drives close through the same original rows.
/// Returning false/Unknown keeps the daemon alive; it is never a kill ACK.
pub fn request_macos_checks_shutdown() -> Result<bool, ValidationLifecycleError> {
    let custody = registry();
    let mut s = custody.state.lock().map_err(unknown)?;
    s.closing = true;
    for run in s.runs.values_mut() {
        run.request_close().map_err(unknown)?;
    }
    Ok(s.runs.is_empty())
}

pub(super) struct MacCheck {
    uid: CallerIdentity,
    key: CheckKey,
    bytes: usize,
    digest: [u8; 32],
    received: usize,
    cancelled: bool,
    result: CheckStatus,
    began: Instant,
    tick: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl MacCheck {
    // Cancellation intent alone cannot retire the original row or its child.
    pub(super) fn request_cancel(&mut self) {
        self.cancelled = true;
    }

    pub(super) fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<ExitStatus>> {
        let tick = self
            .tick
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_millis(10))));
        if tick.as_mut().poll(cx).is_ready() {
            tick.as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_millis(10));
            let _ = tick.as_mut().poll(cx);
        }
        Poll::Pending
    }
}
pub(super) struct MacTerminal {
    uid: CallerIdentity,
    key: CheckKey,
    status: CheckStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    delivered: [usize; 2],
    abandoned: bool,
}

fn meta(run: &CheckRun) -> Result<&MacCheck, ValidationLifecycleError> {
    run.mac
        .as_ref()
        .ok_or_else(|| unknown("not a Root Check request"))
}
fn output(run: &CheckRun) -> Result<Arc<OutputCapture>, ValidationLifecycleError> {
    run.tail
        .output()
        .ok_or_else(|| unknown("Root Check output missing"))
}

pub(super) fn terminal_before_retire(
    run: &CheckRun,
) -> Result<Option<MacTerminal>, ValidationLifecycleError> {
    let Some(m) = run.mac.as_ref() else {
        return Ok(None);
    };
    let out = output(run)?;
    let stdout = out.stdout.lock().map_err(unknown)?.clone();
    let stderr = out.stderr.lock().map_err(unknown)?.clone();
    let status = if m.result.terminal() {
        m.result
    } else if m.cancelled {
        CheckStatus::Cancelled
    } else if !out.errors.lock().map_err(unknown)?.is_empty() {
        CheckStatus::OutputFailed
    } else {
        match run.exit {
            Some(exit) if exit.success() => CheckStatus::Done,
            Some(_) => CheckStatus::Rejected,
            None => return Err(unknown("Root Check has no terminal result")),
        }
    };
    Ok(Some(MacTerminal {
        uid: m.uid.clone(),
        key: m.key.clone(),
        status,
        stdout,
        stderr,
        delivered: [0; 2],
        abandoned: m.cancelled,
    }))
}

impl OutputCapture {
    fn read_root<R: std::io::Read + Send + 'static>(
        self: &Arc<Self>,
        pipe: Option<R>,
        stderr: bool,
    ) -> Result<(), ValidationLifecycleError> {
        let slot = usize::from(stderr);
        let mut tasks = self.tasks.lock().map_err(unknown)?;
        if tasks[slot].handle.is_some()
            || tasks[slot].blocking.is_some()
            || tasks[slot].completed.is_some()
        {
            return Err(unknown("Root Check reader already registered"));
        }
        let out = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name("helper-check-output".into())
            .spawn(move || {
                let Some(mut pipe) = pipe else {
                    return Err("Root Check pipe missing".into());
                };
                let mut kept = Vec::new();
                let mut buffer = [0; 8192];
                let mut overflow = false;
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let remaining = OUTPUT_BYTES.saturating_sub(kept.len());
                            kept.extend_from_slice(&buffer[..n.min(remaining)]);
                            overflow |= n > remaining;
                        }
                        Err(e) => {
                            out.errors
                                .lock()
                                .map_err(|_| "output errors poisoned")?
                                .push(e.to_string());
                            break;
                        }
                    }
                }
                if overflow {
                    out.errors
                        .lock()
                        .map_err(|_| "output errors poisoned")?
                        .push(
                        "validation output exceeds retained-byte budget (remaining bytes drained)"
                            .into(),
                    );
                }
                let target = if stderr { &out.stderr } else { &out.stdout };
                *target.lock().map_err(|_| "output bytes poisoned")? = kept;
                Ok(())
            });
        match handle {
            Ok(h) => tasks[slot].blocking = Some(h),
            Err(e) => {
                // The closure/pipe was dropped without starting a thread. There is
                // no unjoined reader, but its output is unavailable, never accepted.
                self.errors
                    .lock()
                    .map_err(unknown)?
                    .push(format!("Root Check reader could not start: {e}"));
                tasks[slot].completed = Some(Ok(()));
            }
        }
        Ok(())
    }
}

/// Only a prepared exact CheckConfig owner can enter the existing registry.
/// `uid` comes from kernel peer credentials at the authenticated Mac dispatcher.
fn begin_helper_check(
    execution: PreparedExecution,
    lease: &File,
    uid: CallerIdentity,
    core: CoreId,
    bytes: usize,
    digest: [u8; 32],
    correlation: HelperBirthToken,
) -> Result<CheckReply, ValidationLifecycleError> {
    if bytes == 0
        || bytes > MAX_CONFIG_BYTES
        || execution.purpose() != ExecPurpose::CheckConfig
        || execution.payload().id() != &core
    {
        return Err(unknown("Root Check bounded exact selection required"));
    }
    let custody = registry();
    let mut state = custody.state.lock().map_err(unknown)?;
    CheckCustody::admit(&state)?;
    if !state.runs.is_empty() || state.mac_result.is_some() {
        return Err(unknown("validation request/result slot occupied"));
    }
    let payload = execution.payload();
    #[cfg(target_os = "macos")]
    let input = execution.macos_check_input(lease)?;
    #[cfg(windows)]
    let input = execution.windows_check_input(lease)?;
    let id = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
    let request = CheckRequestRef {
        id,
        issuer: Arc::clone(&custody.issuer),
        identity: Arc::new(CheckRequestIdentity),
    };
    let key = CheckKey { core, correlation };
    let output = Arc::new(OutputCapture {
        max_bytes: Some(OUTPUT_BYTES),
        ..OutputCapture::default()
    });
    let completion = Arc::new(CheckCompletion {
        terminal: AtomicBool::new(false),
        worker_pending: AtomicBool::new(true),
    });
    state.runs.insert(
        id,
        CheckRun {
            mac: Some(MacCheck {
                uid: uid.clone(),
                key: key.clone(),
                bytes,
                digest,
                received: 0,
                cancelled: false,
                result: CheckStatus::Cancelled,
                began: Instant::now(),
                tick: None,
            }),
            mac_child: None,
            payload: Some(payload),
            lease_owner: None,
            completion: Arc::clone(&completion),
            request: request.clone(),
            factory: NativeFactoryObservation::PreFactory,
            native_exited: None,
            native_terminal: None,
            child: None,
            tail: CheckTail::MacRoot {
                input: Box::new(input),
                output,
            },
            exit: None,
            debt: true,
            lost_wait_ownership: false,
            io: Arc::new(NativeIo),
        },
    );
    drop(state);
    let owned = Arc::clone(custody);
    let worker_request = request.clone();
    let worker_completion = Arc::clone(&completion);
    if let Err(e) = std::thread::Builder::new()
        .name("helper-check-custody".into())
        .spawn(move || {
            struct Returned(Arc<CheckCompletion>);
            impl Drop for Returned {
                fn drop(&mut self) {
                    self.0.worker_pending.store(false, Ordering::SeqCst);
                }
            }
            let _returned = Returned(worker_completion);
            match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => {
                    if let Err(error) = runtime.block_on(owned.drive_root_check(&worker_request)) {
                        if let Ok(mut s) = owned.state.lock() {
                            if let Ok(Some(run)) = owned.run_mut(&mut s, &worker_request) {
                                if let Some(m) = run.mac.as_mut() {
                                    m.result = CheckStatus::Unknown;
                                }
                                run.debt = true;
                            }
                        }
                        log::error!("Root Check custody retained: {error}");
                    }
                }
                Err(error) => {
                    if let Ok(mut s) = owned.state.lock() {
                        if let Ok(Some(run)) = owned.run_mut(&mut s, &worker_request) {
                            if let Some(m) = run.mac.as_mut() {
                                m.result = CheckStatus::SpawnFailed;
                            }
                        }
                    }
                    if let Err(cleanup) = owned.retire(&worker_request) {
                        log::error!(
                            "Root Check runtime unavailable ({error}); cleanup retained: {cleanup}"
                        );
                    }
                }
            }
        })
    {
        completion.worker_pending.store(false, Ordering::SeqCst);
        custody.retire(&request)?;
        // A known no-child failure can release only this terminal result slot.
        let mut s = custody.state.lock().map_err(unknown)?;
        if s.mac_result
            .as_ref()
            .is_some_and(|r| r.key == key && r.uid == uid)
        {
            s.mac_result = None;
        }
        return Err(unknown(e));
    }
    Ok(CheckReply::Begun { key })
}

impl CheckCustody {
    async fn drive_root_check(
        &self,
        request: &CheckRequestRef,
    ) -> Result<(), ValidationLifecycleError> {
        loop {
            let (born, cancel, expired) = {
                let mut s = self.state.lock().map_err(unknown)?;
                let closing = s.closing;
                let Some(run) = self.run_mut(&mut s, request)? else {
                    return Ok(());
                };
                let m = meta(run)?;
                (
                    run.has_child(),
                    m.cancelled || closing,
                    m.began.elapsed() >= super::super::CONFIG_CHECK_TIMEOUT,
                )
            };
            if cancel || expired {
                {
                    let mut s = self.state.lock().map_err(unknown)?;
                    if let Some(run) = self.run_mut(&mut s, request)? {
                        let m = run.mac.as_mut().expect("Root metadata");
                        if m.result != CheckStatus::SpawnFailed {
                            m.result = if expired {
                                CheckStatus::TimedOut
                            } else {
                                CheckStatus::Cancelled
                            };
                        }
                    }
                }
                return self.close_confirmed(request).await;
            }
            if born {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let remaining = {
            let mut s = self.state.lock().map_err(unknown)?;
            let run = self
                .run_mut(&mut s, request)?
                .ok_or_else(|| unknown("Root Check row missing"))?;
            super::super::CONFIG_CHECK_TIMEOUT.saturating_sub(meta(run)?.began.elapsed())
        };
        let result = tokio::time::timeout(remaining, async {
            let status = self.wait_exit(request).await?;
            self.wait_output(request).await?;
            Ok::<_, ValidationLifecycleError>(status)
        })
        .await;
        match result {
            Ok(Ok(status)) => {
                let mut s = self.state.lock().map_err(unknown)?;
                let Some(run) = self.run_mut(&mut s, request)? else {
                    return Ok(());
                };
                let failed = !output(run)?.errors.lock().map_err(unknown)?.is_empty();
                let m = run.mac.as_mut().expect("Root metadata");
                m.result = if m.cancelled {
                    CheckStatus::Cancelled
                } else if failed {
                    CheckStatus::OutputFailed
                } else if status.success() {
                    CheckStatus::Done
                } else {
                    CheckStatus::Rejected
                };
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                {
                    let mut s = self.state.lock().map_err(unknown)?;
                    if let Some(run) = self.run_mut(&mut s, request)? {
                        run.mac.as_mut().expect("Root metadata").result = CheckStatus::TimedOut;
                    }
                }
                return self.close_confirmed(request).await;
            }
        }
        self.retire(request)
    }
}

fn status_reply(run: &CheckRun) -> Result<CheckReply, ValidationLifecycleError> {
    let m = meta(run)?;
    Ok(CheckReply::Status {
        key: m.key.clone(),
        status: if run.lost_wait_ownership || m.result == CheckStatus::Unknown {
            CheckStatus::Unknown
        } else if run.has_child() {
            CheckStatus::Running
        } else if matches!(run.factory, NativeFactoryObservation::PreFactory) && !m.cancelled {
            CheckStatus::Receiving
        } else {
            CheckStatus::Unknown
        },
        stdout: 0,
        stderr: 0,
    })
}

/// Every operation checks the kernel UID, exact core, correlation and current phase.
fn helper_check_request(
    uid: CallerIdentity,
    request: &CheckRequest,
) -> Result<CheckReply, ValidationLifecycleError> {
    let key = request
        .key()
        .ok_or_else(|| unknown("Begin requires the closed prepared factory"))?;
    let custody = registry();
    let mut s = custody.state.lock().map_err(unknown)?;
    if let Some(t) = s
        .mac_result
        .as_mut()
        .filter(|t| t.uid == uid && t.key == *key)
    {
        return match request {
            CheckRequest::Status { .. } | CheckRequest::Cancel { .. } => {
                if matches!(request, CheckRequest::Cancel { .. }) {
                    t.abandoned = true;
                }
                Ok(CheckReply::Status {
                    key: key.clone(),
                    status: t.status,
                    stdout: t.stdout.len(),
                    stderr: t.stderr.len(),
                })
            }
            CheckRequest::ReadOutput { stream, offset, .. } => {
                let slot = usize::from(*stream == OutputStream::Stderr);
                if *offset != t.delivered[slot] {
                    return Err(unknown("Root Check output offset/replay differs"));
                }
                let bytes = match stream {
                    OutputStream::Stdout => &t.stdout,
                    OutputStream::Stderr => &t.stderr,
                };
                let data = bytes
                    .get(*offset..)
                    .ok_or_else(|| unknown("Root Check output offset differs"))?;
                if data.is_empty() {
                    return Err(unknown(
                        "Root Check output has no next chunk; replay denied",
                    ));
                }
                let bytes = data[..data.len().min(CHUNK_BYTES)].to_vec();
                t.delivered[slot] += bytes.len();
                Ok(CheckReply::Output {
                    key: key.clone(),
                    stream: *stream,
                    offset: *offset,
                    bytes,
                })
            }
            CheckRequest::Release { .. } => {
                if !t.abandoned && t.delivered != [t.stdout.len(), t.stderr.len()] {
                    return Err(unknown("Root Check output must be delivered or explicitly abandoned before Release"));
                }
                s.mac_result = None;
                Ok(CheckReply::Released { key: key.clone() })
            }
            _ => Err(unknown("Root Check input already consumed; replay denied")),
        };
    }
    let closing = s.closing || s.pause.is_some();
    let run = s
        .runs
        .values_mut()
        .find(|r| {
            r.mac
                .as_ref()
                .is_some_and(|m| m.uid == uid && m.key == *key)
        })
        .ok_or_else(|| unknown("Root Check owner/core/request differs"))?;
    match request {
        CheckRequest::Status { .. } => status_reply(run),
        CheckRequest::Cancel { .. } => {
            run.request_close().map_err(unknown)?;
            status_reply(run)
        }
        CheckRequest::Append { offset, bytes, .. } => {
            let m = run.mac.as_mut().expect("Root metadata");
            if closing
                || m.cancelled
                || m.began.elapsed() >= super::super::CONFIG_CHECK_TIMEOUT
                || !matches!(run.factory, NativeFactoryObservation::PreFactory)
                || bytes.is_empty()
                || bytes.len() > CHUNK_BYTES
                || *offset != m.received
                || m.received
                    .checked_add(bytes.len())
                    .is_none_or(|n| n > m.bytes)
            {
                return Err(unknown("Root Check input phase/offset/length differs"));
            }
            let CheckTail::MacRoot { input, .. } = &mut run.tail else {
                return Err(unknown("Root Check input owner missing"));
            };
            if let Err(error) = input.append(*offset, bytes) {
                m.cancelled = true;
                return Err(error.into());
            }
            m.received += bytes.len();
            Ok(CheckReply::Appended {
                key: key.clone(),
                received: m.received,
            })
        }
        CheckRequest::Commit { .. } => {
            let m = run.mac.as_mut().expect("Root metadata");
            if closing
                || m.cancelled
                || m.began.elapsed() >= super::super::CONFIG_CHECK_TIMEOUT
                || m.received != m.bytes
                || !matches!(run.factory, NativeFactoryObservation::PreFactory)
            {
                return Err(unknown("Root Check commit phase/length differs"));
            }
            let CheckTail::MacRoot { input, output } = &mut run.tail else {
                return Err(unknown("Root Check input owner missing"));
            };
            if let Err(error) = input.seal(m.bytes, &m.digest) {
                m.cancelled = true;
                return Err(error.into());
            }
            run.factory = NativeFactoryObservation::Entered;
            let child = input.spawn();
            match child {
                Ok(child) => run.mac_child = Some(child),
                Err(error) => {
                    run.factory = NativeFactoryObservation::ReturnedNoChild;
                    m.result = CheckStatus::SpawnFailed;
                    m.cancelled = true;
                    return Err(error.into());
                }
            }
            run.factory = NativeFactoryObservation::Attached(ValidationNativeMembers {
                request: run.request.clone(),
                birth: Arc::new(ValidationBirthIdentity),
                member: Arc::new(ValidationMemberIdentity),
                role: ValidationRole::Validation,
                scope: ValidationScope::SingleValidationNativeChildV1,
            });
            let child = run.mac_child.as_mut().expect("opaque Root child attached");
            let stdout = child.take_stdout();
            let stderr = child.take_stderr();
            // Both original readers are handed off before this request returns.
            let a = output.read_root(stdout, false);
            let b = output.read_root(stderr, true);
            a.and(b)?;
            m.began = Instant::now();
            m.result = CheckStatus::Running;
            status_reply(run)
        }
        CheckRequest::Release { .. } => Err(unknown(
            "Root Check actual wait/streams/input retirement unconfirmed",
        )),
        CheckRequest::ReadOutput { .. } => Err(unknown("Root Check output is not terminal")),
        CheckRequest::Begin { .. } => Err(unknown("invalid Root Check phase")),
    }
}

#[cfg(target_os = "macos")]
pub fn begin_macos_helper_check(
    execution: PreparedExecution,
    lease: &File,
    uid: u32,
    core: CoreId,
    bytes: usize,
    digest: [u8; 32],
    correlation: HelperBirthToken,
) -> Result<CheckReply, ValidationLifecycleError> {
    begin_helper_check(
        execution,
        lease,
        CallerIdentity::Unix(uid),
        core,
        bytes,
        digest,
        correlation,
    )
}
#[cfg(target_os = "macos")]
pub fn macos_helper_check_request(
    uid: u32,
    request: &CheckRequest,
) -> Result<CheckReply, ValidationLifecycleError> {
    helper_check_request(CallerIdentity::Unix(uid), request)
}
#[cfg(windows)]
pub fn begin_windows_helper_check(
    execution: PreparedExecution,
    lease: &File,
    owner: CallerIdentity,
    core: CoreId,
    bytes: usize,
    digest: [u8; 32],
) -> Result<CheckReply, ValidationLifecycleError> {
    if !matches!(owner, CallerIdentity::Windows(_)) {
        return Err(unknown("Windows Check requires actual peer SID"));
    }
    let correlation = crate::admitted_payload::windows_check_correlation()?;
    begin_helper_check(execution, lease, owner, core, bytes, digest, correlation)
}
#[cfg(windows)]
pub fn windows_helper_check_request(
    owner: CallerIdentity,
    request: &CheckRequest,
) -> Result<CheckReply, ValidationLifecycleError> {
    if !matches!(owner, CallerIdentity::Windows(_)) {
        return Err(unknown("Windows Check requires actual peer SID"));
    }
    helper_check_request(owner, request)
}
#[cfg(all(test, target_os = "macos"))]
mod tests;

#[cfg(all(test, windows))]
mod windows_negative_tests;

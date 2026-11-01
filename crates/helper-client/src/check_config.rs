//! Bounded CheckConfig exchange. Transport errors never become ordinary exec.
use crate::HelperClient;
use polaris_helper_proto::check_config::{
    CheckKey, CheckReply, CheckRequest, CheckStatus, OutputStream, CHUNK_BYTES, MAX_CONFIG_BYTES,
    OUTPUT_BYTES,
};
use polaris_helper_proto::core_manifest::CoreId;
use polaris_helper_proto::{Platform, Request, Response, ResponseKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct HelperCheckOutput {
    pub status: CheckStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum HelperCheckFailure {
    #[error("Helper CheckConfig not admitted: {0}")]
    NotAdmitted(String),
    #[error("Helper CheckConfig failed after confirmed retirement: {0}")]
    Retired(String),
    #[error("Helper CheckConfig cleanup unconfirmed: {0}")]
    Unconfirmed(String),
}

/// The only production remote Check completion source. Each connection goes to
/// the fixed installed Mac endpoint and authenticates its actual kernel root peer
/// before sending bytes. Generic wire clients/streams cannot be converted into it.
///
/// ```compile_fail
/// use polaris_helper_client::HelperClient;
/// use polaris_helper_client::check_config::SystemMacCheckClient;
/// fn spoofed_same_key_wire_client_is_not_an_issuer(client: HelperClient) {
///     // Even a connector forwarding real Begin before fabricating Released is
///     // only a generic wire client; there is no production conversion.
///     let _: SystemMacCheckClient = client;
/// }
/// ```
#[cfg(target_os = "macos")]
pub struct SystemMacCheckClient {
    client: HelperClient,
}

#[cfg(target_os = "macos")]
impl SystemMacCheckClient {
    /// Lazy construction only. Token is authentication data, not a completion
    /// issuer; path/transport/peer identity are never supplied by the caller.
    pub fn system(token: String) -> Result<Self, String> {
        if token.is_empty()
            || token.len() > polaris_helper_proto::command::mac::MAX_WIRE_LINE_BYTES
            || token.contains(['\r', '\n'])
        {
            return Err("Root Helper token is not one bounded nonempty line".into());
        }
        Ok(Self {
            client: HelperClient::new(
                Box::new(crate::connector::MacRootConnector),
                Platform::Mac,
                token,
            ),
        })
    }

    pub fn run_macos_check(
        &self,
        core: CoreId,
        config: &[u8],
        sha256: [u8; 32],
        cancel: &AtomicBool,
    ) -> Result<HelperCheckOutput, HelperCheckFailure> {
        self.client.run_macos_check(core, config, sha256, cancel)
    }
}

impl HelperClient {
    /// One typed operation, without replaying a timeout/partially written request.
    pub fn send_macos_check(
        &self,
        request: CheckRequest,
        timeout: Duration,
    ) -> Result<CheckReply, String> {
        let valid = match &request {
            CheckRequest::Begin { bytes, .. } => *bytes > 0 && *bytes <= MAX_CONFIG_BYTES,
            CheckRequest::Append { offset, bytes, .. } => {
                !bytes.is_empty()
                    && bytes.len() <= CHUNK_BYTES
                    && offset
                        .checked_add(bytes.len())
                        .is_some_and(|n| n <= MAX_CONFIG_BYTES)
            }
            CheckRequest::ReadOutput { offset, .. } => *offset <= OUTPUT_BYTES,
            _ => true,
        };
        if !valid {
            return Err("Helper CheckConfig request exceeds typed bounds".into());
        }
        if self.platform() != Platform::Mac {
            return Err("Helper CheckConfig Unsupported on this platform".into());
        }
        let expected = request.key().cloned();
        let core = request.core().clone();
        match self
            .send_with_timeout(&Request::MacCheck(request), timeout)
            .map_err(|e| format!("Helper CheckConfig transport outcome unknown: {e}"))?
        {
            Response::Ok(ResponseKind::MacCheck(reply))
                if reply.key().core == core
                    && expected.as_ref().is_none_or(|k| k == reply.key()) =>
            {
                Ok(reply)
            }
            Response::Err(error) => Err(format!(
                "Helper CheckConfig refused (trusted Helper upgrade may be required): {error}"
            )),
            _ => Err(
                "Helper CheckConfig response identity/type mismatch; Unsupported, no exec fallback"
                    .into(),
            ),
        }
    }

    /// Generic wire exchange only: injected replies are data, never a production
    /// retirement issuer. Production custody requires `SystemMacCheckClient`.
    /// The worker retains this exchange after an async caller drops. `cancel`
    /// requests cancellation; only a terminal reply permits releasing the row.
    pub fn run_macos_check(
        &self,
        core: CoreId,
        config: &[u8],
        sha256: [u8; 32],
        cancel: &AtomicBool,
    ) -> Result<HelperCheckOutput, HelperCheckFailure> {
        if config.is_empty() || config.len() > MAX_CONFIG_BYTES {
            return Err(HelperCheckFailure::NotAdmitted(
                "CheckConfig requires 1..=8MiB captured bytes".into(),
            ));
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(HelperCheckFailure::NotAdmitted(
                "CheckConfig cancelled before Helper admission".into(),
            ));
        }
        let upload_deadline = Instant::now() + Duration::from_secs(5);
        let remaining = |deadline: Instant| {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or_else(|| {
                    "CheckConfig exchange budget expired; cleanup unconfirmed".to_string()
                })
        };
        if self.platform() != Platform::Mac {
            return Err(HelperCheckFailure::NotAdmitted(
                "Unsupported platform".into(),
            ));
        }
        let expected_core = core.clone();
        let begin = Request::MacCheck(CheckRequest::Begin {
            core,
            bytes: config.len(),
            sha256,
        });
        let key = match self
            .send_with_timeout(
                &begin,
                remaining(upload_deadline).map_err(HelperCheckFailure::NotAdmitted)?,
            )
            .map_err(|e| match &e {
                // The closed system connector fails path/OS-peer admission
                // before any write. This cannot settle a different request.
                crate::ClientError::Connect(_) => HelperCheckFailure::NotAdmitted(e.to_string()),
                _ => HelperCheckFailure::Unconfirmed(e.to_string()),
            })? {
            Response::Ok(ResponseKind::MacCheck(CheckReply::Begun { key }))
                if key.core == expected_core =>
            {
                key
            }
            // Only an unknown command BEFORE Begin dispatch proves no admission.
            // Later unknown/NotFound replies cannot clear an admitted producer.
            Response::Err(e) if e.code == polaris_helper_proto::ErrorCode::Unknown => {
                return Err(HelperCheckFailure::NotAdmitted(
                    "Unsupported; trusted Helper upgrade required".into(),
                ))
            }
            Response::Err(e) if e.code == polaris_helper_proto::ErrorCode::Auth => {
                // Token/peer/native-caller authentication precedes Begin dispatch.
                // This says nothing about any different request's native debt.
                return Err(HelperCheckFailure::NotAdmitted(e.to_string()));
            }
            _ => {
                return Err(HelperCheckFailure::Unconfirmed(
                    "CheckConfig Begin response differs or admission cleanup is unknown".into(),
                ))
            }
        };
        let result = (|| {
            for (index, bytes) in config.chunks(CHUNK_BYTES).enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    return self.cancel_check_and_collect(&key);
                }
                let offset = index * CHUNK_BYTES;
                match self.send_macos_check(
                    CheckRequest::Append {
                        key: key.clone(),
                        offset,
                        bytes: bytes.to_vec(),
                    },
                    remaining(upload_deadline)?,
                )? {
                    CheckReply::Appended { received, .. } if received == offset + bytes.len() => {}
                    _ => return Err("CheckConfig Append offset response differs".into()),
                }
            }
            if cancel.load(Ordering::SeqCst) {
                return self.cancel_check_and_collect(&key);
            }
            match self.send_macos_check(
                CheckRequest::Commit { key: key.clone() },
                remaining(upload_deadline)?,
            )? {
                CheckReply::Status {
                    status: CheckStatus::Running,
                    ..
                } => {}
                _ => return Err("CheckConfig Commit did not enter typed custody".into()),
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return self.cancel_check_and_collect(&key);
                }
                if Instant::now() >= deadline {
                    let mut out = self.cancel_check_and_collect(&key)?;
                    if out.status == CheckStatus::Cancelled {
                        out.status = CheckStatus::TimedOut;
                    }
                    return Ok(out);
                }
                let reply = self.send_macos_check(
                    CheckRequest::Status { key: key.clone() },
                    remaining(deadline)?,
                )?;
                if let CheckReply::Status {
                    status,
                    stdout,
                    stderr,
                    ..
                } = reply
                {
                    if status.terminal() {
                        return self.collect_check(&key, status, stdout, stderr);
                    }
                    if status == CheckStatus::Unknown {
                        return Err(
                            "Helper CheckConfig native cleanup Unknown; owner retained".into()
                        );
                    }
                } else {
                    return Err("CheckConfig Status response differs".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })();
        if let Err(first) = result {
            // A different Cancel request is not a replay of a failed Commit.
            // Preserve the first error even if bounded same-request cleanup succeeds.
            return match self.cancel_check_and_collect(&key) {
                Ok(_) => Err(HelperCheckFailure::Retired(first)),
                Err(cleanup) => Err(HelperCheckFailure::Unconfirmed(format!(
                    "{first}; {cleanup}"
                ))),
            };
        }
        result.map_err(HelperCheckFailure::Unconfirmed)
    }

    fn cancel_check_and_collect(&self, key: &CheckKey) -> Result<HelperCheckOutput, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut first = true;
        loop {
            let timeout = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or("CheckConfig actual wait/pipe joins unconfirmed after cancellation")?;
            let request = if first {
                first = false;
                CheckRequest::Cancel { key: key.clone() }
            } else {
                CheckRequest::Status { key: key.clone() }
            };
            match self.send_macos_check(request, timeout)? {
                CheckReply::Status { status, .. } if status.terminal() => {
                    // Caller explicitly abandons diagnostics on cancellation.
                    self.release_check(key)?;
                    return Ok(HelperCheckOutput {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });
                }
                CheckReply::Status {
                    status: CheckStatus::Unknown,
                    ..
                } => return Err("Helper CheckConfig cancellation Unknown; owner retained".into()),
                CheckReply::Status { .. } => std::thread::sleep(Duration::from_millis(10)),
                _ => return Err("CheckConfig Cancel response differs".into()),
            }
        }
    }

    fn collect_check(
        &self,
        key: &CheckKey,
        status: CheckStatus,
        stdout: usize,
        stderr: usize,
    ) -> Result<HelperCheckOutput, String> {
        let mut outputs = [Vec::new(), Vec::new()];
        for (index, (stream, len)) in [
            (OutputStream::Stdout, stdout),
            (OutputStream::Stderr, stderr),
        ]
        .into_iter()
        .enumerate()
        {
            if len > OUTPUT_BYTES {
                return Err("CheckConfig retained output exceeds bound".into());
            }
            while outputs[index].len() < len {
                let offset = outputs[index].len();
                match self.send_macos_check(
                    CheckRequest::ReadOutput {
                        key: key.clone(),
                        stream,
                        offset,
                    },
                    Duration::from_secs(5),
                )? {
                    CheckReply::Output {
                        stream: got,
                        offset: at,
                        bytes,
                        ..
                    } if got == stream
                        && at == offset
                        && !bytes.is_empty()
                        && bytes.len() <= CHUNK_BYTES
                        && offset + bytes.len() <= len =>
                    {
                        outputs[index].extend(bytes)
                    }
                    _ => return Err("CheckConfig output chunk differs".into()),
                }
            }
        }
        self.release_check(key)?;
        let [stdout, stderr] = outputs;
        Ok(HelperCheckOutput {
            status,
            stdout,
            stderr,
        })
    }
    fn release_check(&self, key: &CheckKey) -> Result<(), String> {
        match self.send_macos_check(
            CheckRequest::Release { key: key.clone() },
            Duration::from_secs(5),
        )? {
            CheckReply::Released { .. } => Ok(()),
            _ => Err("CheckConfig Release did not confirm terminal retirement".into()),
        }
    }
}

#[cfg(test)]
mod tests;

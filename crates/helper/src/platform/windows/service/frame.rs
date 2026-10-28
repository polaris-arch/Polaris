//! Pure accumulation/deadline contract for Windows message-mode requests.
//!
//! One client WriteFile is one request. ERROR_MORE_DATA fragments are parts of
//! that same message, regardless of newlines, argument arity, or UTF-8 boundaries.
//! Distinct WriteFile calls are distinct messages and are never concatenated.
//! This keeps existing request bytes and installer/bootstrap commands unchanged.

use polaris_helper_proto::codec::MAX_FRAME_BYTES;
use std::io::{self, Error, ErrorKind};
use std::time::{Duration, Instant};

/// Result of one nonblocking message-mode ReadFile. `complete` is true only for
/// native success, false only for ERROR_MORE_DATA with the returned byte count.
pub(super) struct MessageRead {
    pub bytes: usize,
    pub complete: bool,
}

/// No text is exposed until the native message is complete and validated.
#[derive(Default)]
struct MessageFrame {
    bytes: Vec<u8>,
    finished: bool,
}

impl MessageFrame {
    fn push(&mut self, fragment: &[u8], complete: bool) -> io::Result<Option<String>> {
        if self.finished {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "request message already ended",
            ));
        }
        let result = self.push_inner(fragment, complete);
        // A rejected or completed message cannot later become a different request.
        if result.is_err() || matches!(&result, Ok(Some(_))) {
            self.finished = true;
        }
        result
    }

    fn push_inner(&mut self, fragment: &[u8], complete: bool) -> io::Result<Option<String>> {
        if fragment.len() > MAX_FRAME_BYTES - self.bytes.len() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "request message too large",
            ));
        }
        self.bytes.extend_from_slice(fragment);
        if !complete {
            if fragment.is_empty() || self.bytes.len() == MAX_FRAME_BYTES {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid request continuation",
                ));
            }
            return Ok(None);
        }
        if self.bytes.last() != Some(&b'\n') {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "request lacks terminal LF",
            ));
        }
        let raw = String::from_utf8(std::mem::take(&mut self.bytes))
            .map_err(|_| Error::new(ErrorKind::InvalidData, "request is not UTF-8"))?;
        if raw.lines().count() < 2 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "request lacks token or command",
            ));
        }
        Ok(Some(raw))
    }
}

/// The source must be nonblocking. One deadline covers all chunks, empty polls,
/// and final decoding. Injectable clock/wait functions permit inert regressions.
pub(super) fn read_frame_until(
    deadline: Instant,
    mut read: impl FnMut(&mut [u8]) -> io::Result<MessageRead>,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(Duration),
) -> io::Result<String> {
    const POLL_INTERVAL: Duration = Duration::from_millis(5);
    let timed_out = || Error::new(ErrorKind::TimedOut, "request frame deadline expired");
    let mut decoder = MessageFrame::default();
    let mut fragment = [0u8; 4096];
    loop {
        if now() >= deadline {
            return Err(timed_out());
        }
        let result = read(&mut fragment);
        if now() >= deadline {
            return Err(timed_out());
        }
        match result {
            Ok(MessageRead { bytes, complete }) if bytes <= fragment.len() => {
                if let Some(raw) = decoder.push(&fragment[..bytes], complete)? {
                    if now() >= deadline {
                        return Err(timed_out());
                    }
                    return Ok(raw);
                }
            }
            Ok(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid read byte count",
                ))
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                let remaining = deadline
                    .checked_duration_since(now())
                    .ok_or_else(timed_out)?;
                if remaining.is_zero() {
                    return Err(timed_out());
                }
                wait(remaining.min(POLL_INTERVAL));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// Keep cancellation active after expiry until its owning scope has finished.
/// A single cancellation can arrive before a blocking operation starts or in the
/// write/flush gap, so it cannot by itself establish a response deadline.
pub(super) fn cancel_after_deadline(
    deadline: Instant,
    mut finished: impl FnMut() -> bool,
    mut now: impl FnMut() -> Instant,
    mut park: impl FnMut(Duration),
    mut cancel: impl FnMut(),
) {
    while !finished() {
        let current = now();
        if current >= deadline {
            cancel();
            park(Duration::from_millis(5));
        } else {
            park(deadline - current);
        }
    }
}

#[cfg(test)]
mod tests;

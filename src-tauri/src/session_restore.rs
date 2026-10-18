//! Bounded session-query receipt for persistent proxy restoration. A timeout
//! gives no cancellation receipt: the producer must handle its late result.

use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::Duration;

pub(crate) fn wait(
    budget: Duration,
    start: impl FnOnce(SyncSender<Result<(), String>>),
) -> Result<Result<(), String>, RecvTimeoutError> {
    // No buffered success can be lost at the timeout/receiver-drop boundary.
    let (send, receive) = mpsc::sync_channel(0);
    start(send);
    receive.recv_timeout(budget)
}

pub(crate) fn complete(
    send: SyncSender<Result<(), String>>,
    result: Result<(), String>,
    late: impl FnOnce(Result<(), String>),
) {
    if let Err(unobserved) = send.send(result) {
        late(unobserved.0);
    }
}

#[cfg(test)]
mod tests;

//! Optional Windows resolver-cache flush, without a console child process.
//! A native RPC can outlive the caller's budget. Retain one worker, reject further
//! work while it is busy, and never claim that timing out cancelled the operation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[cfg(windows)]
mod native;

#[derive(Default)]
struct FlushWorker {
    busy: Arc<AtomicBool>,
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Owns the response, not a cancellable native operation or a child process.
pub struct PendingFlush(mpsc::Receiver<Result<(), String>>);

impl PendingFlush {
    pub fn wait(self, budget: Duration) -> Result<(), String> {
        match self.0.recv_timeout(budget) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "native DNS cache flush exceeded {budget:?}; operation remains in flight"
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("native DNS cache flush worker ended without a result".into())
            }
        }
    }
}

impl FlushWorker {
    fn begin(
        &self,
        flush: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<PendingFlush, String> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| "native DNS cache flush already in flight".to_string())?;
        let busy = BusyGuard(self.busy.clone());
        let (send, receive) = mpsc::sync_channel(1);
        // Dropping the returned JoinHandle deliberately detaches the optional worker.
        // BusyGuard stays with that worker even after the caller times out or drops.
        std::thread::Builder::new()
            .name("dns-cache-flush".into())
            .spawn(move || {
                let result = flush();
                drop(busy);
                if send.send(result).is_err() {
                    log::debug!("native DNS cache flush finished after its caller stopped waiting");
                }
            })
            .map_err(|error| format!("native DNS cache flush worker unavailable: {error}"))?;
        Ok(PendingFlush(receive))
    }
}

/// Start only optional cache work. Admission covers worker creation, not native RPC waiting.
#[cfg(windows)]
pub fn begin_flush() -> Result<PendingFlush, String> {
    static WORKER: std::sync::LazyLock<FlushWorker> =
        std::sync::LazyLock::new(FlushWorker::default);
    let _admission = crate::windows_session::admit_launch().map_err(|error| error.to_string())?;
    WORKER.begin(|| {
        if crate::windows_session::is_ending() {
            return Err("Windows session ending; native DNS cache flush skipped".into());
        }
        native::load()?.flush()
    })
}

#[cfg(test)]
mod tests;

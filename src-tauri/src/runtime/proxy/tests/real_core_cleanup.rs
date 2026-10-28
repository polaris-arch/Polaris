//! Last-resort custody for controlled real-core tests, including assertion unwinds.
//! Normal-path stop assertions remain authoritative; this guard never produces a stop receipt.
use super::ProxyRuntime;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) struct RealCoreCleanupGuard {
    runtime: Arc<ProxyRuntime>,
    test_name: String,
}

impl RealCoreCleanupGuard {
    pub(crate) fn new(runtime: &Arc<ProxyRuntime>) -> Self {
        Self {
            runtime: Arc::clone(runtime),
            test_name: std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned(),
        }
    }

    fn reap_owned_child(&self) -> Result<(bool, serde_json::Value), String> {
        // Shares the spawn admission lock: a birth already in flight must attach its
        // Child before this fence returns, and later births cannot pass the fence.
        self.runtime.begin_shutdown()?;
        let mut slot = self
            .runtime
            .child
            .lock()
            .map_err(|_| "real-core cleanup: Child custody lock poisoned".to_owned())?;
        let run = if slot.is_stopping_for_test() {
            slot.stopping_run_for_test()
        } else {
            slot.running_for_test()
        };
        let Some(run) = run else {
            return Ok((
                false,
                serde_json::json!({"state":"empty-after-shutdown-fence", "runRef":null, "corePid":null}),
            ));
        };
        let run_ref = run.identity.persisted_ref().to_owned();
        let core_pid = run.child_id_for_test();
        if run
            .try_wait_with(|child| child.try_wait())
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Ok((
                false,
                serde_json::json!({"state":"retained-child-exit-observed", "runRef":run_ref, "corePid":core_pid}),
            ));
        }
        // Use the original owned Child, never a rediscovered PID. Synchronous
        // try_wait is essential here: the Tokio runtime is about to be dropped.
        run.signal_with(|child| child.start_kill())
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if run
                .try_wait_with(|child| child.try_wait())
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Ok((
                    true,
                    serde_json::json!({"state":"emergency-owned-child-reaped", "runRef":run_ref, "corePid":core_pid}),
                ));
            }
            if Instant::now() >= deadline {
                return Err("real-core cleanup: owned Child was not reaped in 10s".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RealCoreCleanupGuard {
    fn drop(&mut self) {
        // Best-effort synchronous custody cleanup before Tokio Runtime::drop.
        // The CI job timeout remains the outer bound if admission/Child locks
        // or OS termination fail; this guard cannot promise recovery from those.
        let cleanup =
            catch_unwind(AssertUnwindSafe(|| self.reap_owned_child())).unwrap_or_else(|_| {
                Err("real-core cleanup panicked before custody was confirmed".into())
            });
        match cleanup {
            Ok((killed, custody)) => {
                // Positive original-Child observation, not absence of a stderr warning.
                // The runner binds this one record to its actual spawned test PID/name.
                let record = serde_json::json!({
                    "schema":"polaris-controlled-cleanup-v1", "testName":self.test_name,
                    "processId":std::process::id(), "generation":self.runtime.core_generation(),
                    "shutdownFenced":true, "custody":custody, "publicationEligible":false,
                });
                let mut output = std::io::stdout().lock();
                let _ = writeln!(output, "\nPOLARIS_CONTROLLED_CLEANUP {record}");
                let _ = output.flush();
                // Preserve an existing assertion panic. Otherwise expose missing
                // normal cleanup rather than silently making a broken test pass.
                assert!(
                    !killed || std::thread::panicking(),
                    "real-core test left a live owned Child; emergency cleanup reaped it"
                );
            }
            Err(error) => {
                if std::thread::panicking() {
                    // Reporting must not cause a second panic if stderr is closed.
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "{error}; real-core cleanup custody remains unconfirmed"
                    );
                } else {
                    panic!("{error}; real-core cleanup custody remains unconfirmed");
                }
            }
        }
    }
}

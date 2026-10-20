//! Bounded diagnostic version probes. Empty output is never bundled-version evidence.
//! Owns the original child/readers across Unknown cleanup; no detached replacement or reaper.
use polaris_system_integration::exec::{Command, CommandCleanup, PendingCommand, StdCommandRunner};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const CORE_VERSION_PROBE_BUDGET: Duration = Duration::from_secs(2);
const CORE_VERSION_CLEANUP_BUDGET: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(crate) struct CoreVersionProbe {
    pending: Mutex<Option<PendingCommand>>,
}

impl CoreVersionProbe {
    /// Only probes the supplied file. Call from a blocking context; never treats timeout as success.
    pub(crate) fn first_line(&self, bin: &Path) -> String {
        // Concurrent queries fail cheaply rather than waiting on a probe mutex.
        let Ok(mut pending) = self.pending.try_lock() else {
            return String::new();
        };
        if let Some(previous) = pending.as_mut() {
            if previous.poll_cleanup(CORE_VERSION_CLEANUP_BUDGET) == CommandCleanup::Unknown {
                return String::new(); // Do not accumulate replacement probes or lose custody.
            }
            *pending = None;
        }
        let Some(program) = bin.to_str() else {
            return String::new();
        };
        let mut observed = StdCommandRunner.run_observed(
            &Command::new(program, ["version"]),
            CORE_VERSION_PROBE_BUDGET,
        );
        if let Some(child) = observed.pending_mut() {
            child.poll_cleanup(CORE_VERSION_CLEANUP_BUDGET);
        }
        let line = match observed.operation() {
            Some(Ok(output)) => core_version_first_line(&output.stdout),
            _ => {
                // No raw stderr/path output: failures are diagnostic, never version evidence.
                log::warn!("Core version probe failed or exceeded its budget");
                String::new()
            }
        };
        if observed.cleanup() == CommandCleanup::Unknown {
            *pending = observed.into_pending();
            if let Some(child) = pending.as_mut() {
                child.request_stop();
                child.poll_cleanup(Duration::ZERO);
            }
        }
        line
    }
}

fn core_version_first_line(stdout: &str) -> String {
    stdout.lines().next().unwrap_or("").trim().to_owned()
}

#[cfg(test)]
mod tests;

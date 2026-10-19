//! Bounded diagnostic version probes. Empty output is never bundled-version evidence.
//! Owns the original child/readers across Unknown cleanup; no detached replacement or reaper.
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use polaris_system_integration::exec::{Command, CommandCleanup, PendingCommand, StdCommandRunner};

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
        let Some(program) = bin.to_str() else { return String::new(); };
        let mut observed = StdCommandRunner.run_observed(
            &Command::new(program, ["version"]), CORE_VERSION_PROBE_BUDGET,
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
mod tests {
    use super::*;
    #[test]
    fn attestation_uses_shared_probe_and_keeps_generation_commit_gate() {
        let source = include_str!("proxy/process_supervision.rs");
        assert!(source.contains("let version_probe = Arc::clone(&self.core_version_probe)"));
        assert!(source.contains("version_probe.first_line(&expected)"));
        assert!(source.contains("version_probe.first_line(r)"));
        assert!(source.contains("attestation_commit_allowed(self.gate.generation(), my_gen, &status, pid)"));
        assert!(!source.contains("fn core_version_first_line("));
    }

    #[test]
    fn bounded_original_custody_and_first_line_contract() {
        let source = include_str!("core_version_probe.rs");
        let body = crate::commands::guard_scan::top_level_fn_body(source, "pub(crate) fn first_line(");
        for anchor in ["self.pending.try_lock()", "StdCommandRunner.run_observed(",
            "previous.poll_cleanup(CORE_VERSION_CLEANUP_BUDGET)", "observed.into_pending()", "child.request_stop()"] {
            assert!(body.contains(anchor), "{anchor}");
        }
        for unbounded in [".output()", ".wait()", ".join()", "into_legacy_result"] {
            assert!(!body.contains(unbounded));
        }
        assert_eq!(CORE_VERSION_PROBE_BUDGET, Duration::from_secs(2));
        assert_eq!(CORE_VERSION_CLEANUP_BUDGET, Duration::from_millis(100));
        assert_eq!(core_version_first_line("  sing-box version 1.2.3\r\nextra"), "sing-box version 1.2.3");
        assert_eq!(core_version_first_line(""), "");
    }
}

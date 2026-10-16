use super::ProxyRuntime;
use crate::runtime::tls_fingerprints::Snapshot;
use std::path::Path;

impl ProxyRuntime {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) fn direct_tls_snapshot(&self, generation: u64) -> Snapshot {
        // Same lock order as native birth: Child first, proof second.
        let Ok(mut slot) = self.child.lock() else {
            return Snapshot::unavailable("Core custody is unavailable");
        };
        let Ok(captured) = self.captured_cli_run.lock() else {
            return Snapshot::unavailable("Core byte proof is unavailable");
        };
        match captured.as_ref() {
            Some((captured_generation, identity, entry))
                if *captured_generation == generation
                    && *captured_generation == self.core_generation()
                    && slot.observe_running(identity)
                        == polaris_core_supervisor::ChildObservation::Alive
                    && entry.still_current() =>
            {
                entry.snapshot.clone()
            }
            _ => Snapshot::unavailable(
                "Running core bytes could not be bound to the retained native instance",
            ),
        }
    }

    pub(crate) async fn tls_fingerprint_capabilities(&self) -> Snapshot {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let generation = self.core_generation();
            let status = self.status();
            if status.running
                && !self
                    .core_via_helper
                    .load(std::sync::atomic::Ordering::SeqCst)
            {
                return self.direct_tls_snapshot(generation);
            }
            // Linux /proc/<pid>/exe identifies the loaded image even after rename.
            #[cfg(not(target_os = "linux"))]
            if status.running {
                return Snapshot::unavailable("Helper loaded-image byte proof is unavailable");
            }
            let binary = match self.tls_capability_binary() {
                Ok(path) => path,
                Err(e) => return Snapshot::unavailable(e),
            };
            let snapshot = crate::runtime::tls_fingerprints::probe_cli(&binary).await;
            let current = self.status();
            if generation != self.core_generation()
                || status.pid != current.pid
                || status.running != current.running
            {
                return Snapshot::unavailable("Core selection changed during capability query");
            }
            snapshot
        }
        #[cfg(target_os = "android")]
        {
            super::android_bridge::tls_fingerprint_capabilities().await
        }
        #[cfg(target_os = "ios")]
        {
            match tauri_plugin_polaris_ios::tls_fingerprint_capabilities().await {
                Ok(reply) => Snapshot::from_native(
                    reply.document.as_deref(),
                    &reply.binary_sha256,
                    "packet-tunnel-executable",
                ),
                Err(reason) => Snapshot::unavailable(reason),
            }
        }
    }

    /// Startup supplies its exact binary, rather than a UI version probe path.
    pub(super) async fn tls_fingerprint_capabilities_for(&self, binary: Option<&Path>) -> Snapshot {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            match binary {
                Some(path) => crate::runtime::tls_fingerprints::probe_cli(path).await,
                None => Snapshot::unavailable("No selected core binary"),
            }
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = binary;
            self.tls_fingerprint_capabilities().await
        }
    }
}

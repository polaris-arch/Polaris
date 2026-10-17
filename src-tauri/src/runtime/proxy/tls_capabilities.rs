use super::ProxyRuntime;
use crate::runtime::tls_fingerprints::Snapshot;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
use std::path::Path;

impl ProxyRuntime {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn direct_tls_binding(
        &self,
        generation: u64,
    ) -> Result<
        (
            super::process_supervision::RunIdentity,
            crate::runtime::tls_fingerprints::CliObservation,
        ),
        String,
    > {
        // Same lock order as native birth: Child first, proof second.
        let mut slot = self
            .child
            .lock()
            .map_err(|_| "Core custody is unavailable")?;
        let captured = self
            .captured_cli_run
            .lock()
            .map_err(|_| "Core byte proof is unavailable")?;
        match captured.as_ref() {
            Some((captured_generation, identity, entry))
                if *captured_generation == generation
                    && *captured_generation == self.core_generation()
                    && slot.observe_running(identity)
                        == polaris_core_supervisor::ChildObservation::Alive
                    && entry.still_current() =>
            {
                Ok((identity.clone(), entry.clone()))
            }
            _ => {
                Err("Running core bytes could not be bound to the retained native instance".into())
            }
        }
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) fn direct_tls_snapshot(&self, generation: u64) -> Snapshot {
        match self.direct_tls_binding(generation) {
            Ok((_, entry)) => entry.snapshot,
            Err(reason) => Snapshot::unavailable(reason),
        }
    }

    pub(crate) async fn tls_fingerprint_capabilities(&self) -> Snapshot {
        self.tls_fingerprint_capabilities_observed().await.0
    }

    /// Acquisition may transfer its exact proof to a measurement identity;
    /// an unrelated cache refresh must not erase that observation in between.
    pub(crate) async fn tls_fingerprint_capabilities_observed(
        &self,
    ) -> (
        Snapshot,
        Option<crate::runtime::tls_fingerprints::CliObservation>,
    ) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let generation = self.core_generation();
            let status = self.status();
            if status.running
                && !self
                    .core_via_helper
                    .load(std::sync::atomic::Ordering::SeqCst)
            {
                let (identity, entry) = match self.direct_tls_binding(generation) {
                    Ok(bound) => bound,
                    Err(reason) => return (Snapshot::unavailable(reason), None),
                };
                // Transient startup/query errors can recover on focus or another
                // capability request, using the original file proof and custody.
                let refreshed = crate::runtime::tls_fingerprints::probe_cli(entry.path()).await;
                return match self.direct_tls_binding(generation) {
                    Ok((current, proof))
                        if current.same_run(&identity)
                            && proof.snapshot.binary_sha256 == refreshed.binary_sha256 =>
                    {
                        (refreshed, None)
                    }
                    _ => (
                        Snapshot::unavailable("Core instance changed during capability retry"),
                        None,
                    ),
                };
            }
            // Linux /proc/<pid>/exe identifies the loaded image even after rename.
            #[cfg(not(target_os = "linux"))]
            if status.running {
                return (
                    Snapshot::unavailable("Helper loaded-image byte proof is unavailable"),
                    None,
                );
            }
            let binary = match self.tls_capability_binary() {
                Ok(path) => path,
                Err(e) => return (Snapshot::unavailable(e), None),
            };
            let (snapshot, proof) =
                match crate::runtime::tls_fingerprints::probe_cli_observation(&binary).await {
                    Ok(proof) => (proof.snapshot.clone(), Some(proof)),
                    Err(snapshot) => (snapshot, None),
                };
            let current = self.status();
            if generation != self.core_generation()
                || status.pid != current.pid
                || status.running != current.running
            {
                return (
                    Snapshot::unavailable("Core selection changed during capability query"),
                    None,
                );
            }
            (snapshot, proof)
        }
        #[cfg(target_os = "android")]
        {
            (
                super::android_bridge::tls_fingerprint_capabilities().await,
                None,
            )
        }
        #[cfg(target_os = "ios")]
        {
            let snapshot = match tauri_plugin_polaris_ios::tls_fingerprint_capabilities().await {
                Ok(reply) => Snapshot::from_native(
                    reply.document.as_deref(),
                    &reply.binary_sha256,
                    "packet-tunnel-executable",
                ),
                Err(reason) => Snapshot::unavailable(reason),
            };
            (snapshot, None)
        }
    }

    /// Startup supplies its exact binary, rather than a UI version probe path.
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
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

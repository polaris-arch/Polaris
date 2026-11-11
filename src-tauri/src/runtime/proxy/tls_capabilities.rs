use super::ProxyRuntime;
use crate::runtime::tls_fingerprints::Snapshot;

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

    #[cfg(any(target_os = "android", target_os = "ios"))]
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
            if status.running
                && self
                    .core_via_helper
                    .load(std::sync::atomic::Ordering::SeqCst)
            {
                let client = match self.runtime_management_client(generation).await {
                    Ok(client) => client,
                    Err(()) => {
                        return (
                            Snapshot::unavailable("Running Helper API channel unavailable"),
                            None,
                        )
                    }
                };
                let pairing = self.runtime_pairing_for_capability(generation);
                return match pairing {
                    Some(pairing) => (
                        self.helper_running_tls_snapshot(generation, &client, &pairing)
                            .await,
                        None,
                    ),
                    None => (
                        Snapshot::unavailable("Running Helper pairing unavailable"),
                        None,
                    ),
                };
            }
            // Inventory is a separate selected-cohort query, not a claim about
            // the running process's random TLS choice or a pathname credential.
            let selection = match self.tls_capability_selection() {
                Ok(selection) => selection,
                Err(e) => return (Snapshot::unavailable(e), None),
            };
            let (snapshot, proof) =
                match crate::runtime::tls_fingerprints::probe_cli_observation(selection.path())
                    .await
                {
                    Ok(proof) => (proof.snapshot.clone(), Some(proof)),
                    Err(snapshot) => (*snapshot, None),
                };
            let current = self.status();
            if !selection.is_current()
                || generation != self.core_generation()
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
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl ProxyRuntime {
    /// Running Helper birth + authenticated API channel, never a CLI inventory launch.
    pub(super) async fn helper_running_tls_snapshot(
        &self,
        generation: u64,
        client: &polaris_singbox_grpc::SingBoxApiClient,
        pairing: &super::runtime_config::RuntimePairing,
    ) -> Snapshot {
        // 失败时记下是哪一道绑定，便于真机定位内核哈希缺失（每次起核最多一条）。
        let mut stage = "birth";
        let result = async {
            use crate::runtime::helper::HelperStopTarget;
            let (start_token, target) = self
                .child
                .lock()
                .map_err(|_| "Helper child custody unavailable")?
                .helper_stop_target()
                .ok_or("Helper birth unavailable")?;
            stage = "core-id";
            let platform = crate::runtime::core_identity::current_platform()?;
            let core = crate::runtime::core_identity::selected_id(platform)?;
            stage = "birth";
            let HelperStopTarget::Birth(birth) = target else {
                return Err("Legacy Helper PID is not execution evidence".into());
            };
            let helper = std::sync::Arc::clone(&self.helper);
            let before = tokio::task::spawn_blocking(move || helper.protected_native_core_status())
                .await
                .map_err(|e| e.to_string())??;
            if !native_helper_birth_matches(self.helper.platform(), &before, birth, &core) {
                return Err("Helper original birth/selected Core binding unavailable".into());
            }
            stage = "api-channel";
            let capabilities = client
                .get_runtime_capabilities()
                .await
                .map_err(|_| "Authenticated running capability query unavailable")?;
            stage = "pairing/incarnation";
            if !super::runtime_config::runtime_pairing_matches(pairing, &capabilities)
                || capabilities
                    .active
                    .as_ref()
                    .is_none_or(|active| active.incarnation.is_empty())
            {
                return Err("Running capability incarnation differs".into());
            }
            stage = "tls-probe-contract";
            let manifest = crate::runtime::core_identity::selected_manifest(platform)?;
            let source = crate::runtime::core_identity::selected_source_commit(platform)?;
            let binary = manifest
                .files
                .iter()
                .find(|f| f.name == "sing-box" || f.name == "sing-box.exe")
                .ok_or("Compiled Core member absent")?;
            if capabilities.tls_fingerprint_capabilities_json.is_empty() {
                return Err("Running core has no TLS capability report".into());
            }
            let contract: serde_json::Value =
                serde_json::from_str(polaris_core_supervisor::selected_core::selected_contract()?)
                    .map_err(|e| e.to_string())?;
            let row = &contract["platforms"][platform];
            let proof_field = match platform {
                "win" => "windowsExecution",
                "mac-x64" | "mac-arm64" => "macosExecution",
                "linux" => "sealedExecution",
                _ => return Err("No platform execution report".into()),
            };
            let expected_doc =
                &row["finalPackageCronetSmoke"][proof_field]["tlsProbe"]["capabilities"];
            let arch = match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => other,
            };
            let os = if cfg!(target_os = "macos") {
                "darwin"
            } else {
                std::env::consts::OS
            };
            let mut snapshot = Snapshot::from_authenticated_document(
                &capabilities.tls_fingerprint_capabilities_json,
                expected_doc,
                &source,
                os,
                arch,
                &binary.sha256,
            )?;
            stage = "state-drift";
            let helper = std::sync::Arc::clone(&self.helper);
            let after = tokio::task::spawn_blocking(move || helper.protected_native_core_status())
                .await
                .map_err(|e| e.to_string())??;
            if generation != self.core_generation()
                || after != before
                || !self
                    .child
                    .lock()
                    .map_err(|_| "Helper custody unavailable")?
                    .helper_stop_target()
                    .is_some_and(|(current, t)| current.same(&start_token) && t == target)
            {
                return Err("Helper birth changed during capability query".into());
            }
            snapshot.observation_scope = "running-helper-authenticated-api".into();
            snapshot.byte_identity_scope = "helper-private-executable".into();
            Ok::<_, String>(snapshot)
        }
        .await;
        result.unwrap_or_else(|reason| {
            log::warn!(
                "运行核 TLS 能力自证失败（{stage}，世代 {generation}）：{reason}；内核哈希缺失，测速结果将显示为已过期"
            );
            Snapshot::unavailable(reason)
        })
    }
}

/// Data predicate only. Production calls it only after the closed OS-root/system
/// connector has returned the original native-custody status, never generic IPC.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn native_helper_birth_matches(
    platform: polaris_helper_proto::Platform,
    status: &crate::runtime::helper::ManagedCoreStatus,
    expected: polaris_helper_proto::HelperBirthTarget,
    core: &polaris_helper_proto::core_manifest::CoreId,
) -> bool {
    use crate::runtime::helper::ManagedCoreStatus;
    use polaris_helper_proto::Platform;
    let ManagedCoreStatus::NativeBirthRunning {
        target,
        created,
        image,
        core: reported,
    } = status
    else {
        return false;
    };
    if *target != expected || reported.as_ref().is_some_and(|id| id != core) {
        return false;
    }
    match platform {
        // macOS truth is the Root registry's original Child, immutable selected
        // CoreId and random birth. It does not claim Windows process fields.
        Platform::Mac => reported.as_ref() == Some(core),
        Platform::Win => {
            created.is_some_and(|value| value > 0)
                && image.as_ref().is_some_and(|value| !value.is_empty())
        }
        // Linux uses its separate sealed execution receipt. Mobile runtimes
        // use loaded native containers, and Other has no desktop Helper issuer.
        Platform::Linux | Platform::Android | Platform::Ios | Platform::Other => false,
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests;

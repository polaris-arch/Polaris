//! Native-only Debug credential projection. No new lifecycle/lease owner and no socket work.
//! Start bytes come from AndroidStartReceipt, never current/saved user configuration.
use super::android_bridge::{AndroidExactTarget, AndroidStartReceipt};
use super::{AndroidRequestBirth, ProxyRuntime, SwitchSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(super) enum DebugCoreProbeIngressKind {
    HttpTcp,
    MixedTcpUdp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DebugCoreProbeIngress {
    kind: DebugCoreProbeIngressKind,
    port: u16,
    credential_sha256: String,
}

fn actual_ingress(config: &str, digest: &str) -> Option<DebugCoreProbeIngress> {
    if polaris_updater::verify::sha256_hex(config.as_bytes()) != digest {
        return None;
    }
    let config: serde_json::Value = serde_json::from_str(config).ok()?;
    let mut inbounds = config
        .get("inbounds")?
        .as_array()?
        .iter()
        .filter(|inbound| inbound.get("tag").and_then(|v| v.as_str()) == Some("probe-proxy-in"));
    let inbound = inbounds.next()?;
    if inbounds.next().is_some() || inbound.get("listen")?.as_str()? != "127.0.0.1" {
        return None;
    }
    let kind = match inbound.get("type")?.as_str()? {
        "http" => DebugCoreProbeIngressKind::HttpTcp,
        "mixed" => DebugCoreProbeIngressKind::MixedTcpUdp,
        _ => return None,
    };
    let port = u16::try_from(inbound.get("listen_port")?.as_u64()?)
        .ok()
        .filter(|p| *p != 0)?;
    let users = inbound.get("users")?.as_array()?;
    if users.len() != 1 || users[0].get("username")?.as_str()? != "polaris" {
        return None;
    }
    let password = users[0].get("password")?.as_str()?;
    if !hex(password, 32) {
        return None;
    }
    Some(DebugCoreProbeIngress {
        kind,
        port,
        credential_sha256: polaris_updater::verify::sha256_hex(password.as_bytes()),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AndroidProbeStartBinding {
    generation: u64,
    target: AndroidExactTarget,
    config_digest: String,
    selector_intent: u64,
    ingress: DebugCoreProbeIngress,
}

/// Actual original-session Kotlin projection. It contains no inbound secret or arbitrary target.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DebugCoreProbeSessionScope {
    pub boot_nonce: String,
    pub session_id: String,
    pub nonce: String,
    pub plan_sha256: String,
    pub apk_sha256: String,
    pub expected_source_pin: String,
    pub run_id: String,
    pub birth_nonce: String,
    pub revision: u64,
    pub config_digest: String,
    pub deadline_elapsed: u64,
    pub sampled_elapsed: u64,
}

/// Only android_bridge serializes this native payload. It has no Deserialize or public Debug view.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DebugCoreProbeLoanPayload {
    boot_nonce: String,
    session_id: String,
    nonce: String,
    plan_sha256: String,
    apk_sha256: String,
    expected_source_pin: String,
    generation: String,
    run_id: String,
    birth_nonce: String,
    revision: u64,
    config_digest: String,
    deadline_elapsed: u64,
    probe_port: u16,
    ingress_kind: DebugCoreProbeIngressKind,
    pc_ready_request_id: String,
    expires_elapsed: u64,
    password: Vec<u8>,
}

impl Drop for DebugCoreProbeLoanPayload {
    fn drop(&mut self) {
        self.password.fill(0);
    }
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl DebugCoreProbeSessionScope {
    fn valid(&self) -> bool {
        hex(&self.boot_nonce, 32)
            && hex(&self.session_id, 32)
            && hex(&self.nonce, 48)
            && hex(&self.plan_sha256, 64)
            && hex(&self.apk_sha256, 64)
            && hex(&self.expected_source_pin, 64)
            && hex(&self.config_digest, 64)
            && self.revision > 0
            && self.revision <= i64::MAX as u64
            && self.deadline_elapsed > self.sampled_elapsed
            && self.deadline_elapsed <= i64::MAX as u64
            && self.deadline_elapsed - self.sampled_elapsed <= 300_000
            && AndroidExactTarget {
                run_id: self.run_id.clone(),
                birth_nonce: self.birth_nonce.clone(),
            }
            .is_valid()
    }
}

impl ProxyRuntime {
    /// The original TS state gate is held by startup. Keep custody -> inner, as existing
    /// Android Start admission does; never acquire publication -> custody in this module.
    pub(super) fn record_android_probe_start(
        &self,
        generation: u64,
        birth: &AndroidRequestBirth,
        receipt: &AndroidStartReceipt,
        actual_start_config: &str,
    ) {
        if !hex(&receipt.config_digest, 64) {
            return;
        }
        let Some(ingress) = actual_ingress(actual_start_config, &receipt.config_digest) else {
            return;
        };
        let Ok(mut custody) = self.android_main_token.lock() else {
            return;
        };
        let Some(custody) = custody.as_mut().filter(|c| {
            c.birth.same(birth)
                && c.start_confirmed
                && !c.stop_only
                && !c.historic_unknown
                && c.stop_inflight.is_none()
                && c.exact_target.as_ref() == Some(&receipt.exact_target())
        }) else {
            return;
        };
        self.gate.with_current_generation(generation, |_| {
            custody.debug_probe_input = Some(AndroidProbeStartBinding {
                generation,
                target: receipt.exact_target(),
                config_digest: receipt.config_digest.clone(),
                selector_intent: self.selector_reconcile.intent_generation(),
                ingress,
            });
        });
    }

    /// Freeze binding and port/auth in the existing snapshot's one write. The TS state
    /// gate is already held by startup. Metadata locks precede inner; no work under inner.
    pub(super) fn publish_android_probe_snapshot(
        &self,
        generation: u64,
        mut snapshot: SwitchSnapshot,
    ) {
        let Ok(custody) = self.android_main_token.lock() else {
            return;
        };
        let Ok(mut running) = self.switch_snapshot.write() else {
            return;
        };
        self.gate.with_current_generation(generation, |_| {
            snapshot.android_probe_input = custody.as_ref().and_then(|c| {
                c.debug_probe_input
                    .as_ref()
                    .filter(|b| {
                        b.generation == generation
                            && c.start_confirmed
                            && !c.stop_only
                            && !c.historic_unknown
                            && c.stop_inflight.is_none()
                            && c.exact_target.as_ref() == Some(&b.target)
                    })
                    .cloned()
            });
            *running = Some(snapshot);
        });
    }

    /// No mutex survives this await or the later IPC. The existing TS state gate excludes
    /// startup/teardown writes; selector intent/custody/snapshot/status are short read locks,
    /// then inner proves the current generation without taking any new external lock.
    pub(super) async fn collect_android_probe_loan(
        &self,
        scope: &DebugCoreProbeSessionScope,
    ) -> Result<DebugCoreProbeLoanPayload, String> {
        if !scope.valid() {
            return Err("Core probe scope unavailable".into());
        }
        let _state = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.mesh.tailscale_state_gate(),
        )
        .await
        .map_err(|_| "Core probe state budget expired".to_owned())?;
        self.selector_reconcile
            .with_intent_claim(|intent| {
                let custody = self
                    .android_main_token
                    .lock()
                    .map_err(|_| "Core probe custody unavailable")?;
                let snapshot = self
                    .switch_snapshot
                    .read()
                    .map_err(|_| "Core probe snapshot unavailable")?;
                let status = self
                    .status
                    .read()
                    .map_err(|_| "Core probe status unavailable")?;
                let c = custody.as_ref().ok_or("Core probe custody missing")?;
                let s = snapshot.as_ref().ok_or("Core probe snapshot missing")?;
                let b = c
                    .debug_probe_input
                    .as_ref()
                    .ok_or("Core probe Start binding missing")?;
                if b.generation == 0
                    || !status.running
                    || !c.start_confirmed
                    || c.stop_only
                    || c.historic_unknown
                    || c.stop_inflight.is_some()
                    || s.android_probe_input.as_ref() != Some(b)
                    || c.exact_target.as_ref() != Some(&b.target)
                    || b.selector_intent != intent
                    || self.selector_reconcile.is_required()
                    || b.target.run_id != scope.run_id
                    || b.target.birth_nonce != scope.birth_nonce
                    || b.config_digest != scope.config_digest
                {
                    return Err("Core probe input changed".into());
                }
                let port = s
                    .probe_proxy_port
                    .filter(|p| *p != 0)
                    .ok_or("Core probe port missing")?;
                let auth = s
                    .loopback_auth
                    .as_ref()
                    .filter(|a| a.username == "polaris" && hex(&a.password, 32))
                    .ok_or("Core probe credential missing")?;
                if port != b.ingress.port
                    || polaris_updater::verify::sha256_hex(auth.password.as_bytes())
                        != b.ingress.credential_sha256
                {
                    return Err("Core probe actual ingress changed".into());
                }
                self.gate
                    .with_current_generation(b.generation, |_| DebugCoreProbeLoanPayload {
                        boot_nonce: scope.boot_nonce.clone(),
                        session_id: scope.session_id.clone(),
                        nonce: scope.nonce.clone(),
                        plan_sha256: scope.plan_sha256.clone(),
                        apk_sha256: scope.apk_sha256.clone(),
                        expected_source_pin: scope.expected_source_pin.clone(),
                        generation: b.generation.to_string(),
                        run_id: scope.run_id.clone(),
                        birth_nonce: scope.birth_nonce.clone(),
                        revision: scope.revision,
                        config_digest: scope.config_digest.clone(),
                        deadline_elapsed: scope.deadline_elapsed,
                        probe_port: port,
                        ingress_kind: b.ingress.kind,
                        pc_ready_request_id: String::new(),
                        expires_elapsed: scope.deadline_elapsed,
                        password: auth.password.as_bytes().to_vec(),
                    })
                    .ok_or_else(|| "Core probe generation changed".into())
            })
            .map_err(str::to_owned)
    }

    #[cfg(all(target_os = "android", debug_assertions))]
    pub(crate) async fn debug_android_core_probe(
        &self,
        session_id: String,
    ) -> Result<String, String> {
        let before = super::android_bridge::debug_core_probe_scope(session_id).await?;
        let ready = super::android_bridge::debug_pc_echo_prepare(before.session_id.clone()).await?;
        let admitted = ready.admit(&before)?;
        let mut loan = self.collect_android_probe_loan(&before).await?;
        loan.pc_ready_request_id = admitted.request_id().to_owned();
        loan.expires_elapsed = loan.expires_elapsed.min(admitted.deadline_elapsed());
        let report = super::android_bridge::debug_core_probe_loan(loan).await?;
        // The native callback has already erased the credential and returned its original
        // command ticket. Never label a delayed callback as a current-generation observation.
        // Preserve actual sent/returned evidence even when a later native fence is stale.
        let current =
            match super::android_bridge::debug_core_probe_scope(before.session_id.clone()).await {
                Ok(after) => {
                    let mut expected = before;
                    expected.sampled_elapsed = after.sampled_elapsed;
                    expected == after && self.collect_android_probe_loan(&after).await.is_ok()
                }
                Err(_) => false,
            };
        let mut value: serde_json::Value = serde_json::from_str(&report)
            .map_err(|_| "Core probe report unavailable".to_owned())?;
        value
            .as_object_mut()
            .ok_or("Core probe report unavailable")?
            .insert(
                "nativeScopeStatus".into(),
                serde_json::Value::String(if current { "Current" } else { "StaleScope" }.into()),
            );
        serde_json::to_string(&value).map_err(|_| "Core probe report unavailable".to_owned())
    }
}

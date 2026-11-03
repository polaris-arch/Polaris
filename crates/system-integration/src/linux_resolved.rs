//! Linux `systemd-resolved` 接管的 app 侧生命周期。
//!
//! root 写操作由调用方注入的 [`LinuxResolvedOps`] 完成；本层只负责 intent marker、失败收口、崩溃恢复
//! 与网络变化后的幂等重放。它与 macOS `networksetup` 控制器分离，因为 Linux 保存/恢复的是临时
//! per-link 状态，不应伪装成“备份物理网卡 DNS”。

use crate::proxy::MarkerFs;
use crate::proxy_ops::{TakeoverOutcome, TakeoverReceipt};
use polaris_helper_proto::linux_dns::{Change, Settings, Snapshot};
use polaris_helper_proto::linux_dns::{CONTROLLED_DNS_IP, TUN_INTERFACE_NAME};
use serde::{Deserialize, Serialize};

/// root helper 能力的 app 侧抽象。
pub trait LinuxResolvedOps: Send + Sync {
    /// 在固定 Polaris TUN 链路上接管 resolved。
    fn takeover(&self) -> Result<(), String>;
    /// 撤销固定链路的 resolved 状态。
    fn revert(&self) -> Result<(), String>;
    fn snapshot(&self) -> Result<Snapshot, String> {
        Err("checked Linux DNS snapshot unsupported".into())
    }
    fn compare(&self, expected: &Snapshot, enabled: bool) -> Result<Change, String> {
        let _ = (expected, enabled);
        Err("checked Linux DNS change unsupported".into())
    }
}

/// 崩溃恢复 marker。字段同时用于诊断与防止损坏/旧格式 marker 驱动错误接口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxResolvedMarkerData {
    /// 固定为 `polaris-tun0`。
    pub interface_name: String,
    /// 固定为受 TUN hijack 的 DNS 哨兵。
    pub server_ip: String,
    /// 写入时间（仅诊断，不参与决策）。
    pub at: u64,
}

/// 严格读取不会把不可读或损坏的 marker 当作不存在。
#[derive(Debug, thiserror::Error)]
pub enum LinuxResolvedMarkerError {
    #[error("read Linux resolved marker: {0}")]
    Read(#[source] std::io::Error),
    #[error("invalid Linux resolved marker: {0}")]
    Invalid(#[source] serde_json::Error),
    #[error("Linux resolved marker target does not match Polaris TUN")]
    WrongTarget,
}

struct LinuxResolvedMarker<Fs: MarkerFs> {
    fs: Fs,
    path: String,
}

impl<Fs: MarkerFs> LinuxResolvedMarker<Fs> {
    fn new(fs: Fs, path: impl Into<String>) -> Self {
        Self {
            fs,
            path: path.into(),
        }
    }

    fn write(&self) -> Result<(), String> {
        let marker = LinuxResolvedMarkerData {
            interface_name: TUN_INTERFACE_NAME.to_owned(),
            server_ip: CONTROLLED_DNS_IP.to_owned(),
            at: now_ms(),
        };
        let json = serde_json::to_string(&marker)
            .map_err(|error| format!("serialize Linux resolved marker: {error}"))?;
        self.fs
            .write_marker(&self.path, &json)
            .map_err(|error| format!("write Linux resolved marker: {error}"))
    }

    fn read_checked(&self) -> Result<Option<LinuxResolvedMarkerData>, LinuxResolvedMarkerError> {
        let Some(raw) = self
            .fs
            .read_marker_checked(&self.path)
            .map_err(LinuxResolvedMarkerError::Read)?
        else {
            return Ok(None);
        };
        let marker: LinuxResolvedMarkerData =
            serde_json::from_str(&raw).map_err(LinuxResolvedMarkerError::Invalid)?;
        if marker.interface_name != TUN_INTERFACE_NAME || marker.server_ip != CONTROLLED_DNS_IP {
            return Err(LinuxResolvedMarkerError::WrongTarget);
        }
        Ok(Some(marker))
    }

    fn clear(&self) -> Result<(), String> {
        self.fs
            .remove_marker(&self.path)
            .map_err(|error| format!("remove Linux resolved marker: {error}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LiveDnsFootprint {
    operation_id: String,
    phase: String,
    before: Snapshot,
    original: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LiveDnsMarker {
    interface_name: String,
    server_ip: String,
    at: u64,
    host_effect: LiveDnsFootprint,
}

impl LiveDnsMarker {
    fn parse(raw: &str) -> Result<Option<Self>, String> {
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|error| error.to_string())?;
        if value.get("hostEffect").is_none() {
            return Ok(None);
        }
        let marker: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let effect = &marker.host_effect;
        if marker.interface_name != TUN_INTERFACE_NAME
            || marker.server_ip != CONTROLLED_DNS_IP
            || effect.operation_id.trim().is_empty()
            || !matches!(effect.phase.as_str(), "applying" | "owned" | "restoring")
            || !effect.before.valid()
            || !effect.original.valid()
            || !effect.original.unconfigured()
            || effect
                .before
                .original
                .as_ref()
                .is_some_and(|original| original != &effect.original)
        {
            return Err("invalid exact Linux DNS marker".into());
        }
        Ok(Some(marker))
    }

    fn scope_matches(&self, snapshot: &Snapshot) -> bool {
        self.host_effect.before.target() == snapshot.target()
            && self.host_effect.before.link == snapshot.link
            && snapshot
                .original
                .as_ref()
                .is_none_or(|original| original == &self.host_effect.original)
    }
}

/// Private plan captures a read-only helper attestation, not a caller-selected
/// interface/server. Full original values remain in the marker through cleanup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreparedLinuxDnsEffect {
    marker_path: String,
    marker_raw: Option<String>,
    before: Snapshot,
    original: Settings,
    enabled: bool,
}

impl PreparedLinuxDnsEffect {
    #[must_use]
    pub fn before(&self) -> &Snapshot {
        &self.before
    }
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    #[must_use]
    pub fn prior_marker_operation_id(&self) -> Option<String> {
        self.marker_raw
            .as_ref()
            .and_then(|raw| LiveDnsMarker::parse(raw).ok().flatten())
            .map(|marker| marker.host_effect.operation_id)
    }
    #[must_use]
    pub fn needs_prior_restore(&self) -> bool {
        self.before.cleanup_pending
            || self.marker_raw.as_ref().is_some_and(|raw| {
                LiveDnsMarker::parse(raw)
                    .ok()
                    .flatten()
                    .is_some_and(|marker| marker.host_effect.phase != "owned")
            })
    }
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.marker_path
    }
    #[must_use]
    pub fn before_facts(&self) -> serde_json::Value {
        serde_json::json!({"owned":self.before.owned,"cleanupPending":self.before.cleanup_pending,"settings":self.before.settings})
    }
    #[must_use]
    pub fn target_facts(&self) -> serde_json::Value {
        let settings = if self.enabled {
            Settings {
                servers: vec![CONTROLLED_DNS_IP.into()],
                domains: vec!["~.".into()],
                default_route: true,
                dnssec: "no".into(),
                dns_over_tls: "no".into(),
            }
        } else {
            self.original.clone()
        };
        serde_json::json!({"owned":self.enabled,"cleanupPending":false,"settings":settings})
    }
}

/// Linux resolved 生命周期控制器。
pub struct LinuxResolvedController<Ops: LinuxResolvedOps, Fs: MarkerFs> {
    ops: Ops,
    marker: LinuxResolvedMarker<Fs>,
}

impl<Ops: LinuxResolvedOps, Fs: MarkerFs> LinuxResolvedController<Ops, Fs> {
    /// 构造控制器。
    pub fn new(ops: Ops, fs: Fs, marker_path: impl Into<String>) -> Self {
        Self {
            ops,
            marker: LinuxResolvedMarker::new(fs, marker_path),
        }
    }

    fn raw(&self) -> Result<Option<String>, String> {
        self.marker
            .fs
            .read_marker_checked(&self.marker.path)
            .map_err(|error| error.to_string())
    }

    fn raw_matches(&self, expected: &Option<String>) -> bool {
        self.raw().is_ok_and(|raw| &raw == expected)
    }

    fn replace_raw(
        &self,
        expected: &Option<String>,
        target: &Option<String>,
    ) -> Result<(), String> {
        if !self.raw_matches(expected) {
            return Err("Linux DNS marker owner changed".into());
        }
        match target {
            Some(raw) => self
                .marker
                .fs
                .write_marker(&self.marker.path, raw)
                .map_err(|error| error.to_string()),
            None => self.marker.clear(),
        }?;
        if !self.raw_matches(target) {
            return Err("Linux DNS marker read-back differs".into());
        }
        Ok(())
    }

    pub fn prepare_effect(&self, enabled: bool) -> Result<PreparedLinuxDnsEffect, String> {
        let raw = self.raw()?;
        self.read_marker_checked()
            .map_err(|error| error.to_string())?;
        let live = raw
            .as_deref()
            .map(LiveDnsMarker::parse)
            .transpose()?
            .flatten();
        let snapshot = self.ops.snapshot()?;
        if !snapshot.valid() {
            return Err("invalid helper DNS snapshot".into());
        }
        if let Some(marker) = &live {
            if !marker.scope_matches(&snapshot) {
                return Err(
                    "Linux DNS marker belongs to another native birth/link/baseline".into(),
                );
            }
        }
        if raw.is_some() && !snapshot.owned {
            return Err("Linux DNS marker has no matching helper custody".into());
        }
        if raw.is_none() && snapshot.owned {
            return Err("Linux DNS custody has no local restoration marker".into());
        }
        let original = snapshot
            .original
            .clone()
            .unwrap_or_else(|| snapshot.settings.clone());
        if !original.unconfigured() {
            return Err("Linux DNS original is not a reversible empty per-link baseline".into());
        }
        if snapshot.owned && !snapshot.settings.controlled() && !snapshot.cleanup_pending {
            return Err("Linux DNS controlled values drifted outside accepted custody".into());
        }
        if enabled && !snapshot.owned && !snapshot.settings.unconfigured() {
            return Err("Linux DNS link contains foreign resolver values".into());
        }
        Ok(PreparedLinuxDnsEffect {
            marker_path: self.marker.path.clone(),
            marker_raw: raw,
            before: snapshot,
            original,
            enabled,
        })
    }

    #[must_use]
    pub fn checked_owned_receipt(&self) -> Result<Option<TakeoverReceipt>, String> {
        let prepared = self.prepare_effect(true)?;
        let Some(operation) = prepared.prior_marker_operation_id() else {
            return Ok(None);
        };
        if prepared.needs_prior_restore()
            || prepared.before_facts() != prepared.target_facts()
            || !self.effect_facts_match(&prepared)?
        {
            return Ok(None);
        }
        Ok(Some(TakeoverReceipt::new(
            &operation,
            TakeoverOutcome::Unchanged,
            Some(operation.clone()),
            None,
        )))
    }

    #[must_use]
    pub fn exact_marker_operation_id(&self) -> Option<String> {
        self.raw()
            .ok()
            .flatten()
            .and_then(|raw| LiveDnsMarker::parse(&raw).ok().flatten())
            .map(|marker| marker.host_effect.operation_id)
    }

    pub fn prepare_after_restore(
        &self,
        prior: &PreparedLinuxDnsEffect,
    ) -> Result<PreparedLinuxDnsEffect, String> {
        let next = self.prepare_effect(prior.enabled)?;
        if next.marker_path != prior.marker_path
            || next.marker_raw.is_some()
            || next.before.target() != prior.before.target()
            || next.before.link != prior.before.link
            || next.before.owned
            || next.before.cleanup_pending
            || next.before.settings != prior.original
            || next.original != prior.original
        {
            return Err(
                "Linux DNS birth/link/baseline changed after exact prior-owner restoration".into(),
            );
        }
        Ok(next)
    }

    pub fn effect_facts_match(&self, prepared: &PreparedLinuxDnsEffect) -> Result<bool, String> {
        if prepared.marker_path != self.marker.path || !self.raw_matches(&prepared.marker_raw) {
            return Ok(false);
        }
        Ok(self.ops.snapshot()? == prepared.before)
    }

    fn live_intent(
        &self,
        prepared: &PreparedLinuxDnsEffect,
        operation_id: &str,
        phase: &str,
    ) -> Result<String, String> {
        serde_json::to_string(&LiveDnsMarker {
            interface_name: TUN_INTERFACE_NAME.into(),
            server_ip: CONTROLLED_DNS_IP.into(),
            at: now_ms(),
            host_effect: LiveDnsFootprint {
                operation_id: operation_id.into(),
                phase: phase.into(),
                before: prepared.before.clone(),
                original: prepared.original.clone(),
            },
        })
        .map_err(|error| error.to_string())
    }

    pub fn apply_effect(
        &mut self,
        prepared: &PreparedLinuxDnsEffect,
        operation_id: &str,
    ) -> TakeoverReceipt {
        let receipt = |outcome, reason| {
            TakeoverReceipt::new(operation_id, outcome, Some(operation_id.into()), reason)
        };
        if operation_id.trim().is_empty() {
            return receipt(
                TakeoverOutcome::FailedUnchanged,
                Some("empty Linux DNS operation".into()),
            );
        }
        let _marker_lock = match self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
        {
            Ok(lock) => lock,
            Err(error) => {
                return receipt(TakeoverOutcome::FailedUnchanged, Some(error.to_string()))
            }
        };
        match self.effect_facts_match(prepared) {
            Ok(true) => {}
            Ok(false) => {
                return receipt(
                    TakeoverOutcome::Superseded,
                    Some("Linux DNS plan facts changed".into()),
                )
            }
            Err(error) => return receipt(TakeoverOutcome::FailedUnchanged, Some(error)),
        }
        let typed_owned = prepared.marker_raw.as_ref().is_some_and(|raw| {
            LiveDnsMarker::parse(raw)
                .ok()
                .flatten()
                .is_some_and(|marker| marker.host_effect.phase == "owned")
        });
        if !prepared.before.cleanup_pending
            && (prepared.enabled
                && prepared.before.owned
                && prepared.before.settings.controlled()
                && typed_owned
                || !prepared.enabled && !prepared.before.owned && prepared.marker_raw.is_none())
        {
            let mut unchanged = receipt(TakeoverOutcome::Unchanged, None);
            unchanged.marker_id = None;
            return unchanged;
        }
        let intent = match self.live_intent(
            prepared,
            operation_id,
            if prepared.enabled {
                "applying"
            } else {
                "restoring"
            },
        ) {
            Ok(raw) => raw,
            Err(error) => return receipt(TakeoverOutcome::FailedUnchanged, Some(error)),
        };
        if let Err(error) = self.replace_raw(&prepared.marker_raw, &Some(intent.clone())) {
            return receipt(TakeoverOutcome::RecoveryPending, Some(error));
        }
        let mut before = prepared.before.clone();
        let mut cleaned_prior = false;
        // A new explicitly confirmed retry can first discharge an ambiguous
        // partial footprint. It never treats a prior error as restoration.
        if prepared.enabled && before.cleanup_pending {
            match self.ops.compare(&before, false) {
                Ok(Change::Changed(restored))
                    if !restored.owned
                        && !restored.cleanup_pending
                        && restored.settings == prepared.original =>
                {
                    before = restored;
                    cleaned_prior = true;
                }
                Ok(Change::NoChange) => {
                    if self
                        .replace_raw(&Some(intent), &prepared.marker_raw)
                        .is_ok()
                    {
                        return receipt(
                            TakeoverOutcome::Superseded,
                            Some("Linux DNS retry rejected before write".into()),
                        );
                    }
                    return receipt(
                        TakeoverOutcome::RecoveryPending,
                        Some("Linux DNS retry marker changed".into()),
                    );
                }
                other => {
                    return receipt(
                        TakeoverOutcome::RecoveryPending,
                        Some(format!(
                            "Linux DNS prior footprint cleanup unconfirmed: {other:?}"
                        )),
                    )
                }
            }
            if !self.raw_matches(&Some(intent.clone())) {
                return receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("Linux DNS retry owner changed".into()),
                );
            }
        }
        let result = self.ops.compare(&before, prepared.enabled);
        let changed = match &result {
            Ok(Change::Changed(after)) => {
                after.target() == before.target()
                    && after.link == before.link
                    && after.owned == prepared.enabled
                    && !after.cleanup_pending
                    && if prepared.enabled {
                        after.settings.controlled()
                    } else {
                        after.settings == prepared.original
                    }
            }
            Ok(Change::Gone(proof)) => !prepared.enabled && proof.matches(&before),
            _ => false,
        };
        if changed {
            let settled = if prepared.enabled {
                self.live_intent(prepared, operation_id, "owned").map(Some)
            } else {
                Ok(None)
            };
            return match settled.and_then(|target| self.replace_raw(&Some(intent), &target)) {
                Ok(()) => receipt(TakeoverOutcome::Applied, None),
                Err(error) => receipt(TakeoverOutcome::RecoveryPending, Some(error)),
            };
        }
        if matches!(result, Ok(Change::NoChange)) {
            if cleaned_prior {
                return if self.replace_raw(&Some(intent), &None).is_ok() {
                    receipt(
                        TakeoverOutcome::FailedRestored,
                        Some(
                            "Linux DNS prior footprint restored; replacement rejected before write"
                                .into(),
                        ),
                    )
                } else {
                    receipt(
                        TakeoverOutcome::RecoveryPending,
                        Some("Linux DNS restored retry marker changed".into()),
                    )
                };
            }
            return if self
                .replace_raw(&Some(intent), &prepared.marker_raw)
                .is_ok()
            {
                receipt(
                    TakeoverOutcome::Superseded,
                    Some("Linux DNS compare rejected without writing".into()),
                )
            } else {
                receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("Linux DNS no-write marker restoration failed".into()),
                )
            };
        }
        // A transport/native error may already have changed OS state. Read only
        // the same attested resource, then ask for checked original restoration.
        let observed = self
            .ops
            .snapshot()
            .ok()
            .filter(|now| now.target() == before.target() && now.link == before.link);
        let baseline_observed = observed.as_ref().is_some_and(|now| {
            !now.owned && !now.cleanup_pending && now.settings == prepared.original
        });
        let restored = observed
            .filter(|now| now.original.as_ref() == Some(&prepared.original))
            .and_then(|now| self.ops.compare(&now, false).ok());
        if baseline_observed
            || matches!(&restored, Some(Change::Changed(now)) if !now.owned && !now.cleanup_pending && now.settings == prepared.original)
            || matches!(&restored, Some(Change::Gone(proof)) if proof.matches(&before))
        {
            // The old marker cannot remain 'owned' after real restoration.
            if self.replace_raw(&Some(intent), &None).is_ok() {
                return receipt(
                    TakeoverOutcome::FailedRestored,
                    Some("Linux DNS change failed; original resource restored".into()),
                );
            }
        }
        receipt(
            TakeoverOutcome::RecoveryPending,
            Some(format!(
                "Linux DNS mutation/cleanup remains unconfirmed: {result:?}"
            )),
        )
    }

    fn restore_live(&mut self, raw: String, mut marker: LiveDnsMarker) -> Result<(), String> {
        let _marker_lock = self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
            .map_err(|error| error.to_string())?;
        if !self.raw_matches(&Some(raw.clone())) {
            return Err("Linux DNS restore marker owner changed".into());
        }
        // The original child/link may already be gone after Stop. The helper
        // can prove resource absence without finding or touching a successor.
        let current = self.ops.snapshot();
        if current.as_ref().is_ok_and(|snapshot| {
            marker.scope_matches(snapshot)
                && !snapshot.owned
                && !snapshot.cleanup_pending
                && snapshot.settings == marker.host_effect.original
        }) {
            return self.replace_raw(&Some(raw), &None);
        }
        let snapshot = match current {
            Ok(snapshot) if marker.scope_matches(&snapshot) && snapshot.owned => snapshot,
            _ => marker.host_effect.before.clone(),
        };
        marker.host_effect.phase = "restoring".into();
        let restoring = serde_json::to_string(&marker).map_err(|error| error.to_string())?;
        self.replace_raw(&Some(raw), &Some(restoring.clone()))?;
        match self.ops.compare(&snapshot, false)? {
            Change::Gone(proof) if proof.matches(&marker.host_effect.before) => {
                self.replace_raw(&Some(restoring), &None)
            }
            Change::Changed(after)
                if marker.scope_matches(&after)
                    && !after.owned
                    && !after.cleanup_pending
                    && after.settings == marker.host_effect.original =>
            {
                self.replace_raw(&Some(restoring), &None)
            }
            _ => Err("Linux DNS exact restore remains unconfirmed".into()),
        }
    }

    pub fn reconcile_checked(&mut self) -> Result<Option<TakeoverReceipt>, String> {
        let Some(raw) = self.raw()? else {
            return Ok(None);
        };
        self.read_marker_checked()
            .map_err(|error| error.to_string())?;
        let Some(marker) = LiveDnsMarker::parse(&raw)? else {
            self.ops.takeover()?;
            return Ok(None);
        };
        let operation_id = marker.host_effect.operation_id.clone();
        let snapshot = self.ops.snapshot();
        let accepted = snapshot.as_ref().is_ok_and(|current| {
            marker.host_effect.phase == "owned"
                && marker.scope_matches(current)
                && current.owned
                && !current.cleanup_pending
                && current.settings.controlled()
        });
        Ok(Some(TakeoverReceipt::new(
            operation_id.clone(),
            if accepted {
                TakeoverOutcome::Unchanged
            } else {
                TakeoverOutcome::RecoveryPending
            },
            Some(operation_id),
            (!accepted).then(|| format!("Linux DNS exact reconcile unresolved: {snapshot:?}")),
        )))
    }

    /// 先写 intent，再请求 helper；失败后明确恢复成功才清 intent，否则保留恢复责任。
    pub fn takeover(&mut self) -> Result<(), String> {
        self.read_marker_checked()
            .map_err(|error| error.to_string())?;
        if self
            .raw()?
            .as_deref()
            .map(LiveDnsMarker::parse)
            .transpose()?
            .flatten()
            .is_some()
        {
            return Err("exact Linux DNS marker requires checked ownership".into());
        }
        self.marker.write()?;
        if let Err(error) = self.ops.takeover() {
            // A failed helper call is not a rollback receipt. Keep intent while
            // DNS responsibility is unresolved, so startup recovery can retry.
            if let Err(revert_error) = self.ops.revert() {
                return Err(format!("{error}; DNS rollback unconfirmed: {revert_error}"));
            }
            let clear = self.marker.clear();
            return match clear {
                Ok(()) => Err(error),
                Err(clear_error) => Err(format!("{error}; {clear_error}")),
            };
        }
        Ok(())
    }

    /// 有 marker 才恢复；恢复成功后再清 marker。失败保留 marker 给下一次启动继续恢复。
    pub fn restore(&mut self) -> Result<(), String> {
        if let Some(raw) = self.raw()? {
            if let Some(marker) = LiveDnsMarker::parse(&raw)? {
                return self.restore_live(raw, marker);
            }
        }
        if self
            .read_marker_checked()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
        self.ops.revert()?;
        self.marker.clear()
    }

    /// 网络变化时，仅在接管 intent 仍存在的条件下幂等重放。
    pub fn reconcile(&mut self) -> Result<(), String> {
        self.reconcile_checked().map(|_| ())
    }

    /// 是否存在有效接管 marker。
    #[must_use]
    pub fn has_marker(&self) -> bool {
        self.read_marker_checked()
            .is_ok_and(|marker| marker.is_some())
    }

    /// 仅返回这次原 FS 读取的结果；marker 不存在本身不证明宿主资源已经恢复。
    pub fn read_marker_checked(
        &self,
    ) -> Result<Option<LinuxResolvedMarkerData>, LinuxResolvedMarkerError> {
        self.marker.read_checked()
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;

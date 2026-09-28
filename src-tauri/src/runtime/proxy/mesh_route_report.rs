//! 运行核 force-route 报告取材。只保存最终生成轮的脱敏审计与外化文件基线。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::Ordering;

use polaris_config_engine::builder::endpoint_routes::{
    resolve_mesh_route_snapshot, ForceRouteLeg, MeshRouteCandidate, MeshRouteConfigSource,
    MeshRouteEmissionCandidate, MeshRouteFileSnapshot, MeshRouteLoadEvidence, MeshRouteReport,
    MeshRouteScope, MeshRouteSnapshot, MeshRouteSource, MeshRouteUnknownReason, SourcedCidr,
    MAX_MESH_ROUTE_REPORT_CANDIDATES, MAX_MESH_ROUTE_REPORT_CIDRS,
    MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES,
};
use polaris_config_engine::builder::InvalidNode;
use polaris_config_engine::singbox::{OneOrMany, SingBoxConfig};
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::cidr::normalize_cidr;
use polaris_config_engine::user_config::server_config::is_mesh_node;

use super::lifecycle::now_ms;
use super::ProxyRuntime;

const MAX_RULE_FILE_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_RULE_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CORE_CONFIG_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct MeshRouteRunEvidence {
    pub(super) candidates: Vec<MeshRouteEmissionCandidate>,
    pub(super) total_candidate_count: usize,
    pub(super) diagnostics_limited: bool,
    pub(super) file_baselines: BTreeMap<String, String>,
    pub(super) core_config_path: String,
    pub(super) core_config_sha256: Option<String>,
    pub(super) write_epoch: u64,
    pub(super) run_generation: u64,
    pub(super) ready_at_ms: Option<u64>,
    pub(super) load_evidence: MeshRouteLoadEvidence,
    pub(super) eligible_for_ack: bool,
    pub(super) dns_owner_server_id: Option<String>,
    pub(super) preceding_exceptions: Vec<String>,
}

fn read_bounded(path: &str, limit: usize) -> Result<String, MeshRouteUnknownReason> {
    let file = std::fs::File::open(path).map_err(|_| MeshRouteUnknownReason::FileReadFailed)?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| MeshRouteUnknownReason::FileReadFailed)?;
    if bytes.len() > limit {
        return Err(MeshRouteUnknownReason::ResourceLimitExceeded);
    }
    String::from_utf8(bytes).map_err(|_| MeshRouteUnknownReason::FileParseFailed)
}

fn parse_cidrs(content: &str) -> Result<Vec<String>, MeshRouteUnknownReason> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|_| MeshRouteUnknownReason::FileParseFailed)?;
    let root = value
        .as_object()
        .ok_or(MeshRouteUnknownReason::FileParseFailed)?;
    if root.len() != 2 || root.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(MeshRouteUnknownReason::FileParseFailed);
    }
    let rules = value
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .ok_or(MeshRouteUnknownReason::FileParseFailed)?;
    let mut cidrs = Vec::new();
    for rule in rules {
        let object = rule
            .as_object()
            .ok_or(MeshRouteUnknownReason::FileParseFailed)?;
        if object.len() != 1 {
            return Err(MeshRouteUnknownReason::FileParseFailed);
        }
        let values = rule
            .get("ip_cidr")
            .and_then(serde_json::Value::as_array)
            .ok_or(MeshRouteUnknownReason::FileParseFailed)?;
        for value in values {
            let text = value
                .as_str()
                .ok_or(MeshRouteUnknownReason::FileParseFailed)?;
            if text.len() > MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES
                || cidrs.len() >= MAX_MESH_ROUTE_REPORT_CIDRS
            {
                return Err(MeshRouteUnknownReason::ResourceLimitExceeded);
            }
            cidrs.push(text.to_owned());
        }
    }
    Ok(cidrs)
}

fn source_for_file_cidr(
    configured: &BTreeSet<String>,
    observed: &BTreeSet<String>,
    has_observed_hosts: bool,
    cidr: &str,
) -> MeshRouteSource {
    let Some(normalized) = normalize_cidr(cidr) else {
        return MeshRouteSource::Retained;
    };
    if configured.contains(&normalized) {
        MeshRouteSource::Declared
    } else if observed.contains(&normalized) {
        MeshRouteSource::Observed
    } else if !has_observed_hosts
        && matches!(normalized.as_str(), "100.64.0.0/10" | "fd7a:115c:a1e0::/48")
    {
        MeshRouteSource::Bootstrap
    } else {
        MeshRouteSource::Retained
    }
}

fn source_sets(candidate: &MeshRouteCandidate) -> (BTreeSet<String>, BTreeSet<String>) {
    (
        candidate
            .configured_cidrs
            .iter()
            .filter_map(|cidr| normalize_cidr(cidr))
            .collect(),
        candidate
            .observed_hosts
            .iter()
            .filter_map(|host| normalize_cidr(host))
            .collect(),
    )
}

fn actual_rule_present(config: &SingBoxConfig, emitted: &MeshRouteEmissionCandidate) -> bool {
    if !config.endpoints.as_ref().is_some_and(|endpoints| {
        endpoints
            .iter()
            .any(|item| item.tag == emitted.candidate.tag)
    }) {
        return false;
    }
    let Some(route) = config.route.as_ref() else {
        return false;
    };
    let candidate = &emitted.candidate;
    match candidate.leg {
        ForceRouteLeg::Inline if emitted.emitted_inline.is_empty() => true,
        ForceRouteLeg::Inline => route.rules.iter().any(|rule| {
            rule.outbound.as_deref() == Some(candidate.tag.as_str())
                && rule.ip_cidr.as_deref() == Some(emitted.emitted_inline.as_slice())
        }),
        ForceRouteLeg::PreferredBy => route.rules.iter().any(|rule| {
            rule.outbound.as_deref() == Some(candidate.tag.as_str())
                && rule.preferred_by.as_deref() == Some(std::slice::from_ref(&candidate.tag))
        }),
        ForceRouteLeg::ExternalRuleSet => {
            let Some(path) = emitted.external_path.as_deref() else {
                return false;
            };
            let Some(rule_set) = route
                .rule_set
                .as_ref()
                .and_then(|items| items.iter().find(|item| item.path.as_deref() == Some(path)))
            else {
                return false;
            };
            route.rules.iter().any(|rule| {
                rule.outbound.as_deref() == Some(candidate.tag.as_str())
                    && matches!(&rule.rule_set, Some(OneOrMany::One(tag)) if tag == &rule_set.tag)
            })
        }
    }
}

impl ProxyRuntime {
    /// 最终 gate 产物确定后、spawn 前冻结；只读诊断失败不妨碍原代理启动。
    pub(super) fn prepare_mesh_route_run(
        &self,
        mut candidates: Vec<MeshRouteEmissionCandidate>,
        total_candidate_count: usize,
        diagnostics_limited: bool,
        dns_owner_server_id: Option<String>,
        invalid_nodes: &[InvalidNode],
        original_config: &UserConfig,
        core_config: &SingBoxConfig,
        core_config_path: &Path,
        core_config_json: &str,
        run_generation: u64,
    ) -> MeshRouteRunEvidence {
        let mut limited = diagnostics_limited;
        let generated_ids: BTreeSet<&str> = candidates
            .iter()
            .map(|item| item.candidate.server_id.as_str())
            .collect();
        let failed_ids: BTreeSet<&str> = invalid_nodes
            .iter()
            .filter(|invalid| {
                original_config
                    .servers
                    .iter()
                    .any(|server| server.id == invalid.id && is_mesh_node(server))
            })
            .map(|invalid| invalid.id.as_str())
            .collect();
        let total_candidate_count =
            total_candidate_count.saturating_add(failed_ids.difference(&generated_ids).count());
        for invalid in invalid_nodes {
            if candidates
                .iter()
                .any(|item| item.candidate.server_id == invalid.id)
            {
                continue;
            }
            if !original_config
                .servers
                .iter()
                .any(|server| server.id == invalid.id && is_mesh_node(server))
            {
                continue;
            }
            if candidates.len() >= MAX_MESH_ROUTE_REPORT_CANDIDATES
                || invalid.id.len() > MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES
                || invalid.tag.len() > MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES
                || invalid.reason.len() > MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES
            {
                limited = true;
                break;
            }
            candidates.push(MeshRouteEmissionCandidate {
                candidate: MeshRouteCandidate {
                    server_id: invalid.id.clone(),
                    tag: invalid.tag.clone(),
                    leg: ForceRouteLeg::Inline,
                    generated: false,
                    generation_reason: Some(invalid.reason.clone()),
                    engaged_reason: "generationFailed".into(),
                    configured_cidrs: Vec::new(),
                    observed_hosts: Vec::new(),
                    referenced_file: None,
                    match_cidrs: None,
                    unknown_reasons: vec![MeshRouteUnknownReason::GenerationFailed],
                },
                external_path: None,
                emitted_inline: Vec::new(),
            });
        }
        if candidates.len() > MAX_MESH_ROUTE_REPORT_CANDIDATES {
            limited = true;
        }
        let mut preceding_exceptions = Vec::new();
        if original_config
            .ordered_traffic_rules()
            .iter()
            .any(|rule| rule.enabled)
        {
            preceding_exceptions.push("customRuleMayOverride".into());
        }
        if original_config.app_rules.iter().any(|rule| rule.enabled) {
            preceding_exceptions.push("appRuleMayOverride".into());
        }
        let epoch_before = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        let core_file = if core_config_json.len() <= MAX_CORE_CONFIG_BYTES {
            read_bounded(&core_config_path.to_string_lossy(), MAX_CORE_CONFIG_BYTES).ok()
        } else {
            limited = true;
            None
        };
        let core_config_sha256 = core_file
            .as_ref()
            .filter(|content| content.as_str() == core_config_json)
            .map(|content| polaris_updater::verify::sha256_hex(content.as_bytes()));
        let mut file_baselines = BTreeMap::new();
        let mut remaining_file_bytes = MAX_TOTAL_RULE_FILE_BYTES;
        let mut load_evidence = MeshRouteLoadEvidence::Unknown;
        let mut eligible_for_ack = !limited && core_config_sha256.is_some();
        if core_file
            .as_ref()
            .is_some_and(|content| content != core_config_json)
        {
            load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
        }
        if !limited {
            for emitted in &mut candidates {
                if !actual_rule_present(core_config, emitted) {
                    eligible_for_ack = false;
                    emitted.candidate.match_cidrs = None;
                    emitted
                        .candidate
                        .unknown_reasons
                        .push(MeshRouteUnknownReason::RunningArtifactUnavailable);
                    continue;
                }
                let Some(path) = emitted.external_path.as_deref() else {
                    continue;
                };
                let limit = remaining_file_bytes.min(MAX_RULE_FILE_BYTES);
                match read_bounded(path, limit).and_then(|content| {
                    let cidrs = parse_cidrs(&content)?;
                    Ok((content, cidrs))
                }) {
                    Ok((content, cidrs)) => {
                        remaining_file_bytes -= content.len();
                        let (configured, observed) = source_sets(&emitted.candidate);
                        emitted.candidate.referenced_file = Some(MeshRouteFileSnapshot {
                            cidrs: Some(cidrs.clone()),
                            read_error: None,
                            sampled_at_ms: Some(now_ms()),
                        });
                        emitted.candidate.match_cidrs = Some(
                            cidrs
                                .iter()
                                .map(|cidr| SourcedCidr {
                                    cidr: cidr.clone(),
                                    source: source_for_file_cidr(
                                        &configured,
                                        &observed,
                                        !emitted.candidate.observed_hosts.is_empty(),
                                        cidr,
                                    ),
                                })
                                .collect(),
                        );
                        file_baselines.insert(path.to_owned(), content);
                    }
                    Err(reason) => {
                        eligible_for_ack = false;
                        emitted.candidate.referenced_file = Some(MeshRouteFileSnapshot {
                            cidrs: None,
                            read_error: Some(format!("{reason:?}")),
                            sampled_at_ms: Some(now_ms()),
                        });
                        emitted.candidate.match_cidrs = None;
                        emitted.candidate.unknown_reasons.push(reason);
                    }
                }
            }
        } else {
            candidates.clear();
        }
        let epoch_after = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        if epoch_before != epoch_after || epoch_after % 2 != 0 {
            load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
        }
        MeshRouteRunEvidence {
            candidates,
            total_candidate_count,
            diagnostics_limited: limited,
            file_baselines,
            core_config_path: core_config_path.to_string_lossy().into_owned(),
            core_config_sha256,
            write_epoch: epoch_after,
            run_generation,
            ready_at_ms: None,
            load_evidence,
            eligible_for_ack,
            dns_owner_server_id,
            preceding_exceptions,
        }
    }

    /// ready 与同一运行代对账后认领；写代或文件改变则保留磁盘诊断但不授 applied。
    pub(super) fn publish_mesh_route_run(
        &self,
        mut evidence: MeshRouteRunEvidence,
        ready_at_ms: u64,
    ) {
        let epoch = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        let core_config_matches = evidence
            .core_config_sha256
            .as_ref()
            .is_some_and(|expected| {
                read_bounded(&evidence.core_config_path, MAX_CORE_CONFIG_BYTES)
                    .ok()
                    .is_some_and(|content| {
                        polaris_updater::verify::sha256_hex(content.as_bytes()) == *expected
                    })
            });
        let files_match = evidence.file_baselines.iter().all(|(path, baseline)| {
            read_bounded(path, MAX_RULE_FILE_BYTES).as_deref() == Ok(baseline.as_str())
        });
        let epoch_after = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        if evidence.eligible_for_ack
            && self.gate.generation() == evidence.run_generation
            && epoch == epoch_after
            && epoch == evidence.write_epoch
            && epoch % 2 == 0
            && evidence.load_evidence == MeshRouteLoadEvidence::Unknown
            && core_config_matches
            && files_match
        {
            evidence.load_evidence = MeshRouteLoadEvidence::StartupReady;
        } else if epoch != epoch_after
            || epoch != evidence.write_epoch
            || epoch % 2 != 0
            || (evidence.core_config_sha256.is_some() && !core_config_matches)
            || !files_match
        {
            evidence.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
        }
        evidence.ready_at_ms = Some(ready_at_ms);
        if let Ok(mut slot) = self.mesh_route_run.write() {
            if self.gate.generation() != evidence.run_generation {
                return;
            }
            *slot = Some(evidence);
        }
    }

    /// 调用方填入与启动快照对应的内容版本；报告读文件值，写代变化即撤销 applied 断言。
    pub(crate) fn mesh_route_report(&self, config_version: Option<String>) -> MeshRouteReport {
        let status = self.status();
        let evidence = self
            .mesh_route_run
            .read()
            .ok()
            .and_then(|slot| slot.clone());
        let mut snapshot = MeshRouteSnapshot {
            scope: MeshRouteScope::PersistedUnknown,
            config_source: MeshRouteConfigSource::Persisted,
            config_version: None,
            run_generation: None,
            sampled_at_ms: now_ms(),
            load_evidence: MeshRouteLoadEvidence::Unknown,
            snapshot_stale: false,
            dns_owner_server_id: None,
            preceding_exceptions: Vec::new(),
            candidates: Vec::new(),
        };
        let Some(mut evidence) = evidence else {
            let mut report = resolve_mesh_route_snapshot(snapshot);
            report
                .unknown_reasons
                .push(MeshRouteUnknownReason::RunningArtifactUnavailable);
            return report;
        };
        snapshot.config_source = MeshRouteConfigSource::Running;
        snapshot.config_version = config_version;
        snapshot.run_generation = Some(evidence.run_generation);
        snapshot.dns_owner_server_id = evidence.dns_owner_server_id;
        snapshot.preceding_exceptions = evidence.preceding_exceptions;
        snapshot.load_evidence = evidence.load_evidence;
        let before = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        let mut file_failed = false;
        let mut remaining_file_bytes = MAX_TOTAL_RULE_FILE_BYTES;
        for item in &mut evidence.candidates {
            let Some(path) = item.external_path.as_deref() else {
                continue;
            };
            let limit = remaining_file_bytes.min(MAX_RULE_FILE_BYTES);
            match read_bounded(path, limit).and_then(|content| {
                let cidrs = parse_cidrs(&content)?;
                Ok((content, cidrs))
            }) {
                Ok((content, cidrs)) => {
                    remaining_file_bytes -= content.len();
                    let (configured, observed) = source_sets(&item.candidate);
                    if evidence.file_baselines.get(path) != Some(&content) {
                        snapshot.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
                    }
                    item.candidate.referenced_file = Some(MeshRouteFileSnapshot {
                        cidrs: Some(cidrs.clone()),
                        read_error: None,
                        sampled_at_ms: Some(snapshot.sampled_at_ms),
                    });
                    item.candidate.match_cidrs = Some(
                        cidrs
                            .iter()
                            .map(|cidr| SourcedCidr {
                                cidr: cidr.clone(),
                                source: source_for_file_cidr(
                                    &configured,
                                    &observed,
                                    !item.candidate.observed_hosts.is_empty(),
                                    cidr,
                                ),
                            })
                            .collect(),
                    );
                }
                Err(reason) => {
                    file_failed = true;
                    item.candidate.referenced_file = Some(MeshRouteFileSnapshot {
                        cidrs: None,
                        read_error: Some(format!("{reason:?}")),
                        sampled_at_ms: Some(snapshot.sampled_at_ms),
                    });
                    item.candidate.match_cidrs = None;
                    item.candidate.unknown_reasons.push(reason);
                }
            }
        }
        snapshot.candidates = evidence
            .candidates
            .into_iter()
            .map(|item| item.candidate)
            .collect();
        let after = self.tailnet_file_write_epoch.load(Ordering::SeqCst);
        let core_config_matches = evidence
            .core_config_sha256
            .as_ref()
            .is_some_and(|expected| {
                read_bounded(&evidence.core_config_path, MAX_CORE_CONFIG_BYTES)
                    .ok()
                    .is_some_and(|content| {
                        polaris_updater::verify::sha256_hex(content.as_bytes()) == *expected
                    })
            });
        if file_failed {
            snapshot.load_evidence = MeshRouteLoadEvidence::Unknown;
        } else if before != after
            || before % 2 != 0
            || before != evidence.write_epoch
            || (evidence.core_config_sha256.is_some() && !core_config_matches)
        {
            snapshot.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
        }
        if status.running
            && !status.starting
            && status.start_time == evidence.ready_at_ms
            && snapshot.load_evidence == MeshRouteLoadEvidence::StartupReady
        {
            snapshot.scope = MeshRouteScope::Applied;
        }
        if !status.running
            || status.starting
            || status.start_time != evidence.ready_at_ms
            || self.gate.generation() != evidence.run_generation
        {
            snapshot.snapshot_stale = true;
        }
        let mut report = resolve_mesh_route_snapshot(snapshot);
        report.total_candidate_count = evidence.total_candidate_count;
        if evidence.diagnostics_limited
            || report.total_candidate_count > report.snapshot.candidates.len()
        {
            report
                .unknown_reasons
                .push(MeshRouteUnknownReason::ResourceLimitExceeded);
        }
        report
    }
}

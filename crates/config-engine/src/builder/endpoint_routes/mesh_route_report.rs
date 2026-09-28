//! P1 诊断：按已经生成的 force-route 顺序解释 mesh 层 CIDR 覆盖。
//! 输入由宿主拍快照；本模块不读配置、磁盘或运行状态，也不改变现有发射逻辑。

use super::ForceRouteLeg;
use crate::user_config::cidr::{cidr_contains, cidrs_overlap, normalize_cidr, subtract_cidrs};

pub const MAX_MESH_ROUTE_REPORT_CANDIDATES: usize = 256;
pub const MAX_MESH_ROUTE_REPORT_EVIDENCE_BYTES: usize = 2048;
pub const MAX_MESH_ROUTE_REPORT_CIDRS: usize = 4096;
const MAX_REPORT_CIDRS: usize = MAX_MESH_ROUTE_REPORT_CIDRS;
const MAX_REPORT_WORK: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteScope {
    Preview,
    Applied,
    PersistedUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteConfigSource {
    Draft,
    Running,
    Persisted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteLoadEvidence {
    StartupReady,
    FileWrittenUnacknowledged,
    LoadFailed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteSource {
    Declared,
    Observed,
    Retained,
    Bootstrap,
    Opaque,
}

/// 供 UI 本地化的稳定机器码；原始 FS 错误只放在 readError 细节里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteUnknownReason {
    OpaqueMatchSet,
    EarlierOpaqueMatchSet,
    InvalidCidr,
    InputLimitExceeded,
    ResourceLimitExceeded,
    SnapshotStale,
    FileReadFailed,
    FileParseFailed,
    FileLoadUnacknowledged,
    RunningArtifactUnavailable,
    GenerationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcedCidr {
    pub cidr: String,
    pub source: MeshRouteSource,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteFileSnapshot {
    /// None 表示读取失败或未读，不能解释成空文件。
    pub cidrs: Option<Vec<String>>,
    pub read_error: Option<String>,
    pub sampled_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteCandidate {
    pub server_id: String,
    pub tag: String,
    pub leg: ForceRouteLeg,
    /// 构造失败的节点保留诊断，但绝不参与后续 first-match 争用。
    pub generated: bool,
    pub generation_reason: Option<String>,
    pub engaged_reason: String,
    pub configured_cidrs: Vec<String>,
    pub observed_hosts: Vec<String>,
    pub referenced_file: Option<MeshRouteFileSnapshot>,
    /// 宿主从本轮实际规则腿选出的唯一参与集；None 是未知，不能解释为空集。
    pub match_cidrs: Option<Vec<SourcedCidr>>,
    pub unknown_reasons: Vec<MeshRouteUnknownReason>,
}

/// 生成器块 0c 同轮交给宿主的窄审计；路径仅在本地进程内使用，不进入 wire。
#[derive(Debug, Clone)]
pub struct MeshRouteEmissionCandidate {
    pub candidate: MeshRouteCandidate,
    pub external_path: Option<String>,
    pub emitted_inline: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteSnapshot {
    pub scope: MeshRouteScope,
    /// Draft(D) 或本次运行配置(R) 的既有内容哈希，不是采样时间。
    pub config_source: MeshRouteConfigSource,
    pub config_version: Option<String>,
    pub run_generation: Option<u64>,
    pub sampled_at_ms: u64,
    pub load_evidence: MeshRouteLoadEvidence,
    /// 取材期间配置或运行代改变时置 true；此报告不再自称 applied。
    pub snapshot_stale: bool,
    /// 当前单个 `dns-tailscale` 实际指向的节点；无可证实 owner 时为 null。
    pub dns_owner_server_id: Option<String>,
    /// 这里仅解释 mesh force-route 层；前置 custom/app 等规则以例外摘要单列。
    pub preceding_exceptions: Vec<String>,
    /// 必须是本轮实际发射顺序；resolver 不重排。
    pub candidates: Vec<MeshRouteCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteCoverage {
    Full,
    Partial,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRouteRelation {
    Equal,
    RequestedWithinEarlier,
    EarlierWithinRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteBlockedBy {
    pub server_id: String,
    pub cidr: String,
    pub relation: MeshRouteRelation,
    /// 只记录此前没有 opaque 前驱的确定 mesh 层归属，故这里恒 true。
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteResolution {
    pub server_id: String,
    pub requested: Vec<SourcedCidr>,
    /// None 表示不能证明；Some([]) 才表示本层确定为零覆盖。
    pub effective: Option<Vec<String>>,
    pub blocked_by: Vec<MeshRouteBlockedBy>,
    pub coverage: MeshRouteCoverage,
    pub invalid: Vec<String>,
    pub excluded_catch_all: Vec<String>,
    pub unknown_reasons: Vec<MeshRouteUnknownReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteReport {
    pub schema_version: u8,
    pub snapshot: MeshRouteSnapshot,
    pub results: Vec<MeshRouteResolution>,
    /// 全局不完整原因；非空时 UI 不得把未列出的候选读成零冲突。
    pub unknown_reasons: Vec<MeshRouteUnknownReason>,
    pub total_candidate_count: usize,
}

fn subtract_limited(
    mut pieces: Vec<String>,
    carves: &[String],
    work_left: &mut usize,
) -> Option<Vec<String>> {
    for carve in carves {
        let mut next = Vec::new();
        for piece in pieces {
            if *work_left == 0 {
                return None;
            }
            *work_left -= 1;
            // 一次只让现有差集引擎处理一条 base 与一条 carve，避免全量调用的中间峰值。
            let split = subtract_cidrs(&[piece], std::slice::from_ref(carve));
            if next.len().saturating_add(split.len()) > MAX_REPORT_CIDRS {
                return None;
            }
            next.extend(split);
        }
        pieces = next;
        if pieces.is_empty() {
            break;
        }
    }
    Some(pieces)
}

/// 已知 CIDR 两两相交时，交集是更具体的一条；两族不相交。
fn intersection(requested: &str, earlier: &str) -> Option<(String, MeshRouteRelation)> {
    if !cidrs_overlap(requested, earlier) {
        return None;
    }
    if requested == earlier {
        Some((requested.to_owned(), MeshRouteRelation::Equal))
    } else if cidr_contains(earlier, requested) {
        Some((
            requested.to_owned(),
            MeshRouteRelation::RequestedWithinEarlier,
        ))
    } else {
        Some((
            earlier.to_owned(),
            MeshRouteRelation::EarlierWithinRequested,
        ))
    }
}

/// 纯 resolver：只解释已提供的实际候选，不重新推断文件值、候选顺序或内核运行状态。
#[must_use]
pub fn resolve_mesh_route_snapshot(mut snapshot: MeshRouteSnapshot) -> MeshRouteReport {
    if snapshot.candidates.len() > MAX_MESH_ROUTE_REPORT_CANDIDATES {
        let total_candidate_count = snapshot.candidates.len();
        snapshot.candidates.clear();
        return MeshRouteReport {
            schema_version: 1,
            snapshot,
            results: Vec::new(),
            unknown_reasons: vec![MeshRouteUnknownReason::ResourceLimitExceeded],
            total_candidate_count,
        };
    }
    // 磁盘写入及运行代存在都不等于内核加载。非法 scope 组合安全降级。
    if snapshot.scope == MeshRouteScope::Applied
        && (snapshot.run_generation.is_none()
            || snapshot
                .config_version
                .as_ref()
                .is_none_or(String::is_empty)
            || snapshot.load_evidence != MeshRouteLoadEvidence::StartupReady
            || snapshot.config_source != MeshRouteConfigSource::Running
            || snapshot.snapshot_stale)
    {
        snapshot.scope = MeshRouteScope::PersistedUnknown;
    }
    // 当前磁盘文件可能尚未被运行核 reload。保留其 requested 证据，但不能给它
    // effective/confirmed owner；preview 是明确的假设计算，只有它允许离线求差。
    let unacknowledged = snapshot.scope == MeshRouteScope::PersistedUnknown;

    let mut results = Vec::with_capacity(snapshot.candidates.len());
    let mut confirmed_owners: Vec<(String, Vec<String>)> = Vec::new();
    let mut opaque_predecessor = false;
    let mut input_count = 0usize;
    let mut output_count = 0usize;
    let mut work_left = MAX_REPORT_WORK;
    for candidate in &snapshot.candidates {
        let mut result = MeshRouteResolution {
            server_id: candidate.server_id.clone(),
            requested: Vec::new(),
            effective: None,
            blocked_by: Vec::new(),
            coverage: MeshRouteCoverage::Unknown,
            invalid: Vec::new(),
            excluded_catch_all: Vec::new(),
            unknown_reasons: candidate.unknown_reasons.clone(),
        };
        if !candidate.generated {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::GenerationFailed);
            results.push(result);
            continue;
        }
        let raw = candidate.match_cidrs.as_deref().unwrap_or_default();
        input_count = input_count.saturating_add(raw.len());
        if input_count > MAX_REPORT_CIDRS {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::InputLimitExceeded);
        }
        if candidate.match_cidrs.is_none() || candidate.leg == ForceRouteLeg::PreferredBy {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::OpaqueMatchSet);
        }
        if input_count <= MAX_REPORT_CIDRS {
            for entry in raw {
                match normalize_cidr(&entry.cidr) {
                    Some(cidr) if cidr == "0.0.0.0/0" || cidr == "::/0" => {
                        result.excluded_catch_all.push(cidr);
                    }
                    Some(cidr) => {
                        let normalized = SourcedCidr {
                            cidr,
                            source: entry.source,
                        };
                        if !result.requested.contains(&normalized) {
                            result.requested.push(normalized);
                        }
                    }
                    None => result.invalid.push(entry.cidr.clone()),
                }
            }
        }
        if !result.invalid.is_empty() {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::InvalidCidr);
        }
        if snapshot.snapshot_stale {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::SnapshotStale);
        }
        if unacknowledged {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::FileLoadUnacknowledged);
        }
        if opaque_predecessor {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::EarlierOpaqueMatchSet);
        }

        // 已证明的冲突只来自第一个 opaque 前驱之前的 owner；后续未知 owner 不冒名确权。
        'blocked: for requested in &result.requested {
            for (server_id, owned) in &confirmed_owners {
                for earlier in owned {
                    if work_left == 0 || output_count >= MAX_REPORT_CIDRS {
                        result.blocked_by.clear();
                        result
                            .unknown_reasons
                            .push(MeshRouteUnknownReason::ResourceLimitExceeded);
                        break 'blocked;
                    }
                    work_left -= 1;
                    if let Some((cidr, relation)) = intersection(&requested.cidr, earlier) {
                        let blocked = MeshRouteBlockedBy {
                            server_id: server_id.clone(),
                            cidr,
                            relation,
                            confirmed: true,
                        };
                        if !result.blocked_by.contains(&blocked) {
                            result.blocked_by.push(blocked);
                            output_count += 1;
                        }
                    }
                }
            }
        }
        if !result.unknown_reasons.is_empty() {
            opaque_predecessor = true;
            results.push(result);
            continue;
        }

        let mut effective = Vec::new();
        let mut limit_exceeded = false;
        for requested in &result.requested {
            let mut pieces = vec![requested.cidr.clone()];
            for (_, owned) in &confirmed_owners {
                let Some(next) =
                    subtract_limited(std::mem::take(&mut pieces), owned, &mut work_left)
                else {
                    limit_exceeded = true;
                    break;
                };
                pieces = next;
            }
            if limit_exceeded {
                break;
            }
            let Some(next) = subtract_limited(pieces, &effective, &mut work_left) else {
                limit_exceeded = true;
                break;
            };
            effective.extend(next);
            if output_count.saturating_add(effective.len()) > MAX_REPORT_CIDRS {
                limit_exceeded = true;
                break;
            }
        }
        if limit_exceeded {
            result
                .unknown_reasons
                .push(MeshRouteUnknownReason::ResourceLimitExceeded);
            opaque_predecessor = true;
        } else {
            result.coverage = if effective.is_empty() {
                MeshRouteCoverage::None
            } else if result.blocked_by.is_empty() {
                MeshRouteCoverage::Full
            } else {
                MeshRouteCoverage::Partial
            };
            result.effective = Some(effective.clone());
            output_count += effective.len();
            confirmed_owners.push((candidate.server_id.clone(), effective));
        }
        results.push(result);
    }
    MeshRouteReport {
        schema_version: 1,
        total_candidate_count: snapshot.candidates.len(),
        snapshot,
        results,
        unknown_reasons: Vec::new(),
    }
}

#[cfg(test)]
mod tests;

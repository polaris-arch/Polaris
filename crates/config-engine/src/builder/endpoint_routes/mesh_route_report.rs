//! P1 诊断：按已经生成的 force-route 顺序解释 mesh 层 CIDR 覆盖。
//! 输入由宿主拍快照；本模块不读配置、磁盘或运行状态，也不改变现有发射逻辑。

use super::ForceRouteLeg;
use crate::user_config::cidr::{cidr_contains, cidrs_overlap, normalize_cidr, subtract_cidrs};

const MAX_REPORT_CIDRS: usize = 4096;
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
        snapshot,
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn claim(id: &str, leg: ForceRouteLeg, cidrs: Option<&[&str]>) -> MeshRouteCandidate {
        MeshRouteCandidate {
            server_id: id.into(),
            tag: format!("endpoint-{id}"),
            leg,
            generated: true,
            generation_reason: None,
            engaged_reason: "alwaysRouteSubnets".into(),
            configured_cidrs: vec![],
            observed_hosts: vec![],
            referenced_file: None,
            match_cidrs: cidrs.map(|xs| {
                xs.iter()
                    .map(|x| SourcedCidr {
                        cidr: (*x).into(),
                        source: MeshRouteSource::Declared,
                    })
                    .collect()
            }),
            unknown_reasons: vec![],
        }
    }

    fn snapshot(candidates: Vec<MeshRouteCandidate>) -> MeshRouteSnapshot {
        MeshRouteSnapshot {
            scope: MeshRouteScope::Preview,
            config_source: MeshRouteConfigSource::Draft,
            config_version: Some("test-version".into()),
            run_generation: None,
            sampled_at_ms: 1,
            load_evidence: MeshRouteLoadEvidence::Unknown,
            snapshot_stale: false,
            dns_owner_server_id: None,
            preceding_exceptions: vec!["customRuleMayOverride".into()],
            candidates,
        }
    }

    // 独立 oracle：逐地址按原始规则顺序 first-match，不调用 CIDR 差集/包含实现。
    fn first_match_v4(candidates: &[MeshRouteCandidate], addr: Ipv4Addr) -> Option<&str> {
        let ip = u32::from(addr);
        for candidate in candidates {
            if !candidate.generated {
                continue;
            }
            for entry in candidate.match_cidrs.as_ref()? {
                let (raw, bits) = entry.cidr.split_once('/')?;
                let bits: u32 = bits.parse().ok()?;
                let network = u32::from(raw.parse::<Ipv4Addr>().ok()?);
                let mask = if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                };
                if ip & mask == network & mask {
                    return Some(&candidate.server_id);
                }
            }
        }
        None
    }

    #[test]
    fn nested_and_partial_overlap_match_independent_first_match() {
        let candidates = vec![
            claim("a", ForceRouteLeg::ExternalRuleSet, Some(&["10.20.0.1/16"])),
            claim(
                "b",
                ForceRouteLeg::Inline,
                Some(&["10.20.1.99/24", "10.21.0.0/16"]),
            ),
        ];
        let report = resolve_mesh_route_snapshot(snapshot(candidates.clone()));
        assert_eq!(report.results[0].requested[0].cidr, "10.20.0.0/16");
        assert_eq!(report.results[1].coverage, MeshRouteCoverage::Partial);
        assert_eq!(
            report.results[1].effective,
            Some(vec!["10.21.0.0/16".into()])
        );
        assert_eq!(report.results[1].blocked_by[0].server_id, "a");
        assert_eq!(report.results[1].blocked_by[0].cidr, "10.20.1.0/24");
        for (ip, expected) in [("10.20.1.5", "a"), ("10.20.9.1", "a"), ("10.21.1.1", "b")] {
            let ip = ip.parse().unwrap();
            assert_eq!(first_match_v4(&candidates, ip), Some(expected));
            let resolved_owner = report.results.iter().find(|r| {
                r.effective.as_ref().is_some_and(|xs| {
                    xs.iter().any(|x| {
                        let (network, bits) = x.split_once('/').unwrap();
                        let bits: u32 = bits.parse().unwrap();
                        let mask = if bits == 0 {
                            0
                        } else {
                            u32::MAX << (32 - bits)
                        };
                        u32::from(ip) & mask
                            == u32::from(network.parse::<Ipv4Addr>().unwrap()) & mask
                    })
                })
            });
            assert_eq!(resolved_owner.map(|r| r.server_id.as_str()), Some(expected));
        }
    }

    #[test]
    fn external_and_inline_have_identical_conflict_semantics() {
        for legs in [
            [ForceRouteLeg::ExternalRuleSet, ForceRouteLeg::Inline],
            [ForceRouteLeg::Inline, ForceRouteLeg::ExternalRuleSet],
            [
                ForceRouteLeg::ExternalRuleSet,
                ForceRouteLeg::ExternalRuleSet,
            ],
        ] {
            let report = resolve_mesh_route_snapshot(snapshot(vec![
                claim("a", legs[0], Some(&["fd7a:115c:a1e0:0::1/64"])),
                claim("b", legs[1], Some(&["fd7a:115c:a1e0::2/128"])),
            ]));
            assert_eq!(report.results[1].coverage, MeshRouteCoverage::None);
            assert_eq!(report.results[1].effective, Some(vec![]));
            assert_eq!(
                report.results[1].blocked_by[0].cidr,
                "fd7a:115c:a1e0::2/128"
            );
        }
    }

    #[test]
    fn opaque_predecessor_keeps_later_effective_null_without_false_owner() {
        let report = resolve_mesh_route_snapshot(snapshot(vec![
            claim("unknown", ForceRouteLeg::PreferredBy, None),
            claim("b", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"])),
            claim("c", ForceRouteLeg::ExternalRuleSet, Some(&["10.1.0.0/16"])),
        ]));
        assert_eq!(report.results[1].effective, None);
        assert_eq!(report.results[2].effective, None);
        assert!(report.results[2].blocked_by.is_empty());
    }

    #[test]
    fn bad_input_and_catch_all_are_not_reported_as_full_coverage() {
        let report = resolve_mesh_route_snapshot(snapshot(vec![
            claim(
                "a",
                ForceRouteLeg::Inline,
                Some(&["0.0.0.1/0", "010.0.0.1/8", "10.0.0.0/8"]),
            ),
            claim("b", ForceRouteLeg::Inline, Some(&["192.168.0.0/16"])),
        ]));
        assert_eq!(report.results[0].excluded_catch_all, vec!["0.0.0.0/0"]);
        assert_eq!(report.results[0].invalid, vec!["010.0.0.1/8"]);
        assert_eq!(report.results[0].effective, None);
        assert_eq!(report.results[1].coverage, MeshRouteCoverage::Unknown);
    }

    #[test]
    fn failed_generation_does_not_claim_or_poison_following_candidate() {
        let mut failed = claim("a", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"]));
        failed.generated = false;
        failed.generation_reason = Some("endpointBuildFailed".into());
        let report = resolve_mesh_route_snapshot(snapshot(vec![
            failed,
            claim("b", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"])),
        ]));
        assert_eq!(report.results[0].coverage, MeshRouteCoverage::Unknown);
        assert_eq!(report.results[1].coverage, MeshRouteCoverage::Full);
    }

    #[test]
    fn applied_requires_running_generation_and_ack() {
        let mut input = snapshot(vec![claim(
            "a",
            ForceRouteLeg::Inline,
            Some(&["10.0.0.0/8"]),
        )]);
        input.scope = MeshRouteScope::Applied;
        input.config_source = MeshRouteConfigSource::Running;
        input.run_generation = Some(12);
        input.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
        assert_eq!(
            resolve_mesh_route_snapshot(input).snapshot.scope,
            MeshRouteScope::PersistedUnknown
        );
    }

    #[test]
    fn stale_or_unversioned_running_snapshot_never_confirms_owner() {
        let mut input = snapshot(vec![claim(
            "a",
            ForceRouteLeg::Inline,
            Some(&["10.0.0.0/8"]),
        )]);
        input.scope = MeshRouteScope::Applied;
        input.config_source = MeshRouteConfigSource::Running;
        input.run_generation = Some(12);
        input.load_evidence = MeshRouteLoadEvidence::StartupReady;
        input.snapshot_stale = true;
        let stale = resolve_mesh_route_snapshot(input.clone());
        assert_eq!(stale.snapshot.scope, MeshRouteScope::PersistedUnknown);
        assert_eq!(stale.results[0].effective, None);
        assert_eq!(stale.results[0].coverage, MeshRouteCoverage::Unknown);
        input.snapshot_stale = false;
        input.config_version = None;
        assert_eq!(
            resolve_mesh_route_snapshot(input).snapshot.scope,
            MeshRouteScope::PersistedUnknown
        );
    }

    #[test]
    fn cross_platform_wire_fixture_matches_resolver() {
        let fixture: MeshRouteReport = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/src/contracts/mesh-route-report.fixture.json"
        )))
        .unwrap();
        assert_eq!(
            resolve_mesh_route_snapshot(fixture.snapshot.clone()),
            fixture
        );
    }
}

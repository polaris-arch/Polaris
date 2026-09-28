import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { Rule, ServerConfig, UserConfig } from '@/contracts/types';
import { isMeshNode } from '@/domain/endpoint-routes';
import { meshOverlapRuleIds } from '@/domain/mesh-rule-overlap';
import { configBaseVersion } from '@/lib/staged-config';
import { meshRouteIsApplied } from './MobileMeshRouteEvidence';

/** A running report describes the visible edit only when both projections still match. */
export function meshRouteMatchesVisibleConfig(
  report: MeshRouteReport | null,
  config: UserConfig | null,
  visible: { servers?: readonly ServerConfig[]; rules?: readonly Rule[] } = {},
): boolean {
  if (report === null || config === null || report.snapshot.configSource !== 'running' ||
      report.snapshot.configVersion === null) return false;
  if (report.snapshot.configVersion !== configBaseVersion(config)) return false;
  if (visible.servers !== undefined &&
      configBaseVersion(visible.servers) !== configBaseVersion(config.servers ?? [])) return false;
  if (visible.rules !== undefined && configBaseVersion(visible.rules) !==
      configBaseVersion(config.trafficRules ?? config.policyRules ?? config.customRules ?? [])) return false;
  return true;
}

/** Per-rule hints need a proved intersection with the same visible configuration. */
export function meshRouteRuleHints(
  report: MeshRouteReport | null,
  config: UserConfig | null,
  visibleServers: readonly ServerConfig[],
  visibleRules: Rule[],
  previous: boolean,
  smartMode: boolean,
): { ids: Set<string>; unknown: boolean; contextMismatch: boolean } {
  if (!smartMode || !visibleServers.some(isMeshNode)) {
    return { ids: new Set(), unknown: false, contextMismatch: false };
  }
  const matches = meshRouteMatchesVisibleConfig(report, config, {
    servers: visibleServers, rules: visibleRules,
  });
  const contextMismatch = report?.snapshot.configSource === 'running' && !matches;
  const requested = matches ? report?.results.flatMap((result) => result.requested.map((range) => range.cidr)) ?? [] : [];
  const ids = meshOverlapRuleIds(visibleRules, requested);
  const unknown = report === null || !matches || previous || !meshRouteIsApplied(report) ||
    report.unknownReasons.length > 0 || report.totalCandidateCount > report.snapshot.candidates.length ||
    report.results.length !== report.snapshot.candidates.length ||
    report.results.some((result) => result.effective === null) ||
    report.snapshot.candidates.some((candidate) => candidate.matchCidrs === null);
  return { ids, unknown, contextMismatch };
}

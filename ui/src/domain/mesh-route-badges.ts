import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { Rule, ServerConfig, UserConfig } from '@/contracts/types';
import { configBaseVersion } from '@/lib/staged-config';
import { cidrOverlapsAny } from './mesh-rule-overlap';
import { ruleIpCidrs } from './rules';

export type MeshNodeRouteBadge =
  | { kind: 'blocked'; blocks: { cidr: string; byServerId: string }[]; incomplete: boolean }
  | { kind: 'unknown' }
  | { kind: 'clear' };

function applied(report: MeshRouteReport): boolean {
  return report.snapshot.scope === 'applied'
    && report.snapshot.loadEvidence === 'startupReady'
    && !report.snapshot.snapshotStale;
}

/** R evidence may decorate D only when its content version equals the saved config. */
export function reportForSavedConfig(report: MeshRouteReport | null, savedConfig: UserConfig | null): MeshRouteReport | null {
  return report && savedConfig && report.snapshot.configVersion === configBaseVersion(savedConfig)
    ? report : null;
}

/** A staged-only or edited node is not represented by the saved/R identity. */
export function reportForDisplayedMeshNode(
  report: MeshRouteReport | null,
  savedConfig: UserConfig | null,
  displayed: ServerConfig,
): MeshRouteReport | null {
  const savedReport = reportForSavedConfig(report, savedConfig);
  const savedNode = savedConfig?.servers.find((server) => server.id === displayed.id);
  return savedReport && savedNode && configBaseVersion(savedNode) === configBaseVersion(displayed)
    ? savedReport : null;
}

/** Project only the producer's proved blockers; never repeat route ownership calculation. */
export function meshNodeRouteBadge(report: MeshRouteReport | null, serverId: string): MeshNodeRouteBadge {
  if (report === null || !applied(report)) return { kind: 'unknown' };
  const candidate = report.snapshot.candidates.find((item) => item.serverId === serverId);
  // With the caller's D/R version and per-node gates, absence in a complete
  // claimant list means this saved node did not engage this force-route layer.
  if (!candidate) {
    return report.totalCandidateCount > report.snapshot.candidates.length
      || report.unknownReasons.length > 0
      ? { kind: 'unknown' }
      : { kind: 'clear' };
  }
  const result = report.results.find((item) => item.serverId === serverId);
  if (!candidate.generated || !result) return { kind: 'unknown' };
  const confirmed = result.blockedBy.filter((block) => block.confirmed);
  const incomplete = candidate.unknownReasons.length > 0 || result.unknownReasons.length > 0
    || result.coverage === 'unknown' || result.effective === null
    || result.blockedBy.some((block) => !block.confirmed);
  if (confirmed.length > 0) {
    return {
      kind: 'blocked',
      blocks: confirmed.map(({ cidr, serverId: byServerId }) => ({ cidr, byServerId })),
      incomplete,
    };
  }
  return !incomplete && result.coverage === 'full'
    ? { kind: 'clear' }
    : { kind: 'unknown' };
}

export type RuleMeshRouteEvidence = 'overlap' | 'unknown' | 'none';

/** CIDR intersection is a rule warning only; report results remain the sole route-owner source. */
export function ruleMeshRouteEvidence(
  report: MeshRouteReport | null,
  rule: Rule,
  savedRule?: Rule,
): RuleMeshRouteEvidence {
  if (!rule.enabled) return 'none';
  const ruleCidrs = ruleIpCidrs(rule);
  if (ruleCidrs.length === 0) return 'none';
  // The list may show an unsaved edit while the command reports R. Do not compare those as one state.
  if (!savedRule || savedRule.enabled !== rule.enabled
    || JSON.stringify(ruleIpCidrs(savedRule)) !== JSON.stringify(ruleCidrs)) return 'unknown';
  if (report === null || !applied(report)) return 'unknown';

  const knownRanges = report.results.flatMap((result) => result.effective ?? []);
  if (ruleCidrs.some((cidr) => cidrOverlapsAny(cidr, knownRanges))) return 'overlap';

  const incomplete = report.totalCandidateCount > report.snapshot.candidates.length
    || report.unknownReasons.length > 0
    || report.snapshot.candidates.some((candidate) => candidate.unknownReasons.length > 0)
    || report.results.some((result) => result.effective === null || result.unknownReasons.length > 0)
    || report.results.length !== report.snapshot.candidates.length;
  return incomplete ? 'unknown' : 'none';
}

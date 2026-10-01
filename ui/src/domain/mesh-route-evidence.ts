import type {
  MeshRouteCandidate,
  MeshRouteReport,
  MeshRouteResolution,
} from '@/contracts/mesh-route-report';

export interface MeshRouteEvidenceRow {
  candidate: MeshRouteCandidate;
  resolution: MeshRouteResolution | null;
}

/** Preserve the producer's candidate order. A missing result is unknown, never zero coverage. */
export function meshRouteEvidenceRows(report: MeshRouteReport): MeshRouteEvidenceRow[] {
  const byId = new Map(report.results.map((result) => [result.serverId, result]));
  return report.snapshot.candidates.map((candidate) => ({
    candidate,
    resolution: byId.get(candidate.serverId) ?? null,
  }));
}

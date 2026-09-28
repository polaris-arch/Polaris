// P1 wire schema v1. This report describes the mesh force-route layer only.
// D = draft config, R = running config. A caller must not merge different
// configVersion/runGeneration pairs or let an older request replace a newer one.

export type MeshRouteScope = 'preview' | 'applied' | 'persistedUnknown';
export type MeshRouteConfigSource = 'draft' | 'running' | 'persisted';
export type MeshRouteLoadEvidence =
  | 'startupReady'
  | 'fileWrittenUnacknowledged'
  | 'loadFailed'
  | 'unknown';
export type MeshRouteSource = 'declared' | 'observed' | 'retained' | 'bootstrap' | 'opaque';
export type MeshRouteLeg = 'preferredBy' | 'externalRuleSet' | 'inline';
export type MeshRouteCoverage = 'full' | 'partial' | 'none' | 'unknown';
export type MeshRouteUnknownReason =
  | 'opaqueMatchSet'
  | 'earlierOpaqueMatchSet'
  | 'invalidCidr'
  | 'inputLimitExceeded'
  | 'resourceLimitExceeded'
  | 'snapshotStale'
  | 'fileReadFailed'
  | 'fileParseFailed'
  | 'fileLoadUnacknowledged'
  | 'runningArtifactUnavailable'
  | 'generationFailed';

export interface SourcedCidr {
  cidr: string;
  source: MeshRouteSource;
}

export interface MeshRouteCandidate {
  serverId: string;
  tag: string;
  leg: MeshRouteLeg;
  generated: boolean;
  generationReason: string | null;
  engagedReason: string;
  configuredCidrs: string[];
  observedHosts: string[];
  referencedFile: {
    cidrs: string[] | null;
    readError: string | null;
    sampledAtMs: number | null;
  } | null;
  // Sole range set consumed by the pure resolver. null means unknown, not [].
  matchCidrs: SourcedCidr[] | null;
  unknownReasons: MeshRouteUnknownReason[];
}

export interface MeshRouteSnapshot {
  scope: MeshRouteScope;
  configSource: MeshRouteConfigSource;
  configVersion: string | null;
  runGeneration: number | null;
  sampledAtMs: number;
  loadEvidence: MeshRouteLoadEvidence;
  snapshotStale: boolean;
  dnsOwnerServerId: string | null;
  precedingExceptions: string[];
  candidates: MeshRouteCandidate[];
}

export interface MeshRouteResolution {
  serverId: string;
  requested: SourcedCidr[];
  effective: string[] | null;
  blockedBy: Array<{
    serverId: string;
    cidr: string;
    relation: 'equal' | 'requestedWithinEarlier' | 'earlierWithinRequested';
    // Only a proved owner in this layer and this snapshot may be confirmed.
    confirmed: boolean;
  }>;
  coverage: MeshRouteCoverage;
  invalid: string[];
  excludedCatchAll: string[];
  unknownReasons: MeshRouteUnknownReason[];
}

export interface MeshRouteReport {
  schemaVersion: 1;
  snapshot: MeshRouteSnapshot;
  results: MeshRouteResolution[];
}

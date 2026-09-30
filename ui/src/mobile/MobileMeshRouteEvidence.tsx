import type { CSSProperties, ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { revealOnToggle } from '@/components/reveal';
import type { MeshRouteReport, MeshRouteResolution, MeshRouteUnknownReason, SourcedCidr } from '@/contracts/mesh-route-report';

type RouteState = 'unavailable' | 'legacy' | 'previous' | 'preview' | 'pending' | 'empty' | 'absent' | 'noRequested' | 'bootstrapOverlap' | 'full' | 'partial' | 'none' | 'unknown';

const section: CSSProperties = { display: 'grid', gap: 6, minWidth: 0, marginBottom: 16 };
const heading: CSSProperties = { margin: 0, fontSize: '0.875rem', color: 'hsl(var(--fg))' };
const line: CSSProperties = { margin: 0, fontSize: '0.75rem', lineHeight: 1.5, overflowWrap: 'anywhere' };
const list: CSSProperties = { margin: 0, paddingInlineStart: 18, fontSize: '0.75rem', lineHeight: 1.5, overflowWrap: 'anywhere' };
const mono: CSSProperties = { fontFamily: 'var(--mono)' };

/** A legacy or partial IPC payload must never be promoted to a route conclusion. */
export function asMeshRouteReport(value: unknown): MeshRouteReport | null {
  if (typeof value !== 'object' || value === null) return null;
  const report = value as Record<string, unknown>;
  if (report.schemaVersion !== 1 || typeof report.snapshot !== 'object' || report.snapshot === null || !Array.isArray(report.results)) return null;
  const snapshot = report.snapshot as Record<string, unknown>;
  const record = (item: unknown): item is Record<string, unknown> => typeof item === 'object' && item !== null;
  const strings = (item: unknown): item is string[] => Array.isArray(item) && item.every((entry) => typeof entry === 'string');
  const finite = (item: unknown): item is number => typeof item === 'number' && Number.isFinite(item);
  const sourced = (item: unknown): boolean => record(item) && typeof item.cidr === 'string' &&
    ['declared', 'observed', 'retained', 'bootstrap', 'opaque'].includes(String(item.source));
  if (!['preview', 'applied', 'persistedUnknown'].includes(String(snapshot.scope)) ||
      !['draft', 'running', 'persisted'].includes(String(snapshot.configSource)) ||
      !['startupReady', 'fileWrittenUnacknowledged', 'loadFailed', 'unknown'].includes(String(snapshot.loadEvidence)) ||
      !Array.isArray(snapshot.candidates) || !strings(snapshot.precedingExceptions) ||
      typeof snapshot.snapshotStale !== 'boolean' || !finite(snapshot.sampledAtMs) ||
      !(snapshot.dnsOwnerServerId === null || typeof snapshot.dnsOwnerServerId === 'string') ||
      !(snapshot.configVersion === null || typeof snapshot.configVersion === 'string') ||
      !(snapshot.runGeneration === null || finite(snapshot.runGeneration)) ||
      !strings(report.unknownReasons) || !Number.isSafeInteger(report.totalCandidateCount) ||
      (report.totalCandidateCount as number) < snapshot.candidates.length) return null;
  if (!snapshot.candidates.every((candidate) => record(candidate) && typeof candidate.serverId === 'string' &&
      ['preferredBy', 'externalRuleSet', 'inline'].includes(String(candidate.leg)) &&
      typeof candidate.tag === 'string' && typeof candidate.generated === 'boolean' &&
      (candidate.generationReason === null || typeof candidate.generationReason === 'string') &&
      typeof candidate.engagedReason === 'string' &&
      strings(candidate.configuredCidrs) && strings(candidate.observedHosts) && strings(candidate.unknownReasons) &&
      (candidate.matchCidrs === null || (Array.isArray(candidate.matchCidrs) && candidate.matchCidrs.every(sourced))) &&
      (candidate.referencedFile === null || (record(candidate.referencedFile) &&
        (candidate.referencedFile.cidrs === null || strings(candidate.referencedFile.cidrs)) &&
        (candidate.referencedFile.readError === null || typeof candidate.referencedFile.readError === 'string') &&
        (candidate.referencedFile.sampledAtMs === null || finite(candidate.referencedFile.sampledAtMs)))))) return null;
  if (report.results.length !== snapshot.candidates.length) return null;
  if (!report.results.every((item, index) => {
    if (!record(item)) return false;
    return item.serverId === (snapshot.candidates as Array<{ serverId: string }>)[index].serverId &&
      Array.isArray(item.requested) && item.requested.every(sourced) &&
      (item.effective === null || strings(item.effective)) && Array.isArray(item.blockedBy) &&
      item.blockedBy.every((blocked: unknown) => record(blocked) && typeof blocked.serverId === 'string' &&
        typeof blocked.cidr === 'string' && typeof blocked.confirmed === 'boolean' &&
        ['equal', 'requestedWithinEarlier', 'earlierWithinRequested'].includes(String(blocked.relation))) &&
      ['full', 'partial', 'none', 'unknown'].includes(String(item.coverage)) &&
      strings(item.unknownReasons) && strings(item.invalid) && strings(item.excludedCatchAll);
  })) return null;
  return value as MeshRouteReport;
}

/** Only a fresh, versioned, ready running generation can be called loaded. */
export function meshRouteIsApplied(report: MeshRouteReport): boolean {
  const s = report.snapshot;
  return s.scope === 'applied' && s.configSource === 'running' &&
    typeof s.configVersion === 'string' && s.configVersion.length > 0 &&
    typeof s.runGeneration === 'number' && Number.isFinite(s.runGeneration) &&
    s.loadEvidence === 'startupReady' && !s.snapshotStale;
}

export function meshRouteState(report: MeshRouteReport | null, serverId?: string, previous = false, legacy = false,
  contextMismatch = false): RouteState {
  if (legacy) return 'legacy';
  if (report === null) return 'unavailable';
  if (contextMismatch) return 'unknown';
  if (previous || report.snapshot.snapshotStale) return 'previous';
  if (report.snapshot.scope === 'preview') return 'preview';
  if (!meshRouteIsApplied(report)) return 'pending';
  if (report.unknownReasons.length > 0 || report.results.length !== report.snapshot.candidates.length ||
      report.totalCandidateCount > report.snapshot.candidates.length) return 'unknown';
  if (report.results.length === 0) return serverId === undefined ? 'empty' : 'absent';
  const results = serverId === undefined ? report.results : report.results.filter((result) => result.serverId === serverId);
  if (results.length === 0) return 'absent';
  if (results.some((result) => result.effective === null || result.coverage === 'unknown')) return 'unknown';
  const active = results.filter((result) => result.requested.length > 0);
  if (active.length === 0) return 'noRequested';
  if (active.some((result) => result.blockedBy.length > 0 &&
      result.requested.some((range) => range.source === 'bootstrap'))) return 'bootstrapOverlap';
  const hasOverlap = active.some((result) => result.coverage === 'none' || result.coverage === 'partial' ||
    result.blockedBy.length > 0);
  if (!hasOverlap) return 'full';
  return active.some((result) => (result.effective?.length ?? 0) > 0) ? 'partial' : 'none';
}

export function meshRouteSummaryKey(report: MeshRouteReport | null, serverId?: string, previous = false, legacy = false,
  contextMismatch = false): string {
  return `mobileMeshRouteEvidence.summary.${meshRouteState(report, serverId, previous, legacy, contextMismatch)}`;
}

function sourceKey(source: SourcedCidr['source']): string {
  return `mobileMeshRouteEvidence.source.${source}`;
}

function reasonText(t: (key: string) => string, reason: MeshRouteUnknownReason | string): string {
  switch (reason) {
    case 'opaqueMatchSet': return t('mobileMeshRouteEvidence.reason.opaqueMatchSet');
    case 'earlierOpaqueMatchSet': return t('mobileMeshRouteEvidence.reason.earlierOpaqueMatchSet');
    case 'invalidCidr': return t('mobileMeshRouteEvidence.reason.invalidCidr');
    case 'inputLimitExceeded': return t('mobileMeshRouteEvidence.reason.inputLimitExceeded');
    case 'resourceLimitExceeded': return t('mobileMeshRouteEvidence.reason.resourceLimitExceeded');
    case 'snapshotStale': return t('mobileMeshRouteEvidence.reason.snapshotStale');
    case 'fileReadFailed': return t('mobileMeshRouteEvidence.reason.fileReadFailed');
    case 'fileParseFailed': return t('mobileMeshRouteEvidence.reason.fileParseFailed');
    case 'fileLoadUnacknowledged': return t('mobileMeshRouteEvidence.reason.fileLoadUnacknowledged');
    case 'runningArtifactUnavailable': return t('mobileMeshRouteEvidence.reason.runningArtifactMissing');
    case 'generationFailed': return t('mobileMeshRouteEvidence.reason.generationFailed');
    default: return t('mobileMeshRouteEvidence.reason.other');
  }
}

function Ranges({ values, emptyKey }: { values: readonly string[]; emptyKey: string }): ReactElement {
  const { t } = useTranslation();
  if (values.length === 0) return <p style={line}>{t(emptyKey)}</p>;
  return <ul style={list}>{values.map((cidr, index) => <li key={`${cidr}:${index}`} style={mono}>{cidr}</li>)}</ul>;
}

/** Both node cards and TUN use this exact evidence view; no CIDR arbitration lives here. */
export function MobileMeshRouteEvidence({ report, serverId, nameOf, previous = false, legacy = false,
  contextMismatch = false }: {
  report: MeshRouteReport | null;
  serverId?: string;
  nameOf: (id: string) => string;
  previous?: boolean;
  legacy?: boolean;
  contextMismatch?: boolean;
}): ReactElement {
  const { t } = useTranslation();
  if (report === null) return <p style={line}>{t(legacy ? 'mobileMeshRouteEvidence.legacy' : 'mobileMeshRouteEvidence.unavailable')}</p>;
  const s = report.snapshot;
  const incomplete = report.unknownReasons.length > 0 || report.results.length !== s.candidates.length ||
    report.totalCandidateCount > s.candidates.length;
  const loaded = meshRouteIsApplied(report) && !previous && !incomplete && !contextMismatch;
  const results = serverId === undefined ? report.results : report.results.filter((r) => r.serverId === serverId);
  const candidates = new Map(s.candidates.map((candidate) => [candidate.serverId, candidate]));
  const scopeKey = contextMismatch ? 'unmatchedConfig' : previous || s.snapshotStale ? 'previous'
    : loaded ? 'applied' : s.scope === 'preview' ? 'preview' : 'pending';
  return <div data-mesh-route-scope={scopeKey}>
    <section style={section}>
      <h3 style={heading}>{t('mobileMeshRouteEvidence.scopeTitle')}</h3>
      <p style={line}>{t(`mobileMeshRouteEvidence.scope.${scopeKey}`)}</p>
      <p style={line}>{t(`mobileMeshRouteEvidence.load.${s.loadEvidence}`)}</p>
      <p style={line}>{t('mobileMeshRouteEvidence.sampledAt', { time: new Date(s.sampledAtMs).toLocaleString() })}</p>
      {incomplete && <p style={line}>{t('mobileMeshRouteEvidence.incomplete')}</p>}
      {report.unknownReasons.length > 0 && <ul style={list}>{report.unknownReasons.map((reason) =>
        <li key={reason}>{reasonText(t, reason)}</li>)}</ul>}
      <p style={line}>{s.dnsOwnerServerId === null
        ? t('mobileMeshRouteEvidence.dnsUnknown')
        : t('mobileMeshRouteEvidence.dnsOwner', { node: nameOf(s.dnsOwnerServerId) })}</p>
    </section>
    {results.length === 0 && <p style={line}>{t('mobileMeshRouteEvidence.noResults')}</p>}
    {results.map((result: MeshRouteResolution) => {
      const candidate = candidates.get(result.serverId);
      const noPrivateRanges = result.requested.length === 0 && result.effective?.length === 0;
      return <section key={result.serverId} style={section}>
        <h3 style={heading}>{nameOf(result.serverId)}</h3>
        <p style={line}>{contextMismatch
          ? t('mobileMeshRouteEvidence.runningResult', { result: t(noPrivateRanges
            ? 'mobileMeshRouteEvidence.noRequested' : `mobileMeshRouteEvidence.coverage.${result.coverage}`) })
          : t(noPrivateRanges ? 'mobileMeshRouteEvidence.noRequested' : `mobileMeshRouteEvidence.coverage.${result.coverage}`)}</p>
        {!noPrivateRanges && !loaded && <p style={line}>{t('mobileMeshRouteEvidence.coverageCaveat')}</p>}
        {candidate && <p style={line}>{t(`mobileMeshRouteEvidence.leg.${candidate.leg}`)} · {candidate.generated
          ? t('mobileMeshRouteEvidence.generated') : t('mobileMeshRouteEvidence.notGenerated')}</p>}
        <h4 style={heading}>{t('mobileMeshRouteEvidence.requested')}</h4>
        {result.requested.length === 0
          ? <p style={line}>{t('mobileMeshRouteEvidence.noRequested')}</p>
          : <ul style={list}>{result.requested.map(({ cidr, source }, index) =>
            <li key={`${cidr}:${source}:${index}`}><span style={mono}>{cidr}</span> · {t(sourceKey(source))}</li>)}</ul>}
        {!noPrivateRanges && <>
          <h4 style={heading}>{t(loaded ? 'mobileMeshRouteEvidence.loadedRanges' : 'mobileMeshRouteEvidence.calculatedRanges')}</h4>
          {result.effective === null
            ? <p style={line}>{t('mobileMeshRouteEvidence.effectiveUnknown')}</p>
            : <Ranges values={result.effective} emptyKey={loaded
              ? 'mobileMeshRouteEvidence.zeroEffective' : 'mobileMeshRouteEvidence.zeroCalculated'} />}
        </>}
        {result.blockedBy.length > 0 && <>
          <h4 style={heading}>{t('mobileMeshRouteEvidence.blockedTitle')}</h4>
          <ul style={list}>{result.blockedBy.map((blocked, index) => <li key={`${blocked.serverId}:${blocked.cidr}:${index}`}>
            <span style={mono}>{blocked.cidr}</span> · {t(loaded && blocked.confirmed &&
              !result.requested.some((range) => range.source === 'bootstrap')
              ? 'mobileMeshRouteEvidence.blockedConfirmed' : 'mobileMeshRouteEvidence.blockedCandidate',
              { node: nameOf(blocked.serverId) })}
          </li>)}</ul>
        </>}
        {result.excludedCatchAll.length > 0 && <p style={line}>{t('mobileMeshRouteEvidence.catchAllExcluded', {
          cidrs: result.excludedCatchAll.join(', '),
        })}</p>}
        {candidate && <details onToggle={revealOnToggle}>
          <summary>{t('mobileMeshRouteEvidence.sourceDetails')}</summary>
          <p style={line}>{t(`mobileMeshRouteEvidence.engaged.${['alwaysRouteSubnets', 'selected', 'ruleTargeted'].includes(candidate.engagedReason)
            ? candidate.engagedReason : 'other'}`)}</p>
          <p style={line}>{t('mobileMeshRouteEvidence.configured')}</p>
          <Ranges values={candidate.configuredCidrs} emptyKey="mobileMeshRouteEvidence.noRanges" />
          <p style={line}>{t('mobileMeshRouteEvidence.observed')}</p>
          <Ranges values={candidate.observedHosts} emptyKey="mobileMeshRouteEvidence.noObservation" />
          {candidate.referencedFile !== null && <p style={line}>{candidate.referencedFile.readError !== null
            ? t('mobileMeshRouteEvidence.fileReadFailed')
            : t('mobileMeshRouteEvidence.fileSampled', { time: candidate.referencedFile.sampledAtMs === null
              ? t('mobileMeshRouteEvidence.timeUnknown') : new Date(candidate.referencedFile.sampledAtMs).toLocaleString() })}</p>}
          {candidate.referencedFile?.cidrs && <Ranges values={candidate.referencedFile.cidrs} emptyKey="mobileMeshRouteEvidence.noRanges" />}
        </details>}
        {(result.unknownReasons.length > 0 || result.invalid.length > 0 || (candidate?.unknownReasons.length ?? 0) > 0) && <>
          <h4 style={heading}>{t('mobileMeshRouteEvidence.uncertainReasons')}</h4>
          <ul style={list}>{[...new Set([...result.unknownReasons, ...(candidate?.unknownReasons ?? [])])].map((reason) =>
            <li key={reason}>{reasonText(t, reason)}</li>)}
          {result.invalid.map((cidr, index) => <li key={`${cidr}:${index}`}><span style={mono}>{cidr}</span> · {t('mobileMeshRouteEvidence.invalid')}</li>)}</ul>
        </>}
      </section>;
    })}
    <details style={{ ...line, marginBottom: 12 }} onToggle={revealOnToggle}>
      <summary>{t('mobileMeshRouteEvidence.reportNotes')}</summary>
      <p style={line}>{t('mobileMeshRouteEvidence.layerBoundary')}</p>
      <p style={line}>{t('mobileMeshRouteEvidence.singleDns')}</p>
      <p style={line}>{t('mobileMeshRouteEvidence.precedingExceptions')}</p>
      {s.precedingExceptions.length > 0 && <ul style={list}>{s.precedingExceptions.map((reason, index) =>
        <li key={`${reason}:${index}`}>{reason === 'customRuleMayOverride'
          ? t('mobileMeshRouteEvidence.customRule')
          : reason === 'appRuleMayOverride'
            ? t('mobileMeshRouteEvidence.appRule')
            : t('mobileMeshRouteEvidence.otherException')}</li>)}</ul>}
    </details>
  </div>;
}

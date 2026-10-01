/** Read-only P1 force-route evidence. A report is not proof of connectivity or stable ownership. */
import { useTranslation } from 'react-i18next';
import type { ServerConfig } from '@/contracts/types';
import type { MeshRouteReport, MeshRouteUnknownReason, SourcedCidr } from '@/contracts/mesh-route-report';
import { meshRouteEvidenceRows } from '@/domain/mesh-route-evidence';
import { invalidNodeReasonText } from '@/domain/invalid-node-reason';
import { revealOnToggle } from '@/components/reveal';
import { SetBlock } from './Primitives';

export interface MeshRouteEvidenceBlockProps {
  report: MeshRouteReport | null;
  servers: readonly ServerConfig[];
  loading?: boolean;
  error?: boolean;
  onRefresh?: () => void;
}

function unknownKey(reason: MeshRouteUnknownReason): string {
  if (reason === 'opaqueMatchSet' || reason === 'earlierOpaqueMatchSet') return 'unknownOpaque';
  if (reason === 'invalidCidr') return 'unknownInvalid';
  if (reason === 'inputLimitExceeded' || reason === 'resourceLimitExceeded') return 'unknownLimit';
  if (reason === 'snapshotStale') return 'unknownStale';
  if (reason === 'fileReadFailed' || reason === 'fileParseFailed' || reason === 'fileLoadUnacknowledged') return 'unknownFile';
  return 'unknownArtifact';
}

function sampledAt(ms: number | null): string | null {
  if (ms === null || !Number.isFinite(ms)) return null;
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleString();
}

function engagedKey(reason: string): string {
  if (reason === 'alwaysRouteSubnets' || reason === 'selected' || reason === 'ruleTargeted' || reason === 'generationFailed') {
    return `engaged.${reason}`;
  }
  return 'engaged.unknown';
}

export function MeshRouteEvidenceBlock({
  report, servers, loading = false, error = false, onRefresh,
}: MeshRouteEvidenceBlockProps) {
  const { t } = useTranslation();
  const meshText = (key: string) => t(`settings.tun.meshEvidence.${key}`);
  const nameOf = (id: string) => servers.find((server) => server.id === id)?.name ?? meshText('unknownNode');
  const knownIds = new Set(servers.map((server) => server.id));
  const diagnosticIds = report === null ? [] : [...new Set([
    report.snapshot.dnsOwnerServerId,
    ...report.snapshot.candidates.map((candidate) => candidate.serverId),
    ...report.results.flatMap((result) => result.blockedBy.map((block) => block.serverId)),
  ].filter((id): id is string => id !== null && !knownIds.has(id)))];
  const sourced = (ranges: readonly SourcedCidr[]) => (
    <ul className="cidr-eff-list">
      {ranges.map(({ cidr, source }, index) => (
        <li key={`${cidr}-${source}-${index}`}>
          <span className="mono">{cidr}</span> · {meshText(`source.${source}`)}
        </li>
      ))}
    </ul>
  );
  const plain = (ranges: readonly string[]) => (
    <ul className="cidr-eff-list">
      {ranges.map((cidr, index) => <li className="mono" key={`${cidr}-${index}`}>{cidr}</li>)}
    </ul>
  );

  return (
    <SetBlock header={
      <div className="mesh-route-heading">
        <span>{meshText('title')}</span>
        {onRefresh && (
          <button type="button" className="btn ghost sm" onClick={onRefresh} disabled={loading}>
            {loading ? meshText('loading') : t('common.refresh')}
          </button>
        )}
      </div>
    }>
      <div className="card-sub">{meshText('hint')}</div>
      {loading || error || report === null ? (
        <div className="card-sub" role={error ? 'alert' : undefined}>
          {loading ? meshText('loading') : meshText('unavailable')}
        </div>
      ) : (
        <>
          <div className="card-sub">
            {meshText(`scope.${report.snapshot.scope}`)} · {meshText(`configSource.${report.snapshot.configSource}`)} ·{' '}
            {meshText(`load.${report.snapshot.loadEvidence}`)}
          </div>
          {report.snapshot.snapshotStale && <div className="plat-warn">{meshText('stale')}</div>}
          <div className="card-sub">{meshText('sampledAt')}: {sampledAt(report.snapshot.sampledAtMs) ?? meshText('notRecorded')}</div>
          <div className="card-sub">
            {meshText('dnsOwner')}: {report.snapshot.dnsOwnerServerId
              ? nameOf(report.snapshot.dnsOwnerServerId)
              : meshText('notRecorded')}
          </div>
          <div className="card-sub">
            {meshText('precedingExceptions')}: {report.snapshot.precedingExceptions.length === 0
              ? meshText('noneReported')
              : report.snapshot.precedingExceptions.map((exception) => {
                if (exception === 'customRuleMayOverride') return meshText('customException');
                if (exception === 'appRuleMayOverride') return meshText('appException');
                return meshText('otherException');
              }).join(' · ')}
          </div>
          <div className="card-sub">
            {meshText('candidateCount')}: {report.snapshot.candidates.length} / {report.totalCandidateCount}
          </div>
          {report.snapshot.candidates.length !== report.totalCandidateCount && (
            <div className="plat-warn">{meshText('candidateIncomplete')}</div>
          )}
          {report.unknownReasons.length > 0 && (
            <div className="plat-warn">{meshText('reportUnknown')}: {' '}
              {[...new Set(report.unknownReasons)].map((reason) => meshText(unknownKey(reason))).join(' · ')}
            </div>
          )}
          <details className="tun-details mesh-route-details" onToggle={revealOnToggle}>
            <summary>{meshText('technical')}</summary>
            <div className="card-sub">{meshText('version')}: <span className="mono">{report.snapshot.configVersion ?? meshText('notRecorded')}</span></div>
            <div className="card-sub">{meshText('generation')}: {report.snapshot.runGeneration ?? meshText('notRecorded')}</div>
            {diagnosticIds.length > 0 && (
              <div className="card-sub">{meshText('unresolvedIds')}: {' '}
                <span className="mono">{diagnosticIds.join(', ')}</span>
              </div>
            )}
          </details>
          <details className="tun-details mesh-route-details" onToggle={revealOnToggle}>
            <summary>{meshText('details')}</summary>
            <ol className="mesh-route-nodes">
              {meshRouteEvidenceRows(report).map(({ candidate, resolution }) => (
                <li key={candidate.serverId}>
                  <div className="mesh-route-node-title">
                    <b>{nameOf(candidate.serverId)}</b>
                    <span>{meshText(`leg.${candidate.leg}`)}</span>
                    <span>{meshText('coverage')}: {resolution ? meshText(`coverageValue.${resolution.coverage}`) : meshText('unavailable')}</span>
                  </div>
                  <div className="card-sub">{meshText('engagedReason')}: {meshText(engagedKey(candidate.engagedReason))}</div>
                  {!candidate.generated && (
                    <div className="card-sub">
                      {meshText('generationFailed')} {' '}
                      {invalidNodeReasonText(candidate.generationReason ?? '', (key) => t(key))}
                    </div>
                  )}
                  {candidate.configuredCidrs.length > 0 && (
                    <>
                      <div className="card-sub">{meshText('configured')}</div>
                      {plain(candidate.configuredCidrs)}
                    </>
                  )}
                  {candidate.observedHosts.length > 0 && (
                    <>
                      <div className="card-sub">{meshText('observed')}</div>
                      {plain(candidate.observedHosts)}
                    </>
                  )}
                  {candidate.referencedFile && (
                    <>
                      <div className="card-sub">{meshText('referencedFile')}: {sampledAt(candidate.referencedFile.sampledAtMs) ?? meshText('notRecorded')}</div>
                      {candidate.referencedFile.cidrs === null
                        ? <div className="card-sub">{meshText('fileUnknown')}</div>
                        : candidate.referencedFile.cidrs.length === 0
                          ? <div className="card-sub">{meshText('knownEmpty')}</div>
                          : plain(candidate.referencedFile.cidrs)}
                    </>
                  )}
                  <div className="card-sub">{meshText('requested')}</div>
                  {resolution === null || candidate.matchCidrs === null
                    ? <div className="card-sub">{meshText('unknownRanges')}</div>
                    : resolution.requested.length === 0
                      ? <div className="card-sub">{meshText('knownEmpty')}</div>
                      : sourced(resolution.requested)}
                  <div className="card-sub">{meshText('effective')}</div>
                  {resolution?.effective === null || resolution?.effective === undefined
                    ? <div className="card-sub">{meshText('unknownRanges')}</div>
                    : resolution.effective.length === 0
                      ? <div className="card-sub">{meshText('knownEmpty')}</div>
                      : plain(resolution.effective)}
                  {resolution && resolution.blockedBy.length > 0 && (
                    <>
                      <div className="card-sub">{meshText('blockedBy')}</div>
                      <ul className="cidr-eff-list">
                        {resolution.blockedBy.map((block, index) => (
                          <li key={`${block.serverId}-${block.cidr}-${index}`}>
                            <span className="mono">{block.cidr}</span> → {nameOf(block.serverId)} ·{' '}
                            {meshText(`relation.${block.relation}`)} ·{' '}
                            {block.confirmed ? meshText('confirmedHere') : meshText('unconfirmed')}
                          </li>
                        ))}
                      </ul>
                    </>
                  )}
                  {resolution && resolution.invalid.length > 0 && (
                    <div className="card-sub">{meshText('invalid')}: {resolution.invalid.join(', ')}</div>
                  )}
                  {resolution && resolution.excludedCatchAll.length > 0 && (
                    <div className="card-sub">{meshText('excludedCatchAll')}: {resolution.excludedCatchAll.join(', ')}</div>
                  )}
                  {[...candidate.unknownReasons, ...(resolution?.unknownReasons ?? [])].length > 0 && (
                    <div className="card-sub">{meshText('unknownReasons')}: {' '}
                      {[...new Set([...candidate.unknownReasons, ...(resolution?.unknownReasons ?? [])])]
                        .map((reason) => meshText(unknownKey(reason))).join(' · ')}
                    </div>
                  )}
                </li>
              ))}
            </ol>
            {report.snapshot.candidates.length === 0 && (
              <div className="card-sub">{meshText('noCandidates')}</div>
            )}
          </details>
        </>
      )}
    </SetBlock>
  );
}

/** Read-only P1 force-route evidence. A report is not proof of connectivity or stable ownership. */
import { useTranslation } from 'react-i18next';
import type { ServerConfig } from '@/contracts/types';
import type { MeshRouteReport, MeshRouteUnknownReason, SourcedCidr } from '@/contracts/mesh-route-report';
import { meshRouteEvidenceRows } from '@/domain/mesh-route-evidence';
import { meshNodeRouteBadge } from '@/domain/mesh-route-badges';
import { invalidNodeReasonText } from '@/domain/invalid-node-reason';
import { revealOnToggle } from '@/components/reveal';
import { InfoIcon } from '@/components/InfoIcon';
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
  const meshText = (key: string, values?: Record<string, string>) => t(`settings.tun.meshEvidence.${key}`, values);
  const nameOf = (id: string) => servers.find((server) => server.id === id)?.name ?? meshText('unknownNode');
  const knownIds = new Set(servers.map((server) => server.id));
  const diagnosticIds = report === null ? [] : [...new Set([
    report.snapshot.dnsOwnerServerId,
    ...report.snapshot.candidates.map((candidate) => candidate.serverId),
    ...report.results.flatMap((result) => result.blockedBy.map((block) => block.serverId)),
  ].filter((id): id is string => id !== null && !knownIds.has(id)))];
  const rows = report === null ? [] : meshRouteEvidenceRows(report);
  // The badge supplies the proved blockers. Identity checks only bound the report;
  // no CIDR arbitration or ownership inference belongs in this view.
  const versionedRunningReport = report?.snapshot.configSource === 'running'
    && !!report.snapshot.configVersion
    && report.snapshot.runGeneration !== null && Number.isFinite(report.snapshot.runGeneration)
    ? report : null;
  const summaries = rows.flatMap(({ candidate, resolution }) => {
    const node = nameOf(candidate.serverId);
    if (report?.snapshot.scope === 'preview') {
      return [{ id: candidate.serverId, kind: 'preview', text: meshText('summaryPreview', { node }) }];
    }
    const badge = meshNodeRouteBadge(versionedRunningReport, candidate.serverId);
    if (badge.kind === 'clear') return [];
    if (badge.kind === 'blocked') {
      if (resolution?.requested.some((range) => range.source === 'bootstrap')) {
        return [{ id: candidate.serverId, kind: 'unknown', text: meshText('summaryBootstrap', { node }) }];
      }
      const owners = [...new Set(badge.blocks.map((block) => nameOf(block.byServerId)))].join(' · ');
      if (badge.incomplete) {
        return [{ id: candidate.serverId, kind: 'incomplete', text: meshText('summaryIncomplete', { node, owners }) }];
      }
      if (resolution?.coverage === 'partial' || resolution?.coverage === 'none') {
        return [{ id: candidate.serverId, kind: resolution.coverage,
          text: meshText(resolution.coverage === 'partial' ? 'summaryPartial' : 'summaryNone', { node, owners }) }];
      }
    }
    return [{ id: candidate.serverId, kind: 'unknown', text: meshText('summaryUnknown', { node }) }];
  });
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
        <span className="sr-title"><span>{meshText('title')}</span><InfoIcon tip={meshText('hint')} /></span>
        {onRefresh && (
          <button type="button" className="btn ghost sm" onClick={onRefresh} disabled={loading}>
            {loading ? meshText('loading') : t('common.refresh')}
          </button>
        )}
      </div>
    }>
      {loading || error || report === null ? (
        <div className="card-sub" role={error ? 'alert' : undefined}>
          {loading ? meshText('loading') : meshText('unavailable')}
        </div>
      ) : (
        <>
          <div className="card-sub" data-mesh-route-context>
            {meshText(`scope.${report.snapshot.scope}`)} · {meshText(`load.${report.snapshot.loadEvidence}`)}
            {!(report.snapshot.scope === 'applied' && report.snapshot.configSource === 'running') &&
              <> · {meshText(`configSource.${report.snapshot.configSource}`)}</>}
            {' · '}{meshText('sampledAt')}: {sampledAt(report.snapshot.sampledAtMs) ?? meshText('notRecorded')}
          </div>
          {report.snapshot.snapshotStale && <div className="plat-warn mesh-route-warning">{meshText('stale')}</div>}
          <div className="card-sub">
            {meshText('dnsOwner')}: {report.snapshot.dnsOwnerServerId
              ? nameOf(report.snapshot.dnsOwnerServerId)
              : meshText('notRecorded')}
          </div>
          {report.snapshot.precedingExceptions.length > 0 && <div className="card-sub">
            {meshText('precedingExceptions')}: {report.snapshot.precedingExceptions.map((exception) => {
                if (exception === 'customRuleMayOverride') return meshText('customException');
                if (exception === 'appRuleMayOverride') return meshText('appException');
                return meshText('otherException');
              }).join(' · ')}
          </div>}
          {report.snapshot.candidates.length !== report.totalCandidateCount && (
            <div className="plat-warn mesh-route-warning">
              {meshText('candidateCount')}: {report.snapshot.candidates.length} / {report.totalCandidateCount} · {meshText('candidateIncomplete')}
            </div>
          )}
          {report.unknownReasons.length > 0 && (
            <div className="plat-warn mesh-route-warning">{meshText('reportUnknown')}: {' '}
              {[...new Set(report.unknownReasons)].map((reason) => meshText(unknownKey(reason))).join(' · ')}
            </div>
          )}
          {summaries.length > 0 && (
            <div className="mesh-route-summary" role="status">
              {summaries.map(({ id, kind, text }) => (
                <div className="warn-line" data-mesh-route-summary={kind} key={id}>{text}</div>
              ))}
            </div>
          )}
          <details className="tun-details mesh-route-details" onToggle={revealOnToggle}>
            <summary>{meshText('technical')}</summary>
            <div className="card-sub">{meshText('configSource.' + report.snapshot.configSource)}</div>
            <div className="card-sub">{meshText('candidateCount')}: {report.snapshot.candidates.length} / {report.totalCandidateCount}</div>
            {report.snapshot.precedingExceptions.length === 0 && <div className="card-sub">
              {meshText('precedingExceptions')}: {meshText('noneReported')}
            </div>}
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
              {rows.map(({ candidate, resolution }) => (
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

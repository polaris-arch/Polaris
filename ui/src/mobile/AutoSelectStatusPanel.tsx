import { revealOnToggle } from '@/components/reveal';
import { AUTO_SELECT_CAUSES } from '@/contracts/auto-select';
import type { AutoSelectT } from '@/lib/auto-select-actions';
import type { AutoSelectStatusState } from '@/hooks/use-auto-select-status';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';

interface Props extends AutoSelectStatusState {
  enabled: boolean;
  refresh: () => void;
  t: AutoSelectT;
  servers: ServerConfig[];
  subscriptions: SubscriptionConfig[];
}

/** Shared read-only presentation. Kept in the mobile scan surface so every control remains audited. */
export function AutoSelectStatusPanel({ report, loading, failed, enabled, refresh, t, servers, subscriptions }: Props) {
  if (!enabled || report?.intent.mode === 'manual') return null;
  const text = (key: string) => {
    const value = t(`autoSelect.${key}`);
    return value === `autoSelect.${key}` ? t('autoSelect.unknown') : value;
  };
  const name = (id: string | null) => servers.find(server => server.id === id)?.name
    ?? (id === '__direct__' ? t('autoSelect.direct') : id === '__block__' ? t('autoSelect.block') : t('autoSelect.unavailable'));
  const at = (timestamp: number | null) => {
    const date = timestamp === null ? null : new Date(timestamp);
    return !date || !Number.isFinite(date.getTime()) ? t('autoSelect.unavailable') : date.toISOString();
  };
  const detailReason = (reason: string | null) => reason ? text(`${AUTO_SELECT_CAUSES.some(cause => cause === reason) ? 'cause' : 'reason'}.${reason}`) : t('autoSelect.none');
  const subscription = subscriptions.find(sub => sub.id === report?.intent.subscriptionId)?.name ?? t('autoSelect.unavailable');
  return (
    <aside data-auto-select-status="" aria-label={t('autoSelect.title')} aria-busy={loading}
      style={{ marginBlock: '0.75rem', padding: '0.75rem', border: '1px solid hsl(var(--line))', borderRadius: '0.75rem', overflowWrap: 'anywhere' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '0.75rem' }}>
        <strong>{t('autoSelect.title')}</strong>
        <button type="button" onClick={refresh} disabled={loading}
          style={{ minHeight: '2.75rem', paddingInline: '0.75rem' }}>{t('autoSelect.refresh')}</button>
      </div>
      <div role="status" aria-live="polite">
        {failed ? <p>{t('autoSelect.loadFailed')}</p> : !report ? <p>{t('autoSelect.loading')}</p> : <>
          <p>{report.intent.mode === 'unrecognized' ? t('autoSelect.unrecognized') : text(`mode.${report.mode}`)}</p>
          {report.reason && <p data-auto-select-reason="">{text(`reason.${report.reason}`)}</p>}
          {report.intent.mode === 'auto' && <>
            <p>{t('autoSelect.scope', { subscription, exit: name(report.exit.serverId) })}</p>
            <p>{t('autoSelect.rate', { count: report.counters.betterSwitchesInWindow, max: report.counters.betterSwitchesMax })}</p>
            {report.challenger?.lacking && <p>{text(`lacking.${report.challenger.lacking}`)}</p>}
            {report.barred.length > 0 && <ul aria-label={t('autoSelect.barred')}>
              {report.barred.map(item => <li key={item.serverId}>{t('autoSelect.barredNode', {
                name: name(item.serverId), seconds: Math.ceil(item.remainingMs / 1000),
              })}{item.failures !== undefined && <span> · {t('autoSelect.barStrikes', { count: item.failures })}</span>}</li>)}
            </ul>}
            {report.flags.stalled && <p>{t('autoSelect.stalled')}</p>}
            {report.flags.starved && <p>{t('autoSelect.starved')}</p>}
            {report.flags.exitOutsideSubscription && <p>{t('autoSelect.outside')}</p>}
            {report.flags.exitOutsideOverdue && <p>{t('autoSelect.outsideOverdue')}</p>}
            {report.flags.allUnverified && <p>{t('autoSelect.allUnverified')}</p>}
            <details data-auto-select-details="" onToggle={revealOnToggle}>
              <summary>{t('autoSelect.details')}</summary>
              {report.basis && <p>{t('autoSelect.basis', { state: text(`basisState.${report.basis.state}`),
                latency: report.basis.latencyMs === null ? t('autoSelect.unavailable') : `${report.basis.latencyMs} ms`,
                binding: report.basis.binding ? text(`binding.${report.basis.binding}`) : t('autoSelect.unavailable'),
                at: at(report.basis.measuredAt) })}</p>}
              {report.lastSwitch && <>
                <p>{t('autoSelect.lastSwitch', { from: report.lastSwitch.fromName ?? name(report.lastSwitch.fromId),
                  to: report.lastSwitch.toName, reason: detailReason(report.lastSwitch.cause), at: at(report.lastSwitch.at) })}</p>
                {report.lastSwitch.unverified && <p>{t('autoSelect.unverified')}</p>}
              </>}
              {report.lastEvaluation && <>
                <p>{t('autoSelect.lastEvaluation', { at: at(report.lastEvaluation.at), reason: detailReason(report.lastEvaluation.reason) })}</p>
                <p>{text(`outcome.${report.lastEvaluation.outcome}`)}</p>
                <p>{t('autoSelect.counts', { ...report.lastEvaluation.counts })}</p>
              </>}
              {report.challenger && <p>{t('autoSelect.challenger', { name: name(report.challenger.serverId),
                wins: report.challenger.wins, required: report.challenger.winsRequired })}</p>}
              {report.commit.consecutiveFailures > 0 && <p>{t('autoSelect.commit', { count: report.commit.consecutiveFailures,
                seconds: Math.ceil((report.commit.retryAfterMs ?? 0) / 1000) })}</p>}
              {report.commit.lastFailure && <p>{t('autoSelect.lastFailure', { name: name(report.commit.lastFailure.toId),
                reason: detailReason(report.commit.lastFailure.outcome), at: at(report.commit.lastFailure.at) })}</p>}
              {report.noDataForMs !== null && <p>{t('autoSelect.noDataAge', { seconds: Math.ceil(report.noDataForMs / 1000) })}</p>}
              <p>{t('autoSelect.switchCounts', { failover: report.counters.failoverSwitches, select: report.counters.selectSwitches })}</p>
              <p>{t('autoSelect.dataSource', { blocked: report.dataSource.blockedBy ? text(`dataReason.${report.dataSource.blockedBy}`) : t('autoSelect.none'), skip: report.dataSource.lastSkip ? text(`dataReason.${report.dataSource.lastSkip}`) : t('autoSelect.none') })}</p>
            </details>
          </>}
        </>}
      </div>
    </aside>
  );
}

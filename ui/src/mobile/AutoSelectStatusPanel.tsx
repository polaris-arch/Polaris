import type { TFunction } from 'i18next';
import type { AutoSelectStatusState } from '@/hooks/use-auto-select-status';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';

interface Props extends AutoSelectStatusState {
  enabled: boolean;
  refresh: () => void;
  t: TFunction;
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
              })}</li>)}
            </ul>}
            {report.flags.stalled && <p>{t('autoSelect.stalled')}</p>}
            {report.flags.starved && <p>{t('autoSelect.starved')}</p>}
          </>}
        </>}
      </div>
    </aside>
  );
}

import type { AutoSelectT } from '@/lib/auto-select-actions';
import type { AutoSelectStatusState } from '@/hooks/use-auto-select-status';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';
interface Props extends AutoSelectStatusState { enabled: boolean; refresh: () => void; t: AutoSelectT; servers: ServerConfig[]; subscriptions: SubscriptionConfig[] }
/** Optional diagnostics; cards show only the confirmed TCP node. */
export function AutoSelectStatusPanel({ report, failed, loading, enabled, refresh, t, servers }: Props) {
  if (!enabled) return null;
  return <aside aria-busy={loading} data-auto-select-status="">
    <button type="button" onClick={refresh} disabled={loading}>{t('autoSelect.refresh')}</button>
    {failed ? <p>{t('autoSelect.loadFailed')}</p> : report?.nativeAutoGroups.map(group => <p key={JSON.stringify(group.target)}>
      {group.confirmed ? servers.find(server => server.id === group.actualServerId)?.name ?? t('autoSelect.pendingNode') : t('autoSelect.pendingNode')} · {t('autoSelect.tcpNode')}
    </p>)}
  </aside>;
}

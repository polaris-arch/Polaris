import { Fragment, useEffect, useMemo, useState, type ReactElement } from 'react';
import type { SubscriptionConfig } from '@/contracts/types';
import { defaultOpenGroupIds, groupServersBySubscription } from '@/domain/server-grouping';
import { revealSiblingGroup, useRevealAfterCommit } from '@/components/reveal';
import { latLevel } from '@/components/screens/shared/format';
import type { NodePickRow, HomeScreenViewProps } from './view-model';

/** Same provenance/order/defaults as PC NodeMenu. Search never mutates the manual collapse set. */
export function HomeNodePickerList({ rows, subscriptions = [], query, t, onUseAsExit }: {
  rows: readonly NodePickRow[];
  subscriptions?: readonly SubscriptionConfig[];
  query: string;
  t: HomeScreenViewProps['t'];
  onUseAsExit: HomeScreenViewProps['onUseAsExit'];
}): ReactElement {
  const groups = useMemo(() => groupServersBySubscription(rows.map(row => row.server), [...subscriptions]), [rows, subscriptions]);
  const selectedId = rows.find(row => row.isCurrent)?.server.id;
  const initialGroups = defaultOpenGroupIds(groups, selectedId);
  const defaultKey = [...initialGroups].join('\u0000');
  const [openGroups, setOpenGroups] = useState(() => initialGroups);
  useEffect(() => { setOpenGroups(new Set(defaultKey ? defaultKey.split('\u0000') : [])); }, [defaultKey]);
  const byId = new Map(rows.map(row => [row.server.id, row]));
  const needle = query.trim().toLowerCase();
  const shown = groups.map(group => ({ ...group, servers: group.servers.filter(server => !needle ||
    server.name.toLowerCase().includes(needle) || server.address.toLowerCase().includes(needle) || server.protocol.toLowerCase().includes(needle)) })).filter(group => group.servers.length > 0);
  const scheduleReveal = useRevealAfterCommit();
  if (shown.length === 0) return <p className="h-empty">{t('mobileHome.noMatchingNode')}</p>;
  return <ul className="h-picklist">
    {shown.map(group => {
      const open = !!needle || openGroups.has(group.id);
      const label = group.isManual ? t('nodes.tab.manual') : group.isMesh ? t('nodes.tab.mesh') : group.name;
      return <Fragment key={group.id}>
        <li className="h-pick-group-marker"><button type="button" className="h-pick-group" data-group-id={group.id} aria-expanded={open} disabled={!!needle}
          onClick={event => {
            const header = event.currentTarget.parentElement!;
            setOpenGroups(previous => { const next = new Set(previous); if (next.has(group.id)) next.delete(group.id); else next.add(group.id); return next; });
            scheduleReveal(open ? null : () => revealSiblingGroup(header));
          }}>
          <svg className={`h-pick-group-chevron${open ? ' open' : ''}`} viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden><path d="m9 6 6 6-6 6" /></svg>
          <span>{label}</span><span className="h-pick-group-count">{group.servers.length}</span>
        </button></li>
        {open && group.servers.map(server => {
          const row = byId.get(server.id)!;
          return <li key={server.id} data-picker-node={server.id}>
            <button type="button" className={`h-pickrow${row.isCurrent ? ' cur' : ''}`} data-exit-write="home-node-pick" aria-current={row.isCurrent ? 'true' : undefined} onClick={() => onUseAsExit(server)}>
              <span className="h-pickname">{server.name}</span>
              <span className="h-pickmeta"><span className="h-pill">{row.protocolLabel}</span>
                {row.stagedOnly && <span className="h-pill warn">{t('home.stagedOnlyBadge')}</span>}
                <span className={`h-lat ${row.latencyStale ? 'none' : latLevel(row.latencyMs)}`}>
                  {row.latencyMs === undefined ? t('home.unlockStatus.idle') : row.latencyMs === null ? t('nodes.timeout') : row.latencyStale ? t('nodes.mobileLatencyStale') : `${row.latencyMs} ms`}
                </span>
              </span>
            </button>
          </li>;
        })}
      </Fragment>;
    })}
  </ul>;
}

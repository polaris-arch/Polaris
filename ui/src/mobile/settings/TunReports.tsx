import type { ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { ServerConfig } from '@/contracts/types';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import { MobileInfo } from '../MobileInfo';
import { MobileMeshRouteEvidence, meshRouteSummaryKey } from '../MobileMeshRouteEvidence';
import { RefreshIcon } from '../screens/rules/Primitives';
import { SettingsGroup } from './SettingsChrome';

/** Read-only mesh-layer evidence. The node card uses the same adapter and details. */
export function MobileMeshRouteBlock({ report, loading, error, previous, legacy, onRefresh, servers }: {
  report: MeshRouteReport | null;
  loading: boolean;
  error: boolean;
  previous: boolean;
  legacy: boolean;
  onRefresh: () => void;
  servers: readonly ServerConfig[];
}): ReactElement {
  const { t } = useTranslation();
  const nameOf = (id: string): string => servers.find((server) => server.id === id)?.name ?? id;
  const title = t('mobileMeshRouteEvidence.title');
  return <SettingsGroup header={<div className="ms-force-head">
    <span>{title}</span>
    <button type="button" className="ms-force-refresh" onClick={onRefresh} disabled={loading}
      aria-label={`${t('common.refresh')} · ${title}`}>
      <RefreshIcon />
    </button>
  </div>}>
    <MobileInfo title={title} summary={t(meshRouteSummaryKey(report, undefined, previous, legacy))}
      details={<MobileMeshRouteEvidence report={report} previous={previous} legacy={legacy} nameOf={nameOf} />} />
    {loading && <p className="ms-force-feedback" role="status">{t('settings.tun.forceRouteLoading')}</p>}
    {error && <p className="ms-force-feedback error" role="alert">{t('settings.tun.forceRouteUnavailable')}</p>}
  </SettingsGroup>;
}

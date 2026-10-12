import { useTranslation } from 'react-i18next';
import { useLatencyStore } from '@/store/use-latency-store';
import { measurementText, UNMEASURED_PROJECTION } from '@/domain/measurement-display';
import { useAppStore } from '@/store/app-store';
import { configCanonicalText } from '@/lib/staged-config';
import type { ServerConfig } from '@/contracts/types';
import type { MeasurementProjection } from '@/contracts/measurement-display';

/** Shared desktop/mobile presentation. The store owns qualification and purpose. */
export function MeasurementLabel({ serverId, purpose = 'candidate', displayedServer, translate }: { serverId: string; purpose?: 'candidate' | 'current'; displayedServer?: ServerConfig; translate?: (key: string, vars?: Record<string, unknown>) => string }) {
  const { t: sharedT } = useTranslation();
  const t = translate ?? sharedT;
  const result = useLatencyStore(state => {
    const values = purpose === 'candidate' ? state.projections : state.currentProjections;
    return Object.hasOwn(values, serverId) ? values[serverId] : UNMEASURED_PROJECTION;
  });
  const saved = useAppStore(state => state.config?.servers.find(server => server.id === serverId));
  const paused = useLatencyStore(state => state.snapshot?.schedule.state === 'paused'
    && !!saved?.subscriptionId && Object.hasOwn(state.snapshot.schedule.subscriptions ?? {}, saved.subscriptionId));
  const pauseReason = useLatencyStore(state => state.snapshot?.schedule.reason);
  const readError = useLatencyStore(state => state.readError);
  const projected: MeasurementProjection = result === UNMEASURED_PROJECTION && paused ? { state: 'paused', reason: pauseReason } : result;
  const visible: MeasurementProjection = displayedServer && (!saved || configCanonicalText(displayedServer) !== configCanonicalText(saved))
    ? { state: 'unmeasured' as const, notIncluded: 'displayedEdit' } : projected;
  return <span data-measurement-state={visible.state} data-measurement-purpose={visible.purpose ?? purpose}
    style={{ overflowWrap: 'anywhere' }}>{readError ? t('measurement.unavailable') : measurementText(visible, t)}
    {purpose === 'current' && visible.purpose === 'candidate' ? ` · ${t('measurement.nodePath')}` : ''}</span>;
}

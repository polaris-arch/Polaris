import { visionTransportReason } from '@/domain/node-compatibility';
import { useTranslation } from 'react-i18next';
import { useLatencyStore } from '@/store/use-latency-store';
import { measurementText, UNMEASURED_PROJECTION } from '@/domain/measurement-display';
import { useAppStore } from '@/store/app-store';
import { configCanonicalText } from '@/lib/staged-config';
import type { ServerConfig } from '@/contracts/types';
import type { MeasurementProjection } from '@/contracts/measurement-display';

/** Shared desktop/mobile presentation. The store owns qualification and purpose. */
export function MeasurementLabel({ serverId, purpose = 'candidate', displayedServer, translate, compact = false }: { serverId: string; compact?: boolean; purpose?: 'candidate' | 'current'; displayedServer?: ServerConfig; translate?: (key: string, vars?: Record<string, unknown>) => string }) {
  const { t: sharedT } = useTranslation();
  const t = translate ?? sharedT;
  const result = useLatencyStore(state => {
    const values = purpose === 'candidate' ? state.projections : state.currentProjections;
    return Object.prototype.hasOwnProperty.call(values, serverId) ? values[serverId] : UNMEASURED_PROJECTION;
  });
  const saved = useAppStore(state => state.config?.servers.find(server => server.id === serverId));
  const paused = useLatencyStore(state => state.snapshot?.schedule.state === 'paused'
    && state.snapshot.rows[serverId]?.nativeHttp === undefined
    && !!saved?.subscriptionId && Object.prototype.hasOwnProperty.call(state.snapshot.schedule.subscriptions ?? {}, saved.subscriptionId));
  const pauseReason = useLatencyStore(state => state.snapshot?.schedule.reason);
  const readError = useLatencyStore(state => state.readError);
  const projected: MeasurementProjection = result === UNMEASURED_PROJECTION && paused ? { state: 'paused', reason: pauseReason } : result;
  const visible: MeasurementProjection = displayedServer && (!saved || configCanonicalText(displayedServer) !== configCanonicalText(saved))
    ? { state: 'unmeasured' as const, notIncluded: 'displayedEdit' } : projected;
  const incompatible = (displayedServer ?? saved) && visionTransportReason((displayedServer ?? saved)!);
  const text = incompatible ? t('nodes.nodeInvalid') : readError ? t('measurement.unavailable') : measurementText(visible, t);
  const detail = incompatible ? t('nodes.invalidVisionTransport') : readError ? undefined : [
    visible.notIncluded && t('measurement.notIncluded'),
    visible.purpose === 'nativeHttp' && t('measurement.nativeHttp'),
    visible.purpose === 'system' && t('measurement.systemPath'),
    visible.purpose === 'selected' && t('measurement.selectedPath'),
    purpose === 'current' && visible.purpose === 'candidate' && t('measurement.nodePath'),
    visible.bindingMismatch && t('measurement.bindingMismatch'),
    visible.state === 'expired' && t('measurement.expiredHint'),
  ].filter(Boolean).join(' · ') || undefined;
  return <span data-tip={compact && detail && !incompatible ? `${text} · ${detail}` : detail} data-measurement-state={visible.state} data-measurement-purpose={visible.purpose ?? purpose}
    style={{ overflowWrap: 'anywhere', minWidth: 0 }}>{text}</span>;
}

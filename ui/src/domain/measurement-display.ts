import type { MeasurementProjection, MeasurementRecord, MeasurementRows, MeasurementSnapshot } from '@/contracts/measurement-display';
import type { ServerConfig } from '@/contracts/types';

const UNMEASURED: MeasurementProjection = Object.freeze({ state: 'unmeasured' });
export { UNMEASURED as UNMEASURED_PROJECTION };

/** Display currentness comes from the backend ledger, not an event's received time. */
export function projectMeasurement(
  snapshot: MeasurementSnapshot, server: ServerConfig, now: number,
  purpose: 'candidate' | 'current' = 'candidate',
): MeasurementProjection {
  const rows: MeasurementRows | undefined = Object.prototype.hasOwnProperty.call(snapshot.rows, server.id) ? snapshot.rows[server.id] : undefined;
  const record: MeasurementRecord | undefined = purpose === 'candidate' ? rows?.candidate
    : [rows?.candidate, rows?.system, rows?.selected].filter((value): value is MeasurementRecord => value !== undefined)
      .sort((a, b) => BigInt(a.identity.run) < BigInt(b.identity.run) ? 1 : BigInt(a.identity.run) > BigInt(b.identity.run) ? -1 : b.identity.seq - a.identity.seq)[0];
  // A prefilter omission is not a failed measurement and must not reuse an old green value.
  if (purpose === 'candidate' && rows?.notIncluded) return { state: 'unmeasured', notIncluded: rows.notIncluded };
  const paused = purpose === 'candidate' && !!server.subscriptionId
    && snapshot.schedule.state === 'paused'
    && Object.prototype.hasOwnProperty.call(snapshot.schedule.subscriptions ?? {}, server.subscriptionId);
  if (paused) return { state: 'paused', reason: snapshot.schedule.reason, measuredAt: record?.identity.measuredAt };
  if (!record) return UNMEASURED;
  const expired = record.state === 'expired' || !Number.isFinite(now)
    || now < record.identity.measuredAt || now > record.expiresAt;
  return {
    state: expired ? 'expired' : record.state,
    latency: !expired && record.state === 'success' ? record.latency ?? undefined : undefined,
    measuredAt: record.identity.measuredAt, disconnected: record.disconnected,
    purpose: record.identity.path, reason: record.reason, bindingMismatch: record.bindingMismatch,
  };
}

export function measurementText(
  result: MeasurementProjection, t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  const state = result.state === 'success' && result.latency !== undefined
    ? `${result.latency} ms` : t(`measurement.${result.state}`);
  const notes = [result.notIncluded && t('measurement.notIncluded'),
    result.disconnected && t('measurement.disconnected'),
    result.purpose === 'system' && t('measurement.systemPath'),
    result.purpose === 'selected' && t('measurement.selectedPath'),
    result.bindingMismatch && t('measurement.bindingMismatch')].filter(Boolean);
  return [state, ...notes].join(' · ');
}

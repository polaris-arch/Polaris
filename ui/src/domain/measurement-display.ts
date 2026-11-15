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
  // Native owns candidate quality; a pending first sample cannot reuse App history.
  if (purpose === 'candidate' && rows?.nativeHttp === null) return UNMEASURED;
  // App runs have their own ordering. Select their latest record first, then
  // compare original sample timestamps across engines (never mixed sort keys).
  const appRecord = [rows?.candidate, rows?.system, rows?.selected]
    .filter((value): value is MeasurementRecord => value !== undefined)
    .sort((a, b) => BigInt(a.identity.run) < BigInt(b.identity.run) ? 1
      : BigInt(a.identity.run) > BigInt(b.identity.run) ? -1 : b.identity.seq - a.identity.seq)[0];
  const record: MeasurementRecord | undefined = purpose === 'candidate' ? rows?.nativeHttp ?? rows?.candidate
    : rows?.nativeHttp && (!appRecord || rows.nativeHttp.identity.measuredAt >= appRecord.identity.measuredAt)
      ? rows.nativeHttp : appRecord;
  // A prefilter omission is not a failed measurement and must not reuse an old green value.
  if (purpose === 'candidate' && !rows?.nativeHttp && rows?.notIncluded) return { state: 'unmeasured', notIncluded: rows.notIncluded };
  const paused = purpose === 'candidate' && !rows?.nativeHttp && !!server.subscriptionId
    && snapshot.schedule.state === 'paused'
    && Object.prototype.hasOwnProperty.call(snapshot.schedule.subscriptions ?? {}, server.subscriptionId);
  if (paused) return { state: 'paused', reason: snapshot.schedule.reason, measuredAt: record?.identity.measuredAt };
  if (!record) return UNMEASURED;
  // A different measurement context needs a new sample, not a TTL warning.
  if (!Number.isFinite(now) || now < record.identity.measuredAt
    || record.state === 'expired' && record.reason && record.reason !== 'expired') return UNMEASURED;
  const expired = record.state === 'expired' || now > record.expiresAt;
  return {
    state: expired ? 'expired' : record.state,
    latency: record.latency ?? undefined, failure: record.failure ?? undefined,
    measuredAt: record.identity.measuredAt, disconnected: record.disconnected,
    purpose: record.identity.path, reason: record.reason, bindingMismatch: record.bindingMismatch,
  };
}

export function measurementText(
  result: MeasurementProjection, t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  const state = (result.state === 'success' || result.state === 'expired') && result.latency !== undefined
    ? `${result.latency} ms`
    : result.state === 'failed' || result.state === 'expired'
      ? t(result.failure?.kind === 'timeout' ? 'nodes.timeout' : 'measurement.failed')
      : t(`measurement.${result.state}`);
  return result.state === 'expired' ? `${state} · ${t('measurement.expired')}` : state;
}

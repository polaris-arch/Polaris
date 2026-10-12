import type { MeasurementSnapshot } from '@/contracts/measurement-display';

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const integer = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const decimal = (value: unknown): boolean => typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value);
const nullableNumber = (value: unknown) => value === null || integer(value);

/** Malformed or identity-free IPC data never becomes a green numeric result. */
export function validMeasurementSnapshot(value: unknown): value is MeasurementSnapshot {
  if (!object(value) || value.schema !== 'polaris-measurement-display-v1'
    || typeof value.process !== 'string' || !value.process || value.process.length > 128
    || !decimal(value.revision) || typeof value.configVersion !== 'string' || !/^[a-f0-9]{8}$/.test(value.configVersion)
    || typeof value.frontendConfigDigest !== 'string' || !/^[a-f0-9]{64}$/.test(value.frontendConfigDigest)
    || value.emissionDigest !== null && (typeof value.emissionDigest !== 'string' || !/^[a-f0-9]{64}$/.test(value.emissionDigest))
    || typeof value.consistent !== 'boolean' || !nullableNumber(value.generation) || !nullableNumber(value.startTime)
    || !nullableNumber(value.networkEpoch) || !integer(value.foregroundEpoch) || !integer(value.now)
    || !object(value.schedule) || !['disabled', 'waiting', 'running', 'paused'].includes(String(value.schedule.state))
    || value.schedule.subscriptions !== undefined && !object(value.schedule.subscriptions)
    || !object(value.rows) || Object.keys(value.rows).length > 16384) return false;
  return Object.values(value.rows).every(row => object(row) && Object.entries(row).every(([purpose, record]) => {
    if (purpose === 'notIncluded') return typeof record === 'string';
    if (!['candidate', 'system', 'selected'].includes(purpose) || !object(record) || !object(record.identity)) return false;
    const identity = record.identity;
    return ['success', 'failed', 'expired'].includes(String(record.state))
      && nullableNumber(record.latency) && integer(record.expiresAt)
      && typeof record.disconnected === 'boolean' && typeof record.bindingMismatch === 'boolean'
      && decimal(identity.run) && integer(identity.seq) && integer(identity.measuredAt)
      && identity.path === purpose && identity.metric === 'warm_ttfb_v1'
      && typeof identity.urlDigest === 'string' && /^[a-f0-9]{64}$/.test(identity.urlDigest)
      && ['manual', 'failover', 'schedule', 'companion'].includes(String(identity.origin))
      && ['connected', 'disconnected'].includes(String(identity.context))
      && object(identity.instance) && (identity.instance.kind === 'temp' ||
        identity.instance.kind === 'main' && integer(identity.instance.generation) && nullableNumber(identity.instance.startTime))
      && (record.state === 'expired' || identity.instance.kind !== 'main'
        || identity.instance.generation === value.generation && identity.instance.startTime === value.startTime)
      && (record.state === 'expired' || identity.instance.kind !== 'main' || identity.configDigest === value.emissionDigest && value.emissionDigest !== null)
      && (identity.instance.kind === 'temp') === record.disconnected
      && nullableNumber(identity.networkEpoch)
      && !Object.prototype.hasOwnProperty.call(identity, 'nodeFingerprint')
      && (identity.nodeFingerprintDigest === null || typeof identity.nodeFingerprintDigest === 'string' && /^[a-f0-9]{64}$/.test(identity.nodeFingerprintDigest))
      && (identity.connectionDigest === null || typeof identity.connectionDigest === 'string' && /^[a-f0-9]{64}$/.test(identity.connectionDigest))
      && (identity.configDigest === null || typeof identity.configDigest === 'string')
      && (record.state !== 'success' || integer(record.latency))
      && (record.state === 'expired' || typeof identity.nodeFingerprintDigest === 'string' && typeof identity.connectionDigest === 'string')
      && (record.state !== 'failed' || record.latency === null);
  }));
}

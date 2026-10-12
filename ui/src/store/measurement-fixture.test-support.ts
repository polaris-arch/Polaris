import type { UserConfig, ServerConfig } from '@/contracts/types';
import type { MeasurementRecord, MeasurementSnapshot } from '@/contracts/measurement-display';
import { configBaseVersion } from '@/lib/staged-config';
import { useLatencyStore } from './use-latency-store';

export const measurementServer = (id: string): ServerConfig => ({ id, name: id, protocol: 'socks', address: 'example.test', port: 1080 } as ServerConfig);
export function measurementFixture(config: UserConfig, values: Record<string, number | null>, revision = '1', now = 1000): MeasurementSnapshot {
  return { schema: 'polaris-measurement-display-v1', process: 'fixture-process', revision,
    configVersion: configBaseVersion(config), frontendConfigDigest: 'd'.repeat(64), emissionDigest: 'b'.repeat(64), consistent: true, generation: 7, startTime: 10,
    networkEpoch: 3, foregroundEpoch: 0, now,
    schedule: { state: 'waiting', reason: null, stalled: false },
    rows: Object.fromEntries(Object.entries(values).map(([id, latency]) => [id, { candidate: {
      state: latency === null || latency < 0 ? 'failed' : 'success', reason: null,
      latency: latency === null || latency < 0 ? null : latency,
      failure: latency === null || latency < 0 ? { phase: 'measure', kind: 'timeout' } : null,
      identity: { run: revision, seq: 1, origin: 'manual', scope: null, path: 'candidate', metric: 'warm_ttfb_v1',
        urlDigest: 'a'.repeat(64), instance: { kind: 'main', generation: 7, startTime: 10 }, configDigest: 'b'.repeat(64),
        nodeFingerprint: 'c'.repeat(64), networkEpoch: 3, measuredAt: now, context: 'connected' },
      disconnected: false, bindingMismatch: false, expiresAt: now + 1000,
    } satisfies MeasurementRecord }])) };
}

let sequence = 0;
/** Rendering fixtures carry complete ledger identities, not a bare-number production writer. */
export function seedMeasurementFixture(values: Record<string, number | null>): void {
  const merged = { ...useLatencyStore.getState().latencyMap, ...values };
  const config = { servers: Object.keys(merged).map(measurementServer), subscriptions: [] } as unknown as UserConfig;
  useLatencyStore.getState().applySnapshot(measurementFixture(config, merged, String(++sequence), Date.now()), config.servers);
}

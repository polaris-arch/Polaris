import { describe, expect, it } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import { projectMeasurement, measurementText } from './measurement-display';
import { validMeasurementSnapshot } from './measurement-snapshot';
import { measurementFixture, measurementServer } from '@/store/measurement-fixture.test-support';

const server = { ...measurementServer('a'), subscriptionId: 'sub' };
const config = { servers: [server], subscriptions: [{ id: 'sub', name: 'Source' }] } as unknown as UserConfig;

describe('five-state, purpose-aware measurement projection', () => {
  it('distinguishes real success/failure, missing, expired, and planned pause', () => {
    const snapshot = measurementFixture(config, { a: 40 });
    expect(projectMeasurement(snapshot, server, 1500)).toMatchObject({ state: 'success', latency: 40 });
    expect(projectMeasurement(snapshot, server, 2001)).toMatchObject({ state: 'expired', latency: undefined });
    snapshot.rows = {}; expect(projectMeasurement(snapshot, server, 1500).state).toBe('unmeasured');
    snapshot.schedule = { state: 'paused', reason: 'metered', subscriptions: { sub: { periodMinutes: 30 } }, stalled: false };
    expect(projectMeasurement(snapshot, server, 1500).state).toBe('paused');
    snapshot.schedule.state = 'waiting'; snapshot.rows = measurementFixture(config, { a: null }).rows;
    expect(projectMeasurement(snapshot, server, 1500)).toMatchObject({ state: 'failed', latency: undefined });
  });
  it('keeps prefilter omission and disconnected measurement as different facts', () => {
    const snapshot = measurementFixture(config, { a: 40 });
    snapshot.rows.a.notIncluded = 'notInPool';
    expect(projectMeasurement(snapshot, server, 1500)).toMatchObject({ state: 'unmeasured', notIncluded: 'notInPool' });
    delete snapshot.rows.a.notIncluded;
    snapshot.rows.a.candidate!.disconnected = true; snapshot.rows.a.candidate!.identity.instance = { kind: 'temp' }; snapshot.rows.a.candidate!.identity.context = 'disconnected';
    expect(projectMeasurement(snapshot, server, 1500)).toMatchObject({ state: 'success', latency: 40, disconnected: true });
  });
  it('a newer system-purpose value never replaces the candidate value or its timestamp', () => {
    const snapshot = measurementFixture(config, { a: 40 });
    snapshot.rows.a.system = { ...snapshot.rows.a.candidate!, latency: 5,
      identity: { ...snapshot.rows.a.candidate!.identity, run: '2', path: 'system', origin: 'companion' } };
    expect(projectMeasurement(snapshot, server, 1500, 'candidate').latency).toBe(40);
    expect(projectMeasurement(snapshot, server, 1500, 'current')).toMatchObject({ latency: 5, purpose: 'system' });
    expect(measurementText(projectMeasurement(snapshot, server, 1500, 'current'), key => key)).toContain('measurement.systemPath');
  });
  it('backend identity exclusions cannot be made green by a recently received numeric value', () => {
    const snapshot = measurementFixture(config, { a: 1 });
    snapshot.rows.a.candidate!.state = 'expired'; snapshot.rows.a.candidate!.reason = 'nodeChanged';
    expect(projectMeasurement(snapshot, server, 1500)).toMatchObject({ state: 'expired', latency: undefined });
    expect(projectMeasurement(measurementFixture(config, { a: 1 }), server, 999).state).toBe('expired');
  });
  it('rejects malformed snapshots and path/identity mismatches', () => {
    const snapshot = measurementFixture(config, { a: 1 }); expect(validMeasurementSnapshot(snapshot)).toBe(true);
    snapshot.rows.a.candidate!.identity.path = 'system'; expect(validMeasurementSnapshot(snapshot)).toBe(false);
    const malformed = measurementFixture(config, { a: 1 }); malformed.rows.a.candidate!.latency = Number.NaN;
    expect(validMeasurementSnapshot(malformed)).toBe(false);
  });
});

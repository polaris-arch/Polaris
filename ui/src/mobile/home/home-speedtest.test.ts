import { describe, expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import type { ServerConfig } from '@/contracts/types';
import { batchSpeedTestFeedback, planAllHomeSpeedTest } from './home-speedtest';

const node = (id: string, protocol: ServerConfig['protocol'], extras: Partial<ServerConfig> = {}): ServerConfig =>
  ({ id, name: id, protocol, address: `${id}.example.test`, port: 443, ...extras });
const t = ((key: string) => key) as TFunction;

describe('Home all-node speed test', () => {
  it('uses every source in order, excluding staged-only and mesh-only nodes even when stopped', () => {
    const disk = [
      node('subscription', 'vless', { subscriptionId: 'sub' }),
      node('manual', 'trojan'),
      node('mesh-lan', 'wireguard', { wireguardSettings: { allowInternet: false } as ServerConfig['wireguardSettings'] }),
    ];
    const effective = [...disk, node('staged', 'vless')];
    expect(planAllHomeSpeedTest(effective, disk, false)).toEqual(['subscription', 'manual']);
    expect(planAllHomeSpeedTest(effective, disk, true)).toEqual(['subscription', 'manual']);
  });

  it('hides a completed batch receipt while preserving skipped, empty, and interrupted feedback', () => {
    const completed = batchSpeedTestFeedback(
      { results: { a: 26, b: -1 }, outcome: 'completed', notInPool: [], tsNotReady: [] }, 2, t,
    );
    expect(completed).toBeNull();
    const skipped = batchSpeedTestFeedback(
      { results: { a: 26 }, outcome: 'completed', notInPool: ['b'], tsNotReady: [] }, 2, t,
    );
    expect(skipped).toEqual({ tone: 'info', text: 'nodes.speedTestSkipped' });
    const empty = batchSpeedTestFeedback(
      { results: {}, outcome: 'completed', notInPool: [], tsNotReady: [] }, 2, t,
    );
    expect(empty).toEqual({ tone: 'info', text: 'nodes.speedTestNotApplicable' });
    const interrupted = batchSpeedTestFeedback(
      { results: { a: 26 }, outcome: 'interrupted', notInPool: ['b'], tsNotReady: [] }, 3, t,
    );
    expect(interrupted?.tone).toBe('info');
    expect(interrupted?.text).toContain('nodes.speedTestInterruptedSummary');
  });
});

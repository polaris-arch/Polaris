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

  it('does not report skipped or interrupted results as all-success', () => {
    const completed = batchSpeedTestFeedback(
      { results: { a: 26, b: -1 }, outcome: 'completed', notInPool: [], tsNotReady: [] }, 2, t,
    );
    expect(completed.tone).toBe('ok');
    expect(completed.text).toContain('mobileHome.speedTestReturned');
    const interrupted = batchSpeedTestFeedback(
      { results: { a: 26 }, outcome: 'interrupted', notInPool: ['b'], tsNotReady: [] }, 3, t,
    );
    expect(interrupted.tone).toBe('info');
    expect(interrupted.text).toContain('nodes.speedTestInterruptedSummary');
  });
});

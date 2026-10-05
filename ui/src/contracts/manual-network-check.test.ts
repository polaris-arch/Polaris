import { describe, expect, it } from 'vitest';
import { manualNetworkContextMatches } from './manual-network-check';

describe('manual network receipt binding', () => {
  const context = { requestId: 'requested', mainGeneration: 7, startTime: 1200 };
  it('accepts the main prepared by this request, including a stopped-to-ready transition', () => {
    expect(manualNetworkContextMatches(context, 'requested', { running: true, mainGeneration: 7, startTime: 1200 })).toBe(true);
    expect(manualNetworkContextMatches({ ...context, startTime: undefined }, 'requested', { running: true, mainGeneration: 7 })).toBe(true);
  });
  it('rejects a stop, replaced main, unknown status, stale request, or changed start time', () => {
    for (const status of [null, { running: false, mainGeneration: 7, startTime: 1200 },
      { running: true, mainGeneration: 8, startTime: 1200 }, { running: true, startTime: 1200 },
      { running: true, mainGeneration: 7, startTime: 1201 }]) {
      expect(manualNetworkContextMatches(context, 'requested', status)).toBe(false);
    }
    expect(manualNetworkContextMatches(context, 'other', { running: true, mainGeneration: 7, startTime: 1200 })).toBe(false);
  });
});

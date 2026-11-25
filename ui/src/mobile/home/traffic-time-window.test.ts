import { describe, expect, it } from 'vitest';
import type { TrafficStats } from '@/contracts/types';
import { EMPTY_TRAFFIC_BUFFER, pushTotals, pushTrafficStats, timedTrafficPath, trafficObservationLabel } from './traffic-buffer';

describe('traffic time axis', () => {
  it('places two seconds at the right edge instead of stretching across sixty', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    b = pushTotals(b, 10, 20, 1000);
    b = pushTotals(b, 20, 40, 2000);
    const path = timedTrafficPath(b.samples, 'up', 300, 100, 20);
    expect(path).toMatch(/^M 295\.00 50\.00 C /);
    expect(path).toMatch(/300\.00 50\.00$/);
  });
  it('keeps jittered one-second frames, including zero rates, in one segment', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    b = pushTotals(b, 0, 0, 999);
    b = pushTotals(b, 100, 200, 2001);
    b = pushTotals(b, 100, 200, 3000);
    b = pushTotals(b, 200, 400, 4002);
    expect(b.samples.map(sample => sample.at)).toEqual([999, 2001, 3000, 4002]);
    expect(b.samples[2]).toEqual({ at: 3000, up: 0, down: 0 });
    const path = timedTrafficPath(b.samples, 'up', 300, 100, 200);
    expect(path.match(/M /g)).toHaveLength(1);
    expect(path).toContain('294.99 100.00');
  });
  it('does not join a background gap or retain samples older than the window', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    b = pushTotals(b, 10, 20, 1000);
    b = pushTotals(b, 20, 40, 2000);
    b = pushTotals(b, 30, 60, 10000);
    b = pushTotals(b, 40, 80, 11000);
    expect(timedTrafficPath(b.samples, 'up', 300, 100, 20).match(/M /g)).toHaveLength(2);
    b = pushTotals(b, 50, 100, 72000);
    expect(b.samples).toHaveLength(1);
    expect(timedTrafficPath(b.samples, 'up', 300, 100, 20)).toBe('');
  });
  it('clears history on counter reset', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 100, 100, 0);
    b = pushTotals(b, 200, 200, 1000);
    b = pushTotals(b, 1, 1, 2000);
    expect(b.samples).toEqual([]);
  });
  it('keeps steady rates and continuous paths through wall-clock jumps', () => {
    const observedTimes = [10000, 11000, 2000, 3000, 4000, 12000, 120000];
    let b = EMPTY_TRAFFIC_BUFFER;
    for (const [second, observedAt] of observedTimes.entries()) {
      b = pushTotals(b, second * 100, second * 200, second * 1000, observedAt);
      if (second > 0) expect(trafficObservationLabel(b.samples, String)).toBe(String(observedAt));
    }
    expect(b.samples).toEqual(observedTimes.slice(1).map((observedAt, i) => ({
      at: (i + 1) * 1000, observedAt, up: 100, down: 200,
    })));
    for (const field of ['up', 'down'] as const) {
      const path = timedTrafficPath(b.samples, field, 300, 100, 200);
      expect(path.match(/M /g)).toHaveLength(1);
      expect(path).toMatch(/^M 275\.00 /);
    }
  });
});

const stats = (total: number, trafficSample?: TrafficStats['trafficSample']): TrafficStats => ({
  uploadSpeed: 0, downloadSpeed: 0, totalUpload: total, totalDownload: total, activeConnections: 0, trafficSample,
});

it('plots a batched source clock and still merges genuinely short observations', () => {
  let b = EMPTY_TRAFFIC_BUFFER;
  for (const [monotonicMs, total] of [[1000, 0], [1500, 50], [2000, 100], [3000, 200]]) {
    b = pushTrafficStats(b, stats(total, { monotonicMs, generation: 1, reprime: false }), 90_000);
  }
  expect(b.samples.map(({ at, down, observedAt }) => [at, down, observedAt])).toEqual([
    [2000, 100, 90_000], [3000, 100, 90_000],
  ]);
});

it('re-primes without drawing across a short reconnect and clears on a new core', () => {
  let b = EMPTY_TRAFFIC_BUFFER;
  const frame = (monotonicMs: number, total: number, reprime = false, generation = 1) => {
    b = pushTrafficStats(b, stats(total, { monotonicMs, generation, reprime }), 90_000);
  };
  frame(1000, 0);
  frame(2000, 100);
  frame(3000, 200);
  frame(3000, 1_200, true);
  frame(3750, 1_275);
  frame(4750, 1_375);
  expect(b.samples.map(({ down }) => down)).toEqual([100, 100, 100, 100]);
  expect(timedTrafficPath(b.samples, 'down', 300, 100, 100).match(/M /g)).toHaveLength(2);
  frame(6000, 9_000, false, 2);
  expect(b.samples).toEqual([]);
  frame(7000, 10_000); // An older generation is not a new core.
  frame(7000, 9_100, false, 2);
  expect(b.samples.map(({ down }) => down)).toEqual([100]);
  frame(8000, 0, true, 2);
  expect(b.samples).toEqual([]);
});

it('ignores a reprime older than a received observation even when that observation was merged', () => {
  let b = EMPTY_TRAFFIC_BUFFER;
  const frame = (monotonicMs: number, total: number, reprime = false) => {
    b = pushTrafficStats(b, stats(total, { monotonicMs, generation: 1, reprime }), 90_000);
  };
  frame(1000, 0);
  frame(2000, 100);
  frame(2300, 130);
  const before = b;
  frame(2200, 120, true);
  expect(b).toBe(before);
  frame(3000, 200);
  expect(b.samples.map(({ at, down }) => [at, down])).toEqual([[2000, 100], [3000, 100]]);
});

it('cannot plot a snapshot without source timing or reuse its totals across the gap', () => {
  let b = pushTrafficStats(EMPTY_TRAFFIC_BUFFER, stats(0, { monotonicMs: 1000, generation: 1, reprime: true }), 90_000);
  b = pushTrafficStats(b, stats(100, { monotonicMs: 2000, generation: 1, reprime: false }), 91_000);
  b = pushTrafficStats(b, stats(1_000), 92_000);
  b = pushTrafficStats(b, stats(1_100, { monotonicMs: 3000, generation: 1, reprime: false }), 93_000);
  expect(b.samples.map(({ at }) => at)).toEqual([2000]);
  b = pushTrafficStats(b, stats(1_200, { monotonicMs: 4000, generation: 1, reprime: false }), 94_000);
  expect(b.samples[b.samples.length - 1]).toMatchObject({ at: 4000, down: 100, breakBefore: true });
  expect(pushTrafficStats(b, stats(0, null), 95_000)).toBe(EMPTY_TRAFFIC_BUFFER);
  b = pushTrafficStats(b, stats(0), 95_000);
  b = pushTrafficStats(b, stats(50, { monotonicMs: 5000, generation: 1, reprime: true }), 96_000);
  expect(b.samples).toEqual([]);
});

it('a stopped stream keeps its last observation timestamp instead of claiming Now', () => {
  let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 1000);
  b = pushTotals(b, 10, 20, 2000, 90_000);
  const label = () => trafficObservationLabel(b.samples, at => `observed:${at}`);
  expect(label()).toBe('observed:90000');
  // No new sample after any wall-clock delay: unchanged label is explicitly historical.
  expect(label()).toBe('observed:90000');
  expect(trafficObservationLabel([{ at: 2000, up: 10, down: 20 }], String)).toBe('—');
  expect(trafficObservationLabel([], String)).toBe('—');
});

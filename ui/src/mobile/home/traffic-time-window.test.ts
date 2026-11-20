import { describe, expect, it } from 'vitest';
import { EMPTY_TRAFFIC_BUFFER, pushTotals, timedTrafficPath, trafficObservationLabel } from './traffic-buffer';

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

it('a stopped stream keeps its last observation timestamp instead of claiming Now', () => {
  let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 1000);
  b = pushTotals(b, 10, 20, 2000);
  const label = () => trafficObservationLabel(b.samples, at => `observed:${at}`);
  expect(label()).toBe('observed:2000');
  // No new sample after any wall-clock delay: unchanged label is explicitly historical.
  expect(label()).toBe('observed:2000');
  expect(trafficObservationLabel([], String)).toBe('—');
});

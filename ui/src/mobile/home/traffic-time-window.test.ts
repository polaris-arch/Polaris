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

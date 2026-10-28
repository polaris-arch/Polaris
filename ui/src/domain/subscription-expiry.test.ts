import { describe, expect, it } from 'vitest';
import { subscriptionExpiryDate } from './subscription-expiry';
describe('subscription UTC expiry', () => {
  it.each([undefined, null, 0, -1, NaN, Infinity, 8_640_000_000_001, 1e13, 18446744073709551615])('rejects absent or out-of-range %s', (value) => {
    expect(subscriptionExpiryDate(value)).toBeNull();
  });
  it('preserves ordinary and maximum supported UTC dates', () => {
    expect(subscriptionExpiryDate(1_798_761_600)).toBe(new Date(1_798_761_600_000).toISOString().split('T')[0]);
    expect(subscriptionExpiryDate(8_640_000_000_000)).toBe('+275760-09-13');
  });
});

import { describe, expect, it } from 'vitest';
import { cidrContainsIp, isIpLiteral, cidrsOverlap } from './ip-cidr';
describe('numeric preview CIDR reuse', () => {
  it('uses the whole mask rather than two dotted octets', () => {
    expect(cidrContainsIp('10.2.3.0/24', '10.2.4.1')).toBe(false);
    expect(cidrContainsIp('10.2.3.0/24', '10.2.3.255')).toBe(true);
    expect(cidrContainsIp('0.0.0.0/0', '203.0.113.7')).toBe(true);
    expect(cidrContainsIp('::/0', '203.0.113.7')).toBe(false);
  });
  it('handles IPv4-embedded IPv6 and rejects malformed literals/ranges', () => {
    expect(cidrContainsIp('::ffff:192.0.2.0/120', '::ffff:192.0.2.10')).toBe(true);
    expect(cidrsOverlap('::ffff:c000:200/120', '::ffff:192.0.2.10')).toBe(true);
    for (const ip of ['1.2.3.4/24', ':::', '1:2:3', '01.2.3.4', '::ffff:01.2.3.4']) expect(isIpLiteral(ip)).toBe(false);
    expect(cidrContainsIp('1.2.3.4/', '1.2.3.4')).toBe(false);
    expect(cidrContainsIp('1.2.3.4/24/1', '1.2.3.4')).toBe(false);
  });
});

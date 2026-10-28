import { describe, expect, it, vi } from 'vitest';
import { computeTestMatch, computeTestMatchAsync } from './rule-cond';
describe('truthful preview composition', () => {
  const domain = { t: 'domain' as const, v: 'example.com' };
  it('does not erase unavailable context or invent resource matches', () => {
    expect(computeTestMatch([domain, { t: 'port', v: '443' }], 'and', 'example.com')).toBe('untestable');
    expect(computeTestMatch([domain, { t: 'port', v: '443' }], 'or', 'example.com')).toBe('hit');
    expect(computeTestMatch([domain, { t: 'port', v: '443' }], 'and', 'other.com')).toBe('miss');
    expect(computeTestMatch([{t:'geoip',v:'cn'}], 'or', '8.8.8.8')).toBe('untestable');
    expect(computeTestMatch([{t:'geosite',v:'google'}], 'or', 'google.com')).toBe('untestable');
    expect(computeTestMatch([{t:'sourceIpCidr',v:'10.0.0.0/8'}], 'or', '10.2.3.4')).toBe('untestable');
    expect(computeTestMatch([{t:'domainRegex',v:'(a+)+$'}], 'or', 'aaaa!')).toBe('untestable');
  });
  it('keeps regex case and punctuation unchanged through native adapter', async () => {
    const native = vi.fn(async () => [true]);
    expect(await computeTestMatchAsync([{t:'domainRegex',v:'^A{1,3}$'}], 'or', 'AAA', native)).toBe('hit');
    expect(native).toHaveBeenCalledWith(['^A{1,3}$'], 'AAA');
    expect(await computeTestMatchAsync([{t:'domainRegex',v:'(?=a)'}], 'or', 'a', async () => [null])).toBe('untestable');
  });
  it('does not call native on oversized input and rejects mismatched replies', async () => {
    const native = vi.fn(async () => [true]);
    expect(await computeTestMatchAsync([{t:'domainRegex',v:'a'}], 'or', 'a'.repeat(4097), native)).toBe('untestable');
    expect(native).not.toHaveBeenCalled();
    expect(await computeTestMatchAsync([{t:'domainRegex',v:'a'}], 'or', 'a', async () => [])).toBe('untestable');
  });
});

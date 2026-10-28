import { describe, expect, it } from 'vitest';
import { isSpeedTestUrl, adoptSpeedTestUrlDraft, speedTestUrlInvalid } from './periodic-speed-test';
import { isPureIpDnsSpec, parseDnsServerSpec } from '../components/screens/settings/settings-dns-logic';

describe('settings capability admission', () => {
  it.each(['http://', 'https://example.com:99999', 'ftp://example.com', 'not a URL'])('rejects invalid test endpoint %s', value => {
    expect(isSpeedTestUrl(value)).toBe(false);
  });
  it.each(['http://example.com/generate_204', 'https://example.com:8443/test', 'https://[::1]/test'])('accepts supported endpoint %s', value => {
    expect(isSpeedTestUrl(value)).toBe(true);
  });
  it('restricts node resolution without removing DoT from general DNS settings', () => {
    expect(isPureIpDnsSpec('tls://1.1.1.1')).toBe(false);
    expect(parseDnsServerSpec('tls://1.1.1.1')).not.toBeNull();
    expect(isPureIpDnsSpec('https://1.1.1.1/dns-query')).toBe(true);
    expect(isPureIpDnsSpec('udp://1.1.1.1')).toBe(true);
    expect(isPureIpDnsSpec('https://resolver.example/dns-query')).toBe(false);
  });
});


it.each(['udp://[::::]', '::::', '01.2.3.4'])('node resolver rejects malformed IP %s', value => {
  expect(isPureIpDnsSpec(value)).toBe(false);
});


it('hydrates URL validity in both directions and preserves a dirty visible draft', () => {
  const valid = 'https://example.com/test';
  const invalid = 'https://';
  expect(speedTestUrlInvalid(adoptSpeedTestUrlDraft(valid, valid, invalid))).toBe(true);
  expect(speedTestUrlInvalid(adoptSpeedTestUrlDraft(invalid, invalid, valid))).toBe(false);
  expect(adoptSpeedTestUrlDraft('https://edited.example', valid, invalid)).toBe('https://edited.example');
  expect(speedTestUrlInvalid(adoptSpeedTestUrlDraft('https://edited.example', valid, invalid))).toBe(false);
  expect(speedTestUrlInvalid(adoptSpeedTestUrlDraft('unfinished', valid, valid))).toBe(true);
});

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { restartReasonKeys } from './restart-apply-reasons';

describe('safe restart reasons and locale parity', () => {
  it('maps trusted structural paths and never renders kernel strings', () => {
    expect(restartReasonKeys([{ code: 'structuralChange', path: 'tun.enabled' }])).toEqual(['home.restartApplyTun']);
    expect(restartReasonKeys([{ code: 'unsupportedCore', path: 'password=secret' }])).toEqual(['home.restartApplyUnsupportedCore']);
    expect(restartReasonKeys([{ code: 'arbitrary instruction', path: 'password=secret' }])).toEqual(['home.restartApplyStructural']);
    expect(restartReasonKeys([{ code: 'structuralChange', path: 'password=secret' }])).toEqual(['home.restartApplyStructural']);
  });
  it('maps host-effect planner restart codes', () => {
    expect(restartReasonKeys([{ code: 'HOST_CARRIER_TRANSITION_REQUIRES_RESTART', path: 'proxyModeType' }])).toEqual(['home.restartApplyTun']);
    for (const code of ['HOST_PHYSICAL_BASELINE_UNAVAILABLE', 'LINUX_DNS_EXACT_CAPTURE_UNAVAILABLE', 'HOST_DNS_EXACT_CAPTURE_UNAVAILABLE',
      'HOST_DNS_TAKEOVER_UNSUPPORTED', 'HOST_PROXY_EXACT_CAPTURE_UNAVAILABLE']) {
      expect(restartReasonKeys([{ code, path: 'systemProxy' }])).toEqual(['home.restartApplyHostEffects']);
    }
    expect(restartReasonKeys([{ code: 'structuralChange', path: 'inbounds.network-canary.listen_port' }])).toEqual(['home.restartApplyListener']);
  });
  it('maps only the exact DNS host-restart pair to the shared field-specific explanation', () => {
    const dnsReason = { code: 'hostEffectsRequireRestart', path: 'dnsConfig.takeoverSystemDns' };
    expect(restartReasonKeys([dnsReason])).toEqual(['home.restartApplyDnsAutoSource']);
    expect(restartReasonKeys([dnsReason, dnsReason])).toEqual(['home.restartApplyDnsAutoSource']);
    expect(restartReasonKeys([dnsReason, { code: 'hostEffectsRequireRestart', path: 'systemProxy' }]))
      .toEqual(['home.restartApplyDnsAutoSource', 'home.restartApplyHostEffects']);
    for (const path of ['host.effects', 'systemProxy', 'dnsConfig.takeoverSystemDns.extra',
      'dnsConfig.TakeoverSystemDns', 'dnsConfig.takeoverSystemDns private=secret']) {
      expect(restartReasonKeys([{ code: 'hostEffectsRequireRestart', path }])).toEqual(['home.restartApplyHostEffects']);
    }
    for (const code of ['unknown private instruction', 'structuralChange']) {
      expect(restartReasonKeys([{ code, path: dnsReason.path }])).toEqual(['home.restartApplyStructural']);
    }
  });
  it('presents only local DNS-auto copy for the shared desktop/mobile reason data', () => {
    const reasons = [
      { code: 'hostEffectsRequireRestart', path: 'dnsConfig.takeoverSystemDns' },
      { code: 'unknown private instruction', path: 'private=secret' },
    ];
    const keys = restartReasonKeys(reasons);
    for (const locale of ['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa']) {
      const home = JSON.parse(readFileSync(new URL(`../../i18n/locales/${locale}.json`, import.meta.url), 'utf8')).home;
      const rendered = keys.map(key => home[key.slice('home.'.length)]).join(' ');
      expect(rendered).toBe(home.restartApplyDnsAutoSource);
      expect(rendered.trim().length).toBeGreaterThan(0);
      expect(rendered).not.toBe(home.restartApplyHostEffects);
      for (const raw of ['dnsConfig.takeoverSystemDns', 'unknown private instruction', 'private=secret']) expect(rendered).not.toContain(raw);
    }
  });
  it('bounds and deduplicates reason rendering', () => {
    expect(restartReasonKeys(Array.from({length: 1000}, () => ({code:'structuralChange',path:'tun'})))).toEqual(['home.restartApplyTun']);
  });
  it('has all prompt and receipt translations in every shipped main locale', () => {
    const locales = ['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'];
    const keys = ['restartApplyTitle', 'restartApplyDescription', 'restartApplyConfirm', 'restartApplyKeepConnection',
      'restartApplyUnsupportedCore', 'restartApplyTun', 'restartApplyListener', 'restartApplyNamespace', 'restartApplyServices',
      'restartApplyCache', 'restartApplyDnsAutoSource', 'restartApplyHostEffects', 'restartApplyStructural', 'pendingHotApplied', 'pendingHotDurabilityPending',
      'pendingHotConflict', 'pendingHotRejected', 'pendingHotUnconfirmed'];
    for (const locale of locales) {
      const home = JSON.parse(readFileSync(new URL(`../../i18n/locales/${locale}.json`, import.meta.url), 'utf8')).home;
      for (const key of keys) expect(typeof home[key], `${locale}:${key}`).toBe('string');
    }
  });
});

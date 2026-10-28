import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import type { TFunction } from 'i18next';
import type { DnsPolicyAction, DnsServerResource, UserConfig } from '@/contracts/types';
import {
  buildDnsRealQueryFallbackGroups,
  dnsRealQueryFallbackChoice,
  dnsRealQueryFallbackFromChoice,
} from './dns-action-options';
import { dnsResourceReferenceText, dnsServerReferences } from './dns-resource-logic';
import { dnsConnectionResolutionPatch } from '@/domain/dns-connection-resolution';
import { applyFakeIpTunEntry } from '@/components/screens/home/fakeip-tun-entry';

const t = ((key: string, vars?: Record<string, unknown>) =>
  vars?.id ? `${key}:${vars.id}` : vars?.name ? `${key}:${vars.name}` : key) as TFunction;
const servers: DnsServerResource[] = [
  { id: 'builtin-domestic', name: '', enabled: true, type: 'udp', outbound: { type: 'direct' } },
  { id: 'builtin-remote', name: '', enabled: true, type: 'https', outbound: { type: 'currentExit' } },
  { id: 'custom', name: 'Custom DNS', enabled: true, type: 'tls', outbound: { type: 'direct' } },
  { id: 'disabled', name: 'Disabled DNS', enabled: false, type: 'local', outbound: { type: 'direct' } },
  { id: 'hosts', name: 'Hosts', enabled: true, type: 'hosts', outbound: { type: 'direct' } },
];
const defaults = {
  directServerId: 'custom',
  proxyServerId: 'builtin-remote',
  unmatchedAction: { type: 'fakeIp' } as DnsPolicyAction,
};
const options = (currentValue: string) => buildDnsRealQueryFallbackGroups({ servers, t, currentValue })
  .flatMap((group) => group.options);

describe('default FakeIP real-query fallback', () => {
  it('absent means builtin domestic, independent of the direct/proxy roles', () => {
    expect(dnsRealQueryFallbackChoice({})).toBe('server:builtin-domestic');
    expect(dnsRealQueryFallbackChoice(defaults)).toBe('server:builtin-domestic');
  });

  it.each(['builtin-domestic', 'builtin-remote', 'custom'])('preserves server %s round trips', (serverId) => {
    const action = dnsRealQueryFallbackFromChoice(`server:${serverId}`);
    expect(action).toEqual({ type: 'server', serverId });
    expect(dnsRealQueryFallbackChoice({ realQueryFallbackAction: action! })).toBe(`server:${serverId}`);
  });

  it('preserves reject and accepts its selectable default form', () => {
    expect(dnsRealQueryFallbackFromChoice('reject')).toEqual({ type: 'reject', method: 'default' });
    expect(dnsRealQueryFallbackChoice({ realQueryFallbackAction: { type: 'reject', method: 'drop' } })).toBe('reject');
  });

  it.each(['fakeIp', 'group:race', 'hosts:hosts', 'predefined', 'followRouteDefault', 'server:', ''])
    ('does not write unsupported choice %s or offer it as a legacy option', (choice) => {
      expect(dnsRealQueryFallbackFromChoice(choice)).toBeNull();
      expect(options(choice).map((option) => option.value)).toEqual([
        'server:builtin-domestic', 'server:builtin-remote', 'server:custom', 'server:disabled', 'reject',
      ]);
    });

  it('only offers actual non-Hosts servers and Reject, including disabled-state display', () => {
    const values = options('server:custom');
    expect(values.find((option) => option.value === 'server:custom')?.label).toBe('Custom DNS');
    expect(values.find((option) => option.value === 'server:disabled')?.disabled).toBe(true);
    expect(values.find((option) => option.value === 'reject')?.disabled).toBeFalsy();
    expect(values.some((option) => option.value.includes('hosts'))).toBe(false);
  });

  it('keeps missing-server references visible and disabled instead of replacing them', () => {
    const choice = dnsRealQueryFallbackChoice({ realQueryFallbackAction: { type: 'server', serverId: 'gone' } });
    expect(options(choice).find((option) => option.value === choice)).toMatchObject({
      label: 'rules.dnsActionMissingServer:gone', disabled: true,
    });
  });

  it.each(['fakeIp', 'reject'] as const)('guards deletion even when unmatched action is %s', (type) => {
    const config = {
      dnsDefaults: { ...defaults, unmatchedAction: { type }, realQueryFallbackAction: { type: 'server', serverId: 'fallback-only' } },
    } as UserConfig;
    const refs = dnsServerReferences(config, 'fallback-only');
    expect(refs).toEqual([{ scope: 'defaults', name: 'realQueryFallback' }]);
    expect(dnsResourceReferenceText(refs[0], t)).toBe('settings.dns.resourceRefDefault:settings.dns.realQueryFallback');
  });

  it('reject references no server', () => {
    const config = { dnsDefaults: { ...defaults, realQueryFallbackAction: { type: 'reject' } } } as UserConfig;
    expect(dnsServerReferences(config, 'fallback-only')).toEqual([]);
  });

  it('changing connection resolution or enabling TUN FakeIP retains the saved fallback', () => {
    const config = {
      configSchemaVersion: 4,
      proxyModeType: 'tun',
      dnsConfig: { enableFakeIp: false, fakeIpTunAutoEnable: true },
      dnsDefaults: { ...defaults, realQueryFallbackAction: { type: 'server', serverId: 'custom' } },
    } as UserConfig;
    expect(dnsConnectionResolutionPatch(config, 'dnsRules').dnsDefaults?.realQueryFallbackAction)
      .toEqual(config.dnsDefaults?.realQueryFallbackAction);
    expect(applyFakeIpTunEntry(config).config.dnsDefaults?.realQueryFallbackAction)
      .toEqual(config.dnsDefaults?.realQueryFallbackAction);
  });
});

const source = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
describe('fallback UI wiring', () => {
  const desktop = source('components/screens/rules/DnsPolicyWorkspace.tsx');
  const mobile = source('mobile/settings/DnsPage.tsx');
  it.each([['desktop', desktop], ['mobile', mobile]])('%s shares the restricted codec, options and effective value', (_name, text) => {
    for (const fn of ['dnsRealQueryFallbackChoice(', 'dnsRealQueryFallbackFromChoice(', 'buildDnsRealQueryFallbackGroups(']) {
      expect(text).toContain(fn);
    }
    expect(text).toContain("t('settings.dns.realQueryFallback')");
    expect(text).toContain("t('settings.dns.realQueryFallbackHint')");
    expect(text).toContain('realQueryFallbackAction }');
  });

  it('desktop writes through update and mobile commit reports row-local failures', () => {
    expect(desktop).toContain('void update({ dnsDefaults: { ...dnsDefaults, realQueryFallbackAction } })');
    expect(mobile).toContain('commit(rowId, update({ dnsDefaults: { ...defaults, realQueryFallbackAction } }))');
    expect(mobile).toContain("commitRealQueryFallback('dns-real-query-fallback', next)");
    expect(mobile).toContain("onOpen={() => setSheet('realQueryFallback')}");
    expect(mobile).toContain('openGroupIds={openGroupOf(realQueryFallbackGroups, realQueryFallbackValue)}');
    expect(mobile.split('<SelectSheetPanel').some((panel) => panel.includes("open={sheet === 'realQueryFallback'}"))).toBe(true);
  });

  it('TUN manual enable preserves every DNS default, including the new fallback', () => {
    expect(source('components/screens/settings/SettingsTun.tsx')).toMatch(/dnsDefaults:\s*\{\s*\.\.\.config\.dnsDefaults,/);
  });

  it.each(['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'])('%s explains the non-address scope', (locale) => {
    const dict = JSON.parse(source(`i18n/locales/${locale}.json`));
    expect(dict.settings.dns.realQueryFallback).toBeTruthy();
    const hint = dict.settings.dns.realQueryFallbackHint;
    for (const word of ['FakeIP', 'A/AAAA', 'HTTPS', 'SVCB', 'TXT']) expect(hint).toContain(word);
    expect(dict.rules.dnsActionDescription_fakeIp).toContain('A/AAAA');
    expect(dict.settings.dns.fakeIpDesc).toContain('A/AAAA');
  });
});

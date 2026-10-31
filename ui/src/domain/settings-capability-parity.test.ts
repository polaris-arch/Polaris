import { beforeAll, describe, expect, it } from 'vitest';
import { createElement, type ReactElement } from 'react';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import i18next, { type i18n as I18n } from 'i18next';
import { I18nextProvider, initReactI18next } from 'react-i18next';
import zhCN from '../i18n/locales/zh-CN.json';
import SettingsDns from '../components/screens/settings/SettingsDns';
import { DnsPage } from '../mobile/settings/DnsPage';
import { ModalContext } from '../components/dialogs/Modal';
import type { UserConfig } from '../contracts/types';
import { isSpeedTestUrl, adoptSpeedTestUrlDraft, speedTestUrlInvalid } from './periodic-speed-test';
import { isPureIpDnsSpec, parseDnsServerSpec, nodeResolverTier1Count, editNodeResolverInventory } from '../components/screens/settings/settings-dns-logic';

describe('settings capability admission', () => {
  it.each(['http://', 'https://example.com:99999', 'ftp://example.com', 'not a URL'])('rejects invalid test endpoint %s', value => {
    expect(isSpeedTestUrl(value)).toBe(false);
  });
  it.each(['http://example.com/generate_204', 'https://example.com:8443/test', 'https://[::1]/test'])('accepts supported endpoint %s', value => {
    expect(isSpeedTestUrl(value)).toBe(true);
  });
  it('admits pure-IP native DoT in shared node resolution', () => {
    expect(isPureIpDnsSpec('tls://1.1.1.1')).toBe(true);
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

it('preserves enabled identified inventory through serialization and canonical reload', () => {
  const previous = [{ id: 'dot', spec: 'tls://[2001:db8::1]' }, { id: 'alias', spec: 'tls://[2001:0db8::1]:853' }, { id: 'udp', spec: 'udp://9.9.9.9:5353' }];
  const edit = editNodeResolverInventory(previous, ['ali', 'dnspod', 'alias', 'udp'], previous, () => 'new');
  expect(edit.ok).toBe(true);
  if (!edit.ok) throw new Error('valid identified save rejected');
  const saved = JSON.parse(JSON.stringify(edit));
  expect(saved.pool).toEqual(['ali', 'dnspod', 'dot', 'udp']);
  expect(nodeResolverTier1Count(saved.pool, saved.custom)).toBe(3);
  expect(editNodeResolverInventory(saved.custom, saved.pool, saved.custom, () => 'new')).toEqual(edit);
});

let dnsI18n: I18n;
beforeAll(async () => {
  dnsI18n = i18next.createInstance();
  await dnsI18n.use(initReactI18next).init({
    lng: 'zh-CN', fallbackLng: 'zh-CN',
    resources: { 'zh-CN': { translation: zhCN } },
    interpolation: { escapeValue: false },
  });
});

// Same SSR context as utls-renderers.test.tsx: render the real closed Csel
// inline without a browser portal. No document shim or settings/commit mock.
function renderDnsPage(page: ReactElement): string {
  return renderToStaticMarkup(createElement(I18nextProvider, { i18n: dnsI18n },
    createElement(ModalContext.Provider, { value: { cselCloseRef: { current: null } } }, page)));
}

it('both actual DNS pages reload saved DoT selection with shared encrypted quota', () => {
  const custom = [
    { id: 'dot', spec: 'tls://1.1.1.1' },
    { id: 'udp', spec: 'udp://9.9.9.9:5353' },
    { id: 'extra', spec: 'tls://[2001:db8::1]:8853' },
  ];
  for (const full of [false, true]) {
    const config = JSON.parse(JSON.stringify({ dnsConfig: {
      resolveNodeDomainsAhead: true,
      nodeResolverCustom: custom,
      nodeResolverPool: ['ali', 'dnspod', 'udp', ...(full ? ['dot'] : [])],
    } })) as UserConfig;
    const update = async () => undefined;
    const pages = [
      createElement(SettingsDns, { config, update, section: 'runtime' }),
      createElement(DnsPage, { config, update, commit: () => undefined }),
    ];
    for (const page of pages) {
      const markup = renderDnsPage(page);
      expect(markup).toContain('tls://1.1.1.1');
      expect(markup).toContain('udp://9.9.9.9:5353');
      expect(markup).not.toContain(zhCN.settings.dns.customDohIpOnly);
      expect(markup.includes(zhCN.settings.dns.raceQuotaReached)).toBe(full);
    }
  }
});

it('both inventory commit handlers use the same validated transaction before patching saved DNS', () => {
  for (const [path, handler] of [
    ['../components/screens/settings/SettingsDns.tsx', 'setCustomUpstreams'],
    ['../mobile/settings/DnsPage.tsx', 'commitCustomUpstreams'],
  ]) {
    const source = readFileSync(new URL(path, import.meta.url), 'utf8');
    const start = source.indexOf(`function ${handler}(`);
    expect(start).toBeGreaterThan(0);
    const body = source.slice(start, source.indexOf('\n  }', start));
    expect(body).toContain('editNodeResolverInventory(customUpstreams, racePool,');
    const guard = body.indexOf('if (!edit.ok) return false;');
    expect(guard).toBeGreaterThan(0);
    expect(body.indexOf('patchDns(')).toBeGreaterThan(guard);
    expect(body).toContain('nodeResolverCustom: edit.custom, nodeResolverPool: edit.pool');
    expect(body).toContain('specs.map((spec, index) => ({ spec, id: ids[index] }))'.replace('specs', handler === 'setCustomUpstreams' ? 'specs' : 'next'));
    expect(source).toContain('entryIds={customUpstreams.map((item) => item.id)}');
    expect(body).toContain('{ sources, context }');
    expect(body).toContain('return edit.editor;');
    expect(source).toContain('entryContextKey={JSON.stringify(racePool)}');
    expect(source).toContain("customInventoryError === 'quota' ?");
  }
});

it('both pages keep builtin-shadowed inventory editable without a misleading custom enable switch', () => {
  const config = { dnsConfig: { resolveNodeDomainsAhead: true, nodeResolverPool: ['ali'], nodeResolverCustom: [{ id: 'ali', spec: 'tls://resolver.example' }] } } as UserConfig;
  const update = async () => undefined;
  for (const page of [createElement(SettingsDns, { config, update, section: 'runtime' }), createElement(DnsPage, { config, update, commit: () => undefined })]) {
    const markup = renderDnsPage(page);
    expect(markup).toContain('tls://resolver.example');
    expect(markup).not.toContain('race-custom-ali');
    expect(markup).not.toContain(`aria-label="${zhCN.settings.dns.enableCustomDoh}"`);
  }
});

import { beforeAll, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync } from 'node:fs';
import { SubscriptionUserAgentHint } from './SubscriptionUserAgentHint';
import { MobileSubscriptionUserAgentHint } from '@/mobile/forms/SubscriptionUserAgentHint';
import { isSubscriptionUserAgentInfo, type SubscriptionUserAgentInfo } from '@/contracts/subscription-user-agent';
import i18n, { i18nReady } from '@/i18n';

// Synthetic backend reply: the view must preserve these opaque UA bytes, not reconstruct them.
const info: SubscriptionUserAgentInfo = {
  effectiveStatus: 'ready',
  appVersion: 'app-fixture-17', coreVersion: 'kernel-fixture-23',
  defaultUserAgent: 'opaque-default/alpha', fallbackUserAgent: 'opaque-default/alpha',
  effectiveUserAgent: 'opaque-default/alpha', source: 'default',
};
const surfaces = [
  { Hint: SubscriptionUserAgentHint, className: 'fld-hint', retryClassName: 'btn ghost sm' },
  { Hint: MobileSubscriptionUserAgentHint, className: 'm-form-hint', retryClassName: 'm-form-btn' },
];
beforeAll(async () => { await i18nReady; await i18n.changeLanguage('en-US'); });

it('renders actual backend versions/default and all three precedence labels on both surfaces', () => {
  for (const { Hint, className } of surfaces) {
    for (const source of ['default', 'global', 'subscription'] as const) {
      const effectiveUserAgent = source === 'default' ? info.defaultUserAgent : `${source}/opaque`;
      const html = renderToStaticMarkup(<Hint id="ua-info"
        model={{ status: 'ready', info: { ...info, source, effectiveUserAgent }, refresh: vi.fn() }} />);
      for (const expected of [info.appVersion, info.coreVersion, info.defaultUserAgent, effectiveUserAgent, className]) {
        expect(html).toContain(expected);
      }
      const key = source === 'subscription' ? 'sub.uaEffectiveCustom' : source === 'global' ? 'sub.uaEffectiveGlobal' : 'sub.uaEffectiveDefault';
      expect(html).toContain(i18n.t(key));
      expect(html).not.toContain('unknown');
    }
  }
});

it('loading and failure show no old/default/version values; failure offers a non-submit retry', () => {
  for (const { Hint, retryClassName } of surfaces) {
    for (const status of ['loading', 'failed'] as const) {
      const html = renderToStaticMarkup(<Hint id="ua-info" model={{ status, refresh: vi.fn() }} />);
      expect(html).toContain(i18n.t(status === 'loading' ? 'sub.uaLoading' : 'sub.uaReadFailed'));
      for (const stale of Object.values(info)) expect(html).not.toContain(`>${stale}<`);
      expect(html).not.toContain(info.defaultUserAgent);
      if (status === 'failed') {
        expect(html).toContain('type="button"');
        expect(html).toContain(`class="${retryClassName}"`);
      }
    }
  }
});

it('cold/busy effective state retains known versions/default but never claims a fallback is effective', () => {
  const unavailable: SubscriptionUserAgentInfo = { ...info, effectiveStatus: 'unavailable',
    fallbackUserAgent: null, effectiveUserAgent: null, source: null };
  expect(isSubscriptionUserAgentInfo(unavailable)).toBe(true);
  for (const { Hint, retryClassName } of surfaces) {
    const html = renderToStaticMarkup(<Hint id="ua-info"
      model={{ status: 'ready', info: unavailable, refresh: vi.fn() }} />);
    for (const known of [info.appVersion, info.coreVersion, info.defaultUserAgent]) expect(html).toContain(known);
    expect(html).toContain(i18n.t('sub.uaEffectiveReadFailed'));
    expect(html).not.toContain(i18n.t('sub.uaEffectiveDefault'));
    expect(html).not.toContain(i18n.t('sub.uaEffectiveGlobal'));
    expect(html).not.toContain(i18n.t('sub.uaEffectiveCustom'));
    expect(html).toContain('type="button"');
    expect(html).toContain(`class="${retryClassName}"`);
  }
  for (const field of ['fallbackUserAgent', 'effectiveUserAgent', 'source']) {
    expect(isSubscriptionUserAgentInfo({ ...unavailable, [field]: 'invented' })).toBe(false);
  }
});

describe('native identity reply guard', () => {
  it('accepts opaque custom UA values including the literal unknown, never versions with that sentinel', () => {
    expect(isSubscriptionUserAgentInfo(info)).toBe(true);
    expect(isSubscriptionUserAgentInfo({ ...info, source: 'subscription', effectiveUserAgent: 'unknown' })).toBe(true);
    for (const field of ['appVersion', 'coreVersion']) {
      expect(isSubscriptionUserAgentInfo({ ...info, [field]: 'unknown' })).toBe(false);
    }
  });
  it('rejects unavailable, malformed and empty replies instead of fabricating fallback identity', () => {
    for (const bad of [undefined, null, {}, { ...info, source: 'guessed' }]) expect(isSubscriptionUserAgentInfo(bad)).toBe(false);
    for (const field of ['appVersion', 'coreVersion', 'defaultUserAgent', 'fallbackUserAgent', 'effectiveUserAgent']) {
      for (const value of ['', '  ', undefined, 123]) expect(isSubscriptionUserAgentInfo({ ...info, [field]: value })).toBe(false);
    }
  });
});

it('desktop and mobile share the read model and use their own scanned controls without filling the optional custom field', () => {
  for (const [url, hint] of [
    [new URL('./SubDialog.tsx', import.meta.url), 'SubscriptionUserAgentHint'],
    [new URL('../../mobile/forms/SubFormPanel.tsx', import.meta.url), 'MobileSubscriptionUserAgentHint'],
  ] as const) {
    const source = readFileSync(url, 'utf8');
    expect(source).toContain('useSubscriptionUserAgent(ua)');
    expect(source).toContain(`<${hint}`);
    expect(source).toContain("uaInfo.info.fallbackUserAgent : t('sub.uaPh')");
    expect(source).toContain("useState(base?.userAgent ?? '')");
    expect(source).toContain(url.pathname.endsWith('/SubDialog.tsx')
      ? "ua.trim() || undefined" : "ua.trim() === '' ? undefined : ua.trim()");
    expect(source).toContain("t('common.optional')");
    expect(source).not.toMatch(/setUa\(.*(?:defaultUserAgent|effectiveUserAgent|fallbackUserAgent)/);
  }
});

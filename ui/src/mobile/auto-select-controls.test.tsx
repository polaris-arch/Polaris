import { describe, expect, it, vi } from 'vitest';
import { createInstance } from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import { AutoSelectControls } from './AutoSelectControls';
import type { UserConfig } from '@/contracts/types';
import zhCN from '@/i18n/locales/zh-CN.json';
import zhTW from '@/i18n/locales/zh-TW.json';
import enUS from '@/i18n/locales/en-US.json';
import ru from '@/i18n/locales/ru.json';
import fa from '@/i18n/locales/fa.json';

const state = vi.hoisted(() => ({ report: null as unknown }));
vi.mock('@/hooks/use-auto-select-actions', () => ({ useAutoSelectActions: () => ({ busy: false, run: vi.fn(), feedback: null }) }));
vi.mock('@/hooks/use-auto-select-status', () => ({ useAutoSelectStatus: () => ({ report: state.report, runtimeCurrent: true, enabled: false, loading: false, failed: false, refresh: vi.fn() }) }));
const config = {
  subscriptions: [{ id: 'sub-a', name: 'Polaris' }, { id: 'empty', name: 'Empty' }],
  servers: [{ id: 'node-a', protocol: 'vless', name: 'Tokyo', subscriptionId: 'sub-a' }, { id: 'node-b', protocol: 'trojan', name: 'Paris' }],
  autoSelectEnabled: true,
  selectedServerId: 'node-b',
  selectionIntent: { mode: 'auto', scope: 'subscription', subscriptionId: 'sub-a' },
} as UserConfig;

describe('Auto entry labels and runtime truth', () => {
  for (const [lang, locale] of Object.entries({ 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ru, fa })) {
    it(`${lang}: one localized choice per scope, no old explanatory banner`, async () => {
      state.report = null;
      const i18n = createInstance();
      await i18n.init({ lng: lang, fallbackLng: false, resources: { [lang]: { translation: locale } }, interpolation: { escapeValue: false } });
      const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={i18n.t} target={{ scope: 'subscription', subscriptionId: 'sub-a' }} />);
      expect(html).toContain(i18n.t('autoSelect.auto'));
      expect(html).not.toContain('role="switch"');
      expect(html).toContain('aria-pressed="true"');
      expect(html).toContain(i18n.t('autoSelect.pendingNode'));
      expect(html).not.toContain('Paris');
      expect(html).not.toContain(i18n.t('autoSelect.choiceHelp'));
      const global = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={i18n.t} />);
      expect(global).toContain('data-auto-select-scope="global"');
      expect(global).toContain('aria-pressed="false"');
      const empty = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={i18n.t} target={{ scope: 'subscription', subscriptionId: 'empty' }} />);
      expect(empty).toBe('');
    });
  }
  it('shows only an in-scope runtime-confirmed node for the selected Auto entry', () => {
    state.report = { nativeAutoGroups: [{ target: config.selectionIntent, actualServerId: 'node-a', confirmed: true, state: 'auto' }] };
    const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} target={{ scope: 'subscription', subscriptionId: 'sub-a' }} />);
    expect(html).toContain('Tokyo');
    expect(html).not.toContain('Paris');
    state.report = { nativeAutoGroups: [{ target: config.selectionIntent, actualServerId: 'node-b', confirmed: true, state: 'auto' }] };
    expect(renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} target={{ scope: 'subscription', subscriptionId: 'sub-a' }} />)).not.toContain('Paris');
  });
});


it('empty groups have no Auto card but Settings can always turn off saved Auto', () => {
  state.report = null;
  const empty = { ...config, servers: [], selectedServerId: null };
  const html = renderToStaticMarkup(<AutoSelectControls config={empty} accept={() => {}} t={key => key} variant="switch" />);
  expect(html).toMatch(/role="switch"[^>]*aria-checked="true"/);
  expect(html).toContain('data-exit-write="auto-select-mode"');
  expect(html).not.toMatch(/role="switch"[^>]*disabled/);
});

it('feature defaults off and hides entries while leaving its master switch available', () => {
  const off = { ...config, autoSelectEnabled: undefined };
  expect(renderToStaticMarkup(<AutoSelectControls config={off} accept={() => {}} t={key => key} />)).toBe('');
  const html = renderToStaticMarkup(<AutoSelectControls config={off} accept={() => {}} t={key => key} variant="switch" />);
  expect(html).toContain('aria-checked="false"');
});

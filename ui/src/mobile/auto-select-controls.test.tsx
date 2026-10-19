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

vi.mock('@/hooks/use-auto-select-actions', () => ({ useAutoSelectActions: () => ({ busy: false, run: vi.fn(), feedback: null }) }));
vi.mock('@/hooks/use-auto-select-status', () => ({ useAutoSelectStatus: () => ({ report: null, enabled: false, loading: false, failed: false, refresh: vi.fn() }) }));

const config = {
  subscriptions: [{ id: 'sub-a', name: 'Polaris' }, { id: 'sub-b', name: 'Travel' }, { id: 'empty', name: 'Empty' }],
  servers: [{ id: 'node-a', name: 'Tokyo', subscriptionId: 'sub-a' }, { id: 'node-b', name: 'Paris', subscriptionId: 'sub-b' }],
  selectedServerId: 'node-b',
  selectionIntent: { mode: 'auto', scope: 'subscription', subscriptionId: 'sub-a' },
} as UserConfig;

describe('automatic selection identifies subscription scope without posing as an exit', () => {
  for (const [lang, locale] of Object.entries({ 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ru, fa })) {
    it(`${lang}: labels subscription names and the active choice explicitly`, async () => {
      const i18n = createInstance();
      await i18n.init({ lng: lang, fallbackLng: false, resources: { [lang]: { translation: locale } }, interpolation: { escapeValue: false } });
      const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={i18n.t} />);
      expect(html).toContain(i18n.t('autoSelect.choiceSelected', { subscription: 'Polaris' }));
      expect(html).toContain(i18n.t('autoSelect.choice', { subscription: 'Travel' }));
      expect(html).toMatch(/data-auto-select-subscription="sub-a"[^>]*aria-pressed="true"[^>]*disabled/);
      expect(html).toMatch(/data-auto-select-subscription="sub-b"[^>]*aria-pressed="false"/);
      expect(html).toMatch(/data-auto-select-subscription="empty"[^>]*disabled/);
      expect(html).not.toContain('autoSelect.choiceSelected');
      expect(config.selectedServerId).toBe('node-b');
      const manual = renderToStaticMarkup(<AutoSelectControls config={{ ...config, selectionIntent: undefined }} accept={() => {}} t={i18n.t} />);
      expect(manual).toContain(i18n.t('autoSelect.choice', { subscription: 'Polaris' }));
      expect(manual).not.toContain(i18n.t('autoSelect.choiceSelected', { subscription: 'Polaris' }));
      expect(manual).not.toContain('aria-pressed="true"');
    });
  }
});

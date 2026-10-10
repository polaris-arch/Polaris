import { describe, expect, it } from 'vitest';
import { createInstance } from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import { AutoSelectStatusPanel } from './AutoSelectStatusPanel';
import { statusFixture } from '@/contracts/auto-select-status.test-support';
import {
  AUTO_SELECT_MODES, AUTO_SELECT_NOT_EVALUATED_REASONS, AUTO_SELECT_HOLD_REASONS,
  AUTO_SELECT_NO_DATA_REASONS, AUTO_SELECT_COMMIT_FAILED_REASONS, AUTO_SELECT_CAUSES, AUTO_SELECT_LACKING,
} from '@/contracts/auto-select';
import zhCN from '@/i18n/locales/zh-CN.json';
import zhTW from '@/i18n/locales/zh-TW.json';
import enUS from '@/i18n/locales/en-US.json';
import ru from '@/i18n/locales/ru.json';
import fa from '@/i18n/locales/fa.json';

const locales = { 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ru, fa };
const reasons = [...new Set([...AUTO_SELECT_NOT_EVALUATED_REASONS, ...AUTO_SELECT_HOLD_REASONS,
  ...AUTO_SELECT_NO_DATA_REASONS, ...AUTO_SELECT_COMMIT_FAILED_REASONS, ...AUTO_SELECT_CAUSES])];

describe('read-only Auto status projection', () => {
  for (const [lang, locale] of Object.entries(locales)) {
    it(`${lang}: all backend modes, reasons and lacking conditions have usable translations`, async () => {
      const i18n = createInstance();
      await i18n.init({ lng: lang, fallbackLng: false, resources: { [lang]: { translation: locale } }, interpolation: { escapeValue: false } });
      const render = (report = statusFixture()) => renderToStaticMarkup(<AutoSelectStatusPanel
        report={report} enabled loading={false} failed={false} refresh={() => {}} t={i18n.t}
        servers={[]} subscriptions={[]} />);
      for (const mode of AUTO_SELECT_MODES) {
        expect(i18n.exists(`autoSelect.mode.${mode}`)).toBe(true);
        expect(render(statusFixture({ mode }))).toContain(i18n.t(`autoSelect.mode.${mode}`));
      }
      for (const reason of reasons) {
        expect(i18n.exists(`autoSelect.reason.${reason}`), reason).toBe(true);
        const html = render(statusFixture({ reason }));
        expect(html).not.toContain('autoSelect.reason.');
        expect(html).toContain('data-auto-select-reason');
      }
      for (const lacking of AUTO_SELECT_LACKING) {
        expect(i18n.exists(`autoSelect.lacking.${lacking}`)).toBe(true);
        expect(render(statusFixture({ challenger: { serverId: 'node-b', wins: 1, winsRequired: 2, lacking } }))).not.toContain('autoSelect.lacking.');
      }
    });
  }

  it('disabled and backend manual state do not advertise Auto', () => {
    const props = { enabled: false, loading: false, failed: false, refresh: () => {}, t: createInstance().t, servers: [], subscriptions: [], report: statusFixture() };
    expect(renderToStaticMarkup(<AutoSelectStatusPanel {...props} />)).toBe('');
    expect(renderToStaticMarkup(<AutoSelectStatusPanel {...props} enabled report={statusFixture({ intent: { mode: 'manual', scope: null, subscriptionId: null }, mode: 'manual' })} />)).toBe('');
  });
});

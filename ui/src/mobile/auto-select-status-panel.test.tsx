import { describe, expect, it } from 'vitest';
import { createInstance } from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import { AutoSelectStatusPanel } from './AutoSelectStatusPanel';
import { nativeAutoFixture } from '@/contracts/native-auto.test-support';
import zhCN from '@/i18n/locales/zh-CN.json';
import zhTW from '@/i18n/locales/zh-TW.json';
import enUS from '@/i18n/locales/en-US.json';
import ru from '@/i18n/locales/ru.json';
import fa from '@/i18n/locales/fa.json';

describe('native Auto status projection', () => {
  for (const [lang, locale] of Object.entries({ 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ru, fa })) {
    it(`${lang}: confirmed TCP label and unknown winner remain distinct`, async () => {
      const i18n = createInstance();
      await i18n.init({ lng: lang, fallbackLng: false, resources: { [lang]: { translation: locale } } });
      const report = nativeAutoFixture({ nativeAutoGroups: [{ target: { scope: 'global' }, actualServerId: null, confirmed: false, state: 'pending' }] });
      const html = renderToStaticMarkup(<AutoSelectStatusPanel report={report} enabled loading={false} failed={false} refresh={() => {}} t={i18n.t} servers={[]} subscriptions={[]} />);
      expect(html).toContain(i18n.t('autoSelect.pendingNode'));
      expect(html).toContain(i18n.t('autoSelect.tcpNode'));
      expect(html).not.toContain('autoSelect.');
    });
  }
  it('disabled diagnostics do not create an old explanation banner', () => {
    expect(renderToStaticMarkup(<AutoSelectStatusPanel report={nativeAutoFixture()} enabled={false} loading={false} failed={false} refresh={() => {}} t={key => key} servers={[]} subscriptions={[]} />)).toBe('');
  });
});

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


describe('complete Auto evidence and decision presentation', () => {
  for (const [lang, locale] of Object.entries(locales)) {
    it(`${lang}: all actual scheduler idle/pause/skip causes and full state details are consumable`, async () => {
      const i18n = createInstance();
      await i18n.init({ lng: lang, fallbackLng: false, resources: { [lang]: { translation: locale } }, interpolation: { escapeValue: false } });
      const report = statusFixture({ basis: { state: 'fresh', latencyMs: 23, measuredAt: 1000, binding: 'unverified' },
        lastSwitch: { at: 1000, leg: 'select', cause: 'better', fromId: 'old', fromName: 'Old exit', toId: 'node-b', toName: 'New exit', fromLatencyMs: 80, toLatencyMs: 23, unverified: true },
        lastEvaluation: { at: 1000, outcome: 'switch', reason: 'better', counts: { selectable: 2, failed: 1, stale: 3, unmeasured: 4, skipped: 5, unverified: 2, needsRestart: 1 } },
        challenger: { serverId: 'node-b', wins: 1, winsRequired: 2, lacking: 'streak' },
        barred: [{ serverId: 'node-a', remainingMs: 240001, failures: 3 }],
        commit: { consecutiveFailures: 2, retryAfterMs: 60000, lastFailure: { at: 1000, toId: 'node-b', outcome: 'commitNotEligible' } },
        noDataForMs: 120001, flags: { stalled: true, starved: true, exitOutsideSubscription: true, exitOutsideOverdue: true, allUnverified: true },
        counters: { failoverSwitches: 3, selectSwitches: 4, betterSwitchesInWindow: 1, betterSwitchesMax: 3 } });
      const render = () => renderToStaticMarkup(<AutoSelectStatusPanel report={report} enabled loading={false} failed={false} refresh={() => {}} t={i18n.t} servers={[]} subscriptions={[]} />);
      const html = render();
      for (const key of ['outside', 'outsideOverdue', 'allUnverified', 'unverified', 'outcome.switch', 'cause.better']) expect(html).toContain(i18n.t(`autoSelect.${key}`));
      expect(html).toContain('1970-01-01T00:00:01.000Z');expect(html).toContain('Old exit');expect(html).toContain('New exit');expect(html).toContain('23 ms');
      expect(html).not.toContain(i18n.t('autoSelect.reason.better')); // a decision is not proof of runtime success
      const reasons = ['switchedOff', 'userDisabled', 'platformNotEnabled', 'coreNotRunning', 'noSubscription', 'background', 'metered', 'powerSave', 'gateBusy', 'preempted', 'budget', 'frozen', 'poolUnavailable', 'nothingTestable', 'noTargets', 'crashed'];
      for (const reason of reasons) {
        report.dataSource = { blockedBy: reason, lastSkip: reason };
        expect(i18n.exists(`autoSelect.dataReason.${reason}`)).toBe(true);
        expect(render()).toContain(i18n.t(`autoSelect.dataReason.${reason}`));
      }
    });
  }
  it('legacy additive status without failure count and oversized Unix timestamps remains renderable', async () => {
    const i18n = createInstance();await i18n.init({ lng: 'en-US', resources: { 'en-US': { translation: enUS } } });
    const html = renderToStaticMarkup(<AutoSelectStatusPanel report={statusFixture({ barred: [{ serverId: 'old', remainingMs: 120000 }], basis: { state: 'fresh', latencyMs: null, measuredAt: Number.MAX_VALUE, binding: null } })} enabled loading={false} failed={false} refresh={() => {}} t={i18n.t} servers={[]} subscriptions={[]} />);
    expect(html).toContain('Unavailable');expect(html).not.toContain('undefined');expect(html).not.toContain('NaN');
  });
});

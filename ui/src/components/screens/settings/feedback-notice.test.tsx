import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { Button, FeedbackNotice } from './Primitives';
import { helperActionErrorText } from '@/domain/action-error-text';
import zhCN from '@/i18n/locales/zh-CN.json';
vi.mock('react-i18next', async (original) => ({
  ...await original<typeof import('react-i18next')>(),
  useTranslation: () => ({ t: (key: string) => key }),
}));

const t = (key: string) => key.split('.').reduce<unknown>((value, part) => (value as Record<string, unknown>)[part], zhCN) as string;

describe('persistent feedback uses real settings primitives', () => {
  it('preserves the classified Helper reason, one next step and a real navigation action', () => {
    const html = renderToStaticMarkup(<FeedbackNotice title={t('helper.installFail')}
      detail={helperActionErrorText('installerFailed', t)} hint={t('helper.failureNextStep')} announce={false}
      actions={<Button variant="ghost" size="sm" onClick={() => {}}>{t('helper.viewLogsAction')}</Button>} />);
    expect(html).toContain('role="region"');
    expect(html).not.toContain('role="alert"');
    expect(html).not.toContain('aria-live');
    expect(html).toContain('feedback-notice-title');
    expect(html).toContain('feedback-notice-detail');
    expect(html).toContain('feedback-notice-hint');
    expect(html).toContain('feedback-notice-actions');
    expect(html).toContain(t('helper.actionError.installerFailed'));
    expect(html).toContain(t('helper.failureNextStep'));
    expect(html.match(/查看安装日志/g)).toHaveLength(1);
    expect(html).toContain('<button');
  });
  it('announces inline-only errors politely without repeating an identical detail', () => {
    const html = renderToStaticMarkup(<FeedbackNotice title="Preview failed" detail="Preview failed" />);
    expect(html).toContain('role="status"');
    expect(html).toContain('aria-live="polite"');
    expect(html).not.toContain('feedback-notice-detail');
    expect(html).not.toContain('feedback-notice-actions');
  });
  it('long text is escaped, not injected as markup or hidden behind a tooltip', () => {
    const html = renderToStaticMarkup(<FeedbackNotice title="Failed" detail={'<script>not markup</script>' + '长原因'.repeat(80)} />);
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('data-tip');
    expect(html).toContain('长原因'.repeat(80));
  });
  it('a busy original action stays disabled on the actual button', () => {
    const html = renderToStaticMarkup(<FeedbackNotice title="Failed" actions={<Button disabled onClick={() => {}}>Retry</Button>} />);
    expect(html).toMatch(/<button[^>]*disabled/);
  });
  it('Helper retains its explicit install and migration gates, not an automatic retry', () => {
    const source = readFileSync(new URL('./SettingsHelper.tsx', import.meta.url), 'utf8');
    expect(source).toContain('if (busy || installInFlight.current');
    expect(source).toContain("installFailure !== 'upgradeDeferred'");
    expect(source).toContain("navigate('logs')");
    expect(source).toContain('onClick={stopAndUpgrade}');
    expect(source).toContain('announce={false}');
  });
  it('all locale variants preserve the recheck/no-install distinction and saved-followup distinction', () => {
    for (const locale of ['en-US', 'fa', 'ru', 'zh-CN', 'zh-TW']) {
      const json = JSON.parse(readFileSync(new URL(`../../../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
      expect(Object.keys(json.feedback).sort()).toEqual(['connectionFailed', 'followupIncomplete', 'stopFailed', 'subscriptionSaved']);
      expect(json.helper.failureNextStep).toBeTruthy();
      expect(json.helper.actionError.installerFailed).toBeTruthy();
      expect(json.feedback.followupIncomplete).not.toBe(json.feedback.subscriptionSaved);
    }
  });
});

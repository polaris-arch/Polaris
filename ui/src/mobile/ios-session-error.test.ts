import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { ProxyErrorCode } from '@/contracts/types';
import { proxyErrorText } from '@/domain/proxy-error-text';
import { writeErrorsFromProxyStatus, writeFailureText } from './home/write-errors';
const t = ((key: string) => key) as never;
describe('iOS active system session guidance', () => {
  it('maps the stable code in both cold status and an awaited failure', () => {
    expect(proxyErrorText({ errorCode: ProxyErrorCode.IOS_SESSION_ACTIVE }, (key) => key)).toBe('errors.iosSessionActive');
    expect(writeErrorsFromProxyStatus({}, { running: false, errorCode: ProxyErrorCode.IOS_SESSION_ACTIVE }, t)).toEqual({ connect: 'errors.iosSessionActive' });
    expect(writeFailureText(t, { code: ProxyErrorCode.IOS_SESSION_ACTIVE })).toBe('errors.iosSessionActive');
  });
  it('preserves an existing error or a host-owned running status', () => {
    const errors = { connect: 'existing' };
    expect(writeErrorsFromProxyStatus(errors, { running: false, errorCode: ProxyErrorCode.IOS_SESSION_ACTIVE }, t)).toBe(errors);
    expect(writeErrorsFromProxyStatus({}, { running: true, errorCode: ProxyErrorCode.IOS_SESSION_ACTIVE }, t)).toEqual({});
  });
  it.each(['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'])('supplies %s guidance and honest configuration-only save text', (locale) => {
    const text = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
    expect(text.errors.iosSessionActive).toBeTruthy();
    expect(text.ts.iosAccountActionsUnavailable).toBeTruthy();
    expect(text.ts.iosConfigurationSaved).toBeTruthy();
  });
});

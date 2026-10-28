import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { restartReasonKeys } from './restart-apply-reasons';

describe('safe restart reasons and locale parity', () => {
  it('maps trusted structural paths and never renders kernel strings', () => {
    expect(restartReasonKeys([{ code: 'structuralChange', path: 'tun.enabled' }])).toEqual(['home.restartApplyTun']);
    expect(restartReasonKeys([{ code: 'unsupportedCore', path: 'password=secret' }])).toEqual(['home.restartApplyUnsupportedCore']);
    expect(restartReasonKeys([{ code: 'arbitrary instruction', path: 'password=secret' }])).toEqual(['home.restartApplyStructural']);
    expect(restartReasonKeys([{ code: 'structuralChange', path: 'password=secret' }])).toEqual(['home.restartApplyStructural']);
  });
  it('bounds and deduplicates reason rendering', () => {
    expect(restartReasonKeys(Array.from({length: 1000}, () => ({code:'structuralChange',path:'tun'})))).toEqual(['home.restartApplyTun']);
  });
  it('has all prompt and receipt translations in every shipped main locale', () => {
    const locales = ['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'];
    const keys = ['restartApplyTitle', 'restartApplyDescription', 'restartApplyConfirm', 'restartApplyKeepConnection',
      'restartApplyUnsupportedCore', 'restartApplyTun', 'restartApplyListener', 'restartApplyNamespace', 'restartApplyServices',
      'restartApplyCache', 'restartApplyStructural', 'pendingHotApplied', 'pendingHotDurabilityPending',
      'pendingHotConflict', 'pendingHotRejected', 'pendingHotUnconfirmed'];
    for (const locale of locales) {
      const home = JSON.parse(readFileSync(new URL(`../../i18n/locales/${locale}.json`, import.meta.url), 'utf8')).home;
      for (const key of keys) expect(typeof home[key], `${locale}:${key}`).toBe('string');
    }
  });
});

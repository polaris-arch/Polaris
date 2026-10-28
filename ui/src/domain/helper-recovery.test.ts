import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@/ipc/ipc-client', () => ({ invoke }));
import { needsHelperRecovery, revealHelperRecovery } from './helper-recovery';
import { handleProxyErrorEvent } from './proxy-error-routing';
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
vi.mock('@/lib/desktop-notify', () => ({ notifyDesktop: vi.fn() }));

beforeEach(() => vi.resetAllMocks());

describe('Helper recovery uses the existing visible settings flow', () => {
  it.each(['HELPER_CORE_MISMATCH', 'HELPER_CORE_ABSENT', 'HELPER_CORE_INVALID'])('routes %s', code => {
    expect(needsHelperRecovery(code)).toBe(true);
  });
  it.each(['CORE_BUNDLE_MISMATCH', 'HELPER_CORE_OCCUPIED', 'HELPER_GATE_ABORTED', '', null, undefined])('does not invent a repair for %s', code => {
    expect(needsHelperRecovery(code)).toBe(false);
  });
  it('asks the native window lifecycle to reveal/recreate main and deliver the narrow Helper intent', async () => {
    expect(await revealHelperRecovery()).toBe(true);
    expect(invoke.mock.calls).toEqual([['tray_show_main', { screen: 'settings-helper' }]]);
  });
  it('handles a failed window command without retrying or installing', async () => {
    invoke.mockRejectedValue(new Error('window unavailable'));
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(await revealHelperRecovery()).toBe(false);
    expect(invoke).toHaveBeenCalledTimes(1);
    log.mockRestore();
  });
  it.each(['HELPER_CORE_MISMATCH', 'HELPER_CORE_ABSENT', 'HELPER_CORE_INVALID'])('unclaimed %s events reveal repair and refresh the stopped state', code => {
    const refreshProxyStatus = vi.fn(async () => {});
    handleProxyErrorEvent({ errorCode: code, message: 'private diagnostic' }, { t: key => key, refreshProxyStatus });
    expect(refreshProxyStatus).toHaveBeenCalledOnce();
    expect(invoke.mock.calls).toEqual([['tray_show_main', { screen: 'settings-helper' }]]);
  });
  it('tray failure path is wired without depending on main renderer availability', () => {
    const source = readFileSync(new URL('../tray/TrayMenu.tsx', import.meta.url), 'utf8');
    const body = source.slice(source.indexOf('const noticeActionFailure'), source.indexOf('const connBtn'));
    expect(body).toContain('needsHelperRecovery');
    expect(body).toContain('void revealHelperRecovery()');
    expect(body).toContain('if (!mounted.current || !interactionActive.current) return');
    expect(body.indexOf('if (!mounted.current || !interactionActive.current) return')).toBeLessThan(body.indexOf('void revealHelperRecovery()'));
    const connect = source.slice(source.indexOf('const toggleConnect'), source.indexOf('const setMode'));
    expect(connect).toContain('life === trayLifecycle.current && operation === interactionEpoch.current');
    expect(connect).toContain('if (current()) noticeActionFailure');
  });
  it('installation failure stays visible and offers logs without rendering raw diagnostics', () => {
    const source = readFileSync(new URL('../components/screens/settings/SettingsHelper.tsx', import.meta.url), 'utf8');
    expect(source).toContain("setInstallFailure(r.errorCode ?? 'failed')");
    expect(source).toContain('data-testid="helper-install-failure"');
    expect(source).toContain('helperActionErrorText(installFailure, t)');
    expect(source).toContain("navigate('logs')");
    expect(source).not.toContain('r.diagnostic');
  });
});

it.each(['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'])('all installation stages have localized recovery copy in %s', locale => {
  const translations = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
  for (const code of ['invalidInstallation', 'tokenWriteFailed', 'scriptWriteFailed', 'authorizationLaunchFailed', 'installerFailed', 'upgradeDeferred']) {
    expect(translations.helper.actionError[code]).toEqual(expect.any(String));
    expect(translations.helper.actionError[code].length).toBeGreaterThan(10);
  }
  expect(translations.helper.failureNextStep).toBeTruthy();
  expect(translations.helper.viewLogsAction).toBeTruthy();
});

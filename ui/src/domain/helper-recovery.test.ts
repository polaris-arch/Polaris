import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import * as ts from '@/test/ts-compiler';
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@/ipc/ipc-client', () => ({ invoke }));
import { needsHelperRecovery, revealHelperRecovery } from './helper-recovery';
import { handleProxyErrorEvent } from './proxy-error-routing';
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
vi.mock('@/lib/desktop-notify', () => ({ notifyDesktop: vi.fn() }));

beforeEach(() => vi.resetAllMocks());

describe('Helper recovery uses the existing cancellable main-window explanation', () => {
  it.each(['HELPER_CORE_MISMATCH', 'HELPER_CORE_ABSENT', 'HELPER_CORE_INVALID'])('routes %s', code => {
    expect(needsHelperRecovery(code)).toBe(true);
  });
  it.each(['CORE_BUNDLE_MISMATCH', 'HELPER_CORE_OCCUPIED', 'HELPER_GATE_ABORTED', '', null, undefined])('does not invent a repair for %s', code => {
    expect(needsHelperRecovery(code)).toBe(false);
  });
  it('asks the native window lifecycle to reveal/recreate main and deliver the narrow Helper intent', async () => {
    expect(await revealHelperRecovery()).toBe(true);
    expect(invoke.mock.calls).toEqual([['tray_show_main', { screen: 'helper-recovery' }]]);
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
    expect(invoke.mock.calls).toEqual([['tray_show_main', { screen: 'helper-recovery' }]]);
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
    expect(source).toContain("failure = r.success ? null : r.errorCode ?? 'failed'");
    expect(source).toContain('completeHelperInstall(request, failure)');
    expect(source).toContain('subscribeHelperInstall(({ busy: installing, failure }) => {');
    expect(source).toContain('setInstallFailure(failure)');
    expect(source).toContain('data-testid="helper-install-failure"');
    expect(source).toContain('helperActionErrorText(installFailure, t)');
    expect(source).toContain("navigate('logs')");
    // Inspect code nodes rather than text suffixes: localized string keys are
    // data, while diagnostic property reads remain forbidden for any receiver.
    function diagnosticReads(body: string, fileName = 'helper-diagnostic-probe.tsx'): string[] {
      const file = ts.parseSourceFile(fileName, body);
      const reads: string[] = [];
      const literalName = (node: ts.Node | undefined): string | undefined => {
        if (!node) return undefined;
        if (ts.isParenthesizedExpression(node)) return literalName(node.expression);
        return ts.isIdentifier(node) || ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) ? node.text : undefined;
      };
      const visit = (node: ts.Node): void => {
        if ((ts.isPropertyAccessExpression(node) && node.name.text === 'diagnostic')
          || (ts.isElementAccessExpression(node) && literalName(node.argumentExpression) === 'diagnostic')
          || (ts.isBindingElement(node) && ts.isObjectBindingPattern(node.parent)
            && literalName(node.propertyName ?? node.name) === 'diagnostic')) reads.push(node.getText(file));
        ts.forEachChild(node, visit);
      };
      visit(file);
      return reads;
    }
    expect(diagnosticReads(`
      const a = r.diagnostic;
      const b = r ?. diagnostic;
      const c = r [ 'diagnostic' ];
      const d = r?.[ 'diagnostic' ];
      const e = (r). diagnostic;
      const { diagnostic: message } = r;
    `)).toHaveLength(6);
    expect(diagnosticReads(`
      const label = 'r.diagnostic'; // r.diagnostic is not consumed here
      const data = { diagnostic: 'definition only' };
      const view = <p>{t('helper.diagnosticsTitle')}</p>;
    `)).toEqual([]);
    expect(diagnosticReads(source, fileURLToPath(new URL('../components/screens/settings/SettingsHelper.tsx', import.meta.url)))).toEqual([]);
  });
});

it.each(['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'])('all installation stages have localized recovery copy in %s', locale => {
  const translations = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
  for (const code of ['invalidInstallation', 'tokenWriteFailed', 'scriptWriteFailed', 'authorizationLaunchFailed', 'installerFailed', 'upgradeDeferred']) {
    expect(translations.helper.actionError[code]).toEqual(expect.any(String));
    expect(translations.helper.actionError[code].length).toBeGreaterThan(10);
  }
  for (const key of ['connectionRecoveryTitle', 'connectionRecoveryMessage', 'connectionRecoverySettings']) {
    expect(translations.helper[key]).toEqual(expect.any(String));
    expect(translations.helper[key].length).toBeGreaterThan(key === 'connectionRecoveryMessage' ? 30 : 5);
  }
  expect(translations.helper.failureNextStep).toBeTruthy();
  expect(translations.helper.viewLogsAction).toBeTruthy();
});

it('Home, event routing and tray retain their existing claims while sharing the same recovery intent', () => {
  const home = readFileSync(new URL('../components/screens/home/HomeScreen.tsx', import.meta.url), 'utf8');
  const routing = readFileSync(new URL('./proxy-error-routing.ts', import.meta.url), 'utf8');
  const tray = readFileSync(new URL('../tray/TrayMenu.tsx', import.meta.url), 'utf8');
  for (const source of [home, routing, tray]) {
    expect(source).toContain('needsHelperRecovery');
    expect(source).toContain('void revealHelperRecovery()');
  }
  expect(home).toContain('withProxyStartClaim');
  const branch = routing.slice(routing.indexOf("if (data.errorCode === 'CORE_BUNDLE_MISMATCH'"));
  expect(branch).toContain('if (isProxyStartClaimed()) return');
  expect(branch.indexOf('if (isProxyStartClaimed()) return')).toBeLessThan(branch.indexOf('void revealHelperRecovery()'));
});

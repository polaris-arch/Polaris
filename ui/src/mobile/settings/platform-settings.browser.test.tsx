import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { GeneralPage } from '/src/mobile/settings/GeneralPage';
import { BackupPage } from '/src/mobile/settings/BackupPage';
import { UpdatePage } from '/src/mobile/settings/UpdatePage';
import { MobileSettingsScreen } from '/src/mobile/settings/MobileSettingsScreen';
import { AppAddPanel } from '/src/mobile/forms/AppAddPanel';
import { MobileRulesScreen } from '/src/mobile/screens/rules/RulesScreen';
import { useAppStore } from '/src/store/app-store';
import { useAppPresetsStore } from '/src/store/use-app-presets-store';
import { createCommit, WriteErrorsContext } from '/src/mobile/settings/write-feedback';
import { runAppUpdateCheck } from '/src/mobile/settings/app-update-check';
import { api, autoStartApi, privacyApi, updateApi, versionApi } from '/src/ipc/api-client';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/mobile.css';
import '/src/mobile/settings/settings.css';
await i18nReady; await i18n.changeLanguage('en-US');
const query = new URLSearchParams(location.search);
document.documentElement.dataset.mobileOs = query.get('os') ?? 'ios';
const control = window.__platformSettings = { bootReads: 0, bootWrites: [], backupReads: 0,
  passwordReads: [], skipCalls: [], writes: [], boot: true, authReads: 0, installedReads: 0, mutations: [] };
autoStartApi.getStatus = async () => { control.bootReads++; return control.boot; };
autoStartApi.set = async enabled => { control.bootWrites.push(enabled); control.boot = enabled; };
api.systemBackup.getStatus = async () => { control.backupReads++; return false; };
privacyApi.hasPassword = () => new Promise((resolve, reject) => control.passwordReads.push({ resolve, reject }));
privacyApi.setPassword = async () => ({ success: true });
updateApi.onProgress = () => () => {};
updateApi.getProgress = async () => null;
updateApi.skip = version => new Promise((resolve, reject) => control.skipCalls.push({ version, resolve, reject }));
versionApi.getInfo = async () => ({ appVersion: '1.0.0', platform: document.documentElement.dataset.mobileOs });
const initial = { autoPrivacyMode: true, autoConnect: true, autoStart: false, autoCheckUpdate: false,
  servers: [], subscriptions: [], trafficRules: [], dnsRules: [], customAppPresets: [],
  appRules: [{ appId: 'youtube', action: 'direct', enabled: true }] };
api.config.get = async () => initial;
api.config.onChangedReady = async () => () => {};
api.config.meshRouteReport = async () => null;
api.vpn.getAuthStatus = async () => { control.authReads++; return 'authorized'; };
api.system.listInstalledApps = async () => { control.installedReads++; return [{ packageName: 'fixture.app', label: 'Fixture app', system: false }]; };
api.ruleResources.getCatalog = async () => ({ items: [{ name: 'youtube', category: 'geosite' }] });
api.ruleResources.list = async () => [{ id: 'youtube-resource', name: 'geosite-youtube' }];
api.ruleResources.onProgress = () => () => {};
useAppStore.setState({ config: initial, servers: [], subscriptions: [],
  mutateConfigEntities: async mutations => { control.mutations.push(...mutations); } });
// Built-in labelKey is a suffix; appPresetLabel supplies the rules.apps. prefix.
useAppPresetsStore.setState({ loaded: true, loading: false, failed: false,
  presets: [{ id: 'youtube', labelKey: 'youtube', emoji: '▶', category: 'video',
    geositeTags: ['youtube'], geoipTags: [], processNames: [] }] });
if (query.get('page') === 'apps') control.inspectApps = () => {
  const { loaded, loading, failed, presets } = useAppPresetsStore.getState();
  return { language: i18n.language, loaded, loading, failed, presets,
    appRules: useAppStore.getState().config?.appRules };
};
await runAppUpdateCheck(async () => ({ hasUpdate: true, updateInfo: { version: '9.8.7' } }), initial);
function App() {
  const [config, setConfig] = useState(initial);
  const [errors, setErrors] = useState({});
  const commit = useMemo(() => createCommit(setErrors, i18n.t.bind(i18n)), []);
  const update = async patch => { control.writes.push(patch); setConfig(previous => ({ ...previous, ...patch })); };
  const mode = query.get('page');
  return <WriteErrorsContext.Provider value={errors}>{mode === 'root' ? <MobileSettingsScreen />
    : mode === 'app-add' ? <AppAddPanel instanceId="fixture-app" />
    : mode === 'apps' ? <MobileRulesScreen /> : mode === 'backup'
    ? <BackupPage commit={commit} /> : mode === 'update'
    ? <UpdatePage config={config} update={update} commit={commit} />
    : <GeneralPage config={config} update={update} commit={commit} />}</WriteErrorsContext.Provider>;
}
createRoot(document.getElementById('root')).render(<App />);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile platform settings and void skip receipts', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'platform-settings-fixture',
      resolveId(id) { if (id === '/platform-settings-fixture.tsx') return id; },
      load(id) { if (id === '/platform-settings-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__platform-settings', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__platform-settings',
          '<html lang="en-US"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/platform-settings-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  async function fixture(query: string): Promise<Page> {
    const page = await browser.newPage();
    await page.goto(origin + '/__platform-settings?' + query);
    await page.locator('[data-setting], .m-form-layer, .mr-screen').first().waitFor();
    return page;
  }

  it('iOS startup settings explain their absence without calling Android state APIs', async () => {
    const page = await fixture('os=ios');
    try {
      const startup = page.locator('[data-setting="auto-start"]');
      expect(await startup.textContent()).toContain('iOS does not support connecting automatically after boot.');
      expect(await startup.getByRole('switch').count()).toBe(0);
      expect(await page.locator('[data-setting="boot-auto-connect"]').textContent()).toContain('iOS does not provide Android');
      expect(await page.locator('[data-note="boot-auto-connect-path"]').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__platformSettings.bootReads)).toBe(0);
      expect(await page.evaluate(() => (window as any).__platformSettings.bootWrites)).toEqual([]);
    } finally { await page.close(); }
  });

  it('Android startup status remains authoritative and changing it writes then rereads native state', async () => {
    const page = await fixture('os=android');
    try {
      const toggle = page.locator('[data-setting="auto-start"]').getByRole('switch');
      await page.waitForFunction(() => document.querySelector('[data-setting="auto-start"] [role="switch"]')?.getAttribute('aria-checked') === 'true');
      await toggle.click();
      await page.waitForFunction(() => (window as any).__platformSettings.bootReads === 2);
      expect(await toggle.getAttribute('aria-checked')).toBe('false');
      expect(await page.evaluate(() => (window as any).__platformSettings.bootWrites)).toEqual([false]);
      expect(await page.evaluate(() => (window as any).__platformSettings.writes)).toEqual([{ autoStart: false }]);
    } finally { await page.close(); }
  });

  it('iOS system backup is an explanatory row and manual export/import remain available', async () => {
    const page = await fixture('os=ios&page=backup');
    try {
      const row = page.locator('[data-setting="system-backup"]');
      expect(await row.textContent()).toContain('it does not apply to iOS.');
      expect(await row.getByRole('switch').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__platformSettings.backupReads)).toBe(0);
      expect(await page.locator('[data-setting="backup-actions"]').getByRole('button').count()).toBeGreaterThan(0);
    } finally { await page.close(); }
  });

  it('unread or failed password state never claims unset and retry resolves an actual saved password', async () => {
    const page = await fixture('os=ios');
    try {
      const row = page.locator('[data-setting="privacy-password"]');
      expect(await row.textContent()).toContain('The lock password status has not been read yet.');
      expect(await row.textContent()).not.toContain('Not set');
      await page.waitForFunction(() => (window as any).__platformSettings.passwordReads.length === 1);
      await page.evaluate(() => (window as any).__platformSettings.passwordReads[0].reject(new Error('private backend detail')));
      await page.locator('[data-write-error="privacy-password"]').waitFor();
      expect(await row.textContent()).toContain('Could not read the lock password status. Please retry.');
      expect(await row.textContent()).not.toContain('private backend detail');
      expect(await row.textContent()).not.toContain('Not set');
      await row.getByRole('button', { name: 'Retry', exact: true }).click();
      await page.waitForFunction(() => (window as any).__platformSettings.passwordReads.length === 2);
      await page.evaluate(() => (window as any).__platformSettings.passwordReads[1].resolve(true));
      await row.getByRole('button', { name: 'Change password', exact: true }).waitFor();
      expect(await row.textContent()).toContain('password required after idle');
      expect(await page.locator('[data-write-error="privacy-password"]').count()).toBe(0);
    } finally { await page.close(); }
  });

  it('skip only updates the session after successful void completion; rejected skip stays retryable', async () => {
    const page = await fixture('os=android&page=update');
    try {
      const skip = page.getByRole('button', { name: 'Skip this version', exact: true });
      await skip.click();
      await page.waitForFunction(() => (window as any).__platformSettings.skipCalls.length === 1);
      await page.evaluate(() => (window as any).__platformSettings.skipCalls[0].reject(new Error()));
      await page.locator('[data-write-error="app-update"]').waitFor();
      expect(await skip.count()).toBe(1);
      await skip.click();
      await page.waitForFunction(() => (window as any).__platformSettings.skipCalls.length === 2);
      await page.evaluate(() => (window as any).__platformSettings.skipCalls[1].resolve());
      await skip.waitFor({ state: 'detached' });
      expect(await page.locator('[data-write-error="app-update"]').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__platformSettings.skipCalls.map((call: any) => call.version))).toEqual(['9.8.7', '9.8.7']);
    } finally { await page.close(); }
  });

  it('iOS authorization card describes the unsupported Android query and never requests it on foreground', async () => {
    const page = await fixture('os=ios&page=root');
    try {
      const status = page.locator('[data-status="vpn-authorization"]');
      await status.waitFor();
      expect(await status.textContent()).toBe('Not applicable');
      expect(await page.locator('[data-setting="vpn-authorization"]').textContent()).toContain('Android');
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      expect(await page.evaluate(() => (window as any).__platformSettings.authReads)).toBe(0);
    } finally { await page.close(); }
  });

  it('iOS custom app keeps domain/IP resources and save while omitting the Android package picker', async () => {
    const page = await fixture('os=ios&page=app-add');
    try {
      const form = page.getByRole('dialog', { name: 'Add Custom App', exact: true });
      await form.getByText('youtube', { exact: true }).waitFor();
      expect(await page.evaluate(() => (window as any).__platformSettings.installedReads)).toBe(0);
      expect(await page.getByRole('listbox', { name: 'Match by app', exact: true }).count()).toBe(0);
      expect(await page.locator('.m-form-body').textContent()).toContain('iOS');
      await form.locator('#maa-name').fill('iOS video');
      await form.getByText('youtube', { exact: true }).click();
      await form.getByRole('button', { name: 'Add Custom App', exact: true }).click();
      await page.waitForFunction(() => (window as any).__platformSettings.mutations.length === 1);
      const mutation = await page.evaluate(() => (window as any).__platformSettings.mutations[0]);
      expect(mutation.value.geositeTags).toEqual(['youtube']);
      expect(mutation.value.packageNames).toBeUndefined();
    } finally { await page.close(); }
  });

  it('Android still reads VPN authorization and offers installed package selection', async () => {
    const page = await fixture('os=android&page=root');
    try {
      await page.waitForFunction(() => (window as any).__platformSettings.authReads === 1);
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await page.waitForFunction(() => (window as any).__platformSettings.authReads === 2);
      await page.goto(origin + '/__platform-settings?os=android&page=app-add');
      const form = page.getByRole('dialog', { name: 'Add Custom App', exact: true });
      const packages = form.getByRole('listbox', { name: 'Match by app', exact: true });
      await packages.getByRole('option', { name: /Fixture app/ }).waitFor();
      expect(await page.evaluate(() => (window as any).__platformSettings.installedReads)).toBe(1);
      await packages.getByRole('option', { name: /Fixture app/ }).click();
      await form.locator('#maa-name').fill('Android video');
      await form.getByText('youtube', { exact: true }).click();
      await form.getByRole('button', { name: 'Add Custom App', exact: true }).click();
      await page.waitForFunction(() => (window as any).__platformSettings.mutations.length === 1);
      expect(await page.evaluate(() => (window as any).__platformSettings.mutations[0].value.packageNames)).toEqual(['fixture.app']);
    } finally { await page.close(); }
  });

  it('iOS app rows retain all routing policies and describe domain/IP matching without VPN exclusion promises', async () => {
    const page = await browser.newPage();
    const pageErrors: string[] = [];
    page.on('pageerror', error => { if (pageErrors.length < 12) pageErrors.push(error.message); });
    page.on('console', message => {
      if (message.type() === 'error' && pageErrors.length < 12) pageErrors.push(message.text());
    });
    const started = Date.now();
    let stage = 'navigate to the iOS apps fixture';
    const at = (next: string): void => {
      stage = next;
      // Leave time for the failure snapshot and page cleanup before Vitest's 5s deadline.
      page.setDefaultTimeout(Math.max(1, Math.min(1_000, 3_500 - (Date.now() - started))));
    };
    page.setDefaultNavigationTimeout(1_500);
    try {
      await page.goto(origin + '/__platform-settings?os=ios&page=apps');
      at('select the Apps tab (its accessible name also includes the app count)');
      const tabs = page.getByRole('tablist', { name: 'Routing', exact: true });
      const appsTab = tabs.getByRole('tab').filter({ has: page.getByText('Apps', { exact: true }) });
      await appsTab.waitFor();
      expect(await appsTab.count()).toBe(1);
      expect(await appsTab.locator('.mr-cnt').textContent()).toBe('1');
      expect(await appsTab.getAttribute('aria-selected')).toBe('false');
      await appsTab.click();
      at('render the YouTube app card after selecting Apps');
      const card = page.locator('.rc-app-card').filter({ hasText: 'YouTube' });
      await card.waitFor();
      expect(await appsTab.getAttribute('aria-selected')).toBe('true');
      expect(await card.count()).toBe(1);
      at('read the YouTube card title and matching summary');
      expect(await card.locator('.mr-row-title').textContent()).toBe('YouTube');
      const matching = card.locator('.rc-app-match');
      expect(await matching.locator('summary').textContent()).toContain('Domain / IP only');
      expect(await matching.locator('summary').textContent()).not.toContain('VPN exclusion');
      at('expand the YouTube matching details');
      await matching.locator('summary').click();
      expect(await matching.textContent()).toContain('iOS');
      expect(await matching.textContent()).not.toContain('A built-in preset with a package name can be excluded');
      at('open the YouTube policy control');
      await card.getByRole('button', { name: 'YouTube · Policy', exact: true }).click();
      at('render the YouTube policy dialog');
      const policies = page.getByRole('dialog', { name: 'YouTube', exact: true });
      await policies.waitFor();
      for (const name of ['Follow global', 'Direct', 'Block']) {
        at('read policy option ' + name);
        expect(await policies.getByRole('option', { name, exact: true }).count()).toBe(1);
      }
      at('verify the fixture Direct policy is selected');
      expect(await policies.getByRole('option', { name: 'Direct', exact: true }).getAttribute('aria-selected')).toBe('true');
    } catch (error) {
      const snapshot = await page.locator('body').evaluate(body => ({
        platform: document.documentElement.dataset.mobileOs,
        fixture: (window as any).__platformSettings?.inspectApps?.(),
        html: body.innerHTML.slice(0, 16_000),
      }), undefined, { timeout: 250 }).catch(snapshotError => ({ error: String(snapshotError) }));
      const reason = error instanceof Error ? error.stack ?? error.message : String(error);
      throw new Error('iOS app rows failed at: ' + stage + '\n' + reason
        + '\nPage errors: ' + JSON.stringify(pageErrors) + '\nDOM/fixture: ' + JSON.stringify(snapshot));
    } finally { await page.close(); }
  });
});

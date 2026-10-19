import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { beforeAll, afterAll, expect, it } from 'vitest';

const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { MobileFormHost } from '/src/mobile/forms/MobileFormHost';
import { DialogHost } from '/src/components/dialogs/DialogHost';
import { useDialogStore } from '/src/components/dialogs/dialog-store';
import { dismissTop } from '/src/mobile/back-stack';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { useVpnStatusStore } from '/src/store/use-vpn-status-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
import '/src/mobile/forms/forms.css';
await i18nReady; await i18n.changeLanguage('en-US');
const desktop = new URLSearchParams(location.search).has('desktop');
const forms = desktop ? useDialogStore : useMobileFormStore;
const test = window.__vpn = { calls: [], cancel: [], releases: [] };
for (const method of ['submitOpenVpn', 'submitOpenConnectForm', 'submitOpenConnectBrowser']) {
  api.vpn[method] = (...args) => { test.calls.push([method, ...args]); return new Promise((resolve, reject) => test.releases.push({ resolve, reject })); };
}
api.vpn.cancelOpenVpn = async (...args) => { test.cancel.push(['openvpn', ...args]); };
api.vpn.cancelOpenConnect = async (...args) => { test.cancel.push(['openconnect', ...args]); };
api.system.openExternal = async url => { test.url = url; };
test.status = (protocol, serverId, challenge) => useVpnStatusStore.getState()[protocol === 'openvpn' ? 'setOpenVpn' : 'setOpenConnect']({ serverId, state: 'AUTH', stateText: '', [protocol === 'openvpn' ? 'challenge' : 'authChallenge']: challenge });
test.open = (protocol, serverId, challenge) => { test.status(protocol, serverId, challenge); return forms.getState().open({ kind: 'vpn-auth', protocol, serverId }); };
test.stack = () => forms.getState().stack;
test.back = dismissTop;
createRoot(document.getElementById('root')).render(desktop ? <DialogHost /> : <MobileFormHost />);
`;
let server: ViteDevServer;
let browser: Browser;
let origin: string;
beforeAll(async () => {
  server = await createServer({ root: path.resolve(import.meta.dirname, '../../..'), server: { host: '127.0.0.1', port: 0 }, plugins: [{
    name: 'vpn-fixture', resolveId: id => id === '/vpn-fixture.tsx' ? id : undefined,
    load: id => id === '/vpn-fixture.tsx' ? entry : undefined,
    configureServer(vite) { vite.middlewares.use('/__vpn', async (_req, res) => {
      res.setHeader('Content-Type', 'text/html');
      res.end(await vite.transformIndexHtml('/__vpn', '<html><div id="root"></div><script type="module" src="/vpn-fixture.tsx"></script></html>'));
    }); },
  }] });
  await server.listen();
  const address = server.httpServer!.address();
  if (!address || typeof address === 'string') throw Error('No listener');
  origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
}, 30000);
afterAll(async () => { await browser?.close(); await server?.close(); });

it('mobile credentials and OTP preserve identity, reset secrets and ignore replaced challenge receipts', async () => {
  const page = await browser.newPage(); await page.goto(origin + '/__vpn');
  await page.waitForFunction('window.__vpn?.open');
  await page.evaluate(`window.__vpn.open('openvpn', 'a', { id: 'one', kind: 'credentials', echo: false, deadline: 0 })`);
  await page.locator('[id$=\"-vpn-username\"]').fill('synthetic'); await page.locator('[id$=\"-vpn-password\"]').fill('fixture-only');
  await page.locator('.m-form-foot .primary').click();
  expect(await page.evaluate('window.__vpn.calls[0]')).toEqual(['submitOpenVpn', 'a', 'one', 'synthetic', 'fixture-only', '']);
  await page.evaluate(`window.__vpn.status('openvpn', 'a', { id: 'two', kind: 'secret', echo: false, deadline: 0 })`);
  await page.locator('[id$=\"-vpn-secret\"]').waitFor();
  await page.evaluate('window.__vpn.releases[0].resolve()');
  expect(await page.locator('[id$=\"-vpn-secret\"]').inputValue()).toBe('');
  await page.locator('[id$=\"-vpn-secret\"]').fill('123456'); await page.locator('.m-form-foot .primary').click();
  expect(await page.evaluate('window.__vpn.calls[1]')).toEqual(['submitOpenVpn', 'a', 'two', '', '', '123456']);
  await page.locator('.m-form-foot .m-form-btn').first().click();
  expect(await page.evaluate('window.__vpn.cancel')).toEqual([['openvpn', 'a', 'two']]);
  await page.evaluate(`window.__vpn.open('openvpn', 'b', { id: 'three', kind: 'secret', echo: false, deadline: 0 }); window.__vpn.releases[1].resolve()`);
  await page.locator('[id$=\"-vpn-secret\"]').waitFor();
  expect(await page.evaluate('window.__vpn.stack()[0].serverId')).toBe('b');
  await page.evaluate(`window.__vpn.status('openvpn', 'b', undefined)`);
  await page.waitForFunction('window.__vpn.stack().length === 0'); await page.close();
});

it('mobile OpenConnect form choices and browser payload use shared commands; errors remain generic', async () => {
  const page = await browser.newPage(); await page.goto(origin + '/__vpn'); await page.waitForFunction('window.__vpn?.open');
  await page.evaluate(`window.__vpn.open('openconnect', 'oc', { id: 'form', kind: 'form', fields: [{ submissionKey: 'realm', name: 'realm', kind: 'text', value: 'staff', options: [{ value: 'staff', label: 'Staff' }] }, { submissionKey: 'pass', name: 'Password', kind: 'password', value: '', options: [] }] })`);
  await page.locator('[id$=\"-vpn-pass\"]').fill('synthetic-password'); await page.locator('.m-form-foot .primary').click();
  expect(await page.evaluate('window.__vpn.calls[0]')).toEqual(['submitOpenConnectForm', 'oc', 'form', { realm: 'staff', pass: 'synthetic-password' }]);
  const errors: string[] = []; page.on('console', msg => errors.push(msg.text()));
  await page.evaluate(`window.__vpn.releases[0].reject(new Error('DO_NOT_EXPOSE_SYNTHETIC_SECRET'))`);
  await page.waitForFunction('!document.querySelector(".m-form-foot .primary").disabled');
  expect(errors.join(' ')).not.toContain('DO_NOT_EXPOSE_SYNTHETIC_SECRET');
  await page.evaluate(`window.__vpn.status('openconnect', 'oc', { id: 'browser', kind: 'browser', fields: [], browser: { url: 'https://example.invalid/login', cookieNames: ['session'], headerNames: ['x-auth'], callbackURLPrefixes: [], earlyCookieNames: [] } })`);
  await page.getByRole('button', { name: 'Open browser', exact: false }).click();
  expect(await page.evaluate('window.__vpn.url')).toBe('https://example.invalid/login');
  await page.locator('[id$=\"-vpn-final-url\"]').fill('https://example.invalid/done'); await page.locator('[id$=\"-vpn-cookie-session\"]').fill('fixture-cookie'); await page.locator('[id$=\"-vpn-header-x-auth\"]').fill('one\n two');
  await page.locator('.m-form-foot .primary').click();
  expect(await page.evaluate('window.__vpn.calls[1]')).toEqual(['submitOpenConnectBrowser', 'oc', 'browser', 'https://example.invalid/done', [{ name: 'session', value: 'fixture-cookie' }], [{ name: 'x-auth', values: ['one', 'two'] }]]);
  await page.locator('.m-form-foot .m-form-btn').first().click();
  expect(await page.evaluate('window.__vpn.cancel')).toEqual([['openconnect', 'oc', 'browser']]); await page.close();
});


it('replacing hidden mobile endpoint A preserves the top endpoint B Back layer', async () => {
  const page = await browser.newPage(); await page.goto(origin + '/__vpn'); await page.waitForFunction('window.__vpn?.open');
  await page.evaluate(`window.__vpn.open('openvpn', 'a', { id: 'a1', kind: 'secret', echo: false, deadline: 0 })`);
  await page.locator('.m-form-layer').waitFor();
  await page.evaluate(`window.__vpn.open('openvpn', 'b', { id: 'b1', kind: 'secret', echo: false, deadline: 0 })`);
  await page.waitForFunction('document.querySelectorAll(".m-form-layer").length === 2');
  await page.evaluate(`window.__vpn.shells = [...document.querySelectorAll('.m-form-layer')]; window.__vpn.status('openvpn', 'a', { id: 'a2', kind: 'secret', echo: false, deadline: 0 })`);
  await page.waitForTimeout(50);
  expect(await page.evaluate('window.__vpn.shells.every((shell, i) => shell === document.querySelectorAll(".m-form-layer")[i])')).toBe(true);
  await page.evaluate('window.__vpn.back()');
  await page.waitForFunction('window.__vpn.stack().length === 1');
  expect(await page.evaluate('window.__vpn.stack()[0].serverId')).toBe('a');
  expect(await page.evaluate('window.__vpn.cancel')).toEqual([['openvpn', 'b', 'b1']]); await page.close();
});

it('replacing hidden desktop endpoint A preserves modal identity and focus in B', async () => {
  const page = await browser.newPage(); await page.goto(origin + '/__vpn?desktop'); await page.waitForFunction('window.__vpn?.open');
  await page.evaluate(`window.__vpn.open('openvpn', 'a', { id: 'a1', kind: 'secret', echo: false, deadline: 0 })`);
  await page.locator('dialog').waitFor();
  await page.evaluate(`window.__vpn.open('openvpn', 'b', { id: 'b1', kind: 'secret', echo: false, deadline: 0 })`);
  await page.waitForFunction('document.querySelectorAll("dialog").length === 2');
  await page.locator('dialog').last().locator('input').focus();
  await page.evaluate(`window.__vpn.focus = document.activeElement; window.__vpn.shells = [...document.querySelectorAll('dialog')]; window.__vpn.status('openvpn', 'a', { id: 'a2', kind: 'secret', echo: false, deadline: 0 })`);
  await page.waitForTimeout(50);
  expect(await page.evaluate('window.__vpn.focus === document.activeElement')).toBe(true);
  expect(await page.evaluate('window.__vpn.shells.every((shell, i) => shell === document.querySelectorAll("dialog")[i])')).toBe(true); await page.close();
});

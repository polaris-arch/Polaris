import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { SubscriptionUserAgentInfo } from '@/contracts/subscription-user-agent';

// Existing Vitest/Vite/Playwright lane; only the read-only native port is replaced.
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { useSubscriptionUserAgent } from '/src/hooks/use-subscription-user-agent';
import { SubscriptionUserAgentHint } from '/src/components/dialogs/SubscriptionUserAgentHint';
import { MobileSubscriptionUserAgentHint } from '/src/mobile/forms/SubscriptionUserAgentHint';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/forms/forms.css';
import { subscriptionApi } from '/src/ipc/api/subscriptions';
import { useAppStore } from '/src/store/app-store';
import i18n, { i18nReady } from '/src/i18n';
await i18nReady; await i18n.changeLanguage('en-US');
let view;
const mobile = new URLSearchParams(location.search).get('surface') === 'mobile';
const Hint = mobile ? MobileSubscriptionUserAgentHint : SubscriptionUserAgentHint;
const test = window.__uaTest = { requests: [], submits: 0, mount, unmount,
  global(value) { useAppStore.setState({config:{subscriptionUserAgent:value}}); },
  reply(index, value) { test.requests[index].resolve(value); },
  fail(index) { test.requests[index].reject(new Error('synthetic read failure')); },
};
subscriptionApi.userAgentInfo = userAgent => new Promise((resolve,reject) => test.requests.push({userAgent,resolve,reject}));
function Probe() {
  const [ua,setUa] = useState('');
  const model = useSubscriptionUserAgent(ua);
  return <form className={mobile ? 'm-form-row' : 'fld'} onSubmit={e=>{e.preventDefault(); test.submits++;}}>
    <input data-testid="ua-input" aria-describedby="ua-info" value={ua} onChange={e=>setUa(e.target.value)}
    placeholder={model.status==='ready' && model.info.effectiveStatus==='ready' ? model.info.fallbackUserAgent : i18n.t('sub.uaPh')} />
    <Hint id="ua-info" model={model} /></form>;
}
function mount() { view=createRoot(document.getElementById('root')); view.render(<Probe/>); }
function unmount() { view.unmount(); }
mount();
`;
interface TestPort {
  requests: { userAgent: string }[];
  submits: number;
  mount: () => void;
  unmount: () => void;
  global: (value: string) => void;
  reply: (index: number, info: unknown) => void;
  fail: (index: number) => void;
}
type TestWindow = Window & { __uaTest: TestPort };
const INFO: SubscriptionUserAgentInfo = {
  effectiveStatus: 'ready',
  appVersion: 'app-real-port-fixture', coreVersion: 'kernel-real-port-fixture',
  defaultUserAgent: 'opaque-native-default/1', fallbackUserAgent: 'opaque-native-default/1',
  effectiveUserAgent: 'opaque-native-default/1', source: 'default',
};
let server: ViteDevServer;
let browser: Browser;
let origin: string;
const count = (page: Page, value: number) => page.waitForFunction(n => (window as unknown as TestWindow).__uaTest.requests.length === n, value);
const reply = (page: Page, index: number, info: unknown) => page.evaluate(({ index, info }) => (window as unknown as TestWindow).__uaTest.reply(index, info), { index, info });
const text = (page: Page) => page.locator('#ua-info').textContent();

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1').each(['desktop', 'mobile'] as const)('%s backend-owned subscription UA display', surface => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'subscription-user-agent-fixture',
      resolveId(id) { if (id === '/user-agent-fixture.tsx') return id; },
      load(id) { if (id === '/user-agent-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__ua', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__ua', '<html><div id="root"></div><script type="module" src="/user-agent-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, chromiumSandbox: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });
  async function pageForTest() {
    const page = await browser.newPage({ viewport: { width: surface === 'mobile' ? 390 : 1024, height: 844 } });
    try {
      await page.goto(`${origin}/__ua?surface=${surface}`);
      // The module awaits i18n before installing the port; navigation alone is not readiness.
      await page.waitForFunction(() => '__uaTest' in window, undefined, { timeout: 10_000 });
      await count(page, 1);
      return page;
    } catch (error) {
      await page.close();
      throw error;
    }
  }
  async function ready(page: Page, expected: string) {
    await page.waitForFunction(value => document.querySelector('#ua-info')?.textContent?.includes(value), expected);
  }
  async function metadataText(page: Page) {
    if (surface === 'desktop') return page.locator('#ua-info .info-i').getAttribute('aria-label');
    const trigger = page.locator('#ua-info .m-info-trigger');
    await trigger.click();
    const dialog = page.getByRole('dialog');
    await dialog.waitFor({ state: 'visible' });
    const value = await dialog.textContent();
    await page.keyboard.press('Escape');
    await dialog.waitFor({ state: 'hidden' });
    expect(await trigger.evaluate(node => document.activeElement === node)).toBe(true);
    return value;
  }


  it('shows actual default, global and custom values without filling the optional input; failure clears stale identity and retries', async () => {
    const page = await pageForTest();
    try {
      expect(await page.getByTestId('ua-input').inputValue()).toBe('');
      expect(await text(page)).toContain('Reading versions');
      await reply(page, 0, INFO); await ready(page, INFO.effectiveUserAgent!);
      expect(await metadataText(page)).toContain(INFO.coreVersion);
      expect(await text(page)).toContain(INFO.defaultUserAgent);
      expect(await page.locator('#ua-info bdi').count()).toBe(1);
      expect(await text(page)).not.toContain(INFO.appVersion);
      expect(await page.getByTestId('ua-input').getAttribute('placeholder')).toBe(INFO.fallbackUserAgent);
      expect(await page.getByTestId('ua-input').inputValue()).toBe('');
      await page.evaluate(() => (window as unknown as TestWindow).__uaTest.global('global/2'));
      await count(page, 2);
      expect(await text(page)).not.toContain(INFO.defaultUserAgent);
      await reply(page, 1, { ...INFO, source: 'global', effectiveUserAgent: 'global/2', fallbackUserAgent: 'global/2' });
      await ready(page, 'global setting');
      expect(await page.getByTestId('ua-input').getAttribute('placeholder')).toBe('global/2');
      await page.getByTestId('ua-input').fill(' custom/3 '); await count(page, 3);
      expect(await page.evaluate(() => (window as unknown as TestWindow).__uaTest.requests[2].userAgent)).toBe(' custom/3 ');
      await reply(page, 2, { ...INFO, source: 'subscription', effectiveUserAgent: 'custom/3', fallbackUserAgent: 'global/2' });
      await ready(page, 'custom/3');
      expect(await page.getByTestId('ua-input').inputValue()).toBe(' custom/3 ');
      await page.getByTestId('ua-input').fill('custom/4'); await count(page, 4);
      await page.evaluate(() => (window as unknown as TestWindow).__uaTest.fail(3));
      await ready(page, 'Could not read versions');
      expect(await text(page)).not.toContain(INFO.appVersion);
      expect(await text(page)).not.toContain(INFO.defaultUserAgent);
      expect(await page.getByTestId('ua-input').inputValue()).toBe('custom/4');
      await page.getByRole('button', { name: 'Retry', exact: true }).click(); await count(page, 5);
      expect(await page.evaluate(() => (window as unknown as TestWindow).__uaTest.submits)).toBe(0);
      expect(await page.getByTestId('ua-input').inputValue()).toBe('custom/4');
      await reply(page, 4, { ...INFO, source: 'subscription', effectiveUserAgent: 'custom/4' }); await ready(page, 'custom/4');
    } finally { await page.close(); }
  }, 30_000);

  it('keeps known build identity when saved policy is cold/busy, without presenting the app default as effective', async () => {
    const page = await pageForTest();
    try {
      await reply(page, 0, { ...INFO, effectiveStatus: 'unavailable', fallbackUserAgent: null,
        effectiveUserAgent: null, source: null });
      await ready(page, 'User-Agent preview unavailable');
      const metadata = await metadataText(page);
      expect(metadata).toContain(INFO.appVersion);
      expect(metadata).toContain(INFO.coreVersion);
      expect(metadata).toContain(INFO.defaultUserAgent);
      expect(await page.locator('#ua-info bdi').count()).toBe(0);
      expect(await text(page)).not.toContain('Planned User-Agent (app default)');
      expect(await page.getByTestId('ua-input').getAttribute('placeholder')).not.toBe(INFO.defaultUserAgent);
      expect(await page.getByTestId('ua-input').inputValue()).toBe('');
      await page.getByRole('button', { name: 'Retry', exact: true }).click(); await count(page, 2);
      expect(await page.evaluate(() => (window as unknown as TestWindow).__uaTest.submits)).toBe(0);
      await reply(page, 1, { ...INFO, source: 'global', effectiveUserAgent: 'real-global/2', fallbackUserAgent: 'real-global/2' });
      await ready(page, 'real-global/2');
      expect(await page.getByTestId('ua-input').getAttribute('placeholder')).toBe('real-global/2');
    } finally { await page.close(); }
  }, 30_000);

  it('rejects older A→B→A replies and does not revive cached A while the latest read is pending', async () => {
    const page = await pageForTest();
    try {
      await reply(page, 0, INFO); await ready(page, INFO.effectiveUserAgent!);
      await page.getByTestId('ua-input').fill('A'); await count(page, 2);
      await page.getByTestId('ua-input').fill('B'); await count(page, 3);
      await page.getByTestId('ua-input').fill('A'); await count(page, 4);
      await reply(page, 1, { ...INFO, appVersion: 'stale-A' });
      await reply(page, 2, { ...INFO, appVersion: 'stale-B' });
      expect(await text(page)).not.toMatch(/stale-A|stale-B|app-real-port-fixture/);
      expect(await page.locator('#ua-info .info-i, #ua-info .m-info-trigger').count()).toBe(0);
      await reply(page, 3, { ...INFO, appVersion: 'current-A', source: 'subscription', effectiveUserAgent: 'A' });
      await ready(page, 'Planned User-Agent (subscription setting): A');
      expect(await metadataText(page)).toContain('current-A');
      expect(await metadataText(page)).not.toMatch(/stale-A|stale-B/);
      expect(await page.getByTestId('ua-input').inputValue()).toBe('A');
    } finally { await page.close(); }
  }, 30_000);

  it('ignores unmounted replies and visibly rejects unavailable/malformed native identity', async () => {
    const page = await pageForTest();
    try {
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__uaTest; t.unmount(); t.mount(); });
      await count(page, 2);
      await reply(page, 0, { ...INFO, appVersion: 'disposed' });
      expect(await text(page)).not.toContain('disposed');
      await reply(page, 1, { ...INFO, coreVersion: 'unknown' }); await ready(page, 'Could not read versions');
      expect(await text(page)).not.toContain('unknown');
      expect(await text(page)).not.toContain(INFO.defaultUserAgent);
      expect(await page.locator('#ua-info .info-i, #ua-info .m-info-trigger, #ua-info bdi').count()).toBe(0);
      await page.getByRole('button', { name: 'Retry', exact: true }).click(); await count(page, 3);
      await reply(page, 2, undefined); await ready(page, 'Could not read versions');
      expect(await text(page)).not.toContain(INFO.appVersion);
    } finally { await page.close(); }
  }, 30_000);

  it.runIf(surface === 'mobile')('uses the mobile retry hit target and wraps opaque long UA text within the viewport', async () => {
    const page = await pageForTest();
    try {
      const longUserAgent = 'opaque'.repeat(260);
      await reply(page, 0, { ...INFO, defaultUserAgent: longUserAgent,
        effectiveUserAgent: longUserAgent, fallbackUserAgent: longUserAgent });
      await ready(page, longUserAgent);
      const hint = page.locator('#ua-info');
      expect(await hint.getAttribute('class')).toBe('m-form-hint');
      expect(await hint.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
      expect(await hint.evaluate(node => node.getBoundingClientRect().right <= window.innerWidth)).toBe(true);
      await page.getByTestId('ua-input').fill('retry-current'); await count(page, 2);
      await page.evaluate(() => (window as unknown as TestWindow).__uaTest.fail(1));
      await ready(page, 'Could not read versions');
      const retry = page.getByRole('button', { name: 'Retry', exact: true });
      expect(await retry.getAttribute('class')).toBe('m-form-btn');
      expect(await retry.getAttribute('type')).toBe('button');
      expect((await retry.boundingBox())?.height).toBeGreaterThanOrEqual(44);
      await retry.focus();
      expect(await retry.evaluate(node => document.activeElement === node)).toBe(true);
      await retry.press('Enter'); await count(page, 3);
      expect(await page.evaluate(() => (window as unknown as TestWindow).__uaTest.submits)).toBe(0);
    } finally { await page.close(); }
  }, 30_000);
});

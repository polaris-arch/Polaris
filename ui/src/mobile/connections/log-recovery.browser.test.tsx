import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { LogRow } from '@/components/screens/logs/logs-buffer';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MobileConnectionsScreen } from '/src/mobile/connections/MobileConnectionsScreen';
import { api } from '/src/ipc/api-client';
import { useAppStore } from '/src/store/app-store';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
await i18nReady; await i18n.changeLanguage('en-US');
const state = window.__mobileLogRecovery = { listeners: [], reads: [], searches: [], unsubscribed: [] };
api.logs.onReceivedBatchReady = callback => new Promise((resolve, reject) => {
  const listener = { callback, off: 0, resolve: () => resolve(() => listener.off++), reject };
  state.listeners.push(listener);
});
api.logs.get = id => new Promise((resolve, reject) => state.reads.push({ id, resolve, reject }));
api.logs.search = query => new Promise((resolve, reject) => state.searches.push({ query, resolve, reject }));
api.logs.unsubscribe = async id => { state.unsubscribed.push(id); };
api.logs.diagnosticState = async () => false;
api.logs.runtimeLevel = async () => ({ level: null, reason: 'notRunning' });
api.logs.legacyInfo = async () => ({ exists: false, bytes: 0, path: '' });
api.proxy.onLifecycle = () => () => {};
api.stats.onConnectionsAggregate = () => () => {};
api.stats.onConnectionsDetail = () => () => {};
api.stats.onConnectionsClosed = () => () => {};
api.stats.subscribe = async () => {};
api.stats.unsubscribe = async () => {};
useAppStore.setState({ config: { logLevel: 'info', servers: [], subscriptions: [], trafficRules: [], dnsRules: [] }, privacyMode: false });
function App() {
  const [visible, setVisible] = useState(true);
  state.hide = () => setVisible(false); state.show = () => setVisible(true);
  return visible ? <MobileConnectionsScreen /> : <p>Fixture hidden</p>;
}
createRoot(document.getElementById('root')).render(<App />);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;
const row = (id: number, message: string): LogRow => ({ _id: id, message, timestamp: '2026-11-02T00:00:00Z', level: 'info', source: 'app' });

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile log ownership and read recovery', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'mobile-log-recovery-fixture',
      resolveId(id) { if (id === '/mobile-log-recovery-fixture.tsx') return id; },
      load(id) { if (id === '/mobile-log-recovery-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__mobile-log-recovery', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__mobile-log-recovery',
          '<html lang="en-US"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/mobile-log-recovery-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  async function fixture(): Promise<Page> {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    await page.goto(origin + '/__mobile-log-recovery');
    await page.getByRole('tab', { name: 'Logs', exact: true }).click();
    await page.waitForFunction(() => (window as any).__mobileLogRecovery.listeners.length === 1);
    return page;
  }
  async function ready(page: Page, index: number, readIndex = index): Promise<void> {
    await page.waitForFunction(index => (window as any).__mobileLogRecovery.listeners.length > index, index);
    await page.evaluate(index => (window as any).__mobileLogRecovery.listeners[index].resolve(), index);
    await page.waitForFunction(index => (window as any).__mobileLogRecovery.reads.length > index, readIndex);
  }
  async function hydrate(page: Page, index: number, rows: LogRow[]): Promise<void> {
    await ready(page, index);
    await page.evaluate(({ index, rows }) => (window as any).__mobileLogRecovery.reads[index].resolve(rows), { index, rows });
    await page.locator('.mc-live').waitFor();
  }

  it('makes listener failure visible, retries with a new owner, and becomes live only after valid hydration', async () => {
    const page = await fixture();
    try {
      await page.evaluate(() => (window as any).__mobileLogRecovery.listeners[0].reject(new Error('fixture-private-error')));
      const error = page.locator('[data-log-stream-error]');
      await error.waitFor();
      expect(await error.textContent()).toContain('The live log stream could not connect.');
      expect(await error.textContent()).not.toContain('fixture-private-error');
      expect(await page.locator('.mc-live').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__mobileLogRecovery.reads.length)).toBe(0);
      await error.getByRole('button', { name: 'Retry', exact: true }).click();
      await ready(page, 1, 0);
      expect(await page.locator('.mc-live').count()).toBe(0);
      // A failed listener must not remain a writer after the new owner starts.
      await page.evaluate(rows => (window as any).__mobileLogRecovery.listeners[0].callback(rows), [row(90, 'obsolete listener')]);
      await page.evaluate(() => (window as any).__mobileLogRecovery.reads[0].resolve(null));
      await error.waitFor();
      expect(await page.locator('.mc-log-line').count()).toBe(0);
      await error.getByRole('button', { name: 'Retry', exact: true }).click();
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.listeners.length === 3);
      await page.evaluate(() => (window as any).__mobileLogRecovery.listeners[2].resolve());
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.reads.length === 2);
      await page.evaluate(rows => (window as any).__mobileLogRecovery.reads[1].resolve(rows), [row(1, 'reconnected')]);
      await page.getByText('reconnected', { exact: true }).waitFor();
      expect(await error.count()).toBe(0);
      expect(await page.locator('.mc-live').count()).toBe(1);
      expect(await page.evaluate(() => (window as any).__mobileLogRecovery.listeners[1].off)).toBe(1);
    } finally { await page.close(); }
  });

  it('keeps paused records frozen through failure and retry, merging history and pending ids once', async () => {
    const page = await fixture();
    try {
      await hydrate(page, 0, [row(1, 'frozen first')]);
      await page.getByRole('button', { name: 'More', exact: true }).click();
      await page.locator('.mc-sheet-item').filter({ hasText: 'Auto-scroll' }).click();
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      await page.evaluate(rows => (window as any).__mobileLogRecovery.listeners[0].callback(rows), [row(2, 'buffered second')]);
      await page.getByRole('tab', { name: 'Overview', exact: true }).click();
      await page.getByRole('tab', { name: 'Logs', exact: true }).click();
      await ready(page, 1);
      await page.evaluate(() => (window as any).__mobileLogRecovery.reads[1].reject(new Error('fixture read failed')));
      await page.locator('[data-log-stream-error]').getByRole('button', { name: 'Retry', exact: true }).click();
      await hydrate(page, 2, [row(1, 'frozen first'), row(2, 'buffered second'), row(3, 'retry third')]);
      expect(await page.locator('.mc-log-line').count()).toBe(1);
      expect(await page.locator('.mc-log-line').textContent()).toContain('frozen first');
      expect(await page.locator('[data-log-jump] .mc-cnt').textContent()).toBe('2');
      await page.locator('[data-log-jump]').click();
      await page.getByText('retry third', { exact: true }).waitFor();
      expect(await page.locator('.mc-log-line').count()).toBe(3);
    } finally { await page.close(); }
  });

  it('ignores late callbacks and snapshot errors after tab exit or unmount, and retires only their tokens', async () => {
    const page = await fixture();
    try {
      await ready(page, 0);
      await page.getByRole('tab', { name: 'Overview', exact: true }).click();
      await page.getByRole('tab', { name: 'Logs', exact: true }).click();
      await hydrate(page, 1, [row(10, 'new owner')]);
      await page.evaluate(rows => {
        const state = (window as any).__mobileLogRecovery;
        state.listeners[0].callback(rows); state.reads[0].resolve(rows);
      }, [row(100, 'obsolete owner')]);
      expect(await page.getByText('obsolete owner', { exact: true }).count()).toBe(0);
      expect(await page.locator('[data-log-stream-error]').count()).toBe(0);
      const oldToken = await page.evaluate(() => (window as any).__mobileLogRecovery.reads[0].id as string);
      expect(await page.evaluate(() => (window as any).__mobileLogRecovery.unsubscribed)).toContain(oldToken);
      await page.evaluate(() => (window as any).__mobileLogRecovery.hide());
      await page.getByText('Fixture hidden', { exact: true }).waitFor();
      await page.evaluate(rows => (window as any).__mobileLogRecovery.listeners[1].callback(rows), [row(101, 'after unmount')]);
      expect(await page.evaluate(() => (window as any).__mobileLogRecovery.listeners[1].off)).toBe(1);
      await page.evaluate(() => (window as any).__mobileLogRecovery.show());
      await page.getByRole('tab', { name: 'Logs', exact: true }).click();
      await ready(page, 2);
      await page.evaluate(() => (window as any).__mobileLogRecovery.hide());
      await page.getByText('Fixture hidden', { exact: true }).waitFor();
      await page.evaluate(() => (window as any).__mobileLogRecovery.reads[2].reject(new Error('obsolete rejection')));
      await page.evaluate(() => (window as any).__mobileLogRecovery.show());
      await page.getByRole('tab', { name: 'Logs', exact: true }).click();
      await hydrate(page, 3, [row(20, 'fresh mount')]);
      expect(await page.locator('[data-log-stream-error]').count()).toBe(0);
      expect(await page.locator('.mc-log-line').count()).toBe(1);
    } finally { await page.close(); }
  });

  it('cancels obsolete searches and preserves the current query across a visible failure and retry', async () => {
    const page = await fixture();
    try {
      await hydrate(page, 0, [row(1, 'original history')]);
      const search = page.locator('[data-conn-search] input');
      await search.fill('alpha');
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.searches.length === 1);
      await search.fill('beta');
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.searches.length === 2);
      await page.evaluate(rows => (window as any).__mobileLogRecovery.searches[1].resolve(rows), [row(2, 'beta current')]);
      await page.getByText('beta current', { exact: true }).waitFor();
      await page.evaluate(() => (window as any).__mobileLogRecovery.searches[0].reject(new Error('obsolete search')));
      expect(await page.locator('[data-log-stream-error]').count()).toBe(0);
      await search.fill('gamma');
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.searches.length === 3);
      await page.evaluate(() => (window as any).__mobileLogRecovery.searches[2].reject(new Error('current search failed')));
      await page.locator('[data-log-stream-error]').waitFor();
      expect(await page.locator('[data-log-stream-error]').textContent()).toContain('Operation failed');
      expect(await page.locator('[data-log-stream-error]').textContent()).not.toContain('live log stream could not connect');
      expect(await page.locator('.mc-live').count()).toBe(1);
      await page.locator('[data-log-stream-error]').getByRole('button', { name: 'Retry', exact: true }).click();
      await hydrate(page, 1, [row(1, 'original history')]);
      expect(await search.inputValue()).toBe('gamma');
      await page.waitForFunction(() => (window as any).__mobileLogRecovery.searches.length === 4);
      await page.evaluate(rows => (window as any).__mobileLogRecovery.searches[3].resolve(rows), [row(3, 'gamma recovered')]);
      await page.getByText('gamma recovered', { exact: true }).waitFor();
      expect(await page.locator('[data-log-stream-error]').count()).toBe(0);
      await search.fill('');
      await page.getByText('original history', { exact: true }).waitFor();
    } finally { await page.close(); }
  });
});

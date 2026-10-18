import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { NodeDialog } from '/src/components/dialogs/NodeDialog';
import { api } from '/src/ipc';
import { systemApi } from '/src/ipc/api-client';
import { useAppStore } from '/src/store/app-store';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
await i18nReady; await i18n.changeLanguage('en-US');
systemApi.listNetworkInterfaces = async () => [];
const test = window.__n1 = { requests: [], reset: null, unmount: null };
api.server.tailcatKeypair = input => new Promise((resolve, reject) => test.requests.push({ input, resolve, reject }));
useAppStore.setState({ servers: [], config: { servers: [], subscriptions: [] } });
function App() {
 const [revision, setRevision] = useState(0), [visible, setVisible] = useState(true);
 test.reset = () => { setRevision(n => n + 1); setVisible(true); };
 test.unmount = () => setVisible(false);
 return visible ? <NodeDialog key={revision} instanceId={'test-' + revision} initialProto="tailcat" /> : <p>Unmounted</p>;
}
createRoot(document.getElementById('root')).render(<App />);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('N1 real component transaction regressions', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'n1-component-fixture', resolveId(id) { if (id === '/n1-fixture.tsx') return id; },
      load(id) { if (id === '/n1-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__n1', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__n1', '<html lang="en-US"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/n1-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (typeof address !== 'object' || !address) throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('late generated keys cannot overwrite typed draft or a newer key response', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1');
      await page.getByRole('button', { name: 'Generate key pair', exact: true }).click();
      await page.locator('#nd-f-privateKey').fill('user-edited-private');
      await page.getByRole('button', { name: 'Show public key', exact: true }).click();
      await page.evaluate(() => (window as any).__n1.requests[1].resolve({ privateKey: 'unused', publicKey: 'new-public' }));
      await page.waitForFunction(() => (document.querySelector('[aria-label="Client public key"]') as HTMLInputElement)?.value === 'new-public');
      await page.evaluate(() => (window as any).__n1.requests[0].resolve({ privateKey: 'stale-private', publicKey: 'stale-public' }));
      expect(await page.locator('#nd-f-privateKey').inputValue()).toBe('user-edited-private');
      expect(await page.getByLabel('Client public key', { exact: true }).inputValue()).toBe('new-public');
    } finally { await page.close(); }
  });
  it('reset and unmount invalidate the prior draft even if the same protocol reopens', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1');
      await page.getByRole('button', { name: 'Generate key pair', exact: true }).click();
      await page.evaluate(() => (window as any).__n1.unmount());
      await page.getByText('Unmounted', { exact: true }).waitFor();
      await page.evaluate(() => (window as any).__n1.reset());
      await page.getByRole('button', { name: 'Generate key pair', exact: true }).waitFor();
      await page.evaluate(() => (window as any).__n1.requests[0].resolve({ privateKey: 'stale-private', publicKey: 'stale-public' }));
      expect(await page.locator('#nd-f-privateKey').inputValue()).toBe('');
      expect(await page.getByLabel('Client public key', { exact: true }).inputValue()).toBe('');
    } finally { await page.close(); }
  });

});

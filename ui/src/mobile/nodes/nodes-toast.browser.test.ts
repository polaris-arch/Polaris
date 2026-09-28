import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { api } from '/src/ipc';
import { useAppStore } from '/src/store/app-store';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { useMobileNodeDeletion } from '/src/mobile/nodes/node-deletion';
import { MobileToaster } from '/src/mobile/MobileToaster';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/nodes/nodes.css';
await i18nReady;
const mode = new URLSearchParams(location.search).get('mode');
const node = { id: 'node-a', name: '测试节点', protocol: 'vless', address: '198.51.100.7', port: 443 };
const warp = { id: 'warp-1', name: 'WARP', protocol: 'wireguard', address: 'engage.cloudflareclient.com', port: 2408 };
const test = window.__nodeToastTest = { failCopy: true, writes: [], opened: 0, deleted: 0 };
test.entries = () => useStagedConfigStore.getState().entries;
Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
  writeText: async text => { if (test.failCopy) throw Error('clipboard denied'); test.writes.push(text); },
} });
api.server.generateUrl = async () => 'vless://example';
api.server.delete = async () => { test.deleted++; };
api.config.meshRouteReport = async () => null;
const server = mode?.startsWith('warp') ? warp : node;
useAppStore.setState({ servers: [server], config: { servers: [server], subscriptions: [] },
  selectedServerId: '', invalidNodes: [], proxyStatus: { running: mode?.startsWith('warp') === true } });
useStagedConfigStore.setState({ enabled: mode === 'warp-staged', entries: [] });
function WarpDeleteHarness() {
  const deletion = useMobileNodeDeletion({
    t: key => key,
    clearNotice: () => {},
    runWrite: async op => { await op(); },
    confirm: payload => { queueMicrotask(payload.onConfirm); return 'confirm'; },
    dismiss: () => {},
    exitBatch: () => {},
  });
  return <button onClick={() => deletion.removeWarpNode(warp, {
    title: '重新注册', message: '先删除', okText: '已注销',
    afterDelete: () => { test.opened++; },
  })}>重新注册 WARP</button>;
}
createRoot(document.getElementById('root')).render(<main className="mobile-root">
  {mode?.startsWith('warp') ? <WarpDeleteHarness /> : <MobileNodesScreen />}
  <MobileToaster />
</main>);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile node copy completion receipt', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'nodes-toast-browser-fixture',
      resolveId(id) { if (id === '/nodes-toast-fixture.tsx') return id; },
      load(id) { if (id === '/nodes-toast-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__nodes-toast', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__nodes-toast', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/nodes-toast-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (typeof address !== 'object' || !address) throw Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('copy failure stays by the screen; a later success appears once as Toast and clears the old error', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__nodes-toast`);
      await page.getByRole('button', { name: /测试节点.*更多/ }).click();
      await page.getByRole('dialog').getByRole('button', { name: '复制链接' }).click();
      await page.locator('.mn-notice.err').waitFor();
      expect(await page.locator('.m-toast-ok').count()).toBe(0);

      await page.evaluate(() => { (window as any).__nodeToastTest.failCopy = false; });
      await page.getByRole('dialog').getByRole('button', { name: '复制链接' }).click();
      await page.locator('.m-toast-ok').filter({ hasText: '已复制分享链接' }).waitFor();
      expect(await page.locator('.m-toast-ok').filter({ hasText: '已复制分享链接' }).count()).toBe(1);
      expect(await page.locator('.mn-notice').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__nodeToastTest.writes)).toEqual(['vless://example']);
    } finally { await page.close(); }
  }, 30_000);

  it('staging a persisted WARP deletion leaves the pending entry without reopening registration or reporting completion', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__nodes-toast?mode=warp-staged`);
      await page.getByRole('button', { name: '重新注册 WARP' }).click();
      await page.waitForFunction(() => (window as any).__nodeToastTest.entries?.().length === 1);
      const result = await page.evaluate(() => {
        const test = (window as any).__nodeToastTest;
        return { opened: test.opened, deleted: test.deleted, entries: test.entries() };
      });
      expect(result.opened).toBe(0);
      expect(result.deleted).toBe(0);
      expect(result.entries).toMatchObject([{ id: 'server:warp-1', nextValue: null }]);
      expect(await page.locator('.m-toast-ok').count()).toBe(0);
    } finally { await page.close(); }
  }, 30_000);

  it('direct WARP deletion completes before opening registration and showing one Toast', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__nodes-toast?mode=warp-direct`);
      await page.getByRole('button', { name: '重新注册 WARP' }).click();
      await page.locator('.m-toast-ok').filter({ hasText: '已注销' }).waitFor();
      const result = await page.evaluate(() => {
        const test = (window as any).__nodeToastTest;
        return { opened: test.opened, deleted: test.deleted, entries: test.entries() };
      });
      expect(result).toEqual({ opened: 1, deleted: 1, entries: [] });
      expect(await page.locator('.m-toast-ok').filter({ hasText: '已注销' }).count()).toBe(1);
    } finally { await page.close(); }
  }, 30_000);
});

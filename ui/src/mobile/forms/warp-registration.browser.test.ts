import { readFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// A real Chromium mount exercises React state, native fieldset disabling and the form's API wiring.
// Only synthetic credentials and local API replacements are used; no backend or Cloudflare request.
const registered = JSON.parse(readFileSync(new URL('../../../../crates/mesh/src/warp/tests/registration-draft.json', import.meta.url), 'utf8'));
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { WarpPanel } from '/src/mobile/forms/WarpPanel';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { useAppStore } from '/src/store/app-store';
import { api } from '/src/ipc';
import { systemApi } from '/src/ipc/api-client';
import { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
await i18nReady;
const test = window.__warpTest = { mode: new URLSearchParams(location.search).get('mode'), registers: 0, adds: 0, refreshes: 0, ids: [], nodes: [], release: undefined };
systemApi.listNetworkInterfaces = async () => [];
api.server.registerWarp = async () => { test.registers++; return ${JSON.stringify(registered)}; };
api.server.add = async (server) => {
  test.adds++; test.ids.push(server.id); test.nodes = [server];
  if (test.mode === 'save-pending') await new Promise(resolve => { test.release = resolve; });
  if (test.mode === 'save-reject' && test.adds === 1) throw new Error('synthetic response lost');
};
useAppStore.setState({ servers: [], config: { servers: [], subscriptions: [] }, loadConfig: async () => {
  test.refreshes++;
  if (test.mode === 'refresh-reject' && test.refreshes === 1) throw new Error('synthetic refresh failed');
  if (test.mode === 'readback-missing' && test.refreshes === 1) return;
  useAppStore.setState({ servers: test.nodes });
}});
const instanceId = useMobileFormStore.getState().open({ kind: 'warp' });
test.openForms = () => useMobileFormStore.getState().stack.map(form => form.kind);
createRoot(document.getElementById('root')).render(<main className="mobile-root"><WarpPanel instanceId={instanceId} /></main>);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile WARP retained registration', () => {
beforeAll(async () => {
  server = await createServer({ root, server: { host: '127.0.0.1', port: 0, strictPort: false }, plugins: [{
    name: 'warp-registration-browser-fixture',
    resolveId(id) { if (id === '/warp-registration-fixture.tsx') return id; },
    load(id) { if (id === '/warp-registration-fixture.tsx') return entry; },
    configureServer(vite) { vite.middlewares.use('/__warp', async (_req, res) => {
      const html = await vite.transformIndexHtml('/__warp', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/warp-registration-fixture.tsx"></script></html>');
      res.setHeader('Content-Type', 'text/html'); res.end(html);
    }); },
  }] });
  await server.listen();
  const address = server.httpServer!.address();
  if (typeof address !== 'object' || !address) throw new Error('Vite did not bind');
  origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
}, 30_000);
afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const mode of ['save-reject', 'refresh-reject', 'readback-missing']) {
    it(`${mode}: one registration, stable id, acknowledged saves only refresh`, async () => {
      const page = await browser.newPage({ viewport: { width: 400, height: 869 }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__warp?mode=${mode}`);
        const submit = page.locator('.m-form-foot .primary');
        await submit.waitFor();
        await submit.click();
        await page.locator('.m-form-notice.err').waitFor();
        expect(await submit.textContent()).toBe('重试');
        expect(await page.locator('#mwarp-name').isDisabled()).toBe(true);
        expect(await page.getByRole('button', { name: 'WARP+', exact: true }).isDisabled()).toBe(true);
        expect(await page.getByText('WARP 设备已注册', { exact: true }).count()).toBe(0);
        const before = await page.evaluate(() => {
          const test = (window as unknown as { __warpTest: { registers: number; adds: number; refreshes: number; ids: string[] } }).__warpTest;
          return { registers: test.registers, adds: test.adds, refreshes: test.refreshes, ids: test.ids };
        });
        expect(before).toMatchObject({ registers: 1, adds: 1 });
        const output = path.resolve(root, '../output/playwright');
        mkdirSync(output, { recursive: true });
        await page.screenshot({ path: path.join(output, `warp-${mode}.png`) });
        await submit.click();
        await page.getByText('WARP 设备已注册', { exact: true }).waitFor();
        const after = await page.evaluate(() => {
          const test = (window as unknown as { __warpTest: { registers: number; adds: number; ids: string[] } }).__warpTest;
          return { registers: test.registers, adds: test.adds, ids: test.ids };
        });
        expect(after.registers).toBe(1);
        expect(after.adds).toBe(mode === 'save-reject' ? 2 : 1);
        expect(new Set(after.ids).size).toBe(1);
      } finally { await page.close(); }
    }, 30_000);
  }

  it('locks native and MobileSelect controls while retaining a pending registration', async () => {
    const page = await browser.newPage({ viewport: { width: 400, height: 869 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__warp?mode=save-pending`);
      await page.locator('.m-form-group-h').click();
      const combos = page.getByRole('combobox');
      expect(await combos.count()).toBeGreaterThan(0);
      await page.locator('.m-form-foot .primary').click();
      await page.getByText('注册结果已保留，表单已锁定。', { exact: true }).waitFor();
      for (const combo of await combos.all()) expect(await combo.isDisabled()).toBe(true);
      expect(await page.getByRole('dialog').count()).toBe(1);
      const output = path.resolve(root, '../output/playwright');
      mkdirSync(output, { recursive: true });
      await page.screenshot({ path: path.join(output, 'warp-registered-pending.png') });
      await page.evaluate(() => (window as unknown as { __warpTest: { release: () => void } }).__warpTest.release());
      await page.getByText('WARP 设备已注册', { exact: true }).waitFor();
    } finally { await page.close(); }
  }, 30_000);

  it('warns before discarding an unconfirmed save, but allows closing after a confirmed save', async () => {
    for (const mode of ['save-reject', 'refresh-reject']) {
      const page = await browser.newPage({ viewport: { width: 400, height: 869 }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__warp?mode=${mode}`);
        await page.locator('.m-form-foot .primary').click();
        await page.locator('.m-form-notice.err').waitFor();
        await page.locator('.m-form-foot button').first().click();
        const forms = await page.evaluate(() => (window as unknown as { __warpTest: { openForms: () => string[] } }).__warpTest.openForms());
        expect(forms).toEqual(mode === 'save-reject' ? ['warp', 'confirm'] : []);
      } finally { await page.close(); }
    }
  }, 30_000);
});

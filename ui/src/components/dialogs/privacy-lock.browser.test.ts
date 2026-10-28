import { chromium, webkit, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { beforeAll, afterAll, describe, expect, it } from 'vitest';

// Synthetic in-renderer APIs only. Does not access real privacy settings or a native backend.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Modal } from '/src/components/dialogs/Modal';
import { Csel } from '/src/components/dialogs/Csel';
import LockOverlay from '/src/components/layout/LockOverlay';
import { useAppStore } from '/src/store/app-store';
import { useDialogTopLayerStore } from '/src/components/dialogs/dialog-top-layer';
import { api } from '/src/ipc';
import { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
await i18nReady;
const test = window.__lockTest = {};
api.privacy.hasPassword = async () => true;
api.privacy.unlock = async (password) => ({ ok: password === 'open' });
api.config.setPrivacyMode = async (value) => useAppStore.setState({ privacyMode: value });
useAppStore.setState({ privacyMode: false });
const mode = new URLSearchParams(location.search).get('mode');
function Select() { return <Csel value="a" onChange={() => {}} options={[{ value: 'a', label: 'Alpha' }, { value: 'b', label: 'Beta' }]} />; }
function Fixture() {
  const [ids, setIds] = useState(mode === 'standalone' ? [] : ['original']);
  test.lock = () => useAppStore.setState({ privacyMode: true });
  test.add = () => setIds((ids) => [...ids, 'late']);
  test.remove = (id) => setIds((ids) => ids.filter((value) => value !== id));
  test.layers = () => useDialogTopLayerStore.getState().els.map((el) => el.id || el.getAttribute('aria-labelledby'));
  return <><button id="background">Background</button>{mode === 'standalone' && <Select />}
    {ids.map((id) => <Modal key={id} icon={null} title={id} titleId={id + '-title'} onClose={() => test.remove(id)} footer={null}>
      <input id={id + '-draft'} defaultValue="private draft" /><Select />
    </Modal>)}<LockOverlay /></>;
}
createRoot(document.getElementById('root')).render(<Fixture />);
`;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1').each([
  ['Chromium', chromium], ['WebKit', webkit],
] as const)('privacy native top layer and Csel: %s', (_name, engine) => {
  let server: ViteDevServer;
  let browser: Browser;
  let origin: string;
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'privacy-lock-fixture',
      resolveId(id) { if (id === '/privacy-lock-fixture.tsx') return id; },
      load(id) { if (id === '/privacy-lock-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__privacy', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__privacy', '<html><div id="root"></div><script type="module" src="/privacy-lock-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address === 'string') throw new Error('No test listener');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await engine.launch({ headless: true });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('hides mounted drafts, blocks late dialogs, ignores escape/backdrop and restores draft/focus', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__privacy');
    await page.locator('#original-draft').fill('keep this private');
    await page.evaluate('window.__lockTest.lock()');
    await page.locator('#lock-pw').waitFor();
    expect(await page.locator('#original-draft').isVisible()).toBe(false);
    expect(await page.evaluate('document.activeElement.id')).toBe('lock-pw');
    await page.keyboard.press('Escape');
    await page.mouse.click(4, 4);
    expect(await page.locator('#lock-overlay').isVisible()).toBe(true);
    await page.evaluate('window.__lockTest.add()');
    expect(await page.locator('#late-draft').isVisible()).toBe(false);
    expect(await page.evaluate('window.__lockTest.layers()')).toEqual(['lock-overlay']);
    await page.evaluate('window.__lockTest.remove("late")');
    await page.locator('#lock-pw').fill('open');
    await page.locator('#lock-pw').press('Enter');
    await page.locator('#original-draft').waitFor({ state: 'visible' });
    expect(await page.locator('#original-draft').inputValue()).toBe('keep this private');
    expect(await page.evaluate('document.activeElement.id')).toBe('original-draft');
    await page.evaluate('window.__lockTest.lock(); window.__lockTest.remove("original"); window.__lockTest.add()');
    expect(await page.locator('#late-draft').isVisible()).toBe(false);
    await page.locator('#lock-pw').fill('open');
    await page.locator('#lock-pw').press('Enter');
    await page.locator('#late-draft').waitFor({ state: 'visible' });
    expect(await page.locator('#original-draft').count()).toBe(0);
    await page.close();
  });

  it.each(['standalone', 'modal'])('Escape dismisses only the open Csel in %s context', async (mode) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__privacy?mode=' + mode);
    await page.locator('.csel-trigger').click();
    expect(await page.locator('.csel-menu').isVisible()).toBe(true);
    await page.keyboard.press('Escape');
    expect(await page.locator('.csel-menu').count()).toBe(0);
    if (mode === 'modal') {
      expect(await page.locator('#original-draft').isVisible()).toBe(true);
      await page.keyboard.press('Escape');
      expect(await page.locator('#original-draft').count()).toBe(0);
    } else {
      await page.locator('.csel-trigger').click();
      await page.keyboard.press('Tab');
      expect(await page.locator('.csel-menu').count()).toBe(0);
    }
    await page.close();
  });
});

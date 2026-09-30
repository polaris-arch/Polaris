import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// Real React mounts consume the production hook. The OS enumeration is a deferred local port;
// the cache, refresh effect, shared in-flight request and state updates are not replaced.
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { useNetworkInterfaces } from '/src/hooks/use-network-interfaces';
import { systemApi } from '/src/ipc/api-client';
const roots = new Map();
const test = window.__networkTest = { calls:0, queue:[], models:{}, frames:{}, mount, unmount, reply, fail };
systemApi.listNetworkInterfaces = () => {
  test.calls++;
  return new Promise((resolve,reject)=>test.queue.push({resolve,reject}));
};
function Probe({id}) {
  const model=useNetworkInterfaces();
  test.models[id]=model;
  const frame={items:model.items,loading:model.loading,failed:model.failed};
  (test.frames[id]??=[]).push(frame);
  return <pre data-testid={id}>{JSON.stringify(frame)}</pre>;
}
function mount(id) {
  const el=document.createElement('div');el.id=id;document.body.append(el);
  const root=createRoot(el);roots.set(id,root);root.render(<Probe id={id} />);
}
function unmount(id) { roots.get(id).unmount();roots.delete(id);document.getElementById(id).remove(); }
function reply(items) { test.queue.shift().resolve(items); }
function fail() { test.queue.shift().reject(new Error('synthetic enumeration failure')); }
`;
interface TestPort {
  calls: number;
  frames: Record<string, { items: { name: string }[] }[]>;
  models: Record<string, { refresh: () => Promise<void> }>;
  mount: (id: string) => void;
  unmount: (id: string) => void;
  reply: (items: typeof WIFI) => void;
  fail: () => void;
}
type TestWindow = Window & { __networkTest: TestPort };
const WIFI = [{ name: 'wlan0', displayName: 'Wi-Fi', isUp: true, addresses: [] }];
const CELLULAR = [{ name: 'rmnet0', displayName: 'Cellular', isUp: true, addresses: [] }];
let server: ViteDevServer;
let browser: Browser;
let origin: string;
const calls = (page: Page) => page.evaluate(() => (window as unknown as TestWindow).__networkTest.calls);

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('network interface cache on real mounts', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'network-interface-hook-fixture',
      resolveId(id) { if (id === '/network-interface-fixture.tsx') return id; },
      load(id) { if (id === '/network-interface-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__network', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__network', '<html><div id="root"></div><script type="module" src="/network-interface-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  async function pageForTest(): Promise<Page> {
    const page = await browser.newPage();
    await page.goto(`${origin}/__network`);
    await page.waitForFunction(() => !!(window as unknown as TestWindow).__networkTest);
    return page;
  }

  it('reopening uses the cached first frame, then observes the changed OS interface list', async () => {
    const page = await pageForTest();
    try {
      await page.evaluate(() => (window as unknown as TestWindow).__networkTest.mount('first'));
      await page.waitForFunction(() => (window as unknown as TestWindow).__networkTest.calls === 1);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), WIFI);
      await page.waitForFunction(() => document.querySelector('[data-testid=first]')?.textContent?.includes('wlan0'));
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__networkTest; t.unmount('first'); t.mount('second'); });
      await page.waitForFunction(() => (window as unknown as TestWindow).__networkTest.calls === 2);
      expect(await page.evaluate(() => (window as unknown as TestWindow).__networkTest.frames.second[0].items)).toEqual(WIFI);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), CELLULAR);
      await page.waitForFunction(() => document.querySelector('[data-testid=second]')?.textContent?.includes('rmnet0'));
      expect(await page.getByTestId('second').textContent()).not.toContain('wlan0');
    } finally { await page.close(); }
  }, 30_000);

  it('concurrent mounts and forced refreshes join one pending OS request', async () => {
    const page = await pageForTest();
    try {
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__networkTest; t.mount('a'); t.mount('b'); });
      await page.waitForFunction(() => !!(window as unknown as TestWindow).__networkTest.models.b);
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__networkTest; void t.models.a.refresh(); void t.models.b.refresh(); });
      expect(await calls(page)).toBe(1);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), WIFI);
      await page.waitForFunction(() => ['a','b'].every(id => document.querySelector('[data-testid='+id+']')?.textContent?.includes('wlan0')));
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__networkTest; void t.models.a.refresh(); void t.models.a.refresh(); void t.models.b.refresh(); });
      expect(await calls(page)).toBe(2);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), CELLULAR);
      await page.waitForFunction(() => ['a','b'].every(id => document.querySelector('[data-testid='+id+']')?.textContent?.includes('rmnet0')));
    } finally { await page.close(); }
  }, 30_000);

  it('a failed background enumeration preserves cached items and allows a fresh retry', async () => {
    const page = await pageForTest();
    try {
      await page.evaluate(() => (window as unknown as TestWindow).__networkTest.mount('a'));
      await page.waitForFunction(() => (window as unknown as TestWindow).__networkTest.calls === 1);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), WIFI);
      await page.waitForFunction(() => document.querySelector('[data-testid=a]')?.textContent?.includes('wlan0'));
      await page.evaluate(() => { const t=(window as unknown as TestWindow).__networkTest; t.unmount('a'); t.mount('b'); });
      await page.waitForFunction(() => (window as unknown as TestWindow).__networkTest.calls === 2);
      await page.evaluate(() => (window as unknown as TestWindow).__networkTest.fail());
      await page.waitForFunction(() => JSON.parse(document.querySelector('[data-testid=b]')!.textContent!).failed === true);
      expect(await page.getByTestId('b').textContent()).toContain('wlan0');
      await page.evaluate(() => { void (window as unknown as TestWindow).__networkTest.models.b.refresh(); });
      expect(await calls(page)).toBe(3);
      await page.evaluate(items => (window as unknown as TestWindow).__networkTest.reply(items), CELLULAR);
      await page.waitForFunction(() => document.querySelector('[data-testid=b]')?.textContent?.includes('rmnet0'));
      expect(JSON.parse((await page.getByTestId('b').textContent())!).failed).toBe(false);
    } finally { await page.close(); }
  }, 30_000);
});

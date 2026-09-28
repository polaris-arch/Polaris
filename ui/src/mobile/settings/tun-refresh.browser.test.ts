import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { TunPage } from '/src/mobile/settings/TunPage';
import { api } from '/src/ipc/api-client';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/settings/settings.css';
import '/src/mobile/redesign.css';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const pending=[];
const control=window.__forceRouteTest={count:0,resolve:report=>pending.shift()?.resolve(report),reject:()=>pending.shift()?.reject(Error('read failed')),unmount:()=>{}};
api.config.endpointForceRouteReport=()=>new Promise((resolve,reject)=>{control.count++;pending.push({resolve,reject});});
const config={tunConfig:{stack:'auto',autoRoute:true,strictRoute:true,inboundExcludeCidrs:[]},dnsConfig:{enableFakeIp:true},enableIPv6:false,
  servers:[],bypassLAN:false,bypassLANList:[],configSchemaVersion:1};
function App(){const [mounted,setMounted]=useState(true);control.unmount=()=>setMounted(false);
 return <main className="m-shell"><div className="ms-flow">{mounted&&<TunPage config={config} update={()=>Promise.resolve()} commit={()=>{}}/>}</div></main>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;

let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile TUN report refresh', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-tun-refresh-vite-' + process.pid),
      server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
        name: 'tun-refresh-fixture',
        resolveId(id) { if (id === '/tun-refresh-fixture.tsx') return id; },
        load(id) { if (id === '/tun-refresh-fixture.tsx') return entry; },
        configureServer(vite) { vite.middlewares.use('/__tun-refresh', async (_req, res) => {
          const html = await vite.transformIndexHtml('/__tun-refresh',
            '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/tun-refresh-fixture.tsx"></script></html>');
          res.setHeader('Content-Type', 'text/html'); res.end(html);
        }); },
      }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('loads once, prevents duplicate refreshes, retains the last report on failure, and recovers', async () => {
    const page = await browser.newPage({ viewport: { width: 320, height: 740 } });
    await page.goto(origin + '/__tun-refresh');
    const refresh = page.getByRole('button', { name: '刷新 · 组网网段结算' });
    await refresh.waitFor();
    expect(await refresh.isDisabled()).toBe(true);
    expect(await page.getByRole('status').allInnerTexts()).toContain('正在刷新结算…');
    expect(await page.evaluate(() => (window as any).__forceRouteTest.count)).toBe(1);
    await page.evaluate(() => (window as any).__forceRouteTest.resolve({ servers: [], zeroCoverageServerIds: [], absorbedCount: 0 }));
    await page.waitForFunction(() => !document.querySelector('.ms-force-refresh')?.hasAttribute('disabled'));
    await page.getByText('本次配置结算没有需要独立网段的组网节点。').waitFor();
    await refresh.evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
    expect(await refresh.isDisabled()).toBe(true);
    expect(await page.evaluate(() => (window as any).__forceRouteTest.count)).toBe(2);
    expect(await page.getByText('本次配置结算没有需要独立网段的组网节点。').isVisible()).toBe(true);
    await refresh.evaluate((button: HTMLButtonElement) => button.click());
    expect(await page.evaluate(() => (window as any).__forceRouteTest.count)).toBe(2);
    await page.evaluate(() => (window as any).__forceRouteTest.reject());
    await page.getByRole('alert').getByText('读取失败，暂时拿不到这份报告。').waitFor();
    expect(await refresh.isEnabled()).toBe(true);
    expect(await page.getByText('本次配置结算没有需要独立网段的组网节点。').isVisible()).toBe(true);
    await refresh.click();
    await page.evaluate(() => (window as any).__forceRouteTest.resolve({ servers: [], zeroCoverageServerIds: [], absorbedCount: 0 }));
    await page.waitForFunction(() => !document.querySelector('.ms-force-refresh')?.hasAttribute('disabled'));
    expect(await page.getByRole('alert').count()).toBe(0);
    const paint = await refresh.evaluate(button => ({
      hit: button.getBoundingClientRect().height,
      inset: parseFloat(getComputedStyle(button, '::before').top),
    }));
    expect(paint).toEqual({ hit: 44, inset: 4 });
    await page.screenshot({ path: path.join(tmpdir(), 'polaris-tun-refresh-320.png') });
    await refresh.click();
    await page.evaluate(() => (window as any).__forceRouteTest.unmount());
    await page.locator('.ms-force-refresh').waitFor({ state: 'detached' });
    await page.evaluate(() => (window as any).__forceRouteTest.resolve({ servers: [], zeroCoverageServerIds: [], absorbedCount: 0 }));
    expect(await page.locator('.ms-force-refresh').count()).toBe(0);
    await page.close();
  }, 30_000);
});

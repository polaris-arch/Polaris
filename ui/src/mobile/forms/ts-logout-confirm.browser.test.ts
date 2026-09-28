import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { api, systemApi } from '/src/ipc/api-client';
import { useAppStore } from '/src/store/app-store';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { MobileFormHost } from '/src/mobile/forms/MobileFormHost';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
await i18nReady;
 i18n.addResourceBundle('zh-CN','translation',zhCN,true,true);
await i18n.changeLanguage('zh-CN');
const node={id:'ts-1',name:'Synthetic TS',protocol:'tailscale',address:'',port:0,tailscaleSettings:{hostname:'synthetic'}};
const control=window.__tsLogoutTest={
  confirm:async()=>{const entry=useMobileFormStore.getState().stack.at(-1);if(entry?.kind!=='confirm')throw Error('no confirm');await entry.payload.onConfirm();},
  injectDelegateThrow:()=>{const original=useAppStore.getState;useAppStore.getState=()=>{useAppStore.getState=original;throw Error('delegate failed');};},
  stack:()=>useMobileFormStore.getState().stack.map(entry=>entry.kind),
};
systemApi.listNetworkInterfaces=async()=>[];
api.server.tailscaleGetStatus=async()=>({connected:false,statuses:[]});
api.server.tailscaleLogout=async()=>{throw {code:'TAILSCALE_LOGOUT_MAIN_CORE'};};
api.proxy.getStatus=async()=>({running:true,starting:false,pid:321,startTime:1});
useAppStore.setState({servers:[node],config:{servers:[node],subscriptions:[]},selectedServerId:'ts-1',loadConfig:async()=>{}});
useMobileFormStore.getState().open({kind:'ts-settings',serverId:'ts-1'});
createRoot(document.getElementById('root')).render(<main className="mobile-root"><MobileFormHost/></main>);
`;
let server: ViteDevServer;
let browser: Browser;
let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('TS logout second confirmation', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-ts-logout-vite-' + process.pid),
      server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
        name: 'ts-logout-fixture',
        resolveId(id) { if (id === '/ts-logout-fixture.tsx') return id; },
        load(id) { if (id === '/ts-logout-fixture.tsx') return entry; },
        configureServer(vite) { vite.middlewares.use('/__ts-logout', async (_req, res) => {
          const html = await vite.transformIndexHtml('/__ts-logout',
            '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/ts-logout-fixture.tsx"></script></html>');
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

  it('keeps the settings form and shows an error when a delegate unexpectedly throws', async () => {
    const page = await browser.newPage({ viewport: { width: 320, height: 740 } });
    page.setDefaultTimeout(6_000);
    const pageErrors: string[] = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));
    try {
      await page.goto(origin + '/__ts-logout');
      await page.getByRole('dialog', { name: 'Tailscale 设置' }).getByRole('button', { name: '退出登录' }).click();
      expect(await page.evaluate(() => (window as any).__tsLogoutTest.stack())).toEqual(['ts-settings', 'confirm']);
      await page.evaluate(() => (window as any).__tsLogoutTest.confirm());
      expect(await page.evaluate(() => (window as any).__tsLogoutTest.stack())).toEqual(['ts-settings', 'confirm']);
      await page.getByRole('dialog', { name: '断开连接并退出登录？' }).waitFor();
      await page.evaluate(() => (window as any).__tsLogoutTest.injectDelegateThrow());
      await page.evaluate(() => (window as any).__tsLogoutTest.confirm());
      expect(await page.evaluate(() => (window as any).__tsLogoutTest.stack())).toEqual(['ts-settings']);
      await page.getByRole('dialog', { name: 'Tailscale 设置' }).getByText('登出 Tailscale 失败').waitFor();
      expect(await page.getByRole('dialog', { name: 'Tailscale 设置' }).count()).toBe(1);
      expect(pageErrors).toEqual([]);
    } finally { await page.close(); }
  }, 45_000);
});

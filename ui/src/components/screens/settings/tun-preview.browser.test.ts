import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import SettingsTun from '/src/components/screens/settings/SettingsTun';
import { useAppStore, useEffectiveConfig } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
await i18nReady; await i18n.changeLanguage('en-US');
const config={servers:[],trafficRules:[],appRules:[],bypassLANList:[],tunConfig:{inboundExcludeCidrs:[],macFilterList:[],mtu:1500},dnsConfig:{},neighborDomains:[]};
const test=window.__n1={calls:[],pending:[],conflictCalls:0,conflictPending:[]};
api.config.tunExclusionPreview=()=>{test.calls.push(structuredClone(useAppStore.getState().config));return new Promise(resolve=>test.pending.push(resolve));};
api.config.meshRouteReport=async()=>null;api.config.tunnelConflictReport=()=>{test.conflictCalls++;return new Promise((resolve,reject)=>test.conflictPending.push({resolve,reject}));};api.config.coexSnapshot=async()=>null;api.config.setStagedPending=async()=>{};
useAppStore.setState({config,servers:[],proxyStatus:{running:false},tailscaleStatuses:{}});
useStagedConfigStore.setState({enabled:true,entries:[],baseline:config});
test.peer=()=>useAppStore.setState({tailscaleStatuses:{mesh:{tailscaleIPs:['100.64.0.1'],details:{userGroups:[{peers:[{details:{tailscaleIPs:['100.64.0.2']}}]}]},peers:[]}}});
test.stage=()=>useStagedConfigStore.getState().stage({id:'selected',kind:'setting',label:'selected',entityPath:['selectedServerId'],nextValue:'mesh'});
test.save=()=>{useAppStore.setState({config:{...config,selectedServerId:'mesh'}});useStagedConfigStore.setState({entries:[]});};
test.fake=()=>useAppStore.setState({config:{...useAppStore.getState().config,dnsDefaults:{strategy:'fakeip'},enableFakeIP:true}});
function App(){const config=useEffectiveConfig();return <SettingsTun config={config} update={async()=>{}}/>;}
let root=createRoot(document.getElementById('root'));root.render(<App/>);
test.unmount=()=>root.unmount();
test.mount=()=>{root=createRoot(document.getElementById('root'));root.render(<App/>);};
test.restart=(startTime)=>useAppStore.setState({proxyStatus:{running:true,startTime}});
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


  it('peer-only observations and saved inputs invalidate, staged-only edits do not hide the eventual commit',async()=>{
    const page=await browser.newPage();
    try{
      await page.goto(origin+'/__n1');
      await page.waitForFunction(()=>(window as any).__n1.calls.length===1);
      await page.evaluate(()=>(window as any).__n1.peer());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===2);
      await page.evaluate(()=>(window as any).__n1.stage());
      await page.waitForTimeout(0);
      expect(await page.evaluate(()=>(window as any).__n1.calls.length)).toBe(2);
      await page.evaluate(()=>(window as any).__n1.save());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===3);
      expect(await page.evaluate(()=>(window as any).__n1.calls[2].selectedServerId)).toBe('mesh');
      await page.evaluate(()=>(window as any).__n1.fake());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===4);
      // Resolve older and newer requests in reverse; cancellation belongs to the real component effect.
      await page.evaluate(()=>{const t=(window as any).__n1;t.pending[3](null);t.pending[0](null);});
    }finally{await page.close();}
  });

  it('the foreign-tunnel snapshot can be reread after background completion, failure, a new running generation, and remount', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1');
      const block = page.getByTestId('tunnel-conflict-report');
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 1);
      expect(await block.getByRole('button').isDisabled()).toBe(true);
      await page.evaluate(() => (window as any).__n1.conflictPending[0].resolve({ status: 'notProbed' }));
      await page.waitForFunction(() => !document.querySelector('[data-testid="tunnel-conflict-report"] button')?.hasAttribute('disabled'));
      await block.getByRole('button').click();
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 2);
      await page.evaluate(() => (window as any).__n1.conflictPending[1].reject(new Error('read failed')));
      await page.waitForFunction(() => !document.querySelector('[data-testid="tunnel-conflict-report"] button')?.hasAttribute('disabled'));
      await block.getByRole('button').click();
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 3);
      await page.evaluate(() => (window as any).__n1.conflictPending[2].resolve({ status: 'probeFailed', error: 'current snapshot' }));
      await page.waitForFunction(() => document.querySelector('[data-testid="tunnel-conflict-report"]')?.textContent?.includes('current snapshot'));
      // running remains true on the second generation, so startTime must invalidate.
      await page.evaluate(() => (window as any).__n1.restart(1));
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 4);
      await page.evaluate(() => (window as any).__n1.restart(2));
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 5);
      await page.evaluate(() => { const t = (window as any).__n1;
        t.conflictPending[4].resolve({ status: 'probeFailed', error: 'new generation' });
        t.conflictPending[3].resolve({ status: 'probeFailed', error: 'stale generation' });
      });
      await page.waitForFunction(() => document.querySelector('[data-testid="tunnel-conflict-report"]')?.textContent?.includes('new generation'));
      expect(await block.textContent()).not.toContain('stale generation');
      await block.getByRole('button').click();
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 6);
      await page.evaluate(() => { const t = (window as any).__n1;t.unmount();t.conflictPending[5].resolve({ status: 'probeFailed', error: 'after unmount' });t.mount(); });
      await page.waitForFunction(() => (window as any).__n1.conflictCalls === 7);
      await page.evaluate(() => (window as any).__n1.conflictPending[6].resolve({ status: 'notProbed' }));
      await page.waitForFunction(() => !document.querySelector('[data-testid="tunnel-conflict-report"] button')?.hasAttribute('disabled'));
      expect(await block.textContent()).not.toContain('after unmount');
    } finally { await page.close(); }
  });
});

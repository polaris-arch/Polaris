import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../../../..');
const entry = `
import React, {useLayoutEffect} from 'react';
import {flushSync} from 'react-dom';
import { createRoot } from 'react-dom/client';
import SettingsTun from '/src/components/screens/settings/SettingsTun';
import { useAppStore, useEffectiveConfig } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
await i18nReady; await i18n.changeLanguage('en-US');
const config={servers:[],trafficRules:[],appRules:[],bypassLANList:[],tunConfig:{inboundExcludeCidrs:[],macFilterList:[],mtu:1500},dnsConfig:{},neighborDomains:[]};
const test=window.__n1={ready:false,commits:0,calls:[],pending:[],conflictCalls:0,conflictPending:[]};
api.config.tunExclusionPreview=()=>{test.calls.push(structuredClone(useAppStore.getState().config));return new Promise(resolve=>test.pending.push(resolve));};
api.config.meshRouteReport=async()=>null;api.config.tunnelConflictReport=()=>{test.conflictCalls++;return new Promise((resolve,reject)=>test.conflictPending.push({resolve,reject}));};api.config.coexSnapshot=async()=>null;api.config.setStagedPending=async()=>{};
useAppStore.setState({config,servers:[],proxyStatus:{running:false},tailscaleStatuses:{}});
useStagedConfigStore.setState({enabled:true,entries:[],baseline:config});
test.peer=()=>useAppStore.setState({tailscaleStatuses:{mesh:{tailscaleIPs:['100.64.0.1'],details:{userGroups:[{peers:[{details:{tailscaleIPs:['100.64.0.2']}}]}]},peers:[]}}});
test.stage=()=>flushSync(()=>useStagedConfigStore.getState().stage({id:'selected',kind:'setting',label:'selected',entityPath:['selectedServerId'],nextValue:'mesh'}));
test.save=()=>{useAppStore.setState({config:{...config,selectedServerId:'mesh'}});useStagedConfigStore.setState({entries:[]});};
test.fake=()=>useAppStore.setState({config:{...useAppStore.getState().config,dnsDefaults:{strategy:'fakeip'},enableFakeIP:true}});
function App(){const config=useEffectiveConfig();useLayoutEffect(()=>{test.commits++;test.ready=true;return()=>{test.ready=false;};});return <SettingsTun config={config} update={async()=>{}}/>;}
let root=createRoot(document.getElementById('root'));root.render(<App/>);
test.unmount=()=>root.unmount();
test.mount=()=>{root=createRoot(document.getElementById('root'));root.render(<App/>);};
test.restart=(startTime)=>useAppStore.setState({proxyStatus:{running:true,startTime}});
`;
let server: ViteDevServer; let browser: Browser; let origin: string;

async function openFixture(page: Page, pageErrors: string[]) {
  const moduleFailure = new Promise<never>((_resolve, reject) => {
    page.on('pageerror', (error) => { pageErrors.push(error.message); reject(error); });
  });
  // Navigation does not wait for the async module/i18n bootstrap or React commit.
  // A module error must fail with its real cause instead of an undefined global.
  await Promise.race([
    (async () => {
      await page.goto(origin + '/__n1');
      await page.waitForFunction(() => (window as any).__n1?.ready === true);
    })(),
    moduleFailure,
  ]);
}

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
    const pageErrors: string[] = [];
    try{
      await openFixture(page, pageErrors);
      await page.waitForFunction(()=>(window as any).__n1.calls.length===1);
      await page.evaluate(()=>(window as any).__n1.peer());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===2);
      const commitsBefore = await page.evaluate<number>(() => (window as any).__n1.commits);
      await page.evaluate(()=>(window as any).__n1.stage());
      await page.waitForFunction((before) => (window as any).__n1.commits > before, commitsBefore);
      expect(await page.evaluate(()=>(window as any).__n1.calls.length)).toBe(2);
      await page.evaluate(()=>(window as any).__n1.save());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===3);
      expect(await page.evaluate(()=>(window as any).__n1.calls[2].selectedServerId)).toBe('mesh');
      await page.evaluate(()=>(window as any).__n1.fake());
      await page.waitForFunction(()=>(window as any).__n1.calls.length===4);
      // Resolve older and newer requests in reverse; cancellation belongs to the real component effect.
      await page.evaluate(()=>{const t=(window as any).__n1;
        t.pending[3]({tunActive:true,effective:['198.51.100.0/24'],notes:[]});
        t.pending[0]({tunActive:true,effective:['203.0.113.0/24'],notes:[]});
      });
      await page.getByText('198.51.100.0/24', {exact: true}).waitFor();
      expect(await page.locator('.cidr-eff-list').textContent()).not.toContain('203.0.113.0/24');
    }finally{await page.close();expect(pageErrors).toEqual([]);}
  });

  it('the foreign-tunnel snapshot can be reread after background completion, failure, a new running generation, and remount', async () => {
    const page = await browser.newPage();
    const pageErrors: string[] = [];
    try {
      await openFixture(page, pageErrors);
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
    } finally { await page.close(); expect(pageErrors).toEqual([]); }
  });
});

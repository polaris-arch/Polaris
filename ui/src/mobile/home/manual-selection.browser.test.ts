import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// IPC doubles prove wiring only: mount both production screens and keep the real
// app-store switchServer/write queue/readback, rather than substituting a handler.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { useAppStore, getEffectiveConfig } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import { configApi } from '/src/ipc/api/config';
import { windowApi } from '/src/ipc/api/system';
import { setToastImpl } from '/src/lib/error-handler';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/redesign.css';
await i18nReady;
i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const params=new URLSearchParams(location.search);
const selected=params.get('selected')||'node-a';
const intent=params.get('intent')||'auto';
const servers=['a','b'].map(id=>({id:'node-'+id,name:'Fixture '+id,protocol:'socks',address:'fixture.invalid',port:1}));
let disk={selectedServerId:selected,proxyMode:'smart',servers,subscriptions:[]};
if(intent!=='manual') disk.selectionIntent=intent==='null'?null:intent==='false'?false:intent==='empty'?'':intent==='unknown'?{mode:'future',scope:'unknown'}:{mode:'auto',scope:'subscription',subscriptionId:'sub-a'};
let commandGate,readGate;
const fixture=window.__manual={trace:[],messages:[],armed:false,failCommand:false,failRead:false,holdCommand:false,holdRead:false,
 state:()=>({config:useAppStore.getState().config,selected:useAppStore.getState().selectedServerId}),
 effective:()=>getEffectiveConfig(),disk:()=>disk,releaseCommand:()=>commandGate?.(),releaseRead:()=>readGate?.(),
 missingConfig(){useAppStore.setState({config:null});},
 publish(config){disk=config;useAppStore.setState({config,servers:config.servers,selectedServerId:config.selectedServerId});},
 draft(value){useStagedConfigStore.setState({enabled:true,hydrated:true,baseline:disk,entries:[{id:'intent-draft',kind:'setting',entityPath:['selectionIntent'],nextValue:value,label:'intent fixture'}]});},
 begin(){fixture.trace=[];fixture.armed=true;}};
api.server.switch=async id=>{
 fixture.trace.push(['switch',id]);
 if(fixture.holdCommand){fixture.holdCommand=false;await new Promise(resolve=>commandGate=resolve);}
 if(fixture.failCommand) throw Error('command rejected');
 disk={...disk,selectedServerId:id,readbackMarker:fixture.trace.length};delete disk.selectionIntent;
 return {status:'notRunning'};
};
configApi.get=async()=>{
 if(fixture.armed){
  fixture.trace.push(['get']);
  if(fixture.holdRead){fixture.holdRead=false;await new Promise(resolve=>readGate=resolve);}
  if(fixture.failRead) throw Error('readback failed');
 }
 return structuredClone(disk);
};
configApi.onChanged=()=>()=>{};
windowApi.startupConfigFlags=async()=>({});
api.config.meshRouteReport=async()=>null;
api.stats.onStatsUpdated=()=>()=>{};api.stats.onConnectionsDetail=()=>()=>{};
api.stats.subscribe=async()=>{};api.stats.unsubscribe=async()=>{};
setToastImpl({success:text=>fixture.messages.push(['success',text]),info:text=>fixture.messages.push(['info',text]),warning:text=>fixture.messages.push(['warning',text])});
useStagedConfigStore.setState({enabled:false,hydrated:true,entries:[]});
useAppStore.setState({config:structuredClone(disk),servers,selectedServerId:selected,proxyStatus:{running:false},invalidNodes:[]});
createRoot(document.getElementById('root')).render(<main className="mobile-root">{params.get('screen')==='nodes'?<MobileNodesScreen/>:<MobileHomeScreen/>}</main>);
`;

type Route = 'home-node' | 'home-direct' | 'home-block' | 'nodes-node';
const routes: Route[] = ['home-node', 'home-direct', 'home-block', 'nodes-node'];
const selectedFor = (route: Route) => route === 'home-direct' ? '__direct__' : route === 'home-block' ? '__block__' : 'node-a';
let server: ViteDevServer;
let browser: Browser;
let origin: string;

async function open(route: Route, intent = 'auto', selected = selectedFor(route)): Promise<Page> {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  page.setDefaultTimeout(7_000);
  await page.goto(`${origin}/__manual?screen=${route.startsWith('nodes') ? 'nodes' : 'home'}&intent=${intent}&selected=${selected}`);
  await page.locator(route.startsWith('nodes') ? '[data-exit-write="node-use"]' : '[data-write-control="switch-node"]').first().waitFor();
  // Let the existing home settings hook finish its mount read before tracing writes.
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.evaluate(() => (window as any).__manual.begin());
  if (route.startsWith('home')) {
    await page.locator('[data-write-control="switch-node"]').first().click();
    await page.locator('.h-picker-sheet').waitFor();
  }
  return page;
}

function target(page: Page, route: Route, node = 'a') {
  if (route === 'home-direct' || route === 'home-block') return page.locator(`[data-exit-write="home-sentinel-${route.slice(5)}"]`);
  if (route === 'nodes-node') return page.locator('[data-exit-write="node-use"]').filter({ hasText: `Fixture ${node}` });
  return page.locator(`[data-picker-node="node-${node}"] button`);
}

const snapshot = (page: Page) => page.evaluate(() => {
  const fixture = (window as any).__manual;
  return { ...fixture.state(), trace: fixture.trace, disk: fixture.disk(), messages: fixture.messages };
});
const settled = (page: Page, route: Route) => route.startsWith('home')
  ? page.locator('.h-picker-sheet').waitFor({ state: 'detached' })
  : page.waitForFunction(() => !(window as any).__manual.state().config.selectionIntent);

// Opt-in matches the repository browser suites; one Chromium process, sequential cases.
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile explicit manual selection leaves Auto', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-manual-selection-'+process.pid), server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
      name: 'manual-selection-fixture',
      resolveId(id) { if (id === '/manual-selection-fixture.tsx') return id; },
      load(id) { if (id === '/manual-selection-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__manual', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__manual', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/manual-selection-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen(); const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw Error('Vite did not bind');
    origin = 'http://127.0.0.1:'+address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const route of routes) {
    it(`${route}: Auto + current exit switches once and adopts the complete readback`, async () => {
      const page = await open(route);
      try {
        const button = target(page, route);
        expect(await button.isEnabled()).toBe(true);
        expect(await button.getAttribute('aria-current')).toBe('true');
        await button.click();
        await expect.poll(async () => (await snapshot(page)).trace).toEqual([['switch', selectedFor(route)], ['get']]);
        await settled(page, route);
        const result = await snapshot(page);
        expect(result.trace).toEqual([['switch', selectedFor(route)], ['get']]);
        expect(result.config).toEqual(result.disk);
        expect(result.config.selectionIntent).toBeUndefined();
        expect(result.config.readbackMarker).toBe(1);
      } finally { await page.close(); }
    });

    it(`${route}: manual + current exit remains a no-op`, async () => {
      const page = await open(route, 'manual');
      try {
        const before = await snapshot(page);
        await target(page, route).click();
        if (route.startsWith('home')) await settled(page, route);
        // The handler is synchronous for no-op; cross a render frame before observing.
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
        const after = await snapshot(page);
        expect(after.trace).toEqual([]); expect(after.config).toEqual(before.config);
      } finally { await page.close(); }
    });

    it(`${route}: a different exit retains the existing switch/readback path`, async () => {
      const page = await open(route, 'manual', 'node-b');
      try {
        await target(page, route).click();
        await page.waitForFunction(id => (window as any).__manual.state().selected === id, selectedFor(route));
        const result = await snapshot(page);
        expect(result.trace).toEqual([['switch', selectedFor(route)], ['get']]);
        expect(result.config).toEqual(result.disk);
      } finally { await page.close(); }
    });

    for (const failure of ['command', 'readback']) {
      it(`${route}: ${failure} failure preserves intent and shows the existing error`, async () => {
        const page = await open(route);
        try {
          const before = await snapshot(page);
          await page.evaluate(failure => { const f = (window as any).__manual; f.failCommand = failure === 'command'; f.failRead = failure === 'readback'; }, failure);
          await target(page, route).click();
          await page.locator(route.startsWith('home') ? '[data-write-error="switch-node"]' : '.mn-notice.err').first().waitFor();
          if (route.startsWith('home')) await settled(page, route);
          const result = await snapshot(page);
          expect(result.trace).toEqual(failure === 'command' ? [['switch', selectedFor(route)]] : [['switch', selectedFor(route)], ['get']]);
          expect(result.config).toEqual(before.config); expect(result.selected).toBe(before.selected);
          expect(result.messages).toEqual([]);
          // A command may have committed even when readback fails; UI must not infer success.
          expect(result.disk.selectionIntent === undefined).toBe(failure === 'readback');
          await page.evaluate(() => { const f = (window as any).__manual; f.failCommand = false; f.failRead = false; });
          if (route.startsWith('home')) await page.locator('[data-write-control="switch-node"]').first().click();
          await target(page, route).click(); await settled(page, route);
          const retried = await snapshot(page);
          expect(retried.config).toEqual(retried.disk);
          expect(retried.config.selectionIntent).toBeUndefined();
          expect(retried.trace.slice(-2)).toEqual([['switch', selectedFor(route)], ['get']]);
          expect(await page.locator(route.startsWith('home') ? '[data-write-error="switch-node"]' : '.mn-notice.err').count()).toBe(0);
        } finally { await page.close(); }
      });
    }

    it(`${route}: unknown intent is delegated to the backend`, async () => {
      const page = await open(route, 'unknown');
      try {
        await target(page, route).click(); await settled(page, route);
        expect((await snapshot(page)).trace).toEqual([['switch', selectedFor(route)], ['get']]);
      } finally { await page.close(); }
    });

    for (const intent of ['null', 'false', 'empty']) {
      it(`${route}: present malformed ${intent} intent is not treated as absent`, async () => {
        const page = await open(route, intent);
        try {
          await target(page, route).click(); await settled(page, route);
          expect((await snapshot(page)).trace).toEqual([['switch', selectedFor(route)], ['get']]);
        } finally { await page.close(); }
      });
    }

    it(`${route}: rapid current-exit clicks remain queued through command and readback`, async () => {
      const page = await open(route);
      try {
        const before = await snapshot(page);
        await page.evaluate(() => { const f = (window as any).__manual; f.holdCommand = true; f.holdRead = true; });
        await target(page, route).dblclick({ delay: 10 });
        await page.waitForFunction(() => (window as any).__manual.trace.length === 1);
        expect((await snapshot(page)).config).toEqual(before.config);
        await page.evaluate(() => (window as any).__manual.releaseCommand());
        await page.waitForFunction(() => (window as any).__manual.trace.length === 2);
        const duringRead = await snapshot(page);
        expect(duringRead.trace).toEqual([['switch', selectedFor(route)], ['get']]);
        expect(duringRead.config).toEqual(before.config); expect(duringRead.disk.selectionIntent).toBeUndefined();
        await page.evaluate(() => (window as any).__manual.releaseRead());
        await page.waitForFunction(() => (window as any).__manual.trace.length === 4);
        await settled(page, route);
        const result = await snapshot(page);
        expect(result.trace).toEqual([['switch', selectedFor(route)], ['get'], ['switch', selectedFor(route)], ['get']]);
        expect(result.config).toEqual(result.disk); expect(result.messages).toEqual([]);
      } finally { await page.close(); }
    });
  }

  for (const route of ['home-node', 'nodes-node'] as Route[]) {
    it(`${route}: click observes new authoritative intent without a selected-id change`, async () => {
      const page = await open(route, 'manual');
      try {
        await page.evaluate(() => { const f = (window as any).__manual; f.publish({...f.disk(),selectionIntent:{mode:'auto',scope:'subscription',subscriptionId:'sub-b'}}); });
        await target(page, route).click(); await settled(page, route);
        expect((await snapshot(page)).trace).toEqual([['switch', 'node-a'], ['get']]);
      } finally { await page.close(); }
    });

    it(`${route}: a draft that removes Auto cannot hide the disk intent`, async () => {
      const page = await open(route);
      try {
        await page.evaluate(() => (window as any).__manual.draft(undefined));
        expect(await page.evaluate(() => (window as any).__manual.effective().selectionIntent)).toBeUndefined();
        await target(page, route).click(); await settled(page, route);
        expect((await snapshot(page)).trace).toEqual([['switch', 'node-a'], ['get']]);
      } finally { await page.close(); }
    });

    it(`${route}: a draft Auto cannot turn a disk manual no-op into a write`, async () => {
      const page = await open(route, 'manual');
      try {
        await page.evaluate(() => (window as any).__manual.draft({mode:'auto',scope:'subscription',subscriptionId:'draft'}));
        expect(await page.evaluate(() => (window as any).__manual.effective().selectionIntent.subscriptionId)).toBe('draft');
        await target(page, route).click();
        if (route.startsWith('home')) await settled(page, route);
        expect((await snapshot(page)).trace).toEqual([]);
      } finally { await page.close(); }
    });

    it(`${route}: the latest disk manual state may no-op even after Auto was rendered`, async () => {
      const page = await open(route);
      try {
        await page.evaluate(() => { const f = (window as any).__manual; const config = {...f.disk()}; delete config.selectionIntent; f.publish(config); });
        await target(page, route).click();
        if (route.startsWith('home')) await settled(page, route);
        expect((await snapshot(page)).trace).toEqual([]);
      } finally { await page.close(); }
    });

    it(`${route}: missing authoritative config cannot establish a manual no-op`, async () => {
      const page = await open(route, 'manual');
      try {
        // Click in the same turn before React commits the deliberately missing config.
        await target(page, route).evaluate(button => {
          (window as any).__manual.missingConfig();
          (button as HTMLButtonElement).click();
        });
        await expect.poll(async () => (await snapshot(page)).trace).toEqual([['switch', 'node-a'], ['get']]);
      } finally { await page.close(); }
    });

    it(`${route}: different rapid targets preserve click order and final readback`, async () => {
      const page = await open(route);
      try {
        await page.evaluate(() => (window as any).__manual.holdCommand = true);
        await target(page, route, 'b').click();
        await target(page, route, 'a').click();
        expect((await snapshot(page)).trace).toEqual([['switch', 'node-b']]);
        expect((await snapshot(page)).config.selectionIntent.mode).toBe('auto');
        await page.evaluate(() => (window as any).__manual.releaseCommand());
        await expect.poll(async () => (await snapshot(page)).trace).toEqual([['switch', 'node-b'], ['get'], ['switch', 'node-a'], ['get']]);
        await settled(page, route);
        const result = await snapshot(page);
        expect(result.config).toEqual(result.disk); expect(result.selected).toBe('node-a');
      } finally { await page.close(); }
    });
  }
});

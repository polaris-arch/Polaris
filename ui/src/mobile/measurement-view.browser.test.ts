import path from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React, { useState, useLayoutEffect } from 'react';
import { createRoot } from 'react-dom/client';
import NodesScreen from '/src/components/screens/nodes/NodesScreen';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import { MeasurementStartNotice } from '/src/mobile/MeasurementStartNotice';
import { MeasurementLabel } from '/src/mobile/MeasurementLabel';
import { Csel } from '/src/components/dialogs/Csel';
import { measurementT, refreshTrayLang } from '/src/tray/labels';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { useLatencyStore, subscribeLatencyEvents } from '/src/store/use-latency-store';
import { measurementFixture } from '/src/store/measurement-fixture.test-support';
import { api, unlockApi } from '/src/ipc';
import { configContentDigest } from '/src/lib/staged-config';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const mode=new URLSearchParams(location.search).get('mode')||'desktop';
localStorage.setItem('polaris.language','zh-CN');refreshTrayLang();
document.documentElement.dataset.mobileOs=mode==='ios'?'ios':'android';
if(mode==='desktop'||mode==='csel')await import('/src/styles/index.css');
else {await import('/src/styles/tokens.resolved.css');await import('/src/mobile/mobile.css');await import('/src/mobile/redesign.css');}
const servers=['a','b','c'].map((id,index)=>({id,name:index<2?'同名节点':'自建节点',protocol:'socks',address:'fixture.invalid',port:1,...(index<2?{subscriptionId:index===0?'one':'two'}:{})}));
const config={selectedServerId:'a',proxyMode:'smart',servers,subscriptions:[{id:'one',name:'Alpha',url:'https://fixture.invalid'},{id:'two',name:'Beta',url:'https://fixture.invalid'},{id:'empty',name:'Empty',url:'https://fixture.invalid'}]};
let status={running:mode!=='ios',actualServerId:mode==='ios'?null:'a',mainGeneration:7,startTime:10};
const f=window.__measurement={mounted:false,calls:0,networkCalls:0, reads:0, kind:'success',
 state(){return {selected:useAppStore.getState().selectedServerId,latencies:useLatencyStore.getState().latencyMap};},
 publish(kind){f.kind=kind;const snap=measurementFixture(config,{a:40,b:null},String(++f.reads),Date.now());
  if(kind==='expired')snap.rows.a.candidate.state='expired';
  if(kind==='paused')snap.schedule={state:'paused',reason:'metered',stalled:false,subscriptions:{one:{periodMinutes:30}}};
  if(kind==='notIncluded')snap.rows.a.notIncluded='notInPool';
  if(kind==='disconnected'){snap.rows.a.candidate.disconnected=true;snap.rows.a.candidate.identity.instance={kind:'temp'};snap.rows.a.candidate.identity.context='disconnected';}
  useLatencyStore.getState().applySnapshot(snap,config.servers);},
 actual(serverId){status={...status,actualServerId:serverId};useAppStore.setState({proxyStatus:status});},
 editSelected(){useStagedConfigStore.setState({entries:[{id:'server:a',kind:'server',label:'edit',entityPath:['servers','a'],nextValue:{...servers[0],address:'new.invalid'}}]});},
 language:null,
 unmount(){app.unmount();dispose();},
};
let id=0;window.__TAURI_INTERNALS__={transformCallback(){return ++id;},invoke(cmd){
 if(cmd==='plugin:event|listen')return Promise.resolve(++id);
 if(cmd==='plugin:event|unlisten')return Promise.resolve();
 return Promise.resolve({success:true});
}};
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
api.config.get=async()=>config;api.config.meshRouteReport=async()=>null;
api.proxy.getStatus=async()=>status;
api.server.speedTestResultSnapshot=async()=>{const snap=measurementFixture(config,{a:40,b:null},String(++f.reads),Date.now());snap.generation=status.running?7:null;snap.frontendConfigDigest=await configContentDigest(config);
 if(!status.running||f.kind==='expired')snap.rows.a.candidate.state='expired';
 if(!status.running)snap.rows.b.candidate.state='expired';
 if(f.kind==='paused')snap.schedule={state:'paused',reason:'metered',stalled:false,subscriptions:{one:{periodMinutes:30}}};
 if(f.kind==='notIncluded')snap.rows.a.notIncluded='notInPool';
 if(f.kind==='disconnected'){snap.rows.a.candidate.disconnected=true;snap.rows.a.candidate.identity.instance={kind:'temp'};snap.rows.a.candidate.identity.context='disconnected';}
 return snap;};
api.server.speedTest=async()=>{f.calls++;status={...status,running:true,actualServerId:'a'};useAppStore.setState({proxyStatus:status});return {runId:'1',results:{a:40},outcome:'completed',notInPool:[],tsNotReady:[]};};
unlockApi.manualCheck=async()=>{f.networkCalls++;throw Error('fixture stops after the authorized invocation');};
useStagedConfigStore.setState({enabled:false,hydrated:true,entries:[]});
useAppStore.setState({config,servers,selectedServerId:'a',proxyStatus:status,invalidNodes:[]});
const dispose=subscribeLatencyEvents();
function TrayLanguageFixture(){const [lang,setLang]=useState('zh-CN');f.language=()=>{localStorage.setItem('polaris.language','en-US');setLang(refreshTrayLang());};return <div data-tray-language={lang}><MeasurementLabel serverId="a" displayedServer={servers[0]} translate={measurementT}/></div>;}
function FixtureMount({children}){useLayoutEffect(()=>{f.mounted=true;document.getElementById('root').dataset.fixtureMode=mode;return()=>{f.mounted=false;};},[]);return children;}
function ShallowSelectFixture(){const [value,setValue]=useState('remote');return <section className="card" style={{position:'absolute',left:16,right:16,bottom:24,padding:16,height:120}}><Csel value={value} onChange={setValue} ariaLabel="DNS fallback" options={Array.from({length:12},(_,i)=>({value:i===0?'remote':String(i),label:'Resolver '+i,description:'Long resolver description with DNS routing and fallback details '+i}))}/></section>;}
let app=createRoot(document.getElementById('root'));
app.render(<FixtureMount>{mode==='csel'?<ShallowSelectFixture/>:mode==='tray-language'?<TrayLanguageFixture/>:mode==='home'?<MobileHomeScreen/>:mode==='ios'?<><MobileHomeScreen/><MeasurementStartNotice/></>:mode==='mobile'?<MobileNodesScreen/>:<div className="main-scroll"><NodesScreen/></div>}</FixtureMount>);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;

const diagnostics = new WeakMap<Page, { caseId: string; events: string[] }>();
async function openFixture(page: Page, mode: string, caseId: string, setupTimeout = 1500): Promise<void> {
  const record = { caseId, events: [] as string[] };
  diagnostics.set(page, record);
  const recordEvent = (kind: string, text: string) => {
    record.events.push(`${kind}: ${text.slice(0, 1500)}`);
    if (record.events.length > 40) record.events.shift();
  };
  page.on('pageerror', error => recordEvent('pageerror', error.stack ?? error.message));
  page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') recordEvent('console', message.text()); });
  page.on('requestfailed', request => recordEvent('requestfailed', `${request.url()} ${request.failure()?.errorText}`));
  page.on('response', response => { if (response.status() >= 400) recordEvent('response', `${response.status()} ${response.url()}`); });
  // Keep each behavior test's existing 5000ms deadline. Setup and individual
  // actions fail early enough to collect diagnostics, rather than waiting for
  // Playwright's 30-second default after Vitest has already terminated the case.
  page.setDefaultTimeout(1500);
  page.setDefaultNavigationTimeout(setupTimeout);
  await page.goto(origin + '/__measurement?mode=' + mode, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(expectedMode => {
    const fixture = (window as any).__measurement;
    const root = document.getElementById('root');
    return fixture?.mounted === true && root?.dataset.fixtureMode === expectedMode && root.childElementCount > 0
      && typeof fixture.publish === 'function' && typeof fixture.state === 'function'
      && (expectedMode !== 'tray-language' || typeof fixture.language === 'function');
  }, mode, { timeout: setupTimeout });
}

async function captureFailure(page: Page, error: unknown): Promise<void> {
  const record = diagnostics.get(page) ?? { caseId: 'uninitialized', events: [] };
  try {
    const mounted = await Promise.race([page.evaluate(() => ({
      readyState: document.readyState,
      fixture: (window as any).__measurement ? { mounted: (window as any).__measurement.mounted,
        publish: typeof (window as any).__measurement.publish, language: typeof (window as any).__measurement.language } : null,
      rootMode: document.getElementById('root')?.dataset.fixtureMode,
      buttons: Array.from(document.querySelectorAll('button')).slice(0, 32).map(button => ({
        text: button.textContent?.slice(0, 160), role: button.getAttribute('role'),
        act: button.getAttribute('data-act'), value: button.getAttribute('data-v'),
      })),
      rootHtml: document.getElementById('root')?.innerHTML.slice(0, 20000),
    })), new Promise<never>((_resolve, reject) => setTimeout(() => reject(Error('diagnostic frame read exceeded 500ms')), 500))]);
    const directory = path.join(root, 'test-results', 'ui01-measurement');
    await mkdir(directory, { recursive: true });
    const file = path.join(directory, record.caseId + '.json');
    await writeFile(file, JSON.stringify({ caseId: record.caseId, error: String(error).slice(0, 4000), events: record.events, mounted }, null, 2));
    console.error(`Mounted measurement failure: ${record.caseId}; diagnostics: ${file}`);
  } catch (diagnosticError) {
    // Retain the original test error even if its frame crashed or diagnostics failed.
    console.error(`Mounted measurement diagnostics unavailable: ${record.caseId}; ${String(diagnosticError).slice(0, 1000)}; ${JSON.stringify(record.events.slice(-6))}`);
  }
}

async function selectAllNodes(page: Page, mode: 'desktop' | 'mobile'): Promise<void> {
  // Desktop exposes the actual stable source ID through NodesTabs.data-v; mobile
  // exposes a segmented button with separate label/count spans. Do not infer a
  // shared accessible name from visually spaced adjacent spans.
  const button = mode === 'desktop'
    ? page.locator('#node-tabs-scroll button[data-act="sub-tab"][data-v="__all_nodes__"]')
    : page.locator('.mn-segs button').filter({ has: page.locator('.mn-seg-label', { hasText: /^全部$/ }) });
  expect(await button.count()).toBe(1);
  expect(await button.locator(mode === 'desktop' ? '.cnt' : '.mn-seg-count').innerText()).toBe('3');
  expect(await button.locator(mode === 'desktop' ? 'span:first-child' : '.mn-seg-label').innerText()).toBe('全部');
  await button.scrollIntoViewIfNeeded();
  await button.click();
  if (mode === 'desktop') await expect.poll(() => button.getAttribute('class')).toMatch(/(?:^|\s)on(?:\s|$)/);
  else await expect.poll(() => button.getAttribute('aria-current')).toBe('true');
}

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('UI01 remaining capabilities in mounted production views', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(root, 'node_modules/.vite/ui01-measurement'),
      server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
        name: 'measurement-fixture',
        resolveId(id) { if (id === '/measurement-fixture.tsx') return id; },
        load(id) { if (id === '/measurement-fixture.tsx') return entry; },
        configureServer(vite) { vite.middlewares.use('/__measurement', async (_req, res) => {
          res.setHeader('Content-Type','text/html');
          res.end(await vite.transformIndexHtml('/__measurement', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/measurement-fixture.tsx"></script></html>'));
        }); },
      }] });
    await server.listen(); const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw Error('fixture did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ chromiumSandbox: true, headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
    // Vite module/CSS/i18n preparation belongs to this bounded setup phase, not
    // the first behavior case's 5000ms allowance. Readiness comes from actual
    // production React commit; each later page must independently reach it.
    for (const mode of ['desktop', 'mobile']) {
      const page = await browser.newPage({ viewport: { width: mode === 'desktop' ? 1100 : 390, height: 844 } });
      try {
        await openFixture(page, mode, 'setup-' + mode, 3000);
        await page.evaluate(() => (window as any).__measurement.unmount());
      } catch (error) { await captureFailure(page, error); throw error; }
      finally { await page.close(); }
    }
  }, 30000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const mode of ['desktop','mobile'] as const) {
    it(mode + ': all/source search keeps stable IDs, same-name nodes, and saved manual selection', async () => {
      const page = await browser.newPage({ viewport: { width: mode==='mobile'?390:1100, height:844 } });
      try {
        await openFixture(page, mode, mode + '-source-search');
        await selectAllNodes(page, mode);
        await expect.poll(() => page.locator('[data-node-source]').count()).toBe(3);
        expect(await page.locator('[data-node-source]').allTextContents()).toEqual(expect.arrayContaining([expect.stringContaining('Alpha'),expect.stringContaining('Beta'),expect.stringContaining('自建节点')]));
        expect(await page.evaluate(() => (window as any).__measurement.state().selected)).toBe('a');
        await page.locator(mode === 'desktop' ? '#node-search' : '.mn-search input[type=search]').fill('Beta');
        await expect.poll(() => page.locator('[data-node-source]').count()).toBe(1);
        expect(await page.locator('[data-node-source]').textContent()).toContain('Beta');
        await page.locator(mode === 'desktop' ? '#node-search' : '.mn-search input[type=search]').fill('');
        await page.getByRole('button', { name: 'Empty', exact:true }).click();
        expect(await page.evaluate(() => (window as any).__measurement.state().selected)).toBe('a');
      } catch (error) { await captureFailure(page, error); throw error; }
      finally { await page.close(); }
    });
    it(mode + ': five states and disconnected/not-included labels use the same stored record', async () => {
      const page = await browser.newPage({ viewport: { width: mode==='mobile'?390:1100, height:844 } });
      try {
        await openFixture(page, mode, mode + '-measurement-states');
        await selectAllNodes(page, mode);
        await expect.poll(() => page.locator('[data-measurement-state="success"]').count()).toBeGreaterThan(0);
        expect(await page.locator('[data-measurement-state="failed"]').count()).toBeGreaterThan(0);
        expect(await page.locator('[data-measurement-state="unmeasured"]').count()).toBeGreaterThan(0);
        for (const [kind,state,text] of [['expired','expired','过期'],['paused','paused','暂停'],['notIncluded','unmeasured','未纳入'],['disconnected','success','未连接测量']]) {
          await page.evaluate(kind => (window as any).__measurement.publish(kind), kind);
          await expect.poll(() => page.locator('[data-measurement-state="'+state+'"]').allTextContents()).toContainEqual(expect.stringContaining(text));
        }
      } catch (error) { await captureFailure(page, error); throw error; }
      finally { await page.close(); }
    });
  }


  it('Home rejects the previous result when the displayed effective node has a same-ID address edit',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await openFixture(page, 'home', 'effective-node-edit');
      await expect.poll(()=>page.locator('.h-latency-value [data-measurement-state="success"]').count()).toBe(1);
      // Missing runtime confirmation must not borrow the saved node's result.
      await page.evaluate(()=>(window as any).__measurement.actual(null));
      await expect.poll(()=>page.locator('.h-latency-value [data-measurement-state="success"]').count()).toBe(0);
      await page.evaluate(()=>(window as any).__measurement.actual('a'));
      await expect.poll(()=>page.locator('.h-latency-value [data-measurement-state="success"]').count()).toBe(1);
      await page.evaluate(()=>(window as any).__measurement.editSelected());
      await expect.poll(()=>page.locator('.h-latency-value [data-measurement-state="unmeasured"]').count()).toBe(1);
      expect(await page.locator('.h-latency-value').innerText()).toContain('未纳入');
      expect(await page.locator('.h-latency-value').innerText()).not.toContain('40 ms');
    } catch (error) { await captureFailure(page, error); throw error; }
    finally {await page.close();}
  });
  it('a warm tray measurement label uses the same hot-refreshed language as its auxiliary labels',async()=>{
    const page=await browser.newPage({viewport:{width:420,height:500}});
    try {
      await openFixture(page, 'tray-language', 'tray-language');
      await page.evaluate(()=>(window as any).__measurement.publish('notIncluded'));
      await expect.poll(()=>page.locator('[data-measurement-state]').innerText()).toContain('未纳入');
      await page.evaluate(()=>(window as any).__measurement.language());
      await expect.poll(()=>page.locator('[data-tray-language]').getAttribute('data-tray-language')).toBe('en-US');
      const text=await page.locator('[data-measurement-state]').innerText();
      expect(text).not.toContain('未纳入');expect(text).toContain('Not included');
    } catch (error) { await captureFailure(page, error); throw error; }
    finally {await page.close();}
  });
  it('iOS: cancelling the actual current-measurement prompt issues no server request; confirmation issues one', async () => {
    const page = await browser.newPage({ viewport:{width:390,height:844} });
    try {
      await openFixture(page, 'ios', 'ios-current-consent');
      await page.locator('.h-latency-action').first().click();
      await page.getByRole('dialog').waitFor();
      expect(await page.evaluate(() => (window as any).__measurement.calls)).toBe(0);
      await page.getByRole('dialog').getByRole('button',{name:'取消',exact:true}).click();
      expect(await page.evaluate(() => (window as any).__measurement.calls)).toBe(0);
      await page.locator('.h-latency-action').first().click();
      await page.getByRole('dialog').getByRole('button',{name:"确认",exact:true}).click();
      await expect.poll(() => page.evaluate(() => (window as any).__measurement.calls)).toBe(1);
    } catch (error) { await captureFailure(page, error); throw error; }
    finally { await page.close(); }
  });

  it('iOS: cancelling the actual network-check prompt and leaving its page issue no backend action', async () => {
    const page = await browser.newPage({ viewport:{width:390,height:844} });
    try {
      await openFixture(page, 'ios', 'ios-network-consent');
      await page.locator('[data-write-control="network-check"]').click();
      await page.getByRole('dialog').waitFor();
      expect(await page.evaluate(() => (window as any).__measurement.networkCalls)).toBe(0);
      await page.getByRole('dialog').getByRole('button',{name:'取消',exact:true}).click();
      expect(await page.evaluate(() => (window as any).__measurement.networkCalls)).toBe(0);
      await page.locator('[data-write-control="network-check"]').click();
      await page.getByRole('dialog').waitFor();
      await page.evaluate(() => (window as any).__measurement.unmount());
      expect(await page.evaluate(() => (window as any).__measurement.networkCalls)).toBe(0);
      expect(await page.evaluate(() => (window as any).__measurement.calls)).toBe(0);
    } catch (error) { await captureFailure(page, error); throw error; }
    finally { await page.close(); }
  });
  for (const width of [390, 1100]) for (const rtl of [false, true]) {
    it(`shared select: shallow card at ${width}px / rtl=${rtl} exposes long options and keyboard navigation`, async () => {
      const page = await browser.newPage({ viewport: { width, height: 844 } });
      try {
        await openFixture(page, 'csel', `csel-${width}-${rtl}`);
        if (rtl) await page.evaluate(() => { document.documentElement.dir = 'rtl'; });
        await page.getByRole('button', { name: 'DNS fallback' }).click();
        const menu = page.getByRole('listbox'); await menu.waitFor({ state: 'visible' });
        const box = await menu.boundingBox();
        expect(box!.height).toBeGreaterThan(150); expect(box!.x).toBeGreaterThanOrEqual(0);
        expect(box!.x + box!.width).toBeLessThanOrEqual(width); expect(box!.y).toBeGreaterThanOrEqual(0);
        expect(box!.y + box!.height).toBeLessThanOrEqual(844);
        const trigger = page.getByRole('button', { name: 'DNS fallback' });
        for (let i=0;i<11;i++) await trigger.press('ArrowDown');
        await trigger.press('Enter');
        expect(await trigger.innerText()).toContain('Resolver 11');
        expect(await menu.count()).toBe(0);
      } finally { await page.close(); }
    });
  }

});

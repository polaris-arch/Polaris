import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { statusFixture } from '@/contracts/auto-select-status.test-support';

const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import NodesScreen from '/src/components/screens/nodes/NodesScreen';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const mobile=new URLSearchParams(location.search).get('mobile')==='1';
if(mobile){await import('/src/styles/tokens.resolved.css');await import('/src/mobile/mobile.css');await import('/src/mobile/redesign.css');}
else await import('/src/styles/index.css');
const initial=${JSON.stringify(statusFixture())};
const servers=['a','b'].map(id=>({id:'node-'+id,name:'Fixture '+id,protocol:'socks',address:'fixture.invalid',port:1,subscriptionId:'sub-a'}));
let config={selectedServerId:'node-a',proxyMode:'smart',servers,subscriptions:[{id:'sub-a',name:'Subscription A',url:'https://fixture.invalid'}],selectionIntent:{mode:'auto',scope:'subscription',subscriptionId:'sub-a'}};
let listener; const callbacks=[],jobs=[];
const f=window.__status={reads:0,writes:0,offs:0,offThrows:false,callbacks,jobs,
 resolve(patch={},index=0){jobs[index].resolve({...structuredClone(initial),...patch});},
 reject(index=0){jobs[index].reject(Error('snapshot failed'));},
 event(patch={}){listener?.({...structuredClone(initial),...patch});},
 stale(patch={}){callbacks[0]?.({...structuredClone(initial),...patch});},
 publish(patch){config={...config,...patch};useAppStore.setState({config,servers:config.servers,selectedServerId:config.selectedServerId});},
 manual(){config={...config};delete config.selectionIntent;f.publish(config);},
 draft(){useStagedConfigStore.setState({enabled:true,hydrated:true,baseline:config,entries:[{id:'draft',kind:'setting',entityPath:['selectionIntent'],nextValue:undefined,label:'draft'}]});},
 unmount(){app.unmount();},
 remount(){app=createRoot(document.getElementById('root'));render();},
 state(){return structuredClone(useAppStore.getState().config);},
};
api.server.autoSelectStatus=()=>{f.reads++;return new Promise((resolve,reject)=>jobs.push({resolve,reject}));};
api.server.onAutoSelectStatus=fn=>{listener=fn;callbacks.push(fn);return()=>{f.offs++;if(f.offThrows)throw Error('detach failed');if(listener===fn)listener=null;};};
api.server.autoSelectEnable=async()=>{f.writes++;throw Error('unexpected enable');};
api.server.autoSelectSwitchNow=async()=>{f.writes++;throw Error('unexpected switch');};
api.server.switch=async()=>{f.writes++;throw Error('unexpected manual switch');};
api.config.meshRouteReport=async()=>null;
useStagedConfigStore.setState({enabled:false,hydrated:true,entries:[]});
useAppStore.setState({config,servers,selectedServerId:'node-a',proxyStatus:{running:false},invalidNodes:[]});
let app=createRoot(document.getElementById('root'));const render=()=>app.render(mobile?<MobileNodesScreen/>:<NodesScreen/>);render();
`;
let server: ViteDevServer;
let browser: Browser;
let origin: string;
const panel = (page: Page) => page.locator('[data-auto-select-status]');
const data = (page: Page) => page.evaluate(() => { const f=(window as any).__status;return {reads:f.reads,writes:f.writes,offs:f.offs,config:f.state()}; });
async function open(mobile: boolean) {
  const page = await browser.newPage({ viewport: { width: mobile ? 390 : 1100, height: 844 } });
  page.setDefaultTimeout(7000);
  await page.goto(`${origin}/__status?mobile=${mobile ? 1 : 0}`);
  await panel(page).waitFor();
  await page.waitForFunction(() => (window as any).__status?.reads === 1);
  return page;
}

// Real production screens and state hook, one browser, sequential cases, IPC only is faked.
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('Auto status mounted snapshot/event lifetime', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), `polaris-auto-status-${process.pid}`), server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
      name: 'auto-status-fixture',
      resolveId(id) { if (id === '/auto-status-fixture.tsx') return id; },
      load(id) { if (id === '/auto-status-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__status', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__status', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/auto-status-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen(); const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const mobile of [false, true]) {
    const screen = mobile ? 'mobile' : 'desktop';
    it(`${screen}: snapshot shows backend rate/exclusion and never writes intent or exit`, async () => {
      const page = await open(mobile);
      try {
        const before = await data(page);
        await page.evaluate(() => (window as any).__status.resolve({mode:'rateLimited',reason:null,barred:[{serverId:'node-b',remainingMs:61001}]}));
        await expect.poll(() => panel(page).textContent()).toContain('频度上限');
        expect(await panel(page).textContent()).toContain('2 / 3');
        expect(await panel(page).textContent()).toContain('Fixture b');
        expect(await panel(page).textContent()).toContain('62 秒');
        expect(await data(page)).toEqual(before);
        expect(await panel(page).locator('button').count()).toBe(1);
        expect(await panel(page).evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
        await page.screenshot({ path: path.join(tmpdir(), `polaris-ui01-status-${screen}.png`) });
      } finally { await page.close(); }
    });

    it(`${screen}: newer event wins against a late success snapshot`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.event({mode:'notEvaluated',reason:'background'});f.resolve();});
        await expect.poll(() => panel(page).textContent()).toContain('不在前台');
        expect(await panel(page).textContent()).not.toContain('没有被明显更优');
        expect(await panel(page).getAttribute('aria-busy')).toBe('false');
      } finally { await page.close(); }
    });

    it(`${screen}: newer event wins against a late rejected snapshot`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.event({mode:'commitFailed',reason:'commitReconciling'});f.reject();});
        await expect.poll(() => panel(page).textContent()).toContain('均未证实');
        expect(await panel(page).textContent()).not.toContain('读取状态失败');
      } finally { await page.close(); }
    });

    it(`${screen}: same-turn rapid refresh is single-flight and failure can be retried`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => (window as any).__status.reject());
        await expect.poll(() => panel(page).textContent()).toContain('读取状态失败');
        await panel(page).locator('button').evaluate(el => {for(let i=0;i<8;i++)(el as HTMLButtonElement).click();});
        expect((await data(page)).reads).toBe(2);
        await page.evaluate(() => (window as any).__status.resolve({mode:'noData',reason:'meteredPaused'},1));
        await expect.poll(() => panel(page).textContent()).toContain('计费网络策略');
        expect((await data(page)).writes).toBe(0);
      } finally { await page.close(); }
    });

    it(`${screen}: saved config replacement invalidates old snapshots and updates display names`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => (window as any).__status.publish({subscriptions:[{id:'sub-b',name:'Subscription B'}],selectionIntent:{mode:'auto',scope:'subscription',subscriptionId:'sub-b'}}));
        await expect.poll(async () => (await data(page)).reads).toBe(2);
        await page.evaluate(() => {const f=(window as any).__status;f.resolve({mode:'noData',reason:'currentExpired'},0);f.resolve({intent:{mode:'auto',scope:'subscription',subscriptionId:'sub-b'}},1);});
        await expect.poll(() => panel(page).textContent()).toContain('Subscription B');
        expect(await panel(page).textContent()).not.toContain('Subscription A');
        expect(await panel(page).textContent()).not.toContain('已过期');
      } finally { await page.close(); }
    });

    it(`${screen}: draft intent changes cannot disable the saved-intent status`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.resolve();f.draft();});
        await expect.poll(() => panel(page).textContent()).toContain('Subscription A');
        expect((await data(page)).reads).toBe(1);
      } finally { await page.close(); }
    });

    it(`${screen}: clearing saved intent hides status and ignores late replies/events even if detach fails`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.offThrows=true;f.manual();});
        await panel(page).waitFor({state:'detached'});
        await page.evaluate(() => {const f=(window as any).__status;f.resolve();f.stale({mode:'commitFailed',reason:'commitFailed'});});
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
        expect(await panel(page).count()).toBe(0);
        expect((await data(page)).offs).toBe(1);
      } finally { await page.close(); }
    });

    it(`${screen}: unmount invalidates failed-detach callbacks and pending snapshot before remount`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.offThrows=true;f.unmount();f.remount();});
        await expect.poll(async () => (await data(page)).reads).toBe(2);
        await page.evaluate(() => {const f=(window as any).__status;f.stale({mode:'commitFailed',reason:'commitFailed'});f.resolve({mode:'noData',reason:'currentExpired'},0);f.resolve({mode:'notEvaluated',reason:'measuring'},1);});
        await expect.poll(() => panel(page).textContent()).toContain('测速尚未收尾');
        expect(await panel(page).textContent()).not.toContain('读取状态失败');
        expect((await data(page)).offs).toBe(1);
      } finally { await page.close(); }
    });

    it(`${screen}: backend manual event hides Auto; unknown intent is explained without fake counters`, async () => {
      const page = await open(mobile);
      try {
        await page.evaluate(() => {const f=(window as any).__status;f.event({intent:{mode:'manual',scope:null,subscriptionId:null},mode:'manual',reason:null});f.resolve();});
        await panel(page).waitFor({state:'detached'});
        await page.evaluate(() => (window as any).__status.event({intent:{mode:'unrecognized',scope:null,subscriptionId:null},mode:'notEvaluated',reason:'intentUnrecognized'}));
        await expect.poll(() => panel(page).textContent()).toContain('不认识保存的选择意图');
        expect(await panel(page).textContent()).not.toContain('2 / 3');
      } finally { await page.close(); }
    });
  }
});

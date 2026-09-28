import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { HomeNodePickerList } from '/src/mobile/home/HomeNodePickerList';
import { MobileSpeedTestProgress } from '/src/mobile/MobileSpeedTestProgress';
import { useMobileSpeedTestStore as store } from '/src/mobile/use-mobile-speed-test';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
import '/src/mobile/mobile.css';
import '/src/mobile/home/home.css';
import '/src/mobile/redesign.css';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const t=(key,opts)=>i18n.t(key,opts);
const servers=[
 {id:'manual',name:'Manual fixture',protocol:'socks',address:'manual.invalid',port:1},
 {id:'orphan',name:'Orphan fixture',protocol:'http',address:'orphan.invalid',port:2,subscriptionId:'gone'},
 {id:'mesh',name:'Mesh fixture',protocol:'tailscale',address:'',port:0},
 {id:'sub-a',name:'Selected fixture',protocol:'vmess',address:'selected.invalid',port:3,subscriptionId:'first'},
 {id:'sub-b',name:'Other fixture',protocol:'trojan',address:'other.invalid',port:4,subscriptionId:'second'},
];
const subscriptions=[{id:'first',name:'First subscription'},{id:'second',name:'Second subscription'},{id:'empty',name:'Empty subscription'}];
const test=window.__pickerTest={picked:null,store};
function App(){
 const [query,setQuery]=useState('');const [selected,setSelected]=useState('sub-a');const [open,setOpen]=useState(true);const [screen,setScreen]=useState('home');
 const rows=servers.map(server=>({server,isCurrent:server.id===selected,protocolLabel:server.protocol,stagedOnly:false,latencyMs:undefined,latencyStale:false}));
 return <main className="mobile-root">
  <input aria-label="搜索节点" value={query} onChange={e=>setQuery(e.target.value)}/>
  <button onClick={()=>setOpen(value=>!value)}>开关列表</button>
  <button onClick={()=>setSelected(null)}>无当前节点</button>
  <button onClick={()=>setScreen(value=>value==='home'?'nodes':'home')}>切换页面</button>
  {open&&<HomeNodePickerList rows={rows} subscriptions={subscriptions} query={query} t={t} onUseAsExit={server=>test.picked=server.id}/>}
  <div data-speed-screen={screen} key={screen}><MobileSpeedTestProgress/></div>
 </main>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile picker and measured task consumers', () => {
  beforeAll(async () => {
    server=await createServer({root,cacheDir:path.join(tmpdir(),'polaris-picker-speed-vite-'+process.pid),server:{host:'127.0.0.1',port:0,watch:null},plugins:[{
      name:'picker-speed-fixture',resolveId(id){if(id==='/picker-speed-fixture.tsx')return id;},load(id){if(id==='/picker-speed-fixture.tsx')return entry;},
      configureServer(vite){vite.middlewares.use('/__picker-speed',async(_req,res)=>{
        const html=await vite.transformIndexHtml('/__picker-speed','<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/picker-speed-fixture.tsx"></script></html>');
        res.setHeader('Content-Type','text/html');res.end(html);
      });},
    }]});await server.listen();const address=server.httpServer!.address();if(!address||typeof address!=='object')throw new Error('Vite did not bind');origin='http://127.0.0.1:'+address.port;
    browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});
  },30_000);
  afterAll(async()=>{await browser?.close();await server?.close();});
  it('PC provenance/default folding, physical header clicks, search and reopen preserve their actual semantics',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__picker-speed');
    const groups=page.locator('.h-pick-group');await groups.first().waitFor();
    expect(await groups.evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-group-id')))).toEqual(['manual','mesh','first','second']);
    expect(await page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')))).toEqual(['sub-a']);
    expect(await groups.first().boundingBox()).toMatchObject({height:44});
    await groups.first().click();
    expect(await page.locator('[data-picker-node="orphan"]').count()).toBe(1);
    expect(await page.locator('[data-picker-node="manual"]').count()).toBe(1);
    await page.locator('[data-group-id="first"]').click();
    await page.getByRole('textbox',{name:'搜索节点'}).fill('other.invalid');
    expect(await page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')))).toEqual(['sub-b']);
    expect(await groups.first().isDisabled()).toBe(true);
    expect(await groups.first().evaluate(node=>getComputedStyle(node).opacity)).toBe('1');
    await page.getByRole('textbox',{name:'搜索节点'}).fill('tailscale');
    expect(await page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')))).toEqual(['mesh']);
    await page.getByRole('textbox',{name:'搜索节点'}).fill('');
    expect(await page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')))).toEqual(['manual','orphan']);
    await page.locator('[data-picker-node="orphan"] button').click();expect(await page.evaluate(()=>(window as any).__pickerTest.picked)).toBe('orphan');
    expect(await page.locator('ul.h-picklist ul, button button').count()).toBe(0);
    await page.getByRole('button',{name:'开关列表'}).click();await page.getByRole('button',{name:'开关列表'}).click();
    expect(await page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')))).toEqual(['sub-a']);
    await page.getByRole('button',{name:'无当前节点'}).click();
    await page.waitForFunction(()=>document.querySelectorAll('[data-picker-node]').length===0);
    await page.close();
  },30_000);
  it('real counters persist across screen remount; preparation hides previous results and silence is not an interruption',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__picker-speed');await page.locator('.h-pick-group').first().waitFor();
    await page.evaluate(()=>{const store=(window as any).__pickerTest.store;store.getState().begin(['a','b','c'],'all');});
    await page.getByText('准备测速…',{exact:true}).waitFor();expect(await page.locator('progress').count()).toBe(0);
    await page.evaluate(()=>{const store=(window as any).__pickerTest.store;store.getState().progress({runId:'1',tested:1,ok:0,total:3});});
    await page.locator('progress').waitFor();expect(await page.locator('progress').getAttribute('value')).toBe('1');expect(await page.locator('progress').getAttribute('max')).toBe('3');
    expect(await page.locator('.m-speedtest-counts').innerText()).toContain('成功 0 · 失败 1');
    await page.getByRole('button',{name:'切换页面'}).click();expect(await page.locator('[data-speed-screen="nodes"] progress').getAttribute('value')).toBe('1');
    await page.evaluate(()=>{const s=(window as any).__pickerTest.store.getState();s.done({runId:'1',outcome:'interrupted',tested:1,total:3,serverIds:['a','b','c'],pending:['b','c']});s.settle(s.request.token,{runId:'1',results:{a:-1},outcome:'interrupted',notInPool:[],tsNotReady:[]});s.begin(['a'],'current');});
    await page.getByText('准备测速…',{exact:true}).waitFor();expect(await page.locator('progress').count()).toBe(0);expect(await page.locator('.m-speedtest-counts').count()).toBe(0);
    await page.evaluate(()=>{const s=(window as any).__pickerTest.store.getState();s.progress({runId:'2',tested:0,ok:0,total:1});s.waiting('2');});
    await page.getByText('暂未收到新进度，状态待确认',{exact:true}).waitFor();expect(await page.locator('.m-speedtest-progress').innerText()).not.toContain('中断');
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().done({runId:'1',outcome:'completed',tested:3,total:3,serverIds:['a','b','c'],pending:[]});});
    expect(await page.locator('progress').getAttribute('max')).toBe('1');expect(await page.locator('progress').getAttribute('value')).toBe('0');
    await page.close();
  },30_000);
});

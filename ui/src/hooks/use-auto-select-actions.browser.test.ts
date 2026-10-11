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
import { NodesScreen } from '/src/components/screens/nodes/NodesScreen';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { HomeScreen } from '/src/components/screens/home/HomeScreen';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import TrayMenu from '/src/tray/TrayMenu';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import { IPC_CHANNELS as C } from '/src/domain/ipc-channels';
import { setToastImpl } from '/src/lib/error-handler';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
localStorage.setItem('polaris.language','zh-CN');
const params=new URLSearchParams(location.search),surface=params.get('surface');
if(surface.startsWith('mobile'))document.documentElement.dataset.mobileOs='android';
const initial=${JSON.stringify(statusFixture())};
const servers=['a','b'].map(id=>({id:'node-'+id,name:'Fixture '+id,protocol:'socks',address:'fixture.invalid',port:1,subscriptionId:'sub-'+id}));
let disk={selectedServerId:'node-a',proxyMode:'smart',proxyModeType:'manual',servers,subscriptions:[{id:'sub-a',name:'Subscription A',url:'https://fixture.invalid'},{id:'sub-b',name:'Subscription B',url:'https://fixture.invalid'},{id:'empty',name:'Empty subscription',url:'https://fixture.invalid'}],desktopNotifications:false};
if(params.get('auto')==='1')disk.selectionIntent={mode:'auto',scope:'subscription',subscriptionId:'sub-a'};
let status={...initial,intent:disk.selectionIntent?initial.intent:{mode:'manual',scope:null,subscriptionId:null}};
let id=0;const handlers=new Map(),listeners=[];
const jobs=[],readJobs=[];let app;
const f=window.__actions={calls:[],reads:0,jobs,readJobs,messages:[],readFail:false,hold:false,
 state:()=>structuredClone(useAppStore.getState().config),saved:()=>structuredClone(disk),
 // Observe the full config prop of each production control, including Tray's
 // private state. Only native IPC is replaced; components/stores remain real.
 displayed(){const el=document.querySelector('[data-auto-select-controls]');let fiber=el?.[Object.keys(el).find(k=>k.startsWith('__reactFiber$'))];while(fiber){if(fiber.memoizedProps?.config)return structuredClone(fiber.memoizedProps.config);fiber=fiber.return;}return null;},
 changeDisk(patch){disk={...disk,...patch};},
 refresh(){if(surface==='tray'){for(const l of listeners)if(l.event===C.EVENT_CONFIG_CHANGED)handlers.get(l.handler)?.({payload:undefined});}else f.refreshPending=useAppStore.getState().loadConfig(true);},
 loading:()=>useAppStore.getState().configLoading,
 event(patch){status={...status,...patch};for(const l of listeners)if(l.event===C.EVENT_AUTO_SELECT_STATUS)handlers.get(l.handler)?.({payload:structuredClone(status)});},
 publish(patch){disk={...disk,...patch};useAppStore.setState({config:structuredClone(disk),servers:disk.servers,selectedServerId:disk.selectedServerId});for(const l of listeners)if(l.event===C.EVENT_CONFIG_CHANGED)handlers.get(l.handler)?.({payload:undefined});},
 manual(){delete disk.selectionIntent;f.publish({selectedServerId:'node-b'});},
 finish(index=0,options={}){const job=jobs[index];if(options.reject){job.resolve({success:false,error:'fixture rejected',code:options.reject});return;}
  if(!options.noSave){if(job.cmd===C.AUTO_SELECT_ENABLE)disk.selectionIntent={mode:'auto',scope:'subscription',subscriptionId:job.args.subscriptionId};
   if(job.cmd===C.SERVER_SWITCH){disk.selectedServerId=job.args.serverId;delete disk.selectionIntent;}}
  status={...initial,intent:disk.selectionIntent?{...disk.selectionIntent}:{mode:'manual',scope:null,subscriptionId:null},exit:{serverId:disk.selectedServerId,inSubscription:!!disk.selectionIntent}};
  if(options.staleAuto)status={...initial};
  const receipt=job.cmd===C.SERVER_SWITCH?{serverId:job.args.serverId,status:options.status||'applied'}:
   {intentApplied:options.staleAuto||!!disk.selectionIntent,placement:job.cmd===C.AUTO_SELECT_ENABLE&&!options.status?null:{serverId:disk.selectedServerId,status:options.status||'applied'},status:structuredClone(status)};
  job.resolve({success:true,data:receipt});
 },
 unmount(){app.unmount();},remount(){app=createRoot(document.getElementById('root'));render();},
 manualWrite(){return useAppStore.getState().switchServer('node-b');},
};
window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:surface==='tray'?'tray-menu':'main'},currentWebview:{label:'main'}},
 transformCallback(fn){const key=++id;handlers.set(key,fn);return key;},
 invoke(cmd,args={}){
  if(cmd==='plugin:event|listen'){const key=++id;listeners.push({id:key,event:args.event,handler:args.handler});return Promise.resolve(key);}
  if(cmd==='plugin:event|unlisten')return Promise.resolve();
  if([C.AUTO_SELECT_ENABLE,C.AUTO_SELECT_SWITCH_NOW,C.SERVER_SWITCH].includes(cmd)){f.calls.push({cmd,args});return new Promise(resolve=>{jobs.push({cmd,args,resolve});if(!f.hold)f.finish(jobs.length-1);});}
  if(cmd===C.CONFIG_GET){f.reads++;if(f.delayNextRead){f.delayNextRead=false;const captured=structuredClone(disk);return new Promise(resolve=>{const release=()=>resolve({success:true,data:captured});f.releaseRead=release;readJobs.push({captured,release});});}return f.readFail?Promise.reject(Error('saved readback failed')):Promise.resolve({success:true,data:structuredClone(disk)});}
  if(cmd===C.AUTO_SELECT_STATUS)return Promise.resolve({success:true,data:structuredClone(status)});
  if(cmd===C.PROXY_GET_STATUS)return Promise.resolve({success:true,data:{running:false,starting:false}});
  if(cmd===C.PROXY_GET_PENDING_CHANGES)return Promise.resolve({success:true,data:{needsRestart:false,fields:[],scopes:[]}});
  if(cmd===C.STATS_GET_CONNECTIONS||cmd===C.CONNECTIONS_GET_ALL)return Promise.resolve({success:true,data:[]});
  if(cmd==='plugin:window|inner_size'||cmd==='plugin:window|outer_size')return Promise.resolve({width:600,height:800});
  if(cmd==='plugin:window|scale_factor')return Promise.resolve(1);
  if(cmd==='plugin:window|available_monitors')return Promise.resolve([]);
  return Promise.resolve({success:true,data:null});
 }
};
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(event,key){const at=listeners.findIndex(l=>l.id===key);if(at>=0)listeners.splice(at,1);}};
setToastImpl({success:text=>f.messages.push(['success',text]),info:text=>f.messages.push(['info',text]),warning:text=>f.messages.push(['warning',text]),error:text=>f.messages.push(['error',text])});
api.config.meshRouteReport=async()=>null;
useStagedConfigStore.setState({enabled:false,hydrated:true,entries:[],baseline:disk});
useAppStore.setState({config:structuredClone(disk),servers:disk.servers,selectedServerId:disk.selectedServerId,proxyStatus:{running:false,starting:false},invalidNodes:[]});
const components={'desktop-nodes':NodesScreen,'mobile-nodes':MobileNodesScreen,'desktop-home':HomeScreen,'mobile-home':MobileHomeScreen,'tray':TrayMenu};
await import('/src/styles/index.css');if(surface.startsWith('mobile')){await import('/src/styles/tokens.resolved.css');await import('/src/mobile/mobile.css');await import('/src/mobile/redesign.css');await import('/src/mobile/home/home.css');}
const Component=components[surface];const render=()=>app.render(<Component/>);app=createRoot(document.getElementById('root'));render();
`;
let server: ViteDevServer, browser: Browser, origin: string;
const surfaces = ['desktop-nodes', 'mobile-nodes', 'desktop-home', 'mobile-home', 'tray'] as const;
const controls = (page: Page) => page.locator('[data-auto-select-controls]');
const state = (page: Page) => page.evaluate(() => { const f=(window as any).__actions;return {calls:f.calls,reads:f.reads,config:f.state(),saved:f.saved(),messages:f.messages}; });
async function open(surface: typeof surfaces[number], auto = false) {
  const page=await browser.newPage({viewport:{width:surface.startsWith('mobile')?390:1100,height:844}});page.setDefaultTimeout(7000);
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto(`${origin}/__actions?surface=${surface}&auto=${auto?1:0}`);
  if(surface==='desktop-home')await page.locator('#node-trigger').click();
  if(surface==='mobile-home')await page.locator('[data-write-control="switch-node"], [data-home-action="switch-node"]').first().click();
  if(surface==='tray')await page.getByText('全部节点',{exact:true}).click();
  try{await controls(page).waitFor();}catch(error){throw Error(`${surface} mounted failure: ${errors.join('; ')}; ${await page.locator('body').innerText()}; ${String(error)}`);}
  return page;
}

describe.runIf(process.env.POLARIS_BROWSER_TESTS==='1')('five real mounted Auto entry surfaces; native IPC only fake',()=>{
 beforeAll(async()=>{
  server=await createServer({root,cacheDir:path.join(tmpdir(),`polaris-auto-actions-${process.pid}`),server:{host:'127.0.0.1',port:0,watch:null},plugins:[{name:'auto-actions-native-fixture',resolveId(id){if(id==='/auto-actions-native-fixture.tsx')return id;},load(id){if(id==='/auto-actions-native-fixture.tsx')return entry;},configureServer(vite){vite.middlewares.use('/__actions',async(_req,res)=>{res.setHeader('Content-Type','text/html');res.end(await vite.transformIndexHtml('/__actions','<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/auto-actions-native-fixture.tsx"></script></html>'));});}}]});
  await server.listen();const addr=server.httpServer!.address();if(!addr||typeof addr!=='object')throw Error('Vite did not bind');origin='http://127.0.0.1:'+addr.port;
  browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});
 },30000);
 afterAll(async()=>{await browser?.close();await server?.close();});
 for(const surface of surfaces){
  it(`${surface}: production entry enables saved Auto and disables only the separate empty choice`,async()=>{
   const page=await open(surface);try{
    expect(await controls(page).locator('[data-auto-select-subscription="empty"]').isDisabled()).toBe(true);
    expect(await controls(page).locator('[data-auto-select-empty="empty"]').isVisible()).toBe(true);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).saved.selectionIntent?.subscriptionId).toBe('sub-a');
    await expect.poll(()=>controls(page).textContent()).toContain('意图已保存');
    expect((await state(page)).saved.servers).toHaveLength(2);
    expect((await state(page)).calls[0]).toEqual({cmd:'auto_select_enable',args:{subscriptionId:'sub-a'}});
   }finally{await page.close();}
  });
  it(`${surface}: eight immediate clicks issue one native enable with no optimistic selection`,async()=>{
   const page=await open(surface);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').evaluate(el=>{for(let i=0;i<8;i++)(el as HTMLButtonElement).click();});
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    expect((await state(page)).saved.selectionIntent).toBeUndefined();
    expect(await controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('false');
    await page.evaluate(()=> (window as any).__actions.finish(0,{status:'pending'}));
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('等待运行侧确认');
    expect(await controls(page).locator('[data-auto-select-feedback]').getAttribute('data-auto-select-feedback')).toBe('info');
   }finally{await page.close();}
  });
  it(`${surface}: failed switch-now placement keeps saved Auto and shows incomplete runtime`,async()=>{
   const page=await open(surface,true);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-exit-write="auto-select-now"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    expect((await state(page)).calls[0].cmd).toBe('auto_select_switch_now');
    await page.evaluate(()=> (window as any).__actions.finish(0,{status:'failed'}));
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('运行侧换点未完成');
    expect((await state(page)).saved.selectionIntent.subscriptionId).toBe('sub-a');
    expect(await controls(page).locator('[data-auto-select-feedback]').getAttribute('data-auto-select-feedback')).toBe('warning');
   }finally{await page.close();}
  });
  it(`${surface}: rejected cancellation keeps Auto and a successful retry confirms manual truth`,async()=>{
   const page=await open(surface,true);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-exit-write="auto-select-manual"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    expect((await state(page)).calls[0]).toEqual({cmd:'server_switch',args:{serverId:'node-a'}});
    await page.evaluate(()=> (window as any).__actions.finish(0,{reject:'AUTO_SELECT_SWITCH_FAILED'}));
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('切换未完成');
    expect(await controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('true');
    await controls(page).locator('[data-exit-write="auto-select-manual"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(2);
    await page.evaluate(()=> (window as any).__actions.finish(1));
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('手动选择已保存');
    expect((await state(page)).saved.selectionIntent).toBeUndefined();
    expect((await state(page)).saved.selectedServerId).toBe('node-a');
   }finally{await page.close();}
  });
  it(`${surface}: successful command with failed config readback remains unconfirmed and retryable`,async()=>{
   const page=await open(surface);try{
    await page.evaluate(()=> {const f=(window as any).__actions;f.hold=true;f.readFail=true;});
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> (window as any).__actions.finish());
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('尚不能确认结果');
    expect(await controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('false');
    await page.evaluate(()=> (window as any).__actions.readFail=false);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(2);
    await page.evaluate(()=> (window as any).__actions.finish(1));
    await expect.poll(()=>controls(page).textContent()).toContain('意图已保存');
   }finally{await page.close();}
  });
  it(`${surface}: late old receipt cannot revive Auto after a saved manual snapshot`,async()=>{
   const page=await open(surface);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> (window as any).__actions.manual());
    await expect.poll(async()=> (await state(page)).saved.selectedServerId).toBe('node-b');
    await page.evaluate(()=> (window as any).__actions.finish(0,{noSave:true,staleAuto:true}));
    await expect.poll(()=> controls(page).getAttribute('aria-busy')).toBe('false');
    expect(await controls(page).locator('[data-auto-select-feedback]').count()).toBe(0);
    expect((await state(page)).saved.selectionIntent).toBeUndefined();
    expect(await controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('false');
   }finally{await page.close();}
  });
  it(`${surface}: a captured old config readback never overwrites a newer saved manual snapshot`, async () => {
   const page=await open(surface);try {
    await page.evaluate(()=> {const f=(window as any).__actions;f.hold=true;f.delayNextRead=true;});
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> (window as any).__actions.finish());
    await expect.poll(()=>page.evaluate(()=>typeof (window as any).__actions.releaseRead)).toBe('function');
    await page.evaluate(()=> (window as any).__actions.manual());
    await expect.poll(()=>controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('false');
    await page.evaluate(()=> (window as any).__actions.releaseRead());
    await expect.poll(()=>controls(page).getAttribute('aria-busy')).toBe('false');
    expect((await state(page)).saved.selectionIntent).toBeUndefined();
    expect((await state(page)).config.selectionIntent).toBeUndefined();
    expect((await state(page)).config.selectedServerId).toBe('node-b');
    expect(await controls(page).locator('[data-auto-select-feedback]').count()).toBe(0);
   }finally{await page.close();}
  });
  for(const refreshPending of [false,true])it(`${surface}: same-selection newer ${refreshPending?'in-flight refresh':'published whole config'} survives old CONFIG_GET`,async()=>{
   const page=await open(surface,true);try{
    await page.evaluate(()=>{const f=(window as any).__actions;f.hold=true;f.delayNextRead=true;});
    await controls(page).locator('[data-exit-write="auto-select-now"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> (window as any).__actions.finish());
    await expect.poll(()=>page.evaluate(()=> (window as any).__actions.readJobs.length)).toBe(1);
    const patch={servers:[{id:'node-a',name:'New Server C2',protocol:'socks',address:'new.invalid',port:2,subscriptionId:'sub-a'}],subscriptions:[{id:'sub-a',name:'New Subscription C2',url:'https://new.invalid'}],trafficRules:[{id:'rule-c2',name:'New Rule C2',action:'direct'}]};
    await page.evaluate(({patch,refreshPending})=>{const f=(window as any).__actions;if(refreshPending){f.changeDisk(patch);f.delayNextRead=true;f.refresh();}else f.publish(patch);},{patch,refreshPending});
    if(refreshPending)await expect.poll(()=>page.evaluate(()=> (window as any).__actions.readJobs.length)).toBe(2);
    else await expect.poll(()=>page.evaluate(()=> (window as any).__actions.displayed()?.servers[0]?.name)).toBe('New Server C2');
    await page.evaluate(()=> (window as any).__actions.readJobs[0].release());
    await expect.poll(()=>controls(page).getAttribute('aria-busy')).toBe('false');
    expect(await controls(page).locator('[data-auto-select-feedback]').count()).toBe(0);
    if(refreshPending){
     if(surface!=='tray')expect(await page.evaluate(()=> (window as any).__actions.loading())).toBe(true);
     await page.evaluate(()=> (window as any).__actions.readJobs[1].release());
    }
    await expect.poll(()=>page.evaluate(()=> (window as any).__actions.displayed()?.servers[0]?.name)).toBe('New Server C2');
    const shown=await page.evaluate(()=> (window as any).__actions.displayed());
    expect(shown.subscriptions).toEqual(patch.subscriptions);expect(shown.trafficRules).toEqual(patch.trafficRules);
    expect(shown.selectedServerId).toBe('node-a');expect(shown.selectionIntent.subscriptionId).toBe('sub-a');
    if(surface!=='tray')await expect.poll(()=>page.evaluate(()=> (window as any).__actions.loading())).toBe(false);
   }finally{await page.close();}
  });
  it(`${surface}: an unmounted action reads authoritative truth without emitting stale feedback`,async()=>{
   const page=await open(surface);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    const reads=(await state(page)).reads;
    await page.evaluate(()=> {const f=(window as any).__actions;f.unmount();f.finish();});
    await expect.poll(async()=> (await state(page)).reads).toBeGreaterThan(reads);
    expect(await controls(page).count()).toBe(0);expect((await state(page)).messages).toEqual([]);
    expect((await state(page)).saved.selectionIntent.subscriptionId).toBe('sub-a');
   }finally{await page.close();}
  });
  it(`${surface}: no-target rejection explains the backend gate while keeping current actual exit`,async()=>{
   const page=await open(surface,true);try{
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-exit-write="auto-select-now"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> (window as any).__actions.finish(0,{reject:'AUTO_SELECT_NO_TARGET'}));
    await expect.poll(()=>controls(page).locator('[data-auto-select-feedback]').textContent()).toContain('没有满足切换条件');
    expect((await state(page)).saved.selectedServerId).toBe('node-a');
   }finally{await page.close();}
  });
  it(`${surface}: the shared write lane serializes a rapid manual choice after in-flight Auto`, async () => {
   const page=await open(surface);try {
    await page.evaluate(()=> (window as any).__actions.hold=true);
    await controls(page).locator('[data-auto-select-subscription="sub-a"]').click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    await page.evaluate(()=> {const f=(window as any).__actions;f.manualPending=f.manualWrite();});
    expect((await state(page)).calls).toHaveLength(1);
    await page.evaluate(()=> (window as any).__actions.finish(0));
    await expect.poll(async()=> (await state(page)).calls.length).toBe(2);
    expect((await state(page)).calls[1]).toEqual({cmd:'server_switch',args:{serverId:'node-b'}});
    await page.evaluate(async()=> {const f=(window as any).__actions;f.finish(1);await f.manualPending;f.publish({});});
    await expect.poll(async()=> (await state(page)).config.selectedServerId).toBe('node-b');
    await expect.poll(()=>controls(page).locator('[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed')).toBe('false');
    expect((await state(page)).saved.selectionIntent).toBeUndefined();
    expect(await controls(page).locator('[data-auto-select-feedback]').count()).toBe(0);
   }finally{await page.close();}
  });
  it(`${surface}: status events show backend failure and repeated penalty without frontend timers`,async()=>{
   const page=await open(surface,true);try{
    const panel=page.locator('[data-auto-select-status]');await panel.waitFor();
    await page.evaluate(()=> (window as any).__actions.event({mode:'commitFailed',reason:'commitFailed',barred:[{serverId:'node-b',remainingMs:240000,failures:3}],commit:{consecutiveFailures:2,retryAfterMs:60000,lastFailure:null}}));
    await expect.poll(()=> panel.textContent()).toContain('换点未完成');
    await expect.poll(()=> panel.textContent()).toContain('已因故障换走 3 次');
    expect(await panel.textContent()).toContain('排除期还剩 240 秒');
   }finally{await page.close();}
  });
 }
 for (const [surface,choice] of [['mobile-nodes','node-a'],['mobile-home','node-a'],['mobile-home','__direct__'],['mobile-home','__block__']] as const) {
  it(`${surface}: stale applied manual ${choice} readback preserves window B without A success feedback`, async () => {
   const page=await open(surface,true);try {
    await page.evaluate(()=> {const f=(window as any).__actions;f.hold=true;f.delayNextRead=true;});
    if(surface==='mobile-nodes')await page.locator('[data-exit-write="node-use"]').filter({hasText:'Fixture a'}).click();
    else if(choice==='node-a'){
     if(await page.locator('[data-picker-node="node-a"]').count()===0)await page.locator('[data-group-id="sub-a"]').click();
     await page.locator('[data-picker-node="node-a"] [data-exit-write="home-node-pick"]').click();
    }else await page.locator(`[data-exit-write="home-sentinel-${choice==='__direct__'?'direct':'block'}"]`).click();
    await expect.poll(async()=> (await state(page)).calls.length).toBe(1);
    expect((await state(page)).calls[0]).toEqual({cmd:'server_switch',args:{serverId:choice}});
    await page.evaluate(()=> (window as any).__actions.finish());
    await expect.poll(()=>page.evaluate(()=> (window as any).__actions.readJobs.length)).toBe(1);
    await page.evaluate(()=> (window as any).__actions.manual());
    await expect.poll(async()=> (await state(page)).config.selectedServerId).toBe('node-b');
    await page.evaluate(async()=> {const f=(window as any).__actions;f.readJobs[0].release();await new Promise<void>(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve())));});
    expect((await state(page)).config.selectedServerId).toBe('node-b');
    expect((await state(page)).saved.selectedServerId).toBe('node-b');
    expect((await state(page)).messages).toEqual([]);
   }finally{await page.close();}
  });
 }

});

import path from 'node:path';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { nativeAutoFixture } from '@/contracts/native-auto.test-support';

const root = path.resolve(import.meta.dirname, '../..');
// Exercise the actual host-injected DOM blur route, not a test-only hide helper.
const trayWindowSource = readFileSync(path.resolve(root, '../src-tauri/src/tray/window.rs'), 'utf8');
const trayBlurDismissScript = trayWindowSource.match(/const TRAY_BLUR_DISMISS_JS: &str = r#"([\s\S]*?)"#;/)?.[1];
if (!trayBlurDismissScript) throw new Error('Missing native tray blur dismissal script');
// Execute the real host call-site scripts: geometry and show share a bridge,
// but only the successful-show caller may resume a non-activating warm window.
const trayShowScript = trayWindowSource.match(/pub\(super\) fn show_ready_overlay\([\s\S]*?^}/m)?.[0]
  .match(/win\.eval\("([^"\n]*__POLARIS_TRAY_REMEASURE__[^"\n]*)"\)/)?.[1];
const trayGeometryScript = trayWindowSource.match(/pub\(super\) fn notify_screen_change_remeasure\([\s\S]*?^}/m)?.[0]
  .match(/win\.eval\("([^"\n]*__POLARIS_TRAY_REMEASURE__[^"\n]*)"\)/)?.[1];
if (!trayShowScript || !trayGeometryScript) throw new Error('Missing native tray remeasure call sites');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { NodesScreen } from '/src/components/screens/nodes/NodesScreen';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { HomeScreen } from '/src/components/screens/home/HomeScreen';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import TrayMenu from '/src/tray/TrayMenu';
import { useAppStore } from '/src/store/app-store';
import { configReadGeneration } from '/src/lib/config-read-generation';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import { applyFakeIpTunEntry } from '/src/components/screens/home/fakeip-tun-entry';
import { IPC_CHANNELS as C } from '/src/domain/ipc-channels';
import { setToastImpl } from '/src/lib/error-handler';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
localStorage.setItem('polaris.language','zh-CN');
const params=new URLSearchParams(location.search),surface=params.get('surface');
if(surface.startsWith('mobile'))document.documentElement.dataset.mobileOs='android';
const initial=${JSON.stringify(nativeAutoFixture())};
const servers=['a','b'].map(id=>({id:'node-'+id,name:'Fixture '+id,protocol:'socks',address:'fixture.invalid',port:1,subscriptionId:'sub-'+id}));
let disk={autoSelectEnabled:true,selectedServerId:'node-a',proxyMode:'smart',proxyModeType:'manual',servers,subscriptions:[{id:'sub-a',name:'Subscription A',url:'https://fixture.invalid'},{id:'sub-b',name:'Subscription B',url:'https://fixture.invalid'},{id:'empty',name:'Empty subscription',url:'https://fixture.invalid'}],desktopNotifications:false};
if(params.get('auto')==='1')disk.selectionIntent={mode:'auto',scope:'subscription',subscriptionId:'sub-a'};
let status={...initial,intent:disk.selectionIntent?initial.intent:{mode:'manual',scope:null,subscriptionId:null}};
let id=0;const handlers=new Map(),listeners=[];
let proxyState={running:false,starting:false};
const ipSnapshot={direct:{ip:'198.51.100.1'},proxy:{ip:'203.0.113.1'}};
const jobs=[],readJobs=[];let app;
const f=window.__actions={calls:[],proxyCalls:[],reads:0,jobs,readJobs,messages:[],readFail:false,hold:false,
 state:()=>structuredClone(useAppStore.getState().config),saved:()=>structuredClone(disk),
 cleanReads:0,statusReads:0,ipReads:0,liveReads:0,liveJobs:[],ipJobs:[],patchJobs:[],takeoverJobs:[],hides:[],notifications:[],focusEvents:0,
 configListeners:()=>listeners.filter(l=>l.event===C.EVENT_CONFIG_CHANGED).length,
 nativeConfigSignal(){for(const l of [...listeners])if(l.event===C.EVENT_CONFIG_CHANGED)handlers.get(l.handler)?.({payload:{}});},
 setProxy(value){proxyState={...proxyState,...value};},
 runtime(value){proxyState={...proxyState,...value};useAppStore.setState({proxyStatus:proxyState});},
 setIp(value){Object.assign(ipSnapshot,value);},
 nativeShow(){${trayShowScript}},
 nativeGeometry(){${trayGeometryScript}},
 work:()=>({config:f.reads,status:f.statusReads,ip:f.ipReads,live:f.liveReads}),
 // A task checkpoint drains the released native promise chain; flushSync then
 // commits React state. No delay duration or DOM-disappearance assumption.
 async checkpoint(){await new Promise(resolve=>{const channel=new MessageChannel();channel.port1.onmessage=()=>{channel.port1.close();channel.port2.close();resolve();};channel.port2.postMessage(null);});flushSync(()=>render());},
 armSelectionInterleave(advance){f.selectionPublished=new Promise(resolve=>{const off=useAppStore.subscribe((state,previous)=>{
  if(state.configPublication===previous.configPublication||state.configPublication?.reason!=='selection')return;
  off();f.acceptedGeneration=configReadGeneration.current();
  // Runs after synchronous acceptance, BEFORE switchExit's await continuation.
  queueMicrotask(()=>{f.newGeneration=advance?configReadGeneration.beginRead():configReadGeneration.current();resolve();});
 });});},
 finishPatch(index=0){const job=f.patchJobs[index];disk={...disk,...job.patch};job.resolve({success:true,data:structuredClone(disk)});},
 // Dedicated takeover writes edit the latest saved config and broadcast it,
 // matching config_set_takeover's {config,corrected} response contract.
 finishTakeover(index=0){const job=f.takeoverJobs[index];const result=applyFakeIpTunEntry({...disk,proxyModeType:job.mode});disk=result.config;f.nativeConfigSignal();job.resolve({success:true,data:{config:structuredClone(disk),corrected:result.corrected}});},
 proxySignal(running){proxyState={running,starting:false};for(const l of [...listeners])if(l.event===(running?C.EVENT_PROXY_STARTED:C.EVENT_PROXY_STOPPED))handlers.get(l.handler)?.({payload:{}});},
 stageOnly(){useStagedConfigStore.setState({enabled:true});useStagedConfigStore.getState().stage({id:'server:staged-only',kind:'server',label:'Local draft',entityPath:['servers','staged-only'],nextValue:{id:'staged-only',name:'Local draft',protocol:'socks',address:'draft.invalid',port:1}});},
 staged:()=>useStagedConfigStore.getState().entries,
 stagedHydrated:()=>useStagedConfigStore.getState().hydrated,
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
  if(!options.noSave){if(job.cmd===C.AUTO_SELECT_ENABLE)disk.selectionIntent={mode:'auto',scope:job.args.scope||'subscription',subscriptionId:job.args.subscriptionId};
   if(job.cmd===C.AUTO_SELECT_DISABLE)delete disk.selectionIntent;
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
  if(cmd==='plugin:event|unlisten'){const at=listeners.findIndex(l=>l.id===args.eventId);if(at>=0)listeners.splice(at,1);return Promise.resolve();}
  if([C.AUTO_SELECT_ENABLE,C.AUTO_SELECT_DISABLE,C.AUTO_SELECT_SWITCH_NOW,C.SERVER_SWITCH].includes(cmd)){f.calls.push({cmd,args});return new Promise(resolve=>{jobs.push({cmd,args,resolve});if(!f.hold)f.finish(jobs.length-1);});}
  if(cmd===C.PROXY_START||cmd===C.PROXY_STOP){f.proxyCalls.push(cmd);if(params.has('proxyError'))return Promise.resolve({success:false,error:'synthetic private diagnostic: HELPER_RUNTIME_RESTART_REQUIRED 请重启 /private/fixture-only',code:params.get('proxyError')||undefined});}
  if(cmd===C.TRAY_HIDE){f.hides.push(args);return Promise.resolve({success:true,data:null});}
  if(cmd==='plugin:notification|is_permission_granted')return Promise.resolve(true);
  if(cmd==='plugin:notification|notify'){f.notifications.push(args);return Promise.resolve();}
  if(cmd===C.CONFIG_PATCH)return new Promise(resolve=>{f.patchJobs.push({patch:args.patch,resolve});if(!f.holdPatch)f.finishPatch(f.patchJobs.length-1);});
  if(cmd===C.CONFIG_SET_TAKEOVER)return new Promise(resolve=>{f.takeoverJobs.push({mode:args.mode,resolve});if(!f.holdTakeover)f.finishTakeover(f.takeoverJobs.length-1);});
  if(cmd===C.SERVER_IDENTITY_CONFLICTS)return Promise.resolve({success:true,data:{version:'fixture-config-version',groups:[]}});
  if(cmd===C.CONFIG_GET){f.reads++;if(f.delayNextRead){f.delayNextRead=false;const captured=structuredClone(disk);return new Promise(resolve=>{const release=()=>resolve({success:true,data:captured});f.releaseRead=release;readJobs.push({captured,release});});}return f.readFail?Promise.reject(Error('saved readback failed')):Promise.resolve({success:true,data:structuredClone(disk)});}
  if(cmd===C.AUTO_SELECT_STATUS)return Promise.resolve({success:true,data:structuredClone(status)});
  if(cmd===C.PROXY_GET_STATUS){f.statusReads++;return Promise.resolve({success:true,data:structuredClone(proxyState)});}
  if(cmd===C.APP_TAKE_CLEAN_EXIT_FLAG){f.cleanReads++;return Promise.resolve({success:true,data:false});}
  if(cmd===C.SYSTEM_PROXY_GET_STATUS){f.liveReads++;if(f.holdLive)return new Promise(resolve=>f.liveJobs.push(resolve));return Promise.resolve({success:true,data:{pointsToUs:true}});}
  if(cmd===C.IP_INFO_GET&&args.peek){f.ipReads++;if(f.holdIp)return new Promise(resolve=>f.ipJobs.push(resolve));return Promise.resolve({success:true,data:structuredClone(ipSnapshot)});}
  if(cmd===C.PROXY_GET_PENDING_CHANGES)return Promise.resolve({success:true,data:{needsRestart:false,fields:[],scopes:[]}});
  if(cmd===C.STATS_GET_CONNECTIONS||cmd===C.CONNECTIONS_GET_ALL)return Promise.resolve({success:true,data:[]});
  if(cmd==='plugin:window|inner_size'||cmd==='plugin:window|outer_size')return Promise.resolve({width:600,height:800});
  if(cmd==='plugin:window|scale_factor')return Promise.resolve(1);
  if(cmd==='plugin:window|available_monitors')return Promise.resolve([]);
  return Promise.resolve({success:true,data:null});
 }
};
if(surface==='tray'){${trayBlurDismissScript}}
window.addEventListener('focus',()=>f.focusEvents++);
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(event,key){const at=listeners.findIndex(l=>l.id===key);if(at>=0)listeners.splice(at,1);}};
setToastImpl({success:text=>f.messages.push(['success',text]),info:text=>f.messages.push(['info',text]),warning:text=>f.messages.push(['warning',text]),error:(text,description)=>f.messages.push(['error',text,...(description===undefined?[]:[description])])});
api.config.meshRouteReport=async()=>null;
useStagedConfigStore.setState({enabled:false,hydrated:surface!=='tray',entries:[],baseline:surface==='tray'?null:disk});
useAppStore.setState({config:structuredClone(disk),servers:disk.servers,selectedServerId:disk.selectedServerId,proxyStatus:{running:false,starting:false},invalidNodes:[]});
const components={'desktop-nodes':NodesScreen,'mobile-nodes':MobileNodesScreen,'desktop-home':HomeScreen,'mobile-home':MobileHomeScreen,'tray':TrayMenu};
await import('/src/styles/index.css');if(surface.startsWith('mobile')){await import('/src/styles/tokens.resolved.css');await import('/src/mobile/mobile.css');await import('/src/mobile/redesign.css');await import('/src/mobile/home/home.css');}
const Component=components[surface];const render=()=>app.render(<Component/>);app=createRoot(document.getElementById('root'));render();
`;
let server: ViteDevServer, browser: Browser, origin: string;
const surfaces = ['desktop-nodes', 'mobile-nodes', 'desktop-home', 'mobile-home', 'tray'] as const;
const controls = (page: Page) => page.locator('[data-auto-select-controls][data-auto-select-subscription="sub-a"]');
const pressed = async (page: Page) => await controls(page).count() ? controls(page).locator('button[data-auto-select-subscription="sub-a"]').getAttribute('aria-pressed') : 'false';
const busy = async (page: Page) => await controls(page).count() ? controls(page).getAttribute('aria-busy') : 'false';
const state = (page: Page) => page.evaluate(() => { const f=(window as any).__actions;return {calls:f.calls,reads:f.reads,config:f.state(),saved:f.saved(),messages:f.messages}; });
async function open(surface: typeof surfaces[number], auto = false) {
  const page=await browser.newPage({viewport:{width:surface.startsWith('mobile')?390:1100,height:844}});page.setDefaultTimeout(7000);
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto(`${origin}/__actions?surface=${surface}&auto=${auto?1:0}`);
  if(surface==='desktop-home')await page.locator('#node-trigger').click();
  if(surface==='mobile-home')await page.locator('[data-write-control="switch-node"], [data-home-action="switch-node"]').first().click();
  if(surface==='tray')await page.getByText('全部节点',{exact:true}).click();
  // Stopped Auto has no confirmed actual node, so nodes screens may correctly
  // start on All. Navigate to the real subscription tab before group actions.
  if(surface==='desktop-nodes')await page.locator('[data-act="sub-tab"][data-v="sub-a"]').click();
  if(surface==='mobile-nodes')await page.locator('.mn-seg').filter({has:page.getByText('Subscription A',{exact:true})}).click();
  if(!await controls(page).count() && ['desktop-home','mobile-home','tray'].includes(surface)) {
    const group=page.getByText('Subscription A',{exact:true});if(await group.count())await group.first().click();
  }
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
 for (const [code, expected] of [
  ['HELPER_RUNTIME_RESTART_REQUIRED', '本次切换到本地运行需要重启 Polaris。请正常退出并重新打开应用，再尝试本地运行。'],
  ['STARTUP_FAILED', 'sing-box 启动失败，请检查服务器配置'],
  ['FUTURE_UNKNOWN_ERROR', 'sing-box 启动失败，请检查服务器配置'],
  ['', 'sing-box 启动失败，请检查服务器配置'],
 ] as const) {
  it(`desktop-home helper restart projection: ${code || 'missing code'}`, async () => {
   const page = await browser.newPage({ viewport: { width: 1100, height: 844 } });
   try {
    await page.goto(`${origin}/__actions?surface=desktop-home&proxyError=${code}`);
    await page.locator('#connect-btn').click();
    await expect.poll(() => page.evaluate(() => (window as any).__actions.messages)).toEqual([
     ['error', '连接失败', expected],
    ]);
    const observed = await page.evaluate(() => { const f = (window as any).__actions;
     return { proxyCalls: f.proxyCalls, writes: f.calls, notifications: f.notifications, messages: f.messages }; });
    expect(observed.proxyCalls).toEqual(['proxy_start']);
    expect(observed.writes).toEqual([]);
    expect(observed.notifications).toEqual([]);
    expect(JSON.stringify(observed.messages)).not.toContain('synthetic private diagnostic');
    expect(JSON.stringify(observed.messages)).not.toContain('/private/fixture-only');
    // The existing error button remains a start/retry; this projection performs no relaunch/reset/stop.
    expect(await page.locator('#connect-btn').isEnabled()).toBe(true);
    expect(await page.locator('#connect-btn').getAttribute('class')).toContain('err');
   } finally { await page.close(); }
  });
 }
 for(const surface of surfaces){
  it(`${surface}: Auto choice preserves real nodes and has no separate mode switch`,async()=>{
    const page=await open(surface);try{
      expect(await page.locator('[data-auto-select-controls][data-auto-select-subscription="empty"]').count()).toBe(0);
      expect(await controls(page).locator('[role="switch"]').count()).toBe(0);
      await controls(page).locator('[data-exit-write="auto-select-enable"]').click();
      await expect.poll(async()=>(await state(page)).saved.selectionIntent?.subscriptionId).toBe('sub-a');
      expect((await state(page)).saved.servers).toHaveLength(2);
    }finally{await page.close();}
  });
  it(`${surface}: rapid clicks share one authoritative write and never select optimistically`,async()=>{
    const page=await open(surface);try{
      await page.evaluate(()=>{const f=(window as any).__actions;f.hold=true;for(let i=0;i<8;i++)document.querySelector<HTMLButtonElement>('[data-auto-select-controls][data-auto-select-subscription="sub-a"] [data-exit-write="auto-select-enable"]')!.click();});
      await expect.poll(async()=>(await state(page)).calls.length).toBe(1);
      expect((await state(page)).config.selectionIntent).toBeUndefined();expect(await busy(page)).toBe('true');
      await page.evaluate(()=>(window as any).__actions.finish());
      await expect.poll(()=>pressed(page)).toBe('true');
    }finally{await page.close();}
  });
  it(`${surface}: failed readback cannot claim a saved reference`,async()=>{
    const page=await open(surface);try{
      await page.evaluate(()=>(window as any).__actions.readFail=true);
      await controls(page).locator('[data-exit-write="auto-select-enable"]').click();
      await expect.poll(()=>busy(page)).toBe('false');
      expect((await state(page)).config.selectionIntent).toBeUndefined();
      expect(await controls(page).locator('[data-auto-select-feedback="error"]').count()).toBe(1);
    }finally{await page.close();}
  });
  it(`${surface}: newer saved manual selection beats a delayed Auto readback`,async()=>{
    const page=await open(surface);try{
      await page.evaluate(()=>{const f=(window as any).__actions;f.hold=true;f.delayNextRead=true;});
      await controls(page).locator('[data-exit-write="auto-select-enable"]').click();
      await page.evaluate(()=>(window as any).__actions.finish());
      await page.waitForFunction(()=>(window as any).__actions.readJobs.length===1);
      await page.evaluate(()=>{const f=(window as any).__actions;f.manual();f.releaseRead();});
      await expect.poll(async()=>(await state(page)).config.selectedServerId).toBe('node-b');
      expect((await state(page)).config.selectionIntent).toBeUndefined();
    }finally{await page.close();}
  });
  it(`${surface}: actual group node follows confirmed native events only`,async()=>{
    const page=await open(surface,true);try{
      await page.evaluate(()=>{ const f=(window as any).__actions; f.runtime({running:true,starting:false,mainGeneration:1}); f.event({generation:1,nativeAutoGroups:[{target:{scope:'subscription',subscriptionId:'sub-a'},actualServerId:'node-a',confirmed:true,state:'auto'}]}); });
      await expect.poll(()=>controls(page).locator('[data-auto-select-actual]').textContent()).toContain('Fixture a');
      await page.evaluate(()=>(window as any).__actions.runtime({running:true,starting:false,mainGeneration:2}));
      await expect.poll(()=>controls(page).locator('[data-auto-select-actual]').textContent()).not.toContain('Fixture a');
      await page.evaluate(()=>(window as any).__actions.event({generation:2}));
      await expect.poll(()=>controls(page).locator('[data-auto-select-actual]').textContent()).toContain('Fixture a');
      await page.evaluate(()=>(window as any).__actions.event({nativeAutoGroups:[{target:{scope:'subscription',subscriptionId:'sub-a'},actualServerId:'node-b',confirmed:true,state:'auto'}]}));
      await expect.poll(()=>controls(page).locator('[data-auto-select-actual]').textContent()).not.toContain('Fixture b');
    }finally{await page.close();}
  });
  it(`${surface}: master OFF hides entries without rewriting concurrent group references in UI`,async()=>{
    const page=await open(surface,true);try{
      await page.evaluate(()=>(window as any).__actions.publish({autoSelectEnabled:false,autoSelectPins:[{target:{scope:'subscription',subscriptionId:'sub-a'},serverId:'node-a'}]}));
      await expect.poll(()=>page.locator('[data-auto-select-controls]').count()).toBe(0);
      expect((await state(page)).saved.selectionIntent.subscriptionId).toBe('sub-a');
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

 it('independent main/tray realms each register and read once per signal, with no staged/gate leakage',async()=>{
  const context=await browser.newContext();const main=await context.newPage(),tray=await context.newPage();
  try{
   await main.goto(`${origin}/__actions?surface=mobile-home`);
   await expect.poll(()=>main.evaluate(()=>(window as any).__actions?.configListeners())).toBe(1);
   await expect.poll(()=>main.evaluate(()=>(window as any).__actions?.reads)).toBe(1);
   await main.evaluate(()=>(window as any).__actions.stageOnly());
   await expect.poll(()=>main.evaluate(()=>(window as any).__actions.staged().length)).toBe(1);
   await tray.goto(`${origin}/__actions?surface=tray`);
   await expect.poll(()=>tray.evaluate(()=>(window as any).__actions?.configListeners())).toBe(1);
   await expect.poll(()=>tray.evaluate(()=>(window as any).__actions?.reads)).toBe(1);
   expect(await tray.evaluate(()=>(window as any).__actions.cleanReads)).toBe(0);
   expect(await tray.evaluate(()=>(window as any).__actions.stagedHydrated())).toBe(false);
   expect(await tray.evaluate(()=>(window as any).__actions.staged())).toEqual([]);
   for(const page of [main,tray])await page.evaluate(()=>{const f=(window as any).__actions;f.changeDisk({proxyMode:'global'});f.nativeConfigSignal();});
   for(const page of [main,tray]){
    await expect.poll(async()=>(await state(page)).config.proxyMode).toBe('global');
    expect((await state(page)).reads).toBe(2);
    expect((await state(page)).config.servers.some((server:any)=>server.id==='staged-only')).toBe(false);
   }
   await main.evaluate(()=>(window as any).__actions.unmount());
   await expect.poll(()=>main.evaluate(()=>(window as any).__actions.configListeners())).toBe(0);
   await tray.evaluate(()=>{const f=(window as any).__actions;f.changeDisk({proxyMode:'direct'});f.nativeConfigSignal();});
   await expect.poll(async()=>(await state(tray)).config.proxyMode).toBe('direct');
   expect((await state(tray)).reads).toBe(3);expect((await state(main)).reads).toBe(2);
  }finally{await context.close();}
 });



 for(const reason of ['geometry','legacy','unknown'] as const)it(`hidden warm tray ${reason} remeasure stays retired; explicit native show works without focus`,async()=>{
  const page=await open('tray');try{
   await page.evaluate(async()=>{const f=(window as any).__actions;await f.checkpoint();window.dispatchEvent(new Event('blur'));await f.checkpoint();});
   const before=await page.evaluate(()=>{const f=(window as any).__actions;return {work:f.work(),focus:f.focusEvents};});
   await page.evaluate(async reason=>{const f=(window as any).__actions;f.changeDisk({proxyMode:'global',uiTheme:'light'});f.setProxy({running:true});f.setIp({proxy:{ip:'203.0.113.25'}});
    const bridge=(window as any).__POLARIS_TRAY_REMEASURE__;if(reason==='geometry')f.nativeGeometry();else if(reason==='legacy')bridge();else bridge('not-a-show');await f.checkpoint();},reason);
   expect(await page.evaluate(()=>(window as any).__actions.work())).toEqual(before.work);
   expect(await page.locator('.tray-back').count()).toBe(1);
   expect((await state(page)).config.proxyMode).toBe('smart');
   expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual([{reason:'blur'}]);
   await page.evaluate(()=>(window as any).__actions.nativeShow());
   await expect.poll(()=>page.locator('.tray-status-detail').textContent()).toContain('203.0.113.25');
   expect((await state(page)).config.proxyMode).toBe('global');
   expect(await page.locator('.tray-back').count()).toBe(0);
   const after=await page.evaluate(()=>{const f=(window as any).__actions;return {work:f.work(),focus:f.focusEvents,listeners:f.configListeners()};});
   expect(after.work.config).toBeGreaterThan(before.work.config);expect(after.work.status).toBeGreaterThan(before.work.status);
   expect(after.focus).toBe(before.focus);expect(after.listeners).toBe(1);
  }finally{await page.close();}
 });

 for(const reason of ['geometry','legacy','unknown'] as const)it(`visible nodes ${reason} remeasure preserves notice, submenu and pending action ownership`,async()=>{
  const page=await open('tray');try{
   await page.evaluate(async()=>{const f=(window as any).__actions;await f.checkpoint();f.hold=true;});
   await page.getByRole('button',{name:'直连',exact:true}).click();
   await expect.poll(async()=>(await state(page)).calls.length).toBe(1);
   await page.evaluate(async()=>{const f=(window as any).__actions;f.finish(0,{status:'pending'});await f.checkpoint();});
   const notice=await page.locator('.tray-note').textContent();expect(notice).toBeTruthy();
   await page.getByRole('button',{name:'直连',exact:true}).click();
   await expect.poll(async()=>(await state(page)).calls.length).toBe(2);
   const before=await page.evaluate(()=>(window as any).__actions.work());
   await page.evaluate(async reason=>{const f=(window as any).__actions,bridge=(window as any).__POLARIS_TRAY_REMEASURE__;if(reason==='geometry')f.nativeGeometry();else if(reason==='legacy')bridge();else bridge({reason:'show'});await f.checkpoint();},reason);
   expect(await page.evaluate(()=>(window as any).__actions.work())).toEqual(before);
   expect(await page.locator('.tray-back').count()).toBe(1);
   expect(await page.locator('.tray-note').textContent()).toBe(notice);
   expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual([]);
   await page.evaluate(async()=>{const f=(window as any).__actions;f.finish(1,{status:'applied'});await f.checkpoint();});
   expect(await page.evaluate(()=>(window as any).__actions.hides.length)).toBe(1);
   expect(await page.locator('.tray-back').count()).toBe(0);
   expect((await state(page)).calls).toHaveLength(2);
  }finally{await page.close();}
 });

 it('real focus remains an independent fresh interaction after blur',async()=>{
  const page=await open('tray');try{
   await page.evaluate(async()=>{const f=(window as any).__actions;await f.checkpoint();window.dispatchEvent(new Event('blur'));await f.checkpoint();});
   const before=await page.evaluate(()=>{const f=(window as any).__actions;return {reads:f.reads,focus:f.focusEvents};});
   await page.evaluate(()=>{const f=(window as any).__actions;f.changeDisk({proxyMode:'global'});f.setIp({direct:{ip:'198.51.100.25'}});window.dispatchEvent(new Event('focus'));});
   await expect.poll(()=>page.locator('.tray-status-detail').textContent()).toContain('198.51.100.25');
   expect((await state(page)).config.proxyMode).toBe('global');expect((await state(page)).reads).toBeGreaterThan(before.reads);
   expect(await page.evaluate(()=>(window as any).__actions.focusEvents)).toBe(before.focus+1);
   expect(await page.locator('.tray-back').count()).toBe(0);
  }finally{await page.close();}
 });

 for(const held of ['live','ip'] as const)it(`warm tray native blur retires delayed ${held} and native show refreshes again`,async()=>{
  const page=await open('tray');try{
   await page.evaluate(held=>{const f=(window as any).__actions;f.holdLive=held==='live';f.holdIp=held==='ip';f.setProxy({running:true});f.changeDisk({proxyModeType:held==='live'?'systemProxy':'manual'});f.nativeConfigSignal();},held);
   await expect.poll(()=>page.evaluate(held=>(window as any).__actions[held==='live'?'liveJobs':'ipJobs'].length,held)).toBeGreaterThan(0);
   await page.evaluate(()=>(window as any).__actions.checkpoint());
   const status=await page.locator('.tray-status b').textContent(),detail=await page.locator('.tray-status-detail').textContent();
   await page.evaluate(async held=>{const f=(window as any).__actions;window.dispatchEvent(new Event('blur'));
    for(const resolve of f[held==='live'?'liveJobs':'ipJobs'])resolve({success:true,data:held==='live'?{pointsToUs:false}:{proxy:{ip:'203.0.113.99'},direct:{ip:'198.51.100.99'}}});await f.checkpoint();},held);
   expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual([{reason:'blur'}]);
   expect(await page.locator('.tray-status b').textContent()).toBe(status);
   expect(await page.locator('.tray-status-detail').textContent()).toBe(detail);
   expect(await page.evaluate(()=>(window as any).__actions.configListeners())).toBe(1);
   const reads=(await state(page)).reads;
   await page.evaluate(()=>{const f=(window as any).__actions;f.holdLive=false;f.holdIp=false;f.setProxy({running:false});f.changeDisk({proxyModeType:'manual',uiTheme:'light'});f.setIp({direct:{ip:'198.51.100.2'}});(window as any).__actions.nativeShow();});
   await expect.poll(()=>page.locator('.tray-status-detail').textContent()).toContain('198.51.100.2');
   expect(await page.locator('.tray-status').getAttribute('class')).toContain('stopped');
   expect(await page.locator('html').getAttribute('data-theme')).toBe('light');
   expect((await state(page)).reads).toBeGreaterThan(reads);
   expect(await page.evaluate(()=>(window as any).__actions.configListeners())).toBe(1);
  }finally{await page.close();}
 });

 for(const receipt of ['pending','applied'] as const)for(const reopen of [false,true])it(`warm tray native blur suppresses old ${receipt} selection feedback, reopen=${reopen}`,async()=>{
  const page=await open('tray');try{
   await page.evaluate(()=>{const f=(window as any).__actions;f.hold=true;f.armSelectionInterleave(false);});
   await page.getByRole('button',{name:'直连',exact:true}).click();
   await expect.poll(async()=>(await state(page)).calls.length).toBe(1);
   await page.evaluate(reopen=>{window.dispatchEvent(new Event('blur'));if(reopen)(window as any).__actions.nativeShow();},reopen);
   // Reopening must finish its fresh read BEFORE settling the old action, so
   // suppression is owned by the interaction fence, not by a stale read token.
   if(reopen)await expect.poll(()=>page.evaluate(()=>(window as any).__actions.loading())).toBe(false);
   await page.evaluate(async receipt=>{const f=(window as any).__actions;f.finish(0,{status:receipt});await f.selectionPublished;await f.checkpoint();},receipt);
   expect((await state(page)).config.selectedServerId).toBe('__direct__');
   expect(await page.locator('.tray-note').count()).toBe(0);
   expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual([{reason:'blur'}]);
   expect(await page.evaluate(()=>(window as any).__actions.notifications)).toEqual([]);
   expect(await page.locator('.tray-back').count()).toBe(reopen?0:1);
   expect((await state(page)).calls).toHaveLength(1);
  }finally{await page.close();}
 });

 for(const [dismiss,reopen] of [[false,false],[true,false],[true,true]] as const)it(`warm tray FakeIP correction feedback owns its interaction, dismiss=${dismiss} reopen=${reopen}`,async()=>{
  const page=await open('tray');try{
   await page.locator('.tray-back').click();
   await page.evaluate(()=>{const f=(window as any).__actions;f.holdTakeover=true;f.changeDisk({desktopNotifications:true,dnsConfig:{enableFakeIp:false,fakeIpTunAutoEnable:true}});f.nativeConfigSignal();});
   await expect.poll(async()=>(await state(page)).config.desktopNotifications).toBe(true);
   await page.getByRole('button',{name:'TUN 模式',exact:true}).click();
   await expect.poll(()=>page.evaluate(()=>(window as any).__actions.takeoverJobs.length)).toBe(1);
   expect(await page.evaluate(()=>(window as any).__actions.takeoverJobs.map((job:any)=>job.mode))).toEqual(['tun']);
   await page.evaluate(async({dismiss,reopen})=>{const f=(window as any).__actions;if(dismiss)window.dispatchEvent(new Event('blur'));if(reopen)(window as any).__actions.nativeShow();await f.checkpoint();f.finishTakeover();await f.checkpoint();},{dismiss,reopen});
   await expect.poll(async()=>(await state(page)).config.dnsConfig.enableFakeIp).toBe(true);
   expect(await page.locator('.tray-note').count()).toBe(dismiss?0:1);
   expect(await page.evaluate(()=>(window as any).__actions.notifications.length)).toBe(dismiss?0:1);
   expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual(dismiss?[{reason:'blur'}]:[]);
   expect(await page.evaluate(()=>(window as any).__actions.takeoverJobs.length)).toBe(1);
  }finally{await page.close();}
 });

 // Exercise visible interactions, not the state representation used to retain
 // group choices. Config publications must preserve both open and closed groups.
 for(const surface of ['mobile-home','tray'] as const) {
  const groupHeader=(page:Page,label:string)=>page.locator(surface==='tray'?'.tray-group-h':'[data-group-id]')
    .filter({has:page.getByText(label,{exact:true})});
  it(`${surface}: group choices survive config refresh and seed newly arriving groups`,async()=>{
   const page=await open(surface);try{
    const a=groupHeader(page,'Subscription A'),b=groupHeader(page,'Subscription B');
    await a.click();await b.click();
    for(const selectedServerId of ['node-b','node-a']) {
     await page.evaluate(async selectedServerId=>{const f=(window as any).__actions;f.publish({selectedServerId});await f.checkpoint();},selectedServerId);
     expect(await a.getAttribute('aria-expanded')).toBe('false');
     expect(await b.getAttribute('aria-expanded')).toBe('true');
     expect(await controls(page).count()).toBe(0);
    }
    await page.evaluate(async()=>{const f=(window as any).__actions,saved=f.saved();
     f.publish({selectedServerId:'node-c',servers:[...saved.servers,{id:'node-c',name:'Fixture c',protocol:'socks',address:'fixture.invalid',port:1,subscriptionId:'sub-c'}],
      subscriptions:[...saved.subscriptions,{id:'sub-c',name:'Subscription C',url:'https://fixture.invalid'}]});await f.checkpoint();
    });
    expect(await a.getAttribute('aria-expanded')).toBe('false');
    expect(await b.getAttribute('aria-expanded')).toBe('true');
    await expect.poll(()=>groupHeader(page,'Subscription C').getAttribute('aria-expanded')).toBe('true');
    expect(await page.locator('[data-auto-select-controls][data-auto-select-subscription="sub-c"]').count()).toBe(1);
    expect((await state(page)).calls).toHaveLength(0);
   }finally{await page.close();}
  });
  it(`${surface}: a new picker opening restores current group defaults`,async()=>{
   const page=await open(surface);try{
    const a=groupHeader(page,'Subscription A'),b=groupHeader(page,'Subscription B');
    await a.click();await b.click();
    const reopen=async()=>{
     if(surface==='tray') {
      await page.evaluate(async()=>{const f=(window as any).__actions;window.dispatchEvent(new Event('blur'));await f.checkpoint();f.nativeShow();await f.checkpoint();});
      await expect.poll(()=>page.locator('.tray-back').count()).toBe(0);
      await page.getByText('全部节点',{exact:true}).click();
     } else {
      await page.locator('.h-sheethead').getByRole('button',{name:'关闭',exact:true}).click();
      expect(await page.locator('.h-picklist').count()).toBe(0);
      await page.locator('[data-write-control="switch-node"], [data-home-action="switch-node"]').first().click();
     }
    };
    await reopen();
    await expect.poll(()=>a.getAttribute('aria-expanded')).toBe('true');
    expect(await b.getAttribute('aria-expanded')).toBe('false');
    // A later opening uses current saved truth, not the first opening's seed.
    await page.evaluate(async()=>{const f=(window as any).__actions;f.publish({selectedServerId:'node-b'});await f.checkpoint();});
    await reopen();
    await expect.poll(()=>b.getAttribute('aria-expanded')).toBe('true');
    expect(await a.getAttribute('aria-expanded')).toBe('false');
    expect((await state(page)).calls).toHaveLength(0);
    if(surface==='tray')expect(await page.evaluate(()=>(window as any).__actions.hides)).toEqual([{reason:'blur'},{reason:'blur'}]);
   }finally{await page.close();}
  });
 }

 for(const receipt of ['pending','applied'] as const)for(const advance of [false,true])it(`tray accepted ${receipt} selection checks its new publication generation, interleave=${advance}`,async()=>{
  const page=await open('tray');try{
   await page.evaluate(advance=>{const f=(window as any).__actions;f.hold=true;f.armSelectionInterleave(advance);},advance);
   await page.getByRole('button',{name:'直连',exact:true}).click();
   await expect.poll(async()=>(await state(page)).calls.length).toBe(1);
   await page.evaluate(async receipt=>{const f=(window as any).__actions;f.finish(0,{status:receipt});await f.selectionPublished;await f.checkpoint();},receipt);
   const generations=await page.evaluate(()=>{const f=(window as any).__actions;return [f.acceptedGeneration,f.newGeneration];});
   expect(generations[1]).toBe(generations[0]+(advance?1:0));
   expect((await state(page)).config.selectedServerId).toBe('__direct__');
   expect(await page.locator('.tray-note').count()).toBe(!advance&&receipt==='pending'?1:0);
   expect(await page.evaluate(()=>(window as any).__actions.hides.length)).toBe(!advance&&receipt==='applied'?1:0);
   expect(await page.locator('.tray-back').count()).toBe(!advance&&receipt==='applied'?0:1);
   expect((await state(page)).calls).toHaveLength(1);
  }finally{await page.close();}
 });

 it('tray late system-proxy-live response cannot restore an older connected state or theme',async()=>{
  const page=await open('tray');try{
   await page.evaluate(()=>{const f=(window as any).__actions;f.holdLive=true;f.setProxy({running:true});f.changeDisk({proxyModeType:'systemProxy',uiTheme:'dark'});f.nativeConfigSignal();});
   await expect.poll(()=>page.evaluate(()=>(window as any).__actions.liveJobs.length)).toBeGreaterThan(0);
   await page.evaluate(()=>{const f=(window as any).__actions;f.changeDisk({proxyModeType:'manual',uiTheme:'light'});f.proxySignal(false);f.nativeConfigSignal();});
   await expect.poll(()=>page.locator('.tray-status').getAttribute('class')).toContain('stopped');
   await expect.poll(()=>page.locator('html').getAttribute('data-theme')).toBe('light');
   await page.evaluate(()=>{const f=(window as any).__actions;for(const resolve of f.liveJobs)resolve({success:true,data:{pointsToUs:false}});});
   expect(await page.locator('.tray-status').getAttribute('class')).toContain('stopped');
   expect(await page.locator('html').getAttribute('data-theme')).toBe('light');
   await expect.poll(()=>page.locator('.tray-status-detail').textContent()).toContain('198.51.100.1');
  }finally{await page.close();}
 });

 it('tray late IP peek cannot overwrite the newer stopped/direct IP session',async()=>{
  const page=await open('tray');try{
   await page.evaluate(()=>{const f=(window as any).__actions;f.holdIp=true;f.proxySignal(true);});
   await expect.poll(()=>page.evaluate(()=>(window as any).__actions.ipJobs.length)).toBeGreaterThan(0);
   await page.evaluate(()=>{const f=(window as any).__actions;f.holdIp=false;f.proxySignal(false);});
   await expect.poll(()=>page.locator('.tray-status-detail').textContent()).toContain('198.51.100.1');
   await page.evaluate(()=>{const f=(window as any).__actions;for(const resolve of f.ipJobs)resolve({success:true,data:{proxy:{ip:'203.0.113.99'},direct:{ip:'198.51.100.99'}}});});
   expect(await page.locator('.tray-status-detail').textContent()).not.toContain('203.0.113.99');
   expect(await page.locator('.tray-status-detail').textContent()).toContain('198.51.100.1');
   expect(await page.locator('.tray-status').getAttribute('class')).toContain('stopped');
  }finally{await page.close();}
 });

 for (const surface of ['desktop-home', 'mobile-home', 'tray'] as const) {
  it(`${surface}: global Auto is before groups and writes explicit global scope`, async () => {
   const page = await open(surface); try {
    const global = page.locator('[data-auto-select-controls][data-auto-select-scope="global"]');
    expect(await page.locator('[data-auto-select-controls]').first().getAttribute('data-auto-select-scope')).toBe('global');
    await global.locator('[data-exit-write="auto-select-enable"]').click();
    await expect.poll(async () => (await state(page)).saved.selectionIntent?.scope).toBe('global');
    expect((await state(page)).calls[0]).toEqual({cmd:'auto_select_enable',args:{scope:'global'}});
    expect((await state(page)).saved.servers).toHaveLength(2);
   } finally { await page.close(); }
  });
 }
 for (const surface of ['desktop-nodes', 'mobile-nodes'] as const) {
  it(`${surface}: All tab is first and Auto is the first card`, async () => {
   const page = await open(surface); try {
    const tabs = page.locator(surface === 'desktop-nodes' ? '.sub-tabs > button' : '.mn-segs > button');
    expect(await tabs.first().textContent()).toContain('全部');
    await tabs.first().click();
    const global = page.locator('[data-auto-select-controls][data-auto-select-scope="global"]');
    await global.waitFor();
    expect(await global.evaluate(el => el.closest('.auto-choice')?.parentElement?.firstElementChild === el || el.parentElement?.parentElement?.firstElementChild === el.parentElement)).toBe(true);
   } finally { await page.close(); }
  });
 }

});

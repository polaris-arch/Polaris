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
import { SheetHeading } from '/src/mobile/SheetHeading';
import { useMobileSpeedTestStore as store } from '/src/mobile/use-mobile-speed-test';
import { useNodeSortStore as sortStore } from '/src/store/use-node-sort-store';
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
const extra=[
 {id:'slow',name:'Slow fixture',protocol:'socks',address:'slow.invalid',port:5},
 {id:'fast',name:'Fast fixture',protocol:'socks',address:'fast.invalid',port:6},
 {id:'zero',name:'Zero fixture',protocol:'socks',address:'zero.invalid',port:7},
 {id:'stale',name:'Stale fixture',protocol:'socks',address:'stale.invalid',port:8},
 {id:'fail',name:'Failed fixture',protocol:'socks',address:'fail.invalid',port:9},
 {id:'unknown',name:'Unknown fixture',protocol:'socks',address:'unknown.invalid',port:10},
 ...Array.from({length:14},(_,i)=>({id:'filler-'+i,name:'Filler '+i,protocol:'socks',address:'filler.invalid',port:20+i})),
];
const latencies={manual:80,orphan:20,slow:400,fast:3,zero:0,stale:1,fail:-1};
const test=window.__pickerTest={picked:null,store,sortStore,showExtra:()=>{}};
function App(){
 const [query,setQuery]=useState('');const [selected,setSelected]=useState('sub-a');const [open,setOpen]=useState(true);const [screen,setScreen]=useState('home');const [rich,setRich]=useState(false);
 const sortByLatency=sortStore(state=>state.sortByLatency);test.showExtra=()=>setRich(true);
 const rows=[...servers,...(rich?extra:[])].map(server=>({server,isCurrent:server.id===selected,protocolLabel:server.protocol,stagedOnly:false,latencyMs:latencies[server.id],latencyStale:server.id==='stale'}));
 return <main className="mobile-root m-shell">
  <button onClick={()=>setOpen(value=>!value)}>开关列表</button>
  <button onClick={()=>setSelected(null)}>无当前节点</button>
  <button onClick={()=>setScreen(value=>value==='home'?'nodes':'home')}>切换页面</button>
  {open&&<div className="h-sheet h-picker-sheet" role="dialog" aria-label="选择节点">
   <SheetHeading title="选择节点" closeLabel="关闭" onClose={()=>setOpen(false)} className="h-sheethead" titleClassName="h-sheettitle"/>
   <div className="h-picker-search">
    <input className="h-search" aria-label="搜索节点" value={query} onChange={e=>setQuery(e.target.value)}/>
    <button type="button" className="h-picker-sort" role="switch" aria-checked={sortByLatency} aria-label={t('home.sortByLatency')} onClick={()=>sortStore.getState().toggleSortByLatency()}>
     <svg viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true"><path d="M3 5h14M3 10h10M3 15h6"/></svg><span>{t('home.latencyShort')}</span>
    </button>
   </div>
   <div className="h-picker-scroll">
    <ul className="h-picklist"><li><button className="h-pickrow">直连</button></li><li><button className="h-pickrow">阻断</button></li></ul>
    <HomeNodePickerList rows={rows} subscriptions={subscriptions} query={query} sortByLatency={sortByLatency} t={t} onUseAsExit={server=>test.picked=server.id}/>
   </div>
  </div>}
  <div data-speed-screen={screen} key={screen}><MobileSpeedTestProgress/></div>
 </main>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;
const fullHomeEntry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MobileShell } from '/src/mobile/MobileShell';
import { HomeScreenView } from '/src/mobile/home/HomeScreenView';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/home/home.css';
import '/src/mobile/redesign.css';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const t=(key,opts)=>i18n.t(key,opts);
const noop=()=>{};
const servers=[
 ...Array.from({length:22},(_,i)=>({id:'manual-'+i,name:'Manual node '+i,protocol:'socks',address:'manual.invalid',port:100+i})),
 {id:'sub-a',name:'Selected fixture',protocol:'vmess',address:'selected.invalid',port:443,subscriptionId:'first'},
];
function App(){
 const [query,setQuery]=useState('');const [open,setOpen]=useState(true);
 const props={t,writeErrors:{},core:'running',connState:'connected',
  node:{kind:'node',name:'Selected fixture',protocolLabel:'VMess',flagSrc:null,latencyMs:24,latencyStale:false},
  onToggleConnect:noop,onNetworkCheck:noop,
  latencyCheck:{busyKind:null,error:null,feedback:null,blocked:null,blockedStatus:'mobileHome.notApplicable',allUnavailable:null,onRunCurrent:noop,onRunAll:noop},
  tsExitWarning:'none',onTsExitAction:noop,pickerOpen:open,onOpenPicker:()=>setOpen(true),onClosePicker:()=>setOpen(false),
  pickerQuery:query,onPickerQuery:setQuery,
  pickRows:servers.map(server=>({server,isCurrent:server.id==='sub-a',stagedOnly:false,latencyMs:server.id==='sub-a'?24:undefined,latencyStale:false,protocolLabel:server.protocol})),
  pickerSubscriptions:[{id:'first',name:'First subscription'}],onUseAsExit:noop,onPickSentinel:noop,blockDisabledReason:null,
  noServers:false,onAddServer:noop,onAddSubscription:noop,routing:'smart',reverseRouting:false,
  exitRegion:{kind:'flag',code:'hk'},exitFlagSrc:null,exitIsDirect:false,exitProbing:false,onSetRouting:noop,exitIp:null,
  unlock:[],unlockCheckedLabel:'mobileHome.checkedJustNow',unlockRunning:false,unlockDetailId:null,onOpenUnlockDetail:noop,onCloseUnlockDetail:noop,
  samples:[],composition:[],ruleHits:[],hosts:[],hostsMasked:false,hostQuery:'',onHostQuery:noop,
  ruleSubject:null,ruleSubjectView:'menu',onOpenRuleSubject:noop,onCloseRuleSubject:noop,onRuleSubjectView:noop,onQuickRule:noop,
  newRuleAction:'proxy',onNewRuleAction:noop,newRuleRemarks:'',onNewRuleRemarks:noop,ruleRemarksHint:'home.ruleRemarks',onCreateRule:noop,
  appendQuery:'',onAppendQuery:noop,appendTargets:[],onAppendToRule:noop,
  windowBytes:0,windowConnections:0,kernelConnections:0};
 return <MobileShell active="home" onSelect={noop}><HomeScreenView {...props}/></MobileShell>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;
const speedFeedbackEntry = `
import { useHomeSpeedTest } from '/src/mobile/home/use-home-speed-test';
import { api } from '/src/ipc';
${fullHomeEntry.replace(
  "const [query,setQuery]=useState('');const [open,setOpen]=useState(true);",
  "const [query,setQuery]=useState('');const [open,setOpen]=useState(false);const speed=useHomeSpeedTest({servers,diskServers:servers,selectedId:'sub-a',running:true,routing:'smart'});window.__speedFeedback.view=()=>speed.view;",
).replace(
  "latencyCheck:{busyKind:null,error:null,feedback:null,blocked:null,blockedStatus:'mobileHome.notApplicable',allUnavailable:null,onRunCurrent:noop,onRunAll:noop}",
  'latencyCheck:speed.view',
).replace(
  'function App(){',
  "const pending=[];const control=window.__speedFeedback={calls:0,resolve:receipt=>pending.shift()?.resolve(receipt),view:()=>null};api.server.speedTest=(ids)=>new Promise(resolve=>{control.calls++;pending.push({ids,resolve});});function App(){",
)}
`;
const sentinelEntry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import { useAppStore } from '/src/store/app-store';
import { api } from '/src/ipc';
import { configApi } from '/src/ipc/api/config';
import { windowApi } from '/src/ipc/api/system';
import { setToastImpl } from '/src/lib/error-handler';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/home/home.css';
import '/src/mobile/redesign.css';
await i18nReady;
i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const config={selectedServerId:'node-a',proxyMode:'smart',servers:[{id:'node-a',name:'Fixture node',protocol:'socks',address:'fixture.invalid',port:1}],subscriptions:[]};
const fixture=window.__sentinelTest={calls:[],messages:[],setRouting(mode){useAppStore.setState(s=>({config:{...s.config,proxyMode:mode}}));}};
api.stats.onStatsUpdated=()=>()=>{};
api.stats.onConnectionsDetail=()=>()=>{};
api.stats.subscribe=async()=>{};
api.stats.unsubscribe=async()=>{};
configApi.get=async()=>useAppStore.getState().config;
configApi.onChanged=()=>()=>{};
windowApi.startupConfigFlags=async()=>({});
setToastImpl({success:text=>fixture.messages.push(['success',text]),info:text=>fixture.messages.push(['info',text]),warning:text=>fixture.messages.push(['warning',text])});
useAppStore.setState({config,servers:config.servers,selectedServerId:'node-a',switchServer:async id=>{
 fixture.calls.push(id);useAppStore.setState(s=>({selectedServerId:id,config:{...s.config,selectedServerId:id}}));return {status:'pending'};
}});
createRoot(document.getElementById('root')).render(<MobileHomeScreen/>);
`;
const networkEntry = sentinelEntry.replace("import { api } from '/src/ipc';", "import { api, unlockApi } from '/src/ipc';\nimport { MobileToaster } from '/src/mobile/MobileToaster';").replace(
  "createRoot(document.getElementById('root')).render(<MobileHomeScreen/>);",
  `
document.documentElement.dataset.mobileOs = new URLSearchParams(location.search).get('os') || 'ios';
const net=window.__networkTest={manual:[],raw:[],speeds:[],cancels:[],mainStarts:0,mainStops:0,
 status:{running:false,mainGeneration:6},requestId:null,finish:null,
 state:()=>useAppStore.getState(),setSelection(id){useAppStore.setState({selectedServerId:id});},cancelReject:null,leave:null,
 setStatus(status){net.status=status;useAppStore.getState().setProxyStatus(status);}};
const ip={revision:5,direct:null,proxy:null,updatedAt:123};
const unlock={results:{},checkedAt:123,egress:null};
useAppStore.setState({proxyStatus:net.status,ipInfo:{revision:0,direct:null,proxy:null,updatedAt:0}});
api.proxy.getStatus=async()=>net.status;
api.proxy.start=async()=>{net.mainStarts++;};api.proxy.stop=async()=>{net.mainStops++;};
api.ipInfo.get=async()=>{net.raw.push('ip');return ip;};
unlockApi.run=async()=>{net.raw.push('unlock');return unlock;};
unlockApi.cancelManualCheck=id=>{net.cancels.push(id);return new Promise((_resolve,reject)=>{net.cancelReject=reject;});};
unlockApi.manualCheck=(requestId,force)=>new Promise(resolve=>{
 net.manual.push({requestId,force});net.requestId=requestId;
 net.finish=(ipError,unlockError)=>resolve({context:{requestId,mainGeneration:7,startTime:123},
  ipInfo:ipError?{error:{code:ipError}}:{data:ip},unlock:unlockError?{error:{code:unlockError}}:{data:unlock}});
});
api.server.speedTest=async ids=>{net.speeds.push(ids);return {runId:'1',results:{'node-a':24},outcome:'completed',notInPool:[],tsNotReady:[],
 measurementContext:{runId:'1',requestId:'speed-1',mainGeneration:7,startTime:123}};};
function Host(){const [home,setHome]=React.useState(true);net.leave=()=>setHome(false);return <>{home?<MobileHomeScreen/>:<p>Other screen</p>}<MobileToaster/></>;}
createRoot(document.getElementById('root')).render(<Host/>);
`,
);
const authWarningEntry = sentinelEntry.replace("import { api } from '/src/ipc';", `
import { api } from '/src/ipc';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { useTailscaleLoginProgressStore } from '/src/store/use-tailscale-login-progress-store';
`).replace("createRoot(document.getElementById('root')).render(<MobileHomeScreen/>);", `
const params=new URLSearchParams(location.search);document.documentElement.dataset.mobileOs=params.get('os')||'ios';
const ts={...config.servers[0],protocol:'tailscale',tailscaleSettings:{exitNode:'fixture-exit'}};
const auth=window.__authWarning={opens:[],panels:()=>useMobileFormStore.getState().stack,
 reset(){auth.opens=[];useMobileFormStore.setState({stack:[]});},
 attempt(phase,url){useTailscaleLoginProgressStore.getState().begin('node-a','B');useTailscaleLoginProgressStore.getState().apply({serverId:'node-a',attemptId:'B',phase,url,mainGeneration:7,identityEpoch:'epoch-B'});}};
useAppStore.setState({config:{...config,servers:[ts]},servers:[ts],proxyStatus:{running:true,mainGeneration:7,startTime:123},
 tailscaleStatuses:{'node-a':{serverId:'node-a',nodeName:'fixture',backendState:'NeedsLogin',loggedIn:false,expired:false,peers:[],tailscaleIPs:[],authURL:'https://login.example/status-A'}},
 tailscaleAuthUrls:{'node-a':'https://login.example/store-A'}});
api.config.meshRouteReport=async()=>null;api.system.openExternal=async url=>{auth.opens.push(url);};
createRoot(document.getElementById('root')).render(params.get('screen')==='nodes'?<MobileNodesScreen/>:<MobileHomeScreen/>);
`);
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile picker and measured task consumers', () => {
  beforeAll(async () => {
    server=await createServer({root,cacheDir:path.join(tmpdir(),'polaris-picker-speed-vite-'+process.pid),server:{host:'127.0.0.1',port:0,watch:null},plugins:[{
      name:'picker-speed-fixture',resolveId(id){if(id==='/picker-speed-fixture.tsx'||id==='/full-home-fixture.tsx'||id==='/speed-feedback-fixture.tsx'||id==='/sentinel-fixture.tsx'||id==='/network-fixture.tsx'||id==='/auth-warning-fixture.tsx')return id;},load(id){if(id==='/picker-speed-fixture.tsx')return entry;if(id==='/full-home-fixture.tsx')return fullHomeEntry;if(id==='/speed-feedback-fixture.tsx')return speedFeedbackEntry;if(id==='/sentinel-fixture.tsx')return sentinelEntry;if(id==='/network-fixture.tsx')return networkEntry;if(id==='/auth-warning-fixture.tsx')return authWarningEntry;},
      configureServer(vite){vite.middlewares.use(async(req,res,next)=>{
        const route=req.url?.split('?')[0];
        if(route!=='/__picker-speed'&&route!=='/__full-home'&&route!=='/__speed-feedback'&&route!=='/__sentinel'&&route!=='/__network'&&route!=='/__auth-warning')return next();
        const full=req.url?.includes('full-home');
        const sentinel=req.url?.includes('sentinel');
        const speed=req.url?.includes('speed-feedback');
        const network=route==='/__network';
        const authWarning=route==='/__auth-warning';
        const html=await vite.transformIndexHtml(full?'/__full-home':sentinel?'/__sentinel':speed?'/__speed-feedback':network?'/__network':authWarning?'/__auth-warning':'/__picker-speed',
          '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/'+(full?'full-home-fixture':sentinel?'sentinel-fixture':speed?'speed-feedback-fixture':network?'network-fixture':authWarning?'auth-warning-fixture':'picker-speed-fixture')+'.tsx"></script></html>');
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
  it('sorts only within each group, preserves folding, and restores config order when switched off',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__picker-speed');
    await page.locator('.h-pick-group').first().waitFor();
    await page.evaluate(()=>(window as any).__pickerTest.showExtra());
    await page.locator('[data-group-id="manual"]').click();
    const ids=()=>page.locator('[data-picker-node]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-picker-node')));
    const groups=()=>page.locator('.h-pick-group').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('data-group-id')));
    expect((await ids()).slice(0,8)).toEqual(['manual','orphan','slow','fast','zero','stale','fail','unknown']);
    expect(await groups()).toEqual(['manual','mesh','first','second']);
    const sort=page.getByRole('switch',{name:'按延迟排序'});
    expect(await sort.getAttribute('aria-checked')).toBe('false');
    await sort.click();
    expect(await sort.getAttribute('aria-checked')).toBe('true');
    expect((await ids()).slice(0,6)).toEqual(['zero','fast','orphan','manual','slow','fail']);
    expect((await ids()).indexOf('stale')).toBeGreaterThan((await ids()).indexOf('fail'));
    expect((await ids()).indexOf('unknown')).toBeGreaterThan((await ids()).indexOf('stale'));
    expect(await groups()).toEqual(['manual','mesh','first','second']);
    expect(await page.locator('[data-group-id="manual"]').getAttribute('aria-expanded')).toBe('true');
    expect(await page.evaluate(()=>localStorage.getItem('polaris.nodeSortByLatency'))).toBe('true');
    await sort.click();
    expect((await ids()).slice(0,8)).toEqual(['manual','orphan','slow','fast','zero','stale','fail','unknown']);
    await page.close();
  },30_000);
  it('keeps the heading and search reachable while a long picker list scrolls on all mobile widths and themes',async()=>{
    for (const width of [320,390,768]) for (const theme of ['light','dark']) {
      const page=await browser.newPage({viewport:{width,height:740}});await page.goto(origin+'/__picker-speed');
      await page.locator('.h-pick-group').first().waitFor();
      await page.evaluate((value)=>{document.documentElement.dataset.theme=value;(window as any).__pickerTest.showExtra();},theme);
      await page.locator('[data-group-id="manual"]').click();
      await page.locator('.h-picker-scroll').evaluate(node=>{node.scrollTop=node.scrollHeight;});
      const geometry=await page.evaluate(()=>{
        const sheet=document.querySelector('.h-picker-sheet')!;
        const heading=sheet.querySelector('.h-sheethead')!;
        const search=sheet.querySelector('.h-picker-search')!;
        const scroll=sheet.querySelector('.h-picker-scroll')!;
        const h=heading.getBoundingClientRect(),s=search.getBoundingClientRect(),b=scroll.getBoundingClientRect();
        return {sheetTop:sheet.getBoundingClientRect().top,headingTop:h.top,headingBottom:h.bottom,searchTop:s.top,searchBottom:s.bottom,
          bodyTop:b.top,scrollTop:scroll.scrollTop,scrollMax:scroll.scrollHeight-scroll.clientHeight,sheetScrollTop:sheet.scrollTop,
          headingHit:heading.contains(document.elementFromPoint(h.left+h.width/2,h.top+h.height/2)),
          sortHeight:sheet.querySelector('.h-picker-sort')!.getBoundingClientRect().height,
          sortPaint:parseFloat(getComputedStyle(sheet.querySelector('.h-picker-sort')!,'::before').top)*2};
      });
      expect(geometry.scrollMax).toBeGreaterThan(100);
      expect(geometry.scrollTop).toBeGreaterThan(100);
      expect(geometry.sheetScrollTop).toBe(0);
      expect(geometry.headingTop).toBeGreaterThanOrEqual(geometry.sheetTop);
      expect(geometry.searchTop).toBeGreaterThanOrEqual(geometry.headingBottom);
      expect(geometry.bodyTop).toBeGreaterThanOrEqual(geometry.searchBottom);
      expect(geometry.headingHit).toBe(true);
      expect(geometry.sortHeight).toBe(44);
      expect(geometry.sortHeight-geometry.sortPaint).toBe(36);
      await page.screenshot({path:path.join(tmpdir(),`polaris-picker-scroll-${width}-${theme}.png`)});
      await page.close();
    }
    const page=await browser.newPage({viewport:{width:320,height:420}});await page.goto(origin+'/__picker-speed');
    await page.locator('.h-pick-group').first().waitFor();
    await page.evaluate(()=>{document.documentElement.style.fontSize='32px';(window as any).__pickerTest.showExtra();});
    await page.locator('[data-group-id="manual"]').click();
    const geometry=await page.evaluate(()=>{
      const sheet=document.querySelector('.h-picker-sheet')!,search=sheet.querySelector('.h-picker-search')!,scroll=sheet.querySelector('.h-picker-scroll')!;
      return {sheetTop:sheet.getBoundingClientRect().top,searchBottom:search.getBoundingClientRect().bottom,bodyTop:scroll.getBoundingClientRect().top,bodyHeight:scroll.getBoundingClientRect().height,
        searchWidth:search.getBoundingClientRect().width,inputWidth:search.querySelector('input')!.getBoundingClientRect().width,
        closeVisible:sheet.querySelector('.m-sheet-close')!.getBoundingClientRect().bottom<=scroll.getBoundingClientRect().top};
    });
    expect(geometry.bodyTop).toBeGreaterThanOrEqual(geometry.searchBottom);
    expect(geometry.bodyHeight).toBeGreaterThan(44);
    expect(geometry.inputWidth).toBeGreaterThan(80);
    expect(geometry.closeVisible).toBe(true);
    await page.locator('.h-picker-scroll').evaluate(node=>{node.scrollTop=node.scrollHeight;});
    expect(await page.locator('[data-picker-node="sub-a"]').isVisible()).toBe(true);
    await page.screenshot({path:path.join(tmpdir(),'polaris-picker-scroll-320-2x-short.png')});
    await page.close();
  },60_000);
  it('renders and scrolls the production HomeScreenView inside MobileShell with the mobile CSS chain',async()=>{
    for (const variant of [{width:390,height:844,scale:1},{width:320,height:420,scale:2}]) {
      const page=await browser.newPage({viewport:{width:variant.width,height:variant.height}});
      await page.goto(origin+'/__full-home');
      await page.evaluate(scale=>{document.documentElement.dataset.theme='dark';document.documentElement.style.setProperty('--font-scale',String(scale));},variant.scale);
      const sheet=page.locator('.h-picker-sheet');await sheet.waitFor();
      await sheet.locator('[data-group-id="manual"]').click();
      await sheet.locator('.h-picker-scroll').evaluate(node=>{node.scrollTop=node.scrollHeight;});
      const result=await sheet.evaluate(node=>{
        const head=node.querySelector('.h-sheethead')!,search=node.querySelector('.h-picker-search')!,body=node.querySelector('.h-picker-scroll')!;
        const h=head.getBoundingClientRect(),s=search.getBoundingClientRect(),b=body.getBoundingClientRect();
        const close=node.querySelector('.m-sheet-close')!;
        return {sheetScrollTop:node.scrollTop,bodyScrollTop:body.scrollTop,bodyMax:body.scrollHeight-body.clientHeight,
          headerHit:head.contains(document.elementFromPoint(h.left+h.width/2,h.top+h.height/2)),
          closeVisible:close.getBoundingClientRect().bottom<=b.top,searchVisible:s.bottom<=b.top,
          finalRowVisible:node.querySelector('[data-picker-node="sub-a"]')!.getBoundingClientRect().bottom<=b.bottom+1};
      });
      expect(result.sheetScrollTop).toBe(0);
      expect(result.bodyScrollTop).toBeGreaterThan(0);
      expect(result.bodyMax).toBeGreaterThan(0);
      expect(result.headerHit).toBe(true);
      expect(result.closeVisible).toBe(true);
      expect(result.searchVisible).toBe(true);
      expect(result.finalRowVisible).toBe(true);
      await page.screenshot({path:path.join(tmpdir(),`polaris-full-home-picker-${variant.width}-${variant.scale}x-dark.png`)});
      await page.close();
    }
  },60_000);
  it('terminal progress disappears immediately; late frames do not revive it and new work can start',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__picker-speed');await page.locator('.h-pick-group').first().waitFor();
    await page.evaluate(()=>{const store=(window as any).__pickerTest.store;store.getState().begin(['a','b','c'],'all');});
    await page.getByText('准备测速…',{exact:true}).waitFor();expect(await page.locator('progress').count()).toBe(0);
    await page.evaluate(()=>{const store=(window as any).__pickerTest.store;store.getState().progress({runId:'1',tested:1,ok:0,total:3});});
    await page.locator('progress').waitFor();expect(await page.locator('progress').getAttribute('value')).toBe('1');expect(await page.locator('progress').getAttribute('max')).toBe('3');
    expect(await page.locator('.m-speedtest-counts').innerText()).toContain('成功 0 · 失败 1');
    await page.getByRole('button',{name:'切换页面'}).click();expect(await page.locator('[data-speed-screen="nodes"] progress').getAttribute('value')).toBe('1');
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().done({runId:'1',outcome:'interrupted',tested:1,total:3,serverIds:['a','b','c'],pending:['b','c']});});
    await page.locator('.m-speedtest-progress').waitFor({state:'detached'});
    await page.evaluate(()=>{const s=(window as any).__pickerTest.store.getState();s.settle(s.request.token,{runId:'1',results:{a:-1},outcome:'interrupted',notInPool:[],tsNotReady:[]});s.progress({runId:'1',tested:1,ok:0,total:3});});
    expect(await page.locator('.m-speedtest-progress').count()).toBe(0);
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().begin(['a'],'current');});
    await page.getByText('准备测速…',{exact:true}).waitFor();expect(await page.locator('progress').count()).toBe(0);expect(await page.locator('.m-speedtest-counts').count()).toBe(0);
    await page.evaluate(()=>{const s=(window as any).__pickerTest.store.getState();s.progress({runId:'2',tested:0,ok:0,total:1});s.waiting('2');});
    await page.getByText('暂未收到新进度，状态待确认',{exact:true}).waitFor();expect(await page.locator('.m-speedtest-progress').innerText()).not.toContain('中断');
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().done({runId:'1',outcome:'completed',tested:3,total:3,serverIds:['a','b','c'],pending:[]});});
    expect(await page.locator('progress').getAttribute('max')).toBe('1');expect(await page.locator('progress').getAttribute('value')).toBe('0');
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().done({runId:'2',outcome:'completed',tested:1,total:1,serverIds:['a'],pending:[]});});
    await page.locator('.m-speedtest-progress').waitFor({state:'detached'});
    await page.evaluate(()=>{const s=(window as any).__pickerTest.store.getState();s.settle(s.request.token,{runId:'2',results:{a:27},outcome:'completed',notInPool:[],tsNotReady:[]});s.progress({runId:'2',tested:1,ok:1,total:1});});
    expect(await page.locator('.m-speedtest-progress').count()).toBe(0);
    expect(await page.evaluate(()=>(window as any).__pickerTest.store.getState().task.phase)).toBe('completed');
    await page.evaluate(()=>{(window as any).__pickerTest.store.getState().begin(['b'],'current');});
    await page.getByText('准备测速…',{exact:true}).waitFor();
    await page.close();
  },30_000);
  it('a completed Home batch clears the previous note and renders no completion status',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__speed-feedback');
    const all=page.getByRole('button',{name:'全部测速'});await all.waitFor();
    await all.click();
    expect(await page.evaluate(()=>(window as any).__speedFeedback.calls)).toBe(1);
    await page.evaluate(()=>(window as any).__speedFeedback.resolve({runId:'1',results:{'sub-a':26},outcome:'completed',notInPool:['manual-0'],tsNotReady:[]}));
    await page.getByText('未纳入本次测速',{exact:false}).waitFor();
    expect(await page.locator('.h-latency-note[role="status"]').count()).toBe(1);
    await all.click();
    expect(await page.evaluate(()=>(window as any).__speedFeedback.calls)).toBe(2);
    await page.evaluate(()=>(window as any).__speedFeedback.resolve({runId:'2',results:{'sub-a':27},outcome:'completed',notInPool:[],tsNotReady:[]}));
    await page.waitForFunction(()=>document.querySelector('.h-latency-action[aria-busy="true"]')===null);
    expect(await page.evaluate(()=>(window as any).__speedFeedback.view().feedback)).toBeNull();
    expect(await page.locator('.h-latency-note[role="status"]').count()).toBe(0);
    expect(await page.getByText('已返回',{exact:false}).count()).toBe(0);
    await page.close();
  },30_000);
  it('the live Home picker switches both sentinel exits through the receipt path and keeps its guards',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});await page.goto(origin+'/__sentinel');
    const open=async()=>{await page.locator('.h-nodelead').click();await page.locator('.h-picker-sheet').waitFor();};
    const direct=page.locator('[data-exit-write="home-sentinel-direct"]');
    const block=page.locator('[data-exit-write="home-sentinel-block"]');
    await open();await direct.click();
    await page.waitForFunction(()=>(window as any).__sentinelTest.calls.length===1);
    await page.locator('.h-picker-sheet').waitFor({state:'detached'});
    expect(await page.evaluate(()=>(window as any).__sentinelTest.calls)).toEqual(['__direct__']);
    expect(await page.evaluate(()=>(window as any).__sentinelTest.messages.at(-1))).toEqual(['info','直连 · 已保存，正在应用连接更改']);
    await open();await direct.click();await page.locator('.h-picker-sheet').waitFor({state:'detached'});
    expect(await page.evaluate(()=>(window as any).__sentinelTest.calls)).toEqual(['__direct__']);
    await open();await block.click();
    await page.waitForFunction(()=>(window as any).__sentinelTest.calls.length===2);
    await page.locator('.h-picker-sheet').waitFor({state:'detached'});
    expect(await page.evaluate(()=>(window as any).__sentinelTest.calls)).toEqual(['__direct__','__block__']);
    expect(await page.evaluate(()=>(window as any).__sentinelTest.messages.at(-1))).toEqual(['info','阻断 · 已保存，正在应用连接更改']);
    await page.evaluate(()=>(window as any).__sentinelTest.setRouting('direct'));
    await open();expect(await block.isDisabled()).toBe(true);
    expect(await block.innerText()).toContain('当前为直连模式：全部流量都不经过代理出口，阻断不会生效');
    expect(await page.evaluate(()=>(window as any).__sentinelTest.calls)).toHaveLength(2);
    await page.close();
  },30_000);
  it('iOS explicit network entry holds one request through preparation, preserves partial success and measures only after ready',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.goto(origin+'/__network');
      const button=page.locator('[data-write-control="network-check"]');await button.waitFor();
      expect(await page.evaluate(()=>(window as any).__networkTest.manual)).toEqual([]);
      await button.click();
      await page.waitForFunction(()=>(window as any).__networkTest.manual.length===1);
      expect(await button.isDisabled()).toBe(true);
      expect(await page.evaluate(()=>{const n=(window as any).__networkTest;return [n.raw,n.speeds,n.mainStarts,n.mainStops];})).toEqual([[],[],0,0]);
      await page.evaluate(()=>{const n=(window as any).__networkTest;n.setStatus({running:true,mainGeneration:7,startTime:123});n.finish(null,'targetTimedOut');});
      await page.locator('[data-write-error="network-check"]').waitFor();
      expect(await page.locator('[data-write-error="network-check"]').innerText()).toContain('服务可用性未能检测');
      expect(await page.evaluate(()=>{const n=(window as any).__networkTest;return [n.state().ipInfo.revision,n.speeds,n.cancels,n.mainStops];})).toEqual([5,[['node-a']],[],0]);
      expect(await button.isDisabled()).toBe(false);
    } finally {await page.close();}
  },30_000);
  it('iOS network results from a stopped/replaced main or changed selection do not overwrite the current view',async()=>{
    for(const mutation of ['stop','generation','selection']) {
      const page=await browser.newPage({viewport:{width:390,height:844}});
      try {
        await page.goto(origin+'/__network');
        const button=page.locator('[data-write-control="network-check"]');await button.click();
        await page.waitForFunction(()=>(window as any).__networkTest.manual.length===1);
        await page.evaluate(kind=>{const n=(window as any).__networkTest;
          n.setStatus({running:kind!=='stop',mainGeneration:kind==='generation'?8:7,startTime:123});
          if(kind==='selection')n.setSelection('other-node');
          n.finish(null,null);
        },mutation);
        await page.waitForFunction(()=>document.querySelector('[data-write-control="network-check"]')?.getAttribute('aria-busy')!=='true');
        expect(await page.evaluate(()=>{const n=(window as any).__networkTest;return [n.state().ipInfo.revision,n.speeds,n.mainStarts,n.mainStops];})).toEqual([0,[],0,0]);
        if(mutation==='selection')expect(await page.evaluate(()=>(window as any).__networkTest.cancels)).toHaveLength(1);
      } finally {await page.close();}
    }
  },30_000);
  it('Android disconnected manual network entry keeps the existing independent IP/unlock behavior',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.goto(origin+'/__network?os=android');
      await page.locator('[data-write-control="network-check"]').click();
      await page.waitForFunction(()=>(window as any).__networkTest.raw.length===2);
      expect(await page.evaluate(()=>{const n=(window as any).__networkTest;return [n.manual,n.raw,n.speeds,n.mainStarts,n.mainStops];})).toEqual([[],['ip','unlock'],[],0,0]);
    } finally {await page.close();}
  },30_000);
  it('a rejected exact network cancellation remains visible globally after Home has left',async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.goto(origin+'/__network');
      await page.locator('[data-write-control="network-check"]').click();
      await page.waitForFunction(()=>(window as any).__networkTest.manual.length===1);
      await page.evaluate(()=>(window as any).__networkTest.leave());
      await page.getByText('Other screen',{exact:true}).waitFor();
      await page.waitForFunction(()=>!!(window as any).__networkTest.cancelReject);
      await page.evaluate(()=>(window as any).__networkTest.cancelReject(new Error('private cancellation diagnostic')));
      await page.locator('.m-toast').filter({hasText:'未能取消这次操作，请重试。'}).waitFor();
      const requestId=await page.evaluate(()=>(window as any).__networkTest.requestId as string);
      expect(await page.evaluate(()=>{const n=(window as any).__networkTest;return [n.cancels,n.mainStops];})).toEqual([[requestId],0]);
      expect(await page.locator('[data-write-control="network-check"]').count()).toBe(0);
    } finally {await page.close();}
  },30_000);
  it.each(['home','nodes'])('%s auth warning only opens the active bound request URL on iOS; terminal and legacy STATUS open fresh panels',async screen=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.goto(origin+'/__auth-warning?screen='+screen);
      const button=page.locator(screen==='home'?'.h-noticeact':'.mn-notice-act');await button.waitFor();
      await button.click();
      expect(await page.evaluate(()=>(window as any).__authWarning.opens)).toEqual([]);
      expect(await page.evaluate(()=>(window as any).__authWarning.panels().at(-1))).toMatchObject({kind:'ts-login',serverId:'node-a'});
      await page.evaluate(()=>{const a=(window as any).__authWarning;a.reset();a.attempt('awaitingAuth','https://login.example/bound-B');});
      await button.click();
      expect(await page.evaluate(()=>(window as any).__authWarning.opens)).toEqual(['https://login.example/bound-B']);
      for(const phase of ['cancelled','timedOut','failed']) {
        await page.evaluate(p=>{const a=(window as any).__authWarning;a.reset();a.attempt(p,'https://login.example/bound-B');},phase);
        await button.click();
        expect(await page.evaluate(()=>(window as any).__authWarning.opens)).toEqual([]);
        expect(await page.evaluate(()=>(window as any).__authWarning.panels().at(-1))).toMatchObject({kind:'ts-login',serverId:'node-a'});
      }
    } finally {await page.close();}
  },30_000);
  it.each(['home','nodes'])('%s auth warning preserves Android legacy STATUS URL with no attempt',async screen=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.goto(origin+'/__auth-warning?os=android&screen='+screen);
      await page.locator(screen==='home'?'.h-noticeact':'.mn-notice-act').click();
      expect(await page.evaluate(()=>(window as any).__authWarning.opens)).toEqual(['https://login.example/status-A']);
      expect(await page.evaluate(()=>(window as any).__authWarning.panels())).toEqual([]);
    } finally {await page.close();}
  },30_000);
});

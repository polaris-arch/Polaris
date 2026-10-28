import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { nativeAutoFixture } from '@/contracts/native-auto.test-support';
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React from 'react';
import {createRoot} from 'react-dom/client';
import {useAutoSelectStatus} from '/src/hooks/use-auto-select-status';
import {useAppStore} from '/src/store/app-store';
const params=new URLSearchParams(location.search);
const initial=${JSON.stringify(nativeAutoFixture())};
const servers=[];
let config={servers,autoSelectEnabled:true,selectedServerId:null};
const callbacks=[],jobs=[],registrations=[];let backend=structuredClone(initial);let callbackId=0;const handlers=new Map();
const f=window.__status={reads:0,writes:0,offs:0,offThrows:false,callbacks,jobs,registrations,registrationCalls:0,holdReady:params.get('registration')==='hold',failRegistration:params.get('registration')==='fail',
 ready(index=0){const r=registrations[index];r.registered=true;r.resolve(r.id);},
 failReady(index=0){registrations[index].reject(Error('registration rejected'));},
 active(){return registrations.filter(r=>r.registered).length;},
 resolve(patch={},index=0){jobs[index].resolve({success:true,data:{...jobs[index].snapshot,...patch}});},
 reject(index=0){jobs[index].reject(Error('snapshot failed'));},
 event(patch={}){backend={...structuredClone(initial),...patch};for(const r of registrations)if(r.registered)handlers.get(r.handler)?.({payload:backend});},
 stale(patch={}){callbacks[0]?.({payload:{...structuredClone(initial),...patch}});},
 publish(patch){config={...config,...patch};useAppStore.setState({config,servers:config.servers,selectedServerId:config.selectedServerId});},
 manual(){config={...config,autoSelectEnabled:false,selectionIntent:undefined,autoSelectPins:[]};f.publish(config);},

 unmount(){app.unmount();},
 remount(){app=createRoot(document.getElementById('root'));render();},
 state(){return structuredClone(useAppStore.getState().config);},
};
// Use the real server API, ipc-client listen/listenReady and Tauri event wrapper.
// Only native IPC is replaced, so registration remains genuinely asynchronous.
window.__TAURI_INTERNALS__={
 transformCallback(fn){const id=++callbackId;handlers.set(id,fn);return id;},
 invoke(cmd,args){
  if(cmd==='plugin:event|listen'&&args.event==='event:autoSelectStatus'){
   f.registrationCalls++;callbacks.push(handlers.get(args.handler));
   return new Promise((resolve,reject)=>{
    const r={id:f.registrationCalls,handler:args.handler,resolve,reject,registered:false};registrations.push(r);
    if(f.failRegistration){f.failRegistration=false;reject(Error('registration rejected'));}
    else if(!f.holdReady){r.registered=true;resolve(r.id);}
   });
  }
  if(cmd==='plugin:event|unlisten')return Promise.resolve();
  if(cmd==='auto_select_status'){f.reads++;return new Promise((resolve,reject)=>jobs.push({resolve,reject,snapshot:structuredClone(backend)}));}
  return Promise.resolve({success:true});
 },
};
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(event,id){
 if(event!=='event:autoSelectStatus')return;
 f.offs++;if(f.offThrows)throw Error('detach failed');
 const r=registrations.find(r=>r.id===id);if(r)r.registered=false;
}};

useAppStore.setState({config,servers,selectedServerId:null,proxyStatus:{running:false}});
function Probe({id}) { const c=useAppStore(s=>s.config); const state=useAutoSelectStatus(c); return <div><button id={'refresh'+id} onClick={state.refresh}>refresh</button><pre id={'state'+id}>{JSON.stringify(state)}</pre></div>; }
let app=createRoot(document.getElementById('root'));const render=()=>app.render(<><Probe id="a"/><Probe id="b"/></>);render();
`;
let server:ViteDevServer;let browser:Browser;let origin:string;
const state=(page:Page)=>page.locator('#statea').textContent();
const data=(page:Page)=>page.evaluate(()=>{const f=(window as any).__status;return {reads:f.reads,registrations:f.registrationCalls,offs:f.offs};});
async function open(registration='') { const page=await browser.newPage();await page.goto(origin+'/__status?registration='+registration);await page.waitForFunction(()=>(window as any).__status?.registrationCalls===1);if(!registration)await page.waitForFunction(()=>(window as any).__status?.reads===1);return page; }
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('Shared native Auto snapshot/event lifecycle',()=>{
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


  it('two mounted cards share ready registration and one snapshot',async()=>{const page=await open('hold');try{expect((await data(page)).reads).toBe(0);await page.evaluate(()=>(window as any).__status.ready());await expect.poll(async()=>(await data(page)).reads).toBe(1);await page.evaluate(()=>(window as any).__status.resolve());await expect.poll(()=>state(page)).toContain('"generation":1');expect((await data(page)).registrations).toBe(1);}finally{await page.close();}});
  for(const reject of [false,true])it('newer event survives late '+(reject?'rejection':'snapshot'),async()=>{const page=await open();try{await page.evaluate(()=>(window as any).__status.event({generation:8}));await page.evaluate(reject=>reject?(window as any).__status.reject():(window as any).__status.resolve(),reject);await expect.poll(()=>state(page)).toContain('"generation":8');expect(await state(page)).toContain('"failed":false');}finally{await page.close();}});
  it('registration failure can be retried and rapid refresh stays single-flight',async()=>{const page=await open('fail');try{await expect.poll(()=>state(page)).toContain('"failed":true');await page.evaluate(()=>{for(let i=0;i<8;i++)document.querySelector<HTMLButtonElement>('#refresha')!.click();});await expect.poll(async()=>(await data(page)).reads).toBe(1);expect((await data(page)).registrations).toBe(2);}finally{await page.close();}});
  it('late registration after unmount detaches and never reads',async()=>{const page=await open('hold');try{await page.evaluate(()=>{(window as any).__status.unmount();(window as any).__status.ready();});await expect.poll(async()=>(await data(page)).offs).toBe(1);expect((await data(page)).reads).toBe(0);}finally{await page.close();}});
  it('feature off invalidates pending snapshots and late callbacks',async()=>{const page=await open();try{await page.evaluate(()=>(window as any).__status.manual());await expect.poll(()=>state(page)).toContain('"enabled":false');await page.evaluate(()=>{(window as any).__status.resolve({generation:9});(window as any).__status.stale({generation:10});});expect(await state(page)).not.toContain('"generation":');}finally{await page.close();}});
  it('a failed detach cannot revive an unmounted session',async()=>{const page=await open();try{await page.evaluate(()=>{const f=(window as any).__status;f.offThrows=true;f.unmount();f.stale({generation:99});f.remount();});await expect.poll(async()=>(await data(page)).registrations).toBe(2);await page.evaluate(()=>(window as any).__status.resolve({},1));await expect.poll(()=>state(page)).toContain('"generation":1');}finally{await page.close();}});
  it('config changes invalidate old snapshots while two consumers remain mounted', async () => {
    const page = await open();
    try {
      await page.evaluate(() => (window as any).__status.publish({ selectionIntent: { mode: 'auto', scope: 'manual' } }));
      await expect.poll(async () => (await data(page)).reads).toBe(2);
      await page.evaluate(() => { const f = (window as any).__status; f.resolve({ generation: 99 }, 0); f.stale({ generation: 98 }); f.resolve({ generation: 2 }, 1); });
      await expect.poll(() => state(page)).toContain('"generation":2');
      expect(await state(page)).not.toContain('"generation":99');
    } finally { await page.close(); }
  });

});

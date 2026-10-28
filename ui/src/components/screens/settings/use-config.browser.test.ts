import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../../..');
// Production hooks/stores/IPC adapter are real. Only the native transport is fake.
const entry = `
import React, { useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { useConfig } from '/src/components/screens/settings/use-config';
import { useAppStore, acquireConfigSync, refreshConfig } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { useDialogStore } from '/src/components/dialogs/dialog-store';
import { setToastImpl } from '/src/lib/error-handler';
import { IPC_CHANNELS as C } from '/src/domain/ipc-channels';
import i18n, { i18nReady } from '/src/i18n';
const params = new URLSearchParams(location.search);
let disk = { mixedPort: 7890, controlPort: 9090, logLevel: 'info', desktopNotifications: true,
 hardwareAcceleration: true, windowEffects: true, rememberWindowSize: true,
 servers: [], subscriptions: [], selectedServerId: '__direct__', proxyMode: 'smart', proxyModeType: 'manual' };
let counter=0, app, ids=['a','b'], mounted=true;
const handlers=new Map(), listeners=[], registrations=[], cleanupWaiters=new Map();
const f=window.__config={hooks:{}, cleanups:{}, reads:[], writes:[], updates:[], toasts:[], prompts:0, cleanReads:0,
 holdReads:params.has('holdReads'), holdWrites:false, readFail:false, holdReady:params.has('holdReady'),
 state:()=>useAppStore.getState().config, disk:()=>disk, staged:()=>useStagedConfigStore.getState().entries,
 listenerCount:()=>listeners.filter(l=>l.event===C.EVENT_CONFIG_CHANGED).length,
 registrationCount:0, loading:()=>useAppStore.getState().configLoading,
 snapshot:()=>Object.fromEntries(Object.entries(f.hooks).map(([id,h])=>[id,{config:h.config,loading:h.loading,error:h.error}])),
 signal(){for(const l of [...listeners])if(l.event===C.EVENT_CONFIG_CHANGED)handlers.get(l.handler)?.({payload:{}});},
 external(patch){disk={...disk,...patch};f.signal();},
 silent(patch){disk={...disk,...patch};},
 finishRead(index,error=false){const job=f.reads[index];job.done=true;error?job.reject(Error('read failed')):job.resolve({success:true,data:job.snapshot});},
 commitWrite(index){const job=f.writes[index];disk={...disk,...job.patch};job.saved=structuredClone(disk);},
 finishWrite(index,error=false){const job=f.writes[index];if(!error&&!job.saved)f.commitWrite(index);job.done=true;
  error?job.reject(Error('write failed')):job.resolve({success:true,data:job.saved});},
 update(id,patch,throwOnError=false){const row={settled:false,error:null};f.updates.push(row);
  f.hooks[id].update(patch,{throwOnError}).then(()=>row.settled=true,error=>{row.error=error.message;row.settled=true;});},
 reload(id){f.hooks[id].reload();},
 undo(id){useStagedConfigStore.getState().revert(id);},
 unmount(id){return new Promise(resolve=>{cleanupWaiters.set(id,resolve);ids=ids.filter(x=>x!==id);render();});},
 unmountAll(){mounted=false;app.unmount();},
 register(error=false){const jobs=registrations.splice(0);for(const job of jobs){if(error)job.reject(Error('listen failed'));else{listeners.push(job.listener);job.resolve(job.listener.id);}}},
};
window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},
 transformCallback(fn){const key=++counter;handlers.set(key,fn);return key;},
 invoke(cmd,args={}){
  if(cmd==='plugin:event|listen'){
   const listener={id:++counter,event:args.event,handler:args.handler};
   if(args.event===C.EVENT_CONFIG_CHANGED){f.registrationCount++;if(f.holdReady)return new Promise((resolve,reject)=>registrations.push({listener,resolve,reject}));}
   listeners.push(listener);return Promise.resolve(listener.id);
  }
  if(cmd==='plugin:event|unlisten'){const at=listeners.findIndex(l=>l.id===args.eventId);if(at>=0)listeners.splice(at,1);return Promise.resolve();}
  if(cmd===C.CONFIG_GET){const snapshot=structuredClone(disk);return new Promise((resolve,reject)=>{
   const index=f.reads.push({snapshot,resolve,reject,done:false})-1;
   if(!f.holdReads)queueMicrotask(()=>f.finishRead(index,f.readFail));
  });}
  if(cmd===C.CONFIG_PATCH){return new Promise((resolve,reject)=>{
   const index=f.writes.push({patch:args.patch,resolve,reject,done:false})-1;
   if(!f.holdWrites)queueMicrotask(()=>f.finishWrite(index));
  });}
  if(cmd===C.APP_STARTUP_CONFIG_FLAGS)return Promise.resolve({success:true,data:{hardwareAcceleration:true,windowEffects:true,rememberWindowSize:true}});
  if(cmd===C.APP_TAKE_CLEAN_EXIT_FLAG){f.cleanReads++;return Promise.resolve({success:true,data:false});}
  if(cmd==='plugin:window|inner_size'||cmd==='plugin:window|outer_size')return Promise.resolve({width:700,height:600});
  return Promise.resolve({success:true,data:null});
 }
};
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(event,id){const at=listeners.findIndex(l=>l.id===id);if(at>=0)listeners.splice(at,1);}};
setToastImpl({error:text=>f.toasts.push(text),success:()=>{},warning:()=>{},info:()=>{}});
const originalOpen=useDialogStore.getState().open;
useDialogStore.setState({open:(...args)=>{f.prompts++;originalOpen(...args);}});
await i18nReady;await i18n.changeLanguage('en-US');
useStagedConfigStore.setState({enabled:params.has('staged'),hydrated:true,baseline:disk,entries:[]});
useAppStore.setState({config:params.has('cold')?null:disk,proxyStatus:{running:true,starting:false}});
function Instance({id}){const hook=useConfig();f.hooks[id]=hook;
 // Declared AFTER useConfig: this passive cleanup runs after the hook has
 // retired its lifecycle, aborted owned reads, unsubscribed and released.
 useEffect(()=>{f.hooks[id]=hook;return ()=>{delete f.hooks[id];f.cleanups[id]=(f.cleanups[id]||0)+1;
  cleanupWaiters.get(id)?.();cleanupWaiters.delete(id);
 };},[id]);return <output data-instance={id}>{JSON.stringify({config:hook.config,loading:hook.loading,error:hook.error})}</output>;}
function Root(){useEffect(()=>params.has('root')?acquireConfigSync({scope:'main'}):undefined,[]);return <>{ids.map(id=><Instance key={id} id={id}/>)}</>;}
function render(){if(mounted)app.render(params.has('strict')?<React.StrictMode><Root/></React.StrictMode>:<Root/>);}
app=createRoot(document.getElementById('root'));render();
`;
let server: ViteDevServer, browser: Browser, origin: string;
const read = (page: Page) => page.evaluate(() => { const f=(window as any).__config;return {
  hooks:f.snapshot(), reads:f.reads.length,writes:f.writes.map((j:any)=>j.patch),updates:f.updates,
  state:f.state(),disk:f.disk(),staged:f.staged(),toasts:f.toasts,prompts:f.prompts,
  listeners:f.listenerCount(),registrations:f.registrationCount,loading:f.loading(),
}; });
async function open(query = '') {
  const page = await browser.newPage(); page.setDefaultTimeout(7000);
  await page.goto(`${origin}/__config?${query}`);
  await page.locator('[data-instance="a"]').waitFor();
  return page;
}
async function ready(page: Page) {
  await expect.poll(async () => (await read(page)).reads).toBeGreaterThan(0);
  await expect.poll(async () => (await read(page)).loading).toBe(false);
}

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('real mounted useConfig shared authority and isolated direct intentions', () => {
  beforeAll(async () => {
    server = await createServer({root,cacheDir:path.join(tmpdir(),`polaris-config-hooks-${process.pid}`),server:{host:'127.0.0.1',port:0,watch:null},plugins:[{
      name:'config-hooks-native-fixture',resolveId(id){if(id==='/config-hooks-native-fixture.tsx')return id;},
      load(id){if(id==='/config-hooks-native-fixture.tsx')return entry;},configureServer(vite){vite.middlewares.use('/__config',async(_req,res)=>{
        res.setHeader('Content-Type','text/html');res.end(await vite.transformIndexHtml('/__config','<html><div id="root"></div><script type="module" src="/config-hooks-native-fixture.tsx"></script></html>'));
      });},
    }]});
    await server.listen();const address=server.httpServer!.address();if(!address||typeof address!=='object')throw Error('Vite did not bind');origin=`http://127.0.0.1:${address.port}`;
    browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});
  },30000);
  afterAll(async()=>{await browser?.close();await server?.close();});

  it.each(['root','root&strict',''])('two mounted hooks share a ready-registered reference: %s', async query => {
    const page=await open(`${query}&holdReady&cold`);try{
      let state=await read(page);expect(state.reads).toBe(0);expect(state.hooks.a.loading).toBe(true);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdReady=false;f.register();});await ready(page);
      state=await read(page);expect(state.reads).toBe(1);expect(state.listeners).toBe(1);
      expect(await page.evaluate(()=>{const f=(window as any).__config;return f.hooks.a.config===f.hooks.b.config&&f.hooks.a.config===f.state();})).toBe(true);
    }finally{await page.close();}
  });

  it('seed stays visible while revalidation fails; a cold failure is retryable', async()=>{
    for(const cold of [false,true]){const page=await open(`${cold?'cold&':''}holdReads`);try{
      await expect.poll(async()=>(await read(page)).reads).toBe(1);
      expect((await read(page)).hooks.a.loading).toBe(cold);
      await page.evaluate(()=>(window as any).__config.finishRead(0,true));
      await expect.poll(async()=>(await read(page)).hooks.a.loading).toBe(false);
      expect(Boolean((await read(page)).hooks.a.error)).toBe(cold);
      if(!cold)expect((await read(page)).hooks.a.config.mixedPort).toBe(7890);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdReads=false;f.reload('a');});
      await expect.poll(async()=>(await read(page)).hooks.a.error).toBeNull();
      await expect.poll(async()=>(await read(page)).hooks.a.config?.mixedPort).toBe(7890);
    }finally{await page.close();}}
  });

  it('same-key A/B drafts remain independent and only the failed operation rolls back',async()=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{controlPort:9001});f.update('b',{controlPort:9002},true);});
      await expect.poll(async()=>(await read(page)).hooks.a.config.controlPort).toBe(9001);
      expect((await read(page)).hooks.b.config.controlPort).toBe(9002);
      await page.evaluate(()=>(window as any).__config.finishWrite(0));
      await expect.poll(async()=>(await read(page)).writes.length).toBe(2);
      await page.evaluate(()=>(window as any).__config.finishWrite(1,true));
      await expect.poll(async()=>(await read(page)).updates[1].settled).toBe(true);
      const state=await read(page);expect(state.hooks.a.config.controlPort).toBe(9001);expect(state.hooks.b.config.controlPort).toBe(9001);
      expect(state.updates[1].error).toBe('write failed');expect(state.toasts).toHaveLength(1);expect(state.hooks.b.error).toBeNull();
    }finally{await page.close();}
  });

  it.each([[true,true],[true,false],[false,true],[false,false]])('same instance quick writes, P1 success=%s P2 success=%s',async(one,two)=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{controlPort:9001});f.update('a',{controlPort:9002});});
      await expect.poll(async()=>(await read(page)).writes.length).toBe(1);
      await page.evaluate(error=>(window as any).__config.finishWrite(0,error),!one);
      await expect.poll(async()=>(await read(page)).writes.length).toBe(2);
      expect((await read(page)).hooks.a.config.controlPort).toBe(9002);
      await page.evaluate(error=>(window as any).__config.finishWrite(1,error),!two);
      await expect.poll(async()=>(await read(page)).updates.every((u:any)=>u.settled)).toBe(true);
      expect((await read(page)).hooks.a.config.controlPort).toBe(two?9002:one?9001:9090);
    }finally{await page.close();}
  });

  it('P1 committed reply arrives after external B publication: no whole-config rollback',async()=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{controlPort:9001});});
      await expect.poll(async()=>(await read(page)).writes.length).toBe(1);
      await page.evaluate(()=>{const f=(window as any).__config;f.commitWrite(0);f.external({controlPort:9010,desktopNotifications:false});});
      await expect.poll(async()=>(await read(page)).state.controlPort).toBe(9010);
      await page.evaluate(()=>(window as any).__config.finishWrite(0));
      await expect.poll(async()=>(await read(page)).updates[0].settled).toBe(true);
      const state=await read(page);expect(state.hooks.a.config.controlPort).toBe(9010);expect(state.state.desktopNotifications).toBe(false);
      expect(state.hooks.b.config.desktopNotifications).toBe(false);
    }finally{await page.close();}
  });

  it('saved direct values survive a read outage without reporting or repeating a failed save',async()=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.readFail=true;f.update('a',{controlPort:9001},true);});
      await expect.poll(async()=>(await read(page)).updates[0].settled).toBe(true);
      let state=await read(page);expect(state.updates[0].error).toBeNull();expect(state.toasts).toEqual([]);expect(state.writes).toHaveLength(1);
      expect(state.hooks.a.config.controlPort).toBe(9001);
      await page.evaluate(()=>{const f=(window as any).__config;f.readFail=false;f.external({controlPort:9020});});
      await expect.poll(async()=>(await read(page)).hooks.a.config.controlPort).toBe(9020);
    }finally{await page.close();}
  });

  it('staged-only is zero writes; mixed failure retains staged intent and undo restores raw truth',async()=>{
    const page=await open('staged');try{await ready(page);
      await page.evaluate(()=>(window as any).__config.update('a',{mixedPort:8000}));
      await expect.poll(async()=>(await read(page)).hooks.a.config.mixedPort).toBe(8000);
      expect((await read(page)).writes).toHaveLength(0);expect((await read(page)).prompts).toBe(0);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{mixedPort:8001,controlPort:9001});});
      await expect.poll(async()=>(await read(page)).writes.length).toBe(1);
      expect((await read(page)).writes[0]).toEqual({controlPort:9001});
      await page.evaluate(()=>(window as any).__config.finishWrite(0,true));
      await expect.poll(async()=>(await read(page)).updates[1].settled).toBe(true);
      expect((await read(page)).hooks.a.config.mixedPort).toBe(8001);expect((await read(page)).disk.mixedPort).toBe(7890);
      await page.evaluate(()=>(window as any).__config.undo('setting:mixedPort'));
      await expect.poll(async()=>(await read(page)).hooks.a.config.mixedPort).toBe(7890);
    }finally{await page.close();}
  });

  it('U-7 skips initial fill and reload, deduplicates a publication, and keeps later real changes',async()=>{
    const page=await open('holdReads');try{
      await expect.poll(async()=>(await read(page)).reads).toBe(1);
      await page.evaluate(()=>{const f=(window as any).__config;f.reads[0].snapshot.hardwareAcceleration=false;f.finishRead(0);f.holdReads=false;});await ready(page);
      expect((await read(page)).prompts).toBe(0);
      await page.evaluate(()=>{const f=(window as any).__config;f.silent({windowEffects:false});f.reload('a');});
      await expect.poll(async()=>(await read(page)).hooks.a.config.windowEffects).toBe(false);
      expect((await read(page)).prompts).toBe(0);
      await page.evaluate(()=>(window as any).__config.external({rememberWindowSize:false}));
      await expect.poll(async()=>(await read(page)).prompts).toBe(1);
      await page.evaluate(()=>(window as any).__config.external({hardwareAcceleration:false}));
      await expect.poll(async()=>(await read(page)).prompts).toBe(2);
    }finally{await page.close();}
  });

  it('a successful direct restart edit and its earlier broadcast produce one prompt across hooks',async()=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{hardwareAcceleration:false});});
      await expect.poll(async()=>(await read(page)).writes.length).toBe(1);
      await page.evaluate(()=>{const f=(window as any).__config;f.commitWrite(0);f.signal();});
      await expect.poll(async()=>(await read(page)).prompts).toBe(1);
      await page.evaluate(()=>(window as any).__config.finishWrite(0));
      await expect.poll(async()=>(await read(page)).updates[0].settled).toBe(true);
      expect((await read(page)).prompts).toBe(1);
    }finally{await page.close();}
  });

  it('StrictMode does not double-send an update and an unmounted writer cannot toast or prompt',async()=>{
    const page=await open('strict');try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdWrites=true;f.update('a',{hardwareAcceleration:false});});
      await expect.poll(async()=>(await read(page)).writes.length).toBe(1);
      const cleanups=await page.evaluate(()=>(window as any).__config.cleanups.a||0);
      await page.evaluate(async()=>{const f=(window as any).__config;await f.unmount('a');f.finishWrite(0,true);});
      expect(await page.evaluate(()=>(window as any).__config.cleanups.a)).toBe(cleanups+1);
      await expect.poll(async()=>(await read(page)).updates[0].settled).toBe(true);
      const state=await read(page);expect(state.writes).toHaveLength(1);expect(state.hooks.a).toBeUndefined();expect(state.hooks.b).toBeDefined();
      expect(state.toasts).toEqual([]);expect(state.prompts).toBe(0);expect(state.listeners).toBe(1);
    }finally{await page.close();}
  });

  it('unmount while listener registration is pending does not revive the session',async()=>{
    const page=await open('holdReady');try{
      await page.evaluate(()=>{const f=(window as any).__config;f.unmountAll();f.holdReady=false;f.register();});
      await expect.poll(async()=>(await read(page)).listeners).toBe(0);
      expect((await read(page)).reads).toBe(0);
    }finally{await page.close();}
  });

  it('older reload finally cannot end a newer pending reload',async()=>{
    const page=await open();try{await ready(page);
      await page.evaluate(()=>{const f=(window as any).__config;f.holdReads=true;f.reload('a');});
      await expect.poll(async()=>(await read(page)).reads).toBe(2);
      await page.evaluate(()=>{const f=(window as any).__config;f.reload('a');f.finishRead(1);});
      await expect.poll(async()=>(await read(page)).reads).toBe(3);expect((await read(page)).hooks.a.loading).toBe(true);
      await page.evaluate(()=>(window as any).__config.finishRead(2));
      await expect.poll(async()=>(await read(page)).hooks.a.loading).toBe(false);
    }finally{await page.close();}
  });
});

import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root=path.resolve(import.meta.dirname,'../..');
const entry=`
import React,{useState,useRef} from 'react';
import {createRoot} from 'react-dom/client';
import {PolicySelector} from '/src/components/screens/app-policy/AppPolicyScreen';
import {MeasurementStartNotice,requestIOSMeasurementConsent} from '/src/mobile/MeasurementStartNotice';
import {useSheetFocus} from '/src/mobile/use-sheet-focus';
import {NodeFormPanel} from '/src/mobile/forms/NodeFormPanel';
import {useMobileFormStore} from '/src/mobile/forms/form-store';
import {useSubscriptionCreateDialogOperation} from '/src/components/dialogs/use-subscription-create-dialog-operation';
import {useSubscriptionCreateOperationStore as operations} from '/src/store/subscription-create-operation-store';
import {useDialogStore} from '/src/components/dialogs/dialog-store';
import {useAppStore} from '/src/store/app-store';
import {allFields} from '/src/components/dialogs/node-spec';
import {api} from '/src/ipc';
import {DEMO_CONFIG} from '/harness-fixture';
import i18n,{i18nReady} from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
await i18nReady;await i18n.changeLanguage('en-US');
useAppStore.setState({config:structuredClone(DEMO_CONFIG)});
window.__keyRequests=[];window.__ids=[];window.__clears=[];window.__announcements=[];window.__closed=[];window.__added=[];
window.__privateLabel=i18n.t(allFields('tailcat').find(x=>x.k==='privateKey').label);
window.__generateLabel=i18n.t('node.tcGenerate');window.__deriveLabel=i18n.t('node.tcDerivePublic');
api.server.tailcatKeypair=(key)=>new Promise((resolve,reject)=>window.__keyRequests.push({key,resolve,reject}));
useDialogStore.setState({hasInstance:()=>true,closeInstance:id=>window.__closed.push(id)});
operations.setState({snapshots:{},start:async(id)=>{
 window.__ids.push(id);const snapshot={operationId:id,revision:3,phase:'failed',terminal:true,error:{code:'REQUEST_FAILED'}};
 operations.setState({snapshots:{[id]:snapshot}});return snapshot;
},clearTerminal:id=>window.__clears.push(id),claimTerminalAnnouncement:(id,revision)=>{
 const key=id+':'+revision;if(window.__announcements.includes(key))return false;window.__announcements.push(key);return true;
}});
function Subscription(){const op=useSubscriptionCreateDialogOperation({instanceId:'form',externalCloseLocked:false,requestFormClose:()=>{},onAdded:id=>window.__added.push(id)});
 return <><button id="attempt" onClick={()=>op.start({name:'fixture',url:'https://example.invalid/sub'}).catch(()=>{})}>Attempt</button><output id="busy">{String(op.operationBusy||op.starting||op.cancelling)}</output></>;}
function Consent(){const [disabled,setDisabled]=useState(false);const panel=useRef(null);const nested=location.search.includes('nested');useSheetFocus(panel,nested,()=>{});
 window.__lock=()=>{document.getElementById('lock').setAttribute('data-overlay','privacy-lock');document.getElementById('lock-button').focus();setDisabled(true);};
 return <><div ref={panel} tabIndex={-1}><button id="consent-trigger" onClick={()=>{document.documentElement.dataset.mobileOs='ios';requestIOSMeasurementConsent().then(answer=>window.__answer=answer);}}>Measure</button><button>Background control</button></div><div id="lock"><button id="lock-button">Unlock</button></div><MeasurementStartNotice disabled={disabled}/></>;}
useMobileFormStore.setState({hasInstance:()=>!window.__formGone});
function Key(){const [mounted,setMounted]=useState(true);window.__unmount=()=>{window.__formGone=true;setMounted(false);};return mounted?<NodeFormPanel instanceId="key-form" initialProto="tailcat"/>:<p>Unmounted</p>;}
function App(){if(location.search.includes('consent'))return <Consent/>;if(location.search.includes('key'))return <Key/>;if(location.search.includes('subscription'))return <Subscription/>;
return <PolicySelector appId="fixture" action="proxy" servers={[]} subscriptions={[]} onPick={()=>{}}/>;}
createRoot(document.getElementById('root')).render(<App/>);
`;
let server:ViteDevServer,browser:Browser,origin:string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS==='1')('UIP01/02/04/05 actual React interaction regressions',()=>{
 beforeAll(async()=>{
  server=await createServer({root,server:{host:'127.0.0.1',port:0},plugins:[{name:'audit-ui-fixture',resolveId(id){if(id==='/audit-ui-fixture.tsx')return id;},load(id){if(id==='/audit-ui-fixture.tsx')return entry;},configureServer(vite){vite.middlewares.use('/__audit-ui',async(_req,res)=>{res.setHeader('Content-Type','text/html');res.end(await vite.transformIndexHtml('/__audit-ui','<html><div id="root"></div><script type="module" src="/audit-ui-fixture.tsx"></script></html>'));});}}]});
  await server.listen();const address=server.httpServer!.address();if(!address||typeof address!=='object')throw Error('Vite unavailable');origin='http://127.0.0.1:'+address.port;
  browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});
 },30000);
 afterAll(async()=>{await browser?.close();await server?.close();});
 it('UIP01 native Enter/Space open once, Escape closes and restores trigger',async()=>{
  const page=await browser.newPage();try{await page.goto(origin+'/__audit-ui');const trigger=page.locator('.app-pol');
   for(const key of ['Enter','Space']){await trigger.focus();await page.keyboard.press(key);await page.locator('[role=menu]').waitFor();expect(await trigger.getAttribute('aria-expanded')).toBe('true');await page.keyboard.press('Escape');await page.locator('[role=menu]').waitFor({state:'detached'});expect(await page.locator('[role=menu]').count()).toBe(0);expect(await trigger.evaluate(el=>el===document.activeElement)).toBe(true);}
  }finally{await page.close();}
 });
 for(const nested of [false,true])it('UIP02 focus isolation/restoration; nested='+nested,async()=>{
  const page=await browser.newPage();try{await page.goto(origin+'/__audit-ui?consent'+(nested?'&nested':''));const trigger=page.locator('#consent-trigger');
   await trigger.click();const dialog=page.getByRole('dialog');await dialog.waitFor();await page.waitForFunction(()=>document.querySelector('[role=dialog]')?.contains(document.activeElement));
   for(const key of ['Tab','Tab','Shift+Tab','Shift+Tab']){await page.keyboard.press(key);expect(await dialog.evaluate(el=>el.contains(document.activeElement))).toBe(true);}
   await page.keyboard.press('Escape');await dialog.waitFor({state:'detached'});expect(await trigger.evaluate(el=>el===document.activeElement)).toBe(true);expect(await page.evaluate(()=>(window as any).__answer)).toBe(false);
   await trigger.click();await dialog.waitFor();await dialog.locator('button').last().click();await dialog.waitFor({state:'detached'});expect(await page.evaluate(()=>(window as any).__answer)).toBe(true);expect(await trigger.evaluate(el=>el===document.activeElement)).toBe(true);
   await trigger.click();await dialog.waitFor();await page.evaluate(()=>(window as any).__lock());await dialog.waitFor({state:'detached'});expect(await page.locator('#lock-button').evaluate(el=>el===document.activeElement)).toBe(true);
  }finally{await page.close();}
 });
 it('UIP04 successive operations at revision3 each clean up and announce exactly once',async()=>{
  const page=await browser.newPage();try{await page.goto(origin+'/__audit-ui?subscription');for(let i=1;i<=3;i++){await page.locator('#attempt').click();await page.waitForFunction(n=>(window as any).__clears.length===n,i);}
   const state=await page.evaluate(()=>({ids:(window as any).__ids,clears:(window as any).__clears,announcements:(window as any).__announcements}));expect(new Set(state.ids).size).toBe(3);expect(state.clears).toEqual(state.ids);expect(state.announcements.length).toBe(3);
  }finally{await page.close();}
 });
 it('UIP05 old failure cannot release newer busy state; protocol switch invalidates response',async()=>{
  const page=await browser.newPage();try{await page.goto(origin+'/__audit-ui?key');const label=await page.evaluate(()=>(window as any).__privateLabel);const field=page.getByLabel(label,{exact:false});const generated=await page.evaluate(()=>(window as any).__generateLabel);const derived=await page.evaluate(()=>(window as any).__deriveLabel);
   await page.getByRole('button',{name:generated,exact:true}).click();await field.fill('new-private');await page.getByRole('button',{name:derived,exact:true}).click();
   await page.evaluate(()=>(window as any).__keyRequests[0].reject(new Error('obsolete failure')));await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>resolve(null))));
   // The current request still owns busy. No third backend call can be started.
   expect(await page.getByRole('button',{name:derived,exact:true}).isDisabled()).toBe(true);expect(await page.evaluate(()=>(window as any).__keyRequests.length)).toBe(2);
   await page.evaluate(()=>(window as any).__keyRequests[1].resolve({privateKey:'new-private',publicKey:'new-public'}));await page.waitForFunction(()=>(document.querySelector('#mnf-tc-pub') as HTMLInputElement)?.value==='new-public');
   await page.getByRole('button',{name:derived,exact:true}).click();await page.locator('#mnf-proto').click();await page.getByRole('option',{name:'VLESS',exact:true}).click();
   await page.evaluate(()=>(window as any).__keyRequests[2].resolve({privateKey:'new-private',publicKey:'obsolete-protocol-public'}));await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>resolve(null))));expect(await page.locator('#mnf-tc-pub').count()).toBe(0);
  }finally{await page.close();}
 });
 it('UIP05 edited private key wins over old generated/derived responses; unmount invalidates',async()=>{
  const page=await browser.newPage();try{await page.goto(origin+'/__audit-ui?key');const label=await page.evaluate(()=>(window as any).__privateLabel);const field=page.getByLabel(label,{exact:false});const generated=await page.evaluate(()=>(window as any).__generateLabel);const derived=await page.evaluate(()=>(window as any).__deriveLabel);
   await page.getByRole('button',{name:generated,exact:true}).click();await field.fill('user-key-B');await page.evaluate(()=>(window as any).__keyRequests[0].resolve({privateKey:'old-generated-A',publicKey:'public-A'}));await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>resolve(null))));expect(await field.inputValue()).toBe('user-key-B');expect(await page.locator('#mnf-tc-pub').inputValue()).toBe('');
   await page.getByRole('button',{name:derived,exact:true}).click();await field.fill('user-key-C');await page.evaluate(()=>(window as any).__keyRequests[1].resolve({privateKey:'user-key-B',publicKey:'public-B'}));await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>resolve(null))));expect(await field.inputValue()).toBe('user-key-C');expect(await page.locator('#mnf-tc-pub').inputValue()).toBe('');
   await page.getByRole('button',{name:derived,exact:true}).click();await page.evaluate(()=>{(window as any).__unmount();});await page.getByText('Unmounted').waitFor();await page.evaluate(()=>(window as any).__keyRequests[2].resolve({privateKey:'user-key-C',publicKey:'late-public-C'}));expect(await page.locator('#mnf-tc-pub').count()).toBe(0);
  }finally{await page.close();}
 });
});

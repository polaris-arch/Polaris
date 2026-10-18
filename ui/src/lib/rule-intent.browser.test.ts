import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { RuleDialog } from '/src/components/dialogs/RuleDialog';
import { RulesScreen } from '/src/components/screens/rules/RulesScreen';
import { MobileHomeScreen } from '/src/mobile/home/MobileHomeScreen';
import { MobileConnectionsScreen } from '/src/mobile/connections/MobileConnectionsScreen';
import { ruleAppendTargets } from '/src/components/dialogs/rule-append';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
await i18nReady; await i18n.changeLanguage('en-US');
const params = new URLSearchParams(location.search);
const plane = params.get('plane') === 'dns' ? 'dns' : 'route';
const mode = params.get('mode') ?? 'list';
const collection = plane === 'dns' ? 'dnsRules' : 'trafficRules';
const rule = { id:'r',type:'domain',values:['a.example'],action:'proxy',enabled:true,
 ...(plane==='dns'?{effects:{dns:{enabled:true,action:{type:'fakeIp'}}}}:{}) };
const config = { servers:[], subscriptions:[], trafficRules:plane==='route'?[rule]:[], dnsRules:plane==='dns'?[rule]:[],
  dnsServers:[], dnsServerGroups:[], networkProfiles:[], appRules:[], customRules:[], regionRouting:{}, tunConfig:{}, dnsConfig:{} };
const subject={kind:'domain',type:'domain',value:'b.example'};
const test = window.__n1 = { calls:[], errors:[], subject, rule, target:ruleAppendTargets([rule],subject)[0],
 entries:()=>useStagedConfigStore.getState().entries };
for (const group of Object.values(api)) if(group && typeof group==='object') for(const key of Object.keys(group)) {
 if(typeof group[key]==='function') group[key]=key.startsWith('on')?()=>()=>{}:async()=>null;
}
api.config.get=async()=>config;
api.ruleResources.list=async()=>[];
api.rules.delete=async(...args)=>{test.calls.push(args);};
api.rules.update=async(...args)=>{test.calls.push(args);if(test.race)throw new Error('RULE_CONFLICT');};
useAppStore.setState({config,rules:config.trafficRules,dnsRules:config.dnsRules,servers:[],loadConfig:async()=>{},proxyStatus:{running:false}});
useStagedConfigStore.setState({enabled:false,entries:[],baseline:config});
test.change=()=>{const changed={...rule,remarks:'changed'};useAppStore.setState({config:{...config,[collection]:[changed]},[plane==='dns'?'dnsRules':'rules']:[changed]});};
test.staged=()=>{useAppStore.setState({proxyStatus:{running:true}});useStagedConfigStore.setState({enabled:true});};
test.editStaged=()=>useStagedConfigStore.getState().stage({id:'rule:r',kind:'rule',label:'edit',entityPath:[collection,'r'],nextValue:{...rule,remarks:'staged change'}});
const component=mode==='dialog'?<RuleDialog instanceId="n1-dialog" ruleId="r" initialPlane={plane}/>:mode==='home'?<MobileHomeScreen/>:mode==='connections'?<MobileConnectionsScreen/>:<RulesScreen plane={plane}/>;
createRoot(document.getElementById('root')).render(component);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('N1 real component transaction regressions', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'n1-component-fixture', enforce: 'pre',
      transform(code, id) {
        if(id.endsWith('/mobile/home/MobileHomeScreen.tsx')) return code.replace('  return (\n    <HomeScreenView', '  window.__n1.append = onAppendToRule; window.__n1.chooseSubject = setRuleSubject; window.__n1.subjectValue = ruleSubject;\n  return (\n    <HomeScreenView');
        if(id.endsWith('/mobile/connections/MobileConnectionsScreen.tsx')) return code.replace('  return (\n    <ConnectionsView', '  window.__n1.append = onAppendToRule;\n  return (\n    <ConnectionsView');
      }, resolveId(id) { if (id === '/n1-fixture.tsx') return id; },
      load(id) { if (id === '/n1-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__n1', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__n1', '<html lang="en-US"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/n1-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (typeof address !== 'object' || !address) throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });


  for (const mode of ['list','dialog']) for (const plane of ['route','dns']) for (const staged of [false,true]) {
    it(`${mode}/${plane}/${staged}: an intervening edit requires a new first click`, async () => {
      const page = await browser.newPage();
      try {
        await page.goto(origin+`/__n1?mode=${mode}&plane=${plane}`);
        const button = mode==='list'?page.locator('button.nd-a.err').first():page.locator('button.btn.ghost').filter({hasText:/delete/i}).first();
        await button.waitFor();
        if(staged)await page.evaluate(()=>(window as any).__n1.staged());
        await button.click();
        await page.evaluate((staged)=>{const t=(window as any).__n1;staged?t.editStaged():t.change();},staged);
        await page.waitForFunction(()=>!document.querySelector('button.confirming'));
        await button.click();
        expect(await page.evaluate(()=>(window as any).__n1.calls)).toEqual([]);
        if(staged)expect(await page.evaluate(()=>(window as any).__n1.entries()[0].nextValue.remarks)).toBe('staged change');
        await page.waitForFunction(() => document.querySelector('button.confirming') !== null);
        expect(await button.getAttribute('class')).toContain('confirming');
        await button.click();
        if(staged)await page.waitForFunction(()=>(window as any).__n1.entries()[0].nextValue===null);
        else await page.waitForFunction(()=>(window as any).__n1.calls.length===1);
      } finally {await page.close();}
    });
  }
  for (const mode of ['home','connections']) {
    it(`${mode}: actual append callback sends expected base and refuses a changed staged picker`, async()=>{
      const page=await browser.newPage();
      try {
        await page.goto(origin+'/__n1?mode='+mode);
        await page.waitForFunction(()=>typeof (window as any).__n1.append==='function');
        if(mode==='home'){
          await page.evaluate(()=>{const t=(window as any).__n1;t.chooseSubject(t.subject);});
          await page.waitForFunction(()=>(window as any).__n1.subjectValue !== null);
        }
        await page.evaluate(mode=>{const t=(window as any).__n1;t.race=true;mode==='home'?t.append(t.target):t.append(t.subject,t.target);},mode);
        await page.waitForFunction(()=>(window as any).__n1.calls.length===1);
        expect(await page.evaluate(()=>(window as any).__n1.calls[0][2])).toMatchObject({id:'r',values:['a.example']});
        await page.evaluate(mode=>{const t=(window as any).__n1;t.staged();t.editStaged();if(mode==='home')t.chooseSubject(t.subject);},mode);
        if(mode==='home')await page.waitForFunction(()=>(window as any).__n1.subjectValue !== null);
        await page.evaluate(mode=>{const t=(window as any).__n1;mode==='home'?t.append(t.target):t.append(t.subject,t.target);},mode);
        expect(await page.evaluate(()=>(window as any).__n1.calls.length)).toBe(1);
        expect(await page.evaluate(()=>(window as any).__n1.entries()[0].nextValue.remarks)).toBe('staged change');
      }finally{await page.close();}
    });
  }
});

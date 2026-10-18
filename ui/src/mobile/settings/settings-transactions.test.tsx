import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { DisplayPage } from '/src/mobile/settings/DisplayPage';
import { DnsPage } from '/src/mobile/settings/DnsPage';
import { UpdatePage } from '/src/mobile/settings/UpdatePage';
import { updateApi, versionApi } from '/src/ipc/api-client';
import { createCommit } from '/src/mobile/settings/write-feedback';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
await i18nReady; await i18n.changeLanguage('en-US');
const test = window.__n1 = { writes: [], errors: [], feedback: {}, frame: null, install: null, language: () => i18n.language };
updateApi.check = async () => ({hasUpdate:true,updateInfo:{version:'2.0.0',downloadUrl:'https://example.invalid/fixture.apk',fileName:'fixture.apk',fileSize:1024}});
updateApi.download = target => {test.downloads ??= [];return new Promise((resolve,reject)=>test.downloads.push({target,resolve,reject}));};
versionApi.getInfo = async () => ({ appVersion: '1.0.0', platform: 'android' });
updateApi.onProgress = listener => { test.frame = listener; return () => {}; };
updateApi.getProgress = async () => null;
updateApi.requestInstallPermission = () => test.holdPermission ? new Promise((resolve,reject)=>{test.permission=resolve;test.permissionReject=reject;}) : Promise.resolve({ granted:true });
updateApi.install = () => new Promise((resolve,reject) => { test.install = resolve; test.installReject=reject; });
function App() {
 const [visible,setVisible]=useState(true); test.hide=()=>setVisible(false); test.show=()=>setVisible(true);
 const [config, setConfig] = useState({ language: 'en-US', uiTheme: 'dark', dnsConfig: {
  domesticDns: 'https://223.5.5.5/dns-query', foreignDns: 'https://1.1.1.1/dns-query',
  resolveNodeDomainsAhead: true, nodeResolverPool: ['custom-a'],
  nodeResolverCustom: [{ id: 'custom-a', spec: 'https://1.1.1.1/A' }]
 }});
 const update = patch => new Promise((resolve, reject) => test.writes.push({ patch, resolve: () => { setConfig(c => ({ ...c, ...patch })); resolve(); }, reject }));
 const commit = createCommit(updater=>{test.feedback=updater(test.feedback);if(Object.keys(test.feedback).length)test.errors.push(Object.keys(test.feedback)[0]);}, i18n.t.bind(i18n));
 const mode = new URLSearchParams(location.search).get('mode');
 const Component = mode === 'dns' ? DnsPage : mode === 'update' ? UpdatePage : DisplayPage;
 return visible ? <Component config={config} update={update} commit={commit} /> : <p>Page closed</p>;
}
document.documentElement.dataset.theme = 'dark';
createRoot(document.getElementById('root')).render(<App />);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('N1 real component transaction regressions', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'n1-component-fixture', resolveId(id) { if (id === '/n1-fixture.tsx') return id; },
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

  async function choose(page: import('@playwright/test').Page, row: string, option: string) {
    await page.locator(`[data-setting="${row}"]`).getByRole('combobox').click();
    await page.getByRole('option', { name: option, exact: true }).click();
  }
  it('appearance side effects wait for save and stale responses cannot replace the latest choice', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1');
      await choose(page, 'theme', 'Light');
      expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('dark');
      await page.evaluate(() => (window as any).__n1.writes[0].reject(new Error('save failed')));
      await page.waitForFunction(() => (window as any).__n1.errors.length === 1);
      expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('dark');
      await choose(page, 'theme', 'Light');
      await choose(page, 'theme', 'Dark');
      await page.evaluate(() => (window as any).__n1.writes[2].resolve());
      await page.evaluate(() => (window as any).__n1.writes[1].resolve());
      expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('dark');
      await choose(page, 'language', '简体中文');
      await page.evaluate(() => (window as any).__n1.writes[3].reject(new Error('save failed')));
      await page.waitForFunction(() => (window as any).__n1.errors.length === 2);
      expect(await page.evaluate(() => (window as any).__n1.language())).toBe('en-US');
    } finally { await page.close(); }
  });
  it('mobile custom upstreams can be disabled and invalid input is visible without a write', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1?mode=dns');
      expect(await page.locator('[data-setting="takeover-system-dns"]').count()).toBe(0);
      await page.locator('[data-setting="race-custom-custom-a"]').getByRole('switch').click();
      expect(await page.evaluate(() => (window as any).__n1.writes[0].patch.dnsConfig.nodeResolverPool)).toEqual([]);
      const input = page.locator('[data-list-editor="race-custom-entries"] input').first();
      await input.fill('invalid host'); await input.blur();
      await page.getByText(/invalid host/).first().waitFor();
      expect(await page.evaluate(() => (window as any).__n1.writes.length)).toBe(1);
      await input.fill('https://1.1.1.1/A'); await input.blur();
      expect(await page.getByText(/invalid host/).count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__n1.writes.length)).toBe(1);
      await input.fill('invalid host'); await input.blur();
      await page.getByText(/invalid host/).first().waitFor();
      await input.fill('https://1.1.1.1/A'); await input.blur();
      expect(await page.getByText(/invalid host/).count()).toBe(0);

    } finally { await page.close(); }
  });
  it('a new download invalidates an in-flight installer handoff and its late success', async () => {
    const page = await browser.newPage();
    try {
      await page.goto(origin + '/__n1?mode=update');
      await page.waitForFunction(() => typeof (window as any).__n1.frame === 'function');
      await page.evaluate(() => (window as any).__n1.frame({ status: 'downloaded', percentage: 100, filePath: '/synthetic/A.apk', verified: true }));
      await page.getByRole('button', { name: 'Install', exact: true }).click();
      await page.waitForFunction(() => typeof (window as any).__n1.install === 'function');
      await page.evaluate(() => (window as any).__n1.frame({ status: 'downloading', percentage: 10 }));
      await page.evaluate(() => (window as any).__n1.install({ ok: true, awaitingSystemInstaller: true }));
      expect(await page.getByText(/Handed to the system installer/).count()).toBe(0);
    } finally { await page.close(); }
  });

  for (const kind of ['theme','language']) {
    it(`${kind}: committed A survives newer failed B and navigation before the receipt`,async()=>{
      const page=await browser.newPage();
      try{
        await page.goto(origin+'/__n1');
        await choose(page,kind,kind==='theme'?'Light':'简体中文');
        await choose(page,kind,kind==='theme'?'Dark':'English');
        await page.evaluate(()=>(window as any).__n1.writes[0].resolve());
        await page.evaluate(()=>(window as any).__n1.writes[1].reject(new Error('B failed')));
        await page.waitForFunction(()=>Object.keys((window as any).__n1.feedback).length===1);
        expect(await page.evaluate(kind=>kind==='theme'?document.documentElement.dataset.theme:(window as any).__n1.language(),kind)).toBe(kind==='theme'?'light':'zh-CN');
        // New isolated page retains the global application even after the page component exits.
        await page.goto(origin+'/__n1');
        await choose(page,kind,kind==='theme'?'Light':'简体中文');
        await page.evaluate(()=>(window as any).__n1.hide());
        await page.getByText('Page closed').waitFor();
        await page.evaluate(()=>(window as any).__n1.writes[0].resolve());
        expect(await page.evaluate(kind=>kind==='theme'?document.documentElement.dataset.theme:(window as any).__n1.language(),kind)).toBe(kind==='theme'?'light':'zh-CN');
        await page.evaluate(()=>(window as any).__n1.show());
        await page.locator('[data-setting="theme"]').waitFor();
        expect(await page.evaluate(kind=>kind==='theme'?document.documentElement.dataset.theme:(window as any).__n1.language(),kind)).toBe(kind==='theme'?'light':'zh-CN');
      }finally{await page.close();}
    });
  }
  for(const outcome of ['permission-refusal','permission-rejection','install-refusal','install-rejection','success']) {
    it(`invalidated ${outcome} cannot change replacement package feedback`,async()=>{
      const page=await browser.newPage();
      try{
        await page.goto(origin+'/__n1?mode=update');
        await page.waitForFunction(()=>typeof (window as any).__n1.frame==='function');
        await page.evaluate(outcome=>{const t=(window as any).__n1;t.holdPermission=outcome.startsWith('permission');t.frame({status:'downloaded',percentage:100,filePath:'/synthetic/A.apk',verified:true});},outcome);
        await page.getByRole('button',{name:'Install',exact:true}).click();
        await page.waitForFunction(outcome=>typeof (window as any).__n1[outcome.startsWith('permission')?'permission':'install']==='function',outcome);
        await page.evaluate(()=>{const t=(window as any).__n1;t.frame({status:'downloading',percentage:10});});
        await page.getByRole('button',{name:'Install',exact:true}).waitFor({state:'hidden'});
        await page.evaluate(outcome=>{const t=(window as any).__n1;t.feedback={'app-update':'B failed'};
          if(outcome==='permission-refusal')t.permission({granted:false,reason:'refused'});
          else if(outcome==='permission-rejection')t.permissionReject(new Error('old permission failed'));
          else if(outcome==='install-refusal')t.install({ok:false,reason:'refused'});
          else if(outcome==='install-rejection')t.installReject(new Error('old install failed'));
          else t.install({ok:true,awaitingSystemInstaller:true});
        },outcome);
        await page.waitForTimeout(0);
        expect(await page.evaluate(()=>(window as any).__n1.feedback)).toEqual({'app-update':'B failed'});
      }finally{await page.close();}
    });
  }

  it('removing a newly added invalid DNS row clears feedback without a redundant persistence write',async()=>{
    const page=await browser.newPage();
    try{
      await page.goto(origin+'/__n1?mode=dns');
      const editor=page.locator('[data-list-editor="race-custom-entries"]');
      await editor.getByRole('button').filter({hasText:/Add/}).first().click();
      const input=editor.locator('input').nth(1);
      await input.fill('invalid-added-host');await input.blur();
      await page.getByText(/invalid-added-host/).first().waitFor();
      const writes=await page.evaluate(()=>(window as any).__n1.writes.length);
      await editor.locator('button.m-settings-list-remove').nth(1).click();
      expect(await page.getByText(/invalid-added-host/).count()).toBe(0);
      expect(await page.evaluate(()=>(window as any).__n1.writes.length)).toBe(writes);
    }finally{await page.close();}
  });

  it('download and retry controls attach feedback without an installer-local predicate',async()=>{
    const page=await browser.newPage();
    try{
      await page.goto(origin+'/__n1?mode=update');
      await page.getByRole('button',{name:'Check for updates',exact:true}).click();
      await page.getByRole('button',{name:'Download',exact:true}).click();
      await page.waitForFunction(()=>(window as any).__n1.downloads?.length===1);
      await page.evaluate(()=>{const t=(window as any).__n1;t.downloads[0].reject(new Error('download rejected'));});
      await page.waitForFunction(()=>Object.keys((window as any).__n1.feedback).includes('app-update'));
      await page.evaluate(()=>{const t=(window as any).__n1;t.frame({status:'error',percentage:0,errorCode:'DOWNLOAD_FAILED',errorDetail:'fixture retry'});});
      await page.getByRole('button',{name:'Retry',exact:true}).click();
      await page.waitForFunction(()=>(window as any).__n1.downloads.length===2);
      await page.evaluate(()=>(window as any).__n1.downloads[1].resolve());
      await page.waitForFunction(()=>!Object.keys((window as any).__n1.feedback).includes('app-update'));
      expect(await page.evaluate(()=>(window as any).__n1.downloads[1].target.fileName)).toBe('fixture.apk');
    }finally{await page.close();}
  });

});

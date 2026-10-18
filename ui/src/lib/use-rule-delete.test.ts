import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { useRuleDelete } from '/src/lib/use-rule-delete';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
await i18nReady; await i18n.changeLanguage('en-US');
const plane = new URLSearchParams(location.search).get('plane') === 'dns' ? 'dns' : 'route';
const collection = plane === 'dns' ? 'dnsRules' : 'trafficRules';
const rule = { id:'r',type:'domain',values:['a.example'],action:'proxy',enabled:true };
const config = { servers:[], trafficRules: plane==='route'?[rule]:[], dnsRules: plane==='dns'?[rule]:[] };
const test = window.__n1 = { calls: [], rule, plane, entries: () => useStagedConfigStore.getState().entries };
api.rules.delete = async (...args) => { test.calls.push(args); };
api.config.setStagedPending = async () => {};
useAppStore.setState({ config, rules:config.trafficRules, dnsRules:config.dnsRules, loadConfig: async () => {} });
useStagedConfigStore.setState({enabled:false, entries:[], baseline:config});
test.change = () => useAppStore.setState(plane==='dns'?{dnsRules:[{...rule,remarks:'changed'}]}:{rules:[{...rule,remarks:'changed'}]});
test.staged = () => { useAppStore.setState({proxyStatus:{running:true}}); useStagedConfigStore.setState({enabled:true}); };
test.newRule = () => { useAppStore.setState({rules:[],dnsRules:[]}); useStagedConfigStore.getState().stage({id:'rule:r',kind:'rule',label:'new',entityPath:[collection,'r'],nextValue:rule}); };
test.editStaged = () => useStagedConfigStore.getState().stage({id:'rule:r',kind:'rule',label:'edit',entityPath:[collection,'r'],nextValue:{...rule,remarks:'staged change'}});
function App(){ test.remove=useRuleDelete(plane); return <p>Ready</p>; }
createRoot(document.getElementById('root')).render(<App/>);
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

  for (const plane of ['route','dns']) {
    it(`${plane}: direct deletion sends its expected snapshot and stale displayed rules are rejected`, async () => {
      const page = await browser.newPage();
      try {
        await page.goto(origin + '/__n1?plane=' + plane); await page.getByText('Ready').waitFor();
        await page.evaluate(async () => { const t=(window as any).__n1; await t.remove(t.rule); });
        expect(await page.evaluate(() => (window as any).__n1.calls)).toEqual([['r',plane,{id:'r',type:'domain',values:['a.example'],action:'proxy',enabled:true}]]);
        const rejected = await page.evaluate(async () => { const t=(window as any).__n1;t.change();try{await t.remove(t.rule);return false;}catch{return true;} });
        expect(rejected).toBe(true);
        expect(await page.evaluate(() => (window as any).__n1.calls.length)).toBe(1);
      } finally { await page.close(); }
    });
    it(`${plane}: deleting an existing staged rule creates a tombstone without a backend write`, async () => {
      const page = await browser.newPage();
      try {
        await page.goto(origin + '/__n1?plane=' + plane); await page.getByText('Ready').waitFor();
        await page.evaluate(() => (window as any).__n1.staged());
        await page.evaluate(async () => { const t=(window as any).__n1;await t.remove(t.rule); });
        const entries = await page.evaluate(() => (window as any).__n1.entries());
        expect(entries).toHaveLength(1);
        expect(entries[0].entityPath).toEqual([plane === 'dns' ? 'dnsRules' : 'trafficRules', 'r']);
        expect(entries[0].nextValue).toBeNull();
        expect(await page.evaluate(() => (window as any).__n1.calls)).toEqual([]);
      } finally { await page.close(); }
    });
    it(`${plane}: staged-new removal reverts, while a concurrent staged edit rejects the old confirmation`, async () => {
      const page = await browser.newPage();
      try {
        await page.goto(origin + '/__n1?plane=' + plane); await page.getByText('Ready').waitFor();
        await page.evaluate(() => { const t=(window as any).__n1;t.staged();t.newRule(); });
        await page.evaluate(async () => { const t=(window as any).__n1;await t.remove(t.rule); });
        expect(await page.evaluate(() => (window as any).__n1.entries())).toEqual([]);
        expect(await page.evaluate(() => (window as any).__n1.calls)).toEqual([]);
        await page.evaluate(() => { const t=(window as any).__n1;t.newRule();t.editStaged(); });
        const rejected = await page.evaluate(async () => { const t=(window as any).__n1;try{await t.remove(t.rule);return false;}catch{return true;} });
        expect(rejected).toBe(true);
        expect(await page.evaluate(() => (window as any).__n1.entries()[0].nextValue.remarks)).toBe('staged change');
      } finally { await page.close(); }
    });
  }

});

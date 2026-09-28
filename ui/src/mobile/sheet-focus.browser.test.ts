import path from 'node:path';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MobileSelect } from '/src/mobile/MobileSelect';
import { MobileInfo } from '/src/mobile/MobileInfo';
import { SelectSheetPanel, SelectSheetTrigger } from '/src/mobile/screens/rules/Primitives';
import { FormSheet } from '/src/mobile/forms/FormSheet';
import { MobileShell } from '/src/mobile/MobileShell';
import { MobileToaster } from '/src/mobile/MobileToaster';
import { toast } from '/src/lib/error-handler';
import { dismissTop } from '/src/mobile/back-stack';
import { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/screens/rules/rules-screen.css';
import '/src/mobile/redesign.css';
await i18nReady;
window.__sheetBack=dismissTop;
window.__sheetToast=()=>toast.info('Visible above details', {dismiss:{label:'Dismiss message'}});
const options=Array.from({length:40},(_,index)=>({id:String(index),label:'Option '+index}));
const defaults=new Set(['items']);
const groups=[{id:'items',label:'Choices',options:options.map(option=>({value:option.id,label:option.label}))}];
function App(){
 const [open,setOpen]=useState(false),[value,setValue]=useState('32'),[formClosed,setFormClosed]=useState(false);
 return <><MobileShell active="home" onSelect={()=>{}} toastHost={<MobileToaster/>}><p>Screen fixture</p></MobileShell>{formClosed?<p>Form closed</p>:<FormSheet title="Editing fixture" closeLabel="Close form" cancelLabel="Cancel edit" onRequestClose={()=>setFormClosed(true)}>
  <MobileSelect aria-label="Simple choice" value={value} onChange={event=>setValue(event.currentTarget.value)}>{options.map(option=><option key={option.id} value={option.id}>{option.label}</option>)}</MobileSelect>
  <SelectSheetTrigger label="Grouped choice" value={value} options={options} open={open} onOpen={()=>setOpen(true)}/>
  <SelectSheetPanel label="Grouped choice" value={value} groups={groups} openGroupIds={defaults} open={open} onClose={()=>setOpen(false)} onSelect={id=>{setValue(id);setOpen(false);}} closeLabel="Close choice" note={<MobileInfo title="Choice explanation" summary="Short explanation" details={<p>Complete explanation</p>}/>}/>
 </FormSheet>}</>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('shared sheet focus in real Chromium', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'sheet-focus-fixture', resolveId(id) { if (id === '/sheet-focus-fixture.tsx') return id; },
      load(id) { if (id === '/sheet-focus-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__sheet-focus', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__sheet-focus', '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/sheet-focus-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen(); const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });
  it('simple selector reveals current row, traps Tab, and close paths restore its trigger', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } }); await page.goto(origin + '/__sheet-focus');
    const trigger = page.getByRole('combobox', { name: 'Simple choice' });
    for (const close of ['Escape', 'x', 'select', 'back']) {
      await trigger.click(); const panel = page.locator('.m-select-panel');
      await page.waitForFunction(() => document.activeElement?.getAttribute('aria-selected') === 'true');
      expect(await panel.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
      for (let index = 0; index < 45; index++) { await page.keyboard.press('Tab'); expect(await panel.evaluate(el => el.contains(document.activeElement))).toBe(true); }
      await panel.evaluate(el => { el.scrollTop = el.scrollHeight; }); expect(await panel.locator('.m-sheet-close').isVisible()).toBe(true);
      if (close === 'x') await panel.locator('.m-sheet-close').click();
      else if (close === 'select') await panel.locator('[aria-selected="true"]').click();
      else if (close === 'back') await page.evaluate(() => (window as any).__sheetBack());
      else await page.keyboard.press('Escape');
      await panel.waitFor({ state: 'detached' }); expect(await trigger.evaluate(el => el === document.activeElement)).toBe(true);
      expect(await page.getByText('Form closed').count()).toBe(0);
    }
    await page.close();
  }, 30_000);
  it('selected grouped row gets focus after reset, and nested info Escape only closes the top layer', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } }); await page.goto(origin + '/__sheet-focus');
    const trigger = page.getByRole('button', { name: 'Grouped choice', exact: true }); await trigger.click();
    await page.waitForFunction(() => document.activeElement?.getAttribute('aria-selected') === 'true');
    const panel = page.locator('.mr-sheet'), info = panel.locator('.m-info-trigger'); await info.click();
    const detail = page.locator('.m-info-layer'); await detail.waitFor();
    await page.evaluate(() => (window as any).__sheetToast());
    const message = page.locator('.m-toast-close'); await message.waitFor();
    const paint = await message.evaluate(el => {
      const rect = el.getBoundingClientRect();
      const hit = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
      return { visible: hit?.closest('.m-toast-host') != null, rect: rect.toJSON(), hit: hit?.outerHTML.slice(0, 180), z: getComputedStyle(el).zIndex };
    });
    expect(paint.visible, JSON.stringify(paint)).toBe(true);
    await message.click();
    for (let index = 0; index < 4; index++) { await page.keyboard.press('Tab'); expect(await detail.evaluate(el => el.contains(document.activeElement))).toBe(true); }
    await page.keyboard.press('Escape'); await detail.waitFor({ state: 'detached' });
    expect(await panel.count()).toBe(1); expect(await info.evaluate(el => el === document.activeElement)).toBe(true);
    await page.keyboard.press('Escape'); await panel.waitFor({ state: 'detached' });
    expect(await trigger.evaluate(el => el === document.activeElement)).toBe(true); expect(await page.getByText('Form closed').count()).toBe(0);
    await trigger.click(); await page.waitForFunction(() => document.activeElement?.getAttribute('aria-selected') === 'true');
    await page.evaluate(() => (window as any).__sheetBack()); await panel.waitFor({ state: 'detached' });
    expect(await trigger.evaluate(el => el === document.activeElement)).toBe(true); await page.close();
  }, 30_000);
});

import path from 'node:path';
import { mkdirSync } from 'node:fs';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
const root = path.resolve(import.meta.dirname, '../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MobileSelect } from '/src/mobile/MobileSelect';
import { MobileInfo } from '/src/mobile/MobileInfo';
import { ActionSheet, SelectSheetPanel, SelectSheetTrigger } from '/src/mobile/screens/rules/Primitives';
import { RuleRow } from '/src/mobile/screens/rules/RuleRow';
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
 const [open,setOpen]=useState(false),[value,setValue]=useState('32'),[formClosed,setFormClosed]=useState(false),[actionOpen,setActionOpen]=useState(false);
 return <><MobileShell active="home" onSelect={()=>{}} toastHost={<MobileToaster/>}><p>Screen fixture</p></MobileShell>{formClosed?<p>Form closed</p>:<FormSheet title="Editing fixture" closeLabel="Close form" cancelLabel="Cancel edit" submitLabel="Save" onSubmit={()=>{}} onRequestClose={()=>setFormClosed(true)}>
  <input aria-label="Fixture text" />
  <button onClick={()=>setActionOpen(true)}>Open actions</button>
  {actionOpen && <ActionSheet title="Rule actions" closeLabel="Close actions" onClose={()=>setActionOpen(false)} actions={[{id:'first',label:'First action'},{id:'last',label:'Last action'}]} body={<MobileInfo title="Nested explanation" summary="Nested details" details={<p>Top-layer details</p>}/>} />
  <MobileSelect aria-label="Simple choice" value={value} onChange={event=>setValue(event.currentTarget.value)}>{options.map(option=><option key={option.id} value={option.id}>{option.label}</option>)}</MobileSelect>
  <SelectSheetTrigger label="Grouped choice" value={value} options={options} open={open} onOpen={()=>setOpen(true)}/>
  <SelectSheetPanel label="Grouped choice" value={value} groups={groups} openGroupIds={defaults} open={open} onClose={()=>setOpen(false)} onSelect={id=>{setValue(id);setOpen(false);}} closeLabel="Close choice" note={<MobileInfo title="Choice explanation" summary="Short explanation" details={<p>Complete explanation</p>}/>}/>
 </FormSheet>}</>;
}

window.__reorders=[];
function DragFixture() {
 const [rows,setRows]=useState(['a','b','c','d']);
 return <div id="drag-list" style={{height:400,overflow:'auto',paddingTop:100}}>{rows.map((id,index)=><RuleRow key={id}
  rule={{id,type:'domain',values:[id+'.example'],action:'proxy',enabled:true,remarks:id}}
  index={index} enabled plane="route" isFirst={index===0} isLast={index===3} deleteConfirming={false} sheetOpen={false}
  t={key=>key} onOpenSheet={()=>{}} onToggle={()=>{}} onMove={()=>{}} onDuplicate={()=>{}} onDelete={()=>{}}
  error={id==='b'?'Variable height '.repeat(20):undefined}
  onReorder={(rule,to)=>{window.__reorders.push({id:rule.id,to});setRows(old=>{const next=old.filter(id=>id!==rule.id);next.splice(to,0,rule.id);return next;});}} />)}</div>;
}
createRoot(document.getElementById('root')).render(location.search.includes('drag')?<DragFixture/>:<App/>);
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
    const panel = page.locator('.mr-sheet'), info = panel.locator('.m-info-trigger');
    const output = path.resolve(root, '../output/playwright'); mkdirSync(output, { recursive: true });
    await panel.evaluate(el => { el.scrollTop = 0; });
    await page.evaluate(() => { document.documentElement.dataset.theme = 'light'; });
    await page.screenshot({ path: path.join(output, 'mobile-info-light-390.png') });
    await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
    await page.screenshot({ path: path.join(output, 'mobile-info-dark-390.png') });
    await info.click();
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
  it('keeps the footer within the visible viewport, with only the remaining gesture inset above IME', async () => {
    const output = path.resolve(root, '../output/playwright'); mkdirSync(output, { recursive: true });
    for (const [width, height, fontScale] of [[390, 844, 1], [320, 400, 2], [768, 700, 1]] as const) {
      const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
      await page.goto(origin + '/__sheet-focus');
      await page.locator('.m-form-foot').waitFor();
      await page.evaluate(({ height, fontScale }) => {
        const root = document.documentElement;
        root.dataset.theme = 'dark'; root.style.setProperty('--safe-t', '32px');
        root.style.setProperty('--safe-b', '20px'); root.style.setProperty('--font-scale', String(fontScale));
        root.style.setProperty('--m-vv-height', `${height}px`);
        root.style.setProperty('--m-vv-occluded-bottom', '0px');
      }, { height, fontScale });
      const geometry = async () => page.evaluate(() => {
        const panel = document.querySelector<HTMLElement>('.m-form-panel')!;
        const foot = document.querySelector<HTMLElement>('.m-form-foot')!;
        const body = document.querySelector<HTMLElement>('.m-form-body')!;
        return {
          panel: panel.getBoundingClientRect().toJSON(), foot: foot.getBoundingClientRect().toJSON(),
          body: body.getBoundingClientRect().toJSON(), paddingBottom: parseFloat(getComputedStyle(foot).paddingBottom),
          panelBg: getComputedStyle(panel).backgroundColor, footBg: getComputedStyle(foot).backgroundColor,
          docWidth: document.documentElement.scrollWidth,
        };
      });
      const normal = await geometry();
      expect(normal.panelBg, `${width} panel/footer background`).toBe(normal.footBg);
      expect(normal.paddingBottom, `${width} gesture inset`).toBe(26);
      expect(normal.foot.bottom, `${width} footer within panel`).toBeLessThanOrEqual(normal.panel.bottom + 0.5);
      expect(normal.docWidth, `${width} horizontal overflow`).toBeLessThanOrEqual(width);
      if (width === 390) await page.screenshot({ path: path.join(output, 'mobile-footer-dark-before-ime-390.png') });
      await page.getByRole('textbox', { name: 'Fixture text' }).focus();
      await page.evaluate(({ height }) => {
        document.documentElement.style.setProperty('--m-vv-height', `${height}px`);
        document.documentElement.style.setProperty('--m-vv-occluded-bottom', '362px');
      }, { height: Math.max(240, Math.round(height * .6)) });
      const ime = await geometry();
      expect(ime.paddingBottom, `${width} covered inset`).toBe(6);
      expect(ime.foot.bottom, `${width} footer in visible panel`).toBeLessThanOrEqual(ime.panel.bottom + 0.5);
      expect(ime.body.height, `${width} scrollable form body`).toBeGreaterThan(0);
      expect(await page.getByRole('textbox', { name: 'Fixture text' }).evaluate(el => el === document.activeElement)).toBe(true);
      if (width === 390) await page.screenshot({ path: path.join(output, 'mobile-footer-dark-ime-390.png') });
      if (width === 320) await page.screenshot({ path: path.join(output, 'mobile-footer-dark-font2-320.png') });
      await page.close();
    }
  }, 30_000);

  it('ActionSheet traps focus, yields Escape to nested details, and restores its trigger', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.goto(origin + '/__sheet-focus');
      const trigger = page.getByRole('button', { name: 'Open actions', exact: true });
      await trigger.click();
      const sheet = page.getByRole('dialog', { name: 'Rule actions', exact: true });
      await page.waitForFunction(() => document.querySelector('[aria-label="Rule actions"]')?.contains(document.activeElement));
      for (let i = 0; i < 6; i++) {
        await page.keyboard.press(i % 2 ? 'Shift+Tab' : 'Tab');
        expect(await sheet.evaluate(el => el.contains(document.activeElement))).toBe(true);
      }
      await sheet.locator('.m-info-trigger').click();
      await page.keyboard.press('Escape');
      expect(await sheet.count()).toBe(1);
      await page.keyboard.press('Escape');
      await sheet.waitFor({ state: 'detached' });
      expect(await trigger.evaluate(el => el === document.activeElement)).toBe(true);
    } finally { await page.close(); }
  });
  it('pointer drag uses layout rectangles with unequal heights, edge grabs, scroll and cancellation', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.goto(origin + '/__sheet-focus?drag');
      const rows = page.locator('#drag-list > .mr-row');
      await rows.first().waitFor();
      await page.locator('#drag-list').evaluate(el => { el.scrollTop = 60; });
      const second = await rows.nth(1).boundingBox();
      const grip = await rows.nth(0).locator('.mr-grip').boundingBox();
      if (!second || !grip) throw new Error('missing real row geometry');
      const x = grip.x + grip.width / 2;
      await page.mouse.move(x, grip.y + 2); await page.mouse.down();
      await page.waitForTimeout(350);
      await page.mouse.move(x, second.y + second.height / 2 - 1);
      await page.mouse.up();
      expect(await page.evaluate(() => (window as any).__reorders)).toEqual([]);
      const currentGrip = await rows.nth(0).locator('.mr-grip').boundingBox();
      if (!currentGrip) throw new Error('missing grip');
      await page.mouse.move(x, currentGrip.y + 2); await page.mouse.down();
      await page.waitForTimeout(350);
      await page.mouse.move(x, second.y + second.height / 2 + 2);
      await page.mouse.up();
      expect(await page.evaluate(() => (window as any).__reorders)).toEqual([{id:'a',to:1}]);
      const nextGrip = await rows.nth(1).locator('.mr-grip').boundingBox();
      if (!nextGrip) throw new Error('missing moved grip');
      await page.mouse.move(nextGrip.x + 2, nextGrip.y + 2); await page.mouse.down();
      await page.waitForTimeout(350);
      await rows.nth(1).locator('.mr-grip').dispatchEvent('pointercancel');
      await page.mouse.up();
      expect(await page.locator('.mr-row.dragging').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__reorders)).toEqual([{id:'a',to:1}]);
    } finally { await page.close(); }
  });
});

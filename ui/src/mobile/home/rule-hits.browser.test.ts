import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { HomeScreenView } from '/src/mobile/home/HomeScreenView';
import { MobileShell } from '/src/mobile/MobileShell';
import { FormSheet } from '/src/mobile/forms/FormSheet';
import { MobileSelect } from '/src/mobile/MobileSelect';
import { dismissTop } from '/src/mobile/back-stack';
import i18n, { i18nReady } from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/home/home.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
await i18nReady; i18n.addResourceBundle('zh-CN','translation',zhCN,true,true); await i18n.changeLanguage('zh-CN');
const noop=()=>{};
const hits=Array.from({length:30},(_,index)=>({key:'named:规则 '+index,rule:'规则 '+index,kind:'named',policy:index%2?'direct':'proxied',count:index+1}));
hits.push({key:'policy:direct',rule:'direct',kind:'policy',policy:'direct',count:2});
const fixture=window.__ruleHits={back:dismissTop,picked:null};
function App(){
 const [ruleHits,setHits]=useState(hits), [form,setForm]=useState(false); fixture.setHits=setHits; fixture.openForm=()=>setForm(true);
 const props={t:(key,options)=>i18n.t(key,options),writeErrors:{},core:'stopped',connState:'disconnected',node:{kind:'direct'},
  onToggleConnect:noop,onNetworkCheck:noop,tsExitWarning:'none',onTsExitAction:noop,
  pickerOpen:false,onOpenPicker:noop,onClosePicker:noop,pickerQuery:'',onPickerQuery:noop,pickRows:[],onUseAsExit:noop,onPickSentinel:noop,
  blockDisabledReason:null,noServers:false,onAddServer:noop,onAddSubscription:noop,routing:'smart',reverseRouting:false,onSetRouting:noop,
  exitRegion:{kind:'unknown'},exitFlagSrc:null,exitIsDirect:true,exitProbing:false,exitIp:null,
  unlock:[],unlockRunning:false,unlockDetailId:null,onOpenUnlockDetail:noop,onCloseUnlockDetail:noop,
  samples:[],composition:[],ruleHits,hosts:[],hostsMasked:false,hostQuery:'',onHostQuery:noop,
  ruleSubject:null,ruleSubjectView:'menu',onOpenRuleSubject:noop,onCloseRuleSubject:noop,onRuleSubjectView:noop,onQuickRule:noop,
  newRuleAction:'proxy',onNewRuleAction:noop,newRuleRemarks:'',onNewRuleRemarks:noop,ruleRemarksHint:'',onCreateRule:noop,
  appendQuery:'',onAppendQuery:noop,appendTargets:[],onAppendToRule:noop,
  windowBytes:0,windowConnections:ruleHits.reduce((sum,hit)=>sum+hit.count,0),kernelConnections:0,
  onOpenRuleHit:hit=>fixture.picked=hit};
 return <><MobileShell active="home" onSelect={noop}><HomeScreenView {...props}/></MobileShell>{form&&<FormSheet title="Scroll fixture" closeLabel="Close" cancelLabel="Cancel" onRequestClose={()=>setForm(false)}>
   <MobileSelect aria-label="Scroll choices" value="0" onChange={noop}>{Array.from({length:60},(_,i)=><option key={i} value={String(i)}>Choice {i}</option>)}</MobileSelect>
   <textarea aria-label="Scroll text" style={{height:80}} defaultValue={Array.from({length:60},(_,i)=>'Line '+i).join('\\n')}/>
   {Array.from({length:60},(_,i)=><p key={i}>Field {i}</p>)}
 </FormSheet>}</>;
}
createRoot(document.getElementById('root')).render(<App/>);
`;
let server: ViteDevServer; let browser: Browser; let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('all rule hits in real Chromium', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-rule-hits-'+process.pid), server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
      name: 'rule-hits-fixture', resolveId(id) { if (id === '/rule-hits-fixture.tsx') return id; },
      load(id) { if (id === '/rule-hits-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__rule-hits', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__rule-hits', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/rule-hits-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen(); const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:'+address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('all aggregates remain reachable while the overview keeps its horizontal/two-row layout', async () => {
    for (const [width, scale] of [[390,1],[1040,1],[320,2]]) {
      const page = await browser.newPage({ viewport: { width, height: 844 } }); await page.goto(origin+'/__rule-hits');
      await page.locator('.h-rulehits-all').waitFor();
      await page.evaluate(scale => { document.documentElement.style.fontSize = `${16*scale}px`; }, scale);
      const track = page.locator('.h-rings');
      const overview = await track.evaluate(el => ({ flow: getComputedStyle(el).gridAutoFlow, height: el.clientHeight, rows: [...el.children].map(node => (node as HTMLElement).offsetTop) }));
      if (width === 1040) expect(overview.rows.filter(top => top < overview.rows[0]+overview.height).filter((top,index,all)=>all.indexOf(top)===index).length).toBeLessThanOrEqual(2);
      else expect(overview.flow).toBe('column');
      await page.locator('.h-rulehits-all').click();
      const sheet = page.locator('.h-rulehits-sheet'), body = sheet.locator('.h-rulehits-body');
      await sheet.waitFor(); expect(await sheet.locator('.h-ringcell').count()).toBe(31);
      expect(await body.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
      const heading = await sheet.locator('.h-sheethead').boundingBox(), bounds = await sheet.boundingBox();
      expect(heading!.y).toBeGreaterThanOrEqual(bounds!.y);
      expect(heading!.y+heading!.height).toBeLessThanOrEqual(bounds!.y+bounds!.height);
      expect(await sheet.locator('.h-rulehits-grid').evaluate(el => getComputedStyle(el).gridTemplateColumns.split(' ').length)).toBe(width===1040 ? 2 : 1);
      const triggerBounds = await page.locator('.h-rulehits-all').boundingBox(), trackBounds = await track.boundingBox();
      expect(triggerBounds!.y+triggerBounds!.height).toBeLessThanOrEqual(trackBounds!.y);
      await sheet.getByRole('button', { name: '规则 29 · 30', exact: true }).click();
      await sheet.waitFor({ state: 'detached' });
      expect(await page.evaluate(() => (window as any).__ruleHits.picked)).toEqual({ key: 'named:规则 29', label: '规则 29' });
      await page.close();
    }
  }, 30_000);

  it('hidden scrollbar chrome leaves page, horizontal/vertical tracks, body portals and textarea scrolling intact', async () => {
    const page=await browser.newPage({viewport:{width:390,height:844}}); await page.goto(origin+'/__rule-hits');
    const track=page.locator('.h-rings'), scroller=page.locator('.m-scroll'); await track.waitFor();
    const hidden=async(selector:string)=>{
      expect(await page.locator(selector).evaluate(el=>getComputedStyle(el).scrollbarWidth)).toBe('none');
      expect(await page.locator(selector).evaluate(el=>getComputedStyle(el,'::-webkit-scrollbar').display)).toBe('none');
    };
    await hidden('.m-scroll'); await hidden('.h-rings');
    await scroller.hover({position:{x:5,y:15}}); await page.mouse.wheel(0,300);
    await page.waitForFunction(()=>document.querySelector('.m-scroll')!.scrollTop>0);
    await track.hover(); await page.mouse.wheel(350,0);
    await page.waitForFunction(()=>document.querySelector('.h-rings')!.scrollLeft>0);
    await page.locator('.h-rulehits-all').click(); await hidden('.h-rulehits-body');
    const body=page.locator('.h-rulehits-body'); await body.hover(); await page.mouse.wheel(0,10000);
    await page.waitForFunction(()=>{const el=document.querySelector('.h-rulehits-body')!;return el.scrollTop+el.clientHeight>=el.scrollHeight-1;});
    expect(await page.locator('.h-rulehits-sheet .m-sheet-close').isVisible()).toBe(true);
    await page.keyboard.press('Escape');
    await page.setViewportSize({width:1040,height:844}); await track.hover(); await page.mouse.wheel(0,350);
    await page.waitForFunction(()=>document.querySelector('.h-rings')!.scrollTop>0);
    await page.evaluate(()=>(window as any).__ruleHits.openForm()); await page.locator('.m-form-body').waitFor();
    await hidden('.m-form-body'); await hidden('textarea');
    await page.locator('textarea').focus(); await page.keyboard.press('Control+End');
    expect(await page.locator('textarea').evaluate(el=>el.scrollTop)).toBeGreaterThan(0);
    await page.locator('.m-form-body').hover(); await page.mouse.wheel(0,350);
    await page.waitForFunction(()=>document.querySelector('.m-form-body')!.scrollTop>0);
    await page.getByRole('combobox',{name:'Scroll choices'}).click(); await page.locator('.m-select-panel').waitFor(); await hidden('.m-select-panel');
    await page.getByRole('option',{name:'Choice 59',exact:true}).focus();
    expect(await page.locator('.m-select-panel').evaluate(el=>el.scrollTop)).toBeGreaterThan(0);
    await page.keyboard.press('Escape'); expect(await page.locator('.m-form-panel').count()).toBe(1);
    await page.close();
  },30_000);

  it('close paths restore focus, updates stay live, and named/fallback keys retain exact identity', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } }); await page.goto(origin+'/__rule-hits');
    const trigger = page.locator('.h-rulehits-all'), sheet = page.locator('.h-rulehits-sheet');
    for (const method of ['Escape','x','back','scrim']) {
      await trigger.click(); await sheet.waitFor();
      for (let index=0;index<35;index++) { await page.keyboard.press('Tab'); expect(await sheet.evaluate(el => el.contains(document.activeElement))).toBe(true); }
      if (method==='x') await sheet.locator('.m-sheet-close').click();
      else if (method==='back') await page.evaluate(() => (window as any).__ruleHits.back());
      else if (method==='scrim') await page.locator('.h-scrim').click({position:{x:4,y:4}});
      else await page.keyboard.press('Escape');
      await sheet.waitFor({state:'detached'}); expect(await trigger.evaluate(el => el===document.activeElement)).toBe(true);
    }
    await trigger.click(); await sheet.waitFor();
    await page.evaluate(() => (window as any).__ruleHits.setHits([{key:'named:直连',rule:'直连',kind:'named',policy:'proxied',count:9},{key:'policy:direct',rule:'direct',kind:'policy',policy:'direct',count:3}]));
    await sheet.getByRole('button',{name:'直连 · 9',exact:true}).waitFor(); expect(await trigger.innerText()).toContain('2');
    await sheet.getByRole('button',{name:'直连 · 3',exact:true}).click(); await sheet.waitFor({state:'detached'});
    expect(await page.evaluate(() => (window as any).__ruleHits.picked)).toEqual({key:'policy:direct',label:'直连'});
    await trigger.click(); await sheet.waitFor();
    await page.evaluate(() => (window as any).__ruleHits.setHits([]));
    await sheet.getByRole('status').waitFor(); expect(await sheet.locator('.h-ringcell').count()).toBe(0);
    expect(await sheet.getByRole('status').innerText()).toBe('暂无活动连接');
    await page.keyboard.press('Escape'); await sheet.waitFor({state:'detached'}); expect(await trigger.count()).toBe(0);
    await page.close();
  }, 30_000);
});

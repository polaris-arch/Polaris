import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root=path.resolve(import.meta.dirname,'../../../..');
const entry=`
import React from 'react';
import { createRoot } from 'react-dom/client';
import { ResourcesSegment } from '/src/mobile/screens/rules/ResourcesSegment';
import { MobileShell } from '/src/mobile/MobileShell';
import i18n,{i18nReady} from '/src/i18n';
import zhCN from '/src/i18n/locales/zh-CN.json';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/screens/rules/rules-screen.css';
import '/src/mobile/redesign.css';
import '/src/mobile/screens/rules/rules-redesign.css';
await i18nReady;i18n.addResourceBundle('zh-CN','translation',zhCN,true,true);await i18n.changeLanguage('zh-CN');
const noop=()=>{};
const rows=[
 {id:'builtin',name:'智能分流 · 工作服务与备用域名规则资源名称（长名称布局验收）',builtin:true,references:0,size:123456,progress:.47},
 {id:'failed',name:'external-custom-rules-resource-with-a-very-long-name-for-layout-review',builtin:false,references:21,size:123456,failed:true},
 {id:'download',name:'自定义更新中的资源',builtin:false,references:2,size:123456,progress:.83},
];
createRoot(document.getElementById('root')).render(<MobileShell active="rules" onSelect={noop}><div className="mr-screen">
 <ResourcesSegment t={(key,vars)=>i18n.t(key,vars)} source="all" onSourceChange={noop} loading={false} error={false}
  groups={[{key:'Geosite',label:'Geosite',rows}]} updatingAll={false} updatingIds={new Set(['builtin','download'])}
  onUpdateAll={noop} onResetBuiltin={noop} resetConfirming={false} onUpdateOne={noop} onCancel={noop} onDelete={noop}
  errorOf={key=>key==='res:failed'?'更新失败，请检查网络后重试。':undefined} deleteConfirmingId={null}
  sheetId={null} onOpenSheet={noop} onCatalog={noop} onUrlDownload={noop}/>
 </div></MobileShell>);
`;
let server:ViteDevServer,browser:Browser,origin:string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS==='1')('resource progress layout in real Chromium',()=>{
 beforeAll(async()=>{
  server=await createServer({root,cacheDir:path.join(tmpdir(),'polaris-resource-progress-'+process.pid),server:{host:'127.0.0.1',port:0,watch:null},plugins:[{
   name:'resource-progress-fixture',resolveId(id){if(id==='/resource-progress-fixture.tsx')return id;},load(id){if(id==='/resource-progress-fixture.tsx')return entry;},
   configureServer(vite){vite.middlewares.use('/__resource-progress',async(_req,res)=>{res.setHeader('Content-Type','text/html');res.end(await vite.transformIndexHtml('/__resource-progress','<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/resource-progress-fixture.tsx"></script></html>'));});},
  }]});await server.listen();const address=server.httpServer!.address();if(!address||typeof address!=='object')throw new Error('Vite did not bind');origin='http://127.0.0.1:'+address.port;
  browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH});
 },30_000);
 afterAll(async()=>{await browser?.close();await server?.close();});
 it('status badges, errors and progress have separate space; long names and actions stay reachable',async()=>{
  for(const [width,scale] of [[320,1],[390,1],[1040,1],[320,2]]){
   const page=await browser.newPage({viewport:{width,height:844}});await page.goto(origin+'/__resource-progress');await page.locator('.rc-row').first().waitFor();
   await page.evaluate(scale=>{document.documentElement.style.setProperty('--font-scale',String(scale));document.documentElement.dataset.theme='dark';},scale);
   const geometry=await page.locator('.rc-row').evaluateAll(rows=>rows.map(row=>{
    const body=row.querySelector('.rc-row-body')!,status=row.querySelector('.rc-status')!,bar=row.querySelector('.mr-progress'),actions=row.querySelector('.rc-row-actions')!;
    const b=body.getBoundingClientRect(),s=status.getBoundingClientRect(),a=actions.getBoundingClientRect(),p=bar?.getBoundingClientRect();
    return {gap:p?p.top-s.bottom:null,barHeight:p?.height,barBottom:p?.bottom,bodyBottom:b.bottom,bodyRight:b.right,actionLeft:a.left,rowWidth:row.clientWidth,rowScrollWidth:row.scrollWidth};
   }));
   console.log(JSON.stringify({width,scale,geometry}));
   for(const row of geometry){
    if(row.gap!==null){expect(row.gap).toBeGreaterThanOrEqual(8);expect(row.barHeight).toBe(4);expect(row.barBottom).toBeLessThanOrEqual(row.bodyBottom);}
    expect(row.bodyRight).toBeLessThanOrEqual(row.actionLeft);expect(row.rowScrollWidth).toBeLessThanOrEqual(row.rowWidth);
   }
   expect(await page.locator('[role="progressbar"]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute('aria-valuenow')))).toEqual(['47','83']);
   expect(await page.locator('.rc-status-item.warn').innerText()).toBe('更新失败');
   expect(await page.locator('.mr-error').innerText()).toContain('更新失败，请检查网络后重试。');
   expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBe(width);
   if(process.env.POLARIS_RESOURCE_SCREENSHOTS==='1')await page.screenshot({path:'/home/sway/output/playwright/resource-progress-layout-theme-'+width+(scale===2?'-font2':'')+'.png'});
   await page.close();
  }
 },30_000);
});

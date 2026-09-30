import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { MobileField } from '/src/mobile/forms/FormFields';
import { SettingsRow, SettingsNote, MobileSwitch } from '/src/mobile/settings/SettingsChrome';
import { MobileInfo } from '/src/mobile/MobileInfo';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
await i18nReady; i18n.addResourceBundle('zh-CN', 'translation', zhCN, true, true); await i18n.changeLanguage('zh-CN');
const t = (key) => i18n.t(key);
const test = window.__helpTest = { changes: 0 };
const field = { t: 'text', k: 'pin', label: 'node.field.certSha256', hint: 'node.field.certPinHint' };
createRoot(document.getElementById('root')).render(<main className="mobile-root">
  <MobileField spec={field} value="fixture-pin" t={t} onChange={() => test.changes++} />
  <MobileField spec={{t:'switch',k:'mesh',label:'wg.reverseMesh',hint:'wg.reverseMeshHint',disabled:true,disabledHint:'wg.reverseMeshWarp'}} value={false} t={t} onChange={() => test.changes++} />
  <SettingsRow id="logs" label="日志" desc={t('mobileHelp.disableLogs')} descDetails={t('settings.advanced.disableLogFileDescFull')} problem="当前校验错误" hint="当前不可操作原因" control={<MobileSwitch checked={false} ariaLabel="日志开关" onChange={() => test.changes++} />} />
  <SettingsNote id="backup" title="系统备份" summary={t('mobileHelp.systemBackup')}>{t('mobileSettings.backup.systemBackupDesc')}</SettingsNote>
  <div className="m-form-hint" data-auth-switch><MobileInfo title="Auth Key" summary={t('mobileHelp.tsAuthKeySwitch')} details={t('ts.authKeySwitchLogoutNote')} /></div>
  <div data-dynamic><MobileInfo title="动态规则" summary="匹配范围" details={<p>网段：10.0.0.0/8；名单：fixture-a；数量：3</p>} /></div>
</main>);
`;
let server: ViteDevServer;
let browser: Browser;
let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile authored help consumers', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-help-consumer-vite-' + process.pid), server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
      name: 'help-consumer-fixture',
      resolveId(id) { if (id === '/help-consumer-fixture.tsx') return id; },
      load(id) { if (id === '/help-consumer-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__help', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__help', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/help-consumer-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('physical i clicks preserve fields/switches and show complete certificate/security text', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    page.setDefaultTimeout(8_000);
    await page.goto(origin + '/__help');
    const field = page.locator('.m-form-row').first();
    await field.getByRole('button').click();
    const dialog = page.locator('.m-info-layer');
    await dialog.waitFor();
    expect(await dialog.innerText()).toContain('取代 CA 与域名校验');
    expect(await dialog.innerText()).toContain('只比对服务器证书本身');
    expect(await dialog.innerText()).toContain('Base64');
    await page.keyboard.press('Escape');
    await page.locator('.m-info-layer').waitFor({ state: 'detached' });
    expect(await page.locator('.m-info-layer').count()).toBe(0);
    expect(await field.locator('input').inputValue()).toBe('fixture-pin');
    expect(await page.evaluate(() => (window as any).__helpTest.changes)).toBe(0);
    expect(await field.getByRole('button').evaluate(node => node === document.activeElement)).toBe(true);
    expect(await page.locator('label button, p .m-info-trigger, button button').count()).toBe(0);
    const disabled = page.locator('.m-form-switch');
    expect(await disabled.locator('.m-info-trigger').count()).toBe(0);
    expect(await disabled.innerText()).toContain('WARP');
    expect(await disabled.getByRole('switch').isDisabled()).toBe(true);
    await page.close();
  }, 30_000);

  it('visible consequences/errors remain outside details and i never toggles settings', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    page.setDefaultTimeout(8_000);
    await page.goto(origin + '/__help');
    const logs = page.locator('[data-setting="logs"]');
    await logs.locator('.m-info-trigger').click();
    expect(await page.locator('.m-info-layer').innerText()).toContain('崩溃检测仍可用');
    expect(await page.evaluate(() => (window as any).__helpTest.changes)).toBe(0);
    await page.keyboard.press('Escape');
    await page.locator('.m-info-layer').waitFor({ state: 'detached' });
    expect(await logs.innerText()).toContain('实时日志与日志诊断不可用');
    expect(await logs.innerText()).toContain('当前校验错误');
    expect(await logs.innerText()).toContain('当前不可操作原因');
    expect(await logs.getByRole('switch').getAttribute('aria-checked')).toBe('false');
    const backup = page.locator('[data-note="backup"]');
    expect(await backup.innerText()).toContain('节点凭据和订阅');
    expect(await backup.innerText()).toContain('Google');
    expect(await backup.innerText()).toContain('Android 9+');
    await backup.locator('.m-info-trigger').click();
    expect(await page.locator('.m-info-layer').innerText()).toContain('上面的导出不受此开关影响');
    await page.keyboard.press('Escape');
    await page.locator('.m-info-layer').waitFor({ state: 'detached' });
    expect(await page.locator('[data-auth-switch]').innerText()).toContain('退出当前登录');
    await page.locator('[data-dynamic] .m-info-trigger').click();
    expect(await page.locator('.m-info-layer').innerText()).toContain('网段：10.0.0.0/8；名单：fixture-a；数量：3');
    await page.close();
  }, 30_000);
});

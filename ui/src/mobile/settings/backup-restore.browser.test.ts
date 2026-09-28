import path from 'node:path';
import { tmpdir } from 'node:os';
import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BackupPage } from '/src/mobile/settings/BackupPage';
import { createCommit, WriteErrorsContext } from '/src/mobile/settings/write-feedback';
import { MobileFormHost } from '/src/mobile/forms/MobileFormHost';
import { MobileToaster } from '/src/mobile/MobileToaster';
import { api } from '/src/ipc/api-client';
import zhCN from '/src/i18n/locales/zh-CN.json';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/settings/settings.css';
import '/src/mobile/redesign.css';
await i18nReady;
i18n.addResourceBundle('zh-CN', 'translation', zhCN, true, true);
await i18n.changeLanguage('zh-CN');
const control = window.__backupRestoreTest = { calls: [], result: { success: true } };
api.systemBackup.getStatus = async () => false;
api.backup.importPick = async () => ({ canceled: false, filePath: '/tmp/test.polaris',
  available: ['manualNodes', 'subscriptions'], counts: { manualNodes: 2, subscriptions: 3 } });
api.backup.importApply = async (filePath, categories) => {
  control.calls.push({ filePath, categories });
  return control.result;
};
function App() {
  const [errors, setErrors] = useState({});
  const commit = useMemo(() => createCommit(setErrors, i18n.t), []);
  return <main className="mobile-root"><WriteErrorsContext.Provider value={errors}>
    <BackupPage commit={commit} />
  </WriteErrorsContext.Provider><MobileFormHost /><div className="m-dock"><MobileToaster /></div></main>;
}
createRoot(document.getElementById('root')).render(<App />);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile backup restore confirmation', () => {
  beforeAll(async () => {
    server = await createServer({ root, cacheDir: path.join(tmpdir(), 'polaris-backup-restore-vite-' + process.pid),
      server: { host: '127.0.0.1', port: 0, watch: null }, plugins: [{
        name: 'backup-restore-fixture',
        resolveId(id) { if (id === '/backup-restore-fixture.tsx') return id; },
        load(id) { if (id === '/backup-restore-fixture.tsx') return entry; },
        configureServer(vite) { vite.middlewares.use('/__backup-restore', async (_req, res) => {
          const html = await vite.transformIndexHtml('/__backup-restore',
            '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/backup-restore-fixture.tsx"></script></html>');
          res.setHeader('Content-Type', 'text/html'); res.end(html);
        }); },
      }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address !== 'object') throw new Error('Vite did not bind');
    origin = 'http://127.0.0.1:' + address.port;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('requires confirmation, preserves preview on cancel/failure, rejects stale scope, then reports success', async () => {
    const page = await browser.newPage({ viewport: { width: 320, height: 740 } });
    try {
      await page.goto(origin + '/__backup-restore');
      await page.getByRole('button', { name: '导入…' }).click();
      await page.getByText('文件：test.polaris').waitFor();
      const restore = page.locator('[data-setting="backup-import-actions"]').getByRole('button', { name: '恢复所选' });
      await restore.evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
      const dialog = page.getByRole('dialog', { name: '确认恢复所选类别？' });
      await dialog.waitFor();
      expect(await page.getByRole('dialog').count()).toBe(1);
      expect(await dialog.innerText()).toContain('选中类别将被备份内容整体替换；未选类别保留不变。');
      expect(await dialog.innerText()).toContain('手动节点 · 订阅（含展开节点）');
      expect(await page.evaluate(() => (window as any).__backupRestoreTest.calls)).toEqual([]);
      await dialog.getByRole('button', { name: '取消' }).click();
      await dialog.waitFor({ state: 'detached' });
      expect(await restore.isVisible()).toBe(true);

      await restore.click();
      await dialog.waitFor();
      await page.locator('[data-setting="backup-import-subscriptions"] [role="switch"]').evaluate((input: HTMLButtonElement) => input.click());
      await dialog.getByRole('button', { name: '恢复所选' }).click();
      await dialog.waitFor({ state: 'detached' });
      expect(await page.evaluate(() => (window as any).__backupRestoreTest.calls)).toEqual([]);

      await page.evaluate(() => { (window as any).__backupRestoreTest.result = { success: false, errorCode: 'saveFailed' }; });
      await restore.click();
      await dialog.getByRole('button', { name: '恢复所选' }).click();
      await page.locator('[data-write-error="backup-import-actions"]').waitFor();
      expect(await restore.isVisible()).toBe(true);
      expect(await page.locator('.m-toast-ok').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__backupRestoreTest.calls)).toEqual([
        { filePath: '/tmp/test.polaris', categories: ['manualNodes'] },
      ]);

      await page.evaluate(() => { (window as any).__backupRestoreTest.result = { success: true, unavailableInterfaceBindings: 2 }; });
      await restore.click();
      await dialog.getByRole('button', { name: '恢复所选' }).click();
      await page.locator('.m-toast-ok').getByText('已恢复备份；其中 2 处本机不存在的网卡绑定已改为自动或继承。').waitFor();
      expect(await restore.count()).toBe(0);
      expect(await page.locator('[data-hint="backup-actions"]').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__backupRestoreTest.calls)).toHaveLength(2);
    } finally { await page.close(); }
  }, 45_000);
});

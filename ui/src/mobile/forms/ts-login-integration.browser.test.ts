import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// The app-level IPC feed and the actual mobile panel share a browser instance. Synthetic URLs
// and a local API replacement avoid opening a real browser or contacting a control server.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { api, unlockApi } from '/src/ipc';
import { useAppStore } from '/src/store/app-store';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { useTailscaleLoginProgressStore } from '/src/store/use-tailscale-login-progress-store';
import { TsLoginPanel } from '/src/mobile/forms/TsLoginPanel';
import { MobileToaster } from '/src/mobile/MobileToaster';
import { startMobileAppWiring } from '/src/mobile/app-wiring';
import { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
import '/src/mobile/screens/rules/rules-redesign.css';
import '/src/mobile/connections/connections-redesign.css';
await i18nReady;
const mode = new URLSearchParams(location.search).get('mode');
const listeners = new Map();
const on = name => fn => { const set = listeners.get(name) || new Set(); listeners.set(name, set); set.add(fn); return () => set.delete(fn); };
const bus = (domain, names) => { for (const name of names) api[domain][name] = on(name); };
bus('proxy', ['onStarted','onStopped','onLifecycle','onPendingChanges','onError','onInvalidNodes','onTailscaleStatus','onTailscaleAuth','onTailscaleLoginProgress','onMeshLoginFallback','onAutoNodeSwitched']);
bus('vpn', ['onOpenVpnStatus']);
bus('subscription', ['onUpdateProgress','onAutoUpdate']);
bus('config', ['onEnterPrivacyMode','onExitPrivacyMode']);
bus('ipInfo', ['onUpdated']);
bus('server', ['onTaildropTaskUpdated']);
for (const name of ['onProgress','onUpdated','onInvalidated']) unlockApi[name] = on(name);
api.proxy.getPendingChanges = async () => ({ added: [], modified: [], removed: [], restartDeferred: false });
api.vpn.getStatus = async () => ({ connected: false, openConnect: [], openVpn: [] });
api.config.getPrivacyMode = async () => false;
api.ipInfo.peek = async () => ({ direct: null, proxy: null, updatedAt: 0, revision: 0 });
unlockApi.get = async () => null;
api.subscription.onCreateProgressReady = async fn => on('onCreateProgressReady')(fn);
api.subscription.createList = async () => [];
api.server.taildropTasks = async () => [];
api.server.tailscaleStateExists = async () => {
  if (mode === 'state-read-fail') throw Error('state unavailable');
  return { 'ts-1': mode === 'state-true' };
};
const test = window.__tsTest = { opens: [], cancels: [], starts: 0, saves: 0, prepares: 0, releasePrepare: null, releaseStart: null, releaseSave: null, releaseProgress: null, holdProgress: false, failProgress: false, receipt: null, mode };
api.server.tailscaleLoginProgress = async (serverId, attemptId) => {
  if (test.holdProgress) await new Promise(resolve => { test.releaseProgress = resolve; });
  if (test.failProgress) throw new Error('RECEIPT_READ_FAILED');
  return test.receipt?.serverId === serverId && test.receipt?.attemptId === attemptId ? test.receipt : null;
};
api.server.tailscaleGetStatus = async () => mode?.startsWith('main') && test.starts > 0
  ? { connected: true, statuses: [{ serverId: 'ts-1', backendState: 'Running', loggedIn: true,
      expired: false, peers: [], tailscaleIPs: [], canShareFiles: false,
      waitingFileCount: 0, receivingFileCount: 0, unreadFileCount: 0 }] }
  : { connected: false, statuses: [] };
api.system.openExternal = async url => { test.opens.push(url); };
api.server.tailscaleLoginPrepare = async () => {
  test.prepares++;
  if (mode === 'prepare') await new Promise(resolve => { test.releasePrepare = resolve; });
};
api.server.tailscaleLoginCancel = async (serverId, attemptId) => { test.cancels.push([serverId,attemptId]); };
api.server.tailscaleLogin = async () => {
  test.starts++;
  if (mode === 'early-start') await new Promise(resolve => { test.releaseStart = resolve; });
  if (mode?.startsWith('main')) return { started: false, reason: 'inMainCore',
    configurationPending: mode === 'main-pending' };
  return { started: true };
};
const server = { id: 'ts-1', name: 'Tailscale', protocol: 'tailscale', address: '', port: 0, tailscaleSettings: {} };
useAppStore.setState({ servers: [server], config: { servers: [server], subscriptions: [] },
  refreshProxyStatus: async () => {}, loadConfig: async () => {} });
api.server.update = async next => {
  test.saves++;
  if (mode === 'delayed-save') await new Promise(resolve => { test.releaseSave = resolve; });
  useAppStore.setState({ servers: [next], config: { servers: [next], subscriptions: [] } });
};
const off = startMobileAppWiring(key => key);
test.emit = (name, payload) => { for (const fn of listeners.get(name) || []) fn(payload); };
test.attempt = () => useTailscaleLoginProgressStore.getState().attempts['ts-1'];
test.begin = id => useTailscaleLoginProgressStore.getState().begin('ts-1', id);
test.authUrl = () => useAppStore.getState().tailscaleAuthUrls['ts-1'];
test.initiated = () => useAppStore.getState().tailscaleLoginInitiated['ts-1'];
test.stop = off;
function Host() {
  const stack = useMobileFormStore(s => s.stack);
  return stack.map(form => form.kind === 'ts-login'
    ? <TsLoginPanel key={form.instanceId} instanceId={form.instanceId} serverId={form.serverId} /> : null);
}
useMobileFormStore.getState().open({kind:'ts-login', serverId:'ts-1'});
createRoot(document.getElementById('root')).render(<main className="mobile-root"><Host /><MobileToaster /></main>);
`;

let server: ViteDevServer;
let browser: Browser;
let origin: string;
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile TS attempt lifecycle in real Chromium', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0, strictPort: false }, plugins: [{
      name: 'ts-login-browser-fixture',
      resolveId(id) { if (id === '/ts-login-fixture.tsx') return id; },
      load(id) { if (id === '/ts-login-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__ts-login', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__ts-login', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/ts-login-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (typeof address !== 'object' || !address) throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('URL arrived → close revokes attempt before native cleanup; late AUTH and progress cannot reopen', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.attempt()?.phase === 'starting');
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'awaitingAuth', url: 'https://login.example/auth' });
      });
      await page.getByText('https://login.example/auth').waitFor();
      expect(await page.evaluate(() => (window as any).__tsTest.opens)).toEqual(['https://login.example/auth']);
      const attemptId = await page.evaluate(() => (window as any).__tsTest.attempt().attemptId as string);
      await page.locator('.m-form-head .m-form-x').click();
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      await page.evaluate(id => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: id,
          phase: 'awaitingAuth', url: 'https://login.example/late' });
        test.emit('onTailscaleAuth', { serverId: 'ts-1', nodeName: 'n', url: 'https://login.example/late' });
      }, attemptId);
      expect(await page.evaluate(() => (window as any).__tsTest.opens)).toEqual(['https://login.example/auth']);
      expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('cancelled');
      expect(await page.evaluate(() => (window as any).__tsTest.cancels.length)).toBeGreaterThan(0);
    } finally { await page.close(); }
  }, 30_000);

  it('closing while prepare is pending prevents save/start and still cancels the registered attempt', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login?mode=prepare`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.prepares === 1);
      await page.locator('.m-form-head .m-form-x').click();
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      await page.evaluate(() => (window as any).__tsTest.releasePrepare());
      await page.waitForFunction(() => (window as any).__tsTest.cancels.length >= 2);
      expect(await page.evaluate(() => (window as any).__tsTest.starts)).toBe(0);
      expect(await page.evaluate(() => (window as any).__tsTest.saves)).toBe(0);
      expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('cancelled');
    } finally { await page.close(); }
  }, 30_000);

  it('old panel A cannot render or clear newer attempt B on the same server', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.attempt()?.phase === 'starting');
      const oldId = await page.evaluate(() => (window as any).__tsTest.attempt().attemptId as string);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.begin('attempt-b');
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: 'attempt-b',
          phase: 'awaitingAuth', url: 'https://login.example/owned-by-b' });
      });
      expect(await page.getByText('https://login.example/owned-by-b').count()).toBe(0);
      await page.locator('.m-form-head .m-form-x').click();
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      expect(await page.evaluate(() => (window as any).__tsTest.authUrl())).toBe('https://login.example/owned-by-b');
      expect(await page.evaluate(() => (window as any).__tsTest.attempt())).toMatchObject({ attemptId: 'attempt-b', phase: 'awaitingAuth' });
      expect(await page.evaluate(() => (window as any).__tsTest.initiated())).toBe(true);
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toContainEqual(['ts-1', oldId]);
    } finally { await page.close(); }
  }, 30_000);

  it('confirmed authorization closes its own panel without cancelling or keeping the submitted notice', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'authorized', url: null });
      });
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      await page.locator('.m-toast').filter({ hasText: '授权已完成' }).waitFor();
      expect(await page.locator('.m-toast').filter({ hasText: '授权已完成' }).count()).toBe(1);
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
      expect(await page.evaluate(() => (window as any).__tsTest.initiated())).not.toBe(true);
      expect(await page.evaluate(() => (window as any).__tsTest.authUrl())).toBeFalsy();
    } finally { await page.close(); }
  }, 30_000);

  it('authorization may arrive before the start receipt; its late receipt does not revive the panel', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login?mode=early-start`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => !!(window as any).__tsTest.releaseStart);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'authorized', url: null });
      });
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      await page.evaluate(() => (window as any).__tsTest.releaseStart());
      await page.waitForTimeout(50);
      expect(await page.getByRole('dialog').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
    } finally { await page.close(); }
  }, 30_000);

  it('returning from the browser reconciles an authorized receipt when its event was missed', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.receipt = { serverId: 'ts-1', attemptId: test.attempt().attemptId, phase: 'authorized' };
        window.dispatchEvent(new Event('focus'));
      });
      await page.getByRole('dialog').waitFor({ state: 'detached', timeout: 1500 });
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
    } finally { await page.close(); }
  }, 30_000);

  it('a failed receipt read reports unknown status and a later read clears it without changing authorization', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').waitFor();
      await page.evaluate(() => { (window as any).__tsTest.failProgress = true; });
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      const message = page.getByText('暂时无法核对本次授权结果。授权可能仍在进行；返回应用时会重试。');
      await message.waitFor();
      expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('starting');
      await page.evaluate(() => {
        (window as any).__tsTest.failProgress = false;
        window.dispatchEvent(new Event('focus'));
      });
      await message.waitFor({ state: 'detached' });
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'authorized' });
      });
      await page.getByRole('dialog').waitFor({ state: 'detached' });
    } finally { await page.close(); }
  }, 30_000);

  it('an unknown old session stays visible and Auth Key submission fails closed when the re-read fails', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login?mode=state-read-fail`);
      await page.getByRole('button', { name: 'Auth Key' }).click();
      await page.getByText('旧登录状态暂无法确认；提交 Auth Key 前会重新读取，读取失败则不会更换账号。').waitFor();
      await page.locator('#mts-authkey').fill('synthetic-test-key');
      await page.locator('.m-form-foot .primary').click();
      await page.locator('.m-form-notice.err').waitFor();
      expect(await page.locator('.m-form-notice.err').textContent()).toMatch(
        /无法读取旧登录会话|无法核对已有授权状态/,
      );
      expect(await page.evaluate(() => (window as any).__tsTest.starts)).toBe(0);
      expect(await page.getByRole('dialog').count()).toBe(1);
    } finally { await page.close(); }
  }, 30_000);

  it('a confirmed old session shows the account replacement warning', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login?mode=state-true`);
      await page.getByRole('button', { name: 'Auth Key' }).click();
      await page.getByText('提交将退出当前登录并使用新 Auth Key。').waitFor();
      expect(await page.getByText('旧登录状态暂无法确认；提交 Auth Key 前会重新读取，读取失败则不会更换账号。').count()).toBe(0);
    } finally { await page.close(); }
  }, 30_000);

  it('a missed native failure is shown on return without closing the saved login form', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.receipt = { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'failed', reason: 'invalidAuthUrl' };
        window.dispatchEvent(new Event('focus'));
      });
      await page.waitForFunction(() => (window as any).__tsTest.attempt().phase === 'failed');
      expect(await page.getByRole('dialog').count()).toBe(1);
      await page.getByText('节点已保存；本次授权尚未完成。 控制面返回了无效登录地址，请检查控制面配置。').waitFor();
      expect(await page.getByText('invalidAuthUrl').count()).toBe(0);
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
    } finally { await page.close(); }
  }, 30_000);

  it('an old authorized receipt cannot complete a newer request', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.receipt = { serverId: 'ts-1', attemptId: test.attempt().attemptId, phase: 'authorized' };
        test.begin('attempt-b');
        window.dispatchEvent(new Event('focus'));
      });
      await page.waitForTimeout(100);
      expect(await page.getByRole('dialog').count()).toBe(1);
      expect(await page.evaluate(() => (window as any).__tsTest.attempt())).toMatchObject({ attemptId: 'attempt-b', phase: 'starting' });
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
    } finally { await page.close(); }
  }, 30_000);

  it('a receipt arriving after the panel was cancelled cannot restore authorization', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login`);
      await page.locator('.m-form-foot .primary').waitFor();
      await page.evaluate(() => { (window as any).__tsTest.holdProgress = true; });
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => !!(window as any).__tsTest.releaseProgress);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.receipt = { serverId: 'ts-1', attemptId: test.attempt().attemptId, phase: 'authorized' };
      });
      await page.locator('.m-form-head .m-form-x').click();
      await page.getByRole('dialog').waitFor({ state: 'detached' });
      await page.evaluate(() => (window as any).__tsTest.releaseProgress());
      await page.waitForTimeout(50);
      expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('cancelled');
      expect(await page.evaluate(() => (window as any).__tsTest.cancels.length)).toBeGreaterThan(0);
      expect(await page.locator('.m-toast').filter({ hasText: '授权已完成' }).count()).toBe(0);
    } finally { await page.close(); }
  }, 30_000);

  it('edits made while save is in flight remain visible after authorization', async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
    try {
      await page.goto(`${origin}/__ts-login?mode=delayed-save`);
      await page.locator('#mts-control-url').fill('https://first.example');
      await page.locator('.m-form-foot .primary').click();
      await page.waitForFunction(() => !!(window as any).__tsTest.releaseSave);
      await page.locator('#mts-control-url').fill('https://second.example');
      await page.evaluate(() => (window as any).__tsTest.releaseSave());
      await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
      await page.evaluate(() => {
        const test = (window as any).__tsTest;
        test.emit('onTailscaleLoginProgress', { serverId: 'ts-1', attemptId: test.attempt().attemptId,
          phase: 'authorized', url: null });
      });
      await page.getByText('授权已完成').waitFor();
      expect(await page.getByRole('dialog').count()).toBe(1);
      expect(await page.locator('#mts-control-url').inputValue()).toBe('https://second.example');
      expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
    } finally { await page.close(); }
  }, 30_000);

  it('a fresh main-core pull completes the owned attempt; pending config cannot inherit that success', async () => {
    for (const mode of ['main', 'main-pending']) {
      const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__ts-login?mode=${mode}`);
        await page.locator('.m-form-foot .primary').click();
        await page.waitForFunction(() => (window as any).__tsTest.starts === 1);
        if (mode === 'main') {
          await page.getByRole('dialog').waitFor({ state: 'detached' });
          expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('authorized');
          expect(await page.evaluate(() => (window as any).__tsTest.cancels)).toEqual([]);
        } else {
          await page.waitForTimeout(100);
          expect(await page.getByRole('dialog').count()).toBe(1);
          expect(await page.evaluate(() => (window as any).__tsTest.attempt().phase)).toBe('mainCore');
        }
      } finally { await page.close(); }
    }
  }, 30_000);
});

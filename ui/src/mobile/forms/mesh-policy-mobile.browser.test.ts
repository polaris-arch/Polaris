import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// A real mobile form saves through each consumer; the screenshot is the actual TS settings
// form with an explicit allowlist. No native service, device, or account is contacted.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { api } from '/src/ipc';
import { systemApi } from '/src/ipc/api-client';
import { useAppStore } from '/src/store/app-store';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { NodeFormPanel } from '/src/mobile/forms/NodeFormPanel';
import { WgPanel } from '/src/mobile/forms/WgPanel';
import { TsSettingsPanel } from '/src/mobile/forms/TsSettingsPanel';
import { TsExitPanel } from '/src/mobile/forms/TsExitPanel';
import { i18nReady } from '/src/i18n';
import '/src/styles/tokens.resolved.css';
import '/src/mobile/theme.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
import '/src/mobile/screens/rules/rules-redesign.css';
import '/src/mobile/connections/connections-redesign.css';
await i18nReady;
const mode = new URLSearchParams(location.search).get('mode') || 'ts';
const policy = { mode: 'allowlist', rules: [{ sourceCidrs: ['10.0.0.2/32'], network: 'tcp', ports: ['443'], target: 'local' }] };
const common = { name: mode + ' node', address: '203.0.113.7', port: 443, meshInboundPolicy: policy };
const base = mode === 'node'
  ? { ...common, id: 'oc-1', protocol: 'masque-client', masqueClientSettings: {} }
  : mode === 'wg'
  ? { ...common, id: 'wg-1', protocol: 'wireguard', port: 51820, wireguardSettings: {
      privateKey: 'synthetic-private', localAddress: ['10.0.0.2/32'], peerPublicKey: 'synthetic-public' } }
  : { ...common, id: 'ts-1', protocol: 'tailscale', address: '', port: 0,
      tailscaleSettings: { hostname: 'synthetic-phone', sourceTag: 'imported' } };
const test = window.__meshPolicyTest = { saved: [], mode };
api.server.update = async server => { test.saved.push(server); };
const peer = (name, ip) => ({ hostName: name, ip, online: true, exitNode: false,
  exitNodeOption: true, active: true });
api.server.tailscaleGetStatus = async () => ({ connected: true, statuses: [
  { serverId: 'ts-1', peers: [peer('Own-Peer', '100.1.1.1')] },
  { serverId: 'ts-2', peers: [peer('Other-Peer', '100.2.2.2')] },
] });
systemApi.listNetworkInterfaces = async () => [];
useAppStore.setState({ servers: [base], config: { servers: [base], subscriptions: [] },
  loadConfig: async () => {} });
const kind = mode === 'node' ? 'node' : mode === 'wg' ? 'wg' : mode === 'ts-exit' ? 'ts-exit' : 'ts-settings';
const instanceId = useMobileFormStore.getState().open({ kind, serverId: base.id });
const Panel = mode === 'node' ? NodeFormPanel : mode === 'wg' ? WgPanel : mode === 'ts-exit' ? TsExitPanel : TsSettingsPanel;
createRoot(document.getElementById('root')).render(<main className="mobile-root"><Panel instanceId={instanceId} serverId={base.id} /></main>);
`;
let server: ViteDevServer;
let browser: Browser;
let origin: string;
const output = path.resolve(root, '../output/playwright');
describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('mobile mesh inbound policy consumers', () => {
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0, strictPort: false }, plugins: [{
      name: 'mesh-policy-mobile-browser-fixture',
      resolveId(id) { if (id === '/mesh-policy-mobile-fixture.tsx') return id; },
      load(id) { if (id === '/mesh-policy-mobile-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__mesh-policy', async (_req, res) => {
        const html = await vite.transformIndexHtml('/__mesh-policy', '<html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="root"></div><script type="module" src="/mesh-policy-mobile-fixture.tsx"></script></html>');
        res.setHeader('Content-Type', 'text/html'); res.end(html);
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (typeof address !== 'object' || !address) throw new Error('Vite did not bind');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
    mkdirSync(output, { recursive: true });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const mode of ['node', 'wg', 'ts'] as const) {
    it(`${mode} retains an explicit allowlist on save`, async () => {
      const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__mesh-policy?mode=${mode}`);
        await page.getByText('授权规则 1').waitFor();
        await page.locator('.m-form-foot .primary').click();
        await page.waitForFunction(() => (window as any).__meshPolicyTest.saved.length === 1);
        const saved = await page.evaluate(() => (window as any).__meshPolicyTest.saved[0]);
        expect(saved.meshInboundPolicy).toEqual({ mode: 'allowlist', rules: [
          { sourceCidrs: ['10.0.0.2/32'], network: 'tcp', ports: ['443'], target: 'local' },
        ] });
        if (mode === 'ts') expect(saved.tailscaleSettings.sourceTag).toBe('imported');
      } finally { await page.close(); }
    }, 30_000);
  }

  for (const [width, height, scale, tag] of [[320, 640, 2, '320-font2'], [390, 844, 1, '390'], [1040, 760, 1, '1040']] as const) {
    it(`TS allowlist layout ${tag}: fields and boundary remain readable without horizontal overflow`, async () => {
      const page = await browser.newPage({ viewport: { width, height }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__mesh-policy?mode=ts`);
        await page.evaluate(scale => document.documentElement.style.setProperty('--font-scale', String(scale)), scale);
        const section = page.locator('.m-form-body').getByText('授权规则 1');
        await section.waitFor();
        await (width === 1040 ? page.locator('#mts-ports-0') : section).scrollIntoViewIfNeeded();
        expect(await page.getByText(/Tailscale 内置服务仍由 tailnet 权限控制/).count()).toBe(1);
        const geometry = await page.evaluate(() => ({
          bodyScrollWidth: document.body.scrollWidth, viewportWidth: window.innerWidth,
          panelScrollWidth: document.querySelector('.m-form-panel')?.scrollWidth,
          panelWidth: document.querySelector('.m-form-panel')?.clientWidth,
          headingVisible: (document.querySelector('.m-form-head')?.getBoundingClientRect().top ?? -1) >= 0,
          rootFontSize: getComputedStyle(document.documentElement).fontSize,
          gridColumns: getComputedStyle(document.querySelector('.m-mesh-policy-fields')!).gridTemplateColumns.split(' ').length,
          groupTitleAlign: getComputedStyle(document.querySelector('.m-form-group-h > span')!).textAlign,
        }));
        expect(geometry.bodyScrollWidth).toBeLessThanOrEqual(geometry.viewportWidth);
        expect(geometry.panelScrollWidth ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(geometry.panelWidth ?? 0);
        expect(geometry.headingVisible).toBe(true);
        expect(geometry.rootFontSize).toBe(`${16 * scale}px`);
        expect(geometry.gridColumns).toBe(width === 1040 ? 2 : 1);
        expect(geometry.groupTitleAlign).toBe('start');
        await page.screenshot({ path: path.join(output, `integration-mesh-policy-${tag}.png`) });
      } finally { await page.close(); }
    }, 30_000);
  }

  for (const mode of ['ts', 'ts-exit'] as const) {
    it(`${mode} only offers peers from the selected TS endpoint`, async () => {
      const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: 'zh-CN' });
      try {
        await page.goto(`${origin}/__mesh-policy?mode=${mode}`);
        await page.waitForFunction(() => document.querySelector('select option[value="Own-Peer"]'));
        const choices = await page.locator('select option').allTextContents();
        expect(choices.some((text) => text.includes('Own-Peer'))).toBe(true);
        expect(choices.some((text) => text.includes('Other-Peer'))).toBe(false);
      } finally { await page.close(); }
    }, 30_000);
  }
});

import { chromium, type Browser } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { beforeAll, afterAll, describe, expect, it } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import enUS from '@/i18n/locales/en-US.json';

// The real UI consumers run against deferred, synthetic API receipts. No native mutation/network.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { DialogHost } from '/src/components/dialogs/DialogHost';
import { useDialogStore } from '/src/components/dialogs/dialog-store';
import { useVpnStatusStore } from '/src/store/use-vpn-status-store';
import { useSubscriptionCreateOperationStore } from '/src/store/subscription-create-operation-store';
import { useAppStore } from '/src/store/app-store';
import { useStagedConfigStore } from '/src/store/staged-config-store';
import PendingChangesBar from '/src/components/layout/PendingChangesBar';
import { MobilePendingBar } from '/src/mobile/MobilePendingBar';
import { useNodeActions } from '/src/components/screens/nodes/use-node-actions';
import { NodesBatchBar } from '/src/components/screens/nodes/NodesBatchBar';
import { RulesScreen } from '/src/components/screens/rules/RulesScreen';
import { MobileRulesScreen } from '/src/mobile/screens/rules/RulesScreen';
import { useAppPresetsStore } from '/src/store/use-app-presets-store';
import { MobileNodesScreen } from '/src/mobile/nodes/MobileNodesScreen';
import { MobileConnectionsScreen } from '/src/mobile/connections/MobileConnectionsScreen';
import { ResCatalogPanel } from '/src/mobile/forms/ResourceAddPanel';
import { WarpPanel } from '/src/mobile/forms/WarpPanel';
import { TsSettingsPanel } from '/src/mobile/forms/TsSettingsPanel';
import { systemApi } from '/src/ipc/api-client';
import { useMobileFormStore } from '/src/mobile/forms/form-store';
import { api } from '/src/ipc';
import i18n, { i18nReady } from '/src/i18n';
import '/src/styles/index.css';
import '/src/mobile/mobile.css';
import '/src/mobile/forms/forms.css';
import '/src/mobile/redesign.css';
import '/src/mobile/screens/rules/rules-screen.css';
import '/src/mobile/screens/rules/rules-redesign.css';
await i18nReady;
await i18n.changeLanguage('en-US');
const mode = new URLSearchParams(location.search).get('mode');
const initialGeneration = Number(new URLSearchParams(location.search).get('generation') ?? '10');
const test = window.__transactions = { writes: 0, release: null, events: null };
const config = { servers: [], subscriptions: [], trafficRules: [], dnsRules: [], proxyMode: 'smart', routeRuleOrder: [], dnsRuleOrder: [] };
useAppStore.setState({ config, servers: [], rules: [], dnsRules: [], privacyMode: false, proxyStatus: { running: true, mainGeneration: initialGeneration }, loadConfig: async () => {} });
useStagedConfigStore.setState({ enabled: true, hydrated: true, baseline: config, entries: [], conflict: null, saveStatus: 'idle' });
api.config.setStagedPending = async () => {};
api.config.get = async () => config;
api.config.onChanged = () => () => {};
api.ruleResources.onProgress = () => () => {};
useAppPresetsStore.setState({ loaded: true, loading: false, presets: [] });
api.ruleResources.list = async () => [];
api.config.meshRouteReport = async () => null;
api.networkProfile.onMatchChanged = () => () => {};
api.networkProfile.resolvedSources = async () => [];
api.rules.update = async () => { test.writes++; };
api.vpn.submitOpenVpn = async () => new Promise(resolve => { test.release = resolve; });
api.vpn.cancelOpenVpn = async () => {};
test.applyCalls = 0;
test.stagePersistenceDraft = () => {
  api.config.save = async candidate => { test.writes++; return { status: 'saved', config: candidate, version: 'synthetic' }; };
  useStagedConfigStore.getState().stage({ id: 'setting:mixedPort', kind: 'setting', label: 'Unrelated draft', entityPath: ['mixedPort'], nextValue: 12345 });
};
test.draftCount = () => useStagedConfigStore.getState().entries.length;
api.proxy.onStarted = listener => { test.unownedStarted = listener; return () => { test.unownedStarted = null; }; };
api.proxy.onError = listener => { test.unownedError = listener; return () => { test.unownedError = null; }; };
api.proxy.onLifecycleReady = async listener => {
  const registration = new URLSearchParams(location.search).get('listener');
  if (registration === 'failed') throw new Error('Synthetic listener registration failure');
  if (registration === 'deferred') await new Promise(resolve => { test.releaseListener = resolve; });
  test.events = listener;
  return () => { test.events = null; };
};
api.proxy.applyPendingChanges = async token => { test.applyTokens = [...(test.applyTokens ?? []), token]; test.applyCalls++; return new Promise(resolve => { test.receipt = resolve; }); };
api.proxy.retryRuntimePersistence = async transactionId => { test.persistenceTransactions = [...(test.persistenceTransactions ?? []), transactionId]; return new Promise((resolve, reject) => { test.retryReceipt = resolve; test.retryReject = reject; }); };
api.proxy.runtimeApplyStatus = async transactionId => { test.runtimeStatusReads = (test.runtimeStatusReads ?? 0) + 1; test.runtimeTransactions = [...(test.runtimeTransactions ?? []), transactionId]; return new Promise(resolve => { test.statusReceipt = resolve; }); };
api.proxy.getStatus = async () => { test.statusReads = (test.statusReads ?? 0) + 1; return { running: true, mainGeneration: 99, ownedStopContext: { selectedServerId: useAppStore.getState().selectedServerId, selectionIntentGeneration: 0 } }; };
test.stack = () => useDialogStore.getState().stack.map(({ instanceId, kind, serverId }) => ({ instanceId, kind, serverId }));
test.openVpn = (serverId, challengeId = serverId + '-challenge') => {
  useVpnStatusStore.getState().setOpenVpn({ serverId, challenge: { id: challengeId, kind: 'credentials', username: 'synthetic' } });
  return useDialogStore.getState().open({ kind: 'vpn-auth', protocol: 'openvpn', serverId });
};
test.replaceChallenge = (serverId, id) => useVpnStatusStore.getState().setOpenVpn({ serverId, challenge: { id, kind: 'credentials' } });
test.clearChallenge = (serverId) => useVpnStatusStore.getState().setOpenVpn({ serverId });
let recovery;
if (mode === 'vpn') test.a = test.openVpn('a');
if (mode === 'recovery') {
  recovery = { operationId: 'op', revision: 1, phase: 'fetching', terminal: false };
  useSubscriptionCreateOperationStore.getState().accept(recovery);
  useSubscriptionCreateOperationStore.setState({ cancel: async () => recovery });
  useDialogStore.getState().open({ kind: 'sub-create-task', operationId: 'op' });
  test.fail = () => { recovery = { ...recovery, revision: 2, phase: 'failed', terminal: true }; useSubscriptionCreateOperationStore.getState().accept(recovery); };
}
if (mode === 'apply' || mode === 'mobile-apply') {
  useAppStore.setState({ pendingChanges: { added: [], removed: [], modified: [], restartDeferred: true } });
  const schedule = window.setTimeout.bind(window);
  window.setTimeout = (callback, delay, ...args) => delay === 12000 ? (test.fallback = callback, 0) : schedule(callback, delay, ...args);
}
const ruleMode = ['route', 'dns', 'mobile-route', 'mobile-dns'].includes(mode);
const plane = mode?.endsWith('dns') ? 'dns' : 'route';
if (ruleMode) {
  const rule = { id: 'r', type: 'domain', values: ['example.com'], enabled: true, remarks: 'Rule under test', action: 'direct', effects: { [plane]: plane === 'dns' ? { enabled: true, resolver: 'inherit', answerMode: 'real', action: { type: 'reject' } } : { enabled: true, action: 'direct' } } };
  config[plane === 'dns' ? 'dnsRules' : 'trafficRules'] = [rule];
  useAppStore.setState({ config: { ...config }, rules: plane === 'route' ? [rule] : [], dnsRules: plane === 'dns' ? [rule] : [] });
  useStagedConfigStore.setState({ baseline: { ...config } });
  test.disk = () => (plane === 'dns' ? useAppStore.getState().dnsRules : useAppStore.getState().rules)[0].effects[plane].enabled;
  test.revert = () => useStagedConfigStore.getState().revert('rule:r');
  test.reset = () => useStagedConfigStore.getState().reset();
  test.mirror = () => (plane === 'dns' ? useAppStore.getState().dnsRules : useAppStore.getState().rules);
  test.newerMirror = () => {
    const newer = { ...rule, remarks: 'Newer authoritative value', effects: { ...rule.effects, [plane]: { ...rule.effects[plane], enabled: false } } };
    const other = { ...rule, id: 'other', remarks: 'Concurrent added rule' };
    useAppStore.setState(plane === 'dns' ? { dnsRules: [newer, other] } : { rules: [newer, other] });
  };
  if (new URLSearchParams(location.search).has('direct')) {
    useStagedConfigStore.setState({ enabled: false });
    api.rules.update = async () => { test.writes++; return new Promise((_resolve, reject) => { test.rejectToggle = reject; }); };
  }
}
if (mode === 'res-url' || mode === 'res-catalog' || mode === 'network-profile') {
  useStagedConfigStore.setState({ enabled: false });
  api.ruleResources.getCatalog = async () => ({ items: [], source: 'builtin', fetchedAt: null });
  api.ruleResources.getCachedCatalog = async () => ({ items: [{ id: 'downloadable', name: 'Downloadable fixture', path: 'fixture.srs', category: 'custom', bundled: false }], source: 'cache', fetchedAt: null });
  api.ruleResources.download = async () => new Promise(resolve => { test.release = () => resolve([{ ok: true, id: 'downloaded' }]); });
  api.config.patch = async patch => new Promise(resolve => { test.release = () => resolve({ ...config, ...patch }); });
  if (mode === 'network-profile') {
    config.networkProfiles = [{ id: 'np', name: 'Original profile', enabled: true, probe: 'auto', match: { searchDomains: ['example.invalid'], dnsServerCidrs: [] } }];
    useAppStore.setState({ config: { ...config } });
  }
  test.a = useDialogStore.getState().open(mode === 'network-profile' ? { kind: mode, profileId: 'np' } : { kind: mode });
  test.captureConfirm = () => { test.confirm = useDialogStore.getState().stack.at(-1).payload.onConfirm; };
}
let resourceInstance, meshInstance;
if (mode === 'desktop-warp' || mode === 'mobile-warp' || mode === 'mobile-ts') {
  const onDemand = new URLSearchParams(location.search).get('enabled') === 'true';
  const node = mode === 'mobile-ts' ? { id: 'ts', name: 'Synthetic TS', protocol: 'tailscale', address: '', port: 0, onDemand, tailscaleSettings: { hostname: 'synthetic-ts' } }
    : { id: 'warp', name: 'Synthetic WARP', protocol: 'wireguard', address: 'engage.cloudflareclient.com', port: 2408, onDemand, wireguardSettings: { privateKey: 'synthetic-private', peerPublicKey: 'synthetic-public', localAddress: ['172.16.0.2/32'], warpDevice: { deviceId: 'synthetic-device', token: 'synthetic-token' } } };
  config.servers = [node];
  useAppStore.setState({ config: { ...config }, servers: [node] });
  useStagedConfigStore.setState({ baseline: { ...config } });
  systemApi.listNetworkInterfaces = async () => [];
  api.server.tailscaleGetStatus = async () => ({ connected: false, statuses: [] });
  api.server.tailscaleStateExists = async () => ({ ts: false });
  api.server.update = async server => { test.saved = server; };
  test.payload = () => test.saved ?? useStagedConfigStore.getState().entries[0]?.nextValue;
  if (mode === 'desktop-warp') useDialogStore.getState().open({ kind: 'warp', edit: true });
  else meshInstance = useMobileFormStore.getState().open(mode === 'mobile-ts' ? { kind: 'ts-settings', serverId: 'ts' } : { kind: 'warp', edit: true });
  if (mode === 'mobile-ts' && new URLSearchParams(location.search).has('ownedStop')) {
    const query = new URLSearchParams(location.search);
    const selectedServerId = query.has('nullSelection') ? null : 'other-node';
    const selectionIntentGeneration = Number(query.get('intent') ?? '0');
    const ownedStopContext = { selectedServerId, selectionIntentGeneration };
    const running = { running: true, pid: 123, startTime: 456, mainGeneration: 7, ownedStopContext };
    let currentStatus = running;
    test.logoutCalls = []; test.stopCalls = []; test.unconditionalStops = 0;
    useAppStore.setState({ selectedServerId, proxyStatus: running });
    api.proxy.getStatus = async () => { test.statusReads = (test.statusReads ?? 0) + 1; return currentStatus; };
    api.proxy.stop = async () => { test.unconditionalStops++; throw new Error('Unexpected unconditional Stop'); };
    api.proxy.stopExpected = async expected => {
      test.stopCalls.push(expected);
      if (query.get('ownedStop') === 'changed') return { kind: 'changed' };
      currentStatus = { running: false, mainGeneration: 8, ownedStopContext };
      return { kind: 'stopped', expectedMainGeneration: 7, stopMainGeneration: 8 };
    };
    // The first logout is refused by the native owner gate. Only a successful exact
    // Stop and its matching post-stop status permit the helper's second logout.
    api.server.tailscaleLogout = async id => {
      test.logoutCalls.push(id);
      if (test.logoutCalls.length === 1) throw { code: 'TAILSCALE_LOGOUT_MAIN_CORE' };
    };
    api.server.tailscaleStateExists = async () => ({ ts: true });
    const preflightHold = query.get('preflightHold') ?? 'notHeld';
    // STATUS membership is not custody proof. Bind this preflight to the Ready run;
    // native logout remains the final gate even when that snapshot excludes the target.
    api.server.tailscaleGetStatus = async () => ({
      connected: true, statuses: [{ serverId: 'other-node', peers: [] }],
      mainCoreHold: preflightHold === 'unknown' ? undefined : {
        mainGeneration: running.mainGeneration,
        serverIds: preflightHold === 'held' ? ['ts'] : ['other-node'],
      },
    });
    test.mobileStack = () => useMobileFormStore.getState().stack.map(({ instanceId, kind }) => ({ instanceId, kind }));
    test.mobileSelection = () => useAppStore.getState().selectedServerId;
    test.mobileConfirmation = () => {
      const confirmation = useMobileFormStore.getState().stack.at(-1);
      if (confirmation?.kind !== 'confirm') throw new Error('Expected real mobile confirmation');
      return { instanceId: confirmation.instanceId, title: confirmation.payload.title, message: confirmation.payload.message };
    };
    // Invoke the real consumer-created confirmation payload, not a helper substitute.
    test.confirmMobile = async () => {
      const confirmation = useMobileFormStore.getState().stack.at(-1);
      if (confirmation?.kind !== 'confirm') throw new Error('Expected real mobile confirmation');
      await confirmation.payload.onConfirm();
    };
  }
}

if (mode === 'mobile-nodes') {
  const subs = ['one', 'two'].map(id => ({ id, name: 'Subscription ' + id, url: 'https://example.invalid/' + id, autoUpdate: false, createdAt: '2026-01-01' }));
  const nodes = subs.map(sub => ({ id: 'node-' + sub.id, subscriptionId: sub.id, name: sub.name + ' node', protocol: 'shadowsocks', address: 'example.invalid', port: 443 }));
  config.servers = nodes; config.subscriptions = subs;
  useAppStore.setState({ config: { ...config }, servers: nodes });
  useStagedConfigStore.setState({ baseline: { ...config } });
  test.removeSub = id => useStagedConfigStore.getState().stage({ id: 'sub:' + id, kind: 'subscription', label: id, entityPath: ['subscriptions', id], nextValue: null });
  test.undoSub = id => useStagedConfigStore.getState().revert('sub:' + id);
}
if (mode === 'connections') {
  test.closeCalls = [];
  test.pending = {};
  api.stats.subscribe = async () => {};
  api.stats.unsubscribe = async () => {};
  api.stats.onConnectionsAggregate = () => () => {};
  api.stats.onConnectionsDetail = listener => { test.frame = listener; return () => {}; };
  api.connections.close = id => { test.closeCalls.push(id); return new Promise((resolve, reject) => { test.pending[id] = { resolve, reject }; }); };
  api.connections.closeAll = () => new Promise((resolve, reject) => { test.pending.all = { resolve, reject }; });
  test.sequence = 0;
  test.emit = (ids, reset = false) => test.frame({ reset, generation: 1, sequence: ++test.sequence, at: test.sequence * 1000, connections: ids.map(id => ({ id, chains: ['node'], rule: 'MATCH', metadata: { host: id + '.example', network: 'tcp', destinationIP: '198.51.100.7' }, upload: 100, download: 200, start: '2026-01-01T00:00:00Z' })) });
}
if (mode === 'resources') {
  api.ruleResources.getCatalog = async () => ({ items: [{ id: 'builtin', name: 'builtin', path: 'builtin.srs', category: 'custom', bundled: true }] });
  api.ruleResources.getCachedCatalog = async () => ({ items: ['missing', 'present'].map(id => ({ id, name: id, path: id + '.srs', category: 'custom', bundled: false })) });
  api.ruleResources.list = async () => ['missing', 'present'].map(id => ({ id, tag: id, fileExists: id === 'present' }));
  resourceInstance = useMobileFormStore.getState().open({ kind: 'res-catalog' });
}
function Selection() {
  const [selectedIds, setSelectedIds] = useState(new Set(['a', 'b']));
  const [visible, setVisible] = useState([{ id: 'c' }, { id: 'd' }]);
  const actions = useNodeActions({ servers: visible, visibleServers: visible, selectedIds, setSelectedIds, t: (key) => key, stagingEnabled: true, stage: () => {}, openDialog: () => {} });
  test.selected = () => [...selectedIds].sort();
  test.visible = (ids) => setVisible(ids.map(id => ({ id })));
  return <><button id="select-all" onClick={actions.selectAll}>Select all</button><NodesBatchBar t={key => key} selectedIds={selectedIds} visibleServers={visible} selectAll={actions.selectAll} testSelected={() => {}} testing={false} isSubTab copyLinksBatch={() => {}} confirmArmed={null} nodeDeletion={{ deleteBatch: () => {} }} exitBatch={() => {}} /></>;
}
createRoot(document.getElementById('root')).render(<><DialogHost />
  {mode === 'apply' && <PendingChangesBar />}
  {mode === 'mobile-apply' && <main className="mobile-root"><MobilePendingBar /></main>}
  {mode === 'selection' && <Selection />}
  {mode === 'mobile-nodes' && <main className="mobile-root"><MobileNodesScreen /></main>}
  {mode === 'connections' && <main className="mobile-root"><MobileConnectionsScreen /></main>}
  {mode === 'resources' && <main className="mobile-root"><ResCatalogPanel instanceId={resourceInstance} /></main>}
  {mode === 'mobile-warp' && <main className="mobile-root"><WarpPanel instanceId={meshInstance} edit /></main>}
  {mode === 'mobile-ts' && <main className="mobile-root"><TsSettingsPanel instanceId={meshInstance} serverId="ts" /></main>}
  {(mode === 'route' || mode === 'dns') && <RulesScreen plane={plane} />}
  {(mode === 'mobile-route' || mode === 'mobile-dns') && <main className="mobile-root"><MobileRulesScreen /></main>}
</>);
`;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('App transaction interrupted and repeated flows', () => {
  let server: ViteDevServer;
  let browser: Browser;
  let origin: string;
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'app-transactions-fixture',
      resolveId(id) { if (id === '/app-transactions-fixture.tsx') return id; },
      load(id) { if (id === '/app-transactions-fixture.tsx') return entry; },
      configureServer(vite) { vite.middlewares.use('/__transactions', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__transactions', '<html><div id="root"></div><script type="module" src="/app-transactions-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address === 'string') throw new Error('No fixture listener');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ chromiumSandbox: true, headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it('late VPN success closes its own instance, never a new challenge on top', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=vpn');
    await page.locator('dialog[open] .dlg-foot .flow').click();
    await page.evaluate('window.__transactions.b = window.__transactions.openVpn("b")');
    await page.evaluate('window.__transactions.release()');
    await page.waitForFunction('window.__transactions.stack().length === 1');
    expect(await page.evaluate('window.__transactions.stack()[0].serverId')).toBe('b');
    await page.evaluate('window.__transactions.clearChallenge("a")');
    expect(await page.evaluate('window.__transactions.stack()[0].serverId')).toBe('b');
    await page.close();
  });
  it('late success cannot dismiss a newer challenge in the same mounted instance', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=vpn');
    await page.locator('dialog[open] .dlg-foot .flow').click();
    await page.evaluate('window.__transactions.replaceChallenge("a", "newer"); window.__transactions.release()');
    await page.waitForFunction('document.querySelector("dialog[open] .flow")?.disabled === false');
    expect(await page.evaluate('window.__transactions.stack().length')).toBe(1);
    await page.close();
  });
  it('failed terminal received before Cancel confirmation remains dismissible', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=recovery');
    await page.locator('dialog[open] .dlg-x').click();
    await page.evaluate('window.__transactions.fail()');
    await page.locator('dialog[open]').last().locator('.dlg-foot .btn').last().click();
    await page.waitForFunction('document.querySelector("dialog[open] .dlg-x")?.disabled === false');
    await page.locator('dialog[open] .dlg-x').click();
    expect(await page.evaluate('window.__transactions.stack().length')).toBe(0);
    await page.close();
  });
  describe.each([
    { mode: 'apply', button: '.pending-bar .btn.flow', errorBar: '.pending-bar.err' },
    { mode: 'mobile-apply', button: '.m-pending .m-pending-primary', errorBar: '.m-pending.m-pending-err' },
  ])('attributed Apply consumer: $mode', ({ mode, button, errorBar }) => {
    it('reconciles persistence through the original status token without replay', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction('typeof window.__transactions.statusReceipt === "function"');
      expect(await page.evaluate('window.__transactions.runtimeTransactions')).toEqual(['original']);
      await page.evaluate('window.__transactions.statusReceipt({ ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction(() => !document.body.textContent?.includes('Applied at runtime; persistence is still pending.'));
      expect(await page.getByText('Applied at runtime; persistence is still pending.', {exact:true}).count()).toBe(0);
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.close();
    });
    it('retries only persistence for the original receipt, without a second Apply', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction('typeof window.__transactions.statusReceipt === "function"');
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).click();
      await page.waitForFunction('typeof window.__transactions.retryReceipt === "function"');
      expect(await page.evaluate('window.__transactions.persistenceTransactions')).toEqual(['original']);
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      // The old poll may finish first; its consumer is replaced, so the exact
      // terminal Retry receipt must still settle this new attempt.
      await page.evaluate('window.__transactions.statusReceipt({ ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.evaluate('window.__transactions.retryReceipt({ ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction(() => Array.from(document.querySelectorAll('button'))
        .some(button => button.textContent?.trim() === 'Apply now' && !button.disabled));
      expect(await page.locator(errorBar).count()).toBe(0);
      expect(await page.getByRole('button', { name: 'Retry persistence', exact: true }).count()).toBe(0);
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.close();
    });
    it.each(['unconfirmed', 'ipc', 'wrong-receipt'] as const)('keeps failed persistence retry isolated from drafts: %s', async (failure) => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).waitFor();
      await page.evaluate('window.__transactions.stagePersistenceDraft()');
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).click();
      await page.waitForFunction('typeof window.__transactions.retryReceipt === "function"');
      await page.evaluate(value => {
        const test = (window as any).__transactions;
        if (value === 'ipc') test.retryReject(new Error('synthetic IPC error'));
        else test.retryReceipt({ ok:false, status:'unconfirmed', transactionId: value === 'wrong-receipt' ? 'other' : 'original', runtimeGeneration:10, selectionIntentGeneration:0 });
      }, failure);
      await page.locator(errorBar).waitFor();
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).click();
      await page.waitForFunction('window.__transactions.persistenceTransactions?.length === 2');
      expect(await page.evaluate('window.__transactions.persistenceTransactions')).toEqual(['original', 'original']);
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      expect(await page.evaluate('window.__transactions.writes')).toBe(0);
      expect(await page.evaluate('window.__transactions.draftCount()')).toBe(1);
      await page.evaluate('window.__transactions.retryReceipt({ ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction(() => Array.from(document.querySelectorAll('button'))
        .some(button => button.textContent?.trim() === 'Apply now' && !button.disabled));
      expect(await page.locator(errorBar).count()).toBe(0);
      expect(await page.getByRole('button', { name: 'Retry persistence', exact: true }).count()).toBe(0);
      expect(await page.evaluate('window.__transactions.writes')).toBe(0);
      expect(await page.evaluate('window.__transactions.draftCount()')).toBe(1);
      await page.close();
    });
    it.each(['reply', 'poll'] as const)('exits confirmed obsolete recovery from %s before separately applying drafts', async (source) => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).waitFor();
      await page.waitForFunction('window.__transactions.runtimeStatusReads >= 1');
      await page.evaluate('window.__transactions.stagePersistenceDraft()');
      await page.getByRole('button', { name: 'Retry persistence', exact: true }).click();
      await page.waitForFunction('typeof window.__transactions.retryReceipt === "function"');
      if (source === 'poll') {
        await page.evaluate('window.__transactions.retryReceipt({ ok:false,status:"unconfirmed",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
        await page.waitForFunction('window.__transactions.runtimeStatusReads >= 2');
        await page.evaluate('window.__transactions.statusReceipt({ ok:false,status:"conflict",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0,reasons:[{code:"OWNER_STATE_SUPERSEDED",path:"desired"}] });');
      } else {
        await page.evaluate('window.__transactions.retryReceipt({ ok:false,status:"conflict",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0,reasons:[{code:"OWNER_STATE_SUPERSEDED",path:"desired"}] });');
      }
      await page.getByRole('button', { name: 'Exit recovery', exact: true }).click();
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      expect(await page.evaluate('window.__transactions.writes')).toBe(0);
      expect(await page.evaluate('window.__transactions.draftCount()')).toBe(1);
      await page.getByRole('button', { name: 'Apply now', exact: true }).click();
      await page.waitForFunction('window.__transactions.applyCalls === 2');
      expect(await page.evaluate('window.__transactions.writes')).toBe(1);
      expect(await page.evaluate('window.__transactions.persistenceTransactions')).toEqual(['original']);
      await page.close();
    });
    it('ignores a late successful read after an attributed stop'   , async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
      await page.waitForFunction('typeof window.__transactions.statusReceipt === "function"');
      await page.evaluate('window.__transactions.events({phase:"stopped",kind:"stop",terminal:true,mainGeneration:11}); window.__transactions.statusReceipt({ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0});');
      await page.getByText('Applied at runtime; persistence is still pending.', {exact:true}).waitFor();
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.close();
    });
    it('asks before restart, decline preserves pending, and stale confirmation stays failed', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ ok: false, status: "restartRequired", restartConfirmation: "bound-1", reasons: [{code:"unsupportedCore",path:"secret-never-render"}], pendingApply:true, restartRequired:true });');
      await page.getByRole('dialog').waitFor();
      expect(await page.getByRole('dialog').textContent()).not.toContain('secret-never-render');
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.getByRole('button', { name: 'Keep current connection', exact: true }).click();
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.locator(button).click();
      await page.waitForFunction('window.__transactions.applyCalls === 2');
      await page.evaluate('window.__transactions.receipt({ ok: false, status: "restartRequired", restartConfirmation: "bound-2", reasons: [{code:"structuralChange",path:"tun"}] });');
      await page.getByRole('button', { name: 'Restart and apply', exact: true }).click();
      await page.waitForFunction('window.__transactions.applyCalls === 3');
      expect(await page.evaluate('window.__transactions.applyTokens[2]')).toBe('bound-2');
      await page.evaluate('window.__transactions.receipt({ ok:false, status:"conflict", pendingApply:true });');
      await page.locator(errorBar).waitFor();
      expect(await page.locator(button).isDisabled()).toBe(false);
      await page.close();
    });
    it.each(['hotApplied', 'hotDurabilityPending', 'unconfirmed'] as const)('renders %s without fabricating restart evidence', async (status) => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate(value => (window as any).__transactions.receipt({ok:true,status:value}), status);
      if (status === 'hotDurabilityPending') {
        await page.getByText('Applied at runtime; persistence is still pending.', {exact:true}).waitFor();
        expect(await page.locator(button).count()).toBe(0);
      } else if (status === 'unconfirmed') {
        await page.locator(errorBar).waitFor();
      } else {
        await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
        expect(await page.locator(errorBar).count()).toBe(0);
      }
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.close();
    });
    it.each(['ready', 'failed'])('binds an early intermediate restart and then owned %s', async (phase) => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.events({ phase: "stopped", kind: "restart", terminal: false, applyId: 7, mainGeneration: 11 }); window.__transactions.receipt({ ok: true, status: "applied", applyId: 7 });');
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === true, button);
      await page.evaluate(`window.__transactions.events({ phase: "${phase}", kind: "restart", terminal: true, applyId: 6, mainGeneration: 12 });`);
      expect(await page.locator(button).isDisabled()).toBe(true);
      await page.evaluate(`window.__transactions.events({ phase: "${phase}", kind: "restart", terminal: true, applyId: 7, mainGeneration: 12 });`);
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
      expect(await page.locator(errorBar).count()).toBe(phase === 'failed' ? 1 : 0);
      await page.close();
    });
    it.each([['ready', true], ['failed', true], ['ready', false], ['failed', false]] as const)('retains the early generation high-water mark after buffer overflow: %s/attributed=%s', async (phase, attributed) => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&generation=100');
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate(`
        window.__transactions.events({ phase: "ready", kind: "restart", terminal: true, applyId: ${attributed ? '8' : 'undefined'}, mainGeneration: 102 });
        for (let mainGeneration = 1; mainGeneration <= 31; mainGeneration++) window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration });
        window.__transactions.events({ phase: "${phase}", kind: "restart", terminal: true, applyId: 7, mainGeneration: 101 });
        window.__transactions.receipt({ ok: true, status: "applied", applyId: 7 });
      `);
      // The maximum observation was the evicted 33rd-oldest payload. Neither old terminal
      // is current evidence, so the actual mounted bar must still wait with its timeout armed.
      await page.waitForFunction('typeof window.__transactions.fallback === "function"');
      expect(await page.locator(button).isDisabled()).toBe(true);
      expect(await page.locator(errorBar).count()).toBe(0);
      await page.evaluate(`window.__transactions.events({ phase: "${phase}", kind: "restart", terminal: true, applyId: 7, mainGeneration: 103 });`);
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
      expect(await page.locator(errorBar).count()).toBe(phase === 'failed' ? 1 : 0);
      await page.close();
    });
    it('invalid early generations never raise the retained floor, including across overflow', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&generation=100');
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate(`
        for (const mainGeneration of [0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration });
        for (let mainGeneration = 1; mainGeneration <= 31; mainGeneration++) window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration });
        window.__transactions.events({ phase: "ready", kind: "restart", terminal: true, applyId: 7, mainGeneration: 101 });
        window.__transactions.receipt({ ok: true, status: "applied", applyId: 7 });
      `);
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
      expect(await page.locator(errorBar).count()).toBe(0);
      await page.close();
    });
    it('an observed newer explicit stop stays neutral after its early payload is evicted', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&generation=100');
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate(`
        window.__transactions.events({ phase: "stopped", kind: "stop", terminal: true, mainGeneration: 102 });
        for (let mainGeneration = 1; mainGeneration <= 31; mainGeneration++) window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration });
        window.__transactions.events({ phase: "failed", kind: "restart", terminal: true, applyId: 7, mainGeneration: 101 });
        window.__transactions.receipt({ ok: true, status: "applied", applyId: 7 });
      `);
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
      expect(await page.locator(errorBar).count()).toBe(0);
      await page.evaluate('window.__transactions.events({ phase: "failed", kind: "restart", terminal: true, applyId: 7, mainGeneration: 103 })');
      expect(await page.locator(errorBar).count()).toBe(0);
      await page.close();
    });
    it('a delayed lower-generation stop cannot cancel after a higher early observation', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&generation=100');
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate(`
        window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration: 103 });
        window.__transactions.events({ phase: "stopped", kind: "stop", terminal: true, mainGeneration: 102 });
        for (let mainGeneration = 1; mainGeneration <= 31; mainGeneration++) window.__transactions.events({ phase: "ready", kind: "start", terminal: true, mainGeneration });
        window.__transactions.events({ phase: "ready", kind: "restart", terminal: true, applyId: 7, mainGeneration: 101 });
        window.__transactions.receipt({ ok: true, status: "applied", applyId: 7 });
      `);
      await page.waitForFunction('typeof window.__transactions.fallback === "function"');
      expect(await page.locator(button).isDisabled()).toBe(true);
      await page.evaluate('window.__transactions.events({ phase: "ready", kind: "restart", terminal: true, applyId: 7, mainGeneration: 104 })');
      await page.waitForFunction(selector => (document.querySelector(selector) as HTMLButtonElement | null)?.disabled === false, button);
      await page.close();
    });
    it('lost completion cannot succeed from an unrelated running core status', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode);
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ok:true,status:"deferred",applyId:7})');
      await page.waitForFunction('typeof window.__transactions.fallback === "function"');
      await page.evaluate('window.__transactions.fallback()');
      await page.locator(errorBar).waitFor();
      expect(await page.evaluate('window.__transactions.statusReads')).toBe(1);
      expect(await page.locator(button).isDisabled()).toBe(false);
      await page.close();
    });
    it('awaits registration before Apply and ignores unowned broadcasts', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&listener=deferred');
      await page.waitForFunction('typeof window.__transactions.releaseListener === "function"');
      await page.locator(button).click();
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(0);
      await page.evaluate('window.__transactions.releaseListener()');
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      await page.evaluate('window.__transactions.receipt({ok:true,status:"applied",applyId:7})');
      await page.waitForFunction('typeof window.__transactions.fallback === "function"');
      await page.evaluate(`
        window.__transactions.unownedStarted?.();
        window.__transactions.unownedError?.({errorCode:'STARTUP_FAILED',message:'private diagnostic'});
      `);
      expect(await page.locator(button).isDisabled()).toBe(true);
      expect(await page.locator(errorBar).count()).toBe(0);
      await page.evaluate('window.__transactions.events({phase:"failed",kind:"restart",terminal:true,applyId:7,mainGeneration:11,errorCode:"STARTUP_FAILED",message:"private diagnostic"})');
      await page.locator(errorBar).waitFor();
      expect(await page.locator(errorBar).textContent()).not.toContain('private diagnostic');
      expect(await page.locator(button).isDisabled()).toBe(false);
      await page.close();
    });
    it('registration failure blocks Apply and remains a visible retryable failure', async () => {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&listener=failed');
      await page.locator(button).click();
      await page.locator(errorBar).waitFor();
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(0);
      expect(await page.locator(button).isDisabled()).toBe(false);
      await page.close();
    });
  });
  it.each([
    { selectedServerId: 'other-node', intent: 0, outcome: 'stopped' },
    { selectedServerId: null, intent: 6, outcome: 'stopped' },
    { selectedServerId: 'other-node', intent: 0, outcome: 'changed' },
    { selectedServerId: null, intent: 6, outcome: 'changed' },
  ].flatMap(owner => ['notHeld', 'unknown', 'held'].map(preflightHold => ({ ...owner, preflightHold }))))(
    'mobile logout transparently forwards exact selection ownership: %j', async ({ selectedServerId, intent, outcome, preflightHold }) => {
    const page = await browser.newPage();
    const readMobileConfirmation = async () => {
      const confirmation: unknown = await page.evaluate('window.__transactions.mobileConfirmation()');
      if (typeof confirmation !== 'object' || confirmation === null
        || !('instanceId' in confirmation) || typeof confirmation.instanceId !== 'string'
        || !('title' in confirmation) || typeof confirmation.title !== 'string'
        || !('message' in confirmation) || typeof confirmation.message !== 'string') {
        throw new Error('Expected a mobile confirmation with string instanceId, title, and message');
      }
      return { instanceId: confirmation.instanceId, title: confirmation.title, message: confirmation.message };
    };
    await page.goto(origin + '/__transactions?mode=mobile-ts&ownedStop=' + outcome + '&intent=' + intent + '&preflightHold=' + preflightHold + (selectedServerId === null ? '&nullSelection' : ''));
    await page.locator('.m-form-btn.danger').click();
    const settings = await page.evaluate('window.__transactions.mobileStack()[0]');
    const initialConfirm = await readMobileConfirmation();
    expect(initialConfirm.title).toBe(enUS.ts.logout);
    let previousConfirm = initialConfirm;
    await page.evaluate('window.__transactions.confirmMobile()');
    if (preflightHold !== 'notHeld') {
      // Held or unproven custody requires a separate notice before any native mutation.
      const notice = await readMobileConfirmation();
      expect(notice.instanceId).not.toBe(initialConfirm.instanceId);
      expect(notice.title).toBe(enUS.ts.logoutStopTitle);
      expect(notice.message).toBe(preflightHold === 'unknown' ? enUS.ts.logoutStopUnsureMessage : enUS.ts.logoutStopMessage);
      expect(await page.evaluate('window.__transactions.mobileStack()')).toEqual([
        settings, { instanceId: notice.instanceId, kind: 'confirm' },
      ]);
      expect(await page.evaluate('window.__transactions.logoutCalls')).toEqual([]);
      expect(await page.evaluate('window.__transactions.stopCalls')).toEqual([]);
      expect(await page.evaluate('window.__transactions.unconditionalStops')).toBe(0);
      previousConfirm = notice;
      await page.evaluate('window.__transactions.confirmMobile()');
    }
    // The native refusal creates its own owned-Stop confirmation after the preflight.
    expect(await page.evaluate('window.__transactions.logoutCalls')).toEqual(['ts']);
    expect(await page.evaluate('window.__transactions.stopCalls')).toEqual([]);
    expect(await page.evaluate('window.__transactions.unconditionalStops')).toBe(0);
    expect(await page.evaluate('window.__transactions.mobileStack().at(-1).kind')).toBe('confirm');
    const ownedConfirm = await readMobileConfirmation();
    expect(ownedConfirm.instanceId).not.toBe(initialConfirm.instanceId);
    expect(ownedConfirm.instanceId).not.toBe(previousConfirm.instanceId);
    expect(ownedConfirm.title).toBe(enUS.ts.logoutStopTitle);
    expect(ownedConfirm.message).toBe(enUS.ts.logoutStopMessage);
    expect(await page.evaluate('window.__transactions.mobileStack()')).toEqual([
      settings, { instanceId: ownedConfirm.instanceId, kind: 'confirm' },
    ]);
    await page.evaluate('window.__transactions.confirmMobile()');
    expect(await page.evaluate('window.__transactions.stopCalls')).toEqual([{
      expectedMainGeneration: 7,
      expectedSelectedServerId: selectedServerId,
      expectedSelectionIntentGeneration: intent,
    }]);
    expect(await page.evaluate('window.__transactions.unconditionalStops')).toBe(0);
    expect(await page.evaluate('window.__transactions.logoutCalls')).toEqual(outcome === 'stopped' ? ['ts', 'ts'] : ['ts']);
    expect(await page.evaluate('window.__transactions.mobileStack().map(entry => entry.kind)')).toEqual(outcome === 'stopped' ? [] : ['ts-settings']);
    expect(await page.evaluate('window.__transactions.mobileStack()')).toEqual(outcome === 'stopped' ? [] : [settings]);
    expect(await page.evaluate('window.__transactions.mobileSelection()')).toBe(selectedServerId);
    await page.close();
  });
  it('selects a disjoint same-size visible set and preserves hidden choices', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=selection');
    expect(await page.locator('#nodes-batch .mono').textContent()).toBe('0');
    expect(await page.locator('#batch-test').isDisabled()).toBe(true);
    expect(await page.locator('#batch-all').getAttribute('aria-checked')).toBe('false');
    await page.locator('#select-all').click();
    expect(await page.evaluate('window.__transactions.selected()')).toEqual(['a', 'b', 'c', 'd']);
    expect(await page.locator('#nodes-batch .mono').textContent()).toBe('2');
    expect(await page.locator('#batch-all').getAttribute('aria-checked')).toBe('true');
    await page.locator('#select-all').click();
    expect(await page.evaluate('window.__transactions.selected()')).toEqual(['a', 'b']);
    await page.evaluate('window.__transactions.visible([])');
    await page.locator('#select-all').click();
    expect(await page.evaluate('window.__transactions.selected()')).toEqual(['a', 'b']);
    await page.close();
  });
  it.each(['desktop-warp', 'mobile-warp', 'mobile-ts'])('reads and writes the top-level onDemand flag without losing protocol data: %s', async (mode) => {
    for (const enabled of [true, false]) {
      const page = await browser.newPage();
      await page.goto(origin + '/__transactions?mode=' + mode + '&enabled=' + enabled);
      await page.locator(mode === 'desktop-warp' ? '.form-field-fold > summary' : '.m-form-group-h').filter({ hasText: 'Advanced' }).click();
      const toggle = page.getByRole('switch', { name: 'Connect on demand', exact: true });
      expect(await toggle.getAttribute('aria-checked')).toBe(String(enabled));
      await toggle.click();
      await page.locator(mode === 'desktop-warp' ? '.dlg-foot .btn.flow' : '.m-form-foot .primary').last().click();
      await page.waitForFunction('!!window.__transactions.payload()');
      const payload = await page.evaluate<ServerConfig>('window.__transactions.payload()');
      expect(payload.onDemand).toBe(enabled ? undefined : true);
      if (mode === 'mobile-ts') expect(payload.tailscaleSettings?.hostname).toBe('synthetic-ts');
      else expect(payload.wireguardSettings?.warpDevice?.deviceId).toBe('synthetic-device');
      await page.close();
    }
  });
  it('falls back after staged deletion of a selected mobile tab and does not jump back on undo', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=mobile-nodes');
    await page.locator('.mn-seg').filter({ hasText: 'Subscription two' }).click();
    await page.evaluate('window.__transactions.removeSub("two")');
    await page.waitForFunction('!document.querySelector(".mn-seg[aria-current=true]")?.textContent.includes("Subscription two")');
    const fallback = await page.locator('.mn-seg[aria-current=true]').textContent();
    expect(fallback).toBeTruthy();
    await page.evaluate('window.__transactions.undoSub("two")');
    await page.locator('.mn-seg').filter({ hasText: 'Subscription two' }).waitFor();
    expect(await page.locator('.mn-seg[aria-current=true]').textContent()).toBe(fallback);
    await page.close();
  });
  it('a persisted catalog entry with no file stays downloadable', async () => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=resources');
    await page.getByRole('option').filter({ hasText: 'builtin' }).waitFor();
    expect(await page.getByRole('option').filter({ hasText: 'builtin' }).isDisabled()).toBe(true);
    await page.locator('.m-form-foot button[aria-pressed=false]').click();
    const missing = page.getByRole('option').filter({ hasText: 'missing' });
    await missing.waitFor();
    expect(await missing.isDisabled()).toBe(false);
    expect(await page.getByRole('option').filter({ hasText: 'present' }).isDisabled()).toBe(true);
    await page.close();
  });
  it.each([['single', 'reject'], ['single', 'false'], ['all', 'reject'], ['all', 'false'], ['filtered', 'reject']])('restores only failed mobile close intents after an intervening frame: %s/%s', async (kind, failure) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=connections');
    await page.getByRole('tab').nth(1).click();
    await page.waitForFunction('typeof window.__transactions.frame === "function"');
    await page.evaluate('window.__transactions.emit(["a", "b"], true)');
    await page.locator('[data-conn-row="a"]').waitFor();
    if (kind === 'single') {
      await page.locator('[data-conn-row="a"] .mc-row-x').click();
    } else {
      if (kind === 'filtered') await page.locator('[data-conn-search] input').fill('example');
      await page.locator('.mc-head .mc-btn').click();
      const action = page.locator('.mc-sheet-item').nth(kind === 'all' ? 3 : 2);
      await action.click(); await action.click();
    }
    await page.waitForFunction(kind === 'all' ? '!!window.__transactions.pending.all' : '!!window.__transactions.pending.a');
    await page.evaluate('window.__transactions.emit(["a", "b"])');
    // Freeze the live projection after it has hidden closing rows; rollback cannot await another frame.
    if (kind === 'single') await page.locator('.mc-head .mc-btn').click();
    await page.locator('.mc-sheet-item').first().click();
    const settle = failure === 'false' ? 'resolve({ok:false})' : 'reject(new Error("synthetic"))';
    await page.evaluate('window.__transactions.pending.' + (kind === 'all' ? 'all' : 'a') + '.' + settle + (kind === 'filtered' ? '; window.__transactions.pending.b.resolve({ok:true})' : ''));
    await page.locator('[data-conn-row="a"]').waitFor();
    expect(await page.locator('[data-conn-row="b"]').count()).toBe(kind === 'filtered' ? 0 : 1);
    expect(await page.locator('[data-write-error]').count()).toBeGreaterThan(0);
    await page.close();
  });
  it.each(['route', 'dns', 'mobile-route', 'mobile-dns'])('staged %s toggle does not mutate disk and revert/reset restores visible value', async (mode) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=' + mode);
    if (mode === 'mobile-dns') await page.locator('.mr-seg [role="tab"]').nth(1).click();
    const toggle = page.locator(mode.startsWith('mobile-') ? '#mr-rule-' + (mode.endsWith('dns') ? 'dns' : 'route') + '-r' : '#rule-list [role="switch"]');
    await toggle.click();
    expect(await page.evaluate('window.__transactions.disk()')).toBe(true);
    expect(await toggle.getAttribute('aria-checked')).toBe('false');
    await page.evaluate('window.__transactions.revert()');
    expect(await toggle.getAttribute('aria-checked')).toBe('true');
    await toggle.click();
    await page.evaluate('window.__transactions.reset()');
    expect(await toggle.getAttribute('aria-checked')).toBe('true');
    expect(await page.evaluate('window.__transactions.writes')).toBe(0);
    await page.close();
  });
  it.each(['route', 'dns', 'mobile-route', 'mobile-dns'])('direct %s failure preserves newer same-ID and other-rule mirror updates', async (mode) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=' + mode + '&direct=1');
    if (mode === 'mobile-dns') await page.locator('.mr-seg [role="tab"]').nth(1).click();
    const toggle = page.locator(mode.startsWith('mobile-') ? '#mr-rule-' + (mode.endsWith('dns') ? 'dns' : 'route') + '-r' : '#rule-list [role="switch"]');
    await toggle.click();
    await page.waitForFunction('typeof window.__transactions.rejectToggle === "function"');
    await page.evaluate('window.__transactions.newerMirror(); window.__transactions.rejectToggle(new Error("synthetic toggle failure"))');
    await page.waitForFunction('window.__transactions.mirror()[0].remarks === "Newer authoritative value"');
    // Let the rejected production handler settle before asserting it did not restore its old array.
    await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 0)));
    expect(await page.evaluate('window.__transactions.mirror().map(r => [r.id, r.remarks])')).toEqual([
      ['r', 'Newer authoritative value'], ['other', 'Concurrent added rule'],
    ]);
    expect(await page.evaluate('window.__transactions.disk()')).toBe(false);
    await page.close();
  });
  it.each(['res-url', 'res-catalog', 'network-profile'])('late %s completion closes only its captured instance beneath a new VPN dialog', async (mode) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=' + mode);
    if (mode === 'res-url') await page.locator('#res-url').fill('https://example.invalid/fixture.srs');
    else if (mode === 'res-catalog') {
      await page.locator('.cat-tabs [role="tab"]').nth(1).click();
      await page.locator('.cat-item [role="checkbox"]').click();
    } else await page.locator('dialog input').first().fill('Updated profile');
    await page.locator('dialog[open] .dlg-foot .flow').click();
    await page.waitForFunction('typeof window.__transactions.release === "function"');
    // Escape while saving must retain the original form; then another auth challenge arrives.
    await page.keyboard.press('Escape');
    expect(await page.evaluate('window.__transactions.stack().some(d => d.instanceId === window.__transactions.a)')).toBe(true);
    await page.evaluate('window.__transactions.b = window.__transactions.openVpn("b")');
    await page.locator('dialog[open]').last().locator('input').first().fill('Retained VPN draft');
    await page.evaluate('window.__transactions.release()');
    await page.waitForFunction('window.__transactions.stack().length === 1');
    expect(await page.evaluate('window.__transactions.stack()[0].instanceId')).toBe(await page.evaluate('window.__transactions.b'));
    expect(await page.locator('dialog[open] input').first().inputValue()).toBe('Retained VPN draft');
    await page.close();
  });
  it.each(['res-url', 'network-profile'])('captured %s discard removes its own confirm and form without touching a newer VPN', async (mode) => {
    const page = await browser.newPage();
    await page.goto(origin + '/__transactions?mode=' + mode);
    if (mode === 'res-url') await page.locator('#res-url').fill('https://example.invalid/fixture.srs');
    else await page.locator('dialog input').first().fill('Discarded profile draft');
    await page.locator('dialog[open] .dlg-x').click();
    await page.waitForFunction('window.__transactions.stack().length === 2');
    await page.evaluate('window.__transactions.captureConfirm(); window.__transactions.b = window.__transactions.openVpn("b"); window.__transactions.confirm()');
    await page.waitForFunction('window.__transactions.stack().length === 1');
    expect(await page.evaluate('window.__transactions.stack()[0].instanceId')).toBe(await page.evaluate('window.__transactions.b'));
    await page.close();
  });

});

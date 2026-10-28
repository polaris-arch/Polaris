import { chromium, type Browser, type Page } from '@playwright/test';
import { createServer, type ViteDevServer } from 'vite';
import path from 'node:path';
import { beforeAll, afterAll, afterEach, describe, expect, it, vi } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import { RUNTIME_APPLY_POLL_INTERVAL_MS } from '@/components/layout/runtime-apply-polling';
import enUS from '@/i18n/locales/en-US.json';
import zhCN from '@/i18n/locales/zh-CN.json';
import fa from '@/i18n/locales/fa.json';

type FixtureLanguage = 'en-US' | 'zh-CN' | 'fa';
const fixtureCopy = { 'en-US': enUS, 'zh-CN': zhCN, fa };
function assertFixtureLocale(snapshot: unknown, language: FixtureLanguage) {
  expect(snapshot).toEqual({ language, resolvedLanguage: language, direction: language === 'fa' ? 'rtl' : 'ltr', resourceLoaded: true,
    sample: fixtureCopy[language].feedback.subscriptionSaved });
}

async function openFixture(page: Page, url: string) {
  const target = new URL(url);
  const feedback = target.pathname === '/__feedback';
  const language = (feedback ? target.searchParams.get('lang') || 'zh-CN' : 'en-US') as FixtureLanguage;
  const key = feedback ? '__feedback' : '__transactions';
  await page.goto(url);
  // goto does not await async ES-module evaluation (including lazy locale chunks).
  // This is within the existing test timeout; it never repeats an application action.
  await page.waitForFunction(name => (window as unknown as Record<string, { ready?: boolean }>)[name]?.ready === true, key);
  const bootstrap = await page.evaluate(name => (window as unknown as Record<string, { bootstrap: unknown }>)[name].bootstrap, key);
  assertFixtureLocale(bootstrap, language);
  if (feedback && target.searchParams.has('helper-bridge')) {
    if (target.searchParams.has('recovery')) {
      await page.waitForFunction(() => (window as any).__feedback.privacyApplied && (window as any).__feedback.trayReads >= 1);
      expect(await page.evaluate('window.__feedback.unexpected')).toEqual([]);
      return;
    }
    await page.waitForFunction(() => {
      const state = (window as any).__feedback;
      return state.privacyApplied && state.configReads > 0 && state.cleanExitReads === 1 && state.pendingAcks > 0 && state.trayReads === 1;
    });
    expect(await page.evaluate('window.__feedback.unexpected')).toEqual([]);
  }
}

async function closeFixturePages(browser: Browser | undefined, failed: boolean, name: string) {
  const cleanupErrors: string[] = [];
  if (!browser) return cleanupErrors;
  for (const context of browser.contexts()) {
    try {
      if (failed) for (const [index, page] of context.pages().entries()) {
        if (page.isClosed()) continue;
        try {
          const { mkdirSync, writeFileSync } = await import('node:fs');
          const output = path.join(root, 'test-results', 'feedback');
          mkdirSync(output, { recursive: true });
          const stem = 'failed-' + name.replace(/[^a-zA-Z0-9-]/g, '-').slice(0, 100) + '-' + index;
          const state = await page.evaluate(() => {
            const globals = window as unknown as Record<string, Record<string, unknown> | undefined>;
            const transactions = globals.__transactions;
            const feedback = globals.__feedback;
            const count = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
            return { locale: document.documentElement.lang, documentReadyState: document.readyState,
              transactionsReady: transactions?.ready === true, feedbackReady: globals.__feedback?.ready === true,
              // Only bounded synthetic fixture counts, never arbitrary DOM/IPC values.
              feedbackCounts: { installs: count(feedback?.installs), checks: count(feedback?.checks),
                privacyReads: count(feedback?.privacyReads), privacyApplied: feedback?.privacyApplied === true,
                cleanExitReads: count(feedback?.cleanExitReads), pendingAcks: count(feedback?.pendingAcks), trayReads: count(feedback?.trayReads) },
              dialogs: { attached: document.querySelectorAll('dialog').length, open: document.querySelectorAll('dialog[open]').length,
                hidden: document.querySelectorAll('dialog[aria-hidden="true"]').length },
              transactionCounts: { runtimeStatusReads: count(transactions?.runtimeStatusReads),
                applyCalls: count(transactions?.applyCalls), writes: count(transactions?.writes),
                persistenceRetries: Array.isArray(transactions?.persistenceTransactions) ? transactions.persistenceTransactions.length : null } };
          });
          writeFileSync(path.join(output, stem + '.json'), JSON.stringify({ fixtureOnly: true, state }, null, 2));
          await page.screenshot({ path: path.join(output, stem + '.png'), fullPage: true });
        } catch { console.warn('[fixture] failure evidence unavailable'); }
      }
    } catch {
      // Evidence enumeration is also best-effort; it must not skip later contexts.
      console.warn('[fixture] failure evidence unavailable');
    } finally {
      // Existing success paths close their page/context. Only surviving contexts are here.
      try { await context.close(); } catch { cleanupErrors.push('fixture-context-close-failed'); }
    }
  }
  return cleanupErrors;
}

function enforceFixtureCleanup(errors: string[], alreadyFailed: boolean, metadata: object) {
  if (errors.length === 0) return;
  const receipt = { fixtureCleanupErrors: errors };
  Object.assign(metadata, receipt);
  if (!alreadyFailed) throw new Error('fixture-context-cleanup-failed');
  // Keep the original test error as primary, while attaching fixed secondary evidence.
  console.error('[fixture] additional context cleanup failure');
  return receipt;
}

describe('browser fixture locale contract (independent expected copy)', () => {
  it.each(['en-US', 'zh-CN', 'fa'] as const)('requires actual %s resources, not just a language label', language => {
    const snapshot = { language, resolvedLanguage: language, direction: language === 'fa' ? 'rtl' : 'ltr', resourceLoaded: true, sample: fixtureCopy[language].feedback.subscriptionSaved };
    assertFixtureLocale(snapshot, language);
    expect(() => assertFixtureLocale({ ...snapshot, resourceLoaded: false }, language)).toThrow();
    expect(() => assertFixtureLocale({ ...snapshot, direction: language === 'fa' ? 'ltr' : 'rtl' }, language)).toThrow();
    expect(() => assertFixtureLocale({ ...snapshot, resolvedLanguage: 'fallback' }, language)).toThrow();
    expect(() => assertFixtureLocale({ ...snapshot, sample: 'missing-key' }, language)).toThrow();
    if (language !== 'en-US') expect(() => assertFixtureLocale({ ...snapshot, sample: enUS.feedback.subscriptionSaved }, language)).toThrow();
  });
});

describe('browser fixture cleanup failure contract', () => {
  it('attempts every context after multiple close failures and only returns fixed codes', async () => {
    const attempts: number[] = [];
    const browser = { contexts: () => [0, 1, 2].map(index => ({
      pages: () => [], close: async () => { attempts.push(index); if (index !== 1) throw new Error('private-detail'); },
    })) } as unknown as Browser;
    const errors = await closeFixturePages(browser, false, 'synthetic');
    expect(attempts).toEqual([0, 1, 2]);
    expect(errors).toEqual(['fixture-context-close-failed', 'fixture-context-close-failed']);
    expect(JSON.stringify(errors)).not.toContain('private-detail');
  });
  it('fails a previously successful test when cleanup is incomplete', () => {
    const metadata = {};
    expect(() => enforceFixtureCleanup(['fixture-context-close-failed'], false, metadata)).toThrow('fixture-context-cleanup-failed');
    expect(metadata).toEqual({ fixtureCleanupErrors: ['fixture-context-close-failed'] });
  });
  it('preserves an existing primary failure and attaches the secondary fixed cleanup fact', () => {
    const primary = new Error('original assertion');
    const metadata = { primary };
    const report = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      expect(() => enforceFixtureCleanup(['fixture-context-close-failed'], true, metadata)).not.toThrow();
      expect(metadata.primary).toBe(primary);
      expect(metadata).toEqual({ primary, fixtureCleanupErrors: ['fixture-context-close-failed'] });
      expect(report).toHaveBeenCalledExactlyOnceWith('[fixture] additional context cleanup failure');
    } finally { report.mockRestore(); }
  });
  it('keeps evidence failures best-effort while closing all contexts', async () => {
    const closed: number[] = [];
    const browser = { contexts: () => [0, 1].map(index => ({
      pages: () => { throw new Error('evidence-unavailable'); }, close: async () => { closed.push(index); },
    })) } as unknown as Browser;
    const report = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      expect(await closeFixturePages(browser, true, 'synthetic')).toEqual([]);
      expect(closed).toEqual([0, 1]);
      expect(() => enforceFixtureCleanup([], false, {})).not.toThrow();
    } finally { report.mockRestore(); }
  });
});

// The real UI consumers run against deferred, synthetic API receipts. No native mutation/network.
const root = path.resolve(import.meta.dirname, '../../..');
const entry = `
import React, { useState, useLayoutEffect } from 'react';
import { flushSync } from 'react-dom';
import { NodeMenu } from '/src/components/screens/home/NodeMenu';
import { SubFormPanel } from '/src/mobile/forms/SubFormPanel';
import { useSubscriptionProgressStore } from '/src/store/use-subscription-progress-store';
import { setToastImpl } from '/src/lib/error-handler';
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
// A fresh context's explicit navigator locale drives the real lazy resource loader.
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
  const deferred = new URLSearchParams(location.search).has('deferredPending');
  test.publishPending = value => useAppStore.getState().setPendingChanges({ added: [], removed: [], modified: [], restartDeferred: value });
  test.publishPending(!deferred);
  // Saved intent and the later native pending PUSH are independent publications.
  if (deferred) useAppStore.setState({ config: { ...config, selectionIntent: { mode: 'auto', scope: 'global' } } });
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
let editInstance;
if (mode === 'subscription-edit' || mode === 'mobile-subscription-edit') {
  config.subscriptions = [{id:'sub',name:'Fixture',url:'https://example.invalid/old',autoUpdate:false}];
  config.servers = [{id:'old-node',name:'Preserved',protocol:'http',address:'example.invalid',port:8080,subscriptionId:'sub'}];
  useAppStore.setState({config,servers:config.servers});
  systemApi.listNetworkInterfaces = async () => [];
  test.toasts = [];
  setToastImpl(Object.fromEntries(['success','info','warning','error'].map(channel => [channel,(message, detail) => test.toasts.push({channel,message,detail})])));
  test.refreshCalls = 0;
  test.pendingEdits = [];
  test.previewCalls = 0;
  api.subscription.preview = async () => { test.previewCalls++; return {ok:false,errorKind:'timeout'}; };
  api.subscription.updateServers = async () => {test.refreshCalls++;throw new Error('no second fetch');};
  const editListeners = new Set();
  api.subscription.onUpdateProgressReady = async listener => {
    if(new URLSearchParams(location.search).has('delayEditListener')) await new Promise(resolve => {test.readyEditListener = resolve;});
    editListeners.add(listener);return () => editListeners.delete(listener);
  };
  api.subscription.update = async (next, editRequestId) => {
    test.writes++;
    test.submitted = structuredClone(next);
    return new Promise((resolve,reject) => {const saved = result => resolve({...result,editRequestId});test.release = saved;test.reject = reject;test.pendingEdits.push({resolve:saved,reject,editRequestId});});
  };
  test.progress = frame => {const value = {subscriptionId:'sub',generation:test.writes,editRequestId:test.pendingEdits.at(-1)?.editRequestId,saved:true,...frame};useSubscriptionProgressStore.getState().applyProgress(value);for(const listener of editListeners) listener(value);};
  editInstance = mode === 'subscription-edit' ? useDialogStore.getState().open({kind:'sub',subId:'sub'}) : useMobileFormStore.getState().open({kind:'sub',subId:'sub'});
}
function MobileSubscriptionEdit() {
  const active = useMobileFormStore(s => s.stack.some(entry => entry.instanceId === editInstance));
  return active ? <SubFormPanel instanceId={editInstance} subId="sub" /> : null;
}
function Selection() {
  const [selectedIds, setSelectedIds] = useState(new Set(['a', 'b']));
  const [visible, setVisible] = useState([{ id: 'c' }, { id: 'd' }]);
  const actions = useNodeActions({ servers: visible, visibleServers: visible, selectedIds, setSelectedIds, t: (key) => key, stagingEnabled: true, stage: () => {}, openDialog: () => {} });
  test.selected = () => [...selectedIds].sort();
  test.visible = (ids) => setVisible(ids.map(id => ({ id })));
  return <><button id="select-all" onClick={actions.selectAll}>Select all</button><NodesBatchBar t={key => key} selectedIds={selectedIds} visibleServers={visible} selectAll={actions.selectAll} testSelected={() => {}} testing={false} isSubTab copyLinksBatch={() => {}} confirmArmed={null} nodeDeletion={{ deleteBatch: () => {} }} exitBatch={() => {}} /></>;
}
// Real React lifecycle + production NodeMenu/grouping/reveal/store components.
// Only props are synthetic; no DOM injection, hook mock, or native IPC is used.
function NodeMenuLifecycle() {
  const fixtureServers = ['a', 'b'].map(id => ({id: 'node-' + id, name: 'Loop ' + id.toUpperCase(), protocol: 'socks', address: '127.0.0.1', port: 1, subscriptionId: 'fixture'}));
  const fixtureSubscriptions = [{id: 'fixture', name: 'Package loopback', url: 'https://fixture.invalid'}];
  const [props, setProps] = useState({open: true, servers: new URLSearchParams(location.search).has('initialGroups') ? fixtureServers : [], subscriptions: fixtureSubscriptions, selectedServerId: 'node-a', latencies: {}});
  test.menuCommits ??= [];
  test.menuSnapshot = () => [...document.querySelectorAll('.ns-grp')].map(el => ({label: el.querySelector('span:not(.ns-c)')?.textContent, expanded: el.getAttribute('aria-expanded')}));
  test.menuPatch = patch => flushSync(() => setProps(prev => ({...prev, ...patch})));
  test.menuLoad = () => test.menuPatch({servers: fixtureServers, subscriptions: fixtureSubscriptions});
  test.menuEqualConfig = () => flushSync(() => setProps(prev => ({...prev, servers: structuredClone(prev.servers), subscriptions: structuredClone(prev.subscriptions)})));
  test.menuNewGroup = () => flushSync(() => setProps(prev => ({...prev, selectedServerId: 'node-c', servers: [...prev.servers, {...fixtureServers[0], id: 'node-c', name: 'Loop C', subscriptionId: 'later'}], subscriptions: [...prev.subscriptions, {id: 'later', name: 'Later group', url: 'https://fixture.invalid'}]})));
  useLayoutEffect(() => { test.menuCommits.push(test.menuSnapshot()); });
  return <NodeMenu {...props} blockDisabledReason={null} onPick={id => {test.menuPicked = id;}} onPickDirect={() => {}} onPickBlock={() => {}} onTestAll={() => {}} onManage={() => {}} />;
}
function FixtureReady() {
  useLayoutEffect(() => {
    test.bootstrap = { language: i18n.language, resolvedLanguage: i18n.resolvedLanguage, direction: document.documentElement.dir, resourceLoaded: i18n.hasResourceBundle(i18n.language, 'translation'), sample: i18n.t('feedback.subscriptionSaved') };
    test.ready = true;
    return () => { test.ready = false; };
  }, []);
  return null;
}
createRoot(document.getElementById('root')).render(<><DialogHost /><FixtureReady />
  {mode === 'node-menu' && <NodeMenuLifecycle />}
  {mode === 'apply' && <PendingChangesBar />}
  {mode === 'mobile-apply' && <main className="mobile-root"><MobilePendingBar /></main>}
  {mode === 'selection' && <Selection />}
  {mode === 'mobile-subscription-edit' && <MobileSubscriptionEdit />}
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
  afterEach(async ({ task }) => {
    const failed = task.result?.state === 'fail';
    const cleanup = await closeFixturePages(browser, failed, task.name);
    enforceFixtureCleanup(cleanup, failed, task.meta);
  });
  afterAll(async () => { await browser?.close(); await server?.close(); });

  for (const mode of ['subscription-edit','mobile-subscription-edit']) {
    const urlInput = mode === 'subscription-edit' ? '#sub-url' : '#msf-url';
    it(mode + ' shows the classified preview failure with a live title and no save', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
      await page.getByRole('button', {name: enUS.sub.previewBtn, exact:true}).click();
      const notice = page.locator(mode === 'subscription-edit' ? '[data-feedback-notice]' : '.m-form-notice-detailed');
      await notice.waitFor();
      expect(await notice.getAttribute('role')).toBe('status');
      expect(await notice.locator('strong').textContent()).toBe(enUS.sub.previewFail);
      expect(await notice.textContent()).not.toContain('[object Object]');
      expect(await page.evaluate('window.__transactions.previewCalls')).toBe(1);
      expect(await page.evaluate('window.__transactions.writes')).toBe(0);
      await page.close();
    });
    it(mode + ' keeps saved and failed follow-up as separate facts without replay', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
      await page.locator(urlInput).fill('https://example.invalid/new');
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.waitForFunction('window.__transactions.writes === 1');
      await page.evaluate('window.__transactions.release({saved:true,publication:"failed",refresh:null})');
      await page.waitForFunction('window.__transactions.toasts.length === 2');
      expect(await page.evaluate('window.__transactions.toasts[1]')).toEqual({channel:'error',message:enUS.feedback.subscriptionSaved,detail:enUS.feedback.followupIncomplete});
      expect(await page.evaluate('window.__transactions.writes')).toBe(1);
      expect(await page.evaluate('window.__transactions.refreshCalls')).toBe(0);
      await page.close();
    });
    it(mode + ' registers the saved listener before mutation and remains locked if no ack arrives', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&delayEditListener=1');
      await page.locator(urlInput).fill('https://example.invalid/first');
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.waitForFunction('typeof window.__transactions.readyEditListener === "function"');
      expect(await page.evaluate('window.__transactions.writes')).toBe(0);
      await page.locator(urlInput).fill('https://example.invalid/latest');
      expect(await page.getByRole('button', {name:'Save',exact:true}).isDisabled()).toBe(true);
      await page.evaluate('window.__transactions.readyEditListener()');
      await page.waitForFunction('window.__transactions.writes === 1');
      expect(await page.getByRole('button', {name:'Save',exact:true}).isDisabled()).toBe(true);
      await page.evaluate('window.__transactions.reject(new Error("save-failed-no-ack"))');
      await page.waitForFunction('!Array.from(document.querySelectorAll("button")).find(x=>x.textContent.trim()==="Save")?.disabled');
      expect(await page.evaluate('window.__transactions.toasts')).toEqual([]);
      expect(await page.evaluate('window.__transactions.writes')).toBe(1);
      await page.close();
    });
    it(mode + ' awaits one saved edit and shows refresh failure separately without a second request', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
      await page.locator(urlInput).fill('https://example.invalid/new');
      await page.getByRole('button', {name: 'Save', exact: true}).click();
      await page.waitForFunction('window.__transactions.writes === 1');
      await page.evaluate('window.__transactions.progress({phase:"fetching"})');
      await page.getByText('Fetching…', {exact:true}).waitFor();
      await page.evaluate('window.__transactions.release({saved:true,publication:"scheduled",refresh:{success:false,addedServers:0,updatedServers:0,deletedServers:0,errorKind:"timeout",generation:1}})');
      await page.waitForFunction('window.__transactions.toasts.length === 2');
      expect(await page.evaluate('window.__transactions.toasts.map(x=>x.channel)')).toEqual(['success','error']);
      expect(await page.evaluate('window.__transactions.writes')).toBe(1);
      expect(await page.evaluate('window.__transactions.refreshCalls')).toBe(0);
      await page.close();
    });
    it(mode + ' serializes save until its correlated ack then permits a newer fetch without old completion clearing busy', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
      await page.locator(urlInput).fill('https://example.invalid/first');
      await page.getByRole('button',{name:'Save',exact:true}).click();
      await page.waitForFunction('window.__transactions.writes === 1');
      await page.locator(urlInput).fill('https://example.invalid/latest');
      expect(await page.getByRole('button',{name:'Save',exact:true}).isDisabled()).toBe(true);
      await page.evaluate('window.__transactions.progress({phase:"fetching",editRequestId:"wrong-request"})');
      expect(await page.getByRole('button',{name:'Save',exact:true}).isDisabled()).toBe(true);
      expect(await page.evaluate('window.__transactions.writes')).toBe(1);
      await page.evaluate('window.__transactions.progress({phase:"fetching"})');
      await page.getByRole('button',{name:'Save',exact:true}).click();
      await page.waitForFunction('window.__transactions.writes === 2');
      await page.evaluate('window.__transactions.progress({phase:"fetching",editRequestId:window.__transactions.pendingEdits[0].editRequestId})');
      expect(await page.getByRole('button',{name:'Save',exact:true}).isDisabled()).toBe(true);
      await page.evaluate('window.__transactions.pendingEdits[0].reject(new Error("old-failure"))');
      expect(await page.getByRole('button',{name:'Save',exact:true}).isDisabled()).toBe(true);
      expect(await page.evaluate('window.__transactions.toasts')).toEqual([]);
      expect(await page.locator(urlInput).inputValue()).toBe('https://example.invalid/latest');
      await page.evaluate('window.__transactions.pendingEdits[1].resolve({saved:true,publication:"scheduled",refresh:null})');
      await page.waitForFunction('window.__transactions.toasts.length === 1');
      expect(await page.evaluate('window.__transactions.writes')).toBe(2);
      expect(await page.evaluate('window.__transactions.refreshCalls')).toBe(0);
      await page.close();
    });
    it(mode + ' ignores a late old save failure after the input revision changes', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
      await page.locator(urlInput).fill('https://example.invalid/first');
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.waitForFunction('window.__transactions.writes === 1');
      await page.locator(urlInput).fill('https://example.invalid/latest');
      await page.evaluate('window.__transactions.reject(new Error("synthetic-save-failure"))');
      await page.waitForFunction('!Array.from(document.querySelectorAll("button")).find(x=>x.textContent.trim()==="Save")?.disabled');
      expect(await page.locator(urlInput).inputValue()).toBe('https://example.invalid/latest');
      expect(await page.evaluate('window.__transactions.toasts')).toEqual([]);
      await page.getByRole('button', {name:'Save',exact:true}).click();
      await page.waitForFunction('window.__transactions.writes === 2');
      expect(await page.evaluate('window.__transactions.submitted.url')).toBe('https://example.invalid/latest');
      await page.evaluate('window.__transactions.release({saved:true,publication:"scheduled",refresh:null})');
      await page.close();
    });
  }

  it('node menu first committed render already expands the selected group', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=node-menu&initialGroups=1');
    await page.waitForFunction('window.__transactions.menuCommits?.length > 0');
    expect(await page.evaluate('window.__transactions.menuCommits[0]')).toEqual([{label: 'Package loopback', expanded: 'true'}]);
    await page.close();
  });

  it('node menu initializes async groups before commit and preserves explicit choices across config/status publications', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=node-menu');
    await page.waitForFunction('!!window.__transactions.menuLoad');
    await page.evaluate('window.__transactions.menuLoad()');
    // Parent layout effect observes the committed child BEFORE passive effects.
    // The old empty-Set + useEffect reset commits a collapsed selected group here.
    expect(await page.evaluate('window.__transactions.menuCommits.at(-1)')).toEqual([{label: 'Package loopback', expanded: 'true'}]);
    const group = page.locator('.ns-grp').filter({hasText: 'Package loopback'});
    await group.click();
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await page.evaluate('window.__transactions.menuEqualConfig()');
    await page.waitForFunction('window.__transactions.menuCommits.length >= 3');
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await page.evaluate('window.__transactions.menuPatch({selectedServerId:null,latencies:{}})');
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await group.click();
    await page.evaluate('window.__transactions.menuEqualConfig()');
    await page.evaluate('window.__transactions.menuPatch({selectedServerId:"node-b"})');
    expect(await group.getAttribute('aria-expanded')).toBe('true');
    await page.getByRole('option', {name: /Loop B/}).click();
    expect(await page.evaluate('window.__transactions.menuPicked')).toBe('node-b');
    await page.close();
  });

  it('node menu initializes only unseen async groups and resets on close/reopen', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=node-menu');
    await page.waitForFunction('!!window.__transactions.menuLoad');
    await page.evaluate('window.__transactions.menuLoad()');
    const first = page.locator('.ns-grp').filter({hasText: 'Package loopback'});
    await first.click();
    await page.evaluate('window.__transactions.menuNewGroup()');
    await page.waitForFunction('document.querySelectorAll(".ns-grp").length === 2');
    expect(await first.getAttribute('aria-expanded')).toBe('false');
    expect(await page.locator('.ns-grp').filter({hasText: 'Later group'}).getAttribute('aria-expanded')).toBe('true');
    expect(await page.evaluate('window.__transactions.menuCommits.at(-1)')).toEqual([{label: 'Package loopback', expanded: 'false'}, {label: 'Later group', expanded: 'true'}]);
    await page.locator('.ns-grp').filter({hasText: 'Later group'}).click();
    await page.evaluate('window.__transactions.menuPatch({open:false})');
    expect(await page.locator('#node-menu').getAttribute('hidden')).not.toBeNull();
    await page.evaluate('window.__transactions.menuPatch({open:true})');
    expect(await page.evaluate('window.__transactions.menuCommits.at(-1)')).toEqual([{label: 'Package loopback', expanded: 'false'}, {label: 'Later group', expanded: 'true'}]);
    await page.close();
  });

  it('node menu search temporarily expands without destroying saved collapse intent', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=node-menu');
    await page.waitForFunction('!!window.__transactions.menuLoad');
    await page.evaluate('window.__transactions.menuLoad()');
    // Eight real fixture rows expose the production search input.
    await page.evaluate('window.__transactions.menuPatch({servers:Array.from({length:8},(_,i)=>({id:"node-"+i,name:"Loop "+i,protocol:"socks",address:"127.0.0.1",port:1,subscriptionId:"fixture"})),selectedServerId:null})');
    const group = page.locator('.ns-grp');
    // Existing group preserves its initial true state until explicitly collapsed.
    await group.click();
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await page.locator('#nm-search-inp').fill('Loop 1');
    expect(await group.getAttribute('aria-expanded')).toBe('true');
    expect(await page.getByRole('option').count()).toBe(1);
    await page.locator('#nm-search-inp').fill('');
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await page.locator('#nm-search-inp').fill('Loop');
    await page.evaluate('window.__transactions.menuPatch({open:false})');
    await page.evaluate('window.__transactions.menuPatch({open:true})');
    expect(await page.locator('#nm-search-inp').inputValue()).toBe('');
    expect(await group.getAttribute('aria-expanded')).toBe('false');
    await page.close();
  });

  it('late VPN success closes its own instance, never a new challenge on top', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=vpn');
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=vpn');
    await page.locator('dialog[open] .dlg-foot .flow').click();
    await page.evaluate('window.__transactions.replaceChallenge("a", "newer"); window.__transactions.release()');
    await page.waitForFunction('document.querySelector("dialog[open] .flow")?.disabled === false');
    expect(await page.evaluate('window.__transactions.stack().length')).toBe(1);
    await page.close();
  });
  it('failed terminal received before Cancel confirmation remains dismissible', async () => {
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=recovery');
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
    it('does not treat saved intent plus an empty first frame as accepted runtime', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&deferredPending=1');
      await page.waitForFunction('typeof window.__transactions.publishPending === "function"');
      expect(await page.locator(button).count()).toBe(0);
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(0);
      await page.evaluate('window.__transactions.publishPending(true)');
      await page.locator(button).waitFor({state:'visible'});
      // A newer empty publication must remove the action, not authorize a stale click.
      await page.evaluate('window.__transactions.publishPending(false)');
      await page.locator(button).waitFor({state:'hidden'});
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(0);
      await page.evaluate('window.__transactions.publishPending(true)');
      await page.locator(button).click();
      await page.waitForFunction('typeof window.__transactions.receipt === "function"');
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.evaluate('window.__transactions.publishPending(false); window.__transactions.receipt({ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0})');
      await page.locator(button).waitFor({state:'hidden'});
      expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
      await page.close();
    });
    it('reconciles persistence through the original status token without replay', async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      // Only this two-poll scenario controls the existing production 2s timers.
      // The outer 5s test deadline stays real; no callback or mutation is replayed.
      await page.clock.install();
      let scenarioCompleted = false;
      try {
        await openFixture(page, origin + '/__transactions?mode=' + mode);
        await page.locator(button).click();
        await page.waitForFunction('typeof window.__transactions.receipt === "function"');
        await page.evaluate('window.__transactions.receipt({ ok:true,status:"hotDurabilityPending",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
        await page.getByRole('button', { name: 'Retry persistence', exact: true }).waitFor();
        // Let the committed pending view's passive effects schedule its real poll.
        await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
        await page.clock.runFor(RUNTIME_APPLY_POLL_INTERVAL_MS);
        await page.waitForFunction('window.__transactions.runtimeStatusReads === 1');
        expect(await page.evaluate('window.__transactions.runtimeTransactions')).toEqual(['original']);
        await page.evaluate('window.__transactions.obsoleteStatusReceipt = window.__transactions.statusReceipt');
        await page.evaluate('window.__transactions.stagePersistenceDraft()');
        await page.getByRole('button', { name: 'Retry persistence', exact: true }).click();
        await page.waitForFunction('typeof window.__transactions.retryReceipt === "function"');
        if (source === 'poll') {
          await page.evaluate('window.__transactions.retryReceipt({ ok:false,status:"unconfirmed",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
          await page.getByRole('button', { name: 'Retry persistence', exact: true }).waitFor();
          // The cancelled attempt cannot settle the new unconfirmed generation.
          await page.evaluate('window.__transactions.obsoleteStatusReceipt({ ok:true,status:"hotApplied",transactionId:"original",runtimeGeneration:10,selectionIntentGeneration:0 });');
          await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
          expect(await page.getByRole('button', { name: 'Retry persistence', exact: true }).count()).toBe(1);
          expect(await page.evaluate('window.__transactions.applyCalls')).toBe(1);
          await page.clock.runFor(RUNTIME_APPLY_POLL_INTERVAL_MS);
          await page.waitForFunction('window.__transactions.runtimeStatusReads === 2');
          expect(await page.evaluate('window.__transactions.runtimeTransactions')).toEqual(['original', 'original']);
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
        scenarioCompleted = true;
      } finally {
        // Resume even on assertion failure; the shared afterEach then preserves
        // failure evidence and closes this isolated browser context.
        try { await page.clock.resume(); } catch {
          if (scenarioCompleted) throw new Error('fixture-clock-resume-failed');
          // Keep the original failure primary; afterEach still owns context close.
          console.error('[fixture] additional clock resume failure');
        }
      }
      await page.close();
    });
    it('ignores a late successful read after an attributed stop'   , async () => {
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&generation=100');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&generation=100');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&generation=100');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&generation=100');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode);
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&listener=deferred');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&listener=failed');
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
    const page = await browser.newPage({ locale: 'en-US' });
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
    await openFixture(page, origin + '/__transactions?mode=mobile-ts&ownedStop=' + outcome + '&intent=' + intent + '&preflightHold=' + preflightHold + (selectedServerId === null ? '&nullSelection' : ''));
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=selection');
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
      const page = await browser.newPage({ locale: 'en-US' });
      await openFixture(page, origin + '/__transactions?mode=' + mode + '&enabled=' + enabled);
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=mobile-nodes');
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=resources');
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=connections');
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=' + mode);
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=' + mode + '&direct=1');
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=' + mode);
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
    const page = await browser.newPage({ locale: 'en-US' });
    await openFixture(page, origin + '/__transactions?mode=' + mode);
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

// Presentation fixtures use real consumers and real CSS; only backend receipts are synthetic.
const feedbackEntry = `
import React, { useLayoutEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import { startPrivacyModeSync } from '/src/lib/privacy-mode-sync';
import { DEMO_CONFIG } from '/harness-fixture';
import i18n, { i18nReady } from '/src/i18n';
const q = new URLSearchParams(location.search);
const mobile = q.get('mode') === 'mobile';
const helperBridge = q.has('helper-bridge');
const recovery = q.get('recovery');
if (recovery === 'seed') window.__POLARIS_TRAY_SCREEN__ = 'helper-recovery';
if (mobile) {
  await import('/src/styles/tokens.resolved.css');
  await import('/src/mobile/theme.css');
  await import('/src/mobile/mobile.css');
  await import('/src/mobile/forms/forms.css');
  await import('/src/mobile/settings/settings.css');
  await import('/src/mobile/redesign.css');
} else await import('/src/styles/index.css');
await i18nReady;
// Do not call changeLanguage without loading its bundle: production initialization
// has already selected the explicit navigator locale in this isolated context.
document.documentElement.dataset.theme = q.get('theme') || 'light';
document.documentElement.lang = i18n.language;
document.documentElement.dir = i18n.dir();
document.documentElement.dataset.os = helperBridge && q.has('os') ? q.get('os') : 'mac';
if (q.get('scale') === '2') document.documentElement.style.setProperty('--font-scale', '2');
const state = window.__feedback = { installs: 0, checks: 0, navigation: [], release: null, submitted: 0, closed: 0,
  configReads: 0, privacyReads: 0, privacyApplied: false, cleanExitReads: 0, pendingAcks: 0, trayReads: 0, unexpected: [] };
const config = structuredClone(DEMO_CONFIG);
const status = { supported: true, installed: false, ready: false, needsRepair: false, upgradeable: false, backgroundDisabled: false, loaded: null, pathMismatch: false, installedSingboxPath: null };
if (helperBridge) {
  // Only the native transport is synthetic: real React hooks, SettingsPage/useConfig,
  // helperApi, ipc-client's single unwrap and the actual confirmation dialog stay mounted.
  // This fixture neither installs nor talks to any native helper.
  const repair = { ...status, installed: true, needsRepair: true, version: 1, expectedProtocolVersion: 2,
    expectedBuildId: 'fixture-current-app', selectedCore: { id: 'a'.repeat(64), version: 'fixture-core',
      providerFingerprint: 'b'.repeat(64), state: 'unavailable' } };
  let observedStatus = repair;
  let pendingScreen = recovery === 'seed' || recovery === 'pending' ? 'helper-recovery' : null;
  state.revealTargets = [];
  state.installModes = [];
  state.forcedChecks = 0;
  mockIPC(async (command, payload) => {
    switch (command) {
      case 'plugin:notification|is_permission_granted': return false; // explicit synthetic denied permission, never request or notify
      case 'config_get': state.configReads++; return { success: true, data: config };
      case 'config_get_privacy_mode': state.privacyReads++; return { success: true, data: q.has('privacy-locked') };
      case 'app_take_clean_exit_flag': state.cleanExitReads++; return { success: true, data: false };
      case 'config_set_staged_pending':
        if (payload?.pending !== false || !Array.isArray(payload?.nodeIds) || payload.nodeIds.length !== 0) throw new Error('Unexpected nonempty staged fixture mutation');
        state.pendingAcks++; return { success: true };
      case 'app_startup_config_flags': return { success: true, data: { hardwareAcceleration: true, windowEffects: true, rememberWindowSize: true } };
      case 'tray_take_pending_screen': {
        state.trayReads++;
        const screen = pendingScreen; pendingScreen = null;
        if (recovery === 'late-pending' && state.trayReads === 1) return new Promise(resolve => {
          state.releasePending = () => resolve({ success: true, data: 'helper-recovery' });
        });
        return { success: true, data: screen };
      }
      case 'tray_show_main': {
        if (payload?.screen !== 'helper-recovery') throw new Error('Unexpected native reveal target');
        state.revealTargets.push(payload.screen); pendingScreen = payload.screen;
        // Real event API + raw IPC mock; this is not an actual separate native webview.
        const { emit } = await import('@tauri-apps/api/event');
        await emit('event:trayOpenScreen', payload.screen);
        return { success: true };
      }
      case 'helper_get_status':
        state.checks++;
        if (payload?.force === true) state.forcedChecks++;
        if (state.holdNextStatus && payload?.force !== true) {
          state.holdNextStatus = false;
          return new Promise(resolve => { state.releaseOldStatus = () => resolve({ success: true, data: repair }); });
        }
        return { success: true, data: observedStatus };
      case 'helper_install':
        state.installs++;
        state.installModes.push(payload?.mode ?? 'ordinary');
        return new Promise(resolve => {
          state.release = (errorCode = 'upgradeDeferred') => {
            if (errorCode === null) observedStatus = { ...repair, ready: true, needsRepair: false, version: 2, helperBuildId: 'fixture-current-app', selectedCore: { ...repair.selectedCore, state: 'present' } };
            resolve({ success: true, data: { success: errorCode === null, errorCode, diagnostic: 'synthetic bounded helper rejection', status: observedStatus } });
          };
        });
      default: state.unexpected.push(command); throw new Error('Unexpected helper transport fixture command: ' + command);
    }
  }, { shouldMockEvents: true });
}
// Install the raw transport before importing production modules with startup effects.
// This is the same fail-closed privacy synchronizer that App mounts, not a forced unlock.
const [{ default: SettingsHelper }, { default: SettingsPage }, { DialogHost }, { useDialogStore }, { useAppStore }, { FormSheet }, { SettingsRow, SettingsGroup }, { WriteErrorsContext }, { Toaster }, { MobileToaster }, { MobileShell }, { helperApi, configApi }, { toast }, { useNavStore }] = await Promise.all([
  import('/src/components/screens/settings/SettingsHelper'),
  import('/src/components/screens/settings/SettingsPage'),
  import('/src/components/dialogs/DialogHost'),
  import('/src/components/dialogs/dialog-store'),
  import('/src/store/app-store'),
  import('/src/mobile/forms/FormSheet'),
  import('/src/mobile/settings/SettingsChrome'),
  import('/src/mobile/settings/write-feedback'),
  import('/src/components/layout/Toaster'),
  import('/src/mobile/MobileToaster'),
  import('/src/mobile/MobileShell'),
  import('/src/ipc/api-client'),
  import('/src/lib/error-handler'),
  import('/src/store/nav-store'),
]);
if (helperBridge) {
  useAppStore.setState({ config });
  if (!recovery) useNavStore.getState().enterSettings('helper');
  const { revealHelperRecovery } = await import('/src/domain/helper-recovery');
  const { handleProxyErrorEvent } = await import('/src/domain/proxy-error-routing');
  state.revealRecovery = revealHelperRecovery;
  state.errorEvent = code => handleProxyErrorEvent({ errorCode: code }, { t: key => i18n.t(key), refreshProxyStatus: async () => {} });
  state.scope = () => ({ scope: useNavStore.getState().scope, settings: useNavStore.getState().settingsScreen });
  state.setPrivacy = locked => useAppStore.getState().setPrivacyMode(locked);
  state.leave = () => useNavStore.getState().navigate('logs');
  state.returnToHelper = () => useNavStore.getState().enterSettings('helper');
  state.captureConfirm = () => { const entry = useDialogStore.getState().stack.at(-1); if (entry?.kind !== 'confirm') throw new Error('Missing actual confirmation'); state.savedConfirm = entry.payload.onConfirm; };
} else {
  helperApi.getStatus = async () => { state.checks++; return status; };
  helperApi.onUpgradeable = () => () => {};
  helperApi.install = () => { state.installs++; return new Promise(resolve => { state.release = () => resolve({ success: false, errorCode: 'installerFailed', status }); }); };
  useNavStore.setState({ navigate: screen => { state.navigation.push(screen); } });
}
function HelperBridgeRoute() {
  React.useEffect(() => startPrivacyModeSync(configApi, locked => {
    useAppStore.getState().setPrivacyMode(locked);
    if (state.privacyReads > 0) state.privacyApplied = true;
  }), []);
  const scope = useNavStore(s => s.scope);
  // Same settings parent is genuinely unmounted when navigation changes scope.
  // The logs body itself is irrelevant and deliberately not fabricated as real log output.
  return <>{scope === 'settings' ? <SettingsPage /> : <div data-fixture-logs>synthetic logs destination</div>}<DialogHost /><Toaster /></>;
}
const synthetic = i18n.language === 'fa' ? 'این پیام آزمایشی فقط برای بررسی نمایش است و هیچ دادهٔ کاربری ندارد. ' : i18n.language === 'en-US' ? 'Synthetic layout detail with no user data or credentials. ' : '这是仅用于界面排版的合成错误说明，不含网址、凭据或用户数据。';
const long = i18n.t('feedback.followupIncomplete') + ' ' + synthetic.repeat(16);
state.connectionTitle = i18n.t('feedback.connectionFailed');
state.connectionError = () => toast.error(i18n.t('feedback.connectionFailed'), long, { sticky: true, dismiss: { label: '关闭提示' }, actions: [{ label: synthetic.repeat(2), onClick: () => state.submitted++ }] });
function FeedbackReady() {
  useLayoutEffect(() => {
    state.bootstrap = { language: i18n.language, resolvedLanguage: i18n.resolvedLanguage, direction: document.documentElement.dir, resourceLoaded: i18n.hasResourceBundle(i18n.language, 'translation'), sample: i18n.t('feedback.subscriptionSaved') };
    state.ready = true;
    return () => { state.ready = false; };
  }, []);
  return null;
}
createRoot(document.getElementById('root')).render(<><FeedbackReady />{helperBridge ? <HelperBridgeRoute /> : mobile ?
  <MobileShell active="settings" onSelect={() => {}} toastHost={<MobileToaster />}><FormSheet title="订阅编辑" closeLabel="关闭表单" cancelLabel="取消" submitLabel="保存" onRequestClose={() => state.closed++} onSubmit={() => state.submitted++}
    notice={{ tone: 'err', title: i18n.t('feedback.subscriptionSaved'), text: long }}>
    <WriteErrorsContext.Provider value={{fixture:"保存失败"}}><SettingsGroup><SettingsRow id="fixture" label="设置校验" problem="参数无效" /></SettingsGroup></WriteErrorsContext.Provider>
    <div className="m-form-row"><label className="m-form-label" htmlFor="fixture-url">订阅地址</label><input className="m-form-input mono" id="fixture-url" defaultValue="https://example.invalid/synthetic" /></div>
    <p>{'只读合成表单内容。'.repeat(80)}</p>
  </FormSheet></MobileShell> : <div className="main-scroll" style={{ height: '100vh' }}><SettingsHelper /><Toaster /></div>}</>);
`;

describe.runIf(process.env.POLARIS_BROWSER_TESTS === '1')('feedback presentation with production consumers and CSS', () => {
  let server: ViteDevServer;
  let browser: Browser;
  let origin: string;
  beforeAll(async () => {
    server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
      name: 'feedback-presentation-fixture',
      resolveId(id) { if (id === '/feedback-presentation-fixture.tsx') return id; },
      load(id) { if (id === '/feedback-presentation-fixture.tsx') return feedbackEntry; },
      configureServer(vite) { vite.middlewares.use('/__feedback', async (_req, res) => {
        res.setHeader('Content-Type', 'text/html');
        res.end(await vite.transformIndexHtml('/__feedback', '<html><div id="root"></div><script type="module" src="/feedback-presentation-fixture.tsx"></script></html>'));
      }); },
    }] });
    await server.listen();
    const address = server.httpServer!.address();
    if (!address || typeof address === 'string') throw new Error('No fixture listener');
    origin = `http://127.0.0.1:${address.port}`;
    browser = await chromium.launch({ chromiumSandbox: true, headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  }, 30_000);
  afterEach(async ({ task }) => {
    const failed = task.result?.state === 'fail';
    const cleanup = await closeFixturePages(browser, failed, task.name);
    enforceFixtureCleanup(cleanup, failed, task.meta);
  });
  afterAll(async () => { await browser?.close(); await server?.close(); });

  async function evidence(page: import('@playwright/test').Page, scenario: string, selector: string) {
    const { mkdirSync, writeFileSync } = await import('node:fs');
    const output = path.join(root, 'test-results', 'feedback');
    mkdirSync(output, { recursive: true });
    const stability = await page.locator(selector).evaluate(async el => {
      await document.fonts.ready;
      const related = [el, ...Array.from(el.querySelectorAll('*'))];
      for (let ancestor = el.parentElement; ancestor; ancestor = ancestor.parentElement) related.push(ancestor);
      const animations = [...new Set(related.flatMap(node => node.getAnimations()))]
        .filter(animation => Number.isFinite(animation.effect?.getComputedTiming().iterations));
      await Promise.all(animations.map(animation => animation.finished));
      const sample = () => { const rect = el.getBoundingClientRect(); return [rect.x, rect.y, rect.width, rect.height]; };
      await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
      const before = sample();
      await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
      const after = sample();
      return { stable: before.every((value, index) => Math.abs(value - after[index]!) < 0.5), opacity: getComputedStyle(el).opacity };
    });
    expect(stability.stable).toBe(true);
    expect(stability.opacity).toBe('1');
    const facts = await page.locator(selector).evaluate(el => {
      const style = getComputedStyle(el), rect = el.getBoundingClientRect();
      const typography = Array.from(el.querySelectorAll('strong,p,.toast-msg,.toast-desc')).map(node => { const value = getComputedStyle(node); return { tag: node.tagName, fontSize: value.fontSize, fontWeight: value.fontWeight, lineHeight: value.lineHeight, color: value.color }; });
      return { fontSize: style.fontSize, fontWeight: style.fontWeight, lineHeight: style.lineHeight,
        color: style.color, background: style.backgroundColor, width: rect.width, height: rect.height,
        scrollWidth: el.scrollWidth, clientWidth: el.clientWidth, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight, typography };
    });
    await page.screenshot({ path: path.join(output, scenario + '.png'), fullPage: true });
    writeFileSync(path.join(output, scenario + '.json'), JSON.stringify({ schema: 'polaris-feedback-visual-fixture-v1',
      sourceSha: process.env.GITHUB_SHA ?? 'local-unverified', scenario, viewport: page.viewportSize(),
      presentation: await page.evaluate(() => ({ locale: document.documentElement.lang, direction: document.documentElement.dir, theme: document.documentElement.dataset.theme, rootFontSize: getComputedStyle(document.documentElement).fontSize })),
      fixtureOnly: true, stability, facts, visualVerdict: 'pending-human-review' }, null, 2));
  }

  // Incident regression: a typed deferral must replace the original repair action,
  // not leave the user repeating ordinary installs while recovery is elsewhere above it.
  it.each(['cancel', 'escape', 'close', 'scrim'])('Helper recovery %s dismisses the explanation without navigation or install', async dismiss => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&recovery=event');
    await page.evaluate('window.__feedback.revealRecovery()');
    const dialog = page.getByRole('dialog', { name: zhCN.helper.connectionRecoveryTitle });
    await dialog.waitFor({ state: 'visible' });
    await page.evaluate('window.__feedback.revealRecovery()');
    expect(await page.getByRole('dialog').count()).toBe(1);
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    if (dismiss === 'escape') await page.keyboard.press('Escape');
    else if (dismiss === 'scrim') await page.mouse.click(1, 1);
    else await dialog.getByRole('button', { name: dismiss === 'cancel' ? zhCN.common.cancel : zhCN.common.close, exact: true }).click();
    await dialog.waitFor({ state: 'detached' });
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual([]);
    await page.close();
  });

  it.each(['seed', 'pending'])('Helper recovery cold %s opens translated explanation before any settings navigation', async delivery => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&recovery=' + delivery);
    const dialog = page.getByRole('dialog', { name: zhCN.helper.connectionRecoveryTitle });
    await dialog.waitFor({ state: 'visible' });
    expect(await dialog.textContent()).toContain(zhCN.helper.connectionRecoveryMessage);
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    await dialog.getByRole('button', { name: zhCN.helper.connectionRecoverySettings, exact: true }).click();
    await page.locator('[data-s="needs-repair"]').waitFor();
    expect(await page.evaluate('window.__feedback.scope()')).toEqual({ scope: 'settings', settings: 'helper' });
    expect(await page.evaluate('window.__feedback.installModes')).toEqual([]);
    await page.close();
  });

  it('Helper recovery event then cancel ignores an older pending drain reply', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&recovery=late-pending');
    await page.evaluate('window.__feedback.revealRecovery()');
    const dialog = page.getByRole('dialog', { name: zhCN.helper.connectionRecoveryTitle });
    await dialog.getByRole('button', { name: zhCN.common.cancel, exact: true }).click();
    await page.evaluate(async () => { (window as any).__feedback.releasePending(); await new Promise(resolve => requestAnimationFrame(() => resolve(null))); });
    await dialog.waitFor({ state: 'detached' });
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual([]);
    await page.close();
  });

  it.each(['HELPER_CORE_MISMATCH', 'HELPER_CORE_ABSENT', 'HELPER_CORE_INVALID'])('Helper recovery generic %s event shares the main prompt owner with await delivery', async code => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&recovery=event');
    await page.evaluate(code => (window as any).__feedback.errorEvent(code), code);
    const dialog = page.getByRole('dialog', { name: zhCN.helper.connectionRecoveryTitle });
    await dialog.waitFor({ state: 'visible' });
    await page.evaluate('window.__feedback.revealRecovery()');
    expect(await page.getByRole('dialog').count()).toBe(1);
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual([]);
    await page.close();
  });

  it('Helper recovery privacy hides the prompt and only unlock makes the choice available', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&recovery=seed&privacy-locked');
    const dialog = page.locator('dialog');
    await dialog.waitFor({ state: 'attached' });
    expect(await dialog.isVisible()).toBe(false);
    expect(await dialog.getAttribute('aria-hidden')).toBe('true');
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual([]);
    // Synthetic external unlock, through the same actual setter used by privacy hydration.
    await page.evaluate('window.__feedback.setPrivacy(false)');
    await dialog.waitFor({ state: 'visible' });
    await dialog.getByRole('button', { name: zhCN.common.cancel, exact: true }).click();
    expect(await page.evaluate('window.__feedback.scope().scope')).toBe('main');
    await page.close();
  });

  it('Helper raw IPC deferral reaches real SettingsPage and replaces the repair action in place', async () => {
    const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 640, height: 480 } });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    const repair = page.locator('[data-s="needs-repair"]');
    await repair.getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('window.__feedback.installs === 1 && typeof window.__feedback.release === "function"');
    await page.evaluate('window.__feedback.release()');
    await page.locator('[data-testid="helper-install-failure"]').waitFor();
    // The parent and typed envelope must both be real before asserting the main operation.
    expect(await page.locator('[data-testid="helper-selected-core"]').textContent()).toContain('fixture-core');
    const primary = repair.getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true });
    await primary.waitFor({ state: 'visible' });
    expect(await repair.getByRole('button', { name: zhCN.helper.repairAction, exact: true }).count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await primary.click();
    const dialog = page.getByRole('dialog');
    await dialog.waitFor();
    expect(await dialog.textContent()).toContain(zhCN.helper.stopAndUpgradeWarning);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.evaluate('window.__feedback.captureConfirm()');
    await dialog.getByRole('button', { name: zhCN.common.cancel, exact: true }).click();
    await page.evaluate('window.__feedback.savedConfirm()');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.getByRole('button', { name: zhCN.helper.recheck, exact: true }).click();
    await page.waitForFunction('window.__feedback.forcedChecks >= 2');
    await primary.click();
    await page.evaluate('window.__feedback.captureConfirm()');
    await dialog.getByRole('button', { name: zhCN.helper.stopAndUpgradeConfirm, exact: true }).click();
    await page.evaluate('window.__feedback.savedConfirm(); window.__feedback.savedConfirm()');
    await page.waitForFunction('window.__feedback.installs === 2');
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary', 'stopAndUpgrade']);
    await page.evaluate('window.__feedback.release("cancelled")');
    await page.waitForFunction(copy => document.querySelector('[data-testid="helper-install-failure"]')?.textContent?.includes(copy), zhCN.helper.actionError.cancelled);
    expect(await page.getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary', 'stopAndUpgrade']);
    await page.close();
  });

  it.each(['after-reply', 'before-reply'])('Helper deferral survives real settings unmount %s without re-install or automatic stop', async timing => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('window.__feedback.installs === 1 && typeof window.__feedback.release === "function"');
    if (timing === 'before-reply') {
      await page.evaluate('window.__feedback.leave()');
      await page.locator('[data-fixture-logs]').waitFor();
    }
    await page.evaluate('window.__feedback.release()');
    if (timing === 'after-reply') {
      await page.locator('[data-testid="helper-install-failure"]').waitFor();
      await page.getByRole('button', { name: zhCN.helper.viewLogsAction, exact: true }).click();
      await page.locator('[data-fixture-logs]').waitFor();
    }
    await page.evaluate('window.__feedback.returnToHelper()');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).waitFor({ state: 'visible' });
    expect(await page.getByRole('dialog').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  it('Helper real parent remount retains the in-flight lock and clears feedback only after success', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    const primary = page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true });
    await primary.click();
    await page.waitForFunction('window.__feedback.installs === 1');
    await page.evaluate('window.__feedback.leave()');
    await page.locator('[data-fixture-logs]').waitFor();
    await page.evaluate('window.__feedback.returnToHelper()');
    await primary.waitFor();
    expect(await primary.isDisabled()).toBe(true);
    await primary.evaluate(element => { (element as HTMLButtonElement).click(); (element as HTMLButtonElement).click(); });
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.evaluate('window.__feedback.release(null)');
    await page.locator('[data-s="installed"]').waitFor();
    expect(await page.evaluate('window.__feedback.forcedChecks')).toBeGreaterThanOrEqual(1);
    expect(await page.locator('[data-testid="helper-install-failure"]').count()).toBe(0);
    expect(await page.getByRole('dialog').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  it('Helper terminal refresh wins over a delayed pre-install status read in the remounted parent', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('window.__feedback.installs === 1');
    await page.evaluate('window.__feedback.leave()');
    await page.locator('[data-fixture-logs]').waitFor();
    await page.evaluate('window.__feedback.holdNextStatus = true; window.__feedback.returnToHelper()');
    await page.waitForFunction('typeof window.__feedback.releaseOldStatus === "function"');
    await page.evaluate('window.__feedback.release(null)');
    await page.locator('[data-s="installed"]').waitFor();
    await page.evaluate('window.__feedback.releaseOldStatus()');
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
    expect(await page.locator('[data-s="installed"]').count()).toBe(1);
    expect(await page.locator('[data-s="needs-repair"]').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  it('Helper real settings unmount discards confirmation but retains typed recovery presentation', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('typeof window.__feedback.release === "function"');
    await page.evaluate('window.__feedback.release()');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).click();
    await page.getByRole('dialog').waitFor();
    await page.evaluate('window.__feedback.captureConfirm(); window.__feedback.leave()');
    await page.locator('[data-fixture-logs]').waitFor();
    await page.evaluate('window.__feedback.savedConfirm(); window.__feedback.returnToHelper()');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).waitFor();
    expect(await page.getByRole('dialog').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  it.each(['win', 'lin', 'unknown'])('Helper raw typed deferral on %s cannot select migration', async platform => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&os=' + platform);
    const primary = page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true });
    await primary.click();
    await page.waitForFunction('typeof window.__feedback.release === "function"');
    await page.evaluate('window.__feedback.release()');
    await page.locator('[data-testid="helper-install-failure"]').waitFor();
    await primary.waitFor();
    expect(await page.getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).count()).toBe(0);
    expect(await page.getByRole('dialog').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  it('Helper raw locked privacy snapshot keeps the real confirmation hidden and sends no migration', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge&privacy-locked');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('typeof window.__feedback.release === "function"');
    await page.evaluate('window.__feedback.release()');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).click();
    await page.locator('dialog').waitFor({ state: 'attached' });
    expect(await page.locator('dialog').getAttribute('aria-hidden')).toBe('true');
    expect(await page.locator('dialog').isVisible()).toBe(false);
    expect(await page.locator('dialog[open]').count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    expect(await page.evaluate('window.__feedback.unexpected')).toEqual([]);
    await page.close();
  });

  it('Helper raw ordinary installer failure does not authorize migration', async () => {
    const page = await browser.newPage({ locale: 'zh-CN' });
    await openFixture(page, origin + '/__feedback?helper-bridge');
    await page.locator('[data-s="needs-repair"]').getByRole('button', { name: zhCN.helper.repairAction, exact: true }).click();
    await page.waitForFunction('typeof window.__feedback.release === "function"');
    await page.evaluate('window.__feedback.release("installerFailed")');
    await page.locator('[data-testid="helper-install-failure"]').waitFor();
    expect(await page.getByRole('button', { name: zhCN.helper.stopAndUpgradeAction, exact: true }).count()).toBe(0);
    expect(await page.evaluate('window.__feedback.installModes')).toEqual(['ordinary']);
    await page.close();
  });

  for (const width of [1280, 640]) for (const theme of ['light', 'dark']) {
    it(`Helper failure hierarchy and original actions ${width}/${theme}`, async () => {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 800 } });
      await openFixture(page, origin + '/__feedback?theme=' + theme);
      const install = page.getByRole('button', { name: '安装提权助手（需授权）', exact: true });
      await install.click();
      await page.waitForFunction('typeof window.__feedback.release === "function"');
      expect(await page.evaluate('window.__feedback.installs')).toBe(1);
      await page.evaluate('window.__feedback.release()');
      const notice = page.locator('[data-testid="helper-install-failure"] [data-feedback-notice]');
      await notice.waitFor();
      await evidence(page, `helper-${width}-${theme}`, '[data-feedback-notice]');
      expect(await notice.getAttribute('role')).toBe('region');
      expect(await notice.locator('.feedback-notice-detail').textContent()).toContain('安装命令执行失败');
      expect(await notice.locator('.feedback-notice-hint').textContent()).not.toContain('查看日志');
      const title = await notice.locator('strong').evaluate(el => parseFloat(getComputedStyle(el).fontSize));
      const body = await notice.locator('.feedback-notice-detail').evaluate(el => parseFloat(getComputedStyle(el).fontSize));
      expect(title).toBeGreaterThan(body);
      await page.getByRole('button', { name: '重新检测', exact: true }).click();
      expect(await page.evaluate('window.__feedback.installs')).toBe(1);
      const logs = notice.getByRole('button', { name: '查看日志', exact: true });
      await logs.focus();
      expect(await logs.evaluate(el => el === document.activeElement)).toBe(true);
      await evidence(page, `helper-focus-${width}-${theme}`, '[data-feedback-notice]');
      await page.keyboard.press('Enter');
      expect(await page.evaluate('window.__feedback.navigation')).toEqual(['logs']);
      expect(await notice.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
      await page.close();
    });
  }

  for (const width of [390, 320]) for (const scale of [1, 2]) {
    it(`long mobile FormSheet notice retains focus, scrolling and footer ${width}/${scale}`, async () => {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 568 } });
      await openFixture(page, origin + `/__feedback?mode=mobile&theme=${width === 390 ? 'light' : 'dark'}&scale=${scale}`);
      const notice = page.locator('.m-form-notice-detailed');
      await notice.waitFor();
      await evidence(page, `mobile-notice-${width}-font${scale}`, '.m-form-notice-detailed');
      expect(await notice.getAttribute('role')).toBe('status');
      expect(await notice.locator('strong').textContent()).toBe('订阅已保存');
      expect(await page.locator('[data-write-error="fixture"]').getAttribute('role')).toBe('status');
      expect(await page.locator('[data-setting="fixture"]').textContent()).toContain('参数无效');
      expect(await page.locator('[data-setting="fixture"]').textContent()).toContain('保存失败');
      const geometry = await page.evaluate(() => {
        const row = document.querySelector('[data-setting="fixture"]')!.getBoundingClientRect();
        const error = document.querySelector('[data-write-error="fixture"]')!.getBoundingClientRect();
        const field = document.querySelector('label[for="fixture-url"]')!.getBoundingClientRect();
        const input = document.querySelector('#fixture-url')!.getBoundingClientRect();
        return { rowBottom: row.bottom, errorBottom: error.bottom, fieldTop: field.top, fieldBottom: field.bottom, inputTop: input.top,
          bodyFits: document.documentElement.scrollWidth <= innerWidth };
      });
      expect(geometry.errorBottom).toBeLessThanOrEqual(geometry.rowBottom + 1);
      expect(geometry.rowBottom).toBeLessThanOrEqual(geometry.fieldTop);
      expect(geometry.fieldBottom).toBeLessThanOrEqual(geometry.inputTop);
      expect(geometry.bodyFits).toBe(true);
      for (const label of await page.locator('.m-nav-label').all()) {
        expect(await label.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
      }
      expect(await notice.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
      expect(await notice.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
      await notice.focus();
      await page.keyboard.press('End');
      await page.waitForFunction('document.querySelector(".m-form-notice-detailed").scrollTop > 0');
      expect(await notice.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
      const save = page.getByRole('button', { name: '保存', exact: true });
      const box = await save.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.y + box!.height).toBeLessThanOrEqual(569);
      expect(box!.height).toBeGreaterThanOrEqual(48);
      await page.getByLabel('订阅地址').focus();
      await page.keyboard.press('Tab');
      expect(await page.evaluate('document.activeElement?.tagName')).not.toBe('BODY');
      await evidence(page, `mobile-notice-scrolled-${width}-font${scale}`, '.m-form-notice-detailed');
      await save.click();
      expect(await page.evaluate('window.__feedback.submitted')).toBe(1);
      await page.close();
    });
  }

  it.each(['zh-CN', 'en-US', 'fa'])('the real Toaster keeps long %s detail and dismissal within the viewport', async lang => {
    const page = await browser.newPage({ locale: lang, viewport: { width: 640, height: 800 } });
    await openFixture(page, origin + '/__feedback?theme=light&lang=' + lang);
    await page.locator('[data-s="none"]').waitFor();
    await page.evaluate('window.__feedback.connectionError()');
    await page.locator('.toast.show').waitFor();
    await evidence(page, `connection-toast-long-${lang}-640-light`, '.toast.show');
    expect(await page.locator('.toast-msg').textContent()).toBe(await page.evaluate('window.__feedback.connectionTitle'));
    expect((await page.locator('.toast-desc').textContent())!.length).toBeGreaterThan(300);
    const toastBox = await page.locator('.toast.show').boundingBox();
    expect(toastBox).not.toBeNull();
    expect(toastBox!.y).toBeGreaterThanOrEqual(0);
    expect(toastBox!.y + toastBox!.height).toBeLessThanOrEqual(801);
    expect(toastBox!.x).toBeGreaterThanOrEqual(0);
    expect(toastBox!.x + toastBox!.width).toBeLessThanOrEqual(641);
    const detail = page.locator('.toast-desc');
    expect(await detail.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
    await detail.focus();
    await page.keyboard.press('End');
    await page.waitForFunction('document.querySelector(".toast-desc").scrollTop > 0');
    const close = page.getByRole('button', { name: '关闭提示', exact: true });
    await close.focus();
    expect(await close.evaluate(el => el === document.activeElement)).toBe(true);
    await page.keyboard.press('Enter');
    await page.locator('.toast').waitFor({ state: 'detached' });
    expect(await page.evaluate('window.__feedback.installs')).toBe(0);
    await page.close();
  });
  it('the independent MobileToaster keeps long actions and dismissal reachable above a FormSheet', async () => {
    const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 320, height: 568 } });
    await openFixture(page, origin + '/__feedback?mode=mobile&theme=dark');
    await page.locator('.m-form-notice-detailed').waitFor();
    await page.evaluate('window.__feedback.connectionError()');
    const toast = page.locator('.m-toast-dismissible');
    await toast.waitFor();
    await evidence(page, 'mobile-toast-long-actions-320-dark', '.m-toast-dismissible');
    const box = await toast.boundingBox();
    expect(box!.y).toBeGreaterThanOrEqual(0);
    expect(box!.y + box!.height).toBeLessThanOrEqual(569);
    expect(await toast.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    const columns = await toast.evaluate(el => {
      const body = el.querySelector('.m-toast-body')!.getBoundingClientRect();
      const actions = el.querySelector('.m-toast-actions')!.getBoundingClientRect();
      return { bodyWidth: body.width, toastWidth: el.clientWidth, bodyBottom: body.bottom, actionsTop: actions.top };
    });
    expect(columns.bodyWidth).toBeGreaterThan(columns.toastWidth * 0.8);
    expect(columns.actionsTop).toBeGreaterThanOrEqual(columns.bodyBottom);

    const detail = toast.locator('.m-toast-desc');
    expect(await detail.evaluate(el => getComputedStyle(el).pointerEvents)).toBe('auto');
    await detail.hover();
    await page.mouse.wheel(0, 250);
    await page.waitForFunction('document.querySelector(".m-toast-desc").scrollTop > 0');
    await detail.evaluate(el => { el.scrollTop = 0; });
    await detail.focus();
    await page.keyboard.press('End');
    await page.waitForFunction('document.querySelector(".m-toast-desc").scrollTop > 0');
    const close = toast.getByRole('button', { name: '关闭提示', exact: true });
    const closeBox = await close.boundingBox();
    expect(closeBox!.width).toBeGreaterThanOrEqual(48);
    expect(closeBox!.height).toBeGreaterThanOrEqual(48);
    await close.focus();
    expect(await close.evaluate(el => el === document.activeElement)).toBe(true);
    await page.keyboard.press('Enter');
    await toast.waitFor({ state: 'detached' });
    expect(await page.evaluate('window.__feedback.submitted')).toBe(0);
    expect(await page.evaluate('window.__feedback.installs')).toBe(0);
    await page.close();
  });

});

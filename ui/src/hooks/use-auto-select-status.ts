import { useCallback, useEffect, useSyncExternalStore } from 'react';
import type { NativeAutoStatus } from '@/contracts/auto-select';
import type { UserConfig } from '@/contracts/types';
import { api } from '@/ipc';
import { useAppStore } from '@/store/app-store';

export interface AutoSelectStatusState { report: NativeAutoStatus | null; loading: boolean; failed: boolean }
const empty: AutoSelectStatusState = { report: null, loading: false, failed: false };
let snapshot = empty;
let context = '';
let clients = 0;
let session = 0;
let revision = 0;
let request = 0;
let pendingSession: number | null = null;
let unsubscribe: (() => void) | null = null;
let ready: Promise<void> | null = null;
const listeners = new Set<() => void>();
const publish = (value: AutoSelectStatusState) => {
  snapshot = value;
  const status = useAppStore.getState().proxyStatus;
  if (status?.running && value.report && status.mainGeneration === value.report.generation && status.actualServerId !== value.report.actualServerId) {
    useAppStore.getState().setProxyStatus({ ...status, actualServerId: value.report.actualServerId });
  }
  listeners.forEach(listener => listener());
};
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const getSnapshot = () => snapshot;
const contextKeys = new WeakMap<UserConfig, string>();
function configContext(config: UserConfig): string {
  const cached = contextKeys.get(config);
  if (cached !== undefined) return cached;
  const key = JSON.stringify([config.autoSelectEnabled, config.selectionIntent, config.autoSelectPins,
    config.servers, config.subscriptions, config.trafficRules, config.policyRules, config.customRules,
    config.appRules, config.speedTestUrl]);
  contextKeys.set(config, key);
  return key;
}

function detach(off: () => void) {
  try { void Promise.resolve(off()).catch(error => console.error('[nativeAuto] unsubscribe failed', error)); }
  catch (error) { console.error('[nativeAuto] unsubscribe failed', error); }
}
function start() {
  const token = ++session;
  ready = Promise.resolve().then(() => api.server.onAutoSelectStatusReady(report => {
    if (session !== token) return;
    revision++;
    publish({ report, loading: false, failed: false });
  })).then(off => {
    if (session !== token) { detach(off); return; }
    unsubscribe = off;
  });
  void ready.catch(() => { if (session === token) { ready = null; publish({ report: null, loading: false, failed: true }); } });
}
async function load() {
  if (!clients || pendingSession === session) return;
  if (!ready) start();
  const token = session;
  pendingSession = token;
  const id = ++request;
  let before = revision;
  publish({ ...snapshot, loading: true, failed: false });
  try {
    await ready;
    if (session !== token) return;
    before = revision;
    const report = await api.server.autoSelectStatus();
    if (!Array.isArray(report?.nativeAutoGroups)) throw new TypeError('Invalid native Auto status');
    if (session === token && request === id && revision === before) publish({ report, loading: false, failed: false });
  } catch {
    if (session === token && request === id && revision === before) publish({ report: null, loading: false, failed: true });
  } finally {
    if (pendingSession === token) pendingSession = null;
  }
}

function acquire(key: string) {
    clients++;
    if (context !== key || clients === 1) {
      context = key;
      // Invalidate old reads/events even when other mounted cards still hold a ref.
      if (unsubscribe) detach(unsubscribe);
      unsubscribe = null; ready = null; pendingSession = null;
      start();
      publish({ report: null, loading: true, failed: false });
      void load();
    }
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      clients--;
      if (clients === 0) {
        session++;
        if (unsubscribe) detach(unsubscribe); unsubscribe = null; ready = null; context = '';
        publish(empty);
      }
    };
}


/** Shared production source; React only manages consumer lifetimes. */
export const nativeAutoStatusSource = { acquire, refresh: load, subscribe, getSnapshot };

/** One event subscription and snapshot for all cards in a webview; no per-card polling. */
export function useAutoSelectStatus(config: UserConfig | null) {
  const enabled = !!config && (config.autoSelectEnabled === true || !!config.selectionIntent || !!config.autoSelectPins?.length);
  const running = useAppStore(state => state.proxyStatus?.running === true);
  const generation = useAppStore(state => state.proxyStatus?.mainGeneration);
  const key = enabled && config ? `${configContext(config)}:${running}:${generation ?? ''}` : '';
  const value = useSyncExternalStore(nativeAutoStatusSource.subscribe, nativeAutoStatusSource.getSnapshot, () => empty);
  useEffect(() => enabled ? nativeAutoStatusSource.acquire(key) : undefined, [enabled, key]);
  const refresh = useCallback(() => { if (enabled) void nativeAutoStatusSource.refresh(); }, [enabled]);
  return { ...(enabled && context === key ? value : empty), enabled, refresh, runtimeCurrent: running && generation !== undefined && generation === value.report?.generation };
}

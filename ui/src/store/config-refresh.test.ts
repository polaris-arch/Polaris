import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { UserConfig } from '@/contracts/types';

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const native = vi.hoisted(() => ({ get: vi.fn(), ready: vi.fn(), hydrate: vi.fn(), patch: vi.fn(), mutate: vi.fn() }));
vi.mock('../ipc', () => ({ api: { config: {
  get: () => native.get(), onChangedReady: (callback: () => void) => native.ready(callback),
  patch: (patch: Partial<UserConfig>) => native.patch(patch), mutateEntities: (items: unknown) => native.mutate(items),
} } }));
vi.mock('./staged-config-store', () => ({ hydrateStagedConfig: (...args: unknown[]) => native.hydrate(...args),
  flushStagedPendingSync: vi.fn(), useStagedConfigStore: Object.assign(vi.fn(), { getState: () => ({ entries: [] }) }),
}));
const cfg = (value: string) => ({ selectedServerId: value, servers: [], proxyMode: 'smart' } as unknown as UserConfig);
let store: typeof import('./app-store');
let readOrder: typeof import('../lib/config-read-generation');
let listeners: Set<() => void>, reads: ReturnType<typeof deferred<UserConfig>>[], releases: (() => void)[];
const signal = () => listeners.forEach((callback) => callback());
const started = async (count: number) => vi.waitFor(() => expect(reads).toHaveLength(count));
const tick = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const acquire = () => { const release = store.acquireConfigSync({ scope: 'main' }); releases.push(release); return release; };

beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  listeners = new Set(); reads = []; releases = [];
  native.ready.mockImplementation(async (callback: () => void) => { listeners.add(callback); return () => listeners.delete(callback); });
  native.get.mockImplementation(() => { const read = deferred<UserConfig>(); reads.push(read); return read.promise; });
  native.hydrate.mockResolvedValue(undefined);
  store = await import('./app-store');
  readOrder = await import('../lib/config-read-generation');
});
afterEach(() => releases.forEach((release) => release()));

describe('one ready-registered per-WebView feed with fresh tail barriers', () => {
  it.each([0, 1, 4])('root plus %s consumers share registration and one reference', async (count) => {
    const registration = deferred<() => void>();
    native.ready.mockImplementation((callback) => { listeners.add(callback); return registration.promise; });
    acquire();
    const consumers = Array.from({ length: count }, () => { acquire(); return store.refreshConfig(); });
    await tick(); expect(reads).toHaveLength(0); expect(native.ready).toHaveBeenCalledTimes(1);
    registration.resolve(() => listeners.clear()); await started(1);
    const value = cfg('shared'); reads[0].resolve(value);
    for (const result of await Promise.all(consumers)) expect(result.config).toBe(value);
    await vi.waitFor(() => expect(store.useAppStore.getState().config).toBe(value));
    expect(native.get).toHaveBeenCalledTimes(1);
  });

  it('coalesces many signals into one tail, then follows a signal during that tail', async () => {
    acquire(); const waiting = store.refreshConfig(); await started(1);
    signal(); signal(); signal(); reads[0].resolve(cfg('obsolete'));
    await started(2); expect(store.useAppStore.getState().config).toBeNull();
    signal(); reads[1].resolve(cfg('also-obsolete')); await started(3);
    const final = cfg('final'); reads[2].resolve(final);
    expect((await waiting).config).toBe(final); expect(native.get).toHaveBeenCalledTimes(3);
  });

  it('a fresh caller never consumes a GET that began before its barrier', async () => {
    acquire(); const ordinary = store.refreshConfig(); await started(1);
    let finished = false;
    const fresh = store.refreshConfig({ fresh: true }).then((value) => { finished = true; return value; });
    reads[0].resolve(cfg('pre-barrier')); await started(2); expect(finished).toBe(false);
    const final = cfg('post-barrier'); reads[1].resolve(final);
    expect((await fresh).config).toBe(final); expect((await ordinary).config).toBe(final);
  });

  it('two explicit fresh calls have separate lower bounds when a GET starts between them', async () => {
    const first = store.refreshConfig({ fresh: true }); await started(1);
    const second = store.refreshConfig({ fresh: true }); reads[0].resolve(cfg('first'));
    await started(2); reads[1].resolve(cfg('second'));
    expect((await first).config.selectedServerId).toBe('second');
    expect((await second).config.selectedServerId).toBe('second');
  });

  it('only a physical GET calls beginRead; waiting consumers do not invalidate one another', async () => {
    const begin = vi.spyOn(readOrder.configReadGeneration, 'beginRead');
    acquire(); const waiting = Array.from({ length: 8 }, () => store.refreshConfig());
    await started(1); expect(begin).toHaveBeenCalledTimes(1);
    reads[0].resolve(cfg('same')); await Promise.all(waiting); expect(begin).toHaveBeenCalledTimes(1);
  });

  it('an obsolete rejection still tails, while current errors reject without negative caching', async () => {
    acquire(); const old = store.refreshConfig(); await started(1);
    const newer = store.refreshConfig({ fresh: true }); reads[0].reject(Error('old failure'));
    await started(2); reads[1].resolve(cfg('recovered')); await Promise.all([old, newer]);
    const failing = store.refreshConfig().catch((error) => error); await started(3);
    reads[2].reject(Error('current failure')); expect((await failing).message).toBe('current failure');
    expect(store.useAppStore.getState().config?.selectedServerId).toBe('recovered');
    const retry = store.refreshConfig(); await started(4); reads[3].resolve(cfg('retry')); await retry;
  });

  it('idle ordinary requests revalidate instead of caching a completed promise', async () => {
    const first = store.refreshConfig(); await started(1); reads[0].resolve(cfg('one')); await first;
    const next = store.refreshConfig(); await started(2); reads[1].resolve(cfg('two'));
    expect((await next).config.selectedServerId).toBe('two');
  });

  it('single consumer cleanup leaves other leases and their waiting read intact', async () => {
    const first = acquire(); acquire(); const waiting = store.refreshConfig(); await started(1);
    first(); first(); expect(listeners.size).toBe(1); reads[0].resolve(cfg('live')); await waiting;
    expect(store.useAppStore.getState().config?.selectedServerId).toBe('live');
  });

  it('disposing during registration unlistens the late registration and never starts a GET', async () => {
    const registration = deferred<() => void>(), off = vi.fn(); native.ready.mockReturnValueOnce(registration.promise);
    const release = acquire(); release(); registration.resolve(off); await tick();
    expect(off).toHaveBeenCalledTimes(1); expect(native.get).not.toHaveBeenCalled();
  });

  it('late cleanup and old native finally cannot clear a remounted session or start parallel GETs', async () => {
    const release = acquire(); await started(1); release(); acquire(); await tick();
    expect(reads).toHaveLength(1); expect(listeners.size).toBe(1);
    release(); reads[0].resolve(cfg('old-lifecycle')); await started(2);
    expect(store.useAppStore.getState().config).toBeNull(); reads[1].resolve(cfg('new-lifecycle'));
    await vi.waitFor(() => expect(store.useAppStore.getState().config?.selectedServerId).toBe('new-lifecycle'));
    expect(listeners.size).toBe(1);
  });

  it('one-shot cancellation removes only that requester and does not cancel native work for another', async () => {
    const controller = new AbortController();
    const cancelled = store.refreshConfig({ signal: controller.signal }).catch((error) => error);
    const remaining = store.refreshConfig(); await started(1); controller.abort();
    expect((await cancelled).message).toContain('cancelled'); reads[0].resolve(cfg('remaining'));
    expect((await remaining).config.selectedServerId).toBe('remaining'); expect(listeners.size).toBe(0);
  });

  it('registration failure rejects current requests, retains seed, and retries on demand', async () => {
    const seed = cfg('seed'); store.useAppStore.setState({ config: seed });
    native.ready.mockRejectedValueOnce(Error('registration failed'));
    await expect(store.refreshConfig()).rejects.toThrow('registration failed');
    expect(store.useAppStore.getState().config).toBe(seed); expect(reads).toHaveLength(0);
    const next = store.refreshConfig(); await started(1); reads[0].resolve(cfg('retry')); await next;
  });

  it('main awaits staged hydration and fences a newer signal during the gate', async () => {
    const gate = deferred<void>(); native.hydrate.mockReturnValueOnce(gate.promise);
    acquire(); const waiting = store.refreshConfig(); await started(1); reads[0].resolve(cfg('old'));
    await vi.waitFor(() => expect(native.hydrate).toHaveBeenCalledTimes(1));
    expect(store.useAppStore.getState().config).toBeNull(); signal();
    expect(reads).toHaveLength(1); gate.resolve(); await started(2); reads[1].resolve(cfg('new'));
    expect((await waiting).config.selectedServerId).toBe('new');
  });

  it.each(['read', 'hydrate'])('reset during %s invalidates the old work and refills live leases', async (phase) => {
    const gate = deferred<void>(); if (phase === 'hydrate') native.hydrate.mockReturnValueOnce(gate.promise);
    acquire(); await started(1);
    if (phase === 'hydrate') { reads[0].resolve(cfg('old')); await vi.waitFor(() => expect(native.hydrate).toHaveBeenCalled()); }
    store.useAppStore.getState().reset(); expect(store.useAppStore.getState().configLoading).toBe(false);
    if (phase === 'read') reads[0].resolve(cfg('old')); else gate.resolve();
    await started(2); expect(store.useAppStore.getState().config).toBeNull(); reads[1].resolve(cfg('new'));
    await vi.waitFor(() => expect(store.useAppStore.getState().config?.selectedServerId).toBe('new'));
  });

  it('a dedicated-read generation invalidates shared work without falsely satisfying its waiters', async () => {
    acquire(); const waiting = store.refreshConfig(); await started(1);
    readOrder.configReadGeneration.beginRead(); reads[0].resolve(cfg('older'));
    await started(2); reads[1].resolve(cfg('tail')); expect((await waiting).config.selectedServerId).toBe('tail');
  });

  it('tray never consumes the main clean-exit gate and mixed scopes fail explicitly', async () => {
    const release = store.acquireConfigSync({ scope: 'tray' }); releases.push(release);
    const waiting = store.refreshConfig(); await started(1); reads[0].resolve(cfg('tray')); await waiting;
    expect(native.hydrate).not.toHaveBeenCalled();
    expect(() => store.acquireConfigSync({ scope: 'main' })).toThrow('Mixed main/tray');
  });

  it('a delayed write reply cannot overwrite a newer external publication, and settlement starts a fresh GET', async () => {
    const write = deferred<UserConfig>(); native.patch.mockReturnValueOnce(write.promise);
    const saving = store.useAppStore.getState().saveConfig({ proxyMode: 'global' });
    await vi.waitFor(() => expect(native.patch).toHaveBeenCalledTimes(1));
    const newer = cfg('external'); store.useAppStore.setState({ config: newer });
    write.resolve({ ...cfg('old-reply'), proxyMode: 'global' }); await started(1);
    expect(store.useAppStore.getState().config).toBe(newer);
    const final = cfg('disk-final'); reads[0].resolve(final); await saving;
    expect(store.useAppStore.getState().config).toBe(final); expect(native.patch).toHaveBeenCalledTimes(1);
  });

  it('confirmed writes stay successful when the reconciliation read fails', async () => {
    native.patch.mockResolvedValue(cfg('saved'));
    const saving = store.useAppStore.getState().saveConfig({ proxyMode: 'global' });
    await started(1); reads[0].reject(Error('read failed'));
    await expect(saving).resolves.toBeUndefined(); expect(native.patch).toHaveBeenCalledTimes(1);
  });

  it('write failures still reconcile once without retrying the write or changing its outcome', async () => {
    native.patch.mockRejectedValue(Error('write failed'));
    const saving = store.useAppStore.getState().saveConfig({ proxyMode: 'global' }).catch((error) => error);
    await started(1); expect((await saving).message).toBe('write failed'); reads[0].resolve(cfg('actual-disk'));
    await vi.waitFor(() => expect(store.useAppStore.getState().config?.selectedServerId).toBe('actual-disk'));
    expect(native.patch).toHaveBeenCalledTimes(1);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import { useAppStore } from './app-store';
import { useLatencyStore, subscribeLatencyEvents } from './use-latency-store';
import { measurementFixture, measurementServer } from './measurement-fixture.test-support';
import { api } from '@/ipc';

const handlers = vi.hoisted(() => ({ result: undefined as (() => void) | undefined, lifecycle: undefined as (() => void) | undefined }));
vi.mock('@/lib/staged-config', async importOriginal => ({ ...await importOriginal<typeof import('@/lib/staged-config')>(), configContentDigest: async () => 'd'.repeat(64) }));
vi.mock('@/ipc', () => ({ api: {
  server: { speedTestResultSnapshot: vi.fn(), onSpeedTestResult: (fn: () => void) => { handlers.result = fn; return () => { handlers.result = undefined; }; },
    onSpeedTestSchedule: () => () => {} },
  proxy: { onLifecycle: (fn: () => void) => { handlers.lifecycle = fn; return () => { handlers.lifecycle = undefined; }; } },
} }));

const config = { servers: ['a','b','c'].map(measurementServer), subscriptions: [] } as unknown as UserConfig;
let cleanup: (() => void) | undefined;
const tick = async () => { await vi.advanceTimersByTimeAsync(250); };
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks();
  useAppStore.setState({ config, proxyStatus: { running: true, mainGeneration: 7, startTime: 10 } as never });
  useLatencyStore.setState({ snapshot: null, latencyMap: {}, testedAt: {}, projections: {}, currentProjections: {} });
});
afterEach(() => { cleanup?.(); cleanup = undefined; vi.useRealTimers(); });

describe('one authoritative ledger feed', () => {
  it('does not trust raw event/invoke numbers without a same-context readback', () => {
    useLatencyStore.getState().applyLatencyResult('a', 1);
    useLatencyStore.getState().applyLatencyResults({ a: 2 });
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    expect(useLatencyStore.getState().testedAt).toEqual({});
  });
  it('hydrates after window recreation, keeps all nodes across screen switches, and uses producer timestamps', async () => {
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(measurementFixture(config, { a: 120, b: 240, c: null }));
    cleanup = subscribeLatencyEvents(); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({ a: 120, b: 240, c: null });
    expect(useLatencyStore.getState().testedAt).toEqual({ a: 1000, b: 1000, c: 1000 });
    const acrossScreen = useLatencyStore.getState().latencyMap;
    expect(useLatencyStore.getState().latencyMap).toBe(acrossScreen);
    cleanup(); cleanup = subscribeLatencyEvents(); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({ a: 120, b: 240, c: null });
  });
  it('rejects an in-flight old-config response and then reads the actual new node identity', async () => {
    let resolve!: (snapshot: ReturnType<typeof measurementFixture>) => void;
    vi.mocked(api.server.speedTestResultSnapshot).mockImplementationOnce(() => new Promise(accept => { resolve = accept; }));
    const next = { ...config, servers: [ { ...config.servers[0], address: 'changed.test' } ] };
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(measurementFixture(next, { a: 88 }, '2'));
    cleanup = subscribeLatencyEvents(); await vi.waitFor(() => expect(resolve).toBeTypeOf('function'));
    useAppStore.setState({ config: next });
    resolve(measurementFixture(config, { a: 1 })); await tick(); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({ a: 88 });
  });
  it('lifecycle invalidation withdraws green data immediately; failed readback cannot restore it', async () => {
    const snapshot = measurementFixture(config, { a: 30, b: null, c: 50 });
    snapshot.rows.c.candidate!.state = 'expired'; snapshot.rows.c.candidate!.reason = 'expired';
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(snapshot);
    cleanup = subscribeLatencyEvents(); await tick();
    vi.mocked(api.server.speedTestResultSnapshot).mockRejectedValue(new Error('unavailable'));
    handlers.lifecycle?.(); expect(useLatencyStore.getState().latencyMap).toEqual({});
    expect(useLatencyStore.getState().projections.a).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().projections.b).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().currentProjections.a).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().projections.c).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().testedAt).toEqual({}); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    expect(useLatencyStore.getState().readError).toBe(true);
  });
  it('raw late events only wake the ledger and cannot overwrite a newer stored result', async () => {
    const snapshot = measurementFixture(config, { a: 88, b: null }, '9');
    snapshot.rows.b.nativeHttp = null;
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(snapshot);
    cleanup = subscribeLatencyEvents(); await tick();
    expect(useLatencyStore.getState().projections.b).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().latencyMap).toEqual({ a: 88 });
    const previous = useLatencyStore.getState().latencyMap;
    handlers.result?.(); await tick(); expect(useLatencyStore.getState().latencyMap).toBe(previous);
  });
  it('retains expired values for display while excluding them from latency selection', () => {
    const snapshot = measurementFixture(config, { a: 30, b: null });
    useLatencyStore.getState().applySnapshot(snapshot, config.servers, 2001);
    expect(useLatencyStore.getState().projections.a).toMatchObject({ state: 'expired', latency: 30 });
    expect(useLatencyStore.getState().projections.b).toMatchObject({ state: 'expired', failure: { kind: 'timeout' } });
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    useLatencyStore.getState().invalidate();
    expect(useLatencyStore.getState().projections.a).toEqual({ state: 'unmeasured' });
    expect(useLatencyStore.getState().currentProjections.b).toEqual({ state: 'unmeasured' });
  });
  it('reference-counts listeners and rejects responses after complete window teardown', async () => {
    let resolve!: (snapshot: ReturnType<typeof measurementFixture>) => void;
    vi.mocked(api.server.speedTestResultSnapshot).mockImplementation(() => new Promise(accept => { resolve = accept; }));
    const first = subscribeLatencyEvents(); const second = subscribeLatencyEvents();
    await vi.waitFor(() => expect(resolve).toBeTypeOf('function'));
    first(); expect(handlers.result).toBeTypeOf('function'); second();
    resolve(measurementFixture(config, { a: 1 })); await tick();
    expect(handlers.result).toBeUndefined(); expect(useLatencyStore.getState().latencyMap).toEqual({});
  });
  it('rejects a different generation and an inconsistent or malformed snapshot', async () => {
    const result = measurementFixture(config, { a: 1 }); result.generation = 8;
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(result);
    cleanup = subscribeLatencyEvents(); await tick(); expect(useLatencyStore.getState().latencyMap).toEqual({});
    result.generation = 7; result.consistent = false; handlers.result?.(); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    result.consistent = true; delete (result.rows.a.candidate as unknown as Record<string, unknown>).identity;
    handlers.result?.(); await tick(); expect(useLatencyStore.getState().latencyMap).toEqual({});
  });
  it('rejects an incorrect full content digest even when the short edit version agrees', async () => {
    const result = measurementFixture(config, { a: 1 }); result.frontendConfigDigest = 'e'.repeat(64);
    vi.mocked(api.server.speedTestResultSnapshot).mockResolvedValue(result);
    cleanup = subscribeLatencyEvents(); await tick();
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    expect(useLatencyStore.getState().readError).toBe(true);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NativeAutoStatus } from '@/contracts/auto-select';
import { nativeAutoFixture } from '@/contracts/native-auto.test-support';
const native = vi.hoisted(() => ({ ready: vi.fn(), status: vi.fn() }));
const app = vi.hoisted(() => ({ proxyStatus: null as null | { running: boolean; mainGeneration: number; actualServerId: string | null }, setProxyStatus: vi.fn() }));
vi.mock('@/ipc', () => ({ api: { server: { onAutoSelectStatusReady: native.ready, autoSelectStatus: native.status } } }));
vi.mock('@/store/app-store', () => ({ useAppStore: Object.assign(vi.fn(), { getState: () => app }) }));
import { nativeAutoStatusSource as source } from './use-auto-select-status';
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (error: unknown) => void; const promise = new Promise<T>((ok, no) => { resolve = ok; reject = no; }); return { promise, resolve, reject }; }
let callbacks: Array<(status: NativeAutoStatus) => void>;
let releases: Array<() => void>;
let off = vi.fn<() => void>();
function acquire(key = 'one') { const release = source.acquire(key); releases.push(release); return release; }
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
beforeEach(() => {
  vi.clearAllMocks(); callbacks = []; releases = []; off = vi.fn<() => void>(); app.proxyStatus = null;
  native.ready.mockImplementation((callback: (status: NativeAutoStatus) => void) => { callbacks.push(callback); return Promise.resolve(off); });
  native.status.mockResolvedValue(nativeAutoFixture());
  app.setProxyStatus.mockImplementation(value => { app.proxyStatus = value; });
});
afterEach(async () => { releases.forEach(release => release()); await settle(); vi.restoreAllMocks(); });

describe('shared native status source used by the React hook', () => {
  it('shares one ready registration and snapshot across consumers', async () => {
    const registration = deferred<() => void>(); native.ready.mockImplementation(callback => { callbacks.push(callback); return registration.promise; });
    acquire(); acquire(); await settle();
    expect(native.ready).toHaveBeenCalledTimes(1); expect(native.status).not.toHaveBeenCalled();
    registration.resolve(off); await settle();
    expect(native.status).toHaveBeenCalledTimes(1); expect(source.getSnapshot().report?.generation).toBe(1);
  });
  it.each([false, true])('newer event survives a late snapshot or rejection (%s)', async reject => {
    const read = deferred<NativeAutoStatus>(); native.status.mockReturnValue(read.promise);
    acquire(); await settle(); callbacks[0](nativeAutoFixture({ generation: 8 }));
    if (reject) read.reject(Error('late')); else read.resolve(nativeAutoFixture());
    await settle(); expect(source.getSnapshot()).toMatchObject({ report: { generation: 8 }, failed: false, loading: false });
  });
  it('context changes invalidate old events and reads while other consumers remain', async () => {
    const read = deferred<NativeAutoStatus>(); native.status.mockReturnValueOnce(read.promise);
    acquire(); acquire(); await settle(); acquire('new-generation'); await settle();
    callbacks[0](nativeAutoFixture({ generation: 99 })); read.resolve(nativeAutoFixture({ generation: 98 })); await settle();
    expect(native.ready).toHaveBeenCalledTimes(2); expect(source.getSnapshot().report?.generation).toBe(1);
  });
  it('late listener readiness after release detaches and never starts a read', async () => {
    const registration = deferred<() => void>(); native.ready.mockReturnValue(registration.promise);
    const release = acquire(); await settle(); release(); release(); registration.resolve(off); await settle();
    expect(off).toHaveBeenCalledTimes(1); expect(native.status).not.toHaveBeenCalled(); expect(source.getSnapshot().report).toBeNull();
  });
  it('registration failure, including synchronous throw, is recoverable', async () => {
    native.ready.mockImplementationOnce(() => { throw Error('registration'); }); acquire(); await settle();
    expect(source.getSnapshot().failed).toBe(true); await source.refresh();
    expect(source.getSnapshot().failed).toBe(false); expect(native.status).toHaveBeenCalledTimes(1);
  });
  it('refresh retries a failed snapshot and concurrent refreshes share one read', async () => {
    native.status.mockRejectedValueOnce(Error('read')); acquire(); await settle(); expect(source.getSnapshot().failed).toBe(true);
    const read = deferred<NativeAutoStatus>(); native.status.mockReturnValueOnce(read.promise);
    const retry = source.refresh(); void source.refresh(); void source.refresh(); await settle(); expect(native.status).toHaveBeenCalledTimes(2);
    read.resolve(nativeAutoFixture()); await retry; expect(source.getSnapshot().failed).toBe(false);
  });
  it('only the matching running generation updates the actual exit', async () => {
    app.proxyStatus = { running: true, mainGeneration: 2, actualServerId: null }; acquire(); await settle();
    expect(app.setProxyStatus).not.toHaveBeenCalled(); callbacks[0](nativeAutoFixture({ generation: 2, actualServerId: 'real-node' }));
    expect(app.setProxyStatus).toHaveBeenCalledExactlyOnceWith({ running: true, mainGeneration: 2, actualServerId: 'real-node' });
  });
  it('failed detach cannot revive an old session', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {}); off.mockImplementation(() => { throw Error('detach'); });
    const release = acquire(); await settle(); release(); acquire('new'); await settle(); callbacks[0](nativeAutoFixture({ generation: 99 }));
    expect(source.getSnapshot().report?.generation).toBe(1);
  });
  it('invalid snapshots fail explicitly instead of publishing malformed group state', async () => {
    native.status.mockResolvedValue({}); acquire(); await settle(); expect(source.getSnapshot()).toMatchObject({ failed: true, report: null });
  });
});

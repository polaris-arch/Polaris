import { afterEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { startVpnStatusSync, type VpnStatusPort } from './vpn-status-sync';
import type { VpnStatusSnapshot } from '@/contracts/vpn-status';
function deferred<T>() { let resolve!: (v: T) => void; let reject!: (e: Error) => void; const promise = new Promise<T>((r, j) => { resolve = r; reject = j; }); return { promise, resolve, reject }; }
const settle = async () => { for (let n = 0; n < 8; n++) await Promise.resolve(); };
const snapshot = (id: string): VpnStatusSnapshot => ({ connected: true, openConnect: [{ serverId: 'oc', state: '', stateText: '', authChallenge: { id, kind: 'form', fields: [] } }], openVpn: [{ serverId: 'ov', state: '', stateText: '', challenge: { id, kind: 'secret', echo: false, deadline: 0 } }] });
function fixture() {
  const registrations: ReturnType<typeof deferred<() => void>>[] = [];
  const callbacks: Record<string, (value?: any) => void> = {};
  const offs: ReturnType<typeof vi.fn>[] = [];
  const reads: ReturnType<typeof deferred<VpnStatusSnapshot>>[] = [];
  const register = (name: string, callback: (value?: any) => void) => { callbacks[name] = callback; const d = deferred<() => void>(); registrations.push(d); return d.promise; };
  const port: VpnStatusPort = {
    onOpenConnectStatusReady: callback => register('oc', callback), onOpenVpnStatusReady: callback => register('ov', callback),
    onLifecycleReady: callback => register('lifecycle', callback), onStartedReady: callback => register('started', callback), onStoppedReady: callback => register('stopped', callback),
    getStatus: vi.fn(() => { const d = deferred<VpnStatusSnapshot>(); reads.push(d); return d.promise; }),
  };
  const sink = { snapshot: vi.fn(), openConnect: vi.fn(), openVpn: vi.fn() };
  const sync = startVpnStatusSync(port, sink);
  const ready = async () => { for (const d of registrations) { const off = vi.fn(); offs.push(off); d.resolve(off); } await settle(); };
  return { sync, sink, port, callbacks, registrations, offs, reads, ready };
}
afterEach(() => vi.useRealTimers());
describe('VPN ready/revision/disposal lifecycle', () => {
  it('does not read until all five feeds are registered', async () => {
    const f = fixture(); f.registrations[0].resolve(vi.fn()); await settle(); expect(f.reads).toHaveLength(0);
    await f.ready(); expect(f.reads).toHaveLength(1); f.reads[0].resolve(snapshot('initial')); await settle(); expect(f.sink.snapshot).toHaveBeenCalledWith(snapshot('initial')); f.sync.dispose();
  });
  it.each(['oc', 'ov'])('rejects a stale snapshot after a newer %s challenge event', async protocol => {
    const f = fixture(); await f.ready(); const newer = protocol === 'oc' ? snapshot('new').openConnect[0] : snapshot('new').openVpn[0]; f.callbacks[protocol](newer);
    f.reads[0].resolve(snapshot('old')); await settle(); expect(f.sink.snapshot).not.toHaveBeenCalled(); expect(protocol === 'oc' ? f.sink.openConnect : f.sink.openVpn).toHaveBeenCalledWith(newer); f.sync.dispose();
  });
  it.each(['stopped', 'lifecycle'])('a %s stop invalidates in-flight reads, restart reads cannot resurrect a prior session', async channel => {
    const f = fixture(); await f.ready(); f.callbacks[channel]({ phase: 'stopped' });
    f.callbacks.started(); expect(f.reads).toHaveLength(2);
    f.reads[1].resolve(snapshot('restart')); await settle(); f.reads[0].resolve(snapshot('old')); await settle();
    expect(f.sink.snapshot.mock.calls.map(c => c[0])).toEqual([{ connected: false, openConnect: [], openVpn: [] }, { connected: false, openConnect: [], openVpn: [] }, snapshot('restart')]); f.sync.dispose();
  });
  it('authoritative store boundary invalidates reads and latest read wins', async () => {
    const f = fixture(); await f.ready(); f.sync.boundary(false); f.sync.boundary(true); f.sync.boundary(true);
    f.reads[2].resolve(snapshot('last')); await settle(); f.reads[1].resolve(snapshot('middle')); f.reads[0].resolve(snapshot('old')); await settle();
    expect(f.sink.snapshot.mock.lastCall?.[0]).toEqual(snapshot('last')); expect(f.sink.snapshot).toHaveBeenCalledTimes(4); f.sync.dispose();
  });
  it('dispose releases late registrations and never opens a sheet from late reads or callbacks', async () => {
    const f = fixture(); await f.ready(); f.sync.dispose(); f.reads[0].resolve(snapshot('late')); f.callbacks.oc(snapshot('late').openConnect[0]); f.callbacks.started(); await settle();
    expect(f.sink.snapshot).not.toHaveBeenCalled(); expect(f.sink.openConnect).not.toHaveBeenCalled(); expect(f.offs.every(off => off.mock.calls.length === 1)).toBe(true);
    const g = fixture(); g.sync.dispose(); await g.ready(); expect(g.offs.every(off => off.mock.calls.length === 1)).toBe(true); expect(g.reads).toHaveLength(0);
  });
  it('registration failure retries and releases partial/late registrations', async () => {
    vi.useFakeTimers(); const f = fixture(); const off = vi.fn(); f.registrations[0].resolve(off); f.registrations[1].reject(Error('synthetic')); await settle(); expect(off).toHaveBeenCalledOnce();
    const late = vi.fn(); f.registrations[2].resolve(late); await settle(); expect(late).toHaveBeenCalledOnce(); await vi.advanceTimersByTimeAsync(1000); expect(f.registrations).toHaveLength(10); f.sync.dispose();
  });
  it.each([true, false])('registration recovery survives running=%s boundaries without parallel attempts', async running => {
    vi.useFakeTimers(); const f = fixture();
    const partial = vi.fn(); f.registrations[0].resolve(partial);
    f.registrations[1].reject(Error('registration transient')); await settle();
    f.sync.boundary(running); f.sync.boundary(running);
    expect(f.registrations).toHaveLength(5);
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.registrations).toHaveLength(10);
    // Old attempt's late successful registration still releases exactly once.
    const late = vi.fn(); f.registrations[2].resolve(late); await settle(); expect(late).toHaveBeenCalledOnce(); expect(partial).toHaveBeenCalledOnce();
    f.sync.boundary(running); expect(f.registrations).toHaveLength(10);
    const recovered = f.registrations.slice(5).map(d => { const off = vi.fn(); d.resolve(off); return off; });
    await settle(); expect(f.reads).toHaveLength(running ? 1 : 0);
    if (!running) { f.callbacks.started(); expect(f.reads).toHaveLength(1); }
    f.reads[0].resolve(snapshot('recovered')); await settle(); expect(f.sink.snapshot.mock.lastCall?.[0]).toEqual(snapshot('recovered'));
    f.sync.dispose(); expect(recovered.every(off => off.mock.calls.length === 1)).toBe(true);
    await vi.advanceTimersByTimeAsync(5000); expect(f.registrations).toHaveLength(10);
  });
  it.each(['oc', 'ov'])('new %s event followed by failed read still retries and hydrates snapshot-only endpoints', async protocol => {
    vi.useFakeTimers(); const f = fixture(); await f.ready();
    f.callbacks[protocol](protocol === 'oc' ? snapshot('A-new').openConnect[0] : snapshot('A-new').openVpn[0]);
    f.reads[0].reject(Error('read transient')); await settle();
    await vi.advanceTimersByTimeAsync(1000); expect(f.reads).toHaveLength(2);
    const complete = snapshot('A-new'); complete.openVpn.push({ ...complete.openVpn[0], serverId: 'B', challenge: { ...complete.openVpn[0].challenge!, id: 'B-challenge' } });
    f.reads[1].resolve(complete); await settle(); expect(f.sink.snapshot).toHaveBeenCalledExactlyOnceWith(complete); f.sync.dispose();
  });
  it.each(['stop', 'dispose'])('%s cancels failed-read recovery and ignores subsequent late rejection', async action => {
    vi.useFakeTimers(); const f = fixture(); await f.ready(); f.reads[0].reject(Error('transient')); await settle();
    if (action === 'stop') f.sync.boundary(false); else f.sync.dispose();
    await vi.advanceTimersByTimeAsync(5000); expect(f.reads).toHaveLength(1); f.sync.dispose();
    const g = fixture(); await g.ready(); if (action === 'stop') g.sync.boundary(false); else g.sync.dispose();
    g.reads[0].reject(Error('late')); await settle(); await vi.advanceTimersByTimeAsync(5000); expect(g.reads).toHaveLength(1); g.sync.dispose();
  });
  it('dispose cancels registration recovery before its timer fires', async () => {
    vi.useFakeTimers(); const f = fixture(); f.registrations[0].reject(Error('transient')); await settle(); f.sync.dispose();
    await vi.advanceTimersByTimeAsync(5000); expect(f.registrations).toHaveLength(5);
    const late = vi.fn(); f.registrations[1].resolve(late); await settle(); expect(late).toHaveBeenCalledOnce();
  });
  it('both roots have one shared owner and no unguarded snapshot ingress', () => {
    for (const file of ['../App.tsx', '../mobile/app-wiring.ts']) {
      const source = readFileSync(new URL(file, import.meta.url), 'utf8'); expect(source).toContain('startAppVpnStatusSync('); expect(source).not.toContain('api.vpn.getStatus(');
    }
    const form = readFileSync(new URL('../components/vpn/VpnAuthForm.tsx', import.meta.url), 'utf8'); expect(form).not.toContain('<VpnAuthSession'); expect(form).not.toContain('key={JSON.stringify');
  });
});

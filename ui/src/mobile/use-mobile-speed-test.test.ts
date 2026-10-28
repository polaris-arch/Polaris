import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { useMobileSpeedTestStore as store, compareSpeedTestRunIds, validSpeedTestRunId, mobileSpeedTestBusy, subscribeMobileSpeedTestProgress, runMobileSpeedTest } from './use-mobile-speed-test';
import { api } from '@/ipc';
import { useLatencyStore } from '@/store/use-latency-store';
import { useAppStore } from '@/store/app-store';
import type { SpeedTestInvokeResult, SpeedTestMeasurementContext } from '@/contracts/speed-test';

const applyResults = useLatencyStore.getState().applyLatencyResults;
const receipt = (runId: string, extra: Partial<SpeedTestInvokeResult> = {}): SpeedTestInvokeResult => ({ runId, results: { a: 12, b: -1 }, outcome: 'completed', notInPool: [], tsNotReady: [], ...extra });
const done = (runId: string, extra: Record<string, unknown> = {}) => ({ runId, outcome: 'completed' as const, tested: 2, total: 2, serverIds: ['a', 'b'], pending: [], ...extra });
beforeEach(() => { store.setState({ task: null, request: null, nextToken: 0 }); useLatencyStore.setState({latencyMap:{},testedAt:{},applyLatencyResults:applyResults}); });
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

describe('mobile measured latency task identity', () => {
  it('decimal ids exceed JS integer range safely; malformed/unidentified frames are never adopted', () => {
    expect(compareSpeedTestRunIds('9007199254740993','9007199254740992')).toBe(1);
    expect(compareSpeedTestRunIds('10','9')).toBe(1);
    for (const value of [undefined, '', '0', '01', '-1', 4, '1e4']) expect(validSpeedTestRunId(value)).toBe(false);
    store.getState().progress({ tested: 1, ok: 1, total: 1 });
    store.getState().result({ serverId: 'automatic-rtt', latency: 20 });
    expect(store.getState().task).toBeNull();
  });
  it('out-of-order old progress/DONE/receipt cannot overwrite the next run, including same node range', () => {
    const token = store.getState().begin(['a','b'],'all')!;
    store.getState().progress({ runId: '10', tested: 1, ok: 1, total: 2 });
    store.getState().done(done('10'));
    store.getState().progress({ runId: '11', tested: 1, ok: 0, total: 2 });
    const task = store.getState().task;
    store.getState().done(done('10'));
    store.getState().progress({ runId: '10', tested: 2, ok: 2, total: 2 });
    expect(store.getState().settle(token, receipt('10'))).toBe(false);
    expect(store.getState().task).toBe(task);
    expect(store.getState().request).toBeNull();
    expect(mobileSpeedTestBusy(store.getState())).toBe(true);
  });
  it('DONE before final progress is terminal and duplicate DONE never reopens or rewrites it', () => {
    store.getState().done(done('2', { outcome: 'interrupted', tested: 1, pending: ['b'] }));
    store.getState().progress({ runId:'2', tested:1, ok:0, total:2 });
    const task = store.getState().task;
    expect(task?.phase).toBe('interrupted');
    expect(task?.pending).toBe(1);
    expect(task?.ok).toBe(0);
    store.getState().done(done('2'));
    expect(store.getState().task).toBe(task);
  });
  it('receipt skips/failed results retain truthful counts rather than counting every tested node successful', () => {
    const token = store.getState().begin(['a','b','c','d','e'],'nodes')!;
    expect(store.getState().task).toBeNull(); // no fabricated total during preparation
    expect(store.getState().settle(token, receipt('3', { outcome:'interrupted', notInPool:['c'], dirty:['d'] }))).toBe(true);
    expect(store.getState().task).toMatchObject({tested:2,total:3,ok:1,pending:1,skipped:2,phase:'interrupted'});
  });
  it('cross-screen request persists; an unrelated early failure clears only its own token', () => {
    const token = store.getState().begin(['a'],'current')!;
    expect(store.getState().begin(['b'],'nodes')).toBeNull();
    store.getState().progress({runId:'4', tested:1,ok:1,total:3});
    expect(store.getState().settle(token+1)).toBe(false);
    expect(store.getState().request?.token).toBe(token);
    store.getState().settle(token);
    expect(store.getState().task?.runId).toBe('4');
    expect(mobileSpeedTestBusy(store.getState())).toBe(true);
  });
  it('waiting is unconfirmed, not interrupted; retry may ask the backend and never erases the old task', () => {
    store.getState().progress({runId:'5',tested:1,ok:1,total:3});
    store.getState().waiting('5');
    expect(mobileSpeedTestBusy(store.getState())).toBe(false);
    const token = store.getState().begin(['a'],'current')!;
    expect(store.getState().request?.lastRunId).toBe('5');
    store.getState().settle(token); // backend in-flight refusal
    expect(store.getState().task).toMatchObject({runId:'5',phase:'waiting',tested:1,total:3});
    expect(store.getState().begin(['a'],'current')).not.toBeNull();
  });
  it('stale/duplicate ignored frames do not defer the new run silence timer; dispose unsubscribes', () => {
    vi.useFakeTimers();
    let progress!: Parameters<typeof api.server.onSpeedTestProgress>[0];
    const off = vi.fn();
    vi.spyOn(api.server,'onSpeedTestProgress').mockImplementation(handler => { progress=handler;return off; });
    vi.spyOn(api.server,'onSpeedTestResult').mockReturnValue(off);
    vi.spyOn(api.server,'onSpeedTestDone').mockReturnValue(off);
    const dispose = subscribeMobileSpeedTestProgress();
    progress({runId:'7',tested:1,ok:1,total:3});
    vi.advanceTimersByTime(19_000);
    progress({runId:'6',tested:1,ok:1,total:2});
    vi.advanceTimersByTime(1_000);
    expect(store.getState().task).toMatchObject({runId:'7',phase:'waiting',tested:1,total:3});
    dispose();expect(off).toHaveBeenCalledTimes(3);
  });
  it('only accepted active RESULT writes latency; old results cannot overwrite a newer run and passive RTT still works', () => {
    let result!: Parameters<typeof api.server.onSpeedTestResult>[0];
    vi.spyOn(api.server,'onSpeedTestProgress').mockReturnValue(() => {});
    vi.spyOn(api.server,'onSpeedTestResult').mockImplementation(handler => { result=handler;return () => {}; });
    vi.spyOn(api.server,'onSpeedTestDone').mockReturnValue(() => {});
    const dispose=subscribeMobileSpeedTestProgress();
    result({runId:'9',serverId:'a',latency:10});
    result({runId:'10',serverId:'a',latency:20});
    result({runId:'9',serverId:'a',latency:99});
    expect(useLatencyStore.getState().latencyMap.a).toBe(20);
    result({serverId:'warm',latency:30});
    expect(useLatencyStore.getState().latencyMap.warm).toBe(30);
    expect(store.getState().task?.runId).toBe('10');
    expect(store.getState().task?.results).toEqual({a:20});
    dispose();
  });
  it('current-exit route guard persists across screen departure and receipt/event share one latency writer', async () => {
    let result!: Parameters<typeof api.server.onSpeedTestResult>[0];
    vi.spyOn(api.server,'onSpeedTestProgress').mockReturnValue(() => {});
    vi.spyOn(api.server,'onSpeedTestResult').mockImplementation(handler => { result=handler;return () => {}; });
    vi.spyOn(api.server,'onSpeedTestDone').mockReturnValue(() => {});
    let finish!: (value: SpeedTestInvokeResult) => void;
    vi.spyOn(api.server,'speedTest').mockImplementation(() => new Promise(resolve => {finish=resolve;}));
    const apply=vi.spyOn(useLatencyStore.getState(),'applyLatencyResults');
    const dispose=subscribeMobileSpeedTestProgress();
    let routeMatches=true;
    const first=runMobileSpeedTest(['a'],'current', () => routeMatches);
    routeMatches=false;
    result({runId:'11',serverId:'a',latency:20});
    finish(receipt('11',{results:{a:20}}));await first;
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined();
    result({runId:'11',serverId:'a',latency:25});
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined();
    routeMatches=true;
    const second=runMobileSpeedTest(['a'],'all');
    result({runId:'12',serverId:'a',latency:40});
    finish(receipt('12',{results:{a:40}}));await second;
    expect(useLatencyStore.getState().latencyMap.a).toBe(40);
    expect(apply).toHaveBeenCalledOnce(); // event and receipt together write the value once
    dispose();
  });

  it('preparation has no fake total and an early prerequisite rejection clears its own request', async () => {
    vi.spyOn(api.server, 'speedTest').mockImplementation(async () => {
      store.getState().progress({ runId: '20', phase: 'preparingConnection' });
      expect(store.getState().task).toMatchObject({ phase: 'preparingConnection', total: null, ok: null, results: {} });
      expect(store.getState().begin(['b'], 'nodes')).toBeNull();
      store.getState().progress({ runId: '20', phase: 'waitingForReady' });
      throw { code: 'IOS_VPN_PERMISSION_DENIED' };
    });
    await expect(runMobileSpeedTest(['a'], 'nodes')).rejects.toMatchObject({ code: 'IOS_VPN_PERMISSION_DENIED' });
    expect(store.getState().request).toBeNull();
    expect(store.getState().task).toMatchObject({ phase: 'failed', total: null, results: {} });
    expect(mobileSpeedTestBusy(store.getState())).toBe(false);
  });

  it('auto-started current measurement binds after ready; receipt fills a streamed result withheld by status verification', async () => {
    const node = { id: 'a', name: 'a', protocol: 'vless', address: 'a.example', port: 443 } as const;
    useAppStore.setState({ servers: [node], selectedServerId: 'a', config: { servers: [node], subscriptions: [] } as never, proxyStatus: { running: false, mainGeneration: 1 } });
    const context: SpeedTestMeasurementContext = { runId: '21', requestId: 'speed-21', mainGeneration: 2, startTime: 123 };
    let result!: Parameters<typeof api.server.onSpeedTestResult>[0];
    vi.spyOn(api.server, 'onSpeedTestProgress').mockReturnValue(() => {});
    vi.spyOn(api.server, 'onSpeedTestResult').mockImplementation(fn => { result = fn; return () => {}; });
    vi.spyOn(api.server, 'onSpeedTestDone').mockReturnValue(() => {});
    vi.spyOn(api.proxy, 'getStatus').mockResolvedValueOnce({ running: false, mainGeneration: 1 })
      .mockResolvedValue({ running: true, mainGeneration: 2, startTime: 123 });
    let finish!: (value: SpeedTestInvokeResult) => void;
    vi.spyOn(api.server, 'speedTest').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const write = vi.spyOn(useLatencyStore.getState(), 'applyLatencyResults');
    const dispose = subscribeMobileSpeedTestProgress();
    const pending = runMobileSpeedTest(['a'], 'current');
    result({ runId: '21', serverId: 'a', latency: 42, measurementContext: context });
    await vi.waitFor(() => expect(api.proxy.getStatus).toHaveBeenCalledOnce());
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined();
    expect(store.getState().task?.results.a).toBe(42);
    expect(store.getState().task?.appliedResults).toEqual({});
    finish(receipt('21', { results: { a: 42 }, measurementContext: context }));
    await pending;
    expect(useLatencyStore.getState().latencyMap.a).toBe(42);
    expect(write).toHaveBeenCalledOnce();
    expect(useAppStore.getState().proxyStatus).toMatchObject({ running: true, mainGeneration: 2 });
    dispose();
  });

  it.each(['stopped', 'generation', 'selected', 'configuration', 'deleted'] as const)('rejects a ready result after %s changes', async change => {
    const node = { id: 'a', name: 'a', protocol: 'vless', address: 'a.example', port: 443 } as const;
    useAppStore.setState({ servers: [node], selectedServerId: 'a', config: { servers: [node], subscriptions: [] } as never, proxyStatus: { running: false, mainGeneration: 1 } });
    const context: SpeedTestMeasurementContext = { runId: '22', requestId: 'speed-22', mainGeneration: 2, startTime: 123 };
    let finish!: (value: SpeedTestInvokeResult) => void;
    vi.spyOn(api.server, 'speedTest').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const pending = runMobileSpeedTest(['a'], 'current');
    if (change === 'selected') useAppStore.setState({ selectedServerId: 'b' });
    if (change === 'configuration') useAppStore.setState({ config: { servers: [node], proxyMode: 'direct' } as never });
    if (change === 'deleted') useAppStore.setState({ servers: [] });
    vi.spyOn(api.proxy, 'getStatus').mockResolvedValue({ running: change !== 'stopped', mainGeneration: change === 'generation' ? 3 : 2, startTime: 123 });
    finish(receipt('22', { results: { a: 42 }, measurementContext: context }));
    await pending;
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined();
  });

  it('a stop received while getStatus is pending cannot be overwritten by its older running answer', async () => {
    const node = { id: 'a', name: 'a', protocol: 'vless', address: 'a.example', port: 443 } as const;
    useAppStore.setState({ servers: [node], selectedServerId: 'a', config: { servers: [node], subscriptions: [] } as never,
      proxyStatus: { running: true, mainGeneration: 2, startTime: 123 } });
    const context = { runId: '23', requestId: 'speed-23', mainGeneration: 2, startTime: 123 };
    vi.spyOn(api.server, 'speedTest').mockResolvedValue(receipt('23', { results: { a: 42 }, measurementContext: context }));
    let finish!: (status: { running: boolean; mainGeneration: number; startTime: number }) => void;
    vi.spyOn(api.proxy, 'getStatus').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const pending = runMobileSpeedTest(['a'], 'nodes');
    await vi.waitFor(() => expect(api.proxy.getStatus).toHaveBeenCalled());
    useAppStore.setState({ proxyStatus: { running: false, mainGeneration: 2 } });
    finish({ running: true, mainGeneration: 2, startTime: 123 });
    await pending;
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined();
    expect(useAppStore.getState().proxyStatus?.running).toBe(false);
  });

});

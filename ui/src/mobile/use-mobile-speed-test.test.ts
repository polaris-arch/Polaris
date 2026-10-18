import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { useMobileSpeedTestStore as store, compareSpeedTestRunIds, validSpeedTestRunId, mobileSpeedTestBusy, subscribeMobileSpeedTestProgress, runMobileSpeedTest } from './use-mobile-speed-test';
import { api } from '@/ipc';
import { useLatencyStore } from '@/store/use-latency-store';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
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
    dispose();expect(off).toHaveBeenCalledTimes(4);
  });
  it('accepted task results keep run identity while passive/bare numbers cannot bypass ledger qualification', () => {
    let result!: Parameters<typeof api.server.onSpeedTestResult>[0];
    vi.spyOn(api.server,'onSpeedTestProgress').mockReturnValue(() => {});
    vi.spyOn(api.server,'onSpeedTestResult').mockImplementation(handler => { result=handler;return () => {}; });
    vi.spyOn(api.server,'onSpeedTestDone').mockReturnValue(() => {});
    const dispose=subscribeMobileSpeedTestProgress();
    result({runId:'9',serverId:'a',latency:10});
    result({runId:'10',serverId:'a',latency:20});
    result({runId:'9',serverId:'a',latency:99});
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined(); // Only a qualified ledger snapshot may publish numeric data.
    result({serverId:'warm',latency:30});
    expect(useLatencyStore.getState().latencyMap.warm).toBeUndefined();
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
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined(); // Receipt is a refresh signal, not an identity proof.
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
    expect(useLatencyStore.getState().latencyMap.a).toBeUndefined(); // Qualified snapshot feed is tested separately.
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

describe('mobile measurement in-flight reuse and final intent fences', () => {
  const node = { id: 'a', name: 'a', protocol: 'vless', address: 'a.example', port: 443 } as const;
  const second = { ...node, id: 'b', name: 'b' };
  const context: SpeedTestMeasurementContext = { runId: '101', requestId: 'request-101', mainGeneration: 2, startTime: 123 };
  const running = { running: true, mainGeneration: 2, startTime: 123 };
  const deferred = <T,>() => {
    let resolve!: (value: T) => void;
    let reject!: (cause: unknown) => void;
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  };
  function start(extra?: Parameters<typeof runMobileSpeedTest>[2]) {
    const backend = deferred<SpeedTestInvokeResult>();
    vi.spyOn(api.server, 'speedTest').mockReturnValue(backend.promise);
    const pending = runMobileSpeedTest(['a', 'b'], 'nodes', extra, true);
    const guard = store.getState().request!.acceptLatency!;
    return { guard, pending, backend, finish: async () => {
      backend.resolve(receipt('101', { results: {}, measurementContext: context }));
      await pending;
    } };
  }
  beforeEach(() => {
    useStagedConfigStore.setState({ entries: [] });
    useAppStore.setState({ servers: [node, second], selectedServerId: 'a',
      config: { servers: [node, second], subscriptions: [], mixedPort: 7890 } as never, proxyStatus: { ...running } });
  });
  afterEach(() => { useStagedConfigStore.setState({ entries: [] }); });

  it('shares only concurrent raw status reads; each waiter checks the extra guard and settled reads are not cached', async () => {
    const read = deferred<typeof running>();
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValueOnce(read.promise).mockResolvedValue(running);
    const extra = vi.fn().mockResolvedValue(true);
    const request = start(extra);
    const first = request.guard(context);
    const concurrent = request.guard({ ...context });
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledOnce());
    read.resolve(running);
    expect(await Promise.all([first, concurrent])).toEqual([true, true]);
    expect(extra).toHaveBeenCalledTimes(2);
    expect(await request.guard(context)).toBe(true);
    expect(getStatus).toHaveBeenCalledTimes(2);
    expect(extra).toHaveBeenCalledTimes(3);
    await request.finish();
  });

  it.each(['runId', 'requestId', 'mainGeneration', 'startTime'] as const)('does not merge a different %s', async field => {
    const reads = [deferred<typeof running>(), deferred<typeof running>()];
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValueOnce(reads[0].promise).mockReturnValueOnce(reads[1].promise);
    const request = start();
    const changed = { ...context, [field]: field === 'runId' ? '102' : field === 'requestId' ? 'request-102' : 999 };
    const first = request.guard(context);
    const other = request.guard(changed);
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledTimes(2));
    reads.forEach(read => read.resolve(running));
    await Promise.all([first, other]);
    await request.finish();
  });

  it('settling an old identity does not clear a newer identity in flight', async () => {
    const oldRead = deferred<typeof running>();
    const newRead = deferred<typeof running>();
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValueOnce(oldRead.promise).mockReturnValueOnce(newRead.promise);
    const request = start();
    const newer = { ...context, runId: '102', requestId: 'request-102' };
    const oldWaiter = request.guard(context);
    const newWaiter = request.guard(newer);
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledTimes(2));
    oldRead.resolve(running);
    expect(await oldWaiter).toBe(true);
    const anotherNewWaiter = request.guard(newer);
    await Promise.resolve();
    expect(getStatus).toHaveBeenCalledTimes(2);
    newRead.resolve(running);
    expect(await Promise.all([newWaiter, anotherNewWaiter])).toEqual([true, true]);
    await request.finish();
  });

  it('does not share the same producer identity across distinct captured request intents', async () => {
    const oldRead = deferred<typeof running>();
    const newRead = deferred<typeof running>();
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValueOnce(oldRead.promise).mockReturnValueOnce(newRead.promise);
    const first = start();
    const oldWaiter = first.guard(context);
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledOnce());
    await first.finish();
    const secondRequest = start();
    const newWaiter = secondRequest.guard(context);
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledTimes(2));
    oldRead.resolve(running); newRead.resolve(running);
    await Promise.all([oldWaiter, newWaiter]);
    await secondRequest.finish();
  });

  it.each(['rejected', 'stopped'] as const)('clears a %s flight so a later receipt can retry', async failure => {
    const read = deferred<typeof running>();
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValueOnce(read.promise).mockResolvedValue(running);
    const request = start();
    const first = request.guard(context);
    const secondWaiter = request.guard(context);
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledOnce());
    if (failure === 'rejected') read.reject(new Error('status unavailable'));
    else read.resolve({ ...running, running: false });
    expect(await Promise.all([first, secondWaiter])).toEqual([false, false]);
    expect(await request.guard(context)).toBe(true);
    expect(getStatus).toHaveBeenCalledTimes(2);
    await request.finish();
  });

  it('reuses complete serialization and membership, while equal new config objects retain equality semantics', async () => {
    const config = useAppStore.getState().config!;
    const servers = useAppStore.getState().servers;
    const stringify = vi.spyOn(JSON, 'stringify');
    const members = vi.spyOn(servers, 'map');
    vi.spyOn(api.proxy, 'getStatus').mockResolvedValue(running);
    const request = start();
    expect(await request.guard(context)).toBe(true);
    expect(await request.guard(context)).toBe(true);
    expect(stringify.mock.calls.filter(([value]) => value === config)).toHaveLength(1);
    expect(members).toHaveBeenCalledOnce();
    const equalConfig = { ...config };
    useAppStore.setState({ config: equalConfig });
    expect(await request.guard(context)).toBe(true);
    expect(stringify.mock.calls.filter(([value]) => value === equalConfig)).toHaveLength(1);
    const freshServers = [...servers];
    const freshMembers = vi.spyOn(freshServers, 'map');
    useAppStore.setState({ servers: freshServers });
    expect(await request.guard(context)).toBe(true);
    expect(freshMembers).toHaveBeenCalledOnce();
    await request.finish();
  });

  it.each(['non-node config', 'same-id node', 'staged entry', 'selection', 'membership'] as const)('rejects %s changes during the shared status wait', async change => {
    const read = deferred<typeof running>();
    vi.spyOn(api.proxy, 'getStatus').mockReturnValue(read.promise);
    const request = start();
    const guarded = request.guard(context);
    await vi.waitFor(() => expect(api.proxy.getStatus).toHaveBeenCalledOnce());
    if (change === 'non-node config') useAppStore.setState({ config: { ...useAppStore.getState().config!, mixedPort: 7891 } });
    if (change === 'same-id node') useAppStore.setState({ config: { ...useAppStore.getState().config!, servers: [{ ...node, address: 'other.example' }, second] } as never });
    if (change === 'staged entry') useStagedConfigStore.setState({ entries: [{ id: 'setting:mixedPort', kind: 'setting', label: 'port', entityPath: ['mixedPort'], nextValue: 7891 }] });
    if (change === 'selection') useAppStore.setState({ selectedServerId: 'b' });
    if (change === 'membership') useAppStore.setState({ servers: [node] });
    read.resolve(running);
    expect(await guarded).toBe(false);
    await request.finish();
  });

  it.each(['stop', 'generation', 'start time', 'configuration', 'selection', 'membership', 'new task'] as const)('does not commit after %s changes while the extra guard awaits', async change => {
    const route = deferred<boolean>();
    const extra = vi.fn(() => route.promise);
    vi.spyOn(api.proxy, 'getStatus').mockResolvedValue(running);
    vi.spyOn(api.server, 'speedTest').mockResolvedValue(receipt('101', { results: { a: 42 }, measurementContext: context }));
    const write = vi.spyOn(useLatencyStore.getState(), 'applyLatencyResults').mockImplementation(() => {});
    const pending = runMobileSpeedTest(['a', 'b'], 'nodes', extra, true);
    await vi.waitFor(() => expect(extra).toHaveBeenCalledOnce());
    if (change === 'stop') useAppStore.setState({ proxyStatus: { ...running, running: false } });
    if (change === 'generation') useAppStore.setState({ proxyStatus: { ...running, mainGeneration: 3 } });
    if (change === 'start time') useAppStore.setState({ proxyStatus: { ...running, startTime: 456 } });
    if (change === 'configuration') useAppStore.setState({ config: { ...useAppStore.getState().config!, mixedPort: 7891 } });
    if (change === 'selection') useAppStore.setState({ selectedServerId: 'b' });
    if (change === 'membership') useAppStore.setState({ servers: [node] });
    if (change === 'new task') store.getState().progress({ runId: '102', tested: 1, total: 2, ok: 1, measurementContext: { ...context, runId: '102', requestId: 'request-102' } });
    route.resolve(true);
    await pending;
    expect(write).not.toHaveBeenCalled();
    expect(store.getState().task?.appliedResults).toEqual({});
  });

  it('keeps iOS no-context refusal and other-platform legacy fallback without a status read', async () => {
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockResolvedValue(running);
    const extra = vi.fn().mockResolvedValue(true);
    vi.stubGlobal('document', { documentElement: { dataset: { mobileOs: 'ios' } } });
    try {
      const ios = start(extra);
      expect(await ios.guard()).toBe(false);
      expect(extra).not.toHaveBeenCalled();
      await ios.finish();
      document.documentElement.dataset.mobileOs = 'android';
      const legacy = start(extra);
      expect(await legacy.guard()).toBe(true);
      expect(extra).toHaveBeenCalledOnce();
      expect(getStatus).not.toHaveBeenCalled();
      await legacy.finish();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('one concurrent status read serves burst frames and receipt without duplicate commits', async () => {
    let result!: Parameters<typeof api.server.onSpeedTestResult>[0];
    let progress!: Parameters<typeof api.server.onSpeedTestProgress>[0];
    vi.spyOn(api.server, 'onSpeedTestProgress').mockImplementation(handler => { progress = handler; return () => {}; });
    vi.spyOn(api.server, 'onSpeedTestResult').mockImplementation(handler => { result = handler; return () => {}; });
    vi.spyOn(api.server, 'onSpeedTestDone').mockReturnValue(() => {});
    const read = deferred<typeof running>();
    const getStatus = vi.spyOn(api.proxy, 'getStatus').mockReturnValue(read.promise);
    const write = vi.spyOn(useLatencyStore.getState(), 'applyLatencyResults').mockImplementation(() => {});
    const dispose = subscribeMobileSpeedTestProgress();
    const request = start();
    result({ runId: '101', serverId: 'a', latency: 12, measurementContext: context });
    result({ runId: '101', serverId: 'b', latency: 24, measurementContext: context });
    progress({ runId: '101', tested: 2, total: 2, ok: 2, measurementContext: context });
    request.backend.resolve(receipt('101', { results: { a: 12, b: 24 }, measurementContext: context }));
    await vi.waitFor(() => expect(getStatus).toHaveBeenCalledOnce());
    read.resolve(running);
    await request.pending;
    expect(getStatus).toHaveBeenCalledOnce();
    expect(store.getState().task?.appliedResults).toEqual({ a: 12, b: 24 });
    const writes = write.mock.calls.flatMap(([values]) => Object.entries(values));
    expect(writes.filter(([id]) => id === 'a')).toEqual([['a', 12]]);
    expect(writes.filter(([id]) => id === 'b')).toEqual([['b', 24]]);
    dispose();
  });
});

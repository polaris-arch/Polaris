/**
 * 测速结果（`serverId → 延迟 ms`）的**单一真值** store。
 *
 * # 为什么必须是 store 而不是组件 useState
 *
 * 迁移期这份 map 曾以**四份组件私有 `useState`** 存在（HomeScreen / NodesScreen / StatusBar / TrayMenu），
 * 各自在自己的 `useEffect` 里订 `onSpeedTestResult`。后果是订阅寿命 = 组件挂载寿命：
 * `ScreenRouter` 是裸 `switch`（无 keep-alive）⇒ **切屏即卸载即丢结果**，切回来一片空白；
 * 且测速期间切走，流式结果无人接收。
 *
 * 上游 明确修过同一个 bug —— `use-native-events.ts:371-374`：
 * >「原 per-node 监听写在 handler 作用域内…提到顶层持久 hook 后跨页不丢」
 *
 * 故本 store 的契约有两条，缺一条都会退回原状：
 *  1. 状态**存在组件外**（本 store）；
 *  2. 订阅**挂在窗口级持久位置**（主窗 `App.tsx`、托盘窗 `TrayMenu` 根），经 [`subscribeLatencyEvents`]。
 *
 * # 不持久化（会话内存态）
 *
 * 延迟是**测量数据**不是偏好：重启后旧值不再代表当前网络，留着即误导。
 * 对比 `use-node-sort-store`（排序开关=偏好 → localStorage 持久），二者刻意不同，勿照抄。
 *
 * # 托盘窗是**独立 JS 堆**（别指望跨窗共享）
 *
 * `tray.html` 是独立 Vite 入口 / 独立 webview ⇒ 与主窗同源（localStorage 共享）但**不共享 JS 堆**，
 * 本 store 在托盘窗里是**另一个实例**。这不是缺陷：托盘自订同一事件流，两窗各自收敛到同一后端真值
 * （事件由 `commands/speedtest.rs` 向全部窗口广播）。跨窗共享内存态需 localStorage/IPC，
 * 而延迟按上一节的理由不该落 localStorage。
 */
import { create } from 'zustand';
import { api } from '@/ipc';
import { useAppStore } from './app-store';
import { configBaseVersion, configContentDigest } from '@/lib/staged-config';
import type { ServerConfig } from '@/contracts/types';
import type { MeasurementProjection, MeasurementSnapshot } from '@/contracts/measurement-display';
import { projectMeasurement } from '@/domain/measurement-display';
import { validMeasurementSnapshot } from '@/domain/measurement-snapshot';

/**
 * 「延迟已陈旧」阈值（契约「状态栏」：`当前节点延迟(陈旧>30min 半透明)`）。
 *
 * 为什么必须配一份 `testedAt` 而不是只存数值：延迟是**有保质期的测量**，30 分钟前测出的 42ms 与刚测出的
 * 42ms 在屏幕上一模一样，但对用户的行动含义完全不同（后者可信、前者该重测）。没有时间戳就无从分辨，
 * 半透明这条契约也就无数据基础。
 */
export const LATENCY_STALE_MS = 30 * 60 * 1000;

/** 该节点的延迟是否已陈旧（无时间戳=从未测过 → 不算陈旧，由「—」自己表达「没有值」）。 */
export function isLatencyStale(testedAt: number | undefined, now: number = Date.now()): boolean {
  if (testedAt === undefined) return false;
  const elapsed = now - testedAt;
  // testedAt 是跨组件共享的 epoch 戳，不能换 performance.now；墙钟回拨时宁可提示重测，不能让旧值
  // 在时钟追上之前永远保持“新鲜”。
  return elapsed < 0 || elapsed > LATENCY_STALE_MS;
}

interface LatencyState {
  /** Compatibility numeric view: only current candidate-path ledger values. */
  latencyMap: Record<string, number | null>;
  testedAt: Record<string, number>;
  projections: Record<string, MeasurementProjection>;
  currentProjections: Record<string, MeasurementProjection>;
  snapshot: MeasurementSnapshot | null;
  readError: boolean;
  /** Old callers only request authoritative refresh; their bare numbers never enter the store. */
  applyLatencyResult: (serverId: string, latency: number | null) => void;
  applyLatencyResults: (results: Record<string, number | null>) => void;
  retainServerIds: (serverIds: readonly string[]) => void;
  applySnapshot: (snapshot: MeasurementSnapshot, servers: readonly ServerConfig[], now?: number) => void;
  invalidate: (readFailed?: boolean) => void;
}

/**
 * IPC 用 `-1` 表示「真测了但失败」，渲染域统一用 `null` 表示同一语义。
 *
 * 归一化必须收在 store 的两个写入口，而不是让四个消费方各自记得处理：漏掉 NodeCard 的旧形态会把
 * `-1` 同时渲染成绿色（`latLevel(-1) < 80`）和文字 “-1 ms”，正是用户看到的矛盾状态。
 */
export function normalizeLatencyResult(latency: number | null): number | null {
  return latency === null || !Number.isFinite(latency) || latency < 0 ? null : latency;
}

let requestLedgerRefresh: (() => void) | undefined;
/** Actual invoke failures may have published qualified omissions without a result event. */
export function refreshMeasurementLedger(): void { requestLedgerRefresh?.(); }

function stableMap<T>(previous: Record<string, T>, next: Record<string, T>): Record<string, T> {
  const keys = Object.keys(next);
  return keys.length === Object.keys(previous).length && keys.every(key => Object.is(previous[key], next[key])) ? previous : next;
}

export const useLatencyStore = create<LatencyState>((set) => ({
  latencyMap: {}, testedAt: {}, projections: {}, currentProjections: {}, snapshot: null, readError: false,
  applyLatencyResult: () => requestLedgerRefresh?.(),
  applyLatencyResults: () => requestLedgerRefresh?.(),
  applySnapshot: (snapshot, servers, now = snapshot.now) => set(previous => {
    if (!validMeasurementSnapshot(snapshot) || !snapshot.consistent) return previous;
    if (previous.snapshot?.process === snapshot.process && BigInt(previous.snapshot.revision) > BigInt(snapshot.revision)) return previous;
    const candidate: Array<[string, MeasurementProjection]> = [];
    const current: Array<[string, MeasurementProjection]> = [];
    const values: Array<[string, number | null]> = [];
    const stamps: Array<[string, number]> = [];
    for (const server of servers) {
      if (!Object.prototype.hasOwnProperty.call(snapshot.rows, server.id)) continue;
      const project = (purpose: 'candidate' | 'current', old: Record<string, MeasurementProjection>) => {
        const value = projectMeasurement(snapshot, server, now, purpose);
        return JSON.stringify(old[server.id]) === JSON.stringify(value) ? old[server.id] : value;
      };
      const value = project('candidate', previous.projections);
      candidate.push([server.id, value]); current.push([server.id, project('current', previous.currentProjections)]);
      if (value.state === 'success' && value.latency !== undefined && !value.bindingMismatch) values.push([server.id, value.latency]);
      else if (value.state === 'failed') values.push([server.id, null]);
      if (value.measuredAt !== undefined) stamps.push([server.id, value.measuredAt]);
    }
    return { snapshot, readError: false, projections: stableMap(previous.projections, Object.fromEntries(candidate)),
      currentProjections: stableMap(previous.currentProjections, Object.fromEntries(current)),
      latencyMap: stableMap(previous.latencyMap, Object.fromEntries(values)), testedAt: stableMap(previous.testedAt, Object.fromEntries(stamps)) };
  }),
  invalidate: (readFailed = false) => set(previous => {
    const expire = (values: Record<string, MeasurementProjection>) => Object.fromEntries(Object.entries(values)
      .map(([id, value]) => [id, value.state === 'success' || value.state === 'failed'
        ? { ...value, state: 'expired' as const, latency: undefined } : value]));
    return { snapshot: null, readError: readFailed, latencyMap: {}, projections: expire(previous.projections), currentProjections: expire(previous.currentProjections) };
  }),
  retainServerIds: ids => set(previous => {
    const keep = new Set(ids);
    const retain = <T,>(values: Record<string, T>): Record<string, T> =>
      stableMap(values, Object.fromEntries(Object.entries(values).filter(([id]) => keep.has(id))));
    const next = { latencyMap: retain(previous.latencyMap), testedAt: retain(previous.testedAt),
      projections: retain(previous.projections), currentProjections: retain(previous.currentProjections) };
    return next.latencyMap === previous.latencyMap && next.testedAt === previous.testedAt
      && next.projections === previous.projections && next.currentProjections === previous.currentProjections ? previous : next;
  }),
}));

/**
 * 「这一刻不读延迟」时选择器返回的**模块级冻结哨兵**。
 *
 * # 为什么必须是模块级常量，而不是 `{}` 字面量或 `useMemo`
 *
 * zustand 默认用 `Object.is` 比较选择器的返回值来决定「要不要让这个组件重渲」。选择器里写
 * `: {}` 每次求值都是新对象 ⇒ 每一次 store 提交（测速逐节点回包，一轮几十上百次）都判不等 ⇒
 * 订阅方照样每次提交重渲、派生 memo 照样整表重算 —— 与不做条件订阅**逐字等价**，但多骗了一次
 * code review。`useMemo` 也不行：hook 只在组件里能调，而这里要的是「跨组件实例、跨渲染恒等」
 * 的一个引用，只有模块级常量给得了。
 *
 * 冻结是为了让「被误写一格」当场炸：一张假表喂进排序比较器，症状是排序悄悄用了空数据。
 */
export const EMPTY_LATENCY_MAP: Readonly<Record<string, number | null>> = Object.freeze({});

/**
 * 条件订阅整张延迟表的选择器：`needed` 为假时恒返回 [`EMPTY_LATENCY_MAP`]。
 *
 * 两个消费方，同一条判据（各自的「这一刻有没有人读延迟」不同，但形态必须只有一份）：
 *  - 节点页：`sortKey === 'lat'` 才读（按延迟排序时顺序必须随每次回包同步重排）；
 *  - 首页：出口选单展开才读（`NodeMenu` 关着时第一行就 `return <div hidden />`，一个字都没人读）。
 *
 * 反过来「恒不订、要用时 `getState()` 现取」不成立于上面两档：那时**必须**随回包重渲，否则
 * 顺序/数值停在旧帧。条件订阅要收掉的是「订了也没人读」的那部分重渲，不是重渲本身。
 */
export function latencyMapWhen(
  needed: boolean
): (s: { latencyMap: Record<string, number | null> }) => Readonly<Record<string, number | null>> {
  return (s) => (needed ? s.latencyMap : EMPTY_LATENCY_MAP);
}

/**
 * 把 `onSpeedTestResult` 事件流接到本 store —— **窗口级持久订阅**，返回退订函数。
 *
 * 调用点必须是该窗口生命周期内**只挂载一次**的根（主窗 `App.tsx` / 托盘窗 `TrayMenu`）。
 * 挂到业务组件里即退回「切屏即丢」（见本文件顶部）。
 */
/** One shared, reference-counted feed per JS window. Events are invalidation signals;
 * only a same-context readback may publish numeric data. No persistent results or probes. */
export function subscribeLatencyEvents(options: { poll?: boolean } = {}): () => void {
  return acquireMeasurementFeed(options.poll !== false);
}

let feedUsers = 0;
let stopFeed: (() => void) | undefined;
function acquireMeasurementFeed(poll: boolean): () => void {
  if (feedUsers++ === 0) stopFeed = startMeasurementFeed(poll);
  let stopped = false;
  return () => { if (!stopped) { stopped = true; if (--feedUsers === 0) { stopFeed?.(); stopFeed = undefined; } } };
}

function startMeasurementFeed(pollEnabled: boolean): () => void {
  useLatencyStore.getState().invalidate();
  let alive = true, inFlight = false, pending = false, epoch = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let snapshotMono = 0;
  let snapshotWall = 0;
  let hashInput: unknown;
  let hashPromise: Promise<string> | undefined;
  let nextExpiry = Number.POSITIVE_INFINITY;
  const offs: Array<() => void> = [];
  const refresh = async () => {
    if (!alive) return;
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    if (inFlight) { pending = true; return; }
    const state = useAppStore.getState();
    if (!state.config) return;
    inFlight = true;
    const ticket = epoch;
    const version = configBaseVersion(state.config);
    const context = connectionKey(state.proxyStatus);
    try {
      if (hashInput !== state.config || !hashPromise) {
        hashInput = state.config; hashPromise = configContentDigest(state.config);
      }
      const digest = await hashPromise;
      if (!alive || ticket !== epoch) return;
      const result = await api.server.speedTestResultSnapshot();
      const current = useAppStore.getState();
      if (!alive || ticket !== epoch || !current.config || configBaseVersion(current.config) !== version
        || connectionKey(current.proxyStatus) !== context) return;
      if (!validMeasurementSnapshot(result)
        || result.configVersion !== version || result.frontendConfigDigest !== digest || !result.consistent
        || current.proxyStatus?.running === true && (result.generation !== current.proxyStatus.mainGeneration
          || result.startTime !== (current.proxyStatus.startTime ?? null))
        || current.proxyStatus?.running === false && result.generation !== null) {
        useLatencyStore.getState().invalidate(true); return;
      }
      const prior = useLatencyStore.getState().snapshot;
      if (prior?.process === result.process && BigInt(prior.revision) > BigInt(result.revision)) return;
      useLatencyStore.getState().applySnapshot(result, current.config.servers);
      snapshotMono = performance.now(); snapshotWall = Date.now();
      nextExpiry = earliestExpiry(result, result.now);
    } catch {
      hashPromise = undefined;
      if (alive && ticket === epoch) useLatencyStore.getState().invalidate(true);
    } finally {
      inFlight = false;
      if (alive && pending) { pending = false; schedule(); }
    }
  };
  const schedule = () => {
    if (!alive || timer !== undefined) return;
    timer = setTimeout(() => { timer = undefined; void refresh(); }, 200);
  };
  requestLedgerRefresh = schedule;
  const register = (listen: () => () => void) => {
    try { offs.push(listen()); } catch { useLatencyStore.getState().invalidate(); }
  };
  register(() => api.server.onSpeedTestResult(schedule));
  register(() => api.server.onSpeedTestSchedule(schedule));
  const invalidate = () => { epoch += 1; useLatencyStore.getState().invalidate(); schedule(); };
  register(() => api.proxy.onLifecycle(invalidate));
  offs.push(useAppStore.subscribe((next, previous) => {
    if (next.config !== previous.config || connectionKey(next.proxyStatus) !== connectionKey(previous.proxyStatus)) invalidate();
  }));
  // Bounded read-only refresh covers backend network/foreground changes even without a
  // result event. Stable projections preserve per-node references between unchanged reads.
  const poll = pollEnabled ? setInterval(schedule, 5000) : undefined;
  const clock = setInterval(() => {
    const result = useLatencyStore.getState().snapshot;
    const config = useAppStore.getState().config;
    if (!alive || !result || !config) return;
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    const elapsed = performance.now() - snapshotMono;
    if (Date.now() < snapshotWall || Math.abs(Date.now() - snapshotWall - elapsed) > 5000) {
      invalidate(); return;
    }
    const now = result.now + elapsed;
    if (now > nextExpiry) {
      useLatencyStore.getState().applySnapshot(result, config.servers, now);
      nextExpiry = earliestExpiry(result, now);
    }
  }, 1000);
  const onVisible = () => { if (document.visibilityState === 'visible') invalidate(); };
  if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
    document.addEventListener('visibilitychange', onVisible);
    offs.push(() => document.removeEventListener('visibilitychange', onVisible));
  }
  void refresh();
  return () => { alive = false; epoch += 1; clearTimeout(timer); clearInterval(poll); clearInterval(clock); requestLedgerRefresh = undefined; offs.forEach(off => off()); };
}

function earliestExpiry(snapshot: MeasurementSnapshot, now: number): number {
  let next = Number.POSITIVE_INFINITY;
  for (const row of Object.values(snapshot.rows)) for (const purpose of ['candidate','system','selected'] as const) {
    const record = row[purpose];
    if (record && record.state !== 'expired' && record.expiresAt > now) next = Math.min(next, record.expiresAt);
  }
  return next;
}

function connectionKey(status: ReturnType<typeof useAppStore.getState>['proxyStatus']): string {
  return `${status?.running === true}|${status?.mainGeneration ?? ''}|${status?.startTime ?? ''}`;
}

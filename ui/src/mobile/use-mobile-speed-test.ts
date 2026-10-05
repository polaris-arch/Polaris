import { create } from 'zustand';
import { api } from '@/ipc';
import { isSpeedTestCountProgress, type SpeedTestDonePayload, type SpeedTestInvokeResult, type SpeedTestMeasurementContext, type SpeedTestProgressPayload, type SpeedTestResultPayload } from '@/contracts/speed-test';
import { useLatencyStore } from '@/store/use-latency-store';
import { getEffectiveConfig, useAppStore } from '@/store/app-store';

export type MobileSpeedTestPhase = 'preparingConnection' | 'waitingForReady' | 'running' | 'waiting' | 'completed' | 'interrupted' | 'failed';
type LatencyGuard = (context?: SpeedTestMeasurementContext | null) => boolean | Promise<boolean>;
export interface MobileSpeedTestTask {
  runId: string;
  phase: MobileSpeedTestPhase;
  tested: number;
  total: number | null;
  ok: number | null;
  results: Record<string, number>;
  appliedResults: Record<string, number>;
  measurementContext?: SpeedTestMeasurementContext;
  pending: number | null;
  skipped: number | null;
  /** Current-exit attribution remains guarded even after its screen unmounts. */
  acceptLatency?: LatencyGuard;
}
interface PendingRequest { token: number; kind: 'current' | 'all' | 'nodes'; ids: readonly string[]; lastRunId: string | null; acceptLatency?: LatencyGuard }
interface State {
  task: MobileSpeedTestTask | null;
  request: PendingRequest | null;
  nextToken: number;
  begin: (ids: readonly string[], kind: PendingRequest['kind'], acceptLatency?: LatencyGuard) => number | null;
  progress: (event: SpeedTestProgressPayload) => void;
  result: (event: SpeedTestResultPayload) => void;
  done: (event: SpeedTestDonePayload) => void;
  settle: (token: number, result?: SpeedTestInvokeResult) => boolean;
  waiting: (runId: string) => void;
}
export function validSpeedTestRunId(value: unknown): value is string {
  return typeof value === 'string' && /^[1-9]\d*$/.test(value);
}
export function compareSpeedTestRunIds(a: string, b: string): number {
  return a.length === b.length ? (a === b ? 0 : a > b ? 1 : -1) : a.length > b.length ? 1 : -1;
}
const active = (task: MobileSpeedTestTask | null): boolean => !!task && ['preparingConnection', 'waitingForReady', 'running', 'waiting'].includes(task.phase);
function taskFor(current: MobileSpeedTestTask | null, runId: unknown, request?: PendingRequest | null): MobileSpeedTestTask | null {
  if (!validSpeedTestRunId(runId) || (current && compareSpeedTestRunIds(runId, current.runId) < 0)) return null;
  return current?.runId === runId ? current : { runId, phase: 'running', tested: 0, total: null, ok: null, results: {}, appliedResults: {}, pending: null, skipped: null, acceptLatency: request?.acceptLatency };
}
function withMeasurementContext(task: MobileSpeedTestTask, context?: SpeedTestMeasurementContext | null): MobileSpeedTestTask | null {
  if (!context) return task.measurementContext ? null : task;
  if (context.runId !== task.runId || !context.requestId || !Number.isSafeInteger(context.mainGeneration)
    || context.mainGeneration < 0 || (context.startTime !== null && !Number.isFinite(context.startTime))) return null;
  const bound = task.measurementContext;
  if (bound && (bound.runId !== context.runId || bound.requestId !== context.requestId
    || bound.mainGeneration !== context.mainGeneration || bound.startTime !== context.startTime)) return null;
  return task.measurementContext ? task : { ...task, measurementContext: context };
}
const count = (value: number): boolean => Number.isSafeInteger(value) && value >= 0;
export const useMobileSpeedTestStore = create<State>((set, get) => ({
  task: null, request: null, nextToken: 0,
  begin: (ids, kind, acceptLatency) => {
    const state = get();
    if (state.request || (active(state.task) && state.task?.phase !== 'waiting')) return null;
    const token = state.nextToken + 1;
    set({ nextToken: token, request: { token, kind, ids: [...ids], lastRunId: state.task?.runId ?? null, acceptLatency } });
    return token;
  },
  progress: event => {
    let task = taskFor(get().task, event.runId, get().request);
    if (!task) return;
    if (!isSpeedTestCountProgress(event)) {
      if (!active(task)) return;
      if (event.phase === 'measuring') {
        task = withMeasurementContext(task, event.measurementContext);
        if (task) set({ task: { ...task, phase: 'running' } });
      } else if (!task.measurementContext && task.total === null) {
        set({ task: { ...task, phase: event.phase } });
      }
      return;
    }
    task = withMeasurementContext(task, event.measurementContext);
    if (!task || !count(event.tested) || !count(event.total) || !count(event.ok) || event.total < 1 || event.tested > event.total || event.ok > event.tested) return;
    if (task.total !== null && task.total !== event.total) return;
    if (!active(task)) {
      // A final progress frame may arrive after DONE; it may supply success counts, never reopen the task.
      if (event.tested === task.tested) set({ task: { ...task, ok: event.ok } });
      return;
    }
    if (event.tested < task.tested) return;
    set({ task: { ...task, phase: 'running', tested: event.tested, ok: event.ok, total: event.total } });
  },
  result: event => {
    const candidate = taskFor(get().task, event.runId, get().request);
    const task = candidate && withMeasurementContext(candidate, event.measurementContext);
    if (!task || !event.serverId || !Number.isFinite(event.latency) || task.results[event.serverId] === event.latency) return;
    const results = { ...task.results, [event.serverId]: event.latency };
    const known = Object.values(results);
    set({ task: { ...task, results, ok: task.total !== null && known.length === task.tested ? known.filter(value => value >= 0).length : task.ok } });
  },
  done: event => {
    const candidate = taskFor(get().task, event.runId, get().request);
    const task = candidate && withMeasurementContext(candidate, event.measurementContext);
    if (!task || !count(event.tested) || !count(event.total) || event.tested > event.total || !['completed', 'interrupted'].includes(event.outcome)) return;
    if (!active(task)) return; // duplicate DONE cannot change a terminal receipt
    const known = Object.values(task.results);
    set({ task: { ...task, phase: event.outcome, tested: event.tested, total: event.total,
      ok: known.length === event.tested ? known.filter(value => value >= 0).length : task.tested === event.tested ? task.ok : null,
      pending: event.pending.length } });
  },
  settle: (token, receipt) => {
    const current = get();
    if (current.request?.token !== token) return false;
    set({ request: null }); // clear this local invocation only, including early failures with no run identity
    if (!receipt) {
      if (current.task && ['preparingConnection', 'waitingForReady'].includes(current.task.phase)) {
        set({ task: { ...current.task, phase: 'failed' } });
      }
      return false;
    }
    const candidate = taskFor(current.task, receipt.runId, current.request);
    const task = candidate && withMeasurementContext(candidate, receipt.measurementContext);
    if (!task) return false;
    const results = receipt.results;
    const tested = Object.keys(results).length;
    const skipped = new Set([...receipt.notInPool, ...receipt.tsNotReady, ...(receipt.dirty ?? [])]).size;
    const eligible = Math.max(0, current.request.ids.length - skipped);
    set({ task: { ...task, phase: receipt.outcome, tested, total: task.total ?? eligible,
      ok: Object.values(results).filter(value => value >= 0).length, results,
      pending: task.pending ?? Math.max(0, eligible - tested), skipped } });
    return true;
  },
  waiting: runId => {
    const task = get().task;
    if (task?.runId === runId && task.phase === 'running') set({ task: { ...task, phase: 'waiting' } });
  },
}));

async function verifyMeasurementContext(context: SpeedTestMeasurementContext): Promise<boolean> {
  const before = useAppStore.getState().proxyStatus;
  try {
    const status = await api.proxy.getStatus();
    const current = useAppStore.getState().proxyStatus;
    if (current !== before && (!current?.running || current.mainGeneration !== context.mainGeneration
      || (current.startTime ?? null) !== context.startTime)) return false;
    if (!status.running || status.mainGeneration !== context.mainGeneration
      || (status.startTime ?? null) !== context.startTime) return false;
    useAppStore.setState({ proxyStatus: status });
    return true;
  } catch {
    return false;
  }
}

/** Capture user intent now; the connection identity comes only from the later ready producer. */
function latencyGuard(ids: readonly string[], kind: PendingRequest['kind'], extra?: LatencyGuard): LatencyGuard {
  const config = JSON.stringify(getEffectiveConfig());
  const selectedId = useAppStore.getState().selectedServerId;
  const connection = () => {
    const status = useAppStore.getState().proxyStatus;
    return `${status?.running === true}|${status?.startTime ?? 0}`;
  };
  const legacyConnection = connection();
  const sameSelection = () => useAppStore.getState().selectedServerId === selectedId;
  const stillConfigured = () => JSON.stringify(getEffectiveConfig()) === config
    && ids.every(id => useAppStore.getState().servers.some(server => server.id === id));
  return async context => {
    if (context) {
      if (!sameSelection() || !stillConfigured() || !await verifyMeasurementContext(context)) return false;
      if (!sameSelection() || !stillConfigured()) return false;
    } else if (typeof document !== 'undefined' && document.documentElement.dataset.mobileOs === 'ios') {
      return false;
    } else if (kind === 'current' && (!sameSelection() || connection() !== legacyConnection)) {
      return false;
    }
    return extra ? extra(context) : true;
  };
}

/** Store received results separately from applied latency; a later receipt can fill an earlier withheld write. */
async function applyTaskLatency(runId: string, results: Record<string, number>): Promise<void> {
  const before = useMobileSpeedTestStore.getState().task;
  if (!before || before.runId !== runId) return;
  if (!Object.entries(results).some(([id, value]) => before.results[id] === value && before.appliedResults[id] !== value)) return;
  const context = before.measurementContext;
  const allowed = before.acceptLatency ? await before.acceptLatency(context)
    : context ? await verifyMeasurementContext(context) : true;
  const task = useMobileSpeedTestStore.getState().task;
  if (!allowed || !task || task.runId !== runId || task.measurementContext !== context) return;
  const missing = Object.fromEntries(Object.entries(results).filter(([id, value]) =>
    task.results[id] === value && task.appliedResults[id] !== value));
  if (Object.keys(missing).length === 0) return;
  useLatencyStore.getState().applyLatencyResults(missing);
  useMobileSpeedTestStore.setState({ task: { ...task, appliedResults: { ...task.appliedResults, ...missing } } });
}

/** Window lifetime feed: no timer changes a count or claims a terminal outcome. */
export function subscribeMobileSpeedTestProgress(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = () => {
    clearTimeout(timer);
    const task = useMobileSpeedTestStore.getState().task;
    if (!task || task.phase !== 'running') return;
    timer = setTimeout(() => useMobileSpeedTestStore.getState().waiting(task.runId), 20_000);
  };
  const offs = [
    api.server.onSpeedTestProgress(event => {
      const before = useMobileSpeedTestStore.getState().task;
      useMobileSpeedTestStore.getState().progress(event);
      const after = useMobileSpeedTestStore.getState().task;
      if (before !== after && after && after.runId === event.runId) {
        if (after.measurementContext) void applyTaskLatency(after.runId, after.results);
        arm();
      }
    }),
    api.server.onSpeedTestResult(event => {
      // Unidentified warm RTT is a passive latency result, never an active task frame.
      if (event.runId === undefined) {
        useLatencyStore.getState().applyLatencyResult(event.serverId, event.latency);
        return;
      }
      const before = useMobileSpeedTestStore.getState().task;
      useMobileSpeedTestStore.getState().result(event);
      const after = useMobileSpeedTestStore.getState().task;
      if (before !== after && after && after.runId === event.runId) {
        void applyTaskLatency(after.runId, { [event.serverId]: event.latency });
        arm();
      }
    }),
    api.server.onSpeedTestDone(event => {
      const before = useMobileSpeedTestStore.getState().task;
      useMobileSpeedTestStore.getState().done(event);
      const after = useMobileSpeedTestStore.getState().task;
      if (before !== after && after && after.runId === event.runId) arm();
    }),
  ];
  return () => { clearTimeout(timer); offs.forEach(off => off()); };
}

/** One local request lock across screens; a late invocation cannot replace an observed newer run. */
export async function runMobileSpeedTest(ids: readonly string[], kind: PendingRequest['kind'], acceptLatency?: LatencyGuard): Promise<SpeedTestInvokeResult | null> {
  const token = useMobileSpeedTestStore.getState().begin(ids, kind, latencyGuard(ids, kind, acceptLatency));
  if (token === null) return null;
  try {
    const receipt = await api.server.speedTest([...ids]);
    const accepted = useMobileSpeedTestStore.getState().settle(token, receipt);
    if (accepted) {
      const task = useMobileSpeedTestStore.getState().task!;
      await applyTaskLatency(task.runId, receipt.results);
    }
    return accepted ? receipt : null;
  } catch (cause) {
    useMobileSpeedTestStore.getState().settle(token);
    throw cause;
  }
}

export const mobileSpeedTestBusy = (state: Pick<State, 'task' | 'request'>): boolean => state.request !== null || (active(state.task) && state.task?.phase !== 'waiting');

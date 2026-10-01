import { create } from 'zustand';
import { api } from '@/ipc';
import type { SpeedTestDonePayload, SpeedTestInvokeResult } from '@/contracts/speed-test';
import { useLatencyStore } from '@/store/use-latency-store';

export type MobileSpeedTestPhase = 'running' | 'waiting' | 'completed' | 'interrupted';
export interface MobileSpeedTestTask {
  runId: string;
  phase: MobileSpeedTestPhase;
  tested: number;
  total: number | null;
  ok: number | null;
  results: Record<string, number>;
  pending: number | null;
  skipped: number | null;
  /** Current-exit attribution remains guarded even after its screen unmounts. */
  acceptLatency?: () => boolean;
}
interface PendingRequest { token: number; kind: 'current' | 'all' | 'nodes'; ids: readonly string[]; lastRunId: string | null; acceptLatency?: () => boolean }
interface State {
  task: MobileSpeedTestTask | null;
  request: PendingRequest | null;
  nextToken: number;
  begin: (ids: readonly string[], kind: PendingRequest['kind'], acceptLatency?: () => boolean) => number | null;
  progress: (event: { runId?: string; tested: number; ok: number; total: number }) => void;
  result: (event: { runId?: string; serverId: string; latency: number }) => void;
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
const active = (task: MobileSpeedTestTask | null): boolean => task?.phase === 'running' || task?.phase === 'waiting';
function taskFor(current: MobileSpeedTestTask | null, runId: unknown, request?: PendingRequest | null): MobileSpeedTestTask | null {
  if (!validSpeedTestRunId(runId) || (current && compareSpeedTestRunIds(runId, current.runId) < 0)) return null;
  return current?.runId === runId ? current : { runId, phase: 'running', tested: 0, total: null, ok: null, results: {}, pending: null, skipped: null, acceptLatency: request?.kind === 'current' ? request.acceptLatency : undefined };
}
const count = (value: number): boolean => Number.isSafeInteger(value) && value >= 0;
export const useMobileSpeedTestStore = create<State>((set, get) => ({
  task: null, request: null, nextToken: 0,
  begin: (ids, kind, acceptLatency) => {
    const state = get();
    if (state.request || state.task?.phase === 'running') return null;
    const token = state.nextToken + 1;
    set({ nextToken: token, request: { token, kind, ids: [...ids], lastRunId: state.task?.runId ?? null, acceptLatency } });
    return token;
  },
  progress: event => {
    const task = taskFor(get().task, event.runId, get().request);
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
    const task = taskFor(get().task, event.runId, get().request);
    if (!task || !event.serverId || !Number.isFinite(event.latency) || task.results[event.serverId] === event.latency) return;
    const results = { ...task.results, [event.serverId]: event.latency };
    const known = Object.values(results);
    set({ task: { ...task, results, ok: task.total !== null && known.length === task.tested ? known.filter(value => value >= 0).length : task.ok } });
  },
  done: event => {
    const task = taskFor(get().task, event.runId, get().request);
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
    if (!receipt) return false;
    const task = taskFor(current.task, receipt.runId, current.request);
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
    if (task?.runId === runId && active(task)) set({ task: { ...task, phase: 'waiting' } });
  },
}));

/** Window lifetime feed: no timer changes a count or claims a terminal outcome. */
export function subscribeMobileSpeedTestProgress(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = () => {
    clearTimeout(timer);
    const task = useMobileSpeedTestStore.getState().task;
    if (!task || !active(task)) return;
    timer = setTimeout(() => useMobileSpeedTestStore.getState().waiting(task.runId), 20_000);
  };
  const offs = [
    api.server.onSpeedTestProgress(event => {
      const before = useMobileSpeedTestStore.getState().task;
      useMobileSpeedTestStore.getState().progress(event);
      const after = useMobileSpeedTestStore.getState().task;
      if (before !== after && after?.runId === event.runId) arm();
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
      if (before !== after && after?.runId === event.runId) {
        if (!after.acceptLatency || after.acceptLatency()) useLatencyStore.getState().applyLatencyResult(event.serverId, event.latency);
        arm();
      }
    }),
    api.server.onSpeedTestDone(event => {
      const before = useMobileSpeedTestStore.getState().task;
      useMobileSpeedTestStore.getState().done(event);
      const after = useMobileSpeedTestStore.getState().task;
      if (before !== after && after?.runId === event.runId) arm();
    }),
  ];
  return () => { clearTimeout(timer); offs.forEach(off => off()); };
}

/** One local request lock across screens; a late invocation cannot replace an observed newer run. */
export async function runMobileSpeedTest(ids: readonly string[], kind: PendingRequest['kind'], acceptLatency?: () => boolean): Promise<SpeedTestInvokeResult | null> {
  const token = useMobileSpeedTestStore.getState().begin(ids, kind, acceptLatency);
  if (token === null) return null;
  try {
    const receipt = await api.server.speedTest([...ids]);
    const observed = useMobileSpeedTestStore.getState().task;
    const streamed = observed && observed.runId === receipt.runId ? observed.results : {};
    const accepted = useMobileSpeedTestStore.getState().settle(token, receipt);
    if (accepted) {
      const task = useMobileSpeedTestStore.getState().task!;
      if (!task.acceptLatency || task.acceptLatency()) {
        // RESULT is incremental; the receipt only fills results not already written by that same run.
        const missing = Object.fromEntries(Object.entries(receipt.results).filter(([id, value]) => streamed[id] !== value));
        if (Object.keys(missing).length > 0) useLatencyStore.getState().applyLatencyResults(missing);
      }
    }
    return accepted ? receipt : null;
  } catch (cause) {
    useMobileSpeedTestStore.getState().settle(token);
    throw cause;
  }
}

export const mobileSpeedTestBusy = (state: Pick<State, 'task' | 'request'>): boolean => state.request !== null || state.task?.phase === 'running';

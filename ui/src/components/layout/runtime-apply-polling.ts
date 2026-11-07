import type { NativeRetirementDebt, PendingApplyResult } from '@/contracts/types';

export const RUNTIME_APPLY_POLL_INTERVAL_MS = 2000;

/** Terminal failures need the transaction token, but do not claim runtime ownership. */
export function runtimeApplyPollDisposition(
  result: PendingApplyResult,
  transactionId: string,
  runtimeGeneration: number | undefined,
  selectionIntentGeneration: number | undefined,
): 'failure' | 'owned' | 'ignore' {
  if (result.transactionId !== transactionId) return 'ignore';
  if (result.status === 'conflict' || result.status === 'rejected' || result.status === 'skipped') return 'failure';
  if (runtimeGeneration === undefined || selectionIntentGeneration === undefined
      || result.runtimeGeneration !== runtimeGeneration
      || result.selectionIntentGeneration !== selectionIntentGeneration) return 'ignore';
  return 'owned';
}

/** Native cleanup keeps its original owner even after a different selection or HostOnly intent. */
export function runtimeNativePollDisposition(result: PendingApplyResult, owner: NativeRetirementDebt): 'owned' | 'ignore' {
  return result.transactionId === owner.transactionId && result.runtimeGeneration === owner.runtimeGeneration
    && result.selectionIntentGeneration === owner.selectionIntentGeneration ? 'owned' : 'ignore';
}

export type RuntimeExecutionRef =
  | { kind: 'core'; transactionId: string }
  | { kind: 'host'; hostExecutionId: string };

export function runtimeExecutionMatches(result: PendingApplyResult, execution: RuntimeExecutionRef): boolean {
  return execution.kind === 'core'
    ? result.transactionId === execution.transactionId
    : result.hostExecutionId === execution.hostExecutionId;
}

/** Host replies always require execution + generation + intent, including failures. */
export function runtimeHostPollDisposition(
  result: PendingApplyResult,
  hostExecutionId: string,
  runtimeGeneration: number,
  selectionIntentGeneration: number,
): 'owned' | 'ignore' {
  return result.hostExecutionId === hostExecutionId
    && result.runtimeGeneration === runtimeGeneration
    && result.selectionIntentGeneration === selectionIntentGeneration ? 'owned' : 'ignore';
}

/** One in-flight read per typed execution; observation never replays Apply or OS work. */
export function startRuntimeExecutionPolling(options: {
  execution: RuntimeExecutionRef;
  read: (execution: RuntimeExecutionRef) => Promise<PendingApplyResult>;
  current: () => boolean;
  accept: (result: PendingApplyResult) => boolean;
}): () => void {
  let cancelled = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const current = () => !cancelled && options.current();
  const schedule = () => { timer = setTimeout(() => void poll(), RUNTIME_APPLY_POLL_INTERVAL_MS); };
  const poll = async () => {
    if (!current()) return;
    let result: PendingApplyResult | null = null;
    try { result = await options.read(options.execution); } catch { /* ambiguity retains debt */ }
    if (!current()) return;
    if (result !== null && runtimeExecutionMatches(result, options.execution) && options.accept(result)) return;
    schedule();
  };
  schedule();
  return () => { cancelled = true; if (timer !== undefined) clearTimeout(timer); };
}

/** Backward-compatible Core-only caller; host ids never enter transaction fields. */
export function startRuntimeApplyPolling(options: {
  transactionId: string;
  read: (transactionId: string) => Promise<PendingApplyResult>;
  current: () => boolean;
  accept: (result: PendingApplyResult) => boolean;
}): () => void {
  return startRuntimeExecutionPolling({
    ...options,
    execution: { kind: 'core', transactionId: options.transactionId },
    read: (execution) => {
      if (execution.kind !== 'core') throw new Error('RUNTIME_APPLY_CORE_REFERENCE_REQUIRED');
      return options.read(execution.transactionId);
    },
  });
}

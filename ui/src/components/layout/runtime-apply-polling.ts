import type { PendingApplyResult } from '@/contracts/types';

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

/** One in-flight read, same transaction only, and no Apply replay. */
export function startRuntimeApplyPolling(options: {
  transactionId: string;
  read: (transactionId: string) => Promise<PendingApplyResult>;
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
    try { result = await options.read(options.transactionId); } catch { /* ambiguity is not success */ }
    if (!current()) return;
    if (result !== null && result.transactionId === options.transactionId && options.accept(result)) return;
    schedule();
  };
  schedule();
  return () => { cancelled = true; if (timer !== undefined) clearTimeout(timer); };
}

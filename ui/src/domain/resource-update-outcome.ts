import type { RuleResourceDownloadResult } from '@/contracts/types';

export type ResourceUpdateOutcome = {
  status: 'success' | 'partial' | 'failed' | 'cancelled' | 'empty';
  succeeded: number;
  failed: number;
  cancelled: number;
};

/** IPC 的 TS 类型不能证明运行时收据完整；空、缺项和缺 ok 一律不能报成功。 */
export function resourceUpdateOutcome(
  value: unknown,
  expectedCount: number,
): ResourceUpdateOutcome {
  const rows = Array.isArray(value) ? value as RuleResourceDownloadResult[] : [];
  if (rows.length === 0) {
    return { status: 'empty', succeeded: 0, failed: 0, cancelled: 0 };
  }
  let succeeded = 0;
  let failed = Math.max(0, expectedCount - rows.length);
  let cancelled = 0;
  for (const row of rows) {
    if (row?.ok === true) {
      succeeded++;
    } else if (row?.errorCode === 'RULE_RESOURCE_CANCELLED') {
      cancelled++;
    } else {
      failed++;
    }
  }
  const status = failed === 0 && cancelled === 0
    ? 'success'
    : succeeded > 0
      ? 'partial'
      : failed === 0
        ? 'cancelled'
        : 'failed';
  return { status, succeeded, failed, cancelled };
}

export function resourceUpdateFeedback(
  outcome: ResourceUpdateOutcome,
  t: (key: string, vars?: Record<string, number>) => string,
): { tone: 'success' | 'info' | 'error'; text: string } {
  const vars = { succeeded: outcome.succeeded, failed: outcome.failed, cancelled: outcome.cancelled };
  const key = `resources.updateResult.${outcome.status}`;
  return {
    tone: outcome.status === 'success' ? 'success' : outcome.status === 'cancelled' ? 'info' : 'error',
    text: t(key, vars),
  };
}

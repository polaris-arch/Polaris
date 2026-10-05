import type { IpInfoSnapshot, ProxyStatus } from './types';
import type { UnlockSnapshot } from './unlock-detection';

export interface ManualNetworkCheckContext {
  requestId: string;
  mainGeneration: number;
  startTime?: number;
}

export interface ManualNetworkCheckResult {
  context: ManualNetworkCheckContext;
  ipInfo: { data?: IpInfoSnapshot; error?: { code: string } };
  unlock: { data?: UnlockSnapshot; error?: { code: string } };
}

/** Consume only the requested run, after a fresh status read confirms its ready main still exists. */
export function manualNetworkContextMatches(
  context: ManualNetworkCheckContext,
  requestId: string,
  status: ProxyStatus | null,
): boolean {
  return context.requestId === requestId && Number.isSafeInteger(context.mainGeneration)
    && status?.running === true && status.mainGeneration === context.mainGeneration
    && status.startTime === context.startTime;
}

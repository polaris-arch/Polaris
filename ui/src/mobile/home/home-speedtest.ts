import type { TFunction } from 'i18next';
import type { ServerConfig } from '@/contracts/types';
import type { SpeedTestInvokeResult } from '@/contracts/speed-test';
import { speedTestableIds } from '@/domain/endpoint-routes';
import { stagedOnlyIds } from '@/lib/staged-config';
import { notInPoolMessage } from '@/components/screens/shared/speedtest-feedback';

/** The same full effective-server range and path-aware eligibility as the node page. */
export function planAllHomeSpeedTest(
  servers: readonly ServerConfig[],
  diskServers: readonly ServerConfig[],
  mainCorePool: boolean,
): string[] {
  return speedTestableIds(
    servers,
    { mainCorePool },
    stagedOnlyIds(servers, diskServers),
  );
}

export type HomeSpeedTestFeedback = { tone: 'info'; text: string };

/** Successful completion updates latency in place; only actionable exceptions stay visible. */
export function batchSpeedTestFeedback(
  result: SpeedTestInvokeResult,
  requestedCount: number,
  t: TFunction,
): HomeSpeedTestFeedback | null {
  const tested = Object.keys(result.results).length;
  const skipped = notInPoolMessage(result, t);
  const eligible = Math.max(0, requestedCount - result.notInPool.length - result.tsNotReady.length);
  if (result.outcome === 'completed' && tested > 0 && skipped === null) return null;
  const status = result.outcome === 'interrupted'
    ? t('nodes.speedTestInterruptedSummary', { tested, total: eligible })
    : tested === 0 ? t('nodes.speedTestNotApplicable') : null;
  return {
    tone: 'info',
    text: [status, skipped].filter(Boolean).join(' · '),
  };
}

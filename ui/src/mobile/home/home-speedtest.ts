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

export type HomeSpeedTestFeedback = { tone: 'ok' | 'info'; text: string };

/** A skipped or interrupted request is never described as an all-success result. */
export function batchSpeedTestFeedback(
  result: SpeedTestInvokeResult,
  requestedCount: number,
  t: TFunction,
): HomeSpeedTestFeedback {
  const tested = Object.keys(result.results).length;
  const skipped = notInPoolMessage(result, t);
  const eligible = Math.max(0, requestedCount - result.notInPool.length - result.tsNotReady.length);
  const status = result.outcome === 'interrupted'
    ? t('nodes.speedTestInterruptedSummary', { tested, total: eligible })
    : tested === 0
      ? t('nodes.speedTestNotApplicable')
      : t('mobileHome.speedTestReturned', { count: tested });
  return {
    tone: result.outcome === 'interrupted' || skipped !== null || tested === 0 ? 'info' : 'ok',
    text: [status, skipped].filter(Boolean).join(' · '),
  };
}

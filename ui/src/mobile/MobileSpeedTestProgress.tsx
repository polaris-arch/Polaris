import type { ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { useMobileSpeedTestStore } from './use-mobile-speed-test';

/** Inline measured progress, shared by Home and Nodes; the window feed owns its lifetime. */
export function MobileSpeedTestProgress(): ReactElement | null {
  const { t } = useTranslation();
  const task = useMobileSpeedTestStore(state => state.task);
  const request = useMobileSpeedTestStore(state => state.request);
  const preparing = request !== null;
  if (!task && !preparing) return null;
  const pendingStart = preparing && (!task || task.runId === request?.lastRunId);
  const running = pendingStart || task?.phase === 'running' || task?.phase === 'waiting';
  return <div className="m-speedtest-progress" role="status" aria-live="polite" aria-busy={running}>
    <div className="m-speedtest-line">
      {running && <span className="m-speedtest-spinner" aria-hidden />}
      <span>{pendingStart ? t('mobileSpeedTest.preparing') : task?.phase === 'waiting' ? t('mobileSpeedTest.waiting') : task?.phase === 'interrupted' ? t('nodes.speedTestInterruptedSummary', { tested: task.tested, total: task.total ?? task.tested }) : task?.phase === 'completed' ? t('mobileHome.speedTestReturned', { count: task.tested }) : task?.total === null ? t('mobileHome.latencyChecking') : t('nodes.speedTestingNodes', { tested: task?.tested ?? 0, total: task?.total })}</span>
    </div>
    {!pendingStart && task?.total !== null && task?.total !== undefined && task.total > 0 &&
      <progress className="m-speedtest-bar" value={task.tested} max={task.total} aria-label={t('mobileSpeedTest.progress')} />}
    {!pendingStart && task && task.ok !== null && <span className="m-speedtest-counts">{t('mobileSpeedTest.measuredCounts', { ok: task.ok, failed: Math.max(0, task.tested - task.ok) })}</span>}
    {!pendingStart && task && task.skipped !== null && task.skipped > 0 && <span className="m-speedtest-counts">{t('mobileSpeedTest.skippedCount', { count: task.skipped })}</span>}
    {!pendingStart && task && task.pending !== null && task.pending > 0 && <span className="m-speedtest-counts">{t('mobileSpeedTest.pendingCount', { count: task.pending })}</span>}
  </div>;
}

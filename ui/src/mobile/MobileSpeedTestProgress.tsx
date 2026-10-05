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
  const connecting = task?.phase === 'preparingConnection' || task?.phase === 'waitingForReady';
  const running = pendingStart || connecting || task?.phase === 'running' || task?.phase === 'waiting';
  if (!running) return null;
  return <div className="m-speedtest-progress" role="status" aria-live="polite" aria-busy={running}>
    <div className="m-speedtest-line">
      <span className="m-speedtest-spinner" aria-hidden />
      <span>{connecting ? t(task.phase === 'preparingConnection' ? 'prerequisite.preparingConnection' : 'prerequisite.waitingForReady') : pendingStart ? t('mobileSpeedTest.preparing') : task?.phase === 'waiting' ? t('mobileSpeedTest.waiting') : task?.total === null ? t('mobileHome.latencyChecking') : t('nodes.speedTestingNodes', { tested: task?.tested ?? 0, total: task?.total })}</span>
    </div>
    {!pendingStart && task?.total !== null && task?.total !== undefined && task.total > 0 &&
      <progress className="m-speedtest-bar" value={task.tested} max={task.total} aria-label={t('mobileSpeedTest.progress')} />}
    {!pendingStart && task && task.ok !== null && <span className="m-speedtest-counts">{t('mobileSpeedTest.measuredCounts', { ok: task.ok, failed: Math.max(0, task.tested - task.ok) })}</span>}
    {!pendingStart && task && task.skipped !== null && task.skipped > 0 && <span className="m-speedtest-counts">{t('mobileSpeedTest.skippedCount', { count: task.skipped })}</span>}
    {!pendingStart && task && task.pending !== null && task.pending > 0 && <span className="m-speedtest-counts">{t('mobileSpeedTest.pendingCount', { count: task.pending })}</span>}
  </div>;
}

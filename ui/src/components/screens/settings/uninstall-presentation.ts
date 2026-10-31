import type { UninstallReport } from '@/ipc/api/system';

const META = {
  complete: { key: 'settings.about.uninstallVerdictComplete', tone: 'hsl(var(--ok))' },
  incomplete: { key: 'settings.about.uninstallVerdictIncomplete', tone: 'hsl(var(--warn))' },
  failed: { key: 'settings.about.uninstallVerdictFailed', tone: 'hsl(var(--err))' },
} as const;

/** Keep a successful helper retirement distinct from complete data removal. */
export function uninstallPresentation(report: UninstallReport) {
  const retained = report.steps.some(
    (step) => step.step === 'helper' && step.outcome.helperRemoval === 'retiredDataPreserved',
  );
  const verdict = report.verdict === 'complete' && retained ? 'incomplete' : report.verdict;
  return { ...META[verdict], verdict, mayExitAsComplete: verdict === 'complete' };
}

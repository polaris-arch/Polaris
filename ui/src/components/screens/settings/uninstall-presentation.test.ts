import { describe, expect, it } from 'vitest';
import type { UninstallReport, UninstallStepReport } from '@/ipc/api/system';
import { uninstallPresentation } from './uninstall-presentation';

function report(
  outcome: UninstallStepReport['outcome'],
  verdict: UninstallReport['verdict'],
): UninstallReport {
  // Exercise the actual JSON-shaped projection used by both toast and headline.
  return JSON.parse(JSON.stringify({
    steps: [{ step: 'helper', outcome }], verdict, requiresExit: true,
  }));
}

const retired: UninstallStepReport['outcome'] = {
  kind: 'done', detail: 'protected core/data preserved', helperRemoval: 'retiredDataPreserved',
};

describe('helper maintenance scope in uninstall presentation', () => {
  it('Mac retirement never produces an all-removed headline or automatic exit', () => {
    for (const verdict of ['complete', 'incomplete'] as const) {
      const view = uninstallPresentation(report(retired, verdict));
      expect(view.verdict).toBe('incomplete');
      expect(view.key).toBe('settings.about.uninstallVerdictIncomplete');
      expect(view.mayExitAsComplete).toBe(false);
    }
    expect(uninstallPresentation(report(retired, 'failed')).verdict).toBe('failed');
  });
  it('confirmed Linux/Windows removal remains complete', () => {
    const view = uninstallPresentation(report({
      kind: 'done', detail: 'managed core removed', helperRemoval: 'removed',
    }, 'complete'));
    expect(view.verdict).toBe('complete');
    expect(view.mayExitAsComplete).toBe(true);
  });
  it.each(['cancelled', 'cleanup unknown'])('%s cannot become successful cleanup', (detail) => {
    const view = uninstallPresentation(report({ kind: 'failed', detail }, 'failed'));
    expect(view.verdict).toBe('failed');
    expect(view.mayExitAsComplete).toBe(false);
  });
  it('missing helper with unverified protected data stays incomplete', () => {
    const view = uninstallPresentation(report({
      kind: 'unsupported', detail: 'protected data unverified',
    }, 'incomplete'));
    expect(view.mayExitAsComplete).toBe(false);
  });
});

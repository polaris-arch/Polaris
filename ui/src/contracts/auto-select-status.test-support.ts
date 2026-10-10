import type { AutoSelectStatus } from '@/contracts/auto-select';

export function statusFixture(overrides: Partial<AutoSelectStatus> = {}): AutoSelectStatus {
  return {
    intent: { mode: 'auto', scope: 'subscription', subscriptionId: 'sub-a' },
    exit: { serverId: 'node-a', inSubscription: true },
    mode: 'settled', reason: 'settled', basis: null, lastSwitch: null, lastEvaluation: null,
    challenger: null, barred: [],
    commit: { consecutiveFailures: 0, retryAfterMs: null, lastFailure: null },
    noDataForMs: null,
    flags: { stalled: false, starved: false, exitOutsideSubscription: false, exitOutsideOverdue: false, allUnverified: false },
    counters: { failoverSwitches: 0, selectSwitches: 0, betterSwitchesInWindow: 2, betterSwitchesMax: 3 },
    dataSource: { blockedBy: null, lastSkip: null },
    ...overrides,
  };
}

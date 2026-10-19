import type { AutoSelectTarget, SelectionIntent } from '@/contracts/auto-select';
import type { ServerGroup } from './server-grouping';

/** UI scope identity only. Candidate eligibility and winner selection stay in the backend. */
export function autoSelectTarget(group?: Pick<ServerGroup, 'id' | 'isAll' | 'isManual' | 'isMesh'>): AutoSelectTarget {
  return !group || group.isAll ? { scope: 'global' }
    : group.isManual ? { scope: 'manual' }
    : group.isMesh ? { scope: 'mesh' }
    : { scope: 'subscription', subscriptionId: group.id };
}
export function autoSelectTargetMatches(intent: SelectionIntent | undefined, target: AutoSelectTarget): boolean {
  return intent?.mode === 'auto' && intent.scope === target.scope
    && (target.scope !== 'subscription' || intent.subscriptionId === target.subscriptionId);
}

import { describe, expect, it } from 'vitest';
import { autoSelectTarget, autoSelectTargetMatches } from './auto-select-target';
import { displayedExitId } from './actual-exit';

describe('Auto targets are scoped identities, not synthetic server IDs', () => {
  it('distinguishes global, built-in groups and subscription IDs colliding with their names', () => {
    expect(autoSelectTarget()).toEqual({ scope: 'global' });
    expect(autoSelectTarget({ id: '__all_nodes__', isAll: true, isManual: false })).toEqual({ scope: 'global' });
    expect(autoSelectTarget({ id: 'manual', isManual: true })).toEqual({ scope: 'manual' });
    expect(autoSelectTarget({ id: 'mesh', isManual: false, isMesh: true })).toEqual({ scope: 'mesh' });
    expect(autoSelectTarget({ id: 'global', isManual: false })).toEqual({ scope: 'subscription', subscriptionId: 'global' });
    expect(autoSelectTargetMatches({ mode: 'auto', scope: 'subscription', subscriptionId: 'manual' }, { scope: 'manual' })).toBe(false);
  });
  it('never calls a deferred saved choice the connected node', () => {
    expect(displayedExitId({ running: true, actualServerId: 'actual' }, 'desired')).toBe('actual');
    expect(displayedExitId({ running: true, actualServerId: null }, 'desired')).toBeNull();
    expect(displayedExitId({ running: true }, 'desired')).toBeNull();
    expect(displayedExitId({ running: false, actualServerId: null }, 'desired')).toBe('desired');
  });
});

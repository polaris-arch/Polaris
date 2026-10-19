import { expect, it } from 'vitest';
import { autoSelectChoice, autoSelectTargetFromChoice, autoSelectTargetLabel } from './auto-select-choice';
import { appRuleForPick } from '@/components/screens/app-policy/app-policy-logic';
import type { UserConfig } from '@/contracts/types';

it('roundtrips scope identity without colliding with literal node IDs', () => {
  for (const target of [{ scope: 'global' }, { scope: 'manual' }, { scope: 'mesh' }, { scope: 'subscription', subscriptionId: '全球 / %' }] as const) {
    expect(autoSelectTargetFromChoice(autoSelectChoice(target))).toEqual(target);
    expect(appRuleForPick('app', 'proxy', target)).toEqual({ appId: 'app', action: 'proxy', enabled: true, targetAutoSelect: target });
  }
  expect(autoSelectTargetFromChoice('node:auto:global')).toBeUndefined();
  expect(autoSelectTargetFromChoice('auto:subscription:%zz')).toBeUndefined();
  expect(autoSelectTargetFromChoice('auto:subscription:')).toBeUndefined();
  expect(appRuleForPick('app', 'proxy', 'auto:global').targetServerId).toBe('auto:global');
});
it('feature-off labels each stable rule reference with its own pin', () => {
  const config = { autoSelectEnabled: false, servers: [{ id: 'a', name: 'A' }, { id: 'd', name: 'D' }], autoSelectPins: [{ target: { scope: 'global' }, serverId: 'a' }, { target: { scope: 'subscription', subscriptionId: 'D' }, serverId: 'd' }] } as UserConfig;
  expect(autoSelectTargetLabel(config, { scope: 'global' }, key => key)).toBe('A');
  expect(autoSelectTargetLabel(config, { scope: 'subscription', subscriptionId: 'D' }, key => key)).toBe('D');
  expect(autoSelectTargetLabel(config, { scope: 'mesh' }, key => key)).toBe('autoSelect.pendingNode');
});

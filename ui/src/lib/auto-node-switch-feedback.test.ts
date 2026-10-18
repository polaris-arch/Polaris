import { describe, expect, it } from 'vitest';
import { AUTO_SELECT_CAUSES, type AutoNodeSwitchedPayload } from '@/contracts/auto-select';
import { autoNodeSwitchFeedback } from './auto-node-switch-feedback';
const t = (key: string, vars?: Readonly<Record<string, string | number>>) => key + JSON.stringify(vars);
describe('cause-aware confirmed switch events', () => {
  for (const cause of AUTO_SELECT_CAUSES) it(`${cause} explains its own cause`, () => {
    const event: AutoNodeSwitchedPayload = { cause, leg: cause === 'failover' ? 'failover' : 'select', reason: cause === 'failover' ? 'connectivity' : cause, newServerName: 'Actual node', oldServerName: 'Previous', latency: 24 };
    expect(autoNodeSwitchFeedback(event, t)).toBe(`autoSelect.event.${cause}{"name":"Actual node"}`);
  });
  it('unknown, old and inconsistent events explain only the exit change', () => {
    for (const payload of [{ cause: 'new-version', leg: 'select' }, { reason: 'connectivity' }, { cause: 'better', leg: 'failover' }]) expect(autoNodeSwitchFeedback({ ...payload, newServerName: 'Actual node' } as unknown as AutoNodeSwitchedPayload, t)).toContain('autoSelect.event.unknown');
  });
});

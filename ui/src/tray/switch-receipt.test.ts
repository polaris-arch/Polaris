import { describe, expect, it } from 'vitest';
import { traySwitchFeedback } from './switch-receipt';

const t = (key: string, vars: { node: string }) => `${key}:${vars.node}`;

describe('tray switch receipt', () => {
  it.each([
    ['applied', true, null],
    ['pending', false, 'tray.switchPending:HK'],
    ['restarting', false, 'tray.switchRestarting:HK'],
    ['notRunning', false, 'tray.switchSavedForNextStart:HK'],
    ['deferred', false, 'tray.switchRequiresApply:HK'],
    ['superseded', false, null],
  ] as const)('%s controls closing and notice', (status, close, notice) => {
    expect(traySwitchFeedback({ status }, 'HK', t)).toEqual({ close, notice });
  });
});

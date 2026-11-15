import { describe, expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import type { ServerSwitchReceipt } from '@/contracts/server-switch';
import { switchReceiptFeedback } from './switch-receipt-feedback';

const t = ((key: string) => key) as TFunction;

describe('server switch receipt feedback', () => {
  it.each([
    [{ status: 'applied' }, 'success', 'home.switchedToast'],
    [{ status: 'pending' }, 'info', 'home.switchPending'],
    [{ status: 'restarting' }, 'info', 'home.switchRestarting'],
    [{ status: 'deferred', reason: 'nodeRequiresApply' }, 'warning', 'home.switchRequiresApply'],
  ] as const)('%s reports only its confirmed state', (receipt, tone, text) => {
    expect(switchReceiptFeedback(receipt as ServerSwitchReceipt, 'HK', t)).toEqual({ tone, text });
  });

  it.each(['notRunning', 'superseded'] as const)('%s selection needs no toast', status => {
    expect(switchReceiptFeedback({ status }, 'HK', t)).toBeNull();
  });
});

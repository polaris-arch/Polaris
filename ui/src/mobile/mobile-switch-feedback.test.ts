import { describe, expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import type { ServerSwitchReceipt } from '@/contracts/server-switch';
import { switchReceiptFeedback } from '@/components/screens/shared/switch-receipt-feedback';
import { mobileSwitchReceiptFeedback } from './mobile-switch-feedback';

const t = ((key: string) => key) as TFunction;

describe('mobile switch receipt when the core is stopped', () => {
  it('keeps the saved choice silent only on mobile; desktop still reports its existing outcome', () => {
    const receipt: ServerSwitchReceipt = { status: 'notRunning' };
    expect(mobileSwitchReceiptFeedback(receipt, 'node', t)).toBeNull();
    expect(switchReceiptFeedback(receipt, 'node', t)).toEqual({ tone: 'info', text: 'home.switchSavedForNextStart' });
  });

  it.each(['applied', 'pending', 'deferred', 'superseded'] as const)(
    'preserves the shared %s outcome', (status) => {
      const receipt: ServerSwitchReceipt = { status };
      expect(mobileSwitchReceiptFeedback(receipt, 'node', t)).toEqual(switchReceiptFeedback(receipt, 'node', t));
    },
  );
});

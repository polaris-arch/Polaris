import type { TFunction } from 'i18next';
import type { ServerSwitchReceipt } from '@/contracts/server-switch';
import { switchReceiptFeedback } from '@/components/screens/shared/switch-receipt-feedback';

/** A stopped core has no live exit to apply. The saved selection itself is visible in the picker. */
export function mobileSwitchReceiptFeedback(receipt: ServerSwitchReceipt, node: string, t: TFunction) {
  return receipt.status === 'notRunning' ? null : switchReceiptFeedback(receipt, node, t);
}

import type { TFunction } from 'i18next';
import type { ServerSwitchReceipt } from '@/contracts/server-switch';

/** Report only what the backend confirmed. A saved choice may still await Apply or a restart. */
export function switchReceiptFeedback(
  receipt: ServerSwitchReceipt,
  node: string,
  t: TFunction,
): { tone: 'success' | 'info' | 'warning'; text: string } | null {
  switch (receipt.status) {
    case 'applied': return { tone: 'success', text: t('home.switchedToast', { node }) };
    case 'pending': return { tone: 'info', text: t('home.switchPending', { node }) };
    case 'notRunning': return { tone: 'info', text: t('home.switchSavedForNextStart', { node }) };
    case 'deferred': return { tone: 'warning', text: t('home.switchRequiresApply', { node }) };
    case 'superseded': return null;
  }
}

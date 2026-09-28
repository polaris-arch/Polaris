import type { ServerSwitchReceipt } from '@/contracts/server-switch';

type NoticeKey = 'tray.switchPending' | 'tray.switchSavedForNextStart' | 'tray.switchRequiresApply';

/** The tray stays visible while a saved choice is not confirmed in the running core. */
export function traySwitchFeedback(
  receipt: ServerSwitchReceipt,
  node: string,
  t: (key: NoticeKey, vars: { node: string }) => string,
): { close: boolean; notice: string | null } {
  switch (receipt.status) {
    case 'applied': return { close: true, notice: null };
    case 'pending': return { close: false, notice: t('tray.switchPending', { node }) };
    case 'notRunning': return { close: false, notice: t('tray.switchSavedForNextStart', { node }) };
    case 'deferred': return { close: false, notice: t('tray.switchRequiresApply', { node }) };
    case 'superseded': return { close: false, notice: null };
  }
}

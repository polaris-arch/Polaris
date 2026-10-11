import type { AutoSelectReceipt } from '@/contracts/auto-select';
import type { ServerSwitchReceipt } from '@/contracts/server-switch';
import type { UserConfig } from '@/contracts/types';
import { api } from '@/ipc';
import { withConfigWriteLock } from './config-write-lock';
import { configReadGeneration } from './config-read-generation';

export type AutoSelectT = (key: string, vars?: Readonly<Record<string, string | number>>) => string;
export type AutoSelectAction = { kind: 'enable'; subscriptionId: string } | { kind: 'switchNow' } | { kind: 'manual'; serverId: string };
export type AutoSelectActionResult = { action: AutoSelectAction; receipt: AutoSelectReceipt | ServerSwitchReceipt; config: UserConfig };
export interface AutoSelectActionFeedback { tone: 'success' | 'info' | 'warning' | 'error'; text: string }

/** Intent and actual exit are separate, and derived Auto choices never enter servers. */
export function autoSelectContext(config: UserConfig | null): string {
  return JSON.stringify([config?.selectionIntent ?? null, config?.selectedServerId ?? null]);
}

/** Every attempted write reads the saved truth, including command rejection/partial failure.
 * The existing webview queue also orders these operations against explicit manual choices.
 * beforeStart cancels queued work after navigation or a newer selection; it cannot cancel native IPC.
 */
export function runAutoSelectAction(action: AutoSelectAction, accept: (config: UserConfig) => void | Promise<void>, beforeStart: () => boolean = () => true): Promise<AutoSelectActionResult | null> {
  return withConfigWriteLock(async () => {
    if (!beforeStart()) return null;
    let receipt: AutoSelectReceipt | ServerSwitchReceipt | undefined;
    let failure: unknown;
    try {
      receipt = action.kind === 'enable' ? await api.server.autoSelectEnable(action.subscriptionId)
        : action.kind === 'switchNow' ? await api.server.autoSelectSwitchNow()
        : await api.server.switch(action.serverId);
    } catch (error) { failure = error; }
    const readGeneration = configReadGeneration.beginRead();
    let config: UserConfig;
    try { config = await api.config.get(); }
    catch (error) {
      if (!configReadGeneration.isCurrent(readGeneration)) return null;
      throw Object.assign(new Error('AUTO_SELECT_READBACK_FAILED'), { code: 'AUTO_SELECT_READBACK_FAILED', cause: error, commandFailure: failure });
    }
    // A newer whole-config publication or read supersedes this snapshot.
    if (!configReadGeneration.isCurrent(readGeneration)) return null;
    try { await accept(config); }
    catch (error) { throw Object.assign(new Error('AUTO_SELECT_ADOPTION_FAILED'), { code: 'AUTO_SELECT_READBACK_FAILED', cause: error, commandFailure: failure, savedContext: autoSelectContext(config) }); }
    if (failure !== undefined) {
      const code = failure && typeof failure === 'object' && 'code' in failure ? String(failure.code) : '';
      throw Object.assign(new Error('AUTO_SELECT_COMMAND_FAILED'), { code, cause: failure, savedContext: autoSelectContext(config) });
    }
    if (!receipt) throw new TypeError('Missing selection receipt');
    return { action, receipt, config };
  });
}

export function autoSelectErrorFeedback(error: unknown, t: AutoSelectT): AutoSelectActionFeedback {
  const code = error && typeof error === 'object' && 'code' in error ? String(error.code) : '';
  const known = ['AUTO_SELECT_SWITCHED_OFF', 'AUTO_SELECT_PLATFORM_NOT_OPEN', 'AUTO_SELECT_SUBSCRIPTION_NOT_FOUND', 'AUTO_SELECT_SUBSCRIPTION_EMPTY', 'AUTO_SELECT_NOT_AUTO', 'AUTO_SELECT_NO_TARGET', 'AUTO_SELECT_SWITCH_FAILED', 'AUTO_SELECT_READBACK_FAILED'];
  return { tone: 'error', text: t(known.includes(code) ? `autoSelect.error.${code}` : 'autoSelect.actionFailed') };
}

export function autoSelectActionFeedback({ action, receipt, config }: AutoSelectActionResult, t: AutoSelectT): AutoSelectActionFeedback | null {
  const name = (id: string) => config.servers.find(server => server.id === id)?.name
    ?? (id === '__direct__' ? t('autoSelect.direct') : id === '__block__' ? t('autoSelect.block') : t('autoSelect.unavailable'));
  if (action.kind === 'manual') {
    const manualReceipt = receipt as ServerSwitchReceipt;
    if (manualReceipt.status === 'superseded') return null;
    if (config.selectionIntent !== undefined || config.selectedServerId !== action.serverId) return { tone: 'warning', text: t('autoSelect.selectionChanged') };
    const feedback = receiptFeedback(manualReceipt.status, name(action.serverId), t);
    return feedback ? { ...feedback, text: `${t('autoSelect.manualSaved')} ${feedback.text}` } : null;
  }
  const autoReceipt = receipt as AutoSelectReceipt;
  // A saved newer/manual intent must not be advertised as successfully enabling Auto.
  const intent = config.selectionIntent;
  if (!autoReceipt.intentApplied || intent?.mode !== 'auto' || intent.scope !== 'subscription'
    || intent.subscriptionId !== autoReceipt.status.intent.subscriptionId) return null;
  const placement = autoReceipt.placement;
  if (!placement) return { tone: 'info', text: t('autoSelect.intentSaved') };
  if (placement.status === 'superseded') return null;
  if (placement.status === 'failed') return { tone: 'warning', text: t('autoSelect.placementFailed') };
  if (config.selectedServerId !== placement.serverId) return { tone: 'warning', text: t('autoSelect.selectionChanged') };
  return receiptFeedback(placement.status, name(placement.serverId), t);
}

function receiptFeedback(status: ServerSwitchReceipt['status'], name: string, t: AutoSelectT): AutoSelectActionFeedback | null {
  if (status === 'superseded') return null;
  return { tone: status === 'applied' ? 'success' : status === 'deferred' ? 'warning' : 'info', text: t(`autoSelect.receipt.${status}`, { name }) };
}

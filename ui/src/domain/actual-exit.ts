import type { ProxyStatus } from '@/contracts/types';

/** While connected, never advertise the saved/deferred choice as the actual exit.
 * Older runtimes lacking actualServerId stay unknown rather than inventing confirmation.
 * While stopped, the selector still displays the saved choice for the next connection.
 */
export function displayedExitId(status: ProxyStatus | null | undefined, savedId: string | null | undefined): string | null {
  return status?.running ? status.actualServerId ?? null : savedId ?? null;
}

import type { UserConfig } from '@/contracts/types';
import { autoSelectChoice } from './auto-select-choice';
/** Auto without a running confirmed selection has no actual node to advertise. */
export function configuredExitId(status: ProxyStatus | null | undefined, config: UserConfig | null | undefined): string | null {
  if (status?.running) return status.actualServerId ?? null;
  if (config?.selectionIntent?.mode === 'auto') {
    if (config.autoSelectEnabled) return null;
    const intent = config.selectionIntent;
    const target = intent.scope === 'subscription' && intent.subscriptionId ? { scope: 'subscription' as const, subscriptionId: intent.subscriptionId }
      : intent.scope !== 'subscription' ? { scope: intent.scope } : null;
    return target ? config.autoSelectPins?.find(pin => autoSelectChoice(pin.target) === autoSelectChoice(target))?.serverId ?? null : null;
  }
  return config?.selectedServerId ?? null;
}

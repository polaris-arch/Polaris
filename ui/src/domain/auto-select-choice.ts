import type { AutoSelectTarget } from '@/contracts/auto-select';
/** UI option encoding only; never persisted as a synthetic server id. */
export function autoSelectChoice(target: AutoSelectTarget): string {
  return target.scope === 'subscription' ? `auto:subscription:${encodeURIComponent(target.subscriptionId)}` : `auto:${target.scope}`;
}
export function autoSelectTargetFromChoice(value: string): AutoSelectTarget | undefined {
  if (value === 'auto:global') return { scope: 'global' };
  if (value === 'auto:manual') return { scope: 'manual' };
  if (value === 'auto:mesh') return { scope: 'mesh' };
  if (!value.startsWith('auto:subscription:')) return undefined;
  try { const subscriptionId = decodeURIComponent(value.slice('auto:subscription:'.length)); return subscriptionId.trim() ? { scope: 'subscription', subscriptionId } : undefined; } catch { return undefined; }
}

import type { UserConfig } from '@/contracts/types';
export function autoSelectTargetLabel(config: UserConfig | null | undefined, target: AutoSelectTarget, t: (key: string) => string): string {
  if (!config?.autoSelectEnabled) {
    const pin = config?.autoSelectPins?.find(pin => autoSelectChoice(pin.target) === autoSelectChoice(target));
    return config?.servers.find(server => server.id === pin?.serverId)?.name ?? t('autoSelect.pendingNode');
  }
  const scope = target.scope === 'subscription' ? config.subscriptions?.find(sub => sub.id === target.subscriptionId)?.name ?? t('autoSelect.unavailable')
    : t(`autoSelect.scopeLabels.${target.scope}`);
  return `${t('autoSelect.auto')} · ${scope}`;
}

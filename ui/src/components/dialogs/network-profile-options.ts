/**
 * 规则表单「生效网络」字段的候选（桌面 `RuleDialog` 与移动端 `RuleFormPanel` 共用）。
 *
 * 从 `RuleDialog.tsx` 原样搬出（判据一行未改）：那份 `.tsx` 带桌面弹窗外壳，移动端不许 import；
 * 候选的口径（停用标注但仍可选 / 已删除补一项 / 新建场景哨兵）两端必须是同一份，否则同一条规则在两个客户端
 * 上会显示成两个不同的「生效网络」。
 */

import type { TFunction } from 'i18next';
import type { NetworkProfile } from '@/contracts/types';
import type { CselOptionLike } from './csel-logic';

/** 「生效网络」下拉里「新建场景…」这一项的哨兵值（不是场景 id：场景 id 恒以 `np-` 起头）。 */
export const NEW_PROFILE_CHOICE = '__new-network-profile__';

/**
 * 「生效网络」候选：任何网络 / 各场景（停用的标注但仍可选 —— 选中即「停用期间不生效」，与场景面板语义一致）/
 * 新建场景…。当前值指向已删除的场景时补一项「场景已删除」，不让下拉静默显示成别的值。
 */
export function networkProfileOptions(
  profiles: readonly NetworkProfile[],
  current: string,
  t: TFunction,
): CselOptionLike[] {
  const options: CselOptionLike[] = [
    { value: '', label: t('rules.networkProfile.anyNetwork') },
    ...profiles.map((p) => ({
      value: p.id,
      label: p.name,
      description: p.enabled ? undefined : t('rules.networkProfile.disabledBadge'),
    })),
  ];
  if (current && !profiles.some((p) => p.id === current)) {
    options.push({ value: current, label: t('rules.networkProfile.badgeMissing'), disabled: true });
  }
  options.push({ value: NEW_PROFILE_CHOICE, label: t('rules.networkProfile.newOption') });
  return options;
}

/**
 * 网络场景「本机将使用哪种探测、可不可用」的**取数 hook 与显示文案**（桌面场景面板与移动端规则屏共用）。
 *
 * 为什么单独成一个 `.ts`：它原先住在 `NetworkProfilePanel.tsx` 里，而那份文件 import 桌面弹窗外壳
 * （`Modal` / `ListEditor` / 设置屏原语），移动端入口不许 import（契约 A1：桌面层叠链不进移动模块图）。
 * 判据与文案必须只有一份 —— 移动端另写一份等于两处各自漂 —— 故把零 DOM 依赖的这两样搬到这里。
 *
 * 纪律同 `domain/network-profile.ts` 头注：探测源**不在渲染端算**，本文件只取后端结果、只翻成文案。
 */

import { useEffect, useState } from 'react';
import type { TFunction } from 'i18next';
import type { ResolvedProbe } from '@/contracts/types';
import { api } from '@/ipc';
import type { ProbeDisplay } from '@/domain/network-profile';

/**
 * 后端解析后的探测源 + 命中态。`dep` 变了就重拉（场景、代理模式、TUN 配置都会改变解析结果）；
 * 命中态变更信号（`onMatchChanged`，N4，无载荷）到达时也重拉 —— `matched` 的真值在后端，信号只说「变了」。
 * 桌面场景面板、桌面规则屏与移动端规则屏/表单全经这一个 hook，故订阅只写这一处。
 * 拉不到（IPC 失败 / 后端还没有这条命令 / 返回不是数组）⇒ `null` ⇒ 显示「暂时无法获取」，不猜。
 */
export function useResolvedProbes(dep: unknown): ResolvedProbe[] | null {
  const [resolved, setResolved] = useState<ResolvedProbe[] | null>(null);
  const [matchTick, setMatchTick] = useState(0);
  useEffect(() => api.networkProfile.onMatchChanged(() => setMatchTick((n) => n + 1)), []);
  useEffect(() => {
    let active = true;
    api.networkProfile
      .resolvedSources()
      .then((list) => {
        if (active) setResolved(Array.isArray(list) ? list : null);
      })
      .catch(() => {
        if (active) setResolved(null);
      });
    return () => {
      active = false;
    };
  }, [dep, matchTick]);
  return resolved;
}

export const PROBE_SOURCE_KEYS = {
  system: 'rules.networkProfile.sourceSystem',
  dhcp: 'rules.networkProfile.sourceDhcp',
} as const;

/**
 * 「本机将使用：…」一行（表单）与列表行尾的短标签共用的文案。
 *
 * `reasonKeyOf`：不可用原因的 i18n 键映射（缺省原样）。原因码是同一个，**措辞**可以随客户端不同 ——
 * `dhcpNeedsPrivilege` 在桌面只会出现在 Linux 系统代理模式下（文案据此给出「改用 TUN」的建议），
 * 而在手机上恒成立、那条建议不适用；移动端经这一格换成自己的一句，判据不动。
 */
export function probeDisplayText(
  display: ProbeDisplay,
  t: TFunction,
  reasonKeyOf: (reasonKey: string) => string = (key) => key,
): string {
  switch (display.kind) {
    case 'pending':
      return t('rules.networkProfile.probePending');
    case 'unknown':
      return t('rules.networkProfile.probeUnknown');
    case 'ok':
      return t('rules.networkProfile.probeUses', { source: t(PROBE_SOURCE_KEYS[display.source]) });
    case 'unavailable':
      return t('rules.networkProfile.probeUnavailable', { reason: t(reasonKeyOf(display.reasonKey)) });
  }
}

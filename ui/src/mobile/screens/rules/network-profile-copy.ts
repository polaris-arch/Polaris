/**
 * 网络场景在移动端的**措辞差**（只有措辞，判据一条都不在这里）。
 *
 * 原因码由后端与生成器同一个函数算出（`network_env.rs#resolved_probe` / `builtin_dhcp_status`），
 * 渲染端经 `domain/network-profile#probeReasonKey` 翻成 i18n 键。桌面那几句是按桌面的事实写的：
 *
 *  · `dhcpNeedsPrivilege` —— 桌面只在「Linux 系统代理模式」下出现，文案据此建议「改用 TUN 模式」；
 *    Android / iOS 上它**恒成立**（`network_env.rs#dhcp_privileged` 对两者恒 false：核是应用沙箱里的
 *    libbox，`VpnService` 的 tun fd 不附带任何 capability，绑不了 UDP 68），那条建议在手机上是错的。
 *
 * 故移动端只换这一句的键，其余原因码（场景停用 / 监视网卡失败 …）两端措辞同源。
 */

import { PROBE_REASON_KEYS } from '@/domain/network-profile';

/** 桌面 i18n 键 → 移动端替换键。表外的键原样使用。 */
const MOBILE_REASON_KEYS: Readonly<Record<string, string>> = {
  [PROBE_REASON_KEYS.dhcpNeedsPrivilege!]: 'mobileRules.networkProfile.reasonDhcpNoPermission',
};

/** 模块级函数（引用稳定）：会作为 `useRuleDnsEffect` 的依赖进 `useMemo`。 */
export function mobileReasonKey(reasonKey: string): string {
  return MOBILE_REASON_KEYS[reasonKey] ?? reasonKey;
}

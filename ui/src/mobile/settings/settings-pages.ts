/**
 * 移动端设置屏的**分段模型**（IA §2.4 的根页顺序）。
 *
 * # 为什么是一张表，而不是把顺序写死在 JSX 里
 *
 * 判据要能独立说话：`MobileSettings.test.tsx` 拿这张表与 `store/nav-store.ts` 的 `SettingsScreen`
 * 联合对差 —— 裁定 #4 定的是「代码是真值」，而代码里的真值恰好是那个联合（9 项）。表在这里，
 * 对差就只有一处；顺序写死在 JSX 里，对差就得靠正则去啃 JSX，那是判据自己会塌的那一层。
 *
 * # `helper` 的缺席是**结论**，不是遗漏（IA §4.4）
 *
 * 桌面 9 个子页，这里 8 个：移动端没有提权助手进程，隧道来自 `VpnService` / `NEPacketTunnelProvider`，
 * 权限来自系统弹窗。它的替代不是「助手页的移动版」，而是根页上一行 **VPN 授权**（回答同一个用户
 * 问题、对着完全不同的对象），故它是一条**新**契约、不复用助手页的字形与文案。
 * 判据钉的是「恰好少 `helper` 这一项」，不是「数量等于 8」—— 后者少了别的也照样绿。
 */

import type { UserConfig } from '@/contracts/types';
import type { CommitWrite } from './write-feedback';

/** 二级页 id。取值与 `store/nav-store.ts` 的 `SettingsScreen` 同名（`helper` 除外，见头注）。 */
export type MobileSettingsPageId =
  | 'general'
  | 'display'
  | 'network'
  | 'dns'
  | 'tun'
  | 'update'
  | 'backup'
  | 'about';

/** 桌面 9 子页里移动端**刻意缺席**的那一个。判据按名字对差，不按数量。 */
export const MOBILE_ABSENT_SETTINGS_PAGE = 'helper';

/** 根页一组行。`headerKey` 为 `null` = 无组头（IA §2.4 表格第 1 组）。 */
export interface MobileSettingsGroup {
  readonly id: string;
  readonly headerKey: string | null;
  readonly pages: readonly MobileSettingsPageId[];
  /**
   * 该组内**不进入二级页**的行（今天只有 VPN 授权）。它不是子页：没有页可进，只有一个状态可读。
   *
   * ⚠️ 上一版这里还写着「那个状态今天还没有后端（移动端零行）」——**2026-09-06 已过期并清掉**：
   * 后端两侧都在（Kotlin `PolarisVpnPlugin.kt:149 vpnAuthStatus` → Rust `lib.rs:1134 vpn_auth_status`），
   * 根页那颗芯片读的就是真值，与 `MobileSettingsScreen.tsx` 头注「VPN 授权那一行：状态是**读来的**」
   * 是同一件事。两处说反了会让下一个人以为芯片是编的。
   */
  readonly statusRows?: readonly string[];
}

/**
 * 根页四组，顺序逐字取自 IA §2.4「Content order — root」的表格 —— 那是用户已经有的那张地图，
 * 重排它等于让熟练用户重新学一遍。
 *
 * 与桌面导航的两处差异都是被迫的、都已在 IA §4 登记：
 *  · 提权助手缺席（§4.4）；
 *  · 第 2 组多一行 **VPN 授权**，因为它报告的权限是移动端独有的对象（§4.4）。
 */
export const MOBILE_SETTINGS_GROUPS: readonly MobileSettingsGroup[] = [
  { id: 'basics', headerKey: null, pages: ['general', 'display'] },
  {
    id: 'netstack',
    headerKey: 'settings.nav.groupNetStack',
    pages: ['network', 'dns', 'tun'],
    statusRows: ['vpn-authorization'],
  },
  { id: 'system', headerKey: 'settings.nav.groupSystem', pages: ['update', 'backup'] },
  { id: 'about', headerKey: null, pages: ['about'] },
];

/** 展平后的二级页顺序（= 根页从上到下的进入顺序）。 */
export const MOBILE_SETTINGS_PAGES: readonly MobileSettingsPageId[] = MOBILE_SETTINGS_GROUPS.flatMap(
  (g) => g.pages,
);

/**
 * 二级页标题键。
 *
 * `dns` / `tun` 取常量而非 i18n 键：桌面同样写死（`<Phead title="DNS">` / `title="TUN"`），
 * 五语种写法一致，进 locale 只会多五份要同步的同一个字符串。
 */
export const MOBILE_SETTINGS_PAGE_TITLE: Readonly<Record<MobileSettingsPageId, string>> = {
  general: 'settings.general.pageTitle',
  display: 'settings.nav.appearance',
  network: 'settings.nav.network',
  dns: 'DNS',
  tun: 'TUN',
  update: 'settings.nav.update',
  backup: 'settings.nav.backup',
  about: 'settings.nav.about',
};

/** 标题是否是 i18n 键（false = 直接当文案渲染，见上）。 */
export function isTitleKey(title: string): boolean {
  return title.includes('.');
}

/**
 * 栈里读回的裸字符串收窄成本屏的页 id。
 *
 * 压栈状态住在外壳（`MobileShell` 的 `usePushedPage`，底部导航要读它），而外壳**不认识**任何一屏的
 * 页 id 枚举 —— 认识就等于把屏的内容契约搬进外壳。故那一格存的是 `string`，由本屏在读回处收窄；
 * 不在表里的值一律当「停在根页」，不会渲染出一页空白。
 */
export function isSettingsPageId(value: string): value is MobileSettingsPageId {
  return (MOBILE_SETTINGS_PAGES as readonly string[]).includes(value);
}

/**
 * 八个二级页统一的入参：一份 config + 同一个写漏斗（与桌面 9 子页同口径）+ 写失败回显的接线。
 *
 * ⚠️ 这里的 `update` 是**会抛的那一版**（屏组件已经把 `{ throwOnError: true }` 绑进去）。
 * 桌面那 46 个调用点不 await、失败靠 toast；本屏的回显是**行内**的（那张 `行 id → 失败提示`
 * 表，理由见 `MobileSettingsScreen` 头注）⇒ 不抛就没有失败可接，等于**静默**。
 * 各页因此一律写成 `commit('<行 id>', update({ … }))`：`update({ … })` 这个字面形态留在页里
 * （`config-write-wiring.test.ts` 的 W-a 按它扫），`commit` 负责把失败落到那一行上。
 */
export interface MobileSettingsPageProps {
  config: UserConfig;
  update: (patch: Partial<UserConfig>) => Promise<void>;
  commit: CommitWrite;
}

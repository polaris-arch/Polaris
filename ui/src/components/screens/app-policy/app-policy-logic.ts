import type { AutoSelectTarget } from '@/contracts/auto-select';
/**
 * 应用分流屏的排序判据（纯函数，脱离组件可断言 —— 本仓 Vitest 走 node 环境无 DOM，
 * 判据留在 .tsx 里就等于没有门）。
 *
 * 为什么必须带 locale：`a.localeCompare(b)` 不传 locale 时用**运行时默认语言环境**（本机 node 与
 * 多数 webview 解析为 en-US），而 en-US 的排序对汉字退化成码位序 —— 「工具/游戏/社交/视频」会排成
 * 工具·游戏·社交·视频，既不是拼音序也不是笔画序，用户读作「没排序」。传 zh-CN 才走 CLDR 拼音
 * 整理（工具·社交·视频·游戏）。裸 `<` 更差（纯码位）。
 *
 * 副作用：zh 的 CLDR 整理含 `[reorder Hani]`（汉字排在拉丁之前），故中文界面下「AI」这类拉丁名
 * 类目恒落在汉字类目之后。这是标准中文整理的既定语义，不另做特例覆盖。
 */

/** 「全部」不是类目而是「不过滤」，排序时恒置首（拼音序下「全部」会掉进 工具/社交 之间，语义错位）。 */
export const APP_CATEGORY_ALL = 'all';

export interface LabeledCategory {
  key: string;
  label: string;
}

/**
 * 显示名比较判据 —— 分类筛选下拉、内容分组、组内应用三处共用同一份，避免各排各的。
 * locale 传当前界面语言（i18n.language，恒为 SupportedLanguage 之一，故不会是非法 BCP-47 标签）。
 */
export function compareLabel(a: string, b: string, locale: string): number {
  return a.localeCompare(b, locale);
}

/**
 * 分类排序：'all' 恒首，其余按当前语言下的显示名。
 *
 * 返回新数组（`Array.prototype.sort` 原地改），调用方传的是 memo 依赖里的常量表，不能被就地打乱。
 */
export function sortAppCategories<T extends LabeledCategory>(
  cats: readonly T[],
  locale: string,
): T[] {
  return [...cats].sort((a, b) => {
    if (a.key === APP_CATEGORY_ALL || b.key === APP_CATEGORY_ALL) {
      // 两边都是 'all' 时返回 0（sort 稳定 → 保持原相对次序），否则 'all' 那侧在前。
      return (a.key === APP_CATEGORY_ALL ? 0 : 1) - (b.key === APP_CATEGORY_ALL ? 0 : 1);
    }
    return compareLabel(a.label, b.label, locale);
  });
}

import type { AppRule } from '@/contracts/types';

/** 四个档位。`proxy` = 指定节点（必须带 `targetServerId`），`follow` = 跟随全局。 */
export type AppPolicyWritablePick = 'follow' | 'direct' | 'block' | 'proxy';

/**
 * 档位白名单。面板给出的四档都在其中；它今天守的是「档位串合法」这一格（见文件头注末节）。
 */
export const APP_POLICY_WRITABLE: ReadonlySet<string> = new Set<AppPolicyWritablePick>([
  'follow',
  'direct',
  'block',
  'proxy',
]);

export function isAppPolicyWritable(pick: string): pick is AppPolicyWritablePick {
  return APP_POLICY_WRITABLE.has(pick);
}

/**
 * 一次选档落成的 `AppRule`。与桌面 `AppPolicyScreen.tsx:259 setAppPolicy` 逐字段同义，
 * 两处**刻意**的差别各自有理由：
 *
 *  ① `follow` 落成 `{ action: 'proxy', 不带 targetServerId }` —— 桌面快选「默认代理」的落盘形态，
 *     两端 `appPolicyView` 都把它显示成「跟随全局」。它不是「没有规则」：显式写一条
 *     跟随全局的规则，才能把一条已存在的 `direct` 从 `exclude_package` 里拿掉。
 *
 *  ② **`enabled` 恒 `true`**，不沿用 `existing?.enabled ?? true`。移动端**没有**逐条启停应用规则的
 *     控件（桌面在卡片上有），沿用一条 `enabled:false` 的旧规则会让用户看见档位变了、而
 *     `inbounds.rs:717` 的 `filter(|r| r.enabled && …)` 把它整条滤掉 —— 又一颗「拨了不生效」的控件。
 *     这是**扩大**了一次用户显式动作的效果，故写在这里，不是顺手。
 *
 * ⚠️ **`targetServerId` 只在 `proxy` 档且真的给了节点时才写**（连 `undefined` 都不写）：
 * `AppRule` 的这一格是可选的，Rust 侧 `rule.rs:293` 带 `skip_serializing_if = "Option::is_none"`。
 * 写成 `targetServerId: undefined` 会让 `JSON.stringify` 的结果与夹具在**深比较**上分叉。
 * 反过来，`proxy` 档**没给**节点 id 时落成的就是 `follow` 那条形态 —— 那正是它的语义
 * （「代理但没指定去哪」= 跟随全局），不是一个需要报错的非法输入。
 */
export function appRuleForPick(
  appId: string,
  pick: AppPolicyWritablePick,
  targetServerId?: string | AutoSelectTarget,
): AppRule {
  if (pick === 'direct') return { appId, action: 'direct', enabled: true };
  if (pick === 'block') return { appId, action: 'block', enabled: true };
  if (pick === 'proxy' && typeof targetServerId === 'object') return { appId, action: 'proxy', enabled: true, targetAutoSelect: targetServerId };
  if (pick === 'proxy' && typeof targetServerId === 'string' && targetServerId !== '') {
    return { appId, action: 'proxy', enabled: true, targetServerId };
  }
  return { appId, action: 'proxy', enabled: true };
}

/** An explicit disabled rule must never advertise its configured action as active. */
export function appRuleDisabled(rule: AppRule | undefined): boolean {
  return rule?.enabled === false;
}

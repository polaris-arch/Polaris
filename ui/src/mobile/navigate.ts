/**
 * 跨屏跳转的**唯一**入口 —— 「去设置→更新」这类动作的落点。
 *
 * # 为什么需要它
 *
 * 五个主目的地是 `MobileApp.tsx` 里一个 `useState`（无路由库、无栈，理由见那份头注），
 * 而二级页是 `MobileShell` 的 `setPushedPage`（模块级、按目的地分格）。于是「从节点屏跳到
 * 设置屏的更新页」这件事**没有任何地方能表达** —— 订阅「更多」里的「更新间隔」正撞在这上面：
 * Polaris 没有 per-sub 间隔字段，桌面那一项做的就是 `enterSettings('update')`。
 *
 * 缺这条腿的表现不是崩，是**一句话**：「间隔是全局的，请去设置里改」——一条指路牌，
 * 而指的那扇门要用户自己去找。仓里已经有一条同形的债
 * （`mobileSettings.general.bootConnectPending`：「应用内快捷入口还没接，按上面的路径走」），
 * 不该再添一条。
 *
 * # 形态：安装式，不是全局可变函数
 *
 * `MobileApp` 在 effect 里装上自己的 `setActive`，卸载时摘掉。判据（与非移动外壳的调用者）
 * 拿到的是 `false` —— **调用方必须处理这个返回值**，不许假设跳成功了。
 * 这与 `back-stack.ts` 的登记表同型：模块级表 + 组件安装，本仓已有先例。
 *
 * ⚠️ 与 `back-stack` **不同**的是这里可以用 effect：它不需要在渲染期就绪（没有哪道门要在
 * `renderToStaticMarkup` 里读它），而 effect 保证了「组件真的挂上了」才接受跳转。
 */

import { setPushedPage } from './MobileShell';
import type { DestinationId } from './destinations';
import { useExactRuleFilter } from './connections/exact-rule-filter';
import type { RuleGroupFilter } from './connections/exact-rule-filter';

type Navigator = (destination: DestinationId) => void;

let navigator: Navigator | null = null;

/**
 * 装上导航器，返回卸载闭包。**幂等**：同一个函数重复装只是覆盖。
 * 第二个组件来装会顶掉第一个 —— 移动树上只有 `MobileApp` 一个装载点，多一个就是接线错了。
 */
export function installMobileNavigator(go: Navigator): () => void {
  navigator = go;
  return () => {
    if (navigator === go) navigator = null;
  };
}

/**
 * 跳到某个目的地（可选带一个二级页）。
 *
 * 返回 `false` = **没有导航器**（浏览器调试档 / 判据渲染 / 组件还没挂）。调用方据此如实告知，
 * 绝不许把 `false` 当成跳成功了 —— 那正是「点了没反应」的来源。
 *
 * 二级页**先设再切**：反过来的话切到设置屏那一帧读到的是上一次的页，会闪一下根页。
 */
export function navigateMobile(
  destination: DestinationId,
  page?: string,
  options?: { ruleGroup: RuleGroupFilter },
): boolean {
  if (navigator === null) return false;
  useExactRuleFilter.getState().setExactRule(
    destination === 'connections' ? options?.ruleGroup ?? null : null,
  );
  if (page !== undefined) setPushedPage(destination, page);
  navigator(destination);
  return true;
}

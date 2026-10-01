/**
 * 目的地 → 屏的**唯一**映射表。
 *
 * # 五个屏都是真的了（2026-09-05）
 *
 * 本文件在 F0 那一批只交占位屏，存在的唯一理由是证明「路由通」。五个屏逐批接完之后
 * 占位屏成了死代码，`tsc` 当场报 TS6133 —— 那正是它该有的下场，已删。
 * 接屏的方式不变：换 `SCREENS` 里对应的那一项，外壳侧零改动
 * （section 节奏、断点、滚动、底部导航停靠、待应用条槽位都在 `MobileShell` 里）。
 *
 * | 目的地 | 屏 | 落地批 |
 * |---|---|---|
 * | `home` | `home/MobileHomeScreen` | F1 —— 八张卡，DOM 序即重要性序 |
 * | `nodes` | `nodes/MobileNodesScreen` | F2 —— 取消卡片/列表双视图 |
 * | `rules` | `screens/rules/RulesScreen` | F3 —— 四合一：流量 · DNS · 应用 · 资源 |
 * | `connections` | `connections/MobileConnectionsScreen` | F5 —— 四分段：概览 · 活动 · 已结束 · 日志 |
 * | `settings` | `settings/MobileSettingsScreen` | F4 —— 八个二级页 |
 *
 * 屏级内容契约在 `~/docs/polaris/design/mobile-kit/component-specs/`
 * （首页看 `home-screen.md`，其余四个看 `information-architecture.md` §2）。
 */

import type { ReactElement } from 'react';
import type { DestinationId } from './destinations';
import { MobileHomeScreen } from './home/MobileHomeScreen';
import { MobileConnectionsScreen } from './connections/MobileConnectionsScreen';
import { MobileNodesScreen } from './nodes/MobileNodesScreen';
import { MobileRulesScreen } from './screens/rules/RulesScreen';
import { MobileSettingsScreen } from './settings/MobileSettingsScreen';

/**
 * 写成 `Record<DestinationId, …>` 的字面量而不是从 `DESTINATIONS` 派生：派生要一次 `as` 断言才
 * 满足 `Record`，而断言恰好会把"少一个屏"这件事从编译器眼里抹掉。字面量形态下**少一个键直接编译不过**
 * —— 这是本文件最强的一道判据，比任何运行期检查都早。门另在源码层对拍一次（编译期不变式不进产物，
 * 门要能独立说得出话）。
 */
export const SCREENS: Record<DestinationId, () => ReactElement> = {
  home: () => <MobileHomeScreen />,
  nodes: () => <MobileNodesScreen />,
  rules: () => <MobileRulesScreen />,
  connections: () => <MobileConnectionsScreen />,
  settings: () => <MobileSettingsScreen />,
};

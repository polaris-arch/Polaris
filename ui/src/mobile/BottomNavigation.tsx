/**
 * 底部导航（spec：`~/docs/polaris/design/mobile-kit/component-specs/bottom-navigation.md`）。
 *
 * 五个目的地等分、内容高 64 + 底部安全区、图标 23、活动指示条 2×28。
 * 取色：背景 `surface.primary`(`--surface`) / 顶分隔 `border.subtle`(`--line`) /
 * 未选中 `text.tertiary`(`--fg-faint`) / 选中图标·文字·指示条 `interactive.primary`(`--flow`)。
 * 几何与停靠规则全在 `mobile.css` 的 `.m-nav*`（含宽容器下"居中 + 单项封顶"，不拉伸到拇指够不着）。
 *
 * ⚠️ 上面四个值里有三个**与 `bottom-navigation.md` 明文不同**（它写 24 / 40 / `divider.default`
 * / `text.secondary`），取的是原型 `../prototype/mobile-home-breakpoints.html:156,160,162,163`。
 * 依据是 kit `README.md`「Source of truth」那句：分歧时原型胜、kit 是错的。逐条理由写在
 * `mobile.css` 对应声明处 —— **这段注释本身就是回改的诱因**，改前先看那里。
 * 那四个值由 `bottom-navigation-kit-conflict.test.ts` 钉住：照 md 改回去会当场转红。
 *
 * **目的地表不在本文件里**：本组件只渲染 `DESTINATIONS`。导航自带一份目的地字面量是这类外壳最典型的
 * 失效形态 —— 改了路由没改导航（或反过来）不报错，只表现为"点了没反应/有屏进不去"。
 * `mobile-entry.test.ts` 为此断言本文件不含任何目的地标签字面量，且导航确实是 `DESTINATIONS.map(` 画出来的。
 *
 * 图标**复用**桌面 `components/Icons.tsx` 里那五个（1:1 提取自原型的 nav-item SVG，纯 `SVGProps`
 * 组件、零 CSS 依赖）。spec 要求的正是"existing outline icon"，另画一套等于凭空多一份要对齐的资产。
 */

import type { ReactElement, SVGProps } from 'react';
import { useTranslation } from 'react-i18next';
import {
  NavConnectionsIcon,
  NavHomeIcon,
  NavNodesIcon,
  NavRulesIcon,
  NavSettingsIcon,
} from '@/components/Icons';
import { DESTINATIONS, type DestinationId } from './destinations';

/** id → 图标。`Record<DestinationId, …>` ⇒ 新增目的地却忘了配图标，编译期即红。 */
const ICONS: Record<DestinationId, (p: SVGProps<SVGSVGElement>) => ReactElement> = {
  home: NavHomeIcon,
  nodes: NavNodesIcon,
  rules: NavRulesIcon,
  connections: NavConnectionsIcon,
  settings: NavSettingsIcon,
};

export function BottomNavigation({
  active,
  onSelect,
}: {
  active: DestinationId;
  /** 点非当前项 = 切换；**再点当前项** = 该栈回顶（spec「Interaction」），由外壳消费。 */
  onSelect: (id: DestinationId) => void;
}): ReactElement {
  const { t } = useTranslation();
  return (
    // aria-label 复用桌面侧栏那条 `sidebar.primaryNav`（"主要导航"/"Primary navigation"）——
    // 它描述的是同一件事，且五语种早已齐备；为它另开一条 key 只会多一份要翻译的同义文案。
    <nav className="m-nav" aria-label={t('sidebar.primaryNav')}>
      {DESTINATIONS.map((d) => {
        const Icon = ICONS[d.id];
        const isActive = d.id === active;
        return (
          <button
            aria-current={isActive ? 'page' : undefined}
            className="m-nav-item"
            key={d.id}
            onClick={() => onSelect(d.id)}
            type="button"
          >
            {/* 指示条是 ::before 画的（见 mobile.css）：它不参与 flex 排布，故不挤压图标与文字。 */}
            <Icon className="m-nav-icon" />
            <span className="m-nav-label">{t(d.labelKey)}</span>
          </button>
        );
      })}
    </nav>
  );
}

/**
 * 「规则」屏用到的移动端呈现原语（IA spec §3.1 增量表的落地）。
 *
 * # 为什么不 import 桌面组件
 *
 * 不是排期问题，是**契约冲突**。桌面这些控件的外观全落在 `.card` / `.btn` / `.pill` / `.swt` /
 * `.mini-menu` 这些类上，而这些类住在 `prototype.css` / `components.css` —— 桌面那条五层层叠链。
 * 移动入口只许走 `tokens.resolved.css`（契约 A1），`import` 桌面组件就等于把整条桌面 CSS 拖进
 * 移动包，`mobile-entry.test.ts` 的 CSS 集合判据当场红。
 *
 * 能原样复用的是**逻辑**（`domain/*` 的纯函数、`store/*` 的 hook、`lib/*` 的工具），本屏容器
 * 正是这么做的 —— 缺的只是一套移动端呈现，这个文件就是那套呈现。
 *
 * # 三条不许破的规矩（IA §3.3）
 *
 * 1. **桌面两套词汇在移动端仍是两套。** `.sub-tabs`（`SegmentedTabs`）与 `.seg2`（`Choice`）看着像，
 *    但桌面它们就是两个控件；合并会造出一个两端都不存在的移动端专有词汇。
 * 2. **被拿掉的控件要么缺席带理由、要么在场且禁用带理由，绝不静默消失。** 桌面已经在守这条线并写明
 *    了理由：`NodeCard.tsx:371-374` 宁可留一颗禁用的测速按钮也不隐藏，因为「按钮凭空少一个，
 *    用户只会以为这张卡坏了」。
 * 3. **拿不到的数不许显示。** 同首页卡片那条规则，用到行与角标上。
 *
 * # 图标：登记引用，不新画资产
 *
 * 每个 `svg` 的路径数据都逐字来自桌面客户端的同一个图标（下面各处注明 file:line），
 * 并按 IA §3.2 的口径登记进 `mobile-kit/icons/icon-registry.json`。契约 D-1 的规矩是
 * **omit or request, never invent** —— 没登记的图标宁可不画。
 *
 * 🔴 **规则类型图标（[`TypeIcon`]）本批补上，此前的「刻意不画」理由已被推翻。**
 * 旧理由是「条件摘要那行（`域名后缀 ×15 · 规则集 ×4`）已经用文字说出了同一件事，且说得更准」。
 * 它对**摘要在场**的行成立，而摘要只在 `conds.length > 0` 时渲染（`RuleRow.tsx` 的条件分支）——
 * 一条零条件规则（匹配全部流量）在移动端行上**一个字都没有**，分类图标是那一行仅剩的类型线索。
 * 桌面两处都在场（行首图标 + hover 卡的「（无条件）」），移动端此前两处都没有。
 * 五档字形逐字取自 `RuleItem.tsx:90-129`，与其余图标同一条出处纪律。
 *
 * # `data-tip` 在本文件里一次都不出现
 *
 * 触屏没有 hover。桌面靠 `data-tip` 独家承载的解释，在这里一律落成**常驻通道**：行下面的
 * `.mr-note`、动作面板条目的第二行、或开关旁边的说明行（IA §4.12）。
 * 判据在 `rules-screen.test.tsx`：本目录源码里出现 `data-tip` 即红。
 */

import { useEffect, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { useDismissableLayer } from '../../back-stack';
import { SheetHeading } from '../../SheetHeading';
import { useSheetFocus } from '../../use-sheet-focus';
import { revealOnToggle, revealSiblingGroup, useRevealAfterCommit } from '@/components/reveal';
import type { RuleType } from '@/contracts/types';
import { RULE_TYPE_CATEGORY } from '@/domain/rules';
import { buildCselRows, type CselGroup } from '@/components/dialogs/csel-logic';

/* ═══════════ 图标（全部登记在 icon-registry.json，源自桌面同一资产）═══════════ */

/** `action.add` —— 桌面 `RulesScreen.tsx:524-526` / `NodesHeader.tsx:60-62`。 */
export function AddIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

/** `action.edit` —— 桌面 `RuleItem.tsx:157-161`。 */
export function EditIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M12 20h9M16.5 3.5a2.1 2.1 0 013 3L7 19l-4 1 1-4z" />
    </svg>
  );
}

/** `action.delete` —— 桌面 `RuleItem.tsx:478-480`。 */
export function DeleteIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M4 7h16M9 7V5h6v2M6 7l1 13h10l1-13" />
    </svg>
  );
}

/** `action.duplicate` —— 桌面 `RuleItem.tsx:451-453`。 */
export function DuplicateIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M9 9h10v10H9zM5 15V5h10" />
    </svg>
  );
}

/**
 * `action.move-top` / `.move-up` / `.move-down` / `.move-bottom` —— 桌面 `RuleItem.tsx:385-423`。
 *
 * 四条 path **逐字写在 JSX 里**，不走 `d={d}` 那种算出来的形态：字形出处那道门
 * （`rules-screen.test.tsx` ⑭）比对的是几何指纹，几何属性一旦是 JS 表达式，它取不到指纹，
 * 而它此前的失效形态是 **fail-open**（取不到就退化成「能匹配」）—— 这四枚字形从来没被比对过。
 * 渲染结果不变：`to` 是四值联合，恰好一支为真。
 */
export function MoveIcon({ to }: { to: 'top' | 'up' | 'down' | 'bottom' }): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      {to === 'top' && <path d="M5 5h14M12 20V9M7 14l5-5 5 5" />}
      {to === 'up' && <path d="M12 19V6M6 12l6-6 6 6" />}
      {to === 'down' && <path d="M12 5v13M6 12l6 6 6-6" />}
      {to === 'bottom' && <path d="M5 19h14M12 4v11M7 10l5 5 5-5" />}
    </svg>
  );
}

/**
 * `action.drag-handle` —— 桌面 `RuleItem.tsx:143-150` 的 `GripIcon`，六个圆点逐字。
 *
 * IA §3.2 的图标增量表已经为 §4.13 登记了这一格。桌面的抓握条挂在 `.rule-pri` 之后
 * （`RuleItem.tsx:280-282`），移动端同位。
 */
export function GripIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="9" cy="6" r="1" />
      <circle cx="15" cy="6" r="1" />
      <circle cx="9" cy="12" r="1" />
      <circle cx="15" cy="12" r="1" />
      <circle cx="9" cy="18" r="1" />
      <circle cx="15" cy="18" r="1" />
    </svg>
  );
}

/** `action.more` —— 桌面 `SubInfoBar.tsx:101-107`。 */
export function MoreIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="5" cy="12" r="1.6" />
      <circle cx="12" cy="12" r="1.6" />
      <circle cx="19" cy="12" r="1.6" />
    </svg>
  );
}

/**
 * `select-sheet` 的下箭头 —— 桌面 `dialogs/Csel.tsx:369` 的 `.csel-chev`，路径逐字。
 * 它是 `Csel` 触发器读起来「这是个可以展开选项的东西」的那一笔，不是装饰。
 */
export function ChevronDownIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

/**
 * `select-sheet` 的选中勾 —— 桌面 `dialogs/Csel.tsx:341` 的 `.csel-ck`，路径与线宽逐字。
 * IA §3.1 #20 明写它要跟过来：「Group headers, **the selected check**, the description line
 * and the danger variant carry over.」它也是 `select-sheet` 与 `action-sheet` 在视觉上
 * 分得开的那一处 —— §3.3 第 1 条要求这两套词汇在移动端仍是两套。
 */
export function CheckIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.4} aria-hidden>
      <path d="M5 12l5 5 9-11" />
    </svg>
  );
}

/**
 * 可折叠**分组组头**的右向箭头 —— 桌面 `dialogs/Csel.tsx:305` 的 `.csel-grp-chev`
 * 与 `app-policy/AppPolicyScreen.tsx:958` 的 `.ns-chev` 用的是同一条路径、同一个线宽。
 * 展开态由 CSS 旋转它（与桌面同形），不换字形 —— 换字形会让「展开/收起」读成两个不同的控件。
 */
export function ChevronRightIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} aria-hidden>
      <path d="M9 6l6 6-6 6" />
    </svg>
  );
}

/**
 * 规则类型的分类图标（domain / network / device / process / ruleset 五档）。
 *
 * **分类判据不在这里**：`RULE_TYPE_CATEGORY` 派生自 `domain/rules.ts` 的 15 份描述符表
 * （`RULE_TYPES[id].category`），本组件只做「分类 → 字形」这一跳。加第 16 个规则类型时
 * 这里一行都不用改 —— 那正是判据住在描述符表里的意义。
 *
 * 五档字形逐字取自桌面 `screens/rules/RuleItem.tsx:90-129`。
 * ⚠️ `ruleset` 与 `domain` 只差第二条 path（经线 vs 十字线），照抄时最容易混的就是这一格。
 */
export function TypeIcon({ type }: { type: RuleType }): ReactElement {
  switch (RULE_TYPE_CATEGORY[type]) {
    case 'ruleset':
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <circle cx="12" cy="12" r="9" />
          <path d="M3 12h18M12 3c3 3 3 15 0 18" />
        </svg>
      );
    case 'process':
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <rect x="4" y="4" width="16" height="16" rx="2" />
          <path d="M9 9h6v6" />
        </svg>
      );
    case 'network':
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <path d="M4 7h16M4 12h16M4 17h10" />
        </svg>
      );
    case 'device':
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <rect x="3" y="4" width="18" height="12" rx="1.5" />
          <path d="M8 20h8M12 16v4" />
        </svg>
      );
    default:
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <circle cx="12" cy="12" r="9" />
          <path d="M12 2v20M2 12h20" />
        </svg>
      );
  }
}

/** `action.search` —— 桌面 `NodesToolbar.tsx:74-77`。 */
export function SearchIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-3-3" />
    </svg>
  );
}

/** `action.refresh` —— 桌面 `SubInfoBar.tsx:71-77`。 */
export function RefreshIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M4 4v6h6M20 20v-6h-6" />
      <path d="M4 10a8 8 0 0114-3M20 14a8 8 0 01-14 3" />
    </svg>
  );
}

/** `action.close` —— 桌面 `ConnectionsScreen.tsx:1027-1029`。 */
export function CloseIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M5 5l14 14M19 5L5 19" />
    </svg>
  );
}

/** `state.warning` —— 桌面 `RulesScreen.tsx:575-578`。 */
export function WarningIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 8v5M12 16h.01" />
    </svg>
  );
}

/** `state.info` —— 桌面 `RulesScreen.tsx:684-687`（`.info-i` 里那颗）。 */
export { InfoIcon } from '../../InfoIcon';

/**
 * 网络场景（spec §6.1）—— 路径数据逐字取自桌面 `NetworkProfilePanel.tsx#ProfileIcon`
 * （与桌面规则页页头那颗入口同一个字形），不另画（契约 D-1 / ⑭ 字形出处门）。
 */
export function NetworkProfileIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M5 12.5a10 10 0 0114 0M8.5 16a5 5 0 017 0M12 19.5h.01M2 9a14.5 14.5 0 0120 0" />
    </svg>
  );
}

/**
 * `navigation.chevron` —— 已在注册表里（指向 `Icons.tsx#ChevronLeftIcon`），此处复用作返回箭头。
 * 路径数据逐字取自那份资产：契约 D-1 是「复用现有客户端的资产」，不是「画一个差不多的」。
 */
export function BackIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M15 6l-6 6 6 6" />
    </svg>
  );
}

/* ═══════════ 版式原语 ═══════════ */

/**
 * 行的多列流（IA §2.2「Breakpoints」最后一句）。
 *
 * medium / expanded 下把**无序集合**（应用行 / 资源行 / DNS 服务器与分组）铺成两列。
 * 规范对这一面用的是 “**may** flow into two columns” —— 可做项。
 * **有序的规则列表不许进来**，它走 [`OrderedList`]：行号就是优先级，双列会让第 5 条落在
 * 第 1 条右边 ⇒ 阅读序不再等于求值序。
 *
 * 判据按「祖先链里不许出现 `data-flow="multi"`」写，不是按「不许出现 `.mr-flow`」写 ⇒
 * 多列入口可以有两个（另一个是 [`Cards`]），但**每一个都必须挂这个标记**。
 * 漏挂标记才是给判据漏一条路，多一个类名不是。
 */
export function Flow({ children }: { children: ReactNode }): ReactElement {
  return (
    <div className="mr-flow" data-flow="multi">
      {children}
    </div>
  );
}

/**
 * 卡片块的多列流（IA §2.2「Breakpoints」第二条）。
 *
 * 规范对这一面用的是 “two-column multi-column flow **for the card-shaped blocks only**” ——
 * 卡片块是**必做面**，行列表才是可做面。之前两者反了：唯一的双列机制挂给了行列表，
 * 于是平板上流量段 / DNS 段的卡片各自铺满整屏宽、纵向排成一条，右侧全是空白，
 * 而同一台设备上连接屏的两张卡是并排的。
 *
 * 与 [`Flow`] 共用 `data-flow="multi"` 标记：有序列表的祖先链判据因此同时覆盖这条路。
 * 有序列表**留在本容器之外**（`OrderedList` 是 `.mr-cards` 的兄弟，不是它的孩子）。
 */
export function Cards({ children }: { children: ReactNode }): ReactElement {
  return (
    <div className="mr-cards" data-flow="multi">
      {children}
    </div>
  );
}

/**
 * 有序列表 —— 恒单列，任何断点都不进 [`Flow`]。
 *
 * `data-ordered="1"` 是判据的锚：门渲染出静态 DOM 后按它定位，再往上走祖先链，
 * 撞见 `data-flow="multi"` 即红。
 */
export function OrderedList({
  children,
  label,
}: {
  children: ReactNode;
  label?: string;
}): ReactElement {
  return (
    <div className="mr-list" data-ordered="1" role="list" aria-label={label}>
      {children}
    </div>
  );
}

export function Card({
  children,
  title,
  note,
  className,
}: {
  children?: ReactNode;
  title?: ReactNode;
  /** 常驻说明行 —— 桌面挂在 `.info-i` 的 `data-tip` 在这里变成它（§4.12）。 */
  note?: ReactNode;
  className?: string;
}): ReactElement {
  return (
    <section className={className ? `mr-card ${className}` : 'mr-card'}>
      {title != null && <div className="mr-card-h">{title}</div>}
      {note != null && <div className="mr-note">{note}</div>}
      {children}
    </section>
  );
}

/* ═══════════ 通告条 / 空态 / 进度 ═══════════ */

/** 三档严重度保持三档（IA §3.1 #1）。 */
export function NoticeBanner({
  tone,
  text,
  action,
}: {
  tone: 'warn' | 'info' | 'off';
  text: ReactNode;
  action?: { label: string; onClick: () => void };
}): ReactElement {
  return (
    <div className={`mr-notice ${tone}`} role={tone === 'warn' ? 'alert' : undefined}>
      <WarningIcon />
      <span className="mr-notice-tx">{text}</span>
      {action && (
        <button type="button" className="mr-btn ghost" onClick={action.onClick}>
          {action.label}
        </button>
      )}
    </div>
  );
}

export function EmptyState({ text }: { text: ReactNode }): ReactElement {
  return <div className="mr-empty">{text}</div>;
}

/**
 * 写操作失败的**行内**回显 —— 本屏唯一一条对用户可见的失败通道。
 *
 * # 为什么必须有它（立项时的根因：`toast` 在移动端是个 console）
 *
 * `lib/error-handler.ts:62-71` 的 `toast` 默认实现是 `consoleToast`（只 `console.*`），
 * 真实实现由 `setToastImpl` 注入，而立项当时**唯一**的注入点是 `components/layout/Toaster.tsx`，
 * 它只挂在桌面外壳上 ⇒ 移动端 `toast.error(...)` = `console.error(...)`：写失败时用户看到的只有
 * 「开关自己弹回去、列表项跳回原样」，**一句话都没有**。那正是 W10/W14 那批真机缺陷的形态
 * （`contracts/action-failure-visibility.test.ts`：失败被静默吞掉，用户只看到按钮点下去毫无反应）。
 *
 * ⚠️ **2026-09-06 起移动端有自己的宿主了**（`mobile/MobileToaster.tsx`），本屏那 24 处 `toast.*`
 * 因此不再静音。**本组件照旧**，理由从「没有宿主」换成产品口径：
 *
 * # 为什么是行内而不是只靠那个全局宿主
 *
 * 状态弹回**原值** + 紧挨着控件的一行「保存失败」比飘过去的浮层更可发现（用户的视线本来就在那），
 * 而这一屏一次可以有好几处并发的写。两者不互斥：toast 说「发生了什么」，行内说「是哪一颗」。
 *
 * `role="alert"` 让读屏当场播报；`data-write-error` 是门的锚。
 */
export function InlineError({ text }: { text?: string }): ReactElement | null {
  if (text == null || text === '') return null;
  return (
    <div className="mr-error" role="alert" data-write-error="1">
      {text}
    </div>
  );
}

/**
 * 任务进度条（IA §3.1 #5）。与首页那条「份额条」**刻意不共用**：
 * 一条说「这件事完成了 60%」，一条说「这个出站占了 60% 的字节」——同形反义。
 */
export function ProgressBar({ ratio, label }: { ratio: number; label: string }): ReactElement {
  const pct = Math.max(0, Math.min(1, ratio)) * 100;
  return (
    <div
      className="mr-progress"
      role="progressbar"
      aria-label={label}
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <span style={{ width: `${pct}%` }} />
    </div>
  );
}

/* ═══════════ 分段条 / 二选一 / 开关 / 搜索 ═══════════ */

export type SegmentItem<T extends string> = { id: T; label: string; count?: number };

/** `segmented-tabs`（桌面 `.sub-tabs`）：横向可滚，每项 ≥48 高，计数徽标与下划线不变。 */
export function SegmentedTabs<T extends string>({
  items,
  active,
  onSelect,
  label,
}: {
  items: ReadonlyArray<SegmentItem<T>>;
  active: T;
  onSelect: (id: T) => void;
  label: string;
}): ReactElement {
  return (
    <div className="mr-seg" role="tablist" aria-label={label}>
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          role="tab"
          className="mr-seg-item"
          aria-selected={item.id === active}
          onClick={() => onSelect(item.id)}
        >
          <span>{item.label}</span>
          {item.count != null && <span className="mr-cnt">{item.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** `segmented-control`（桌面 `.seg2`）：与上面那个**不是**同一个控件，别合并。 */
export function Choice<T extends string>({
  items,
  active,
  onSelect,
  label,
}: {
  items: ReadonlyArray<{ id: T; label: string }>;
  active: T;
  onSelect: (id: T) => void;
  label: string;
}): ReactElement {
  return (
    <div className="mr-geo-regions" role="group" aria-label={label}>
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          className="mr-btn"
          aria-pressed={item.id === active}
          onClick={() => onSelect(item.id)}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

/**
 * 开关（IA §3.1 #11）：48×48 命中面包住一个与桌面逐像素相同的视觉。视觉不变，命中面变。
 *
 * **它只画开关本体**。带说明的场合一律走 [`SwitchRow`] —— 让开关自己吐一段说明，会把那段文字
 * 塞进调用处的 flex 行里挤成一条竖着的字，那不是「常驻可读」而是「常驻但读不了」。
 */
export function Switch({
  checked,
  onChange,
  label,
  disabled,
  id,
  describedBy,
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
  disabled?: boolean;
  id: string;
  describedBy?: string;
}): ReactElement {
  return (
    <button
      type="button"
      id={id}
      className="mr-switch"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      disabled={disabled}
      onClick={onChange}
    />
  );
}

/**
 * 带标题与说明的开关行（桌面 `settings-row`，IA §3.1 #8）。
 *
 * `description` 是 §4.12 的主要落点之一：桌面把「这个开关管什么 / 为什么不能动」挂在 `data-tip` 上，
 * 触屏没有 hover ⇒ 这里它是**常驻可见的第二行**，并经 `aria-describedby` 与开关绑定
 * （给读屏用户的是同一句，不是另写一份）。
 */
export function SwitchRow({
  id,
  title,
  description,
  checked,
  onChange,
  disabled,
}: {
  id: string;
  title: string;
  description?: string;
  checked: boolean;
  onChange: () => void;
  disabled?: boolean;
}): ReactElement {
  const descId = description != null ? `${id}-desc` : undefined;
  return (
    <div className="mr-switch-row">
      <div className="mr-switch-tx">
        <div className="mr-switch-title">{title}</div>
        {description != null && (
          <div className="mr-note" id={descId}>
            {description}
          </div>
        )}
      </div>
      <Switch
        id={id}
        checked={checked}
        onChange={onChange}
        label={title}
        disabled={disabled}
        describedBy={descId}
      />
    </div>
  );
}

/** `search-field`（IA §3.1 #9）：48 高、带清除按钮（触屏没有 Esc）、平台搜索键盘。 */
export function SearchField({
  value,
  onChange,
  placeholder,
  clearLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder: string;
  clearLabel: string;
}): ReactElement {
  return (
    <label className="mr-search">
      <SearchIcon />
      <input
        type="search"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
      {value !== '' && (
        <button
          type="button"
          className="mr-btn ghost icon"
          aria-label={clearLabel}
          onClick={() => onChange('')}
        >
          <CloseIcon />
        </button>
      )}
    </label>
  );
}

/* ═══════════ 折叠 / 优先级链 ═══════════ */

/** `fold`（桌面 `components/Fold.tsx`）：带计数徽标的展开，本身就是个原生触控目标。 */
export function Fold({
  title,
  count,
  note,
  children,
}: {
  title: ReactNode;
  count?: number;
  /** 桌面 `Fold` 的 `tip` 在这里变成常驻行（§4.12）。 */
  note?: ReactNode;
  children: ReactNode;
}): ReactElement {
  return (
    /* `onToggle` 是全仓不变量（`components/reveal.test.ts`）：展开后不把新内容滚进视区，
       用户会以为点了没反应 —— 折叠块越靠近屏底越明显。复用共享实现，不自造第二份。 */
    <details className="mr-fold" onToggle={revealOnToggle}>
      {/*
        说明行放在 `summary` **里**，不是 details 的内容里：放内容里等于「展开才看得见」，
        而 §4.12 要的是常驻可达 —— 折叠态下看不见的解释，和挂在 hover 上没有区别。
      */}
      <summary>
        <span className="mr-fold-tx">
          <span>{title}</span>
          {note != null && <span className="mr-note">{note}</span>}
        </span>
        {count != null && <span className="mr-cnt">{count}</span>}
      </summary>
      <div className="mr-fold-body">{children}</div>
    </details>
  );
}

/** `priority-flow`（桌面 `rules/PriorityFlow.tsx`）：静态链，零交互。 */
export function PriorityFlow({
  label,
  steps,
}: {
  label: string;
  steps: ReadonlyArray<{ id: string; label: string; active?: boolean }>;
}): ReactElement {
  return (
    <div>
      <div className="mr-note">{label}</div>
      <div className="mr-chain">
        {steps.map((step, i) => (
          <span key={step.id} className="mr-chain">
            <span className={step.active ? 'mr-chain-step on' : 'mr-chain-step'}>{step.label}</span>
            {i < steps.length - 1 && <span className="mr-chain-arrow">›</span>}
          </span>
        ))}
      </div>
    </div>
  );
}

/* ═══════════ 选择面板 ═══════════ */

/**
 * `select-sheet`（IA §3.1 #20，桌面 `dialogs/Csel.tsx`）。
 *
 * # 为什么它不能是 `segmented-tabs`
 *
 * §3.3 第 1 条：**桌面两套词汇在移动端仍是两套**，`.mini-menu` 与 `Csel` 同理。
 * 分类筛选在桌面就是 `Csel`（`AppPolicyScreen.tsx:466` 的 `<Csel className="ap-cat">`），
 * 把它并进本屏的 `.sub-tabs`（[`SegmentedTabs`]）会造出一个两端都不存在的移动端专有词汇。
 *
 * 还有一条与形式无关的理由，桌面在 `AppPolicyScreen.tsx:462-465` 已经写过：平铺 chips 的
 * 横向占用随类目数线性增长，而下拉的宽度与类目数无关。390 宽的屏上，平铺意味着屏幕外的
 * 类目**没有任何可见提示** —— 用户得先横向拨一下才知道还有别的分类。
 *
 * # 与 `action-sheet` 的分界
 *
 * 容器共用（`bottom-sheet`，§3.1 #14 就是干这个的），但条目不同：这里每一行是一个
 * `role="option"`，选中那行带勾（`CheckIcon`）；`action-sheet` 的行是命令，带图标、分隔线、
 * 危险变体，没有选中态。两者读起来必须不一样。
 *
 * 面板的开合态由容器持有（与本屏其余面板同一套），本组件是纯呈现。
 *
 * # 为什么触发器与面板是**两个**组件
 *
 * 面板（`.mr-sheet-scrim`，`position: fixed`）必须能挂在**没有任何 opacity/transform 祖先**的
 * 地方。本屏的应用分段在总开关关闭时给正文加 `.mr-ap-body.dim{opacity:.5}` —— `opacity < 1`
 * 会把整棵子树当**一组**去绘制、并就地建立层叠上下文：面板连同它的遮罩一起变成半透明，
 * 且因为 `.mr-ap-body` 自身非定位（`z-index: auto`），整组会排在 `position: sticky; z-index: 2`
 * 的屏头**之前**绘制 ⇒ 屏头压在遮罩上面。而置灰态恰恰是规范要求「控件仍可编辑」的那个态，
 * 也就是这个筛选器本该可用的时候。
 *
 * 合成一个组件就没法把这两半放到不同的 DOM 位置，于是拆成
 * `SelectSheetTrigger`（留在正文里）+ `SelectSheetPanel`（由容器渲染在置灰包裹之外），
 * 与本屏 `ActionSheet` 的摆法一致。
 */
/**
 * 一个选项。`disabled` / `danger` 两格与 [`SheetAction`] **同名同义**（§3.3 第 1 条只禁止把两套
 * 词汇并成一套，不禁止它们的修饰词同名 —— 同名反而让「哪一档是危险的」在两套里读法一致）。
 *
 * `disabled` 的用法有硬约束：**只用于「这一档在本平台兑现不了」，不用于「本批还没做」**。
 * 后者该是整颗控件不出现或另有登记，塞进选项列表里等于给用户一个永远点不动的档位。
 * 用它的地方必须同时给面板传 `note`，把理由写成可见文字（§4.12：解释要有常驻通道）。
 */
type SelectSheetOption = {
  id: string;
  label: string;
  description?: string;
  disabled?: boolean;
  danger?: boolean;
};

export function SelectSheetTrigger({
  label,
  options,
  value,
  valueLabel,
  toneClass,
  open,
  onOpen,
}: {
  /** 控件的可访问名（触发器的 `aria-label` + 面板标题）。 */
  label: string;
  options: ReadonlyArray<SelectSheetOption>;
  value: string;
  /**
   * 当前值的显示文案，覆盖按 `options` 反查出来的那一条。
   *
   * 存在的理由只有一个：选中项的文案是**运行期数据**时，选项表里放不下它。应用分流的「指定节点」
   * 这一档，触发器上要显示的是那个节点的名字，而选项表里那一行只能写「指定节点」——
   * 不给这一格，触发器会把用户选中的 `HK-01` 显示成「指定节点」，读作「我选的节点没了」。
   */
  valueLabel?: string;
  /**
   * 追加在触发器根上的调色类（如 `act-direct` / `act-block`）。
   *
   * 触发器承载的是**当前动作**，而「阻断」在动作标签轴上恒 `--err` 且常驻
   * （`styles/style-invariants.test.ts` 的两轴不变量）—— 收起来的下拉是这个动作唯一的常驻载体，
   * 不给它上色，这一档在本屏就没有颜色了。
   */
  toneClass?: string;
  open: boolean;
  onOpen: () => void;
}): ReactElement {
  const current = options.find((o) => o.id === value);
  return (
    <button
      type="button"
      className={['mr-sel-trigger', toneClass ?? ''].filter(Boolean).join(' ')}
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-label={label}
      onClick={(event) => { event.currentTarget.focus({ preventScroll: true }); onOpen(); }}
    >
      {/* 当前值必须在触发器上看得见 —— 收起来的下拉如果不说自己选了什么，它就只是一颗按钮。 */}
      <span className="mr-sel-val">{valueLabel ?? current?.label ?? value}</span>
      <ChevronDownIcon />
    </button>
  );
}

/** 一个空的展开集：模块级常量，免得每次渲染都新建一个 Set 把 effect 依赖搅乱（同桌面 `NO_OPEN_GROUPS`）。 */
const NO_OPEN_GROUPS: ReadonlySet<string> = new Set<string>();

/**
 * 选项面板。**两种入参二选一**：
 *  · `options` —— 扁平档位表（策略四档、分类筛选…）；
 *  · `groups`  —— 分组表（`CselGroup`，与桌面 `Csel` **同一个类型**）。带 `id` 的组可折叠。
 *
 * # 为什么分组这一维要跟过来（`Csel.tsx|f:header` 的移动端落点）
 *
 * 桌面用可折叠组头把 15 种规则类型收进 5 组、把几十个节点按订阅收进若干组。触屏上这一维比桌面
 * **更**要紧：一个 390 宽的屏摆不下几十行平铺选项，全展开等于把用户扔进一条读不完的滚动条。
 * 此前移动端的 sheet 只有平铺 + 分隔线，这一维整条没有。
 *
 * 行的摊平走 `csel-logic` 的 [`buildCselRows`]（**同一份判据**，不在这里重写）：
 * 组头不占扁平索引、折叠组的选项不渲染但索引照占。
 *
 * 折叠态由本组件自持并**在每次打开时重置**为 `openGroupIds`（同桌面 `Csel.tsx:158-159`）——
 * 不跨次残留：上一次展开过哪几组，与这一次要看哪一组没有关系；而「含当前选中项的那组默认展开」
 * 由调用方用 `defaultOpenGroupIds` 算好传进来（`domain/server-grouping`，三处选择器共用）。
 */
export function SelectSheetPanel({
  label,
  note,
  options,
  groups,
  value,
  open,
  onClose,
  onSelect,
  closeLabel,
  openGroupIds,
}: {
  label: string;
  /**
   * 面板顶部的只读说明块（与 [`ActionSheet`] 的 `body` 同一格、同一个类）。
   *
   * 有 `disabled` 选项时**必须**给：一个点不动的档位，如果不说为什么点不动，它与「控件坏了」
   * 在用户眼里长得一模一样。
   */
  note?: ReactNode;
  /** 扁平档位表。与 `groups` 二选一（两个都给时以 `groups` 为准，调用点不该同时给）。 */
  options?: ReadonlyArray<SelectSheetOption>;
  /** 分组表（可折叠组带 `id`）。与桌面 `Csel` 共用 `CselGroup`，不另造一套形状。 */
  groups?: readonly CselGroup[];
  value: string;
  open: boolean;
  onClose: () => void;
  onSelect: (id: string) => void;
  closeLabel: string;
  /** 打开时默认展开的组 id 集合（缺省 = 全折叠，**不猜第一组**）。 */
  openGroupIds?: ReadonlySet<string>;
}): ReactElement {
  /* 系统返回键关掉的就是这一层。本组件由容器**无条件**渲染（面板要落在置灰包裹之外），
     开合靠 `open` ⇒ 登记也跟着 `open` 走，不是恒真。 */
  useDismissableLayer(open, onClose);
  const panel = useRef<HTMLDivElement>(null);
  useSheetFocus(panel, open, onClose, { initialFocus: '[aria-selected="true"]:not(:disabled)' });
  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(openGroupIds ?? NO_OPEN_GROUPS);
  /*
   * 展开之后把**整段**滚进视区（全仓不变量，`components/reveal.test.ts` 守着）。
   *
   * 面板是 `max-height` + `overflow-y:auto` 的滚动容器：组头落在底部时展开，新出现的节点整段
   * 在视区之外 —— 用户看到的是「点了组头，什么都没发生」。
   * 用 `revealSiblingGroup` 而不是 `revealElement`：这里的组头与选项是**扁平兄弟**
   * （同在 `.mr-sel-list` 里），没有分组容器可滚，与桌面四个菜单同一形。
   */
  const scheduleReveal = useRevealAfterCommit();
  /* 每次**打开**重置（同桌面 `openMenu()`）。写成 effect 而不是渲染期赋值：`open` 由容器持有，
     组件不卸载，没有「挂载即初始化」这条路可走。 */
  useEffect(() => {
    if (open) setOpenGroups(openGroupIds ?? NO_OPEN_GROUPS);
  }, [open, openGroupIds]);

  const toggleGroup = (id: string, header: HTMLElement): void => {
    const wasOpen = openGroups.has(id);
    setOpenGroups((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    /* 只在**展开**那一下露出（收起时没有新内容可露）。`useRevealAfterCommit` 把它推到
       提交之后 —— 点的那一刻子项还没渲染，此时量出来的段长恒等于组头自己。 */
    scheduleReveal(wasOpen ? null : () => revealSiblingGroup(header));
  };

  /** 一个选项行。两种入参归一到这里，勾与危险变体只有一份实现。 */
  const optionRow = (opt: {
    id: string;
    label: string;
    description?: string;
    disabled?: boolean;
    danger?: boolean;
  }): ReactElement => (
    <button
      key={opt.id}
      type="button"
      className={['mr-sel-opt', opt.danger === true ? 'danger' : ''].filter(Boolean).join(' ')}
      role="option"
      aria-selected={opt.id === value}
      disabled={opt.disabled}
      onClick={() => onSelect(opt.id)}
    >
      <span className="mr-sel-opt-tx">
        <span>{opt.label}</span>
        {opt.description != null && (
          <span className="mr-sheet-item-sub">{opt.description}</span>
        )}
      </span>
      {/* 勾恒在 DOM 里、按 `aria-selected` 决定可见性：否则「有没有勾」会随
          渲染时机抖动，而它是这套词汇与 `action-sheet` 的唯一视觉分界。 */}
      <span className="mr-sel-ck" aria-hidden>
        <CheckIcon />
      </span>
    </button>
  );

  const body =
    groups !== undefined
      ? buildCselRows(groups, openGroups).map((row) =>
          row.kind === 'header' ? (
            row.groupId === undefined ? (
              /* 不可折叠组：纯视觉分隔（规则类型那 15×5 分组即此形态，与桌面 `.csel-grp` 同）。 */
              <div key={row.key} className="mr-sel-grp" role="presentation">
                {row.label}
              </div>
            ) : (
              /* 可折叠组头。**刻意不给 role**（listbox 里没有「可展开组头」这个角色）：
                 靠原生 button 的隐含角色 + `aria-expanded`，与桌面 `.csel-grp-t` / `.ns-grp` 同形。 */
              <button
                key={row.key}
                type="button"
                className={`mr-sel-grp mr-sel-grp-t${row.collapsed ? '' : ' open'}`}
                aria-expanded={!row.collapsed}
                onClick={(event) => toggleGroup(row.groupId as string, event.currentTarget)}
              >
                <ChevronRightIcon />
                <span>{row.label}</span>
                <span className="mr-cnt">{row.count}</span>
              </button>
            )
          ) : (
            optionRow({
              id: row.opt.value,
              label: row.opt.label,
              description: row.opt.description,
              disabled: row.opt.disabled,
              danger: row.opt.danger,
            })
          ),
        )
      : (options ?? []).map((opt) => optionRow(opt));

  return (
    <>
      {open && (
        <div
          className="mr-sheet-scrim"
          role="dialog"
          aria-modal="true"
          aria-label={label}
          onClick={onClose}
        >
          <div ref={panel} className="mr-sheet" tabIndex={-1} onClick={(e) => e.stopPropagation()}>
            <div className="mr-sheet-grip" />
            <SheetHeading title={label} onClose={onClose} closeLabel={closeLabel} titleClassName="mr-sheet-h" />
            {note != null && <div className="mr-note">{note}</div>}
            <div className="mr-sel-list" role="listbox" aria-label={label}>
              {body}
            </div>
          </div>
        </div>
      )}
    </>
  );
}

/* ═══════════ 动作面板 ═══════════ */

export type SheetAction = {
  id: string;
  label: string;
  /** 禁用理由 / 补充说明 —— 桌面的 `data-tip` 在这里是可见的第二行（§4.12）。 */
  description?: string;
  icon?: ReactElement;
  danger?: boolean;
  disabled?: boolean;
  /**
   * 「再点一次即执行」的武装态。**必须传，且必须落成 `.confirming` 类** ——
   * 这不是样式，是与 `lib/confirm-twice.ts:110,136` 的**跨文件契约**：武装期间它在 document 上挂
   * capture 阶段的 `pointerdown`，凡是没落在 `.confirming` 里的按下一律 `reset()`。
   * 不带这个类的话，第二次点击的 `pointerdown` 先把 armed 清掉，随后的 `click` 又重新武装 ——
   * 用户看到文案在「删除 ↔ 再点一次确认」之间**无限翻转，永远删不掉**。
   * 桌面 `RuleItem.tsx:474` / `ResourcesScreen.tsx:563` 都带它，移植时最容易掉的就是这一格。
   */
  confirming?: boolean;
  onSelect?: () => void;
};

/**
 * `action-sheet`（IA §3.1 #19）：桌面把浮动菜单锚在触发器上，触屏没有 hover 去开、
 * 没有光标去锚、拇指也够不到屏幕上半部 ⇒ 它变成底部面板。条目行 / 分隔线 / 组标题 /
 * 危险变体全是桌面那套，只有容器与位置变了。
 */
export function ActionSheet({
  title,
  body,
  actions,
  onClose,
  closeLabel,
}: {
  title: string;
  /**
   * 面板顶部的只读说明块。桌面把这份内容放在**悬停详情卡**里（`RuleHoverCard` /
   * `ResourceRefsHoverCard`），触屏没有悬停 ⇒ 它跟着行的溢出入口一起被点开。
   * 与 §4.12 同一条判断：解释不能只挂在一个触屏够不到的通道上。
   */
  body?: ReactNode;
  actions: ReadonlyArray<SheetAction | 'separator'>;
  onClose: () => void;
  closeLabel: string;
}): ReactElement {
  /* 系统返回键关掉的就是这一层。三个调用点（规则行 / 资源行 / DNS 溢出）都只在打开时才渲染
     本组件 ⇒ 恒 `true`；登记住在组件里，故第四个调用点自动带上。 */
  useDismissableLayer(true, onClose);
  return (
    <div
      className="mr-sheet-scrim"
      role="dialog"
      aria-modal="true"
      aria-label={title}
      /* 桌面那条「两次指针事件都落在背景上才关闭」的规矩在这里的对应物是「手势不能起自内容」：
         点在面板本体上不冒泡到这里。 */
      onClick={onClose}
    >
      <div className="mr-sheet" onClick={(e) => e.stopPropagation()}>
        <div className="mr-sheet-grip" />
        <SheetHeading title={title} onClose={onClose} closeLabel={closeLabel} titleClassName="mr-sheet-h" />
        {body != null && <div className="mr-note">{body}</div>}
        {actions.map((action, i) =>
          action === 'separator' ? (
            <div key={`sep-${i}`} className="mr-sheet-sep" />
          ) : (
            <button
              key={action.id}
              type="button"
              className={[
                'mr-sheet-item',
                action.danger === true ? 'danger' : '',
                action.confirming === true ? 'confirming' : '',
              ]
                .filter(Boolean)
                .join(' ')}
              disabled={action.disabled}
              onClick={() => {
                action.onSelect?.();
              }}
            >
              {action.icon}
              <span>
                {action.label}
                {action.description != null && (
                  <span className="mr-sheet-item-sub">{action.description}</span>
                )}
              </span>
            </button>
          ),
        )}
      </div>
    </div>
  );
}

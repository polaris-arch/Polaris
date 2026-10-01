/**
 * 「连接」屏的**纯呈现层**（IA spec §2.3）。props 进、HTML 出，零 hook、零 IPC、零 store。
 *
 * # 为什么容器与呈现要拆开
 *
 * 本仓 vitest 是 node 环境、无 jsdom（既定取舍，先例见 `SubInfoBar.progress.test.tsx` 头注），
 * 门只能靠 `react-dom/server` 真渲染来断言 DOM。拆出这一层，门才能拿夹具直接喂它 ——
 * 而「组件源码里出现过某个名字」不是同一件事：那种断言挡不住「它渲染在一个永不为真的分支里」。
 *
 * # `data-tip` 在本文件里一次都不出现（IA §4.12）
 *
 * 触屏没有 hover。桌面连接表把域名/目标/规则/链路/进程/L4/结束时间的**全文**、以及四颗破坏性
 * 按钮的解释全挂在 `data-tip` 上；这里它们分别落成**展开区的常驻明细**与**动作面板条目的第二行**。
 * 判据在 `connections-screen.test.tsx` ⑦：本目录源码里出现 `data-tip` 即红。
 *
 * # 图标全部来自桌面同一资产（契约 D-1：omit or request, never invent）
 *
 * 每个 `svg` 的路径数据逐字取自桌面客户端，出处在各函数注释里。
 */

import { useState, type ReactElement, type ReactNode } from 'react';
import { useDismissableLayer } from '../back-stack';
import { SheetHeading } from '../SheetHeading';
import { revealSiblingGroup, useRevealAfterCommit } from '@/components/reveal';
import { buildCselRows } from '@/components/dialogs/csel-logic';
import { fmtBytes, fmtDuration, fmtRate } from '@/components/screens/shared/format';
import type { SortState } from '@/components/screens/connections/sort-cycle';
import {
  connRowAge,
  connRowRates,
  TOP_N_OPTIONS,
  type ConnRowVM,
  type ConnSegment,
  type ConnSortKey,
  type LogRowVM,
  type PageSlice,
} from './view-model';

/** 无数可显示时的占位符。**不是文案**：五个语种同形，进 locale 只会多五行同样的字符。 */
const DASH = '—';

export type Translate = (key: string, vars?: Record<string, unknown>) => string;

/* ═══════════ 图标 ═══════════ */

/** `action.search` —— 桌面 `NodesToolbar.tsx:74-77`。 */
function SearchIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-3-3" />
    </svg>
  );
}

/** `action.close` —— 桌面 `ConnectionsScreen.tsx:1027-1029`。 */
function CloseIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M5 5l14 14M19 5L5 19" />
    </svg>
  );
}

/** `action.more` —— 桌面 `SubInfoBar.tsx:101-107`。 */
function MoreIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="5" cy="12" r="1.6" />
      <circle cx="12" cy="12" r="1.6" />
      <circle cx="19" cy="12" r="1.6" />
    </svg>
  );
}

/** `state.warning` —— 桌面 `RulesScreen.tsx:575-578`。 */
function WarningIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 8v5M12 16h.01" />
    </svg>
  );
}

/** 「回到底部」箭头 —— 桌面 `LogsScreen.tsx` 底栏那颗（同 follow 按钮的路径数据）。 */
function DownIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M12 5v14M6 13l6 6 6-6" />
    </svg>
  );
}

/* ═══════════ 通用原语 ═══════════ */

/**
 * 写操作失败的**行内**回显 —— 本屏唯一一条对用户可见的失败通道（IA 裁定 #14）。
 *
 * 立项时 `lib/error-handler.ts` 的 `toast` 在移动端落 `console.*`（真实实现由 `Toaster.tsx` 注入，
 * 而它只挂在桌面 `AppShell.tsx` 上）⇒ 用户看到的只有「行自己跳回来了」，一句话都没有。
 * 那正是 W10/W14 那批真机缺陷的形态。
 *
 * ⚠️ **2026-09-06 起移动端有自己的宿主了**（`mobile/MobileToaster.tsx`），本组件照旧，理由换成
 * 产品口径：连接列表一屏几十行，失败必须落在**那一行**上；全局 toast 说得清「发生了什么」，
 * 说不清「是哪一条」。两者不互斥，本屏要的是前者。
 *
 * `role="alert"` 让读屏当场播报；`data-write-error` 是门的锚。
 */
function InlineError({ text }: { text?: string }): ReactElement | null {
  if (text === undefined || text === '') return null;
  return (
    <div className="mc-error" role="alert" data-write-error="1">
      <WarningIcon />
      <span>{text}</span>
    </div>
  );
}

/** 常驻说明行。桌面挂在 `data-tip` / hover 卡上的解释在这里一律是可见的第二行（§4.12）。 */
function Note({ children }: { children: ReactNode }): ReactElement {
  return <div className="mc-note">{children}</div>;
}

function EmptyState({ text, mark }: { text: string; mark?: string }): ReactElement {
  return (
    <div className="mc-empty" {...(mark === undefined ? {} : { [mark]: '1' })}>
      {text}
    </div>
  );
}

/* ═══════════ 动作面板 ═══════════ */

export type SheetItem = {
  id: string;
  label: string;
  /** 禁用理由 / 补充说明 —— 桌面的 `data-tip` 在这里是可见的第二行（§4.12）。 */
  description?: string;
  danger?: boolean;
  disabled?: boolean;
  /** 选中标记（排序 / 筛选面板用）。 */
  selected?: boolean;
  submenu?: boolean;
  busy?: boolean;
  /**
   * 「再点一次即执行」的武装态。**必须落成 `.confirming` 类** —— 这不是样式，是与
   * `lib/confirm-twice.ts` 的跨文件契约：武装期间它在 document 上挂 capture 阶段的
   * `pointerdown`，凡是没落在 `.confirming` 里的按下一律 `reset()`。不带这个类，
   * 第二次点击的 `pointerdown` 先把 armed 清掉、随后的 `click` 又重新武装 ⇒ 文案在
   * 「全部关闭 ↔ 确认」之间无限翻转，永远关不掉。
   */
  confirming?: boolean;
  onSelect?: () => void;
};

/**
 * 面板里的**可折叠分组** —— 桌面 `Csel` 那个组头（`Csel.tsx` 的 `row.kind === 'header'` 那一支）
 * 在触屏上的对应物。
 *
 * 语义逐字照 `CselGroup`，包括「有没有 `id`」这条判别：
 *  · **带 `id` ⇒ 可折叠**（组头变成可点的展开钮，展开态按这个 id 记）；
 *  · **省略 ⇒ 恒展开**（主路径不许被折进去 —— 同 `RuleRouteEffect` 里那三个快速策略的理由）。
 * 用「有没有 id」而不是另加一个 `collapsible` 布尔，是因为折叠态必须有键才记得住，
 * 两个字段永远要一起给／一起不给，合成一个就没有「可折叠但没键」这种非法组合可表达。
 *
 * 折叠判据本体**不在这里**：`buildCselRows`（`components/dialogs/csel-logic.ts`，纯函数）是
 * 全仓唯一一份，桌面下拉与本面板共用它。各写一份的下场是「桌面折起来了、手机没折」这类
 * 两端分叉，而分叉了没有任何东西会红。
 */
export interface SheetGroup {
  readonly id?: string;
  readonly label: string;
  readonly items: readonly SheetItem[];
}

export interface SheetVM {
  title: string;
  /** 面板顶部的只读说明块（桌面把它放在悬停卡里，触屏够不到）。 */
  body?: ReactNode;
  items: ReadonlyArray<SheetItem | 'separator'>;
  /** 分组区，渲染在 `items` 之后（两者可以同时有：平铺的主路径 + 折起来的长尾）。 */
  groups?: readonly SheetGroup[];
  /** 初始展开的组 id。**省略 = 带 id 的组全部折叠**（同 `Csel` 的 `openGroupIds` 缺省语义）。 */
  openGroups?: ReadonlySet<string>;
  /**
   * 换一张面板时用来重置「本次打开」的折叠态。
   *
   * 折叠是**本次打开**的临时视图态，不跨次残留（`Csel.openGroupIds` 的头注写的就是这条）：
   * 本组件在整个面板层生命周期里只挂载一次（`props.sheet !== null` 即渲染），面板之间是换 props
   * 而不是重挂，故由调用方给一个稳定键，`ConnectionsView` 拿它当 React `key` 强制重挂。
   * 省略 ⇒ 退回用标题当键。
   */
  resetKey?: string;
  /** 面板内的失败回显 —— 破坏性动作就在这张面板上按下的，回显必须留在这里。 */
  error?: string;
  status?: string;
}

/** 未传 `openGroups` 时的稳定空集（每次新建会让 `useState` 初值无谓抖动，同 `Csel.NO_OPEN_GROUPS`）。 */
const NO_OPEN_GROUPS: ReadonlySet<string> = new Set();

/** `Csel` 组头那个折叠雪佛龙 —— 逐字取自 `Csel.tsx` 的 `.csel-grp-chev`。 */
function ChevronIcon(): ReactElement {
  return (
    <svg
      className="mc-sheet-grp-chev"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      aria-hidden
    >
      <path d="M9 6l6 6-6 6" />
    </svg>
  );
}

/** 面板里的一行（平铺区与分组区共用，两处各画一遍必然分叉）。 */
function SheetRow({ item }: { item: SheetItem }): ReactElement {
  return (
    <button
      type="button"
      className={[
        'mc-sheet-item',
        item.danger === true ? 'danger' : '',
        item.confirming === true ? 'confirming' : '',
        item.selected === true ? 'on' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      disabled={item.disabled}
      aria-pressed={item.selected}
      aria-haspopup={item.submenu ? "dialog" : undefined}
      aria-busy={item.busy}
      onClick={() => item.onSelect?.()}
    >
      <span className="mc-sheet-item-tx">
        <span>{item.label}</span>
        {item.description !== undefined && (
          <span className="mc-sheet-item-sub">{item.description}</span>
        )}
      </span>
      {item.submenu && <span aria-hidden="true">›</span>}
    </button>
  );
}

/**
 * `action-sheet`（IA §3.1 #19）：桌面把浮动菜单锚在触发器上，触屏没有 hover 去开、
 * 没有光标去锚、拇指也够不到屏幕上半部 ⇒ 它变成底部面板。条目行 / 分隔线 / 危险变体
 * 全是桌面那套，只有容器与位置变了。
 */
/* **导出**同 `home/HomeScreenView.tsx` 的 `Scrim`：返回键那道门要能单独把这一层渲染开。 */
export function ActionSheet({
  sheet,
  onClose,
  closeLabel,
  onBack,
  backLabel,
}: {
  sheet: SheetVM;
  onBack?: () => void;
  backLabel?: string;
  onClose: () => void;
  closeLabel: string;
}): ReactElement {
  /* 系统返回键关掉的就是这一层。容器只在 `props.sheet !== null` 时渲染本组件 ⇒ 恒 `true`。 */
  useDismissableLayer(true, onBack ?? onClose);
  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(
    () => sheet.openGroups ?? NO_OPEN_GROUPS,
  );
  /* 折叠行序**全部**由 `buildCselRows` 算（组头行 + 未折叠组的选项行 + 组内条目数）。
     这里只做一次「`SheetItem` → `CselOptionLike`」的形状搬运：那张表要的是 `value`/`label`，
     本面板的条目键叫 `id`，字段名不同而已；回填时按 `value` 取回原条目，拿它的 `onSelect`。 */
  const groupRows = buildCselRows(
    (sheet.groups ?? []).map((g) => ({
      ...(g.id === undefined ? {} : { id: g.id }),
      label: g.label,
      options: g.items.map((it) => ({ value: it.id, label: it.label })),
    })),
    openGroups,
  );
  const byId = new Map((sheet.groups ?? []).flatMap((g) => g.items.map((it) => [it.id, it] as const)));
  /* `.mc-sheet` 是 `max-height: 82%` + `overflow-y: auto` 的滚动容器 ⇒ 组头在底部时展开，
     新露出的那一段整个落在视区之外，用户读作「点了没反应」。露出走全仓唯一那条腿
     （`components/reveal.ts`，与 `Csel` / `.ns-grp` / `.tray-group-h` 同一个函数、同一份判据）。 */
  const scheduleReveal = useRevealAfterCommit();
  const toggleGroup = (gid: string, header: HTMLElement, collapsed: boolean): void => {
    setOpenGroups((prev) => {
      const next = new Set(prev);
      if (next.has(gid)) next.delete(gid);
      else next.add(gid);
      return next;
    });
    scheduleReveal(collapsed ? () => revealSiblingGroup(header) : null);
  };
  return (
    <div
      className="mc-scrim"
      role="dialog"
      aria-modal="true"
      aria-label={sheet.title}
      /* 桌面那条「两次指针事件都落在背景上才关闭」的规矩在这里的对应物是
         「手势不能起自内容」：点在面板本体上不冒泡到这里。 */
      onClick={onClose}
    >
      <div className="mc-sheet" onClick={(e) => e.stopPropagation()}>
        <div className="mc-sheet-grip" />
        <SheetHeading title={sheet.title} onClose={onClose} closeLabel={closeLabel} titleClassName="mc-sheet-h"
          leading={onBack && <button type="button" className="m-menu-back" onClick={onBack}>{backLabel}</button>} />
        {sheet.body !== undefined && <Note>{sheet.body}</Note>}
        <InlineError text={sheet.error} />
        {sheet.status && <p className="m-action-status" role="status">{sheet.status}</p>}
        {sheet.items.map((item, i) =>
          item === 'separator' ? (
            <div key={`sep-${i}`} className="mc-sheet-sep" />
          ) : (
            <SheetRow key={item.id} item={item} />
          ),
        )}
        {groupRows.map((row) => {
          if (row.kind === 'header') {
            const gid = row.groupId;
            /* 不可折叠的组：组头是纯视觉分隔，不可聚焦（同 `Csel` 的 `.csel-grp` 那一支）。 */
            if (gid === undefined) {
              return (
                <div key={row.key} className="mc-sheet-grp" role="presentation">
                  {row.label}
                </div>
              );
            }
            /* 可折叠组：组头是按钮。**不给 role** —— 同 `Csel` 那条理由，原生 button 的隐含角色
               与 `aria-expanded` 已经把「可展开的组头」说清楚，硬套别的角色都是撒谎。
               `row.count` 是折叠态下唯一能看出「这组有多少条」的信号，故恒显示。 */
            return (
              <button
                key={row.key}
                type="button"
                className={`mc-sheet-grp mc-sheet-grp-t${row.collapsed ? '' : ' open'}`}
                aria-expanded={!row.collapsed}
                data-sheet-group={gid}
                onClick={(e) => toggleGroup(gid, e.currentTarget, row.collapsed)}
              >
                <ChevronIcon />
                <span>{row.label}</span>
                <span className="mc-sheet-grp-c">{row.count}</span>
              </button>
            );
          }
          const item = byId.get(row.opt.value);
          return item === undefined ? null : <SheetRow key={item.id} item={item} />;
        })}
      </div>
    </div>
  );
}

/* ═══════════ 分页 ═══════════ */

/**
 * `list-pager`（IA §3.1 #12）：两颗按钮到 48 高，页码指示保留 live region。
 * 分页**保留**、不换成无限滚动：桌面那条理由（限住挂载的 DOM）在手机上更强，
 * 而一个行在被实时增删的无限列表会丢掉读者的位置。
 */
function Pager({
  t,
  window: win,
  total,
  onPage,
}: {
  t: Translate;
  window: PageSlice;
  total: number;
  onPage: (page: number) => void;
}): ReactElement | null {
  if (win.pageCount <= 1) return null;
  return (
    <div className="mc-pager" aria-live="polite" data-pager="1">
      <span className="mc-pager-status">
        {t('common.pageStatus', { start: win.start + 1, end: win.end, total })}
      </span>
      <div className="mc-pager-acts">
        <button
          type="button"
          className="mc-btn"
          disabled={win.page === 0}
          onClick={() => onPage(win.page - 1)}
        >
          {t('common.previousPage')}
        </button>
        <button
          type="button"
          className="mc-btn"
          disabled={win.page >= win.pageCount - 1}
          onClick={() => onPage(win.page + 1)}
        >
          {t('common.nextPage')}
        </button>
      </div>
    </div>
  );
}

/* ═══════════ 概览：两张排名卡 ═══════════ */

export interface BarVM {
  /** 已本地化的显示名（`TOPOLOGY_OTHERS_KEY` 的替换在容器里做完，呈现层看不到 sentinel）。 */
  label: string;
  count: number;
  /** 直连出站与具名出站配色不同（桌面 `renderTopN` 同款）。 */
  direct?: boolean;
  /** 域名条恒用 aurora 色（桌面同款）。 */
  aurora?: boolean;
}

function BarList({ rows, empty }: { rows: readonly BarVM[]; empty: string }): ReactElement {
  if (rows.length === 0) return <EmptyState text={empty} />;
  const max = rows.reduce((m, r) => Math.max(m, r.count), 0) || 1;
  return (
    <div className="mc-bars">
      {rows.map((r) => (
        <div className="mc-bar-row" key={r.label}>
          <span className="mc-bar-name">{r.label}</span>
          <span className="mc-bar">
            <i
              className={r.aurora === true ? 'aurora' : r.direct === true ? 'direct' : 'flow'}
              style={{ width: `${(r.count / max) * 100}%` }}
            />
          </span>
          <span className="mc-bar-v">{r.count}</span>
        </div>
      ))}
    </div>
  );
}

/* ═══════════ 连接行 ═══════════ */

function DetailRow({ label, value }: { label: string; value: string }): ReactElement {
  return (
    <div className="mc-det-row">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function ConnRow({
  t,
  row,
  segment,
  clock,
  expanded,
  onToggle,
  onMenu,
  onClose,
  error,
}: {
  t: Translate;
  row: ConnRowVM;
  segment: 'active' | 'closed';
  clock: { at: number; sequence: number };
  expanded: boolean;
  onToggle: (id: string) => void;
  onMenu: (id: string) => void;
  onClose: (id: string) => void;
  error?: string;
  status?: string;
}): ReactElement {
  const rates = connRowRates(row, clock.sequence);
  const age = connRowAge(row, clock.at);
  const chainText =
    row.chainKind === 'block'
      ? t('home.routingBlock')
      : row.chainKind === 'direct'
        ? t('home.routingDirect')
        : row.chain || DASH;
  /* 计数器缺席时画占位符而不是 `0 B` —— `data-contract.json#connections-detail` 的 trap：
     absent 不是 measured zero。桌面走 `?? 0` ⇒ 屏幕上是「量到了 0 字节」。 */
  const totalText = row.totalBytes === null ? DASH : fmtBytes(row.totalBytes);
  const ruleSummary = row.ruleName || t(
    row.ruleGroupKey === 'policy:direct' ? 'home.ruleDirect'
      : row.ruleGroupKey === 'policy:blocked' ? 'home.routingBlock'
        : row.ruleGroupKey === 'policy:proxied' ? 'home.ruleProxy' : 'common.unknown',
  );
  return (
    <div className="mc-row" data-conn-row={row.id}>
      <div className="mc-row-top">
        <button
          type="button"
          className="mc-row-main"
          aria-expanded={expanded}
          onClick={() => onToggle(row.id)}
        >
          <span className="mc-row-l1">
            <span className={`mc-pill ${row.udp ? 'udp' : 'tcp'}`}>{row.l4 || DASH}</span>
            <span className="mc-row-host">{row.host || DASH}</span>
          </span>
          <span className="mc-row-l2">
            <span className="mc-row-rule">{ruleSummary}</span>
            <span className="mc-row-sep">›</span>
            <span className={`mc-row-chain ${row.chainKind}`}>{chainText}</span>
            {/*
              expanded 断点上行内多出的第三格（IA §2.3「Breakpoints」：expanded 下连接行
              先长出进程列，再谈展开）。**始终渲染、由容器查询决定可见性** —— 按 JS 断点
              条件渲染要在屏里再造一份断点真值，而断点的真值源是容器查询本身。
              隐私态下 `procName` 已在投影层被摘空，这一格于是自然为空。
            */}
            <span className="mc-row-proc">{row.procName}</span>
          </span>
        </button>
        <span className="mc-row-num">
          {segment === 'active' ? (
            <>
              <span className="mc-rate d">{fmtRate(rates.down)}</span>
              <span className="mc-rate u">{fmtRate(rates.up)}</span>
            </>
          ) : (
            <span className="mc-rate">{totalText}</span>
          )}
        </span>
        {segment === 'active' && (
          <button
            type="button"
            className="mc-row-x"
            aria-label={t('connections.close')}
            onClick={() => onClose(row.id)}
          >
            <CloseIcon />
          </button>
        )}
        <button
          type="button"
          className="mc-row-more"
          aria-label={t('mobileConnections.rowMore')}
          onClick={() => onMenu(row.id)}
        >
          <MoreIcon />
        </button>
      </div>
      {/*
        展开区 = 桌面那张九列表里塞不进两行摘要的其余列，**外加**桌面只靠 `data-tip` 承载的
        那几处全文（域名 / 目标 / 规则 / 链路 / 进程 / L4 / 结束时间）。§4.12 的落点就是这里：
        触屏没有 hover，截断的文字必须有一个够得到的地方能读全。
      */}
      {expanded && (
        <dl className="mc-det" data-conn-detail={row.id}>
          <DetailRow label={t('connections.colHost')} value={row.host || DASH} />
          <DetailRow label={t('connections.colDest')} value={row.dest || DASH} />
          <DetailRow label={t('connections.colRule')} value={row.rule || DASH} />
          <DetailRow label={t('connections.colChain')} value={row.chain || DASH} />
          <DetailRow label={t('connections.colType')} value={row.l4Full || DASH} />
          <DetailRow label={t('connections.colTraffic')} value={totalText} />
          <DetailRow label={t('connections.colTime')} value={fmtDuration(age)} />
          {segment === 'closed' && (
            <DetailRow
              label={t('connections.colEnded')}
              value={row.endedAt ? new Date(row.endedAt).toLocaleString() : DASH}
            />
          )}
          <DetailRow label={t('connections.colProcess')} value={row.procFull || DASH} />
          {/* 隐私模式下容器已把 sourceIP 投影成 null ⇒ 这一行整个不存在，而不是画一个空值。 */}
          {row.sourceIP !== null && (
            <DetailRow label={t('mobileConnections.sourceIP')} value={row.sourceIP} />
          )}
        </dl>
      )}
      <InlineError text={error} />
    </div>
  );
}

/* ═══════════ 屏 ═══════════ */

export interface ConnectionsViewProps {
  t: Translate;

  segment: ConnSegment;
  onSegment: (segment: ConnSegment) => void;

  /** 搜索词（活动 / 已结束 / 日志三段共用一个输入框的位置，各段各自的值由容器持有）。 */
  search: string;
  onSearch: (value: string) => void;
  /** Exact upstream rule filter supplied by a Home hit, independent of text search. */
  exactRule?: { key: string; label: string } | null;
  onClearExactRule?: () => void;

  /** 屏头溢出入口。为空数组时按钮不渲染（一颗点开什么都没有的 `⋯` 比没有更坏）。 */
  overflow: ReadonlyArray<SheetItem | 'separator'>;
  onOverflow: () => void;

  /** 概览段。 */
  privacy: boolean;
  aggregateLoaded: boolean;
  topN: number;
  onTopN: (n: number) => void;
  hosts: readonly BarVM[];
  outbounds: readonly BarVM[];

  /** 列表段。 */
  rows: readonly ConnRowVM[];
  listTotal: number;
  listLoaded: boolean;
  listWindow: PageSlice;
  onListPage: (page: number) => void;
  clock: { at: number; sequence: number };
  expandedId: string | null;
  onToggleRow: (id: string) => void;
  onRowMenu: (id: string) => void;
  onCloseRow: (id: string) => void;
  sort: SortState<ConnSortKey> | null;
  paused: boolean;

  /** 日志段。 */
  logRows: readonly LogRowVM[];
  logTotal: number;
  logWindow: PageSlice;
  onLogPage: (page: number) => void;
  logPrivacyNote: boolean;
  /**
   * W26 前遗留的无界 `singbox.log` 那条提示（桌面 `LogsScreen.tsx` 的 `#log-legacy-note`）。
   *
   * 桌面把「归档 / 删除」两颗按钮直接摆在提示块里；这里**只放话、不放按钮** —— IA §2.3 要求
   * 破坏性动作进 header overflow，摆一行工具栏在 compact 上放不下，且与本屏其余四颗破坏性
   * 动作分处两地。两颗动作在「更多」面板里，随这条提示一起在场／一起消失。
   * `tone: 'ok'` 是动作做完之后的回执（提示块原地换成「已归档 / 已删除」，而不是凭空消失）。
   */
  legacyNotice: { tone: 'warn' | 'ok'; text: string } | null;
  coreLevelNotice: string | null;
  coreLevelHint: string;
  follow: boolean;
  pendingCount: number;
  onResumeFollow: () => void;
  /** 日志滚动容器的 ref 挂点由容器提供（follow / 离底检测都要真实 DOM）。 */
  logViewRef?: React.Ref<HTMLDivElement>;

  /** 屏级失败回显（屏头溢出里按下的那些动作）。行级失败走 `rowErrors`。 */
  writeError?: string;
  rowErrors: Readonly<Record<string, string>>;

  /** 当前打开的面板；`null` = 没有面板。 */
  sheet: SheetVM | null;
  onCloseSheet: () => void;
  onBackSheet?: () => void;
}

export function ConnectionsView(props: ConnectionsViewProps): ReactElement {
  const { t, segment } = props;
  const isList = segment === 'active' || segment === 'closed';
  const showSearch = segment !== 'overview';

  const segments: ReadonlyArray<{ id: ConnSegment; label: string }> = [
    { id: 'overview', label: t('mobileConnections.seg.overview') },
    { id: 'active', label: t('connections.activeTab') },
    { id: 'closed', label: t('connections.closedTab') },
    { id: 'logs', label: t('mobileConnections.seg.logs') },
  ];

  const emptyText = props.privacy
    ? t('connections.privacyHidden')
    : !props.listLoaded
      ? t('connections.loading')
      : props.listTotal === 0 && (props.search.trim() !== '' || props.exactRule != null)
        ? t('connections.noMatch')
        : segment === 'active'
          ? t('connections.noActive')
          : t('connections.noClosed');

  return (
    <section className="mc" data-conn-scope="mobile-connections" aria-labelledby="mc-title">
      {/*
        ── 吸顶块（IA §2.3「Fixed and scrolling」）──────────────────────────────
        分段条**和**搜索框都在这一块里，而这一块是唯一 `position: sticky` 的元素。

        搜索框为什么必须留在吸顶块里：活动/已结束/日志三段读的都是**实时列表**，边读边改
        检索条件是常态。把它随内容滚走，等于每改一次条件都要先滚回顶部 —— 那条路上列表还在
        自己动，滚回去的时候已经不是刚才那一屏了。判据在门 ②：搜索框的祖先链里必须有本块。
      */}
      <div className="mc-top" data-sticky="header">
        <div className="mc-head">
          <h1 className="mc-title" id="mc-title">
            {t('connections.pageTitle')}
          </h1>
          {props.overflow.length > 0 && (
            <button
              type="button"
              className="mc-btn icon"
              aria-label={t('mobileConnections.more')}
              onClick={props.onOverflow}
            >
              <MoreIcon />
            </button>
          )}
        </div>

        <div className="mc-seg" role="tablist" aria-label={t('connections.pageTitle')}>
          {segments.map((s) => (
            <button
              key={s.id}
              type="button"
              role="tab"
              className="mc-seg-item"
              aria-selected={s.id === segment}
              onClick={() => props.onSegment(s.id)}
            >
              <span>{s.label}</span>
            </button>
          ))}
        </div>

        {isList && props.exactRule != null && (
          <div className="mc-exact-rule" role="status">
            <span>{t('mobileConnections.exactRuleFilter')}: <strong>{props.exactRule.label}</strong></span>
            <button type="button" onClick={props.onClearExactRule} aria-label={t('mobileConnections.clearExactRule')}>
              {t('mobileConnections.clearExactRule')}
            </button>
          </div>
        )}

        {showSearch && (
          <label className="mc-search" data-conn-search="1">
            <SearchIcon />
            <input
              type="search"
              value={props.search}
              placeholder={
                segment === 'logs' ? t('logs.searchPlaceholder') : t('connections.search')
              }
              onChange={(e) => props.onSearch(e.target.value)}
            />
            {props.search !== '' && (
              <button
                type="button"
                className="mc-btn icon"
                aria-label={t('mobileConnections.clearSearch')}
                onClick={() => props.onSearch('')}
              >
                <CloseIcon />
              </button>
            )}
          </label>
        )}

        {/* 屏头溢出里按下的破坏性动作，失败就回显在按下它的那颗按钮下面。 */}
        <InlineError text={props.writeError} />
      </div>

      {/* ══════════ 概览 ══════════ */}
      {segment === 'overview' && (
        <div className="mc-body" data-segment="overview">
          <section className="mc-card">
            <div className="mc-card-h">
              <span>{t('mobileConnections.overview.hostsTitle')}</span>
              <div className="mc-choice" role="group" aria-label={t('connections.topCount')}>
                {TOP_N_OPTIONS.map((n) => (
                  <button
                    key={n}
                    type="button"
                    className="mc-btn"
                    aria-pressed={props.topN === n}
                    onClick={() => props.onTopN(n)}
                  >
                    {n}
                  </button>
                ))}
              </div>
            </div>
            {/*
              这条说明不是装饰：`data-contract.json#connections-aggregate` 的 trap 写明
              两张卡的 `count` 是**连接数**、不是字节数，而首页那两张按字节排名的卡长得几乎一样。
              不写出来，用户会拿两个不同的测量互相对账，然后认定其中一个坏了。
            */}
            <Note>{t('mobileConnections.overview.unitNote')}</Note>
            {props.privacy ? (
              <EmptyState text={t('connections.privacyHidden')} />
            ) : (
              <BarList
                rows={props.hosts}
                empty={
                  props.aggregateLoaded ? t('connections.noActive') : t('connections.loading')
                }
              />
            )}
          </section>

          <section className="mc-card">
            <div className="mc-card-h">
              <span>{t('mobileConnections.overview.outboundsTitle')}</span>
              {/* 只读回显：与上面那张卡共用一个 N。不做成第二个控件 —— 两份控件绑同一状态，
                  用户会问「这两个有什么区别」，而答案是「没有」。 */}
              <span className="mc-pill region">{t('connections.topBadge', { n: props.topN })}</span>
            </div>
            <BarList
              rows={props.outbounds}
              empty={props.aggregateLoaded ? t('connections.noActive') : t('connections.loading')}
            />
          </section>
        </div>
      )}

      {/* ══════════ 活动 / 已结束 ══════════ */}
      {isList && (
        <div className="mc-body" data-segment={segment}>
          {segment === 'active' && props.paused && (
            <div className="mc-notice warn" role="status">
              <WarningIcon />
              <span>{t('mobileConnections.pausedNote')}</span>
            </div>
          )}
          {props.sort !== null && (
            <div className="mc-sortbar" data-sort-active="1">
              {t('mobileConnections.sortedBy', {
                key: t(sortLabelKey(props.sort.key)),
                dir: t(
                  props.sort.dir > 0 ? 'mobileConnections.sortAsc' : 'mobileConnections.sortDesc',
                ),
              })}
            </div>
          )}
          {props.rows.length === 0 ? (
            <EmptyState text={emptyText} mark="data-list-empty" />
          ) : (
            <div className="mc-list" role="list">
              {props.rows.map((row) => (
                <ConnRow
                  key={row.id}
                  t={t}
                  row={row}
                  segment={segment}
                  clock={props.clock}
                  expanded={props.expandedId === row.id}
                  onToggle={props.onToggleRow}
                  onMenu={props.onRowMenu}
                  onClose={props.onCloseRow}
                  error={props.rowErrors[row.id]}
                />
              ))}
            </div>
          )}
          <Pager t={t} window={props.listWindow} total={props.listTotal} onPage={props.onListPage} />
        </div>
      )}

      {/* ══════════ 日志 ══════════ */}
      {segment === 'logs' && (
        <div className="mc-body" data-segment="logs">
          {props.logPrivacyNote && (
            <div className="mc-notice info" role="status">
              <WarningIcon />
              <span>{t('logs.privacyNote')}</span>
            </div>
          )}
          {props.legacyNotice !== null && (
            <div
              className={`mc-notice ${props.legacyNotice.tone}`}
              role="status"
              data-log-legacy="1"
            >
              <WarningIcon />
              <span>{props.legacyNotice.text}</span>
            </div>
          )}
          {props.coreLevelNotice !== null && (
            <div className="mc-notice warn" role="status">
              <WarningIcon />
              {/* 用 div 而不是 span 包这两行：`Note` 渲染的是块级元素，塞进 span 里是非法嵌套。 */}
              <div className="mc-notice-tx">
                <div>{props.coreLevelNotice}</div>
                {/* 桌面把成因挂在徽标的 `data-tip` 上；触屏够不到 ⇒ 成因是常驻的第二行（§4.12）。 */}
                <Note>{props.coreLevelHint}</Note>
              </div>
            </div>
          )}

          <div className="mc-logwrap">
            <div
              className="mc-logview"
              ref={props.logViewRef}
              tabIndex={0}
              aria-label={t('logs.liveStream')}
            >
              {/*
                ── IA 裁定 #5：空态**给文案** ──────────────────────────────────
                桌面刻意不渲染任何占位，理由写在 `LogsScreen.tsx` 头注：逐字复现原型。
                那是一条**保真约束**，不是产品原则 —— 它管的是「桌面要和原型像素一致」，
                移动端没有那份原型，这条约束在这里不存在。而移动端有一个桌面没有的失败形态：
                固定头与固定底导航之间一片空白，读作「屏幕坏了」，不读作「暂时没有日志」。
              */}
              {props.logRows.length === 0 ? (
                <div className="mc-log-empty" data-log-empty="1">
                  <div className="mc-log-empty-t">{t('mobileConnections.logs.emptyTitle')}</div>
                  <Note>{t('mobileConnections.logs.emptyBody')}</Note>
                </div>
              ) : (
                props.logRows.map((l) => (
                  <div className="mc-log-line" key={l.key}>
                    <span className="mc-log-ts">{l.ts}</span>{' '}
                    <span className={`mc-log-lv ${l.level}`}>{l.level.toUpperCase()}</span>{' '}
                    <span className="mc-log-msg">{l.message}</span>
                  </div>
                ))
              )}
            </div>
            {/* 浮动「回到底部」：向上滚打断跟随后才出现，点它 = 回填缓冲 + 吸回底部 + 恢复跟随。 */}
            {!props.follow && (
              <button
                type="button"
                className="mc-jump"
                data-log-jump="1"
                onClick={props.onResumeFollow}
              >
                <DownIcon />
                <span>{t('logs.scrollToBottom')}</span>
                {props.pendingCount > 0 && <span className="mc-cnt">{props.pendingCount}</span>}
              </button>
            )}
          </div>

          <div className="mc-logfoot">
            <span className="mc-live">
              <span className={`mc-live-dot ${props.follow ? 'on' : 'off'}`} />
              <span>{t('logs.liveStream')}</span>
            </span>
            <span className="mc-log-count">
              {t('mobileConnections.logs.lines', { n: props.logTotal })}
            </span>
          </div>
          <Pager t={t} window={props.logWindow} total={props.logTotal} onPage={props.onLogPage} />
        </div>
      )}

      {props.sheet !== null && (
        <ActionSheet
          /* 折叠态按「本次打开」重置（见 `SheetVM.resetKey`）：换面板 = 换 key = 重挂。 */
          key={props.sheet.resetKey ?? props.sheet.title}
          sheet={props.sheet}
          onClose={props.onCloseSheet}
          onBack={props.onBackSheet}
          backLabel={t("mobileActions.back")}
          closeLabel={t('common.cancel')}
        />
      )}
    </section>
  );
}

/**
 * 排序键 → 桌面列名的 i18n key。**返回完整字面量键**，不返回后缀去拼。
 *
 * 拼出来的键（`` t(`connections.col${suffix}`) ``）会让 i18n 的死键判据看不见这九条列名的
 * 消费点，于是把它们全判成「声明了但没人用」—— 桌面 `LogsScreen` 里那处刻意写成三元
 * 而不是模板串的注释，记的是同一条坑。
 */
export function sortLabelKey(key: ConnSortKey): string {
  switch (key) {
    case 'type':
      return 'connections.colType';
    case 'host':
      return 'connections.colHost';
    case 'dest':
      return 'connections.colDest';
    case 'rule':
      return 'connections.colRule';
    case 'chain':
      return 'connections.colChain';
    case 'rate':
      return 'connections.colSpeed';
    case 'total':
      return 'connections.colTraffic';
    case 'time':
      return 'connections.colTime';
    case 'ended':
      return 'connections.colEnded';
    case 'proc':
      return 'connections.colProcess';
  }
}

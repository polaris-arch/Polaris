/**
 * 单条规则的移动端行（`entity-row`，IA §3.1 #18 的 `RuleItem.tsx` 那一格）。
 *
 * # 与桌面 `RuleItem.tsx` 的差与不差
 *
 * **不变**：优先级序号、标题、条件摘要「类型 ×n」、动作 pill、DNS pill、四个角标
 * （暂存 / 覆盖组网 / 资源缺失 / 节点已失效）、`→ 目标节点`、停用后整行降透明。
 * 颜色角色、severity 分级（①③ warn、② 中性）全部照抄，一处不改。
 *
 * **变**：桌面沿行尾排七颗纯图标按钮（`RuleItem.tsx:372-486`：置顶/上移/下移/置底/开关/复制/编辑/删除），
 * 每颗都远小于 48 且彼此相邻。移动端**留一颗主动作（启停开关）+ 一颗溢出**，其余进
 * `action-sheet`。变的是「动作怎么触达」，不是视觉层。
 *
 * **拖拽重排换手势，不砍功能**（IA §4.13）：桌面用的是 HTML5 `draggable`，那套事件在触屏上
 * 根本不触发 ⇒ 处置是「在原有 grip 上做长按拖拽，**并且**保留四颗移动按钮」两件都做。
 * 四颗移动按钮全部保留 —— 桌面当初加它们的理由（「原生拖拽在长列表 + 触控板上极难命中」，
 * `RuleItem.tsx:373-374`）在手机上只会更强，而且它们是这条重排的**可访问通道**：
 * 抓握条同桌面一样 `aria-hidden`，读屏用户走面板里那四条可见文字。
 *
 * 另有一条不靠规范也成立的理由：`rules.priorityTip` 这句用户可见文案本来就写着
 * 「**拖拽**或用行内按钮排序」。没有拖拽 = 文案在撒谎。
 *
 * # §4.12：桌面这一行有 11 处 `data-tip`，这里一处都没有
 *
 * | 桌面 | 落点 |
 * |---|---|
 * | `:307` 路由效果未生效 | 角标下的常驻 `.mr-note` |
 * | `:340` 待保存 | 同上 |
 * | `:346` 覆盖组网 | 同上 |
 * | `:352` 资源缺失 | 同上 |
 * | `:361` 节点已失效 | 同上 |
 * | `:389/:401/:413/:425` 四颗移动 | 动作面板条目的**可见文字** |
 * | `:437` 启停开关 | 开关的 `aria-label` + 面板里的可见条目 |
 * | `:449` 复制 / `:462` 编辑 / `:476` 删除 | 动作面板条目的可见文字（删除的「再点一次」同理） |
 *
 * 条件的**逐值明细**在桌面是 hover 详情卡（`RuleHoverCard`）。触屏没有 hover ⇒ 它跟着溢出入口
 * 一起被点开，落在面板顶部的只读说明块里。
 */

import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
} from 'react';
import type { Rule } from '@/contracts/types';
import { ruleConditions, ruleDnsEffect, ruleRouteEffect } from '@/domain/rules';
import { PROFILE_MATCH_KEYS, type RuleProfileBadge } from '@/domain/network-profile';
import { MatchDot } from '@/components/screens/rules/MatchDot';
import { mobileReasonKey } from './network-profile-copy';
import {
  ActionSheet,
  DeleteIcon,
  InlineError,
  DuplicateIcon,
  EditIcon,
  GripIcon,
  MoreIcon,
  MoveIcon,
  Switch,
  TypeIcon,
  type SheetAction,
} from './Primitives';

export type RuleRowStrings = {
  /** `t` 的薄封装，容器注入 —— 行本身是纯呈现，不自己碰 i18n 实例。 */
  t: (key: string, vars?: Record<string, unknown>) => string;
};

export interface RuleRowProps extends RuleRowStrings {
  rule: Rule;
  /** 1-based 优先级序号（= `index + 1`），**就是求值次序**。 */
  index: number;
  enabled: boolean;
  plane: 'route' | 'dns';
  targetNodeName?: string;
  dnsActionName?: string;
  /** 角标①：引用的规则资源本地缺失 ⇒ 生成配置时该条件被整条跳过（fail-closed）。 */
  hasMissingResource?: boolean;
  /** 角标②：ipCidr 与组网 force-route 段重叠（中性，不是错误）。 */
  hasMeshOverlap?: boolean;
  /** 角标③：指定出口节点已删除 ⇒ 运行时回退跟随全局。 */
  targetMissing?: boolean;
  /** 角标④：只在暂存里、磁盘上还没有。 */
  stagedOnly?: boolean;
  /** 非 smart 模式只忽略流量效果；DNS 效果仍生效。 */
  routeInactive?: boolean;
  /**
   * 角标⑤「生效网络」（判据 `domain/network-profile#ruleProfileBadge`，与桌面 `RuleItem` 同一个函数）。
   * 不挂场景 ⇒ `null` / 缺省 ⇒ 不渲染。
   */
  networkProfileBadge?: RuleProfileBadge | null;
  isFirst: boolean;
  isLast: boolean;
  /** 本行的删除按钮处于「再点一次即删」待定态（状态由屏单点持有，同桌面）。 */
  deleteConfirming: boolean;
  /** 溢出面板是否为本行打开（同样单点持有：同屏两个面板同时开是误触面）。 */
  sheetOpen: boolean;
  onOpenSheet: (rule: Rule | null) => void;
  onToggle: (rule: Rule) => void;
  onMove: (rule: Rule, to: 'up' | 'down' | 'top' | 'bottom') => void;
  /**
   * 长按拖拽落点的提交路径（IA §4.13）。与 `onMove` 是**同一条写路径的两个入口**：
   * 容器两边都落到 `commitOrder`，所以拖拽与四颗按钮不会各写各的顺序。
   */
  onReorder: (rule: Rule, toIndex: number) => void;
  onDuplicate: (rule: Rule) => void;
  onDelete: (rule: Rule) => void;
  /**
   * 编辑入口。`undefined` = 本批不可用；此时按钮**在场且禁用并带可见理由**，不隐藏
   * （IA §3.3 第 2 条：桌面已经在守这条线，理由是「按钮凭空少一个，用户只会以为这张卡坏了」）。
   */
  onEdit?: (rule: Rule) => void;
  /** 编辑不可用时展示的理由（可见文字，不是 tip）。 */
  editUnavailableReason?: string;
  /** 本行上一次写操作的失败文案（失败要落在**这一行**上，理由见 `Primitives.InlineError` 头注）。 */
  error?: string;
}

/** 规则标题：remarks 优先，否则退到类型名（与桌面 `ruleTitle` 同口径）。 */
function titleOf(rule: Rule): string {
  return rule.remarks && rule.remarks.trim() ? rule.remarks : rule.type;
}

/** 动作 → pill 类 + 文案。key 与桌面 `actionPill` 共用，避免两端文案漂移。 */
function actionPill(
  action: Rule['action'],
  t: RuleRowStrings['t'],
): { cls: string; text: string } {
  switch (action) {
    case 'block':
      return { cls: 'act-block', text: t('rules.targetBlock') };
    case 'direct':
      return { cls: 'act-direct', text: t('rules.targetDirect') };
    default:
      return { cls: 'act-proxy', text: t('rules.policyProxy') };
  }
}

/** 长按多久算「拿起来了」。比 Android 的 `ViewConfiguration` 长按阈值（500ms）短一档，
 *  但足够长到不会被「按住开始滚动」误触发；滑动超过 `DRAG_SLOP` 会取消这次长按。 */
const LONG_PRESS_MS = 320;
/** 长按未成立前的滑动容差（px）。超过它 ⇒ 用户的意图是滚列表，不是拿起这一行。 */
const DRAG_SLOP = 8;

/**
 * 松手时落到哪一格。
 *
 * 用兄弟行的**实际矩形**算，不用「行高 × 位移」换算：规则行的高度随角标数量与说明行数变化，
 * 按固定行高换算在长列表里会越算越偏，而排序错一格就是求值优先级错一格。
 *
 * `from > 落点` 时不调整、`from < 落点` 时减一 —— 因为落点是在**含被拖行**的当前 DOM 上量的，
 * 而提交时被拖行会先从数组里摘出去。少这一步，往下拖永远多走一格。
 *
 * 导出是为了让门能直接喂矩形断言落点（拖拽手势本身在 jsdom 里量不出真实 rect）。
 */
export function dropIndexAt(
  rects: ReadonlyArray<{ top: number; bottom: number }>,
  from: number,
  y: number,
): number {
  let raw = rects.length;
  for (let i = 0; i < rects.length; i += 1) {
    if (y < (rects[i].top + rects[i].bottom) / 2) {
      raw = i;
      break;
    }
  }
  return raw > from ? raw - 1 : raw;
}

export function RuleRow(props: RuleRowProps): ReactElement {
  const {
    rule,
    index,
    enabled,
    plane,
    targetNodeName,
    dnsActionName,
    hasMissingResource,
    hasMeshOverlap,
    targetMissing,
    stagedOnly,
    routeInactive,
    networkProfileBadge,
    isFirst,
    isLast,
    deleteConfirming,
    sheetOpen,
    onOpenSheet,
    onToggle,
    onMove,
    onReorder,
    onDuplicate,
    onDelete,
    onEdit,
    editUnavailableReason,
    error,
    t,
  } = props;

  /* ── 长按拖拽重排（IA §4.13）────────────────────────────────────────────────
   * 手势状态是**本行局部**的：它每帧都变，抬到容器只会让整段规则列表跟着重渲。
   * 落盘的那一下才走容器（`onReorder` → `commitOrder`），与四颗移动按钮同一条写路径。
   */
  const rowRef = useRef<HTMLDivElement | null>(null);
  const armTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startY = useRef(0);
  /** 已挂上的平移接管监听的卸载函数（`null` = 当前没接管）。 */
  const releaseScroll = useRef<(() => void) | null>(null);
  const [dragDy, setDragDy] = useState<number | null>(null);
  const dragging = dragDy !== null;

  /** 松开平移接管。幂等 —— 每条结束路径都会调它，重复调必须无害。 */
  const unseizeScroll = (): void => {
    releaseScroll.current?.();
    releaseScroll.current = null;
  };

  /**
   * 长按成立那一刻接管纵向平移。
   *
   * 抓握条平时是 `touch-action: pan-y`（否则整条行左侧成了滚动死区，见 `rules-screen.css`
   * `.mr-grip` 的头注）。`touch-action` 在 touchstart 时求值、之后不可改 ⇒ 「拖拽时才禁滚动」
   * 只能在**事件层**做：此刻手指尚未移动、浏览器还没开始平移，第一条 `touchmove` 上
   * `preventDefault()` 就能把这次平移拦下来。
   *
   * 必须是原生 `addEventListener` + `{ passive: false }`：React 的 `onTouchMove` 走的是被动注册，
   * 在里面调 `preventDefault()` 不生效（且控制台只给一条警告，真机上表现为「拖拽时列表还在滚」）。
   */
  const seizeScroll = (handle: HTMLElement): void => {
    unseizeScroll();
    const block = (ev: TouchEvent): void => {
      if (ev.cancelable) ev.preventDefault();
    };
    handle.addEventListener('touchmove', block, { passive: false });
    releaseScroll.current = () => handle.removeEventListener('touchmove', block);
  };

  const cancelArm = (): void => {
    if (armTimer.current !== null) {
      clearTimeout(armTimer.current);
      armTimer.current = null;
    }
    unseizeScroll();
  };

  /*
   * 拆卸时把待触发的长按计时器清掉。不清的话，切分段（本行随之卸载）而长按还没到点，
   * 计时器仍会在 320ms 后拿着一个已经脱离文档的元素去 `setPointerCapture` —— 那会抛
   * `NotFoundError`，而它抛在 `setTimeout` 里，真机上既不进任何 catch 也没有日志。
   *
   * 平移接管的监听同理要摘：它挂在抓握条这个 DOM 节点上，不摘就随节点一起泄漏。
   */
  useEffect(
    () => () => {
      if (armTimer.current !== null) clearTimeout(armTimer.current);
      releaseScroll.current?.();
      releaseScroll.current = null;
    },
    [],
  );

  const onGripDown = (e: ReactPointerEvent<HTMLSpanElement>): void => {
    const handle = e.currentTarget;
    const { pointerId } = e;
    startY.current = e.clientY;
    cancelArm();
    armTimer.current = setTimeout(() => {
      armTimer.current = null;
      /* 捕获指针：之后的 move/up 都回到抓握条上，滚动容器抢不走这条手势。 */
      handle.setPointerCapture(pointerId);
      /* 并在事件层接管平移 —— 只靠指针捕获挡不住浏览器自己的滚动（那由 `touch-action` 决定）。 */
      seizeScroll(handle);
      setDragDy(0);
    }, LONG_PRESS_MS);
  };

  const onGripMove = (e: ReactPointerEvent<HTMLSpanElement>): void => {
    const dy = e.clientY - startY.current;
    if (!dragging) {
      // 长按还没成立就滑动 ⇒ 用户要滚列表，不是要拿起这一行。
      if (Math.abs(dy) > DRAG_SLOP) cancelArm();
      return;
    }
    setDragDy(dy);
  };

  const onGripUp = (e: ReactPointerEvent<HTMLSpanElement>): void => {
    cancelArm();
    if (!dragging) return;
    setDragDy(null);
    const row = rowRef.current;
    const list = row?.parentElement;
    if (!row || !list) return;
    const rects = [...list.children].map((el) => {
      const r = el.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom };
    });
    const to = dropIndexAt(rects, index, e.clientY);
    if (to !== index) onReorder(rule, to);
  };

  const onGripCancel = (): void => {
    cancelArm();
    setDragDy(null);
  };

  const route = ruleRouteEffect(rule);
  const dns = ruleDnsEffect(rule);
  const act = route ? actionPill(route.action, t) : null;
  const conds = ruleConditions(rule);
  const sep = rule.combineMode === 'and' ? '∧' : '·';
  const title = titleOf(rule);

  const dnsActionText = ((): string | null => {
    const action = dns?.action;
    if (!action || action.type === 'fakeIp') return null;
    if (action.type === 'server') {
      return t('rules.dnsActionServer', { name: dnsActionName ?? action.serverId });
    }
    if (action.type === 'group') {
      return t('rules.dnsActionGroup', { name: dnsActionName ?? action.groupId });
    }
    if (action.type === 'hostsFirst') {
      return t('rules.dnsActionHosts', { name: dnsActionName ?? action.hostsServerId });
    }
    if (action.type === 'reject') return t('rules.dnsActionReject');
    if (action.type === 'predefined') return t('rules.dnsActionPredefined');
    return t('rules.dnsResolverInherit');
  })();

  /**
   * 常驻解释行。**每条角标一行，不做聚合**（§4.12 明写 per item, not in aggregate）：
   * 聚合成一句「本行有 2 个问题」等于把用户又推回去猜是哪两个。
   */
  const notes: Array<{ id: string; text: string; warn?: boolean }> = [];
  if (routeInactive) notes.push({ id: 'route-inactive', text: t('rules.routeInactiveHint') });
  if (stagedOnly) notes.push({ id: 'staged', text: t('home.stagedOnlyHint') });
  if (hasMeshOverlap) notes.push({ id: 'mesh', text: t('rules.meshOverlapTip') });
  if (hasMissingResource)
    notes.push({ id: 'res-missing', text: t('rules.resourceMissingTip'), warn: true });
  if (targetMissing && route?.action === 'proxy')
    notes.push({ id: 'target-missing', text: t('rules.targetMissingTip'), warn: true });
  /* 角标⑤：桌面 `NetworkProfilePill` 挂在 `data-tip` 上的那几句，不生效的四档在这里常驻（§4.12）；
     正常那一档的说明（「只在 X 网络下生效 · 本机用 … 识别」）跟着溢出面板的详情走。 */
  const badge = networkProfileBadge ?? null;
  if (badge?.state === 'missing')
    notes.push({ id: 'np', text: t('rules.networkProfile.badgeMissingTip'), warn: true });
  else if (badge?.state === 'disabled')
    notes.push({ id: 'np', text: t('rules.networkProfile.badgeDisabledTip'), warn: true });
  else if (badge?.state === 'unavailable')
    notes.push({
      id: 'np',
      text: t('rules.networkProfile.badgeUnavailableTip', { reason: t(mobileReasonKey(badge.reasonKey)) }),
      warn: true,
    });
  else if (badge?.state === 'warning')
    notes.push({ id: 'np', text: `${t(badge.warningKey)} · ${t(PROFILE_MATCH_KEYS[badge.match])}`, warn: true });
  /* 命中态（N4）：只有「正常 / 告警」两档谈得上（同桌面 `NetworkProfilePill`）。圆点画在徽标里；
     文案桌面挂在 tip 上，这里告警档并进上面那条常驻说明，正常档跟着溢出面板的详情走。 */
  const matchText =
    badge?.state === 'ok' || badge?.state === 'warning' ? t(PROFILE_MATCH_KEYS[badge.match]) : null;

  const sheetActions: Array<SheetAction | 'separator'> = [
    {
      id: 'move-top',
      label: t('rules.moveTop'),
      icon: <MoveIcon to="top" />,
      disabled: isFirst,
      onSelect: () => onMove(rule, 'top'),
    },
    {
      id: 'move-up',
      label: t('rules.moveUp'),
      icon: <MoveIcon to="up" />,
      disabled: isFirst,
      onSelect: () => onMove(rule, 'up'),
    },
    {
      id: 'move-down',
      label: t('rules.moveDown'),
      icon: <MoveIcon to="down" />,
      disabled: isLast,
      onSelect: () => onMove(rule, 'down'),
    },
    {
      id: 'move-bottom',
      label: t('rules.moveBottom'),
      icon: <MoveIcon to="bottom" />,
      disabled: isLast,
      onSelect: () => onMove(rule, 'bottom'),
    },
    'separator',
    {
      /*
       * 启停也进面板。行尾那颗开关只有 `aria-label` —— 属性不是**常驻通道**（§4.12 要的是
       * 看得见的字），且开关本身说不出「这是在启用还是停用这条规则」。
       */
      id: 'toggle',
      label: t('rules.toggleEnabled'),
      onSelect: () => onToggle(rule),
    },
    {
      id: 'duplicate',
      label: t('rules.duplicate'),
      icon: <DuplicateIcon />,
      onSelect: () => onDuplicate(rule),
    },
    {
      id: 'edit',
      label: t('common.edit'),
      icon: <EditIcon />,
      disabled: onEdit === undefined,
      description: onEdit === undefined ? editUnavailableReason : undefined,
      onSelect: onEdit ? () => onEdit(rule) : undefined,
    },
    {
      id: 'delete',
      label: deleteConfirming ? t('common.confirmAgain') : t('common.delete'),
      icon: <DeleteIcon />,
      danger: true,
      // `.confirming` 是与 `confirm-twice` 的契约，不是样式；漏了它这条删除永远确认不了。
      confirming: deleteConfirming,
      onSelect: () => onDelete(rule),
    },
  ];

  return (
    <div
      ref={rowRef}
      className={[enabled ? 'mr-row' : 'mr-row off', dragging ? 'dragging' : '']
        .filter(Boolean)
        .join(' ')}
      role="listitem"
      /* 拖起来时跟着手指走：没有这一笔，长按之后屏幕上什么都不动，用户读不出「已经拿起来了」。 */
      style={dragging ? { transform: `translateY(${dragDy ?? 0}px)` } : undefined}
    >
      <span className="mr-pri" aria-label={t('rules.priority')}>
        {index + 1}
      </span>
      {/*
        抓握条（IA §4.13 的「on the existing grip」）。位置与桌面同 —— `.rule-pri` 之后、
        行主体之前（`RuleItem.tsx:280-282`）。
        `aria-hidden` 同桌面：它是指针手势的可供性，重排的**可访问通道**是面板里那四条移动动作。
      */}
      <span
        className="mr-grip"
        data-drag-handle="1"
        aria-hidden
        onPointerDown={onGripDown}
        onPointerMove={onGripMove}
        onPointerUp={onGripUp}
        onPointerCancel={onGripCancel}
      >
        <GripIcon />
      </span>
      {/*
        类型分类图标。位置同桌面（`RuleItem.tsx:285` 的 `.rule-type-ic`，在抓握条之后、行主体之前）。
        桌面把 hover 详情卡的触发器挂在它身上；触屏没有 hover ⇒ 这里它只承担**分类识别**这一格，
        详情由行尾那颗溢出键点开（同一份内容，见下面 `ActionSheet` 的 `body`）。
        它不是装饰：条件摘要只在 `conds.length > 0` 时渲染，零条件规则的行上没有别的类型线索。
      */}
      <span className="mr-row-ty" aria-hidden>
        <TypeIcon type={rule.type} />
      </span>
      <div className="mr-row-main">
        <div className="mr-row-title">{title}</div>
        {conds.length > 0 && (
          <div className="mr-row-sub">
            {conds.map((c, i) => (
              <span key={`${c.type}-${i}`}>
                {i > 0 && <span> {sep} </span>}
                {t(`rules.types.${c.type}.name`)}
                <span> ×{c.values.length}</span>
              </span>
            ))}
          </div>
        )}
        <div className="mr-row-pills">
          {act && (
            <span className={routeInactive ? `mr-pill ${act.cls} inactive` : `mr-pill ${act.cls}`}>
              {act.text}
            </span>
          )}
          {dns && (
            <span className="mr-pill">
              {t('rules.dnsPill', {
                answer:
                  dns.answerMode === 'fakeIp' ? t('rules.dnsAnswerFakeIp') : t('rules.dnsAnswerReal'),
              })}
            </span>
          )}
          {dns && dnsActionText != null && <span className="mr-pill">{dnsActionText}</span>}
          {dns && !dns.action && dns.answerMode === 'real' && (
            <span className="mr-pill">
              {dns.resolver === 'proxy'
                ? t('rules.dnsResolverProxy')
                : dns.resolver === 'direct'
                  ? t('rules.dnsResolverDirect')
                  : t('rules.dnsResolverInherit')}
            </span>
          )}
          {stagedOnly && <span className="mr-pill">{t('home.stagedOnlyBadge')}</span>}
          {hasMeshOverlap && <span className="mr-pill">{t('rules.meshOverlap')}</span>}
          {hasMissingResource && <span className="mr-pill warn">{t('rules.resourceMissing')}</span>}
          {/* 角标③与 `→ 节点名` 互斥：节点还在就显示名字，删了就显示角标，绝不显示空箭头。 */}
          {route?.action === 'proxy' &&
            (targetMissing ? (
              <span className="mr-pill warn">{t('rules.targetMissing')}</span>
            ) : (
              targetNodeName != null && <span className="mr-pill">→ {targetNodeName}</span>
            ))}
          {badge !== null &&
            (badge.state === 'missing' ? (
              <span className="mr-pill warn">{t('rules.networkProfile.badgeMissing')}</span>
            ) : (
              <span className={badge.state === 'ok' ? 'mr-pill' : 'mr-pill warn'}>
                {(badge.state === 'ok' || badge.state === 'warning') && matchText !== null && (
                  <MatchDot match={badge.match} label={matchText} />
                )}
                {t('rules.networkProfile.badge', { name: badge.name })}
              </span>
            ))}
        </div>
        {notes.map((note) => (
          <div key={note.id} className={note.warn ? 'mr-note warn' : 'mr-note'}>
            {note.text}
          </div>
        ))}
        <InlineError text={error} />
      </div>
      <div className="mr-row-acts">
        <Switch
          id={`mr-rule-${plane}-${rule.id}`}
          checked={enabled}
          onChange={() => onToggle(rule)}
          label={t('rules.toggleEnabled')}
        />
        <button
          type="button"
          className="mr-btn ghost icon"
          aria-label={t('mobileRules.more')}
          aria-haspopup="dialog"
          aria-expanded={sheetOpen}
          onClick={() => onOpenSheet(rule)}
        >
          <MoreIcon />
        </button>
      </div>
      {sheetOpen && (
        <ActionSheet
          title={title}
          /*
           * 桌面规则 hover 详情卡（`hover-cards/RuleHoverCard.tsx#RuleHoverCardContent`）在这里的落点。
           * 触屏没有 hover ⇒ 同一份内容跟着行尾那颗溢出键被点开（§4.12：解释要有常驻通道）。
           *
           * 🔴 **四格此前整块缺**，逐条补齐（缺的那几格恰好在最需要解释的那些行上）：
           *  ① **零条件**（`conds.length === 0`，一条匹配全部流量的规则）—— 桌面明写
           *     `rules.noConditions`「（无条件）」，移动端此前 `conds.map` 对空数组渲染出**空白**：
           *     点开面板什么都没有，与「详情坏了」同形；
           *  ② **与/或标记** —— 多条件时桌面在头部画 `rules.combineAnd` / `rules.combineOr` 标签。
           *     行摘要上那个 `∧` / `·` 连接符只有两个字符宽，读不出「全部满足」还是「满足任一」，
           *     而这决定这条规则到底命中什么；
           *  ③ **停用角标** `rules.disabledBadge` —— 判据是**裸 `rule.enabled`**（同桌面），
           *     与行上那个按执行平面算的 `enabled` 不是一回事：一条整体停用的规则，在 DNS 平面
           *     仍可能因为 `effects.dns.enabled !== false` 而在行上显示成开着的；
           *  ④ **路由 / DNS 生效摘要** —— 桌面用两条 `.tc-row` 说「这条规则最终把流量送去哪、
           *     DNS 怎么答」。行上的药丸只画了其中一部分且没有标签，说不出哪条是路由哪条是 DNS。
           */
          body={
            <>
              <div className="mr-row-pills">
                {!rule.enabled && <span className="mr-pill warn">{t('rules.disabledBadge')}</span>}
                {conds.length > 1 && (
                  <span className="mr-pill">
                    {t(rule.combineMode === 'and' ? 'rules.combineAnd' : 'rules.combineOr')}
                  </span>
                )}
              </div>
              {conds.length === 0 ? (
                <div>{t('rules.noConditions')}</div>
              ) : (
                conds.map((c, i) => (
                  <div key={`${c.type}-${i}`}>
                    {t(`rules.types.${c.type}.name`)} · {c.values.length}
                    {c.values.length > 0 && <div>{c.values.join(', ')}</div>}
                  </div>
                ))
              )}
              {route && (
                <div>
                  {t('rules.routeEffect')} ·{' '}
                  {actionPill(route.action, t).text}
                  {route.action === 'proxy' &&
                    ` → ${targetNodeName ?? t('rules.targetDefaultProxy')}`}
                </div>
              )}
              {badge?.state === 'ok' && (
                <div>
                  {badge.source
                    ? t('rules.networkProfile.badgeTip', {
                        name: badge.name,
                        source: t(
                          badge.source === 'dhcp'
                            ? 'rules.networkProfile.sourceDhcp'
                            : 'rules.networkProfile.sourceSystem',
                        ),
                      })
                    : t('rules.networkProfile.badgeTipNoSource', { name: badge.name })}
                  {` · ${matchText}`}
                </div>
              )}
              {dns && (
                <div>
                  {t('rules.dnsEffect')} ·{' '}
                  {dns.answerMode === 'fakeIp'
                    ? t('rules.dnsAnswerFakeIp')
                    : t('rules.dnsAnswerReal')}
                  {dns.answerMode === 'real' &&
                    ` · ${
                      dns.resolver === 'proxy'
                        ? t('rules.dnsResolverProxy')
                        : dns.resolver === 'direct'
                          ? t('rules.dnsResolverDirect')
                          : t('rules.dnsResolverInherit')
                    }`}
                </div>
              )}
            </>
          }
          actions={sheetActions}
          onClose={() => onOpenSheet(null)}
          closeLabel={t('common.cancel')}
        />
      )}
    </div>
  );
}

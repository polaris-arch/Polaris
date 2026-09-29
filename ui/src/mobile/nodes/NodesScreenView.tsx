import { MobileSpeedTestProgress } from '../MobileSpeedTestProgress';
import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端「节点」屏的**呈现层**（IA spec §2.1）。纯组件：不读 store、不发 IPC、不碰 `document`，
 * 全部输入经 `view-model.ts` 的 props 进来 —— 这样门才能在 node 环境下真渲染它并对着 DOM 断言。
 *
 * ── DOM 序 = 重要性序（IA §2 前言）────────────────────────────────────────────
 *   .mn-top（sticky）  ① 页头（标题 + 次级计数 + 添加 + 溢出）
 *                      ② 分组段（自建 / 组网 / 每个订阅，带计数角标）
 *   ③ 订阅摘要（仅订阅分组）
 *   ④ 工具栏（搜索 + 筛选/排序）
 *   ⑤ 安全注脚（TsExitWarning）+ 组网隧道健康
 *   ⑥ 列表
 *   ⑦ 批量条（sticky bottom，仅批选态）
 *
 * 页头与分组段合成**一块** sticky：spec 要求「页头固定 + 分组段吸在页头下」，两者都钉在滚动区顶端，
 * 合成一块与分别两块 sticky 的可见结果逐像素相同，而后者要给第二块算一个「页头有多高」的偏移 ——
 * 那个数会随 Dynamic Type 变，写死就漂。**结构上不需要的数就不要引进来。**
 *
 * ── IA 裁定 #3：`TsExitWarning` 移植的是不变量，不是坐标 ─────────────────────
 * 不变量 =「**紧贴每一个**能产生该状态的控件」。本屏能产生该状态的控件只有一类：每一行的
 * 「设为出口」（整行即按钮，桌面整卡同语义）。它们全部落在 `[data-exit-scope]` 这一个容器里，
 * 警示就渲染在同一个容器内、列表正上方 —— 这是「紧贴」在一个纵向列表上的形态。
 *
 * 每一颗写 exit_node 的控件都必须带 `data-exit-write`。这不是装饰：门会
 *  ① 扫 `ui/src/mobile/**` 全部源码，找出所有绑了出口写入回调的 JSX 元素，
 *     断言它们**逐个**带这个标记（新加一颗不带标记的 ⇒ 当场红）；
 *  ② 真渲染本屏，断言每一个带标记的元素都与一条 `data-ts-exit-warning` 同处一个 scope。
 * 只钉「本屏上有一处警示」守不住第二个入口 —— 那正是这类静默直连泄漏活下来的方式。
 *
 * ── 卡片/列表双视图不移植（收敛文档 §2.2）──────────────────────────────────
 * compact 宽度下卡片墙每行只放得下一张卡，形态收敛成「更高的列表」。维持两套排布换零个可见差异。
 *
 * ── §4.12：hover 承载的解释全部换常驻通道 ──────────────────────────────────
 * 触屏没有 hover。凡桌面只挂 `data-tip` 的解释，这里要么是行内第二行（`.mn-note`），
 * 要么是 action-sheet 里那一行自己的说明。逐处落点见设计文档的对照表。
 */
import { useEffect, useRef, useState, type ReactElement, type ReactNode } from 'react';
/* 延迟分档取桌面**已有**的那条腿，不在这里另写阈值：同一个 42ms 在两个客户端上必须读成同一档，
   就地抄一份阈值等于给「快」这个词造第二个定义。`format.ts` 是纯 `.ts`、无 CSS、不导出组件，
   契约 A1 的两条风险（视觉走桌面层叠链 / 控件藏到门的取材面外）一条都不成立。 */
import { latLevel, type LatLevel } from '@/components/screens/shared/format';
import { useDismissableLayer } from '../back-stack';
import { MobileInfo } from '../MobileInfo';
import { MobileMeshRouteEvidence, meshRouteSummaryKey } from '../MobileMeshRouteEvidence';
import { TS_EXIT_SUMMARY_KEY } from '../ts-exit-help';
import { SheetHeading } from '../SheetHeading';
import { buildNodeMoreItems, speedTestBlockedReason } from './view-model';
import type {
  EmptyKind,
  GroupTabVM,
  NodeRowVM,
  NodesScreenViewProps,
  SheetItem,
} from './view-model';

/* ────────────────────────────────────────────────────────────────────────────
 * 图标：只用 `icons/icon-registry.json` 登记过的那几个（policy: reuse-current-client-only，
 * allowRedesign: false，missingIconBehavior: omit the icon or request an approved source;
 * never invent one）。字形的 path data 逐字取自桌面同名控件，**不重画**；内联而不是 import
 * 桌面 `.tsx`（契约 A1）。其余位置一律用文字标签 —— 未登记的图标宁可不画。
 * ──────────────────────────────────────────────────────────────────────────── */

/** `state.warning`：逐字取自桌面 `home/TsExitWarning.tsx` 的三角警示。 */
function WarningGlyph(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.9} aria-hidden>
      <path d="M12 3.2L21 19H3z" />
      <path d="M12 10v4M12 17h.01" />
    </svg>
  );
}

/** `state.info`：逐字取自桌面 `home/MeshTunnelHealth.tsx` 的圆圈感叹号。两条注脚**用不同图标**
 *  区分「公网不经这条隧道」与「这条隧道本身通不通」—— 桌面那条理由原样成立。 */
function InfoGlyph(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.9} aria-hidden>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7.5v5M12 16h.01" />
    </svg>
  );
}

/** `action.search`（已登记，purpose: Search field leading glyph）：逐字取自桌面 `NodesToolbar.tsx:74-77`。 */
function SearchGlyph(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-3-3" />
    </svg>
  );
}

/** `action.close`（已登记，purpose: Clear a search field）：逐字取自桌面 `ConnectionsScreen.tsx` 的关闭字形。 */
function CloseGlyph(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <path d="M5 5l14 14M19 5L5 19" />
    </svg>
  );
}

/* ────────────────────────────────────────────────────────────────────────────
 * bottom-sheet（§3.1 第 14 条）：桌面 `dialogs/Modal.tsx` 的三段式结构原样搬到底部锚定容器。
 * 遮罩点击关闭对应桌面那条「两次 pointer 事件都落在遮罩上才关」的规则 —— 理由相同：
 * 不因一个起点在内容里的手势而关闭。
 * ──────────────────────────────────────────────────────────────────────────── */
export function Sheet({
  title,
  onClose,
  closeLabel,
  children,
}: {
  title: string;
  onClose: () => void;
  closeLabel: string;
  children: ReactNode;
}): ReactElement {
  /* 系统返回键关掉的就是这一层。本屏五处面板（添加 / 更多 / 行动作 / 订阅 / 筛选排序）都只在
     打开时渲染本组件 ⇒ 恒 `true`。**导出**是为了让门能单独把它渲染开 —— 面板的开合是
     `NodesScreenView` 的屏内 `useState`，容器外驱动不动，不导出就只能靠源码扫描代替行为断言。 */
  useDismissableLayer(true, onClose);
  return (
    <div className="mn-sheet-layer" role="dialog" aria-modal="true" aria-label={title}>
      <button type="button" className="mn-sheet-scrim" aria-label={closeLabel} onClick={onClose} />
      <div className="mn-sheet">
        <div className="mn-sheet-grip" aria-hidden />
        <SheetHeading title={title} onClose={onClose} closeLabel={closeLabel} className="mn-sheet-head m-menu-heading" closeClassName="mn-sheet-close" />
        <div className="mn-sheet-body">{children}</div>
      </div>
    </div>
  );
}

/** action-sheet 的一行。置灰行**保留在场**并把理由写在下面（§3.3 第 2 条 + §4.12）。 */
function SheetRow({ item, onDone, busy = false, busyLabel, stayOpen = false }: { item: SheetItem; onDone: () => void; busy?: boolean; busyLabel?: string; stayOpen?: boolean }): ReactElement {
  const disabled = item.disabledReason !== undefined || busy;
  return (
    <div className={`mn-sheet-row${item.danger === true ? ' danger' : ''}`}>
      <button
        type="button"
        className="mn-sheet-btn"
        disabled={disabled}
        aria-busy={busy}
        onClick={() => {
          item.onSelect?.();
          if (!stayOpen) onDone();
        }}
      >
        {busy ? busyLabel : item.label}
      </button>
      {item.note !== undefined && <p className="mn-note">{item.note}</p>}
      {disabled && <p className="mn-note">{item.disabledReason}</p>}
    </div>
  );
}

/* ────────────────────────────────────────────────────────────────────────────
 * 分组段（§3.1 第 6 条 segmented-tabs，touch-adapted）：横向滚动 + **选中项滚进视野** +
 * 每项最小 48 高；计数角标、下划线、字型全不变。前两项就是这条组件被列为 touch-adapted 的
 * 那两项改动，缺一条都不叫做完。
 * ──────────────────────────────────────────────────────────────────────────── */
function GroupSegments({
  groups,
  activeTab,
  onSelectTab,
  t,
}: {
  groups: readonly GroupTabVM[];
  activeTab: string;
  onSelectTab: (id: string) => void;
  t: NodesScreenViewProps['t'];
}): ReactElement {
  /*
   * 选中项滚进视野。订阅多到段条溢出时，进屏或恢复上次分组后，选中的那一颗可能停在屏幕右侧
   * 看不见的位置 —— 用户看到的是一条**没有任何高亮**的段条，得先横向拨一下才知道自己在哪个分组。
   * 桌面靠 hover 与键盘遍历绕过这件事，触屏两样都没有。
   *
   * `block: 'nearest'` 是关键：段条住在 sticky 的 `.mn-top` 里，用默认的 `'start'` 会顺手把
   * 整个滚动区往上顶一截。写法照抄本仓已有先例 `components/dialogs/Csel.tsx:234`。
   * 收集器用 ref map 而不是 querySelector：后者要碰 `document`，本组件是纯组件（见文件头注）。
   */
  const segRefs = useRef(new Map<string, HTMLButtonElement | null>());
  useEffect(() => {
    segRefs.current.get(activeTab)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [activeTab]);

  return (
    <nav className="mn-segs" aria-label={t('nodes.mobileGroupsLabel')}>
      {groups.map((g) => (
        <button
          key={g.id}
          type="button"
          className="mn-seg"
          ref={(el) => {
            segRefs.current.set(g.id, el);
          }}
          aria-current={g.id === activeTab ? 'true' : undefined}
          onClick={() => onSelectTab(g.id)}
        >
          <span className="mn-seg-label">{g.label}</span>
          {g.count > 0 && <span className="mn-seg-count">{g.count}</span>}
          {/* 失败标记带**文案**，与桌面同键（`NodesTabs.tsx:71`）。此前这里是一个裸「!」：
              不切过去就不知道它是什么意思，读屏软件读出来的也只是「感叹号」——§4.12 那句
              「含义够不着的角标比没有角标更坏，它会被读成装饰」说的就是这一颗。
              `.mn-segs` 是 overflow-x:auto、`.mn-seg` 是 nowrap，容得下，不必改样式。 */}
          {g.failureDetail !== undefined && (
            <span className="mn-pill err">{t('nodes.subUpdateFailed')}</span>
          )}
        </button>
      ))}
    </nav>
  );
}

/** 一行节点。桌面 `.nd-card` 的视觉词汇原样过来；变的只是**动作从铺开改为可达**（§3.1 第 18 条）。 */
/** The card retains its status pills; all applicable background explanations share one details entry. */
export function nodeExplanationSections(row: NodeRowVM, t: NodesScreenViewProps['t']): readonly { title: string; text: string }[] {
  const sections: { title: string; text: string }[] = [];
  if (!row.speedTestable && row.speedTestBlockedHint !== undefined && !row.stagedOnly) {
    sections.push({ title: t('mobileHelp.speedTest'), text: row.speedTestBlockedHint });
  }
  if (row.speedTestable && row.latencyMs === null) {
    sections.push({ title: t('nodes.timeout'), text: t('nodes.timeoutHint') });
  }
  if (row.lanOnly) sections.push({ title: t('nodes.lanOnly'), text: t('nodes.lanOnlyHint') });
  if (row.stagedOnly) sections.push({ title: t('home.stagedOnlyBadge'), text: t('mobileHelp.stagedNodeDetails') });
  return sections;
}

function NodeRow({
  row,
  t,
  batchMode,
  selected,
  onUseAsExit,
  onToggleSelect,
  onOpenActions,
}: {
  row: NodeRowVM;
  t: NodesScreenViewProps['t'];
  batchMode: boolean;
  selected: boolean;
  onUseAsExit: (row: NodeRowVM) => void;
  onToggleSelect: (row: NodeRowVM) => void;
  onOpenActions: (row: NodeRowVM) => void;
}): ReactElement {
  /*
   * 延迟位：陈旧必须读作陈旧、不可测必须读作不可测 —— 都不许渲染成一个数（IA §2.1 States）。
   *
   * 文案与**分档色**走同一条分支链，一次算出来。两处各判各的会造出「写着『超时』却染成中性色」
   * 这种自相矛盾的格子 —— 那正是把一条判据复制成两条的标准后果。
   */
  const lat: { readonly text: string; readonly level: LatLevel } = !row.speedTestable
    ? { text: t(row.stagedOnly ? 'mobileHelp.stagedSpeedTest' : 'nodes.speedTestNotApplicable'), level: 'none' }
    : row.latencyMs === null
      ? { text: t('nodes.timeout'), level: 'dead' }
      : row.latencyMs === undefined
        ? { text: '', level: 'none' }
        : row.latencyStale
          ? /* IA §2.1 States：陈旧**渲染成陈旧，绝不渲染成一个数**。桌面用半透明保留数值，
               那条信号在手机上太容易被读成「就是这个值」——而 30 分钟前的 42ms 与刚测的 42ms
               对用户的下一步含义完全不同。色也跟着退到 `none`：给一个过期的数染上绿，
               等于用颜色把刚才那句话收回去。 */
            { text: t('nodes.mobileLatencyStale'), level: 'none' }
          : { text: `${row.latencyMs} ms`, level: latLevel(row.latencyMs) };
  const numericLatency = row.speedTestable && typeof row.latencyMs === 'number' && !row.latencyStale;
  const explanations = nodeExplanationSections(row, t);
  const hasMeshRoute = row.meshRouteReport !== undefined;

  return (
    <div role="listitem" className={`mn-row${row.isCurrent ? ' cur' : ''}${selected ? ' sel' : ''}`}>
      {/*
        整行 = 主动作。批选态下是勾选，否则是**设为出口** —— 后者是本屏唯一能写 exit_node 的控件，
        故带 `data-exit-write`。改这一行前先读文件头注里门的两条腿。
      */}
      <button
        type="button"
        className="mn-row-main"
        data-exit-write={batchMode ? undefined : 'node-use'}
        aria-pressed={batchMode ? selected : undefined}
        aria-current={!batchMode && row.isCurrent ? 'true' : undefined}
        onClick={() => (batchMode ? onToggleSelect(row) : onUseAsExit(row))}
      >
        <span className="mn-row-copy">
          <span className="mn-row-head">
            <span className={`mn-name${row.invalidReason !== undefined ? ' invalid' : ''}`}>
              {row.server.name}
            </span>
          </span>
          <span className="mn-pills">
            <span className="mn-pill proto">{row.protocolLabel}</span>
            {row.transport !== '' && <span className="mn-xfer">{row.transport}</span>}
            {row.isCurrent && <span className="mn-pill cur">{t('nodes.selectedChoice')}</span>}
            {row.isExit && !row.isCurrent && (
              <span className="mn-pill exit">{t('nodes.exitCapableBadge')}</span>
            )}
            {row.lanOnly && <span className="mn-pill warn">{t('nodes.lanOnly')}</span>}
            {row.stagedOnly && <span className="mn-pill warn">{t('mobileHelp.stagedNode')}</span>}
          </span>
          {hasMeshRoute && <span className="mn-route-summary">{t(meshRouteSummaryKey(
            row.meshRouteReport ?? null, row.server.id, row.meshRoutePrevious, row.meshRouteLegacy,
            row.meshRouteContextMismatch,
          ))}</span>}
          {/* Invalid configuration remains directly visible and actionable, rather than hidden in help. */}
          {row.invalidReason !== undefined && <span className="mn-note">{row.invalidReason}</span>}
        </span>
        {lat.text !== '' && <span className={`mn-lat ${lat.level}`} data-numeric={numericLatency ? '' : undefined}>{lat.text}</span>}
      </button>
      {/* Both secondary controls are outside the exit-selection button. Opening help never selects a node. */}
      {(explanations.length > 0 || hasMeshRoute || !batchMode) && <span className="mn-row-actions">
      {(explanations.length > 0 || hasMeshRoute) && <MobileInfo
        title={t('mobileHelp.nodeDetails', { name: row.server.name })}
        triggerLabel={t('mobileHelp.viewDetails', { title: row.server.name })}
        details={<>
          {hasMeshRoute && <MobileMeshRouteEvidence report={row.meshRouteReport ?? null}
            serverId={row.server.id} previous={row.meshRoutePrevious} legacy={row.meshRouteLegacy}
            contextMismatch={row.meshRouteContextMismatch}
            nameOf={(id) => row.meshRouteNames?.get(id) ?? id} />}
          {explanations.map((section, index) => <section key={index}><h3>{section.title}</h3><p>{section.text}</p></section>)}
        </>}
      />}
      {!batchMode && (
        <button
          type="button"
          className="mn-row-more"
          aria-label={`${row.server.name} · ${t('nodes.mobileRowMore')}`}
          onClick={() => onOpenActions(row)}
        >
          {/* `navigation.chevron`（已登记）：这颗打开的是本行的动作面，语义就是「展开到别处」。 */}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
            <path d="M9 6l6 6-6 6" />
          </svg>
        </button>
      )}
      </span>}
    </div>
  );
}

const EMPTY_KEY: Record<EmptyKind, string> = {
  filtered: 'nodes.emptyFiltered',
  sub: 'nodes.emptySub',
  mesh: 'nodes.meshEmpty',
  all: 'nodes.empty',
};

/** 组网隧道健康的文案表（照抄桌面 `MeshTunnelHealth.tsx` 的 `TEXT_KEY`，一档不多一档不少）。 */
const MESH_HEALTH_KEY: Record<string, string> = {
  'ts-expired': 'home.meshTunnelTsExpired',
  'ts-not-running': 'home.meshTunnelTsNotReady',
  'ts-peers-offline': 'home.meshTunnelTsPeersOffline',
  'vpn-error': 'home.meshTunnelVpnError',
  'vpn-disconnected': 'home.meshTunnelVpnDisconnected',
};

/** TsExitWarning 的文案与动作（照抄桌面同名组件的分支，一档不多一档不少）。 */
const TS_EXIT_KEY: Record<string, string> = {
  'needs-auth': 'home.tsExitNeedsAuthWarn',
  'no-exit-device': 'home.tsExitNoDeviceWarn',
  'exit-device-not-advertised': 'home.tsExitNotAdvertisedWarn',
  'exit-device-offline': 'home.tsExitDeviceOfflineWarn',
};

export function NodesScreenView(props: NodesScreenViewProps): ReactElement {
  const { t } = props;
  const [sheet, setSheet] = useState<
    | { kind: 'add' }
    | { kind: 'more' }
    | { kind: 'row'; row: NodeRowVM }
    | { kind: 'filter' }
    | { kind: 'sub' }
    | null
  >(null);
  const closeSheet = (): void => setSheet(null);

  /* §4.10：核没跑 ⇒ 测速**置灰并给理由**，绝不隐藏。移动端不会另起临时测速内核，
     这与桌面「临时核」那条腿的差异必须说出来，否则用户只会反复点。
     核这一档对**所有**测速入口都成立 —— 判据住在 `view-model.ts#speedTestBlockedReason`，
     行动作面那一侧（接线层 `rowItems`）消费的是同一个函数，不许在任何一侧就地重写。 */
  const coreBlocked = speedTestBlockedReason(t, props.coreRunning);
  /* 两个射程分开：「全部测速」问的是全量，「测可见 / 批量」问的是当前可见集。
     混用会在「当前分组恰好没有可测节点」时把「全部测速」也锁死。 */
  const speedTestBlocked =
    coreBlocked ?? (props.testableTotal === 0 ? t('nodes.noTestableNodes') : undefined);
  const visibleTestBlocked =
    coreBlocked ??
    (props.rows.some((r) => r.speedTestable) ? undefined : t('nodes.noTestableNodes'));

  const selectedCount = props.selectedIds.size;

  return (
    <section className="mn" aria-label={t('mobileNav.nodes')} data-exit-scope="mobile-nodes">
      {/* ── ①② 页头 + 分组段：一块 sticky ─────────────────────────────────── */}
      <div className="mn-top">
        <header className="mn-head">
          <div className="mn-head-title">
            <h1 className="mn-title">{t('mobileNav.nodes')}</h1>
            <p className="mn-count">{props.countSummary}</p>
          </div>
          <div className="mn-head-acts">
            <button type="button" className="mn-act primary" onClick={() => setSheet({ kind: 'add' })}>
              {t('nodes.add')}
            </button>
            <button
              type="button"
              className="mn-act more"
              aria-label={t('nodes.mobileMoreActions')}
              onClick={() => setSheet({ kind: 'more' })}
            >
              <span aria-hidden="true">⋯</span>
            </button>
          </div>
        </header>
        <GroupSegments
          groups={props.groups}
          activeTab={props.activeTab}
          onSelectTab={props.onSelectTab}
          t={t}
        />
      </div>

      {/* ── ③ 订阅摘要 ────────────────────────────────────────────────────── */}
      {props.sub !== undefined && (
        <div className="mn-sub" role="group" aria-label={props.sub.name}>
          <div className="mn-sub-head">
            <div className="mn-sub-status">
              <span className={`mn-pill${props.sub.updatingLabel !== undefined || props.refreshBusy ? ' warn' : ''}`}>
                {props.sub.updatingLabel ?? (props.refreshBusy ? t('mobileActions.preparing') : props.sub.autoLabel)}
              </span>
              {props.sub.lastUpdated !== undefined && <span className="mn-sub-last">{props.sub.lastUpdated}</span>}
            </div>
            <div className="mn-sub-actions">
              <button
                type="button"
                className="mn-act"
                /* 后端没有单飞闸，连点会真的并发拉两次同一订阅（两次对账互相覆盖）。 */
                disabled={props.sub.updatingLabel !== undefined || props.refreshBusy}
                onClick={props.onRefreshSub}
              >
                {t('nodes.subRefresh')}
              </button>
              <button type="button" className="mn-act" onClick={() => setSheet({ kind: 'sub' })}>
                {t('nodes.subMenu')}
              </button>
            </div>
          </div>
          {props.sub.autoHint !== undefined && <p className="mn-note">{props.sub.autoHint}</p>}
          {props.sub.failureDetail !== undefined && (
            <p className="mn-note err">
              {t('nodes.subUpdateFailed')}
              {t('common.colon')}
              {props.sub.failureDetail}
            </p>
          )}
          {/*
            IA §2.1「Data positions with no registered source」的三处（用量 / 到期 / 上次更新）——
            2026-09-06（W-06）来源已登记进 `mobile-kit/data-contract.json#sources.subscription-summary`
            ⇒ 从「标 pending」变成真的画。判据与桌面 `SubInfoBar.tsx:191-194` 同一批，值由接线层算好。

            🔴 **每一处都是「有才画」**：`undefined` 表示订阅面板没下发那个字段，此时**整格不出现**，
            不画 `0 B / 0 B` 也不画「—」——「面板没给」与「用了 0」是两件事，后者是可以据以决策的
            事实，前者不是。
          */}
          {props.sub.usage !== undefined && (
            <div className={`mn-sub-usage${props.sub.usage.warn ? ' warn' : ''}`}>
              <span className="mn-sub-usage-tx">{props.sub.usage.text}</span>
              <span className="mn-sub-bar" role="presentation">
                <i style={{ width: `${props.sub.usage.pct}%` }} />
              </span>
            </div>
          )}
          {props.sub.expiry !== undefined && <p className="mn-note">{props.sub.expiry}</p>}
        </div>
      )}

      {/* ── ④ 工具栏 ───────────────────────────────────────────────────────── */}
      <div className="mn-toolbar">
        {/* §3.1 第 9 条：前导字形（身份标识）+ 清除键（触屏没有 Esc；`type="search"` 在
            Android WebView 上并不兑现那颗原生键）。整块是 `<label>`，点字形也能聚焦到输入框。 */}
        <label className="mn-search">
          <SearchGlyph />
          <input
            type="search"
            value={props.search}
            placeholder={t('common.search')}
            aria-label={t('common.search')}
            onChange={(e) => props.onSearch(e.target.value)}
          />
          {props.search !== '' && (
            <button
              type="button"
              className="mn-search-clear"
              aria-label={t('mobileConnections.clearSearch')}
              onClick={() => props.onSearch('')}
            >
              <CloseGlyph />
            </button>
          )}
        </label>
        {/* 协议筛选（expanded 内联）与排序（medium 起内联）；compact 下两者都在下面那张 sheet 里。
            **控件在三档都存在**，变的只是排布 —— 这正是 IA §2 前言的 content parity。 */}
        <MobileSelect
          className="mn-select mn-only-expanded"
          value={props.protoFilter}
          aria-label={t('nodes.mobileProtocolFilter')}
          onChange={(e) => props.onProtoFilter(e.target.value)}
        >
          <option value="">{t('common.all')}</option>
          {props.protoOptions.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </MobileSelect>
        <MobileSelect
          className="mn-select mn-from-medium"
          value={props.sortKey}
          aria-label={t('nodes.sortBy')}
          onChange={(e) => props.onSortKey(e.target.value as NodesScreenViewProps['sortKey'])}
        >
          <option value="default">{t('common.default')}</option>
          <option value="name">{t('nodes.mobileSortName')}</option>
          <option value="lat">{t('nodes.mobileSortLatency')}</option>
          <option value="proto">{t('nodes.mobileSortProtocol')}</option>
        </MobileSelect>
        <button
          type="button"
          className="mn-act mn-until-expanded"
          onClick={() => setSheet({ kind: 'filter' })}
        >
          {t('nodes.mobileFilterSort')}
        </button>
        <button
          type="button"
          className="mn-act mn-only-expanded"
          disabled={visibleTestBlocked !== undefined || props.testing}
          onClick={props.onTestVisible}
        >
          {t('nodes.testVisible')}
        </button>
        {visibleTestBlocked !== undefined && (
          <span className="mn-toolbar-reason mn-only-expanded">{visibleTestBlocked}</span>
        )}
      </div>

      {/* ── ⑤ 安全注脚：紧贴下面那批能写 exit_node 的控件 ───────────────────── */}
      {props.tsExitWarning !== 'none' && (
        <div className="mn-notice warn" role="status" data-ts-exit-warning={props.tsExitWarning}>
          <span className="mn-notice-icon">
            <WarningGlyph />
          </span>
          <p className="mn-notice-text">
            <MobileInfo title={t('mobileHelp.tsExitTitle')} tone="warn"
              summary={<>{t(TS_EXIT_SUMMARY_KEY[props.tsExitWarning])}
                <button type="button" className="mn-notice-act" onClick={props.onTsExitAction}>
                  {props.tsExitWarning === 'needs-auth' ? t('home.tsExitGoAuth') : t('home.tsExitPickDevice')}
                </button>
              </>}
              details={<p>{t(TS_EXIT_KEY[props.tsExitWarning] ?? 'home.tsExitNoDeviceWarn')}</p>} />
          </p>
        </div>
      )}
      {/* 组网隧道健康（IA 裁定 #3 后半）：状态展示，不是 safety 类，故落组网分组、不跟控件。
          `'unprobeable'` 与 `'none'` 都不渲染 —— 前者是核跑着就恒亮的静态事实，常驻横幅是纯噪声。 */}
      {MESH_HEALTH_KEY[props.meshTunnelHealth] !== undefined && (
        <div className="mn-notice" role="status" data-mesh-tunnel-health={props.meshTunnelHealth}>
          <span className="mn-notice-icon">
            <InfoGlyph />
          </span>
          <p className="mn-notice-text">{t(MESH_HEALTH_KEY[props.meshTunnelHealth])}</p>
        </div>
      )}
      {/* 写失败与持续状态留在屏内；真实完成由移动端 Toast 宿主短暂回执。
          行内错误保留动作上下文，避免用户只看到状态回滚。 */}
      {props.notice !== undefined && (
        <div className={`mn-notice ${props.notice.tone}`} role="status">
          <p className="mn-notice-text">{props.notice.text}</p>
        </div>
      )}

      <MobileSpeedTestProgress />

      {/* ── ⑥ 列表 ─────────────────────────────────────────────────────────── */}
      {props.rows.length === 0 ? (
        <div className="mn-empty">
          {/* 桌面那句 `nodes.empty` 指向右上角的「添加」。2026-09-06（W-01）那条路径通了 ⇒
              此前补的那句「移动端还没有表单层」随之删掉：它现在会把用户从一个能打开的门支走。 */}
          <p>{t(EMPTY_KEY[props.emptyKind])}</p>
        </div>
      ) : (
        <div className="mn-list" role="list">
          {props.rows.map((row) => (
            <NodeRow
              key={row.server.id}
              row={row}
              t={t}
              batchMode={props.batchMode}
              selected={props.selectedIds.has(row.server.id)}
              onUseAsExit={props.onUseAsExit}
              onToggleSelect={props.onToggleSelect}
              onOpenActions={(r) => setSheet({ kind: 'row', row: r })}
            />
          ))}
        </div>
      )}

      {/* ── ⑦ 批量条：sticky bottom ⇒ 停在滚动区底沿，也就是底部导航正上方。
             不用 `position: fixed` + 「导航高 + 安全区」的魔法数：那个数会随外壳变，且待应用条
             一进槽位就会与它重叠。sticky 让排版自己说出这个位置。 ────────────── */}
      {props.batchMode && (
        <div className="mn-batch" role="group" aria-label={t('nodes.batch')}>
          <span className="mn-batch-count">
            {t('nodes.selectedPrefix')}
            {selectedCount}
            {t('nodes.selectedSuffix')}
          </span>
          <button type="button" className="mn-act" onClick={props.onSelectAll}>
            {t('nodes.selectAll')}
          </button>
          <button
            type="button"
            className="mn-act"
            disabled={selectedCount === 0 || visibleTestBlocked !== undefined || props.testing}
            onClick={props.onBatchSpeedTest}
          >
            {t('nodes.testGroup')}
          </button>
          <button
            type="button"
            className="mn-act"
            disabled={selectedCount === 0 || props.copyBusy}
            aria-busy={props.copyBusy}
            onClick={props.onBatchCopyLinks}
          >
            {props.copyBusy ? t('mobileActions.preparing') : t('nodes.batchCopyLinks')}
          </button>
          {/* 删除（2026-09-06 / W-03 接上）。二次确认**不在这颗按钮上翻红**：触屏没有 hover，
              翻红那一下在拇指底下被自己的手指挡住，而「再点一次」的第二击极易被读成误触重复 ⇒
              点它叠一层确认面板（`forms/ConfirmPanel.tsx`），理由见 `node-deletion.ts` 头注。 */}
          <button
            type="button"
            className="mn-act danger"
            disabled={selectedCount === 0}
            onClick={props.onBatchDelete}
          >
            {t('common.delete')}
          </button>
          {/* 桌面 `.batch-bar` 六颗，这里画五颗；缺的那颗（「移动到分组」§4.14 —— 归因是数据模型，
              桌面同一颗也永久灰着）**登记**在 `absence-register.ts#BATCH_ACTIONS`，理由写在那里。
              桌面留着灰按钮是为了让功能看起来不缺；移动端停靠条一共放得下四五个目标，
              一颗永远死的会挤掉一颗活的。
              门拿桌面按钮数 ↔ 登记表条目数 ↔ 这里真渲染出的按钮数三方对差（`nodes-screen.test.tsx` ⑤），
              下次桌面加一颗、或这里少画一颗，都不会再像「删除」那样悄无声息地没了。 */}
          <button type="button" className="mn-act" onClick={props.onToggleBatchMode}>
            {t('nodes.batchExit')}
          </button>
        </div>
      )}

      {/* ── sheets ─────────────────────────────────────────────────────────── */}
      {sheet?.kind === 'add' && (
        <Sheet title={t('nodes.add')} closeLabel={t('common.close')} onClose={closeSheet}>
          {props.addItems.map((item) => (
            <SheetRow key={item.id} item={item} onDone={closeSheet} />
          ))}
        </Sheet>
      )}
      {sheet?.kind === 'more' && (
        <Sheet
          title={t('nodes.mobileMoreActions')}
          closeLabel={t('common.close')}
          onClose={closeSheet}
        >
          {buildNodeMoreItems({
            t, batchMode: props.batchMode, testing: props.testing,
            testAllBlocked: speedTestBlocked, testVisibleBlocked: visibleTestBlocked,
            onTestAll: props.onTestAll, onTestVisible: props.onTestVisible,
            onToggleBatchMode: props.onToggleBatchMode,
          }).map((item) => <SheetRow key={item.id} item={item} onDone={closeSheet} />)}
        </Sheet>
      )}
      {sheet?.kind === 'row' && (
        <Sheet title={sheet.row.server.name} closeLabel={t('common.close')} onClose={closeSheet}>
          {props.rowItems(sheet.row).map((item) => (
            <SheetRow key={item.id} item={item} onDone={closeSheet}
              stayOpen={item.id === 'copy-link'} busy={item.id === 'copy-link' && props.copyBusy}
              busyLabel={t('mobileActions.preparing')} />
          ))}
        </Sheet>
      )}
      {sheet?.kind === 'sub' && (
        <Sheet
          title={t('nodes.subMenuHeader')}
          closeLabel={t('common.close')}
          onClose={closeSheet}
        >
          {props.subItems.map((item) => (
            <SheetRow key={item.id} item={item} onDone={closeSheet}
              stayOpen={item.id === 'copy-url'} busy={item.id === 'copy-url' && props.copyBusy}
              busyLabel={t('mobileActions.preparing')} />
          ))}
        </Sheet>
      )}
      {sheet?.kind === 'filter' && (
        <Sheet
          title={t('nodes.mobileFilterSort')}
          closeLabel={t('common.close')}
          onClose={closeSheet}
        >
          <div className="mn-sheet-field">
            <span>{t('nodes.mobileProtocolFilter')}</span>
            <MobileSelect
              className="mn-select"
              value={props.protoFilter}
              aria-label={t('nodes.mobileProtocolFilter')}
              onChange={(e) => props.onProtoFilter(e.target.value)}
            >
              <option value="">{t('common.all')}</option>
              {props.protoOptions.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </MobileSelect>
          </div>
          <div className="mn-sheet-field">
            <span>{t('nodes.sortBy')}</span>
            <MobileSelect
              className="mn-select"
              value={props.sortKey}
              aria-label={t('nodes.sortBy')}
              onChange={(e) => props.onSortKey(e.target.value as NodesScreenViewProps['sortKey'])}
            >
              <option value="default">{t('common.default')}</option>
              <option value="name">{t('nodes.mobileSortName')}</option>
              <option value="lat">{t('nodes.mobileSortLatency')}</option>
              <option value="proto">{t('nodes.mobileSortProtocol')}</option>
            </MobileSelect>
          </div>
        </Sheet>
      )}
    </section>
  );
}

export default NodesScreenView;

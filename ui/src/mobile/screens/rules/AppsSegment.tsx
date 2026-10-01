import { MobileInfo } from '../../MobileInfo';
/** Mobile presentation for the production AppsSegment contract.
 * The production RulesScreen owns state, validation, persistence, and sheet
 * selection. This view keeps every callback and shows the platform matching
 * caveat once at the top and on demand for each application. */

import type { ReactElement } from 'react';
import type { CselGroup } from '@/components/dialogs/csel-logic';
import { brandIcon } from '@/components/brand-icons';
import { revealOnToggle } from '@/components/reveal';
import {
  Choice,
  EmptyState,
  Flow,
  InlineError,
  NoticeBanner,
  SearchField,
  SelectSheetPanel,
  SelectSheetTrigger,
} from './Primitives';
import { isAppPolicyWritable, type AppPolicyWritablePick } from './app-policy-write';

/** 应用列表的两种排布。与桌面 `AppPolicyScreen.tsx:80` 的 `AppView` 同两档。 */
export type AppView = 'cards' | 'list';

export interface AppRowModel {
  id: string;
  /** 已解出的显示名（内置走 `rules.apps.*`，自定义直接是用户填的名字）。 */
  name: string;
  /** 桌面 `AppIcon` 的第三态兜底 —— 移动端只用这一态，不走图标代理的网络腿。 */
  emoji: string;
  /** 进程名摘要（桌面 `.ap-proc`）。手机上永不命中，正是上面那条通告条要说的事。 */
  processes?: string;
  /** 当前策略的可读文案（跟随全局 / 直连 / 阻断 / 指定节点名）。 */
  policyText: string;
  /** 策略动作，决定 pill 配色，与规则行同一套颜色角色。 */
  policyTone: 'follow' | 'proxy' | 'direct' | 'block';
  /**
   * 选择器里**当前选中那一项的值**（`follow` / `direct` / `block` / `node:<id>`）。
   *
   * 与 `policyTone` 不是一回事，两者都要：`policyTone` 只说「哪一族颜色」，说不出是**哪个**节点；
   * 而选择器要在几十个节点里勾出正确的那一个、并据此决定默认展开哪一组。
   * 桌面用 `curKind` + `targetServerId` 两个变量表达同一件事（`AppPolicyScreen.tsx:903`）。
   */
  policyValue: string;
  /** 自定义应用（用户自己加的）。只有它才画删除键 —— 内置预设删不掉。 */
  isCustom: boolean;
  category: string;
}

export interface AppsSegmentProps {
  t: (key: string, vars?: Record<string, unknown>) => string;
  /** 五数摘要：总数 / 跟随全局 / 指定节点 / 直连 / 阻断。 */
  summary: { total: number; follow: number; node: number; direct: number; block: number };
  /** 内置预设表还没到（loading）—— 空态文案要说「加载中」而不是「无匹配」。 */
  presetsPending: boolean;
  /** 内置预设加载失败：降级为只剩自定义应用，并明说，而不是给一面白墙。 */
  presetsFailed: boolean;
  /** 应用分流总开关（真值在 config，开关本体在屏头动作里）。 */
  masterEnabled: boolean;
  isSmartMode: boolean;
  onBackToSmart: () => void;
  categories: ReadonlyArray<{ key: string; label: string }>;
  category: string;
  onCategoryChange: (next: string) => void;
  search: string;
  onSearchChange: (next: string) => void;
  /** 已筛选、已按分类分好组的行。 */
  groups: ReadonlyArray<{ key: string; label: string; rows: readonly AppRowModel[] }>;
  /** 这台设备按什么识别应用（平台事实，**不是**缺席声明）。通告条与每一行共用同一句。 */
  platformNote: string;
  /** 列表排布。桌面在同一处有这颗二选一，移动端同两档。 */
  view: AppView;
  onViewChange: (next: AppView) => void;
  /** 总开关写失败的行内回显。 */
  masterError?: string;
  /**
   * 分类 `select-sheet` 的开合态。与本屏其余面板同一套：状态在容器，分段是纯呈现 ——
   * 门因此能拿夹具把「面板开着」这一态直接渲染出来断言，不必先立起 store。
   */
  categorySheetOpen: boolean;
  onCategorySheetOpen: () => void;
  onCategorySheetClose: () => void;
  /** 打开了策略面板的那一行（`null` = 都没开）。与分类面板同一套：单点持有，状态在容器。 */
  policySheetAppId: string | null;
  onPolicySheetOpen: (appId: string) => void;
  onPolicySheetClose: () => void;
  /**
   * 策略选择器的分组候选：**一组策略 + 若干组按订阅折叠的节点**（与桌面 `PolicySelector` 同形）。
   * 由容器用 `domain/server-grouping` 算好传进来 —— 分段是纯呈现，不碰 store。
   */
  policyGroups: readonly CselGroup[];
  /** 默认展开哪几组（含当前选中节点的那一组；没选节点 = 空集全折叠，**不猜第一组**）。 */
  policyOpenGroups: ReadonlySet<string>;
  /** 落一档策略。`targetServerId` 只在「指定某个节点」时给。 */
  onPolicyPick: (appId: string, pick: AppPolicyWritablePick, targetServerId?: string) => void;
  /** 添加一个自定义应用（开表单宿主的 `app-add` 那一层）。 */
  onAddCustom: () => void;
  /** 删除一个自定义应用（预设 + 它的 appRule 同一事务，二次确认在容器）。 */
  onRemoveCustom: (appId: string) => void;
  /** 处于「再点一次即删」武装态的那个自定义应用（单点持有，同规则行 / 资源行）。 */
  removeConfirmingId: string | null;
  /** 逐行的写失败回显（key = `appRule:<appId>`）—— 移动端没有 toast 宿主，失败只能贴在行上。 */
  errorOf: (key: string) => string | undefined;
}

/**
 * 策略档 → 调色类。**只给动作那一半**（`act-*`），底座类由控件自己带 ——
 * 现在承载它的是 `.mr-sel-trigger`（可点的选择器），不再是 `.mr-pill`（只读角标）。
 * `follow` 是默认态，不着色（与 `.mr-pill` 的无修饰基线同调）。
 */
const TONE_ACT_CLASS: Record<AppRowModel['policyTone'], string> = {
  follow: '',
  proxy: 'act-proxy',
  direct: 'act-direct',
  block: 'act-block',
};

/** The Android package leg is absent for custom presets and two built-ins; all
 * variants still require local rule resources for domain/IP matching. */
function matchCopy(row: AppRowModel, t: AppsSegmentProps['t']): { short: string; detail: string } {
  if (row.isCustom) return {
    short: t('mobileRules.matchTrafficOnlyShort'),
    detail: t('mobileRules.matchCustomDetail'),
  };
  if (row.id === 'epic' || row.id === 'riot') return {
    short: t('mobileRules.matchTrafficOnlyShort'),
    detail: t('mobileRules.matchPresetNoPackageDetail'),
  };
  if (row.policyTone === 'direct') return {
    short: t('mobileRules.matchDirectShort'),
    detail: t('mobileRules.matchDirectDetail'),
  };
  return {
    short: t('mobileRules.matchPackageShort'),
    detail: t('mobileRules.matchPackageDetail'),
  };
}

export function AppsSegment({
  t,
  summary,
  presetsPending,
  presetsFailed,
  masterEnabled,
  isSmartMode,
  onBackToSmart,
  categories,
  category,
  onCategoryChange,
  search,
  onSearchChange,
  groups,
  platformNote,
  view,
  onViewChange,
  masterError,
  categorySheetOpen,
  onCategorySheetOpen,
  onCategorySheetClose,
  policySheetAppId,
  onPolicySheetOpen,
  onPolicySheetClose,
  policyGroups,
  policyOpenGroups,
  onPolicyPick,
  onAddCustom,
  onRemoveCustom,
  removeConfirmingId,
  errorOf,
}: AppsSegmentProps): ReactElement {
  const rowCount = groups.reduce((n, g) => n + g.rows.length, 0);
  const matchingLabel = t('mobileRules.matchDetailsLabel');

  /*
   * 桌面 `AppPolicyScreen.tsx:334` 的 `bodyDimmed`，逐字同义。
   * IA §2.2 States 要的是「提示条 **plus** 正文置灰」两件事，不是二选一。
   */
  const bodyDimmed = !masterEnabled || !isSmartMode;
  /* 触发器与面板是两个组件（摆放位置不同，见下），选项表只算一次给两处共用。 */
  const categoryOptions = categories.map((cat) => ({ id: cat.key, label: cat.label }));

  /* 面板是**一个**、由 `policySheetAppId` 决定给谁开（同屏两个面板同时开是误触面，与资源行同口径）。 */
  const policyRow = groups
    .flatMap((group) => group.rows)
    .find((row) => row.id === policySheetAppId);

  /**
   * 一次选择的落点。面板给回来的是一个**值编码**，这里把它解回「哪一档 + 哪个节点」。
   *
   * 🔴 `isAppPolicyWritable` 这一层不能省：DOM 上能点的只有面板里那些项，但「UI 拦住了」
   * 不等于「写路径拦住了」—— 两条判据要成对交，少一条就留着一条从别处调用进来的路。
   */
  const handlePick = (appId: string, value: string): void => {
    if (value.startsWith('node:')) {
      onPolicyPick(appId, 'proxy', value.slice(5));
      onPolicySheetClose();
      return;
    }
    if (!isAppPolicyWritable(value)) return;
    onPolicyPick(appId, value);
    onPolicySheetClose();
  };

  /** 一行 / 一张卡共用的策略选择器触发器。 */
  const policyTrigger = (row: AppRowModel): ReactElement => (
    <SelectSheetTrigger
      /* 🔴 `label` 是**控件名**不是它的值：写成某一档（「指定节点」）会让读屏把控件念成
         一个它未必提供的档位。用 `appPolicy.policy`（桌面同一颗菜单的表头就是它），
         当前档位由 `valueLabel` 那一格自己念。 */
      label={`${row.name} · ${t('appPolicy.policy')}`}
      options={[]}
      value={row.policyValue}
      /* `valueLabel` 不能省：指定节点那一档要显示的是**节点名**（运行期数据），
         选项表里那一行只写得下「指定节点」。 */
      valueLabel={row.policyText}
      toneClass={TONE_ACT_CLASS[row.policyTone]}
      open={policySheetAppId === row.id}
      onOpen={() => onPolicySheetOpen(row.id)}
    />
  );

  /** 自定义应用的删除键。内置预设不画它 —— 那一条删不掉，画一颗点不动的键只会更糟。 */
  const removeButton = (row: AppRowModel): ReactElement | null => {
    if (!row.isCustom) return null;
    const confirming = removeConfirmingId === row.id;
    return (
      <button
        type="button"
        /* `.confirming` 是与 `lib/confirm-twice.ts` 的跨文件契约，不是样式（见 `SheetAction` 头注）。 */
        className={confirming ? 'mr-btn danger confirming' : 'mr-btn danger'}
        onClick={() => onRemoveCustom(row.id)}
      >
        {confirming ? t('common.confirmAgain') : t('common.delete')}
      </button>
    );
  };

  const iconOf = (row: AppRowModel): ReactElement => (
    <span className="rc-app-icon" aria-hidden="true">{brandIcon(row.id) ?? row.emoji}</span>
  );

  const matchDetails = (row: AppRowModel): ReactElement => {
    const copy = matchCopy(row, t);
    return (
      <details className="rc-app-match" onToggle={revealOnToggle}>
        <summary><span>{copy.short}</span><span className="rc-app-match-link">{matchingLabel}</span></summary>
        <p>{copy.detail}</p>
        <p>{platformNote}</p>
      </details>
    );
  };

  return (
    <>
      {/* ① 五数摘要（桌面 `.app-summary`，随标题同行；移动端屏头容不下五个数，落在分段首块）。 */}
      <div className="mr-sum rc-app-summary">
        {summary.total} {t('rules.appCountUnit')} · {t('appPolicy.followGlobal')} {summary.follow} ·{' '}
        {t('appPolicy.summary.node')} {summary.node} · {t('appPolicy.summary.direct')}{' '}
        {summary.direct} ·{' '}
        {/* 「阻断」在**动作标签轴**上恒 --err 且常驻（`styles/style-invariants.test.ts` 的两轴不变量）。
            桌面汇总行同色；让同一个词在两端读成两个意思，是这条不变量存在的理由。 */}
        <span className="mr-block-n">
          {t('appPolicy.summary.block')} {summary.block}
        </span>
      </div>

      {/*
        总开关的含义。桌面把它挂在开关的 `data-tip` 上（`AppPolicyScreen.tsx:369`），
        而开关本体在屏头动作里 —— 屏头容不下一行说明，故说明落在这里（§4.12：
        要的是「常驻可达」，不是「贴在原来那个位置」）。
      */}
      <div className="mr-note rc-app-master-note">{t('appPolicy.masterTip')}</div>
      <InlineError text={masterError} />

      <div className="mr-note rc-app-platform">
        <MobileInfo title={t('mobileRules.matchSummary')} summary={t('mobileHelp.appRoutingMatch')} details={platformNote} />
      </div>

      {/* ③ 预设加载失败 —— 降级为只剩自定义应用并明说，而不是白墙。 */}
      {presetsFailed && <NoticeBanner tone="warn" text={t('appPolicy.presetsLoadFailed')} />}

      {/* ④ 非智能模式警告。 */}
      {!isSmartMode && (
        <NoticeBanner
          tone="warn"
          text={t('appPolicy.modeWarn')}
          action={{ label: t('appPolicy.backToSmart'), onClick: onBackToSmart }}
        />
      )}

      {/* ⑤ 总开关关闭提示：编辑照常保存，启用后生效（桌面 `.ap-off-note` 逐字同义）。 */}
      {!masterEnabled && <NoticeBanner tone="off" text={t('appPolicy.offNote')} />}

      {/*
        ⑥–⑧ 是分段正文。总开关关闭或非智能模式下**整块置灰**（IA §2.2 States）——
        提示条只在视野里待一屏，置灰是往下滚多远都还在的那个信号。
        只降透明度，控件仍可编辑（桌面 `.ap-disabled` 同一条口径）。
      */}
      <div className={bodyDimmed ? 'mr-ap-body dim' : 'mr-ap-body'}>
        {/* ⑥ 分类筛选 + 搜索 + 视图切换（桌面同一条 flex 行的三件）。

               分类筛选是 `select-sheet`（IA §1.2 那一行明写 “Category select-sheet”，
               桌面 `AppPolicyScreen.tsx:466` 的 `Csel`），**不是** `.mr-seg`：
               后者是 `segmented-tabs`（桌面 `.sub-tabs`），§3.3 第 1 条不许把这两套并成一套。

               🔴 视图切换 2026-09-13（批 10）补上。上一版不移植，理由是「compact 下卡片墙就是
               一行一张，即一个更高的列表，两种排布收敛到同一个东西」—— 那句话只在 compact 成立：
               `Cards` 在 medium / expanded 下是**双列**（IA §2.2「Breakpoints」），
               与单列的 `Flow` 行不是同一个东西。平板与折叠屏展开态正是这两档。 */}
        <div className="rc-app-controls">
          <SelectSheetTrigger
            label={t('appPolicy.categoryFilter')}
            options={categoryOptions}
            value={category}
            open={categorySheetOpen}
            onOpen={onCategorySheetOpen}
          />
          <SearchField
            value={search}
            onChange={onSearchChange}
            placeholder={t('appPolicy.search')}
            clearLabel={t('common.close')}
          />
          <Choice
            label={t('appPolicy.viewAria')}
            active={view}
            onSelect={onViewChange}
            items={[
              { id: 'cards' as const, label: t('appPolicy.view.cards') },
              { id: 'list' as const, label: t('appPolicy.view.list') },
            ]}
          />
        </div>

        {/* ⑦ 分组应用行 / 卡。**无序集合** ⇒ 两种排布都走多列容器；有序列表那条例外只针对规则列表。 */}
        {rowCount === 0 ? (
          <EmptyState text={presetsPending ? t('common.loading') : t('appPolicy.empty')} />
        ) : (
          groups.map((group) => (
            <div key={group.key}>
              <div className="mr-group-h">
                <span>{group.label}</span>
                <span className="mr-cnt">{group.rows.length}</span>
              </div>
              {view === 'cards' ? (
                <div className="mr-cards rc-app-grid" data-flow="multi">
                  {group.rows.map((row) => (
                    <section key={row.id} className="mr-card mr-app-card rc-app-card">
                      <div className="rc-app-card-head">
                        {iconOf(row)}
                        <div className="rc-app-identity">
                          <div className="mr-row-title">{row.name}</div>
                          <div className="mr-row-sub">{t(`appPolicy.cat.${row.category}`)}</div>
                        </div>
                        <div className="rc-app-policy">{policyTrigger(row)}</div>
                      </div>
                      {matchDetails(row)}
                      {removeButton(row)}
                      <InlineError text={errorOf(`appRule:${row.id}`)} />
                    </section>
                  ))}
                </div>
              ) : (
                <Flow>
                  {group.rows.map((row) => (
                    <div key={row.id} className="mr-row rc-app-list-row">
                      {iconOf(row)}
                      <div className="mr-row-main">
                        <div className="rc-app-list-head">
                          <div className="mr-row-title">{row.name}</div>
                          <div className="rc-app-policy">{policyTrigger(row)}</div>
                        </div>
                        {row.processes != null && <div className="mr-row-sub">{row.processes}</div>}
                        {matchDetails(row)}
                        {removeButton(row)}
                        {/* 写失败贴在动手的那一行下面。移动端没有 toast 宿主 ⇒ 不贴就是「点了没反应」。 */}
                        <InlineError text={errorOf(`appRule:${row.id}`)} />
                      </div>
                    </div>
                  ))}
                </Flow>
              )}
            </div>
          ))
        )}

        {/* ⑧ 添加自定义应用。开表单宿主的 `app-add` 那一层（`forms/AppAddPanel`）。 */}
        <button type="button" className="mr-btn" onClick={onAddCustom}>
          {t('appPolicy.addCustom')}
        </button>
      </div>

      {/*
        分类面板**渲染在 `.mr-ap-body` 之外**，与本屏 `RulesScreen` 里那个 `dnsViewsSheet`
        同一层。塞回正文里会被 `.dim` 的 `opacity: .5` 连遮罩一起画成半透明，并且因为
        `opacity < 1` 就地建了层叠上下文、而 `.mr-ap-body` 自身 `z-index: auto`，
        整组会排在 `position: sticky; z-index: 2` 的屏头之前 —— 屏头压在遮罩上面。
        而置灰态正是「控件仍可编辑」的那个态，也就是这个筛选器本该能用的时候。
      */}
      <SelectSheetPanel
        label={t('appPolicy.categoryFilter')}
        options={categoryOptions}
        value={category}
        open={categorySheetOpen}
        onClose={onCategorySheetClose}
        onSelect={(next) => {
          onCategoryChange(next);
          onCategorySheetClose();
        }}
        closeLabel={t('common.close')}
      />

      {/*
        策略面板。与分类面板同一层、同一个理由。

        结构与桌面 `PolicySelector` 同形：一组不可折叠的策略 + 若干组按订阅折叠的节点。
        顶部那句说明讲的是**识别方式**（这台设备按什么认应用），不是「有几档点不动」——
        四档现在都点得动。
      */}
      <SelectSheetPanel
        label={policyRow?.name ?? t('appPolicy.policy')}
        note={<MobileInfo title={t('mobileRules.matchSummary')} summary={t('mobileHelp.appRoutingMatch')} details={platformNote} />}
        groups={policyGroups}
        value={policyRow?.policyValue ?? 'follow'}
        open={policyRow != null}
        openGroupIds={policyOpenGroups}
        onClose={onPolicySheetClose}
        onSelect={(next) => {
          if (policyRow == null) return;
          handlePick(policyRow.id, next);
        }}
        closeLabel={t('common.close')}
      />
    </>
  );
}

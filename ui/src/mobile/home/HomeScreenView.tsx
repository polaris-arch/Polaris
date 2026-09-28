import { MobileSpeedTestProgress } from '../MobileSpeedTestProgress';
import { HomeNodePickerList } from './HomeNodePickerList';
import { RuleHitButton } from './RuleHitButton';
/** Pure mobile home presentation. State, network operations and write semantics stay in MobileHomeScreen. */
import { useLayoutEffect, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { useDismissableLayer } from '../back-stack';
import { MobileInfo } from '../MobileInfo';
import { SheetHeading } from '../SheetHeading';
import { useSheetFocus } from '../use-sheet-focus';
import { TS_EXIT_SUMMARY_KEY } from '../ts-exit-help';
import type { RuleAction } from '@/contracts/types';
import { ruleSubjectForValue, ruleTypeNameKey } from '@/domain/rules';
import type {
  AppendTargetVM,
  HomeScreenViewProps,
  WriteControlId,
} from './view-model';
import { HOME_CARD_ORDER } from './view-model';
import { fmtBytes, fmtRate, latLevel } from '@/components/screens/shared/format';
import { splinePath, scaleMax } from './traffic-buffer';

/* ═══════════ 小原语（全部 code-rendered；本屏不引任何位图）═══════════ */

function WarningGlyph(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      <path
        d="M12 3.6 22 20.4H2L12 3.6Zm0 5.4v5m0 2.6v.1"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * 品牌标记。桌面那枚 `brand.polaris-star` 是一个挂在 `AppShell` 顶层的一次性 `<defs>` sprite
 * （`components/brand-icons/PolarisStarSprite.tsx`，`.tsx` + 需要外壳挂载点）——
 * 移动端两个前提都不成立（契约 A1 禁 `.tsx`，外壳已定稿不许加挂载点）。
 * 故这里画一枚同族的四芒星，**用桌面 sprite 的同一组品牌色**（那些 hex 是品牌资产，不是主题 token）。
 * 差异登记在设计文档；要真正共用得先把 sprite 拆成不依赖挂载点的形态，那是一次桌面侧改动。
 */
function BrandMark(): ReactElement {
  return <img src="./polaris-logo.svg" width="28" height="28" className="h-brand" alt="" aria-hidden="true" />;
}

/*
 * 动作行的三枚字形与连接态的对勾。
 *
 * **path data 逐字取自原型的 `<symbol id="i-…">`**，与当前桌面客户端同出一份图
 * （`icon-registry.json` 把 action.connect-toggle / action.speed-test / action.switch-node /
 * state.connected 分别指到 `HomeScreen.tsx#connect-btn`、`#network-check-radar`、
 * `Icons.tsx#NavNodesIcon`、对勾；逐条比对过，与原型 symbol 完全一致）。
 * 注册表 policy 是 `reuse-current-client-only` / `allowRedesign:false` ⇒ 不许重画、不许换第三方。
 *
 * 为什么**内联**而不是 import 桌面那几个组件：本屏的既有口径就是「全部 code-rendered，
 * 一个位图都不引」（见 `WarningGlyph` / `BrandMark`），而那三枚桌面字形分别散在
 * `HomeScreen.tsx` 的行内 JSX 里、拿不到具名导出。描边宽 1.8 / `fill:none` / `currentColor`
 * 是注册表 policy 的三个默认值。
 */
function ActionGlyph({ shape }: { shape: 'power' | 'radar' | 'nodes' }): ReactElement {
  return (
    <svg
      className="h-acticon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      aria-hidden="true"
    >
      {shape === 'power' && (
        <>
          <path d="M12 3v9" strokeLinecap="round" />
          <path d="M6.5 6.8a7 7 0 108.9 0" strokeLinecap="round" />
        </>
      )}
      {shape === 'radar' && (
        <>
          <circle cx="12" cy="12" r="7.5" />
          <circle cx="12" cy="12" r="2.5" />
          <path d="M12 2v2.5M12 19.5V22M2 12h2.5M19.5 12H22" />
        </>
      )}
      {shape === 'nodes' && (
        <>
          <rect x="3" y="4" width="18" height="7" rx="1.5" />
          <rect x="3" y="13" width="18" height="7" rx="1.5" />
          <path d="M7 7.5h.01M7 16.5h.01" />
        </>
      )}
    </svg>
  );
}

/** 搜索框的清空叉（`home.clear`）。与本屏其余字形同源：code-rendered、描边 1.8、currentColor。 */
function ClearGlyph(): ReactElement {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      aria-hidden="true"
    >
      <path d="M6 6l12 12M18 6L6 18" strokeLinecap="round" />
    </svg>
  );
}

/**
 * 忙态：**顶替字形**，标签原样保留（`quick-actions.md`「States」）。
 * 画成一个 div 而不是 svg —— 转的是边框，`prefers-reduced-motion` 下降级成一个静止的缺口环
 * （见 home.css `.h-actspin`），而不是整个消失。
 */
function ActionBusy(): ReactElement {
  return <span className="h-actspin" aria-hidden="true" />;
}

/** 行内写失败。**贴着那颗控件**，与它同处一张卡（门对拍这件事）。 */
function WriteError({
  id,
  errors,
}: {
  id: WriteControlId;
  errors: HomeScreenViewProps['writeErrors'];
}): ReactElement | null {
  const text = errors[id];
  if (text === undefined) return null;
  return (
    <p className="h-werr" role="status" data-write-error={id}>
      {text}
    </p>
  );
}

/** 卡壳。`data-home-card` 是内容序判据的取材点，八张卡各一次，不多不少。 */
/**
 * 弹层背幕。
 *
 * 不是装饰：弹层是 `position: fixed` 的浮层，没有背幕时「点空白处关掉」这个动作会**穿透**到
 * 底下那张卡的控件上 —— 首页底下恰好排着连接开关，误触的代价是真起核/真断开。
 * 背幕同时兑现「点外面关掉」这个移动端默认预期。
 */
/* **导出**是为了让返回键那道门能把这一层单独渲染开：本屏三处弹层的开合是容器的 props，
   在门里凑齐整份 `HomeScreenViewProps` 只为验一条登记，代价远大于多一个导出。 */
export function Scrim({ onClose, label }: { onClose: () => void; label: string }): ReactElement {
  /* 系统返回键关掉的就是这一层。登记落在**背幕组件自己**身上（本屏三处弹层共用它），
     不落在三个调用点上：以后再开第四处弹层复用同一个背幕就自动带上。
     背幕在场 ⇔ 弹层在场 ⇒ 这里恒 `true`，卸载即退订（见 `back-stack.ts`）。 */
  useDismissableLayer(true, onClose);
  return <button type="button" className="h-scrim" aria-label={label} onClick={onClose} />;
}

function HomePickerSheet({ title, closeLabel, onClose, children }: {
  title: string; closeLabel: string; onClose: () => void; children: ReactNode;
}): ReactElement {
  const panel = useRef<HTMLDivElement>(null);
  useSheetFocus(panel, true, onClose, { initialFocus: '.h-pickrow.cur:not(:disabled)' });
  return <div ref={panel} className="h-sheet" role="dialog" aria-modal="true" aria-label={title} tabIndex={-1}>
    <SheetHeading title={title} onClose={onClose} closeLabel={closeLabel} className="h-sheethead" titleClassName="h-sheettitle" />
    {children}
  </div>;
}

export function RuleHitsSheet({ ruleHits, windowConnections, t, onClose, onOpenRuleHit }: Pick<HomeScreenViewProps,
  'ruleHits' | 'windowConnections' | 't' | 'onOpenRuleHit'> & { readonly onClose: () => void }): ReactElement {
  const panel = useRef<HTMLDivElement>(null);
  useSheetFocus(panel, true, onClose);
  return <div ref={panel} className="h-sheet h-rulehits-sheet" role="dialog" aria-modal="true"
    aria-label={t('mobileHome.ruleHitsAll')} tabIndex={-1}>
    <SheetHeading title={t('mobileHome.ruleHitsAll')} onClose={onClose} closeLabel={t('common.close')}
      className="h-sheethead" titleClassName="h-sheettitle" />
    <p className="h-rulehits-scope">{t('connections.active')}</p>
    <div className="h-rulehits-body">
      {ruleHits.length === 0 ? <p className="h-empty" role="status">{t('connections.noActive')}</p>
        : <div className="h-rulehits-grid">{ruleHits.map(hit => <RuleHitButton key={hit.key} hit={hit}
          connections={windowConnections} t={t} onOpen={onOpenRuleHit} />)}</div>}
    </div>
  </div>;
}

function Card({
  id,
  title,
  meta,
  children,
  scope,
}: {
  id: (typeof HOME_CARD_ORDER)[number];
  title?: string;
  /* `ReactNode` 而不是 `string`：`unlock-detection.md:21` 要求「上次检测」时间戳与标题**同一行**，
     而它在本屏是一颗可点的重测按钮，不是一段文字。 */
  meta?: ReactNode;
  children: ReactNode;
  scope?: string;
}): ReactElement {
  return (
    <section className="h-card" data-home-card={id} data-exit-scope={scope}>
      {title !== undefined && (
        <div className="h-cardhead">
          <h2 className="h-cardtitle">{title}</h2>
          {meta !== undefined && meta !== null && <span className="h-cardmeta">{meta}</span>}
        </div>
      )}
      {children}
    </section>
  );
}

/**
 * 「给这个主机加一条规则」面板。
 *
 * # 它替代的是桌面的什么
 *
 * 桌面把这条入口挂在首页那张 Sankey 拓扑图的图元命中区上：右键出一张 `.ctx-menu`，里面四颗 ——
 * `RuleSubjectMenuItems` 的「新建规则」/「加入已有规则」，加上 `ConnectionTopology` 自己的
 * 「代理」/「直连」两颗快速规则。移动端首页**没有那张图**（三列 Sankey 在 360px 上不可读，
 * 且首页八张卡是 IA 定死的），但图上承载的这条能力挂在了承载同一批数据的主机 Top 行上。
 * 换的是呈现形态，不是能力 —— 四颗动作一颗不少。
 *
 * # 为什么是「换视图」而不是「再叠一层弹层」
 *
 * 「新建」与「合并」各自要一整屏的输入。再叠一层就有两个背幕，系统返回键要按两次才回得到卡上，
 * 而 `Scrim` 的返回栈登记是**按背幕**算的（见它的头注）。故三个视图共用同一层弹层。
 *
 * # 判据一条都不在这里
 *
 * 对象推导（`ruleSubjectForValue`）、覆盖分析（`analyzeRuleCoverage`）、追加目标的展开 / 排序 /
 * 过滤 / 变换（`ruleAppendTargets` / `sortAppendTargets` / `matchAppendTargets` /
 * `appendSubjectToRule`）全部是桌面已有的纯函数，接线层调、本组件只画。两端各写一份同义实现
 * 会各自漂移，且漂了不会红。
 */
function RuleSubjectPanel(props: HomeScreenViewProps): ReactElement | null {
  const { t, ruleSubject: vm } = props;
  if (vm === null) return null;
  const value = vm.subject.value;
  const typeName = t(ruleTypeNameKey(vm.subject.type));
  /* 二级视图的头键是「返回上一层」而不是「关掉」。文案借规则屏那条通用词
     （`mobileRules.back` = 「返回」）—— 本仓已有两份同义的 `*.back`，不再造第三份。 */
  const secondary = props.ruleSubjectView !== 'menu';

  return (
    <>
      <Scrim onClose={props.onCloseRuleSubject} label={t('common.close')} />
      <div className="h-sheet" role="dialog" aria-label={value}>
        <div className="h-sheethead">
          <span className="h-sheettitle">
            {props.ruleSubjectView === 'new'
              ? t('rules.newTitle')
              : props.ruleSubjectView === 'pick'
                ? t('rules.pickTitle')
                : value}
          </span>
          <button
            type="button"
            className="h-sheetclose"
            onClick={
              secondary ? () => props.onRuleSubjectView('menu') : props.onCloseRuleSubject
            }
          >
            {secondary ? t('mobileRules.back') : t('common.close')}
          </button>
        </div>

        {/* 已被一条**启用中**的规则覆盖：如实说出来，但**不禁用任何一颗动作** ——
            这份判断是客户端启发式，权威匹配在内核（`rule-append.ts` 逐字写着这一条）。 */}
        {vm.coveringName !== null && (
          <p className="h-sheetnote">
            {t('rules.subjectAlreadyInRule', { value })}
            <span className="h-pill">{vm.coveringName}</span>
          </p>
        )}

        {props.ruleSubjectView === 'menu' && (
          <ul className="h-optlist">
            {/* 快速两颗：零输入直接落一条只含该对象的规则，备注由接线层按同一张
                `RULE_ACTION_KEY` 拼（与桌面 `home.ruleRemarks` 逐字同形）。 */}
            <li>
              <button
                type="button"
                className="h-opt"
                data-write-control="rule-add"
                onClick={() => props.onQuickRule('proxy')}
              >
                {t('home.ruleProxy')}
              </button>
            </li>
            <li>
              <button
                type="button"
                className="h-opt"
                data-write-control="rule-add"
                onClick={() => props.onQuickRule('direct')}
              >
                {t('home.ruleDirect')}
              </button>
            </li>
            <li>
              <button
                type="button"
                className="h-opt"
                onClick={() => props.onRuleSubjectView('new')}
              >
                {t('rules.addNew')}
              </button>
            </li>
            <li>
              <button
                type="button"
                className="h-opt"
                onClick={() => props.onRuleSubjectView('pick')}
              >
                {t('rules.addExisting')}
              </button>
            </li>
          </ul>
        )}

        {props.ruleSubjectView === 'new' && (
          <>
            <p className="h-sheetnote">{`${typeName}: ${value}`}</p>
            {/* 动作三档。`radiogroup` 而不是三颗独立按钮：读屏要能说出「三选一，当前第几个」。 */}
            <ul className="h-optlist" role="radiogroup" aria-label={t('rules.name')}>
              {RULE_ACTION_ORDER.map((action) => (
                <li key={action}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={props.newRuleAction === action}
                    /* `blk` = 动作标签轴恒 `--err` 且**常驻**（`style-invariants.test.ts` ⑧ 的两轴
                       规矩）：点下去会断掉这个目的地的流量，危险度不该等到选中或悬停才显出来。 */
                    className={`h-opt${props.newRuleAction === action ? ' cur' : ''}${
                      action === 'block' ? ' blk' : ''
                    }`}
                    onClick={() => props.onNewRuleAction(action)}
                  >
                    {t(RULE_ACTION_KEY[action])}
                  </button>
                </li>
              ))}
            </ul>
            {/* 备注留空 = 落盘那句默认备注（占位符显示的就是它本身，两处同源）。
                备注在这条腿上是必填：规则列表标题无备注时回落成裸类型名，同类型的快速规则
                会完全无法区分，而顺序直接决定命中优先级。 */}
            <input
              className="h-search"
              type="text"
              value={props.newRuleRemarks}
              placeholder={props.ruleRemarksHint}
              aria-label={t('rules.name')}
              onChange={(e) => props.onNewRuleRemarks(e.currentTarget.value)}
            />
            <button
              type="button"
              className="h-sheetsubmit"
              data-write-control="rule-add"
              onClick={props.onCreateRule}
            >
              {t('common.confirm')}
            </button>
          </>
        )}

        {props.ruleSubjectView === 'pick' && (
          <>
            <p className="h-sheetnote">{t('rules.pickHint', { value, type: typeName })}</p>
            <div className="h-searchrow">
              <input
                className="h-search"
                type="search"
                value={props.appendQuery}
                placeholder={t('rules.pickSearchPh')}
                aria-label={t('rules.pickSearchPh')}
                onChange={(e) => props.onAppendQuery(e.currentTarget.value)}
              />
              {/* 清空键。桌面那颗挂在拓扑图的搜索框上（`home.clear`）；移动端首页里**真正需要
                  在一个大集合里定位**的搜索框就是这一处（规则可以有几十条，主机 Top 只有 5 行），
                  故这颗零件跟着它走。触屏上逐字删一串搜索词的代价远高于鼠标，少了它就是缺陷。 */}
              {props.appendQuery !== '' && (
                <button
                  type="button"
                  className="h-searchclear"
                  aria-label={t('home.clear')}
                  onClick={() => props.onAppendQuery('')}
                >
                  <ClearGlyph />
                </button>
              )}
            </div>
            {props.appendTargets.length === 0 ? (
              <p className="h-empty">
                {props.appendQuery === '' ? t('rules.pickEmpty') : t('rules.pickNoMatch')}
              </p>
            ) : (
              <ul className="h-optlist">
                {props.appendTargets.map((row) => (
                  <AppendRow
                    key={`${row.target.ruleId}#${row.target.condIndex}`}
                    row={row}
                    subjectValue={value}
                    subjectTypeName={typeName}
                    t={t}
                    onPick={props.onAppendToRule}
                  />
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </>
  );
}

/**
 * 「合并进已有规则」列表里的一行。
 *
 * 置灰的三档（已包含 / AND 规则 / 值不适用）**各自带出路**，不用一句笼统的「不可追加」——
 * 逐字沿用桌面 `RulePickDialog` 的 `whyText`。「前面可能先命中」只是提示，**不禁用**：
 * 判据是客户端启发式，用户完全可以就是想把值加进后面那条。
 */
function AppendRow({
  row,
  subjectValue,
  subjectTypeName,
  t,
  onPick,
}: {
  row: AppendTargetVM;
  subjectValue: string;
  subjectTypeName: string;
  t: HomeScreenViewProps['t'];
  onPick: HomeScreenViewProps['onAppendToRule'];
}): ReactElement {
  const { target } = row;
  const identity = target.remarks || t(ruleTypeNameKey(target.ruleType));
  const why =
    target.block === 'andMode'
      ? t('rules.pickWhyAnd')
      : target.block === 'valueUnfit'
        ? t('rules.pickWhyUnfit', { value: subjectValue, type: subjectTypeName })
        : target.block === 'contains'
          ? t('rules.subjectAlreadyInRule', { value: subjectValue })
          : target.condIndex < 0
            ? t('rules.pickNewCond', { type: t(ruleTypeNameKey(target.type)) })
            : target.values.length > 0
              ? `${t(ruleTypeNameKey(target.type))}: ${target.values.join(', ')}`
              : t(ruleTypeNameKey(target.type));
  return (
    <li>
      <button
        type="button"
        className="h-opt col"
        data-write-control="rule-add"
        disabled={target.block !== null}
        onClick={() => onPick(target)}
      >
        <span className="h-optname">
          {identity}
          {!target.enabled && <span className="h-pill">{t('rules.pickDisabledTag')}</span>}
          {row.shadowed && <span className="h-pill warn">{t('rules.pickShadowTag')}</span>}
        </span>
        <span className="h-optwhy">{why}</span>
      </button>
    </li>
  );
}

/* ═══════════ 文案表（键 → 值都是既有 locale 键，不新造同义键）═══════════ */

/**
 * 规则动作 → 标签键。**接线层也引它**（拼默认备注，与桌面 `ConnectionTopology#addSubjectRule`
 * 的 `home.ruleRemarks` 逐字同形）—— 两处各写一张表会让「界面上写着代理、备注里写着别的」
 * 这种对不上的形态无声出现。
 *
 * 代理 / 直连两档取桌面快速腿用的那两条键（`home.*`），阻断取规则列表的目标标签
 * （`rules.targetBlock`）—— 快速腿本来就没有阻断这一档，不新造同义键。
 *
 * 🔴 **放在本文件而不是 `view-model.ts`**：`styles/style-invariants.test.ts` ⑧ 的花名册按
 * 「哪些文件引用了『阻断』文案键」取材，它数的是**渲染点**（每一处都要按两轴定一档颜色）。
 * 把这张表放进契约文件，会让一个不渲染任何东西的 `.ts` 出现在那张花名册上。
 */
export const RULE_ACTION_KEY: Readonly<Record<RuleAction, string>> = {
  proxy: 'home.ruleProxy',
  direct: 'home.ruleDirect',
  block: 'rules.targetBlock',
};

/** 「新建规则」视图里三档动作的呈现序（代理最常用，阻断最危险，故排末位）。 */
export const RULE_ACTION_ORDER: readonly RuleAction[] = ['proxy', 'direct', 'block'];

const ROUTING_KEY = {
  smart: 'home.routingSmart',
  global: 'home.routingGlobal',
  direct: 'home.routingDirect',
} as const;

const CORE_KEY = {
  stopped: 'home.statusDisconnected',
  starting: 'home.statusStarting',
  running: 'home.statusConnected',
  stopping: 'home.statusStopping',
  error: 'home.statusProxyUnavailable',
} as const;

const TS_EXIT_KEY: Record<string, string> = {
  'needs-auth': 'home.tsExitNeedsAuthWarn',
  'no-exit-device': 'home.tsExitNoDeviceWarn',
  'exit-device-offline': 'home.tsExitDeviceOfflineWarn',
  'exit-device-not-advertised': 'home.tsExitNotAdvertisedWarn',
};

/**
 * 有序类别色序（`traffic-composition.md`「Colors/tokens」）：
 * `interactive.primary` → `status.connected` → `text.tertiary` → `status.warning`，之后降亮度重复。
 * `host-traffic-top` 复用同一序列，故一个 host 与它的 outbound 不会为语义打架。
 * **同一 key 的取色必须跨刷新稳定**：这里按排名取，而排名由字节数+key 字典序决定（`aggregate.ts`），
 * 故不会在两次刷新之间跳色。
 */
const CAT_SLOTS = 4;
const catClass = (i: number): string => `c${i % CAT_SLOTS} ${i >= CAT_SLOTS ? 'dim' : ''}`.trim();

/* ═══════════ 屏 ═══════════ */

export function HomeScreenView(props: HomeScreenViewProps): ReactElement {
  const { t, writeErrors: err } = props;
  const ruleTrack = useRef<HTMLDivElement>(null);
  const [allRuleHitsOpen, setAllRuleHitsOpen] = useState(false);
  const ruleHitIdentity = props.ruleHits.map(hit => hit.key).join('\u0000');
  // The second row's height depends on the longest localized rule name and font scale.
  // Measure those rendered rows; a fixed CSS cap clips the second one on foldables.
  useLayoutEffect(() => {
    const track = ruleTrack.current;
    if (!track) return;
    const cards = [...track.querySelectorAll<HTMLElement>(':scope > .h-ringcell')];
    const measure = () => {
      if (getComputedStyle(track).gridAutoFlow === 'column') {
        track.style.removeProperty('max-height');
        return;
      }
      if (cards.length === 0) return;
      const firstTop = cards[0].offsetTop;
      const secondTop = cards.map(card => card.offsetTop).find(top => top > firstTop + 1);
      const visibleBottom = Math.max(...cards
        .filter(card => secondTop === undefined || card.offsetTop <= secondTop + 1)
        .map(card => card.offsetTop + card.offsetHeight));
      const style = getComputedStyle(track);
      const height = visibleBottom - firstTop + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
      track.style.maxHeight = `${Math.ceil(height)}px`;
    };
    const observer = new ResizeObserver(measure);
    cards.forEach(card => observer.observe(card));
    window.addEventListener('resize', measure);
    measure();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, [ruleHitIdentity]);
  const connected = props.connState === 'connected';
  const coreBusy = props.core === 'starting' || props.core === 'stopping';
  const willDisconnect = connected || props.core === 'running';
  const currentLatencyLevel = props.node.kind !== 'node' || props.node.latencyStale ||
    props.latencyCheck?.blocked || props.latencyCheck?.busyKind === 'current'
    ? 'none' : latLevel(props.node.latencyMs);
  /* 详情弹层看的是**当下**那条结果，不是打开那一刻的快照：检测在飞时结果整体被替换，
     存快照会让弹层停在旧结论上。找不到（服务已下线）就当没开，不画一个空壳。 */
  const detail = props.unlock.find((s) => s.id === props.unlockDetailId) ?? null;

  return (
    <div className="h-screen">
      {/* ── 固定区：应用页头（`app-header.md`）───────────────────────────────
          spec 把它列在滚动内容**之外**。移动外壳（F0）没有页头槽位，而外壳已定稿不属本线射程 ⇒
          这里用屏内 sticky 兑现同一语义：它不随内容滚走，且外壳零改动。登记在设计文档。 */}
      <header className="h-header">
        <div className="h-brandline">
          <BrandMark />
          <span className="h-wordmark">Polaris</span>
        </div>
        {/* 状态用**颜色 + 文字**，绝不只用颜色（`app-header.md`「States」）。 */}
        <div className="h-header-actions">
          <span className={`h-runchip s-${props.core}`}>
            <i className="h-dot" aria-hidden="true" />
            {t(CORE_KEY[props.core])}
          </span>
          <button
            type="button"
            className={`h-connect-action ${willDisconnect ? 'is-disconnect' : 'is-connect'}`}
            data-write-control="connect"
            onClick={props.onToggleConnect}
            disabled={coreBusy}
            aria-busy={coreBusy}
          >
            {coreBusy ? <ActionBusy /> : <ActionGlyph shape="power" />}
            <span>{willDisconnect ? t('mobileHome.disconnect') : t('mobileHome.connect')}</span>
          </button>
        </div>
        <WriteError id="connect" errors={err} />
      </header>

      {/* 相关能力共用外框；内部各卡仍持有原来的动作、错误和弹层。 */}
      <div className="h-flow">
        <div className="h-connection-panel">
        {/* ① 节点状态卡（动作行已并入本卡）。整卡 = 出口 scope。 */}
        <div className="h-control-panel">
          <Card id="node-status-card" scope="mobile-home">
          <div className="h-nodetop">
            {/* 前导区（旗 → 名 → 协议/延迟）本身就是节点选择入口（spec「Anatomy」第 8 条）。 */}
            <button
              type="button"
              className="h-nodelead"
              data-write-control="switch-node"
              onClick={(event) => { event.currentTarget.focus({ preventScroll: true }); props.onOpenPicker(); }}
            >
              {props.node.kind === 'node' && props.node.flagSrc !== null && (
                <img className="h-flag" src={props.node.flagSrc} alt="" aria-hidden="true" />
              )}
              <span className="h-nodecopy">
                {/* 「阻断」是**动作标签轴**（`style-invariants` ⑧ 的花名册），恒 `--err` 且常驻 ——
                    与桌面 `#cur-node` 的 `.act-block-txt` 同一档。它说的是「当前出口把流量丢了」，
                    不是一段流量的构成；后者是流量表达轴（`--warn`），在本屏由规则环承担。 */}
                {/* `copy` = 这一格装的是静态 i18n 整句而不是节点名，几何规矩因此不同
                    （见 home.css `.h-nodename.copy`：整句换行，节点名才省略号）。 */}
                <span
                  className={`h-nodename${props.node.kind === 'node' ? '' : ' copy'}${
                    props.node.kind === 'block' ? ' blk' : ''
                  }`}
                >
                  {props.node.kind === 'node'
                    ? props.node.name
                    : props.node.kind === 'direct'
                      ? t('home.routingDirect')
                      : props.node.kind === 'block'
                        ? t('home.routingBlock')
                        : t('home.plsConfigServer')}
                </span>
                <span className="h-nodemeta">
                  {props.node.kind === 'node' ? (
                    <>
                      <span className="h-pill">{props.node.protocolLabel}</span>
                    </>
                  ) : (
                    /* 直连 / 阻断 / 未选：**不渲染协议与旗**，换成如实的路由语义
                       （哨兵选择没有 ServerConfig，硬渲染只会得到一个空名字）。 */
                    <span className="h-pill muted">
                      {props.node.kind === 'direct'
                        ? t('home.directExitAddr')
                        : props.node.kind === 'block'
                          ? t('home.blockExitAddr')
                          : t('mobileHome.noNodeSelected')}
                    </span>
                  )}
                </span>
              </span>
            </button>
          </div>

          {props.latencyCheck && (
            <div className="h-latency-row" aria-live="polite">
              <span className="h-latency-summary">
                <span className="h-latency-value">
                  {t('mobileHome.latencyLabel')} · <span className={`h-latency-grade ${currentLatencyLevel}`}>{props.latencyCheck.busyKind === 'current'
                    ? t('mobileHome.latencyChecking')
                    : props.latencyCheck.blocked || props.node.kind !== 'node'
                      ? props.latencyCheck.blockedStatus
                      : props.node.latencyMs === undefined
                        ? t('home.unlockStatus.idle')
                        : props.node.latencyMs === null
                          ? t('nodes.timeout')
                          : props.node.latencyStale
                            ? t('nodes.mobileLatencyStale')
                            : `${props.node.latencyMs} ms`}</span>
                </span>
                {props.latencyCheck.blocked && (
                  <span className="h-latency-reason">{props.latencyCheck.blocked}</span>
                )}
              </span>
              <span className="h-latency-actions">
                {props.node.kind === 'node' && (
                  <button type="button" className="h-latency-action" onClick={props.latencyCheck.onRunCurrent}
                    disabled={props.latencyCheck.blocked !== null || (props.latencyCheck.busy || props.latencyCheck.busyKind !== null)}
                    aria-busy={props.latencyCheck.busyKind === 'current'}>
                    {t('mobileHome.currentSpeedTest')}
                  </button>
                )}
                <button type="button" className="h-latency-action" onClick={props.latencyCheck.onRunAll}
                  disabled={props.latencyCheck.allUnavailable !== null || (props.latencyCheck.busy || props.latencyCheck.busyKind !== null)}
                  aria-busy={props.latencyCheck.busyKind === 'all'}>
                  {props.latencyCheck.busyKind === 'all' ? t('mobileHome.allSpeedTestBusy') : t('mobileHome.allSpeedTest')}
                </button>
              </span>
            </div>
          )}
          <MobileSpeedTestProgress />
          {props.latencyCheck?.error && <p className="h-latency-error" role="alert">{props.latencyCheck.error}</p>}
          {props.latencyCheck?.allUnavailable && <p className="h-latency-note">{props.latencyCheck.allUnavailable}</p>}
          {props.latencyCheck?.feedback && <p className={`h-latency-note ${props.latencyCheck.feedback.tone}`} role="status">{props.latencyCheck.feedback.text}</p>}

          {/*
            空态两条直达入口（桌面 `HomeScreen.tsx` 的 `home.addServer` / `home.addSubscription`）。
            桌面那两颗落到桌面的造节点弹窗，整条链不进移动端；这里落到移动端**自己的**表单宿主
            （`mobile/forms/`，批 2 接的那一层）—— 同一条能力，不同的宿主。

            判据（`noServers`）与桌面 `emptyState` 逐字同源：一个节点都没有**且**没选哨兵出口。
            选了直连 / 阻断的人不缺节点，给他两颗「去加节点」是噪音。
          */}
          {props.noServers && (
            <div className="h-emptyacts">
              <button type="button" className="h-emptyact" onClick={props.onAddServer}>
                {t('home.addServer')}
              </button>
              <button type="button" className="h-emptyact" onClick={props.onAddSubscription}>
                {t('home.addSubscription')}
              </button>
            </div>
          )}

          <hr className="h-hair" />

          {/* 节点动作留在出口卡；连接动作位于常驻页头。 */}
          <div className="h-acts">
            <button
              type="button"
              className="h-act"
              data-write-control="switch-node"
              onClick={(event) => { event.currentTarget.focus({ preventScroll: true }); props.onOpenPicker(); }}
            >
              <ActionGlyph shape="nodes" />
              <span className="h-actlabel">{t('home.switchNodeTip')}</span>
            </button>
          </div>
          <WriteError id="switch-node" errors={err} />

          {/* 出口安全警示（IA 裁定 #3）：紧贴上面那颗能写 exit_node 的控件。
              TS 当出口而未配 exit_node 时，公网会**静默走直连且没有任何提示** —— 这是 safety 类。 */}
          {props.tsExitWarning !== 'none' && (
            <div className="h-notice warn" role="status" data-ts-exit-warning={props.tsExitWarning}>
              <span className="h-noticeicon">
                <WarningGlyph />
              </span>
              <p className="h-noticetext">
                <MobileInfo title={t('mobileHelp.tsExitTitle')} tone="warn"
                  summary={<>{t(TS_EXIT_SUMMARY_KEY[props.tsExitWarning])}
                    <button type="button" className="h-noticeact" onClick={props.onTsExitAction}>
                      {props.tsExitWarning === 'needs-auth' ? t('home.tsExitGoAuth') : t('home.tsExitPickDevice')}
                    </button>
                  </>}
                  details={<p>{t(TS_EXIT_KEY[props.tsExitWarning] ?? 'home.tsExitNoDeviceWarn')}</p>} />
              </p>
            </div>
          )}

          {/* 节点选择器。**本屏唯一能写 exit_node 的地方**，故与警示同处一个 scope。 */}
          {props.pickerOpen && (
            <Scrim onClose={props.onClosePicker} label={t('common.close')} />
          )}
          {props.pickerOpen && (
            <HomePickerSheet title={t('home.nodesMenu')} closeLabel={t('common.close')} onClose={props.onClosePicker}>
              <div className="h-picker-search">
                <input
                  className="h-search"
                  type="search"
                  value={props.pickerQuery}
                  placeholder={t('home.searchNodesPlaceholder')}
                  onChange={(e) => props.onPickerQuery(e.currentTarget.value)}
                />
              </div>
              {/*
                哨兵出口（直连 / 阻断）—— 桌面出口下拉顶上那两行（`NodeMenu.tsx` 的 `.mi`）。

                它们**不是节点**：没有 `ServerConfig`、没有协议、没有延迟，故不画 `h-pickmeta` 那一串。
                选中态与节点行共用 `.cur`（桌面那边是两枚同路径不同 class 的对勾 `MiCheck` / `NmCheck`，
                移动端两组都用整行的选中样式回答同一个问题）。

                **不参与搜索过滤**：桌面的搜索框只过滤节点，这两行恒在（搜不到节点时仍然要能退回直连）。
              */}
              <ul className="h-picklist">
                <li>
                  <button
                    type="button"
                    className={`h-pickrow${props.node.kind === 'direct' ? ' cur' : ''}`}
                    aria-current={props.node.kind === 'direct' ? 'true' : undefined}
                    data-exit-write="home-sentinel-direct"
                    onClick={() => props.onPickSentinel('direct')}
                  >
                    <span className="h-pickname">{t('home.routingDirect')}</span>
                  </button>
                </li>
                <li>
                  {/* 直连模式下「阻断」是一次静默 no-op（`route.final` 恒 = direct，没有流量经过
                      proxy-selector）⇒ **在场置灰 + 把原因写在旁边**，不留一颗按下去什么也不发生的选项。 */}
                  <button
                    type="button"
                    className={`h-pickrow${props.node.kind === 'block' ? ' cur' : ''}`}
                    aria-current={props.node.kind === 'block' ? 'true' : undefined}
                    data-exit-write="home-sentinel-block"
                    onClick={() => props.onPickSentinel('block')}
                    disabled={props.blockDisabledReason !== null}
                  >
                    <span className="h-pickname blk">{t('home.routingBlock')}</span>
                    {props.blockDisabledReason !== null && (
                      <span className="h-pickwhy">{props.blockDisabledReason}</span>
                    )}
                  </button>
                </li>
              </ul>
              <HomeNodePickerList rows={props.pickRows} subscriptions={props.pickerSubscriptions}
                query={props.pickerQuery} t={t} onUseAsExit={props.onUseAsExit} />
            </HomePickerSheet>
          )}
        </Card>
          <Card id="mode-chips" title={t('home.routingStrategy')}>
          <div className="h-routing" role="group" aria-label={t('home.routingStrategy')}>
            {(['smart', 'global', 'direct'] as const).map((v) => (
              <button
                type="button"
                key={v}
                className={`h-route-option${props.routing === v ? ' sel' : ''}`}
                data-write-control="routing"
                aria-pressed={props.routing === v}
                onClick={() => props.onSetRouting(v)}
              >
                {t(ROUTING_KEY[v])}
              </button>
            ))}
          </div>
          {props.reverseRouting && (
            <div className="h-revrow">
              <span className="h-pill warn">{t('home.reverseRoutingBadge')}</span>
              <MobileInfo title={t('mobileHelp.reverseRoutingTitle')} tone="warn"
                summary={t('mobileHelp.reverseRouting')}
                details={<p>{t('home.reverseRoutingTip')}</p>} />
            </div>
          )}
          <WriteError id="routing" errors={err} />
        </Card>
        </div>
        <Card
          id="unlock-detection"
          title={t('mobileHome.networkCheck')}
          meta={
            <span className="h-check-meta">
              {props.unlockCheckedLabel !== null && <span className="h-check-time">{props.unlockCheckedLabel}</span>}
              <button
                type="button"
                className="h-check-btn"
                data-write-control="network-check"
                onClick={props.onNetworkCheck}
                disabled={props.unlockRunning || props.exitProbing}
                aria-busy={props.unlockRunning || props.exitProbing}
              >
                {/* 陈旧结果保留圆点，但时间戳必须**如实变老**，不许悄悄重渲染成刚测过。 */}
                {props.unlockRunning || props.exitProbing
                  ? t('home.unlockStatus.checking')
                  : t('mobileHome.checkNow')}
              </button>
            </span>
          }
        >
          <div className="h-exit-info" aria-live="polite">
            <span className="h-exit-label">{t('mobileHome.exitIp')}</span>
            <span className="h-exit-address">{props.exitIp ?? t('home.unlockStatus.idle')}</span>
            {props.exitFlagSrc !== null && <img className="h-exit-flag" src={props.exitFlagSrc} alt="" aria-hidden="true" />}
            <span>{props.exitRegion.kind === 'flag' ? props.exitRegion.code.toUpperCase()
              : props.exitRegion.kind === 'text' ? props.exitRegion.region : ''}</span>
            {props.exitIsDirect && <span className="h-exit-direct">{t('home.routingDirect')}</span>}
          </div>
          <WriteError id="network-check" errors={err} />
          <div className="h-unlockgrid">
            {props.unlock.map((s) => (
              /*
                条目是**按钮**（`unlock-detection.md`「Interaction」：点服务开详情）。
                这一层不是装饰：`blocked`（换国家）与 `restricted`（换同国干净出口）带的是两个
                不同的用户动作，而卡面上它们的差别只有一颗 8px 的点的颜色 —— 一条纯颜色通道。
              */
              <button
                type="button"
                className="h-unlock"
                key={s.id}
                aria-label={`${s.name}: ${t(`home.unlockStatus.${s.result.status}`)}`}
                aria-busy={s.result.status === 'checking'}
                onClick={() => props.onOpenUnlockDetail(s.id)}
              >
                <span className={`h-badge st-${s.result.status}`}>
                  {s.badgeSrc !== null && <img src={s.badgeSrc} alt="" aria-hidden="true" />}
                  {/* 状态点：七态各一档。`blocked`（换国家）与 `restricted`（换同国干净出口）
                      **必须视觉可分** —— 那是检测引擎产出的唯一可行动差异。 */}
                  <i className={`h-stdot st-${s.result.status}`} aria-hidden="true" />
                </span>
                <span className="h-unlockname">{s.name}</span>
              </button>
            ))}
          </div>
        </Card>
          {/*
            服务详情。复用本屏既有的 `Scrim + h-sheet`，放在卡的断点容器之外，避免旧 WebView
            将 fixed 弹层锚在滚动卡内。外壳的不滚动屏边界承接定位。
            内容三项，全部取自既有键 —— 移动端不新造同义译文（与桌面 `UnlockBadge` 同一条拼法）：
             · 地区码：`UnlockResult.region`，缺省时**整段不拼**，不留「· undefined」尾巴；
               检测在飞时也不拼（那是上一轮的结论，配在「检测中」旁边会被读成本轮已测出）。
             · 上次探测时间：卡级 `unlockCheckedLabel` —— 七个服务是**同一次** `detect_all` 批量
               探测出来的（`crates/unlock`），故卡级时间就是每个服务的上次探测时间，不是近似。
             · 失败原因：状态本身。契约 `UnlockResult` 只有 `status | region` 两个字段，
               没有第三个「原因」字段；七态各自的含义就是这里能说的全部。
          */}
          {detail !== null && (
            <>
              <Scrim onClose={props.onCloseUnlockDetail} label={t('common.close')} />
              <div className="h-sheet" role="dialog" aria-label={detail.name}>
                <div className="h-sheethead">
                  <span className="h-sheettitle">{detail.name}</span>
                  <button
                    type="button"
                    className="h-sheetclose"
                    onClick={props.onCloseUnlockDetail}
                  >
                    {t('common.close')}
                  </button>
                </div>
                {/* 这里**不重复那颗彩色点**：详情层存在的理由就是颜色通道说不清，
                    再画一遍颜色等于把同一条通道说第二遍。 */}
                <p className="h-detailrow">
                  {t(`home.unlockStatus.${detail.result.status}`)}
                  {props.unlockRunning || detail.result.region === undefined
                    ? ''
                    : ` · ${detail.result.region.toUpperCase()}`}
                </p>
                <p className="h-detailmeta">
                  {props.unlockRunning
                    ? t('home.unlockStatus.checking')
                    : (props.unlockCheckedLabel ?? t('home.unlockStatus.idle'))}
                </p>
              </div>
            </>
          )}
        </div>
        <Card id="rule-hits" title={t('mobileHome.ruleHits')} meta={<span className="h-rulehits-meta">
          <span>{t('connections.active')}</span>
          {props.ruleHits.length > 0 && <button type="button" className="h-rulehits-all" aria-haspopup="dialog"
            aria-expanded={allRuleHitsOpen} onClick={event => { event.currentTarget.focus(); setAllRuleHitsOpen(true); }}>
            {t('mobileHome.ruleHitsViewAll', { count: props.ruleHits.length })}
          </button>}
        </span>}>
          {props.ruleHits.length === 0 ? (
            <p className="h-empty">{t('connections.noActive')}</p>
          ) : (
            <div className="h-rings" ref={ruleTrack}>
              {props.ruleHits.map(hit => <RuleHitButton key={hit.key} hit={hit} connections={props.windowConnections}
                t={t} onOpen={props.onOpenRuleHit} />)}
            </div>
          )}
        </Card>
        {allRuleHitsOpen && <>
          <Scrim onClose={() => setAllRuleHitsOpen(false)} label={t('common.close')} />
          <RuleHitsSheet {...props} onClose={() => setAllRuleHitsOpen(false)} onOpenRuleHit={props.onOpenRuleHit
            ? hit => { setAllRuleHitsOpen(false); props.onOpenRuleHit?.(hit); } : undefined} />
        </>}
        <div className="h-statistics-panel">
          <div className="h-statistics-left">
        <Card id="traffic-chart">
          <TrafficChart samples={props.samples} t={t} />
        </Card>
        <Card
          id="traffic-composition"
          title={t('connections.topOutboundsTitle')}
          meta={`${t('connections.active')} · ${fmtBytes(props.windowBytes)}`}
        >
          {props.composition.length === 0 ? (
            <p className="h-empty">{t('connections.noActive')}</p>
          ) : (
            <>
              <div className="h-stack">
                {props.composition.map((s, i) => (
                  <span
                    key={s.key}
                    className={`h-seg ${catClass(i)}`}
                    style={{
                      width:
                        props.windowBytes > 0
                          ? `${((s.bytes / props.windowBytes) * 100).toFixed(2)}%`
                          : '0%',
                    }}
                  />
                ))}
              </div>
              <ul className="h-legend">
                {props.composition.map((s, i) => (
                  <li key={s.key}>
                    <span className={`h-swatch ${catClass(i)}`} aria-hidden="true" />
                    <span className="h-legendname">{s.key}</span>
                    <span className="h-legendval">{fmtBytes(s.bytes)}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </Card>
          </div>
        <Card
          id="host-traffic-top"
          title={t('connections.topHostsTitle')}
          meta={`${t('mobileHome.listedHostsTotal')} · ${fmtBytes(
            props.hosts.reduce((acc, h) => acc + h.bytes, 0),
          )}`}
        >
          {/*
            主机筛选 + 清空键。**这一颗才是桌面 `ConnectionTopology.tsx|k:home.clear` 那笔账**
            （桌面把搜索框挂在 Sankey 图的卡头上，移动端没有那张图，这一维落在承载同一批数据的
            这张卡里 —— 首页八张卡由 IA §2.1 定死，不许为它新开一张）。

            ⚠️ 本屏另有一颗同样叫 `home.clear` 的清空键（规则选择器的搜索框，`RuleSubjectPanel`
            里那一处）。两颗只是**恰好同名**，服务的是两个对象。故这一颗带
            `data-host-filter`：登记表按 id 认人，两颗都用 `aria-label` 认就分不开，
            将来谁把规则选择器的搜索去掉，这笔账会静默变成僵尸。

            筛选面是**整个活动连接窗口**而不是已经画出来的那 5 行（判据 `filterByHostQuery`，
            与后端 `project_connections_topology_iter` 逐字同源）—— 只在 5 行里筛等于没筛。

            搜索框**恒在**（不随命中数隐藏）：筛没命中时若把框一起收掉，用户就再也够不着那颗
            清空键，只剩重启应用一条路。
          */}
          <div className="h-searchrow">
            <input
              className="h-search"
              type="search"
              data-host-filter="query"
              value={props.hostQuery}
              placeholder={t('home.searchTopology')}
              aria-label={t('home.searchTopology')}
              onChange={(e) => props.onHostQuery(e.currentTarget.value)}
            />
            {props.hostQuery !== '' && (
              <button
                type="button"
                className="h-searchclear"
                data-host-filter="clear"
                aria-label={t('home.clear')}
                onClick={() => props.onHostQuery('')}
              >
                <ClearGlyph />
              </button>
            )}
          </div>
          {props.hosts.length === 0 ? (
            /* 空态两句必须分得开：窗口里本来就没有连接 vs 筛了但一条没命中。
               同一句话会让用户以为代理断了。 */
            <p className="h-empty">
              {props.hostQuery === '' ? t('connections.noActive') : t('home.searchTopologyNoMatch')}
            </p>
          ) : (
            <ul className="h-hosts">
              {props.hosts.map((h, i) => {
                /*
                  这一行能不能开「加一条规则」的面板，两个条件都必须成立：

                   · **判得出观测对象** —— `hostNameOf` 的第三级回落是 `rule` 名（一条规则的名字，
                     不是一个目的地），带空格的规则备注判不出对象 ⇒ 不画入口。判据是
                     `ruleSubjectForValue`，与桌面拓扑图**同一个函数**（单标签的规则名会被判成
                     域名、于是可点 —— 两端同此行为，不在这一屏另开一条更严的判据）；
                   · **不在隐私模式** —— 遮蔽的只是显示，对象本身仍是真域名。留着入口等于给了一条
                     「点开就能看见被遮蔽的那个名字」的路，遮蔽当场作废。
                */
                const ruleable = !props.hostsMasked && ruleSubjectForValue(h.key) !== null;
                const body = (
                  <>
                    <span className="h-hostline">
                      {/* 隐私模式**遮蔽域名而不是藏起整张卡**：流量形状仍然可见，只是不点名目的地。 */}
                      <span className="h-hostname">
                        {props.hostsMasked ? t('connections.privacyHidden') : h.key}
                      </span>
                      <span className="h-hostval">{fmtBytes(h.bytes)}</span>
                    </span>
                    <span className="h-hostbar">
                      <i
                        className={catClass(i)}
                        style={{
                          width:
                            props.hosts[0].bytes > 0
                              ? `${((h.bytes / props.hosts[0].bytes) * 100).toFixed(2)}%`
                              : '0%',
                        }}
                      />
                    </span>
                  </>
                );
                return (
                  <li key={h.key}>
                    {ruleable ? (
                      <button
                        type="button"
                        className="h-hostrow act"
                        aria-label={`${h.key} · ${t('rules.addNew')}`}
                        onClick={() => props.onOpenRuleSubject(h.key)}
                      >
                        {body}
                      </button>
                    ) : (
                      <span className="h-hostrow">{body}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          {/* 行内写失败回显。**在弹层之外** —— 规则写成功失败都会关掉弹层（它是一层覆盖，
              错误留在下面等于看不见），故这条必须留在卡本体上，与 `switch-node` 同形。 */}
          <WriteError id="rule-add" errors={err} />
          <RuleSubjectPanel {...props} />
        </Card>
        </div>
      </div>
    </div>
  );
}

/* ═══════════ 流量图 ═══════════ */

/**
 * 由代码从数据绘制，**不存在任何静态图片**。
 *
 * viewBox 固定、纵横各自铺满：spec 禁的是「拉伸把描边一起拉粗」，那一格由
 * `vector-effect="non-scaling-stroke"` 兜住（描边在任何断点都是 1:1）。
 * 真正的几何 1:1 需要运行期量容器宽度（`ResizeObserver`），而那会在本屏引入一条 JS 侧的尺寸依赖 ——
 * 本屏刻意一条都不留（见文件头注②）。登记在设计文档。
 */
function TrafficChart({
  samples,
  t,
}: {
  samples: HomeScreenViewProps['samples'];
  t: HomeScreenViewProps['t'];
}): ReactElement {
  const max = scaleMax(samples);
  const down = splinePath(
    samples.map((s) => s.down),
    300,
    100,
    max,
  );
  const up = splinePath(
    samples.map((s) => s.up),
    300,
    100,
    max,
  );
  const last = samples[samples.length - 1];
  return (
    <>
      <div className="h-rates">
        <span className="h-rate dn">
          <i aria-hidden="true">↓</i>
          {/* 没有样本时**不编一个 0**：那是「还没测到」，不是「测到了 0」。 */}
          {last === undefined ? '—' : fmtRate(last.down)}
        </span>
        <span className="h-rate up">
          <i aria-hidden="true">↑</i>
          {last === undefined ? '—' : fmtRate(last.up)}
        </span>
      </div>
      <div className={`h-plotwrap${down === '' ? ' empty' : ''}`}>
        {down === '' ? (
          <p className="h-empty">{t('mobileHome.noSamples')}</p>
        ) : (
          <svg
            className="h-plot"
            viewBox="0 0 300 100"
            preserveAspectRatio="none"
            role="img"
            aria-label={t('mobileHome.trafficChart')}
          >
            {/* 四条网格线：0 / 33 / 66 / 100 %。 */}
            {[0, 33, 66, 100].map((p) => (
              <line
                key={p}
                className="h-grid"
                x1="0"
                x2="300"
                y1={p}
                y2={p}
                vectorEffect="non-scaling-stroke"
              />
            ))}
            <path className="h-series dn" d={down} vectorEffect="non-scaling-stroke" />
            <path className="h-series up" d={up} vectorEffect="non-scaling-stroke" />
          </svg>
        )}
        {/* Y 轴**只标两个数**：刻度上限与 0，中间那两条不标。 */}
        {down !== '' && <>
          <span className="h-ymax">{fmtRate(max)}</span>
          <span className="h-ymin">0</span>
        </>}
      </div>
      {/* X 轴五个刻度，60 秒窗口。 */}
      {down !== '' && <div className="h-xaxis">
        {['-60s', '-45s', '-30s', '-15s', t('mobileHome.axisNow')].map((label) => (
          <span key={label}>{label}</span>
        ))}
      </div>}
    </>
  );
}

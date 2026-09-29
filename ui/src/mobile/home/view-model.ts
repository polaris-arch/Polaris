import type { SubscriptionConfig } from '@/contracts/types';
/**
 * 首页视图模型 —— **呈现层与接线层之间的唯一契约**。
 *
 * 与节点屏同一条分界（`mobile/nodes/view-model.ts` 头注）：呈现层必须是纯的，才能在 node 环境下用
 * `react-dom/server` 真渲染。本屏的门要断言的三条不变量（三档内容一致 / DOM 序不许重排 /
 * 写失败必有可见回显）里，前两条只有在**真渲染出的 DOM** 上断言才有意义。
 *
 * 屏级呈现以移动端能力保全重设计为准；本契约仍保持纯视图边界。
 */
import type { ProxyMode, RuleAction, ServerConfig } from '@/contracts/types';
import type { UnlockResult, ServiceId } from '@/contracts/unlock-detection';
import type { TsExitWarning } from '@/domain/tailscale-exit-warning';
import type { ExitRegionDisplay } from '@/domain/exit-flag';
import type { RuleSubject } from '@/domain/rules';
import type { RuleAppendTarget } from '@/components/dialogs/rule-append';
import type { TakeoverConnState } from '@/components/screens/home/connection-state';
import type { ByteSlice, RuleHit } from './aggregate';
import type { RateSample } from './traffic-buffer';

export type {
  TsExitWarning,
  TakeoverConnState,
  ExitRegionDisplay,
  ByteSlice,
  RuleHit,
  RateSample,
  RuleSubject,
  RuleAppendTarget,
};

/**
 * **八张卡的 DOM 序，就是重要性序，且它是权威的**（`home-screen.md`「Composition」）。
 *
 * compact 逐字渲染成一列；medium / expanded 走多列列流，由排版引擎配平列高，**DOM 序不动**。
 * 竖屏手机从上往下读，那个读序就是排名 —— 任何断点重排都会让同一份产品在两种设备上给出两种排名。
 *
 * 这个数组是判据的正面等式那一半：门渲染本屏、按序取出全部 `data-home-card`，与它逐项相等。
 */
export const HOME_CARD_ORDER = [
  'node-status-card',
  'mode-chips',
  'unlock-detection',
  'rule-hits',
  'traffic-chart',
  'traffic-composition',
  'host-traffic-top',
] as const;

export type HomeCardId = (typeof HOME_CARD_ORDER)[number];

/**
 * **会写的控件登记表**（IA 裁定 #14）。
 *
 * 每一颗都要满足两件事，门逐条对拍：
 *  ① 接线层里它的写调用在 `runWrite(` 的参数区间内（唯一写出口）；
 *  ② 呈现层里它带 `data-write-control="<id>"`，且**同一张卡内**有一处 `data-write-error="<id>"`
 *     的槽位 —— 行内错误贴着动手的那颗控件，而不是飘到屏顶。
 *
 * 移动端**没有** toast / 弹窗宿主（`lib/error-handler` 的门面只在桌面 `App.tsx` 注入实现），
 * 且本批**不许**往外壳里造全局宿主。所以「写失败」在这一屏的唯一可见形态就是这条行内错误。
 */
export const WRITE_CONTROLS = [
  'connect',
  'network-check',
  'switch-node',
  'routing',
  /**
   * 主机 Top 卡上的**规则写**（快速代理 / 快速直连 / 新建 / 合并进已有，共四颗）。
   *
   * 四颗共用**一个** id，而 `network-check` / `unlock-recheck` 必须分两个 —— 差别是**卡**不是控件数：
   * 同卡的控件共用一个 id 时，那条行内错误仍然贴在用户刚动过的地方（四颗全在同一张弹层里，
   * 弹层本身在 `host-traffic-top` 这张卡内）。跨卡才必须分 id。
   */
  'rule-add',
] as const;

export type WriteControlId = (typeof WRITE_CONTROLS)[number];

/** 控件 id → 该控件当前的写失败提示。缺席 = 上一次写成功（或还没写过）。 */
export type WriteErrors = Readonly<Partial<Record<WriteControlId, string>>>;

/** 核心生命周期。与 `app-header.md`「States」同集合。 */
export type CoreLifecycle = 'stopped' | 'starting' | 'running' | 'stopping' | 'error';

/**
 * 节点身份的四态。
 *
 * **哨兵选择不是节点**（`data-contract.json#sources.config-and-nodes.traps`）：直连 / 阻断
 * 根本没有 `ServerConfig`，拿 `currentServer?.name` 渲染会得到一个空名字而不是一句真话。
 * 故这里在类型层就把它们分开，呈现层无从「顺手」渲染成空。
 */
export type NodeIdentity =
  | {
      readonly kind: 'node';
      readonly name: string;
      readonly protocolLabel: string;
      /** 有验证过的国家码才有旗；**绝不合成一个回退国家**。 */
      readonly flagSrc: string | null;
      /** `undefined`=本会话没测过；`null`=超时；数字=毫秒。 */
      readonly latencyMs: number | null | undefined;
      /** 超过 `LATENCY_STALE_MS`（30 分钟）。陈旧必须**渲染成陈旧**，不许印成一个数。 */
      readonly latencyStale: boolean;
    }
  | { readonly kind: 'direct' }
  | { readonly kind: 'block' }
  | { readonly kind: 'none' };

/** 节点选择器里的一行。 */
export interface NodePickRow {
  readonly server: ServerConfig;
  readonly isCurrent: boolean;
  /** 只在暂存里、盘上还没有（选它会连带一次整核重启）。与节点屏同一条口径，必须标出来。 */
  readonly stagedOnly: boolean;
  readonly latencyMs: number | null | undefined;
  readonly latencyStale: boolean;
  readonly protocolLabel: string;
}

/**
 * 「给这个观测对象加一条规则」面板的三个视图。
 *
 * 桌面把四颗动作平铺在一张右键菜单里（`RuleSubjectMenuItems` 两颗 + 拓扑图自己两颗），
 * 靠鼠标悬停出二级弹窗。移动端没有悬停，且弹层是**贴底**的一层 ——
 * 四颗一起平铺还塞得下，但「新建」与「合并」各自要一整屏的输入，只能是换视图而不是再叠一层
 * （再叠一层就有两个背幕，系统返回键要按两次才回得到卡上）。
 */
export type RuleSubjectView = 'menu' | 'new' | 'pick';

/** 面板当前挂着的那个观测对象。`null` = 面板没开。 */
export interface RuleSubjectVM {
  readonly subject: RuleSubject;
  /**
   * 已经有一条**启用中**的规则覆盖了它 ⇒ 那条规则的显示名；`null` = 没有。
   *
   * 判据是 `analyzeRuleCoverage`（桌面同一个函数）。它只影响提示与排序，**不禁用任何一颗动作** ——
   * 权威匹配在内核，客户端这份是启发式（那句话是 `rule-append.ts` 逐字写的）。
   */
  readonly coveringName: string | null;
}

/** 「合并进已有规则」列表里的一行。 */
export interface AppendTargetVM {
  readonly target: RuleAppendTarget;
  /** 上面还有一条更靠前的启用规则会先命中它 ⇒ 追加到这条也不会生效，如实标出来。 */
  readonly shadowed: boolean;
}

/** 一个服务的解锁结果 + 它的品牌元数据。 */
export interface UnlockEntryVM {
  readonly id: ServiceId;
  readonly name: string;
  readonly badgeSrc: string | null;
  readonly result: UnlockResult;
}

/** Shared current/all node latency controls. Results are measured by the real speed-test API. */
export interface LatencyCheckVM {
  readonly busyKind: 'current' | 'all' | null;
  readonly busy?: boolean;
  readonly error: string | null;
  readonly feedback: { tone: 'ok' | 'info'; text: string } | null;
  readonly blocked: string | null;
  readonly blockedStatus: string;
  readonly allUnavailable: string | null;
  readonly onRunCurrent: () => void;
  readonly onRunAll: () => void;
}

export interface HomeScreenViewProps {
  readonly t: (key: string, opts?: Record<string, unknown>) => string;

  /* ── 行内写失败回显（裁定 #14）─────────────────────────────────────────── */
  readonly writeErrors: WriteErrors;

  /* ── ① 节点状态卡 ──────────────────────────────────────────────────────── */
  readonly core: CoreLifecycle;
  readonly connState: TakeoverConnState;
  readonly node: NodeIdentity;
  readonly onToggleConnect: () => void;
  readonly onNetworkCheck: () => void;
  /** Measured node latency; independent from IP and service reachability. */
  readonly latencyCheck?: LatencyCheckVM;
  /** 出口警示（IA 裁定 #3）。本屏能切出口 ⇒ 本屏必须有它。 */
  readonly tsExitWarning: TsExitWarning;
  readonly onTsExitAction: () => void;
  /* 节点选择器（本屏唯一能写 exit_node 的地方）。 */
  readonly pickerOpen: boolean;
  readonly onOpenPicker: () => void;
  readonly onClosePicker: () => void;
  readonly pickerQuery: string;
  readonly onPickerQuery: (q: string) => void;
  readonly pickRows: readonly NodePickRow[];
  readonly pickerSubscriptions?: readonly SubscriptionConfig[];
  readonly onUseAsExit: (server: ServerConfig) => void;
  /**
   * **哨兵出口**（直连 / 阻断）。它们不是节点：`selectedServerId` 取 `__direct__` / `__block__`
   * 时全局出口分别走 direct 出站与 block 出站（判据在 `domain/direct-selection.ts`，两端同一份）。
   *
   * 选中态由 `node.kind` 给出（`'direct'` / `'block'`），这里不再重复一格。
   */
  readonly onPickSentinel: (kind: 'direct' | 'block') => void;
  /**
   * 非 `null` ⇒ 「阻断」这一行**在场置灰**，并把这句话写在旁边。
   *
   * 唯一的成因是直连模式：那时 `route.final` 恒 = `direct`，一个用户流量都不经过 proxy-selector，
   * 选了也是 no-op（`domain/direct-selection.ts#BLOCK_SERVER_ID` 的语义边界逐字写着这一条）。
   * 留一颗静默无效的选项比置灰更坏 —— 用户会以为自己断网了，实际什么都没发生。
   */
  readonly blockDisabledReason: string | null;
  /**
   * 一个节点都没有、且没选哨兵出口 ⇒ 空态：节点卡上给两条**直达入口**（加节点 / 加订阅）。
   *
   * 判据与桌面 `HomeScreen.tsx` 的 `emptyState` 逐字同源（`servers.length === 0 && !哨兵`）：
   * 哨兵是一个有效的出口配置，选了它的人不缺节点，给他两颗「去加节点」是噪音。
   */
  readonly noServers: boolean;
  readonly onAddServer: () => void;
  readonly onAddSubscription: () => void;

  /* ── ② 模式芯片 ────────────────────────────────────────────────────────────
   *
   * **本屏没有「接管方式」这一维**（Android 恒 TUN，见 `MobileHomeScreen` 的 `MOBILE_TAKEOVER`）。
   * 它曾经是芯片 1，连同 `modeSheet: 'takeover' | 'routing' | null` 这个二值状态机一起删掉了：
   * 剩下一维之后那个枚举只会有一种取值，留着就是一个假装还能分叉的开关。
   */
  readonly routing: ProxyMode;
  /**
   * 地区分流是否处于**反向（回国）**语义：所在地区走代理、其余直连 —— 与默认相反。
   *
   * 真机 2026-07-20 §1.4：`regionRouting.reverse` 唯一入口是规则页地区卡里的一个开关，
   * 首页/状态栏零呈现，唯一提示是切换瞬间那一条 toast ⇒ 误触之后首页仍只显示「智能 · 已连接」，
   * 用户不知道分流语义已经反过来了（叠加规则集缺失后退化成全量明文直连且零告警）。
   * 桌面为此在主页分流标签行旁补了一枚常驻指示（`components/screens/home/ReverseRoutingBadge.tsx`）；
   * 移动端今天原样复现了那个事故形态，故把同一个不变量搬过来。
   * 判据仍是 `domain/region-routing.ts` 的 `isReverseRegionRouting`，本屏不新造一条。
   */
  readonly reverseRouting: boolean;
  /** 芯片 2 = **出口国家**（读出，不是设置）。产品里根本没有「地区」这个模式维度。 */
  readonly exitRegion: ExitRegionDisplay;
  readonly exitFlagSrc: string | null;
  /** 出口腿取的是 `ipInfo.direct`（未连接时的本机出网），必须**标明是直连**。 */
  readonly exitIsDirect: boolean;
  readonly exitProbing: boolean;
  readonly exitIp: string | null;
  readonly onSetRouting: (v: ProxyMode) => void;

  /* ── ③ 网络检测 ────────────────────────────────────────────────────────── */
  readonly unlock: readonly UnlockEntryVM[];
  /** 已本地化的「上次检测」相对时间；`null` = 从未检测过。 */
  readonly unlockCheckedLabel: string | null;
  readonly unlockRunning: boolean;
  /**
   * 打开了详情的那个服务（`unlock-detection.md`「Interaction」：点服务开详情）。`null` = 没开。
   *
   * 存 id 而不是整条 `UnlockEntryVM`：结果会在检测过程中被整体替换，存快照会让详情停在旧结论上，
   * 而这张卡的「States」明写陈旧结果必须**如实变老**，不许静默呈现成刚测过。
   */
  readonly unlockDetailId: ServiceId | null;
  readonly onOpenUnlockDetail: (id: ServiceId) => void;
  readonly onCloseUnlockDetail: () => void;

  /* ── ④ 流量图 ──────────────────────────────────────────────────────────── */
  readonly samples: readonly RateSample[];

  /* ── ⑤⑥⑦ 三张分析卡（共用**活动连接**这一个窗口）────────────────────── */
  readonly composition: readonly ByteSlice[];
  readonly ruleHits: readonly RuleHit[];
  /** Navigate to active connections with the exact displayed group identity. */
  readonly onOpenRuleHit?: (group: { key: string; label: string }) => void;
  readonly hosts: readonly ByteSlice[];
  /** 隐私模式：**遮蔽域名而不是藏起整张卡**，流量形状仍可见。 */
  readonly hostsMasked: boolean;
  /* ── ⑦ 主机筛选（桌面拓扑图卡头那个搜索框的落点）──────────────────────
   *
   * 桌面把它挂在 Sankey 图的卡头上；移动端没有那张图，同一维挂在承载同一批数据的
   * 主机 Top 卡上。筛的是**整个活动连接窗口**（`filterByHostQuery`），不是已经画出来的那 5 行 ——
   * 所以它解决的确实是「在大集合里定位」：排在第 30 位的域名靠它才够得着。
   *
   * `hosts` 已经是筛过再排名的结果 ⇒ 呈现层不再做任何筛选；它只需要知道**筛没筛**
   * （`hostQuery !== ''`），用来把空态那句话从「没有活动连接」换成「无匹配结果」。
   */
  readonly hostQuery: string;
  readonly onHostQuery: (query: string) => void;

  /* ── ⑦ 主机 Top 行上的「给这个主机加一条规则」面板 ──────────────────────
   *
   * 桌面把这条入口挂在首页那张 Sankey 拓扑图的图元命中区上（`ConnectionTopology.tsx` +
   * `components/RuleSubjectMenuItems.tsx`）。移动端首页没有那张图（三列 Sankey 在 360px 上
   * 不可读），**同一条能力挂在承载同一批数据的主机 Top 行上** —— 换的是呈现形态，不是能力。
   *
   * 四颗动作与桌面逐条对应：快速代理 / 快速直连（零输入直接落一条规则）、新建规则（可选动作 +
   * 备注）、合并进已有规则（`rule-append.ts` 的纯函数全套，与桌面 `RulePickDialog` 同源）。
   */
  /** 面板挂着的对象；`null` = 没开。 */
  readonly ruleSubject: RuleSubjectVM | null;
  readonly ruleSubjectView: RuleSubjectView;
  /** 主机行被按下 —— 值不合法（既不是域名也不是 IP，例如回落到 `rule` 名）时接线层不开面板。 */
  readonly onOpenRuleSubject: (host: string) => void;
  readonly onCloseRuleSubject: () => void;
  readonly onRuleSubjectView: (view: RuleSubjectView) => void;
  /** 快速规则：零输入直接落一条只含该对象的规则。 */
  readonly onQuickRule: (action: 'proxy' | 'direct') => void;
  /* 「新建规则」视图的两格草稿。 */
  readonly newRuleAction: RuleAction;
  readonly onNewRuleAction: (action: RuleAction) => void;
  readonly newRuleRemarks: string;
  readonly onNewRuleRemarks: (remarks: string) => void;
  /** 备注留空时**真正会落盘**的那一句（已本地化）。呈现层拿它当占位符 —— 占位符与落盘值同源，
      否则输入框里灰着一句、存进去的是另一句。 */
  readonly ruleRemarksHint: string;
  readonly onCreateRule: () => void;
  /* 「合并进已有规则」视图。 */
  readonly appendQuery: string;
  readonly onAppendQuery: (query: string) => void;
  readonly appendTargets: readonly AppendTargetVM[];
  readonly onAppendToRule: (target: RuleAppendTarget) => void;
  /** 三张卡共用的合计 = 它们各自展示的部分之和（`globalRules` 第 3 条）。 */
  readonly windowBytes: number;
  readonly windowConnections: number;

  /* ── ⑧ 诊断列表 ────────────────────────────────────────────────────────── */
  /** 内核未过滤口径的活跃连接数；`null` = 还没有帧。 */
  readonly kernelConnections: number | null;
}

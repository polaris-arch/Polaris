/**
 * 节点屏视图模型 —— **呈现层与接线层之间的唯一契约**。
 *
 * 为什么把它单独拎出来：呈现层（`NodesScreenView`）必须是纯的，才能在 node 环境下用
 * `react-dom/server` 真渲染（本仓不装 jsdom / testing-library，既有先例见
 * `SubInfoBar.progress.test.tsx` 的头注）。门要断言的那条不变量 ——
 * 「每一个能写 exit_node 的控件同屏都有 TsExitWarning」——只有在**真渲染出的 DOM** 上断言才有意义；
 * 对着源码正则断言「有没有写那个组件名」是另一回事，它挡不住「渲染在别的分支里」。
 *
 * 于是接线层（`MobileNodesScreen`）负责读 store / 发 IPC，本文件的类型是它交给呈现层的全部东西。
 */
import type { ServerConfig, SubscriptionConfig, UserConfig } from '@/contracts/types';
import type { SubscriptionUpdateProgress } from '@/contracts/subscription-progress';
import { subUsage } from '@/components/screens/nodes/nodes-logic';
import { fmtBytes } from '@/components/screens/shared/format';
import { relativeTimeTextIso } from '@/lib/relative-time';
import { subscriptionErrorDetail } from '@/domain/subscription-error-text';
import {
  subAutoUpdateStatus,
  subEffectiveIntervalHours,
} from '@/domain/subscription-auto-update';
import type { TsExitWarning } from '@/domain/tailscale-exit-warning';
import type { MeshTunnelHealth } from '@/domain/mesh-tunnel-health';
import type { NodesListSortKey } from '@/components/screens/nodes/nodes-list-projection';
/* 处置码**取登记表那一份**，不在这里重新声明一遍：本文件重声明会让全仓多出一张长得像
   登记表的东西（`wiring-completeness.test.ts` 的「登记表唯一性」当场红，而它红得对 ——
   两处各写一份处置码的下一步就是两处各自漂）。 */
import type { Disposition } from './absence-register';

export type { TsExitWarning, MeshTunnelHealth, NodesListSortKey };

/** 一行节点。字段全部由接线层按桌面同一套判据算好——呈现层不得自己再判一遍。 */
export interface NodeRowVM {
  readonly server: ServerConfig;
  /** 当前出口。 */
  readonly isCurrent: boolean;
  /** 组网/端点协议节点（桌面 `.nd-cap exit` 那颗角标的判据）。 */
  readonly isExit: boolean;
  /** 仅局域网（`meshAllowsInternet === false`）。 */
  readonly lanOnly: boolean;
  /** `undefined`=本会话没测过；`null`=真实超时；数字=毫秒。与 `use-latency-store` 同口径。 */
  readonly latencyMs: number | null | undefined;
  /** 延迟已陈旧（`isLatencyStale`）。IA §2.1：陈旧必须**渲染成陈旧**，不得渲染成一个数。 */
  readonly latencyStale: boolean;
  /** 本轮能不能测（`speedTestBlockReason === null`）。 */
  readonly speedTestable: boolean;
  /** 不可测的理由（已本地化）。桌面只挂 `data-tip`；移动端必须常驻（§4.12）。 */
  readonly speedTestBlockedHint?: string;
  /** 被启动 gate 剔除的理由（已本地化）。同上，必须常驻。 */
  readonly invalidReason?: string;
  /** 只在暂存里、盘上还没有。 */
  readonly stagedOnly: boolean;
  /** 被更早节点抢占的网段（已把抢占者解析成显示名）。 */
  readonly shadowed?: readonly { readonly cidr: string; readonly by: string }[];
  /** 是否渲染删除入口（订阅节点为 false —— 删了下次对账会拉回来）。 */
  readonly deletable: boolean;
  /** 传输摘要（`reality · tcp` 之类）。IA §2.1 记它是标签不是测量，可安全展示。 */
  readonly transport: string;
  /** 协议显示名。 */
  readonly protocolLabel: string;
}

/**
 * **全部**测速入口的置灰理由 —— 单一来源（IA §4.10）。
 *
 * 核这一档排在结构可测性**之前**，且对每一个入口都成立。这不是排版偏好：`isSpeedTestable`
 * （`domain/endpoint-routes.ts`）里 `caps.mainCorePool` 只对 `protocol === 'tailscale'` 生效，
 * 于是核停着时一个普通 VLESS 节点仍然 `speedTestable === true`。屏级「全部测速」入口自己额外
 * 判了核，行动作面里那一条没判 ⇒ 同一屏上，屏级入口提前说清「请先连接」，行里那条却让你点下去
 * 换回一条红色后端错误。§4.10 的处置是「置灰并给理由」，两处必须同源同优先级。
 *
 * 抽成函数而不是在两处各写一遍：这条腿今天有两个消费点（屏级入口、行动作面），
 * 各写各的正是它此前分叉的方式；第三条腿（比如将来的长按菜单）再漏一次也不会有人发现。
 *
 * `row` 省略 ⇒ 屏级入口（全部测速 / 测可见 / 批量），只判核这一档；射程各自的空集判据由调用方补。
 *
 * # 核停这一档的文案为什么是 `home.stubProxyStopped`（2026-09-05 换的）
 *
 * 此前是 `nodes.mobileSpeedTestCoreStopped`，五语种写的都是「……请先在**首页**连接后再测」。
 * 本轮行动作面里补上了「连接」那颗（K3-RV03，§4.10 的第三件事），那句话当场变假 ——
 * 用户被指去首页，而按钮就在同一张面板上。故改指一个**已存在**的中性键：
 * 「代理未运行 / Proxy stopped」只陈述事实，不指路，与紧随其后的那颗连接按钮不冲突。
 * 不新造键是硬约束（凭空写 ru/fa 译文不接受），旧那条键随之删除（五语同批）。
 */
export function speedTestBlockedReason(
  t: (key: string) => string,
  coreRunning: boolean,
  row?: { readonly speedTestable: boolean; readonly speedTestBlockedHint?: string },
): string | undefined {
  if (!coreRunning) return t('home.stubProxyStopped');
  if (row === undefined || row.speedTestable) return undefined;
  return row.speedTestBlockedHint;
}

/** 分组段（自建 / 组网 / 每个订阅）。 */
export interface GroupTabVM {
  readonly id: string;
  readonly label: string;
  readonly count: number;
  /** 订阅更新失败的详情（已本地化）。桌面挂 `data-tip`；移动端要常驻（§4.12）。 */
  readonly failureDetail?: string;
}

/**
 * 订阅摘要（block 2）。**没有登记数据源的位置一律不出现在这里**（IA §2.1 末节）。
 *
 * 2026-09-06（W-06）：用量 / 到期 / 上次更新三处的来源已登记进
 * `mobile-kit/data-contract.json#sources.subscription-summary`，随之从「标 pending」变成**真的画**。
 * 三处**全部是可选的**，且缺席时接线层传 `undefined` 而不是零值 —— 订阅面板不下发 `userInfo`
 * 与订阅从没更新过是两种不同的事实，画一个 `0 B / 0 B` 会把前者谎报成后者。
 */
export interface SubSummaryVM {
  readonly name: string;
  readonly nodeCount: number;
  /** 自动更新徽标文案（已本地化）。 */
  readonly autoLabel: string;
  /** 自动更新被全局暂停或只在启动时检查时的可操作提示；常态由短状态说明。 */
  readonly autoHint?: string;
  /** 正在更新（含阶段文案）。为 `undefined` 表示不在更新中。 */
  readonly updatingLabel?: string;
  /** 上一次更新失败的详情（已本地化）。 */
  readonly failureDetail?: string;
  /**
   * 流量用量（`已用 / 总量`，已格式化带单位）+ 百分比 + 是否告警档。
   * `undefined` = 订阅面板没下发 `userInfo.total`（不是「用了 0」）。
   */
  readonly usage?: { readonly text: string; readonly pct: number; readonly warn: boolean };
  /** 到期日（已本地化整句）。`undefined` = 面板没给 `userInfo.expire`。 */
  readonly expiry?: string;
  /** 上次更新的相对时间（已本地化）。`undefined` = 从未更新过 / 时间戳解析不出来。 */
  readonly lastUpdated?: string;
}

/** 空态四档（IA §2.1 States 表的前四行）。 */
export type EmptyKind = 'filtered' | 'sub' | 'mesh' | 'all';

/** 行上的次级动作（进 action-sheet）。`disabledReason` 非空 ⇒ 在场但置灰 + 常驻理由。 */
export interface SheetItem {
  readonly id: string;
  readonly label: string;
  readonly danger?: boolean;
  readonly disabledReason?: string;
  /**
   * 常驻说明（§4.12 的落点之一）。与 `disabledReason` 分开：那条是「为什么不能点」，
   * 这条是「点了会发生什么」——桌面把后者也塞在 `data-tip` 里，触屏摸不到。
   */
  readonly note?: string;
  readonly onSelect?: () => void;
}

/** 节点页的全量/当前可见测速保留为不同动作，禁用原因跟着对应行走。 */
export function buildNodeMoreItems(args: {
  t: (key: string) => string;
  batchMode: boolean;
  testing: boolean;
  testAllBlocked?: string;
  testVisibleBlocked?: string;
  onTestAll: () => void;
  onTestVisible: () => void;
  onToggleBatchMode: () => void;
}): SheetItem[] {
  const { t, batchMode, testing, testAllBlocked, testVisibleBlocked } = args;
  const busy = testing ? t('mobileActions.preparing') : undefined;
  return [
    { id: 'test-all', label: t('nodes.testAll'), disabledReason: testAllBlocked ?? busy, onSelect: args.onTestAll },
    { id: 'batch', label: t(batchMode ? 'nodes.batchExit' : 'nodes.batch'), onSelect: args.onToggleBatchMode },
    {
      id: 'test-visible', label: t('nodes.testVisible'), note: t('nodes.testVisibleHint'),
      disabledReason: testVisibleBlocked ?? busy, onSelect: args.onTestVisible,
    },
  ];
}

export interface NodesScreenViewProps {
  readonly t: (key: string, vars?: Record<string, unknown>) => string;

  // ── block 1：分组段 ───────────────────────────────────────────────────────
  readonly groups: readonly GroupTabVM[];
  readonly activeTab: string;
  readonly onSelectTab: (id: string) => void;

  // ── block 2：订阅摘要 ─────────────────────────────────────────────────────
  readonly sub?: SubSummaryVM;
  readonly onRefreshSub?: () => void;
  /**
   * 订阅「更多」里的五条（重命名 / 编辑 URL / 复制 URL / 更新间隔 / 删除订阅）。
   * 桌面把它们放在一个 hover 触发的 `.mini-menu` 里；移动端改成 action-sheet。
   * 本批没接的逐条置灰 + 给理由 —— 整个菜单凭空消失，用户只会以为订阅管不了了。
   */
  readonly subItems: readonly SheetItem[];

  // ── block 3：工具栏 ───────────────────────────────────────────────────────
  readonly search: string;
  readonly onSearch: (v: string) => void;
  readonly protoFilter: string;
  readonly protoOptions: readonly string[];
  readonly onProtoFilter: (v: string) => void;
  readonly sortKey: NodesListSortKey;
  readonly onSortKey: (v: NodesListSortKey) => void;

  // ── block 4：列表 ─────────────────────────────────────────────────────────
  readonly rows: readonly NodeRowVM[];
  readonly emptyKind: EmptyKind;
  /** 整屏节点计数摘要（桌面 `.nd-count`）。 */
  readonly countSummary: string;

  // ── 安全类注脚（IA 裁定 #3） ─────────────────────────────────────────────
  readonly tsExitWarning: TsExitWarning;
  readonly onTsExitAction: () => void;
  /** 组网分组的隧道健康（IA 裁定 #3 后半）。非组网分组恒 `'none'`。 */
  readonly meshTunnelHealth: MeshTunnelHealth;

  // ── 页头动作 ──────────────────────────────────────────────────────────────
  /** 主核是否在跑（§4.10 的能力位：`!!proxyStatus?.running`）。 */
  readonly coreRunning: boolean;
  readonly testing: boolean;
  readonly copyBusy: boolean;
  readonly refreshBusy: boolean;
  /**
   * **全部**节点里结构上可测的条数（`speedTestableIds(servers, …)`）。
   * 「全部测速」的射程是全量，不是当前分组的可见集：拿可见数禁用它会在
   * 「当前 tab 恰好没有可测节点」时锁死一个本来有目标的动作。
   */
  readonly testableTotal: number;
  readonly onTestAll: () => void;
  readonly onTestVisible: () => void;
  /** 「添加」动作面：四条路径逐条带处置（能移植的可点，没移植的置灰 + 理由）。 */
  readonly addItems: readonly SheetItem[];

  // ── 行动作 ────────────────────────────────────────────────────────────────
  /** 点整行 = 设为出口（桌面整卡同语义）。已是当前出口时接线层传 `undefined`。 */
  readonly onUseAsExit: (row: NodeRowVM) => void;
  /** 行的次级动作（测速 / 复制链接 / 删除 / 未移植项）。 */
  readonly rowItems: (row: NodeRowVM) => readonly SheetItem[];

  // ── 批选（block 5：停靠条） ───────────────────────────────────────────────
  readonly batchMode: boolean;
  readonly onToggleBatchMode: () => void;
  readonly selectedIds: ReadonlySet<string>;
  readonly onToggleSelect: (row: NodeRowVM) => void;
  readonly onSelectAll: () => void;
  readonly onBatchCopyLinks: () => void;
  readonly onBatchSpeedTest: () => void;
  /**
   * 批量删除（W-03）。桌面 `.batch-bar` 那颗的移动端对应物 —— 本批之前**既没登记也没渲染**，
   * 处置见 `absence-register.ts#BATCH_DELETE`。二次确认由接线层叠一层确认面板，不在停靠条上翻红。
   */
  readonly onBatchDelete: () => void;

  // ── 行内反馈（为什么不用全局 toast，见 `MobileNodesScreen` 头注「写失败必须可见」） ──
  /**
   * 行内反馈（IA 裁定 #14 的落点）。`info` 是**中性**档（一条说明，不是「成功了」）：
   * CSS 里刻意没有 `.mn-notice.info` 规则 —— 基线样式本身就是中性那一档，
   * 多写一条同值规则只会让人以为它们不同。
   */
  readonly notice?: { readonly tone: 'ok' | 'info' | 'warn' | 'err'; readonly text: string };
}

/* ════════════════════════════════════════════════════════════════════════════
 * 三张动作表的**构造器**（2026-09-06 批 2 抽出）。
 *
 * 🔴 抽出来的理由不是「整洁」，是**判据够得着**。本仓 vitest 跑 `environment:'node'`，接线层
 *    （`MobileNodesScreen`）要 store 与 `api`，门渲染不动它 ⇒ 此前 `nodes-screen.test.tsx` ⑥
 *    只能**自造夹具**喂给呈现层：它证明的是「视图收到一条置灰项时画得出来」，而**从不读生产的
 *    那张表**。于是「生产忘了把某一条放进表里」「理由键与登记表对不上」这两类缺陷一条都抓不到。
 *
 * ⚠️ **抽函数会造出新的缝**：测了这三个纯函数 ≠ 生产在用它们。故门对着它们的每一条判据都要
 *    **成对**：一条喂真实入参跑本函数、断言输出；另一条对着 `MobileNodesScreen.tsx` 的源码断言
 *    那三个 `useMemo/useCallback` 真的在调本函数。两条缺任一条，缝就留在那里。
 * ═══════════════════════════════════════════════════════════════════════════ */

/** 「添加」四条各自的动作（由接线层给，构造器不认识表单宿主）。 */
export interface AddActionHandlers {
  readonly onManualAdd: () => void;
  readonly onMeshJoin: () => void;
  readonly onImport: () => void;
  readonly onAddSubscription: () => void;
}

/**
 * 「添加」四条路径（W-01）。**id 与 `absence-register.ts#ADD_ACTIONS` 逐条同序** ——
 * 那张表与桌面 `NodesHeader` 的菜单项数对差，本函数与那张表对差，两段接起来才是
 * 「桌面加一项 ⇒ 移动端必须显式回答」。
 */
export function buildAddItems(
  t: (key: string) => string,
  h: AddActionHandlers,
): SheetItem[] {
  return [
    { id: 'manual-add', label: t('nodes.manualAdd'), onSelect: h.onManualAdd },
    { id: 'mesh-join', label: t('nodes.meshAddAccess'), onSelect: h.onMeshJoin },
    { id: 'import', label: t('nodes.manualImport'), onSelect: h.onImport },
    { id: 'subscription', label: t('nodes.addSubscription'), onSelect: h.onAddSubscription },
  ];
}

/** 订阅「更多」五条各自的动作。 */
export interface SubMenuHandlers {
  readonly onRename: () => void;
  readonly onEditUrl: () => void;
  readonly onCopyUrl: () => void;
  readonly onInterval: () => void;
  readonly onDelete: () => void;
}

/**
 * 订阅「更多」五条（W-04）。id 与 `absence-register.ts#SUB_MENU_ACTIONS` 逐条同序。
 *
 * 「更新间隔」带一条**常驻说明**（`nodes.subIntervalHint`：间隔是全局的）——桌面把这句话挂在
 * `data-tip` 上，触屏摸不到（§4.12）。它不是置灰理由：那一项点得动，去的是设置里那唯一一处真开关。
 */
export function buildSubItems(t: (key: string) => string, h: SubMenuHandlers): SheetItem[] {
  return [
    { id: 'rename', label: t('nodes.subRename'), onSelect: h.onRename },
    { id: 'edit-url', label: t('common.edit'), onSelect: h.onEditUrl },
    { id: 'copy-url', label: t('nodes.subCopyUrl'), onSelect: h.onCopyUrl },
    {
      id: 'interval',
      label: t('nodes.subInterval'),
      note: t('nodes.subIntervalHint'),
      onSelect: h.onInterval,
    },
    { id: 'delete', label: t('nodes.subDeleteTitle'), danger: true, onSelect: h.onDelete },
  ];
}

/** 行动作各自的动作。 */
export interface RowActionHandlers {
  readonly onSpeedTest: () => void;
  readonly onConnect: () => void;
  readonly onCopyLink: () => void;
  readonly onClone: () => void;
  readonly onEdit: () => void;
  readonly onDelete: () => void;
}

/** 一条行动作的处置码（由 `absence-register.ts` 给，本函数不认识那张表的存放形式）。 */
export type RowDispositionLookup = (id: string) => Disposition | undefined;

export interface RowItemsArgs {
  readonly t: (key: string) => string;
  readonly row: NodeRowVM;
  /** 主核在跑没有（§4.10 的能力位）。 */
  readonly coreRunning: boolean;
  /* 🔴 **`editable` 2026-09-06（批 3）摘除。** 它此前表达的是「这一行是 wireguard / tailscale
     （含 WARP），那三张表还没移植」，而三张表本批落地 ⇒ `mobileEditFormFor` 不再返回 `null`，
     这一档在生产里恒为真。留着一个恒真的入参会留下一条**没有牙的**判据：门测得到那个分支，
     测不到任何真实入口，而下一个人会以为「编辑不可用」仍是一种产品状态。 */
  readonly disposition: RowDispositionLookup;
  readonly handlers: RowActionHandlers;
}

/**
 * 行动作面的那张表。
 *
 * 顺序是有意的：测速 → （核停时）连接 → 复制链接 → 克隆 / 编辑 / 删除。
 * 「连接」紧挨着「测速为什么是灰的」那句话 —— 理由与补救相邻，用户不必去别的屏找（§4.10）。
 */
export function buildRowItems(args: RowItemsArgs): SheetItem[] {
  const { t, row, coreRunning, disposition, handlers } = args;
  const items: SheetItem[] = [
    {
      id: 'speed-test',
      label: t('nodes.speedTest'),
      /* §4.10：核这一档排在结构可测性之前，且与页头那三个入口**同一个函数**算出来。 */
      disabledReason: speedTestBlockedReason(t, coreRunning, row),
      onSelect: handlers.onSpeedTest,
    },
    /* 核在跑时「连接」不出现 —— 一颗「连接」摆在已连接的屏上是噪音，而断开归首页那颗危险动作管。 */
    ...(coreRunning
      ? []
      : [{ id: 'connect', label: t('mobileHome.connect'), onSelect: handlers.onConnect }]),
    { id: 'copy-link', label: t('nodes.copyLink'), onSelect: handlers.onCopyLink },
  ];
  const run: Record<'clone' | 'edit' | 'delete', () => void> = {
    clone: handlers.onClone,
    edit: handlers.onEdit,
    delete: handlers.onDelete,
  };
  const label: Record<'clone' | 'edit' | 'delete', string> = {
    clone: t('nodes.clone'),
    edit: t('common.edit'),
    delete: t('common.delete'),
  };
  for (const id of ['clone', 'edit', 'delete'] as const) {
    /* 🔴 **订阅托管的节点没有删除入口**（2026-09-06 复审 major）。`row.deletable` 在接线层
       就算好了（`server.subscriptionId === undefined`），此前这个构造器从头到尾没读它 ——
       删除那一条只看 `ROW_ACTIONS` 的处置（今天是 ported）无条件入表，于是订阅分组里每一行
       都画得出一颗能点的「删除」，删完下次订阅刷新的 reconcile 又原样拉回来：操作没有净效果、
       只剩误删风险（陈先生 2026-07-29 裁定，逐字见桌面 `NodeCard.tsx:159-164` 的 props 注释）。
       处置与桌面 `NodeCard.tsx:476 {deletable && (…)}` 同口径 —— **入口不渲染，而非渲染后置灰**：
       这一条不是「移动端没移植」（那种要在场置灰给理由），是「这一行上它本就不该存在」。 */
    if (id === 'delete' && !row.deletable) continue;
    const d = disposition(id);
    if (d === undefined) continue;
    if (d.kind !== 'ported') {
      /* 登记成缺席 ⇒ **在场置灰 + 理由**，不是凭空消失（§3.3 第 2 条）。 */
      items.push({ id, label: label[id], danger: id === 'delete', disabledReason: t(d.reasonKey) });
      continue;
    }
    items.push({ id, label: label[id], danger: id === 'delete', onSelect: run[id] });
  }
  return items;
}

/**
 * 订阅摘要（block 2）的整块算值。抽出来的理由与上面三张动作表**同一条**：接线层要 store 与
 * `api`，门渲染不动它 ⇒ 判据此前只能自造一个 `sub` 夹具喂给呈现层，而那证明的是「视图收到值
 * 会画」，**不是**「生产真的算出了值」。实测：把接线层那一格改成恒 `undefined`（用量整格消失），
 * 旧判据全绿。
 *
 * ⚠️ 同样要**成对**：一条喂真实入参跑本函数、断言输出；另一条对着 `MobileNodesScreen.tsx`
 * 断言那个 `useMemo` 真的在调它。
 *
 * # 三处「有才画」（W-06）
 *
 * 用量 / 到期 / 上次更新的来源登记在 `mobile-kit/data-contract.json#sources.subscription-summary`。
 * 判据与桌面 `SubInfoBar.tsx:191-194` **同一批**：`subUsage`（阈值住在 `nodes-logic`，不在这里
 * 重定）· `fmtBytes`（自带单位，别再拼一个）· `relativeTimeTextIso` · `userInfo.expire`（秒级 epoch）。
 * 三处缺席一律传 `undefined` 而不是零值：「订阅面板没下发」与「用了 0 / 从没更新过」是不同的事实，
 * 画 `0 B / 0 B` 会把前者谎报成后者。
 */
export function buildSubSummary(args: {
  readonly t: (key: string, vars?: Record<string, unknown>) => string;
  readonly sub: SubscriptionConfig;
  readonly nodeCount: number;
  readonly progress: SubscriptionUpdateProgress | undefined;
  readonly config: UserConfig | undefined;
}): SubSummaryVM {
  const { t, sub, nodeCount, progress, config } = args;
  const updating = progress !== undefined && progress !== null && progress.phase !== 'failed';
  const status = subAutoUpdateStatus(sub, config);
  const hours = subEffectiveIntervalHours(config);
  const autoLabel =
    status === 'active'
      ? t('nodes.subAutoUpdateEvery', { h: hours })
      : status === 'master-off'
        ? t('nodes.subAutoUpdatePaused')
        : status === 'startup-only'
          ? t('nodes.subAutoUpdateStartupOnly')
          : t('nodes.subManualUpdate');
  /* 活跃周期与更新中已由短徽标说明；仅把会改变下一步操作的异常/特殊档常驻。
     周期如何计算在对应的更新设置行解释，116 节点列表不重复铺长文。 */
  const autoHint = updating || status === 'active'
    ? undefined
    : status === 'master-off'
      ? t('nodes.subAutoUpdateMasterOffHint')
      : status === 'startup-only'
        ? t('nodes.subAutoUpdateStartupOnlyHint')
        : undefined;

  const info = sub.userInfo;
  const usageRaw = subUsage(info);
  const expireSec = info?.expire;
  const updatedText = relativeTimeTextIso(sub.lastUpdated, t);
  return {
    name: sub.name,
    nodeCount,
    autoLabel,
    autoHint,
    updatingLabel: updating ? t('nodes.subUpdating') : undefined,
    failureDetail: progress?.phase === 'failed' ? subscriptionErrorDetail(progress, t) : undefined,
    usage:
      usageRaw.total > 0
        ? {
            text: `${fmtBytes(usageRaw.used)} / ${fmtBytes(usageRaw.total)}`,
            pct: usageRaw.pct,
            warn: usageRaw.warn,
          }
        : undefined,
    expiry:
      typeof expireSec === 'number' && expireSec > 0
        ? t('nodes.subExpiry', { date: new Date(expireSec * 1000).toISOString().slice(0, 10) })
        : undefined,
    /* `relativeTimeTextIso` 解析不出来时返回空串（那是「这条时间戳读不动」，不是「刚刚」）
       ⇒ 空串同样落成缺席，绝不回落成一个看起来对的相对时间。 */
    lastUpdated: updatedText === '' ? undefined : updatedText,
  };
}

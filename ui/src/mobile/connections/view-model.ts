/**
 * 「连接」屏的**纯投影层**（IA spec §2.3）。
 *
 * # 为什么它是单独一个文件
 *
 * 本屏三条数据腿（活动明细 / 已结束历史 / TOP 聚合）里，前两条走的是**增量协议**：
 * `reset=true` 带完整基线、`generation` 标记数据集、`sequence` 在代际内严格递增。
 * 这三件事写错的表现全都是「列表看起来有数据但不对」—— 静态门看不出来，真机也不会报错，
 * 只会在某次重连之后把两个数据集悄悄并成一份。故合并语义必须是**能单独喂夹具的纯函数**。
 *
 * 协议本体不在这里重写：`@/components/screens/connections/active-detail` 与 `./closed-history`
 * 是桌面已经在用、且各自带单测的纯模块，本文件**原样复用**它们。这里加的是它们上面那一层
 * ——「一帧增量 → 一批可渲染的行」——那一层同样会把协议搞错（reset 了却不清速率基线，
 * 就会拿旧帧的字节数去减新帧的，算出一个负速率被 clamp 成 0），所以它也要有自己的判据。
 *
 * # 一处与桌面**不同**的呈现口径（有意，且理由在数据契约里）
 *
 * `data-contract.json#connections-detail` 的 trap 原文：
 *   "upload and download are Option: a connection can legitimately have no byte counters yet.
 *    Treat absent as zero for ranking but never render '0 B' as a measured value."
 * 桌面 `ConnectionsScreen.tsx` 走的是 `entry.upload ?? 0` 然后 `fmtBytes(0)` ⇒ 屏幕上是 `0 B`，
 * 也就是把「还没有计数器」画成了「量到了 0 字节」。本屏按 trap 走：两个计数器都缺席时
 * `totalBytes` 是 `null`，渲染成占位符而不是 `0 B`；排序仍按 0 参与（trap 的前半句）。
 * 这是本批第二处与桌面可见行为不同的地方，已在设计文档「与桌面的差异」一节登记。
 */

import type {
  ClosedConnectionEntry,
  ConnectionEntry,
  ConnectionsClosedUpdate,
  ConnectionsDetailUpdate,
} from '@/contracts/types';
import { ruleGroupOf } from '../home/aggregate';
import {
  applyActiveDetailUpdate,
  stickyDisplay,
  type ActiveDetailSync,
} from '@/components/screens/connections/active-detail';
import { applyClosedHistoryUpdate } from '@/components/screens/connections/closed-history';
import type { SortState } from '@/components/screens/connections/sort-cycle';

/** 四个分段。默认 `overview`，对齐桌面 `ConnectionsScreen.tsx:249` 的 `top`。 */
export type ConnSegment = 'overview' | 'active' | 'closed' | 'logs';

/**
 * 九个可排序键（桌面表里**每个数据列**都在列）。IA §3.1 #22 明文：排序移进面板，
 * 但九个键一个不许少 —— 少一个就是**新造**一处两端不一致。
 * `rate` 只在活动、`ended` 只在已结束，与桌面的列集一致。
 */
export type ConnSortKey =
  | 'type'
  | 'host'
  | 'dest'
  | 'rule'
  | 'chain'
  | 'rate'
  | 'total'
  | 'time'
  | 'ended'
  | 'proc';

export const ACTIVE_SORT_KEYS: readonly ConnSortKey[] = [
  'type',
  'host',
  'dest',
  'rule',
  'chain',
  'rate',
  'total',
  'time',
  'proc',
];

export const CLOSED_SORT_KEYS: readonly ConnSortKey[] = [
  'type',
  'host',
  'dest',
  'rule',
  'chain',
  'total',
  'time',
  'ended',
  'proc',
];

/** 一行连接的显示态。**不含任何 i18n**：呈现层拿 `t` 自己翻，投影层只管数据。 */
export interface ConnRowVM {
  id: string;
  host: string;
  /** `IP:port` 的显示串。 */
  dest: string;
  /**
   * 目标 IP 原值（不带端口）。单独留一份是因为 `connectionRuleSubjects` 按
   * `metadata.destinationIP` 取「IP」这个规则对象 —— 喂它拼过端口的显示串，
   * `validateRuleValue('ipCidr', …)` 会判不过，行菜单里的「复制 IP」于是**静默少一条**。
   */
  destIP: string;
  rule: string;
  ruleName?: string;
  ruleGroupKey: string;
  chain: string;
  /** L4 类型（network 优先，回落 inbound type）。 */
  l4: string;
  /** L4 完整标签（桌面挂在 `data-tip` 上，这里进展开区）。 */
  l4Full: string;
  udp: boolean;
  procName: string;
  procFull: string;
  /** 隐私字段：隐私模式下由投影层置 null，呈现层拿不到就画不出来。 */
  sourceIP: string | null;
  /**
   * 累计字节。**`null` = 计数器缺席，不是 0**（数据契约 trap，见文件头注）。
   * 排序时按 0 参与，渲染时画占位符。
   */
  totalBytes: number | null;
  upRate: number;
  dnRate: number;
  /** 连接建立时刻 epoch ms；无有效时间为 NaN。 */
  startAt: number;
  /** 已结束时刻 epoch ms；活动连接为 null。 */
  endedAt: number | null;
  /** 产生本行速率的 detail 序列；不是当前序列时速率按 0 展示（桌面同款）。 */
  rateSequence: number;
  /** 该连接是不是 `direct` / `block` 这两个特殊出站（呈现层据此换文案与颜色角色）。 */
  chainKind: 'direct' | 'block' | 'named';
}

/**
 * 有界页窗口。结构与 `@/components/ListPager` 的 `pageWindow()` 返回值等价（TS 结构类型
 * ⇒ 那个函数的返回值直接可赋给它），但**不从那边 import 类型**。
 *
 * 理由是门的取材面：`nodes-screen.test.tsx` 的 A 腿边界按 **(模块, 符号)** 放行从桌面 `.tsx`
 * 里具名取的**纯函数**，而它的 import 识别只认「`import {` 紧跟花括号」那一种写法 ——
 * `import type { … }` 落不进那张表。与其为一个编译期就被擦掉、既不渲染也不带 CSS 的类型
 * 去把那张表的语义拉宽，不如在这里写下这四个字段：它本来就是本屏与呈现层之间的契约。
 */
export interface PageSlice {
  page: number;
  pageCount: number;
  start: number;
  end: number;
}

/** 每页挂载上限。与桌面同值（`ConnectionsScreen.tsx` 的 `CONNECTION_PAGE_SIZE`）。 */
export const CONNECTION_PAGE_SIZE = 20;
/** 日志每页挂载上限，同桌面 `LOG_PAGE_SIZE`。 */
export const LOG_PAGE_SIZE = 20;
/** TOP 卡的条数选项，同桌面 `TOP_N_OPTIONS`。 */
export const TOP_N_OPTIONS: readonly number[] = [5, 10, 15];

function basename(path: string): string {
  const at = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return at >= 0 ? path.slice(at + 1) : path;
}

function chainKindOf(chain: string): ConnRowVM['chainKind'] {
  if (chain === 'block') return 'block';
  if (chain === 'direct') return 'direct';
  return 'named';
}

/**
 * `ConnectionEntry` → 行的静态部分。速率与时长每秒都在变，不进这里。
 *
 * `privacy` 为真时 **不把 sourceIP 与进程路径带出来**：脱敏发生在投影层而不是渲染层，
 * 因为渲染层还有第二个消费者（展开区），两处各写一遍判断就必然有一处漏。
 */
function projectStatic(
  entry: ConnectionEntry,
  privacy: boolean,
): Omit<ConnRowVM, 'totalBytes' | 'upRate' | 'dnRate' | 'startAt' | 'endedAt' | 'rateSequence'> {
  const meta = entry.metadata ?? {};
  const host = meta.host || meta.destinationIP || '';
  const port = meta.destinationPort ? `:${meta.destinationPort}` : '';
  const dest = meta.destinationIP ? `${meta.destinationIP}${port}` : '';
  const chain = entry.chains?.[0] ?? '';
  const network = meta.network ?? '';
  const inbound = meta.type ?? '';
  const procFull = privacy ? '' : (meta.processPath ?? '');
  return {
    id: entry.id,
    host,
    dest,
    destIP: meta.destinationIP ?? '',
    rule: entry.rule ?? '',
    ruleName: entry.ruleName,
    ruleGroupKey: ruleGroupOf(entry).key,
    chain,
    l4: (network || inbound).toUpperCase(),
    l4Full: [network, inbound].filter(Boolean).join(' · '),
    udp: network.toLowerCase() === 'udp',
    procName: procFull ? basename(procFull) : '',
    procFull,
    sourceIP: privacy ? null : (meta.sourceIP ?? null),
    chainKind: chainKindOf(chain),
  };
}

/** 两个计数器**都**缺席 ⇒ 没有计数器（`null`）；任一在场 ⇒ 缺的那个按 0 计。 */
function totalBytesOf(entry: ConnectionEntry): number | null {
  const up = entry.upload;
  const dn = entry.download;
  if (up === undefined && dn === undefined) return null;
  return (up ?? 0) + (dn ?? 0);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * 活动连接：增量协议 + 速率记账
 * ═══════════════════════════════════════════════════════════════════════════ */

export interface ActiveFeed {
  index: Map<string, ConnectionEntry>;
  sync: ActiveDetailSync;
  /** 上一帧字节记账（算速率）：id → {up, dn, at(ms)}。 */
  prev: Map<string, { up: number; dn: number; at: number }>;
  /** M8 显示迟滞缓存：id → 上次「显示」的 d/u/total。 */
  sticky: Map<string, { d: number; u: number; t: number }>;
  /** 已乐观关闭、等后端增量确认消失的 id（挡在途帧把行画回来）。 */
  closing: Set<string>;
}

export function createActiveFeed(): ActiveFeed {
  return {
    index: new Map(),
    sync: { generation: null, sequence: 0 },
    prev: new Map(),
    sticky: new Map(),
    closing: new Set(),
  };
}

/** 离开活动视图 / 暂停时整体清空。**`generation` 必须回 `null`**，否则重订后的第一帧会被当成旧代拒掉。 */
export function resetActiveFeed(feed: ActiveFeed): void {
  feed.index.clear();
  feed.prev.clear();
  feed.sticky.clear();
  feed.closing.clear();
  feed.sync.generation = null;
  feed.sync.sequence = 0;
}

export interface ActiveFrame {
  rows: ConnRowVM[];
  /** 本帧采样时刻 epoch ms（时长按它算）。 */
  at: number;
  sequence: number;
}

/**
 * 应用一帧活动增量。**被协议守卫拒掉的帧返回 `null`**（调用方据此原样保留上一批行）。
 *
 * 三件必须由本函数保证的事，各自有单测钉着：
 *  1. 没收到 reset 基线之前的孤立增量不许污染空索引；
 *  2. `generation` 跳变而没带 reset 的帧不许被并进来（那正是「两个数据集并成一份」的形态）；
 *  3. reset 帧必须**连速率基线一起清**——只清索引不清 `prev`，重连后第一帧会拿旧账算速率。
 */
export function applyActiveFrame(
  feed: ActiveFeed,
  update: ConnectionsDetailUpdate,
  privacy: boolean,
): ActiveFrame | null {
  const applied = applyActiveDetailUpdate(feed.index, feed.sync, update);
  if (!applied.accepted) return null;

  const now = update.at || Date.now();
  if (applied.reset) {
    feed.prev.clear();
    feed.sticky.clear();
    feed.closing.clear();
  }
  for (const id of applied.removedIds) {
    feed.prev.delete(id);
    feed.sticky.delete(id);
  }
  // 抑制集自清理：索引里已经没有这条了，说明后端确认关掉了。
  for (const id of [...feed.closing]) {
    if (!feed.index.has(id)) feed.closing.delete(id);
  }

  const rows: ConnRowVM[] = [];
  for (const entry of feed.index.values()) {
    if (feed.closing.has(entry.id)) continue;
    const up = entry.upload ?? 0;
    const dn = entry.download ?? 0;
    const p = feed.prev.get(entry.id);
    let upRate = 0;
    let dnRate = 0;
    if (p && now > p.at) {
      const dt = (now - p.at) / 1000;
      upRate = Math.max(0, (up - p.up) / dt);
      dnRate = Math.max(0, (dn - p.dn) / dt);
    }
    feed.prev.set(entry.id, { up, dn, at: now });

    const measured = totalBytesOf(entry);
    const shown = feed.sticky.get(entry.id);
    const dVal = shown ? stickyDisplay(shown.d, dnRate) : dnRate;
    const uVal = shown ? stickyDisplay(shown.u, upRate) : upRate;
    const tVal = shown ? stickyDisplay(shown.t, up + dn, 64) : up + dn;
    feed.sticky.set(entry.id, { d: dVal, u: uVal, t: tVal });

    rows.push({
      ...projectStatic(entry, privacy),
      totalBytes: measured === null ? null : tVal,
      upRate: uVal,
      dnRate: dVal,
      startAt: entry.start ? Date.parse(entry.start) : Number.NaN,
      endedAt: null,
      rateSequence: update.sequence,
    });
  }
  return { rows, at: now, sequence: update.sequence };
}

/* ═══════════════════════════════════════════════════════════════════════════
 * 已结束历史：有界环
 * ═══════════════════════════════════════════════════════════════════════════ */

export interface ClosedFeed {
  index: Map<string, ClosedConnectionEntry>;
}

export function createClosedFeed(): ClosedFeed {
  return { index: new Map() };
}

export function resetClosedFeed(feed: ClosedFeed): void {
  feed.index.clear();
}

/**
 * 应用一帧已结束增量。上限由 `applyClosedHistoryUpdate` 自己裁剪（与后端同一个 1000），
 * 超出的最旧条目被逐出 —— 「本次会话总量」这类数字若要用它，必须自认是**有界近似**
 * （`data-contract.json#connections-closed` 的 trap）。本屏不画那种数字，只画列表。
 */
export function applyClosedFrame(
  feed: ClosedFeed,
  update: ConnectionsClosedUpdate,
  privacy: boolean,
): ConnRowVM[] {
  const ordered = applyClosedHistoryUpdate(feed.index, update);
  return ordered.map((item) => ({
    ...projectStatic(item.entry, privacy),
    totalBytes: totalBytesOf(item.entry),
    upRate: 0,
    dnRate: 0,
    startAt: item.entry.start ? Date.parse(item.entry.start) : Number.NaN,
    // sing-box 给的是 UnixNano。
    endedAt: Math.max(0, item.closedAt / 1_000_000),
    rateSequence: 0,
  }));
}

/* ═══════════════════════════════════════════════════════════════════════════
 * 检索 / 排序 / 派生量
 * ═══════════════════════════════════════════════════════════════════════════ */

/** 桌面检索字段 + 运行态规则名称；列表显示的名称也必须能搜到。 */
export function matchConnRow(row: ConnRowVM, query: string): boolean {
  if (!query) return true;
  return (row.host + row.dest + row.rule + (row.ruleName ?? '') + row.chain + row.procName + row.procFull + row.l4Full)
    .toLowerCase()
    .includes(query);
}

export function connRowAge(row: ConnRowVM, observedAt: number): number {
  return Number.isNaN(row.startAt)
    ? 0
    : Math.max(0, ((row.endedAt ?? observedAt) - row.startAt) / 1000);
}

export function connRowRates(row: ConnRowVM, activeSequence: number): { up: number; down: number } {
  return row.endedAt === null && row.rateSequence === activeSequence
    ? { up: row.upRate, down: row.dnRate }
    : { up: 0, down: 0 };
}

/**
 * 排序。`sort` 为 `null` = 保留数据源自身的稳定顺序（不伪造一个默认 comparator）。
 * 缺席的字节计数按 0 参与排名 —— 数据契约 trap 的前半句（"treat absent as zero for ranking"）。
 */
export function sortConnRows(
  rows: readonly ConnRowVM[],
  sort: SortState<ConnSortKey> | null,
  clock: { at: number; sequence: number },
): ConnRowVM[] {
  if (!sort) return [...rows];
  const { key, dir } = sort;
  const cmp = (a: ConnRowVM, b: ConnRowVM): number => {
    switch (key) {
      case 'type':
        return a.l4.localeCompare(b.l4);
      case 'host':
        return a.host.localeCompare(b.host);
      case 'dest':
        return a.dest.localeCompare(b.dest);
      case 'rule':
        return a.rule.localeCompare(b.rule);
      case 'chain':
        return a.chain.localeCompare(b.chain);
      case 'rate': {
        const ar = connRowRates(a, clock.sequence);
        const br = connRowRates(b, clock.sequence);
        return ar.down + ar.up - (br.down + br.up);
      }
      case 'total':
        return (a.totalBytes ?? 0) - (b.totalBytes ?? 0);
      case 'time':
        return connRowAge(a, clock.at) - connRowAge(b, clock.at);
      case 'ended':
        return (a.endedAt ?? 0) - (b.endedAt ?? 0);
      case 'proc':
        return a.procName.localeCompare(b.procName);
    }
  };
  return [...rows].sort((a, b) => dir * cmp(a, b));
}

/* ═══════════════════════════════════════════════════════════════════════════
 * 日志行
 * ═══════════════════════════════════════════════════════════════════════════ */

export interface LogRowVM {
  key: string;
  /** HH:MM:SS。 */
  ts: string;
  level: string;
  message: string;
}

/** 时间戳 → HH:MM:SS（与桌面 `fmtTs` 同形）。解析不了就原样吐回去，不编一个时间。 */
export function fmtLogTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

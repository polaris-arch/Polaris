/**
 * 移动端「连接」屏 —— 四合一入口（IA spec §1.1 第 7/8 行 + §2.3）。
 *
 * 它收拢桌面的两个屏：`connections`（拓扑 / 活动 / 已结束三视图）+ `logs`，
 * 落成四个分段：**概览 · 活动 · 已结束 · 日志**，默认概览（对齐 `ConnectionsScreen.tsx:249`）。
 *
 * # 为什么是这两个屏，而不是「都算诊断」
 *
 * 桌面把它们放进同一个 `诊断` 侧栏组（`Sidebar.tsx:53-56`），但真正的判据是**生命周期同构**：
 * 两者都是订阅驱动、进页订离页退，两者都在隐私模式下脱敏，而最强的一条是**导出菜单**——
 * 桌面日志屏那一个导出控件同时给出 `api.diagnostic.export` 与 `api.logs.export`
 * （`LogsScreen.tsx` 头注 :18）。「诊断」在桌面上已经是一件事了。
 *
 * # 容器 / 呈现分离
 *
 * 本文件**只做接线**：订阅、发 IPC、持有屏级状态、算派生量。呈现是纯组件 `ConnectionsView`，
 * 门可以拿夹具直接渲染它 —— 「测方法体」与「测接线」是两件事，分开才各自测得动。
 *
 * # 复用的是逻辑，不是组件
 *
 * 增量协议（`active-detail.ts`）、有界环（`closed-history.ts`）、排序三态（`sort-cycle.ts`）、
 * 规则对象（`connection-rule-subjects.ts`）、日志缓冲（`logs-buffer.ts`）、跟随判据
 * （`follow-scroll.ts`）、核内级别投影（`runtime-level.ts`）、格式化（`shared/format.ts`）
 * **全部原样复用桌面那几个纯 `.ts` 模块**。不能复用的只有桌面**组件**：它们的外观落在
 * `prototype.css` / `components.css` 上，那是契约 A1 禁止进入移动端的层叠链。
 *
 * # 写失败必有可见回显（IA 裁定 #14）
 *
 * 立项时移动外壳没挂任何 toast 宿主（唯一注入点是桌面 `AppShell.tsx`）⇒ `toast.error(...)`
 * 在这里就是 `console.error(...)`。**那半条根因 2026-09-06 已经不成立**（`mobile/MobileToaster.tsx`
 * 已由 `MobileApp` 挂在停靠区）。处置不变，理由换成今天仍成立的那半：本屏的写操作恰恰是
 * 「暂停 / 全部关闭 / 关闭筛选项 / 清空历史」这类**破坏性动作**，用户要知道的是**哪一个**失败了、
 * 那一行现在是什么状态，而全局 toast 是瞬态且不带行归属的。两者不互斥，本屏要的是前者。
 * 处置：**唯一写出口 `runWrite`** + 行内错误。判据在门 ⑧，且刻意不只钉「有 catch」——
 * 有 catch 而 catch 里什么都不做，正是要抓的那一种。
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from 'react';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from '../back-stack';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { pageWindow } from '@/components/ListPager';
import { useAppStore, useEffectiveConfig, useEffectiveRules } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { useLogRedactStore } from '@/store/use-log-redact-store';
import { editRoute } from '@/lib/staged-config';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { createTopicSubscription } from '@/lib/topic-subscription';
import { fmtBytes } from '@/components/screens/shared/format';
import { redactSensitive, shouldRedactLogs } from '@/domain/privacy';
import { ruleTypeNameKey, type RuleSubject } from '@/domain/rules';
import {
  analyzeRuleCoverage,
  appendSubjectToRule,
  isShadowedTarget,
  ruleAppendTargets,
  sortAppendTargets,
  type RuleAppendTarget,
} from '@/components/dialogs/rule-append';
import { closeMobileForm, openMobileForm } from '../forms/form-store';
import {
  TOPOLOGY_OTHERS_KEY,
  type Rule,
  type ConnectionsAggregate,
  type ConnectionsClosedUpdate,
  type ConnectionsDetailUpdate,
  type LogLevel,
  type RuntimeLogLevel,
} from '@/contracts/types';
import { cycleSortState, type SortState } from '@/components/screens/connections/sort-cycle';
import { connectionRuleSubjects } from '@/components/screens/connections/connection-rule-subjects';
import { maxLogId, mergeHydration, type LogRow } from '@/components/screens/logs/logs-buffer';
import { runtimeLevelView } from '@/components/screens/logs/runtime-level';
import {
  isUpwardLogScrollKey,
  shouldPauseLogFollow,
} from '@/components/screens/logs/follow-scroll';
import {
  ConnectionsView,
  sortLabelKey,
  type BarVM,
  type SheetGroup,
  type SheetItem,
  type SheetVM,
} from './ConnectionsView';
import {
  ACTIVE_SORT_KEYS,
  CLOSED_SORT_KEYS,
  CONNECTION_PAGE_SIZE,
  LOG_PAGE_SIZE,
  applyActiveFrame,
  applyClosedFrame,
  createActiveFeed,
  createClosedFeed,
  fmtLogTs,
  matchConnRow,
  resetActiveFeed,
  resetClosedFeed,
  sortConnRows,
  type ConnRowVM,
  type ConnSegment,
  type ConnSortKey,
  type LogRowVM,
} from './view-model';
import './connections.css';
import { filterByExactRule, useExactRuleFilter } from './exact-rule-filter';

/** 原地二次确认的键。语义与复位全在 `lib/confirm-twice.ts`，本文件不另起定时器。 */
const CLOSE_ALL_KEY = 'mconn-close-all';
const CLOSE_FILTERED_KEY = 'mconn-close-filtered';
const CLEAR_CLOSED_KEY = 'mconn-clear-closed';
const LOGS_CLEAR_KEY = 'mconn-logs-clear';

/** 日志内存结果上限（同桌面 `MAX_BUFFERED_ROWS`）。它不是检索历史上限。 */
const MAX_BUFFERED_ROWS = 500;
const SEARCH_DEBOUNCE_MS = 180;
/** 核内级别的重读间隔（仅日志分段挂载期间）。 */
const RUNTIME_LEVEL_POLL_MS = 5000;

const LEVEL_WEIGHT: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
  fatal: 4,
};
const LEVEL_OPTIONS: readonly LogLevel[] = ['debug', 'info', 'warn', 'error', 'fatal'];
type LogSource = 'all' | 'sing-box' | 'app';
const SOURCE_OPTIONS: readonly LogSource[] = ['all', 'sing-box', 'app'];

let logSubscriptionSeq = 0;
function nextLogSubscriptionId(): string {
  logSubscriptionSeq += 1;
  return `mobile-logs-${Date.now()}-${logSubscriptionSeq}`;
}

/**
 * 一行连接能转成的规则对象（域名 / 目的 IP / 进程）。
 *
 * 判据在桌面 `connection-rule-subjects.ts`（**唯一实现点**：`dest` 那一格是拼了端口的串，
 * 喂进去会静默少一条「复制 IP」）。本函数只做「行视图模型 → 那个函数要的形状」这一次投影，
 * 且**只写一次** —— 行动作面板与「加入规则」面板必须看到同一组对象，各投一次早晚会分叉。
 */
function subjectsOf(row: ConnRowVM): RuleSubject[] {
  return connectionRuleSubjects({
    id: row.id,
    chains: [row.chain],
    rule: row.rule,
    metadata: {
      host: row.host,
      destinationIP: row.destIP,
      processPath: row.procFull,
    },
  });
}

function matchesLog(row: LogRow, threshold: number, source: LogSource, query: string): boolean {
  return (
    LEVEL_WEIGHT[row.level] >= threshold &&
    (source === 'all' || row.source === source) &&
    (!query || (row.message + row.level).toLowerCase().includes(query))
  );
}

/** 旧日志「归档 / 删除」的失败回显键（两颗都在「更多」面板里按下，故与屏级动作同档）。 */
const LEGACY_WRITE_KEY = 'screen';

/**
 * 打开哪张面板。`null` = 没有面板。
 *
 * `rule` / `pick` 是行动作那条链的第二、第三跳：行 → 「加入规则」（逐维两颗）→ 「加入已有规则」
 * 的规则选择器。桌面把这三层摆在同一个右键面板里（主体切换器 + 两颗菜单项 + 一个 `rule-pick`
 * 弹窗），触屏这边是同一条链的三张底部面板。
 */
type SheetKind =
  | 'overflow'
  | 'sort'
  | 'logFilter'
  | 'export'
  | { row: string }
  | { rule: string }
  | { pick: string; subject: RuleSubject };

export function MobileConnectionsScreen(): ReactElement {
  const exactRule = useExactRuleFilter((s) => s.exactRule);
  const setExactRule = useExactRuleFilter((s) => s.setExactRule);
  const { t } = useTranslation();
  /** 薄封装：呈现层是纯组件，不该自己持有 i18next 实例。 */
  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string,
    [t],
  );
  /** 供回调读取最新翻译函数，避免把 `t` 塞进每个 `useCallback` 的依赖里。 */
  const trRef = useRef(tr);
  trRef.current = tr;

  const privacyMode = useAppStore((s) => s.privacyMode);
  const config = useEffectiveConfig();
  const diskConfig = useAppStore((s) => s.config);
  const saveConfig = useAppStore((s) => s.saveConfig);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const redactLogs = useLogRedactStore((s) => s.redactLogs);
  const toggleRedactLogs = useLogRedactStore((s) => s.toggleRedactLogs);
  const { armed, confirmTwice } = useConfirmTwice();

  const [segment, setSegment] = useState<ConnSegment>(exactRule === null ? 'overview' : 'active');
  const [sheet, setSheet] = useState<SheetKind | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  /* 展开的连接行是一个**模式态**，系统返回键先把它收起来 —— 登记点在下面 `visibleRows` 算出来之后，
     理由见那里。分段（概览/活动/已结束/日志）不吃返回：IA 里分段是屏内视图，不是栈，
     返回把视图切回上一个分段等于凭空发明一段历史。 */
  const collapseRow = useCallback(() => setExpandedId(null), []);

  /* ═══════════ 写失败的行内回显（本屏唯一可见通道）═══════════ */

  /**
   * 键 → 已翻译的文案。`screen` 是屏级动作（屏头溢出里按下的那些），`row:<id>` 是行级。
   * **每次重试先清**，否则上一次的红字会挂在一个已经成功的控件旁边。
   */
  const [writeErrors, setWriteErrors] = useState<Record<string, string>>({});
  const clearWriteError = useCallback((key: string) => {
    setWriteErrors((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }, []);

  /**
   * **本屏唯一的写出口。**每一处会改后端状态或碰系统资源的调用都必须经过它。
   *
   * `action` 回 `false` 与抛异常是**同一类失败**：后端 `connections_close` 一族把
   * 「核没跑 / gRPC 连不上 / 内核拒绝」装进 `ok:false` 的正常应答里，只 `catch` 是接不住的。
   * 两条腿都写进 `writeErrors` —— 门 ⑧ 逐条钉着它们，空 catch 与只回滚状态都不算数。
   */
  const runWrite = useCallback(
    async (key: string, failKey: string, action: () => Promise<boolean>): Promise<boolean> => {
      clearWriteError(key);
      try {
        const ok = await action();
        if (!ok) {
          setWriteErrors((prev) => ({ ...prev, [key]: trRef.current(failKey) }));
        }
        return ok;
      } catch (err) {
        console.error('[mobile-connections] write failed:', key, err);
        setWriteErrors((prev) => ({ ...prev, [key]: trRef.current(failKey) }));
        return false;
      }
    },
    [clearWriteError],
  );

  /* ═══════════ 活动 / 已结束：增量协议两条腿 ═══════════ */

  const [search, setSearch] = useState('');
  const [paused, setPaused] = useState(false);
  const [sort, setSort] = useState<SortState<ConnSortKey> | null>(null);
  const [page, setPage] = useState(0);

  const [activeRows, setActiveRows] = useState<ConnRowVM[]>([]);
  const [closedRows, setClosedRows] = useState<ConnRowVM[]>([]);
  const [clock, setClock] = useState({ at: 0, sequence: 0 });
  const [activeLoaded, setActiveLoaded] = useState(false);
  const [closedLoaded, setClosedLoaded] = useState(false);

  const activeFeedRef = useRef(createActiveFeed());
  const closedFeedRef = useRef(createClosedFeed());
  /** 隐私态要参与投影（sourceIP / 进程路径在投影层就被摘掉），故经 ref 供帧回调读最新值。 */
  const privacyRef = useRef(privacyMode);
  privacyRef.current = privacyMode;

  const onDetailFrame = useCallback((update: ConnectionsDetailUpdate) => {
    const frame = applyActiveFrame(activeFeedRef.current, update, privacyRef.current);
    // 被 generation / sequence 守卫拒掉的帧：**原样保留上一批行**，不清空、不合并。
    if (frame === null) return;
    setActiveRows(frame.rows);
    setClock({ at: frame.at, sequence: frame.sequence });
    setActiveLoaded(true);
  }, []);

  useEffect(() => {
    if (paused || segment !== 'active') return;
    /* 每次（重）订阅都清速率记账：恢复时两帧之间隔了整段暂停时长，拿旧账去减会算出一个
       被 clamp 成 0 的假速率，而那一格看起来完全正常。桌面同一处也是这么做的。 */
    activeFeedRef.current.prev.clear();
    const sub = createTopicSubscription<ConnectionsDetailUpdate>(
      {
        onFrame: (cb) => api.stats.onConnectionsDetail(cb),
        subscribe: () => api.stats.subscribe('detail'),
        unsubscribe: () => api.stats.unsubscribe('detail'),
      },
      onDetailFrame,
    );
    sub.setWanted(true);
    return () => sub.dispose();
  }, [paused, segment, onDetailFrame]);

  /*
   * **离开活动段**才整体清账 —— 暂停不清。
   *
   * 暂停的语义是「停止更新」，不是「清屏」：用户按下暂停，正是为了把此刻这一屏**留住**看清楚。
   * 把 `paused` 写进这条守卫，暂停会当场把列表清空 —— 那比不做暂停更坏。桌面同一处的守卫
   * 也只看视图（`ConnectionsScreen.tsx` 的 `if (view === 'active') return;`）。
   *
   * `generation` 必须一起回 `null`，否则重进本段后的第一帧 reset 会被当成「同代重复」拒掉，
   * 列表停在旧数据上一动不动。
   */
  useEffect(() => {
    if (segment === 'active') return;
    resetActiveFeed(activeFeedRef.current);
    setActiveRows([]);
    setClock({ at: 0, sequence: 0 });
    setActiveLoaded(false);
  }, [segment]);

  useEffect(() => {
    if (segment !== 'closed') return;
    setClosedLoaded(false);
    const sub = createTopicSubscription<ConnectionsClosedUpdate>(
      {
        onFrame: (cb) => api.stats.onConnectionsClosed(cb),
        subscribe: () => api.stats.subscribe('closed'),
        unsubscribe: () => api.stats.unsubscribe('closed'),
      },
      (update) => {
        setClosedRows(applyClosedFrame(closedFeedRef.current, update, privacyRef.current));
        setClosedLoaded(true);
      },
    );
    sub.setWanted(true);
    return () => {
      sub.dispose();
      resetClosedFeed(closedFeedRef.current);
      setClosedRows([]);
      setClosedLoaded(false);
    };
  }, [segment]);

  /* ═══════════ 概览：TOP 聚合 ═══════════ */

  const [aggregate, setAggregate] = useState<ConnectionsAggregate | null>(null);
  const [topN, setTopN] = useState(10);

  useEffect(() => {
    if (segment !== 'overview') return;
    const sub = createTopicSubscription<ConnectionsAggregate>(
      {
        onFrame: (cb) => api.stats.onConnectionsAggregate(cb),
        subscribe: () => api.stats.subscribe('aggregate'),
        unsubscribe: () => api.stats.unsubscribe('aggregate'),
      },
      setAggregate,
    );
    sub.setWanted(true);
    return () => sub.dispose();
  }, [segment]);

  const hosts: BarVM[] = useMemo(
    () =>
      [...(aggregate?.hosts ?? [])]
        // 先剔除后端的「其它」合并桶再排序截断：它的 count 常居高，小 N 下会挤掉真实域名。
        .filter((h) => h.name !== TOPOLOGY_OTHERS_KEY)
        .sort((a, b) => b.count - a.count)
        .slice(0, topN)
        .map((h) => ({ label: h.name, count: h.count, aurora: true })),
    [aggregate, topN],
  );

  const outbounds: BarVM[] = useMemo(
    () =>
      [...(aggregate?.outbounds ?? [])]
        .sort((a, b) => b.count - a.count)
        .slice(0, topN)
        .map((o) => ({
          // sentinel 合并桶必须换成本地化文案，**永远不许原样显示**
          // （`data-contract.json#connections-aggregate` 的 trap）。域名卡那边是整条剔除，
          // 出站卡这边是替换 —— 出站的「其它」是一个有意义的分组，剔掉会让占比对不上 100%。
          label:
            o.name === TOPOLOGY_OTHERS_KEY
              ? tr('home.others')
              : o.name === 'Direct'
                ? tr('home.routingDirect')
                : o.name,
          count: o.count,
          direct: o.name === 'Direct',
        })),
    [aggregate, topN, tr],
  );

  /* ═══════════ 日志腿 ═══════════ */

  const [logs, setLogs] = useState<LogRow[]>([]);
  const [logSearch, setLogSearch] = useState('');
  const [logSnapshot, setLogSnapshot] = useState<{ key: string; rows: LogRow[] } | null>(null);
  const [logStreamReady, setLogStreamReady] = useState(false);
  const [logSource, setLogSource] = useState<LogSource>('all');
  const [follow, setFollow] = useState(true);
  const [pendingCount, setPendingCount] = useState(0);
  const [logPage, setLogPage] = useState(0);
  const [runtimeLevel, setRuntimeLevel] = useState<RuntimeLogLevel | null>(null);
  const [diagnosticMode, setDiagnosticMode] = useState<boolean | null>(null);

  const followRef = useRef(follow);
  followRef.current = follow;
  const pendingRef = useRef<LogRow[]>([]);
  const lastIdRef = useRef(-1);
  const lastUserScrollAtRef = useRef<number | null>(null);
  const displayedLogPageRef = useRef(0);
  const logViewRef = useRef<HTMLDivElement>(null);
  const runtimeSeqRef = useRef(0);
  const runtimeMountedRef = useRef(false);

  const level: LogLevel = config?.logLevel ?? 'info';
  const displayLevel: LogLevel = diagnosticMode === true ? 'debug' : level;
  const threshold = LEVEL_WEIGHT[displayLevel];
  const logQuery = logSearch.trim().toLowerCase();
  const logSearchKey = `${displayLevel}\u0000${logSource}\u0000${logQuery}`;
  const logCriteriaRef = useRef({ key: logSearchKey, query: logQuery, threshold, source: logSource });
  logCriteriaRef.current = { key: logSearchKey, query: logQuery, threshold, source: logSource };

  const dedupe = useCallback((batch: LogRow[]): LogRow[] => {
    const fresh: LogRow[] = [];
    for (const l of batch) {
      if (typeof l._id !== 'number') {
        // 缺 `_id` 一律放行：宁可偶有重复行，也不能因为字段缺失把日志吞掉。
        fresh.push(l);
        continue;
      }
      if (l._id <= lastIdRef.current) continue;
      lastIdRef.current = l._id;
      fresh.push(l);
    }
    return fresh;
  }, []);

  /* 先监听、再水合；水合走**合并**而不是整体替换（两条腿同时起跑，替换会把订阅腿
     已经入列的行清掉 —— 表现为「核在高频输出时进日志页，历史区恒空」）。 */
  useEffect(() => {
    if (segment !== 'logs') return;
    let alive = true;
    const subscriptionId = nextLogSubscriptionId();
    let off: (() => void) | null = null;

    const onBatch = (raw: LogRow[]) => {
      if (!Array.isArray(raw) || raw.length === 0) return;
      const batch = dedupe(raw);
      if (batch.length === 0) return;
      const criteria = logCriteriaRef.current;
      if (criteria.query) {
        const matched = batch.filter((row) =>
          matchesLog(row, criteria.threshold, criteria.source, criteria.query),
        );
        if (matched.length > 0) {
          setLogSnapshot((prev) => ({
            key: criteria.key,
            rows: mergeHydration(
              prev?.key === criteria.key ? prev.rows : [],
              matched,
              MAX_BUFFERED_ROWS,
            ),
          }));
        }
      }
      if (followRef.current) {
        setLogs((prev) => {
          const next = [...prev, ...batch];
          return next.length > MAX_BUFFERED_ROWS ? next.slice(-MAX_BUFFERED_ROWS) : next;
        });
      } else {
        pendingRef.current.push(...batch);
        if (pendingRef.current.length > MAX_BUFFERED_ROWS) {
          pendingRef.current = pendingRef.current.slice(-MAX_BUFFERED_ROWS);
        }
        setPendingCount(pendingRef.current.length);
      }
    };

    void (async () => {
      off = await api.logs.onReceivedBatchReady(onBatch);
      if (!alive) {
        off();
        off = null;
        return;
      }
      setLogStreamReady(true);
      const batch = await api.logs.get(subscriptionId, MAX_BUFFERED_ROWS);
      if (!alive) {
        void api.logs.unsubscribe(subscriptionId);
        return;
      }
      if (!Array.isArray(batch)) return;
      const snapshot = batch.slice(-MAX_BUFFERED_ROWS);
      const top = maxLogId(snapshot);
      if (top !== null && top > lastIdRef.current) lastIdRef.current = top;
      setLogs((prev) => mergeHydration(prev, snapshot, MAX_BUFFERED_ROWS));
    })().catch((err) => {
      // 读腿失败没有用户动作可归因（不是「点了没反应」）；留控制台证据，不占用户的行内通道。
      console.error('[mobile-connections] log hydrate failed:', err);
    });

    return () => {
      alive = false;
      off?.();
      off = null;
      void api.logs.unsubscribe(subscriptionId);
      setLogStreamReady(false);
    };
  }, [segment, dedupe]);

  /* 非空查询走后端完整保留环；页面内存上限只约束 IPC / DOM。 */
  useEffect(() => {
    if (segment !== 'logs') return;
    if (!logQuery) {
      setLogSnapshot(null);
      return;
    }
    if (!logStreamReady) return;
    let alive = true;
    const key = logSearchKey;
    const timer = window.setTimeout(() => {
      void api.logs
        .search(logQuery, displayLevel, logSource, MAX_BUFFERED_ROWS)
        .then((batch) => {
          if (!alive || !Array.isArray(batch)) return;
          setLogSnapshot((prev) => ({
            key,
            rows: mergeHydration(prev?.key === key ? prev.rows : [], batch, MAX_BUFFERED_ROWS),
          }));
        })
        .catch((err) => {
          console.error('[mobile-connections] log search failed:', err);
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [segment, logSearchKey, logQuery, displayLevel, logSource, logStreamReady]);

  useEffect(() => {
    if (segment !== 'logs') return;
    let alive = true;
    api.logs
      .diagnosticState()
      .then((enabled) => {
        if (alive) setDiagnosticMode(enabled);
      })
      .catch(() => {
        if (alive) setDiagnosticMode(false);
      });
    return () => {
      alive = false;
    };
  }, [segment]);

  const refreshRuntimeLevel = useCallback(async () => {
    const seq = ++runtimeSeqRef.current;
    try {
      const next = await api.logs.runtimeLevel();
      if (runtimeMountedRef.current && seq === runtimeSeqRef.current) setRuntimeLevel(next);
    } catch {
      /* 读不到就保持上一份可自证状态，**不编一个级别**（`runtime-level.ts` 头注那条不变量）。 */
    }
  }, []);

  useEffect(() => {
    if (segment !== 'logs') return;
    runtimeMountedRef.current = true;
    void refreshRuntimeLevel();
    const timer = window.setInterval(() => void refreshRuntimeLevel(), RUNTIME_LEVEL_POLL_MS);
    const offLifecycle = api.proxy.onLifecycle(() => void refreshRuntimeLevel());
    return () => {
      runtimeMountedRef.current = false;
      runtimeSeqRef.current += 1;
      window.clearInterval(timer);
      offLifecycle();
    };
  }, [segment, refreshRuntimeLevel]);

  const runtimeView = runtimeLevelView(runtimeLevel, level, diskConfig?.logLevel ?? null);
  /* 写成三元而非拼键：拼出来的键会被 i18n 的死键判据判成「声明了但没人用」。 */
  const coreLevelHint =
    runtimeView.kind === 'known' && runtimeView.drift === 'unsaved'
      ? tr('logs.coreLevelDriftUnsaved')
      : runtimeView.kind === 'known' && runtimeView.drift === 'coreRestart'
        ? tr('logs.coreLevelDriftRestart')
        : tr('logs.coreLevelHint');
  const coreLevelNotice =
    runtimeView.kind === 'known' && runtimeView.drift
      ? tr('logs.coreLevelPending', { level: runtimeView.level.toUpperCase() })
      : runtimeView.kind === 'unavailable'
        ? tr('logs.coreLevelUnavailable')
        : null;

  const visibleLogs = useMemo(() => {
    const rows = logQuery ? (logSnapshot?.key === logSearchKey ? logSnapshot.rows : []) : logs;
    return rows.filter((row) => matchesLog(row, threshold, logSource, logQuery));
  }, [logs, logQuery, logSnapshot, logSearchKey, logSource, threshold]);

  const logWindow = pageWindow(
    visibleLogs.length,
    follow ? Number.MAX_SAFE_INTEGER : logPage,
    LOG_PAGE_SIZE,
  );
  displayedLogPageRef.current = logWindow.page;

  const redacting = shouldRedactLogs(privacyMode, redactLogs);
  const logRows: LogRowVM[] = useMemo(
    () =>
      visibleLogs.slice(logWindow.start, logWindow.end).map((l, i) => ({
        key: String(l._id ?? `${l.timestamp}-${i}`),
        ts: fmtLogTs(l.timestamp),
        level: l.level,
        message: redacting ? redactSensitive(l.message) : l.message,
      })),
    [visibleLogs, logWindow.start, logWindow.end, redacting],
  );

  const scrollLogsToBottom = useCallback(() => {
    const el = logViewRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  const pauseFollow = useCallback(() => {
    followRef.current = false;
    setLogPage(displayedLogPageRef.current);
    setFollow(false);
  }, []);

  useEffect(() => {
    if (segment === 'logs' && follow) scrollLogsToBottom();
  }, [segment, logRows, follow, scrollLogsToBottom]);

  /* 离底检测：只有**用户**滚动脱离底部才打断跟随。首次水合、字体重排、程序化吸底
     都会发 scroll，把它们算成用户意图 ⇒ 跟随会自己莫名其妙断掉。 */
  useEffect(() => {
    const el = logViewRef.current;
    if (segment !== 'logs' || !el) return;
    const mark = (): void => {
      lastUserScrollAtRef.current = performance.now();
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (isUpwardLogScrollKey(event.key, event.shiftKey)) mark();
    };
    const onScroll = (): void => {
      if (
        shouldPauseLogFollow({
          follow: followRef.current,
          metrics: el,
          lastUserIntentAt: lastUserScrollAtRef.current,
          now: performance.now(),
        })
      ) {
        pauseFollow();
      }
    };
    el.addEventListener('wheel', mark, { passive: true });
    el.addEventListener('touchstart', mark, { passive: true });
    el.addEventListener('pointerdown', mark, { passive: true });
    el.addEventListener('keydown', onKeyDown);
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      el.removeEventListener('wheel', mark);
      el.removeEventListener('touchstart', mark);
      el.removeEventListener('pointerdown', mark);
      el.removeEventListener('keydown', onKeyDown);
      el.removeEventListener('scroll', onScroll);
    };
  }, [segment, pauseFollow]);

  const resumeFollow = useCallback(() => {
    if (pendingRef.current.length > 0) {
      const pending = pendingRef.current;
      pendingRef.current = [];
      setPendingCount(0);
      setLogs((prev) => {
        const merged = [...prev, ...pending];
        return merged.length > MAX_BUFFERED_ROWS ? merged.slice(-MAX_BUFFERED_ROWS) : merged;
      });
    }
    followRef.current = true;
    setFollow(true);
    scrollLogsToBottom();
  }, [scrollLogsToBottom]);

  /* ═══════════ 列表派生：检索 → 排序 → 分页 ═══════════ */

  const listRows = segment === 'closed' ? closedRows : activeRows;
  const query = search.trim().toLowerCase();
  const filteredRows = useMemo(
    () => sortConnRows(filterByExactRule(listRows, segment === 'active' ? exactRule : null).filter((row) => matchConnRow(row, query)), sort, clock),
    [listRows, segment, exactRule, query, sort, clock],
  );
  const listWindow = pageWindow(filteredRows.length, page, CONNECTION_PAGE_SIZE);
  const visibleRows = filteredRows.slice(listWindow.start, listWindow.end);

  /* 🔴 登记条件挂在**交给视图渲染的那批行**上，不是「`expandedId` 非空」。
     `expandedId` 是屏级 state，与「这一行现在还画不画得出来」完全解耦：连接结束会被下一帧
     从 `activeRows` 里移走、翻页会让它滑出窗口 —— 两种都是常态。那时屏上没有任何一行是展开的，
     而层还在表上 ⇒ 按返回「关」掉一个看不见的东西：用户看到的是**返回键没反应**，
     比接线之前的「按一下退出」更糟，正是本批要挡的那种新缺陷。
     另外八处登记都不需要这一步：弹层的层住在弹层组件里（卸载即退订），批选态与二级页是
     「state 真 ⇒ UI 必在场」，只有这一处能指向一个不在场的目标。
     行内面板（`ConnectionsView` 的 `ActionSheet`）登记得比它晚 ⇒ LIFO 先关面板、再收行。 */
  useDismissableLayer(
    expandedId !== null && visibleRows.some((row) => row.id === expandedId),
    collapseRow,
  );

  useEffect(() => {
    setPage(0);
    setExpandedId(null);
  }, [segment, exactRule, query, sort]);

  useEffect(() => {
    if (page !== listWindow.page) setPage(listWindow.page);
  }, [page, listWindow.page]);

  useEffect(() => {
    setSort(null);
    setSheet(null);
    /* 换段即清失败回显：一条「关闭连接失败」挂在日志段的页头下面，指的是一颗现在看不见的按钮 ——
       那不是回显，是噪声。回显的寿命等于「用户还看得见那颗控件」。 */
    setWriteErrors({});
  }, [segment]);

  /* ═══════════ 写操作（全部经 runWrite）═══════════ */

  const closeFailKey = 'connections.closeFailed';

  const onCloseRow = useCallback(
    (id: string) => {
      const feed = activeFeedRef.current;
      const snapshot = activeRows;
      const at = snapshot.findIndex((r) => r.id === id);
      // 乐观移除 + 入抑制集：detail 流按 1s 合并，请求发出时可能已有一帧在途，
      // 那帧仍会更新这条连接 ⇒ 行「关掉又冒回来」。
      feed.closing.add(id);
      setActiveRows((prev) => prev.filter((r) => r.id !== id));
      void runWrite(`row:${id}`, closeFailKey, async () => {
        const res = await api.connections.close(id);
        if (res?.ok) return true;
        // 失败必须显式放回**原来的位置**：暂停态没有后续帧，不放回那条连接就凭空消失了，
        // 用户以为关成功了。追加到表尾同样不行 —— 那是「关不掉」+「还换了地方」两件事叠一起。
        feed.closing.delete(id);
        setActiveRows((prev) => {
          if (prev.some((r) => r.id === id)) return prev;
          const next = [...prev];
          const row = snapshot[at];
          if (row) next.splice(at < 0 ? next.length : Math.min(at, next.length), 0, row);
          return next;
        });
        return false;
      });
    },
    [activeRows, runWrite],
  );

  const onCloseAll = useCallback(() => {
    const ids = activeRows.map((r) => r.id);
    const feed = activeFeedRef.current;
    // fan-out **之前**批量入抑制集：请求在飞期间就可能有增量帧回来。
    for (const id of ids) feed.closing.add(id);
    void runWrite('screen', closeFailKey, async () => {
      const res = await api.connections.closeAll();
      if (res?.ok) return true;
      for (const id of ids) feed.closing.delete(id);
      return false;
    });
  }, [activeRows, runWrite]);

  const onCloseFiltered = useCallback(() => {
    // 射程是**筛选命中的全部**，不是当前页：分页只改 DOM 行数，不得把动作偷偷降级。
    const ids = filteredRows.map((r) => r.id);
    const feed = activeFeedRef.current;
    for (const id of ids) feed.closing.add(id);
    void runWrite('screen', closeFailKey, async () => {
      const results = await Promise.all(ids.map((id) => api.connections.close(id)));
      if (results.every((r) => r?.ok)) return true;
      // **只放回失败的那几条**，成功的继续被抑制到删除增量追上为止，否则整批一起闪回。
      results.forEach((r, i) => {
        if (!r?.ok) feed.closing.delete(ids[i]);
      });
      return false;
    });
  }, [filteredRows, runWrite]);

  const onClearClosed = useCallback(() => {
    void runWrite('screen', 'connections.clearClosedFailed', async () => {
      const snapshot = await api.stats.clearClosed();
      resetClosedFeed(closedFeedRef.current);
      setClosedRows([]);
      setClosedLoaded(true);
      return snapshot.connections.length === 0;
    });
  }, [runWrite]);

  const onClearLogs = useCallback(() => {
    void runWrite('screen', 'logs.clearFailed', async () => {
      await api.logs.clear();
      setLogs([]);
      setLogSnapshot(null);
      pendingRef.current = [];
      setPendingCount(0);
      return true;
    });
  }, [runWrite]);

  const onToggleDiagnostic = useCallback(() => {
    void runWrite('screen', 'logs.diagnosticFailed', async () => {
      const enabled = await api.logs.setDiagnostic(!(diagnosticMode ?? false));
      setDiagnosticMode(enabled);
      return true;
    });
  }, [diagnosticMode, runWrite]);

  /* ═══════════ W26 前遗留的无界 `singbox.log`（归档 / 删除）═══════════ */

  /**
   * 只读探测结果。`null` = 还没问过；`exists:false` = 问过了、没有这个文件。
   *
   * 两者必须分开：`null` 时不画提示（不能凭「还没问到」就说「没有旧日志」），而 `exists:false`
   * 是一个**已知**答案。桌面同一条腿同样是先 `legacyInfo()` 再决定要不要画那块提示
   * （`LogsScreen.tsx` 的 `legacyLog` state）。
   */
  const [legacyLog, setLegacyLog] = useState<{ exists: boolean; bytes: number } | null>(null);

  useEffect(() => {
    if (segment !== 'logs') return;
    let alive = true;
    void api.logs
      .legacyInfo()
      .then((info) => {
        if (alive) setLegacyLog({ exists: info.exists, bytes: info.bytes });
      })
      .catch((err) => {
        /* 读不到就**保持 `null`**（不画提示），不编一个「没有旧日志」的答案：那会把一块 1.4 GB
           的历史资产说成不存在。失败没有可归因的控件（不是用户按出来的），故只留 console。 */
        console.error('[mobile-connections] read legacy log info failed:', err);
      });
    return () => {
      alive = false;
    };
  }, [segment]);

  const onArchiveLegacy = useCallback(() => {
    void runWrite(LEGACY_WRITE_KEY, 'logs.archiveLegacyFailed', async () => {
      const res = await api.logs.archiveLegacy();
      /* 用户在系统保存器里自己按了取消 —— **不是失败**，什么都不说（同两条导出腿）。 */
      if (res.error === 'cancelled') return true;
      if (!res.success) return false;
      setLegacyLog((cur) => (cur === null ? cur : { ...cur, exists: false, bytes: 0 }));
      if (res.archived) toast.success(trRef.current('logs.archiveLegacyDone'));
      return true;
    });
  }, [runWrite]);

  /**
   * 删除那份旧日志。
   *
   * 二次确认走**表单宿主的 `confirm` 层**（`forms/form-store.ts`），不走本屏其余四颗破坏性动作
   * 用的原地 `confirmTwice`，也**绝不**用 `window.confirm`：
   *  · `window.confirm` 被 tauri-plugin-dialog 的 init 脚本覆写成 `plugin:dialog|confirm`，
   *    ACL 漏授时整条腿抛 rejection，用户看到的是「操作失败」而不是一个确认框；
   *  · 这一颗与那四颗的差别是**不可逆且不可重来**：删掉的是用户的历史资产，不是一批可以再攒出来的
   *    连接或日志缓冲。原地「再点一次」在拇指底下只差 40 ms，而一层写明「删的是哪一个、删了会怎样」
   *    的面板才配得上这个后果。`ConfirmPanel` 的头注写的正是这条分工。
   *
   * `onConfirm` 自行关闭那一层（`MobileConfirmPayload` 的既定语义）。
   */
  const onDeleteLegacy = useCallback(() => {
    const size = legacyLog === null ? '' : fmtBytes(legacyLog.bytes);
    const instanceId = openMobileForm({
      kind: 'confirm',
      payload: {
        title: trRef.current('logs.deleteLegacy'),
        message: `${trRef.current('logs.legacyTitle')} ${trRef.current('logs.legacyBody', { size })}`,
        confirmLabel: trRef.current('common.delete'),
        danger: true,
        onConfirm: () => {
          closeMobileForm(instanceId);
          void runWrite(LEGACY_WRITE_KEY, 'logs.deleteLegacyFailed', async () => {
            const res = await api.logs.deleteLegacy();
            setLegacyLog((cur) => (cur === null ? cur : { ...cur, exists: false, bytes: 0 }));
            if (res.deleted) toast.success(trRef.current('logs.deleteLegacyDone'));
            return true;
          });
        },
      },
    });
  }, [legacyLog, runWrite]);

  /** 旧日志仍在时持续提醒；操作完成后由短 Toast 回执。 */
  const legacyNotice = useMemo((): { tone: 'warn'; text: string } | null => {
    if (legacyLog === null || !legacyLog.exists) return null;
    return {
      tone: 'warn',
      text: `${tr('logs.legacyTitle')} ${tr('logs.legacyBody', { size: fmtBytes(legacyLog.bytes) })}`,
    };
  }, [legacyLog, tr]);

  const [exportBusy, setExportBusy] = useState<'report' | 'logs' | null>(null);
  const exportInFlight = useRef(false);
  const [copyBusy, setCopyBusy] = useState(false);
  const copyInFlight = useRef(false);

  /*
   * 两种导出（诊断报告 / 纯日志）。保存框开在 Rust 侧；回来的 `filePath` 桌面是路径、
   * Android 是 `content://…`（W-18 起 SAF 目标不再被后端吃成「用户取消了」）。
   *
   * `error === 'cancelled'` 是用户自己在系统保存器里按了取消 —— **不是失败**，什么都不说。
   *
   * 🔴 两条腿刻意**各写一遍**，不抽成一个吃回调的 `runExport`：本屏的唯一写出口判据
   * （`connections-screen.test.tsx` ⑧）要求写调用落在 `runWrite(` 的**实参区间**内，
   * 而把 `api.*.export()` 挪进外层包装的回调里，它就落到区间外了 —— 那正是那道门要抓的形态。
   * 省六行换一条绕过唯一出口的路，不划算。
   */
  const onExportReport = useCallback(() => {
    if (exportInFlight.current) return;
    exportInFlight.current = true;
    setExportBusy('report');
    void runWrite('export:report', 'logs.exportDiagFailed', async () => {
      const res = await api.diagnostic.export();
      if (res.error === 'cancelled') return true;
      if (!res.success) return false;
      const done = res.shared ? 'mobileActions.shareOpened' : 'logs.exportDiagDone';
      toast.success(trRef.current(done));
      return true;
    }).finally(() => { exportInFlight.current = false; setExportBusy(null); });
  }, [runWrite]);

  const onExportLogs = useCallback(() => {
    if (exportInFlight.current) return;
    exportInFlight.current = true;
    setExportBusy('logs');
    void runWrite('export:logs', 'logs.exportFailed', async () => {
      const res = await api.logs.export();
      if (res.error === 'cancelled') return true;
      if (!res.success) return false;
      toast.success(trRef.current('logs.exportDone'));
      return true;
    }).finally(() => { exportInFlight.current = false; setExportBusy(null); });
  }, [runWrite]);

  const onCopyLogs = useCallback(() => {
    if (copyInFlight.current) return;
    copyInFlight.current = true;
    setCopyBusy(true);
    const text = visibleLogs
      .map(
        (l) =>
          `[${fmtLogTs(l.timestamp)}] ${l.level.toUpperCase()}: ${
            redacting ? redactSensitive(l.message) : l.message
          }`,
      )
      .join('\n');
    void runWrite('screen', 'common.copyFail', async () => {
      await navigator.clipboard.writeText(text);
      toast.success(trRef.current('connections.copied'));
      return true;
    }).finally(() => { copyInFlight.current = false; setCopyBusy(false); });
  }, [visibleLogs, redacting, runWrite]);

  const onCopyText = useCallback(
    (key: string, text: string) => {
      if (copyInFlight.current) return;
      copyInFlight.current = true;
      setCopyBusy(true);
      void runWrite(key, 'common.copyFail', async () => {
        await navigator.clipboard.writeText(text);
        toast.success(trRef.current('connections.copied'));
        return true;
      }).finally(() => { copyInFlight.current = false; setCopyBusy(false); });
    },
    [runWrite],
  );

  const onLevelChange = useCallback(
    (next: LogLevel) => {
      if (!diskConfig) return;
      // 配置暂存闸门：`logLevel` 喂 sing-box 的 `log.level`，改了核要重启才跟上 ⇒ 它是暂存项，
      // 直落盘会让核继续按旧级别跑而暂存条只字不提。
      if (editRoute('logLevel', stagingEnabled) === 'staged') {
        stage({
          id: 'setting:logLevel',
          kind: 'setting',
          label: `${trRef.current('logs.currentLevel')} ${next}`,
          entityPath: ['logLevel'],
          nextValue: next,
        });
        return;
      }
      void runWrite('screen', 'mobileConnections.logs.levelSaveFailed', async () => {
        await saveConfig({ logLevel: next });
        return true;
      });
    },
    [diskConfig, stagingEnabled, stage, saveConfig, runWrite],
  );

  /* ═══════════ 把连接对象写进规则（桌面 `RuleSubjectMenuItems.tsx` 那两颗）═══════════ */

  /**
   * 展示面取 effective：暂存中新建 / 编辑过的规则也必须能当追加目标，否则「刚加的规则挑不到」
   * （逐字同桌面 `RulePickDialog` 那一行的理由）。
   */
  const rules = useEffectiveRules();

  /**
   * 追加进已有规则 —— 与桌面 `RuleSubjectMenuItems#append` 同一条腿，判据全部来自 `rule-append.ts`
   * （`appendSubjectToRule` 返回**整条** Rule，`{...base}` 起底 + 镜像同步 ⇒ 幂等整体替换）。
   *
   * 桌面那份住在一个 `.tsx` 组件里（渲染 `.ctx-i` 菜单按钮，外观落桌面层叠链），复用不了组件；
   * 复用的是它下面那整层纯函数。
   */
  const onAppendToRule = useCallback(
    (subject: RuleSubject, target: RuleAppendTarget) => {
      setSheet(null);
      const base: Rule | null = rules.find((rule) => rule.id === target.ruleId) ?? null;
      const next = base ? appendSubjectToRule(base, target, subject) : null;
      if (!next) {
        /* `null` = 无事可做 / 目标漂移 / 目标置灰。三种都不该走写腿，但**必须说话** ——
           静默返回与「点了没反应」在用户那里是同一件事。 */
        setWriteErrors((prev) => ({ ...prev, screen: trRef.current('rules.appendFail') }));
        return;
      }
      const label = next.remarks?.trim() || trRef.current(ruleTypeNameKey(next.type));
      // 暂存灰度的**唯一**闸门（与桌面 `RuleSubjectMenuItems` 同一个 `editRoute` + 同一个集合键）。
      if (editRoute('trafficRules', stagingEnabled) === 'staged') {
        stage({
          id: `rule:${next.id}`,
          kind: 'rule',
          label: `${trRef.current('rules.editTitle')} ${label}`,
          entityPath: ['trafficRules', next.id],
          nextValue: next,
        });
        return; // 零 IPC 写、零磁盘写
      }
      void runWrite('screen', 'rules.appendFail', async () => {
        await api.rules.update(next, 'route');
        /* 写后端即刷 store：`store.config` 只由 loadConfig/saveConfig 写，不刷则下一次打开这张
           选择器看到的还是旧规则（后端广播是慢路径）。 */
        await loadConfig(true);
        return true;
      });
    },
    [rules, stagingEnabled, stage, runWrite, loadConfig],
  );

  /* ═══════════ 面板（屏头溢出 / 行菜单 / 排序 / 日志筛选 / 导出 / 规则）═══════════ */

  const overflowItems = useMemo((): Array<SheetItem | 'separator'> => {
    if (segment === 'overview') return [];
    if (segment === 'active') {
      const items: Array<SheetItem | 'separator'> = [
        {
          id: 'pause',
          label: paused ? tr('connections.resume') : tr('connections.pause'),
          // 桌面把「暂停到底冻的是什么」留给了实现注释；触屏这边它是常驻的第二行。
          description: tr('mobileConnections.pauseNote'),
          onSelect: () => setPaused((p) => !p),
        },
        {
          id: 'sort',
          label: tr('mobileConnections.sort'),
          description: tr('mobileConnections.sortNote'),
          submenu: true,
          onSelect: () => setSheet('sort'),
        },
        'separator',
        {
          id: 'close-filtered',
          label: armed === CLOSE_FILTERED_KEY
            ? tr('connections.confirm')
            : tr('connections.closeFiltered', { n: filteredRows.length }),
          description: tr('connections.closeFilteredTitle'),
          danger: true,
          confirming: armed === CLOSE_FILTERED_KEY,
          // 桌面只在「搜索命中非空」时才显示这颗；这里在场并置灰 + 给理由（IA §3.3 第 2 条）。
          disabled: (query === '' && exactRule === null) || filteredRows.length === 0,
          onSelect: () => confirmTwice(CLOSE_FILTERED_KEY, onCloseFiltered),
        },
        {
          id: 'close-all',
          label: armed === CLOSE_ALL_KEY ? tr('connections.confirm') : tr('connections.closeAll'),
          description: tr('connections.closeAllTitle'),
          danger: true,
          confirming: armed === CLOSE_ALL_KEY,
          disabled: activeRows.length === 0,
          onSelect: () => confirmTwice(CLOSE_ALL_KEY, onCloseAll),
        },
      ];
      return items;
    }
    if (segment === 'closed') {
      return [
        {
          id: 'sort',
          label: tr('mobileConnections.sort'),
          description: tr('mobileConnections.sortNote'),
          submenu: true,
          onSelect: () => setSheet('sort'),
        },
        'separator',
        {
          id: 'clear-closed',
          label:
            armed === CLEAR_CLOSED_KEY
              ? tr('connections.confirmClearClosed')
              : tr('connections.clearClosed'),
          description: tr('connections.clearClosedTitle'),
          danger: true,
          confirming: armed === CLEAR_CLOSED_KEY,
          disabled: closedRows.length === 0,
          onSelect: () => confirmTwice(CLEAR_CLOSED_KEY, onClearClosed),
        },
      ];
    }
    return [
      {
        id: 'filter',
        label: tr('mobileConnections.logs.filter'),
        description: tr('mobileConnections.logs.filterSummary'),
        submenu: true,
        onSelect: () => setSheet('logFilter'),
      },
      {
        id: 'diagnostic',
        label: tr('logs.diagnosticMode'),
        description:
          diagnosticMode === true ? tr('logs.diagnosticTipOn') : tr('logs.diagnosticTipOff'),
        selected: diagnosticMode === true,
        disabled: diagnosticMode === null,
        onSelect: onToggleDiagnostic,
      },
      {
        id: 'redact',
        label: tr('logs.redact'),
        description: tr('logs.redactTip'),
        selected: shouldRedactLogs(privacyMode, redactLogs),
        // 隐私锁开着时恒脱敏，这颗开关改不动它 —— 置灰并说清楚，而不是让它看起来能关。
        disabled: privacyMode,
        onSelect: toggleRedactLogs,
      },
      {
        id: 'follow',
        label: tr('logs.follow'),
        description: follow ? tr('logs.followTipOn') : tr('logs.followTipOff'),
        selected: follow,
        onSelect: () => (follow ? pauseFollow() : resumeFollow()),
      },
      {
        id: 'copy',
        label: tr('mobileActions.copyLogs'),
        description: tr('logs.copyTip'),
        disabled: visibleLogs.length === 0 || copyBusy,
        busy: copyBusy,
        onSelect: onCopyLogs,
      },
      {
        id: 'export',
        label: tr('mobileActions.exportOptions'),
        submenu: true,
        description: tr('mobileActions.exportHint'),
        onSelect: () => {
          setSheet('export');
        },
      },
      'separator',
      {
        id: 'clear',
        label: armed === LOGS_CLEAR_KEY ? tr('logs.clearConfirm') : tr('logs.clear'),
        description: tr('logs.clearTip'),
        danger: true,
        confirming: armed === LOGS_CLEAR_KEY,
        disabled: logs.length === 0 && pendingCount === 0 && visibleLogs.length === 0,
        onSelect: () => confirmTwice(LOGS_CLEAR_KEY, onClearLogs),
      },
      /* ── W26 前遗留的无界 `singbox.log`：**有这个文件才有这两行** ──────────────────
         桌面把它们摆在提示块里的一行工具栏上；这里进「更多」（IA §2.3：破坏性动作不摆成工具栏）。
         条件与桌面同源（`legacyInfo().exists`）—— 没有那个文件时两端都一行不画，
         那不是「移动端少了两颗按钮」，是「没有可归档 / 可删除的对象」。 */
      ...(legacyLog?.exists === true
        ? ([
            'separator',
            {
              id: 'archive-legacy',
              label: tr('logs.archiveLegacy'),
              /* 归档要**目标所在的目录**（同目录临时文件 + `sync_all` + `rename` 提交，见
                 `logs.rs#archive_legacy_log`），而 SAF 交回的 content URI 上没有目录这个概念 ⇒
                 后端对 URI 目标**显式报错**（不假装成功、也不假装取消），红字走 `runWrite`。
                 这一行如实写出它要的是什么，用户才知道该往哪儿存。 */
              description: tr('logs.legacyTitle'),
              onSelect: onArchiveLegacy,
            },
            {
              id: 'delete-legacy',
              label: tr('logs.deleteLegacy'),
              description: tr('logs.legacyBody', { size: fmtBytes(legacyLog.bytes) }),
              danger: true,
              onSelect: onDeleteLegacy,
            },
          ] as Array<SheetItem | 'separator'>)
        : []),
    ];
  }, [
    legacyLog,
    onArchiveLegacy,
    onDeleteLegacy,
    segment,
    paused,
    armed,
    query,
    filteredRows.length,
    activeRows.length,
    closedRows.length,
    diagnosticMode,
    privacyMode,
    redactLogs,
    follow,
    logs.length,
    pendingCount,
    visibleLogs.length,
    tr,
    confirmTwice,
    onCloseFiltered,
    onCloseAll,
    onClearClosed,
    onClearLogs,
    onToggleDiagnostic,
    onCopyLogs,
    copyBusy,
    toggleRedactLogs,
    pauseFollow,
    resumeFollow,
  ]);

  const sheetVM = useMemo((): SheetVM | null => {
    if (sheet === null) return null;
    const screenError = writeErrors['screen'];
    if (sheet === 'overflow') {
      return {
        title: tr('mobileConnections.more'),
        items: overflowItems,
        error: screenError,
        status: copyBusy ? tr('mobileActions.preparing') : undefined,
      };
    }
    if (sheet === 'sort') {
      const keys = segment === 'closed' ? CLOSED_SORT_KEYS : ACTIVE_SORT_KEYS;
      return {
        title: tr('mobileConnections.sort'),
        body: tr('mobileConnections.sortNote'),
        items: keys.map((key) => ({
          id: key,
          label: tr(sortLabelKey(key)),
          description:
            sort?.key === key
              ? tr(sort.dir > 0 ? 'mobileConnections.sortAsc' : 'mobileConnections.sortDesc')
              : undefined,
          selected: sort?.key === key,
          onSelect: () => setSort((cur) => cycleSortState(cur, key)),
        })),
      };
    }
    if (sheet === 'logFilter') {
      return {
        title: tr('mobileConnections.logs.filter'),
        body: `${tr('logs.levelCaption')}${tr('logs.levelCoreRestartHint')}`,
        error: screenError,
        items: [
          ...LEVEL_OPTIONS.map((opt): SheetItem => ({
            id: `level-${opt}`,
            label: `${tr('logs.currentLevel')} · ${opt.toUpperCase()}`,
            selected: displayLevel === opt,
            disabled: diagnosticMode === true,
            // 置灰必须带理由（IA §3.3 第 2 条）：诊断模式把本页门槛钉在 DEBUG，
            // 不说出来的话这五行看起来就是「坏了」。
            description: diagnosticMode === true ? tr('logs.diagnosticTipOn') : undefined,
            onSelect: () => onLevelChange(opt),
          })),
          'separator',
          ...SOURCE_OPTIONS.map((opt): SheetItem => ({
            id: `source-${opt}`,
            label:
              opt === 'all'
                ? `${tr('logs.sourceLabel')} · ${tr('common.all')}`
                : opt === 'app'
                  ? `${tr('logs.sourceLabel')} · ${tr('logs.sourceApp')}`
                  : `${tr('logs.sourceLabel')} · sing-box`,
            selected: logSource === opt,
            onSelect: () => setLogSource(opt),
          })),
        ],
      };
    }
    if (sheet === 'export') {
      /* IA §4.15：两种导出都保留（它们回答不同的问题），载体是平台的原生对话框。
         桌面日志工具栏上那颗「日志目录」在 Android 上**没有对应物**（应用私有内部存储没有任何
         DocumentsProvider 暴露它，本应用能把私有文件交出去的两条路都是单个文件）——
         处置与证据登记在 `absence-register.test-support.ts` 的 `LogsScreen.tsx|k:logs.openDir…` 一条。

         🔴 **平台对象：是文件保存器，不是分享面板。** Android 上后端走的
         `app.dialog().file().save_file(...)`（`src-tauri/src/commands/misc/logs.rs`，
         与桌面同一行代码）落到 tauri-plugin-dialog 的 `DialogPlugin.kt:204 saveFileDialog`，
         发的是 `Intent.ACTION_CREATE_DOCUMENT`；share sheet（`ACTION_SEND`）全程没出现过。

         🔴 **W-18 起两颗按钮是真的接上了**（上一版是禁用 + 理由）。当时的缺口在 Rust 侧：
         `logs.rs` 的 `into_path().ok()` 对 SAF 返的 `FilePath::Url(content://…)` 恒 Err ⇒
         被当成「用户取消」静默失败。现在那两条腿按目标形态分派（`commands/picked_file.rs`），
         content URI 经 `tauri-plugin-fs` 的 fd 写出去，桌面仍走 `std::fs`。 */
      return {
        title: tr('mobileActions.chooseExport'),
        body: tr('mobileActions.exportHint'),
        status: exportBusy ? tr('mobileActions.preparing') : undefined,
        error: writeErrors['export:report'] ?? writeErrors['export:logs'],
        items: [
          {
            id: 'export-report',
            label: tr('mobileActions.report'),
            disabled: exportBusy !== null,
            busy: exportBusy === 'report',
            description: tr('mobileActions.reportHint'),
            onSelect: onExportReport,
          },
          {
            id: 'export-logs',
            label: tr('mobileActions.saveLogs'),
            disabled: exportBusy !== null,
            busy: exportBusy === 'logs',
            description: tr('logs.exportLogsOnlyDesc'),
            onSelect: onExportLogs,
          },
        ],
      };
    }
    /* ── 「加入规则」那一层（桌面 `RuleSubjectMenuItems` 的两颗菜单项）───────────────
       桌面一次只对**当前选中的那一维**出这两颗（右键面板里另有一个主体切换器）；这里逐维各铺
       一组，理由同「复制主体」那一排：触屏没有「先选维再点动作」这一跳。 */
    if (typeof sheet === 'object' && 'rule' in sheet) {
      const ruleRow = listRows.find((r) => r.id === sheet.rule);
      if (!ruleRow) return null;
      const items: Array<SheetItem | 'separator'> = [];
      for (const [i, subject] of subjectsOf(ruleRow).entries()) {
        if (i > 0) items.push('separator');
        const coverage = analyzeRuleCoverage(rules, subject);
        const covering = coverage.firstId
          ? (rules.find((rule) => rule.id === coverage.firstId) ?? null)
          : null;
        const subjectName = tr(`connections.ruleSubjects.${subject.kind}`);
        /* 顺序随「这个对象已经被某条规则盖住了没有」翻转 —— 逐字同桌面
           `RuleSubjectMenuItems` 的 `covering ? [pickExisting, createNew] : [createNew, pickExisting]`：
           已经有规则盖住它时，「加进那一条」远比「再建一条更靠后的」更可能是用户想要的。 */
        const createNew: SheetItem = {
          id: `rule-new-${subject.kind}`,
          label: `${tr('rules.addNew')} · ${subjectName}`,
          description: subject.value,
          onSelect: () => {
            setSheet(null);
            openMobileForm({ kind: 'rule', preset: { type: subject.type, value: subject.value } });
          },
        };
        const pickExisting: SheetItem = {
          id: `rule-existing-${subject.kind}`,
          label: `${tr('rules.addExisting')} · ${subjectName}`,
          description: covering
            ? tr('rules.subjectAlreadyInRule', { value: subject.value })
            : subject.value,
          submenu: true,
          onSelect: () => setSheet({ pick: ruleRow.id, subject }),
        };
        items.push(...(covering ? [pickExisting, createNew] : [createNew, pickExisting]));
      }
      return {
        title: tr('mobileConnections.addRule'),
        body: ruleRow.host || ruleRow.dest || ruleRow.id,
        items,
        error: screenError,
      };
    }
    /* ── 「加入已有规则」的规则选择器（桌面 `RulePickDialog`）──────────────────────
       判据全部来自 `rule-append.ts`：`ruleAppendTargets` 列出**每一条**规则的追加目标，
       `sortAppendTargets` 定序（可追加 → 已包含 → 其余），`isShadowedTarget` 给优先级提示。 */
    if (typeof sheet === 'object' && 'pick' in sheet) {
      const subject = sheet.subject;
      const targets = sortAppendTargets(ruleAppendTargets(rules, subject));
      const coverage = analyzeRuleCoverage(rules, subject);
      const toItem = (target: RuleAppendTarget): SheetItem => {
        const ruleName = tr(ruleTypeNameKey(target.ruleType));
        const typeName = tr(ruleTypeNameKey(target.type));
        /* 无备注时的行名：没有目标条件的行（新开腿 / 置灰行）第二行放的是动作或原因，
           规则身份必须挪到第一行来（逐字同桌面 `RulePickDialog#identity`）。 */
        const identity =
          target.condIndex < 0 && target.ruleValues.length > 0
            ? `${ruleName}: ${target.ruleValues.join(', ')}`
            : ruleName;
        const why =
          target.block === 'andMode'
            ? tr('rules.pickWhyAnd')
            : target.block === 'valueUnfit'
              ? tr('rules.pickWhyUnfit', { value: subject.value, type: tr(ruleTypeNameKey(subject.type)) })
              : target.block === 'contains'
                ? tr('rules.subjectAlreadyInRule', { value: subject.value })
                : null;
        /* 遮蔽提示只对**可追加**的项有意义（置灰项本来就点不下去，再挂一个「前面可能先命中」是噪音），
           且它**只是提示**：客户端启发式，权威匹配在内核 ⇒ 不据此禁用本项。 */
        const shadowed = target.block === null && isShadowedTarget(coverage, target);
        const detail =
          why ??
          (target.condIndex < 0
            ? tr('rules.pickNewCond', { type: typeName })
            : target.values.length > 0
              ? `${typeName}: ${target.values.join(', ')}`
              : typeName);
        const tags = [
          ...(target.enabled ? [] : [tr('rules.pickDisabledTag')]),
          ...(shadowed ? [tr('rules.pickShadowTag')] : []),
        ];
        return {
          id: `pick-${target.ruleId}#${target.condIndex}`,
          label: target.remarks || identity,
          description: tags.length > 0 ? `${detail} · ${tags.join(' · ')}` : detail,
          disabled: target.block !== null,
          onSelect: () => onAppendToRule(subject, target),
        };
      };
      const appendable = targets.filter((target) => target.block === null);
      const blocked = targets.filter((target) => target.block !== null);
      return {
        title: tr('rules.pickTitle'),
        body:
          targets.length === 0
            ? tr('rules.pickEmpty')
            : tr('rules.pickHint', {
                value: subject.value,
                type: tr(ruleTypeNameKey(subject.type)),
              }),
        resetKey: `pick:${sheet.pick}:${subject.kind}`,
        items: [],
        /* 可追加的那一组**不带 id ⇒ 恒展开**（主路径不许被折进去）；点不下去的那一组带 id ⇒
           可折叠、默认折起，组头上的计数是「还有多少条、为什么点不了」的入口。
           这正是桌面 `Csel` 那个可折叠组头在触屏上的对应物。 */
        groups: [
          ...(appendable.length > 0
            ? [{ label: tr('mobileConnections.rules.canAppend'), items: appendable.map(toItem) }]
            : []),
          ...(blocked.length > 0
            ? [
                {
                  id: 'blocked',
                  label: tr('mobileConnections.rules.cannotAppend'),
                  items: blocked.map(toItem),
                },
              ]
            : []),
        ] satisfies SheetGroup[],
        error: screenError,
      };
    }
    const row = listRows.find((r) => r.id === sheet.row);
    if (!row) return null;
    const subjects = subjectsOf(row);
    const items: Array<SheetItem | 'separator'> = subjects.map((subject) => ({
      id: `copy-${subject.kind}`,
      label: tr('connections.copySubject', {
        type: tr(`connections.ruleSubjects.${subject.kind}`),
      }),
      description: subject.detail || subject.value,
      disabled: copyBusy,
      busy: copyBusy,
      onSelect: () => onCopyText(`row:${row.id}`, subject.value),
    }));
    items.push({
      id: 'add-rule',
      label: tr('mobileConnections.addRule'),
      // 逐维两颗（新建 / 加入已有）在下一层，理由见那一支的注释。
      description: subjects.map((subject) => subject.value).join(' · '),
      submenu: true,
      onSelect: () => setSheet({ rule: row.id }),
    });
    if (segment === 'active') {
      items.push('separator');
      items.push({
        id: 'close',
        label: tr('connections.close'),
        danger: true,
        onSelect: () => {
          setSheet(null);
          onCloseRow(row.id);
        },
      });
    }
    return {
      title: row.host || row.dest || row.id,
      body: `${row.rule} › ${row.chain}`,
      items,
      error: writeErrors[`row:${row.id}`],
      status: copyBusy ? tr('mobileActions.preparing') : undefined,
    };
  }, [
    sheet,
    segment,
    sort,
    overflowItems,
    listRows,
    rules,
    writeErrors,
    displayLevel,
    diagnosticMode,
    logSource,
    exportBusy,
    tr,
    onLevelChange,
    onCopyText,
    copyBusy,
    onCloseRow,
    onExportReport,
    onExportLogs,
    onAppendToRule,
  ]);

  const rowErrors = useMemo(() => {
    const out: Record<string, string> = {};
    for (const [key, text] of Object.entries(writeErrors)) {
      if (key.startsWith('row:')) out[key.slice('row:'.length)] = text;
    }
    return out;
  }, [writeErrors]);

  const onSearchChange = useCallback(
    (value: string) => {
      if (segment === 'logs') setLogSearch(value);
      else setSearch(value);
    },
    [segment],
  );

  return (
    <ConnectionsView
      t={tr}
      segment={segment}
      onSegment={(next) => {
        setSegment(next);
        if (next !== 'active') setExactRule(null);
      }}
      search={segment === 'logs' ? logSearch : search}
      exactRule={segment === 'active' ? exactRule : null}
      onClearExactRule={() => setExactRule(null)}
      onSearch={onSearchChange}
      overflow={overflowItems}
      onOverflow={() => setSheet('overflow')}
      privacy={privacyMode}
      aggregateLoaded={aggregate !== null}
      topN={topN}
      onTopN={setTopN}
      hosts={hosts}
      outbounds={outbounds}
      rows={visibleRows}
      listTotal={filteredRows.length}
      listLoaded={segment === 'closed' ? closedLoaded : activeLoaded}
      listWindow={listWindow}
      onListPage={setPage}
      clock={clock}
      expandedId={expandedId}
      onToggleRow={(id) => setExpandedId((cur) => (cur === id ? null : id))}
      onRowMenu={(id) => setSheet({ row: id })}
      onCloseRow={onCloseRow}
      sort={sort}
      paused={paused}
      logRows={logRows}
      logTotal={visibleLogs.length}
      logWindow={logWindow}
      onLogPage={(next) => {
        followRef.current = false;
        setFollow(false);
        setLogPage(next);
      }}
      logPrivacyNote={privacyMode}
      legacyNotice={legacyNotice}
      coreLevelNotice={coreLevelNotice}
      coreLevelHint={coreLevelHint}
      follow={follow}
      pendingCount={pendingCount}
      onResumeFollow={resumeFollow}
      logViewRef={logViewRef}
      writeError={writeErrors['screen']}
      rowErrors={rowErrors}
      sheet={sheetVM}
      onCloseSheet={() => setSheet(null)}
      onBackSheet={sheet === null || sheet === 'overflow' || (typeof sheet === 'object' && 'row' in sheet) ? undefined : () => {
        if (typeof sheet === 'object' && 'pick' in sheet) setSheet({ rule: sheet.pick });
        else if (typeof sheet === 'object' && 'rule' in sheet) setSheet({ row: sheet.rule });
        else setSheet('overflow');
      }}
    />
  );
}

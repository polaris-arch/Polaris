/**
 * Polaris 主应用 store（Zustand）。状态与 action 按当前 UI 消费面组织，跨组件共享的配置、代理、节点、
 * 订阅事件和瞬态交互状态均在此维护；仅展示层投影通过独立 selector 派生。
 * 与上游实现的差异：原实现经 Electron ipcClient（{success,data} 信封），当前经 Tauri api（raw 值）；
 * api-client 已抹平此差异，store 层无感知。
 */

import { create } from 'zustand';
import type {
  UserConfig,
  ProxyStatus,
  Rule,
  TrafficStats,
  HelperStatus,
  IpInfoSnapshot,
  InvalidNodeInfo,
  PendingNodeChanges,
  ServerConfig,
} from '../contracts/types';
import type { UnlockResult, UnlockEgress, UnlockSnapshot } from '../contracts/unlock-detection';
import type { TailscaleStatusEvent } from '../contracts/tailscale-status';
import type { ServerSwitchReceipt } from '../contracts/server-switch';
import { api } from '../ipc';
import { withConfigWriteLock } from '../lib/config-write-lock';
import { configReadGeneration } from '../lib/config-read-generation';
import { replay, type StagedEntry } from '../lib/staged-config';
import {
  flushStagedPendingSync,
  hydrateStagedConfig,
  useStagedConfigStore,
} from './staged-config-store';
import i18n from '../i18n';
import {
  loadTailscaleLoginStatesFromCache,
  useTailscaleLoginCacheStore,
} from './use-tailscale-login-cache-store';
import { retainItemsById, retainRecordKeys } from './retain-entities';

// 兼容旧的类型别名（Polaris 习惯）
type ProxyMode = UserConfig['proxyMode'];

/**
 * 解锁检测显示态：提到 store 使其跨首页组件卸载存活（切导航离开 → 卸载但检测态留存，切回直接展示不重跑）。
 */
export interface UnlockDisplayState {
  results: Record<string, UnlockResult>;
  running: boolean;
  checkedAt: number | null;
  egress: UnlockEgress | null;
  lastRunAt: number | null;
}

/**
 * §2 待应用差集显示态（`proxy:getPendingChanges` pull + `event:proxyPendingChanges` push 的 store 落点）。
 *
 * 形状与后端 `PendingChangesSummary` / 前端契约 [`PendingNodeChanges`] 逐字段一致
 * （pull 与 push 同构，后端无适配层）。
 *
 * 需要**即时**差集的场景仍走 pull；切节点的实际应用状态由 server_switch 收据决定，
 * 读 store 快照可能被 push 更新滞后一拍，pull 拿的是当下真值。
 */
export interface PendingChangesState {
  added: string[];
  modified: string[];
  removed: string[];
  /** 「保存只持久化」延后的非节点结构性变更（三个数组看不见它，见 `PendingNodeChanges`）。 */
  restartDeferred: boolean;
}

/**
 * 后端载荷 → store 形态的**唯一**降级口径（pull / push / 两个入口共四条腿读同一份）。
 *
 * 三个 `?? []` 与 `?? false` 不是防御性洁癖：核未运行 / IPC 降级 / 旧版后端时可能拿到缺字段的
 * 对象，缺一个字段就按「那一类没有欠账」降级，绝不抛、也绝不让操作条恒亮。
 *
 * 🔴 **必须是一份**：这段降级此前在 `App.tsx` 里写了两遍（pull 一遍、push 一遍），移动入口接线时
 * 差点写成第三遍。`modified` 曾经恒空的那次退化，成因正是「pull 与 push 各持一份形状」。
 */
export function normalizePendingChanges(
  raw: Partial<PendingNodeChanges> | null | undefined
): PendingChangesState {
  return {
    added: raw?.added ?? [],
    modified: raw?.modified ?? [],
    removed: raw?.removed ?? [],
    restartDeferred: raw?.restartDeferred ?? false,
  };
}

/**
 * 跨屏一次性意图（契约「主页 Home · 空状态」：无节点 → 「添加节点/添加订阅」跳 server 页并**携 action**）。
 *
 * 为什么要一个 store 字段而不是「跳过去让用户自己再点一次添加」：空状态那两个按钮的语义就是
 * 「我要加一个节点/订阅」，落到节点页却只看到同样空的列表 = 意图在导航途中被丢掉，用户得在陌生页面
 * 重新找入口。对齐 上游 `serverPageAction`（`renderer/store/app-store.ts:139`，消费点
 * `pages/server-page.tsx:178-191`）。
 *
 * **一次性**：消费方（NodesScreen）读到后必须立刻置回 null，否则用户手动关掉对话框、切走再切回节点页时
 * 会被同一个意图反复弹窗。
 */
export type ServerPageAction = 'add-server' | 'add-sub';

export interface ConfigEntityMutation {
  collection: 'customAppPresets' | 'appRules';
  entityId: string;
  /** null = 删除；对象 = 按该集合主键整体 upsert。 */
  value: unknown | null;
}

/** These numbers order this WebView only; none is a backend revision/baseVersion. */
export interface ConfigPublication {
  config: UserConfig;
  serial: number;
  epoch: number;
  reason: string;
}
type ConfigScope = 'main' | 'tray';
type ConfigSession = {
  scope: ConfigScope;
  leases: number;
  requests: number;
  ready: Promise<void> | null;
  off: (() => void) | null;
};
type ConfigWaiter = {
  session: ConfigSession;
  minimumEpoch: number;
  resolve: (publication: ConfigPublication) => void;
  reject: (error: unknown) => void;
  cleanup: () => void;
};
let configScope: ConfigScope | null = null;
let configSession: ConfigSession | null = null;
let requestedEpoch = 0;
let publicationSerial = 0;
let refreshReason = 'mount';
let refreshNeeded = false;
// Keep the native flight even after disposal: JS invalidation cannot cancel Rust I/O.
let loadConfigInflight: { session: ConfigSession; epoch: number } | null = null;
const configWaiters = new Set<ConfigWaiter>();

function invalidateLoadConfig(reason: string): number {
  refreshReason = reason;
  configReadGeneration.invalidate();
  return ++requestedEpoch;
}

const configRefreshMessages = {
  CONFIG_REFRESH_CANCELLED: () => i18n.t('errors.configRefreshCancelled'),
  CONFIG_REFRESH_SCOPE_MISMATCH: () => i18n.t('errors.configRefreshScopeMismatch'),
  CONFIG_REFRESH_INVALID_RESPONSE: () => i18n.t('errors.configRefreshInvalidResponse'),
} as const;
function configRefreshError(code: keyof typeof configRefreshMessages): Error & { code: string } {
  return Object.assign(new Error(configRefreshMessages[code]()), { code });
}

type ConfigProjection = Pick<AppState, 'config' | 'servers' | 'selectedServerId' | 'rules' | 'dnsRules'>;
function prepareConfigPublication(config: UserConfig): ConfigProjection {
  // IPC can resolve malformed data (including undefined outside Tauri). Prepare
  // every mirror before changing publication serial, generation or store state.
  if (!config || typeof config !== 'object' || Array.isArray(config)) {
    throw configRefreshError('CONFIG_REFRESH_INVALID_RESPONSE');
  }
  const projection = {
    config,
    servers: config.servers ?? [], selectedServerId: config.selectedServerId,
    rules: config.trafficRules ?? config.policyRules ?? config.customRules ?? [],
    dnsRules: config.dnsRules ?? [],
  };
  if (!Array.isArray(projection.servers) || !Array.isArray(projection.rules) || !Array.isArray(projection.dnsRules)
    || (projection.selectedServerId != null && typeof projection.selectedServerId !== 'string')) {
    throw configRefreshError('CONFIG_REFRESH_INVALID_RESPONSE');
  }
  return projection;
}

function publishConfig(config: UserConfig, epoch: number, reason: string,
  projection = prepareConfigPublication(config)): ConfigPublication {
  const publication = { config, epoch, reason, serial: ++publicationSerial };
  configReadGeneration.published();
  useAppStore.setState({ ...projection, configPublication: { epoch, reason, serial: publication.serial } });
  return publication;
}

function releaseUnusedSession(session: ConfigSession): void {
  if (session !== configSession || session.leases || session.requests) return;
  configSession = null;
  session.off?.();
  session.off = null;
  refreshNeeded = false;
  invalidateLoadConfig('dispose');
  useAppStore.setState({ configLoading: false });
}

function settleConfigWaiters(session: ConfigSession, result: ConfigPublication | null, error?: unknown, maximumEpoch = Infinity): void {
  for (const waiter of configWaiters) {
    if (waiter.session !== session || waiter.minimumEpoch > (result?.epoch ?? maximumEpoch)) continue;
    configWaiters.delete(waiter);
    waiter.cleanup();
    session.requests--;
    if (result) waiter.resolve(result); else waiter.reject(error);
  }
  releaseUnusedSession(session);
}

function ensureConfigListener(session: ConfigSession): Promise<void> {
  if (session.ready) return session.ready;
  const registration = api.config.onChangedReady(() => {
    if (session !== configSession) return;
    invalidateLoadConfig('signal');
    refreshNeeded = true;
    pumpConfigRefresh();
  }).then((off) => {
    if (session !== configSession) { off(); return; }
    session.off = off;
  });
  session.ready = registration;
  void registration.then(() => pumpConfigRefresh(), (error) => {
    if (session !== configSession || session.ready !== registration) return;
    session.ready = null;
    refreshNeeded = false;
    useAppStore.setState({ configLoading: false });
    settleConfigWaiters(session, null, error);
    console.error('[app-store] config listener failed:', error);
  });
  return registration;
}

function getConfigSession(scope = configScope ?? 'main'): ConfigSession {
  if (configScope && configScope !== scope) throw configRefreshError('CONFIG_REFRESH_SCOPE_MISMATCH');
  configScope = scope;
  if (!configSession) {
    configSession = { scope, leases: 0, requests: 0, ready: null, off: null };
    invalidateLoadConfig('activate');
    refreshNeeded = true;
  }
  return configSession;
}

function pumpConfigRefresh(): void {
  const session = configSession;
  if (!session || !session.off || !refreshNeeded || loadConfigInflight) return;
  refreshNeeded = false;
  const flight = { session, epoch: requestedEpoch };
  const reason = refreshReason;
  loadConfigInflight = flight;
  // Exactly one beginRead per actual native GET, never per waiter.
  const readGeneration = configReadGeneration.beginRead();
  const current = () => session === configSession && flight.epoch === requestedEpoch
    && configReadGeneration.isCurrent(readGeneration);
  useAppStore.setState({ configLoading: true });
  let completed = false;
  let publicationStarted = false;
  void (async () => {
    try {
      const config = await api.config.get();
      if (!current()) return;
      const projection = prepareConfigPublication(config);
      if (!current()) return;
      if (session.scope === 'main') await hydrateStagedConfig(config, current);
      if (!current()) return;
      refreshNeeded = false;
      publicationStarted = true;
      const publication = publishConfig(config, flight.epoch, reason, projection);
      completed = true;
      settleConfigWaiters(session, publication);
    } catch (error) {
      // A publish/observer exception belongs to this flight even though our own
      // publication advanced the generation. Do not turn it into an endless retry.
      if (publicationStarted || current()) {
        completed = true;
        // An observer may synchronously request a newer epoch (or reacquire a
        // different session) before throwing. Keep that real tail-read intent.
        if (session === configSession && flight.epoch === requestedEpoch) refreshNeeded = false;
        settleConfigWaiters(session, null, error, flight.epoch);
        console.error('[app-store] loadConfig failed:', error);
      }
    } finally {
      if (loadConfigInflight !== flight) return;
      loadConfigInflight = null;
      // A dedicated read/publication can invalidate us without changing the epoch.
      // Waiters still require a real shared publication, so arrange a tail read.
      if (session === configSession && (!completed || [...configWaiters].some((w) => w.session === session))) refreshNeeded = true;
      if (configSession) {
        useAppStore.setState({ configLoading: refreshNeeded });
        pumpConfigRefresh();
      }
    }
  })();
}

/** One listener per WebView. Importing this module never acquires a lease. */
export function acquireConfigSync({ scope }: { scope: ConfigScope }): () => void {
  const session = getConfigSession(scope);
  session.leases++;
  if (!loadConfigInflight) refreshNeeded = true;
  void ensureConfigListener(session);
  pumpConfigRefresh();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    session.leases--;
    releaseUnusedSession(session);
  };
}

/** A fresh caller requires a GET started at/after this synchronous barrier. */
export function refreshConfig({ fresh = false, reason = 'refresh', signal }: { fresh?: boolean; reason?: string; signal?: AbortSignal } = {}): Promise<ConfigPublication> {
  if (signal?.aborted) return Promise.reject(configRefreshError('CONFIG_REFRESH_CANCELLED'));
  const session = getConfigSession();
  const minimumEpoch = fresh ? invalidateLoadConfig(reason) : requestedEpoch;
  if (!loadConfigInflight) refreshReason = reason;
  session.requests++;
  const result = new Promise<ConfigPublication>((resolve, reject) => {
    const abort = () => {
      if (!configWaiters.delete(waiter)) return;
      waiter.cleanup();
      session.requests--;
      reject(configRefreshError('CONFIG_REFRESH_CANCELLED'));
      releaseUnusedSession(session);
    };
    const waiter: ConfigWaiter = { session, minimumEpoch, resolve, reject,
      cleanup: () => signal?.removeEventListener('abort', abort) };
    configWaiters.add(waiter);
    signal?.addEventListener('abort', abort, { once: true });
  });
  if (!loadConfigInflight || loadConfigInflight.epoch !== requestedEpoch) refreshNeeded = true;
  void ensureConfigListener(session);
  pumpConfigRefresh();
  return result;
}

/** Call inside the existing write queue, when the mutation actually begins. */
export function beginConfigMutation(): number { return invalidateLoadConfig('mutation'); }

/** Saving and reading settle separately. Never republish an unversioned write reply. */
export function reconcileConfigMutation(): { barrier: number; settled: Promise<ConfigPublication> } {
  const settled = refreshConfig({ fresh: true, reason: 'mutation-settled' });
  return { barrier: requestedEpoch, settled };
}

export interface AppState {
  // ── 配置 ──
  config: UserConfig | null;
  configLoading: boolean;
  configPublication: Omit<ConfigPublication, 'config'> | null;

  // ── 代理运行态 ──
  proxyStatus: ProxyStatus | null;
  proxyStarting: boolean;
  proxyStopping: boolean;

  // ── 节点 / 订阅 ──
  servers: ServerConfig[];
  selectedServerId: string | null;
  tailscaleLoginStates: Record<string, boolean>;
  // serverId → 交互登录 URL。瞬态登录核经 EVENT_TAILSCALE_AUTH_URL 回填；1.14 主核也会为 NeedsLogin 的
  // endpoint always-emit。放 store 而非弹窗本地 state：卡片角标要据此判「登录中」，弹窗关了它也得在。
  tailscaleAuthUrls: Record<string, string>;
  // serverId → 用户是否**显式发起**了登录（TsLoginDialog 提交即置位，弹窗收尾清除）。
  // 与 authUrl 分开：主核 always-emit 会给未选中/未就绪节点持续推 URL，只凭 hasAuthUrl 判「登录中」会误报。
  tailscaleLoginInitiated: Record<string, boolean>;

  // ── 规则 ──
  rules: Rule[];
  dnsRules: Rule[];

  // ── 流量统计 / 连接（订阅驱动）──
  trafficStats: TrafficStats | null;

  // ── 出口 IP ──
  ipInfo: IpInfoSnapshot | null;
  /** 最高已接收后端帧版本；清空显示帧时仍保留，拒绝迟到的旧会话帧。 */
  ipInfoRevision: number;

  // ── 解锁检测 ──
  unlock: UnlockDisplayState;

  // ── §2 待应用差集 ──
  pendingChanges: PendingChangesState;
  invalidNodes: InvalidNodeInfo[];

  // ── helper（macOS 提权）──
  helperStatus: HelperStatus | null;

  // ── 隐私模式 ──
  privacyMode: boolean;

  // ── Tailscale 节点详情：STATUS 流推送的**整帧末态**（serverId → 末帧）──
  // 存整帧而非只存 `peers`：出口告警要同时读 `peers`（离线/未广告）与 `backendState`/`expired`
  // （未认证），两者必须来自**同一帧**，拆成两个切片会出现「peers 是第 N 帧、认证态是第 M 帧」的
  // 撕裂态。`loggedIn` 另有 `tailscaleLoginStates`（缓存+state 目录+STATUS 三源折叠的**综合**态，
  // 代理关时也有值）——本切片是**未折叠的原始帧**，只在核跑着时有，两者不互相替代。
  tailscaleStatuses: Record<string, TailscaleStatusEvent | undefined>;

  // ── 跨屏一次性意图（Home 空状态 → Nodes 页自动唤起对应对话框）──
  serverPageAction: ServerPageAction | null;
  // ── 跨屏一次性意图（设置页「在主页切换 →」→ Home 页高亮落点，原型 :4052）──

  // ── 动作 ──
  loadConfig: (force?: boolean) => Promise<void>;
  /** Publish only a complete authoritative selection readback. No writes or optimistic intent. */
  acceptSelectionConfig: (config: UserConfig) => Promise<void>;
  /** 原子更新本次明确变动的顶层字段；未列出的字段保留后端锁内最新值。 */
  saveConfig: (patch: Partial<UserConfig>) => Promise<void>;
  /** 在后端锁内最新集合上按实体主键提交；多条 mutation 同盘提交，避免数组快照覆盖与半成功。 */
  mutateConfigEntities: (mutations: readonly ConfigEntityMutation[]) => Promise<void>;
  updateProxyMode: (mode: ProxyMode) => Promise<void>;
  startProxy: () => Promise<void>;
  stopProxy: () => Promise<void>;
  switchServer: (serverId: string) => Promise<ServerSwitchReceipt | null>;
  setProxyStatus: (status: ProxyStatus | null) => void;
  refreshProxyStatus: () => Promise<void>;
  setTrafficStats: (stats: TrafficStats | null) => void;
  setIpInfo: (info: IpInfoSnapshot | null) => void;
  setUnlock: (partial: Partial<UnlockDisplayState>) => void;
  setUnlockProgress: (serviceId: string, result: UnlockResult) => void;
  applyUnlockSnapshot: (snap: UnlockSnapshot) => void;
  beginUnlockCheck: () => void;
  resetUnlock: () => void;
  setPendingChanges: (changes: PendingChangesState) => void;
  setInvalidNodes: (nodes: InvalidNodeInfo[]) => void;
  setPrivacyMode: (enabled: boolean) => void;
  setTailscaleLoginState: (serverId: string, loggedIn: boolean) => void;
  applyTailscaleStateExists: (states: Record<string, boolean>) => void;
  setTailscaleAuthUrl: (serverId: string, url: string | null) => void;
  setTailscaleStatus: (event: TailscaleStatusEvent) => void;
  clearTailscaleStatus: (serverId: string) => void;
  setTailscaleLoginInitiated: (serverId: string, initiated: boolean) => void;
  /** 当前配置删除节点后统一驱逐所有以 serverId 为键的派生状态。 */
  retainServerIds: (serverIds: readonly string[]) => void;
  setServerPageAction: (action: ServerPageAction | null) => void;
  reset: () => void;
}

const EMPTY_PENDING: PendingChangesState = {
  added: [],
  modified: [],
  removed: [],
  restartDeferred: false,
};

const freshUnlock = (): UnlockDisplayState => ({
  results: {},
  running: false,
  checkedAt: null,
  egress: null,
  lastRunAt: null,
});

export const useAppStore = create<AppState>((set, get) => ({
  config: null,
  configLoading: false,
  configPublication: null,

  proxyStatus: null,
  proxyStarting: false,
  proxyStopping: false,

  servers: [],
  selectedServerId: null,
  // 启动秒显：从 localStorage 缓存恢复「登录过没」布尔表（代理关时的兜底显示）
  tailscaleLoginStates: loadTailscaleLoginStatesFromCache(),
  tailscaleAuthUrls: {},
  tailscaleLoginInitiated: {},

  rules: [],
  dnsRules: [],

  trafficStats: null,
  ipInfo: null,
  ipInfoRevision: -1,

  unlock: freshUnlock(),

  pendingChanges: EMPTY_PENDING,
  invalidNodes: [],

  helperStatus: null,
  privacyMode: true,
  tailscaleStatuses: {},
  serverPageAction: null,

  loadConfig: async (force?: boolean) => {
    try { await refreshConfig({ fresh: force, reason: force ? 'reload' : 'load' }); }
    catch (error) { console.error('[app-store] loadConfig failed:', error); }
  },

  acceptSelectionConfig: async (config) => {
    // Callers retain their dedicated-read generation check before this boundary.
    publishConfig(config, -1, 'selection');
    const publication = configReadGeneration.current();
    if (configScope !== 'tray') await hydrateStagedConfig(config, () => configReadGeneration.isCurrent(publication));
    await get().refreshProxyStatus();
  },

  saveConfig: async (patch) => {
    try {
      await withConfigWriteLock(async () => {
        beginConfigMutation();
        await api.config.patch(patch);
      });
    } catch (error) {
      void reconcileConfigMutation().settled.catch(() => {});
      throw error;
    }
    // A failed read is not a failed save and must not encourage duplicate writes.
    await reconcileConfigMutation().settled.catch(() => {});
  },

  mutateConfigEntities: async (mutations) => {
    try {
      await withConfigWriteLock(async () => {
        beginConfigMutation();
        await api.config.mutateEntities(mutations);
      });
    } catch (error) {
      void reconcileConfigMutation().settled.catch(() => {});
      throw error;
    }
    await reconcileConfigMutation().settled.catch(() => {});
  },

  updateProxyMode: async (mode) => {
    if (!get().config) return;
    await get().saveConfig({ proxyMode: mode });
  },

  startProxy: async () => {
    // `config` 只用来判「配置加载完了没」（没加载完 = UI 还没准备好让用户点连接）。
    // **不再作为起核载荷下发**：起核用哪份配置由后端读盘决定（见 Rust `proxy_start` 头注）——
    // 这份内存副本靠 configChanged 异步刷新，「写盘 → 立刻点启动」会用写之前的那份。
    if (!get().config) {
      throw new Error(i18n.t('errors.configNotLoaded'));
    }
    set({ proxyStarting: true });
    try {
      // 停核后保留下来的 staged 仍是用户此刻在 UI 里看到的有效配置。连接是下一次运行态提交点：
      // 先复用“保存”的冲突/合并事务；保存失败或等待用户裁决时保持停止，绝不按旧磁盘配置偷偷起核。
      if (!(await useStagedConfigStore.getState().save())) return;
      // persist(false) 的 IPC 是排队异步腿；等它落定再 start，后端的托盘/自动连接保险才不会误拦本次。
      await flushStagedPendingSync();
      // 保存 IPC 在途期间仍可产生新编辑；旧回包只会结算自己捕获的那批，所以此刻可能又有
      // staged。连接不能跨过它按旧批次起核；保持停止并让待保存条继续明示，由用户下一次提交。
      // 后端 pending marker 还有同样的 fail-closed 闸，本检查用于避免一次可预期的启动报错。
      if (useStagedConfigStore.getState().entries.length > 0) return;
      await api.proxy.start();
    } finally {
      set({ proxyStarting: false });
    }
  },

  stopProxy: async () => {
    set({ proxyStopping: true });
    try {
      await api.proxy.stop();
    } finally {
      set({ proxyStopping: false });
    }
  },

  /** 切节点后采用后端最新完整配置：磁盘写本身已是原子 RMW，但若只拿进入队列前捕获的本地快照
   * 乐观改 selectedServerId，会把排在它前面的其它设置写在 UI 内存里暂时回滚。 */
  switchServer: async (serverId) => {
    // 与暂存保存/配置补丁共用同一条 webview 内写队列：快速连点必须按调用顺序落盘，
    // 否则较慢的旧 IPC 可能最后回包，把最后一次点击覆盖回旧节点。
    const readback = await withConfigWriteLock(async () => {
      let receipt: Awaited<ReturnType<typeof api.server.switch>> | undefined;
      let failure: unknown;
      try { receipt = await api.server.switch(serverId); } catch (error) { failure = error; }
      const readGeneration = configReadGeneration.beginRead();
      try {
        return { receipt, failure, config: await api.config.get(), readGeneration };
      } catch (error) {
        if (!configReadGeneration.isCurrent(readGeneration)) return null;
        throw Object.assign(error instanceof Error ? error : new Error('Selection readback failed'), { code: 'SERVER_SWITCH_READBACK_FAILED', commandFailure: failure });
      }
    });
    if (!readback || !configReadGeneration.isCurrent(readback.readGeneration)) return null;
    const { receipt, config, failure } = readback;
    publishConfig(config, -1, 'selection');
    const publicationGeneration = configReadGeneration.current();
    const hydration = configScope === 'tray' ? Promise.resolve()
      : hydrateStagedConfig(config, () => configReadGeneration.isCurrent(publicationGeneration));
    if (typeof document !== 'undefined' && document.documentElement.dataset.mobileOs === 'ios') await hydration;
    // A newer publication/read also owns feedback during asynchronous iOS hydration.
    await get().refreshProxyStatus();
    if (failure !== undefined) throw failure;
    return configReadGeneration.isCurrent(publicationGeneration) ? receipt ?? null : null;
  },

  setProxyStatus: (status) => set({ proxyStatus: status }),

  /** 从后端重拉连接态。事件驱动的唯一刷新入口（App.tsx 的 proxyStarted/proxyStopped 订阅调它）——
   * `startProxy`/`stopProxy` 自身不写 `proxyStatus`，因为托盘浮层（独立窗口，不共享本 store）也能
   * 触发启停，只有走事件才能让两个窗口收敛到同一真值。失败不抛：连接态刷新失败不该冒泡成 UI 异常。 */
  refreshProxyStatus: async () => {
    try {
      const status = await api.proxy.getStatus();
      set({ proxyStatus: status ?? null });
    } catch (err) {
      console.error('[app-store] refreshProxyStatus failed:', err);
    }
  },
  setTrafficStats: (stats) => set({ trafficStats: stats }),
  setIpInfo: (info) => set((state) => {
    if (info === null) return { ipInfo: null };
    // IPC payloads are runtime data. A missing/unsafe revision must never poison
    // the watermark (undefined would make every later comparison ineffective).
    if (!Number.isSafeInteger(info.revision) || info.revision < 0) return state;
    // Rust 在缓存提交时分配 revision。所有写入来源共享此门：事件、peek、
    // 手动 get 即使按相反顺序返回，也不能将已显示的新帧倒退。
    if (info.revision <= state.ipInfoRevision) return state;
    return { ipInfo: info, ipInfoRevision: info.revision };
  }),
  setUnlock: (partial) =>
    set((state) => ({ unlock: { ...state.unlock, ...partial } })),

  /**
   * 单服务 settle 逐个点亮 —— **merge 进 results，不整体替换**（对齐 上游 `setUnlockProgress`
   * `app-store.ts:554-557`）。
   *
   * 曾经的写法是 `setUnlock({ results: { [id]: result } })`：`setUnlock` 只浅合并**顶层字段**，`results`
   * 整个被换成单键对象 ⇒ 每收到一条 progress 就抹掉另外五项，六个徽章只有最后 settle 的那个留得住。
   */
  setUnlockProgress: (serviceId, result) =>
    set((state) => ({
      unlock: { ...state.unlock, results: { ...state.unlock.results, [serviceId]: result } },
    })),

  /**
   * 应用一份终态快照（`unlock:run` 返回值 / `EVENT_UNLOCK_UPDATED` / 挂载水合共用），对齐 上游
   * `applyUnlockSnapshot`（`app-store.ts:558-600`）。
   *
   * **为何必须消费 `run()` 的返回值**：后端有若干条早退路径（gating 短路 / S-gate notReady / TTL 命中 /
   * force 硬下限）——它们都 emit 了终态，但把返回值丢掉的调用方会把 `running:true` 永远留在 store 上。
   * 三态分流按 上游 逐条对齐：
   * - `blockedReason`（代理没跑 / 出口无效）→ 复位 idle，**不打 lastRunAt**（毫秒级短路，不该触发 15s 冷却）；
   * - `notReady`（就绪门未过 / 整轮预算耗尽）→ idle **但打戳**（后端已置 lastRunAt，force 硬下限已生效，冷却须镜像）；
   * - 真检测落定 → 落 results/checkedAt/egress，`lastRunAt` 取 `snap.checkedAt`（**后端真跑那一轮的完成时刻**，
   *   而非 `Date.now()`）——TTL 命中回放的是旧 checkedAt，用它派生冷却才不会把 15s 前的旧检测误算成刚跑完。
   */
  applyUnlockSnapshot: (snap) =>
    set((state) => {
      // 陈旧轮 no-op 快照（空 results + 无终态标记）：本轮在飞期间被 invalidate 已被后端丢弃，
      // 新一轮可能已 beginUnlockCheck 接管显示 → 不覆盖，否则把新一轮的「检测中」清成空 idle。
      if (
        !snap.checkedAt &&
        !snap.notReady &&
        !snap.blockedReason &&
        Object.keys(snap.results).length === 0
      ) {
        return { unlock: state.unlock };
      }
      if (snap.blockedReason) {
        return {
          unlock: { results: {}, running: false, checkedAt: null, egress: null, lastRunAt: null },
        };
      }
      if (snap.notReady) {
        return {
          unlock: {
            results: {},
            running: false,
            checkedAt: null,
            egress: null,
            lastRunAt: Date.now(),
          },
        };
      }
      return {
        unlock: {
          results: snap.results,
          running: false,
          checkedAt: snap.checkedAt,
          egress: snap.egress,
          lastRunAt: snap.checkedAt ?? Date.now(),
        },
      };
    }),

  /** 置「检测中」（六徽章转圈）；保留 lastRunAt——冷却窗由上一次**完成**时刻派生，不因新一轮开跑而重置。 */
  beginUnlockCheck: () =>
    set((state) => ({
      unlock: {
        results: {},
        running: true,
        checkedAt: null,
        egress: null,
        lastRunAt: state.unlock.lastRunAt,
      },
    })),

  /** 复位 idle（停代理 / IPC 失败兜底）：连 lastRunAt 一并清，重连后不被陈旧冷却误锁刷新钮。 */
  resetUnlock: () => set({ unlock: freshUnlock() }),
  setPendingChanges: (changes) => set({ pendingChanges: changes }),
  setInvalidNodes: (nodes) => set({ invalidNodes: nodes }),
  setPrivacyMode: (enabled) => set({ privacyMode: enabled }),

  /**
   * 写入某 TS 节点的登录态真值（STATUS 流 / state 清除事件的唯一入口）。
   *
   * 双写：内存态供 UI 即时渲染；localStorage 缓存供下次启动秒显——代理关时没有常驻核 STATUS 流，
   * 缓存是那时唯一能秒显的来源（see use-tailscale-login-cache-store 顶注）。
   */
  setTailscaleLoginState: (serverId, loggedIn) => {
    if (!serverId) return;
    useTailscaleLoginCacheStore.getState().setCached(serverId, loggedIn);
    const prev = get().tailscaleLoginStates;
    if (prev[serverId] === loggedIn) return; // 值未变 → 不新建对象，省订阅者重渲染
    set({ tailscaleLoginStates: { ...prev, [serverId]: loggedIn } });
  },

  /**
   * 挂载兜底：用当前持久会话批量校正账号显示（TAILSCALE_STATE_EXISTS 的 cached 模式，不起核）。
   *
   * 不写 localStorage 缓存：缓存的语义是「上次 STATUS 已知态」，这里的持久会话不证明授权仍有效。
   * 与 STATUS 的竞态（挂载时代理已在跑）无需额外守卫：STATUS 是持续流，
   * 下一帧即把真值盖回来。
   */
  applyTailscaleStateExists: (states) => {
    const prev = get().tailscaleLoginStates;
    const next = { ...prev };
    let changed = false;
    for (const [id, exists] of Object.entries(states)) {
      if (next[id] !== exists) {
        next[id] = exists;
        changed = true;
      }
    }
    if (changed) set({ tailscaleLoginStates: next });
  },

  /** 写入/清除某节点的交互登录 URL（null = 清除，如重新发起登录前抹掉上一轮的陈旧 URL）。 */
  setTailscaleAuthUrl: (serverId, url) => {
    if (!serverId) return;
    const prev = get().tailscaleAuthUrls;
    if ((prev[serverId] ?? null) === url) return;
    const next = { ...prev };
    if (url === null) delete next[serverId];
    else next[serverId] = url;
    set({ tailscaleAuthUrls: next });
  },

  /**
   * 落 STATUS 末帧（每帧全量覆盖该 serverId）。
   *
   * **与 [`setTailscaleLoginState`] 的分工**：那条是折叠登录态的**判决门**（过
   * `isDefinitiveTsLoginFrame`，非终局帧不许翻转、且双写 localStorage）；本条是**原始帧留存**，
   * 不过门、不写缓存 —— 出口告警要的正是「控制面这一刻怎么说」（`backendState` / `expired` /
   * `peers`），过一道折叠门反而把判据抹掉了。两条从同一个订阅各取所需。
   *
   * 逐字段比对早退（`backendState`/`expired`/`authURL` + peers 全等）：STATUS 是 ~1/s 的持续流，
   * 无脑 `set` 会让每个订阅者每秒重渲染一次。
   */
  setTailscaleStatus: (event) => {
    if (!event?.serverId) return;
    const prev = get().tailscaleStatuses;
    const old = prev[event.serverId];
    if (
      old &&
      old.backendState === event.backendState &&
      old.loggedIn === event.loggedIn &&
      old.expired === event.expired &&
      old.authURL === event.authURL &&
      old.canShareFiles === event.canShareFiles &&
      old.waitingFileCount === event.waitingFileCount &&
      old.receivingFileCount === event.receivingFileCount &&
      old.unreadFileCount === event.unreadFileCount &&
      JSON.stringify(old.details) === JSON.stringify(event.details) &&
      old.peers.length === event.peers.length &&
      old.peers.every((p, i) => {
        const n = event.peers[i];
        return (
          p.hostName === n.hostName &&
          p.ip === n.ip &&
          p.online === n.online &&
          p.exitNode === n.exitNode &&
          p.exitNodeOption === n.exitNodeOption
        );
      })
    ) {
      return;
    }
    set({ tailscaleStatuses: { ...prev, [event.serverId]: event } });
  },

  /** A successful native logout invalidates only that node's last main-core frame. */
  clearTailscaleStatus: (serverId) => {
    const prev = get().tailscaleStatuses;
    if (!(serverId in prev)) return;
    const next = { ...prev };
    delete next[serverId];
    set({ tailscaleStatuses: next });
  },

  /** 标记/清除「用户显式发起的登录在飞」。 */
  setTailscaleLoginInitiated: (serverId, initiated) => {
    if (!serverId) return;
    const prev = get().tailscaleLoginInitiated;
    if ((prev[serverId] ?? false) === initiated) return;
    const next = { ...prev };
    if (initiated) next[serverId] = true;
    else delete next[serverId];
    set({ tailscaleLoginInitiated: next });
  },

  retainServerIds: (serverIds) =>
    set((state) => {
      const keep = new Set(serverIds);
      const tailscaleLoginStates = retainRecordKeys(state.tailscaleLoginStates, keep);
      const tailscaleAuthUrls = retainRecordKeys(state.tailscaleAuthUrls, keep);
      const tailscaleLoginInitiated = retainRecordKeys(state.tailscaleLoginInitiated, keep);
      const tailscaleStatuses = retainRecordKeys(state.tailscaleStatuses, keep);
      const invalidNodes = retainItemsById(state.invalidNodes, keep);
      if (
        tailscaleLoginStates === state.tailscaleLoginStates &&
        tailscaleAuthUrls === state.tailscaleAuthUrls &&
        tailscaleLoginInitiated === state.tailscaleLoginInitiated &&
        tailscaleStatuses === state.tailscaleStatuses &&
        invalidNodes === state.invalidNodes
      ) {
        return state;
      }
      return {
        tailscaleLoginStates,
        tailscaleAuthUrls,
        tailscaleLoginInitiated,
        tailscaleStatuses,
        invalidNodes,
      };
    }),

  /** 置/清跨屏一次性意图。消费方读到后立刻 `setServerPageAction(null)`（见类型注释）。 */
  setServerPageAction: (action) => set({ serverPageAction: action }),

  /** 同上，首页版（消费点 HomeScreen 的高亮 effect）。 */

  reset: () => {
    invalidateLoadConfig('reset');
    set({
      config: null,
      configPublication: null,
      configLoading: false,
      proxyStatus: null,
      proxyStarting: false,
      proxyStopping: false,
      servers: [],
      selectedServerId: null,
      rules: [],
      dnsRules: [],
      trafficStats: null,
      ipInfo: null,
      // 不重置 ipInfoRevision：WebView 同一进程内清空显示态后，旧事件仍可能迟到。
      unlock: freshUnlock(),
      pendingChanges: EMPTY_PENDING,
      invalidNodes: [],
      helperStatus: null,
      privacyMode: true,
      // 原始帧只在核跑着时有意义（reset 语义 = 回到「什么都不知道」），清掉即回落到
      // 「无帧 → 出口告警不据陈旧帧下认证/离线结论」。
      tailscaleStatuses: {},
      // 一次性跨屏意图是瞬时态：reset 后没人再该被它弹窗/高亮。
      serverPageAction: null,
          // 登录中的瞬时态随 reset 清掉；tailscaleLoginStates 刻意不清——它是缓存/state 文件支撑的持久登录态，
      // 不随一次 app 状态重置而「变成未登录」。
      tailscaleAuthUrls: {},
      tailscaleLoginInitiated: {},
    });
    if (configSession) { refreshNeeded = true; pumpConfigRefresh(); }
  },
}));

// Includes mutation results, staged-store publications and external setState
// callers. Whole-config freshness cannot be inferred from selection context.
useAppStore.subscribe((state, previous) => {
  if (state.config === previous.config) return;
  if (state.configPublication === previous.configPublication || !state.config) configReadGeneration.published();
  // Staged-save and legacy authoritative publishers remain visible to hooks, but
  // cannot satisfy a settlement barrier with a pre-barrier snapshot.
  if (state.config && state.configPublication === previous.configPublication) {
    publishConfig(state.config, -1, 'external-publication');
  }
});

// ─────────────────────────── effectiveConfig（暂存回显的唯一读点）───────────────────────────

/**
 * 「用户现在应该看到的那份配置」= `replay(磁盘 config, staged 条目)`（spec §2.5 Q2 的 `effectiveConfig`）。
 *
 * # 为什么必须有这层，而不是让展示点直接读 `config`
 *
 * `config` 是**磁盘真值**。暂存层一开，用户改完节点、对话框关掉，列表读 `config` 就还是旧值 ——
 * 那不是「延迟生效」，是 UI 撒谎，与 §Q3 否决「切节点进暂存」的理由完全同型（列表高亮 A、
 * 状态栏显示 A，而流量走 B）。展示可编辑实体的读点一律经本层；**只有回答「磁盘上是什么」的读点**
 * （暂存基准 / 落盘入参 / 与后端对账）才继续读裸 `config`，判据表见 `lib/config-read-wiring.test.ts`。
 *
 * # 引用稳定性是硬要求，不是优化
 *
 * zustand 的 selector 按引用比较：每次返回一个新对象 = 全应用每次 store 变更都重渲染。故：
 *  - **条目为空 ⇒ 原样返回入参 `config` 本身**（不是深拷贝的等值对象）。`STAGED_CONFIG_ENABLED`
 *    为 `false` 时条目恒空 ⇒ 本层在产品行为与渲染开销上都是逐字节零变化。
 *  - 条目非空 ⇒ 单槽记忆化，**失效判据取入参的引用**（`config` / `entries` 任一换了才重算）。
 *    不拿 `JSON.stringify` 当 key：config 与 `config.json` 同量级，每次渲染序列化一遍比 `replay` 还贵；
 *    两个入参都由各自的 store 不可变替换（`set({config})` / `stageEntry` 恒建新数组），引用即是判据。
 *
 * `replay` 对畸形条目**跳过而非抛**（`lib/staged-config.ts` 的 `applyEntry`）—— 本函数在渲染路径上，
 * 抛 = 白屏，故包装层也不得引入新的抛点。
 */
/**
 * 每个 `(磁盘配置引用, 条目表引用)` 各自保留结果，而不是只记最后一对。
 *
 * 同一屏可以同时存在 app-store 的全局配置读点与 `useConfig` 的设置页局部副本；两者共享条目表、
 * 但磁盘配置对象引用不一定相同。单槽缓存会被两个 selector 交替击穿，使 Zustand 的 getSnapshot
 * 每次都拿到新对象并进入无限重渲染。双层 WeakMap 既保证多读点引用稳定，也不延长旧快照生命周期。
 */
const effectiveConfigMemo = new WeakMap<
  UserConfig,
  WeakMap<readonly StagedEntry[], UserConfig>
>();

export function effectiveConfigOf(
  config: UserConfig | null,
  entries: readonly StagedEntry[]
): UserConfig | null {
  if (config === null || entries.length === 0) return config;
  let byEntries = effectiveConfigMemo.get(config);
  if (!byEntries) {
    byEntries = new WeakMap();
    effectiveConfigMemo.set(config, byEntries);
  }
  const cached = byEntries.get(entries);
  if (cached) return cached;
  const result = replay(config, entries);
  byEntries.set(entries, result);
  return result;
}

const identity = (c: UserConfig | null): unknown => c;

/**
 * 订阅式读点。**默认取整份**（`useEffectiveConfig()`），传 selector 可保持今天的窄订阅粒度：
 * `useEffectiveConfig((c) => c?.uiTheme)` 与原来的 `useAppStore((s) => s.config?.uiTheme)` 一样
 * 只在该字段变化时重渲染 —— 改读本 hook **不放大**任何组件的重渲染面。
 *
 * 两个订阅缺一不可：条目变了要重渲（暂存的编辑要立刻回显），config 变了也要（磁盘侧被别处改了）。
 */
export function useEffectiveConfig<T = UserConfig | null>(
  select: (c: UserConfig | null) => T = identity as (c: UserConfig | null) => T
): T {
  const entries = useStagedConfigStore((s) => s.entries);
  return useAppStore((s) => select(effectiveConfigOf(s.config, entries)));
}

/** 命令式读点（事件回调 / 一次性快照）。与 `useEffectiveConfig` 同一记忆化槽。 */
export function getEffectiveConfig(): UserConfig | null {
  return effectiveConfigOf(
    useAppStore.getState().config,
    useStagedConfigStore.getState().entries
  );
}

// ───────────────────── 磁盘镜像集合的合成视图（列表回显）─────────────────────

/**
 * `servers` / `rules` 是 app-store 从 `config.servers` / `config.customRules` 抄出来的**扁平磁盘镜像**
 * （`loadConfig` / `saveConfig` 各抄一次）—— 节点页、规则页、首页出口选单真正渲染的是它们，
 * 不是 `config`。故只把 `config` 读点接上 `effectiveConfig` **不会**让列表回显 staged 编辑：
 * 那条腿仍在读盘。本层补的就是这一段。
 *
 * # 判据：同一集合上的两个不同问题（handoff 已裁定，勿重新选型）
 *
 * | 读点在问什么 | 读谁 |
 * |---|---|
 * | 「用户**现在编辑出来**的实体集合长什么样」（列表、编辑基准、下拉枚举） | effective |
 * | 「**后端此刻能按 id 找到 / 正在使用**的实体集合长什么样」（出口选单、切节点、按 id 下发的 IPC） | disk 镜像 |
 *
 * 后者读 effective 必错：staged-only 实体在盘上不存在，后端按 id 查不到 ⇒ 拒绝。
 * 前者读 disk 必错：用户看不见自己刚做的编辑 ⇒ UI 撒谎（与 spec §2.5 Q3 否决「切节点进暂存」同型）。
 * ⇒ staged-only 实体**在列表里可见且带「待保存」标记**（判据见 `stagedOnlyIds`），**不进出口选单**。
 * 逐读点的分类结果在 `lib/config-read-wiring.test.ts` 的 `MIRROR_SITES`。
 *
 * # 为什么重放到**镜像**上而不是取 `effectiveConfig.servers`
 *
 * `servers` 镜像恒等于 `config.servers`，两条路等价；但 `rules` 镜像**会与 `config.customRules` 分叉**
 * ——`RulesScreen` 的拖拽重排 / 开关切换是先写镜像的乐观更新（等一轮 IPC 会先弹回再跳过去）。
 * 取 `effectiveConfig.customRules` 会把那次乐观更新丢掉 ⇒ 拖完弹回原位，**且开关关着时也会发生**。
 * 重放到镜像上则两种情形都对：条目是「幂等的整体替换」，同一实体已被乐观写过再重放一次结果相同。
 *
 * # 引用稳定性是硬要求，不是优化（与 `effectiveConfigOf` 同一条）
 *
 * 本函数直接当 zustand selector 用：每次返回新数组 = 每次 store 变更都让整张列表重渲染，
 * 且 `useSyncExternalStore` 会因快照不稳定而抖动。故**每集合一个记忆化槽**，失效判据取入参引用
 * （镜像由 `set({servers})` 不可变替换、条目数组由 `stageEntry` 恒建新数组，引用即是判据）。
 *
 * 条目为空时早退返回镜像本体：`STAGED_CONFIG_ENABLED` 为 `false` 时条目恒空 ⇒ 本层不建任何对象、
 * 不占记忆化槽，在产品行为与渲染开销上都是逐字节零变化。
 * （注意：`replay` 自身对空条目集也是结构共享的 —— `{...baseline}` 换了外层对象，`.servers`
 * 仍是同一个数组。故早退**省的是那次分配**，引用稳定性两条路都成立。）
 */
interface CollectionMemo {
  mirror: unknown[] | null;
  entries: readonly StagedEntry[] | null;
  result: unknown[] | null;
}

type EffectiveCollectionKey = 'servers' | 'customRules' | 'trafficRules' | 'dnsRules';

const COLLECTION_MEMO: Record<EffectiveCollectionKey, CollectionMemo> = {
  servers: { mirror: null, entries: null, result: null },
  customRules: { mirror: null, entries: null, result: null },
  trafficRules: { mirror: null, entries: null, result: null },
  dnsRules: { mirror: null, entries: null, result: null },
};

export function effectiveCollection<T>(
  collection: EffectiveCollectionKey,
  mirror: T[],
  entries: readonly StagedEntry[]
): T[] {
  if (entries.length === 0) return mirror;
  const memo = COLLECTION_MEMO[collection];
  if (mirror === memo.mirror && entries === memo.entries) return memo.result as T[];
  const next = (replay({ [collection]: mirror }, entries) as Record<string, unknown>)[collection];
  // `replay` 对畸形条目跳过而非抛；万一某条把集合换成了非数组，退回镜像而不是让列表崩掉。
  const result = Array.isArray(next) ? (next as T[]) : mirror;
  memo.mirror = mirror;
  memo.entries = entries;
  memo.result = result;
  return result;
}

/** 节点**列表**读点（展示面）。出口选单 / 切节点 / 按 id 下发 IPC 的读点仍读 `s.servers`。 */
export function useEffectiveServers(): ServerConfig[] {
  const entries = useStagedConfigStore((s) => s.entries);
  return useAppStore((s) => effectiveCollection('servers', s.servers, entries));
}

/** 规则**列表**读点（展示面）。乐观更新自身的基准（`getState().rules`）仍读镜像。 */
export function useEffectiveRules(plane: 'route' | 'dns' = 'route'): Rule[] {
  const entries = useStagedConfigStore((s) => s.entries);
  return useAppStore((s) =>
    plane === 'dns'
      ? effectiveCollection('dnsRules', s.dnsRules, entries)
      : effectiveCollection('trafficRules', s.rules, entries),
  );
}

/**
 * 移动端**全局接线层**的门 —— 射程 = 「后端把事实推过来之后，界面读的那些 store 会不会动」。
 *
 * # 被守的缺陷（本批立项时的实测）
 *
 * 接线之前 `MobileApp` 只挂了 `useMobileConfigSync()`。于是：
 *  · `refreshProxyStatus` **全仓只有桌面 `App.tsx` 在调**，移动端零调用点 ⇒ 按下连接、后端真的起了核，
 *    首页永远停在「未连接」；测速按钮（`speedTestCaps.mainCorePool = proxyRunning`）随之恒置灰。
 *  · `tailscaleStatuses` / `tailscaleLoginStates` / `tailscaleAuthUrls` / `useVpnStatusStore.openVpn`
 *    四张表在移动端**没有任何写入方**，而首页与节点屏都在读它们。
 *  · `pendingChanges` 的 pull 与 push 两条腿一条都没挂 ⇒ 外壳那个 `.m-pending-slot` 恒空。
 *  · `setDesktopNotificationsEnabled` 没人调、`notifyDesktop` 在移动入口的静态 import 图上零消费点
 *    ⇒ 设置页那个通知开关写进 config 之后没有任何东西读它。
 *
 * # 哪一半能行为驱动、哪一半只能走源码
 *
 * 本仓 vitest 跑 `environment:'node'`、刻意不装 jsdom ⇒ `useEffect` 在 `renderToStaticMarkup` 下
 * **不执行**（同 `config-sync.test.tsx` 头注那张表）。故：
 *
 * | 半 | 手段 | 判据 |
 * |---|---|---|
 * | effect 体（水合 / 订阅 / 退订 / 轮询） | 喂假 IPC 直接跑 `startMobileAppWiring()` | ①–⑦ |
 * | 「`MobileApp` 真的挂了这条腿」 | 真渲染 + 对 hook 打桩（hook 在**渲染期**被调） | ⑧ |
 * | 对账 / 通知开关（依赖 React 输入的那几条） | 直接调导出的纯函数 | ⑨ |
 * | 「喂数腿在移动树里真有调用点」 | 源码扫描（`ui/src/mobile/**` 闭包，不是夹具自造） | ⑩ |
 * | 「hook / 组件体里**真的**建了那条订阅」 | AST：effect 回调的**作用域内**有那次调用 | ⑪ |
 * | 「桌面那批订阅一条都没漏判」 | 从 `App.tsx` 抓清单，与移动树 + 登记表对差 | ⑫ |
 *
 * ⑩ 与 ①–⑧ **必须成对**：前者证明「生产源码里那几个调用点在」，后者证明「调了之后 store 真的动」。
 * 只有前者会被「调用点在、参数喂错」骗过；只有后者会被「机制对、但生产代码绕开了它」骗过
 * （那正是本批之前的状态：`refreshProxyStatus` 这个函数一直是好的，只是移动端没人调）。
 *
 * 🔴 **⑪ 是 2026-09-06 三条复审同时点名的那条 blocker 的修复**：⑧ 把 hook 整个 mock 成了桩、
 * ①–⑦ 绕过 hook 直接调、⑩ 数的调用点全住在那个被删了调用方的函数体里 —— 三层判据首尾都在，
 * 中间「hook 体 → `startMobileAppWiring`」这一跳空着，把它删掉全门照绿而移动端一条订阅都不挂。
 */

import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { TailscaleStatusEvent } from '@/contracts/tailscale-status';
import type { TailscaleLoginProgress } from '@/domain/tailscale-login-progress';
import type { OpenVpnStatusEvent, VpnStatusSnapshot } from '@/contracts/vpn-status';
import type { InvalidNodeInfo, IpInfoSnapshot, ProxyStatus, UserConfig } from '@/contracts/types';
import type { UnlockSnapshot } from '@/contracts/unlock-detection';
import type { SubscriptionUpdateProgress } from '@/contracts/subscription-progress';
import type { SubscriptionCreateSnapshot } from '@/contracts/subscription-create-operation';
import type { TaildropTaskSnapshot } from '@/contracts/taildrop';
import * as ts from '@/test/ts-compiler';
import {
  CALL_SITE_FIXTURE,
  callSitesIn,
  isCallSiteLine,
  stripComments,
} from '@/test/call-sites.test-support';

/* ── 假后端 ────────────────────────────────────────────────────────────────── */

/** 每条事件通道的登记簿。退订**必须真的从这里移除** —— ⑦ 靠它区分「退订了」与「只是没发」。 */
type Bus = Map<string, Set<(payload: never) => void>>;
const bus: Bus = new Map();
const offSpy = vi.fn();

function channel<T>(name: string): (listener: (payload: T) => void) => () => void {
  return (listener) => {
    const set = bus.get(name) ?? new Set();
    bus.set(name, set);
    set.add(listener as (payload: never) => void);
    return () => {
      offSpy(name);
      set.delete(listener as (payload: never) => void);
    };
  };
}

function emit<T>(name: string, payload: T): void {
  for (const listener of [...(bus.get(name) ?? [])]) (listener as (p: T) => void)(payload);
}

let proxyStatus: ProxyStatus | null = null;
let pendingRaw: unknown = { added: [], modified: [], removed: [], restartDeferred: false };
let vpnSnapshot: VpnStatusSnapshot = { connected: false, openConnect: [], openVpn: [] };

const getStatusMock = vi.fn(async () => proxyStatus);
const getPendingMock = vi.fn(async () => pendingRaw);
const vpnGetStatusMock = vi.fn(async () => vpnSnapshot);
const tailscaleStateExistsMock = vi.fn(async (_ids: string[]) => ({}) as Record<string, boolean>);
const openExternalMock = vi.fn(async (_url: string) => undefined);
const cancelLoginMock = vi.fn(async (_serverId: string, _attemptId: string) => undefined);
/* ζ 批 A8 / A12 两条应用级腿的假后端。list / status 的返回由每条用例摆布。 */
let createListSeed: SubscriptionCreateSnapshot[] = [];
const createListMock = vi.fn(async () => createListSeed);
const createStatusMock = vi.fn(async (id: string) => {
  const hit = createListSeed.find((s) => s.operationId === id);
  if (!hit) throw new Error('not found');
  return hit;
});
let taildropSeed: TaildropTaskSnapshot[] = [];
const taildropTasksMock = vi.fn(async (_serverId?: string) => taildropSeed);
let privacyModeSeed = false;
const getPrivacyModeMock = vi.fn(async () => privacyModeSeed);
const setPrivacyModeMock = vi.fn(async (_value: boolean) => undefined);

const exitProbeMock = vi.fn(async () => ({direct:null,proxy:null,updatedAt:1,revision:1}));
const unlockCheckMock = vi.fn(async () => ({results:{},checkedAt:1,blockedReason:'not-running'}));
const unlockGetMock = vi.fn(async (): Promise<UnlockSnapshot | null> => null);
const ipPeekMock = vi.fn(async (): Promise<IpInfoSnapshot> => ({direct:null,proxy:null,updatedAt:0,revision:0}));

vi.mock('@/ipc', () => ({
  unlockApi: {
    run: () => unlockCheckMock(), get: () => unlockGetMock(),
    onProgress: channel('unlockProgress'), onUpdated: channel('unlockUpdated'),
    onInvalidated: channel('unlockInvalidated'),
  },
  api: {
    ipInfo: {get: () => exitProbeMock(), peek: () => ipPeekMock(), onUpdated: channel('ipInfo')},
    proxy: {
      getStatus: () => getStatusMock(),
      getPendingChanges: () => getPendingMock(),
      onStarted: channel<void>('started'),
      onStopped: channel<void>('stopped'),
      onLifecycle: channel<{ phase: string }>('lifecycle'),
      onPendingChanges: channel<unknown>('pending'),
      onError: channel<{ errorCode?: string }>('error'),
      onTailscaleStatus: channel<TailscaleStatusEvent>('tsStatus'),
      onTailscaleAuth: channel<{ url: string; serverId?: string; nodeName: string; transient?: boolean }>('tsAuth'),
      onTailscaleLoginProgress: channel<TailscaleLoginProgress>('tsLoginProgress'),
      onMeshLoginFallback: channel<{ engaged: boolean; serverName?: string }>('meshFallback'),
      onAutoNodeSwitched: channel<{ reason: string; newServerName: string; latency: number }>('autoSwitch'),
      onInvalidNodes: channel<InvalidNodeInfo[]>('invalidNodes'),
    },
    vpn: {
      getStatus: () => vpnGetStatusMock(),
      onOpenVpnStatus: channel<OpenVpnStatusEvent>('openVpn'),
    },
    subscription: {
      onUpdateProgress: channel<SubscriptionUpdateProgress>('subProgress'),
      onAutoUpdate: channel<{ success: boolean }>('subAuto'),
      // 真后端的这一条是**异步**登记（返回 Promise<off>）—— 假的也得是，否则「先 await 登记、再 list」
      // 那条顺序在这里量不出来。
      onCreateProgressReady: async (listener: (s: SubscriptionCreateSnapshot) => void) =>
        channel<SubscriptionCreateSnapshot>('createProgress')(listener),
      createList: () => createListMock(),
      createStatus: (id: string) => createStatusMock(id),
    },
    server: {
      tailscaleStateExists: (ids: string[]) => tailscaleStateExistsMock(ids),
      tailscaleLoginCancel: (serverId: string, attemptId: string) => cancelLoginMock(serverId, attemptId),
      onSpeedTestResult: channel<{ runId?: string; serverId: string; latency: number }>('speedTestResult'),
      onSpeedTestProgress: channel('speedTestProgress'),
      onSpeedTestDone: channel('speedTestDone'),
      onTaildropTaskUpdated: channel<TaildropTaskSnapshot>('taildropTask'),
      taildropTasks: (serverId?: string) => taildropTasksMock(serverId),
    },
    system: { openExternal: (url: string) => openExternalMock(url) },
    config: {
      onChanged: () => () => undefined,
      // 隐私锁（W-16）：首帧水合 + 进/出两条事件。水合的返回值由每条用例自己摆布
      // （`privacyModeSeed`），故 ⑧ 那组能验「锁着的时候重建 ⇒ 遮罩立刻回来」。
      getPrivacyMode: () => getPrivacyModeMock(),
      setPrivacyMode: (value: boolean) => setPrivacyModeMock(value),
      onEnterPrivacyMode: channel<void>('enterPrivacy'),
      onExitPrivacyMode: channel<void>('exitPrivacy'),
    },
  },
}));

const notifyDesktopMock = vi.fn(async (_t: string, _b: string) => undefined);
const setEnabledMock = vi.fn((_v: boolean | undefined) => undefined);
vi.mock('@/lib/desktop-notify', () => ({
  notifyDesktop: (t: string, b: string) => notifyDesktopMock(t, b),
  setDesktopNotificationsEnabled: (v: boolean | undefined) => setEnabledMock(v),
}));

/** `t()` 桩：把 key 原样返回（断言落在「取了哪个键」上，与语种文案解耦）。 */
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN', t: (k: string) => k } }),
}));

/** ⑧ 只对 hook 打桩，`startMobileAppWiring`（①–⑦ 直接跑的那个）保持真身。 */
const wiringHookSpy = vi.fn();
vi.mock('./app-wiring', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./app-wiring')>();
  return { ...actual, useMobileAppWiring: () => wiringHookSpy() };
});

import { renderToStaticMarkup } from 'react-dom/server';
import { setToastImpl, type ToastOptions } from '@/lib/error-handler';
import { useAppStore } from '@/store/app-store';
import { useLatencyStore } from '@/store/use-latency-store';
import { subscribeMobileSpeedTestProgress } from './use-mobile-speed-test';
import { useSubscriptionProgressStore } from '@/store/use-subscription-progress-store';
import { useTailscaleLoginCacheStore } from '@/store/use-tailscale-login-cache-store';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { useVpnStatusStore } from '@/store/use-vpn-status-store';
import { useSubscriptionCreateOperationStore } from '@/store/subscription-create-operation-store';
import { useTaildropTaskStore } from '@/store/use-taildrop-task-store';
import { useMobileFormStore } from './forms/form-store';
import {
  MOBILE_STATUS_POLL_MS,
  hydrateTailscaleStates,
  reconcileEntityCaches,
  startMobileAppWiring,
} from './app-wiring';
import { MobileApp } from './MobileApp';

const t = (key: string): string => key;

/** 收 toast 的假宿主（门面是模块级单例，注入一次即可）。 */
interface ToastCall {
  channel: 'success' | 'info' | 'warning' | 'error';
  msg: string;
}
let toasts: ToastCall[] = [];
setToastImpl({
  success: (msg: string) => void toasts.push({ channel: 'success', msg }),
  info: (msg: string) => void toasts.push({ channel: 'info', msg }),
  warning: (msg: string) => void toasts.push({ channel: 'warning', msg }),
  error: (msg: string, _d?: string, _o?: ToastOptions) =>
    void toasts.push({ channel: 'error', msg }),
});

const tsFrame = (over: Partial<TailscaleStatusEvent> = {}): TailscaleStatusEvent =>
  ({
    serverId: 'ts-1',
    loggedIn: true,
    backendState: 'Running',
    expired: false,
    authURL: '',
    tailscaleIPs: [],
    peers: [],
    canShareFiles: false,
    waitingFileCount: 0,
    receivingFileCount: 0,
    unreadFileCount: 0,
    ...over,
  }) as TailscaleStatusEvent;

/** 等到断言成立或超时 —— 首帧水合是异步的（`.then` 一跳）。 */
const settle = (): Promise<void> => new Promise((r) => setTimeout(r, 0));
const ipFrame = (revision: number, loading: boolean, updatedAt = 100): IpInfoSnapshot => ({
  direct: null, proxy: null, revision, updatedAt, loading,
});

beforeEach(() => {
  bus.clear();
  toasts = [];
  proxyStatus = null;
  pendingRaw = { added: [], modified: [], removed: [], restartDeferred: false };
  vpnSnapshot = { connected: false, openConnect: [], openVpn: [] };
  offSpy.mockClear();
  getStatusMock.mockClear();
  getPendingMock.mockClear();
  vpnGetStatusMock.mockClear();
  tailscaleStateExistsMock.mockClear();
  openExternalMock.mockClear();
  cancelLoginMock.mockClear();
  notifyDesktopMock.mockClear();
  setEnabledMock.mockClear();
  wiringHookSpy.mockClear();
  useAppStore.getState().reset();
  // Production reset preserves the backend's highest revision; each test instead
  // starts a new synthetic backend lifetime.
  useAppStore.setState({ ipInfoRevision: -1 });
  useVpnStatusStore.getState().replace(false, [], []);
  useSubscriptionProgressStore.setState({ progress: {} });
  useLatencyStore.setState({ latencyMap: {}, testedAt: {} });
  useTailscaleLoginCacheStore.setState({ cache: {} });
  useTailscaleLoginProgressStore.setState({ attempts: {} });
  createListSeed = [];
  createListMock.mockClear();
  createStatusMock.mockClear();
  taildropSeed = [];
  taildropTasksMock.mockClear();
  useSubscriptionCreateOperationStore.setState({ snapshots: {}, trackedOperationIds: [], handledTerminalRevisions: {} });
  useTaildropTaskStore.setState({ tasks: {} });
  useMobileFormStore.setState({ stack: [] });
  exitProbeMock.mockClear();
  unlockCheckMock.mockClear();
  unlockGetMock.mockClear();
  ipPeekMock.mockClear();
});

// ─────────────────────────────────────────────────────────────────────────────
describe('① 首帧水合：冷启动时核可能已经在跑，初值必须自取', () => {
  it('接线之前 store 是空的 —— 这就是今天移动端的状态', () => {
    expect(useAppStore.getState().proxyStatus).toBeNull();
  });

  it('跑一次 startMobileAppWiring：连接态 / 差集 / VPN 快照各拉一次', async () => {
    proxyStatus = { running: true, pid: 42 } as ProxyStatus;
    pendingRaw = { added: ['a'], modified: [], removed: [], restartDeferred: false };
    vpnSnapshot = { connected: true, openConnect: [], openVpn: [] };

    const off = startMobileAppWiring(t);
    await settle();

    // 正面等值，不是「非 null」：读回来的就是后端那一份。
    expect(useAppStore.getState().proxyStatus?.running).toBe(true);
    expect(useAppStore.getState().proxyStatus?.pid).toBe(42);
    expect(useAppStore.getState().pendingChanges.added).toEqual(['a']);
    expect(useVpnStatusStore.getState().connected).toBe(true);
    off();
  });

  it('回前台重拉连接态，通知权限关闭仍能看到原生重连原因', async () => {
    const listeners = new Map<string, EventListener>();
    const fakeDocument = {
      visibilityState: 'hidden',
      addEventListener: (name: string, listener: EventListener) => listeners.set(name, listener),
      removeEventListener: (name: string, listener: EventListener) => {
        if (listeners.get(name) === listener) listeners.delete(name);
      },
    };
    vi.stubGlobal('document', fakeDocument);
    try {
      const off = startMobileAppWiring(t);
      await settle();
      const before = getStatusMock.mock.calls.length;
      proxyStatus = { running: true, reconnectRequired: true } as ProxyStatus;
      fakeDocument.visibilityState = 'visible';
      listeners.get('visibilitychange')?.(new Event('visibilitychange'));
      await settle();
      expect(getStatusMock.mock.calls.length).toBeGreaterThan(before);
      expect(useAppStore.getState().proxyStatus?.reconnectRequired).toBe(true);
      off();
      expect(listeners.has('visibilitychange')).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('畸形差集载荷按空集降级、绝不抛（核未运行 / IPC 降级）', async () => {
    pendingRaw = {};
    const off = startMobileAppWiring(t);
    await settle();
    expect(useAppStore.getState().pendingChanges).toEqual({
      added: [],
      modified: [],
      removed: [],
      restartDeferred: false,
    });
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('② 连接态：三条事件腿都把 proxyStatus 重拉到真值', () => {
  it.each(['started', 'stopped', 'lifecycle'] as const)('%s → refreshProxyStatus', async (name) => {
    const off = startMobileAppWiring(t);
    await settle();
    const before = getStatusMock.mock.calls.length;

    // 后端换了真值：事件只是信号，权威源是 getStatus。
    proxyStatus = { running: true, pid: 7 } as ProxyStatus;
    emit(name, name === 'lifecycle' ? { phase: 'ready' } : undefined);
    await settle();

    expect(getStatusMock.mock.calls.length).toBeGreaterThan(before);
    expect(useAppStore.getState().proxyStatus?.pid).toBe(7);
    off();
  });

  it('历史缺陷回放：不接这条腿时，起核之后 store 一动不动', async () => {
    // 反向对照 —— 证明上面那三条量的是接线，不是「store 自己会变」。
    proxyStatus = { running: true, pid: 7 } as ProxyStatus;
    emit('started', undefined);
    await settle();
    expect(useAppStore.getState().proxyStatus).toBeNull();
  });

  it('差集：PUSH 帧写 store，stopped 顺带 pull 一次', async () => {
    const off = startMobileAppWiring(t);
    await settle();

    emit('pending', { added: ['x'], modified: ['y'], removed: [], restartDeferred: true });
    expect(useAppStore.getState().pendingChanges.modified).toEqual(['y']);
    expect(useAppStore.getState().pendingChanges.restartDeferred).toBe(true);

    pendingRaw = { added: [], modified: [], removed: [], restartDeferred: false };
    const before = getPendingMock.mock.calls.length;
    emit('stopped', undefined);
    await settle();
    expect(getPendingMock.mock.calls.length).toBeGreaterThan(before);
    expect(useAppStore.getState().pendingChanges.modified).toEqual([]);
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ②b 隐私态（W-16）—— 遮罩的**唯一**收敛点。
 *
 * 三条腿各修一个具体的洞：
 *  · 首帧水合：事件只推增量。锁着的时候 WebView 被系统回收重建（Android 上是常态），
 *    没有这一拍就会拿到一个**没有遮罩的解锁界面** —— 这把锁要挡的正是这种场景。
 *  · enter：闲置计时把后端的 `PRIVACY_MODE` 翻上去了，前端没人听 ⇒ 遮罩永远不出现。
 *  · exit：解锁成功后没人把遮罩收回去 ⇒ 密码对了界面也不解锁。
 *
 * 每条都带反向对照（不接这条腿时 store 一动不动），与 ② 组「历史缺陷回放」同一套写法。
 */
describe('②b 隐私态：水合 + 进/出两条事件都把 store 收敛到真值', () => {
  it('🔴 首帧水合：后端说「已锁」⇒ store 立刻是锁着的（WebView 重建那一档）', async () => {
    privacyModeSeed = true;
    const off = startMobileAppWiring(t);
    await settle();
    expect(
      useAppStore.getState().privacyMode,
      '重建之后拿到的是一个没有遮罩的解锁界面 —— 这把锁要挡的正是这种场景',
    ).toBe(true);
    off();
    privacyModeSeed = false;
  });

  it('enter / exit 两条事件各把 store 推到对应的那一档', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    expect(useAppStore.getState().privacyMode).toBe(false);
    emit('enterPrivacy', undefined);
    expect(useAppStore.getState().privacyMode, '进隐私态没人听 ⇒ 遮罩永远不出现').toBe(true);
    emit('exitPrivacy', undefined);
    expect(useAppStore.getState().privacyMode, '解锁之后没人收遮罩 ⇒ 密码对了界面也不解锁').toBe(
      false,
    );
    off();
  });

  it('反向对照：不接这条腿时，事件推不动 store（证明上面量的是接线）', () => {
    emit('enterPrivacy', undefined);
    expect(useAppStore.getState().privacyMode).toBe(false);
  });

  it('退订之后事件推不动（切窗/卸载不该留下一条会翻锁的订阅）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    off();
    emit('enterPrivacy', undefined);
    expect(useAppStore.getState().privacyMode).toBe(false);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('③ Tailscale：原始帧整帧落 store，折叠登录态过判决门', () => {
  it('definitive 帧：整帧写 tailscaleStatuses，且翻转登录态', async () => {
    const off = startMobileAppWiring(t);
    await settle();

    emit('tsStatus', tsFrame({ peers: [{ id: 'p1' }] as never }));
    expect(useAppStore.getState().tailscaleStatuses['ts-1']?.backendState).toBe('Running');
    // `peers` 必须留着：`deriveTsExitWarning` 的两条腿全靠它，丢了就在产品里永不可达。
    expect(useAppStore.getState().tailscaleStatuses['ts-1']?.peers).toHaveLength(1);
    expect(useAppStore.getState().tailscaleLoginStates['ts-1']).toBe(true);
    off();
  });

  it('启动过渡帧（Stopped + loggedIn:false）**不许**把已知的已登录翻成未登录', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('tsStatus', tsFrame());
    expect(useAppStore.getState().tailscaleLoginStates['ts-1']).toBe(true);

    emit('tsStatus', tsFrame({ loggedIn: false, backendState: 'Stopped' }));
    // 原始帧照写（告警要的是控制面这一刻怎么说）……
    expect(useAppStore.getState().tailscaleStatuses['ts-1']?.backendState).toBe('Stopped');
    // ……但登录态保留上一次已知值（写穿进 localStorage 缓存 = 下次冷启动显示「需登录」）。
    expect(useAppStore.getState().tailscaleLoginStates['ts-1']).toBe(true);

    // 正面对照：控制面明说凭据不能用时，判决门放行、登录态确实翻得动。
    emit('tsStatus', tsFrame({ loggedIn: false, backendState: 'NeedsLogin' }));
    expect(useAppStore.getState().tailscaleLoginStates['ts-1']).toBe(false);
    off();
  });

  it('登录 URL：落 store + 开外部浏览器 + 系统通知；同一 URL 不重复触发', async () => {
    const off = startMobileAppWiring(t);
    await settle();

    emit('tsAuth', { url: 'https://login.example/a', serverId: 'ts-1', nodeName: 'n' });
    expect(useAppStore.getState().tailscaleAuthUrls['ts-1']).toBe('https://login.example/a');
    expect(openExternalMock).toHaveBeenCalledTimes(1);
    expect(notifyDesktopMock).toHaveBeenCalledWith('notify.tsLogin.title', 'notify.tsLogin.body');

    emit('tsAuth', { url: 'https://login.example/a', serverId: 'ts-1', nodeName: 'n' });
    expect(openExternalMock).toHaveBeenCalledTimes(1);

    // URL 变了仍要送达（去重的是「同一个 URL」，不是「这个节点」）。
    emit('tsAuth', { url: 'https://login.example/b', serverId: 'ts-1', nodeName: 'n' });
    expect(openExternalMock).toHaveBeenCalledTimes(2);
    off();
  });

  it('窗口进度事件喂当前 attempt：URL 可在面板 store 读到；旧 attempt 和取消后的迟到 URL 均无效', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    const progress = useTailscaleLoginProgressStore.getState();
    progress.begin('ts-1', 'attempt-a');
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'attempt-a', phase: 'awaitingAuth', url: 'https://login.example/a' });
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.url).toBe('https://login.example/a');
    expect(useAppStore.getState().tailscaleAuthUrls['ts-1']).toBe('https://login.example/a');
    expect(openExternalMock).toHaveBeenCalledTimes(1);

    progress.begin('ts-1', 'attempt-b');
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'attempt-a', phase: 'awaitingAuth', url: 'https://login.example/late-a' });
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.attemptId).toBe('attempt-b');
    expect(openExternalMock).toHaveBeenCalledTimes(1);
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'attempt-b', phase: 'cancelled', url: null });
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'attempt-b', phase: 'awaitingAuth', url: 'https://login.example/late-b' });
    emit('tsAuth', { url: 'https://login.example/main-late', serverId: 'ts-1', nodeName: 'n' });
    expect(useAppStore.getState().tailscaleAuthUrls['ts-1']).toBeUndefined();
    expect(openExternalMock).toHaveBeenCalledTimes(1);
    off();
  });

  it('无效 URL 拒绝打开；全局 Running 和 AUTH 均不能升级请求，只接受绑定 producer 结果', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    const progress = useTailscaleLoginProgressStore.getState();
    progress.begin('ts-1', 'bad-url');
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'bad-url', phase: 'awaitingAuth', url: 'file:///secret' });
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.reason).toBe('invalidAuthUrl');
    expect(openExternalMock).not.toHaveBeenCalled();
    expect(cancelLoginMock).toHaveBeenCalledWith('ts-1', 'bad-url');

    progress.begin('ts-1', 'main-core');
    const binding = { serverId: 'ts-1', attemptId: 'main-core', mainGeneration: 8, identityEpoch: 'epoch-A' };
    emit('tsLoginProgress', { ...binding, phase: 'mainCore', url: null });
    emit('tsStatus', tsFrame({ backendState: 'Starting', loggedIn: true, authURL: 'https://login.example/main' }));
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.phase).toBe('mainCore');
    expect(openExternalMock).not.toHaveBeenCalled();
    emit('tsStatus', tsFrame({ backendState: 'Running', loggedIn: true, authURL: '' }));
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.phase).toBe('mainCore');
    emit('tsLoginProgress', { ...binding, phase: 'awaitingAuth', url: 'https://login.example/bound' });
    expect(openExternalMock).toHaveBeenCalledTimes(1);
    emit('tsLoginProgress', { ...binding, phase: 'authorized', url: null });
    expect(useTailscaleLoginProgressStore.getState().attempts['ts-1']?.phase).toBe('authorized');
    emit('tsAuth', { url: 'https://login.example/late', serverId: 'ts-1', nodeName: 'n' });
    expect(openExternalMock).toHaveBeenCalledTimes(1);
    off();
  });

  it('无效 URL 的原生取消失败经全局宿主提示安全文案，且不重新开放链接', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    cancelLoginMock.mockRejectedValueOnce(new Error('private native path and token'));
    useTailscaleLoginProgressStore.getState().begin('ts-1', 'cancel-fails');
    emit('tsLoginProgress', { serverId: 'ts-1', attemptId: 'cancel-fails', phase: 'awaitingAuth', url: 'file:///private' });
    await settle();
    expect(cancelLoginMock).toHaveBeenCalledWith('ts-1', 'cancel-fails');
    expect(toasts).toContainEqual({ channel: 'warning', msg: 'ts.loginCancelFailed' });
    expect(toasts.some(({ msg }) => msg.includes('private native path'))).toBe(false);
    expect(openExternalMock).not.toHaveBeenCalled();
    off();
  });

  it('挂载兜底：只查 state 目录，空节点集不打 IPC', async () => {
    hydrateTailscaleStates([]);
    expect(tailscaleStateExistsMock).not.toHaveBeenCalled();

    tailscaleStateExistsMock.mockResolvedValueOnce({ 'ts-9': true });
    hydrateTailscaleStates(['ts-9']);
    await settle();
    expect(useAppStore.getState().tailscaleLoginStates['ts-9']).toBe(true);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('④ OpenVPN 状态 → use-vpn-status-store（节点屏隧道健康读的就是它）', () => {
  it('帧到达即写 store', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('openVpn', { serverId: 'ovpn-1', state: 'CONNECTED', stateText: '已连接' });
    expect(useVpnStatusStore.getState().openVpn['ovpn-1']?.state).toBe('CONNECTED');
    off();
  });

  it('停核清空快照（留着旧帧会让「核已停」这一屏还画着上一秒的隧道健康）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('openVpn', { serverId: 'ovpn-1', state: 'CONNECTED', stateText: '已连接' });
    emit('stopped', undefined);
    await settle();
    expect(useVpnStatusStore.getState().openVpn).toEqual({});
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ④b **无效节点 → `store.invalidNodes`**（批 18 接上；W-23 的形状）。
 *
 * 这条腿在本批之前是「读点在、写入方一个都没有」：节点屏一直读 `invalidNodes` 算
 * `invalidNodeIndex`、把剔除理由渲染成常驻 `.mn-note`，而 `onInvalidNodes` 登记着不接 ⇒
 * 那一格**恒不出现**（不是坏了，是永远不会亮）。
 *
 * 本组是**行为**判据，不是源码级：effect 体是普通函数，喂假 IPC 能真跑。源码级那一半
 * （「登记表里没留僵尸」＋「订阅真的写了 store」）在 ⑫。
 */
describe('④b 无效节点 → store.invalidNodes（节点屏那条剔除理由读的就是它）', () => {
  it('帧到达即写 store', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('invalidNodes', [{ id: 's1', tag: 'HK03', reason: 'detour-cascade' }] as InvalidNodeInfo[]);
    expect(useAppStore.getState().invalidNodes.map((n) => n.id)).toEqual(['s1']);
    off();
  });

  it('空帧整表覆盖（节点改好了那一格必须灭，不能留着上一轮的红字）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('invalidNodes', [{ id: 's1', tag: 'HK03', reason: 'detour-cascade' }] as InvalidNodeInfo[]);
    emit('invalidNodes', [] as InvalidNodeInfo[]);
    expect(useAppStore.getState().invalidNodes).toEqual([]);
    off();
  });

  it('退订之后推不动（负面断言带正面对照）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('invalidNodes', [{ id: 's1', tag: 'HK03', reason: 'detour-cascade' }] as InvalidNodeInfo[]);
    expect(useAppStore.getState().invalidNodes).toHaveLength(1);
    off();
    emit('invalidNodes', [{ id: 's2', tag: 'JP01', reason: 'control-url-ip' }] as InvalidNodeInfo[]);
    expect(
      useAppStore.getState().invalidNodes.map((n) => n.id),
      '退订之后事件还推得动 —— 切窗/卸载会留下一条仍在写 store 的订阅',
    ).toEqual(['s1']);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑤ 订阅进度：后台 scheduler 那条腿没有任何前端调用点可挂，只能走事件', () => {
  it('过程帧写进 store，终态帧删键', async () => {
    const off = startMobileAppWiring(t);
    await settle();

    emit('subProgress', { subscriptionId: 'sub-1', phase: 'fetching' } as SubscriptionUpdateProgress);
    expect(useSubscriptionProgressStore.getState().progress['sub-1']?.phase).toBe('fetching');

    emit('subProgress', { subscriptionId: 'sub-1', phase: 'done' } as SubscriptionUpdateProgress);
    expect(useSubscriptionProgressStore.getState().progress['sub-1']).toBeUndefined();
    off();
  });

  it('自动更新结果 → 拉一次差集（它会改节点集，而 PUSH 与本事件异步并行）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    const before = getPendingMock.mock.calls.length;
    emit('subAuto', { success: false });
    await settle();
    expect(getPendingMock.mock.calls.length).toBeGreaterThan(before);
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑥ 代理错误 → 与桌面同一份路由表（toast + 系统通知 + 刷连接态）', () => {
  it('崩溃腿：刷连接态 + error toast + 系统通知', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    const before = getStatusMock.mock.calls.length;

    emit('error', { errorCode: 'PROCESS_EXITED' });
    await settle();

    expect(getStatusMock.mock.calls.length).toBeGreaterThan(before);
    expect(toasts[toasts.length - 1]?.channel).toBe('error');
    expect(notifyDesktopMock).toHaveBeenCalledWith('notify.proxyError.title', 'notify.proxyError.body');
    off();
  });

  it('出口误导腿：**不**刷连接态（活核不许被标成已停），只报警告', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    const before = getStatusMock.mock.calls.length;

    emit('error', { errorCode: 'EXIT_MISMATCH' });
    await settle();

    expect(getStatusMock.mock.calls.length).toBe(before);
    expect(toasts[toasts.length - 1]?.channel).toBe('warning');
    off();
  });

  it('反向对照：`STARTUP_FAILED` 刻意不路由（发起方自己会报，两处都报 = 弹两遍）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    toasts = [];
    notifyDesktopMock.mockClear();
    emit('error', { errorCode: 'STARTUP_FAILED' });
    await settle();
    expect(toasts).toEqual([]);
    expect(notifyDesktopMock).not.toHaveBeenCalled();
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑥.2 **后端自驱地改变了「流量走哪」，而界面上什么都没变**的两条腿。
 *
 * 两条都是 2026-09-06 复审 major 补接的：桌面 `App.tsx` 有、移动端既没移植也不在头注那张判定表里，
 * 而两条的开关都露在移动端设置页上、后端 emit 无任何 `#[cfg]` 门控 ⇒ 在 Android 上真会发。
 * 不接它们的产品表现，与 W-27 移植的那条 2026-07-20 事故同型：**语义变了、首页零呈现**。
 */
describe('⑥.2 登录期出口让位 / 自动换节点：后端改了出口，用户得知道', () => {
  it('让位生效 → 一条「已临时直连」（此刻流量是明文直连，首页却仍显示走着组网出口）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    toasts = [];
    emit('meshFallback', { engaged: true });
    expect(toasts).toEqual([{ channel: 'info', msg: 'nodes.meshLoginFallbackTitle' }]);
    off();
  });

  it('同出口就绪切回（带 serverName）→ 一条「已切回」', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    toasts = [];
    emit('meshFallback', { engaged: false, serverName: '东京-1' });
    expect(toasts).toEqual([{ channel: 'success', msg: 'nodes.meshLoginFallbackRestored' }]);
    off();
  });

  it('反向对照：切走出口的静默复位（engaged=false 且无 serverName）**不弹**', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    toasts = [];
    emit('meshFallback', { engaged: false });
    // 弹了就会误报「已切回 XXX」——用户根本没切回去。
    expect(toasts).toEqual([]);
    off();
  });

  it('自动换节点 → 重拉连接态 + 一条 toast（不然首页节点名会静默变成另一个）', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    toasts = [];
    const before = getStatusMock.mock.calls.length;
    emit('autoSwitch', { reason: 'unreachable', newServerName: '新加坡-2', latency: 88 });
    await settle();
    expect(getStatusMock.mock.calls.length, '没重拉连接态 ⇒ 出口名/国旗要等 30s 兜底才收敛').toBeGreaterThan(
      before,
    );
    expect(toasts).toEqual([{ channel: 'success', msg: 'home.autoSwitched' }]);
    // 刻意不发系统通知（同 `onAutoUpdate` 的口径：结果本身是常驻呈现，通知留给「不看就错过」的那类）。
    expect(notifyDesktopMock).not.toHaveBeenCalled();
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑦ 生命周期：兜底轮询在、退订之后一切归零', () => {
  afterEach(() => vi.useRealTimers());

  it('30s 兜底轮询确实在跑（事件面盖不住的边缘态的最后一道网）', async () => {
    vi.useFakeTimers();
    const off = startMobileAppWiring(t);
    await vi.advanceTimersByTimeAsync(0);
    const before = getStatusMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(MOBILE_STATUS_POLL_MS + 1);
    expect(getStatusMock.mock.calls.length).toBeGreaterThan(before);
    off();
  });

  it('退订前推得动、退订后推不动（负面断言带正面对照）', async () => {
    vi.useFakeTimers();
    const off = startMobileAppWiring(t);
    await vi.advanceTimersByTimeAsync(0);

    // 正面对照：同一条发射路径在退订**之前**确实改变了 store。
    emit('tsStatus', tsFrame({ serverId: 'ts-2' }));
    expect(useAppStore.getState().tailscaleStatuses['ts-2']).toBeDefined();

    off();
    expect(offSpy.mock.calls.length).toBeGreaterThanOrEqual(12);
    for (const [, set] of bus) expect(set.size).toBe(0);

    emit('tsStatus', tsFrame({ serverId: 'ts-3' }));
    expect(useAppStore.getState().tailscaleStatuses['ts-3']).toBeUndefined();

    // 定时器也要停：退订后再走 10 个周期，一次 IPC 都不许打。
    const before = getStatusMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(MOBILE_STATUS_POLL_MS * 10);
    expect(getStatusMock.mock.calls.length).toBe(before);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑧ 接线：MobileApp 真的挂了这条腿（渲染期可观测，不是源码扫描）', () => {
  it('渲染 MobileApp 恰好调一次 useMobileAppWiring', () => {
    expect(wiringHookSpy).not.toHaveBeenCalled();
    renderToStaticMarkup(<MobileApp />);
    expect(wiringHookSpy).toHaveBeenCalledTimes(1);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑨ 配置对账：删掉的实体，其派生缓存跟着走', () => {
  const config = (serverIds: string[], subIds: string[]): UserConfig =>
    ({
      servers: serverIds.map((id) => ({ id, name: id })),
      subscriptions: subIds.map((id) => ({ id, name: id })),
    }) as unknown as UserConfig;

  beforeEach(() => {
    useAppStore.setState({
      tailscaleLoginStates: { keep: true, gone: false },
      tailscaleAuthUrls: { keep: 'u', gone: 'u' },
    });
    useLatencyStore.setState({ latencyMap: { keep: 1, gone: 2 }, testedAt: { keep: 1, gone: 2 } });
    useSubscriptionProgressStore.setState({
      progress: {
        's-keep': { subscriptionId: 's-keep', phase: 'failed' } as SubscriptionUpdateProgress,
        's-gone': { subscriptionId: 's-gone', phase: 'failed' } as SubscriptionUpdateProgress,
      },
    });
  });

  it('config 到达后逐个 store 驱逐（长会话里这些表只增不减就是泄漏）', () => {
    reconcileEntityCaches(config(['keep'], ['s-keep']));
    expect(Object.keys(useAppStore.getState().tailscaleLoginStates)).toEqual(['keep']);
    expect(Object.keys(useLatencyStore.getState().latencyMap)).toEqual(['keep']);
    expect(Object.keys(useSubscriptionProgressStore.getState().progress)).toEqual(['s-keep']);
  });

  it('反向对照：`config === null` 是「尚未水合」，**一个键都不许清**', () => {
    reconcileEntityCaches(null);
    // 清了的话，冷启动秒显用的持久登录缓存会被误删（那份缓存是代理关着时唯一的登录态来源）。
    expect(Object.keys(useAppStore.getState().tailscaleLoginStates).sort()).toEqual(['gone', 'keep']);
    expect(Object.keys(useSubscriptionProgressStore.getState().progress).sort()).toEqual([
      's-gone',
      's-keep',
    ]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑩ **「store 喂数腿」门** —— 判据面直接钉 `ui/src/mobile/**`，不用夹具自造。
 *
 * 它回答的问题只有一个：**移动端这棵树里，有没有人在喂这几张表。**
 * 上面 ①–⑨ 全绿也答不了这个问题：它们直接调 `startMobileAppWiring(...)`，
 * 生产代码把整条腿删掉、只留一个没人调的导出，那九组一条都不会红（⑧ 只盖到 hook 挂没挂）。
 *
 * 🔴 **调用点判据不许把声明行算进去**（2026-09-06 复审 major，已修）。上一版的正则是
 * `(?<![\w$])name\s*\(`，它对 `export function foo(` 与 `void foo(` 给出同一个答案。于是
 * 「把 `startMobileAppWiring` 的唯一调用点删掉」这个**恰好是本批要修的原缺陷**的变异，仍能靠
 * 定义那一行把本组喂绿。口径连同自检一起搬到 `@/test/call-sites.test-support`，两个消费面共用一份。
 */
describe('⑩ 喂数腿在移动树里真有调用点（源码面，与上面的行为面成对）', () => {
  const MOBILE_DIR = fileURLToPath(new URL('.', import.meta.url));
  const UI_SRC = fileURLToPath(new URL('..', import.meta.url));

  const walk = (dir: string, out: string[] = []): string[] => {
    for (const entry of readdirSync(dir).sort()) {
      const full = join(dir, entry);
      if (statSync(full).isDirectory()) walk(full, out);
      // 🔴 `IS_TEST_ONLY_MODULE`，不是就地的 `/\.(test|spec)\./`（2026-09-06 实测的一次**假绿**）：
      // 四屏对差登记表 `*.test-support.ts` 里写着 `subscribeSubscriptionProgressEvents()` 这样的**锚文本**，
      // 旧口径把它当成了移动树里的一个调用点 —— 于是把 `app-wiring.ts` 那条订阅整行删掉，
      // ⑩ 与 ⑫ 照样全绿。判据被别人的判据喂饱，和被自己的注释喂饱是同一种病。
      else if (/\.tsx?$/.test(full) && !IS_TEST_ONLY_MODULE.test(full)) out.push(full);
    }
    return out;
  };

  const FILES = walk(MOBILE_DIR);
  const SOURCES = new Map(FILES.map((f) => [relative(UI_SRC, f), stripComments(readFileSync(f, 'utf8'))]));

  /** 「谁调了 `name(`」——返回相对路径 + 行号，红的时候人能直接跳过去。 */
  const callSites = (name: string): string[] => callSitesIn(SOURCES, name);

  it('⓪ 取材面自检：文件数、行数、以及「注释 / 声明行都不算调用点」', () => {
    expect(FILES.length, '移动树的生产源码文件数掉了一大截 —— 取材面塌了').toBeGreaterThan(30);
    const lines = [...SOURCES.values()].reduce((n, s) => n + s.split('\n').length, 0);
    expect(lines, '扫到的行数掉了一大截').toBeGreaterThan(5000);
    // 共用谓词的正反自检（含「声明行不算」那四对）。夹具住在被共用的那份实现旁边。
    for (const [name, line, expected] of CALL_SITE_FIXTURE) {
      expect(isCallSiteLine(name, line), `${name} @ ${line}`).toBe(expected);
    }
    // 剥注释真的把注释里的同款形态挡在外面（本仓正是这么被喂绿过），且**保行号**。
    expect(stripComments('/* refreshProxyStatus() */\nconst x = 1;')).not.toContain(
      'refreshProxyStatus',
    );
    expect(stripComments('/* a\nb */\nvoid f();').split('\n')).toHaveLength(3);
  });

  it.each([
    ['refreshProxyStatus', '首页连接态 / 测速按钮可用性的唯一喂数腿'],
    ['setTailscaleStatus', '组网卡角标与 TS 出口警示的原始帧'],
    ['subscribeSubscriptionProgressEvents', '节点屏分组标签上的订阅更新进度与失败详情'],
    ['setToastImpl', '移动端 toast 宿主的注入（缺它 24 处反馈全部落 console）'],
    ['startMobileAppWiring', '上面那批订阅的**唯一**生产调用点（缺它整层接线一条都不挂）'],
    ['installMobileToastHost', 'toast 宿主组件里的注入调用（缺它宿主挂着也不注入）'],
    ['reconcileEntityCaches', '派生缓存的所有权对账（缺它四张表在长会话里只增不减）'],
    ['hydrateTailscaleStates', 'TS 登录态的挂载兜底（缺它代理关着时只剩 localStorage 一条来源）'],
  ])('`%s` 在移动树里至少有一个调用点（%s）', (name) => {
    const sites = callSites(name);
    expect(
      sites,
      `\`${name}\` 在 ui/src/mobile/** 里零调用点（**声明行不算**）—— 读它的那张表恒为初值，界面会说假话`,
    ).not.toEqual([]);
  });

  it('反向对照：一个并不存在的喂数腿必须报零（证明上面那条不是恒真）', () => {
    expect(callSites('refreshProxyStatusThatNeverExisted')).toEqual([]);
  });

  it('反向对照：只有声明、没有调用点的符号必须报零（证明剔声明那一步真的在生效）', () => {
    // `WiringT` 那种类型别名不算；这里要的是一个**函数声明在移动树里、而全树无人调用**的名字。
    // 取材面上今天没有这种符号 ⇒ 用合成源码喂同一条谓词，与上面 ⓪ 的夹具同一口径。
    const synthetic = new Map([
      ['mobile/synthetic.ts', 'export function onlyDeclared(x: number): number {\n  return x;\n}\n'],
    ]);
    expect(callSitesIn(synthetic, 'onlyDeclared')).toEqual([]);
    // 正面对照：同一份源码加一行调用就认得出（否则上面那条空数组可能只是谓词永远匹不到）。
    const withCall = new Map([...synthetic, ['mobile/caller.ts', 'void onlyDeclared(1);\n']]);
    expect(callSitesIn(withCall, 'onlyDeclared')).toEqual(['mobile/caller.ts:1']);
  });

  it('通知总开关也有人调（否则设置页那个开关写完没有任何消费方）', () => {
    expect(callSites('setDesktopNotificationsEnabled')).not.toEqual([]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑪ **挂载面门** —— 「hook / 组件体里**真的**建了那条订阅」。
 *
 * # 它补的是 ⑧ 与 ⑩ 之间那条缝
 *
 * 2026-09-06 三条复审同时点名的 blocker：本批**唯一**的承重接缝
 * （`useMobileAppWiring` 体内那条 `useEffect(() => startMobileAppWiring(...), [])`）
 * 一道门都没有，三层判据首尾都在、中间那一跳空着：
 *
 *  · ⑧ 把 `useMobileAppWiring` 整个 `vi.mock` 成 spy ⇒ 证明的是「`MobileApp` 调了一个桩」；
 *  · ①–⑦ 直接调 `startMobileAppWiring(t)`，跟 hook 挂没挂无关；
 *  · ⑩ 数的是 `refreshProxyStatus(` 等符号出现在哪些**行**，而那些行全在 `startMobileAppWiring`
 *    的函数体内部 —— 把调用它的那条 effect 删掉，调用点一个不少。
 *
 * 实测（复审的 M-A）：把那条 effect 换成 `useEffect(() => { void i18nRef.current; }, [])`，
 * `tsc` rc=0、全量 vitest 4159 通过 0 红、`report-wiring.sh` 逐字不变 —— 而生产上移动端一条
 * app 级订阅都不挂，正是本批立项要修的那个缺陷原样复发。`MobileToaster` 那一侧同形（复审 M-B）。
 *
 * # 为什么走 AST 而不是「文件里含这个字符串」
 *
 * 「函数体**内部**有没有这次调用」是作用域问题，行级正则答不了：`installMobileToastHost` 的定义
 * 与调用同在一个文件里，按文件文本匹配的判据对「组件不再调它」完全不可见（那正是 M-B 的形态）。
 * 本仓已有先例（`write-failure-visibility.test.ts` 的 `localFns` / `collectNodes`），照抄。
 *
 * # 为什么连 `useEffect` 一起钉
 *
 * 订阅必须建在 effect 里：写在渲染期等于每渲染一次就重订一整批（React 18 的并发渲染还可能
 * 建了再丢）。故判据是「effect 的回调里调了它」，不是「函数体里的某处调了它」。
 * 每条都配 `cleanup` 断言：effect 的回调必须返回退订闭包 —— 订阅建了不退是另一种泄漏，
 * 而 ⑦ 只证明 `startMobileAppWiring` 返回的那个闭包好使，证不到「生产把它当 cleanup 交回去了」。
 */
describe('⑪ 挂载面：hook / 组件体里真的建了那条订阅（AST，作用域内）', () => {
  const collectNodes = (root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] => {
    const out: ts.Node[] = [];
    const walk = (n: ts.Node): void => {
      if (pred(n)) out.push(n);
      ts.forEachChild(n, walk);
    };
    walk(root);
    return out;
  };

  const calleeNameOf = (call: ts.CallExpression): string | null => {
    const e = call.expression;
    if (ts.isIdentifier(e)) return e.text;
    if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
    return null;
  };

  const callsTo = (root: ts.Node, name: string): ts.CallExpression[] =>
    (collectNodes(root, ts.isCallExpression) as ts.CallExpression[]).filter(
      (c) => calleeNameOf(c) === name,
    );

  /** 顶层函数（`function f(){}` 与 `const f = (…) => …` 两形态，同 `write-failure-visibility`）。 */
  const bodyOf = (sf: ts.SourceFile, name: string): ts.Node | null => {
    for (const n of collectNodes(sf, ts.isFunctionDeclaration) as ts.FunctionDeclaration[]) {
      if (n.name?.text === name) return n;
    }
    for (const d of collectNodes(sf, ts.isVariableDeclaration) as ts.VariableDeclaration[]) {
      if (!ts.isIdentifier(d.name) || d.name.text !== name || !d.initializer) continue;
      if ((collectNodes(d.initializer, ts.isArrowFunction) as ts.Node[]).length > 0) return d.initializer;
    }
    return null;
  };

  /** effect 的回调**返回**了东西（简写箭头体，或块体里有 `return`）—— 即退订闭包交回去了。 */
  const returnsCleanup = (callback: ts.Node): boolean => {
    if (ts.isArrowFunction(callback) && !ts.isBlock(callback.body)) return true;
    return collectNodes(callback, ts.isReturnStatement).some(
      (r) => (r as ts.ReturnStatement).expression !== undefined,
    );
  };

  /** `fn` 体内某个 `useEffect(…)` 的回调里调了 `callee`；`cleanup` 要求那个回调返回退订闭包。 */
  const effectInstalls = (
    sf: ts.SourceFile,
    fn: string,
    callee: string,
    opts: { cleanup: boolean } = { cleanup: false },
  ): boolean => {
    const body = bodyOf(sf, fn);
    if (body === null) return false;
    return callsTo(body, 'useEffect').some((effect) => {
      const callback = effect.arguments[0];
      if (callback === undefined) return false;
      if (callsTo(callback, callee).length === 0) return false;
      return !opts.cleanup || returnsCleanup(callback);
    });
  };

  const parse = (file: string): ts.SourceFile => {
    const abs = fileURLToPath(new URL(file, import.meta.url));
    return ts.parseSourceFile(abs, readFileSync(abs, 'utf8'));
  };

  const synthetic = (name: string, text: string): ts.SourceFile => ts.parseSourceFile(name, text);

  /* ── 挂载面的第二形态：**组件返回的 JSX 里有没有那一层** ────────────────────────
   *
   * 上面 `effectInstalls` 只答得了「effect 里调没调某个函数」。批 2 的总承重接缝是另一种形状：
   * `<MobileFormHost />` 是不是**真的挂在** `MobileApp` 的返回值里 —— 它不是一次调用，是一个
   * JSX 元素，`callsTo` 看不见它。
   *
   * 判据必须带「**不在任何条件下**」这一半：实测把它改成 `{false && <MobileFormHost />}`
   * ⇒ import 仍被使用（`noUnusedLocals` 不报）、`forms.css` 仍进包（`css-oracle` 的进包顺序
   * 快照仍绿）、`tsc` rc=0、全量 vitest 0 红、`report-wiring.sh` 逐字不变，而生产上批 2 的
   * 全部产出（添加四项 / 编辑 / 克隆 / 删除确认 / 订阅更多五项 / TS 出口选择器）点下去零反应
   * —— 比陈先生截图那一屏更糟：从「灰着且有理由」退化成「亮着但没反应」，账本还宣称已接线。
   *
   * 「有没有被条件守着」用**自顶向下**判，不读 `node.parent`：进入三元的两个分支、逻辑
   * `&&` / `||` / `??` 的右侧、以及任何嵌套函数体（那是回调，不是本次渲染的产物）就置守卫位。
   */
  const tagOf = (name: ts.Node): string =>
    ts.isIdentifier(name) ? name.text : ts.isPropertyAccessExpression(name) ? tagOf(name.name) : '';

  /** `root` 子树里**无条件**渲染出来的 JSX 标签名。 */
  const unconditionalTags = (root: ts.Node): Set<string> => {
    const out = new Set<string>();
    const walk = (n: ts.Node, guarded: boolean): void => {
      if (ts.isConditionalExpression(n)) {
        walk(n.condition, guarded); // 条件本身照常走（它里面也可能有 JSX）
        walk(n.whenTrue, true);
        walk(n.whenFalse, true);
        return;
      }
      if (
        ts.isBinaryExpression(n) &&
        (n.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken ||
          n.operatorToken.kind === ts.SyntaxKind.BarBarToken ||
          n.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken)
      ) {
        walk(n.left, guarded);
        walk(n.right, true);
        return;
      }
      if (!guarded) {
        if (ts.isJsxSelfClosingElement(n)) out.add(tagOf(n.tagName));
        else if (ts.isJsxElement(n)) out.add(tagOf(n.openingElement.tagName));
      }
      /* 嵌套函数体 = 回调 / 渲染 prop：那里面的 JSX 不是本次渲染无条件产出的。 */
      const inner =
        guarded || ts.isArrowFunction(n) || ts.isFunctionExpression(n) || ts.isFunctionDeclaration(n);
      ts.forEachChild(n, (c) => walk(c, inner));
    };
    walk(root, false);
    return out;
  };

  /** 顶层组件 `fn` 的返回 JSX 里，`tag` 无条件在场。 */
  const mountsUnconditionally = (sf: ts.SourceFile, fn: string, tag: string): boolean => {
    const body = bodyOf(sf, fn);
    if (body === null) return false;
    const returns = collectNodes(body, ts.isReturnStatement) as ts.ReturnStatement[];
    return returns.some(
      (r) => r.expression !== undefined && unconditionalTags(r.expression).has(tag),
    );
  };

  it('⓪ 谓词自检：正反四例都要判对（否则下面每条都是空话）', () => {
    const wired = `
function useX(t: string) {
  useEffect(() => startWiring(t), []);
}
`;
    expect(effectInstalls(synthetic('syn-wired.ts', wired), 'useX', 'startWiring')).toBe(true);

    // ① effect 还在、调用没了 —— 复审 M-A / M-B 的**精确形态**。
    const gutted = `
function useX(t: string) {
  useEffect(() => {
    void t;
  }, []);
}
`;
    expect(effectInstalls(synthetic('syn-gutted.ts', gutted), 'useX', 'startWiring')).toBe(false);

    // ② 调用还在、但在渲染期（不在 effect 里）—— 每渲染一次重订一整批，同样不算接上。
    const inRender = `
function useX(t: string) {
  startWiring(t);
}
`;
    expect(effectInstalls(synthetic('syn-render.ts', inRender), 'useX', 'startWiring')).toBe(false);

    // ③ 函数被整个改名/删掉 ⇒ 判否，不是「找不到就跳过」。
    expect(effectInstalls(synthetic('syn-wired2.ts', wired), 'useGone', 'startWiring')).toBe(false);

    // ④ cleanup 那一档：块体里不 return 退订闭包 ⇒ 判否；return 了 ⇒ 判是。
    const noCleanup = `
function useX(t: string) {
  useEffect(() => {
    startWiring(t);
  }, []);
}
`;
    const withCleanup = `
function useX(t: string) {
  useEffect(() => {
    const off = startWiring(t);
    return () => off();
  }, []);
}
`;
    expect(
      effectInstalls(synthetic('syn-nocleanup.ts', noCleanup), 'useX', 'startWiring', {
        cleanup: true,
      }),
    ).toBe(false);
    expect(
      effectInstalls(synthetic('syn-cleanup.ts', withCleanup), 'useX', 'startWiring', {
        cleanup: true,
      }),
    ).toBe(true);
  });

  it('`useMobileAppWiring` 的 effect 里真的调了 `startMobileAppWiring`，且退订闭包交回去了', () => {
    const sf = parse('./app-wiring.ts');
    expect(
      effectInstalls(sf, 'useMobileAppWiring', 'startMobileAppWiring', { cleanup: true }),
      'hook 体里那条 `useEffect(() => startMobileAppWiring(...), [])` 没了 —— ' +
        '移动端一条 app 级订阅都不挂：按下连接、后端真起了核，首页恒「未连接」、测速按钮恒置灰、' +
        '`.m-pending-slot` 恒空、组网角标恒「未登录」、`onError` 无人接。' +
        '而 ⑧（hook 被 mock 成桩）与 ⑩（调用点都在那个没人调的函数体里）都看不见这件事',
    ).toBe(true);
  });

  it('`useMobileAppWiring` 在应用生命周期内订阅 RTT 事件并交还退订闭包', () => {
    expect(
      effectInstalls(parse('./app-wiring.ts'), 'useMobileAppWiring', 'subscribeMobileSpeedTestProgress', { cleanup: true }),
      '后台 warm RTT 只有事件结果；若订阅退回首页，切页后自动测速结果会丢失',
    ).toBe(true);
  });

  it.each([
    ['reconcileEntityCaches', '派生缓存对账：缺它 latency / TS 登录 / VPN / 订阅进度四张表只增不减'],
    ['setDesktopNotificationsEnabled', '通知总开关：缺它设置页那个开关写进 config 后没有消费方'],
    ['hydrateTailscaleStates', 'TS 登录态挂载兜底：缺它代理关着时只剩 localStorage 一条来源'],
  ])('`useMobileAppWiring` 的 effect 里真的调了 `%s`（%s）', (callee) => {
    expect(effectInstalls(parse('./app-wiring.ts'), 'useMobileAppWiring', callee)).toBe(true);
  });

  it('`MobileToaster` 的 effect 里真的调了 `installMobileToastHost`，且恢复腿交回去了', () => {
    const sf = parse('./MobileToaster.tsx');
    expect(
      effectInstalls(sf, 'MobileToaster', 'installMobileToastHost', { cleanup: true }),
      '宿主组件不再注入门面 —— 它照旧被摆在停靠区里（`mobile-chrome` ④ 全绿），' +
        '但 `RulesScreen` 那 24 处 `toast.*` 与 `app-wiring` 的 `reportIfFails` 全部退回 console。' +
        '`half-truth-facts` C 组那条「静音 → 0」的棘轮按文件文本匹配，看不见这一档',
    ).toBe(true);
  });

  it('⓪-b 谓词自检：三种「挂了但等于没挂」的形态都要判否，正面形态判是', () => {
    const mounted = `
function App() {
  return (
    <>
      <Shell />
      <FormHost />
    </>
  );
}
`;
    expect(mountsUnconditionally(synthetic('syn-mount.tsx', mounted), 'App', 'FormHost')).toBe(true);
    // ① 写死短路 —— 复审实测的**精确形态**（import 仍被使用，tsc / 全量 vitest 全绿）。
    const dead = `
function App() {
  return (
    <>
      <Shell />
      {false && <FormHost />}
    </>
  );
}
`;
    expect(mountsUnconditionally(synthetic('syn-dead.tsx', dead), 'App', 'FormHost')).toBe(false);
    // ② 挂在一个条件上 —— 表单层是全局宿主，它自己按栈空返回 null，不该由外面再守一道。
    const cond = `
function App() {
  return <>{ready ? <FormHost /> : null}</>;
}
`;
    expect(mountsUnconditionally(synthetic('syn-cond.tsx', cond), 'App', 'FormHost')).toBe(false);
    // ③ 整条删掉 / 组件改名 ⇒ 判否，不是「找不到就跳过」。
    expect(mountsUnconditionally(synthetic('syn-mount2.tsx', mounted), 'App', 'Gone')).toBe(false);
    expect(mountsUnconditionally(synthetic('syn-mount3.tsx', mounted), 'NoSuchFn', 'FormHost')).toBe(
      false,
    );
    // ④ 只出现在回调里（渲染 prop / 事件闭包）不算挂上了。
    const inCallback = `
function App() {
  return <Shell render={() => <FormHost />} />;
}
`;
    expect(mountsUnconditionally(synthetic('syn-cb.tsx', inCallback), 'App', 'FormHost')).toBe(false);
  });

  it('🔴 `MobileApp` 的返回 JSX 里**无条件**挂着 `<MobileFormHost />`（批 2 的总承重接缝）', () => {
    expect(
      mountsUnconditionally(parse('./MobileApp.tsx'), 'MobileApp', 'MobileFormHost'),
      '表单宿主没挂（或被一个条件守住了）—— 批 2 的全部产出（添加四项 / 编辑 / 克隆 / ' +
        '删除确认 / 订阅更多五项 / TS 出口选择器）点下去零反应，比截图那一屏更糟：' +
        '从「灰着且有理由」退化成「亮着但没反应」。`form-host.test.ts` 只测 store 纯语义与 ' +
        'union↔case 恰等，`back-navigation` / `nodes-screen` 读的都是 `forms/**` 与 `nodes/**` 的源码，' +
        '没有任何一条判据看得见 `MobileApp` 返回了什么',
    ).toBe(true);
  });

  it.each([
    ['onStarted', '「应用中…」在起核落地后收场'],
    ['onError', '重启失败时把原因写进条上（而不是永久转圈）'],
    ['onLifecycle', '「立即应用」走的是后端自驱去抖重启，那条路径 started/error 一个都不发'],
    ['applyStateOnLifecycle', '收场判定复用桌面那份纯逻辑（含「failed 带原因 / stopped 不判失败」）'],
  ])('`MobilePendingBar` 的 effect 里真的订了 `%s`（%s）', (callee) => {
    expect(effectInstalls(parse('./MobilePendingBar.tsx'), 'MobilePendingBar', callee)).toBe(true);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑫ **桌面订阅覆盖面对差** —— `app-wiring.ts` 头注那张「桌面 ~25 条 effect 逐条判定」表由门维护。
 *
 * 2026-09-06 复审 major：那张表自称覆盖桌面全部 ~25 条，实测**漏了两条**
 * （`onMeshLoginFallback` / `onAutoNodeSwitched`）—— 既没移植，也不在表里，而两条的开关都露在
 * 移动端设置页上、后端 emit 无 `#[cfg]` 门控。漏的原因不是判错，是**没人把两张清单对过差**。
 *
 * 判据：从桌面 `App.tsx` 抓出全部订阅形态的调用，逐个问「移动树里有没有调用点」。
 * 有 ⇒ 已接（不必登记）；没有 ⇒ **必须**在下面这张表里带理由登记。两边求并集必须恰等于桌面那一份，
 * 少一个（新订阅没判）与多一个（登记表里留了僵尸）都红。
 *
 * # 2026-09-06 第二次扩面：**订阅形态之外的 app 级腿**
 *
 * 上一版的取材面只有 `api.<域>.on<X>(` 与 `subscribe<X>(` 两种形态，而 `app-wiring.ts` 头注那张
 * 「桌面 ~25 条 effect 逐条判定」表里有**四条根本不长这个样子**：`useSystemProxyLivePolling()` /
 * `initTooltips()` / `useIdlePrivacyLock()` / 界面语言水合。它们在表里被人判过，
 * 但**没有任何门在数它们** —— 桌面再加一条这种形态的 app 级腿，⑫ 一声不吭。
 *
 * 补上的第二个取材面是**结构性**的、不是又一张措辞白名单：
 * 「`App.tsx` 从**仓内**（`@/…` / 相对路径）import 进来、并且**真的调用了**的每一个符号」。
 * 它按构造覆盖任何形态 —— hook、`init*`、`subscribe*`、`hydrate*`、自造门面全在内，
 * 而 React 内建（从 `'react'` import）与纯类型 import 天然在外。
 *
 * # 射程自曝
 *
 *  · 订阅面认两种形态：`api.<域>.on<X>(` 与 `subscribe<X>(`。桌面若把订阅**内联**成
 *    `window.__TAURI__.event.listen(...)` 这种不经门面的写法，订阅面看不见 —— 但只要它是
 *    从仓内 import 进来调用的，第二个面就抓得到。两个面都抓不到的只剩「在 `App.tsx` 里就地
 *    定义、就地调用、不经任何 import」这一种，⓪ 带数量下限自检兜住整体塌陷。
 *  · 「移动树里有调用点」用的是 ⑩ 那份共用谓词（声明行不算）。它证明源码里写了这次订阅，
 *    「订阅真的建起来了」由 ⑪ 与 ①–⑦ 分别守。
 *  · 本门**不问**「移动端读的那格状态有没有人写」—— 一条被登记成「不接」的订阅，只要没人注意到
 *    有屏在读它写的那格，本门照绿。那一格由 `screen-parity.test.ts` ⑤ 的喂数腿面守
 *    （取材面在移动端这一侧，方向与本门相反）。`onInvalidNodes` 就是这么被抓到的 ——
 *    它在本门的登记表里躺了一版（理由还写着「没人读」这句假话），2026-09-13（批 18）接上，
 *    登记随之删掉，正面事实断言见本组最后那条 it。
 */
describe('⑫ 桌面那批 app 级订阅：要么移植了，要么在表里登记了不移植的理由', () => {
  const MOBILE_DIR = fileURLToPath(new URL('.', import.meta.url));
  const UI_SRC = fileURLToPath(new URL('..', import.meta.url));

  const walk = (dir: string, out: string[] = []): string[] => {
    for (const entry of readdirSync(dir).sort()) {
      const full = join(dir, entry);
      if (statSync(full).isDirectory()) walk(full, out);
      // 🔴 `IS_TEST_ONLY_MODULE`，不是就地的 `/\.(test|spec)\./`（2026-09-06 实测的一次**假绿**）：
      // 四屏对差登记表 `*.test-support.ts` 里写着 `subscribeSubscriptionProgressEvents()` 这样的**锚文本**，
      // 旧口径把它当成了移动树里的一个调用点 —— 于是把 `app-wiring.ts` 那条订阅整行删掉，
      // ⑩ 与 ⑫ 照样全绿。判据被别人的判据喂饱，和被自己的注释喂饱是同一种病。
      else if (/\.tsx?$/.test(full) && !IS_TEST_ONLY_MODULE.test(full)) out.push(full);
    }
    return out;
  };
  const MOBILE_SOURCES = new Map(
    walk(MOBILE_DIR).map((f) => [relative(UI_SRC, f), stripComments(readFileSync(f, 'utf8'))]),
  );

  const APP_TSX = stripComments(readFileSync(join(UI_SRC, 'App.tsx'), 'utf8'));

  /** 桌面 `App.tsx` 上的订阅形态：聚合门面的 `on<X>(` 与具名的 `subscribe<X>(`。 */
  const desktopSubscriptions = (source: string): string[] => {
    const names = new Set<string>();
    for (const m of source.matchAll(/\bapi\s*\.\s*[a-zA-Z]+\s*\.\s*(on[A-Z]\w*)\s*\(/g)) names.add(m[1]);
    for (const m of source.matchAll(/(?<![\w$.])(subscribe[A-Z]\w*)\s*\(/g)) names.add(m[1]);
    return [...names].sort();
  };

  /**
   * 第二个取材面：`App.tsx` 从**仓内**（`@/…` / 相对路径）import 进来、并且真的调用了的符号。
   * 按构造覆盖 hook / `init*` / `hydrate*` / 自造门面等一切形态；React 内建与纯类型 import 天然在外
   * （前者不是从仓内 import，后者带 `import type`）。
   */
  const desktopAppLegs = (source: string): string[] => {
    const imported = new Set<string>();
    for (const m of source.matchAll(/import\s+(type\s+)?\{([^}]*)\}\s+from\s+'([^']+)'/g)) {
      if (m[1] || !/^[@.]/.test(m[3])) continue;
      for (const part of m[2].split(',')) {
        const name = part.trim().split(/\s+as\s+/).pop()!.trim();
        if (/^[A-Za-z_$][\w$]*$/.test(name)) imported.add(name);
      }
    }
    for (const m of source.matchAll(/import\s+([A-Za-z_$][\w$]*)\s+from\s+'([^']+)'/g)) {
      if (/^[@.]/.test(m[2])) imported.add(m[1]);
    }
    return [...imported]
      .filter((name) => new RegExp(`(?<![\\w$.])${name}\\s*\\(`).test(source))
      .sort();
  };

  /**
   * **不移植**的登记表：`[名字, 理由]`。理由必须点名「为什么移动端接了也没有消费者 / 没有对象」，
   * 不许写「本批不做」——那是排期，不是判定。
   */
  const NOT_PORTED: ReadonlyArray<readonly [string, string]> = [
    ['subscribeLatencyEvents', '移动端由 subscribeMobileSpeedTestProgress 同时接主动轮次与被动 warm RTT：旧 runId 不落延迟；原桌面无轮次门订阅不重复挂。应用级实际 RTT/退订行为见本文件末尾'],
    ['onOpenConnectStatus', 'OpenConnect 增量在移动端零消费者：节点屏的 `deriveMeshTunnelHealth` 只读 `openVpn`，首帧快照那一跳已经把 `openConnect` 落进 store 了'],
    ['onSystemProxyResidual', '移动端接管方式恒为 TUN，系统代理这个对象在 Android 上不存在'],
    ['subscribeSpeedTestProgressToast', '移动端通过runId会话store与行内进度消费同样事件；不重复显示桌面的sticky toast'],
    // ── 2026-09-06 第二次扩面抓出来的七条：它们不长成订阅的样子，此前整条掉在取材面外 ──
    ['useSystemProxyLivePolling', '移动端接管方式恒为 TUN（`MobileHomeScreen.MOBILE_TAKEOVER`），systemProxy 档在 Android 上连入站都不发；那条轮询每轮 exec `networksetup`/`gsettings`/`reg`，在这里既无对象也无消费者'],
    ['useIdlePrivacyLock', '隐私锁在移动端不存在：`privacyMode` 在 Android 上三个生产端（托盘「立即锁定」/ idle 计时 / 本窗解锁）一个都不在移动入口上。设置页 `privacyLockAbsent` 那句是它的用户可见面'],
    ['initTooltips', '引擎是 `data-tip` 的 hover 委托，触屏没有 hover。移动端按 IA §4.12 把桌面的 `data-tip` 一律落成常驻行，全树零 `data-tip` ⇒ 挂上它连一个委托目标都没有'],
    ['createOnceGate', '它是 `initTooltips` / 隐私态水合那几条 effect 的一次性闸门工具，那几条本身都不接 ⇒ 闸门没有可闸的对象。它不是一条能力腿，是它们的实现细节'],
    ['openTrackedSubscriptionCreateRecovery', '它往**桌面** `dialog-store` 的栈里压一个 `sub-create-task` 弹窗，移动端没有那个宿主。同一条判据（按 kind + operationId 去重、只为本机追踪的操作开）落在移动端自己的栈上：`forms/form-store.ts#openMobileSubscriptionCreateRecovery`，开的是 `SubCreateTaskPanel`（2026-09-25 ζ 批 A8）'],
  ];

  const DESKTOP_SUBSCRIPTIONS = desktopSubscriptions(APP_TSX);
  const DESKTOP_LEGS = desktopAppLegs(APP_TSX);
  const DESKTOP = [...new Set([...DESKTOP_SUBSCRIPTIONS, ...DESKTOP_LEGS])].sort();
  const ported = DESKTOP.filter((name) => callSitesIn(MOBILE_SOURCES, name).length > 0);
  const registered = NOT_PORTED.map(([name]) => name);

  it('⓪ 取材面自检：两个桌面清单都抓得到量，且谓词分得清「订阅 / app 级腿」与「别的调用」', () => {
    expect(
      DESKTOP_SUBSCRIPTIONS.length,
      '从 App.tsx 只抓到这么几条订阅 —— 订阅面塌了，下面的对差恒绿',
    ).toBeGreaterThan(20);
    expect(
      DESKTOP_LEGS.length,
      '从 App.tsx 只抓到这么几条仓内 import 且被调用的符号 —— 第二个取材面塌了',
    ).toBeGreaterThan(15);
    // 第二个面必须真的**多带出**一批订阅面看不见的腿（否则它是一条恒等的摆设）。
    const legsOnly = DESKTOP_LEGS.filter((n) => !DESKTOP_SUBSCRIPTIONS.includes(n));
    expect(
      legsOnly,
      '第二个取材面没带出任何订阅面之外的腿 —— 它没在干活',
    ).toEqual(expect.arrayContaining(['initTooltips', 'useIdlePrivacyLock', 'useSystemProxyLivePolling']));
    // 正反对照：仓内 import 且调用的认得出；React 内建（非仓内）与纯类型 import 不认。
    expect(desktopAppLegs("import { initTooltips } from '@/lib/tooltip-engine';\ninitTooltips();")).toEqual([
      'initTooltips',
    ]);
    expect(desktopAppLegs("import { useEffect } from 'react';\nuseEffect(() => {});")).toEqual([]);
    expect(desktopAppLegs("import type { UserConfig } from '@/contracts/types';\nUserConfig();")).toEqual([]);
    expect(desktopAppLegs("import { neverCalled } from '@/lib/x';\nconst a = neverCalled;")).toEqual([]);
    // 正反对照：两种形态都认得出，且不误吞普通调用。
    expect(desktopSubscriptions('const off = api.proxy.onStarted(() => {});')).toEqual(['onStarted']);
    expect(desktopSubscriptions('useEffect(() => subscribeLatencyEvents(), []);')).toEqual([
      'subscribeLatencyEvents',
    ]);
    expect(desktopSubscriptions('void api.proxy.getStatus();')).toEqual([]);
    // `onStarted` 这条已知移植过的腿必须落在「已接」那一侧，否则整条对差是在读空集。
    expect(ported).toContain('onStarted');
  });

  it('每一条桌面订阅要么在移动树里有调用点，要么在登记表里带理由', () => {
    const unjudged = DESKTOP.filter((n) => !ported.includes(n) && !registered.includes(n));
    expect(
      unjudged,
      '桌面这些 app 级订阅在移动端**既没接、也没登记**。逐条判：接上它，或补一行到 NOT_PORTED\n' +
        '（2026-09-06 就是这么漏掉 onMeshLoginFallback / onAutoNodeSwitched 两条的）：\n' +
        unjudged.join('\n'),
    ).toEqual([]);
  });

  it('登记表里不留僵尸：登记为「不接」的，移动树里就不许有调用点', () => {
    const contradicting = registered.filter((n) => ported.includes(n));
    expect(
      contradicting,
      '这些名字登记着「不接」，移动树里却有调用点 —— 要么接上了忘了删登记，要么名字撞车了：\n' +
        contradicting.join('\n'),
    ).toEqual([]);
    const stale = registered.filter((n) => !DESKTOP.includes(n));
    expect(stale, `桌面上已经没有这些订阅了，登记表该删：\n${stale.join('\n')}`).toEqual([]);
  });

  it('每条不移植的理由都不是「本批不做」（排期不是判定）', () => {
    for (const [name, why] of NOT_PORTED) {
      expect(why.length, `${name} 的理由太短，说不清射程`).toBeGreaterThan(15);
      expect(why, `${name} 的理由写的是排期，不是判定`).not.toMatch(/本批|下一批|以后|排期/);
    }
  });

  it('两条 2026-09-06 补接的腿真的在（漏掉它们正是这道门存在的理由）', () => {
    expect(callSitesIn(MOBILE_SOURCES, 'onMeshLoginFallback')).not.toEqual([]);
    expect(callSitesIn(MOBILE_SOURCES, 'onAutoNodeSwitched')).not.toEqual([]);
  });

  /*
   * 🔴 隐私态那两条**从登记表里搬到了「已接」那一侧**（W-16）。
   *
   * 它们此前登记着「不接」，理由是「`privacyMode` 在 Android 上没有生产端 ⇒ 接了恒收不到帧」。
   * 那条判定的射程只到「订阅桌面发的隐私态事件」这一格，当时是对的；W-16 让移动端**自己**
   * 产生这个状态（闲置计时 + 遮罩的解锁腿），生产端于是有了。
   *
   * 单独一条断言而不是只靠上面那条「不留僵尸」：僵尸那条只保证「登记表里没有它们」，
   * 而**两条订阅真的挂上了**这件事要正面说一次 —— 少了它，把订阅删掉再把登记删掉，
   * 那条僵尸断言照样绿（两边一起没了），而遮罩从此永远不出现。
   */
  it('🔴 隐私态进/出两条订阅真的挂上了（遮罩的唯一收敛点）', () => {
    expect(
      callSitesIn(MOBILE_SOURCES, 'onEnterPrivacyMode'),
      '闲置计时把后端的 `PRIVACY_MODE` 翻上去了，而前端没人听 ⇒ 遮罩永远不出现',
    ).not.toEqual([]);
    expect(
      callSitesIn(MOBILE_SOURCES, 'onExitPrivacyMode'),
      '解锁成功后没人把遮罩收回去 ⇒ 密码对了界面也不解锁',
    ).not.toEqual([]);
    // 首帧水合那一拍同样要在：事件只推增量，「锁着的时候 WebView 被系统回收重建」这一档靠它。
    expect(
      callSitesIn(MOBILE_SOURCES, 'getPrivacyMode'),
      '缺首帧水合 ⇒ 锁定期间 WebView 重建后拿到的是一个没有遮罩的解锁界面',
    ).not.toEqual([]);
  });

  /*
   * 🔴 无效节点那条**从登记表里搬到了「已接」那一侧**（批 18）。
   *
   * 同隐私态那两条，单独一条断言而不是只靠上面那条「不留僵尸」：僵尸那条只保证「登记表里没有它」，
   * 把订阅删掉再把登记删掉，它照样绿（两边一起没了），而「这个节点已失效」从此永远不会亮。
   *
   * 两句都要：**订阅挂上了**、**它真的往 store 里写**。只断言前一句的话，把回调体掏空
   * （`onInvalidNodes(() => {})`）能让这条恒绿，而那与没接一模一样。
   */
  it('🔴 无效节点那条订阅真的挂上了，且真的往 store 里写', () => {
    expect(
      callSitesIn(MOBILE_SOURCES, 'onInvalidNodes'),
      '节点屏读 `invalidNodes` 算 `invalidNodeIndex`、渲染成常驻 `.mn-note`，而没有任何写入方 ⇒ 那一格恒不出现',
    ).not.toEqual([]);
    expect(
      callSitesIn(MOBILE_SOURCES, 'setInvalidNodes'),
      '订阅挂上了却没落 store —— 与没接无从区分',
    ).not.toEqual([]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑬ ζ 批两条「曾经登记为不接」的应用级腿（2026-09-25）：订阅创建操作水合 + 恢复面（A8）、
 * Taildrop 发件任务事件（A12）。
 *
 * 两条原来的理由形状相同 ——「唯一的消费者是一张短命面板，面板活着时自持进度」。代价是：
 *  · A8：进程被回收 / WebView 重建之后，那次创建仍在后端跑（或已跑完），而移动端找不回来；
 *  · A12：发件进行中进度不会自己跳，面板关掉再打开也只是再拉一次当时的快照。
 * 每条都带反向对照（不跑接线时同一帧推不动 store / 不开恢复面），与 ② 组同一套写法。
 */
describe('⑬ 订阅创建恢复面 + Taildrop 发件进度：应用级订阅真的挂上、真的往 store 写', () => {
  const createSnap = (over: Partial<SubscriptionCreateSnapshot> = {}): SubscriptionCreateSnapshot => ({
    operationId: 'op-1',
    revision: 1,
    phase: 'fetching',
    terminal: false,
    ...over,
  });
  const taskSnap = (over: Partial<TaildropTaskSnapshot> = {}): TaildropTaskSnapshot =>
    ({
      taskId: 'task-1',
      serverId: 'ts-1',
      peerStableId: 'peer',
      phase: 'sending',
      files: [],
      sentBytes: 10,
      totalBytes: 100,
      revision: 1,
      updatedAtMs: 1,
      ...over,
    }) as TaildropTaskSnapshot;
  const recoveryOps = (): string[] =>
    useMobileFormStore
      .getState()
      .stack.flatMap((e) => (e.kind === 'sub-create-task' ? [e.operationId] : []));

  it('A8 冷启动水合：本机追踪着的在跑操作 ⇒ 开一层恢复面；list 里只有历史终态的 ⇒ 不开', async () => {
    useSubscriptionCreateOperationStore.setState({ trackedOperationIds: ['op-1'] });
    createListSeed = [
      createSnap(),
      // 别的渲染器 / 更早的历史：后端有界 list 会带回来，但它不是本机发起的 ⇒ 不许重播。
      createSnap({ operationId: 'op-old', phase: 'succeeded', terminal: true, revision: 9 }),
    ];
    const off = startMobileAppWiring(t);
    await settle();
    await settle();
    expect(recoveryOps(), '进程被回收后那次创建还在后端跑，移动端却找不回来').toEqual(['op-1']);
    expect(useSubscriptionCreateOperationStore.getState().snapshots['op-1']?.phase).toBe('fetching');
    off();
  });

  it('A8 事件帧落 store（恢复面读的就是它），退订之后推不动', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    await settle();
    emit('createProgress', createSnap({ revision: 2, phase: 'parsing' }));
    expect(useSubscriptionCreateOperationStore.getState().snapshots['op-1']?.phase).toBe('parsing');
    off();
    emit('createProgress', createSnap({ revision: 3, phase: 'committing' }));
    expect(useSubscriptionCreateOperationStore.getState().snapshots['op-1']?.phase).toBe('parsing');
  });

  it('A8 反向对照：不跑接线时，同样的追踪 + list 一层恢复面都不开、事件也推不动', async () => {
    useSubscriptionCreateOperationStore.setState({ trackedOperationIds: ['op-1'] });
    createListSeed = [createSnap()];
    emit('createProgress', createSnap({ revision: 2, phase: 'parsing' }));
    await settle();
    expect(recoveryOps()).toEqual([]);
    expect(useSubscriptionCreateOperationStore.getState().snapshots['op-1']).toBeUndefined();
  });

  it('A12 发件任务：先挂事件再水合；事件逐帧推进同一条任务', async () => {
    taildropSeed = [taskSnap()];
    const off = startMobileAppWiring(t);
    await settle();
    expect(taildropTasksMock, '冷启动 / WebView 重建后没有水合 ⇒ 面板打开前在跑的任务看不见').toHaveBeenCalled();
    expect(useTaildropTaskStore.getState().tasks['task-1']?.sentBytes).toBe(10);
    emit('taildropTask', taskSnap({ revision: 2, sentBytes: 60, updatedAtMs: 2 }));
    expect(
      useTaildropTaskStore.getState().tasks['task-1']?.sentBytes,
      '发件进行中进度不会自己跳 —— 这正是 A12 的缺陷形状',
    ).toBe(60);
    off();
    emit('taildropTask', taskSnap({ revision: 3, sentBytes: 90, updatedAtMs: 3 }));
    expect(useTaildropTaskStore.getState().tasks['task-1']?.sentBytes).toBe(60);
  });

  it('A12 反向对照：不跑接线时，事件推不动 store（证明上面量的是接线）', () => {
    emit('taildropTask', taskSnap({ revision: 2, sentBytes: 60 }));
    expect(useTaildropTaskStore.getState().tasks['task-1']).toBeUndefined();
  });
});

describe('连接后检测由 Rust 排程，应用级事件跨页接收', () => {
  it('后台 warm RTT 在切屏后仍落全局 store，应用卸载后监听器确实退订', () => {
    const off = subscribeMobileSpeedTestProgress();
    emit('speedTestResult', { serverId: 'n-hk', latency: 38 });
    expect(useLatencyStore.getState().latencyMap['n-hk']).toBe(38);
    // Switching from Home to Nodes does not unmount the app-level subscription.
    emit('speedTestResult', { serverId: 'n-jp', latency: 73 });
    expect(useLatencyStore.getState().latencyMap).toMatchObject({ 'n-hk': 38, 'n-jp': 73 });
    off();
    emit('speedTestResult', { serverId: 'n-sg', latency: 91 });
    expect(useLatencyStore.getState().latencyMap['n-sg']).toBeUndefined();
    expect(offSpy).toHaveBeenCalledWith('speedTestResult');
  });

  it('失效后显示检测中，progress 与终态落库；连接变化不额外 force', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    useAppStore.getState().setProxyStatus({ running: true, startTime: 100 });
    emit('unlockInvalidated', { running: true, exitBlocked: false });
    expect(useAppStore.getState().unlock.running).toBe(true);
    emit('ipInfo', { direct: null, proxy: null, updatedAt: 101, revision: 1, loading: true });
    expect(useAppStore.getState().ipInfo?.loading).toBe(true);
    emit('unlockProgress', { serviceId: 'netflix', result: { status: 'ok' } });
    expect(useAppStore.getState().unlock.results.netflix?.status).toBe('ok');
    emit('unlockUpdated', { results: { netflix: { status: 'ok' } }, checkedAt: 102, egress: null });
    emit('ipInfo', { direct: null, proxy: null, updatedAt: 103, revision: 2, loading: false });
    expect(useAppStore.getState().unlock.running).toBe(false);
    expect(useAppStore.getState().unlock.checkedAt).toBe(102);
    expect(useAppStore.getState().ipInfo?.updatedAt).toBe(103);
    useAppStore.getState().setProxyStatus({ running: true, startTime: 100, uptime: 2 });
    useAppStore.getState().setProxyStatus({ running: false });
    useAppStore.getState().setProxyStatus({ running: true, startTime: 200 });
    expect(exitProbeMock).not.toHaveBeenCalled();
    expect(unlockCheckMock).not.toHaveBeenCalled();
    emit('unlockInvalidated', { running: false, exitBlocked: false });
    expect(useAppStore.getState().unlock.checkedAt).toBeNull();
    off();
    emit('ipInfo', { direct: null, proxy: null, updatedAt: 200, revision: 3, loading: false });
    expect(useAppStore.getState().ipInfo?.updatedAt).toBe(103);
  });

  it.each(['progress', 'updated', 'invalidated'] as const)(
    '迟到的 unlock:get 冷水合不能覆盖 %s 实时事件', async (kind) => {
      let resolveGet!: (snapshot: UnlockSnapshot | null) => void;
      unlockGetMock.mockImplementationOnce(() => new Promise((resolve) => { resolveGet = resolve; }));
      const off = startMobileAppWiring(t);
      if (kind === 'progress') {
        emit('unlockProgress', { serviceId: 'netflix', result: { status: 'ok' } });
      } else if (kind === 'updated') {
        emit('unlockUpdated', { results: { netflix: { status: 'ok' } }, checkedAt: 200, egress: null });
      } else {
        emit('unlockInvalidated', { running: false, exitBlocked: false });
      }
      const before = useAppStore.getState().unlock;
      resolveGet({ results: { netflix: { status: 'blocked' } }, checkedAt: 100, egress: null });
      await settle();
      expect(useAppStore.getState().unlock).toEqual(before);
      off();
    },
  );

  it('旧 IP peek 即使带更新的时间戳也不能覆盖停止事件', async () => {
    let resolvePeek!: (snapshot: IpInfoSnapshot) => void;
    ipPeekMock.mockImplementationOnce(() => new Promise((resolve) => { resolvePeek = resolve; }));
    const off = startMobileAppWiring(t);
    const stopped: IpInfoSnapshot = { direct: null, proxy: null, updatedAt: 100, revision: 2, loading: false };
    emit('ipInfo', stopped);
    resolvePeek({ direct: null, proxy: null, updatedAt: 101, revision: 1, loading: true });
    await settle();
    expect(useAppStore.getState().ipInfo).toEqual(stopped);
    off();
  });

  it('同毫秒事件按后端版本前进；清空后仍拒绝旧帧和同版回填', async () => {
    const off = startMobileAppWiring(t);
    await settle();
    emit('ipInfo', ipFrame(2, true, 100));
    emit('ipInfo', ipFrame(3, false, 100));
    const settled = useAppStore.getState().ipInfo;
    emit('ipInfo', ipFrame(2, true, 900));
    emit('ipInfo', ipFrame(3, true, 901));
    expect(useAppStore.getState().ipInfo).toBe(settled);
    expect(useAppStore.getState().ipInfoRevision).toBe(3);

    useAppStore.getState().reset();
    expect(useAppStore.getState().ipInfo).toBeNull();
    expect(useAppStore.getState().ipInfoRevision).toBe(3);
    emit('ipInfo', ipFrame(2, true, 902));
    emit('ipInfo', ipFrame(3, false, 903));
    expect(useAppStore.getState().ipInfo).toBeNull();

    emit('ipInfo', ipFrame(4, false, 100)); // stop
    emit('ipInfo', ipFrame(5, true, 100)); // restart pending, same wall-clock millisecond
    emit('ipInfo', ipFrame(4, false, 904)); // late stop delivery
    expect(useAppStore.getState().ipInfo).toEqual(ipFrame(5, true, 100));
    off();
  });

  it('手动检测的迟到返回不能覆盖其间收到的自动检测事件', async () => {
    let resolveManual!: (snapshot: IpInfoSnapshot) => void;
    const manual = new Promise<IpInfoSnapshot>((resolve) => { resolveManual = resolve; })
      .then((snapshot) => useAppStore.getState().setIpInfo(snapshot));
    const off = startMobileAppWiring(t);
    await settle();
    emit('ipInfo', ipFrame(8, false, 100));
    resolveManual(ipFrame(7, true, 900));
    await manual;
    expect(useAppStore.getState().ipInfo).toEqual(ipFrame(8, false, 100));
    off();
  });

  it('初始 revision 0 可接收，畸形版本不会污染水位', () => {
    useAppStore.getState().setIpInfo(ipFrame(0, false));
    expect(useAppStore.getState().ipInfoRevision).toBe(0);
    for (const revision of [undefined, -1, 0.5, Number.MAX_SAFE_INTEGER + 1, NaN]) {
      useAppStore.getState().setIpInfo({
        ...ipFrame(1, true), revision,
      } as unknown as IpInfoSnapshot);
      expect(useAppStore.getState().ipInfo).toEqual(ipFrame(0, false));
      expect(useAppStore.getState().ipInfoRevision).toBe(0);
    }
    useAppStore.getState().setIpInfo(ipFrame(1, true));
    expect(useAppStore.getState().ipInfoRevision).toBe(1);
  });
});

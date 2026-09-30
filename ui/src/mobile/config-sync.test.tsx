/**
 * 移动端配置水合腿的门 —— 射程 = 「盘上的值能不能走到首页那颗芯片上」这整条链。
 *
 * 被守的缺陷（真机三重实测复现过）：`config.json` 里 `proxyMode: "global"`，移动端首页芯片显示
 * `Smart`；`force-stop` 冷启动后仍是 `Smart`；而**同一时刻**设置页显示正确。根因见
 * `config-sync.ts` 头注——两份配置副本里 app-store 那份在移动端既没首拉、也没订广播。
 *
 * ── 哪一半能行为驱动、哪一半只能走源码 ──────────────────────────────────────
 * 本仓 vitest 跑 `environment: 'node'`、刻意不装 jsdom（见 `vite.config.ts` test 段），
 * `useEffect` 在 `renderToStaticMarkup` 下**不执行**。故：
 *
 * | 半 | 手段 | 判据 |
 * |---|---|---|
 * | effect 体（首拉 / 重拉 / 退订） | 喂假 IPC 直接跑 `startConfigSync()` | ①②③ |
 * | 「盘上的值 → 首页芯片」 | `renderToStaticMarkup` 真渲染**接线层** `MobileHomeScreen` | ④ |
 * | 「`MobileApp` 真的挂了这条腿」 | 真渲染 + 对 hook 打桩（hook 在**渲染期**被调，不在 effect 里） | ⑤ |
 * | hook 体那一行 | 源码断言（effect 体在 node 下跑不到，只剩这一条） | ⑥ |
 *
 * ── 接线判据必须成对 ────────────────────────────────────────────────────────
 * ①②③④ 全部直接调 `startConfigSync()` / 直接播种 store —— 它们证明的是**机制对**。
 * 生产代码若绕开这条机制（`MobileApp` 不挂 hook、或 hook 不把函数交给 `useEffect`），
 * 这四条一条都不会红。⑤⑥ 就是补上的那一对：把接线撤掉必须当场转红并点名。
 *
 * ── 每条否定断言都配正面对照 ────────────────────────────────────────────────
 * 「不许是 null」会被「什么都没发生」骗过。故本文件的读回断言一律是**正面等值**
 * （读回来的那份逐字段等于喂进去的那份、芯片文案等于盘上那个模式），
 * 并各配一条反向对照（回到「不接线」的状态必须显示兜底常量、退订前同一帧确实推动过 store）。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { UserConfig } from '@/contracts/types';

/* ── 假磁盘 ──────────────────────────────────────────────────────────────────
   一份**可变**的盘上真值：判据要能表达「盘上改了 → UI 跟着改」，固定常量表达不了。 */
const BASE = {
  proxyMode: 'global',
  proxyModeType: 'systemProxy',
  selectedServerId: 'srv-1',
  language: 'auto',
  servers: [{ id: 'srv-1', name: '节点一', protocol: 'vless', type: 'vless' }],
  customRules: [],
  trafficRules: [{ id: 'r-1', type: 'domain', value: 'example.com', action: 'proxy' }],
  dnsRules: [],
  subscriptions: [],
} as unknown as UserConfig;

let disk: UserConfig = { ...BASE };

const getMock = vi.fn(async (): Promise<UserConfig> => ({ ...disk }));
/** 已登记的 `config:changed` 订阅者。**退订必须真的从这里移除**，③ 靠它区分「退订了」与「只是没发」。 */
const listeners = new Set<() => void>();
const offSpy = vi.fn();
const onChangedMock = vi.fn((cb: () => void) => {
  listeners.add(cb);
  return () => {
    offSpy();
    listeners.delete(cb);
  };
});
const emitConfigChanged = (): void => {
  for (const cb of [...listeners]) cb();
};
const setStagedPendingMock = vi.fn(async (_pending: boolean) => undefined);
const takeCleanExitFlagMock = vi.fn(async () => false);
const startupConfigFlagsMock = vi.fn(async () => ({}));
const noop = vi.fn();

vi.mock('@/ipc', () => ({
  api: {
    config: {
      get: () => getMock(),
      onChanged: (cb: () => void) => onChangedMock(cb),
      setStagedPending: (pending: boolean) => setStagedPendingMock(pending),
    },
    window: { takeCleanExitFlag: () => takeCleanExitFlagMock() },
    stats: {
      onStatsUpdated: () => noop,
      onConnectionsDetail: () => noop,
      subscribe: async () => undefined,
      unsubscribe: async () => undefined,
    },
    ipInfo: { get: async () => null },
    proxy: { getStatus: async () => null },
    server: { switch: async () => undefined },
    system: { openExternal: async () => undefined },
  },
  unlockApi: { run: async () => ({}) },
}));

/** 设置页漏斗（首页也用它的 `update`）另从 `@/ipc/api-client` 取句柄，与 `@/ipc` 不是同一个说明符。 */
vi.mock('@/ipc/api-client', () => ({
  configApi: {
    get: () => getMock(),
    onChanged: (cb: () => void) => onChangedMock(cb),
    patch: async (patch: Partial<UserConfig>) => ({ ...disk, ...patch }),
  },
  windowApi: { startupConfigFlags: () => startupConfigFlagsMock() },
}));

/** `t()` 桩：返回 key 本身（`harness-screens.test.tsx` 同款）——断言落在结构上，与语种文案解耦。 */
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
}));

/**
 * ⑤ 要断言的是「`MobileApp` 在渲染期真的调了这个 hook」，故只对 hook 打桩，
 * `startConfigSync`（①②③ 直接跑的那个）保持真身。
 */
const syncHookSpy = vi.fn();
vi.mock('./config-sync', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./config-sync')>();
  return { ...actual, useMobileConfigSync: () => syncHookSpy() };
});

import { renderToStaticMarkup } from 'react-dom/server';
import { useAppStore, getEffectiveConfig } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { startConfigSync } from './config-sync';
import { MobileApp } from './MobileApp';
import { MobileHomeScreen } from './home/MobileHomeScreen';

const HERE = fileURLToPath(new URL('.', import.meta.url));

/* ── SSR 快照的播种 ──────────────────────────────────────────────────────────
   zustand v5 在 `react-dom/server` 下读的是 `getServerState || getInitialState`
   （`useSyncExternalStore` 的第三个参数），也就是**创建那一刻**的状态对象；`set()` 是整体替换、
   不改那个对象 ⇒ 只 `set` 的话每一屏都会拿着空 store 渲染，判据退化成「渲染空屏也全绿」。
   故渲染前把**当下的实时状态整体**镜像进初始态对象。镜像是机械的（整份 state，不挑字段），
   值仍然来自被测的那条腿，不是手写的期望值。这条腿一旦失效（zustand 换了快照来源），
   ④ 的反向对照会立刻转红，不会静默变空。 */
function mirrorLiveStateIntoSsrSnapshot(): void {
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
}

/**
 * 首页那颗模式芯片的文案（DOM 定位，不是「源码里出现过这个 key」）。
 *
 * 定位分两步：先按 `data-write-control="routing"` 切出**整颗芯片**（开标签 → `</button>`），
 * 再在它**内部**找 `.h-chiplabel.copy`。
 *
 * 旧写法是一条 `data-write-control="routing"[^>]*>\s*<span class="h-chiplabel copy">` 的正则，
 * 要求标签**紧跟**在按钮开标签之后。`[^>]*` 跨不过标签自己的 `>` ⇒ 在标签前插任何元素都会把这条
 * 门打红 —— 它因此把 `mode-chips.md`「Anatomy」要求的前导字形挡在门外整整一轮
 * （`HomeScreenView.tsx` 那段头注逐字记着这件事）。
 *
 * 收紧的部分一格没松：标签仍必须是**这颗芯片内部**的 `.h-chiplabel.copy`（不是页面上任意一个
 * 同类元素），`copy` 那一档也仍然钉着 —— 静态文案整句换行、不给省略号，是同批的一条裁定。
 */
function routingChipOf(markup: string): string | null {
  return /data-write-control="routing" aria-pressed="true">([^<]+)<\/button>/.exec(markup)?.[1] ?? null;
}

beforeEach(() => {
  disk = { ...BASE };
  listeners.clear();
  getMock.mockClear();
  onChangedMock.mockClear();
  offSpy.mockClear();
  syncHookSpy.mockClear();
  useAppStore.getState().reset();
  useStagedConfigStore.setState({ entries: [], baseline: null, baseVersion: null, hydrated: false });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('① 首拉：store 里那份等于盘上那份（正面等值，不是「非 null」）', () => {
  it('接线之前 store 是空的 —— 这就是今天移动端的状态', () => {
    expect(useAppStore.getState().config).toBeNull();
    expect(getEffectiveConfig()).toBeNull();
  });

  it('跑一次 startConfigSync 之后，逐字段等于盘上的值', async () => {
    const off = startConfigSync();
    await vi.waitFor(() => expect(useAppStore.getState().config).not.toBeNull());

    const loaded = useAppStore.getState().config as UserConfig;
    expect(loaded).toEqual(disk);
    // 首页那颗芯片读的就是这条：等于盘上的 `global`，而**不是**兜底常量 `smart`。
    expect(getEffectiveConfig()?.proxyMode).toBe('global');
    expect(getEffectiveConfig()?.proxyMode).not.toBe('smart');
    // 两个扁平镜像也必须被投影出来（节点屏 / 规则屏读的是它们，不是 `config`）。
    expect(useAppStore.getState().servers).toEqual(disk.servers);
    expect(useAppStore.getState().selectedServerId).toBe('srv-1');
    expect(useAppStore.getState().rules).toEqual(disk.trafficRules);
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('② 外部改动收敛：广播一到，store 追到盘上的新值', () => {
  it('盘上改成 direct → 发一帧 config:changed → store 变成 direct', async () => {
    const off = startConfigSync();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('global'));

    disk = { ...BASE, proxyMode: 'direct' } as UserConfig;
    emitConfigChanged();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('direct'));
    expect(getEffectiveConfig()?.proxyMode).toBe('direct');
    off();
  });

  it('订阅确实登记了一个**零形参**回调（无载荷信号，读它只会拿到 {}）', () => {
    const off = startConfigSync();
    expect(onChangedMock).toHaveBeenCalledTimes(1);
    expect(onChangedMock.mock.calls[0][0].length).toBe(0);
    off();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('③ 卸载不泄漏：退订之后同一帧不再推动 store', () => {
  it('退订前推得动、退订后推不动（负面断言带正面对照）', async () => {
    const off = startConfigSync();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('global'));

    // 正面对照：同一条发射路径在退订**之前**确实改变了 store。
    disk = { ...BASE, proxyMode: 'direct' } as UserConfig;
    emitConfigChanged();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('direct'));

    off();
    expect(offSpy).toHaveBeenCalledTimes(1);
    expect(listeners.size).toBe(0);

    const callsBefore = getMock.mock.calls.length;
    disk = { ...BASE, proxyMode: 'smart' } as UserConfig;
    emitConfigChanged();
    await new Promise((r) => setTimeout(r, 20));
    expect(getMock.mock.calls.length).toBe(callsBefore);
    expect(useAppStore.getState().config?.proxyMode).toBe('direct');
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('④ 首页芯片真的跟着 store 走（真渲染接线层，不是读源码）', () => {
  it('盘 → startConfigSync → store → 芯片：芯片文案 = 盘上那个模式', async () => {
    const off = startConfigSync();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('global'));
    off();

    mirrorLiveStateIntoSsrSnapshot();
    expect(routingChipOf(renderToStaticMarkup(<MobileHomeScreen />))).toBe('home.routingGlobal');
  });

  it('历史缺陷回放：store 空着（＝今天移动端不 load）时，芯片是硬编码兜底 `smart`', () => {
    useAppStore.getState().reset();
    mirrorLiveStateIntoSsrSnapshot();
    expect(useAppStore.getInitialState().config).toBeNull();
    expect(routingChipOf(renderToStaticMarkup(<MobileHomeScreen />))).toBe('home.routingSmart');
  });

  it('盘上是 direct 时芯片是 direct —— 证明它读的是值，不是恰好只认得 global', async () => {
    disk = { ...BASE, proxyMode: 'direct' } as UserConfig;
    const off = startConfigSync();
    await vi.waitFor(() => expect(useAppStore.getState().config?.proxyMode).toBe('direct'));
    off();

    mirrorLiveStateIntoSsrSnapshot();
    expect(routingChipOf(renderToStaticMarkup(<MobileApp />))).toBe('home.routingDirect');
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('⑤ 接线：MobileApp 真的挂了这条腿（渲染期可观测，不是源码扫描）', () => {
  it('渲染 MobileApp 恰好调一次 useMobileConfigSync', () => {
    expect(syncHookSpy).not.toHaveBeenCalled();
    renderToStaticMarkup(<MobileApp />);
    expect(syncHookSpy).toHaveBeenCalledTimes(1);
  });

  it('对照：同一次渲染里不挂这条腿的组件不会让计数自己涨（桩没被别处触发）', () => {
    renderToStaticMarkup(<MobileHomeScreen />);
    expect(syncHookSpy).not.toHaveBeenCalled();
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑥ hook 体那一行 —— effect 体在 node 下跑不到，这一段只能落在源码上。
 * 取材面先剥注释与字符串：本文件与被测文件的注释里都逐字写着被断言的形态，不剥就是判据自污染。
 */
describe('⑥ hook 把 startConfigSync 交给了 useEffect，且依赖为空', () => {
  const SRC = readFileSync(`${HERE}config-sync.ts`, 'utf8');

  /** 剥注释与字符串，两者一遍扫完（分两遍会互相误吞）。 */
  function strip(src: string): string {
    let out = '';
    let i = 0;
    while (i < src.length) {
      const c = src[i];
      const next = src[i + 1];
      if (c === '/' && next === '*') {
        const end = src.indexOf('*/', i + 2);
        i = end === -1 ? src.length : end + 2;
        out += ' ';
      } else if (c === '/' && next === '/') {
        const end = src.indexOf('\n', i);
        i = end === -1 ? src.length : end;
        out += ' ';
      } else if (c === "'" || c === '"' || c === '`') {
        let j = i + 1;
        while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
        i = j + 1;
        out += '""';
      } else {
        out += c;
        i += 1;
      }
    }
    return out;
  }

  const CODE = strip(SRC).replace(/\s+/g, '');

  it('⓪ 取材面自检：源码非空、剥离器认得注释里的假形态', () => {
    expect(SRC.length).toBeGreaterThan(2000);
    expect(CODE).toContain('exportfunctionstartConfigSync');
    expect(CODE).toContain('exportfunctionuseMobileConfigSync');
    // 注释里的同款形态不得被算进取材面（否则删掉真代码也能靠头注喂饱本判据）。
    expect(strip('/* useEffect(startConfigSync, []) */\nconst x = 1;').replace(/\s+/g, '')).toBe(
      'constx=1;',
    );
  });

  it('hook 体就是 `useEffect(startConfigSync, [])`', () => {
    expect(CODE).toContain('useEffect(startConfigSync,[])');
  });

  it('首拉不带 force、广播重拉必须带 force（非 force 会被在飞的那次单飞合并）', () => {
    expect(CODE).toContain('loadConfig()');
    expect(CODE).toContain('loadConfig(true)');
  });

  it('`.onChanged(` 恰好一处，且实参是零形参箭头（本文件不在 Rust 侧那张消费方表内，自守）', () => {
    const sites = [...CODE.matchAll(/\.onChanged\(/g)];
    expect(sites.length).toBe(1);
    expect(CODE.slice(sites[0].index)).toMatch(/^\.onChanged\(\(\)=>/);
  });
});

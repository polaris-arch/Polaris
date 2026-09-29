/**
 * 移动端**停靠区 chrome** 的门：toast 宿主（W-25）+ 待应用条（W-26）+ 反向分流指示器（W-27）。
 *
 * 三件事各自被守的缺陷：
 *  · **toast 宿主**：`lib/error-handler` 的门面未注入实现时落 console，而接线之前全仓唯一的
 *    `setToastImpl` 注入点在桌面 `Toaster.tsx` ⇒ 移动端那 24 处 `toast.*` 一条都到不了用户眼前。
 *  · **待应用条**：外壳早就留了 `.m-pending-slot`，但槽位恒空 ⇒「保存了但还没进核」在移动端完全不可见。
 *  · **反向分流指示器**：`regionRouting.reverse` 唯一入口在规则页的一个开关里，首页零呈现
 *    （真机 2026-07-20 §1.4）—— 用户误触后首页仍显示「智能 · 已连接」，看不出分流语义已反转。
 *
 * # 判据成对：注入面 / 渲染面 / 装配面
 *
 * 本仓 vitest 无 jsdom ⇒ effect 不跑。故三层各用各的手段，缺任一层都留着一条绕行路：
 *  · **注入面**（行为）：直接调 `installMobileToastHost(spy)`，再走真门面 `toast.error(...)`，
 *    看它有没有落到 spy 上。这一层测的是「门面 → 宿主」这一跳。
 *  · **渲染面**（真渲染）：`renderToStaticMarkup` 直渲条与首页，断言 DOM 里有/没有该有的东西。
 *  · **装配面**（真渲染 + 模块打桩）：渲染整棵 `MobileApp`，断言两件 chrome 真的被摆进了停靠区
 *    （而且待应用条真的落在 `.m-pending-slot` 里面）。只有前两层时，「组件写好了但没人挂」全绿。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import { context, contextOf, selectorKey, type Decl } from '@/styles/css-cascade.test-support';

const confirmState = vi.hoisted(() => ({ armed: null as string | null }));
vi.mock('@/lib/confirm-twice', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/confirm-twice')>();
  return {
    ...actual,
    useConfirmTwice: () => ({ armed: confirmState.armed, confirmTwice: () => undefined }),
  };
});

/* ── 假 IPC（整棵树渲染要用；本文件不驱动任何一条 IPC 腿）─────────────────────
   `vi.mock` 的工厂会被提升到文件最前，**顶层变量在它跑的时候还没初始化** ⇒ 退订桩只能写在工厂里面。 */
vi.mock('@/ipc', () => {
  const noop = (): (() => void) => () => undefined;
  return {
  api: {
    config: { get: async () => ({}), onChanged: noop, setStagedPending: async () => undefined },
    proxy: {
      getStatus: async () => null,
      getPendingChanges: async () => ({ added: [], modified: [], removed: [], restartDeferred: false }),
      onStarted: noop,
      onStopped: noop,
      onLifecycle: noop,
      onPendingChanges: noop,
      onError: noop,
      onTailscaleStatus: noop,
      onTailscaleAuth: noop,
      onMeshLoginFallback: noop,
      onAutoNodeSwitched: noop,
    },
    vpn: { getStatus: async () => ({ connected: false, openConnect: [], openVpn: [] }), onOpenVpnStatus: noop },
    subscription: { onUpdateProgress: noop, onAutoUpdate: noop },
    server: { tailscaleStateExists: async () => ({}), switch: async () => undefined },
    system: { openExternal: async () => undefined },
    stats: {
      onStatsUpdated: noop,
      onConnectionsDetail: noop,
      subscribe: async () => undefined,
      unsubscribe: async () => undefined,
    },
    ipInfo: { get: async () => null },
    window: { takeCleanExitFlag: async () => false },
  },
  unlockApi: { run: async () => ({}) },
  };
});
vi.mock('@/ipc/api-client', () => ({
  configApi: { get: async () => ({}), onChanged: () => () => undefined, patch: async () => ({}) },
  windowApi: { startupConfigFlags: async () => ({}) },
}));
vi.mock('@/lib/desktop-notify', () => ({
  notifyDesktop: async () => undefined,
  setDesktopNotificationsEnabled: () => undefined,
}));
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    // 带插值的键（`home.pendingBarTitle` 收 `{count}`）也照原样返回键：断言落在结构上。
    t: (key: string) => key,
    i18n: { language: 'zh-CN', t: (key: string) => key },
  }),
}));

/**
 * 装配面用的**标记桩**：只桩 toast 宿主这一个，且**只桩组件本体**（`installMobileToastHost`
 * 从 `actual` 原样带出，① 组测的仍是真身）。
 *
 * 为什么只有它需要桩：`MobileToaster` 的内容全部由 effect 注入的 `push` 驱动，而本仓无 jsdom ⇒
 * effect 不跑 ⇒ 真身在判据里**恒渲染成 null**，「有没有被摆进停靠区」就无从断言。
 * 待应用条不同 —— 它的可见性只看 store，故 ④ 组用**真身 + 喂数**去证装配，不用桩。
 */
vi.mock('./MobileToaster', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./MobileToaster')>();
  return { ...actual, MobileToaster: () => <i data-probe="toast-host" /> };
});

import { renderToStaticMarkup } from 'react-dom/server';
import { setToastImpl, toast, type ToastOptions } from '@/lib/error-handler';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { installMobileToastHost, type ToastPush } from './MobileToaster';
import { MobilePendingBar } from './MobilePendingBar';
import { MobileApp } from './MobileApp';
import { HomeScreenView } from './home/HomeScreenView';
import { MobileHomeScreen } from './home/MobileHomeScreen';
import type { HomeScreenViewProps } from './home/view-model';

/* zustand v5 在 `react-dom/server` 下读的是**创建那一刻**的状态对象（`getInitialState`）⇒
   渲染前必须把当下实时状态整体镜像进去，否则每一屏都拿空 store 渲染，判据退化成「渲染空屏也全绿」。 */
function mirrorLiveStateIntoSsrSnapshot(): void {
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
}

beforeEach(() => {
  confirmState.armed = null;
  useAppStore.getState().reset();
  useStagedConfigStore.setState({
    entries: [],
    baseline: null,
    baseVersion: null,
    hydrated: false,
    saveStatus: 'idle',
    conflict: null,
    conflictBaseline: null,
  });
  setToastImpl({}); // 回到 console 兜底
});

// ─────────────────────────────────────────────────────────────────────────────
describe('① toast 宿主：门面 → 宿主这一跳真的通了', () => {
  const calls: Array<{ msg: string; kind: string; desc?: string; opts?: ToastOptions }> = [];
  const push: ToastPush = (msg, kind, desc, opts) => void calls.push({ msg, kind, desc, opts });

  beforeEach(() => {
    calls.length = 0;
  });

  it('反向对照：**没有**宿主时，`toast.*` 一个字节都到不了（这就是接线前的状态）', () => {
    toast.error('boom');
    expect(calls).toEqual([]);
  });

  it('注入之后四条通道各自落到宿主，且 kind 与 description 都接上了', () => {
    const restore = installMobileToastHost(push);
    toast.success('ok');
    toast.error('bad', '因为某某');
    toast.info('hi');
    toast.warning('careful');
    expect(calls.map((c) => [c.msg, c.kind])).toEqual([
      ['ok', 'ok'],
      ['bad', 'err'],
      ['hi', ''],
      ['careful', ''],
    ]);
    // `error` 的第二参是失败原因；桌面那版曾经把它丢了，这里正面钉住。
    expect(calls[1].desc).toBe('因为某某');
    restore();
  });

  it('`ToastOptions` 原样透传（去重键 / sticky / 动作 —— 队列语义靠它们）', () => {
    const restore = installMobileToastHost(push);
    toast.info('p', { key: 'speedtest', sticky: true });
    expect(calls[0].opts).toEqual({ key: 'speedtest', sticky: true });
    restore();
  });

  it('卸载恢复 console 兜底：restore 之后再发就不该落到已卸载的宿主上', () => {
    const restore = installMobileToastHost(push);
    toast.info('a');
    restore();
    toast.info('b');
    expect(calls.map((c) => c.msg)).toEqual(['a']);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('② 待应用条：差集/暂存有东西时它才出现，且动作按运行态分流', () => {
  const render = (): string => {
    mirrorLiveStateIntoSsrSnapshot();
    return renderToStaticMarkup(<MobilePendingBar />);
  };

  it('无事可做 ⇒ 返 null（空槽 `:empty` 才能零高度）', () => {
    expect(render()).toBe('');
  });

  it('仅有未保存草稿：重置清空后条立即消失', () => {
    useStagedConfigStore.setState({
      enabled: true,
      entries: [{ id: 'draft', kind: 'server', label: 'draft', entityPath: ['servers', 'n1'], nextValue: {} }],
    } as never);
    expect(render()).toContain('home.pendingStagedTitle');
    useStagedConfigStore.getState().reset();
    expect(render()).toBe('');
  });

  it('草稿与已保存待应用并存：重置只清草稿，待应用条和解释保留', () => {
    useAppStore.setState({
      pendingChanges: { added: ['n1'], modified: [], removed: [], restartDeferred: false },
      proxyStatus: { running: true } as never,
    });
    useStagedConfigStore.setState({
      enabled: true,
      entries: [{ id: 'draft', kind: 'server', label: 'draft', entityPath: ['servers', 'n2'], nextValue: {} }],
    } as never);
    expect(render()).toContain('home.pendingStagedAndPending');
    useStagedConfigStore.getState().reset();
    const html = render();
    expect(html).toContain('home.pendingBarTitle');
    expect(html).toContain('home.pendingSavedStillNeedsApply');
    expect(html).not.toContain('home.pendingResetBtn');
  });

  it('武装态确认按钮带独立 confirming 类，短文案留在文字区', () => {
    confirmState.armed = 'mobile-reset-pending';
    useStagedConfigStore.setState({
      enabled: true,
      entries: [{ id: 'draft', kind: 'server', label: 'draft', entityPath: ['servers', 'n1'], nextValue: {} }],
    } as never);
    const html = render();
    expect(html).toContain('home.pendingResetShortConfirm');
    expect(html).toContain('m-pending-confirming confirming');
    expect(html).toMatch(/class="[^"]*m-pending-confirming confirming[^"]*"[^>]*>common\.confirm<\/button>/);
    expect(html).not.toContain('home.pendingResetConfirm');
  });

  it('有差集 + 核在跑 ⇒ 出条，标题带计数，且有「立即应用」', () => {
    useAppStore.setState({
      pendingChanges: { added: ['a', 'b'], modified: [], removed: [], restartDeferred: false },
      proxyStatus: { running: true } as never,
    });
    const html = render();
    expect(html).toContain('m-pending');
    expect(html).toContain('home.pendingBarTitle');
    expect(html).toContain('home.pendingApplyBtn');
  });

  it('核没在跑 ⇒「立即应用」**整颗不渲染**（不是禁用）—— 那颗按钮的后半句无对象', () => {
    useAppStore.setState({
      pendingChanges: { added: ['a'], modified: [], removed: [], restartDeferred: false },
      proxyStatus: { running: false } as never,
    });
    const html = render();
    expect(html).toContain('home.pendingBarTitle');
    expect(html).not.toContain('home.pendingApplyBtn');
  });

  it('`restartDeferred` 那笔欠账：三个数组全空也要出条，且不能说「0 项待应用」', () => {
    useAppStore.setState({
      pendingChanges: { added: [], modified: [], removed: [], restartDeferred: true },
      proxyStatus: { running: true } as never,
    });
    const html = render();
    expect(html).toContain('home.pendingBarConfigOnly');
    expect(html).not.toContain('home.pendingBarTitle');
  });

  it('保存冲突：就地给出裁决入口（桌面走弹窗层，移动端禁入 ⇒ 不接就是「点了保存没反应」）', () => {
    useStagedConfigStore.setState({
      conflict: [{ entryId: 'e1', label: '节点 A', mine: 'x', disk: 'y' }],
    } as never);
    const html = render();
    expect(html).toContain('home.stagedConflictTitle');
    expect(html).toContain('home.stagedConflictMine');
    expect(html).toContain('common.cancel');
    expect(html, '冲突项的名字要说出来，否则用户不知道是哪一条被顶了').toContain('节点 A');
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('③ 反向分流指示器（真机 2026-07-20 §1.4 的移植）', () => {
  const baseProps = (over: Partial<HomeScreenViewProps>): HomeScreenViewProps =>
    ({
      t: ((key: string) => key) as HomeScreenViewProps['t'],
      writeErrors: {},
      core: 'stopped',
      connState: 'disconnected',
      node: { kind: 'none' },
      onToggleConnect: () => undefined,
      onNetworkCheck: () => undefined,
      tsExitWarning: null,
      onTsExitAction: () => undefined,
      pickerOpen: false,
      onOpenPicker: () => undefined,
      onClosePicker: () => undefined,
      pickerQuery: '',
      onPickerQuery: () => undefined,
      pickRows: [],
      onUseAsExit: () => undefined,
      onPickSentinel: () => undefined,
      blockDisabledReason: null,
      noServers: false,
      onAddServer: () => undefined,
      onAddSubscription: () => undefined,
      routing: 'smart',
      reverseRouting: false,
      exitRegion: { kind: 'unknown' },
      exitFlagSrc: null,
      exitIsDirect: true,
      exitProbing: false,
      routingSheetOpen: false,
      onOpenRoutingSheet: () => undefined,
      onCloseRoutingSheet: () => undefined,
      onSetRouting: () => undefined,
      onProbeExit: () => undefined,
      unlock: [],
      unlockCheckedLabel: null,
      unlockRunning: false,
      unlockDetailId: null,
      onOpenUnlockDetail: () => undefined,
      onCloseUnlockDetail: () => undefined,
      samples: [],
      composition: [],
      ruleHits: [],
      hosts: [],
      hostsMasked: false,
      ruleSubject: null,
      ruleSubjectView: 'menu',
      onOpenRuleSubject: () => undefined,
      onCloseRuleSubject: () => undefined,
      onRuleSubjectView: () => undefined,
      onQuickRule: () => undefined,
      newRuleAction: 'proxy',
      onNewRuleAction: () => undefined,
      newRuleRemarks: '',
      onNewRuleRemarks: () => undefined,
      ruleRemarksHint: '',
      onCreateRule: () => undefined,
      appendQuery: '',
      onAppendQuery: () => undefined,
      appendTargets: [],
      onAppendToRule: () => undefined,
      windowBytes: 0,
      windowConnections: 0,
      kernelConnections: null,
      ...over,
    }) as HomeScreenViewProps;

  it('反向语义 ⇒ 常驻角标与后果摘要，完整说明可从 i 打开', () => {
    const html = renderToStaticMarkup(<HomeScreenView {...baseProps({ reverseRouting: true })} />);
    expect(html).toContain('home.reverseRoutingBadge');
    expect(html).toContain('mobileHelp.reverseRouting');
    expect(html).toContain('class="m-info-trigger"');
    expect(html).toContain('aria-haspopup="dialog"');
  });

  it('正向语义 ⇒ 零额外 DOM（正向下这条提示就是噪音）', () => {
    const html = renderToStaticMarkup(<HomeScreenView {...baseProps({ reverseRouting: false })} />);
    expect(html).not.toContain('home.reverseRoutingBadge');
  });

  it('接线层真的按 `isReverseRegionRouting` 喂它（不是就地读 `reverse`）', () => {
    // 正面：enabled + reverse ⇒ 亮。
    useAppStore.setState({
      config: { regionRouting: { enabled: true, region: 'cn', reverse: true } } as UserConfig,
    });
    mirrorLiveStateIntoSsrSnapshot();
    expect(renderToStaticMarkup(<MobileHomeScreen />)).toContain('home.reverseRoutingBadge');

    // 反向对照：关掉地区分流后 `reverse` 是**死数据**，此时提示即噪音。
    // 就地写 `config?.regionRouting?.reverse` 的实现会在这一条上红 —— 那正是这条对照存在的理由。
    useAppStore.setState({
      config: { regionRouting: { enabled: false, region: 'cn', reverse: true } } as UserConfig,
    });
    mirrorLiveStateIntoSsrSnapshot();
    expect(renderToStaticMarkup(<MobileHomeScreen />)).not.toContain('home.reverseRoutingBadge');
  });
});

// ─────────────────────────────────────────────────────────────────────────────
describe('④ 装配面：两件 chrome 真的被摆进了停靠区（不是「写好了没人挂」）', () => {
  const dockOf = (html: string): string => {
    const at = html.indexOf('class="m-dock"');
    expect(at, '渲染结果里没有停靠区 —— 外壳结构变了').toBeGreaterThan(-1);
    return html.slice(at);
  };

  it('toast 宿主在停靠区里', () => {
    mirrorLiveStateIntoSsrSnapshot();
    const html = renderToStaticMarkup(<MobileApp />);
    expect(dockOf(html), 'toast 宿主没有被摆进停靠区 ⇒ 那 24 处反馈仍然落 console').toContain(
      'data-probe="toast-host"',
    );
  });

  it('反向对照：宿主在**停靠区**里，不在滚动区里（在滚动区就会随内容滚走）', () => {
    mirrorLiveStateIntoSsrSnapshot();
    const html = renderToStaticMarkup(<MobileApp />);
    const scroll = html.indexOf('class="m-scroll"');
    const dock = html.indexOf('class="m-dock"');
    expect(scroll).toBeGreaterThan(-1);
    expect(dock).toBeGreaterThan(scroll);
    expect(html.slice(scroll, dock), '宿主落在了滚动区里').not.toContain('data-probe="toast-host"');
  });

  it('待应用条（**真身**，不是桩）落在 `.m-pending-slot` 里面', () => {
    // 用真身 + 喂数：桩能证明「装配了」，但证明不了「条自己在无事可做时会返 null」这条同样承重的腿
    // —— 那条腿正是 `.m-pending-slot:empty` 零高度的前提。两个方向在这里一次量完。
    useAppStore.setState({
      pendingChanges: { added: ['a'], modified: [], removed: [], restartDeferred: false },
      proxyStatus: { running: true } as never,
    });
    mirrorLiveStateIntoSsrSnapshot();
    const html = renderToStaticMarkup(<MobileApp />);
    const slot = html.indexOf('data-slot="pending-changes"');
    expect(slot, '槽位没了').toBeGreaterThan(-1);
    expect(html.slice(slot, slot + 400), '条没有落进槽位').toContain('class="m-pending"');
  });

  it('反向对照：无事可做时槽位里空空如也（否则 `:empty` 永远命不中，空条会占一行高度）', () => {
    mirrorLiveStateIntoSsrSnapshot();
    const html = renderToStaticMarkup(<MobileApp />);
    const slot = html.indexOf('data-slot="pending-changes"');
    expect(html.slice(slot, slot + 400)).not.toContain('class="m-pending"');
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑤ **停靠区的页边距**：`--page-inset` 必须在 `.m-dock` 上也解析一次。
 *
 * 2026-09-06 复审 minor：停靠区里的两件 chrome（`.m-toast-host` / `.m-pending`）都用
 * `var(--page-inset)` 表达「跟着页面边距走」，但那两条断点覆写只写在 `.m-page` 上，而 `.m-dock`
 * 是 `.m-scroll` 的**兄弟**、拿不到那次覆写 ⇒ 容器 ≥37.5em（横屏手机 / 平板）时，同一竖线上
 * 卡片是 24 / 32、停靠区里的标题与 toast 正文仍是 16 —— 8~16px 的可见错位，零运行期报错。
 *
 * 判据按**规则级取材**（同 `dynamic-type.test.ts` 的先例）：每一个声明了 `--page-inset` 的
 * 断点条件块，其选择器集合必须同时覆盖 `.m-page` 与 `.m-dock`。写 `var(--page-inset)` 本身
 * 就是「要对齐」的意图声明（若本意恒 16 该写 `var(--sp-4)`），本门守的是这个意图不被级联静默吃掉。
 */
describe('⑤ 停靠区页边距：响应标量在 .m-dock 上也解析（否则断点上与内容页错位）', () => {
  const RESPONSIVE_COND = /@container\s+mscreen\s*\(\s*min-width:/;
  const REQUIRED = ['.m-page', '.m-dock'].map((s) => selectorKey(s));

  /** 每个「声明了 `--page-inset` 的断点块」里缺了哪些必备选择器。空数组 = 都覆盖到了。 */
  const gaps = (decls: readonly Decl[]): string[] => {
    const byCond = new Map<string, Set<string>>();
    for (const d of decls) {
      if (d.prop !== '--page-inset') continue;
      const cond = d.conds.find((c) => RESPONSIVE_COND.test(c));
      if (cond === undefined) continue;
      const seen = byCond.get(cond) ?? new Set<string>();
      for (const sel of d.sels) seen.add(sel);
      byCond.set(cond, seen);
    }
    const out: string[] = [];
    for (const [cond, seen] of byCond) {
      for (const need of REQUIRED) if (!seen.has(need)) out.push(`${cond} 缺 ${need}`);
    }
    return out.sort();
  };

  const DECLS = context('mobile').decls;

  it('⓪ 取材面自检：断点块解析得到，且谓词分得清「覆盖了」与「没覆盖」', () => {
    const conds = new Set(
      DECLS.filter((d) => d.prop === '--page-inset')
        .flatMap((d) => d.conds)
        .filter((c) => RESPONSIVE_COND.test(c)),
    );
    expect(conds.size, '一个 `--page-inset` 断点块都没解析到 —— 取材面塌了，下面那条恒绿').toBe(2);
    // 缺省那一档仍由无条件规则给（两边同为 16），否则「只在断点上有值」是另一种错。
    expect(
      DECLS.some((d) => d.prop === '--page-inset' && d.conds.length === 0),
      '`--page-inset` 没有无条件缺省值',
    ).toBe(true);
    // 反向对照：合成一份「只覆写 .m-page」的 CSS，谓词必须点名缺的那一个。
    const onlyPage = contextOf([
      { file: '<synthetic>', css: '@container mscreen (min-width: 37.5em){.m-page{--page-inset:24px}}' },
    ]).decls;
    expect(gaps(onlyPage)).toEqual(['@container mscreen (min-width: 37.5em) 缺 .m-dock']);
    // 正面对照：两个都写上就不报了（证明它不是恒红）。
    const both = contextOf([
      {
        file: '<synthetic>',
        css: '@container mscreen (min-width: 37.5em){.m-page{--page-inset:24px}.m-dock{--page-inset:24px}}',
      },
    ]).decls;
    expect(gaps(both)).toEqual([]);
  });

  it('每个断点块都同时覆写 `.m-page` 与 `.m-dock`', () => {
    expect(
      gaps(DECLS),
      '停靠区拿不到这一档的页边距 —— `.m-dock` 是 `.m-scroll` 的兄弟，`.m-page` 上的覆写到不了它；' +
        '结果是待应用条标题与 toast 正文比同屏卡片少缩进 8~16px',
    ).toEqual([]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
/**
 * ⑥ **toast 宿主的层序**：它必须画在移动端**全部**弹层之上。
 *
 * 2026-09-06 批 3 复审 major：`.m-toast-host` 此前没有 `z-index`（即 `auto`），而
 * `.m-form-layer`（`forms/forms.css`）是 `position:fixed; z-index:40` 的兄弟层，两者同处根层叠
 * 上下文（`.m-shell` / `.m-dock` 都不建层叠上下文）。按绘制顺序，正 `z-index` 的定位元素画在
 * `z-index:auto` 的定位元素之上 ⇒ 表单开着时发出的 toast 被整片压住。
 *
 * 后果不是「不好看」：WARP 注册腿「单例槽被占」那一档是**零远端请求 + 一条 toast**，
 * toast 看不见就等于用户按下「注册」之后屏幕上什么都不发生。`half-truth-facts.test.ts` 里
 * `SILENT_TOAST_WITHOUT_HOST` 那段注释当初就写着「哪天有人让某条 toast 在表单还开着时发出，
 * 先解决那个层叠问题再抬数」——本条就是把那句话变成一道会红的门。
 *
 * 取材面是**整个移动端层叠上下文**（`context('mobile')` 从入口模块图推导，不写死文件清单）：
 * 谁在哪个文件里新加一个比 50 还高的层，这条当场红，而不是等到真机上发现 toast 又不见了。
 */
describe('⑥ toast 宿主的 z 序：高于移动端每一个弹层（否则表单开着时的 toast 是静音的）', () => {
  const HOST = selectorKey('.m-toast-host');

  /** 每条 `z-index` 声明的数值（`auto` / 非数值不计入）。 */
  const zLayers = (
    decls: readonly Decl[],
  ): ReadonlyArray<{ sel: string; where: string; z: number }> =>
    decls
      .filter((d) => d.prop === 'z-index')
      .flatMap((d) =>
        d.sels.map((sel, i) => ({
          sel,
          where: `${d.file}:${d.line} ${d.rawSels[i] ?? sel}`,
          z: Number(d.value.trim()),
        })),
      )
      .filter((r) => Number.isFinite(r.z));

  /** 宿主够不着的层（`z >= 宿主`）。宿主自己没声明 z 时**全部**层都算够不着。 */
  const overshadowing = (decls: readonly Decl[]): string[] => {
    const layers = zLayers(decls);
    const host = layers.filter((r) => r.sel === HOST).map((r) => r.z);
    const hostZ = host.length > 0 ? Math.max(...host) : Number.NEGATIVE_INFINITY;
    return layers
      .filter((r) => r.sel !== HOST && r.z >= hostZ)
      .map((r) => `${r.where} = ${r.z} ≥ 宿主 ${hostZ}`)
      .sort();
  };

  const DECLS = context('mobile').decls;

  it('⓪ 取材面自检 + 正反对照：认得出「被压住」，也不误判「没被压住」', () => {
    const layers = zLayers(DECLS);
    expect(layers.length, '移动端一条 `z-index` 都没解析到 —— 取材面塌了，下面那条恒绿').toBeGreaterThan(5);
    // 表单层必须在取材面上：它正是当初压住 toast 的那一层，不在面上则下面那条无信息量。
    expect(
      layers.some((r) => r.sel === selectorKey('.m-form-layer')),
      '`.m-form-layer` 不在取材面上 —— 这条门守的就是它',
    ).toBe(true);

    // 反向对照 ①：宿主**没有** z-index（缺陷的原始形态）⇒ 必须点名那一层。
    const noZ = contextOf([
      { file: '<syn>', css: '.m-toast-host{position:absolute}.m-form-layer{position:fixed;z-index:40}' },
    ]).decls;
    expect(overshadowing(noZ), '宿主无 z 时判据没报错 —— 它认不出缺陷的原始形态').toHaveLength(1);

    // 反向对照 ②：有人新加了一个比宿主还高的层。
    const higher = contextOf([
      { file: '<syn>', css: '.m-toast-host{z-index:50}.m-sheet{position:fixed;z-index:60}' },
    ]).decls;
    expect(overshadowing(higher)[0], '新加的更高层没被点名').toContain('= 60');

    // 正面对照：宿主更高时不报（证明它不是恒红）。
    const ok = contextOf([
      { file: '<syn>', css: '.m-toast-host{z-index:50}.m-form-layer{position:fixed;z-index:40}' },
    ]).decls;
    expect(overshadowing(ok)).toEqual([]);
  });

  it('宿主真的声明了 `position` 与 `z-index`（`z-index` 对非定位元素不生效 = 白写）', () => {
    const own = DECLS.filter((d) => d.sels.includes(HOST));
    expect(own.map((d) => d.prop), '`.m-toast-host` 没有 `position` —— z-index 是死的').toContain(
      'position',
    );
    expect(own.map((d) => d.prop), '`.m-toast-host` 没有 `z-index` —— 回到了被表单层压住的原始形态').toContain(
      'z-index',
    );
  });

  it('移动端没有任何一层画得比 toast 宿主高', () => {
    expect(
      overshadowing(DECLS),
      '有弹层压在 toast 宿主之上 ⇒ 那一层开着时发出的 toast 用户看不见。' +
        '组网单例被占那一档的唯一反馈就是一条 toast —— 看不见等于按下主按钮之后什么都不发生',
    ).toEqual([]);
  });
});

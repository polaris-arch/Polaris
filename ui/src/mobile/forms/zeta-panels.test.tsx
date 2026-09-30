/**
 * ζ 批（2026-09-25）两张面板的**渲染期**判据：它们读的是应用级 store，不是面板自持的 state。
 *
 * # 为什么要这一层（`app-wiring.test.tsx` ⑬ 之外）
 *
 * ⑬ 证明「应用级订阅真的把帧写进了 store」。那只是一半：面板若仍读自己的 `useState`
 * （A12 修之前 `TaildropPanel` 就是这样，`setTasks(...)` 只在打开 / 刷新时写一次），store 动得再勤，
 * 屏上也还是那张静态截图。本文件补另一半 —— **store 里有什么，面板就画什么**。
 * 本仓 vitest 跑 `environment:'node'`，`renderToStaticMarkup` 不跑 effect ⇒ 面板首帧画出来的
 * 任务只可能来自 store（打开时那次 pull 是 effect，这里根本没跑），正好把「读 store」与
 * 「读自持 state」分开。
 *
 * # 恢复面（A8）
 *
 * `SubCreateTaskPanel` 由应用级水合打开，读 `useSubscriptionCreateOperationStore.snapshots`。
 * 三格：进行中 ⇒ 阶段文案 + 「取消添加」；失败 ⇒ 本地化原因；宿主真的按 kind 把它画出来。
 */
import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';

import type { TaildropTaskSnapshot } from '@/contracts/taildrop';
import type { TailscaleStatusEvent } from '@/contracts/tailscale-status';
import type { SubscriptionCreateSnapshot } from '@/contracts/subscription-create-operation';
import i18n, { i18nReady } from '@/i18n';
import { useAppStore } from '@/store/app-store';
import { useSubscriptionCreateOperationStore } from '@/store/subscription-create-operation-store';
import { useTaildropTaskStore } from '@/store/use-taildrop-task-store';

import { MobileFormHost } from './MobileFormHost';
import { SubCreateTaskPanel } from './SubFormPanel';
import { TaildropPanel } from './TaildropPanel';
import { openMobileSubscriptionCreateRecovery, useMobileFormStore } from './form-store';

beforeAll(async () => {
  await i18nReady;
  await i18n.changeLanguage('en-US');
});

/**
 * 喂 store。**两处都写**：zustand 在服务端渲染下读的是**初始态**（`getServerState || getInitialState`，
 * 同 `harness-screens.test.tsx` 头注），只 `setState` 的话 `renderToStaticMarkup` 看不见 ——
 * 那会让下面每条正面断言都红在「没喂进去」上，而不是量到面板读没读 store。
 */
function seed<T extends object>(store: { getInitialState: () => T; setState: (p: Partial<T>) => void }, part: Partial<T>): void {
  Object.assign(store.getInitialState(), part);
  store.setState(part);
}

beforeEach(() => {
  seed(useAppStore, { tailscaleStatuses: {} });
  seed(useTaildropTaskStore, { tasks: {} });
  seed(useSubscriptionCreateOperationStore, { snapshots: {}, trackedOperationIds: [], handledTerminalRevisions: {} });
  seed(useMobileFormStore, { stack: [] });
});

const task = (over: Partial<TaildropTaskSnapshot> = {}): TaildropTaskSnapshot => ({
  taskId: 'task-1',
  serverId: 'ts-1',
  peerStableId: 'peer-1',
  phase: 'sending',
  files: [],
  sentBytes: 60,
  acknowledgedBytes: 50,
  totalBytes: 100,
  startedAtMs: 1,
  updatedAtMs: 2,
  revision: 2,
  ...over,
});

const readyStatus = (): TailscaleStatusEvent =>
  ({
    serverId: 'ts-1',
    loggedIn: true,
    backendState: 'Running',
    expired: false,
    authURL: '',
    tailscaleIPs: [],
    peers: [{ stableID: 'peer-1', hostName: 'laptop', online: true, details: { canReceiveFiles: true } }],
    canShareFiles: true,
    waitingFileCount: 0,
    receivingFileCount: 0,
    unreadFileCount: 0,
  }) as unknown as TailscaleStatusEvent;

describe('A12 TaildropPanel 读应用级任务 store', () => {
  it('store 里本节点在跑的任务 ⇒ 首帧就画出进度（没有任何 pull 跑过）', () => {
    seed(useAppStore, { tailscaleStatuses: { 'ts-1': readyStatus() } });
    seed(useTaildropTaskStore, { tasks: { 'task-1': task() } });
    const html = renderToStaticMarkup(<TaildropPanel instanceId="i1" serverId="ts-1" />);
    expect(html, '发件进度区没画出来 —— 面板没读 store').toContain(i18n.t('taildrop.outgoing'));
    expect(html).toContain('60%');
    expect(html).toContain(i18n.t('taildrop.toPeer', { peer: 'laptop' }));
  });

  it('反向对照：别的节点的任务不画（按 serverId 取视图，不是整表）；store 空 ⇒ 没有进度区', () => {
    seed(useAppStore, { tailscaleStatuses: { 'ts-1': readyStatus() } });
    seed(useTaildropTaskStore, { tasks: { 'task-9': task({ taskId: 'task-9', serverId: 'ts-2' }) } });
    const html = renderToStaticMarkup(<TaildropPanel instanceId="i1" serverId="ts-1" />);
    expect(html).not.toContain(i18n.t('taildrop.outgoing'));
    seed(useTaildropTaskStore, { tasks: {} });
    expect(renderToStaticMarkup(<TaildropPanel instanceId="i1" serverId="ts-1" />)).not.toContain('60%');
  });
});

const createSnap = (over: Partial<SubscriptionCreateSnapshot> = {}): SubscriptionCreateSnapshot => ({
  operationId: 'op-1',
  revision: 1,
  phase: 'fetching',
  terminal: false,
  ...over,
});

describe('A8 SubCreateTaskPanel 恢复面', () => {
  it('进行中：阶段文案 + 「取消添加」（关闭 = 取消，同桌面）', () => {
    seed(useSubscriptionCreateOperationStore, { snapshots: { 'op-1': createSnap() } });
    const html = renderToStaticMarkup(<SubCreateTaskPanel instanceId="i1" operationId="op-1" />);
    expect(html).toContain(i18n.t('sub.createTaskTitle'));
    expect(html).toContain(i18n.t('sub.createTaskPhaseFetching'));
    expect(html).toContain(i18n.t('sub.cancelCreate'));
  });

  it('失败：显示本地化原因，脚键是「关闭」', () => {
    seed(useSubscriptionCreateOperationStore, {
      snapshots: { 'op-1': createSnap({ phase: 'failed', terminal: true, revision: 3, error: { errorKind: 'timeout' } }) },
    });
    const html = renderToStaticMarkup(<SubCreateTaskPanel instanceId="i1" operationId="op-1" />);
    expect(html).toContain(i18n.t('sub.createTaskFailed'));
    expect(html).not.toContain(i18n.t('sub.cancelCreate'));
    expect(html).toContain(i18n.t('common.close'));
  });

  it('开表判据：同一个 operationId 只开一层；宿主按 kind 真的把恢复面画出来', () => {
    seed(useSubscriptionCreateOperationStore, { snapshots: { 'op-1': createSnap() } });
    expect(openMobileSubscriptionCreateRecovery('op-1')).toBe(true);
    expect(openMobileSubscriptionCreateRecovery('op-1')).toBe(false);
    expect(openMobileSubscriptionCreateRecovery('op-2')).toBe(true);
    expect(useMobileFormStore.getState().stack.map((e) => e.kind)).toEqual(['sub-create-task', 'sub-create-task']);
    seed(useMobileFormStore, { stack: useMobileFormStore.getState().stack });
    const html = renderToStaticMarkup(<MobileFormHost />);
    expect(html).toContain(i18n.t('sub.createTaskPhaseFetching'));
  });
});

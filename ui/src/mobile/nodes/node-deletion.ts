/**
 * 移动端**删除腿**（单节点 / 批量 / 订阅），W-03 与 W-04 的删除那一条。
 *
 * # 判据一条都不新造：桌面 `use-node-deletion.ts` 的三段逐段复用
 *
 * 桌面那条编排是**纯前端 TS**，三段各自有自己的纯函数，本文件把它们原样串起来：
 *  ① **暂存事务** —— `lib/staged-config#splitStagedOnly`：只在暂存里、盘上还没有的节点，
 *     删除 = **撤销那条暂存条目**，一次后端调用都不该发（发了会得到一个 404 形态的错误，
 *     而那个节点在用户看来明明就在屏幕上）。
 *  ② **写入路由分流** —— `node-delete-fallback#partitionNodeDeleteRoutes`（按 `editRoute` 的
 *     单一闸门产出的 policy 分流 staged / 直落盘）。
 *  ③ **兜底改选** —— `node-delete-fallback#fallbackExitAfterDelete`：删掉当前出口后回落到
 *     「剩下最快的一个**承载全出网流量**的节点」。它防的是流量裸奔（subnet-only 组网节点字段齐备
 *     但只路由内网段，选中它 = 公网走 direct）。
 *     ⚠️ 后端**已经保证出口不会为空**（`src-tauri/src/commands/server.rs:212 resolve_fallback_selected`
 *     不存活即回落 `DIRECT_SERVER_ID`，`:232 apply_selection_fallback` 在 `server_delete` 内**无条件
 *     调用**）⇒ 传它拿回来的不是「不裸奔」，而是**回落到哪一个**的选择权。
 *
 * # 与桌面的两处形态差异（都不是判据差异）
 *
 *  · **二次确认**：桌面是原地 `confirmTwice`（按钮翻红 + 再点一次），这里是叠一层确认面板 ——
 *    触屏没有 hover，翻红那一下在拇指底下被自己的手指挡住，而第二击极易被读成误触重复。
 *  · **回显**：失败留屏级行内 notice 以保留归属；真正完成的删除给短 Toast。
 *    暂存中的删除由底部待应用条持续说明。
 *
 * 本模块不认识 React 之外的 UI：确认与回显都由调用方以闭包传进来，故它在 node 环境下可直测。
 */

import { useCallback } from 'react';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { DIRECT_SERVER_ID } from '@/domain/direct-selection';
import {
  fallbackExitAfterDelete,
  partitionNodeDeleteRoutes,
} from '@/components/screens/nodes/node-delete-fallback';
import { selectedVisibleIds, subDeleteNodeCount } from '@/components/screens/nodes/nodes-logic';
import { editRoute, splitStagedOnly } from '@/lib/staged-config';
import { useAppStore, useEffectiveConfig, useEffectiveServers } from '@/store/app-store';
import { useLatencyStore } from '@/store/use-latency-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import type { MobileConfirmPayload } from '../forms/form-store';

export interface MobileDeletionDeps {
  readonly t: (key: string, vars?: Record<string, unknown>) => string;
  /** 成功后清除这张屏或面板上先前留下的失败回执。 */
  readonly clearNotice: () => void;
  /**
   * 屏内**唯一写出口**（`MobileNodesScreen#runWrite`）。删除腿不自带 try/catch ——
   * 那会在同一个屏上开出第二条失败回显通道，而 `write-failure-visibility.test.ts` 的
   * 「每一处写调用都被它那个屏的已登记机制罩住」正是按「唯一出口」判的。
   * 失败文案由第二个入参给（`runWrite` 的 catch 把它写进屏级 notice）。
   */
  readonly runWrite: (
    op: () => Promise<void>,
    describe: (err: unknown) => string,
  ) => Promise<void>;
  /** 叠一层确认面板，返回那一层的 instanceId（供 `onConfirm` 关掉自己）。 */
  readonly confirm: (payload: MobileConfirmPayload) => string;
  /** 关掉某一层（确认面板自己）。 */
  readonly dismiss: (instanceId: string) => void;
  /** 批删成功后退出批选态。 */
  readonly exitBatch: () => void;
}

/**
 * 「重新注册 / 注销 WARP」这两颗次动作的差异面 —— 逐字对位桌面
 * `use-node-deletion.ts#WarpRemovalOptions`（那边叫 `okToast`，这里落的是屏级/面板级 notice）。
 *
 * 两颗走的是**同一条删除腿**，差别只有三处文案与「删完还做不做一件事」：
 * 「重新注册」= 删掉现有那台 + 立刻开注册表（`afterDelete`），「注销」= 只删。
 */
export interface MobileWarpRemovalOptions {
  readonly title: string;
  readonly message: string;
  /** 成功那一句。**不复用 `nodes.deleteSuccess`**：用户按的是「注销 WARP」，不是「删除节点」。 */
  readonly okText: string;
  /** 删成之后再做的一件事（「重新注册」用它开注册表）。失败时不执行。 */
  readonly afterDelete?: () => void;
}

export interface MobileDeletion {
  readonly deleteNode: (server: ServerConfig) => void;
  readonly deleteBatch: (
    selectedIds: ReadonlySet<string>,
    visibleServers: readonly ServerConfig[],
  ) => void;
  readonly deleteSubscription: (sub: SubscriptionConfig) => void;
  /**
   * 组网接入面上那两颗 WARP 次动作（2026-09-13 批 16 接通）。桌面同一条腿
   * （`use-node-deletion.ts#removeWarpNode`）—— 它本身就是「`deleteNode` 换三句文案、
   * 外加一个 `afterDelete`」，故这里也不另起编排，直接复用本模块的 `runDeleteOne`：
   * 暂存事务 / 写入路由分流 / 兜底改选三段一字不改。
   */
  readonly removeWarpNode: (server: ServerConfig, opts: MobileWarpRemovalOptions) => void;
}

export function useMobileNodeDeletion(deps: MobileDeletionDeps): MobileDeletion {
  const { t, clearNotice, confirm, dismiss, exitBatch, runWrite } = deps;
  const servers = useEffectiveServers();
  const diskServers = useAppStore((s) => s.servers);
  const selectedServerId = useAppStore((s) => s.selectedServerId);
  const config = useEffectiveConfig();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const revertStaged = useStagedConfigStore((s) => s.revert);

  /** 只在暂存里、盘上还没有的那批（判据与节点行上「待保存」角标同一函数）。 */
  const effectiveIds = new Set(servers.map((s) => s.id));
  const diskIds = new Set(diskServers.map((s) => s.id));
  const stagedOnly = new Set(servers.filter((s) => !diskIds.has(s.id)).map((s) => s.id));
  /** 已暂存删除的：盘上还在、effective 里已经没有。兜底出口不许命中它们。 */
  const stagedDeleted = new Set(
    diskServers.filter((s) => !effectiveIds.has(s.id)).map((s) => s.id),
  );
  /** 写入路由只在持有统一暂存态的入口裁定；下游纯函数不再自己解读开关（同桌面）。 */
  const policy = { all: editRoute('servers', stagingEnabled) };

  /** 把一批节点删除暂存成完整实体删除意图，兜底 id 附着在同一条意图上（逐字同桌面）。 */
  const stageServerDeletions = useCallback(
    (targets: readonly ServerConfig[], removedIds: ReadonlySet<string>, groupId?: string): void => {
      const effectiveSelected = config?.selectedServerId;
      const fallback =
        fallbackExitAfterDelete(
          servers,
          effectiveSelected,
          removedIds,
          useLatencyStore.getState().latencyMap,
        ) ?? DIRECT_SERVER_ID;
      for (const server of targets) {
        stage({
          id: `server:${server.id}`,
          kind: 'server',
          label: `${t('common.delete')} ${server.name}`,
          entityPath: ['servers', server.id],
          nextValue: null,
          groupId,
          selectedServerFallback: effectiveSelected === server.id ? fallback : undefined,
        });
      }
    },
    [config?.selectedServerId, servers, stage, t],
  );

  /**
   * 删掉一个节点。`opts` 是**三条成功支共用**的差异面（WARP 那两颗次动作换文案用），缺省
   * 即原来的「删除成功」。`afterDelete` 只在本地撤销或直写真正完成后跑；暂存删除仍待应用，
   * 不得先开重新注册表，否则旧 WARP 仍在盘上占用单例槽。
   * 失败那条（`runWrite` 的 catch）自然跑不到 —— 「重新注册」不许在删失败之后还把注册表打开，
   * 那会让用户对着一台**还在**的 WARP 再注册一台。
   */
  const runDeleteOne = useCallback(
    async (server: ServerConfig, opts?: MobileWarpRemovalOptions): Promise<void> => {
      const okText = opts?.okText ?? t('nodes.deleteSuccess');
      const done = (completed: boolean): void => {
        clearNotice();
        if (completed) {
          toast.success(okText);
          opts?.afterDelete?.();
        }
      };
      const split = splitStagedOnly('server.delete', [server.id], stagedOnly, stagedEntries, 'servers');
      if (split.backend.length === 0) {
        // 全是 staged-only ⇒ 撤销那条暂存条目，零 IPC。
        split.revertEntryIds.forEach(revertStaged);
        done(true);
        return;
      }
      const routes = partitionNodeDeleteRoutes(diskServers, split.backend, policy);
      if (routes.staged.length > 0) {
        split.revertEntryIds.forEach(revertStaged);
        stageServerDeletions(routes.staged, new Set([server.id]));
        done(false);
        return;
      }
      const id = routes.directIds[0];
      if (id === undefined) return;
      const removedIds = new Set([...stagedDeleted, server.id]);
      const fallback = fallbackExitAfterDelete(
        diskServers,
        selectedServerId,
        removedIds,
        useLatencyStore.getState().latencyMap,
      );
      await runWrite(
        async () => {
          await api.server.delete(id, fallback);
          split.revertEntryIds.forEach(revertStaged);
          done(true);
        },
        () => t('nodes.deleteFail'),
      );
    },
    [
      diskServers,
      clearNotice,
      policy,
      runWrite,
      revertStaged,
      selectedServerId,
      stageServerDeletions,
      stagedDeleted,
      stagedEntries,
      stagedOnly,
      t,
    ],
  );

  const deleteNode = useCallback(
    (server: ServerConfig): void => {
      const confirmId = confirm({
        title: t('common.delete'),
        message: t('nodes.deleteConfirmMobile', { name: server.name }),
        confirmLabel: t('common.delete'),
        danger: true,
        onConfirm: () => {
          dismiss(confirmId);
          void runDeleteOne(server);
        },
      });
    },
    [confirm, dismiss, runDeleteOne, t],
  );

  /**
   * 「重新注册 / 注销 WARP」—— 与 `deleteNode` 同一条腿，只换确认文案与成功文案
   * （逐字对位桌面 `use-node-deletion.ts#removeWarpNode`）。
   *
   * ⚠️ 二次确认是**必须**的，且不复用 `nodes.deleteConfirmMobile`：注销一台 WARP 意味着
   * Cloudflare 侧那台匿名设备在本机没有副本了，重新注册会烧一台新的 —— 那不是「删掉一行节点」。
   */
  const removeWarpNode = useCallback(
    (server: ServerConfig, opts: MobileWarpRemovalOptions): void => {
      const confirmId = confirm({
        title: opts.title,
        message: opts.message,
        confirmLabel: t('common.confirm'),
        danger: true,
        onConfirm: () => {
          dismiss(confirmId);
          void runDeleteOne(server, opts);
        },
      });
    },
    [confirm, dismiss, runDeleteOne, t],
  );

  const deleteBatch = useCallback(
    (selectedIds: ReadonlySet<string>, visibleServers: readonly ServerConfig[]): void => {
      /* 射程上界与桌面同一条：勾选集**先与可见集求交** —— 勾选集不随 tab 切换 / 筛选收窄而收缩，
         直接消费它会删掉用户此刻看不见的节点。求交后为空 ⇒ 连确认都不该弹。 */
      const ids = new Set(selectedVisibleIds(visibleServers, selectedIds));
      if (ids.size === 0) return;
      const confirmId = confirm({
        title: t('common.delete'),
        message: t('nodes.batchDeleteConfirmMobile', { count: ids.size }),
        confirmLabel: t('common.delete'),
        danger: true,
        onConfirm: () => {
          dismiss(confirmId);
          const split = splitStagedOnly(
            'server.deleteBatch',
            [...ids],
            stagedOnly,
            stagedEntries,
            'servers',
          );
          const routes = partitionNodeDeleteRoutes(diskServers, split.backend, policy);
          void runWrite(
            async () => {
              if (routes.directIds.length > 0) {
                const removedIds = new Set([...stagedDeleted, ...ids]);
                const fallback = fallbackExitAfterDelete(
                  diskServers,
                  selectedServerId,
                  removedIds,
                  useLatencyStore.getState().latencyMap,
                );
                await api.server.deleteBatch(routes.directIds, fallback);
              }
              split.revertEntryIds.forEach(revertStaged);
              if (routes.staged.length > 0) {
                const groupId =
                  routes.staged.length > 1 ? `serverDeleteBatch:${crypto.randomUUID()}` : undefined;
                stageServerDeletions(routes.staged, ids, groupId);
              }
              exitBatch();
              clearNotice();
              const completed = routes.directIds.length + split.revertEntryIds.length;
              if (completed > 0) toast.success(t('nodes.batchDeleteOk', { count: completed }));
            },
            () => t('nodes.deleteFail'),
          );
        },
      });
    },
    [
      confirm,
      clearNotice,
      diskServers,
      dismiss,
      exitBatch,
      policy,
      runWrite,
      revertStaged,
      selectedServerId,
      stageServerDeletions,
      stagedDeleted,
      stagedEntries,
      stagedOnly,
      t,
    ],
  );

  const deleteSubscription = useCallback(
    (sub: SubscriptionConfig): void => {
      const count = subDeleteNodeCount(diskServers, sub);
      const confirmId = confirm({
        title: t('nodes.subDeleteTitle'),
        message: t('nodes.subDeleteConfirm', { name: sub.name, count }),
        confirmLabel: t('common.delete'),
        danger: true,
        onConfirm: () => {
          dismiss(confirmId);
          /* 订阅的暂存腿与节点不同：删订阅要**连同它的节点**一起进同一个 group，
             否则撤销时会剩下一批没有归属的节点（逐字同桌面 `use-node-subscription-actions.ts`）。 */
          if (editRoute('subscriptions', stagingEnabled) === 'staged') {
            const targets = diskServers.filter((s) => s.subscriptionId === sub.id);
            const removedIds = new Set(targets.map((s) => s.id));
            const groupId = `subscriptionDelete:${crypto.randomUUID()}`;
            stage({
              id: `subscription:${sub.id}`,
              kind: 'subscription',
              label: `${t('common.delete')} ${sub.name}`,
              entityPath: ['subscriptions', sub.id],
              nextValue: null,
              groupId,
            });
            stageServerDeletions(targets, removedIds, groupId);
            clearNotice();
            return;
          }
          void runWrite(
            async () => {
              await api.subscription.delete(sub.id);
              clearNotice();
              toast.success(t('nodes.subDeleteOk', { count }));
            },
            () => t('nodes.deleteFail'),
          );
        },
      });
    },
    [confirm, clearNotice, diskServers, dismiss, runWrite, stage, stageServerDeletions, stagingEnabled, t],
  );

  return { deleteNode, deleteBatch, deleteSubscription, removeWarpNode };
}

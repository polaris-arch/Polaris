// HOT receipts settle directly; only explicit restart scheduling uses the existing attributed lifecycle path.
/**
 * 移动端的**待应用 / 待保存操作条** —— 落进外壳早就备好的 `.m-pending-slot`。
 *
 * # 接线之前那个槽位恒空
 *
 * 三个产生方在移动端俱在（节点变更 / 订阅刷新 / 设置改动），而差集的两条腿（pull 与 push）
 * 一条都没挂 ⇒ `store.pendingChanges` 恒为空对象、`staged-config-store` 的条目没有任何呈现面。
 * 后果是移动端上「保存了但还没进核」这件事**完全不可见**：用户改完设置、界面毫无反应，
 * 而核里跑的还是旧配置。喂数那半在 `app-wiring.ts`（pull + PUSH + started/stopped/自动更新四个触发点），
 * 本文件是它的呈现面。
 *
 * # 判定全部复用桌面那份纯逻辑，本文件不新造一条
 *
 * `components/layout/pending-bar-logic.ts` 的 `composeBarView(A, B)` 是两个正交维度的合成
 * （A=staged 前端未落盘 / B=pending 磁盘 vs 运行核），**不是一个状态机**（硬塞成状态机会得到
 * 16 格里 8 个不可达）。移动端另起一套等于让同一份状态在两个客户端上得到两种解释。
 * 复用的还有：`applyOutcome`（apply 排程结果 → 下一态）、`applyStateOnLifecycle`
 * （本次 applyId/generation 的终态收场，中间 stopped 不判失败）、
 * `APPLY_CONFIRM_TIMEOUT_MS`（12s 兜底）。
 *
 * 桌面 `PendingChangesBar.tsx` 本体不能直接复用：它的外观全落在 `.pending-bar` / `.btn flow` /
 * `.pd-pop` 这些类上，而这些类住在 `prototype.css` / `components.css` —— 桌面那条五层层叠链，
 * `import` 即违约契约 A1（`MobileShell` 头注早登记过这条）。
 *
 * # 三处与桌面**不同**的处置，逐条写明为什么
 *
 * ① **不渲染桌面可点击的明细入口。** 桌面两个 detailKey 的文案分别是「需重启内核生效 · 点击查看明细」与
 *    「点击查看明细」，而明细 popover（`.pd-pop`，逐条撤销 + 逐条标生效类别）本批不做。
 *    移动端保留 B 维的短说明「已保存的改动仍待应用」，让放弃 A 草稿后留下的待应用状态可解释。
 * ② **「重置」的二次确认走 `useConfirmTwice`**（与桌面同一份实现、同一个 2600ms）。这颗按钮一次
 *    误点丢光全部未保存编辑，而移动端的手指比鼠标更容易误点；其余动作都不销毁用户输入，不设闸。
 * ③ **保存冲突就地呈现，不开弹窗。** 桌面在 `conflict !== null` 时开 `StagedConflictDialog` 逐项选择，
 *    弹窗层住在 `components/dialogs/**`（契约 A1 禁入）。不处理的话，用户点「保存」会得到
 *    **什么都没发生**（`save()` 在冲突时把 `saveStatus` 退回 idle 并 return false）——正是本轮在修的
 *    那类静默。这里落成条上的一档：标题说「保存冲突」、副行列出冲突项的名字，两颗按钮
 *    [用我的]（`resolveConflict` 全量）/ [取消]（`dismissConflict`，改动仍留在暂存里）。
 *    ⚠️ 如实登记射程：这是**整批**裁决，不是桌面那样的逐项裁决。逐项面归表单批。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '@/ipc';
import { useRuntimeApplyReconciliation } from '@/components/layout/use-runtime-apply-reconciliation';
import { RestartApplyDialog, type RestartPrompt } from '@/components/dialogs/RestartApplyDialog';
import type { ProxyErrorTranslate } from '@/domain/proxy-error-text';
import type { ProxyLifecycleEvent } from '@/contracts/types';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import {
  APPLY_CONFIRM_TIMEOUT_MS,
  applyOutcome,
  runtimePersistenceRecoveryEnded,
  applyStateOnLifecycle,
  composeBarView,
  pendingChangesCount,
  pendingPhaseOf,
  stagedPhaseOf,
  type ApplyState,
  type BarActionId,
} from '@/components/layout/pending-bar-logic';
import { toast } from '@/lib/error-handler';

const ACTION_LABEL: Record<BarActionId, string> = {
  reset: 'home.pendingResetBtn',
  save: 'home.pendingSaveBtn',
  apply: 'home.pendingApplyBtn',
  retryApply: 'home.pendingRetryBtn',
  dismissApply: 'home.pendingIgnoreBtn',
  retrySave: 'home.pendingRetrySaveBtn',
  retryPersistence: 'home.pendingRetryPersistenceBtn',
};

/** 主按钮（实心）：断流的那颗永远是主按钮，与桌面同口径。 */
const PRIMARY_ACTIONS: ReadonlySet<BarActionId> = new Set<BarActionId>(['apply', 'retryApply', 'retryPersistence']);

/** 「重置」的原地二次确认 key（同桌面 `PendingChangesBar` 的 `RESET_KEY`）。 */
const RESET_KEY = 'mobile-reset-pending';

export function MobilePendingBar(): ReactElement | null {
  const { t } = useTranslation();
  const pending = useAppStore((s) => s.pendingChanges);
  /** 「立即应用」保存后请求 HOT；仅确认重启时允许中断连接。核没在跑时没有应用对象，那颗整颗不渲染（见 `BarInput.coreRunning`）。 */
  const coreRunning = useAppStore((s) => s.proxyStatus?.running ?? false);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const saveStatus = useStagedConfigStore((s) => s.saveStatus);
  const conflict = useStagedConfigStore((s) => s.conflict);
  const [apply, setApply] = useState<ApplyState>({
    phase: 'idle',
    reason: null,
  });
  const [restartPrompt, setRestartPrompt] = useState<RestartPrompt | null>(null);
  const applyRequest = useRef<{
    awaitingReply: boolean;
    cancelled: boolean;
    intentGeneration: number | undefined;
    early: ProxyLifecycleEvent[];
    earlyGeneration: number | undefined;
    earlyStopped: boolean;
  } | null>(null);
  const lifecycleReady = useRef<Promise<void> | null>(null);
  // Adapt the existing minimal translator contract without widening its optional argument.
  const simpleT: ProxyErrorTranslate = (key, fallback) =>
    fallback === undefined ? t(key) : t(key, fallback);
  const translateRef = useRef(simpleT);
  translateRef.current = simpleT;
  const { armed, confirmTwice } = useConfirmTwice();

  useRuntimeApplyReconciliation(apply, setApply, applyRequest, simpleT);

  // Await actual listener registration before issuing Apply; plain start/error broadcasts have
  // no action ownership and therefore must not decide this request's outcome.
  useEffect(() => {
    let disposed = false;
    let offLifecycle: (() => void) | undefined;
    const ready = api.proxy.onLifecycleReady((data) => {
      const request = applyRequest.current;
      if (!request) return;
      if (data.kind === 'stop' && data.terminal === true && data.phase === 'stopped'
          && Number.isSafeInteger(data.mainGeneration)
          && data.mainGeneration >= (request.earlyGeneration ?? Number.POSITIVE_INFINITY)) {
        request.cancelled = true;
        setApply((state) => state.reconciling ? { ...state, reconciling: false } : state);
      }
      if (request.awaitingReply) {
        // Payload retention is bounded, but generation knowledge must never be evicted.
        // Observe explicit Stop against the floor at arrival, before raising it: replaying
        // later from the maximum floor alone would hide an already-observed neutral cancel.
        if (Number.isSafeInteger(data.mainGeneration) && data.mainGeneration > 0) {
          const previous = request.earlyGeneration;
          if (data.kind === 'stop' && data.terminal === true && data.phase === 'stopped'
              && previous !== undefined && data.mainGeneration > previous) {
            request.earlyStopped = true;
          }
          if (data.mainGeneration > (previous ?? 0)) request.earlyGeneration = data.mainGeneration;
        }
        request.early = [...request.early, data].slice(-32);
        return;
      }
      setApply((state) => applyStateOnLifecycle(state, data, translateRef.current) ?? state);
    }).then((off) => {
      if (disposed) off();
      else offLifecycle = off;
    });
    lifecycleReady.current = ready;
    void ready.catch(() => undefined); // onApply reports the registration failure, no unhandled rejection.
    return () => {
      disposed = true;
      offLifecycle?.();
      applyRequest.current = null;
      lifecycleReady.current = null;
    };
  }, []);

  // A running core alone may be a successor or the old generation. A lost attributed event
  // therefore ends as unconfirmed/failed, preserving the existing retry path without false success.
  useEffect(() => {
    if (apply.phase !== 'applying' || apply.applyId === undefined) return;
    const request = applyRequest.current;
    const timer = setTimeout(() => {
      void api.proxy.getStatus().catch(() => null).then(() => {
        if (applyRequest.current !== request) return;
        setApply((state) => state.phase === 'applying' && state.applyId === apply.applyId
          ? { phase: 'failed', reason: null } : state);
      });
    }, APPLY_CONFIRM_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [apply.phase, apply.applyId]);

  /* ── 冲突档：优先于常规合成（用户点了保存却什么都没发生，先把这件事说清楚）─────────── */
  if (conflict !== null && conflict.length > 0) {
    return (
      <div className="m-pending m-pending-err" role="status" aria-live="polite">
        <div className="m-pending-text">
          <b>{t('home.stagedConflictTitle')}</b>
          <span className="m-pending-detail">{conflict.map((c) => c.label).join(' · ')}</span>
        </div>
        <div className="m-pending-acts">
          <button
            type="button"
            className="m-pending-btn"
            onClick={() => useStagedConfigStore.getState().dismissConflict()}
          >
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="m-pending-btn m-pending-primary"
            onClick={() =>
              void useStagedConfigStore.getState().resolveConflict(conflict.map((c) => c.entryId))
            }
          >
            {t('home.stagedConflictMine')}
          </button>
        </div>
      </div>
    );
  }

  const pendingPhase = pendingPhaseOf(apply.phase, pending);
  const view = composeBarView({
    stagedPhase: stagedPhaseOf(saveStatus, stagedEntries.length),
    pendingPhase,
    stagedCount: stagedEntries.length,
    pendingCount: pendingChangesCount(pending),
    applyError: apply.reason,
    coreRunning,
    persistenceRetryAvailable: !!apply.transactionId,
    persistenceOnly: apply.persistenceOnly,
    persistenceRecoveryEnded: apply.persistenceRecoveryEnded,
  });
  if (!view.visible && restartPrompt === null) return null;

  const onApply = async (restartConfirmation?: string, persistenceTransactionId?: string): Promise<void> => {
    if (applyRequest.current?.awaitingReply || apply.phase === 'applying') return;
    setRestartPrompt(null);
    const generation = useAppStore.getState().proxyStatus?.mainGeneration;
    const initial: ApplyState = { phase: 'applying', reason: null,
      persistenceOnly: persistenceTransactionId !== undefined, transactionId: persistenceTransactionId,
      ...(Number.isSafeInteger(generation) && (generation ?? 0) > 0 ? { mainGeneration: generation } : {}) };
    const request = { awaitingReply: true, cancelled: false,
      intentGeneration: undefined as number | undefined,
      early: [] as ProxyLifecycleEvent[],
      earlyGeneration: initial.mainGeneration, earlyStopped: false };
    applyRequest.current = request;
    setApply(initial);
    try {
      if (!lifecycleReady.current) throw new Error('APPLY_LIFECYCLE_LISTENER_UNAVAILABLE');
      await lifecycleReady.current;
      if (applyRequest.current !== request) return;
      const result = persistenceTransactionId === undefined
        ? await useStagedConfigStore.getState().applyNow(restartConfirmation)
        : { ...await api.proxy.retryRuntimePersistence(persistenceTransactionId), saved: true };
      if (applyRequest.current !== request) return;
      request.awaitingReply = false;
      if (!result.saved) {
        setApply({ phase: 'idle', reason: null });
        return;
      }
      if ((result.status === 'hotApplied' || result.status === 'hotDurabilityPending' || result.status === 'unconfirmed')
          && (request.cancelled || (request.earlyGeneration ?? 0) > (initial.mainGeneration ?? 0))) {
        request.early = [];
        setApply({ ...initial, phase: 'failed', reason: t('home.pendingHotUnconfirmed') });
        return;
      }
      if (persistenceTransactionId !== undefined
          && (result.transactionId !== persistenceTransactionId
            || !['hotApplied', 'hotDurabilityPending', 'conflict', 'rejected', 'unconfirmed'].includes(result.status ?? ''))) {
        request.early = [];
        setApply({ ...initial, phase: 'failed', reason: t('home.pendingHotUnconfirmed') });
        return;
      }
      if (result.status === 'restartRequired') {
        request.early = [];
        // Missing/empty token must never turn into implicit restart permission.
        if (!result.restartConfirmation) {
          setApply({ ...initial, phase: 'failed', reason: t('home.pendingHotUnconfirmed') });
          return;
        }
        setRestartPrompt({ token: result.restartConfirmation, reasons: result.reasons ?? [] });
        setApply({ phase: 'idle', reason: null });
        return;
      }
      const reconcileOwnerKnown = Number.isSafeInteger(result.runtimeGeneration) && (result.runtimeGeneration ?? 0) > 0
        && Number.isSafeInteger(result.selectionIntentGeneration) && (result.selectionIntentGeneration ?? -1) >= 0;
      request.intentGeneration = result.selectionIntentGeneration;
      const outcome = applyOutcome(result.status, result.reasons);
      let next: ApplyState = { ...initial, phase: outcome.phase,
        reason: outcome.phase === 'failed' && outcome.toast ? t(outcome.toast.key) : null,
        transactionId: result.transactionId,
        persistenceRecoveryEnded: persistenceTransactionId !== undefined
          && runtimePersistenceRecoveryEnded(result.status, result.reasons),
        mainGeneration: result.runtimeGeneration ?? initial.mainGeneration,
        reconciling: !request.cancelled && reconcileOwnerKnown && !!result.transactionId && (result.status === 'hotDurabilityPending' || result.status === 'unconfirmed') };
      if (outcome.phase === 'applying') {
        if (typeof result.applyId !== 'number' || !Number.isSafeInteger(result.applyId) || result.applyId <= 0) {
          next = { phase: 'failed', reason: null };
        } else {
          next.applyId = result.applyId;
          if (request.earlyStopped) {
            next = { phase: 'idle', reason: null };
          } else {
            // A retained old terminal cannot settle below a newer observation that overflow
            // evicted. Missing payload remains pending for a future owned event or timeout.
            next.mainGeneration = request.earlyGeneration;
            for (const event of request.early) next = applyStateOnLifecycle(next, event, translateRef.current) ?? next;
          }
        }
      }
      request.early = [];
      setApply(next);
      // Preserve the mobile policy: an Apply racing Stop stays neutral without a skipped toast.
      if (outcome.toast !== null && result.status !== 'skipped') toast[outcome.toast.kind](t(outcome.toast.key));
    } catch (error) {
      if (applyRequest.current !== request) return;
      request.awaitingReply = false;
      request.early = [];
      setApply({ ...initial, phase: 'failed', reason: null });
      console.error('[MobilePendingBar] Apply failed:', error);
    }
  };

  const runAction = (id: BarActionId): void => {
    switch (id) {
      case 'reset':
        confirmTwice(RESET_KEY, () => {
          const hadDrafts = useStagedConfigStore.getState().entries.length > 0;
          useStagedConfigStore.getState().reset();
          if (hadDrafts && useStagedConfigStore.getState().entries.length === 0) {
            toast.success(t('home.pendingResetDone'));
          }
        });
        return;
      case 'save':
      case 'retrySave':
        void useStagedConfigStore.getState().save();
        return;
      case 'apply':
      case 'retryApply':
        void onApply();
        return;
      case 'retryPersistence':
        if (apply.transactionId) void onApply(undefined, apply.transactionId);
        return;
      case 'dismissApply':
        setApply({ phase: 'idle', reason: null });
    }
  };

  return (
    <div
      className={`m-pending${view.err ? ' m-pending-err' : ''}`}
      role="status"
      aria-live="polite"
    >
      <div className="m-pending-text">
        <b>{armed === RESET_KEY ? t('home.pendingResetShortConfirm') : t(view.titleKey, view.titleVars)}</b>
        {pendingPhase === 'pending' && (
          <span className="m-pending-detail">{t('home.pendingSavedStillNeedsApply')}</span>
        )}
      </div>
      <div className="m-pending-acts">
        {view.actions.map((a) => {
          const arming = a.id === 'reset' && armed === RESET_KEY;
          return (
            <button
              key={a.id}
              type="button"
              className={`m-pending-btn${PRIMARY_ACTIONS.has(a.id) ? ' m-pending-primary' : ''}${
                arming ? ' m-pending-confirming confirming' : ''
              }`}
              disabled={a.disabled}
              onClick={() => runAction(a.id)}
            >
              {arming ? t('common.confirm') : t(a.id === 'dismissApply' && apply.persistenceRecoveryEnded ? 'home.pendingExitPersistenceRecoveryBtn' : ACTION_LABEL[a.id])}
            </button>
          );
        })}
      </div>
      {restartPrompt !== null && (
        <RestartApplyDialog prompt={restartPrompt}
          onDismiss={() => setRestartPrompt(null)}
          onConfirm={() => void onApply(restartPrompt.token)} />
      )}
    </div>
  );
}

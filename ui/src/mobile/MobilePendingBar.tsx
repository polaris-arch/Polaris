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
 * （「应用中…」的收场，含「failed 带原因 / stopped 不判失败」）、`isRestartFailureCode`、
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

import { useEffect, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '@/ipc';
import { proxyErrorReason } from '@/domain/proxy-error-text';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import {
  APPLY_CONFIRM_TIMEOUT_MS,
  applyOutcome,
  applyStateOnLifecycle,
  composeBarView,
  isRestartFailureCode,
  pendingChangesCount,
  pendingPhaseOf,
  stagedPhaseOf,
  type ApplyPhase,
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
};

/** 主按钮（实心）：断流的那颗永远是主按钮，与桌面同口径。 */
const PRIMARY_ACTIONS: ReadonlySet<BarActionId> = new Set<BarActionId>(['apply', 'retryApply']);

/** 「重置」的原地二次确认 key（同桌面 `PendingChangesBar` 的 `RESET_KEY`）。 */
const RESET_KEY = 'mobile-reset-pending';

export function MobilePendingBar(): ReactElement | null {
  const { t } = useTranslation();
  const pending = useAppStore((s) => s.pendingChanges);
  /** 「立即应用」= 保存 + 强制重启核；核没在跑时后半句无对象，那颗整颗不渲染（见 `BarInput.coreRunning`）。 */
  const coreRunning = useAppStore((s) => s.proxyStatus?.running ?? false);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const saveStatus = useStagedConfigStore((s) => s.saveStatus);
  const conflict = useStagedConfigStore((s) => s.conflict);
  const [apply, setApply] = useState<{ phase: ApplyPhase; reason: string | null }>({
    phase: 'idle',
    reason: null,
  });
  const { armed, confirmTwice } = useConfirmTwice();

  /* 「应用中…」的收场。三条腿都**只在 `applying` 时改态** —— 别的时候来的起停/报错是别人的事
     （首页连接按钮、后端自愈重启），把它算成「应用失败」会红得毫无道理。
     主腿是 `event:proxyLifecycle`（后端在真状态跃迁点发，覆盖「立即应用」触发的后端自驱去抖重启，
     那条路径 started/error 两个事件一个都不发）。 */
  useEffect(() => {
    const simpleT = t as (key: string, fallback?: string) => string;
    const offStarted = api.proxy.onStarted(() =>
      setApply((a) => (a.phase === 'applying' ? { phase: 'idle', reason: null } : a)),
    );
    const offError = api.proxy.onError((data) =>
      setApply((a) =>
        a.phase === 'applying' && isRestartFailureCode(data.errorCode)
          ? { phase: 'failed', reason: proxyErrorReason(data, simpleT) }
          : a,
      ),
    );
    const offLifecycle = api.proxy.onLifecycle((data) =>
      setApply((a) => applyStateOnLifecycle(a.phase, data, simpleT) ?? a),
    );
    return () => {
      offStarted();
      offError();
      offLifecycle();
    };
    // `t` 入依赖：切界面语言后失败原因要按新语种解。三个 off 都是幂等注销。
  }, [t]);

  /* 「应用中…」的最后一道兜底：事件可能送不到（WebView 被系统回收重建、订阅还没挂上），
     而落空的代价是条永久转圈。到点**查一次真运行态**再判，不盲目超时判失败。 */
  useEffect(() => {
    if (apply.phase !== 'applying') return;
    const timer = setTimeout(() => {
      void api.proxy
        .getStatus()
        .then((s) =>
          setApply((a) =>
            a.phase !== 'applying'
              ? a
              : s?.running
                ? { phase: 'idle', reason: null }
                : { phase: 'failed', reason: null },
          ),
        )
        .catch(() =>
          setApply((a) => (a.phase === 'applying' ? { phase: 'failed', reason: null } : a)),
        );
    }, APPLY_CONFIRM_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [apply.phase]);

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
  });
  if (!view.visible) return null;

  const onApply = async (): Promise<void> => {
    setApply({ phase: 'applying', reason: null });
    const r = await useStagedConfigStore.getState().applyNow();
    if (!r.saved) {
      // 保存那一半就没过 ⇒ 核根本没碰。条已由 A 维度显示「保存失败」，这里再叠一个「应用失败」
      // 会让人以为炸了两回。
      setApply({ phase: 'idle', reason: null });
      return;
    }
    const outcome = applyOutcome(r.status);
    setApply({ phase: outcome.phase, reason: null });
    if (outcome.toast !== null) toast[outcome.toast.kind](t(outcome.toast.key));
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
              {arming ? t('common.confirm') : t(ACTION_LABEL[a.id])}
            </button>
          );
        })}
      </div>
    </div>
  );
}

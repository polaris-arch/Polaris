/**
 * 全局待应用/待保存操作条（原型 `.pending-bar`：CSS prototype.css:551-564 / markup L2447-2455 / 行为 L3277-3310）。
 *
 * **为什么在 layout 而不在 Home**（P2，spec §2.6 P1 / U-6）：待应用差集是**全局**语义 —— 规则页、设置页、
 * 节点页都能产生它。此前它是 `HomeScreen` 的内联卡片（靠 `screens.css` 的 `#s-home > .pending-bar`
 * override 掉 docked 专有属性），后果是「在设置页改完东西看不到任何反馈」。现挂 `AppShell` 的 `<main>` 内、
 * `.main-scroll` 与 `<StatusBar />` 之间 —— 与 `AppUpdateBanner`（`main` 顶）分居 flex 列两端，中间
 * `.main-scroll` 吸收剩余高度，两者互不挤压。因是 `main` 的直接子元素，**结构上不可能跨到侧栏上**（原型注释）。
 *
 * # 条的态 = 两个正交维度的合成，不是一个状态机（spec §2.4）
 *
 * - **A：staged**（前端，磁盘之前）—— 来自 `staged-config-store`，「重置 / 保存」作用于它。
 * - **B：pending**（磁盘 vs 运行核）—— 来自 `store.pendingChanges` + 本次 apply 的生命周期，「立即应用」作用于它。
 *
 * 硬塞成一个状态机会得到 16 个态里 8 个不可达（spec 原话）。故文案与按钮集全部由纯函数
 * `composeBarView(A, B)` 派生，本组件只负责「取两个维度的值 + 把动作接回去」。
 *
 * **`.show` 必须显式挂**：docked 基线是 `height:0; overflow:hidden`（折叠态），`.show` 才给 36px。
 * 内联时代靠 `#s-home > .pending-bar{height:auto}` 顶掉这条，搬出 `#s-home` 后那条 override 不再命中本条
 * ⇒ 不挂 `.show` 就是一条**恒不可见**的条。由 style-invariants 门钉住。
 *
 * # 「应用中…」的收场不由 `applyPendingChanges` 的返回值定（C-8 / spec §2.5 Q8）
 *
 * `applied` 只意味着 `schedule_restart()` 已排程，**重启到底成没成走的是另一条通道**。故点完「立即应用」
 * 后条进 `applying`、不报成功。
 *
 * **收场只接受 `event:proxyLifecycle` 的本次 applyId + 有效 generation**：
 * restart 的中间 stopped 不收场；匹配的 terminal ready/failed 才决定结果。
 * 较新的显式 stop 中性取消本次等待。无归属的 started/error 广播不能证明本次 Apply 完成。
 * 监听先注册，早于命令回执到达的事件先有界缓存，再绑定回执里的 force_restart_seq。
 *
 * `.err` 态（原型 `:531`/`:534`，此前是**零使用点的死 CSS**）：toast 是 fire-and-forget（2.2s 自动消失），
 * 失败信息一闪即逝；条本身转红才是持续可见的那一份。重试 / 忽略即清红 —— 条只反映**最近一次**尝试的
 * 结果，不累积历史失败。
 */

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { StagedConflictDialog } from '@/components/dialogs/StagedConflictDialog';
import { replay } from '@/lib/staged-config';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { cn } from '@/lib/utils';
import { useConfirmTwice } from '@/lib/confirm-twice';
import type { ProxyLifecycleEvent } from '@/contracts/types';
import type { ProxyErrorTranslate } from '@/domain/proxy-error-text';
import {
  APPLY_CONFIRM_TIMEOUT_MS,
  applyOutcome,
  applyStateOnLifecycle,
  composeBarView,
  hasPendingWork,
  pendingChangesCount,
  pendingPhaseOf,
  stagedPhaseOf,
  type ApplyState,
  type BarActionId,
} from './pending-bar-logic';

const ACTION_LABEL: Record<BarActionId, string> = {
  reset: 'home.pendingResetBtn',
  save: 'home.pendingSaveBtn',
  apply: 'home.pendingApplyBtn',
  retryApply: 'home.pendingRetryBtn',
  dismissApply: 'home.pendingIgnoreBtn',
  retrySave: 'home.pendingRetrySaveBtn',
};

/** `btn flow`（主）vs `btn ghost`（次）。断流的那颗永远是主按钮，与今天的单按钮形态一致。 */
const PRIMARY_ACTIONS: ReadonlySet<BarActionId> = new Set<BarActionId>(['apply', 'retryApply']);

/**
 * 「重置」的原地二次确认 key（原型 :4070 `reset-pending` → `confirmTwice(t,'放弃全部待应用改动？',…)`）。
 *
 * 为什么只有这一颗要闸：本条常驻主界面底部，重置与「保存」「立即应用」并排且同尺寸，一次误点
 * **丢光全部未保存编辑**；而 popover 里的逐条 `pd-x` 撤销（:296-303）只丢一条，不设闸。
 * 其余动作（保存/应用/重试/忽略）都不销毁用户输入，不该分摊弹窗疲劳。
 */
const RESET_KEY = 'reset-pending';

export default function PendingChangesBar() {
  const { t } = useTranslation();
  const pending = useAppStore((s) => s.pendingChanges);
  /** 「立即应用」= 保存 + 强制重启核；核没在跑时后半句无对象，故那颗整颗不渲染（见 BarInput.coreRunning）。 */
  const coreRunning = useAppStore((s) => s.proxyStatus?.running ?? false);
  const servers = useAppStore((s) => s.servers);
  /** **裸磁盘 config**（不是 `effectiveConfig`）：下面 FR-9 那轮 `classifyStaged` 问的是
   *  「**只有这一条**时保存后是否还待应用」，重放基准必须是盘。喂 `effectiveConfig`
   *  等于把整批条目先叠一遍再叠这一条，逐条标注会全部失真。 */
  const config = useAppStore((s) => s.config);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const saveStatus = useStagedConfigStore((s) => s.saveStatus);
  const conflict = useStagedConfigStore((s) => s.conflict);
  /** B 维度里属于「这一次 apply」的那部分。`reason` 只在 `failed` 时有意义。 */
  const [apply, setApply] = useState<ApplyState>({
    phase: 'idle',
    reason: null,
  });
  const applyRequest = useRef<{
    awaitingReply: boolean;
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
  /** 明细 popover 开合（原型 `.pd-pop`）。 */
  const [detailOpen, setDetailOpen] = useState(false);
  /** staged 条目 id → 「保存后仍需应用」（FR-9）。当前 Apply 会重启核；缺 key = 尚未判出，不猜。 */
  const [effects, setEffects] = useState<Record<string, boolean>>({});
  const { armed, confirmTwice } = useConfirmTwice();
  const barRef = useRef<HTMLDivElement>(null);
  const n = pendingChangesCount(pending);

  /* 点外部 / Esc 关闭（对齐首页出口选单、节点页 add-menu 的既有模式）。 */
  useEffect(() => {
    if (!detailOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!barRef.current?.contains(e.target as Node)) setDetailOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setDetailOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [detailOpen]);

  // Await actual listener registration before issuing Apply; plain start/error broadcasts have
  // no action ownership and therefore must not decide this request's outcome.
  useEffect(() => {
    let disposed = false;
    let offLifecycle: (() => void) | undefined;
    const ready = api.proxy.onLifecycleReady((data) => {
      const request = applyRequest.current;
      if (!request) return;
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

  /* FR-9：逐条标注「仅需保存 / 保存后仍需应用」。**只在 popover 打开时问**——它是 N 次只读 IPC，
     挂在条目变化上会让每次表单提交都打一轮。 */
  useEffect(() => {
    if (!detailOpen || stagedEntries.length === 0 || config === null) return;
    let alive = true;
    void Promise.all(
      stagedEntries.map(async (e) => {
        // 单条重放后问：拿到的是「只有这一条时保存后是否还待应用」。
        const c = await api.config.classifyStaged(replay(config, [e])).catch(() => null);
        return [e.id, c === null ? null : c.restartRequired] as const;
      })
    ).then((rows) => {
      if (!alive) return;
      setEffects(
        Object.fromEntries(rows.filter((r): r is readonly [string, boolean] => r[1] !== null))
      );
    });
    return () => {
      alive = false;
    };
  }, [detailOpen, stagedEntries, config]);

  const view = composeBarView({
    stagedPhase: stagedPhaseOf(saveStatus, stagedEntries.length),
    pendingPhase: pendingPhaseOf(apply.phase, pending),
    stagedCount: stagedEntries.length,
    pendingCount: n,
    applyError: apply.reason,
    coreRunning,
  });
  /* 冲突弹窗（Q8-b 第 4 步）。挂在条上而不是 DialogHost 栈里：它的生命周期完全由 store 的
     `conflict` 字段决定（保存那一步开、裁决/取消关），走命令式 push/pop 会多出一个能与该字段
     分叉的真值。渲染位置无所谓 —— `Modal` 是原生 `<dialog>` + top-layer。
     **必须排在 `view.visible` 早退之前**：条被隐藏时弹窗也得能显示，否则用户会遇到
     「点了保存什么也没发生」。 */
  const conflictDialog =
    conflict === null ? null : (
      <StagedConflictDialog
        conflicts={conflict}
        onResolve={(ids) => void useStagedConfigStore.getState().resolveConflict(ids)}
        onDismiss={() => useStagedConfigStore.getState().dismissConflict()}
      />
    );
  if (!view.visible) return conflictDialog;

  /** n===0 而差集腿仍有活 ⇒ 唯一可能是 `restartDeferred`：明细里没有 id 可列，给一行说明。 */
  const configOnly = n === 0 && hasPendingWork(pending);

  const onSave = async () => {
    await useStagedConfigStore.getState().save();
  };

  const onApply = async () => {
    if (applyRequest.current?.awaitingReply || apply.phase === 'applying') return;
    const generation = useAppStore.getState().proxyStatus?.mainGeneration;
    const initial: ApplyState = { phase: 'applying', reason: null,
      ...(Number.isSafeInteger(generation) && (generation ?? 0) > 0 ? { mainGeneration: generation } : {}) };
    const request = { awaitingReply: true, early: [] as ProxyLifecycleEvent[],
      earlyGeneration: initial.mainGeneration, earlyStopped: false };
    applyRequest.current = request;
    setApply(initial);
    try {
      if (!lifecycleReady.current) throw new Error('APPLY_LIFECYCLE_LISTENER_UNAVAILABLE');
      await lifecycleReady.current;
      if (applyRequest.current !== request) return;
      const result = await useStagedConfigStore.getState().applyNow();
      if (applyRequest.current !== request) return;
      request.awaitingReply = false;
      if (!result.saved) {
        setApply({ phase: 'idle', reason: null });
        return;
      }
      const outcome = applyOutcome(result.status);
      let next: ApplyState = { ...initial, phase: outcome.phase };
      if (outcome.phase === 'applying') {
        if (!Number.isSafeInteger(result.applyId) || (result.applyId ?? 0) <= 0) {
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
      if (outcome.toast !== null) toast[outcome.toast.kind](t(outcome.toast.key));
    } catch (error) {
      if (applyRequest.current !== request) return;
      request.awaitingReply = false;
      request.early = [];
      setApply({ phase: 'failed', reason: null });
      console.error('[PendingChangesBar] Apply failed:', error);
    }
  };

  const runAction = (id: BarActionId): void => {
    switch (id) {
      case 'reset':
        // 原地二次点击（`lib/confirm-twice.ts`），不弹窗 —— 见 RESET_KEY 头注。
        confirmTwice(RESET_KEY, () => useStagedConfigStore.getState().reset());
        return;
      case 'save':
      case 'retrySave':
        void onSave();
        return;
      case 'apply':
      case 'retryApply':
        void onApply();
        return;
      case 'dismissApply':
        setApply({ phase: 'idle', reason: null });
        return;
    }
  };

  /** 转圈只挂在「正在跑的那颗」上：保存在飞 → 保存钮，重启在飞 → 立即应用钮。 */
  const spins = (id: BarActionId): boolean =>
    (id === 'save' && saveStatus === 'saving') || (id === 'apply' && apply.phase === 'applying');

  /** 差集里存的是 serverId。节点可能刚被删（差集是快照、config 是当下）→ 回落显 id 而非空行：
   *  「有一项待应用但说不出是哪个」也比凭空少一行诚实。 */
  const nameOf = (id: string): string => servers.find((s) => s.id === id)?.name ?? id;

  // 三组顺序 = 计数口径顺序（pendingChangesCount），保证「条上的数字 = popover 行数」逐项对得上。
  const groups: { key: string; labelKey: string; ids: string[] }[] = [
    { key: 'added', labelKey: 'home.pendingGroupAdded', ids: pending?.added ?? [] },
    { key: 'modified', labelKey: 'home.pendingGroupModified', ids: pending?.modified ?? [] },
    { key: 'removed', labelKey: 'home.pendingGroupRemoved', ids: pending?.removed ?? [] },
  ].filter((g) => g.ids.length > 0);

  return (
    <div
      className={cn('pending-bar show', view.err && 'err')}
      role="status"
      aria-live="polite"
      ref={barRef}
    >
      <span className="pb-ic" aria-hidden>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <path d="M10.3 3.9 2.4 18a1.9 1.9 0 0 0 1.7 2.9h15.8A1.9 1.9 0 0 0 21.6 18L13.7 3.9a1.9 1.9 0 0 0-3.4 0Z" strokeLinejoin="round" />
          <path d="M12 9v4" strokeLinecap="round" />
          <circle cx="12" cy="16.6" r="0.9" fill="currentColor" stroke="none" />
        </svg>
      </span>
      {/* `.pb-tx` 一直带着 `cursor:pointer`（prototype.css:564）却没有任何 onClick —— 假可点。
          真正缺的不是把手型去掉，而是它本该开的那个明细：「3 项待应用」不说是哪三项，用户没法判断
          现在重启值不值。接上 popover 后手型才名副其实。 */}
      <button
        type="button"
        className="pb-tx"
        aria-expanded={detailOpen}
        aria-haspopup="true"
        data-tip={t('home.pendingDetailTip')}
        onClick={() => setDetailOpen((v) => !v)}
      >
        <b>{t(view.titleKey, view.titleVars)}</b>
        {view.detailKey !== '' && <div>{t(view.detailKey)}</div>}
      </button>
      <div className="pb-acts">
        {view.actions.map((a) => {
          // 武装态：翻实心红（`.btn.confirming`）+ 换文案，与原型 confirmTwice 对文字按钮的两条动作一致。
          const arming = a.id === 'reset' && armed === RESET_KEY;
          return (
            <button
              key={a.id}
              type="button"
              className={cn(
                'btn sm',
                PRIMARY_ACTIONS.has(a.id) ? 'flow' : 'ghost',
                arming && 'confirming',
              )}
              disabled={a.disabled}
              onClick={() => runAction(a.id)}
            >
              {spins(a.id) && <span className="spinner" />}
              {arming ? t('home.pendingResetConfirm') : t(ACTION_LABEL[a.id])}
            </button>
          );
        })}
      </div>
      {/* 明细列表（原型 `.pd-pop`/`.pd-row`/`.pd-h`）。
          **向上弹**（原型 :3308 `top = r.top - pop.height - 6`）：条 docked 在窗口底部贴状态栏，
          向下弹整块出屏。用 `bottom:calc(100% + 6px)` 表达，无需测量。 */}
      {detailOpen && (
        <div className="pd-pop" style={{ bottom: 'calc(100% + 6px)', left: 0 }} role="group">
          {/* 待保存组：逐条可撤销（FR-3）+ 逐条标生效类别（FR-9）。撤销走「移除后重放」，
              效果等价于「这一条从未加入过」。 */}
          {stagedEntries.length > 0 && (
            <div>
              <div className="pd-h">{t('home.pendingGroupStaged')}</div>
              {stagedEntries.map((e) => (
                <div className="pd-row" key={e.id}>
                  <span className="dot idle" aria-hidden />
                  <span className="pd-l" data-tip={e.label}>
                    {e.label}
                  </span>
                  <span className="pd-eff">
                    {e.id in effects
                      ? t(
                          effects[e.id]
                            ? 'home.pendingEffectNeedsRestart'
                            : 'home.pendingEffectOnSave'
                        )
                      : ''}
                  </span>
                  <button
                    type="button"
                    className="pd-x"
                    data-tip={t('home.pendingRevertOne')}
                    aria-label={t('home.pendingRevertOne')}
                    onClick={() => {
                      // 原型 `:4072` 撤销后 notify「已撤销该项改动」。缺它时唯一反馈是「这一行消失了」——
                      // 与「点歪了、点到别的行」在观感上无从区分，而这条动作不可撤销（撤销没有撤销）。
                      useStagedConfigStore.getState().revert(e.id);
                      toast.success(t('home.pendingRevertedOne'));
                    }}
                  >
                    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                      <path d="M18 6 6 18M6 6l12 12" />
                    </svg>
                  </button>
                </div>
              ))}
            </div>
          )}
          {/* 非节点结构性变更没有 id 可列（改的是端口/TUN/DNS 这类整体设置）→ 给一行说明。
              没有这一行，明细 popover 在 configOnly 态下是**空白框**：比不显示更让人以为出了 bug。 */}
          {configOnly && (
            <div>
              <div className="pd-h">{t('home.pendingGroupConfig')}</div>
            </div>
          )}
          {groups.map((g) => (
            <div key={g.key}>
              <div className="pd-h">{t(g.labelKey)}</div>
              {g.ids.map((id) => (
                <div className="pd-row" key={id}>
                  <span className="dot idle" aria-hidden />
                  <span className="pd-l" data-tip={nameOf(id)}>
                    {nameOf(id)}
                  </span>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
      {conflictDialog}
    </div>
  );
}

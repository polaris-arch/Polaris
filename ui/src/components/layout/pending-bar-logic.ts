import type { PendingApplyResult, PendingNodeChanges, ProxyLifecycleEvent } from '@/contracts/types';
import {
  proxyErrorReason,
  type ProxyErrorTranslate,
} from '@/domain/proxy-error-text';

/**
 * 待应用差集操作条纯逻辑（抽出以便 node 环境 vitest 直测，同 connect-button-state.ts / status-bar-display.ts）。
 *
 * 与组件同居 `components/layout/`：条已从 Home 内联卡片迁到 `AppShell` 的 docked 全局位（P2），
 * 本文件是它的唯一消费方。
 */

/**
 * 操作条计数「N 项待应用」= added + modified + removed —— 三个集合都是「运行核尚未吃进去的差异」，
 * 都能被「立即应用」那颗按钮解决，故都该计入；漏计哪一类，条上的数字就与明细 popover 的行数对不上。
 *
 * 缺字段（核未运行/IPC 降级时的畸形对象）→ 该项按 0 降级，绝不抛。
 */
export function pendingChangesCount(pending: PendingNodeChanges): number {
  return (
    (pending?.added?.length ?? 0) +
    (pending?.modified?.length ?? 0) +
    (pending?.removed?.length ?? 0)
  );
}

/**
 * 条该不该出现 —— **不等于** `pendingChangesCount(pending) > 0`。
 *
 * `restartDeferred` 是「保存只持久化」延后的非节点结构性变更（`mixedPort` / TUN / DNS …）：
 * 它一个节点都不动，三个数组恒空，但确实需要重启才生效。只按计数判可见性，
 * 「保存」在 UI 上就完全无痕 —— 用户保存了、什么也没发生、也没人说还差一步。
 */
export function hasPendingWork(pending: PendingNodeChanges): boolean {
  return pendingChangesCount(pending) > 0 || pending?.restartDeferred === true;
}

/** HOT receipt outcomes plus the backward-compatible explicit restart scheduling states. */
export type ApplyStatus = PendingApplyResult['status'];

/** 条的 B 维度里由「这一次 apply」驱动的那部分（§2.4 维度 B 的 applying / applyFailed 两态）。 */
export type ApplyPhase = 'idle' | 'applying' | 'failed' | 'durabilityPending';

export interface ApplyOutcome {
  phase: ApplyPhase;
  /** 顺带的 toast；`null` = 不弹。 */
  toast: { kind: 'info' | 'error'; key: string } | null;
}

/**
 * apply 排程结果 → 条的下一态 + 是否顺带 toast。
 *
 * **`applied` 不再报「已应用」**（spec §2.5 Q8 的第三行）：C-8 —— `apply_pending` 的返回值只表达
 * **排程结果**，`applied` 仅意味着 `schedule_restart()` 已排程，重启到底成没成走的是另一条通道
 * （带 applyId 的 `event:proxyLifecycle`）。此前直接报 success 的写法会在「排程了但核没起来」时
 * 说谎，而那正是最需要如实说的一格。故 applied/deferred 一律进 `applying` 态、**不弹 toast**：
 * 条本身持续显示「应用中…」，而 toast 2.2s 就消失，用它表达一个还在进行中的过程只会误导。
 *
 * `skipped`（核未运行）不是失败，也不进 applying —— 没有重启在飞，下次起核自然纳入，给一次 info 即可。
 * `null`/`undefined`（IPC 失败或未知态）→ 失败态 + error toast，绝不静默吞（点了「立即应用」没反应
 * 与按钮失灵不可区分）。
 */
/**
 * 「应用中…」等多久就去查一次真运行态（兜底出口）。
 *
 * 存在的理由不是「重启可能慢」，而是**有真实失败路径一个事件都不发** —— TUN 模式下重启撞上提权
 * 助手安装门、用户取消时，`runtime/proxy.rs` 那类极早期失败自陈「无 emitter」，分类只随 `invoke`
 * 返回值出栈，而 `applyPendingChanges` 早已返回。没有这条兜底，条会永远停在「应用中…」。
 *
 * 取 12s：起核正常路径（含 TUN 建栈 + 系统代理设置）实测在数秒内；给慢盘/首次授权留一倍余量，
 * 又不至于让用户对着转圈干等太久。到点后重读状态；没有本次归属的完成证据，仍必须保守报未确认失败。
 */
export const APPLY_CONFIRM_TIMEOUT_MS = 12_000;

/** Only the trusted code/path pair selects this restart-source explanation. */
export function runtimeApplyConflictKey(reasons?: PendingApplyResult['reasons']): string {
  return reasons?.some(reason => reason.code === 'OWNER_STATE_SUPERSEDED' && reason.path === 'desired')
    ? 'home.pendingHotOwnerSuperseded' : 'home.pendingHotConflict';
}

/** Only a verified obsolete desired owner ends original-transaction recovery. */
export function runtimePersistenceRecoveryEnded(status: ApplyStatus | null | undefined, reasons?: PendingApplyResult['reasons']): boolean {
  return status === 'conflict' && reasons?.some(reason => reason.code === 'OWNER_STATE_SUPERSEDED' && reason.path === 'desired') === true;
}

export function runtimeApplyRejectedKey(reasons?: PendingApplyResult['reasons']): string {
  if (reasons?.some(reason => reason.code === 'VISION_TRANSPORT_UNSUPPORTED' && reason.path === 'outbounds.vision.transport')) return 'nodes.invalidVisionTransport';
  const gateway = reasons?.find(reason => reason.path === 'inbounds.tun.gateway.forwarding' &&
    (reason.code === 'LAN_GATEWAY_FORWARDING_REQUIRED' || reason.code === 'LAN_GATEWAY_FORWARDING_UNCONFIRMED'));
  return gateway?.code === 'LAN_GATEWAY_FORWARDING_REQUIRED' ? 'errors.lanGatewayForwardingRequired'
    : gateway ? 'errors.lanGatewayForwardingUnconfirmed' : 'home.pendingHotRejected';
}

export function applyOutcome(status: ApplyStatus | null | undefined, reasons?: PendingApplyResult['reasons']): ApplyOutcome {
  switch (status) {
    case 'applied':
    case 'deferred':
      return { phase: 'applying', toast: null };
    case 'hotApplied':
      return { phase: 'idle', toast: { kind: 'info', key: 'home.pendingHotApplied' } };
    case 'hotDurabilityPending':
      return { phase: 'durabilityPending', toast: null };
    case 'restartRequired':
      return { phase: 'idle', toast: null };
    case 'conflict':
      return { phase: 'failed', toast: { kind: 'error', key: runtimeApplyConflictKey(reasons) } };
    case 'rejected':
      return { phase: 'failed', toast: { kind: 'error', key: runtimeApplyRejectedKey(reasons) } };
    case 'unconfirmed':
      return { phase: 'failed', toast: { kind: 'error', key:
        reasons?.some(reason => reason.code === 'CORE_BUNDLE_MISMATCH' && reason.path === 'runtime.protocol')
          ? 'errors.coreBundleMismatch' : 'home.pendingHotUnconfirmed' } };
    case 'skipped':
      return { phase: 'idle', toast: { kind: 'info', key: 'home.pendingSkippedNotRunning' } };
    default:
      return { phase: 'failed', toast: { kind: 'error', key: 'home.pendingApplyFailed' } };
  }
}

/**
 * `event:proxyError` 里**哪些码代表核仍在跑** —— 这几个在 `runtime/proxy.rs` 的 `code` 模块里逐条
 * 注明「非终态」，走 `set_nonfatal_error`（保留 running/pid/端口），不是本次重启没起来。
 *
 * 判据取补集（不在表里的一律算失败）而非正列失败码：漏判一个失败码 ⇒ 条永远停在「应用中…」、
 * 用户没有任何出口；漏判一个非终态码 ⇒ 多一次可点掉的红。两害相权取后者。
 *
 * `CORE_BINARY_MISMATCH` 是**起核后**的内核自证告警（实跑二进制 ≠ 本次期望的核）——核这时已经
 * 起来并在服务流量，只是跑的是旧核。把它误判成「重启没落地」会让条卡死在「应用中…」，而实际上
 * 那次 apply 已经成功了。
 *
 * `AUTO_SWITCH_NEEDS_RESTART` 同理，且与「这一次 apply」更是毫无关系：它由后台自动换节点心跳
 * 在**核已稳定运行**期间发出（可用节点都要整核重启才能切过去）。算成重启失败会让一次早已成功的
 * apply 事后被判红。
 */
const CORE_STILL_RUNNING_CODES: ReadonlySet<string> = new Set([
  'SYSTEM_PROXY_FAILED',
  'SYSTEM_DNS_TAKEOVER_FAILED',
  'EXIT_MISMATCH',
  'RULE_RESOURCES_MISSING',
  'NETWORK_PROFILE_RULES_PRUNED',
  'CORE_BINARY_MISMATCH',
  'AUTO_SWITCH_NEEDS_RESTART',
]);

/** 这条 `event:proxyError` 是否意味着「这次立即应用的重启没落地」。 */
export function isRestartFailureCode(code: string | undefined): boolean {
  return code === undefined || !CORE_STILL_RUNNING_CODES.has(code);
}

/** 条里由「这一次 apply」驱动的那部分状态（`phase` + 失败原因）。 */
export interface ApplyState {
  phase: ApplyPhase;
  applyId?: number;
  /** Only a status read may resolve an ambiguous or durability-pending transaction. */
  reconciling?: boolean;
  transactionId?: string;
  /** Preserve persistence-only recovery across ambiguous/failed replies. */
  persistenceOnly?: boolean;
  /** Confirmed regeneration supersession ends this original recovery, not durability. */
  persistenceRecoveryEnded?: boolean;
  mainGeneration?: number;
  /** 只在 `failed` 时有意义。 */
  reason: string | null;
}

/** Only an attributed terminal event may conclude this Apply request. */
export function applyStateOnLifecycle(
  current: ApplyState,
  event: ProxyLifecycleEvent,
  t: ProxyErrorTranslate
): ApplyState | null {
  if (current.phase !== 'applying' || !Number.isSafeInteger(current.applyId) || (current.applyId ?? 0) <= 0) return null;
  if (!Number.isSafeInteger(event.mainGeneration) || event.mainGeneration <= 0) return null;
  if (event.mainGeneration < (current.mainGeneration ?? 0)) return null;
  // Delivery order across lifecycle producers is not globally ordered. Even an unrelated
  // operation advances the observed floor before ownership filtering, otherwise a delayed
  // owned terminal frame could incorrectly complete against an already superseded core.
  const observed = event.mainGeneration > (current.mainGeneration ?? 0)
    ? { ...current, mainGeneration: event.mainGeneration } : current;
  // A newer explicit user stop cancels neutrally; an old stop cannot cancel a newer Apply.
  if (event.kind === 'stop' && event.terminal === true && event.phase === 'stopped'
      && current.mainGeneration !== undefined && event.mainGeneration > current.mainGeneration) {
    return { phase: 'idle', reason: null };
  }
  if (event.applyId !== current.applyId || event.kind !== 'restart') {
    return observed === current ? null : observed;
  }
  if (event.terminal !== true) return observed;
  if (event.phase === 'failed') return { phase: 'failed', reason: proxyErrorReason(event, t) };
  if (event.phase === 'ready') return { phase: 'idle', reason: null };
  return observed === current ? null : observed;
}

// ─────────────────────── §2.4 两个正交维度的合成（不是一个状态机）───────────────────────

/**
 * 维度 A：staged（前端，磁盘之前）。`clean` / `staged` 由条目数派生，另两个是「保存」这一次动作的生命周期。
 */
export type StagedPhase = 'clean' | 'staged' | 'saving' | 'saveFailed';

/** 维度 B：pending（磁盘 vs 运行核）。 */
export type PendingPhase = 'none' | 'pending' | 'applying' | 'applyFailed' | 'durabilityPending';

export type BarActionId = 'reset' | 'save' | 'apply' | 'retryApply' | 'dismissApply' | 'retrySave' | 'retryPersistence';

/** `disabled` 与「不出现」是两回事：表里的「禁用」要求按钮仍在位（否则条会在重启期间抖一下）。 */
export interface BarAction {
  id: BarActionId;
  disabled: boolean;
}

export interface BarView {
  visible: boolean;
  /** `.pending-bar.err` 红态。 */
  err: boolean;
  titleKey: string;
  titleVars: Record<string, string | number>;
  /** 副标题（`.pb-tx div`）。空串 = 该行不渲染。 */
  detailKey: string;
  actions: BarAction[];
}

export interface BarInput {
  stagedPhase: StagedPhase;
  pendingPhase: PendingPhase;
  /** staged 条目数（= stagedDiff 大小，Q5：不需要后端算）。 */
  stagedCount: number;
  /** 待应用节点差集计数。0 而 `pendingPhase==='pending'` ⇒ 只有 `restartDeferred` 那笔欠账。 */
  pendingCount: number;
  applyError: string | null;
  /**
   * 内核当前在跑没有。
   *
   * 决定「立即应用」这颗**出不出现**：它的语义是「保存 + 强制重启内核让改动即刻生效」。
   * 核没在跑时后半句无对象 —— 改动本来就会在下次起核时带上，此时把这颗摆出来，
   * 用户点它得到的要么是一次空转、要么是一次莫名其妙的起核（陈先生 2026-07-30 报）。
   * 故 `false` 时整颗**不渲染**（而非禁用）：禁用是「现在不能点」，不渲染才是「这件事此刻不存在」。
   * 「保存」不受影响 —— 落盘在两种运行态下语义完全相同。
   */
  coreRunning: boolean;
  /** Exact original receipt is available for an explicit persistence-only retry. */
  persistenceRetryAvailable?: boolean;
  persistenceOnly?: boolean;
  /** Confirmed regeneration supersession ends this original recovery, not durability. */
  persistenceRecoveryEnded?: boolean;
}

const HIDDEN: BarView = {
  visible: false,
  err: false,
  titleKey: '',
  titleVars: {},
  detailKey: '',
  actions: [],
};

const act = (id: BarActionId, disabled = false): BarAction => ({ id, disabled });

/**
 * §2.4 的「合成呈现规则」表逐行落地。
 *
 * **不能塞成一个状态机**（spec 原话）：A×B 是 16 格，硬塞会得到 8 个不可达态。这里按表分派：
 * A 的两个「动作生命周期」态（saving / saveFailed）盖住整行 B（表末两行的 `*`），其余按 (A,B) 取格。
 *
 * 表里没有 `staged × applyFailed` 那一格 —— 它可达（保存成功清空 staged → apply 失败 → 用户又改了别的），
 * 落法按两个维度**各自贡献**推出：A=staged 给 [重置][保存][立即应用]，B=applyFailed 额外给 [忽略]
 * 并把条点红（「立即应用」本身就是那格里的[重试]，不重复出一颗）。**此格属 spec 缺口，已在交付里单列。**
 */
export function composeBarView(input: BarInput): BarView {
  const { stagedPhase, pendingPhase, stagedCount, pendingCount, applyError, coreRunning } =
    input;

  /** 核没在跑 ⇒ 抹掉全部「重启核」类动作（见 `BarInput.coreRunning`）。收在一处，别逐格判。 */
  const gateApply = (actions: BarAction[]): BarAction[] =>
    coreRunning ? actions : actions.filter((a) => a.id !== 'apply' && a.id !== 'retryApply' && a.id !== 'retryPersistence');

  // ── A 的动作生命周期态覆盖整行 B ──
  if (stagedPhase === 'saving') {
    return {
      visible: true,
      err: false,
      titleKey: 'home.pendingSaving',
      titleVars: {},
      detailKey: '',
      // 全禁用：保存在飞时点重置会丢掉正在落盘的那批意图，点应用会叠一次 force-restart。
      actions: gateApply([act('reset', true), act('save', true), act('apply', true)]),
    };
  }
  if (stagedPhase === 'saveFailed') {
    return {
      visible: true,
      err: true,
      // NFR-1：staged 一条不丢，故这里给的是 [重置][重试保存]，没有「忽略」——
      // 忽略一次失败的保存 = 让用户以为改动还在路上，而它其实哪也没去。
      titleKey: 'home.pendingSaveFailed',
      titleVars: {},
      detailKey: 'home.pendingDetailTipStaged',
      actions: [act('reset'), act('retrySave')],
    };
  }

  // Persistence recovery is a distinct original-transaction action, never Apply.
  if (pendingPhase === 'durabilityPending') {
    return { visible: true, err: false, titleKey: 'home.pendingHotDurabilityPending',
      titleVars: {}, detailKey: '', actions: input.persistenceRetryAvailable ? gateApply([act('retryPersistence')]) : [] };
  }

  const applyFailedTitle: Pick<BarView, 'titleKey' | 'titleVars'> = {
    titleKey: applyError === null ? 'home.pendingApplyFailed' : 'home.pendingApplyFailedReason',
    titleVars: applyError === null ? {} : { reason: applyError },
  };

  if (pendingPhase === 'applyFailed' && input.persistenceOnly) {
    return { visible: true, err: true, ...applyFailedTitle, detailKey: '',
      actions: input.persistenceRecoveryEnded ? [act('dismissApply')]
        : input.persistenceRetryAvailable ? gateApply([act('retryPersistence')]) : [] };
  }

  if (stagedPhase === 'clean') {
    switch (pendingPhase) {
      case 'none':
        return HIDDEN;
      case 'pending':
        return {
          visible: true,
          err: false,
          // 计数为 0 却在 pending ⇒ 只有 `restartDeferred`：条上不能说「0 项待应用」。
          titleKey: pendingCount === 0 ? 'home.pendingBarConfigOnly' : 'home.pendingBarTitle',
          titleVars: pendingCount === 0 ? {} : { count: pendingCount },
          detailKey: 'home.pendingDetailTip',
          actions: gateApply([act('apply')]),
        };
      case 'applying':
        return {
          visible: true,
          err: false,
          titleKey: 'home.pendingApplying',
          titleVars: {},
          detailKey: 'home.pendingDetailTip',
          actions: gateApply([act('apply', true)]),
        };
      case 'applyFailed':
        return {
          visible: true,
          err: true,
          ...applyFailedTitle,
          detailKey: 'home.pendingDetailTip',
          actions: gateApply([act('retryApply'), act('dismissApply')]),
        };
    }
  }

  // ── A = staged ──
  const stagedActions = gateApply([act('reset'), act('save'), act('apply')]);
  switch (pendingPhase) {
    case 'none':
      return {
        visible: true,
        err: false,
        titleKey: 'home.pendingStagedTitle',
        titleVars: { count: stagedCount },
        detailKey: 'home.pendingDetailTipStaged',
        actions: stagedActions,
      };
    case 'pending':
      return {
        visible: true,
        err: false,
        // 「另有 0 项待应用」是假话：pendingCount===0 时那笔欠账是非节点结构性变更（restartDeferred）。
        titleKey:
          pendingCount === 0 ? 'home.pendingStagedAndConfig' : 'home.pendingStagedAndPending',
        titleVars:
          pendingCount === 0
            ? { count: stagedCount }
            : { staged: stagedCount, pending: pendingCount },
        detailKey: 'home.pendingDetailTipStaged',
        actions: stagedActions,
      };
    case 'applying':
      return {
        visible: true,
        err: false,
        titleKey: 'home.pendingApplyingWithStaged',
        titleVars: { count: stagedCount },
        detailKey: 'home.pendingDetailTipStaged',
        // 「立即应用」禁用：在飞的重启被第二次 force-restart 打断没有意义（后端 `gate.is_busy()`
        // 会返 deferred，功能上安全），禁用是为了挡住用户连点。
        actions: gateApply([act('reset'), act('save'), act('apply', true)]),
      };
    case 'applyFailed':
      return {
        visible: true,
        err: true,
        ...applyFailedTitle,
        detailKey: 'home.pendingDetailTipStaged',
        actions: [...stagedActions, act('dismissApply')],
      };
  }
}

/** A 维度取值：`saving`/`saveFailed` 来自「保存」这次动作，其余由条目数派生。 */
export function stagedPhaseOf(
  saveStatus: 'idle' | 'saving' | 'saveFailed',
  entryCount: number
): StagedPhase {
  if (saveStatus !== 'idle') return saveStatus;
  return entryCount > 0 ? 'staged' : 'clean';
}

/** B 维度取值：本次 apply 的生命周期优先于「磁盘 vs 运行核」的静态差集。 */
export function pendingPhaseOf(applyPhase: ApplyPhase, pending: PendingNodeChanges): PendingPhase {
  if (applyPhase === 'durabilityPending') return 'durabilityPending';
  if (applyPhase === 'failed') return 'applyFailed';
  if (applyPhase === 'applying') return 'applying';
  return hasPendingWork(pending) ? 'pending' : 'none';
}

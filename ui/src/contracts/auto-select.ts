/**
 * 自动选择的合同：选择意图、选择状态、设置意图与立即切换的回执。
 *
 * 单一真值在后端 `src-tauri/src/runtime/auto_select.rs`；本文件与它的一致性由
 * `auto-select.test.ts` 逐字对拍。界面不自行计算胜出者，也不自行判断「为什么没在选」——
 * 模式与原因全部取自这里的状态。
 *
 * 意图与实际出口分列：意图存配置的顶层键 `selectionIntent`（缺席即手动），`selectedServerId`
 * 始终是实际出口（自动选择下即当前胜出节点）。界面没有意图键的写入权：置上经
 * `api.server.autoSelectEnable`，清掉只需点任一节点（`api.server.switch`）。
 */
import type { ServerSwitchReceipt } from './server-switch';

/** 配置里的选择意图。作用域今天只有 `subscription`。 */
export interface SelectionIntent {
  mode: 'auto';
  scope: 'subscription';
  subscriptionId: string;
}

/**
 * 状态的模式。
 *
 * `manual` 手动 / `settled` 自动且已选定 / `waitingFirstRound` 等待首轮测速 / `noCandidates` 订阅里
 * 没有可用候选 / `noData` 没有数据可据以选点（当前出口没有新鲜结果，或周期测速没在供数）/
 * `notEvaluated` 此刻没有评估 / `needsRestart` 有候选但切过去须重启内核，后台不重启（可用
 * `autoSelectSwitchNow`）/ `rateLimited` 已到换点频度上限 / `commitFailed` 该换而提交没有换成。
 */
export const AUTO_SELECT_MODES = [
  'manual',
  'settled',
  'waitingFirstRound',
  'noCandidates',
  'noData',
  'notEvaluated',
  'needsRestart',
  'rateLimited',
  'commitFailed',
] as const;
export type AutoSelectMode = (typeof AUTO_SELECT_MODES)[number];

/**
 * 模式为 `notEvaluated` 时的原因：自动选择此刻没有评估。
 *
 * `intentUnrecognized` 配置里的意图是更新的版本写的，本版本不认识（原样留着，不据它做事）/
 * `switchedOff` 源码里的总开关关着 / `platformNotOpen` 本平台暂未开放 / `coreNotRunning` 代理未运行 /
 * `reconciling` 出口正在后台对账 / `configPending` 有已保存未应用的出口变更或未保存草稿 /
 * `background` 手机不在前台 / `subscriptionEmpty` 订阅里没有节点 / `measuring` 有测速在飞 /
 * `periodicDisabled` 首轮未完成，周期测速的全局总开关关着 / `meteredPaused` 首轮未完成，周期测速
 * 因计费网络暂停 / `powerSave` 首轮未完成，周期测速因省电暂停 / `notYetEvaluated` 意图刚写入，
 * 还没有轮到一次评估。
 */
export const AUTO_SELECT_NOT_EVALUATED_REASONS = [
  'intentUnrecognized',
  'switchedOff',
  'platformNotOpen',
  'coreNotRunning',
  'reconciling',
  'configPending',
  'background',
  'subscriptionEmpty',
  'measuring',
  'periodicDisabled',
  'meteredPaused',
  'powerSave',
  'notYetEvaluated',
] as const;
export type AutoSelectNotEvaluatedReason = (typeof AUTO_SELECT_NOT_EVALUATED_REASONS)[number];

/**
 * 模式为 `settled` 时的原因：评估了、没有换点的细分（刚换了点时是换点原因，见 `AutoSelectCause`）。
 *
 * `settled` 当前出口有新鲜结果，没有明显更优的候选 / `challengerPending` 当前出口被明显胜过，
 * 换点条件未齐（见 `challenger.lacking`）/ `currentFailedOnce` 当前出口测速失败一次，未到确认次数 /
 * `cooldown` 该换而冷却未过 / `betterLegOff` 源码里的择优开关关着 / `committing` 正在提交换点。
 */
export const AUTO_SELECT_HOLD_REASONS = [
  'settled',
  'challengerPending',
  'currentFailedOnce',
  'cooldown',
  'betterLegOff',
  'committing',
] as const;
export type AutoSelectHoldReason = (typeof AUTO_SELECT_HOLD_REASONS)[number];

/**
 * 模式为 `noData` 时的原因：首轮之后，没有数据可据以选点。持续的时长见 `noDataForMs`，超过三个
 * 周期后 `flags.starved` 置位。
 *
 * 当前出口没有新鲜结果：`currentExpired` 已过期 / `currentUnmeasured` 未测 / `currentSkipped` 最近
 * 一轮被预筛跳过 / `currentMismatch` 测出了值但读回判定量到的不是它。周期测速此刻没在供数时，
 * 原因是它被什么挡住：`periodicDisabled` 全局总开关关着 / `meteredPaused` 计费网络暂停 /
 * `powerSave` 省电暂停。
 */
export const AUTO_SELECT_NO_DATA_REASONS = [
  'currentExpired',
  'currentUnmeasured',
  'currentSkipped',
  'currentMismatch',
  'periodicDisabled',
  'meteredPaused',
  'powerSave',
] as const;
export type AutoSelectNoDataReason = (typeof AUTO_SELECT_NO_DATA_REASONS)[number];

/**
 * 模式为 `commitFailed` 时的原因：裁决为换点，提交没有换成。连续次数与重试间隔见 `commit`。
 *
 * `commitFailed` 目标没能通过运行态读回，已恢复原出口 / `commitNotEligible` 提交时目标已不满足
 * 零重启热切的条件 / `commitReconciling` 目标与原出口都没能自证，出口交给了后台对账 /
 * `commitYielded` 让位给了手动选择、配置变更或新的内核世代（不计入连续失败）/ `retryPending`
 * 此前提交失败，正在等下一次重试。
 */
export const AUTO_SELECT_COMMIT_FAILED_REASONS = [
  'commitFailed',
  'commitNotEligible',
  'commitReconciling',
  'commitYielded',
  'retryPending',
] as const;
export type AutoSelectCommitFailedReason = (typeof AUTO_SELECT_COMMIT_FAILED_REASONS)[number];

/**
 * 换点的原因。
 *
 * `firstPlacement` 出口不在订阅内，落到订阅里最快的 / `better` 当前出口连续几轮被明显胜过 /
 * `currentFailed` 当前出口连续测速失败 / `failover` 连通性故障切换 / `winnerRemoved` 订阅刷新删掉了
 * 胜出节点，改选同订阅的 / `requested` 用户要求立即切换，而当前出口没有新鲜结果可比。
 */
export const AUTO_SELECT_CAUSES = [
  'firstPlacement',
  'better',
  'currentFailed',
  'failover',
  'winnerRemoved',
  'requested',
] as const;
export type AutoSelectCause = (typeof AUTO_SELECT_CAUSES)[number];

/** 哪条腿换的点：`failover` 连通性故障切换；`select` 自动选择。 */
export type AutoSelectLeg = 'failover' | 'select';

/**
 * 离换点还差什么：`streak` 连胜轮数不够 / `gap` 新的一轮与上一次计胜相隔太近 / `pair` 当前出口与
 * 候选的读数不是同一轮测得、没法比 / `dwell` 距上次换点不够久 / `rateLimit` 已到频度上限 /
 * `cooldown` 冷却未过。
 */
export const AUTO_SELECT_LACKING = [
  'streak',
  'gap',
  'pair',
  'dwell',
  'rateLimit',
  'cooldown',
] as const;
export type AutoSelectLacking = (typeof AUTO_SELECT_LACKING)[number];

/** 订阅成员在账本里的分布。 */
export interface AutoSelectCounts {
  selectable: number;
  failed: number;
  stale: number;
  unmeasured: number;
  /** 最近一轮起测前被预筛跳过（未纳入）。 */
  skipped: number;
  /** 可选点里承载出站没有被读回证实的个数；它们照常可选。 */
  unverified: number;
  /** 可选点里因「切过去要整核重启」不进候选的个数。 */
  needsRestart: number;
}

/** 一次换点的记录。 */
export interface AutoSelectSwitchRecord {
  /** Unix 毫秒。 */
  at: number;
  leg: AutoSelectLeg;
  cause: AutoSelectCause;
  fromId: string | null;
  fromName: string | null;
  toId: string;
  toName: string;
  fromLatencyMs: number | null;
  toLatencyMs: number | null;
  /** 胜出依据是否没有被读回证实；故障切换现测的结果不带这个结论。 */
  unverified: boolean | null;
}

/**
 * 选择状态（`auto_select_status` 的返回，也是 `event:autoSelectStatus` 的载荷）。
 *
 * 非自动意图下除 `intent` 与 `exit` 外全部为空或零值。进程重启后 `lastSwitch` 与计数从头开始：
 * 界面须写成「本次运行以来」。时间戳是 Unix 毫秒，只用于展示；「持续多久」「还剩多久」后端按
 * 单调时钟算好了给（`noDataForMs`、`barred[].remainingMs`、`commit.retryAfterMs`）。
 */
export interface AutoSelectStatus {
  intent: {
    /** `unrecognized`：配置里存着更新的版本写的意图，本版本不认识、不据它做事。 */
    mode: 'manual' | 'auto' | 'unrecognized';
    scope: 'subscription' | null;
    subscriptionId: string | null;
  };
  exit: {
    /** 实际出口：节点 id、`__direct__`、`__block__`，或未选。 */
    serverId: string | null;
    /** 是否属于意图指向的订阅；非自动意图下为空。 */
    inSubscription: boolean | null;
  };
  mode: AutoSelectMode;
  reason:
    | AutoSelectNotEvaluatedReason
    | AutoSelectHoldReason
    | AutoSelectNoDataReason
    | AutoSelectCommitFailedReason
    | AutoSelectCause
    | null;
  /** 当前出口的依据；出口不是节点（直连、阻断、未选）或非自动意图下为空。 */
  basis: {
    state: 'fresh' | 'failed' | 'expired' | 'unmeasured' | 'skipped' | 'mismatch';
    latencyMs: number | null;
    measuredAt: number | null;
    binding: 'confirmed' | 'unverified' | null;
  } | null;
  lastSwitch: AutoSelectSwitchRecord | null;
  lastEvaluation: {
    at: number;
    outcome: 'held' | 'switch' | 'notEvaluated';
    reason: string;
    counts: AutoSelectCounts;
  } | null;
  /**
   * 当前出口被候选明显胜过的进度。`wins` 是当前出口已连续被胜过的轮数（每一轮里胜过它的可以是
   * 不同的节点），`serverId` 是最近一轮里的延迟最小者 —— 凑满时换到的是当轮的这一个。
   */
  challenger: {
    serverId: string;
    wins: number;
    winsRequired: number;
    lacking: AutoSelectLacking | null;
  } | null;
  /** 因近期被故障切换换走而暂时不作为挑战者的节点。 */
  barred: { serverId: string; remainingMs: number }[];
  /** 提交的状况：连续没有换成的次数、下一次重试前的间隔、最近一次没换成的记录。 */
  commit: {
    consecutiveFailures: number;
    retryAfterMs: number | null;
    lastFailure: { at: number; toId: string; outcome: AutoSelectCommitFailedReason } | null;
  };
  /** 没有数据可据以选点已持续的毫秒数；有数据时为空。 */
  noDataForMs: number | null;
  flags: {
    /** 该评估而长时间没有评估。 */
    stalled: boolean;
    /** 没有数据可据以选点已持续过久。 */
    starved: boolean;
    exitOutsideSubscription: boolean;
    exitOutsideOverdue: boolean;
    /** 可选点全部没有被读回证实：读回通道在本机很可能不可用。 */
    allUnverified: boolean;
  };
  counters: {
    failoverSwitches: number;
    selectSwitches: number;
    betterSwitchesInWindow: number;
    betterSwitchesMax: number;
  };
  /** 周期测速一侧的状况：没有数据时原因从这里追。 */
  dataSource: {
    /** 周期计划没在执行的原因（计划状态的未启用或暂停原因）；在执行时为空。 */
    blockedBy: string | null;
    /** 意图指向的订阅最近一次到期未执行的原因。 */
    lastSkip: string | null;
  };
}

/**
 * 设置意图与立即切换的回执。命令成功返回即表示写入已落盘，`intentApplied` 为真时意图此刻是
 * 自动选择。
 *
 * - `autoSelectEnable`：只有当前出口不属于该订阅、且已有新鲜候选时才带落点（首次落点）；当前
 *   出口已是该订阅的成员时只写意图，`placement` 为空，出口不动。
 * - `autoSelectSwitchNow`：恒带落点；没有落点时命令以 `AUTO_SELECT_NO_TARGET` 被拒。
 *
 * 落点的运行侧可能没换成：`placement.status` 为 `failed`、`reason` 为 `AUTO_SELECT_SWITCH_FAILED`，
 * 此时出口与意图都已落盘，实际出口由后台或下一次操作收敛。
 */
export interface AutoSelectReceipt {
  intentApplied: boolean;
  placement: {
    serverId: string;
    status: ServerSwitchReceipt['status'] | 'failed';
    reason?: ServerSwitchReceipt['reason'] | 'AUTO_SELECT_SWITCH_FAILED';
    error?: string;
  } | null;
  status: AutoSelectStatus;
}

/**
 * 设置意图与立即切换的稳定码。前六个出现在被拒的信封里；`AUTO_SELECT_SWITCH_FAILED` 在信封里
 * 表示落点没通过选择校验，在成功回执的 `placement.reason` 里表示落点已落盘而运行侧没换成。
 */
export const AUTO_SELECT_ERROR_CODES = [
  'AUTO_SELECT_SWITCHED_OFF',
  'AUTO_SELECT_PLATFORM_NOT_OPEN',
  'AUTO_SELECT_SUBSCRIPTION_NOT_FOUND',
  'AUTO_SELECT_SUBSCRIPTION_EMPTY',
  'AUTO_SELECT_NOT_AUTO',
  'AUTO_SELECT_NO_TARGET',
  'AUTO_SELECT_SWITCH_FAILED',
] as const;
export type AutoSelectErrorCode = (typeof AUTO_SELECT_ERROR_CODES)[number];

/**
 * `event:autoNodeSwitched` 的载荷。前三个字段是原有的；后三个是自动选择加的，只增不改。
 * `leg` 为 `failover` 时 `reason` 是 `connectivity`；为 `select` 时 `reason` 与 `cause` 相同。
 */
export interface AutoNodeSwitchedPayload {
  reason: string;
  newServerName: string;
  latency: number;
  leg: AutoSelectLeg;
  cause: AutoSelectCause;
  oldServerName: string | null;
}

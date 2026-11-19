/**
 * 配置暂存 store —— 条目容器 + 渲染端持久化（Q1-b）。纯逻辑在 `lib/staged-config.ts`。
 *
 * # 存储介质：`localStorage`（spec §Q1-b 字面口径）+ 后端「正常退出标记」当 ② 的判据
 *
 * U-1 拍板的语义是两条：**① 渲染端持久化，webview 重载可恢复；② 进程退出即丢**。
 * spec 为满足 ② 设计了「主进程退出腿向 main 窗发一次清除指令」。落地时发现两条都不成立：
 *
 *  - 渲染端能拿到的退出信号只有 `beforeunload` / `pagehide`，而这两个事件在**重载**时同样触发
 *    ⇒ 结构性无法区分「关 App」与「自愈重载」⇒ 用它清除会直接违反 ①（NFR-1，自愈重载吃掉用户编辑）。
 *  - 退出那一刻 webview 正在拆，「发指令」是竞态、可能永远送不到；强杀更是根本没有那一刻。
 *
 * ⇒ 改成**后端留标记、前端下次启动读**（`api.window.takeCleanExitFlag()`，实现见
 * `src-tauri/src/clean_exit.rs`）：正常退出腿落一个持久标记，下次启动**在 hydrate 之前**读一次
 * （读即清），为真才清掉持久化的暂存。判据是「上次进程有没有走完退出腿」，正是 ② 要的那个区分。
 *
 * **为什么介质是 `localStorage` 而不是 `sessionStorage`**：sessionStorage 的生命周期看着像 ①+②
 * （重载保留、webview 重建即空），但「webview 重建」在 Polaris 里**不等于**「进程退出」——
 * C16 轻量模式（`tray_enter_lightweight`）会在 idle 计时到点时**销毁主窗 webview**、用户唤出时重建，
 * 全程进程没退、用户无感。用 sessionStorage ⇒ 挂机一会儿回来，暂存的编辑被 App 自己吃掉，
 * 这正是 NFR-1 禁止的那类伤害，且比自愈重载更常发生（有定时器驱动）。localStorage 跨 webview 重建
 * 存活，退出与否交给上面那个标记判 —— 每条腿都由知道真相的一侧来判。
 *
 * # 恢复不再按版本丢弃（spec §Q1-b 那条已作废，理由在 `lib/staged-config.ts` 的 `StagedPayload`）
 *
 * 冷启/重载时磁盘版本与暂存时不等 ⇒ **保留条目、拿持久化的 `baseline` 当基准**，把「陈旧 staged
 * 对上新磁盘」交给保存时的 Q8-b 冲突腿（多半无实体重叠 → 静默合并）。丢弃是 P5 合并器落地前的
 * 权宜，如今只剩静默数据丢失：后台写盘者刷版本极频繁，用户暂存 N 条 → 后台写一次盘 → 重载 → 全没。
 *
 * # 存不进去怎么办（R4 降级路径）
 *
 * 一律 try/catch 吞掉：**存储不可用 ⇒ 退化成不持久化，不崩、不丢内存里的编辑**。
 * 隐私模式 / 配额满 / webview 不给 storage，用户能看到的差别只有「重载后条目没了」，
 * 而不是白屏或编辑消失。node 单测环境本身就没有 storage，这条降级路径被每次跑测天然覆盖。
 */

import { create } from 'zustand';

import type { SaveOutcome, UserConfig } from '@/contracts/types';
import { configSaveErrorText } from '@/domain/action-error-text';
import i18n from '@/i18n';
import { api } from '@/ipc';
import { withConfigWriteLock } from '@/lib/config-write-lock';
import { toast } from '@/lib/error-handler';
import {
  STAGED_CONFIG_ENABLED,
  STAGED_STORAGE_KEY,
  configBaseVersion,
  conflictingEntries,
  decodeStagedPayload,
  encodeStagedPayload,
  entitySnapshot,
  isAbsentSnapshot,
  isValidStagedEntry,
  pruneSatisfiedEntries,
  remainingEntriesAfterCommit,
  replay,
  revertEntry,
  savedEditNotice,
  stageEntry,
  type SavedEditChange,
  type StagedEntry,
} from '@/lib/staged-config';

/** 直接从 IPC 契约取，避免与 `pending-bar-logic.ts` 的同名联合各写一份（store 不该反向依赖组件层）。 */
type ApplyReceipt = Awaited<ReturnType<typeof api.proxy.applyPendingChanges>>;
export type SavedRuntimeApply = ApplyReceipt & {
  changes?: readonly SavedEditChange[];
  saveRevision?: number;
  configVersion?: string;
};

type ApplyDispatch = (precedingSave: ApplyReceipt | null) => boolean;

/** Order the renderer's explicit IPC dispatch after any Save already in flight. */
export async function dispatchSavedApply<T>(invoke: () => Promise<T>, onDispatch?: ApplyDispatch): Promise<T | undefined> {
  const admitted = await withConfigWriteLock(async () => {
    if (onDispatch?.(useStagedConfigStore.getState().savedRuntimeApply) === false) return undefined;
    return { reply: invoke() };
  });
  return admitted?.reply;
}
type ApplyStatus = ApplyReceipt['status'];
type EditSubmission = 'automatic' | 'manual' | 'ignored';

type QueuedEdit = {
  baseline: UserConfig;
  fields: ReadonlySet<string>;
  committedVersion?: string;
};
let latestQueuedEdit: QueuedEdit | undefined;

/**
 * 冲突弹窗的一行：这条条目要改的实体，磁盘侧也被别人改过（Q8-b 第 4 步）。
 *
 * 两侧值给的是**稳定序列化后的串**而不是原对象：弹窗要做的只是让用户认出「这是我改的那个」
 * 与「盘上现在是什么」，给对象还得在 UI 层再序列化一遍（而且是另一套规则）。
 */
export interface StagedConflict {
  /** 对应的 `StagedEntry.id`，用户的选择按它回填。 */
  readonly entryId: string;
  /** 条目 label（「编辑节点 香港 IEPL 01」）。 */
  readonly label: string;
  /** 「用我的」：这条条目落下去之后该实体的样子。 */
  readonly mine: string;
  /** 「用磁盘的」：磁盘现值里该实体的样子；**`null` = 该实体在磁盘侧已被删除**（Q8 最后一行）。 */
  readonly disk: string | null;
}

export interface ApplyNowResult extends Omit<ApplyReceipt, 'ok' | 'status' | 'applyId'> {
  /**
   * 保存这一半有没有过。**`false` ⇒ 根本没调过 `applyPendingChanges`** —— 调用方据此不要再叠一个
   * 「应用失败」：条已经由 A 维度显示「保存失败：<原因>」，两句话说同一次失败会让人以为炸了两回。
   */
  saved: boolean;
  /** 排程结果；`null` = IPC 失败。`saved:false` 时恒 `null`（没调过）。 */
  status: ApplyStatus | null;
  /** Backend force_restart_seq identity; absent receipts cannot prove completion. */
  applyId?: number;
}

/** 取持久存储；不可用（隐私模式 / node / webview 未提供）返 null。介质选型理由见文件头。 */
function storage(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

// Tauri invoke 的完成顺序不应由 IPC 调度偶然决定：stage(true) 后紧接 reset(false) 时，后端最终值
// 必须是 false。单条 Promise 队列把这个 bit 的镜像顺序固定为用户动作顺序；失败只削弱跨进程保险，
// 不影响内存/localStorage 里的草稿正文。
let stagedPendingSync: Promise<void> = Promise.resolve();

function syncStagedPending(entries: readonly StagedEntry[]): void {
  const pending = entries.length > 0;
  const nodeIds = [
    ...new Set(
      entries.flatMap((entry) =>
        entry.entityPath.length === 2 && entry.entityPath[0] === 'servers'
          ? [entry.entityPath[1]]
          : []
      )
    ),
  ];
  stagedPendingSync = stagedPendingSync
    .catch(() => undefined)
    .then(() => api.config.setStagedPending(pending, nodeIds))
    .catch((error) => {
      console.error('[staged-config] sync pending marker failed:', error);
    });
}

/** 起核入口等待此前所有 marker 更新落定，防“保存刚清空、后端仍看到 pending”的时序竞态。 */
export function flushStagedPendingSync(): Promise<void> {
  return stagedPendingSync;
}

/**
 * 写存储。两种情况写的是「删除」而非「空值」：
 *  - `entries` 为空 —— 不留空数组残渣，下次恢复走「没存过」那条腿。
 *  - `baseline` 为 null（还没 hydrate 过）—— 没有基准就判不了冲突，存进去的东西下次
 *    只能无条件相信或无条件丢弃，两条都不对；宁可不持久化。
 *
 * 带上 baseline 后单次写入从 ~1KB 涨到与 `config.json` 同量级（见 `StagedPayload` 的体积那节）。
 * try/catch 因此从「理论兜底」变成**真会走到的降级腿**：配额超 ⇒ 本次不持久化，
 * 状态与内存里的条目一个不动（崩了才是数据丢失，写不进去只是重载后条目没了）。
 */
function persist(baseline: UserConfig | null, entries: StagedEntry[]): void {
  // 即使 localStorage 不可用，当前进程的托盘/自动连接也必须知道内存里还有草稿。
  syncStagedPending(entries);
  const s = storage();
  if (s === null) return;
  try {
    if (baseline === null || entries.length === 0) s.removeItem(STAGED_STORAGE_KEY);
    else {
      s.setItem(
        STAGED_STORAGE_KEY,
        encodeStagedPayload({ baseline: baseline as unknown as Record<string, unknown>, entries })
      );
    }
  } catch {
    /* 配额满 / 隐私模式 → 退化为不持久化，内存里的条目照常可用 */
  }
}

function readPersisted(): ReturnType<typeof decodeStagedPayload> {
  const s = storage();
  if (s === null) return null;
  try {
    return decodeStagedPayload(s.getItem(STAGED_STORAGE_KEY));
  } catch {
    return null;
  }
}

export interface StagedConfigState {
  /**
   * 总开关（初值 = 编译期常量 `STAGED_CONFIG_ENABLED`）。
   * 关 ⇒ 下面每个 action 都是 no-op：不改状态、不碰存储。产品行为由常量决定，本字段只为单测能跑两侧。
   */
  enabled: boolean;
  /** 有序意图条目。`entries.length` 即 stagedDiff 的大小（Q5：不需要后端算）。 */
  entries: StagedEntry[];
  /** 最近一次读到的已保存配置及版本。清空最后一条草稿时用它开始下一批。 */
  baseVersion: string | null;
  latestSavedConfig: UserConfig | null;
  /**
   * **staged 建立那一刻的磁盘快照**（Q8-b 的 `baseline`）—— 冲突检出的比对基准，
   * 也是「保存」重放的语义原点。
   *
   * `latestSavedConfig` 跟随读到的磁盘快照，`baseline` 在这批草稿存续期间保持不变。
   *
   * 让 baseline 跟着磁盘走 = 冲突检出恒空 ⇒ 静默吃掉别人的改动（Q1 里那条 NFR-2 违反）。
   * 反过来用 `effectiveConfig` 当 baseline ⇒ 用户自己的每条改动都被判成「磁盘变了」⇒ 恒冲突。
   */
  baseline: UserConfig | null;
  /** 是否已从存储恢复过（恢复只发生一次，此后 config 重载只跟版本，不再覆盖内存条目）。 */
  hydrated: boolean;
  /**
   * 「保存」这一次动作的生命周期（§2.4 维度 A 的后两个态）。
   * `clean` / `staged` **不入此字段**——那两个由 `entries.length` 派生，多存一份就多一处会分叉的真值。
   */
  saveStatus: 'idle' | 'saving' | 'saveFailed';
  /** Includes completed staged saves that received no runtime receipt. */
  completedSaveRevision: number;
  /** Owned receipt for the shared desktop/mobile apply observer. */
  savedRuntimeApply: SavedRuntimeApply | null;
  /**
   * 待用户裁决的冲突集（Q8-b 第 4 步）；`null` = 无弹窗。
   *
   * 非空期间 `saveStatus` 停在 `idle` 而不是 `saving`：条已经被弹窗盖住了，再让它转圈
   * 会在用户关掉弹窗后留下一条永远转下去的「保存中…」。
   */
  conflict: StagedConflict[] | null;
  /** 冲突弹窗生成时的磁盘快照；仅在用户裁决后成为新 baseline，不提前持久化。 */
  conflictBaseline: UserConfig | null;

  /** 暂存一条编辑意图。同 id 覆盖。 */
  stage: (entry: StagedEntry) => void;
  /** Submit one edit, with any related entries, for native hot admission. */
  submit: (first: StagedEntry, ...related: StagedEntry[]) => Promise<EditSubmission>;
  /** 撤销单条（明细 popover 的红叉）。 */
  revert: (id: string) => void;
  /** 「重置」：丢弃全部草稿，从最近一次读到的已保存配置开始下一批。 */
  reset: () => void;
  /**
   * 「保存」：Q8-b 的四步 —— 拉磁盘现值 → 算冲突集 → 无冲突静默 `replay(disk, entries)` 后落盘
   * → 有冲突开弹窗。通过 `baseVersion` 校验后，只有获证的安全热更新可随保存生效。
   *
   * 返回「保存这一半是否已过关」：没有 staged 也返 `true`（没有要落盘的东西 ⇒ 不算失败，
   * `applyNow` 在 `clean × pending` 那一格靠这条腿直通 apply）。**开了冲突弹窗返 `false`**
   * （还没落盘）。失败**绝不丢 staged**（NFR-1）：条目原样保留；技术诊断只进开发者日志，
   * 用户界面显示本地化的稳定失败文案。
   */
  save: () => Promise<boolean>;
  /**
   * 冲突弹窗的落定：`keepEntryIds` = 用户选了「用我的」的那些条目 id。
   *
   * 未列入的条目（用户选了「用磁盘的」）**从 staged 里彻底移除** —— 留着它只会在下一次保存
   * 再问一遍同一个问题。随后以弹窗生成时的磁盘快照为新 baseline 重走保存：磁盘没再变时不会
   * 重复询问；弹窗停留期间若同一实体又变了，则必须把新冲突重新交给用户，而不是覆盖未知新值。
   */
  resolveConflict: (keepEntryIds: readonly string[]) => Promise<boolean>;
  /** 关掉冲突弹窗、不保存（staged 一条不丢）。 */
  dismissConflict: () => void;
  /** Save then request Apply; an explicit restart token confirms the already saved candidate without re-saving. */
  applyNow: (restartConfirmation?: string, impactConfirmation?: string, onDispatch?: ApplyDispatch) => Promise<ApplyNowResult>;
  /**
   * 每次拿到磁盘 config 后调用。
   *  - 首次：**无条件恢复**存储里的条目，并连同存储里的 `baseline` 一起恢复（那是「这批编辑
   *    相对哪一版盘做的」）。盘变没变都恢复——变了就交给保存时的冲突腿，不再静默丢弃。
   *    唯一仍丢弃的情形：存储里根本没有可用载荷（没存过 / 畸形 / 老形态无 `baseline`）。
   *  - 此后：把 `baseVersion` 跟到新盘值，**不动内存里的条目**（那是用户的活编辑）。
   *  - `baseline` 只在**没有 staged 时**跟到新盘值；有 staged 时冻住（Q8-b：冲突比对的基准
   *    必须是「这批编辑相对哪一版盘做的」，跟着盘走就永远比不出冲突）。
   */
  hydrate: (config: UserConfig) => void;
}

export const useStagedConfigStore = create<StagedConfigState>((set, get) => ({
  enabled: STAGED_CONFIG_ENABLED,
  entries: [],
  baseVersion: null,
  latestSavedConfig: null,
  baseline: null,
  hydrated: false,
  saveStatus: 'idle',
  completedSaveRevision: 0,
  savedRuntimeApply: null,
  conflict: null,
  conflictBaseline: null,

  stage: (entry) => {
    const { enabled, entries, baseline } = get();
    // 非法条目直接拒收：不写状态、不写存储、不抛（构造方是应用内代码，抛出去只会变成白屏）。
    if (!enabled || !isValidStagedEntry(entry)) return;
    const next = stageEntry(entries, entry);
    // 冲突提示只对“生成它时的那批条目”有效。弹窗期间新增/重编条目仍基于旧 baseline，
    // 不能被一份旧裁决整批重基到 conflictBaseline，否则会覆盖用户当时没被问过的磁盘改动。
    set({ entries: next, conflict: null, conflictBaseline: null });
    persist(baseline, next);
  },

  submit: (first, ...related) => {
    const entries = [first, ...related];
    const { enabled, latestSavedConfig: authoredAgainst } = get();
    if (!enabled || !entries.every(isValidStagedEntry)) return Promise.resolve('ignored');
    return submitEdit(set, get, entries, authoredAgainst);
  },

  revert: (id) => {
    const { enabled, entries, baseline, latestSavedConfig } = get();
    if (!enabled) return;
    const next = revertEntry(entries, id);
    if (next.length === entries.length) return;
    const nextBaseline = next.length === 0 ? latestSavedConfig ?? baseline : baseline;
    // 撤销同样改变了裁决的对象集；旧弹窗不再是一份可提交的冲突快照。
    set({ entries: next, baseline: nextBaseline, conflict: null, conflictBaseline: null });
    persist(nextBaseline, next);
  },

  reset: () => {
    const { enabled, entries, baseline, latestSavedConfig, saveStatus, conflict } = get();
    // 「已经是干净态」才短路。`saveStatus` / `conflict` 一并看：条在 `.err` 上（或弹窗开着）时
    // 「重置」的可见承诺是**把红点掉、把窗关上**，只按 entries 判会在「保存失败后又被别处清空
    // 条目」这种边角留一条永远消不掉的红。
    if (!enabled || (entries.length === 0 && saveStatus === 'idle' && conflict === null)) return;
    set({
      entries: [],
      baseline: latestSavedConfig ?? baseline,
      saveStatus: 'idle',
      conflict: null,
      conflictBaseline: null,
    });
    persist(baseline, []);
  },

  save: async () => performSave(set, get),

  resolveConflict: async (keepEntryIds) => {
    const { enabled, entries, latestSavedConfig, conflict, conflictBaseline } = get();
    if (!enabled) return true;
    // A stale/dismissed dialog cannot authorize a new save or rebase.
    if (conflict === null || conflictBaseline === null) return false;
    const keep = new Set(keepEntryIds);
    // 裁决只作用于**进了冲突集的那些条目**。没冲突的条目与这次弹窗无关，一条都不能动 ——
    // 按 `keepEntryIds` 直接过滤全表会把「同一批里没冲突的编辑」一起丢掉（用户没被问过、
    // 也不会被告知，是纯粹的静默丢编辑）。
    const contested = new Set((conflict ?? []).map((c) => c.entryId));
    const kept = entries.filter((e) => !contested.has(e.id) || keep.has(e.id));
    // 弹窗展示的是 conflictBaseline 那一版磁盘。裁决后拿它当新基准再走正常冲突检测：磁盘未再变
    // 不会重复询问；弹窗停留期间若同一实体又变了，则会基于新变化重新提示。
    const resolvedBaseline = kept.length === 0 ? latestSavedConfig ?? conflictBaseline : conflictBaseline;
    set({
      entries: kept,
      conflict: null,
      conflictBaseline: null,
      baseline: resolvedBaseline,
    });
    persist(resolvedBaseline, kept);
    // 全都选了「用磁盘的」⇒ 没有要落盘的东西了，直接判成功（等价于用户把这批编辑撤了）。
    return performSave(set, get);
  },

  dismissConflict: () => {
    if (get().conflict === null) return;
    set({ conflict: null, conflictBaseline: null });
  },

  applyNow: async (restartConfirmation, impactConfirmation, onDispatch) => {
    // Confirmation is bound to the already saved candidate. Never save newer drafts here.
    if (restartConfirmation !== undefined && impactConfirmation !== undefined) return { saved: false, status: 'conflict' };
    if (impactConfirmation === undefined) {
      if (restartConfirmation === undefined && !(await performSave(set, get, false))) return { saved: false, status: null };
    } else if (get().entries.length > 0) {
      void api.proxy.cancelRuntimeImpact(impactConfirmation).catch(() => null);
      return { saved: true, status: 'conflict' };
    }
    const dispatched = await dispatchSavedApply(async () => {
      const { entries, baseVersion: version } = get();
      const r = await (impactConfirmation === undefined
        ? api.proxy.applyPendingChanges(restartConfirmation)
        : api.proxy.confirmRuntimeImpact(impactConfirmation)).catch(() => null);
      return { entries, version, r };
    }, onDispatch);
    if (!dispatched) return { saved: false, status: null };
    const { entries, version, r } = dispatched;
    if (r?.impactConfirmation && (get().entries !== entries || get().baseVersion !== version)) {
      void api.proxy.cancelRuntimeImpact(r.impactConfirmation).catch(() => null);
      return { saved: true, status: 'conflict' };
    }
    return { saved: true, status: r?.status ?? null,
      ...(r?.persistenceMode === undefined ? {} : { persistenceMode: r.persistenceMode }),
      ...(r?.transactionId === undefined ? {} : { transactionId: r.transactionId }),
      ...(r?.runtimeGeneration === undefined ? {} : { runtimeGeneration: r.runtimeGeneration }),
      ...(r?.selectionIntentGeneration === undefined ? {} : { selectionIntentGeneration: r.selectionIntentGeneration }),
      ...(r?.applyId == null ? {} : { applyId: r.applyId }),
      ...(r?.restartConfirmation === undefined ? {} : { restartConfirmation: r.restartConfirmation }),
      ...(r?.impactConfirmation === undefined ? {} : { impactConfirmation: r.impactConfirmation }),
      ...(r?.coreImpacts === undefined ? {} : { coreImpacts: r.coreImpacts }),
      ...(r?.hostEffectPreviews === undefined ? {} : { hostEffectPreviews: r.hostEffectPreviews }),
      ...(r?.hostExecutionId === undefined ? {} : { hostExecutionId: r.hostExecutionId }),
      ...(r?.hostEffects === undefined ? {} : { hostEffects: r.hostEffects }),
      ...(r?.coreState === undefined ? {} : { coreState: r.coreState }),
      ...(r?.configurationDurable === undefined ? {} : { configurationDurable: r.configurationDurable }),
      ...(r?.persistenceRetryable === undefined ? {} : { persistenceRetryable: r.persistenceRetryable }),
      ...(r?.persistenceTransactionId === undefined ? {} : { persistenceTransactionId: r.persistenceTransactionId }),
      ...(r?.retirementState === undefined ? {} : { retirementState: r.retirementState }),
      ...(r?.nativeRetirements === undefined ? {} : { nativeRetirements: r.nativeRetirements }),
      ...(r?.reasons === undefined ? {} : { reasons: r.reasons }),
      ...(r?.pendingApply === undefined ? {} : { pendingApply: r.pendingApply }),
      ...(r?.restartRequired === undefined ? {} : { restartRequired: r.restartRequired }),
    };
  },

  hydrate: (config) => {
    const { enabled, hydrated, entries } = get();
    if (!enabled) return;
    const baseVersion = configBaseVersion(config);
    if (hydrated) {
      // 保留这批草稿的比较基准，同时记住盘值供重置或撤销最后一条时使用。
      set({ baseVersion, latestSavedConfig: config, ...(entries.length === 0 ? { baseline: config } : {}) });
      return;
    }
    const stored = readPersisted();
    // 保存成功回包可能因 webview/进程恰在落盘后退出而丢失：此时持久载荷仍有条目，但磁盘已经
    // 满足它们。只恢复仍会改变当前磁盘的意图，避免“已保存却重新显示待保存”的假状态。
    const restored = pruneSatisfiedEntries(config, stored?.entries ?? []);
    // 基准取**存储里那份 baseline**（= 这批编辑建立那一刻的盘），而不是当前盘值：盘在此期间被
    // 后台写过时，拿当前盘当基准会让冲突检出两侧恒等 ⇒ 永远判不出冲突 ⇒ 静默吃掉别人的改动。
    // 没有条目可恢复时才跟到当前盘值 —— 与已 hydrate 那条腿同一条规则：baseline 冻住当且仅当有 staged。
    const restoredBaseline =
      restored.length > 0 && stored !== null
        ? (stored.baseline as unknown as UserConfig)
        : config;
    set({ hydrated: true, baseVersion, latestSavedConfig: config, baseline: restoredBaseline, entries: restored });
    // 没有可恢复的载荷（没存过 / 畸形 / 老形态无 baseline）⇒ 清掉残留；有 ⇒ 原样回写（幂等）。
    persist(restoredBaseline, restored);
  },
}));

type SetState = (partial: Partial<StagedConfigState>) => void;
type GetState = () => StagedConfigState;

function submitEdit(
  set: SetState, get: GetState, entries: StagedEntry[], authoredAgainst: UserConfig | null
): Promise<EditSubmission> {
  const preceding = latestQueuedEdit;
  const queued: QueuedEdit | undefined = authoredAgainst ? {
    baseline: authoredAgainst,
    fields: new Set(entries.filter(isScalarAssignment).map(({ entityPath }) => entityPath[0])),
  } : undefined;
  latestQueuedEdit = queued;
  const manualDraft = () => {
    const current = get();
    const baseline = current.entries.length > 0
      ? current.baseline
      : authoredAgainst ?? current.latestSavedConfig;
    const next = entries.reduce(stageEntry, current.entries);
    set({ entries: next, baseline, conflict: null, conflictBaseline: null });
    persist(baseline, next);
    return 'manual' as const;
  };
  const warn = (description: string) => {
    toast.warning(entries[0].label, { description });
    return 'ignored' as const;
  };
  const failed = (description: string, retryable: boolean) => {
    const label = entries[0].label.trim() || i18n.t('common.saveFailed');
    toast.error(label, description, retryable ? {
      key: `config-edit:${entries[0].id}`,
      sticky: true,
      dismiss: { label: i18n.t('common.close') },
      actions: [{ label: i18n.t('common.retry'), onClick: () => {
        void submitEdit(set, get, entries, authoredAgainst);
      } }],
    } : undefined);
    return 'ignored' as const;
  };
  return withConfigWriteLock(async () => {
    if (!get().enabled) return 'ignored' as const;
    if (!authoredAgainst) return warn(i18n.t('home.pendingHotConflict'));
    try {
      let retryOnConflict = true;
      for (;;) {
        const disk = await api.config.get();
        if (!get().enabled) return 'ignored' as const;
        if (dependsOnDraft(disk, get().entries, entries)) return warn(i18n.t('home.stagedOnlyBlocked'));
        const merged = replay(disk, entries);
        const diskVersion = configBaseVersion(disk);
        const ownPrecedingEdit = preceding?.baseline === authoredAgainst
          && preceding.committedVersion === diskVersion ? preceding : undefined;
        const clashes = changedOnDisk(authoredAgainst, disk, merged, entries);
        if (clashes.some((entry) => !isScalarAssignment(entry) || !ownPrecedingEdit?.fields.has(entry.entityPath[0]))) {
          return warn(i18n.t('home.pendingHotConflict'));
        }
        const observedAtDispatch = get().latestSavedConfig;
        const outcome = await api.config.save(merged, true, diskVersion, true, true);
        if (outcome.status === 'conflict') {
          if (!retryOnConflict) return warn(i18n.t('home.pendingHotConflict'));
          retryOnConflict = false;
          continue;
        }
        if (outcome.status === 'needsManual') return manualDraft();
        if (outcome.status === 'blockedByPending') return warn(i18n.t('home.pendingSavedStillNeedsApply'));
        if (outcome.status !== 'saved') return failed(i18n.t('common.saveFailed'), outcome.status !== 'rejected');
        if (queued) queued.committedVersion = outcome.version;
        completeSave(set, get, disk, merged, entries, observedAtDispatch, outcome, false);
        return 'automatic' as const;
      }
    } catch (error) {
      console.error('[staged-config] automatic edit failed:', error);
      return failed(configSaveErrorText(error, i18n.t), true);
    }
  }).finally(() => {
    if (latestQueuedEdit === queued) latestQueuedEdit = undefined;
  });
}

/**
 * 「保存」的本体（Q8-b 四步）。`save` 与 `resolveConflict` 共用 —— 两条腿只差
 * 裁决后 baseline 已更新到用户看过的那份磁盘；若磁盘再次变化，仍需重新询问。
 *
 * 抽成模块级函数而不是 store 里的第三个 action：它不是外部可调用的能力，
 * 摆进 `StagedConfigState` 只会多一个能被误用的入口。
 */
function performSave(
  set: SetState,
  get: GetState,
  autoApply = true
): Promise<boolean> {
  // **整个函数体进临界区**（因果在 `withConfigWriteLock` 头注）。范围必须含开头那次 `get()`：
  // 把读 `entries` 留在临界区外，排队的第二次就会捕获 A 清空前的陈旧条目并把它们再写一遍，
  // 那是「串行了但仍在重复写」——比不串行更难查。
  return withConfigWriteLock(() => performSaveLocked(set, get, autoApply));
}

async function performSaveLocked(
  set: SetState,
  get: GetState,
  autoApply: boolean
): Promise<boolean> {
  const { enabled, entries, baseline, latestSavedConfig } = get();
  // 没有 staged ⇒ 没有要落盘的东西。这不是失败：`clean × pending` 那一格的「立即应用」正是走这条腿。
  if (!enabled) return true;
  if (entries.length === 0) {
    // 可能是“后端已保存、前端确认丢失”遗留的 marker。空草稿保存必须把它收敛为 false；起核入口
    // 随后会 flush 这条队列，不能让一个无正文的旧 bit 永久阻塞连接。
    syncStagedPending([]);
    await flushStagedPendingSync();
    return true;
  }
  if (baseline === null) {
    // config 还没加载完却已有 staged（理论上不可达：编辑入口都在 config 到位后才渲染）。
    // 没有 baseline 就无法判「相对哪一版盘做的」，此时落盘只能整份覆盖 —— 宁可报失败保住条目。
    set({ saveStatus: 'saveFailed' });
    return false;
  }
  set({ saveStatus: 'saving' });
  const isCurrentDraft = () => {
    const current = get();
    return current.entries === entries && current.baseline === baseline;
  };
  const finishFailedSave = () => set({ saveStatus: isCurrentDraft() ? 'saveFailed' : 'idle' });

  // ① 拉磁盘现值。**必须重新拉**：store 里的 `baseVersion` 只在 configChanged 广播到达时才刷，
  //    而 §Q8 明确记着有一条**不广播**的落盘（订阅验证器）。拿没刷过的版本当基准 = 漏检。
  let disk: UserConfig;
  try {
    disk = await api.config.get();
  } catch (err) {
    console.error('[staged-config] load current config before save failed:', err);
    finishFailedSave();
    return false;
  }
  // Edits made while this read was in flight need their own full conflict check.
  if (!isCurrentDraft()) {
    set({ saveStatus: 'idle' });
    return false;
  }
  const diskVersion = configBaseVersion(disk);
  if (get().latestSavedConfig === latestSavedConfig) set({ baseVersion: diskVersion, latestSavedConfig: disk });
  /** 合并结果 = 把 staged 重放到磁盘现值上。落盘、弹窗里的「用我的」、成功后的新 baseline 同一个值。 */
  const merged = replay(disk, entries);

  // ② 冲突集：只在磁盘确实变过时才算（没变 ⇒ 恒空，省一趟 O(条目数) 的子树序列化）。
  if (diskVersion !== configBaseVersion(baseline)) {
    const clashes = changedOnDisk(baseline, disk, merged, entries);
    if (clashes.length > 0) {
      // ④ 有冲突 ⇒ 弹窗，交给用户逐条选。`saveStatus` 退回 idle：条被弹窗盖住，不该同时转圈。
      set({
        saveStatus: 'idle',
        conflictBaseline: disk,
        conflict: clashes.map((e) => {
          const diskSnapshot = entitySnapshot(disk, e.entityPath);
          return {
            entryId: e.id,
            label: e.label,
            mine: entitySnapshot(merged, e.entityPath),
            disk: isAbsentSnapshot(diskSnapshot) ? null : diskSnapshot,
          };
        }),
      });
      return false;
    }
  }

  // ③ 无冲突（或已裁决）⇒ 静默合并：重放到**磁盘现值**上，用户无感。
  //    `baseVersion=diskVersion` 是最后一道 TOCTOU 闸：从上面那次 `config.get()` 到这次写盘之间
  //    若又有人写了盘，后端会判 conflict 并一个字节都不写。
  let outcome;
  let observedAtDispatch: UserConfig | null;
  try {
    // pending marker 是草稿正文的 write-ahead 保险：先确保“有未保存草稿”已落到后端，再写 config。
    // 后端不在提交点盲清这个 bit（它不知道在途期间是否又有下一批）；成功回包后由
    // `persist(outcome.config, remaining)` 按实际剩余条目收账。回包丢失只会保守地多拦一次，不会误启动。
    await flushStagedPendingSync();
    if (!isCurrentDraft()) {
      set({ saveStatus: 'idle' });
      return false;
    }
    observedAtDispatch = get().latestSavedConfig;
    outcome = await api.config.save(merged, true, diskVersion, autoApply);
  } catch (err) {
    console.error('[staged-config] save failed:', err);
    finishFailedSave();
    return false;
  }
  if (outcome.status !== 'saved') {
    // 后端判冲突 = 上面那两句之间盘又被改了。**不自动重试**：重试一次就得防重试两次，
    // 而「点[重试保存]」本身就会把整条腿（含冲突检出）从头跑一遍，那是更诚实也更可控的出口。
    finishFailedSave();
    return false;
  }
  completeSave(set, get, disk, merged, entries, observedAtDispatch, outcome, true);
  return true;
}

function changedOnDisk(
  baseline: UserConfig, disk: UserConfig, merged: UserConfig, entries: StagedEntry[]
): StagedEntry[] {
  return conflictingEntries(baseline, disk, entries).filter(({ entityPath }) =>
    entitySnapshot(disk, entityPath) !== entitySnapshot(merged, entityPath));
}

function isScalarAssignment({ entityPath, nextValue }: StagedEntry): boolean {
  // A later explicit value may follow our own queued write. Structured edits can
  // carry unedited stale fields copied from a form and still need the conflict check.
  return entityPath.length === 1 && (nextValue === null || typeof nextValue !== 'object');
}

function dependsOnDraft(disk: UserConfig, drafts: StagedEntry[], entries: StagedEntry[]): boolean {
  return drafts.some(({ entityPath }) => {
    const [field, id] = entityPath;
    const newEntity = id !== undefined && isAbsentSnapshot(entitySnapshot(disk, entityPath));
    return entries.some(({ entityPath: submitted, nextValue, selectedServerFallback }) =>
      (submitted[0] === field && (id === undefined || submitted[1] === undefined || submitted[1] === id))
      || (newEntity && (selectedServerFallback === id || refersTo(nextValue, id))));
  });
}

function refersTo(value: unknown, id: string): boolean {
  if (value === id) return true;
  if (value === null || typeof value !== 'object') return false;
  return Object.values(value).some((part) => refersTo(part, id));
}

function completeSave(
  set: SetState,
  get: GetState,
  disk: UserConfig,
  merged: UserConfig,
  entries: StagedEntry[],
  observedAtDispatch: UserConfig | null,
  outcome: Extract<SaveOutcome, { status: 'saved' }>,
  manualSave: boolean
): void {
  // 成功 ⇒ 本次捕获且期间未变化的意图已经**是**磁盘现值，留着只会让计数虚高。
  // 保存 IPC 在途期间仍可继续编辑；新 id 或同 id 新版本不属于本次提交，绝不能被旧回包清掉。
  const current = get();
  // The native Save can finish its hot leg after another writer has published a new disk snapshot.
  const observedDuringSave = current.latestSavedConfig !== observedAtDispatch;
  const latestSaved = observedDuringSave ? current.latestSavedConfig ?? outcome.config : outcome.config;
  const committedEntries = observedDuringSave
    ? entries.filter((entry) => current.entries.includes(entry)
      || entitySnapshot(latestSaved, entry.entityPath) === entitySnapshot(outcome.config, entry.entityPath))
    : entries;
  const remaining = manualSave ? remainingEntriesAfterCommit(committedEntries, current.entries) : current.entries;
  // New/revised intents have not approved changes found on disk by this save.
  // Keep their original comparison base; the next save rechecks them conservatively.
  const remainingBaseline = remaining.length > 0 ? current.baseline : latestSaved;
  const saveRevision = current.completedSaveRevision + 1;
  const changes = entries.flatMap(({ entityPath, label }) => {
    const before = entitySnapshot(disk, entityPath);
    const changed = before !== entitySnapshot(merged, entityPath)
      && before !== entitySnapshot(outcome.config, entityPath);
    if (!changed || !label.trim()) return [];
    const level = entityPath.length === 1 && entityPath[0] === 'logLevel' ? outcome.config.logLevel : undefined;
    return [{ label: label.trim(), ...(typeof level === 'string' ? { logLevel: level.toUpperCase() } : {}) }];
  });
  const savedRuntimeApply = outcome.runtimeApply === undefined ? undefined : {
    ...outcome.runtimeApply,
    saveRevision,
    configVersion: outcome.version,
    changes,
  };
  set({
    entries: remaining,
    ...(manualSave ? { saveStatus: 'idle', conflict: null, conflictBaseline: null } : {}),
    completedSaveRevision: saveRevision,
    ...(savedRuntimeApply === undefined ? {} : { savedRuntimeApply }),
    baseVersion: observedDuringSave ? current.baseVersion : outcome.version,
    latestSavedConfig: latestSaved,
    baseline: remainingBaseline,
  });
  if (manualSave) persist(remainingBaseline, remaining);
  else if (savedRuntimeApply === undefined && (!observedDuringSave || current.baseVersion === outcome.version)) {
    const notice = savedEditNotice(changes, i18n.t);
    if (notice) toast.success(notice);
  }
}

/**
 * Q1-b ④「主进程退出前清 staged」的渲染端半边：**首次恢复的异步前置**。
 *
 * 问后端「上次进程是不是正常退出的」（读即清），为真就把持久化的暂存清掉 —— 用户已经对两天前那半截
 * 编辑没有记忆了，留着它只会参与下一次保存的整份合成（Q1-b 管这叫埋雷）。为假（强杀 / 崩溃，
 * 或进程压根没退：自愈重载、C16 轻量模式销毁重建）则什么都不做，照常恢复。
 *
 * **一个 webview 只问一次**（memo 住整条 promise）：标记是读即清的一次性资源，问第二次恒得假，
 * 而并发的两次 hydrate 各问一次会让「谁先谁后」决定清不清 —— 那是竞态而不是判据。
 *
 * IPC 失败（后端没这条命令 / 调用抛）⇒ **当作「不是正常退出」= 恢复**。方向与 Q1-b 原文一致：
 * 宁可多恢复一次让用户看见「N 项待保存」，也不静默吞掉。
 */
let cleanExitGate: Promise<void> | null = null;

function consumeCleanExitFlag(): Promise<void> {
  cleanExitGate ??= (async () => {
    try {
      // `persist(null, [])` 走的就是 `removeItem` 腿（baseline 为 null ⇒ 删而非写空值）。
      if (await api.window.takeCleanExitFlag()) persist(null, []);
    } catch {
      /* 见上：失败即保守恢复 */
    }
  })();
  return cleanExitGate;
}

/**
 * 供 `app-store.loadConfig` 单点调用的薄封装 —— 总开关关时**连 store 都不碰**。
 *
 * 放在这里而不是让 app-store 直接 `getState().hydrate(...)`：调用点只该知道「有份新 config 到了」，
 * 不该知道暂存层的开关与恢复语义（现在还多了一条「恢复前要先问后端上次怎么退的」）。
 */
export function hydrateStagedConfig(config: UserConfig, isCurrent: () => boolean = () => true): Promise<void> {
  // 判据取 **store 的 `enabled`** 而非编译期常量：`stage`/`revert`/`reset`/`editRoute` 判的都是前者，
  // 两处不同源会造出一个「半开」态 —— 谁在运行期把 `enabled` 置 true（文档明说该字段就是为了让单测
  // 两侧都跑到），hydrate 却因常量为 false 永不执行 ⇒ `baseVersion` 恒 null ⇒ `persist()` 走
  // `removeItem` 腿 ⇒ 暂存能用但**永不持久化**，还会顺手删掉已有存储项。
  // store 初值本就取自该常量，故生产行为逐字节不变。
  const store = useStagedConfigStore.getState();
  if (!store.enabled) return Promise.resolve();
  // 已 hydrate 过 ⇒ 同步快路径。这条腿只跟盘刷 `baseVersion`/`baseline`，**不读存储**，
  // 与「上次怎么退的」无关；配置变更事件走的全是这条，不该为它多绕一个微任务。
  if (store.hydrated) {
    if (isCurrent()) store.hydrate(config);
    return Promise.resolve();
  }
  // 首次恢复**必须**排在标记消费之后：反过来的话条目已经进内存，事后再清存储也追不回来。
  return consumeCleanExitFlag().then(() => {
    if (isCurrent()) useStagedConfigStore.getState().hydrate(config);
  });
}

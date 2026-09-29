/**
 * 设置屏的**写失败回显**（裁定 #14）。
 *
 * # 为什么屏内必须自己接
 *
 * `useConfig().update` 失败时做两件事：把乐观值**回滚**，然后 `toast.error`。立项时移动入口没有
 * toast 宿主，门面落 console ⇒ 真机上的表现是「开关自己弹回原位，一句话都没有」。
 * 用户会把它读成「我没点中」，再拨一次，再弹回去。
 *
 * ⚠️ **宿主 2026-09-06 已经有了**（`mobile/MobileToaster.tsx`），但回显照旧落在**行内**，
 * 理由从「没有宿主」换成产品口径：设置屏一屏十几行开关，一条飘过去的 toast 说不清是哪一行失败了，
 * 而错误就贴在那颗控件下面一行时，用户的视线本来就在那。两者不互斥 —— 本屏要的是前者。
 *
 * # 三条设计约束，每条都对应一个具体的失效形态
 *
 * 1. **`commit` 收的是「已经在飞的那个 promise」，不是 patch。** 若改成 `commit(id, patch)` 由本模块
 *    内部去调 `update(patch, …)`，各页里就不再有 `update({ … })` 这个字面形态 ——
 *    `lib/config-write-wiring.test.ts` 的 W-a 判据正是按它扫的，那样等于**给写腿开一条绕过登记表的新路**。
 *    现在的形态下每一页仍然逐字写着 `update({ … })`，那道守卫照常看得见。
 * 2. **错误挂在行 id 上，由 `SettingsRow` 经 context 自动渲染**，不靠各页逐行传。逐行传是「忘一处就静默
 *    一处」的形态，而这正是本文件要消掉的那个类别。
 * 3. **`createCommit` 是纯工厂**（吃一个 `setErrors` 就能跑），故判据能直接拿一个必然 reject 的 promise
 *    驱动它、断言错误真的落进了表 —— 「有 `catch` 但 `catch` 体是空的」会被这条当场抓住。
 *    只钉「有没有 catch」是钉不住那个形态的。
 */

import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { TFunction } from 'i18next';

/** 行 id → 该行当前的写失败提示。 */
export type WriteErrors = Readonly<Record<string, string>>;

/** 记一条（同一行重复失败覆盖同一条，不堆积）。 */
export function withWriteError(prev: WriteErrors, rowId: string, message: string): WriteErrors {
  if (prev[rowId] === message) return prev;
  return { ...prev, [rowId]: message };
}

/** 清一条（该行写成功了）。引用不变时原样返回，避免无谓重渲染。 */
export function withoutWriteError(prev: WriteErrors, rowId: string): WriteErrors {
  if (prev[rowId] === undefined) return prev;
  const next = { ...prev };
  delete next[rowId];
  return next;
}

/**
 * 失败原因取文（通用形态）。
 *
 * 只取**第一行**并截断：后端经 `ipc-client` 抛上来的 message 通常是一句人话，但也可能带多行细节
 * 或调用栈，那些是开发者信息，不该出现在用户界面上（元规则：调试信息不外露）。
 * 取不到人话就只说那句无原因的 —— **宁可少说，不许编一个原因**。
 *
 * 两个键成对传入而不是写死「保存失败」：本屏的写腿不都是「保存」。关于页那四条外链走的是
 * `systemApi.openExternal`，一个字节的配置都不写，套「保存失败」的壳会让人去查配置。
 * 取文的**纪律**（只取首行 / 截断 / 不编原因）在这一处，两组键共用它。
 */
export function failureText(
  t: TFunction,
  err: unknown,
  keys: { readonly withReason: string; readonly plain: string },
): string {
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
  const reason = raw.split('\n')[0].trim().slice(0, 120);
  return reason.length > 0 ? t(keys.withReason, { reason }) : t(keys.plain);
}

/** 保存失败的取文（`commit` 的缺省口径）。 */
export function writeFailureText(t: TFunction, err: unknown): string {
  return failureText(t, err, {
    withReason: 'mobileSettings.saveFailedWithReason',
    plain: 'common.saveFailed',
  });
}

/**
 * `commit(rowId, promise)`：promise 落定后把该行的错误记上或清掉。
 *
 * `toText` 是**可选**的取文覆盖（缺省 = [`writeFailureText`]）。加它而不是让非保存类的写腿另起一套
 * 机制：辖区判据（`write-failure-visibility.test.ts` ②）认的是「写腿落在 `commit(` 的实参区间内」，
 * 另起一套就等于给那条写腿开一条绕过登记表的路，而这里要改的只是那句话的措辞。
 */
export type CommitWrite = (
  rowId: string,
  write: Promise<void>,
  toText?: (err: unknown) => string,
) => void;

/**
 * 纯工厂：只吃一个 `setErrors`（React 的 setState 或判据里的一个普通函数）。
 * 抽出来是为了**可测**——本仓 vitest 跑 node、无 jsdom，真 hook 一行都跑不起来，
 * 而这里要守的恰恰是「失败时到底有没有东西被记下来」。
 */
export function createCommit(
  setErrors: (updater: (prev: WriteErrors) => WriteErrors) => void,
  t: TFunction,
): CommitWrite {
  return (rowId, write, toText) => {
    void write.then(
      () => setErrors((prev) => withoutWriteError(prev, rowId)),
      (err: unknown) =>
        setErrors((prev) =>
          withWriteError(prev, rowId, (toText ?? ((e) => writeFailureText(t, e)))(err)),
        ),
    );
  };
}

/* ── 弹窗宿主缺席那一条腿（裁定 #14 登记在案的第二条）────────────────────────── */

/** 本屏用来挂「弹窗本来会说、但移动端没有宿主能说」的那一行的固定行 id。 */
export const DEFERRED_NOTICE_ROW = 'deferred-dialog-notice';

/** [`deferredNoticeOf`] 只需要 `dialog-store` 栈项的这一小块形状（不吃整个 union）。 */
export interface DialogLike {
  readonly kind: string;
  readonly instanceId: string;
  readonly payload?: { readonly title?: string; readonly message?: string };
}

/** 一条被截下来、要就地显示的弹窗内容。 */
export interface DeferredNotice {
  readonly instanceId: string;
  /** 弹窗自己的标题 —— 直接用它，不另起一句文案（另起一句就是同一条消息的第二份真值）。 */
  readonly label: string;
  /** 正文。段间空行折成空格：行内那一格是一行文本，`\n\n` 在那里会被折叠成什么都看不见。 */
  readonly text: string;
}

/**
 * 把 `dialog-store` 栈里**移动端没有宿主可渲染**的那条 confirm 折成一条行内提示。
 *
 * # 这条腿修的是什么
 *
 * `use-config.ts` 的 `promptAppRestart` 在「已落盘、但要重启 App 才生效」时 `open({kind:'confirm'})`，
 * 而 `DialogHost` 只挂在桌面 `AppShell` 上 ⇒ 移动端**那个弹窗永不出现**：用户改完设置，界面上什么
 * 都没说，改动看起来生效了其实没有。裁定 #14 点名的三条无锚点失败之一。
 *
 * # 为什么是纯函数
 *
 * 与 [`createCommit`] 同一个理由：本仓 vitest 跑 node、无 jsdom，真 hook 一行都跑不起来，而这里要守的
 * 恰恰是「截到了没有、截下来的话对不对」。纯函数才驱动得动（`write-failure-visibility.test.ts` ③）。
 *
 * # 只认 `confirm`，且只取最底下那一条
 *
 * 只有 `confirm` 的载荷是**自解释的两段文本**（title + message），别的 kind 是要渲染一整个表单的组件，
 * 折成一行文本只会得到一句没头没尾的话 —— 那些属于别的批次的射程，本函数**不假装**能显示它们。
 * 取栈底而不是栈顶：先来的先答，后来的下一轮再取（调用点截一条关一条）。
 */
export function deferredNoticeOf(stack: readonly DialogLike[]): DeferredNotice | null {
  const entry = stack.find((e) => e.kind === 'confirm');
  if (entry === undefined) return null;
  const label = entry.payload?.title ?? '';
  const text = (entry.payload?.message ?? '').split('\n').map((x) => x.trim()).filter(Boolean).join(' ');
  return { instanceId: entry.instanceId, label, text };
}

/**
 * 行内错误的分发通道。
 *
 * 缺省是空表：屏之外（如判据单独渲染某一页）不提供 Provider 时，行为退化成「没有错误」而不是崩，
 * 与「空槽零 DOM」同一口径。
 */
export const WriteErrorsContext = createContext<WriteErrors>({});

/** 某一行当前的写失败提示（`SettingsRow` 内部消费，各页不必逐行传）。 */
export function useWriteError(rowId: string): string | undefined {
  return useContext(WriteErrorsContext)[rowId];
}

export interface WriteFeedback {
  errors: WriteErrors;
  commit: CommitWrite;
  /**
   * 直接往表里记一条（没有 promise 可等的那些失败）。
   *
   * 唯一的调用点是被截下来的弹窗（[`deferredNoticeOf`]）：那条消息不是某个 promise 的拒绝腿，
   * 而是别处已经落定的一件事。走同一张表 ⇒ 同一个红色行内格子渲染它，**不新增第二种回显形态**。
   */
  report: (rowId: string, text: string) => void;
}

/** 屏级接线：持错误表 + 造 `commit`。 */
export function useWriteFeedback(t: TFunction): WriteFeedback {
  const [errors, setErrors] = useState<WriteErrors>({});
  const commit = useMemo(() => createCommit(setErrors, t), [t]);
  const report = useCallback(
    (rowId: string, text: string) => setErrors((prev) => withWriteError(prev, rowId, text)),
    [],
  );
  // `errors` / `commit` / `report` 一起进 context / props，稳定引用交给消费方。
  return useMemo(() => ({ errors, commit, report }), [errors, commit, report]);
}

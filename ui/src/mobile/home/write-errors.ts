/**
 * 首页的**写失败回显**（IA 裁定 #14）。
 *
 * # 为什么是行内、为什么必须屏内自解
 *
 * 立项时的根因：移动外壳既没挂 `DialogHost`，也没有 toast 宿主 —— `lib/error-handler` 的 toast
 * 门面在未注入实现时静默落 console。于是一次写失败的表现是**「控件自己弹回原位，一句话都没有」**
 * —— 正是 `contracts/action-failure-visibility.test.ts` 那道门守的形态（W10/W14 真机首曝过）。
 *
 * ⚠️ **那半条根因 2026-09-06 已经不成立**：移动端有自己的 toast 宿主了
 * （`mobile/MobileToaster.tsx`，由 `MobileApp` 挂在停靠区）。**但本模块照旧**，理由变成产品口径而
 * 不是能力缺席：行内回显**贴着那颗控件**，而 toast 是全局瞬态的。首页有六颗会写的控件，
 * 一条飘过去的 toast 说不清是哪一颗失败了，而用户下一步要动的恰恰是那一颗。
 * 两者不互斥 —— 本模块要的是前者。
 *
 * # 三条设计约束，每条对应一个具体失效形态
 *
 * 1. **错误按控件 id 存**，不是一条全局 notice ⇒ 呈现层能把它渲染在那颗控件下面一行。
 * 2. **`createRunWrite` 是纯工厂**（吃一个 `setErrors` 就能跑）⇒ 门可以直接拿一个**必然 reject**
 *    的 op 驱动它，断言错误真的落进了表。「有 `catch` 但 `catch` 体什么都不做」会被这条当场抓住 ——
 *    只钉「有没有 catch」是钉不住那个形态的（裁定 #14 原文点名）。
 * 3. **写成功要清掉上一次的错误**，否则一条修好的失败会永久挂在控件下面，读作「它还是坏的」。
 */
import type { TFunction } from 'i18next';
import type { WriteControlId, WriteErrors } from './view-model';

/** 记一条（同一控件重复失败覆盖同一条，不堆积）。 */
export function withWriteError(
  prev: WriteErrors,
  id: WriteControlId,
  message: string,
): WriteErrors {
  if (prev[id] === message) return prev;
  return { ...prev, [id]: message };
}

/** 清一条（该控件这次写成功了）。引用不变时原样返回，避免无谓重渲染。 */
export function withoutWriteError(prev: WriteErrors, id: WriteControlId): WriteErrors {
  if (prev[id] === undefined) return prev;
  const next = { ...prev };
  delete next[id];
  return next;
}

/**
 * 失败原因取文。
 *
 * 只取**第一行**并截断：后端经 `ipc-client` 抛上来的 message 通常是一句人话，但也可能带多行细节
 * 或调用栈 —— 那些是开发者信息，不该出现在用户界面上。取不到人话就只说「操作失败」：
 * **宁可少说，不许编一个原因**。
 */
export function writeFailureText(t: TFunction, err: unknown): string {
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
  const reason = raw.split('\n')[0].trim().slice(0, 120);
  return reason.length > 0
    ? t('mobileHome.actionFailedWithReason', { reason })
    : t('mobileHome.actionFailed');
}

/** 唯一写出口。接线层的**每一处**写调用都必须在它的参数区间内，门逐个对拍。 */
export type RunWrite = (id: WriteControlId, op: () => Promise<unknown>) => Promise<void>;

/**
 * 纯工厂。抽出来是为了**可测** —— 本仓 vitest 跑 node、无 jsdom，真 hook 一行都跑不起来，
 * 而这里要守的恰恰是「失败时到底有没有东西被记下来」。
 */
export function createRunWrite(
  setErrors: (updater: (prev: WriteErrors) => WriteErrors) => void,
  t: TFunction,
): RunWrite {
  return async (id, op) => {
    try {
      await op();
      setErrors((prev) => withoutWriteError(prev, id));
    } catch (err) {
      setErrors((prev) => withWriteError(prev, id, writeFailureText(t, err)));
    }
  };
}

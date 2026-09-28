/**
 * `refreshSubscriptionWithToast` 的**行为**门 —— 一个函数、两个消费端、两条不许互相踩的路。
 *
 * # 为什么这道门必须存在
 *
 * 2026-09-05 把它的返回值从 `boolean` 拓宽成判别联合，理由是移动端拿不到失败详情
 * （toast 的实现只在桌面注入 —— `components/layout/Toaster.tsx` 是全仓唯一的 `setToastImpl`
 * 调用点 ⇒ 移动外壳里 `toast.error(...)` 就是 `console.error(...)`，详情当场蒸发）。
 * 这是一次**共用文件**的改动，而本仓有过教训：改 shared 谓词没跑对面那侧的门。
 * 于是这里把两侧同时钉住：
 *
 *  · **桌面侧不变** —— 三态各自走哪个 toast 通道、正文与副标题各是哪个 i18n key 与哪些实参，
 *    逐条断言。桌面调用方（`use-node-subscription-actions.ts`）连返回值都不读，它的可观测行为
 *    **全部**在这几条 toast 上；钉住它们就等于钉住「桌面那条路没变」。
 *  · **移动侧拿得到详情** —— 失败时返回的 `detail` 必须与 toast 副标题**同一个值**。
 *    这条是本次修复的本体：不是「有一个字段」，是「那个字段里装的就是用户本该看到的那句话」。
 *
 * # ⚠️ 这道门只管到「交出来」，管不到「有没有用」
 *
 * 本文件全绿**不代表**用户看得到详情：消费侧完全可以写成
 * `if (!r.ok) setNotice({ tone: 'err', text: t('nodes.subRefreshFail') })` —— `r.detail` 拿到手
 * 再丢掉，`tsc` 过、本门 11 条照样全绿，而用户读到的仍是「刷新失败」四个字。
 * 那一半由 **`mobile/nodes/nodes-screen.test.tsx` ⑪** 守（沿 AST 判 `setNotice.text` 那一格
 * 装的是 `r.detail` 还是 `t(…)` 兜底）。两道门必须成对存在 ——
 * **凡是「把信息从 A 交到 B」的修复，A 侧的行为门与 B 侧的接线判据缺一不可。**
 *
 * # 判据形态：行为对照，不是源码断言
 *
 * 「详情有没有丢」这件事没法用 grep 判 —— 旧版源码里 `subscriptionErrorDetail(...)` 那一行
 * 一直在，它只是没被交出去。故这里真的驱动函数：假 IPC 给出四种后端返回，假 toast 记录每一次
 * 调用，然后对着记录与返回值同时断言。
 *
 * # 回放：修复前的形态必须被本门判红
 *
 * 最后一组用例把「只返回布尔、详情只进 toast」那个形态在算式里重建一遍，断言本门的判据
 * 当场判它有罪 —— 一道加进来却永远不会红的门比没有更坏。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { setToastImpl, type ToastOptions } from '@/lib/error-handler';
import { SUBSCRIPTION_ERROR_I18N_KEY } from '@/contracts/subscription-preview';

/** 后端 `updateServers` 的下一次返回（或抛出）。逐条用例自己摆。 */
let nextResult: unknown = null;
let nextThrow: unknown = null;
const updateServers = vi.fn(async (_subscriptionId: string) => {
  if (nextThrow !== null) throw nextThrow;
  return nextResult as never;
});

vi.mock('@/ipc', () => ({
  api: {
    subscription: {
      updateServers: (id: string) => updateServers(id),
    },
  },
}));

import { refreshSubscriptionWithToast } from './subscription-refresh';

// ── 假 toast 宿主：记录每一次调用（通道 + 实参）─────────────────────────────────
interface ToastCall {
  channel: 'success' | 'info' | 'warning' | 'error';
  msg: string;
  desc?: string;
}
let calls: ToastCall[] = [];
setToastImpl({
  success: (msg: string) => void calls.push({ channel: 'success', msg }),
  info: (msg: string) => void calls.push({ channel: 'info', msg }),
  warning: (msg: string) => void calls.push({ channel: 'warning', msg }),
  error: (msg: string, desc?: string, _o?: ToastOptions) =>
    void calls.push({ channel: 'error', msg, desc }),
});

/**
 * 假 `t`：把 key 与实参原样拼回去。
 *
 * **不查真语料**是刻意的：本门判的是「哪个 key、带哪些实参」，不是「那句话翻得对不对」
 * （后者归 i18n 覆盖门）。拼回 key 让断言直接读作契约，语料改词也不会让这道门变色。
 */
const t = ((key: string, vars?: Record<string, unknown>) =>
  vars === undefined ? key : `${key}(${JSON.stringify(vars)})`) as never;

beforeEach(() => {
  calls = [];
  nextResult = null;
  nextThrow = null;
  updateServers.mockClear();
});

describe('① 桌面侧三态 toast —— 逐条钉住（这几条变了 = 桌面行为变了）', () => {
  it('自检：函数真的调了后端，假 toast 真的被驱动过（扫 0 次会让下面全部恒绿）', async () => {
    nextResult = { success: true, addedServers: 1, updatedServers: 0, deletedServers: 0 };
    await refreshSubscriptionWithToast('sub-1', t);
    expect(updateServers).toHaveBeenCalledTimes(1);
    expect(updateServers).toHaveBeenCalledWith('sub-1');
    expect(calls.length, '一次 toast 都没记到 —— 假宿主没接上，本文件全部断言失去意义').toBe(1);
  });

  it('有变化 ⇒ `toast.success(nodes.subRefreshOk, {count: 增+改+删})`', async () => {
    nextResult = { success: true, addedServers: 2, updatedServers: 3, deletedServers: 4 };
    const r = await refreshSubscriptionWithToast('sub-1', t);
    expect(calls).toEqual([
      { channel: 'success', msg: 'nodes.subRefreshOk({"count":9})' },
    ]);
    expect(r).toEqual({ ok: true });
  });

  it('无变化（304/内容等价）⇒ `toast.info(nodes.subRefreshUnchanged)`，不是 success', async () => {
    // 计数恒 0/0/0：走 success 档会把「什么都没变」报成「已更新 0 个」。
    nextResult = { success: true, unchanged: true, addedServers: 0, updatedServers: 0, deletedServers: 0 };
    const r = await refreshSubscriptionWithToast('sub-1', t);
    expect(calls).toEqual([{ channel: 'info', msg: 'nodes.subRefreshUnchanged' }]);
    expect(r).toEqual({ ok: true });
  });

  it('业务失败 ⇒ `toast.error(nodes.subRefreshFail, 分类详情)`，副标题按 errorKind 本地化', async () => {
    nextResult = {
      success: false,
      addedServers: 0,
      updatedServers: 0,
      deletedServers: 0,
      errorKind: 'http',
      httpStatus: 503,
      error: 'GET https://… -> 503', // 诊断，**不得**出现在用户界面
    };
    const r = await refreshSubscriptionWithToast('sub-1', t);
    const detail = `${SUBSCRIPTION_ERROR_I18N_KEY.http.detail}({"status":503})`;
    expect(calls).toEqual([{ channel: 'error', msg: 'nodes.subRefreshFail', desc: detail }]);
    expect(r).toEqual({ ok: false, detail });
    // 反向：后端诊断串一个字都不许进用户可见的两个位置。
    expect(JSON.stringify(calls) + JSON.stringify(r)).not.toContain('GET https://');
  });

  it('IPC reject ⇒ `toast.error` 两段都回落 `nodes.subRefreshFail`（不把 transport 文案带到 UI）', async () => {
    nextThrow = new Error('ipc channel closed: subscription_update_servers');
    const r = await refreshSubscriptionWithToast('sub-1', t);
    expect(calls).toEqual([
      { channel: 'error', msg: 'nodes.subRefreshFail', desc: 'nodes.subRefreshFail' },
    ]);
    expect(r).toEqual({ ok: false, detail: 'nodes.subRefreshFail' });
    expect(JSON.stringify(calls) + JSON.stringify(r)).not.toContain('ipc channel closed');
  });

  it('反向对照：`toast.warning` 这条通道本函数一次都不用（证明上面几条不是"什么都算过"）', async () => {
    for (const fixture of [
      { success: true, addedServers: 1, updatedServers: 0, deletedServers: 0 },
      { success: true, unchanged: true, addedServers: 0, updatedServers: 0, deletedServers: 0 },
      { success: false, addedServers: 0, updatedServers: 0, deletedServers: 0, errorKind: 'dns' },
    ]) {
      calls = [];
      nextResult = fixture;
      await refreshSubscriptionWithToast('sub-1', t);
      expect(calls.map((c) => c.channel)).not.toContain('warning');
      expect(calls.length, '这一档一条 toast 都没发 —— 桌面用户什么都看不到').toBe(1);
    }
  });
});

describe('② 移动侧：失败详情必须随返回值一起出来（本次修复的本体）', () => {
  /**
   * 全部 14 个 `SubscriptionErrorKind` 逐个过一遍。
   *
   * 只测 `http` 一档会漏掉「某个 kind 忘了进 `SUBSCRIPTION_ERROR_I18N_KEY`」那一类 ——
   * 那种情况下 `subscriptionErrorDetail` 会静默回落到笼统的 `nodes.subRefreshFail`，
   * 移动端于是又回到「刷新失败四个字」，而门若只测一档是看不见的。
   */
  const KINDS = Object.keys(SUBSCRIPTION_ERROR_I18N_KEY) as Array<
    keyof typeof SUBSCRIPTION_ERROR_I18N_KEY
  >;

  it(`自检：错误分类表非空且规模没缩水（今天 ${KINDS.length} 档）`, () => {
    expect(KINDS.length).toBeGreaterThanOrEqual(14);
  });

  it('每个 errorKind 的详情都 ≠ 笼统文案，且与 toast 副标题逐字同值', async () => {
    const generic: string[] = [];
    for (const kind of KINDS) {
      calls = [];
      nextResult = {
        success: false,
        addedServers: 0,
        updatedServers: 0,
        deletedServers: 0,
        errorKind: kind,
        httpStatus: 404,
      };
      const r = await refreshSubscriptionWithToast('sub-1', t);
      expect(r.ok).toBe(false);
      const detail = r.ok ? '' : r.detail;
      // 正面断言：详情是这个 kind 自己的那条 key，不是回落。
      expect(detail, `${kind} 的详情没走分类表`).toContain(SUBSCRIPTION_ERROR_I18N_KEY[kind].detail);
      // 同源断言：桌面看到的与移动端拿到的是**同一个字符串**，不是两条各自拼的。
      expect(calls[0]?.desc, `${kind}: toast 副标题与返回详情不同源`).toBe(detail);
      if (detail === 'nodes.subRefreshFail') generic.push(kind);
    }
    expect(generic, '这些 kind 回落到了笼统文案 —— 移动端用户又只能读到「刷新失败」').toEqual([]);
  });

  it('无分类的旧载荷仍有兜底（`detail` 恒是字符串，消费方不需要第二层 `??`）', async () => {
    nextResult = { success: false, addedServers: 0, updatedServers: 0, deletedServers: 0 };
    const r = await refreshSubscriptionWithToast('sub-1', t);
    expect(r.ok).toBe(false);
    expect(r.ok ? undefined : r.detail).toBe('nodes.subRefreshFail');
  });
});

describe('③ 回放修复前的形态 —— 判据必须当场判它有罪', () => {
  /**
   * 修复前：`Promise<boolean>`，详情算出来只喂给 toast。移动端唯一拿得到的是那个布尔。
   * 这里不改真实现，只把那个形态在算式里重建一遍，然后用本文件 ② 的两条判据去打它。
   */
  const legacyShape = (detailForToast: string) => ({
    returned: false as boolean, // 旧返回值：只有成功与否
    toastDesc: detailForToast, // 详情：只进了 toast
  });

  it('旧形态下「移动端拿到的东西」里根本没有详情这一格', () => {
    const legacy = legacyShape('sub.preview.httpDetail({"status":503})');
    // 判据②的第一条：消费方要从返回值里读出详情 —— 布尔身上没有这个字段。
    expect(typeof legacy.returned).toBe('boolean');
    expect(Object.prototype.hasOwnProperty.call(legacy, 'detail')).toBe(false);
    // 判据②的第二条：同源。旧形态下压根无从比对（详情只存在于 toast 那一侧）。
    expect(legacy.toastDesc).not.toBe(String(legacy.returned));
  });

  it('今天的形态过得去同两条判据（证明上一条红的是旧形态，不是判据本身写错了）', async () => {
    nextResult = {
      success: false,
      addedServers: 0,
      updatedServers: 0,
      deletedServers: 0,
      errorKind: 'http',
      httpStatus: 503,
    };
    const r = await refreshSubscriptionWithToast('sub-1', t);
    expect(Object.prototype.hasOwnProperty.call(r, 'detail')).toBe(true);
    expect(r.ok ? undefined : r.detail).toBe(calls[0]?.desc);
  });
});

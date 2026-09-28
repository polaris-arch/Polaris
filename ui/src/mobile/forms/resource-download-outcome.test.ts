/**
 * `downloadOutcome` 的单测 —— 「一次资源下载回来之后，该说什么」。
 *
 * # 为什么这三档必须分开
 *
 * 空数组 = **后端没受理**这次请求；逐条 `ok:false` = 受理了但这几条失败；全成功才关表。
 * 并成一句「失败了」，用户分不出该换一个 URL 再试、还是该等一等再点 —— 而这两件事的
 * 下一步动作完全不同。桌面那两个弹窗也是三档分开的，本函数是它们在移动端的对位。
 *
 * # 为什么它不发起调用
 *
 * 发起那一行**刻意**留在两个面板各自的 `try` 里（见 `ResourceAddPanel.tsx` 的头注）：
 * 跨屏门按「写腿落在一个 try 里、且那个 try 的 catch 真的调了本层 reporter」判，
 * 抽进模块级函数它就落在任何 catch 的辖区之外。判据只有一份的是**结果怎么读**，那正是这里。
 */
import { describe, expect, it } from 'vitest';
import { downloadOutcome } from './ResourceAddPanel';

/** 取词器替身：返回键本身 ⇒ 断言能直接对拍「用的是哪一条文案」，不受译文改动影响。 */
const t = (key: string): string => key;

describe('downloadOutcome：一次下载的三档结果', () => {
  it('全成功 ⇒ ok（没有文案，调用方据此关表）', () => {
    expect(downloadOutcome([{ ok: true }, { ok: true }], t)).toEqual({ ok: true });
  });

  it('任一条失败 ⇒ 用「下载失败」那句，**不关表**（用户可以少选几项重试）', () => {
    expect(downloadOutcome([{ ok: true }, { ok: false }], t)).toEqual({
      ok: false,
      text: 'resCatalog.downloadAllFailed',
    });
  });

  it('目录选中两项但只收到一项成功，仍不得关表报成功', () => {
    expect(downloadOutcome([{ ok: true }], t, 2)).toEqual({
      ok: false,
      text: 'resCatalog.downloadAllFailed',
    });
  });

  /**
   * 🔴 空数组走的是**中性**失败文案，不是桌面那条 `resCatalog.errUnavailable`
   * （「下载后端尚未接入」）—— 那是桌面自己的一条缺席声明，在移动端渲染它等于把桌面的债
   * 记到移动端账上（接线完成度门 A 面对这批键的收窄条件正是「移动端有没有消费点」）。
   */
  it('空数组 / 非数组 ⇒ 后端没受理，用中性失败文案（不是桌面那条缺席声明）', () => {
    for (const bad of [[], null, undefined, 'nope', { ok: true }]) {
      expect(downloadOutcome(bad, t), `${JSON.stringify(bad)} 这一档判错了`).toEqual({
        ok: false,
        text: 'errors.operationFailed',
      });
    }
  });

  it('反向对照：三档给出的是**三种**结果（否则上面几条可以同时被一个常量满足）', () => {
    const results = [
      JSON.stringify(downloadOutcome([{ ok: true }], t)),
      JSON.stringify(downloadOutcome([{ ok: false }], t)),
      JSON.stringify(downloadOutcome([], t)),
    ];
    expect(new Set(results).size).toBe(3);
  });

  /**
   * `ok` 缺席（后端回了一个形状对不上的信封）按**失败**读，不按成功读。
   * 反过来会把一次没落盘的下载当成功关掉表单，而用户以为资源已经在了。
   */
  it('信封里没有 `ok` 字段 ⇒ 按失败读（fail-closed）', () => {
    expect(downloadOutcome([{ resource: 'x' }], t)).toEqual({
      ok: false,
      text: 'resCatalog.downloadAllFailed',
    });
  });
});
